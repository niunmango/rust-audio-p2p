use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};

use crate::models::{CacheEntry, CacheStats, Playlist, Track, TrackSource};

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0)
}

pub struct Database {
    conn: Connection,
}

impl Database {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        let db = Self { conn };
        db.init_schema()?;
        Ok(db)
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let db = Self { conn };
        db.init_schema()?;
        Ok(db)
    }

    fn init_schema(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"
            PRAGMA journal_mode = WAL;
            PRAGMA foreign_keys = ON;

            CREATE TABLE IF NOT EXISTS tracks (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                source_key TEXT UNIQUE NOT NULL,
                source_type TEXT NOT NULL,
                source_json TEXT NOT NULL,
                title TEXT NOT NULL,
                artist TEXT NOT NULL,
                album TEXT NOT NULL,
                year INTEGER,
                track_number INTEGER,
                duration_secs INTEGER,
                file_size_bytes INTEGER NOT NULL DEFAULT 0,
                format TEXT NOT NULL,
                cover_path TEXT,
                pinned INTEGER NOT NULL DEFAULT 0,
                updated_at INTEGER NOT NULL
            );

            CREATE VIRTUAL TABLE IF NOT EXISTS tracks_fts USING fts5(
                title,
                artist,
                album,
                content='tracks',
                content_rowid='id'
            );

            CREATE TRIGGER IF NOT EXISTS tracks_ai AFTER INSERT ON tracks BEGIN
                INSERT INTO tracks_fts(rowid, title, artist, album)
                VALUES (new.id, new.title, new.artist, new.album);
            END;

            CREATE TRIGGER IF NOT EXISTS tracks_ad AFTER DELETE ON tracks BEGIN
                INSERT INTO tracks_fts(tracks_fts, rowid, title, artist, album)
                VALUES ('delete', old.id, old.title, old.artist, old.album);
            END;

            CREATE TRIGGER IF NOT EXISTS tracks_au AFTER UPDATE ON tracks BEGIN
                INSERT INTO tracks_fts(tracks_fts, rowid, title, artist, album)
                VALUES ('delete', old.id, old.title, old.artist, old.album);
                INSERT INTO tracks_fts(rowid, title, artist, album)
                VALUES (new.id, new.title, new.artist, new.album);
            END;

            CREATE TABLE IF NOT EXISTS playlists (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT UNIQUE NOT NULL,
                pinned INTEGER NOT NULL DEFAULT 0,
                created_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS playlist_tracks (
                playlist_id INTEGER NOT NULL REFERENCES playlists(id) ON DELETE CASCADE,
                track_id INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
                position INTEGER NOT NULL,
                PRIMARY KEY (playlist_id, track_id)
            );

            CREATE TABLE IF NOT EXISTS cache_entries (
                cache_key TEXT PRIMARY KEY,
                track_id INTEGER REFERENCES tracks(id) ON DELETE SET NULL,
                file_path TEXT NOT NULL,
                size_bytes INTEGER NOT NULL,
                pinned INTEGER NOT NULL DEFAULT 0,
                last_accessed_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            "#,
        )?;
        Ok(())
    }

    fn row_to_track(row: &rusqlite::Row<'_>) -> rusqlite::Result<Track> {
        let source_json: String = row.get(2)?;
        let source: TrackSource = serde_json::from_str(&source_json).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(2, rusqlite::types::Type::Text, Box::new(e))
        })?;
        Ok(Track {
            id: row.get(0)?,
            source,
            title: row.get(3)?,
            artist: row.get(4)?,
            album: row.get(5)?,
            year: row.get(6)?,
            track_number: row.get(7)?,
            duration_secs: row.get::<_, Option<i64>>(8)?.map(|d| d.max(0) as u64),
            file_size_bytes: row.get::<_, i64>(9)?.max(0) as u64,
            format: row.get(10)?,
            cover_path: row.get(11)?,
            pinned: row.get::<_, i64>(12)? != 0,
        })
    }

    pub fn upsert_track(&self, track: &Track) -> Result<i64> {
        let source_key = track.source.unique_key();
        let source_type = track.source.source_type();
        let source_json = serde_json::to_string(&track.source)?;
        let now = now_unix();

        self.conn.execute(
            r#"
            INSERT INTO tracks (
                source_key, source_type, source_json, title, artist, album,
                year, track_number, duration_secs, file_size_bytes, format,
                cover_path, pinned, updated_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
            ON CONFLICT(source_key) DO UPDATE SET
                source_type = excluded.source_type,
                source_json = excluded.source_json,
                title = excluded.title,
                artist = excluded.artist,
                album = excluded.album,
                year = excluded.year,
                track_number = excluded.track_number,
                duration_secs = excluded.duration_secs,
                file_size_bytes = excluded.file_size_bytes,
                format = excluded.format,
                cover_path = excluded.cover_path,
                updated_at = excluded.updated_at
            "#,
            params![
                source_key,
                source_type,
                source_json,
                track.title,
                track.artist,
                track.album,
                track.year,
                track.track_number,
                track.duration_secs.map(|d| d as i64),
                track.file_size_bytes as i64,
                track.format,
                track.cover_path,
                if track.pinned { 1 } else { 0 },
                now,
            ],
        )?;

        let id: i64 = self.conn.query_row(
            "SELECT id FROM tracks WHERE source_key = ?1",
            params![source_key],
            |row| row.get(0),
        )?;
        Ok(id)
    }

    pub fn count_tracks(&self) -> Result<usize> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM tracks", [], |row| row.get(0))?;
        Ok(count.max(0) as usize)
    }

    pub fn get_track(&self, id: i64) -> Result<Option<Track>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, source_key, source_json, title, artist, album, year, track_number, duration_secs, file_size_bytes, format, cover_path, pinned FROM tracks WHERE id = ?1",
        )?;
        let res = stmt.query_row(params![id], Self::row_to_track).optional()?;
        Ok(res)
    }

    pub fn list_tracks(&self, limit: usize, offset: usize) -> Result<Vec<Track>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT id, source_key, source_json, title, artist, album, year, track_number, duration_secs, file_size_bytes, format, cover_path, pinned
            FROM tracks
            ORDER BY artist COLLATE NOCASE ASC, album COLLATE NOCASE ASC, COALESCE(track_number, 0) ASC, title COLLATE NOCASE ASC
            LIMIT ?1 OFFSET ?2
            "#,
        )?;
        let rows = stmt.query_map(params![limit as i64, offset as i64], Self::row_to_track)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn search_tracks_fts(&self, query: &str, limit: usize) -> Result<Vec<Track>> {
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return self.list_tracks(limit, 0);
        }

        let tokens: Vec<String> = trimmed
            .split(|c: char| !c.is_alphanumeric())
            .filter(|s| !s.is_empty())
            .map(|s| format!("\"{}\"*", s.replace('"', "")))
            .collect();

        if !tokens.is_empty() {
            let fts_query = tokens.join(" AND ");
            let sql = r#"
                SELECT t.id, t.source_key, t.source_json, t.title, t.artist, t.album, t.year, t.track_number, t.duration_secs, t.file_size_bytes, t.format, t.cover_path, t.pinned
                FROM tracks_fts f
                JOIN tracks t ON t.id = f.rowid
                WHERE tracks_fts MATCH ?1
                ORDER BY bm25(tracks_fts), t.artist COLLATE NOCASE ASC, t.album COLLATE NOCASE ASC
                LIMIT ?2
            "#;
            if let Ok(mut stmt) = self.conn.prepare(sql) {
                if let Ok(rows) = stmt.query_map(params![fts_query, limit as i64], Self::row_to_track) {
                    let mut out = Vec::new();
                    for r in rows {
                        out.push(r?);
                    }
                    if !out.is_empty() {
                        return Ok(out);
                    }
                }
            }
        }

        let pattern = format!("%{}%", trimmed);
        let mut stmt = self.conn.prepare(
            r#"
            SELECT id, source_key, source_json, title, artist, album, year, track_number, duration_secs, file_size_bytes, format, cover_path, pinned
            FROM tracks
            WHERE title LIKE ?1 OR artist LIKE ?1 OR album LIKE ?1
            ORDER BY artist COLLATE NOCASE ASC, album COLLATE NOCASE ASC
            LIMIT ?2
            "#,
        )?;
        let rows = stmt.query_map(params![pattern, limit as i64], Self::row_to_track)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn create_playlist(&self, name: &str) -> Result<i64> {
        let clean = name.trim();
        self.conn.execute(
            "INSERT OR IGNORE INTO playlists (name, pinned, created_at) VALUES (?1, 0, ?2)",
            params![clean, now_unix()],
        )?;
        let id: i64 = self.conn.query_row(
            "SELECT id FROM playlists WHERE name = ?1",
            params![clean],
            |row| row.get(0),
        )?;
        Ok(id)
    }

    pub fn list_playlists(&self) -> Result<Vec<Playlist>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT p.id, p.name, p.pinned, COUNT(pt.track_id) as cnt
            FROM playlists p
            LEFT JOIN playlist_tracks pt ON pt.playlist_id = p.id
            GROUP BY p.id
            ORDER BY p.pinned DESC, p.name COLLATE NOCASE ASC
            "#,
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Playlist {
                id: row.get(0)?,
                name: row.get(1)?,
                pinned: row.get::<_, i64>(2)? != 0,
                track_count: row.get::<_, i64>(3)?.max(0) as usize,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn add_track_to_playlist(&self, playlist_id: i64, track_id: i64) -> Result<()> {
        let max_pos: i64 = self.conn.query_row(
            "SELECT COALESCE(MAX(position), 0) FROM playlist_tracks WHERE playlist_id = ?1",
            params![playlist_id],
            |row| row.get(0),
        )?;
        self.conn.execute(
            "INSERT OR IGNORE INTO playlist_tracks (playlist_id, track_id, position) VALUES (?1, ?2, ?3)",
            params![playlist_id, track_id, max_pos + 1],
        )?;
        Ok(())
    }

    pub fn remove_track_from_playlist(&self, playlist_id: i64, track_id: i64) -> Result<()> {
        self.conn.execute(
            "DELETE FROM playlist_tracks WHERE playlist_id = ?1 AND track_id = ?2",
            params![playlist_id, track_id],
        )?;
        Ok(())
    }

    pub fn get_local_tracks_index(&self) -> Result<std::collections::HashMap<String, (u64, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT source_key, file_size_bytes, updated_at FROM tracks WHERE source_type = 'local'",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                (row.get::<_, i64>(1)?.max(0) as u64, row.get::<_, i64>(2)?),
            ))
        })?;
        let mut map = std::collections::HashMap::new();
        for r in rows {
            let (k, v) = r?;
            map.insert(k, v);
        }
        Ok(map)
    }

    pub fn get_playlist_tracks(&self, playlist_id: i64) -> Result<Vec<Track>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT t.id, t.source_key, t.source_json, t.title, t.artist, t.album, t.year, t.track_number, t.duration_secs, t.file_size_bytes, t.format, t.cover_path, t.pinned
            FROM playlist_tracks pt
            JOIN tracks t ON t.id = pt.track_id
            WHERE pt.playlist_id = ?1
            ORDER BY pt.position ASC
            "#,
        )?;
        let rows = stmt.query_map(params![playlist_id], Self::row_to_track)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn set_track_pin(&self, track_id: i64, pinned: bool) -> Result<()> {
        let p = if pinned { 1 } else { 0 };
        self.conn.execute(
            "UPDATE tracks SET pinned = ?1 WHERE id = ?2",
            params![p, track_id],
        )?;
        self.conn.execute(
            "UPDATE cache_entries SET pinned = ?1 WHERE track_id = ?2",
            params![p, track_id],
        )?;
        Ok(())
    }

    pub fn toggle_track_pin(&self, track_id: i64) -> Result<bool> {
        let current: i64 = self.conn.query_row(
            "SELECT pinned FROM tracks WHERE id = ?1",
            params![track_id],
            |row| row.get(0),
        )?;
        let next = current == 0;
        self.set_track_pin(track_id, next)?;
        Ok(next)
    }

    pub fn toggle_playlist_pin(&self, playlist_id: i64) -> Result<bool> {
        let current: i64 = self.conn.query_row(
            "SELECT pinned FROM playlists WHERE id = ?1",
            params![playlist_id],
            |row| row.get(0),
        )?;
        let next = current == 0;
        let p = if next { 1 } else { 0 };
        self.conn.execute(
            "UPDATE playlists SET pinned = ?1 WHERE id = ?2",
            params![p, playlist_id],
        )?;
        if next {
            self.conn.execute(
                "UPDATE tracks SET pinned = 1 WHERE id IN (SELECT track_id FROM playlist_tracks WHERE playlist_id = ?1)",
                params![playlist_id],
            )?;
            self.conn.execute(
                "UPDATE cache_entries SET pinned = 1 WHERE track_id IN (SELECT track_id FROM playlist_tracks WHERE playlist_id = ?1)",
                params![playlist_id],
            )?;
        }
        Ok(next)
    }

    pub fn list_pinned_tracks(&self) -> Result<Vec<Track>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT id, source_key, source_json, title, artist, album, year, track_number, duration_secs, file_size_bytes, format, cover_path, pinned
            FROM tracks
            WHERE pinned = 1
            ORDER BY artist COLLATE NOCASE ASC, album COLLATE NOCASE ASC, COALESCE(track_number, 0) ASC
            "#,
        )?;
        let rows = stmt.query_map([], Self::row_to_track)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn get_setting(&self, key: &str) -> Result<Option<String>> {
        let val = self
            .conn
            .query_row(
                "SELECT value FROM settings WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()?;
        Ok(val)
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn upsert_cache_entry(
        &self,
        cache_key: &str,
        track_id: Option<i64>,
        file_path: &str,
        size_bytes: u64,
        pinned: bool,
    ) -> Result<()> {
        self.conn.execute(
            r#"
            INSERT INTO cache_entries (cache_key, track_id, file_path, size_bytes, pinned, last_accessed_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            ON CONFLICT(cache_key) DO UPDATE SET
                track_id = COALESCE(excluded.track_id, cache_entries.track_id),
                file_path = excluded.file_path,
                size_bytes = excluded.size_bytes,
                pinned = MAX(cache_entries.pinned, excluded.pinned),
                last_accessed_at = excluded.last_accessed_at
            "#,
            params![
                cache_key,
                track_id,
                file_path,
                size_bytes as i64,
                if pinned { 1 } else { 0 },
                now_unix(),
            ],
        )?;
        Ok(())
    }

    pub fn touch_cache_entry(&self, cache_key: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE cache_entries SET last_accessed_at = ?1 WHERE cache_key = ?2",
            params![now_unix(), cache_key],
        )?;
        Ok(())
    }

    pub fn list_cache_entries_lru(&self) -> Result<Vec<CacheEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT cache_key, track_id, file_path, size_bytes, pinned, last_accessed_at FROM cache_entries ORDER BY last_accessed_at ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(CacheEntry {
                cache_key: row.get(0)?,
                track_id: row.get(1)?,
                file_path: row.get(2)?,
                size_bytes: row.get::<_, i64>(3)?.max(0) as u64,
                pinned: row.get::<_, i64>(4)? != 0,
                last_accessed_at: row.get(5)?,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn remove_cache_entry(&self, cache_key: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM cache_entries WHERE cache_key = ?1",
            params![cache_key],
        )?;
        Ok(())
    }

    pub fn cache_stats(&self, limit_bytes: u64) -> Result<CacheStats> {
        let mut stats = CacheStats {
            limit_bytes,
            ..Default::default()
        };
        let mut stmt = self
            .conn
            .prepare("SELECT size_bytes, pinned FROM cache_entries")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?.max(0) as u64,
                row.get::<_, i64>(1)? != 0,
            ))
        })?;
        for r in rows {
            let (sz, pinned) = r?;
            stats.entry_count += 1;
            stats.used_bytes += sz;
            if pinned {
                stats.pinned_count += 1;
                stats.pinned_bytes += sz;
            } else {
                stats.evictable_bytes += sz;
            }
        }
        Ok(stats)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fts5_search_and_playlists() {
        let db = Database::open_in_memory().unwrap();
        let t1 = Track {
            id: 0,
            title: "Muchacha (Ojos de Papel)".to_string(),
            artist: "Almendra".to_string(),
            album: "Almendra (1969)".to_string(),
            year: Some(1969),
            track_number: Some(1),
            duration_secs: Some(304),
            file_size_bytes: 30_000_000,
            format: "flac".to_string(),
            cover_path: None,
            source: TrackSource::LocalFile {
                path: "/music/almendra/01.flac".to_string(),
            },
            pinned: false,
        };
        let t2 = Track {
            id: 0,
            title: "Cabildo y Juramento".to_string(),
            artist: "Conociendo Rusia".to_string(),
            album: "Cabildo y Juramento (2019)".to_string(),
            year: Some(2019),
            track_number: Some(2),
            duration_secs: Some(210),
            file_size_bytes: 20_000_000,
            format: "flac".to_string(),
            cover_path: None,
            source: TrackSource::LocalFile {
                path: "/music/conociendo/02.flac".to_string(),
            },
            pinned: false,
        };

        let id1 = db.upsert_track(&t1).unwrap();
        let id2 = db.upsert_track(&t2).unwrap();
        assert_eq!(db.count_tracks().unwrap(), 2);

        let res = db.search_tracks_fts("Almen", 10).unwrap();
        assert_eq!(res.len(), 1);
        assert_eq!(res[0].id, id1);

        let res2 = db.search_tracks_fts("Juramento", 10).unwrap();
        assert_eq!(res2.len(), 1);
        assert_eq!(res2[0].id, id2);

        let pl_id = db.create_playlist("Viaje").unwrap();
        db.add_track_to_playlist(pl_id, id1).unwrap();
        let pinned_now = db.toggle_playlist_pin(pl_id).unwrap();
        assert!(pinned_now);
        assert_eq!(db.list_pinned_tracks().unwrap().len(), 1);
    }
}
