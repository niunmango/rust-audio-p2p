use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::layout::Rect;
use rust_audio_core::{
    AppPaths, CacheStats, Database, I18n, LanguagePreference, LruCacheManager, PairedPeer,
    PlaybackQueue, Playlist, ScanStatus, Track, TrackSource,
};
use rust_audio_net_friends::{
    FriendNodeClient, FriendNodeServer, NodeIdentity, PeerTicket, RemoteCatalogDto,
};
use rust_audio_player::PlayerEngine;
use rust_audio_source_local::{extract_track_metadata, is_supported_audio, spawn_home_scan, RecommendedWatcher};
use rust_audio_source_torrent::{
    extract_magnet_display_name, redact_url_secrets, sanitize_torrent_uri, CompositeSearchProvider,
    SearchProgressEvent, TorrentEngine, TorrentSearchResult, TorrentStatusSnapshot,
};
use tokio::sync::{mpsc, watch};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Library,
    Search,
    Playlists,
    Torrents,
    CacheSettings,
    Friends,
}

impl Tab {
    pub fn all() -> [Tab; 6] {
        [
            Tab::Library,
            Tab::Search,
            Tab::Playlists,
            Tab::Torrents,
            Tab::CacheSettings,
            Tab::Friends,
        ]
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct PeerDownloadIntent {
    pub play_on_ready: bool,
    pub pin_on_ready: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TorrentInputMode {
    None,
    Search,
    Magnet,
    JackettUrl,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaylistPanelFocus {
    Playlists,
    PlaylistTracks,
}

#[derive(Debug, Clone)]
pub struct PlaylistPickerState {
    pub track: Track,
    pub selected: usize, // 0..playlists.len()-1 are playlists; playlists.len() is "+ Create new"
    pub creating_new: bool,
    pub new_name: String,
}

#[derive(Debug, Default, Clone)]
pub struct UiLayout {
    pub tab_rects: [Rect; 6],
    pub library_table: Rect,
    pub library_scroll: usize,
    pub search_input: Rect,
    pub search_table: Rect,
    pub search_scroll: usize,
    pub playlist_input: Rect,
    pub playlists_list: Rect,
    pub playlists_scroll: usize,
    pub playlist_tracks_table: Rect,
    pub playlist_tracks_scroll: usize,
    pub torrent_search_input: Rect,
    pub torrent_magnet_input: Rect,
    pub torrent_results_table: Rect,
    pub torrent_results_scroll: usize,
    pub torrent_downloads_area: Rect,
    pub peer_connect_input: Rect,
    pub peer_discover_btn: Rect,
    pub peer_lan_share_btn: Rect,
    pub peer_qr_btn: Rect,
    pub peer_list: Rect,
    pub peer_list_scroll: usize,
    pub peer_tracks_table: Rect,
    pub peer_tracks_scroll: usize,
    pub qr_modal_box: Rect,
    pub player_bar: Rect,
    pub player_gauge: Rect,
    pub auto_next_pill: Rect,
    pub add_playlist_btn: Rect,
    pub modal_box: Rect,
    pub modal_list: Rect,
}

pub enum AsyncEvent {
    TorrentEngineReady(Arc<TorrentEngine>),
    TorrentEngineError(String),
    TorrentSearchLog(String),
    TorrentSearchPartialResults(Vec<TorrentSearchResult>),
    TorrentSearchFinished(Vec<TorrentSearchResult>, Vec<String>),
    TorrentAdded {
        uri: String,
        result: Result<TorrentStatusSnapshot, String>,
    },
    P2pServerReady(Arc<FriendNodeServer>),
    P2pServerError(String),
    P2pLanDiscovered(Vec<PeerTicket>),
    P2pPeerSynced {
        peer: PairedPeer,
        catalog: RemoteCatalogDto,
    },
    P2pPeerSyncFailed {
        target: String,
        error: String,
    },
    P2pTrackDownloaded {
        source_key: String,
        track: Track,
        cached_path: String,
        file_size: u64,
    },
    P2pTrackDownloadFailed {
        source_key: String,
        title: String,
        error: String,
    },
}

pub struct App {
    pub paths: AppPaths,
    pub db: Arc<Mutex<Database>>,
    pub cache: LruCacheManager,
    pub i18n: I18n,
    pub queue: PlaybackQueue,
    pub player: PlayerEngine,
    pub auto_next: bool,
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
    pub selected_playlist_tracks: Vec<Track>,
    pub playlist_track_selected: usize,
    pub playlist_focus: PlaylistPanelFocus,
    pub pinned_tracks: Vec<Track>,
    pub playlist_input: String,
    pub playlist_editing: bool,
    pub playlist_picker: Option<PlaylistPickerState>,
    pub torrent_query: String,
    pub torrent_magnet_input: String,
    pub jackett_url_input: String,
    pub torrent_input_mode: TorrentInputMode,
    pub torrent_searching: bool,
    pub torrent_search_ticks: u16,
    pub torrent_search_logs: Vec<String>,
    pub torrent_results: Vec<TorrentSearchResult>,
    pub torrent_result_selected: usize,
    pub pending_magnets: Vec<(String, String, Instant)>,
    pub pending_queued_uris: Vec<String>,
    pub active_torrents: Vec<TorrentStatusSnapshot>,
    pub active_torrent_selected: usize,
    pub synced_torrent_hashes: HashSet<String>,
    pub node_identity: NodeIdentity,
    pub p2p_server: Option<Arc<FriendNodeServer>>,
    pub p2p_client: FriendNodeClient,
    pub paired_peers: Vec<PairedPeer>,
    pub peer_selected: usize,
    pub peer_tracks: Vec<Track>,
    pub peer_track_selected: usize,
    pub peer_connect_input: String,
    pub peer_connect_editing: bool,
    pub show_qr_modal: bool,
    pub p2p_logs: Vec<String>,
    pub pending_peer_downloads: HashMap<String, PeerDownloadIntent>,
    pub cache_stats: CacheStats,
    pub status_message: String,
    pub should_quit: bool,
    pub layout: RefCell<UiLayout>,
    watcher_slot: Arc<Mutex<Option<RecommendedWatcher>>>,
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
        let saved_jackett_url = db.get_setting("jackett_url")?.unwrap_or_default();
        let saved_jackett_key = db.get_setting("jackett_api_key")?.unwrap_or_default();
        let search_provider = Arc::new(CompositeSearchProvider::with_overrides(
            (!saved_jackett_url.trim().is_empty()).then_some(saved_jackett_url.as_str()),
            (!saved_jackett_key.trim().is_empty()).then_some(saved_jackett_key.as_str()),
        ));
        let jackett_url_input = if !saved_jackett_url.trim().is_empty() {
            saved_jackett_url
        } else {
            search_provider
                .jackett_endpoint_hint()
                .unwrap_or("http://localhost:9117")
                .to_string()
        };
        let pref = lang_override.unwrap_or(saved_pref);
        let i18n = I18n::new(pref);

        let limit_bytes = LruCacheManager::load_limit_from_db(&db);
        let cache = LruCacheManager::new(
            paths.cache_dir.clone(),
            paths.offline_pins_dir.clone(),
            limit_bytes,
        );
        let cache_stats = cache.stats(&db)?;
        let node_identity = NodeIdentity::load_or_create(&db)?;
        let paired_peers = db.load_paired_peers()?;
        let p2p_client = FriendNodeClient::new();

        let db = Arc::new(Mutex::new(db));
        let player = if headless {
            PlayerEngine::new_headless()
        } else {
            PlayerEngine::new()
        };

        let (scan_tx, scan_rx) = watch::channel(ScanStatus::Idle);
        let (async_tx, async_rx) = mpsc::unbounded_channel();
        let watcher_slot = Arc::new(Mutex::new(None));

        if start_background_services {
            // Scan & watcher attach completely asynchronously without blocking initial UI draw.
            spawn_home_scan(
                scan_root.clone(),
                Arc::clone(&db),
                scan_tx.clone(),
                Some(Arc::clone(&watcher_slot)),
            );

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

            // Start lightweight P2P node server + initial LAN discovery + existing peers refresh
            let p2p_db = Arc::clone(&db);
            let p2p_id = node_identity.clone();
            let p2p_tx = async_tx.clone();
            let client_clone = p2p_client.clone();
            let existing_peers = paired_peers.clone();
            tokio::spawn(async move {
                match FriendNodeServer::start(p2p_db, p2p_id).await {
                    Ok(srv) => {
                        let srv_arc = Arc::new(srv);
                        let _ = p2p_tx.send(AsyncEvent::P2pServerReady(Arc::clone(&srv_arc)));
                    }
                    Err(e) => {
                        let _ = p2p_tx.send(AsyncEvent::P2pServerError(e.to_string()));
                    }
                }
                let found = client_clone
                    .discover_lan_peers(Duration::from_millis(900))
                    .await;
                if !found.is_empty() {
                    let _ = p2p_tx.send(AsyncEvent::P2pLanDiscovered(found));
                }
                for peer in existing_peers {
                    let ticket = PeerTicket {
                        node_id: peer.node_id.clone(),
                        node_name: peer.node_name.clone(),
                        endpoint: peer.endpoint.clone(),
                        auth_token: peer.auth_token.clone(),
                    };
                    if let Ok((updated_peer, catalog)) =
                        client_clone.resolve_and_sync_peer(&ticket.to_uri()).await
                    {
                        let _ = p2p_tx.send(AsyncEvent::P2pPeerSynced {
                            peer: updated_peer,
                            catalog,
                        });
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
            auto_next: true,
            torrent_engine: None,
            search_provider,
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
            selected_playlist_tracks: Vec::new(),
            playlist_track_selected: 0,
            playlist_focus: PlaylistPanelFocus::Playlists,
            pinned_tracks: Vec::new(),
            playlist_input: String::new(),
            playlist_editing: false,
            playlist_picker: None,
            torrent_query: String::new(),
            torrent_magnet_input: String::new(),
            jackett_url_input,
            torrent_input_mode: TorrentInputMode::None,
            torrent_searching: false,
            torrent_search_ticks: 0,
            torrent_search_logs: Vec::new(),
            torrent_results: Vec::new(),
            torrent_result_selected: 0,
            pending_magnets: Vec::new(),
            pending_queued_uris: Vec::new(),
            active_torrents: Vec::new(),
            active_torrent_selected: 0,
            synced_torrent_hashes: HashSet::new(),
            node_identity,
            p2p_server: None,
            p2p_client,
            paired_peers,
            peer_selected: 0,
            peer_tracks: Vec::new(),
            peer_track_selected: 0,
            peer_connect_input: String::new(),
            peer_connect_editing: false,
            show_qr_modal: false,
            p2p_logs: Vec::new(),
            pending_peer_downloads: HashMap::new(),
            cache_stats,
            status_message: String::new(),
            should_quit: false,
            layout: RefCell::new(UiLayout::default()),
            watcher_slot,
        };
        app.refresh_from_db()?;
        Ok(app)
    }

    pub fn switch_tab(&mut self, tab: Tab) {
        self.active_tab = tab;
        self.search_editing = tab == Tab::Search;
        self.playlist_editing = false;
        self.peer_connect_editing = false;
        self.show_qr_modal = false;
        if tab == Tab::Torrents {
            if self.torrent_results.is_empty() && self.torrent_query.is_empty() {
                self.torrent_input_mode = TorrentInputMode::Search;
            }
        } else {
            self.torrent_input_mode = TorrentInputMode::None;
        }
    }

    pub fn refresh_from_db(&mut self) -> Result<()> {
        let guard = self.db.lock().expect("db mutex poisoned");
        self.tracks = guard.list_tracks(2000, 0)?;
        if self.library_selected >= self.tracks.len() && !self.tracks.is_empty() {
            self.library_selected = self.tracks.len() - 1;
        }
        self.search_results = if self.search_query.trim().is_empty() {
            self.tracks.clone()
        } else {
            guard.search_tracks_fts(&self.search_query, 300)?
        };
        if self.search_selected >= self.search_results.len() && !self.search_results.is_empty() {
            self.search_selected = self.search_results.len() - 1;
        }
        self.playlists = guard.list_playlists()?;
        if self.playlist_selected >= self.playlists.len() && !self.playlists.is_empty() {
            self.playlist_selected = self.playlists.len() - 1;
        }
        self.selected_playlist_tracks = if let Some(pl) = self.playlists.get(self.playlist_selected) {
            guard.get_playlist_tracks(pl.id)?
        } else {
            Vec::new()
        };
        if self.playlist_track_selected >= self.selected_playlist_tracks.len()
            && !self.selected_playlist_tracks.is_empty()
        {
            self.playlist_track_selected = self.selected_playlist_tracks.len() - 1;
        }
        self.pinned_tracks = guard.list_pinned_tracks()?;
        self.peer_tracks = guard.list_peer_tracks(2000)?;
        if self.peer_track_selected >= self.peer_tracks.len() && !self.peer_tracks.is_empty() {
            self.peer_track_selected = self.peer_tracks.len() - 1;
        }
        if self.peer_selected >= self.paired_peers.len() && !self.paired_peers.is_empty() {
            self.peer_selected = self.paired_peers.len() - 1;
        }
        self.cache_stats = self.cache.stats(&guard)?;
        Ok(())
    }

    pub fn poll_async_events(&mut self) {
        let mut need_refresh = false;
        if self.scan_rx.has_changed().unwrap_or(false) {
            let _ = self.scan_rx.borrow_and_update();
            need_refresh = true;
        }
        if self.torrent_searching {
            self.torrent_search_ticks = (self.torrent_search_ticks + 6) % 100;
        }
        if self.player.take_track_finished() && self.auto_next {
            let _ = self.next_track();
        }
        while let Ok(ev) = self.async_rx.try_recv() {
            match ev {
                AsyncEvent::TorrentEngineReady(engine) => {
                    self.torrent_engine = Some(Arc::clone(&engine));
                    let queued = std::mem::take(&mut self.pending_queued_uris);
                    for uri in queued {
                        let tx = self.async_tx.clone();
                        let eng = Arc::clone(&engine);
                        let u_clone = uri.clone();
                        tokio::spawn(async move {
                            let res = eng
                                .add_magnet(&u_clone)
                                .await
                                .map_err(|e| redact_url_secrets(&format!("{e:#}")));
                            let _ = tx.send(AsyncEvent::TorrentAdded {
                                uri: u_clone,
                                result: res,
                            });
                        });
                    }
                }
                AsyncEvent::TorrentEngineError(err) => {
                    let safe_err = redact_url_secrets(&err);
                    self.pending_magnets.clear();
                    self.torrent_search_logs
                        .push(format!("[ERR] Motor BitTorrent: {safe_err}"));
                    self.status_message = format!("Torrent init: {safe_err}");
                }
                AsyncEvent::TorrentSearchLog(line) => {
                    self.torrent_search_logs.push(redact_url_secrets(&line));
                    if self.torrent_search_logs.len() > 80 {
                        let excess = self.torrent_search_logs.len() - 80;
                        self.torrent_search_logs.drain(0..excess);
                    }
                }
                AsyncEvent::TorrentSearchPartialResults(mut items) => {
                    self.torrent_results.append(&mut items);
                    self.torrent_results.sort_by(|a, b| b.seeders.cmp(&a.seeders));
                }
                AsyncEvent::TorrentSearchFinished(results, errors) => {
                    self.torrent_searching = false;
                    self.torrent_search_ticks = 0;
                    if !results.is_empty() {
                        self.torrent_results = results;
                        self.torrent_result_selected = 0;
                    }
                    if !errors.is_empty() && self.torrent_results.is_empty() {
                        self.status_message = redact_url_secrets(&errors.join(" | "));
                    } else {
                        self.status_message =
                            format!("Torrent results: {}", self.torrent_results.len());
                    }
                }
                AsyncEvent::TorrentAdded { uri, result } => {
                    self.pending_magnets.retain(|(u, _, _)| u != &uri);
                    match result {
                        Ok(snap) => {
                            let mb = snap.total_bytes as f64 / (1024.0 * 1024.0);
                            self.torrent_search_logs.push(format!(
                                "[OK] Descarga iniciada: '{}' ({} archivos, {:.1} MB, estado: {})",
                                snap.name,
                                snap.files.len(),
                                mb,
                                snap.state
                            ));
                            self.status_message = format!("Torrent: {} ({})", snap.name, snap.state);
                            need_refresh = true;
                        }
                        Err(e) => {
                            let name = extract_magnet_display_name(&uri);
                            let safe_err = redact_url_secrets(&e);
                            self.torrent_search_logs.push(format!(
                                "[ERR] Falló magnet '{}': {safe_err}",
                                name
                            ));
                            self.status_message = format!("Torrent err: {safe_err}");
                        }
                    }
                }
                AsyncEvent::P2pServerReady(srv) => {
                    let ticket = srv.ticket();
                    self.push_p2p_log(format!(
                        "[OK] Nodo P2P escuchando en {} (LAN Share: {})",
                        ticket.endpoint,
                        if srv.is_lan_share_enabled() { "ON" } else { "OFF" }
                    ));
                    self.p2p_server = Some(srv);
                }
                AsyncEvent::P2pServerError(err) => {
                    self.push_p2p_log(format!("[ERR] Servidor P2P: {err}"));
                }
                AsyncEvent::P2pLanDiscovered(tickets) => {
                    let my_id = self.node_identity.node_id.clone();
                    let remote_tickets: Vec<PeerTicket> =
                        tickets.into_iter().filter(|t| t.node_id != my_id).collect();
                    if remote_tickets.is_empty() {
                        self.push_p2p_log("[INFO] Escaneo LAN: sin otros nodos detectados".to_string());
                    } else {
                        for ticket in remote_tickets {
                            self.push_p2p_log(format!(
                                "[LAN] Nodo detectado: {} ({})",
                                ticket.node_name, ticket.endpoint
                            ));
                            self.spawn_peer_sync(ticket.to_uri());
                        }
                    }
                }
                AsyncEvent::P2pPeerSynced { peer, catalog } => {
                    if let Err(e) = self.apply_synced_peer(peer, catalog) {
                        self.push_p2p_log(format!("[ERR] Error guardando catálogo P2P: {e}"));
                    } else {
                        need_refresh = true;
                    }
                }
                AsyncEvent::P2pPeerSyncFailed { target, error } => {
                    self.push_p2p_log(format!("[ERR] No se pudo conectar a '{target}': {error}"));
                    self.status_message = format!("P2P err: {error}");
                }
                AsyncEvent::P2pTrackDownloaded {
                    source_key,
                    mut track,
                    cached_path,
                    file_size,
                } => {
                    let intent = self
                        .pending_peer_downloads
                        .remove(&source_key)
                        .unwrap_or_default();
                    if let TrackSource::Peer {
                        cached_path: ref mut cp,
                        ..
                    } = track.source
                    {
                        *cp = Some(cached_path.clone());
                    }
                    track.file_size_bytes = file_size.max(track.file_size_bytes);
                    if let Ok(guard) = self.db.lock() {
                        if let Ok(tid) = guard.upsert_track(&track) {
                            if intent.pin_on_ready {
                                track.id = tid;
                                let _ = self.cache.set_track_pin(&guard, &track, true);
                            }
                            let _ = self.cache.register_torrent_file(
                                &guard,
                                Some(tid),
                                &source_key,
                                Path::new(&cached_path),
                                file_size,
                                intent.pin_on_ready || track.pinned,
                            );
                        }
                    }
                    self.queue.update_track_cached_path(&source_key, &cached_path);
                    self.push_p2p_log(format!(
                        "[OK] Cacheado P2P: '{}' ({:.2} MB)",
                        track.title,
                        file_size as f64 / (1024.0 * 1024.0)
                    ));
                    need_refresh = true;
                    if intent.play_on_ready {
                        if self.queue.current().map(|c| c.source.unique_key())
                            == Some(source_key)
                        {
                            let _ = self.play_current_in_queue();
                        }
                    }
                }
                AsyncEvent::P2pTrackDownloadFailed {
                    source_key,
                    title,
                    error,
                } => {
                    self.pending_peer_downloads.remove(&source_key);
                    self.push_p2p_log(format!("[ERR] Descarga P2P '{title}': {error}"));
                    self.status_message = format!("P2P stream err: {error}");
                }
            }
        }
        if let Some(engine) = &self.torrent_engine {
            self.active_torrents = engine.list_torrents();
            for t in &self.active_torrents {
                if t.finished && !self.synced_torrent_hashes.contains(&t.info_hash) {
                    self.synced_torrent_hashes.insert(t.info_hash.clone());
                    let mut indexed_audio = 0usize;
                    for f in &t.files {
                        if f.full_path.exists() && is_supported_audio(&f.full_path) {
                            let mut tr = extract_track_metadata(&f.full_path).unwrap_or_else(|_| Track {
                                id: 0,
                                title: f.path.clone(),
                                artist: t.name.clone(),
                                album: t.name.clone(),
                                year: None,
                                track_number: None,
                                duration_secs: None,
                                file_size_bytes: f.length,
                                format: "audio".to_string(),
                                cover_path: None,
                                source: TrackSource::LocalFile {
                                    path: f.full_path.to_string_lossy().to_string(),
                                },
                                pinned: false,
                            });
                            tr.source = TrackSource::Torrent {
                                info_hash: t.info_hash.clone(),
                                magnet_uri: redact_url_secrets(&self.torrent_magnet_input),
                                file_index: f.index,
                                cached_path: Some(f.full_path.to_string_lossy().to_string()),
                            };
                            if let Ok(guard) = self.db.lock() {
                                if let Ok(tid) = guard.upsert_track(&tr) {
                                    let _ = self.cache.register_torrent_file(
                                        &guard,
                                        Some(tid),
                                        &tr.source.unique_key(),
                                        &f.full_path,
                                        f.length,
                                        false,
                                    );
                                    indexed_audio += 1;
                                }
                            }
                        }
                    }
                    if indexed_audio > 0 {
                        self.torrent_search_logs.push(format!(
                            "[OK] Descarga completa '{}': {} pistas indexadas en Biblioteca",
                            t.name, indexed_audio
                        ));
                        need_refresh = true;
                    }
                }
            }
        }
        if need_refresh {
            let _ = self.refresh_from_db();
        }
    }

    pub fn selected_track_in_active_tab(&self) -> Option<Track> {
        match self.active_tab {
            Tab::Library => self.tracks.get(self.library_selected).cloned(),
            Tab::Search => self.search_results.get(self.search_selected).cloned(),
            Tab::Friends => self.peer_tracks.get(self.peer_track_selected).cloned(),
            Tab::Playlists if self.playlist_focus == PlaylistPanelFocus::PlaylistTracks => {
                self.selected_playlist_tracks
                    .get(self.playlist_track_selected)
                    .cloned()
            }
            _ => None,
        }
    }

    pub fn play_selected_from_active_list(&mut self) -> Result<()> {
        let (list, idx) = match self.active_tab {
            Tab::Library => (&self.tracks, self.library_selected),
            Tab::Search => (&self.search_results, self.search_selected),
            Tab::Friends => (&self.peer_tracks, self.peer_track_selected),
            Tab::Playlists => (&self.selected_playlist_tracks, self.playlist_track_selected),
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

        // Prefetch next peer track in background if not cached on disk yet
        if let Some(ref next_track) = next {
            if matches!(next_track.source, TrackSource::Peer { .. }) {
                let next_ready = next_track
                    .source
                    .playable_path()
                    .map(|p| Path::new(p).exists())
                    .unwrap_or(false);
                if !next_ready {
                    self.ensure_peer_track_download(next_track, false, false);
                }
            }
        }

        if let TrackSource::Peer { .. } = &track.source {
            let ready = track
                .source
                .playable_path()
                .map(|p| Path::new(p).exists())
                .unwrap_or(false);
            if !ready {
                self.status_message = format!("⏳ P2P streaming: {}...", track.title);
                self.ensure_peer_track_download(&track, true, false);
                return Ok(());
            }
            if let Ok(guard) = self.db.lock() {
                let _ = self.cache.touch(&guard, &track.source.unique_key());
            }
        } else if let TrackSource::Torrent { .. } = &track.source {
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

    pub fn toggle_auto_next(&mut self) {
        self.auto_next = !self.auto_next;
        self.status_message = self.i18n.auto_next_label(self.auto_next).to_string();
    }

    pub fn toggle_pin_selected(&mut self) -> Result<()> {
        match self.active_tab {
            Tab::Library | Tab::Search | Tab::Friends => {
                if let Some(t) = self.selected_track_in_active_tab() {
                    let next = !t.pinned;
                    {
                        let guard = self.db.lock().expect("db mutex poisoned");
                        self.cache.set_track_pin(&guard, &t, next)?;
                    }
                    if next {
                        if let TrackSource::Peer { cached_path, .. } = &t.source {
                            let exists = cached_path
                                .as_deref()
                                .map(|p| Path::new(p).exists())
                                .unwrap_or(false);
                            if !exists {
                                self.ensure_peer_track_download(&t, false, true);
                            }
                        }
                    }
                    self.status_message = self.i18n.pin_toggled(&t.title, next);
                    self.refresh_from_db()?;
                }
            }
            Tab::Playlists => {
                if self.playlist_focus == PlaylistPanelFocus::PlaylistTracks {
                    if let Some(t) = self.selected_track_in_active_tab() {
                        let next = !t.pinned;
                        {
                            let guard = self.db.lock().expect("db mutex poisoned");
                            self.cache.set_track_pin(&guard, &t, next)?;
                        }
                        self.status_message = self.i18n.pin_toggled(&t.title, next);
                        self.refresh_from_db()?;
                    }
                } else if let Some(pl) = self.playlists.get(self.playlist_selected).cloned() {
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

    pub fn open_playlist_picker_for_selected(&mut self) {
        if let Some(track) = self.selected_track_in_active_tab() {
            let selected = if self.playlists.is_empty() {
                0
            } else {
                self.playlist_selected.min(self.playlists.len() - 1)
            };
            let creating_new = self.playlists.is_empty();
            self.playlist_picker = Some(PlaylistPickerState {
                track,
                selected,
                creating_new,
                new_name: String::new(),
            });
        }
    }

    pub fn confirm_playlist_picker(&mut self) -> Result<()> {
        let Some(picker) = self.playlist_picker.take() else {
            return Ok(());
        };
        if picker.creating_new || picker.selected >= self.playlists.len() {
            let name = picker.new_name.trim();
            if name.is_empty() {
                self.playlist_picker = Some(PlaylistPickerState {
                    creating_new: true,
                    ..picker
                });
                return Ok(());
            }
            {
                let guard = self.db.lock().expect("db mutex poisoned");
                let pl_id = guard.create_playlist(name)?;
                guard.add_track_to_playlist(pl_id, picker.track.id)?;
            }
            self.status_message = self.i18n.added_to_playlist(&picker.track.title, name);
            self.refresh_from_db()?;
        } else if let Some(pl) = self.playlists.get(picker.selected).cloned() {
            {
                let guard = self.db.lock().expect("db mutex poisoned");
                guard.add_track_to_playlist(pl.id, picker.track.id)?;
            }
            self.status_message = self.i18n.added_to_playlist(&picker.track.title, &pl.name);
            self.refresh_from_db()?;
        }
        Ok(())
    }

    pub fn remove_selected_from_playlist(&mut self) -> Result<()> {
        if self.active_tab != Tab::Playlists {
            return Ok(());
        }
        let Some(pl) = self.playlists.get(self.playlist_selected).cloned() else {
            return Ok(());
        };
        let Some(tr) = self
            .selected_playlist_tracks
            .get(self.playlist_track_selected)
            .cloned()
        else {
            return Ok(());
        };
        {
            let guard = self.db.lock().expect("db mutex poisoned");
            guard.remove_track_from_playlist(pl.id, tr.id)?;
        }
        self.status_message = self.i18n.removed_from_playlist(&tr.title, &pl.name);
        self.refresh_from_db()?;
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
            Some(Arc::clone(&self.watcher_slot)),
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

    pub fn save_jackett_url_override(&mut self) {
        let clean = self.jackett_url_input.trim().to_string();
        if let Ok(guard) = self.db.lock() {
            let _ = guard.set_setting("jackett_url", &clean);
        }
        let key_opt = self
            .db
            .lock()
            .ok()
            .and_then(|g| g.get_setting("jackett_api_key").ok().flatten());
        self.search_provider = Arc::new(CompositeSearchProvider::with_overrides(
            (!clean.is_empty()).then_some(clean.as_str()),
            key_opt.as_deref(),
        ));
        let safe_url = if clean.is_empty() {
            "auto".to_string()
        } else {
            redact_url_secrets(&clean)
        };
        self.torrent_search_logs.push(format!(
            "[INFO] Endpoint Jackett actualizado: {safe_url} (providers: {})",
            self.search_provider.provider_names().join(", ")
        ));
    }

    pub fn trigger_torrent_search(&mut self) {
        let query = self.torrent_query.clone();
        if rust_audio_source_torrent::is_torrent_uri(&query) {
            let clean = sanitize_torrent_uri(&query);
            self.torrent_magnet_input = clean.clone();
            self.torrent_query.clear();
            self.add_torrent_uri(clean);
            return;
        }
        self.torrent_searching = true;
        self.torrent_search_ticks = 10;
        self.torrent_results.clear();
        self.torrent_result_selected = 0;
        self.torrent_search_logs.clear();
        self.torrent_search_logs.push(format!(
            "[INFO] Buscando '{}' en: {}",
            query.trim(),
            self.search_provider.provider_names().join(", ")
        ));
        self.status_message = self.i18n.torrents_searching_progress(&query);

        let provider = Arc::clone(&self.search_provider);
        let async_tx = self.async_tx.clone();
        let (prog_tx, mut prog_rx) = mpsc::unbounded_channel::<SearchProgressEvent>();

        let fwd_tx = async_tx.clone();
        tokio::spawn(async move {
            while let Some(ev) = prog_rx.recv().await {
                match ev {
                    SearchProgressEvent::Log(line) => {
                        let _ = fwd_tx.send(AsyncEvent::TorrentSearchLog(line));
                    }
                    SearchProgressEvent::PartialResults(items) => {
                        let _ = fwd_tx.send(AsyncEvent::TorrentSearchPartialResults(items));
                    }
                }
            }
        });

        tokio::spawn(async move {
            let (res, errs) = provider.search_streaming(&query, prog_tx).await;
            let _ = async_tx.send(AsyncEvent::TorrentSearchFinished(res, errs));
        });
    }

    pub fn add_torrent_uri(&mut self, raw_uri: String) {
        let uri = sanitize_torrent_uri(&raw_uri);
        if uri.is_empty() {
            self.torrent_search_logs
                .push("[ERR] Magnet / .torrent URI vacía".to_string());
            return;
        }
        let display_name = extract_magnet_display_name(&uri);
        self.torrent_magnet_input = uri.clone();
        if !self.pending_magnets.iter().any(|(u, _, _)| u == &uri) {
            self.pending_magnets
                .push((uri.clone(), display_name.clone(), Instant::now()));
        }
        self.torrent_search_logs.push(format!(
            "[INFO] Magnet recibido: '{display_name}' — conectando a DHT y trackers..."
        ));
        self.status_message = format!("⏳ Resolviendo magnet: {display_name}...");

        let Some(engine) = self.torrent_engine.as_ref().map(Arc::clone) else {
            if !self.pending_queued_uris.iter().any(|u| u == &uri) {
                self.pending_queued_uris.push(uri);
            }
            self.torrent_search_logs.push(
                "[INFO] Motor BitTorrent iniciándose; descarga encolada automáticamente."
                    .to_string(),
            );
            return;
        };

        let tx = self.async_tx.clone();
        tokio::spawn(async move {
            let res = engine
                .add_magnet(&uri)
                .await
                .map_err(|e| redact_url_secrets(&format!("{e:#}")));
            let _ = tx.send(AsyncEvent::TorrentAdded { uri, result: res });
        });
    }

    pub fn play_selected_active_torrent(&mut self) -> Result<()> {
        let Some(snap) = self.active_torrents.get(self.active_torrent_selected).cloned() else {
            return Ok(());
        };
        let audio_files: Vec<Track> = snap
            .files
            .iter()
            .filter(|f| f.full_path.exists() && is_supported_audio(&f.full_path))
            .map(|f| {
                let mut tr = extract_track_metadata(&f.full_path).unwrap_or_else(|_| Track {
                    id: 0,
                    title: f.path.clone(),
                    artist: snap.name.clone(),
                    album: snap.name.clone(),
                    year: None,
                    track_number: None,
                    duration_secs: None,
                    file_size_bytes: f.length,
                    format: "audio".to_string(),
                    cover_path: None,
                    source: TrackSource::LocalFile {
                        path: f.full_path.to_string_lossy().to_string(),
                    },
                    pinned: false,
                });
                tr.source = TrackSource::Torrent {
                    info_hash: snap.info_hash.clone(),
                    magnet_uri: redact_url_secrets(&self.torrent_magnet_input),
                    file_index: f.index,
                    cached_path: Some(f.full_path.to_string_lossy().to_string()),
                };
                tr
            })
            .collect();
        if audio_files.is_empty() {
            self.status_message = format!("Torrent '{}' aún sin pistas de audio listas", snap.name);
            return Ok(());
        }
        self.queue.set_queue(audio_files, 0);
        self.play_current_in_queue()
    }

    pub fn push_p2p_log(&mut self, line: String) {
        self.p2p_logs.push(line);
        if self.p2p_logs.len() > 80 {
            let excess = self.p2p_logs.len() - 80;
            self.p2p_logs.drain(0..excess);
        }
    }

    pub fn local_p2p_ticket(&self) -> PeerTicket {
        if let Some(srv) = &self.p2p_server {
            srv.ticket()
        } else {
            PeerTicket {
                node_id: self.node_identity.node_id.clone(),
                node_name: self.node_identity.node_name.clone(),
                endpoint: format!(
                    "{}:{}",
                    rust_audio_net_friends::detect_lan_ip(),
                    rust_audio_net_friends::DEFAULT_P2P_PORT
                ),
                auth_token: self.node_identity.auth_token.clone(),
            }
        }
    }

    pub fn is_lan_share_enabled(&self) -> bool {
        self.p2p_server
            .as_ref()
            .map(|s| s.is_lan_share_enabled())
            .unwrap_or(true)
    }

    pub fn toggle_lan_share(&mut self) {
        let next = !self.is_lan_share_enabled();
        if let Some(srv) = &self.p2p_server {
            if let Ok(guard) = self.db.lock() {
                srv.set_lan_share_enabled(&guard, next);
            }
        }
        let msg = self.i18n.friends_lan_share_label(next).to_string();
        self.push_p2p_log(format!("[INFO] {msg}"));
        self.status_message = msg;
    }

    pub fn toggle_qr_modal(&mut self) {
        self.show_qr_modal = !self.show_qr_modal;
    }

    pub fn connect_peer_input(&mut self) {
        let raw = self.peer_connect_input.trim().to_string();
        self.peer_connect_editing = false;
        if raw.is_empty() {
            return;
        }
        self.push_p2p_log(format!("[INFO] Conectando a nodo '{}'...", raw));
        self.status_message = format!("⏳ P2P: {raw}...");
        self.spawn_peer_sync(raw);
    }

    pub fn trigger_lan_discovery(&mut self) {
        self.push_p2p_log("[INFO] Buscando nodos rust-audio-p2p en LAN...".to_string());
        let client = self.p2p_client.clone();
        let tx = self.async_tx.clone();
        tokio::spawn(async move {
            let found = client.discover_lan_peers(Duration::from_millis(900)).await;
            let _ = tx.send(AsyncEvent::P2pLanDiscovered(found));
        });
    }

    pub fn resync_selected_peer(&mut self) {
        if let Some(peer) = self.paired_peers.get(self.peer_selected).cloned() {
            let ticket = PeerTicket {
                node_id: peer.node_id,
                node_name: peer.node_name.clone(),
                endpoint: peer.endpoint,
                auth_token: peer.auth_token,
            };
            self.push_p2p_log(format!("[INFO] Resincronizando '{}'...", peer.node_name));
            self.spawn_peer_sync(ticket.to_uri());
        }
    }

    fn spawn_peer_sync(&self, target: String) {
        let client = self.p2p_client.clone();
        let tx = self.async_tx.clone();
        tokio::spawn(async move {
            match client.resolve_and_sync_peer(&target).await {
                Ok((peer, catalog)) => {
                    let _ = tx.send(AsyncEvent::P2pPeerSynced { peer, catalog });
                }
                Err(e) => {
                    let _ = tx.send(AsyncEvent::P2pPeerSyncFailed {
                        target,
                        error: e.to_string(),
                    });
                }
            }
        });
    }

    pub fn apply_synced_peer(
        &mut self,
        peer: PairedPeer,
        catalog: RemoteCatalogDto,
    ) -> Result<()> {
        if let Some(existing) = self
            .paired_peers
            .iter_mut()
            .find(|p| p.node_id == peer.node_id)
        {
            *existing = peer.clone();
        } else {
            self.paired_peers.push(peer.clone());
        }

        let track_count = catalog.tracks.len();
        let playlist_count = catalog.playlists.len();

        {
            let guard = self.db.lock().expect("db mutex poisoned");
            guard.save_paired_peers(&self.paired_peers)?;

            let mut remote_to_local: HashMap<i64, i64> = HashMap::new();
            for rt in &catalog.tracks {
                let track = Track {
                    id: 0,
                    title: rt.title.clone(),
                    artist: rt.artist.clone(),
                    album: rt.album.clone(),
                    year: rt.year,
                    track_number: rt.track_number,
                    duration_secs: rt.duration_secs,
                    file_size_bytes: rt.file_size_bytes,
                    format: rt.format.clone(),
                    cover_path: None,
                    source: TrackSource::Peer {
                        node_id: catalog.node_id.clone(),
                        node_name: catalog.node_name.clone(),
                        remote_track_id: rt.id,
                        cached_path: None,
                    },
                    pinned: false,
                };
                if let Ok(local_id) = guard.upsert_track(&track) {
                    remote_to_local.insert(rt.id, local_id);
                }
            }

            for rpl in &catalog.playlists {
                let pl_name = format!("[P2P: {}] {}", catalog.node_name, rpl.name);
                if let Ok(local_pl_id) = guard.create_playlist(&pl_name) {
                    for remote_tid in &rpl.track_ids {
                        if let Some(&local_tid) = remote_to_local.get(remote_tid) {
                            let _ = guard.add_track_to_playlist(local_pl_id, local_tid);
                        }
                    }
                }
            }
        }

        self.refresh_from_db()?;
        let ok_msg = format!(
            "[OK] Nodo '{}' sincronizado: {} pistas, {} playlists",
            peer.node_name, track_count, playlist_count
        );
        self.push_p2p_log(ok_msg.clone());
        self.status_message = ok_msg;
        Ok(())
    }

    pub fn ensure_peer_track_download(
        &mut self,
        track: &Track,
        play_on_ready: bool,
        pin_on_ready: bool,
    ) {
        let TrackSource::Peer {
            node_id,
            remote_track_id,
            ..
        } = &track.source
        else {
            return;
        };

        let key = track.source.unique_key();
        if let Some(existing) = self.pending_peer_downloads.get_mut(&key) {
            existing.play_on_ready |= play_on_ready;
            existing.pin_on_ready |= pin_on_ready;
            return;
        }

        let Some(peer) = self
            .paired_peers
            .iter()
            .find(|p| p.node_id == *node_id)
            .cloned()
        else {
            self.push_p2p_log(format!(
                "[ERR] Nodo '{node_id}' no encontrado en nodos emparejados"
            ));
            return;
        };

        self.pending_peer_downloads.insert(
            key.clone(),
            PeerDownloadIntent {
                play_on_ready,
                pin_on_ready,
            },
        );
        self.push_p2p_log(format!(
            "[INFO] Descargando '{}' desde '{}'...",
            track.title, peer.node_name
        ));

        let client = self.p2p_client.clone();
        let tx = self.async_tx.clone();
        let cache_dir = self.paths.cache_dir.clone();
        let track_clone = track.clone();
        let remote_id = *remote_track_id;
        tokio::spawn(async move {
            match client
                .download_peer_track_to_cache(&peer, remote_id, &track_clone.format, &cache_dir)
                .await
            {
                Ok(path) => {
                    let file_size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                    let _ = tx.send(AsyncEvent::P2pTrackDownloaded {
                        source_key: key,
                        track: track_clone,
                        cached_path: path.to_string_lossy().to_string(),
                        file_size,
                    });
                }
                Err(e) => {
                    let _ = tx.send(AsyncEvent::P2pTrackDownloadFailed {
                        source_key: key,
                        title: track_clone.title,
                        error: e.to_string(),
                    });
                }
            }
        });
    }
}
