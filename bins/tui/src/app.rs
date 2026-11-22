use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use rust_audio_core::{
    AppPaths, CacheStats, Database, I18n, LanguagePreference, LruCacheManager, PlaybackQueue,
    Playlist, ScanStatus, Track, TrackSource,
};
use rust_audio_player::PlayerEngine;
use rust_audio_source_local::{spawn_home_scan, start_incremental_watcher};
use rust_audio_source_torrent::{CompositeSearchProvider, TorrentEngine, TorrentSearchResult, TorrentStatusSnapshot};
use tokio::sync::{mpsc, watch};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Library,
    Search,
    Playlists,
    Torrents,
    CacheSettings,
}

impl Tab {
    pub fn all() -> [Tab; 5] {
        [
            Tab::Library,
            Tab::Search,
            Tab::Playlists,
            Tab::Torrents,
            Tab::CacheSettings,
        ]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TorrentInputMode {
    None,
    Search,
    Magnet,
}

pub enum AsyncEvent {
    TorrentEngineReady(Arc<TorrentEngine>),
    TorrentEngineError(String),
    TorrentSearchFinished(Vec<TorrentSearchResult>, Vec<String>),
    TorrentAdded(Result<TorrentStatusSnapshot, String>),
}

pub struct App {
    pub paths: AppPaths,
    pub db: Arc<Mutex<Database>>,
    pub cache: LruCacheManager,
    pub i18n: I18n,
    pub queue: PlaybackQueue,
    pub player: PlayerEngine,
    pub torrent_engine: Option<Arc<TorrentEngine>>,
    pub search_provider: Arc<CompositeSearchProvider>,
    pub scan_root: PathBuf,
    pub scan_tx: watch::Sender<ScanStatus>,
    pub scan_rx: watch::Receiver<ScanStatus>,
    pub async_tx: mpsc::UnboundedSender<AsyncEvent>,
    pub async_rx: mpsc::UnboundedReceiver<AsyncEvent>,
    pub active_tab: Tab,
    pub tracks: Vec<Track>,
    pub library_selected: usize,
    pub search_query: String,
    pub search_editing: bool,
    pub search_results: Vec<Track>,
    pub search_selected: usize,
    pub playlists: Vec<Playlist>,
    pub playlist_selected: usize,
    pub pinned_tracks: Vec<Track>,
    pub playlist_input: String,
    pub playlist_editing: bool,
    pub torrent_query: String,
    pub torrent_magnet_input: String,
    pub torrent_input_mode: TorrentInputMode,
    pub torrent_results: Vec<TorrentSearchResult>,
    pub torrent_result_selected: usize,
    pub active_torrents: Vec<TorrentStatusSnapshot>,
    pub cache_stats: CacheStats,
    pub status_message: String,
    pub should_quit: bool,
    _watcher: Option<Box<dyn std::any::Any + Send>>,
}

impl App {
    pub fn new(
        paths: AppPaths,
        db: Database,
        lang_override: Option<LanguagePreference>,
        scan_root: PathBuf,
        headless: bool,
        start_background_services: bool,
    ) -> Result<Self> {
        let saved_pref = db
            .get_setting("language_pref")?
            .map(|s| LanguagePreference::parse(&s))
            .unwrap_or(LanguagePreference::Auto);
        let pref = lang_override.unwrap_or(saved_pref);
        let i18n = I18n::new(pref);

        let limit_bytes = LruCacheManager::load_limit_from_db(&db);
        let cache = LruCacheManager::new(
            paths.cache_dir.clone(),
            paths.offline_pins_dir.clone(),
            limit_bytes,
        );
        let cache_stats = cache.stats(&db)?;

        let db = Arc::new(Mutex::new(db));
        let player = if headless {
            PlayerEngine::new_headless()
        } else {
            PlayerEngine::new()
        };

        let (scan_tx, scan_rx) = watch::channel(ScanStatus::Idle);
        let (async_tx, async_rx) = mpsc::unbounded_channel();

        let watcher = if start_background_services {
            spawn_home_scan(scan_root.clone(), Arc::clone(&db), scan_tx.clone());
            match start_incremental_watcher(&scan_root, Arc::clone(&db)) {
                Ok(w) => Some(Box::new(w) as Box<dyn std::any::Any + Send>),
                Err(_) => None,
            }
        } else {
            None
        };

        if start_background_services {
            let dl_dir = paths.torrent_download_dir.clone();
            let tx = async_tx.clone();
            tokio::spawn(async move {
                match TorrentEngine::new(dl_dir).await {
                    Ok(engine) => {
                        let _ = tx.send(AsyncEvent::TorrentEngineReady(Arc::new(engine)));
                    }
                    Err(e) => {
                        let _ = tx.send(AsyncEvent::TorrentEngineError(e.to_string()));
                    }
                }
            });
        }

        let mut app = Self {
            paths,
            db,
            cache,
            i18n,
            queue: PlaybackQueue::new(),
            player,
            torrent_engine: None,
            search_provider: Arc::new(CompositeSearchProvider::with_defaults()),
            scan_root,
            scan_tx,
            scan_rx,
            async_tx,
            async_rx,
            active_tab: Tab::Library,
            tracks: Vec::new(),
            library_selected: 0,
            search_query: String::new(),
            search_editing: false,
            search_results: Vec::new(),
            search_selected: 0,
            playlists: Vec::new(),
            playlist_selected: 0,
            pinned_tracks: Vec::new(),
            playlist_input: String::new(),
            playlist_editing: false,
            torrent_query: String::new(),
            torrent_magnet_input: String::new(),
            torrent_input_mode: TorrentInputMode::None,
            torrent_results: Vec::new(),
            torrent_result_selected: 0,
            active_torrents: Vec::new(),
            cache_stats,
            status_message: String::new(),
            should_quit: false,
            _watcher: watcher,
        };
        app.refresh_from_db()?;
        Ok(app)
    }

    pub fn refresh_from_db(&mut self) -> Result<()> {
        let guard = self.db.lock().expect("db mutex poisoned");
        self.tracks = guard.list_tracks(1000, 0)?;
        if self.library_selected >= self.tracks.len() && !self.tracks.is_empty() {
            self.library_selected = self.tracks.len() - 1;
        }
        self.search_results = if self.search_query.trim().is_empty() {
            self.tracks.clone()
        } else {
            guard.search_tracks_fts(&self.search_query, 200)?
        };
        if self.search_selected >= self.search_results.len() && !self.search_results.is_empty() {
            self.search_selected = self.search_results.len() - 1;
        }
        self.playlists = guard.list_playlists()?;
        if self.playlist_selected >= self.playlists.len() && !self.playlists.is_empty() {
            self.playlist_selected = self.playlists.len() - 1;
        }
        self.pinned_tracks = guard.list_pinned_tracks()?;
        self.cache_stats = self.cache.stats(&guard)?;
        Ok(())
    }

    pub fn poll_async_events(&mut self) {
        let mut need_refresh = false;
        if self.scan_rx.has_changed().unwrap_or(false) {
            let _ = self.scan_rx.borrow_and_update();
            need_refresh = true;
        }
        while let Ok(ev) = self.async_rx.try_recv() {
            match ev {
                AsyncEvent::TorrentEngineReady(engine) => {
                    self.torrent_engine = Some(engine);
                }
                AsyncEvent::TorrentEngineError(err) => {
                    self.status_message = format!("Torrent init: {err}");
                }
                AsyncEvent::TorrentSearchFinished(results, errors) => {
                    self.torrent_results = results;
                    self.torrent_result_selected = 0;
                    if !errors.is_empty() && self.torrent_results.is_empty() {
                        self.status_message = errors.join(" | ");
                    } else {
                        self.status_message = format!("Torrent results: {}", self.torrent_results.len());
                    }
                }
                AsyncEvent::TorrentAdded(res) => match res {
                    Ok(snap) => {
                        self.status_message = format!("Torrent: {} ({})", snap.name, snap.state);
                        if let Ok(guard) = self.db.lock() {
                            let tr = Track {
                                id: 0,
                                title: snap.name.clone(),
                                artist: "BitTorrent".to_string(),
                                album: snap.info_hash.clone(),
                                year: None,
                                track_number: None,
                                duration_secs: None,
                                file_size_bytes: snap.total_bytes,
                                format: "torrent".to_string(),
                                cover_path: None,
                                source: TrackSource::Torrent {
                                    info_hash: snap.info_hash.clone(),
                                    magnet_uri: self.torrent_magnet_input.clone(),
                                    file_index: 0,
                                    cached_path: None,
                                },
                                pinned: false,
                            };
                            let _ = guard.upsert_track(&tr);
                        }
                        need_refresh = true;
                    }
                    Err(e) => {
                        self.status_message = format!("Torrent err: {e}");
                    }
                },
            }
        }
        if let Some(engine) = &self.torrent_engine {
            self.active_torrents = engine.list_torrents();
        }
        if need_refresh {
            let _ = self.refresh_from_db();
        }
    }

    pub fn play_selected_from_active_list(&mut self) -> Result<()> {
        let (list, idx) = match self.active_tab {
            Tab::Library => (&self.tracks, self.library_selected),
            Tab::Search => (&self.search_results, self.search_selected),
            _ => return Ok(()),
        };
        if list.is_empty() || idx >= list.len() {
            return Ok(());
        }
        self.queue.set_queue(list.clone(), idx);
        self.play_current_in_queue()
    }

    pub fn play_current_in_queue(&mut self) -> Result<()> {
        let Some(track) = self.queue.current().cloned() else {
            return Ok(());
        };
        let next = self.queue.peek_next_for_prefetch().cloned();
        if let TrackSource::Torrent { .. } = &track.source {
            if let Ok(guard) = self.db.lock() {
                let _ = self.cache.touch(&guard, &track.source.unique_key());
            }
        }
        self.player.play_track(track, next)?;
        Ok(())
    }

    pub fn next_track(&mut self) -> Result<()> {
        if self.queue.next().is_some() {
            self.play_current_in_queue()?;
        }
        Ok(())
    }

    pub fn prev_track(&mut self) -> Result<()> {
        if self.queue.prev().is_some() {
            self.play_current_in_queue()?;
        }
        Ok(())
    }

    pub fn toggle_pin_selected(&mut self) -> Result<()> {
        match self.active_tab {
            Tab::Library | Tab::Search => {
                let track = match self.active_tab {
                    Tab::Library => self.tracks.get(self.library_selected).cloned(),
                    Tab::Search => self.search_results.get(self.search_selected).cloned(),
                    _ => None,
                };
                if let Some(t) = track {
                    let next = !t.pinned;
                    {
                        let guard = self.db.lock().expect("db mutex poisoned");
                        self.cache.set_track_pin(&guard, &t, next)?;
                    }
                    self.status_message = self.i18n.pin_toggled(&t.title, next);
                    self.refresh_from_db()?;
                }
            }
            Tab::Playlists => {
                if let Some(pl) = self.playlists.get(self.playlist_selected).cloned() {
                    let pinned = {
                        let guard = self.db.lock().expect("db mutex poisoned");
                        guard.toggle_playlist_pin(pl.id)?
                    };
                    self.status_message = self.i18n.pin_toggled(&pl.name, pinned);
                    self.refresh_from_db()?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub fn add_selected_to_default_playlist(&mut self) -> Result<()> {
        let track = match self.active_tab {
            Tab::Library => self.tracks.get(self.library_selected).cloned(),
            Tab::Search => self.search_results.get(self.search_selected).cloned(),
            _ => None,
        };
        if let Some(t) = track {
            let pl_name = self
                .playlists
                .get(self.playlist_selected)
                .map(|p| p.name.clone())
                .unwrap_or_else(|| "Viaje / Travel".to_string());
            {
                let guard = self.db.lock().expect("db mutex poisoned");
                let pl_id = guard.create_playlist(&pl_name)?;
                guard.add_track_to_playlist(pl_id, t.id)?;
            }
            self.status_message = self.i18n.added_to_playlist(&t.title, &pl_name);
            self.refresh_from_db()?;
        }
        Ok(())
    }

    pub fn create_playlist_from_input(&mut self) -> Result<()> {
        let name = self.playlist_input.trim().to_string();
        if !name.is_empty() {
            {
                let guard = self.db.lock().expect("db mutex poisoned");
                guard.create_playlist(&name)?;
            }
            self.status_message = self.i18n.playlist_created(&name);
            self.playlist_input.clear();
            self.refresh_from_db()?;
        }
        self.playlist_editing = false;
        Ok(())
    }

    pub fn trigger_rescan(&mut self) {
        spawn_home_scan(
            self.scan_root.clone(),
            Arc::clone(&self.db),
            self.scan_tx.clone(),
        );
    }

    pub fn cycle_language(&mut self) {
        self.i18n.cycle_language();
        if let Ok(guard) = self.db.lock() {
            let _ = guard.set_setting("language_pref", self.i18n.pref.as_str());
        }
        self.status_message = self.i18n.language_label();
    }

    pub fn purge_cache(&mut self) -> Result<()> {
        let report = {
            let guard = self.db.lock().expect("db mutex poisoned");
            self.cache.purge_unpinned(&guard)?
        };
        let mb = report.freed_bytes as f64 / (1024.0 * 1024.0);
        self.status_message = self.i18n.cache_purged(report.evicted_count, mb);
        self.refresh_from_db()?;
        Ok(())
    }

    pub fn adjust_cache_limit_mb(&mut self, delta_mb: i64) -> Result<()> {
        let cur_mb = (self.cache.limit_bytes / (1024 * 1024)) as i64;
        let next_mb = (cur_mb + delta_mb).max(256) as u64;
        let guard = self.db.lock().expect("db mutex poisoned");
        self.cache.save_limit_to_db(&guard, next_mb * 1024 * 1024)?;
        drop(guard);
        self.refresh_from_db()?;
        Ok(())
    }

    pub fn trigger_torrent_search(&self) {
        let query = self.torrent_query.clone();
        let provider = Arc::clone(&self.search_provider);
        let tx = self.async_tx.clone();
        tokio::spawn(async move {
            let (res, errs) = provider.search_all(&query).await;
            let _ = tx.send(AsyncEvent::TorrentSearchFinished(res, errs));
        });
    }

    pub fn add_torrent_uri(&self, uri: String) {
        let Some(engine) = self.torrent_engine.as_ref().map(Arc::clone) else {
            let _ = self.async_tx.send(AsyncEvent::TorrentEngineError(
                "Torrent engine still initializing".to_string(),
            ));
            return;
        };
        let tx = self.async_tx.clone();
        tokio::spawn(async move {
            let res = engine.add_magnet(&uri).await.map_err(|e| e.to_string());
            let _ = tx.send(AsyncEvent::TorrentAdded(res));
        });
    }
}
