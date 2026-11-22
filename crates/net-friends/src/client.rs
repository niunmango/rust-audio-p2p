use std::collections::HashSet;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use rust_audio_core::PairedPeer;
use tokio::io::AsyncWriteExt;
use tokio::net::UdpSocket;

use crate::protocol::{
    sanitize_ext, sanitize_id, PeerTicket, RemoteCatalogDto, DISCOVER_REQ_MSG,
    DISCOVER_RESP_PREFIX, LAN_DISCOVERY_PORT,
};

const MAX_CATALOG_JSON_BYTES: usize = 16 * 1024 * 1024;
const MAX_STREAM_DOWNLOAD_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Clone)]
pub struct FriendNodeClient {
    http: reqwest::Client,
}

impl Default for FriendNodeClient {
    fn default() -> Self {
        Self::new()
    }
}

impl FriendNodeClient {
    pub fn new() -> Self {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(120))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self { http }
    }

    pub async fn discover_lan_peers(&self, timeout: Duration) -> Vec<PeerTicket> {
        self.discover_lan_peers_on_port(LAN_DISCOVERY_PORT, timeout).await
    }

    pub async fn discover_lan_peers_on_port(
        &self,
        discovery_port: u16,
        timeout: Duration,
    ) -> Vec<PeerTicket> {
        let Ok(sock) = UdpSocket::bind("0.0.0.0:0").await else {
            return Vec::new();
        };
        let _ = sock.set_broadcast(true);

        let bcast = SocketAddr::from(([255, 255, 255, 255], discovery_port));
        let local = SocketAddr::from(([127, 0, 0, 1], discovery_port));
        let _ = sock.send_to(DISCOVER_REQ_MSG, bcast).await;
        let _ = sock.send_to(DISCOVER_REQ_MSG, local).await;

        let mut seen = HashSet::new();
        let mut discovered = Vec::new();
        let mut buf = [0u8; 2048];
        let deadline = Instant::now() + timeout;

        loop {
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            let rem = deadline - now;
            match tokio::time::timeout(rem, sock.recv_from(&mut buf)).await {
                Ok(Ok((n, _src))) => {
                    let msg = String::from_utf8_lossy(&buf[..n]);
                    if let Some(uri) = msg.trim().strip_prefix(DISCOVER_RESP_PREFIX) {
                        if let Ok(ticket) = PeerTicket::parse(uri) {
                            let key = (ticket.node_id.clone(), ticket.endpoint.clone());
                            if seen.insert(key) {
                                discovered.push(ticket);
                            }
                        }
                    }
                }
                _ => break,
            }
        }
        discovered
    }

    pub async fn resolve_and_sync_peer(&self, input: &str) -> Result<(PairedPeer, RemoteCatalogDto)> {
        let mut ticket = PeerTicket::parse(input)?;
        if ticket.auth_token.is_empty() {
            let url = format!("http://{}/api/v1/lan-ticket", ticket.endpoint);
            let resp = self
                .http
                .get(&url)
                .timeout(Duration::from_secs(10))
                .send()
                .await
                .with_context(|| format!("Failed connecting to peer {}", ticket.endpoint))?;
            if !resp.status().is_success() {
                anyhow::bail!(
                    "LAN ticket denied on {} (HTTP {}): use full rap2p:// token if LAN Share is OFF",
                    ticket.endpoint,
                    resp.status().as_u16()
                );
            }
            let bytes = read_limited_bytes(resp, 64 * 1024).await?;
            let fetched: PeerTicket = serde_json::from_slice(&bytes)?;
            let dialed_endpoint = ticket.endpoint;
            ticket = fetched;
            if !dialed_endpoint.is_empty() {
                ticket.endpoint = dialed_endpoint;
            }
        }

        let catalog_url = format!("http://{}/api/v1/catalog", ticket.endpoint);
        let resp = self
            .http
            .get(&catalog_url)
            .header("X-Peer-Token", &ticket.auth_token)
            .timeout(Duration::from_secs(25))
            .send()
            .await
            .with_context(|| format!("Failed fetching catalog from {}", ticket.endpoint))?;

        if resp.status().as_u16() == 403 {
            anyhow::bail!("Unauthorized P2P token rejected by {}", ticket.endpoint);
        }
        if !resp.status().is_success() {
            anyhow::bail!(
                "Peer catalog error on {} (HTTP {})",
                ticket.endpoint,
                resp.status().as_u16()
            );
        }

        let bytes = read_limited_bytes(resp, MAX_CATALOG_JSON_BYTES).await?;
        let catalog: RemoteCatalogDto = serde_json::from_slice(&bytes)?;

        let peer = PairedPeer {
            node_id: catalog.node_id.clone(),
            node_name: catalog.node_name.clone(),
            endpoint: ticket.endpoint,
            auth_token: ticket.auth_token,
            track_count: catalog.tracks.len(),
            playlist_count: catalog.playlists.len(),
        };

        Ok((peer, catalog))
    }

    pub async fn download_peer_track_to_cache(
        &self,
        peer: &PairedPeer,
        remote_track_id: i64,
        format: &str,
        cache_dir: &Path,
    ) -> Result<PathBuf> {
        tokio::fs::create_dir_all(cache_dir).await?;
        let safe_node = sanitize_id(&peer.node_id);
        let safe_ext = sanitize_ext(format);
        let target_name = format!("peer_{safe_node}_{remote_track_id}.{safe_ext}");
        let target_path = cache_dir.join(&target_name);
        let tmp_path = cache_dir.join(format!(".{target_name}.{}.part", std::process::id()));

        let url = format!("http://{}/api/v1/stream/{remote_track_id}", peer.endpoint);
        let mut resp = self
            .http
            .get(&url)
            .header("X-Peer-Token", &peer.auth_token)
            .send()
            .await
            .with_context(|| format!("Failed streaming track {remote_track_id} from {}", peer.endpoint))?;

        if !resp.status().is_success() {
            anyhow::bail!(
                "Peer stream failed (HTTP {}) for track {}",
                resp.status().as_u16(),
                remote_track_id
            );
        }

        let mut out_file = tokio::fs::File::create(&tmp_path).await?;
        let mut total_written: u64 = 0;

        while let Some(chunk) = resp.chunk().await? {
            total_written += chunk.len() as u64;
            if total_written > MAX_STREAM_DOWNLOAD_BYTES {
                drop(out_file);
                let _ = tokio::fs::remove_file(&tmp_path).await;
                anyhow::bail!("Peer stream exceeded safety limit of {MAX_STREAM_DOWNLOAD_BYTES} bytes");
            }
            out_file.write_all(&chunk).await?;
        }

        out_file.flush().await?;
        drop(out_file);
        tokio::fs::rename(&tmp_path, &target_path).await?;
        Ok(target_path)
    }
}

async fn read_limited_bytes(mut resp: reqwest::Response, max_bytes: usize) -> Result<Vec<u8>> {
    if let Some(len) = resp.content_length() {
        if len as usize > max_bytes {
            anyhow::bail!("Response size {len} exceeds {max_bytes} byte limit");
        }
    }
    let mut out = Vec::new();
    while let Some(chunk) = resp.chunk().await? {
        if out.len().saturating_add(chunk.len()) > max_bytes {
            anyhow::bail!("Response exceeded {max_bytes} byte limit");
        }
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}
