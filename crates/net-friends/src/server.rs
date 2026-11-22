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
use tokio::sync::watch;

use crate::protocol::{
    detect_lan_ip, verify_token_ct, NodeIdentity, NodeInfoDto, PeerTicket, RemoteCatalogDto,
    RemotePlaylistDto, RemoteTrackDto, DEFAULT_P2P_PORT, DISCOVER_REQ_MSG, DISCOVER_RESP_PREFIX,
    LAN_DISCOVERY_PORT, P2P_PORT_END,
};

const MAX_REQUEST_HEAD_BYTES: usize = 16 * 1024;
const STREAM_CHUNK_BYTES: usize = 64 * 1024;

pub struct FriendNodeServer {
    pub port: u16,
    pub lan_ip: String,
    pub identity: NodeIdentity,
    pub lan_share_enabled: Arc<AtomicBool>,
    shutdown_tx: watch::Sender<bool>,
}

impl Drop for FriendNodeServer {
    fn drop(&mut self) {
        let _ = self.shutdown_tx.send(true);
    }
}

impl FriendNodeServer {
    pub async fn start(db: Arc<Mutex<Database>>, identity: NodeIdentity) -> Result<Self> {
        Self::start_with_options(db, identity, None, Some(LAN_DISCOVERY_PORT)).await
    }

    pub async fn start_with_options(
        db: Arc<Mutex<Database>>,
        identity: NodeIdentity,
        port_override: Option<u16>,
        udp_discovery_port: Option<u16>,
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
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        // Spawn TCP HTTP server loop
        let tcp_db = Arc::clone(&db);
        let tcp_id = identity.clone();
        let tcp_lan_share = Arc::clone(&lan_share_enabled);
        let tcp_lan_ip = lan_ip.clone();
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
                        let db_c = Arc::clone(&tcp_db);
                        let id_c = tcp_id.clone();
                        let share_c = Arc::clone(&tcp_lan_share);
                        let ip_c = tcp_lan_ip.clone();
                        tokio::spawn(async move {
                            let _ = handle_connection(stream, db_c, id_c, share_c, ip_c, port).await;
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
            shutdown_tx,
        })
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

async fn handle_connection(
    mut stream: TcpStream,
    db: Arc<Mutex<Database>>,
    identity: NodeIdentity,
    lan_share_enabled: Arc<AtomicBool>,
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
    let path = raw_target.split('?').next().unwrap_or(raw_target);

    if method != "GET" {
        write_simple_response(
            &mut stream,
            "405 Method Not Allowed",
            "text/plain",
            b"Only GET supported",
        )
        .await?;
        return Ok(());
    }

    let mut token_hdr: Option<String> = None;
    let mut range_hdr: Option<String> = None;
    let mut host_hdr: Option<String> = None;

    for line in lines {
        if let Some((name, val)) = line.split_once(':') {
            let k = name.trim().to_ascii_lowercase();
            let v = val.trim().to_string();
            match k.as_str() {
                "x-peer-token" => token_hdr = Some(v),
                "range" => range_hdr = Some(v),
                "host" => host_hdr = Some(v),
                _ => {}
            }
        }
    }

    if path == "/api/v1/lan-ticket" {
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
        let endpoint = host_hdr
            .as_deref()
            .and_then(|h| {
                let clean = h.trim();
                (!clean.is_empty() && !clean.contains('/') && !clean.chars().any(char::is_whitespace))
                    .then(|| {
                        if clean.contains(':') {
                            clean.to_string()
                        } else {
                            format!("{clean}:{port}")
                        }
                    })
            })
            .unwrap_or_else(|| format!("{lan_ip}:{port}"));

        let ticket = PeerTicket {
            node_id: identity.node_id,
            node_name: identity.node_name,
            endpoint,
            auth_token: identity.auth_token,
        };
        let body = serde_json::to_vec(&ticket)?;
        write_simple_response(&mut stream, "200 OK", "application/json", &body).await?;
        return Ok(());
    }

    // All remaining endpoints require valid X-Peer-Token
    let supplied = token_hdr.as_deref().unwrap_or("");
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

    if path == "/api/v1/info" {
        let catalog = build_local_catalog(&db, &identity)?;
        let info = NodeInfoDto {
            node_id: identity.node_id,
            node_name: identity.node_name,
            endpoint: format!("{lan_ip}:{port}"),
            track_count: catalog.tracks.len(),
            playlist_count: catalog.playlists.len(),
        };
        let body = serde_json::to_vec(&info)?;
        write_simple_response(&mut stream, "200 OK", "application/json", &body).await?;
        return Ok(());
    }

    if path == "/api/v1/catalog" {
        let catalog = build_local_catalog(&db, &identity)?;
        let body = serde_json::to_vec(&catalog)?;
        write_simple_response(&mut stream, "200 OK", "application/json", &body).await?;
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

fn build_local_catalog(db: &Arc<Mutex<Database>>, identity: &NodeIdentity) -> Result<RemoteCatalogDto> {
    let guard = db
        .lock()
        .map_err(|_| anyhow::anyhow!("Database mutex poisoned"))?;

    let all_tracks = guard.list_tracks(5000, 0)?;
    let mut allowed_ids = HashSet::new();
    let mut tracks = Vec::new();

    for t in all_tracks {
        // Avoid echo loops re-exporting another peer's tracks, and only expose files existing locally.
        if matches!(t.source, TrackSource::Peer { .. }) {
            continue;
        }
        let Some(p) = t.source.playable_path() else {
            continue;
        };
        if !Path::new(p).exists() {
            continue;
        }
        allowed_ids.insert(t.id);
        tracks.push(RemoteTrackDto {
            id: t.id,
            title: t.title,
            artist: t.artist,
            album: t.album,
            year: t.year,
            track_number: t.track_number,
            duration_secs: t.duration_secs,
            file_size_bytes: t.file_size_bytes,
            format: t.format,
        });
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

async fn serve_track_stream(
    stream: &mut TcpStream,
    db: &Arc<Mutex<Database>>,
    track_id: i64,
    range_hdr: Option<&str>,
) -> Result<()> {
    enum LookupOutcome {
        OkPath(String),
        ErrMsg(&'static [u8]),
    }

    let lookup = {
        let guard = db
            .lock()
            .map_err(|_| anyhow::anyhow!("Database mutex poisoned"))?;
        match guard.get_track(track_id)? {
            None => LookupOutcome::ErrMsg(b"Track not found"),
            Some(track) if matches!(track.source, TrackSource::Peer { .. }) => {
                LookupOutcome::ErrMsg(b"Track not locally hosted")
            }
            Some(track) => match track.source.playable_path() {
                Some(p) => LookupOutcome::OkPath(p.to_string()),
                None => LookupOutcome::ErrMsg(b"Track file unavailable"),
            },
        }
    };

    let file_path = match lookup {
        LookupOutcome::OkPath(p) => p,
        LookupOutcome::ErrMsg(msg) => {
            write_simple_response(stream, "404 Not Found", "text/plain", msg).await?;
            return Ok(());
        }
    };

    let Ok(mut file) = tokio::fs::File::open(&file_path).await else {
        write_simple_response(stream, "404 Not Found", "text/plain", b"File missing on disk").await?;
        return Ok(());
    };
    let meta = file.metadata().await?;
    let total_len = meta.len();

    if total_len == 0 {
        write_simple_response(stream, "200 OK", "application/octet-stream", &[]).await?;
        return Ok(());
    }

    let (status, start, end) = match range_hdr.and_then(|h| parse_byte_range(h, total_len)) {
        Some(Some((s, e))) => ("206 Partial Content", s, e),
        Some(None) => {
            let hdr = format!(
                "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{total_len}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
            stream.write_all(hdr.as_bytes()).await?;
            return Ok(());
        }
        None => ("200 OK", 0, total_len - 1),
    };

    let content_len = end - start + 1;
    let mut header = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/octet-stream\r\nAccept-Ranges: bytes\r\nContent-Length: {content_len}\r\nConnection: close\r\n"
    );
    if status.starts_with("206") {
        header.push_str(&format!("Content-Range: bytes {start}-{end}/{total_len}\r\n"));
    }
    header.push_str("\r\n");
    stream.write_all(header.as_bytes()).await?;

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
        stream.write_all(&buf[..n]).await?;
        remaining -= n as u64;
    }
    stream.flush().await?;
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

async fn write_simple_response(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    body: &[u8],
) -> Result<()> {
    let header = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(header.as_bytes()).await?;
    if !body.is_empty() {
        stream.write_all(body).await?;
    }
    stream.flush().await?;
    Ok(())
}
