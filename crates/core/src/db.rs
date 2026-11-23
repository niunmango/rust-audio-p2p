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
            PRAGMA synchronous = NORMAL;
            PRAGMA busy_timeout = 5000;
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

            DROP TRIGGER IF EXISTS tracks_au;
            CREATE TRIGGER tracks_au AFTER UPDATE OF title, artist, album ON tracks
            WHEN old.title IS NOT new.title OR old.artist IS NOT new.artist OR old.album IS NOT new.album
            BEGIN
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
        struct ExistingTrackRow {
            id: i64,
            source_type: String,
            source_json: String,
            title: String,
            artist: String,
            album: String,
            year: Option<i32>,
            track_number: Option<u32>,
            duration_secs: Option<i64>,
            file_size_bytes: i64,
            format: String,
            cover_path: Option<String>,
        }

        let source_key = track.source.unique_key();
        let source_type = track.source.source_type();
        let duration_i64 = track.duration_secs.map(|d| d as i64);
        let file_size_i64 = track.file_size_bytes as i64;
        let now = now_unix();

        let existing = self
            .conn
            .query_row(
                r#"
                SELECT id, source_type, source_json, title, artist, album,
                       year, track_number, duration_secs, file_size_bytes, format, cover_path
                FROM tracks
                WHERE source_key = ?1
                "#,
                params![&source_key],
                |row| {
                    Ok(ExistingTrackRow {
                        id: row.get(0)?,
                        source_type: row.get(1)?,
                        source_json: row.get(2)?,
                        title: row.get(3)?,
                        artist: row.get(4)?,
                        album: row.get(5)?,
                        year: row.get(6)?,
                        track_number: row.get(7)?,
                        duration_secs: row.get(8)?,
                        file_size_bytes: row.get(9)?,
                        format: row.get(10)?,
                        cover_path: row.get(11)?,
                    })
                },
            )
            .optional()?;

        if let Some(existing) = existing {
            let mut effective_source: Cow<'_, TrackSource> = Cow::Borrowed(&track.source);
            if let TrackSource::Peer {
                node_id,
                node_name,
                remote_track_id,
                cached_path: None,
            } = &track.source
            {
                if let Ok(TrackSource::Peer {
                    cached_path: Some(existing_path),
                    ..
                }) = serde_json::from_str::<TrackSource>(&existing.source_json)
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

            let source_json = serde_json::to_string(effective_source.as_ref())?;
            if existing.source_type == source_type
                && existing.source_json == source_json
                && existing.title == track.title
                && existing.artist == track.artist
                && existing.album == track.album
                && existing.year == track.year
                && existing.track_number == track.track_number
                && existing.duration_secs == duration_i64
                && existing.file_size_bytes == file_size_i64
                && existing.format == track.format
                && existing.cover_path == track.cover_path
            {
                return Ok(existing.id);
            }

            self.conn.execute(
                r#"
                UPDATE tracks SET
                    source_type = ?1,
                    source_json = ?2,
                    title = ?3,
                    artist = ?4,
                    album = ?5,
                    year = ?6,
                    track_number = ?7,
                    duration_secs = ?8,
                    file_size_bytes = ?9,
                    format = ?10,
                    cover_path = ?11,
                    updated_at = ?12
                WHERE id = ?13
                "#,
                params![
                    source_type,
                    source_json,
                    track.title,
                    track.artist,
                    track.album,
                    track.year,
                    track.track_number,
                    duration_i64,
                    file_size_i64,
                    track.format,
                    track.cover_path,
                    now,
                    existing.id,
                ],
            )?;
            return Ok(existing.id);
        }

        let source_json = serde_json::to_string(&track.source)?;
        self.conn.execute(
            r#"
            INSERT INTO tracks (
                source_key, source_type, source_json, title, artist, album,
                year, track_number, duration_secs, file_size_bytes, format,
                cover_path, pinned, updated_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
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
                duration_i64,
                file_size_i64,
                track.format,
                track.cover_path,
                if track.pinned { 1 } else { 0 },
                now,
            ],
        )?;

        Ok(self.conn.last_insert_rowid())
    }

    pub fn upsert_tracks_batch(&self, tracks: &[Track]) -> Result<Vec<i64>> {
        if tracks.is_empty() {
            return Ok(Vec::new());
        }
        let tx = self.conn.unchecked_transaction()?;
        let mut ids = Vec::with_capacity(tracks.len());
        for track in tracks {
            ids.push(self.upsert_track(track)?);
        }
        tx.commit()?;
        Ok(ids)
    }

    pub fn apply_peer_catalog_tx(
        &self,
        paired_peers: &[PairedPeer],
        tracks_with_remote_ids: &[(i64, Track)],
        playlists: &[(String, Vec<i64>)],
    ) -> Result<usize> {
        let tx = self.conn.unchecked_transaction()?;
        self.save_paired_peers(paired_peers)?;

        let mut remote_to_local =
            std::collections::HashMap::with_capacity(tracks_with_remote_ids.len());
        let mut upserted = 0usize;
        for (remote_id, track) in tracks_with_remote_ids {
            if let Ok(local_id) = self.upsert_track(track) {
                remote_to_local.insert(*remote_id, local_id);
                upserted += 1;
            }
        }

        for (pl_name, remote_track_ids) in playlists {
            if let Ok(local_pl_id) = self.create_playlist(pl_name) {
                for remote_tid in remote_track_ids {
                    if let Some(&local_tid) = remote_to_local.get(remote_tid) {
                        let _ = self.add_track_to_playlist(local_pl_id, local_tid);
                    }
                }
            }
        }

        tx.commit()?;
        Ok(upserted)
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

    pub fn rename_playlist(&self, playlist_id: i64, new_name: &str) -> Result<()> {
        let clean = new_name.trim();
        anyhow::ensure!(!clean.is_empty(), "Playlist name cannot be empty");
        self.conn.execute(
            "UPDATE playlists SET name = ?1 WHERE id = ?2",
            params![clean, playlist_id],
        )?;
        Ok(())
    }

    pub fn delete_playlist(&self, playlist_id: i64) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        self.conn.execute(
            "DELETE FROM playlist_tracks WHERE playlist_id = ?1",
            params![playlist_id],
        )?;
        self.conn.execute(
            "DELETE FROM playlists WHERE id = ?1",
            params![playlist_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn add_tracks_to_playlist(&self, playlist_id: i64, track_ids: &[i64]) -> Result<usize> {
        let tx = self.conn.unchecked_transaction()?;
        let mut added = 0usize;
        for &track_id in track_ids {
            self.add_track_to_playlist(playlist_id, track_id)?;
            added += 1;
        }
        tx.commit()?;
        Ok(added)
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

    #[test]
    fn test_playlist_rename_delete_and_batch_add() {
        let db = Database::open_in_memory().unwrap();
        let id1 = db
            .upsert_track(&Track {
                id: 0,
                title: "Tema Uno".into(),
                artist: "Artista".into(),
                album: "Album".into(),
                year: Some(2020),
                track_number: Some(1),
                duration_secs: Some(180),
                file_size_bytes: 1000,
                format: "flac".into(),
                cover_path: None,
                source: TrackSource::LocalFile {
                    path: "/tmp/t1.flac".into(),
                },
                pinned: false,
            })
            .unwrap();
        let id2 = db
            .upsert_track(&Track {
                id: 0,
                title: "Tema Dos".into(),
                artist: "Artista".into(),
                album: "Album".into(),
                year: Some(2020),
                track_number: Some(2),
                duration_secs: Some(200),
                file_size_bytes: 1000,
                format: "flac".into(),
                cover_path: None,
                source: TrackSource::LocalFile {
                    path: "/tmp/t2.flac".into(),
                },
                pinned: false,
            })
            .unwrap();

        let pl_id = db.create_playlist("Borrador").unwrap();
        let added = db.add_tracks_to_playlist(pl_id, &[id1, id2]).unwrap();
        assert_eq!(added, 2);
        assert_eq!(db.get_playlist_tracks(pl_id).unwrap().len(), 2);

        db.rename_playlist(pl_id, "Favoritas Ruta").unwrap();
        let pls = db.list_playlists().unwrap();
        assert_eq!(pls.len(), 1);
        assert_eq!(pls[0].name, "Favoritas Ruta");
        assert_eq!(pls[0].track_count, 2);

        db.remove_track_from_playlist(pl_id, id1).unwrap();
        assert_eq!(db.get_playlist_tracks(pl_id).unwrap().len(), 1);

        db.delete_playlist(pl_id).unwrap();
        assert!(db.list_playlists().unwrap().is_empty());
        assert!(db.get_playlist_tracks(pl_id).unwrap().is_empty());
    }

    #[test]
    fn test_upsert_track_idempotent_no_sequence_burn_and_fts_update() {
        let db = Database::open_in_memory().unwrap();
        let mut track = Track {
            id: 0,
            title: "Original Song".to_string(),
            artist: "Band A".to_string(),
            album: "Album One".to_string(),
            year: Some(2024),
            track_number: Some(1),
            duration_secs: Some(180),
            file_size_bytes: 12345,
            format: "flac".to_string(),
            cover_path: None,
            source: TrackSource::LocalFile {
                path: "/tmp/original_song.flac".to_string(),
            },
            pinned: false,
        };

        let id1 = db.upsert_track(&track).unwrap();
        let seq1: i64 = db
            .conn
            .query_row(
                "SELECT seq FROM sqlite_sequence WHERE name = 'tracks'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let updated1: i64 = db
            .conn
            .query_row(
                "SELECT updated_at FROM tracks WHERE id = ?1",
                params![id1],
                |r| r.get(0),
            )
            .unwrap();

        for _ in 0..5 {
            let id_again = db.upsert_track(&track).unwrap();
            assert_eq!(id_again, id1);
        }

        let seq2: i64 = db
            .conn
            .query_row(
                "SELECT seq FROM sqlite_sequence WHERE name = 'tracks'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let updated2: i64 = db
            .conn
            .query_row(
                "SELECT updated_at FROM tracks WHERE id = ?1",
                params![id1],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(seq1, seq2);
        assert_eq!(updated1, updated2);

        track.title = "Renamed Anthem".to_string();
        let id_updated = db.upsert_track(&track).unwrap();
        assert_eq!(id_updated, id1);
        let seq3: i64 = db
            .conn
            .query_row(
                "SELECT seq FROM sqlite_sequence WHERE name = 'tracks'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(seq3, seq1);
        assert_eq!(db.search_tracks_fts("Renamed", 10).unwrap().len(), 1);
    }

    #[test]
    fn test_batch_upsert_and_peer_catalog_tx() {
        let temp = std::env::temp_dir().join(format!(
            "rust_audio_batch_tx_{}_{}",
            std::process::id(),
            now_unix()
        ));
        let _ = std::fs::remove_dir_all(&temp);
        let db_path = temp.join("library.db");
        let db = Database::open(&db_path).unwrap();

        let batch: Vec<Track> = (0..550)
            .map(|i| Track {
                id: 0,
                title: format!("Local Track {i}"),
                artist: "Local Artist".to_string(),
                album: "Local Album".to_string(),
                year: Some(2025),
                track_number: Some(i as u32 + 1),
                duration_secs: Some(120),
                file_size_bytes: 1024 + i as u64,
                format: "flac".to_string(),
                cover_path: None,
                source: TrackSource::LocalFile {
                    path: format!("/tmp/batch_{i}.flac"),
                },
                pinned: false,
            })
            .collect();
        let ids = db.upsert_tracks_batch(&batch).unwrap();
        assert_eq!(ids.len(), 550);

        let peer = PairedPeer {
            node_id: "peer1234".to_string(),
            node_name: "remote-node".to_string(),
            endpoint: "127.0.0.1:4261".to_string(),
            auth_token: "token123".to_string(),
            track_count: 500,
            playlist_count: 1,
        };
        let peer_tracks: Vec<(i64, Track)> = (1..=500)
            .map(|rid| {
                (
                    rid,
                    Track {
                        id: 0,
                        title: format!("Peer Song {rid}"),
                        artist: "Peer Band".to_string(),
                        album: "Peer LP".to_string(),
                        year: Some(2024),
                        track_number: Some(rid as u32),
                        duration_secs: Some(200),
                        file_size_bytes: 2048,
                        format: "mp3".to_string(),
                        cover_path: None,
                        source: TrackSource::Peer {
                            node_id: "peer1234".to_string(),
                            node_name: "remote-node".to_string(),
                            remote_track_id: rid,
                            cached_path: None,
                        },
                        pinned: false,
                    },
                )
            })
            .collect();
        let playlists = vec![("[P2P: remote-node] Favs".to_string(), vec![1, 2, 3])];
        let count = db
            .apply_peer_catalog_tx(&[peer.clone()], &peer_tracks, &playlists)
            .unwrap();
        assert_eq!(count, 500);
        assert_eq!(db.count_tracks().unwrap(), 1050);
        assert_eq!(db.load_paired_peers().unwrap(), vec![peer]);
        let pls = db.list_playlists().unwrap();
        assert_eq!(pls.len(), 1);
        assert_eq!(pls[0].track_count, 3);

        let _ = std::fs::remove_dir_all(&temp);
    }
}
