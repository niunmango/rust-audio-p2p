use std::path::PathBuf;

pub const DEFAULT_CACHE_LIMIT_BYTES: u64 = 3 * 1024 * 1024 * 1024; // 3 GiB

#[derive(Debug, Clone)]
pub struct AppPaths {
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub offline_pins_dir: PathBuf,
    pub torrent_download_dir: PathBuf,
    pub db_path: PathBuf,
}

impl AppPaths {
    pub fn resolve_default() -> Self {
        let data_base = dirs::data_local_dir().unwrap_or_else(|| PathBuf::from("."));
        let cache_base = dirs::cache_dir().unwrap_or_else(|| PathBuf::from(".cache"));

        let data_dir = data_base.join("rust-audio-p2p");
        let cache_dir = cache_base.join("rust-audio-p2p").join("media");
        let offline_pins_dir = cache_base.join("rust-audio-p2p").join("offline");
        let torrent_download_dir = cache_base.join("rust-audio-p2p").join("torrents");
        let db_path = data_dir.join("library.db");

        Self {
            data_dir,
            cache_dir,
            offline_pins_dir,
            torrent_download_dir,
            db_path,
        }
    }

    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.data_dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&self.data_dir, std::fs::Permissions::from_mode(0o700))?;
        }
        std::fs::create_dir_all(&self.cache_dir)?;
        std::fs::create_dir_all(&self.offline_pins_dir)?;
        std::fs::create_dir_all(&self.torrent_download_dir)?;
        Ok(())
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn test_ensure_dirs_restricts_data_dir_permissions() {
        let temp = std::env::temp_dir().join(format!("rust_audio_cfg_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        let paths = AppPaths {
            data_dir: temp.join("data"),
            cache_dir: temp.join("cache/media"),
            offline_pins_dir: temp.join("cache/offline"),
            torrent_download_dir: temp.join("cache/torrents"),
            db_path: temp.join("data/library.db"),
        };
        paths.ensure_dirs().unwrap();
        let mode = std::fs::metadata(&paths.data_dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
        let _ = std::fs::remove_dir_all(&temp);
    }
}
