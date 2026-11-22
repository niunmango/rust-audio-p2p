use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use librqbit::{
    AddTorrent, AddTorrentOptions, AddTorrentResponse, ManagedTorrent, Session, SessionOptions,
};
use serde::{Deserialize, Serialize};

pub const DEFAULT_PORT_RANGE_START: u16 = 4240;
pub const DEFAULT_PORT_RANGE_END: u16 = 4260;

const PUBLIC_TRACKERS: &[&str] = &[
    "udp://tracker.opentrackr.org:1337/announce",
    "udp://open.stealth.si:80/announce",
    "udp://tracker.torrent.eu.org:451/announce",
    "udp://tracker.dler.org:6969/announce",
    "udp://exodus.desync.com:6969/announce",
];

pub fn sanitize_torrent_uri(raw: &str) -> String {
    let trimmed = raw.trim().trim_matches(|c| c == '"' || c == '\'');
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("agnet:?") || lower.starts_with("agnet://") {
        format!("m{trimmed}")
    } else {
        trimmed.to_string()
    }
}

pub fn is_torrent_uri(raw: &str) -> bool {
    let clean = sanitize_torrent_uri(raw);
    let lower = clean.to_ascii_lowercase();
    lower.starts_with("magnet:?")
        || lower.starts_with("http://")
        || lower.starts_with("https://")
        || (clean.len() == 40 && clean.chars().all(|c| c.is_ascii_hexdigit()))
}

pub fn extract_magnet_display_name(raw: &str) -> String {
    let uri = sanitize_torrent_uri(raw);
    if let Some(query) = uri.strip_prefix("magnet:?") {
        let mut hash_fallback = None;
        for part in query.split('&') {
            if let Some(dn) = part.strip_prefix("dn=") {
                let plus_fixed = dn.replace('+', " ");
                if let Ok(decoded) = urlencoding::decode(&plus_fixed) {
                    let s = decoded.trim().to_string();
                    if !s.is_empty() {
                        return s;
                    }
                }
            }
            if let Some(xt) = part.strip_prefix("xt=urn:btih:") {
                if !xt.is_empty() {
                    hash_fallback = Some(format!("magnet:{}", &xt[..xt.len().min(12)]));
                }
            }
        }
        if let Some(fb) = hash_fallback {
            return fb;
        }
    }
    if let Some(name) = uri.rsplit('/').next() {
        let s = name.trim();
        if !s.is_empty() {
            return s.to_string();
        }
    }
    uri.chars().take(40).collect()
}

fn append_fallback_trackers_if_missing(uri: &str) -> String {
    if !uri.to_ascii_lowercase().starts_with("magnet:?") {
        return uri.to_string();
    }
    if uri.contains("&tr=") || uri.contains("?tr=") {
        return uri.to_string();
    }
    let mut out = uri.to_string();
    for tr in PUBLIC_TRACKERS {
        out.push_str("&tr=");
        out.push_str(&urlencoding::encode(tr));
    }
    out
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TorrentFileEntry {
    pub index: usize,
    pub path: String,
    pub full_path: PathBuf,
    pub length: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TorrentStatusSnapshot {
    pub id: usize,
    pub info_hash: String,
    pub name: String,
    pub state: String,
    pub progress_bytes: u64,
    pub total_bytes: u64,
    pub finished: bool,
    pub download_speed_mbps: f64,
    pub peers_live: usize,
    pub files: Vec<TorrentFileEntry>,
}

pub struct TorrentEngine {
    session: Arc<Session>,
    pub download_dir: PathBuf,
}

impl TorrentEngine {
    pub async fn new(download_dir: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&download_dir)?;
        let trackers: HashSet<reqwest::Url> = PUBLIC_TRACKERS
            .iter()
            .filter_map(|t| reqwest::Url::parse(t).ok())
            .collect();
        let opts = SessionOptions {
            // Dedicated range 4240..4260 to avoid colliding with qBittorrent (6881) on air.
            listen_port_range: Some(DEFAULT_PORT_RANGE_START..DEFAULT_PORT_RANGE_END),
            enable_upnp_port_forwarding: false,
            disable_dht_persistence: true,
            fastresume: false,
            trackers,
            ..Default::default()
        };
        let session = Session::new_with_opts(download_dir.clone(), opts).await?;
        Ok(Self {
            session,
            download_dir,
        })
    }

    pub fn listen_port(&self) -> Option<u16> {
        self.session.tcp_listen_port()
    }

    pub async fn add_magnet(&self, magnet_or_url: &str) -> Result<TorrentStatusSnapshot> {
        let sanitized = sanitize_torrent_uri(magnet_or_url);
        let uri_with_trackers = append_fallback_trackers_if_missing(&sanitized);
        let opts = AddTorrentOptions {
            overwrite: true,
            trackers: Some(PUBLIC_TRACKERS.iter().map(|s| s.to_string()).collect()),
            ..Default::default()
        };
        let resp = self
            .session
            .add_torrent(AddTorrent::from_url(&uri_with_trackers), Some(opts))
            .await?;
        let handle = match resp {
            AddTorrentResponse::Added(_, h) | AddTorrentResponse::AlreadyManaged(_, h) => h,
            AddTorrentResponse::ListOnly(_) => {
                anyhow::bail!("unexpected list-only response when adding torrent");
            }
        };
        Ok(Self::snapshot_from_handle(&self.download_dir, &handle))
    }

    pub async fn add_torrent_bytes(&self, bytes: Vec<u8>) -> Result<TorrentStatusSnapshot> {
        let opts = AddTorrentOptions {
            overwrite: true,
            ..Default::default()
        };
        let resp = self
            .session
            .add_torrent(AddTorrent::from_bytes(bytes), Some(opts))
            .await?;
        let handle = match resp {
            AddTorrentResponse::Added(_, h) | AddTorrentResponse::AlreadyManaged(_, h) => h,
            AddTorrentResponse::ListOnly(_) => {
                anyhow::bail!("unexpected list-only response when adding torrent bytes");
            }
        };
        Ok(Self::snapshot_from_handle(&self.download_dir, &handle))
    }

    pub fn list_torrents(&self) -> Vec<TorrentStatusSnapshot> {
        self.session.with_torrents(|iter| {
            iter.map(|(_, h)| Self::snapshot_from_handle(&self.download_dir, h))
                .collect()
        })
    }

    pub async fn open_file_stream(
        &self,
        id: usize,
        file_index: usize,
    ) -> Result<impl tokio::io::AsyncRead + tokio::io::AsyncSeek + Send + Unpin> {
        let handle = self
            .session
            .get(id.into())
            .ok_or_else(|| anyhow::anyhow!("torrent {id} not found"))?;
        handle.stream(file_index)
    }

    pub async fn stop(&self) {
        self.session.stop().await;
    }

    fn snapshot_from_handle(
        download_dir: &std::path::Path,
        handle: &Arc<ManagedTorrent>,
    ) -> TorrentStatusSnapshot {
        let stats = handle.stats();
        let download_speed_mbps = stats
            .live
            .as_ref()
            .map(|l| l.download_speed.mbps)
            .unwrap_or(0.0);
        let peers_live = stats
            .live
            .as_ref()
            .map(|l| l.snapshot.peer_stats.live)
            .unwrap_or(0);

        let torrent_name = handle
            .name()
            .unwrap_or_else(|| format!("Torrent #{}", handle.id()));
        let files = handle
            .with_metadata(|meta| {
                let file_count = meta.file_infos.len();
                let sub = if file_count >= 2 {
                    meta.name.as_deref().unwrap_or(&torrent_name)
                } else {
                    ""
                };
                meta.file_infos
                    .iter()
                    .enumerate()
                    .map(|(idx, f)| {
                        let direct = download_dir.join(&f.relative_filename);
                        let nested = if sub.is_empty() {
                            direct.clone()
                        } else {
                            download_dir.join(sub).join(&f.relative_filename)
                        };
                        let full_path = if direct.exists() { direct } else { nested };
                        TorrentFileEntry {
                            index: idx,
                            path: f.relative_filename.to_string_lossy().to_string(),
                            full_path,
                            length: f.len,
                        }
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        TorrentStatusSnapshot {
            id: handle.id(),
            info_hash: format!("{:?}", handle.info_hash()),
            name: torrent_name,
            state: format!("{:?}", stats.state),
            progress_bytes: stats.progress_bytes,
            total_bytes: stats.total_bytes,
            finished: stats.finished,
            download_speed_mbps,
            peers_live,
            files,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_and_extract_magnet_name() {
        let broken = "  agnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567&dn=Almendra+1969\n";
        let fixed = sanitize_torrent_uri(broken);
        assert!(fixed.starts_with("magnet:?xt="));
        assert!(is_torrent_uri(broken));
        assert_eq!(extract_magnet_display_name(broken), "Almendra 1969");

        let with_tr = append_fallback_trackers_if_missing(&fixed);
        assert!(with_tr.contains("&tr=udp%3A%2F%2Ftracker.opentrackr.org"));
    }
}
