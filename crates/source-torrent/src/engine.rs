use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use librqbit::{AddTorrent, AddTorrentOptions, AddTorrentResponse, ManagedTorrent, Session, SessionOptions};
use serde::{Deserialize, Serialize};

pub const DEFAULT_PORT_RANGE_START: u16 = 4240;
pub const DEFAULT_PORT_RANGE_END: u16 = 4260;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TorrentFileEntry {
    pub index: usize,
    pub path: String,
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
        let opts = SessionOptions {
            // Dedicated range 4240..4260 to avoid colliding with qBittorrent (6881) on air.
            listen_port_range: Some(DEFAULT_PORT_RANGE_START..DEFAULT_PORT_RANGE_END),
            enable_upnp_port_forwarding: false,
            disable_dht_persistence: true,
            fastresume: false,
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
        let opts = AddTorrentOptions {
            overwrite: true,
            ..Default::default()
        };
        let resp = self
            .session
            .add_torrent(AddTorrent::from_url(magnet_or_url), Some(opts))
            .await?;
        let handle = match resp {
            AddTorrentResponse::Added(_, h) | AddTorrentResponse::AlreadyManaged(_, h) => h,
            AddTorrentResponse::ListOnly(_) => {
                anyhow::bail!("unexpected list-only response when adding torrent");
            }
        };
        Ok(Self::snapshot_from_handle(&handle))
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
        Ok(Self::snapshot_from_handle(&handle))
    }

    pub fn list_torrents(&self) -> Vec<TorrentStatusSnapshot> {
        self.session.with_torrents(|iter| {
            iter.map(|(_, h)| Self::snapshot_from_handle(h)).collect()
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

    fn snapshot_from_handle(handle: &Arc<ManagedTorrent>) -> TorrentStatusSnapshot {
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

        let files = handle
            .with_metadata(|meta| {
                meta.file_infos
                    .iter()
                    .enumerate()
                    .map(|(idx, f)| TorrentFileEntry {
                        index: idx,
                        path: f.relative_filename.to_string_lossy().to_string(),
                        length: f.len,
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        TorrentStatusSnapshot {
            id: handle.id(),
            info_hash: format!("{:?}", handle.info_hash()),
            name: handle
                .name()
                .unwrap_or_else(|| format!("Torrent #{}", handle.id())),
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
