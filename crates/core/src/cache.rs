use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::config::DEFAULT_CACHE_LIMIT_BYTES;
use crate::db::Database;
use crate::models::{CacheStats, Track, TrackSource};

#[derive(Debug, Default, Clone)]
pub struct EvictionReport {
    pub evicted_count: usize,
    pub freed_bytes: u64,
}

pub struct LruCacheManager {
    pub cache_dir: PathBuf,
    pub offline_pins_dir: PathBuf,
    pub limit_bytes: u64,
}

impl LruCacheManager {
    pub fn new(cache_dir: PathBuf, offline_pins_dir: PathBuf, limit_bytes: u64) -> Self {
        Self {
            cache_dir,
            offline_pins_dir,
            limit_bytes,
        }
    }

    pub fn load_limit_from_db(db: &Database) -> u64 {
        db.get_setting("cache_limit_bytes")
            .ok()
            .flatten()
            .and_then(|s| s.parse::<u64>().ok())
            .filter(|&b| b >= 64 * 1024 * 1024)
            .unwrap_or(DEFAULT_CACHE_LIMIT_BYTES)
    }

    pub fn save_limit_to_db(&mut self, db: &Database, limit_bytes: u64) -> Result<()> {
        self.limit_bytes = limit_bytes;
        db.set_setting("cache_limit_bytes", &limit_bytes.to_string())?;
        Ok(())
    }

    /// Registers a downloaded/torrent file in the LRU cache. LocalFile sources stay in-place.
    pub fn register_torrent_file(
        &self,
        db: &Database,
        track_id: Option<i64>,
        cache_key: &str,
        file_path: &Path,
        size_bytes: u64,
        pinned: bool,
    ) -> Result<EvictionReport> {
        db.upsert_cache_entry(
            cache_key,
            track_id,
            &file_path.to_string_lossy(),
            size_bytes,
            pinned,
        )?;
        self.enforce_limit(db)
    }

    /// Pins or unpins a track for offline travel.
    /// Local files are never duplicated into the LRU download cache; we protect their DB entry
    /// and optionally create a zero-copy hardlink in `offline_pins_dir` if on the same filesystem.
    pub fn set_track_pin(&self, db: &Database, track: &Track, pinned: bool) -> Result<()> {
        db.set_track_pin(track.id, pinned)?;
        if let TrackSource::LocalFile { path } = &track.source {
            let src = Path::new(path);
            if let Some(name) = src.file_name() {
                let _ = std::fs::create_dir_all(&self.offline_pins_dir);
                let link_path = self.offline_pins_dir.join(format!("{}_{}", track.id, name.to_string_lossy()));
                if pinned {
                    if src.exists() && !link_path.exists() {
                        // Hardlink uses zero extra disk space on the same LV/filesystem.
                        let _ = std::fs::hard_link(src, &link_path);
                    }
                } else if link_path.exists() {
                    let _ = std::fs::remove_file(&link_path);
                }
            }
        }
        Ok(())
    }

    pub fn touch(&self, db: &Database, cache_key: &str) -> Result<()> {
        db.touch_cache_entry(cache_key)
    }

    pub fn stats(&self, db: &Database) -> Result<CacheStats> {
        db.cache_stats(self.limit_bytes)
    }

    pub fn enforce_limit(&self, db: &Database) -> Result<EvictionReport> {
        let mut stats = self.stats(db)?;
        let mut report = EvictionReport::default();
        if stats.used_bytes <= self.limit_bytes {
            return Ok(report);
        }

        let entries = db.list_cache_entries_lru()?;
        for entry in entries {
            if stats.used_bytes <= self.limit_bytes {
                break;
            }
            if entry.pinned {
                continue;
            }
            let p = Path::new(&entry.file_path);
            if p.exists() && p.starts_with(&self.cache_dir) {
                let _ = std::fs::remove_file(p);
            }
            db.remove_cache_entry(&entry.cache_key)?;
            stats.used_bytes = stats.used_bytes.saturating_sub(entry.size_bytes);
            report.evicted_count += 1;
            report.freed_bytes += entry.size_bytes;
        }
        Ok(report)
    }

    pub fn purge_unpinned(&self, db: &Database) -> Result<EvictionReport> {
        let mut report = EvictionReport::default();
        let entries = db.list_cache_entries_lru()?;
        for entry in entries {
            if entry.pinned {
                continue;
            }
            let p = Path::new(&entry.file_path);
            if p.exists() && p.starts_with(&self.cache_dir) {
                let _ = std::fs::remove_file(p);
            }
            db.remove_cache_entry(&entry.cache_key)?;
            report.evicted_count += 1;
            report.freed_bytes += entry.size_bytes;
        }
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lru_eviction_respects_pinned_entries() {
        let temp = std::env::temp_dir().join(format!("rust_audio_lru_test_{}", std::process::id()));
        let cache_dir = temp.join("media");
        let offline_dir = temp.join("offline");
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(&cache_dir).unwrap();
        std::fs::create_dir_all(&offline_dir).unwrap();

        let db = Database::open_in_memory().unwrap();
        let mgr = LruCacheManager::new(cache_dir.clone(), offline_dir, 120);

        let f1 = cache_dir.join("old_unpinned.bin");
        let f2 = cache_dir.join("pinned.bin");
        let f3 = cache_dir.join("new_unpinned.bin");
        std::fs::write(&f1, vec![0u8; 60]).unwrap();
        std::fs::write(&f2, vec![0u8; 50]).unwrap();
        std::fs::write(&f3, vec![0u8; 40]).unwrap();

        mgr.register_torrent_file(&db, None, "k1", &f1, 60, false).unwrap();
        mgr.register_torrent_file(&db, None, "k2", &f2, 50, true).unwrap();
        let report = mgr.register_torrent_file(&db, None, "k3", &f3, 40, false).unwrap();

        assert_eq!(report.evicted_count, 1);
        assert!(!f1.exists(), "oldest unpinned file should be evicted");
        assert!(f2.exists(), "pinned file must NEVER be evicted");
        assert!(f3.exists(), "newest unpinned file should remain within 120B limit");

        let _ = std::fs::remove_dir_all(&temp);
    }
}
