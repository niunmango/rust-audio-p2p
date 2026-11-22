use std::borrow::Cow;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};

use crate::models::{CacheEntry, CacheStats, PairedPeer, Playlist, Track, TrackSource};

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
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
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
        let mut effective_source: Cow<'_, TrackSource> = Cow::Borrowed(&track.source);

        if let TrackSource::Peer {
            node_id,
            node_name,
            remote_track_id,
            cached_path: None,
        } = &track.source
        {
            if let Ok(Some(existing_json)) = self
                .conn
                .query_row(
                    "SELECT source_json FROM tracks WHERE source_key = ?1",
                    params![&source_key],
                    |row| row.get::<_, String>(0),
                )
                .optional()
            {
                if let Ok(TrackSource::Peer {
                    cached_path: Some(existing_path),
                    ..
                }) = serde_json::from_str::<TrackSource>(&existing_json)
                {
                    if Path::new(&existing_path).exists() {
                        effective_source = Cow::Owned(TrackSource::Peer {
                            node_id: node_id.clone(),
                            node_name: node_name.clone(),
                            remote_track_id: *remote_track_id,
                            cached_path: Some(existing_path),
                        });
                    }
                }
            }
        }

        let source_json = serde_json::to_string(effective_source.as_ref())?;
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

    pub fn load_paired_peers(&self) -> Result<Vec<PairedPeer>> {
        let Some(raw) = self.get_setting("paired_peers_json")? else {
            return Ok(Vec::new());
        };
        if raw.trim().is_empty() {
            return Ok(Vec::new());
        }
        let peers: Vec<PairedPeer> = serde_json::from_str(&raw).unwrap_or_default();
        Ok(peers)
    }

    pub fn save_paired_peers(&self, peers: &[PairedPeer]) -> Result<()> {
        let json = serde_json::to_string(peers)?;
        self.set_setting("paired_peers_json", &json)
    }

    pub fn list_peer_tracks(&self, limit: usize) -> Result<Vec<Track>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT id, source_key, source_json, title, artist, album, year, track_number, duration_secs, file_size_bytes, format, cover_path, pinned
            FROM tracks
            WHERE source_type = 'peer'
            ORDER BY artist COLLATE NOCASE ASC, album COLLATE NOCASE ASC, COALESCE(track_number, 0) ASC, title COLLATE NOCASE ASC
            LIMIT ?1
            "#,
        )?;
        let rows = stmt.query_map(params![limit as i64], Self::row_to_track)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
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

    #[cfg(unix)]
    #[test]
    fn test_database_open_restricts_file_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let temp = std::env::temp_dir().join(format!("rust_audio_db_perm_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        let db_path = temp.join("library.db");
        let db = Database::open(&db_path).unwrap();
        drop(db);

        let mode = std::fs::metadata(&db_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn test_paired_peers_and_peer_track_preserves_cache_and_pin() {
        let temp = std::env::temp_dir().join(format!("rust_audio_peer_db_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(&temp).unwrap();
        let cached_file = temp.join("remote_1.flac");
        std::fs::write(&cached_file, b"fLaCtest").unwrap();

        let db = Database::open_in_memory().unwrap();
        let peer = PairedPeer {
            node_id: "0123456789abcdef".to_string(),
            node_name: "air".to_string(),
            endpoint: "192.168.1.10:4261".to_string(),
            auth_token: "deadbeef".to_string(),
            track_count: 1,
            playlist_count: 0,
        };
        db.save_paired_peers(&[peer.clone()]).unwrap();
        let loaded = db.load_paired_peers().unwrap();
        assert_eq!(loaded, vec![peer]);

        let t_cached = Track {
            id: 0,
            title: "Track Remote".to_string(),
            artist: "Charly".to_string(),
            album: "Piano Bar".to_string(),
            year: Some(1984),
            track_number: Some(1),
            duration_secs: Some(200),
            file_size_bytes: 8,
            format: "flac".to_string(),
            cover_path: None,
            source: TrackSource::Peer {
                node_id: "0123456789abcdef".to_string(),
                node_name: "air".to_string(),
                remote_track_id: 42,
                cached_path: Some(cached_file.to_string_lossy().to_string()),
            },
            pinned: true,
        };
        db.upsert_track(&t_cached).unwrap();

        // Resync with cached_path = None and pinned = false must preserve local valid cache & pin
        let mut t_resync = t_cached.clone();
        t_resync.pinned = false;
        t_resync.source = TrackSource::Peer {
            node_id: "0123456789abcdef".to_string(),
            node_name: "air".to_string(),
            remote_track_id: 42,
            cached_path: None,
        };
        db.upsert_track(&t_resync).unwrap();

        let peers_tracks = db.list_peer_tracks(10).unwrap();
        assert_eq!(peers_tracks.len(), 1);
        assert!(peers_tracks[0].pinned);
        assert_eq!(
            peers_tracks[0].source.playable_path(),
            Some(cached_file.to_string_lossy().as_ref())
        );

        let _ = std::fs::remove_dir_all(&temp);
    }
}
