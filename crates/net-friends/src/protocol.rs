use std::net::UdpSocket;
use std::time::{SystemTime, UNIX_EPOCH};

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

fn random_hex(num_bytes: usize) -> String {
    let mut buf = vec![0u8; num_bytes];
    if read_urandom(&mut buf).is_err() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0x5eed);
        let pid = std::process::id() as u128;
        let mut seed = nanos ^ (pid << 32) ^ 0x9e3779b97f4a7c15;
        for b in &mut buf {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            *b = (seed & 0xff) as u8;
        }
    }
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

fn read_urandom(buf: &mut [u8]) -> std::io::Result<()> {
    use std::io::Read;
    let mut f = std::fs::File::open("/dev/urandom")?;
    f.read_exact(buf)
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

        let mut auth_token = String::new();
        let mut node_name = host_only(&endpoint).to_string();
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
                            let decoded = percent_decode(v);
                            if !decoded.trim().is_empty() {
                                node_name = decoded;
                            }
                        }
                        "id" => {
                            let decoded = percent_decode(v);
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
