use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TrackSource {
    LocalFile {
        path: String,
    },
    Torrent {
        info_hash: String,
        magnet_uri: String,
        file_index: usize,
        cached_path: Option<String>,
    },
}

impl TrackSource {
    pub fn source_type(&self) -> &'static str {
        match self {
            TrackSource::LocalFile { .. } => "local",
            TrackSource::Torrent { .. } => "torrent",
        }
    }

    pub fn unique_key(&self) -> String {
        match self {
            TrackSource::LocalFile { path } => format!("local:{}", path),
            TrackSource::Torrent {
                info_hash,
                file_index,
                ..
            } => format!("torrent:{}:{}", info_hash, file_index),
        }
    }

    pub fn playable_path(&self) -> Option<&str> {
        match self {
            TrackSource::LocalFile { path } => Some(path.as_str()),
            TrackSource::Torrent { cached_path, .. } => cached_path.as_deref(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Track {
    pub id: i64,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub year: Option<i32>,
    pub track_number: Option<u32>,
    pub duration_secs: Option<u64>,
    pub file_size_bytes: u64,
    pub format: String,
    pub cover_path: Option<String>,
    pub source: TrackSource,
    pub pinned: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Album {
    pub title: String,
    pub artist: String,
    pub year: Option<i32>,
    pub cover_path: Option<String>,
    pub track_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artist {
    pub name: String,
    pub album_count: usize,
    pub track_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Playlist {
    pub id: i64,
    pub name: String,
    pub pinned: bool,
    pub track_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheEntry {
    pub cache_key: String,
    pub track_id: Option<i64>,
    pub file_path: String,
    pub size_bytes: u64,
    pub pinned: bool,
    pub last_accessed_at: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CacheStats {
    pub used_bytes: u64,
    pub pinned_bytes: u64,
    pub evictable_bytes: u64,
    pub limit_bytes: u64,
    pub entry_count: usize,
    pub pinned_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScanStatus {
    Idle,
    Scanning {
        root: String,
        scanned_dirs: usize,
        audio_files_found: usize,
        indexed: usize,
    },
    Completed {
        root: String,
        audio_files_found: usize,
        indexed: usize,
    },
    Error(String),
}
