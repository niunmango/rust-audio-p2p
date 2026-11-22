use std::path::{Component, Path, PathBuf};

use anyhow::Result;

use crate::config::DEFAULT_CACHE_LIMIT_BYTES;
use crate::db::Database;
use crate::models::{CacheStats, Track, TrackSource};

fn safe_remove_in_dir(base_dir: &Path, target: &Path) {
    if target
        .components()
        .any(|c| matches!(c, Component::ParentDir))
    {
        return;
    }
    let Ok(meta) = std::fs::symlink_metadata(target) else {
        return;
    };
    if meta.file_type().is_symlink() {
        return;
    }
    let (Ok(canon_base), Ok(canon_target)) = (
        std::fs::canonicalize(base_dir),
        std::fs::canonicalize(target),
    ) else {
        return;
    };
    if canon_target.starts_with(&canon_base) && canon_target != canon_base {
        let _ = std::fs::remove_file(&canon_target);
    }
}

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

    fn sync_offline_pin_hardlink(&self, track_id: i64, src: &Path, prefix: &str, pinned: bool) {
        if let Some(name) = src.file_name() {
            let _ = std::fs::create_dir_all(&self.offline_pins_dir);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(
                    &self.offline_pins_dir,
                    std::fs::Permissions::from_mode(0o700),
                );
            }
            let safe_name = name.to_string_lossy().replace('/', "_");
            let link_path = self
                .offline_pins_dir
                .join(format!("{prefix}{track_id}_{safe_name}"));
            if pinned {
                let is_regular = std::fs::symlink_metadata(src)
                    .map(|m| m.is_file() && !m.file_type().is_symlink())
                    .unwrap_or(false);
                if is_regular && !link_path.exists() {
                    // Hardlink uses zero extra disk space on the same LV/filesystem.
                    let _ = std::fs::hard_link(src, &link_path);
                }
            } else {
                safe_remove_in_dir(&self.offline_pins_dir, &link_path);
            }
        }
    }

    /// Pins or unpins a track for offline travel.
    /// Local files are never duplicated into the LRU download cache; we protect their DB entry
    /// and optionally create a zero-copy hardlink in `offline_pins_dir` if on the same filesystem.
    pub fn set_track_pin(&self, db: &Database, track: &Track, pinned: bool) -> Result<()> {
        db.set_track_pin(track.id, pinned)?;
        match &track.source {
            TrackSource::LocalFile { path } => {
                self.sync_offline_pin_hardlink(track.id, Path::new(path), "", pinned);
            }
            TrackSource::Peer {
                cached_path: Some(path),
                ..
            } => {
                self.sync_offline_pin_hardlink(track.id, Path::new(path), "peer_", pinned);
            }
            _ => {}
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
            safe_remove_in_dir(&self.cache_dir, p);
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
            safe_remove_in_dir(&self.cache_dir, p);
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

    #[test]
    fn test_eviction_refuses_traversal_outside_cache_dir() {
        let temp = std::env::temp_dir().join(format!("rust_audio_lru_trav_{}", std::process::id()));
        let cache_dir = temp.join("media");
        let offline_dir = temp.join("offline");
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(&cache_dir).unwrap();
        std::fs::create_dir_all(&offline_dir).unwrap();

        let outside = temp.join("outside.bin");
        std::fs::write(&outside, vec![1u8; 80]).unwrap();
        let traversal = cache_dir.join("../outside.bin");

        let db = Database::open_in_memory().unwrap();
        let mgr = LruCacheManager::new(cache_dir, offline_dir, 50);
        let _ = mgr
            .register_torrent_file(&db, None, "trav", &traversal, 80, false)
            .unwrap();

        assert!(outside.exists(), "file outside cache_dir must NEVER be deleted");
        let _ = std::fs::remove_dir_all(&temp);
    }
}
