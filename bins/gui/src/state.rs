use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use rust_audio_core::{
    AppPaths, CacheStats, Database, I18n, LanguagePreference, LruCacheManager, PairedPeer,
    PlaybackQueue, Playlist, ScanStatus, Track, TrackSource,
};
use rust_audio_net_friends::{
    sanitize_display_text, track_to_remote_dto, FriendNodeClient, FriendNodeServer, NodeIdentity,
    PeerTicket, RemoteCatalogDto, WebPlayerSnapshot, WebRemoteCommand,
};
use rust_audio_player::{AudioBackendKind, PlaybackState, PlayerEngine};
use rust_audio_source_local::{
    extract_track_metadata, is_supported_audio, spawn_home_scan, RecommendedWatcher,
};
use rust_audio_source_torrent::{
    extract_magnet_display_name, redact_url_secrets, sanitize_torrent_uri, CompositeSearchProvider,
    SearchProgressEvent, TorrentEngine, TorrentSearchResult, TorrentStatusSnapshot,
};
use tokio::sync::{mpsc, watch};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibrarySourceFilter {
    All,
    PinnedOnly,
    PeerOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuiTab {
    Library,
    Search,
    Playlists,
    Torrents,
    CacheSettings,
    Friends,
}

impl GuiTab {
    pub fn all() -> [GuiTab; 6] {
        [
            GuiTab::Library,
            GuiTab::Search,
            GuiTab::Playlists,
            GuiTab::Torrents,
            GuiTab::CacheSettings,
            GuiTab::Friends,
        ]
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct PeerDownloadIntent {
    pub play_on_ready: bool,
    pub pin_on_ready: bool,
}

pub struct PlaylistPickerModal {
    pub tracks: Vec<Track>,
    pub new_name: String,
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

pub struct GuiState {
    pub paths: AppPaths,
    pub db: Arc<Mutex<Database>>,
    pub cache: LruCacheManager,
    pub i18n: I18n,
    pub queue: PlaybackQueue,
    pub player: PlayerEngine,
    pub auto_next: bool,
    pub seek_preview: Option<f64>,
    pub torrent_engine: Option<Arc<TorrentEngine>>,
    pub search_provider: Arc<CompositeSearchProvider>,
    pub scan_root: PathBuf,
    pub scan_tx: watch::Sender<ScanStatus>,
    pub scan_rx: watch::Receiver<ScanStatus>,
    pub async_tx: mpsc::UnboundedSender<AsyncEvent>,
    pub async_rx: mpsc::UnboundedReceiver<AsyncEvent>,
    pub web_cmd_rx: mpsc::UnboundedReceiver<WebRemoteCommand>,
    pub active_tab: GuiTab,
    pub show_help_modal: bool,
    pub library_filter: String,
    pub library_source_filter: LibrarySourceFilter,
    pub selected_row_idx: usize,
    pub tracks: Vec<Track>,
    pub search_query: String,
    pub search_results: Vec<Track>,
    pub playlists: Vec<Playlist>,
    pub selected_playlist_idx: Option<usize>,
    pub selected_playlist_tracks: Vec<Track>,
    pub new_playlist_input: String,
    pub rename_playlist_id: Option<i64>,
    pub rename_playlist_input: String,
    pub marked_track_ids: HashSet<i64>,
    pub pinned_tracks: Vec<Track>,
    pub playlist_picker: Option<PlaylistPickerModal>,
    pub torrent_query: String,
    pub torrent_magnet_input: String,
    pub jackett_url_input: String,
    pub torrent_searching: bool,
    pub torrent_search_ticks: f32,
    pub torrent_search_logs: Vec<String>,
    pub torrent_results: Vec<TorrentSearchResult>,
    pub pending_magnets: Vec<(String, String, Instant)>,
    pub pending_queued_uris: Vec<String>,
    pub active_torrents: Vec<TorrentStatusSnapshot>,
    pub synced_torrent_hashes: HashSet<String>,
    pub node_identity: NodeIdentity,
    pub p2p_server: Option<Arc<FriendNodeServer>>,
    pub p2p_client: FriendNodeClient,
    pub paired_peers: Vec<PairedPeer>,
    pub peer_tracks: Vec<Track>,
    pub peer_connect_input: String,
    pub p2p_logs: Vec<String>,
    pub pending_peer_downloads: HashMap<String, PeerDownloadIntent>,
    pub cache_stats: CacheStats,
    pub status_message: String,
    watcher_slot: Arc<Mutex<Option<RecommendedWatcher>>>,
}

impl GuiState {
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
        let (web_cmd_tx, web_cmd_rx) = mpsc::unbounded_channel();
        let watcher_slot = Arc::new(Mutex::new(None));

        let mut st = Self {
            paths: paths.clone(),
            db: Arc::clone(&db),
            cache,
            i18n,
            queue: PlaybackQueue::new(),
            player,
            auto_next: true,
            seek_preview: None,
            torrent_engine: None,
            search_provider,
            scan_root: scan_root.clone(),
            scan_tx: scan_tx.clone(),
            scan_rx,
            async_tx: async_tx.clone(),
            async_rx,
            web_cmd_rx,
            active_tab: GuiTab::Library,
            show_help_modal: false,
            library_filter: String::new(),
            library_source_filter: LibrarySourceFilter::All,
            selected_row_idx: 0,
            tracks: Vec::new(),
            search_query: String::new(),
            search_results: Vec::new(),
            playlists: Vec::new(),
            selected_playlist_idx: None,
            selected_playlist_tracks: Vec::new(),
            new_playlist_input: String::new(),
            rename_playlist_id: None,
            rename_playlist_input: String::new(),
            marked_track_ids: HashSet::new(),
            pinned_tracks: Vec::new(),
            playlist_picker: None,
            torrent_query: String::new(),
            torrent_magnet_input: String::new(),
            jackett_url_input,
            torrent_searching: false,
            torrent_search_ticks: 0.0,
            torrent_search_logs: Vec::new(),
            torrent_results: Vec::new(),
            pending_magnets: Vec::new(),
            pending_queued_uris: Vec::new(),
            active_torrents: Vec::new(),
            synced_torrent_hashes: HashSet::new(),
            node_identity: node_identity.clone(),
            p2p_server: None,
            p2p_client: p2p_client.clone(),
            paired_peers: paired_peers.clone(),
            peer_tracks: Vec::new(),
            peer_connect_input: String::new(),
            p2p_logs: Vec::new(),
            pending_peer_downloads: HashMap::new(),
            cache_stats,
            status_message: String::new(),
            watcher_slot: Arc::clone(&watcher_slot),
        };
        // Populate initial UI from SQLite before background directory walking acquires DB locks.
        st.refresh_from_db()?;

        if start_background_services {
            spawn_home_scan(
                scan_root,
                Arc::clone(&db),
                scan_tx,
                Some(watcher_slot),
            );

            let dl_dir = paths.torrent_download_dir;
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

            let p2p_db = db;
            let p2p_id = node_identity;
            let p2p_tx = async_tx;
            let client_clone = p2p_client;
            let existing_peers = paired_peers;
            let web_tx = web_cmd_tx;
            tokio::spawn(async move {
                match FriendNodeServer::start_with_control(p2p_db, p2p_id, Some(web_tx)).await {
                    Ok(srv) => {
                        let _ = p2p_tx.send(AsyncEvent::P2pServerReady(Arc::new(srv)));
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

        Ok(st)
    }

    pub fn refresh_from_db(&mut self) -> Result<()> {
        let guard = self.db.lock().expect("db mutex poisoned");
        self.tracks = guard.list_tracks(2000, 0)?;
        self.search_results = if self.search_query.trim().is_empty() {
            self.tracks.clone()
        } else {
            guard.search_tracks_fts(&self.search_query, 300)?
        };
        self.playlists = guard.list_playlists()?;
        if self.playlists.is_empty() {
            self.selected_playlist_idx = None;
            self.selected_playlist_tracks.clear();
        } else {
            let idx = self
                .selected_playlist_idx
                .unwrap_or(0)
                .min(self.playlists.len() - 1);
            self.selected_playlist_idx = Some(idx);
            self.selected_playlist_tracks = guard.get_playlist_tracks(self.playlists[idx].id)?;
        }
        self.pinned_tracks = guard.list_pinned_tracks()?;
        self.peer_tracks = guard.list_peer_tracks(2000)?;
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
            self.torrent_search_ticks = (self.torrent_search_ticks + 0.04) % 1.0;
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
                    self.torrent_search_ticks = 0.0;
                    if !results.is_empty() {
                        self.torrent_results = results;
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
                                "[OK] '{}': {} files, {:.1} MB ({})",
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
                            self.torrent_search_logs
                                .push(format!("[ERR] Magnet '{name}': {safe_err}"));
                            self.status_message = format!("Torrent err: {safe_err}");
                        }
                    }
                }
                AsyncEvent::P2pServerReady(srv) => {
                    let ticket = srv.ticket();
                    self.push_p2p_log(format!(
                        "[OK] P2P Web/API: http://{} (LAN Share: {})",
                        ticket.endpoint,
                        if srv.is_lan_share_enabled() { "ON" } else { "OFF" }
                    ));
                    self.p2p_server = Some(srv);
                }
                AsyncEvent::P2pServerError(err) => {
                    self.push_p2p_log(format!("[ERR] P2P Server: {err}"));
                }
                AsyncEvent::P2pLanDiscovered(tickets) => {
                    let my_id = self.node_identity.node_id.clone();
                    let remote_tickets: Vec<PeerTicket> =
                        tickets.into_iter().filter(|t| t.node_id != my_id).collect();
                    if remote_tickets.is_empty() {
                        self.push_p2p_log("[INFO] LAN scan: no other peers found".to_string());
                    } else {
                        for ticket in remote_tickets {
                            self.push_p2p_log(format!(
                                "[LAN] Peer detected: {} ({})",
                                ticket.node_name, ticket.endpoint
                            ));
                            self.spawn_peer_sync(ticket.to_uri());
                        }
                    }
                }
                AsyncEvent::P2pPeerSynced { peer, catalog } => {
                    if let Err(e) = self.apply_synced_peer(peer, catalog) {
                        self.push_p2p_log(format!("[ERR] P2P sync save failed: {e}"));
                    } else {
                        need_refresh = true;
                    }
                }
                AsyncEvent::P2pPeerSyncFailed { target, error } => {
                    let safe_target = sanitize_display_text(&redact_url_secrets(&target));
                    let safe_err = sanitize_display_text(&redact_url_secrets(&error));
                    self.push_p2p_log(format!("[ERR] Connect '{safe_target}': {safe_err}"));
                    self.status_message = format!("P2P err: {safe_err}");
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
                        "[OK] Cached P2P: '{}' ({:.2} MB)",
                        track.title,
                        file_size as f64 / (1024.0 * 1024.0)
                    ));
                    need_refresh = true;
                    if intent.play_on_ready
                        && self.queue.current().map(|c| c.source.unique_key()) == Some(source_key)
                    {
                        let _ = self.play_current_in_queue();
                    }
                }
                AsyncEvent::P2pTrackDownloadFailed {
                    source_key,
                    title,
                    error,
                } => {
                    self.pending_peer_downloads.remove(&source_key);
                    self.push_p2p_log(format!("[ERR] P2P stream '{title}': {error}"));
                    self.status_message = format!("P2P stream err: {error}");
                }
            }
        }

        if let Some(engine) = &self.torrent_engine {
            self.active_torrents = engine.list_torrents();
            for t in &self.active_torrents {
                if t.finished && !self.synced_torrent_hashes.contains(&t.info_hash) {
                    self.synced_torrent_hashes.insert(t.info_hash.clone());
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
                                    need_refresh = true;
                                }
                            }
                        }
                    }
                }
            }
        }

        while let Ok(cmd) = self.web_cmd_rx.try_recv() {
            match cmd {
                WebRemoteCommand::PlayTrack { track_id } => {
                    if let Some(pos) = self.tracks.iter().position(|t| t.id == track_id) {
                        self.queue.set_queue(self.tracks.clone(), pos);
                        let _ = self.play_current_in_queue();
                    } else {
                        let fetched = self
                            .db
                            .lock()
                            .ok()
                            .and_then(|g| g.get_track(track_id).ok().flatten());
                        if let Some(tr) = fetched {
                            self.queue.play_now(tr);
                            let _ = self.play_current_in_queue();
                        }
                    }
                }
                WebRemoteCommand::EnqueueTrack { track_id } => {
                    let fetched = self
                        .db
                        .lock()
                        .ok()
                        .and_then(|g| g.get_track(track_id).ok().flatten());
                    if let Some(tr) = fetched {
                        let _ = self.enqueue_tracks(std::slice::from_ref(&tr));
                    }
                }
                WebRemoteCommand::EnqueueTracks { track_ids } => {
                    let mut fetched = Vec::new();
                    if let Ok(guard) = self.db.lock() {
                        for id in track_ids {
                            if let Ok(Some(tr)) = guard.get_track(id) {
                                fetched.push(tr);
                            }
                        }
                    }
                    if !fetched.is_empty() {
                        let _ = self.enqueue_tracks(&fetched);
                    }
                }
                WebRemoteCommand::TogglePause => {
                    let _ = self.player.toggle_pause();
                }
                WebRemoteCommand::Next => {
                    let _ = self.next_track();
                }
                WebRemoteCommand::Prev => {
                    let _ = self.prev_track();
                }
                WebRemoteCommand::Seek { position_secs } => {
                    let _ = self.player.seek_to(position_secs);
                }
                WebRemoteCommand::SetVolume { volume } => {
                    let _ = self.player.set_volume(volume);
                }
                WebRemoteCommand::ToggleAutoNext => {
                    self.toggle_auto_next();
                }
                WebRemoteCommand::TogglePin { .. } => {}
                WebRemoteCommand::CreatePlaylist { .. }
                | WebRemoteCommand::CreatePlaylistWithTracks { .. }
                | WebRemoteCommand::RenamePlaylist { .. }
                | WebRemoteCommand::DeletePlaylist { .. }
                | WebRemoteCommand::AddToPlaylist { .. }
                | WebRemoteCommand::AddTracksToPlaylist { .. }
                | WebRemoteCommand::RemoveFromPlaylist { .. } => {
                    need_refresh = true;
                }
                WebRemoteCommand::ConnectPeer { target } => {
                    let safe_target = sanitize_display_text(&redact_url_secrets(&target));
                    self.push_p2p_log(format!("[WEB] Connect '{safe_target}'..."));
                    self.spawn_peer_sync(target);
                }
                WebRemoteCommand::DiscoverLan => {
                    self.trigger_lan_discovery();
                }
                WebRemoteCommand::SyncRefresh { message } => {
                    if let Some(m) = message {
                        self.status_message = m;
                    }
                    need_refresh = true;
                }
            }
        }

        if need_refresh {
            let _ = self.refresh_from_db();
        }
        self.sync_web_player_snapshot();
    }

    pub fn sync_web_player_snapshot(&self) {
        let Some(srv) = &self.p2p_server else {
            return;
        };
        let pst = self.player.status();
        let state_str = match pst.state {
            PlaybackState::Playing => "playing",
            PlaybackState::Paused => "paused",
            PlaybackState::Stopped => "stopped",
        };
        let backend_str = match pst.backend {
            AudioBackendKind::NativeAlsaPipeWire => "native",
            AudioBackendKind::HeadlessNullSink => "headless",
        };
        srv.update_player_snapshot(WebPlayerSnapshot {
            state: state_str.to_string(),
            position_secs: pst.position_secs,
            duration_secs: pst.duration_secs,
            volume: pst.volume,
            auto_next: self.auto_next,
            current_track: pst.current_track.as_ref().map(track_to_remote_dto),
            backend: backend_str.to_string(),
        });
    }

    pub fn play_list_at(&mut self, list: &[Track], idx: usize) -> Result<()> {
        if list.is_empty() || idx >= list.len() {
            return Ok(());
        }
        self.queue.set_queue(list.to_vec(), idx);
        self.play_current_in_queue()
    }

    pub fn play_current_in_queue(&mut self) -> Result<()> {
        let Some(track) = self.queue.current().cloned() else {
            return Ok(());
        };
        let next = self.queue.peek_next_for_prefetch().cloned();

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

    pub fn toggle_track_pin(&mut self, t: &Track) -> Result<()> {
        let next = !t.pinned;
        {
            let guard = self.db.lock().expect("db mutex poisoned");
            self.cache.set_track_pin(&guard, t, next)?;
        }
        if next {
            if let TrackSource::Peer { cached_path, .. } = &t.source {
                let exists = cached_path
                    .as_deref()
                    .map(|p| Path::new(p).exists())
                    .unwrap_or(false);
                if !exists {
                    self.ensure_peer_track_download(t, false, true);
                }
            }
        }
        self.status_message = self.i18n.pin_toggled(&t.title, next);
        self.refresh_from_db()?;
        Ok(())
    }

    pub fn toggle_playlist_pin(&mut self, pl_id: i64, name: &str) -> Result<()> {
        let pinned = {
            let guard = self.db.lock().expect("db mutex poisoned");
            guard.toggle_playlist_pin(pl_id)?
        };
        self.status_message = self.i18n.pin_toggled(name, pinned);
        self.refresh_from_db()?;
        Ok(())
    }

    pub fn create_playlist_from_input(&mut self) -> Result<()> {
        let name = self.new_playlist_input.trim().to_string();
        if !name.is_empty() {
            {
                let guard = self.db.lock().expect("db mutex poisoned");
                guard.create_playlist(&name)?;
            }
            self.status_message = self.i18n.playlist_created(&name);
            self.new_playlist_input.clear();
            self.refresh_from_db()?;
        }
        Ok(())
    }

    pub fn open_batch_playlist_picker(&mut self) {
        if self.marked_track_ids.is_empty() {
            return;
        }
        let tracks = self.collect_marked_tracks();
        if !tracks.is_empty() {
            self.playlist_picker = Some(PlaylistPickerModal {
                tracks,
                new_name: String::new(),
            });
        }
    }

    fn collect_marked_tracks(&self) -> Vec<Track> {
        let mut by_id = HashMap::new();
        for list in [
            &self.tracks,
            &self.search_results,
            &self.peer_tracks,
            &self.selected_playlist_tracks,
            &self.pinned_tracks,
        ] {
            for t in list {
                if self.marked_track_ids.contains(&t.id) {
                    by_id.entry(t.id).or_insert_with(|| t.clone());
                }
            }
        }
        if let Ok(guard) = self.db.lock() {
            for &id in &self.marked_track_ids {
                if !by_id.contains_key(&id) {
                    if let Ok(Some(t)) = guard.get_track(id) {
                        by_id.insert(id, t);
                    }
                }
            }
        }
        by_id.into_values().collect()
    }

    pub fn enqueue_tracks(&mut self, tracks: &[Track]) -> Result<()> {
        if tracks.is_empty() {
            return Ok(());
        }
        let should_start =
            self.queue.is_empty() || self.player.status().state == PlaybackState::Stopped;
        let start_idx = self.queue.len();
        self.queue.enqueue_many(tracks.iter().cloned());
        if should_start {
            let target_idx = if start_idx < self.queue.len() {
                start_idx
            } else {
                0
            };
            self.queue.jump_to(target_idx);
            self.play_current_in_queue()?;
        } else {
            let _ = self
                .player
                .set_prefetch(self.queue.peek_next_for_prefetch().cloned());
        }
        let total = self.queue.len();
        self.status_message = if tracks.len() == 1 {
            self.i18n.added_to_queue(&tracks[0].title, total)
        } else {
            self.i18n.added_multiple_to_queue(tracks.len(), total)
        };
        self.marked_track_ids.clear();
        Ok(())
    }

    pub fn enqueue_marked_tracks(&mut self) -> Result<()> {
        if self.marked_track_ids.is_empty() {
            return Ok(());
        }
        let tracks = self.collect_marked_tracks();
        self.enqueue_tracks(&tracks)
    }

    pub fn add_tracks_to_playlist(
        &mut self,
        pl_id: i64,
        pl_name: &str,
        tracks: &[Track],
    ) -> Result<()> {
        if tracks.is_empty() {
            return Ok(());
        }
        let ids: Vec<i64> = tracks.iter().map(|t| t.id).collect();
        {
            let guard = self.db.lock().expect("db mutex poisoned");
            guard.add_tracks_to_playlist(pl_id, &ids)?;
        }
        self.status_message = if tracks.len() == 1 {
            self.i18n.added_to_playlist(&tracks[0].title, pl_name)
        } else {
            self.i18n.added_multiple_to_playlist(tracks.len(), pl_name)
        };
        self.marked_track_ids.clear();
        self.refresh_from_db()?;
        Ok(())
    }

    pub fn create_playlist_and_add_tracks(&mut self, name: &str, tracks: &[Track]) -> Result<()> {
        let clean = name.trim();
        if clean.is_empty() || tracks.is_empty() {
            return Ok(());
        }
        let ids: Vec<i64> = tracks.iter().map(|t| t.id).collect();
        {
            let guard = self.db.lock().expect("db mutex poisoned");
            let pl_id = guard.create_playlist(clean)?;
            guard.add_tracks_to_playlist(pl_id, &ids)?;
        }
        self.status_message = if tracks.len() == 1 {
            self.i18n.added_to_playlist(&tracks[0].title, clean)
        } else {
            self.i18n.added_multiple_to_playlist(tracks.len(), clean)
        };
        self.marked_track_ids.clear();
        self.refresh_from_db()?;
        Ok(())
    }

    pub fn create_playlist_and_add_track(&mut self, name: &str, track: &Track) -> Result<()> {
        self.create_playlist_and_add_tracks(name, std::slice::from_ref(track))
    }

    pub fn rename_playlist(&mut self, pl_id: i64, new_name: &str) -> Result<()> {
        let clean = new_name.trim();
        if clean.is_empty() {
            return Ok(());
        }
        {
            let guard = self.db.lock().expect("db mutex poisoned");
            guard.rename_playlist(pl_id, clean)?;
        }
        self.rename_playlist_id = None;
        self.rename_playlist_input.clear();
        self.status_message = self.i18n.playlist_renamed(clean);
        self.refresh_from_db()?;
        Ok(())
    }

    pub fn delete_playlist(&mut self, pl_id: i64, pl_name: &str) -> Result<()> {
        {
            let guard = self.db.lock().expect("db mutex poisoned");
            guard.delete_playlist(pl_id)?;
        }
        if self.rename_playlist_id == Some(pl_id) {
            self.rename_playlist_id = None;
            self.rename_playlist_input.clear();
        }
        self.status_message = self.i18n.playlist_deleted(pl_name);
        self.refresh_from_db()?;
        Ok(())
    }

    pub fn remove_track_from_active_playlist(&mut self, track: &Track) -> Result<()> {
        let Some(idx) = self.selected_playlist_idx else {
            return Ok(());
        };
        let Some(pl) = self.playlists.get(idx).cloned() else {
            return Ok(());
        };
        {
            let guard = self.db.lock().expect("db mutex poisoned");
            guard.remove_track_from_playlist(pl.id, track.id)?;
        }
        self.status_message = self.i18n.removed_from_playlist(&track.title, &pl.name);
        self.refresh_from_db()?;
        Ok(())
    }

    pub fn set_language_preference(&mut self, pref: LanguagePreference) {
        self.i18n.set_preference(pref);
        if let Ok(guard) = self.db.lock() {
            let _ = guard.set_setting("language_pref", self.i18n.pref.as_str());
        }
        self.status_message = self.i18n.language_label();
    }

    pub fn trigger_rescan(&self) {
        spawn_home_scan(
            self.scan_root.clone(),
            Arc::clone(&self.db),
            self.scan_tx.clone(),
            Some(Arc::clone(&self.watcher_slot)),
        );
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
        self.torrent_search_logs
            .push(format!("[INFO] Jackett endpoint -> {safe_url}"));
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
        self.torrent_search_ticks = 0.1;
        self.torrent_results.clear();
        self.torrent_search_logs.clear();
        self.torrent_search_logs.push(format!(
            "[INFO] Searching '{}' via {}",
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
                .push("[ERR] Empty Magnet / .torrent URI".to_string());
            return;
        }
        let display_name = extract_magnet_display_name(&uri);
        self.torrent_magnet_input = uri.clone();
        if !self.pending_magnets.iter().any(|(u, _, _)| u == &uri) {
            self.pending_magnets
                .push((uri.clone(), display_name.clone(), Instant::now()));
        }
        self.torrent_search_logs.push(format!(
            "[INFO] Magnet queued: '{display_name}' — resolving DHT/trackers..."
        ));
        self.status_message = format!("⏳ Resolving magnet: {display_name}...");

        let Some(engine) = self.torrent_engine.as_ref().map(Arc::clone) else {
            if !self.pending_queued_uris.iter().any(|u| u == &uri) {
                self.pending_queued_uris.push(uri);
            }
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

    pub fn play_torrent_snapshot(&mut self, snap: &TorrentStatusSnapshot) -> Result<()> {
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
            self.status_message = format!("Torrent '{}' audio not ready yet", snap.name);
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

    pub fn connect_peer_input(&mut self) {
        let raw = self.peer_connect_input.trim().to_string();
        if raw.is_empty() {
            return;
        }
        let safe_raw = sanitize_display_text(&redact_url_secrets(&raw));
        self.push_p2p_log(format!("[INFO] Connecting to peer '{safe_raw}'..."));
        self.status_message = format!("⏳ P2P: {safe_raw}...");
        self.spawn_peer_sync(raw);
    }

    pub fn trigger_lan_discovery(&mut self) {
        self.push_p2p_log("[INFO] Scanning LAN for rust-audio-p2p peers...".to_string());
        let client = self.p2p_client.clone();
        let tx = self.async_tx.clone();
        tokio::spawn(async move {
            let found = client.discover_lan_peers(Duration::from_millis(900)).await;
            let _ = tx.send(AsyncEvent::P2pLanDiscovered(found));
        });
    }

    pub fn spawn_peer_sync(&self, target: String) {
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
        mut peer: PairedPeer,
        catalog: RemoteCatalogDto,
    ) -> Result<()> {
        peer.node_name = sanitize_display_text(&peer.node_name);
        if let Some(existing) = self
            .paired_peers
            .iter_mut()
            .find(|p| p.node_id == peer.node_id)
        {
            *existing = peer.clone();
        } else {
            self.paired_peers.push(peer.clone());
        }

        let clean_node_name = sanitize_display_text(&catalog.node_name);
        let track_count = catalog.tracks.len().min(5000);
        let playlist_count = catalog.playlists.len().min(100);

        {
            let guard = self.db.lock().expect("db mutex poisoned");
            guard.save_paired_peers(&self.paired_peers)?;

            let mut remote_to_local: HashMap<i64, i64> = HashMap::new();
            for rt in catalog.tracks.iter().take(5000) {
                let track = Track {
                    id: 0,
                    title: sanitize_display_text(&rt.title),
                    artist: sanitize_display_text(&rt.artist),
                    album: sanitize_display_text(&rt.album),
                    year: rt.year,
                    track_number: rt.track_number,
                    duration_secs: rt.duration_secs,
                    file_size_bytes: rt.file_size_bytes,
                    format: sanitize_display_text(&rt.format),
                    cover_path: None,
                    source: TrackSource::Peer {
                        node_id: catalog.node_id.clone(),
                        node_name: clean_node_name.clone(),
                        remote_track_id: rt.id,
                        cached_path: None,
                    },
                    pinned: false,
                };
                if let Ok(local_id) = guard.upsert_track(&track) {
                    remote_to_local.insert(rt.id, local_id);
                }
            }

            for rpl in catalog.playlists.iter().take(100) {
                let clean_pl = sanitize_display_text(&rpl.name);
                let pl_name = format!("[P2P: {clean_node_name}] {clean_pl}");
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
            "[OK] Peer '{}' synced: {} tracks, {} playlists",
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
            self.push_p2p_log(format!("[ERR] Peer '{node_id}' not in paired peers"));
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
            "[INFO] Streaming '{}' from '{}'...",
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
