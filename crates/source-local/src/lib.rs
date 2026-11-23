use std::collections::{BTreeSet, HashMap};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use anyhow::Result;
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::probe::Probe;
use lofty::tag::Accessor;
pub use notify::RecommendedWatcher;
use notify::event::{AccessKind, AccessMode, CreateKind, EventKind, ModifyKind, RenameMode};
use notify::{Event, RecursiveMode, Watcher};
use rust_audio_core::{Database, ScanStatus, Track, TrackSource};
use tokio::sync::watch;
use walkdir::{DirEntry, WalkDir};

const SUPPORTED_EXTENSIONS: &[&str] = &["flac", "mp3", "ogg", "opus", "m4a", "wav", "aac"];
const EXCLUDED_DIRS: &[&str] = &[
    "target",
    "node_modules",
    "snap",
    "venv",
    "__pycache__",
    "dist",
    "build",
    "AppData",
    "Library",
    "Applications",
    "$RECYCLE.BIN",
    "System Volume Information",
    "VirtualBox VMs",
    "__MACOSX",
];
const MAX_WATCHED_DIRS: usize = 2048;
const MAX_WATCHER_SEEN_CACHE: usize = 50_000;
const COVER_FILENAMES: &[&str] = &["cover.jpg", "cover.png", "folder.jpg", "folder.png", "artist.jpg"];

pub fn is_supported_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|ext| SUPPORTED_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

pub fn is_regular_non_symlink(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|m| m.is_file() && !m.file_type().is_symlink())
        .unwrap_or(false)
}

pub fn should_descend(entry: &DirEntry) -> bool {
    if entry.depth() == 0 {
        return true;
    }
    if entry.file_type().is_symlink() {
        return false;
    }
    if !entry.file_type().is_dir() {
        return true;
    }
    let name = entry.file_name().to_string_lossy();
    if name.starts_with('.') {
        return false;
    }
    !EXCLUDED_DIRS.contains(&name.as_ref())
}

pub fn default_scan_root() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

pub fn find_cover_image(audio_path: &Path) -> Option<String> {
    let dir = audio_path.parent()?;
    for candidate in COVER_FILENAMES {
        let p = dir.join(candidate);
        if is_regular_non_symlink(&p) {
            return Some(p.to_string_lossy().to_string());
        }
    }
    if let Some(parent) = dir.parent() {
        for candidate in COVER_FILENAMES {
            let p = parent.join(candidate);
            if is_regular_non_symlink(&p) {
                return Some(p.to_string_lossy().to_string());
            }
        }
    }
    None
}

fn parse_filename_fallback(path: &Path) -> (Option<u32>, String, String, String, Option<i32>) {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "Unknown Track".to_string());

    let (track_num, title) = if let Some((left, right)) = stem.split_once(" - ") {
        if let Ok(num) = left.trim().parse::<u32>() {
            (Some(num), right.trim().to_string())
        } else {
            (None, stem.clone())
        }
    } else {
        (None, stem.clone())
    };

    let album_dir = path
        .parent()
        .and_then(|p| p.file_name())
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "Unknown Album".to_string());

    let (album, year) = extract_album_and_year(&album_dir);

    let artist = path
        .parent()
        .and_then(|p| p.parent())
        .and_then(|p| p.file_name())
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "Unknown Artist".to_string());

    (track_num, title, artist, album, year)
}

fn extract_album_and_year(raw: &str) -> (String, Option<i32>) {
    if let Some(open) = raw.rfind('(') {
        if raw.ends_with(')') && open + 5 == raw.len() - 1 {
            if let Ok(y) = raw[open + 1..raw.len() - 1].parse::<i32>() {
                return (raw[..open].trim().to_string(), Some(y));
            }
        }
    }
    (raw.to_string(), None)
}

pub fn extract_track_metadata(path: &Path) -> Result<Track> {
    let meta = std::fs::symlink_metadata(path)?;
    anyhow::ensure!(
        meta.is_file() && !meta.file_type().is_symlink(),
        "Refusing to index non-regular file or symlink"
    );
    anyhow::ensure!(is_supported_audio(path), "Unsupported audio extension");
    let size = meta.len();
    let format = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_else(|| "unknown".to_string());

    let (fb_num, fb_title, fb_artist, fb_album, fb_year) = parse_filename_fallback(path);

    let mut title = fb_title;
    let mut artist = fb_artist;
    let mut album = fb_album;
    let mut year = fb_year;
    let mut track_number = fb_num;
    let mut duration_secs = None;

    if let Ok(probe) = Probe::open(path) {
        if let Ok(tagged) = probe.read() {
            let secs = tagged.properties().duration().as_secs();
            if secs > 0 {
                duration_secs = Some(secs);
            }
            if let Some(tag) = tagged.primary_tag().or_else(|| tagged.first_tag()) {
                if let Some(t) = tag.title().filter(|s| !s.trim().is_empty()) {
                    title = t.to_string();
                }
                if let Some(a) = tag.artist().filter(|s| !s.trim().is_empty()) {
                    artist = a.to_string();
                }
                if let Some(al) = tag.album().filter(|s| !s.trim().is_empty()) {
                    album = al.to_string();
                }
                if let Some(y) = tag.year() {
                    year = Some(y as i32);
                }
                if let Some(tn) = tag.track() {
                    track_number = Some(tn);
                }
            }
        }
    }

    let cover_path = find_cover_image(path);

    Ok(Track {
        id: 0,
        title,
        artist,
        album,
        year,
        track_number,
        duration_secs,
        file_size_bytes: size,
        format,
        cover_path,
        source: TrackSource::LocalFile {
            path: path.to_string_lossy().to_string(),
        },
        pinned: false,
    })
}

pub fn scan_directory_sync(
    root: &Path,
    db: &Arc<Mutex<Database>>,
    status_tx: Option<&watch::Sender<ScanStatus>>,
) -> Result<usize> {
    let (indexed, _) = scan_directory_incremental(root, db, status_tx)?;
    Ok(indexed)
}

pub fn scan_directory_incremental(
    root: &Path,
    db: &Arc<Mutex<Database>>,
    status_tx: Option<&watch::Sender<ScanStatus>>,
) -> Result<(usize, BTreeSet<PathBuf>)> {
    let root_str = root.to_string_lossy().to_string();
    let existing = db
        .lock()
        .ok()
        .and_then(|g| g.get_local_tracks_index().ok())
        .unwrap_or_default();

    if let Some(tx) = status_tx {
        let _ = tx.send(ScanStatus::Scanning {
            root: root_str.clone(),
            scanned_dirs: 0,
            audio_files_found: 0,
            indexed: existing.len(),
        });
    }

    let mut scanned_dirs = 0usize;
    let mut found = 0usize;
    let mut indexed = 0usize;
    let mut audio_dirs = BTreeSet::new();
    let mut pending_batch: Vec<Track> = Vec::new();
    let mut last_status_emit = Instant::now();

    let walker = WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(should_descend);

    for entry in walker.flatten() {
        if entry.file_type().is_dir() {
            scanned_dirs += 1;
            if scanned_dirs % 100 == 0 && last_status_emit.elapsed() >= Duration::from_millis(200) {
                if let Some(tx) = status_tx {
                    let _ = tx.send(ScanStatus::Scanning {
                        root: root_str.clone(),
                        scanned_dirs,
                        audio_files_found: found,
                        indexed,
                    });
                }
                last_status_emit = Instant::now();
            }
            continue;
        }

        let path = entry.path();
        if !is_supported_audio(path) {
            continue;
        }

        found += 1;
        if let Some(parent) = path.parent() {
            audio_dirs.insert(parent.to_path_buf());
        }

        let key = format!("local:{}", path.to_string_lossy());
        let file_len = entry.metadata().map(|m| m.len()).ok();
        if let (Some((prev_len, _)), Some(cur_len)) = (existing.get(&key), file_len) {
            if *prev_len == cur_len {
                // Already indexed with identical size: skip re-opening with lofty on startup.
                indexed += 1;
                continue;
            }
        }

        if let Ok(track) = extract_track_metadata(path) {
            pending_batch.push(track);
            if pending_batch.len() >= 100 {
                if let Ok(guard) = db.lock() {
                    if let Ok(ids) = guard.upsert_tracks_batch(&pending_batch) {
                        indexed += ids.len();
                    }
                }
                pending_batch.clear();
            }
        }
        if found % 25 == 0 && last_status_emit.elapsed() >= Duration::from_millis(200) {
            if let Some(tx) = status_tx {
                let _ = tx.send(ScanStatus::Scanning {
                    root: root_str.clone(),
                    scanned_dirs,
                    audio_files_found: found,
                    indexed,
                });
            }
            last_status_emit = Instant::now();
        }
    }

    if !pending_batch.is_empty() {
        if let Ok(guard) = db.lock() {
            if let Ok(ids) = guard.upsert_tracks_batch(&pending_batch) {
                indexed += ids.len();
            }
        }
        pending_batch.clear();
    }

    if let Some(tx) = status_tx {
        let _ = tx.send(ScanStatus::Completed {
            root: root_str,
            audio_files_found: found,
            indexed,
        });
    }

    Ok((indexed, audio_dirs))
}

pub fn spawn_home_scan(
    root: PathBuf,
    db: Arc<Mutex<Database>>,
    status_tx: watch::Sender<ScanStatus>,
    watcher_slot: Option<Arc<Mutex<Option<RecommendedWatcher>>>>,
) -> tokio::task::JoinHandle<Result<usize>> {
    tokio::task::spawn_blocking(move || {
        let (indexed, audio_dirs) =
            scan_directory_incremental(&root, &db, Some(&status_tx))?;
        if let Some(slot) = watcher_slot {
            if let Ok(w) = start_watcher_for_dirs(&audio_dirs, &root, Arc::clone(&db)) {
                if let Ok(mut g) = slot.lock() {
                    *g = Some(w);
                }
            }
        }
        Ok(indexed)
    })
}

pub fn should_process_watcher_event(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::Create(CreateKind::File | CreateKind::Any)
            | EventKind::Modify(ModifyKind::Data(_))
            | EventKind::Modify(ModifyKind::Name(
                RenameMode::To | RenameMode::Both | RenameMode::Any
            ))
            | EventKind::Access(AccessKind::Close(AccessMode::Write))
    )
}

pub fn start_watcher_for_dirs(
    dirs: &BTreeSet<PathBuf>,
    fallback_root: &Path,
    db: Arc<Mutex<Database>>,
) -> Result<RecommendedWatcher> {
    let mut seen: HashMap<PathBuf, (u64, Option<SystemTime>, Instant)> = HashMap::new();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
        let Ok(event) = res else {
            return;
        };
        if !should_process_watcher_event(&event.kind) {
            return;
        }
        for path in event.paths {
            if !is_supported_audio(&path) {
                continue;
            }
            let Ok(meta) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if !(meta.is_file() && !meta.file_type().is_symlink()) {
                continue;
            }
            let len = meta.len();
            let mtime = meta.modified().ok();
            let now = Instant::now();
            if let Some(&(prev_len, prev_mtime, prev_at)) = seen.get(&path) {
                if prev_len == len && prev_mtime == mtime {
                    continue;
                }
                if matches!(event.kind, EventKind::Modify(ModifyKind::Data(_)))
                    && now.duration_since(prev_at) < Duration::from_millis(500)
                {
                    continue;
                }
            }
            if seen.len() >= MAX_WATCHER_SEEN_CACHE {
                seen.clear();
            }
            seen.insert(path.clone(), (len, mtime, now));

            if let Ok(track) = extract_track_metadata(&path) {
                if let Ok(guard) = db.lock() {
                    let _ = guard.upsert_track(&track);
                }
            }
        }
    })?;

    if dirs.is_empty() {
        if fallback_root.exists() {
            let _ = watcher.watch(fallback_root, RecursiveMode::NonRecursive);
        }
    } else {
        for dir in dirs.iter().take(MAX_WATCHED_DIRS) {
            if dir.exists() {
                let _ = watcher.watch(dir, RecursiveMode::NonRecursive);
            }
        }
    }
    Ok(watcher)
}

pub fn start_incremental_watcher(
    root: &Path,
    db: Arc<Mutex<Database>>,
) -> Result<RecommendedWatcher> {
    let mut dirs = BTreeSet::new();
    dirs.insert(root.to_path_buf());
    start_watcher_for_dirs(&dirs, root, db)
}

/// Seekable reader supporting both byte-range seeking and proportional time-based seeking.
pub struct SeekableAudioFile {
    file: File,
    pub total_bytes: u64,
}

impl SeekableAudioFile {
    pub fn open(path: &Path) -> Result<Self> {
        let file = File::open(path)?;
        let total_bytes = file.metadata()?.len();
        Ok(Self { file, total_bytes })
    }

    pub fn seek_byte(&mut self, offset: u64) -> Result<u64> {
        let clamped = offset.min(self.total_bytes);
        Ok(self.file.seek(SeekFrom::Start(clamped))?)
    }

    pub fn seek_time_ratio(&mut self, position_secs: f64, total_duration_secs: f64) -> Result<u64> {
        if total_duration_secs <= 0.0 {
            return self.seek_byte(0);
        }
        let ratio = (position_secs / total_duration_secs).clamp(0.0, 1.0);
        let byte_offset = (self.total_bytes as f64 * ratio) as u64;
        self.seek_byte(byte_offset)
    }
}

impl Read for SeekableAudioFile {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.file.read(buf)
    }
}

impl Seek for SeekableAudioFile {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.file.seek(pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_supported_audio_and_exclusions() {
        assert!(is_supported_audio(Path::new("/tmp/01 - Test.FLAC")));
        assert!(is_supported_audio(Path::new("/tmp/song.mp3")));
        assert!(!is_supported_audio(Path::new("/tmp/movie.mkv")));
        assert!(!is_supported_audio(Path::new("/tmp/cover.jpg")));
        assert!(EXCLUDED_DIRS.contains(&"AppData"));
        assert!(EXCLUDED_DIRS.contains(&"Library"));
        assert!(EXCLUDED_DIRS.contains(&"Applications"));
    }

    #[test]
    fn test_scan_ignores_hidden_dirs_and_indexes_audio() {
        let base = std::env::temp_dir().join(format!("rust_audio_scan_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let music_dir = base.join("Artist/Album (2022)");
        let hidden_dir = base.join(".cache/hidden_audio");
        std::fs::create_dir_all(&music_dir).unwrap();
        std::fs::create_dir_all(&hidden_dir).unwrap();

        std::fs::write(music_dir.join("01 - Track One.flac"), b"not-a-real-flac").unwrap();
        std::fs::write(music_dir.join("cover.jpg"), b"jpg").unwrap();
        std::fs::write(hidden_dir.join("ignored.mp3"), b"ignored").unwrap();
        #[cfg(unix)]
        let secret_outside = {
            let secret = std::env::temp_dir()
                .join(format!("rust_audio_secret_outside_{}.flac", std::process::id()));
            std::fs::write(&secret, b"secret").unwrap();
            let _ = std::fs::remove_file(music_dir.join("02 - Symlinked.flac"));
            std::os::unix::fs::symlink(&secret, music_dir.join("02 - Symlinked.flac"))
                .unwrap();
            secret
        };

        let db = Arc::new(Mutex::new(Database::open_in_memory().unwrap()));
        let indexed = scan_directory_sync(&base, &db, None).unwrap();
        assert_eq!(indexed, 1);

        let tracks = db.lock().unwrap().list_tracks(10, 0).unwrap();
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].title, "Track One");
        assert_eq!(tracks[0].artist, "Artist");
        assert_eq!(tracks[0].album, "Album");
        assert_eq!(tracks[0].year, Some(2022));
        assert!(tracks[0].cover_path.as_deref().unwrap().ends_with("cover.jpg"));

        #[cfg(unix)]
        let _ = std::fs::remove_file(&secret_outside);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn test_seekable_audio_file() {
        let path = std::env::temp_dir().join(format!("rust_audio_seek_test_{}.bin", std::process::id()));
        std::fs::write(&path, b"0123456789").unwrap();
        let mut reader = SeekableAudioFile::open(&path).unwrap();
        assert_eq!(reader.seek_time_ratio(5.0, 10.0).unwrap(), 5);
        let mut buf = [0u8; 2];
        reader.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, b"56");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_watcher_event_kind_filter() {
        use notify::event::{DataChange, MetadataKind};

        assert!(!should_process_watcher_event(&EventKind::Access(
            AccessKind::Open(AccessMode::Any)
        )));
        assert!(!should_process_watcher_event(&EventKind::Access(
            AccessKind::Close(AccessMode::Read)
        )));
        assert!(!should_process_watcher_event(&EventKind::Access(
            AccessKind::Read
        )));
        assert!(!should_process_watcher_event(&EventKind::Modify(
            ModifyKind::Metadata(MetadataKind::Any)
        )));

        assert!(should_process_watcher_event(&EventKind::Create(
            CreateKind::File
        )));
        assert!(should_process_watcher_event(&EventKind::Modify(
            ModifyKind::Data(DataChange::Any)
        )));
        assert!(should_process_watcher_event(&EventKind::Modify(
            ModifyKind::Name(RenameMode::To)
        )));
        assert!(should_process_watcher_event(&EventKind::Modify(
            ModifyKind::Name(RenameMode::Both)
        )));
        assert!(should_process_watcher_event(&EventKind::Access(
            AccessKind::Close(AccessMode::Write)
        )));
    }

    #[test]
    fn test_watcher_does_not_loop_on_file_open_and_read() {
        let base = std::env::temp_dir().join(format!(
            "rust_audio_watcher_noloop_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&base);
        let music_dir = base.join("Artist/Album");
        std::fs::create_dir_all(&music_dir).unwrap();
        let track_path = music_dir.join("01 - Stable Track.flac");
        std::fs::write(&track_path, b"not-a-real-flac").unwrap();

        let db = Arc::new(Mutex::new(Database::open_in_memory().unwrap()));
        let (indexed, audio_dirs) = scan_directory_incremental(&base, &db, None).unwrap();
        assert_eq!(indexed, 1);

        let key = format!("local:{}", track_path.to_string_lossy());
        let initial_updated_at = db
            .lock()
            .unwrap()
            .get_local_tracks_index()
            .unwrap()
            .get(&key)
            .unwrap()
            .1;

        let _watcher = start_watcher_for_dirs(&audio_dirs, &base, Arc::clone(&db)).unwrap();

        for _ in 0..5 {
            let mut f = File::open(&track_path).unwrap();
            let mut buf = Vec::new();
            f.read_to_end(&mut buf).unwrap();
        }
        std::thread::sleep(Duration::from_millis(300));

        let after_updated_at = db
            .lock()
            .unwrap()
            .get_local_tracks_index()
            .unwrap()
            .get(&key)
            .unwrap()
            .1;
        assert_eq!(after_updated_at, initial_updated_at);

        let _ = std::fs::remove_dir_all(&base);
    }
}
