use std::collections::HashSet;
use std::io::SeekFrom;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use rust_audio_core::{Database, TrackSource};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::{mpsc, watch, Semaphore};

use crate::protocol::{
    detect_lan_ip, extract_query_param, track_to_remote_dto, verify_token_ct, NodeIdentity,
    NodeInfoDto, PeerTicket, RemoteCatalogDto, RemotePlaylistDto, RemoteTrackDto, WebPeerSummary,
    WebPlayerSnapshot, WebPlaylistSummary, WebRemoteCommand, WebStateDto, DEFAULT_P2P_PORT,
    DISCOVER_REQ_MSG, DISCOVER_RESP_PREFIX, LAN_DISCOVERY_PORT, P2P_PORT_END,
};
use crate::webui;

const MAX_REQUEST_HEAD_BYTES: usize = 16 * 1024;
const MAX_CONTROL_BODY_BYTES: usize = 64 * 1024;
const MAX_COVER_BYTES: u64 = 8 * 1024 * 1024;
const MAX_CONCURRENT_CONNECTIONS: usize = 64;
const STREAM_CHUNK_BYTES: usize = 64 * 1024;
const SECURITY_HEADERS: &str = "X-Content-Type-Options: nosniff\r\nX-Frame-Options: DENY\r\nReferrer-Policy: no-referrer\r\nContent-Security-Policy: default-src 'self'; img-src 'self' data:; media-src 'self'; style-src 'self' 'unsafe-inline'; script-src 'self' 'unsafe-inline'; frame-ancestors 'none'\r\n";

pub struct FriendNodeServer {
    pub port: u16,
    pub lan_ip: String,
    pub identity: NodeIdentity,
    pub lan_share_enabled: Arc<AtomicBool>,
    pub player_snapshot: Arc<Mutex<WebPlayerSnapshot>>,
    shutdown_tx: watch::Sender<bool>,
}

impl Drop for FriendNodeServer {
    fn drop(&mut self) {
        let _ = self.shutdown_tx.send(true);
    }
}

impl FriendNodeServer {
    pub async fn start(db: Arc<Mutex<Database>>, identity: NodeIdentity) -> Result<Self> {
        Self::start_with_control(db, identity, None).await
    }

    pub async fn start_with_control(
        db: Arc<Mutex<Database>>,
        identity: NodeIdentity,
        cmd_tx: Option<mpsc::UnboundedSender<WebRemoteCommand>>,
    ) -> Result<Self> {
        Self::start_with_full_options(db, identity, None, Some(LAN_DISCOVERY_PORT), cmd_tx).await
    }

    pub async fn start_with_options(
        db: Arc<Mutex<Database>>,
        identity: NodeIdentity,
        port_override: Option<u16>,
        udp_discovery_port: Option<u16>,
    ) -> Result<Self> {
        Self::start_with_full_options(db, identity, port_override, udp_discovery_port, None).await
    }

    pub async fn start_with_full_options(
        db: Arc<Mutex<Database>>,
        identity: NodeIdentity,
        port_override: Option<u16>,
        udp_discovery_port: Option<u16>,
        cmd_tx: Option<mpsc::UnboundedSender<WebRemoteCommand>>,
    ) -> Result<Self> {
        let listener = if let Some(port) = port_override {
            let bind_ip = if port == 0 {
                IpAddr::V4(Ipv4Addr::LOCALHOST)
            } else {
                IpAddr::V4(Ipv4Addr::UNSPECIFIED)
            };
            TcpListener::bind(SocketAddr::new(bind_ip, port))
                .await
                .with_context(|| format!("Failed to bind P2P TCP server on {bind_ip}:{port}"))?
        } else {
            bind_first_available_port(DEFAULT_P2P_PORT, P2P_PORT_END).await?
        };

        let port = listener.local_addr()?.port();
        let lan_ip = if port_override == Some(0) {
            "127.0.0.1".to_string()
        } else {
            detect_lan_ip()
        };

        let initial_lan_share = db
            .lock()
            .ok()
            .and_then(|g| g.get_setting("p2p_lan_share").ok().flatten())
            .map(|v| v != "0" && !v.eq_ignore_ascii_case("false"))
            .unwrap_or(true);
        let lan_share_enabled = Arc::new(AtomicBool::new(initial_lan_share));
        let player_snapshot = Arc::new(Mutex::new(WebPlayerSnapshot::default()));
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        // Spawn TCP HTTP server loop with bounded concurrency
        let tcp_db = Arc::clone(&db);
        let tcp_id = identity.clone();
        let tcp_lan_share = Arc::clone(&lan_share_enabled);
        let tcp_snapshot = Arc::clone(&player_snapshot);
        let tcp_lan_ip = lan_ip.clone();
        let conn_sem = Arc::new(Semaphore::new(MAX_CONCURRENT_CONNECTIONS));
        let mut tcp_shutdown = shutdown_rx.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = tcp_shutdown.changed() => {
                        if *tcp_shutdown.borrow() {
                            break;
                        }
                    }
                    accept_res = listener.accept() => {
                        let Ok((stream, _peer_addr)) = accept_res else {
                            break;
                        };
                        let Ok(permit) = Arc::clone(&conn_sem).try_acquire_owned() else {
                            drop(stream);
                            continue;
                        };
                        let db_c = Arc::clone(&tcp_db);
                        let id_c = tcp_id.clone();
                        let share_c = Arc::clone(&tcp_lan_share);
                        let snap_c = Arc::clone(&tcp_snapshot);
                        let cmd_c = cmd_tx.clone();
                        let ip_c = tcp_lan_ip.clone();
                        tokio::spawn(async move {
                            let _permit = permit;
                            let _ = handle_connection(
                                stream,
                                db_c,
                                id_c,
                                share_c,
                                snap_c,
                                cmd_c,
                                ip_c,
                                port,
                            )
                            .await;
                        });
                    }
                }
            }
        });

        // Optional UDP LAN discovery responder (best-effort if port is free)
        if let Some(udp_port) = udp_discovery_port {
            if let Ok(udp_sock) = UdpSocket::bind(SocketAddr::from(([0, 0, 0, 0], udp_port))).await {
                let udp_id = identity.clone();
                let udp_share = Arc::clone(&lan_share_enabled);
                let udp_ip = lan_ip.clone();
                let mut udp_shutdown = shutdown_rx;
                tokio::spawn(async move {
                    let mut buf = [0u8; 256];
                    loop {
                        tokio::select! {
                            _ = udp_shutdown.changed() => {
                                if *udp_shutdown.borrow() {
                                    break;
                                }
                            }
                            recv_res = udp_sock.recv_from(&mut buf) => {
                                let Ok((n, src)) = recv_res else {
                                    continue;
                                };
                                if buf[..n].starts_with(DISCOVER_REQ_MSG) && udp_share.load(Ordering::Relaxed) {
                                    let ticket = PeerTicket {
                                        node_id: udp_id.node_id.clone(),
                                        node_name: udp_id.node_name.clone(),
                                        endpoint: format!("{udp_ip}:{port}"),
                                        auth_token: udp_id.auth_token.clone(),
                                    };
                                    let reply = format!("{DISCOVER_RESP_PREFIX}{}", ticket.to_uri());
                                    let _ = udp_sock.send_to(reply.as_bytes(), src).await;
                                }
                            }
                        }
                    }
                });
            }
        }

        Ok(Self {
            port,
            lan_ip,
            identity,
            lan_share_enabled,
            player_snapshot,
            shutdown_tx,
        })
    }

    pub fn update_player_snapshot(&self, snap: WebPlayerSnapshot) {
        if let Ok(mut guard) = self.player_snapshot.lock() {
            *guard = snap;
        }
    }

    pub fn web_url(&self) -> String {
        format!("http://{}:{}", self.lan_ip, self.port)
    }

    pub fn ticket(&self) -> PeerTicket {
        PeerTicket {
            node_id: self.identity.node_id.clone(),
            node_name: self.identity.node_name.clone(),
            endpoint: format!("{}:{}", self.lan_ip, self.port),
            auth_token: self.identity.auth_token.clone(),
        }
    }

    pub fn is_lan_share_enabled(&self) -> bool {
        self.lan_share_enabled.load(Ordering::Relaxed)
    }

    pub fn set_lan_share_enabled(&self, db: &Database, enabled: bool) {
        self.lan_share_enabled.store(enabled, Ordering::Relaxed);
        let _ = db.set_setting("p2p_lan_share", if enabled { "1" } else { "0" });
    }

    pub fn shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
    }
}

async fn bind_first_available_port(start: u16, end: u16) -> Result<TcpListener> {
    for port in start..=end {
        if let Ok(listener) = TcpListener::bind(SocketAddr::from(([0, 0, 0, 0], port))).await {
            return Ok(listener);
        }
    }
    Ok(TcpListener::bind(SocketAddr::from(([0, 0, 0, 0], 0))).await?)
}

#[allow(clippy::too_many_arguments)]
async fn handle_connection(
    mut stream: TcpStream,
    db: Arc<Mutex<Database>>,
    identity: NodeIdentity,
    lan_share_enabled: Arc<AtomicBool>,
    player_snapshot: Arc<Mutex<WebPlayerSnapshot>>,
    cmd_tx: Option<mpsc::UnboundedSender<WebRemoteCommand>>,
    lan_ip: String,
    port: u16,
) -> Result<()> {
    let mut head_buf = Vec::with_capacity(1024);
    let mut tmp = [0u8; 1024];

    let header_end = loop {
        let read_fut = stream.read(&mut tmp);
        let n = tokio::time::timeout(Duration::from_secs(5), read_fut)
            .await
            .map_err(|_| anyhow::anyhow!("Request header timeout"))??;
        if n == 0 {
            return Ok(());
        }
        head_buf.extend_from_slice(&tmp[..n]);
        if let Some(pos) = head_buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos;
        }
        if head_buf.len() > MAX_REQUEST_HEAD_BYTES {
            write_simple_response(&mut stream, "400 Bad Request", "text/plain", b"Headers too large")
                .await?;
            return Ok(());
        }
    };

    let head_str = String::from_utf8_lossy(&head_buf[..header_end]);
    let mut lines = head_str.lines();
    let Some(request_line) = lines.next() else {
        write_simple_response(&mut stream, "400 Bad Request", "text/plain", b"Empty request").await?;
        return Ok(());
    };
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let raw_target = parts.next().unwrap_or("");
    let (path, query_opt) = match raw_target.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (raw_target, None),
    };

    if method != "GET" && method != "POST" {
        write_simple_response(
            &mut stream,
            "405 Method Not Allowed",
            "text/plain",
            b"Only GET/POST supported",
        )
        .await?;
        return Ok(());
    }

    let mut token_hdr: Option<String> = None;
    let mut range_hdr: Option<String> = None;
    let mut host_hdr: Option<String> = None;
    let mut origin_hdr: Option<String> = None;
    let mut sec_fetch_site_hdr: Option<String> = None;
    let mut content_len: usize = 0;

    for line in lines {
        if let Some((name, val)) = line.split_once(':') {
            let k = name.trim().to_ascii_lowercase();
            let v = val.trim().to_string();
            match k.as_str() {
                "x-peer-token" => token_hdr = Some(v),
                "range" => range_hdr = Some(v),
                "host" => host_hdr = Some(v),
                "origin" => origin_hdr = Some(v),
                "sec-fetch-site" => sec_fetch_site_hdr = Some(v),
                "content-length" => content_len = v.parse::<usize>().unwrap_or(0),
                _ => {}
            }
        }
    }

    if !is_allowed_host_header(host_hdr.as_deref(), &lan_ip) {
        write_simple_response(
            &mut stream,
            "403 Forbidden",
            "text/plain",
            b"Host header rejected (DNS rebinding protection)",
        )
        .await?;
        return Ok(());
    }

    if !is_allowed_browser_origin(
        path,
        origin_hdr.as_deref(),
        sec_fetch_site_hdr.as_deref(),
        &lan_ip,
    ) {
        write_simple_response(
            &mut stream,
            "403 Forbidden",
            "text/plain",
            b"Cross-site browser request rejected",
        )
        .await?;
        return Ok(());
    }

    let req_body = if method == "POST" && content_len > 0 {
        if content_len > MAX_CONTROL_BODY_BYTES {
            write_simple_response(
                &mut stream,
                "413 Payload Too Large",
                "text/plain",
                b"Body too large",
            )
            .await?;
            return Ok(());
        }
        let already = &head_buf[header_end + 4..];
        let mut body = Vec::with_capacity(content_len);
        body.extend_from_slice(&already[..already.len().min(content_len)]);
        while body.len() < content_len {
            let mut chunk = [0u8; 4096];
            let need = (content_len - body.len()).min(chunk.len());
            let n = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut chunk[..need]))
                .await
                .map_err(|_| anyhow::anyhow!("Body read timeout"))??;
            if n == 0 {
                break;
            }
            body.extend_from_slice(&chunk[..n]);
        }
        body
    } else {
        Vec::new()
    };

    if method == "GET" {
        match path {
            "/" | "/index.html" => {
                write_simple_response(
                    &mut stream,
                    "200 OK",
                    "text/html; charset=utf-8",
                    webui::INDEX_HTML.as_bytes(),
                )
                .await?;
                return Ok(());
            }
            "/manifest.json" => {
                write_simple_response(
                    &mut stream,
                    "200 OK",
                    "application/manifest+json; charset=utf-8",
                    webui::MANIFEST_JSON.as_bytes(),
                )
                .await?;
                return Ok(());
            }
            "/icon.svg" => {
                write_simple_response(
                    &mut stream,
                    "200 OK",
                    "image/svg+xml",
                    webui::ICON_SVG.as_bytes(),
                )
                .await?;
                return Ok(());
            }
            _ => {}
        }
    }

    let effective_endpoint = host_hdr
        .as_deref()
        .and_then(|h| extract_host_component(h.trim()))
        .filter(|host| {
            host.eq_ignore_ascii_case("localhost")
                || *host == lan_ip
                || host.parse::<IpAddr>().is_ok()
        })
        .map(|host| {
            if host.contains(':') {
                format!("[{host}]:{port}")
            } else {
                format!("{host}:{port}")
            }
        })
        .unwrap_or_else(|| format!("{lan_ip}:{port}"));

    if method == "GET" && path == "/api/v1/lan-ticket" {
        if !lan_share_enabled.load(Ordering::Relaxed) {
            write_simple_response(
                &mut stream,
                "403 Forbidden",
                "text/plain",
                b"LAN share disabled on this node",
            )
            .await?;
            return Ok(());
        }

        let ticket = PeerTicket {
            node_id: identity.node_id,
            node_name: identity.node_name,
            endpoint: effective_endpoint,
            auth_token: identity.auth_token,
        };
        let body = serde_json::to_vec(&ticket)?;
        write_simple_response(&mut stream, "200 OK", "application/json", &body).await?;
        return Ok(());
    }

    // All remaining endpoints require valid token (X-Peer-Token header or ?token=... query param)
    let query_tok = query_opt.and_then(|q| extract_query_param(q, "token"));
    let supplied = token_hdr
        .as_deref()
        .or(query_tok.as_deref())
        .unwrap_or("");
    if !verify_token_ct(&identity.auth_token, supplied) {
        write_simple_response(
            &mut stream,
            "403 Forbidden",
            "text/plain",
            b"Unauthorized peer token",
        )
        .await?;
        return Ok(());
    }

    if method == "POST" && path == "/api/v1/control" {
        let Ok(cmd) = serde_json::from_slice::<WebRemoteCommand>(&req_body) else {
            write_simple_response(&mut stream, "400 Bad Request", "text/plain", b"Invalid JSON command")
                .await?;
            return Ok(());
        };
        apply_web_control_command(&db, &cmd, cmd_tx.as_ref())?;
        write_simple_response(&mut stream, "200 OK", "application/json", br#"{"ok":true}"#).await?;
        return Ok(());
    }

    if method != "GET" {
        write_simple_response(&mut stream, "404 Not Found", "text/plain", b"Not found").await?;
        return Ok(());
    }

    if path == "/api/v1/info" {
        let catalog = build_local_catalog(&db, &identity, false)?;
        let info = NodeInfoDto {
            node_id: identity.node_id,
            node_name: identity.node_name,
            endpoint: effective_endpoint,
            track_count: catalog.tracks.len(),
            playlist_count: catalog.playlists.len(),
        };
        let body = serde_json::to_vec(&info)?;
        write_simple_response(&mut stream, "200 OK", "application/json", &body).await?;
        return Ok(());
    }

    if path == "/api/v1/catalog" {
        let include_peer = query_opt
            .and_then(|q| extract_query_param(q, "include_peer"))
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        let catalog = build_local_catalog(&db, &identity, include_peer)?;
        let body = serde_json::to_vec(&catalog)?;
        write_simple_response(&mut stream, "200 OK", "application/json", &body).await?;
        return Ok(());
    }

    if path == "/api/v1/search" {
        let q = query_opt
            .and_then(|qs| extract_query_param(qs, "q"))
            .unwrap_or_default();
        let results = search_playable_tracks(&db, &q)?;
        let body = serde_json::to_vec(&results)?;
        write_simple_response(&mut stream, "200 OK", "application/json", &body).await?;
        return Ok(());
    }

    if path == "/api/v1/state" {
        let state = build_web_state(
            &db,
            &identity,
            &effective_endpoint,
            lan_share_enabled.load(Ordering::Relaxed),
            &player_snapshot,
        )?;
        let body = serde_json::to_vec(&state)?;
        write_simple_response(&mut stream, "200 OK", "application/json", &body).await?;
        return Ok(());
    }

    if let Some(id_str) = path.strip_prefix("/api/v1/cover/") {
        let Ok(track_id) = id_str.parse::<i64>() else {
            write_simple_response(&mut stream, "400 Bad Request", "text/plain", b"Invalid track id")
                .await?;
            return Ok(());
        };
        serve_track_cover(&mut stream, &db, track_id).await?;
        return Ok(());
    }

    if let Some(id_str) = path.strip_prefix("/api/v1/stream/") {
        let Ok(track_id) = id_str.parse::<i64>() else {
            write_simple_response(&mut stream, "400 Bad Request", "text/plain", b"Invalid track id")
                .await?;
            return Ok(());
        };
        serve_track_stream(&mut stream, &db, track_id, range_hdr.as_deref()).await?;
        return Ok(());
    }

    write_simple_response(&mut stream, "404 Not Found", "text/plain", b"Not found").await?;
    Ok(())
}

fn is_local_file_ready(source: &TrackSource) -> bool {
    source
        .playable_path()
        .map(|p| Path::new(p).exists())
        .unwrap_or(false)
}

fn build_local_catalog(
    db: &Arc<Mutex<Database>>,
    identity: &NodeIdentity,
    include_peer: bool,
) -> Result<RemoteCatalogDto> {
    let guard = db
        .lock()
        .map_err(|_| anyhow::anyhow!("Database mutex poisoned"))?;

    let limit = if include_peer { 25_000 } else { 5_000 };
    let all_tracks = guard.list_tracks(limit, 0)?;
    let mut allowed_ids = HashSet::new();
    let mut tracks = Vec::new();

    for t in all_tracks {
        let is_peer = matches!(t.source, TrackSource::Peer { .. });
        // Avoid echo loops re-exporting another peer's tracks on P2P sync, while allowing Web UI view with include_peer=1.
        if !include_peer && is_peer {
            continue;
        }
        // Web UI (include_peer=true) must list uncached P2P tracks too so 1. Library search & play work across connected nodes.
        if !is_peer && !is_local_file_ready(&t.source) {
            continue;
        }
        allowed_ids.insert(t.id);
        tracks.push(track_to_remote_dto(&t));
    }

    let raw_playlists = guard.list_playlists()?;
    let mut playlists = Vec::new();
    for pl in raw_playlists {
        let pl_tracks = guard.get_playlist_tracks(pl.id)?;
        let track_ids: Vec<i64> = pl_tracks
            .into_iter()
            .filter(|t| allowed_ids.contains(&t.id))
            .map(|t| t.id)
            .collect();
        if !track_ids.is_empty() {
            playlists.push(RemotePlaylistDto {
                name: pl.name,
                pinned: pl.pinned,
                track_ids,
            });
        }
    }

    Ok(RemoteCatalogDto {
        node_id: identity.node_id.clone(),
        node_name: identity.node_name.clone(),
        tracks,
        playlists,
    })
}

fn search_playable_tracks(db: &Arc<Mutex<Database>>, query: &str) -> Result<Vec<RemoteTrackDto>> {
    let guard = db
        .lock()
        .map_err(|_| anyhow::anyhow!("Database mutex poisoned"))?;
    let rows = guard.search_tracks_fts(query, 300)?;
    Ok(rows
        .into_iter()
        .filter(|t| matches!(t.source, TrackSource::Peer { .. }) || is_local_file_ready(&t.source))
        .map(|t| track_to_remote_dto(&t))
        .collect())
}

fn build_web_state(
    db: &Arc<Mutex<Database>>,
    identity: &NodeIdentity,
    endpoint: &str,
    lan_share: bool,
    player_snapshot: &Arc<Mutex<WebPlayerSnapshot>>,
) -> Result<WebStateDto> {
    let ticket_uri = PeerTicket {
        node_id: identity.node_id.clone(),
        node_name: identity.node_name.clone(),
        endpoint: endpoint.to_string(),
        auth_token: identity.auth_token.clone(),
    }
    .to_uri();

    let (peers, playlists) = {
        let guard = db
            .lock()
            .map_err(|_| anyhow::anyhow!("Database mutex poisoned"))?;
        let peers: Vec<WebPeerSummary> = guard
            .load_paired_peers()?
            .into_iter()
            .map(|p| WebPeerSummary {
                node_id: p.node_id,
                node_name: p.node_name,
                endpoint: p.endpoint,
                track_count: p.track_count,
                playlist_count: p.playlist_count,
            })
            .collect();
        let mut playlists = Vec::new();
        for pl in guard.list_playlists()? {
            let track_ids: Vec<i64> = guard
                .get_playlist_tracks(pl.id)?
                .into_iter()
                .map(|t| t.id)
                .collect();
            playlists.push(WebPlaylistSummary {
                id: pl.id,
                name: pl.name,
                pinned: pl.pinned,
                track_ids,
            });
        }
        (peers, playlists)
    };

    let player = player_snapshot
        .lock()
        .map(|g| g.clone())
        .unwrap_or_default();

    Ok(WebStateDto {
        node_id: identity.node_id.clone(),
        node_name: identity.node_name.clone(),
        endpoint: endpoint.to_string(),
        ticket_uri,
        lan_share,
        player,
        peers,
        playlists,
    })
}

fn apply_web_control_command(
    db: &Arc<Mutex<Database>>,
    cmd: &WebRemoteCommand,
    cmd_tx: Option<&mpsc::UnboundedSender<WebRemoteCommand>>,
) -> Result<()> {
    let forwarded = match cmd {
        WebRemoteCommand::TogglePin { track_id } => {
            let guard = db
                .lock()
                .map_err(|_| anyhow::anyhow!("Database mutex poisoned"))?;
            let next = guard.toggle_track_pin(*track_id).unwrap_or(false);
            WebRemoteCommand::SyncRefresh {
                message: Some(format!("Web pin -> {next} (#{track_id})")),
            }
        }
        WebRemoteCommand::CreatePlaylist { name, track_id } => {
            let clean = name.trim();
            if !clean.is_empty() {
                let guard = db
                    .lock()
                    .map_err(|_| anyhow::anyhow!("Database mutex poisoned"))?;
                let pl_id = guard.create_playlist(clean)?;
                if let Some(tid) = track_id {
                    guard.add_track_to_playlist(pl_id, *tid)?;
                }
            }
            WebRemoteCommand::SyncRefresh {
                message: Some(format!("Web playlist: {}", name.trim())),
            }
        }
        WebRemoteCommand::CreatePlaylistWithTracks { name, track_ids } => {
            let clean = name.trim();
            if !clean.is_empty() {
                let guard = db
                    .lock()
                    .map_err(|_| anyhow::anyhow!("Database mutex poisoned"))?;
                let pl_id = guard.create_playlist(clean)?;
                if !track_ids.is_empty() {
                    guard.add_tracks_to_playlist(pl_id, track_ids)?;
                }
            }
            WebRemoteCommand::SyncRefresh {
                message: Some(format!(
                    "Web playlist: {} (+{})",
                    name.trim(),
                    track_ids.len()
                )),
            }
        }
        WebRemoteCommand::RenamePlaylist {
            playlist_id,
            new_name,
        } => {
            let clean = new_name.trim();
            if !clean.is_empty() {
                let guard = db
                    .lock()
                    .map_err(|_| anyhow::anyhow!("Database mutex poisoned"))?;
                guard.rename_playlist(*playlist_id, clean)?;
            }
            WebRemoteCommand::SyncRefresh {
                message: Some(format!("Web rename playlist #{playlist_id} -> {}", clean)),
            }
        }
        WebRemoteCommand::DeletePlaylist { playlist_id } => {
            let guard = db
                .lock()
                .map_err(|_| anyhow::anyhow!("Database mutex poisoned"))?;
            guard.delete_playlist(*playlist_id)?;
            WebRemoteCommand::SyncRefresh {
                message: Some(format!("Web delete playlist #{playlist_id}")),
            }
        }
        WebRemoteCommand::AddToPlaylist {
            playlist_id,
            track_id,
        } => {
            let guard = db
                .lock()
                .map_err(|_| anyhow::anyhow!("Database mutex poisoned"))?;
            guard.add_track_to_playlist(*playlist_id, *track_id)?;
            WebRemoteCommand::SyncRefresh {
                message: Some(format!("Web +playlist (#{track_id})")),
            }
        }
        WebRemoteCommand::AddTracksToPlaylist {
            playlist_id,
            track_ids,
        } => {
            let guard = db
                .lock()
                .map_err(|_| anyhow::anyhow!("Database mutex poisoned"))?;
            let added = guard.add_tracks_to_playlist(*playlist_id, track_ids)?;
            WebRemoteCommand::SyncRefresh {
                message: Some(format!("Web +playlist (+{added} tracks)")),
            }
        }
        WebRemoteCommand::RemoveFromPlaylist {
            playlist_id,
            track_id,
        } => {
            let guard = db
                .lock()
                .map_err(|_| anyhow::anyhow!("Database mutex poisoned"))?;
            guard.remove_track_from_playlist(*playlist_id, *track_id)?;
            WebRemoteCommand::SyncRefresh {
                message: Some(format!("Web -playlist (#{track_id})")),
            }
        }
        other => other.clone(),
    };

    if let Some(tx) = cmd_tx {
        let _ = tx.send(forwarded);
    }
    Ok(())
}

async fn serve_track_cover(
    stream: &mut TcpStream,
    db: &Arc<Mutex<Database>>,
    track_id: i64,
) -> Result<()> {
    let cover_opt = {
        let guard = db
            .lock()
            .map_err(|_| anyhow::anyhow!("Database mutex poisoned"))?;
        guard.get_track(track_id)?.and_then(|t| t.cover_path)
    };

    if let Some(cover_path) = cover_opt {
        let p = Path::new(&cover_path);
        if let Ok(meta) = tokio::fs::symlink_metadata(p).await {
            if !meta.file_type().is_symlink() && meta.len() > 0 && meta.len() <= MAX_COVER_BYTES {
                if let Ok(bytes) = tokio::fs::read(p).await {
                    let mime = match p
                        .extension()
                        .and_then(|e| e.to_str())
                        .unwrap_or("")
                        .to_ascii_lowercase()
                        .as_str()
                    {
                        "png" => "image/png",
                        "webp" => "image/webp",
                        "svg" => "image/svg+xml",
                        _ => "image/jpeg",
                    };
                    write_simple_response(stream, "200 OK", mime, &bytes).await?;
                    return Ok(());
                }
            }
        }
    }

    write_simple_response(stream, "200 OK", "image/svg+xml", webui::ICON_SVG.as_bytes()).await?;
    Ok(())
}

async fn serve_track_stream(
    stream: &mut TcpStream,
    db: &Arc<Mutex<Database>>,
    track_id: i64,
    range_hdr: Option<&str>,
) -> Result<()> {
    enum LookupOutcome {
        OkPath(String),
        FetchRemote {
            track: rust_audio_core::Track,
            peer: rust_audio_core::PairedPeer,
            remote_track_id: i64,
        },
        ErrMsg(&'static [u8]),
    }

    let lookup = {
        let guard = db
            .lock()
            .map_err(|_| anyhow::anyhow!("Database mutex poisoned"))?;
        match guard.get_track(track_id)? {
            None => LookupOutcome::ErrMsg(b"Track not found"),
            Some(track) => {
                if let Some(p) = track.source.playable_path() {
                    if Path::new(p).exists() {
                        LookupOutcome::OkPath(p.to_string())
                    } else if let TrackSource::Peer {
                        ref node_id,
                        remote_track_id,
                        ..
                    } = track.source
                    {
                        match guard
                            .load_paired_peers()?
                            .into_iter()
                            .find(|pr| &pr.node_id == node_id)
                        {
                            Some(peer) => LookupOutcome::FetchRemote {
                                track,
                                peer,
                                remote_track_id,
                            },
                            None => LookupOutcome::ErrMsg(b"Remote peer not paired"),
                        }
                    } else {
                        LookupOutcome::OkPath(p.to_string())
                    }
                } else if let TrackSource::Peer {
                    ref node_id,
                    remote_track_id,
                    ..
                } = track.source
                {
                    match guard
                        .load_paired_peers()?
                        .into_iter()
                        .find(|pr| &pr.node_id == node_id)
                    {
                        Some(peer) => LookupOutcome::FetchRemote {
                            track,
                            peer,
                            remote_track_id,
                        },
                        None => LookupOutcome::ErrMsg(b"Remote peer not paired"),
                    }
                } else {
                    LookupOutcome::ErrMsg(b"Track file unavailable")
                }
            }
        }
    };

    let file_path = match lookup {
        LookupOutcome::OkPath(p) => p,
        LookupOutcome::FetchRemote {
            mut track,
            peer,
            remote_track_id,
        } => {
            let cache_dir = rust_audio_core::AppPaths::resolve_default().cache_dir;
            let client = crate::client::FriendNodeClient::new();
            let Ok(downloaded) = client
                .download_peer_track_to_cache(&peer, remote_track_id, &track.format, &cache_dir)
                .await
            else {
                write_simple_response(
                    stream,
                    "502 Bad Gateway",
                    "text/plain",
                    b"Failed fetching track from remote P2P node",
                )
                .await?;
                return Ok(());
            };
            let downloaded_str = downloaded.to_string_lossy().to_string();
            let file_size = tokio::fs::metadata(&downloaded)
                .await
                .map(|m| m.len())
                .unwrap_or(0);
            if let TrackSource::Peer {
                ref mut cached_path,
                ..
            } = track.source
            {
                *cached_path = Some(downloaded_str.clone());
            }
            if let Ok(guard) = db.lock() {
                let _ = guard.upsert_track(&track);
                let _ = guard.upsert_cache_entry(
                    &track.source.unique_key(),
                    Some(track.id),
                    &downloaded_str,
                    file_size,
                    track.pinned,
                );
            }
            downloaded_str
        }
        LookupOutcome::ErrMsg(msg) => {
            write_simple_response(stream, "404 Not Found", "text/plain", msg).await?;
            return Ok(());
        }
    };

    if !is_supported_audio_ext(&file_path) {
        write_simple_response(stream, "403 Forbidden", "text/plain", b"Unsupported audio type")
            .await?;
        return Ok(());
    }

    let Ok(smeta) = tokio::fs::symlink_metadata(&file_path).await else {
        write_simple_response(stream, "404 Not Found", "text/plain", b"File missing on disk").await?;
        return Ok(());
    };
    if !smeta.is_file() || smeta.file_type().is_symlink() {
        write_simple_response(stream, "403 Forbidden", "text/plain", b"Symlinks not allowed").await?;
        return Ok(());
    }

    let Ok(mut file) = tokio::fs::File::open(&file_path).await else {
        write_simple_response(stream, "404 Not Found", "text/plain", b"File missing on disk").await?;
        return Ok(());
    };
    let meta = file.metadata().await?;
    let total_len = meta.len();

    if total_len == 0 {
        write_simple_response(stream, "200 OK", audio_mime_type(&file_path), &[]).await?;
        return Ok(());
    }

    let (status, start, end) = match range_hdr.and_then(|h| parse_byte_range(h, total_len)) {
        Some(Some((s, e))) => ("206 Partial Content", s, e),
        Some(None) => {
            let hdr = format!(
                "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{total_len}\r\n{SECURITY_HEADERS}Content-Length: 0\r\nConnection: close\r\n\r\n"
            );
            stream.write_all(hdr.as_bytes()).await?;
            return Ok(());
        }
        None => ("200 OK", 0, total_len - 1),
    };

    let content_len = end - start + 1;
    let mime = audio_mime_type(&file_path);
    let mut header = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {mime}\r\nAccept-Ranges: bytes\r\n{SECURITY_HEADERS}Content-Length: {content_len}\r\nConnection: close\r\n"
    );
    if status.starts_with("206") {
        header.push_str(&format!("Content-Range: bytes {start}-{end}/{total_len}\r\n"));
    }
    header.push_str("\r\n");
    tokio::time::timeout(Duration::from_secs(10), stream.write_all(header.as_bytes()))
        .await
        .map_err(|_| anyhow::anyhow!("Stream header write timeout"))??;

    if start > 0 {
        file.seek(SeekFrom::Start(start)).await?;
    }

    let mut remaining = content_len;
    let mut buf = vec![0u8; STREAM_CHUNK_BYTES];
    while remaining > 0 {
        let to_read = (remaining as usize).min(buf.len());
        let n = file.read(&mut buf[..to_read]).await?;
        if n == 0 {
            break;
        }
        tokio::time::timeout(Duration::from_secs(10), stream.write_all(&buf[..n]))
            .await
            .map_err(|_| anyhow::anyhow!("Stream chunk write timeout"))??;
        remaining -= n as u64;
    }
    tokio::time::timeout(Duration::from_secs(10), stream.flush())
        .await
        .map_err(|_| anyhow::anyhow!("Stream flush timeout"))??;
    Ok(())
}

/// Returns:
/// - `None` if no valid `bytes=` range prefix
/// - `Some(None)` if range is syntactically present but invalid / unsatisfiable
/// - `Some(Some((start, end)))` if valid inclusive byte range
fn parse_byte_range(hdr: &str, total_len: u64) -> Option<Option<(u64, u64)>> {
    let spec = hdr.trim().strip_prefix("bytes=")?;
    let first = spec.split(',').next()?.trim();
    let (s_str, e_str) = first.split_once('-')?;
    if s_str.is_empty() {
        return Some(None);
    }
    let Ok(start) = s_str.parse::<u64>() else {
        return Some(None);
    };
    if start >= total_len {
        return Some(None);
    }
    let end = if e_str.trim().is_empty() {
        total_len - 1
    } else {
        let Ok(parsed_end) = e_str.trim().parse::<u64>() else {
            return Some(None);
        };
        if parsed_end < start {
            return Some(None);
        }
        parsed_end.min(total_len - 1)
    };
    Some(Some((start, end)))
}

fn extract_host_component(raw: &str) -> Option<&str> {
    if raw.is_empty() || raw.contains('/') || raw.contains('@') || raw.chars().any(char::is_whitespace) {
        return None;
    }
    if raw.starts_with('[') {
        let end = raw.find(']')?;
        let host = &raw[1..end];
        let rest = &raw[end + 1..];
        if !rest.is_empty() {
            let port_str = rest.strip_prefix(':')?;
            if port_str.parse::<u16>().is_err() {
                return None;
            }
        }
        return Some(host);
    }
    if let Some((h, port_str)) = raw.rsplit_once(':') {
        if !h.contains(':') && port_str.parse::<u16>().is_ok() {
            return (!h.is_empty()).then_some(h);
        }
    }
    Some(raw)
}

fn is_allowed_host_literal(host: &str, lan_ip: &str) -> bool {
    host.eq_ignore_ascii_case("localhost") || host == lan_ip || host.parse::<IpAddr>().is_ok()
}

fn is_allowed_host_header(host_hdr: Option<&str>, lan_ip: &str) -> bool {
    match host_hdr.map(str::trim).filter(|s| !s.is_empty()) {
        None => true,
        Some(h) => extract_host_component(h)
            .map(|host| is_allowed_host_literal(host, lan_ip))
            .unwrap_or(false),
    }
}

fn is_allowed_browser_origin(
    path: &str,
    origin_hdr: Option<&str>,
    sec_fetch_site: Option<&str>,
    lan_ip: &str,
) -> bool {
    if path.starts_with("/api/v1/") {
        if let Some(sfs) = sec_fetch_site.map(str::trim) {
            if sfs.eq_ignore_ascii_case("cross-site") {
                return false;
            }
        }
    }
    if let Some(orig) = origin_hdr.map(str::trim).filter(|s| !s.is_empty()) {
        let Some(rest) = orig
            .strip_prefix("http://")
            .or_else(|| orig.strip_prefix("https://"))
        else {
            return false;
        };
        let authority = rest.split('/').next().unwrap_or("");
        return extract_host_component(authority)
            .map(|host| is_allowed_host_literal(host, lan_ip))
            .unwrap_or(false);
    }
    true
}

fn is_supported_audio_ext(path: &str) -> bool {
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(
        ext.as_str(),
        "flac" | "mp3" | "ogg" | "oga" | "opus" | "m4a" | "mp4" | "aac" | "wav"
    )
}

fn audio_mime_type(path: &str) -> &'static str {
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "flac" => "audio/flac",
        "mp3" => "audio/mpeg",
        "ogg" | "oga" => "audio/ogg",
        "opus" => "audio/opus",
        "m4a" | "mp4" => "audio/mp4",
        "aac" => "audio/aac",
        "wav" => "audio/wav",
        _ => "application/octet-stream",
    }
}

async fn write_simple_response(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    body: &[u8],
) -> Result<()> {
    let header = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\n{SECURITY_HEADERS}Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(header.as_bytes()).await?;
    if !body.is_empty() {
        stream.write_all(body).await?;
    }
    stream.flush().await?;
    Ok(())
}
