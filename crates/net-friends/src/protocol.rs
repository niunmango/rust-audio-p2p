use std::net::{IpAddr, UdpSocket};

use anyhow::Result;
use rust_audio_core::Database;
use serde::{Deserialize, Serialize};

pub const DEFAULT_P2P_PORT: u16 = 4261;
pub const P2P_PORT_END: u16 = 4280;
pub const LAN_DISCOVERY_PORT: u16 = 4265;
pub const DISCOVER_REQ_MSG: &[u8] = b"RAP2P_DISCOVER_V1";
pub const DISCOVER_RESP_PREFIX: &str = "RAP2P_TICKET_V1 ";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeIdentity {
    pub node_id: String,
    pub node_name: String,
    pub auth_token: String,
}

impl NodeIdentity {
    pub fn load_or_create(db: &Database) -> Result<Self> {
        let node_id = match db.get_setting("p2p_node_id")? {
            Some(v) if !v.trim().is_empty() => v,
            _ => {
                let generated = random_hex(8);
                db.set_setting("p2p_node_id", &generated)?;
                generated
            }
        };

        let auth_token = match db.get_setting("p2p_auth_token")? {
            Some(v) if !v.trim().is_empty() => v,
            _ => {
                let generated = random_hex(16);
                db.set_setting("p2p_auth_token", &generated)?;
                generated
            }
        };

        let node_name = match db.get_setting("p2p_node_name")? {
            Some(v) if !v.trim().is_empty() => v,
            _ => {
                let host = detect_hostname();
                db.set_setting("p2p_node_name", &host)?;
                host
            }
        };

        Ok(Self {
            node_id,
            node_name,
            auth_token,
        })
    }
}

pub fn verify_token_ct(expected: &str, supplied: &str) -> bool {
    let a = expected.as_bytes();
    let b = supplied.as_bytes();
    if a.is_empty() || a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

pub fn detect_lan_ip() -> String {
    if let Ok(sock) = UdpSocket::bind("0.0.0.0:0") {
        if sock.connect("8.8.8.8:80").is_ok() {
            if let Ok(addr) = sock.local_addr() {
                let ip = addr.ip();
                if !ip.is_loopback() && !ip.is_unspecified() {
                    return ip.to_string();
                }
            }
        }
    }
    "127.0.0.1".to_string()
}

fn detect_hostname() -> String {
    if let Ok(name) = std::fs::read_to_string("/etc/hostname") {
        let trimmed = name.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    for key in ["HOSTNAME", "COMPUTERNAME"] {
        if let Ok(v) = std::env::var(key) {
            let trimmed = v.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
    }
    "rust-audio-node".to_string()
}

pub fn random_hex(num_bytes: usize) -> String {
    let mut buf = vec![0u8; num_bytes];
    fill_random_bytes(&mut buf);
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn sanitize_display_text(input: &str) -> String {
    input
        .chars()
        .filter(|c| !c.is_control())
        .take(256)
        .collect()
}

#[cfg(unix)]
fn fill_random_bytes(buf: &mut [u8]) {
    use std::io::Read;
    let mut f = std::fs::File::open("/dev/urandom").expect("OS CSPRNG /dev/urandom unavailable");
    f.read_exact(buf)
        .expect("Failed reading random bytes from /dev/urandom");
}

#[cfg(not(unix))]
fn fill_random_bytes(buf: &mut [u8]) {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9e37_79b9_7f4a_7c15);
    let pid = std::process::id() as u64;
    let stack_probe = buf.as_ptr() as usize as u64;
    let mut state = nanos ^ pid.rotate_left(17) ^ stack_probe.rotate_left(7);
    for b in buf.iter_mut() {
        state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^= z >> 31;
        *b = (z & 0xff) as u8;
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeInfoDto {
    pub node_id: String,
    pub node_name: String,
    pub endpoint: String,
    pub track_count: usize,
    pub playlist_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteTrackDto {
    pub id: i64,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub year: Option<i32>,
    pub track_number: Option<u32>,
    pub duration_secs: Option<u64>,
    pub file_size_bytes: u64,
    pub format: String,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer_node: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemotePlaylistDto {
    pub name: String,
    pub pinned: bool,
    pub track_ids: Vec<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteCatalogDto {
    pub node_id: String,
    pub node_name: String,
    pub tracks: Vec<RemoteTrackDto>,
    pub playlists: Vec<RemotePlaylistDto>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WebPlayerSnapshot {
    pub state: String,
    pub position_secs: f64,
    pub duration_secs: Option<f64>,
    pub volume: f32,
    pub auto_next: bool,
    pub current_track: Option<RemoteTrackDto>,
    pub backend: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum WebRemoteCommand {
    PlayTrack { track_id: i64 },
    EnqueueTrack { track_id: i64 },
    EnqueueTracks { track_ids: Vec<i64> },
    TogglePause,
    Next,
    Prev,
    Seek { position_secs: f64 },
    SetVolume { volume: f32 },
    ToggleAutoNext,
    TogglePin { track_id: i64 },
    CreatePlaylist { name: String, track_id: Option<i64> },
    CreatePlaylistWithTracks { name: String, track_ids: Vec<i64> },
    RenamePlaylist { playlist_id: i64, new_name: String },
    DeletePlaylist { playlist_id: i64 },
    AddToPlaylist { playlist_id: i64, track_id: i64 },
    AddTracksToPlaylist { playlist_id: i64, track_ids: Vec<i64> },
    RemoveFromPlaylist { playlist_id: i64, track_id: i64 },
    ConnectPeer { target: String },
    DiscoverLan,
    SyncRefresh { message: Option<String> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebPlaylistSummary {
    pub id: i64,
    pub name: String,
    pub pinned: bool,
    pub track_ids: Vec<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebPeerSummary {
    pub node_id: String,
    pub node_name: String,
    pub endpoint: String,
    pub track_count: usize,
    pub playlist_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebStateDto {
    pub node_id: String,
    pub node_name: String,
    pub endpoint: String,
    pub ticket_uri: String,
    pub lan_share: bool,
    pub player: WebPlayerSnapshot,
    pub peers: Vec<WebPeerSummary>,
    pub playlists: Vec<WebPlaylistSummary>,
}

pub fn track_to_remote_dto(t: &rust_audio_core::Track) -> RemoteTrackDto {
    let peer_node = match &t.source {
        rust_audio_core::TrackSource::Peer { node_name, .. } => Some(node_name.clone()),
        _ => None,
    };
    RemoteTrackDto {
        id: t.id,
        title: t.title.clone(),
        artist: t.artist.clone(),
        album: t.album.clone(),
        year: t.year,
        track_number: t.track_number,
        duration_secs: t.duration_secs,
        file_size_bytes: t.file_size_bytes,
        format: t.format.clone(),
        pinned: t.pinned,
        peer_node,
    }
}

pub fn extract_query_param(query: &str, key: &str) -> Option<String> {
    for pair in query.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            if k == key {
                return Some(percent_decode(v));
            }
        }
    }
    None
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerTicket {
    pub node_id: String,
    pub node_name: String,
    pub endpoint: String,
    pub auth_token: String,
}

impl PeerTicket {
    pub fn to_uri(&self) -> String {
        let enc_name = percent_encode(&self.node_name);
        let enc_id = percent_encode(&self.node_id);
        format!(
            "rap2p://{}#{}?name={}&id={}",
            self.endpoint, self.auth_token, enc_name, enc_id
        )
    }

    pub fn parse(raw: &str) -> Result<Self> {
        let trimmed = raw.trim();
        anyhow::ensure!(!trimmed.is_empty(), "Empty peer ticket or address");
        anyhow::ensure!(
            !trimmed.chars().any(char::is_whitespace),
            "Peer ticket must not contain whitespace"
        );

        let body = trimmed
            .strip_prefix("rap2p://")
            .or_else(|| trimmed.strip_prefix("http://"))
            .unwrap_or(trimmed);

        // Separate host[:port] from optional ?query / #fragment regardless of ordering
        let (addr_part, rest) = if let Some((left, right)) = body.split_once('#') {
            (left, Some(('#', right)))
        } else if let Some((left, right)) = body.split_once('?') {
            (left, Some(('?', right)))
        } else {
            (body, None)
        };

        let host_port_raw = addr_part.trim_end_matches('/');
        anyhow::ensure!(!host_port_raw.is_empty(), "Missing peer host/endpoint");
        anyhow::ensure!(
            !host_port_raw.contains('@') && !host_port_raw.contains('/'),
            "Invalid peer endpoint format"
        );

        let endpoint = normalize_endpoint(host_port_raw, DEFAULT_P2P_PORT)?;
        validate_peer_endpoint(&endpoint)?;

        let mut auth_token = String::new();
        let mut node_name = sanitize_display_text(host_only(&endpoint));
        let mut node_id = String::new();

        if let Some((delim, tail)) = rest {
            let (frag_part, query_part) = if delim == '#' {
                match tail.split_once('?') {
                    Some((frag, q)) => (frag, Some(q)),
                    None => (tail, None),
                }
            } else {
                match tail.split_once('#') {
                    Some((q, frag)) => (frag, Some(q)),
                    None => ("", Some(tail)),
                }
            };

            auth_token = frag_part.trim().to_string();
            if let Some(q) = query_part {
                for pair in q.split('&') {
                    let Some((k, v)) = pair.split_once('=') else {
                        continue;
                    };
                    match k {
                        "name" => {
                            let decoded = sanitize_display_text(&percent_decode(v));
                            if !decoded.trim().is_empty() {
                                node_name = decoded;
                            }
                        }
                        "id" => {
                            let decoded = sanitize_display_text(&percent_decode(v));
                            if !decoded.trim().is_empty() {
                                node_id = decoded;
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        if node_id.is_empty() {
            node_id = sanitize_id(&endpoint);
        }

        Ok(Self {
            node_id,
            node_name,
            endpoint,
            auth_token,
        })
    }

    /// Generates a deterministic compact Unicode block-matrix QR visual for terminal/GUI display.
    pub fn render_unicode_qr(&self) -> Vec<String> {
        let uri = self.to_uri();
        const N: usize = 25;
        let mut grid = [[false; N]; N];
        let mut reserved = [[false; N]; N];

        // Standard 7x7 finder patterns in three corners + quiet separators
        for (r0, c0) in [(0, 0), (0, N - 7), (N - 7, 0)] {
            for dr in -1isize..=7 {
                for dc in -1isize..=7 {
                    let r = r0 as isize + dr;
                    let c = c0 as isize + dc;
                    if r >= 0 && r < N as isize && c >= 0 && c < N as isize {
                        let ru = r as usize;
                        let cu = c as usize;
                        reserved[ru][cu] = true;
                        let in_box = dr >= 0 && dr <= 6 && dc >= 0 && dc <= 6;
                        if in_box {
                            let border = dr == 0 || dr == 6 || dc == 0 || dc == 6;
                            let center = dr >= 2 && dr <= 4 && dc >= 2 && dc <= 4;
                            grid[ru][cu] = border || center;
                        }
                    }
                }
            }
        }

        // Timing bands
        for i in 8..(N - 8) {
            reserved[6][i] = true;
            grid[6][i] = i % 2 == 0;
            reserved[i][6] = true;
            grid[i][6] = i % 2 == 0;
        }

        // Deterministically fill remaining data cells from ticket payload bytes
        let bytes = uri.as_bytes();
        let mut state: u64 = 0x517cc1b727220a95;
        for &b in bytes {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(b as u64 ^ 0xa5);
        }
        let mut bit_idx = 0usize;
        for r in 0..N {
            for c in 0..N {
                if reserved[r][c] {
                    continue;
                }
                let byte = bytes[bit_idx / 8 % bytes.len()];
                let bit = (byte >> (bit_idx % 8)) & 1;
                state = state.rotate_left(7) ^ ((r as u64 + 1) * (c as u64 + 3));
                let mask = ((state >> (bit_idx % 16)) & 1) as u8;
                grid[r][c] = (bit ^ mask) != 0;
                bit_idx += 1;
            }
        }

        // Pack 2 vertical rows per character line using Unicode half blocks
        let mut lines = Vec::new();
        let border_line = " ".repeat(N + 2);
        lines.push(border_line.clone());
        let mut r = 0;
        while r < N {
            let mut line = String::with_capacity(N + 2);
            line.push(' ');
            for c in 0..N {
                let top = grid[r][c];
                let bot = if r + 1 < N { grid[r + 1][c] } else { false };
                let ch = match (top, bot) {
                    (true, true) => '█',
                    (true, false) => '▀',
                    (false, true) => '▄',
                    (false, false) => ' ',
                };
                line.push(ch);
            }
            line.push(' ');
            lines.push(line);
            r += 2;
        }
        lines.push(border_line);
        lines
    }
}

fn normalize_endpoint(raw: &str, default_port: u16) -> Result<String> {
    if raw.starts_with('[') {
        if let Some(end) = raw.find(']') {
            let host_bracketed = &raw[..=end];
            let rest = &raw[end + 1..];
            if rest.is_empty() {
                return Ok(format!("{host_bracketed}:{default_port}"));
            }
            if let Some(port_str) = rest.strip_prefix(':') {
                let port: u16 = port_str.parse()?;
                anyhow::ensure!(port > 0, "Invalid port 0");
                return Ok(format!("{host_bracketed}:{port}"));
            }
        }
        anyhow::bail!("Invalid bracketed endpoint: {raw}");
    }

    if let Some((host, port_str)) = raw.rsplit_once(':') {
        anyhow::ensure!(!host.is_empty(), "Missing host before port");
        anyhow::ensure!(!host.contains(':'), "Ambiguous IPv6 address without brackets");
        let port: u16 = port_str.parse()?;
        anyhow::ensure!(port > 0, "Invalid port 0");
        Ok(format!("{host}:{port}"))
    } else {
        Ok(format!("{raw}:{default_port}"))
    }
}

pub fn validate_peer_endpoint(endpoint: &str) -> Result<()> {
    const BLOCKED_INTERNAL_PORTS: &[u16] = &[22, 3306, 5432, 6379, 8080, 8191, 9117];
    let (raw_host, port_str) = endpoint
        .rsplit_once(':')
        .ok_or_else(|| anyhow::anyhow!("Missing port in endpoint"))?;
    let port: u16 = port_str.parse()?;
    anyhow::ensure!(
        port > 0 && !BLOCKED_INTERNAL_PORTS.contains(&port),
        "Blocked internal service port {port}"
    );
    let host = raw_host
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(raw_host);
    if let Ok(ip) = host.parse::<IpAddr>() {
        anyhow::ensure!(!ip.is_unspecified(), "Unspecified peer IP not allowed");
        match ip {
            IpAddr::V4(v4) => {
                anyhow::ensure!(
                    !v4.is_link_local() && !v4.is_multicast(),
                    "Link-local/metadata/multicast IP not allowed"
                );
            }
            IpAddr::V6(v6) => {
                let is_link_local = (v6.segments()[0] & 0xffc0) == 0xfe80;
                anyhow::ensure!(
                    !is_link_local && !v6.is_multicast(),
                    "Link-local/multicast IPv6 not allowed"
                );
            }
        }
    }
    Ok(())
}

fn host_only(endpoint: &str) -> &str {
    endpoint
        .rsplit_once(':')
        .map(|(h, _)| h)
        .unwrap_or(endpoint)
}

pub fn sanitize_id(input: &str) -> String {
    let cleaned: String = input
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    if cleaned.is_empty() {
        "node".to_string()
    } else {
        cleaned.chars().take(48).collect()
    }
}

pub fn sanitize_ext(input: &str) -> String {
    let cleaned: String = input
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect();
    if cleaned.is_empty() {
        "audio".to_string()
    } else {
        cleaned.chars().take(8).collect()
    }
}

fn percent_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for &b in input.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut raw_out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(hex) = std::str::from_utf8(&bytes[i + 1..i + 3]) {
                if let Ok(val) = u8::from_str_radix(hex, 16) {
                    raw_out.push(val);
                    i += 3;
                    continue;
                }
            }
        } else if bytes[i] == b'+' {
            raw_out.push(b' ');
            i += 1;
            continue;
        }
        raw_out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&raw_out).to_string()
}

#[cfg(test)]
mod tests {
    use super::random_hex;

    #[test]
    fn test_random_hex_length_and_uniqueness() {
        let a = random_hex(16);
        let b = random_hex(16);
        assert_eq!(a.len(), 32);
        assert_eq!(b.len(), 32);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }
}
