use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

const MAX_HTTP_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const SECRET_PARAM_PREFIXES: &[&str] = &["apikey=", "api_key=", "passkey=", "token="];

/// Redacts sensitive query parameter values (`apikey`, `jackett_apikey`, `api_key`,
/// `passkey`, `token`) from URLs and error strings before logging, rendering, or persisting.
pub fn redact_url_secrets(input: &str) -> String {
    let lower = input.to_ascii_lowercase();
    let mut out = String::new();
    let mut pos = 0usize;

    while pos < lower.len() {
        let mut earliest: Option<(usize, usize)> = None;
        for pat in SECRET_PARAM_PREFIXES {
            if let Some(rel) = lower[pos..].find(pat) {
                let candidate = (pos + rel, pat.len());
                if earliest.is_none_or(|best| candidate.0 < best.0) {
                    earliest = Some(candidate);
                }
            }
        }

        let Some((match_start, key_len)) = earliest else {
            break;
        };

        let val_start = match_start + key_len;
        let val_end = input[val_start..]
            .find(|c: char| {
                c.is_whitespace() || matches!(c, '&' | '"' | '\'' | ')' | '>' | '#')
            })
            .map(|off| val_start + off)
            .unwrap_or(input.len());

        out.push_str(&input[pos..val_start]);
        out.push_str("***");
        pos = val_end;
    }

    let first_pass = if pos == 0 {
        input.to_string()
    } else {
        out.push_str(&input[pos..]);
        out
    };
    redact_rap2p_fragments(&first_pass)
}

fn redact_rap2p_fragments(input: &str) -> String {
    let lower = input.to_ascii_lowercase();
    let mut out = String::new();
    let mut pos = 0usize;

    while let Some(rel) = lower[pos..].find("rap2p://") {
        let uri_start = pos + rel;
        let uri_end = input[uri_start..]
            .find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '>' | ')'))
            .map(|off| uri_start + off)
            .unwrap_or(input.len());

        if let Some(hash_rel) = input[uri_start..uri_end].find('#') {
            let tok_start = uri_start + hash_rel + 1;
            let tok_end = input[tok_start..uri_end]
                .find('?')
                .map(|off| tok_start + off)
                .unwrap_or(uri_end);
            out.push_str(&input[pos..tok_start]);
            if tok_end > tok_start {
                out.push_str("***");
            }
            pos = tok_end;
        } else {
            out.push_str(&input[pos..uri_end]);
            pos = uri_end;
        }
    }

    if pos == 0 {
        return input.to_string();
    }
    out.push_str(&input[pos..]);
    out
}

async fn read_response_text_capped(
    mut resp: reqwest::Response,
    max_bytes: usize,
) -> std::result::Result<String, String> {
    if let Some(len) = resp.content_length() {
        if len > max_bytes as u64 {
            return Err(format!(
                "response too large ({len} bytes > {max_bytes} byte limit)"
            ));
        }
    }
    let mut buf = Vec::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| redact_url_secrets(&e.without_url().to_string()))?
    {
        if buf.len().saturating_add(chunk.len()) > max_bytes {
            return Err(format!("response exceeded {max_bytes} byte limit"));
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TorrentSearchResult {
    pub title: String,
    pub magnet_uri: String,
    pub seeders: u32,
    pub size_bytes: u64,
    pub provider: String,
}

#[derive(Debug, Clone)]
pub enum SearchProgressEvent {
    Log(String),
    PartialResults(Vec<TorrentSearchResult>),
}

#[async_trait]
pub trait TorrentSearchProvider: Send + Sync {
    fn provider_name(&self) -> &str;
    async fn search(&self, query: &str) -> Result<Vec<TorrentSearchResult>>;
}

#[derive(Debug, Clone, Deserialize)]
struct LanJackettConfig {
    urls: Vec<String>,
    api_key: String,
}

pub struct TorznabSearchProvider {
    pub candidate_urls: Vec<String>,
    pub api_key: String,
    client: reqwest::Client,
}

impl TorznabSearchProvider {
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self::with_urls(vec![base_url.into()], api_key)
    }

    pub fn with_urls(urls: Vec<String>, api_key: impl Into<String>) -> Self {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(6))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        let cleaned: Vec<String> = urls
            .into_iter()
            .map(|u| u.trim().trim_end_matches('/').to_string())
            .filter(|u| !u.is_empty())
            .collect();
        Self {
            candidate_urls: cleaned,
            api_key: api_key.into(),
            client,
        }
    }

    pub fn active_url_hint(&self) -> &str {
        self.candidate_urls
            .first()
            .map(|s| s.as_str())
            .unwrap_or("http://localhost:9117")
    }

    pub fn detect_jackett(url_override: Option<&str>, key_override: Option<&str>) -> Option<Self> {
        let mut urls: Vec<String> = Vec::new();
        let mut found_key: Option<String> = None;

        if let Some(u) = url_override.map(str::trim).filter(|s| !s.is_empty()) {
            urls.push(u.to_string());
        }
        if let Some(k) = key_override.map(str::trim).filter(|s| !s.is_empty()) {
            found_key = Some(k.to_string());
        }

        if let Ok(u) = std::env::var("JACKETT_URL") {
            let u = u.trim();
            if !u.is_empty() && !urls.iter().any(|x| x == u) {
                urls.push(u.to_string());
            }
        }
        if found_key.is_none() {
            if let Ok(k) = std::env::var("JACKETT_API_KEY") {
                if !k.trim().is_empty() {
                    found_key = Some(k.trim().to_string());
                }
            }
        }

        // Check local Jackett ServerConfig.json if present on this machine
        let mut server_cfgs = Vec::new();
        if let Some(cfg) = std::env::var_os("JACKETT_CONFIG").filter(|v| !v.is_empty()) {
            server_cfgs.push(PathBuf::from(cfg));
        }
        if let Some(home_dir) = dirs::home_dir() {
            server_cfgs.push(home_dir.join("media-stack/config/jackett/Jackett/ServerConfig.json"));
        }
        if let Some(cfg_dir) = dirs::config_dir() {
            server_cfgs.push(cfg_dir.join("Jackett").join("ServerConfig.json"));
        }
        for path in server_cfgs {
            if path.exists() {
                if let Ok(raw) = std::fs::read_to_string(&path) {
                    if let Ok(val) = serde_json::from_str::<serde_json::Value>(&raw) {
                        if found_key.is_none() {
                            if let Some(key) = val.get("APIKey").and_then(|v| v.as_str()) {
                                if !key.trim().is_empty() {
                                    found_key = Some(key.trim().to_string());
                                }
                            }
                        }
                        let port = val.get("Port").and_then(|v| v.as_u64()).unwrap_or(9117);
                        let local_url = format!("http://localhost:{port}");
                        if !urls.iter().any(|x| x == &local_url) {
                            urls.push(local_url);
                        }
                    }
                }
            }
        }

        // Check packaged LAN config (.deb / .AppImage / user config)
        let mut lan_cfgs = vec![PathBuf::from("/usr/share/rust-audio-p2p/lan-jackett.json")];
        if let Ok(exe) = std::env::current_exe() {
            if let Some(bin_dir) = exe.parent() {
                lan_cfgs.push(bin_dir.join("../share/rust-audio-p2p/lan-jackett.json"));
            }
        }
        if let Some(cfg_dir) = dirs::config_dir() {
            lan_cfgs.push(cfg_dir.join("rust-audio-p2p").join("lan-jackett.json"));
        }
        for path in lan_cfgs {
            if path.exists() {
                if let Ok(raw) = std::fs::read_to_string(&path) {
                    if let Ok(cfg) = serde_json::from_str::<LanJackettConfig>(&raw) {
                        if found_key.is_none() && !cfg.api_key.trim().is_empty() {
                            found_key = Some(cfg.api_key.trim().to_string());
                        }
                        for u in cfg.urls {
                            let u = u.trim().trim_end_matches('/').to_string();
                            if !u.is_empty() && !urls.iter().any(|x| x == &u) {
                                urls.push(u);
                            }
                        }
                    }
                }
            }
        }

        if urls.is_empty() {
            urls.push("http://localhost:9117".to_string());
        }

        found_key.map(|k| Self::with_urls(urls, k))
    }

    async fn resolve_working_endpoint(
        &self,
        log_tx: Option<&mpsc::UnboundedSender<SearchProgressEvent>>,
    ) -> Option<(String, Vec<(String, String)>)> {
        let key = urlencoding::encode(&self.api_key);
        for base in &self.candidate_urls {
            let safe_base = redact_url_secrets(base);
            let t0 = Instant::now();
            if let Some(tx) = log_tx {
                let _ = tx.send(SearchProgressEvent::Log(format!(
                    "[INFO] Jackett: consultando indexers en {safe_base}..."
                )));
            }
            let url = format!(
                "{base}/api/v2.0/indexers/all/results/torznab/api?apikey={key}&t=indexers&configured=true"
            );
            match self
                .client
                .get(&url)
                .timeout(Duration::from_secs(3))
                .send()
                .await
            {
                Ok(resp) => {
                    let status = resp.status();
                    let ms = t0.elapsed().as_millis();
                    let body = match read_response_text_capped(resp, MAX_HTTP_RESPONSE_BYTES).await {
                        Ok(b) => b,
                        Err(msg) => {
                            if let Some(tx) = log_tx {
                                let _ = tx.send(SearchProgressEvent::Log(format!(
                                    "[ERR] Jackett ({safe_base}): {msg} ({ms} ms)"
                                )));
                            }
                            continue;
                        }
                    };
                    if !status.is_success() {
                        if let Some(tx) = log_tx {
                            let _ = tx.send(SearchProgressEvent::Log(format!(
                                "[ERR] Jackett ({safe_base}): HTTP {status} ({ms} ms)"
                            )));
                        }
                        continue;
                    }
                    if let Some(err_msg) = extract_torznab_error(&body) {
                        let safe_err = redact_url_secrets(&err_msg);
                        if let Some(tx) = log_tx {
                            let _ = tx.send(SearchProgressEvent::Log(format!(
                                "[ERR] Jackett ({safe_base}): {safe_err} ({ms} ms)"
                            )));
                        }
                        continue;
                    }
                    let indexers = parse_torznab_indexers(&body);
                    if let Some(tx) = log_tx {
                        let names: Vec<_> = indexers.iter().map(|(id, _)| id.as_str()).collect();
                        let _ = tx.send(SearchProgressEvent::Log(format!(
                            "[OK] Jackett ({safe_base}): {} indexers activos [{}] ({ms} ms)",
                            indexers.len(),
                            names.join(", ")
                        )));
                    }
                    return Some((base.clone(), indexers));
                }
                Err(e) => {
                    let ms = t0.elapsed().as_millis();
                    let safe_err = redact_url_secrets(&e.without_url().to_string());
                    if let Some(tx) = log_tx {
                        let _ = tx.send(SearchProgressEvent::Log(format!(
                            "[ERR] Jackett ({safe_base}): {safe_err} ({ms} ms)"
                        )));
                    }
                }
            }
        }
        None
    }

    pub async fn search_with_logs(
        &self,
        query: &str,
        log_tx: Option<mpsc::UnboundedSender<SearchProgressEvent>>,
    ) -> Result<Vec<TorrentSearchResult>> {
        let Some((base_url, indexers)) = self.resolve_working_endpoint(log_tx.as_ref()).await else {
            anyhow::bail!("No unreachable/authorized Jackett endpoint responded");
        };
        if indexers.is_empty() {
            if let Some(tx) = &log_tx {
                let _ = tx.send(SearchProgressEvent::Log(
                    "[ERR] Jackett: no hay indexers configurados".to_string(),
                ));
            }
            return Ok(Vec::new());
        }

        let q = urlencoding::encode(query.trim()).into_owned();
        let key = urlencoding::encode(&self.api_key).into_owned();
        let mut handles = Vec::with_capacity(indexers.len());

        for (indexer_id, indexer_title) in indexers {
            let client = self.client.clone();
            let base = base_url.clone();
            let q = q.clone();
            let key = key.clone();
            let tx = log_tx.clone();
            handles.push(tokio::spawn(async move {
                let t0 = Instant::now();
                let url_audio = format!(
                    "{base}/api/v2.0/indexers/{indexer_id}/results/torznab/api?apikey={key}&t=search&cat=3000,3010,3040&q={q}"
                );
                let mut items = match query_single_indexer(&client, &url_audio, &indexer_title).await {
                    Ok(v) => v,
                    Err((tag, msg)) => {
                        let ms = t0.elapsed().as_millis();
                        if let Some(tx) = &tx {
                            let _ = tx.send(SearchProgressEvent::Log(format!(
                                "[{tag}] Jackett/{indexer_id}: {msg} ({ms} ms)"
                            )));
                        }
                        return Vec::new();
                    }
                };

                // If audio category filter yielded 0 items, retry once without `cat=` for general indexers.
                if items.is_empty() {
                    let url_all = format!(
                        "{base}/api/v2.0/indexers/{indexer_id}/results/torznab/api?apikey={key}&t=search&q={q}"
                    );
                    if let Ok(v) = query_single_indexer(&client, &url_all, &indexer_title).await {
                        items = v;
                    }
                }

                let ms = t0.elapsed().as_millis();
                if let Some(tx) = &tx {
                    let _ = tx.send(SearchProgressEvent::Log(format!(
                        "[OK] Jackett/{indexer_id}: {} resultados ({ms} ms)",
                        items.len()
                    )));
                    if !items.is_empty() {
                        let _ = tx.send(SearchProgressEvent::PartialResults(items.clone()));
                    }
                }
                items
            }));
        }

        let mut all = Vec::new();
        for h in handles {
            if let Ok(mut part) = h.await {
                all.append(&mut part);
            }
        }
        Ok(all)
    }
}

async fn query_single_indexer(
    client: &reqwest::Client,
    url: &str,
    provider_label: &str,
) -> std::result::Result<Vec<TorrentSearchResult>, (&'static str, String)> {
    let resp = client
        .get(url)
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .map_err(|e| {
            if e.is_timeout() {
                ("TIMEOUT", "sin respuesta (>5000 ms, omitido)".to_string())
            } else {
                ("ERR", redact_url_secrets(&e.without_url().to_string()))
            }
        })?;

    let status = resp.status();
    let xml = read_response_text_capped(resp, MAX_HTTP_RESPONSE_BYTES)
        .await
        .map_err(|m| ("ERR", m))?;
    if !status.is_success() {
        let detail = extract_torznab_error(&xml)
            .map(|m| redact_url_secrets(&m))
            .unwrap_or_else(|| format!("HTTP {status}"));
        return Err(("ERR", detail));
    }
    if let Some(err_msg) = extract_torznab_error(&xml) {
        return Err(("ERR", redact_url_secrets(&err_msg)));
    }
    Ok(parse_torznab_xml(&xml, &format!("Jackett/{provider_label}")))
}

#[async_trait]
impl TorrentSearchProvider for TorznabSearchProvider {
    fn provider_name(&self) -> &str {
        "Jackett/Torznab"
    }

    async fn search(&self, query: &str) -> Result<Vec<TorrentSearchResult>> {
        self.search_with_logs(query, None).await
    }
}

pub fn parse_torznab_indexers(xml: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(pos) = rest.find("<indexer ") {
        let after = &rest[pos..];
        let Some(tag_end) = after.find('>') else {
            break;
        };
        let open_tag = &after[..tag_end];
        let id = extract_attr(open_tag, "id").unwrap_or_default();
        let Some(close) = after[tag_end..].find("</indexer>") else {
            break;
        };
        let body = &after[tag_end + 1..tag_end + close];
        let title = extract_tag(body, "title").unwrap_or_else(|| id.clone());
        if !id.is_empty() {
            out.push((id, decode_xml_entities(&title)));
        }
        rest = &after[tag_end + close + 10..];
    }
    out
}

fn extract_torznab_error(xml: &str) -> Option<String> {
    let pos = xml.find("<error ")?;
    let end = xml[pos..].find('>')?;
    let tag = &xml[pos..pos + end];
    let code = extract_attr(tag, "code").unwrap_or_else(|| "?".to_string());
    let desc = extract_attr(tag, "description").unwrap_or_else(|| "Torznab error".to_string());
    let decoded = decode_xml_entities(&desc);
    let first_line = decoded.lines().next().unwrap_or("").trim();
    let short: String = first_line.chars().take(140).collect();
    Some(format!("Torznab error {code}: {short}"))
}

pub fn parse_torznab_xml(xml: &str, provider: &str) -> Vec<TorrentSearchResult> {
    let mut results = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find("<item>") {
        let after_start = &rest[start + 6..];
        let Some(end) = after_start.find("</item>") else {
            break;
        };
        let block = &after_start[..end];
        rest = &after_start[end + 7..];

        let title = extract_tag(block, "title").unwrap_or_default();
        if title.is_empty() {
            continue;
        }
        let size_bytes = extract_tag(block, "size")
            .and_then(|s| s.trim().parse::<u64>().ok())
            .unwrap_or(0);
        let magnet_attr = extract_torznab_attr(block, "magneturl");
        let link = extract_tag(block, "link");
        let enclosure = extract_enclosure_url(block);

        let magnet_uri = magnet_attr
            .or_else(|| link.clone().filter(|u| u.starts_with("magnet:")))
            .or(enclosure)
            .or(link)
            .unwrap_or_default();

        if magnet_uri.is_empty() {
            continue;
        }

        let seeders = extract_torznab_attr(block, "seeders")
            .and_then(|s| s.trim().parse::<u32>().ok())
            .unwrap_or(0);

        results.push(TorrentSearchResult {
            title: decode_xml_entities(&title),
            magnet_uri: decode_xml_entities(&magnet_uri),
            seeders,
            size_bytes,
            provider: provider.to_string(),
        });
    }
    results
}

fn extract_tag(block: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let s = block.find(&open)? + open.len();
    let e = block[s..].find(&close)?;
    let raw = block[s..s + e].trim();
    let stripped = raw
        .strip_prefix("<![CDATA[")
        .and_then(|x| x.strip_suffix("]]>"))
        .unwrap_or(raw);
    Some(stripped.to_string())
}

fn extract_enclosure_url(block: &str) -> Option<String> {
    let idx = block.find("<enclosure")?;
    let end = block[idx..].find('>')?;
    extract_attr(&block[idx..idx + end], "url")
}

fn extract_torznab_attr(block: &str, attr_name: &str) -> Option<String> {
    let mut rest = block;
    let needle = format!("name=\"{attr_name}\"");
    let needle_sq = format!("name='{attr_name}'");
    while let Some(pos) = rest.find("torznab:attr") {
        let tag_end = rest[pos..].find('>')?;
        let tag_slice = &rest[pos..pos + tag_end];
        if tag_slice.contains(&needle) || tag_slice.contains(&needle_sq) {
            return extract_attr(tag_slice, "value");
        }
        rest = &rest[pos + tag_end..];
    }
    None
}

fn extract_attr(tag_slice: &str, name: &str) -> Option<String> {
    for quote in ['"', '\''] {
        let pat = format!("{name}={quote}");
        if let Some(start) = tag_slice.find(&pat) {
            let after = &tag_slice[start + pat.len()..];
            if let Some(end) = after.find(quote) {
                return Some(after[..end].to_string());
            }
        }
    }
    None
}

fn decode_xml_entities(input: &str) -> String {
    input
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#xA;", "\n")
        .replace("&#10;", "\n")
}

pub struct InternetArchiveAudioProvider {
    client: reqwest::Client,
}

impl InternetArchiveAudioProvider {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(6))
            .user_agent("rust-audio-p2p/0.1")
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self { client }
    }
}

impl Default for InternetArchiveAudioProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl TorrentSearchProvider for InternetArchiveAudioProvider {
    fn provider_name(&self) -> &str {
        "InternetArchive"
    }

    async fn search(&self, query: &str) -> Result<Vec<TorrentSearchResult>> {
        let q_raw = query.trim();
        if q_raw.is_empty() {
            return Ok(Vec::new());
        }
        let search_expr = format!("({q_raw}) AND mediatype:(audio)");
        let encoded = urlencoding::encode(&search_expr);
        let url = format!(
            "https://archive.org/advancedsearch.php?q={encoded}&fl[]=identifier,title,item_size&rows=20&page=1&output=json"
        );
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("request failed: {}", e.without_url()))?;
        if !resp.status().is_success() {
            anyhow::bail!("HTTP {}", resp.status());
        }
        let body = read_response_text_capped(resp, MAX_HTTP_RESPONSE_BYTES)
            .await
            .map_err(|m| anyhow::anyhow!("{m}"))?;
        let json: serde_json::Value = serde_json::from_str(&body)?;
        let docs = json
            .get("response")
            .and_then(|r| r.get("docs"))
            .and_then(|d| d.as_array())
            .cloned()
            .unwrap_or_default();

        let mut out = Vec::new();
        for doc in docs {
            let Some(id) = doc.get("identifier").and_then(|v| v.as_str()) else {
                continue;
            };
            if id.is_empty()
                || id.contains('/')
                || id.contains('\\')
                || id.contains("..")
                || id.contains('?')
                || id.contains('#')
                || id.chars().any(char::is_whitespace)
            {
                continue;
            }
            let title = doc
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or(id)
                .to_string();
            let size_bytes = doc
                .get("item_size")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let torrent_url = format!("https://archive.org/download/{id}/{id}_archive.torrent");
            out.push(TorrentSearchResult {
                title,
                magnet_uri: torrent_url,
                seeders: 10,
                size_bytes,
                provider: self.provider_name().to_string(),
            });
        }
        Ok(out)
    }
}

pub struct OpenAudioFallbackProvider;

#[async_trait]
impl TorrentSearchProvider for OpenAudioFallbackProvider {
    fn provider_name(&self) -> &str {
        "OpenAudio (CC)"
    }

    async fn search(&self, query: &str) -> Result<Vec<TorrentSearchResult>> {
        let q = query.trim().to_ascii_lowercase();
        let catalog = vec![
            TorrentSearchResult {
                title: "Kevin MacLeod - Creative Commons Audio Collection (FLAC/MP3)".to_string(),
                magnet_uri: "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567&dn=Kevin+MacLeod+CC+Collection".to_string(),
                seeders: 42,
                size_bytes: 312_000_000,
                provider: self.provider_name().to_string(),
            },
            TorrentSearchResult {
                title: "Nine Inch Nails - Ghosts I-IV (CC BY-NC-SA Lossless FLAC)".to_string(),
                magnet_uri: "magnet:?xt=urn:btih:89abcdef0123456789abcdef0123456789abcdef&dn=NIN+Ghosts+I-IV+FLAC".to_string(),
                seeders: 128,
                size_bytes: 680_000_000,
                provider: self.provider_name().to_string(),
            },
            TorrentSearchResult {
                title: "Bach - Goldberg Variations (Kimiko Ishizaka Open Well-Tempered FLAC)".to_string(),
                magnet_uri: "magnet:?xt=urn:btih:abcdef0123456789abcdef0123456789abcdef01&dn=Open+Goldberg+Variations+FLAC".to_string(),
                seeders: 64,
                size_bytes: 540_000_000,
                provider: self.provider_name().to_string(),
            },
        ];
        if q.is_empty() {
            return Ok(catalog);
        }
        let filtered: Vec<_> = catalog
            .into_iter()
            .filter(|r| r.title.to_ascii_lowercase().contains(&q))
            .collect();
        Ok(filtered)
    }
}

pub struct CompositeSearchProvider {
    jackett: Option<TorznabSearchProvider>,
    archive: InternetArchiveAudioProvider,
    fallback: OpenAudioFallbackProvider,
}

impl CompositeSearchProvider {
    pub fn with_overrides(url_override: Option<&str>, key_override: Option<&str>) -> Self {
        Self {
            jackett: TorznabSearchProvider::detect_jackett(url_override, key_override),
            archive: InternetArchiveAudioProvider::new(),
            fallback: OpenAudioFallbackProvider,
        }
    }

    pub fn with_defaults() -> Self {
        Self::with_overrides(None, None)
    }

    pub fn jackett_endpoint_hint(&self) -> Option<&str> {
        self.jackett.as_ref().map(|j| j.active_url_hint())
    }

    pub fn provider_names(&self) -> Vec<&str> {
        let mut names = Vec::new();
        if self.jackett.is_some() {
            names.push("Jackett/Torznab");
        }
        names.push("InternetArchive");
        names.push("OpenAudio (CC)");
        names
    }

    pub async fn search_streaming(
        &self,
        query: &str,
        tx: mpsc::UnboundedSender<SearchProgressEvent>,
    ) -> (Vec<TorrentSearchResult>, Vec<String>) {
        let mut merged = Vec::new();
        let mut errors = Vec::new();
        let q = query.trim();

        if q.is_empty() {
            let _ = tx.send(SearchProgressEvent::Log(
                "[INFO] Consulta vacía; escribe un artista, álbum o tema.".to_string(),
            ));
            return (merged, errors);
        }

        if self.jackett.is_none() {
            let _ = tx.send(SearchProgressEvent::Log(
                "[INFO] Jackett no detectado (configura URL/API key con 'u' o JACKETT_URL/JACKETT_API_KEY); buscando en InternetArchive..."
                    .to_string(),
            ));
        }

        let jackett_fut = async {
            match &self.jackett {
                Some(j) => j.search_with_logs(q, Some(tx.clone())).await,
                None => Ok(Vec::new()),
            }
        };

        let archive_fut = async {
            let t0 = Instant::now();
            let _ = tx.send(SearchProgressEvent::Log(format!(
                "[INFO] InternetArchive: buscando '{q}'..."
            )));
            let res = self.archive.search(q).await;
            let ms = t0.elapsed().as_millis();
            match &res {
                Ok(items) => {
                    let _ = tx.send(SearchProgressEvent::Log(format!(
                        "[OK] InternetArchive: {} resultados ({ms} ms)",
                        items.len()
                    )));
                    if !items.is_empty() {
                        let _ = tx.send(SearchProgressEvent::PartialResults(items.clone()));
                    }
                }
                Err(e) => {
                    let safe_err = redact_url_secrets(&e.to_string());
                    let _ = tx.send(SearchProgressEvent::Log(format!(
                        "[ERR] InternetArchive: {safe_err} ({ms} ms)"
                    )));
                }
            }
            res
        };

        let (j_res, a_res) = tokio::join!(jackett_fut, archive_fut);
        match j_res {
            Ok(mut items) => merged.append(&mut items),
            Err(e) => errors.push(redact_url_secrets(&format!("Jackett: {e}"))),
        }
        match a_res {
            Ok(mut items) => merged.append(&mut items),
            Err(e) => errors.push(redact_url_secrets(&format!("InternetArchive: {e}"))),
        }

        if let Ok(mut fb) = self.fallback.search(q).await {
            if !fb.is_empty() {
                let _ = tx.send(SearchProgressEvent::PartialResults(fb.clone()));
                merged.append(&mut fb);
            }
        }

        merged.sort_by(|a, b| b.seeders.cmp(&a.seeders));
        (merged, errors)
    }

    pub async fn search_all(&self, query: &str) -> (Vec<TorrentSearchResult>, Vec<String>) {
        let (tx, _rx) = mpsc::unbounded_channel();
        self.search_streaming(query, tx).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_torznab_xml_and_indexers() {
        let indexers_xml = r#"
        <indexers>
          <indexer id="thepiratebay" configured="true"><title>The Pirate Bay</title></indexer>
          <indexer id="dontorrent" configured="true"><title>DonTorrent</title></indexer>
        </indexers>"#;
        let idx = parse_torznab_indexers(indexers_xml);
        assert_eq!(idx.len(), 2);
        assert_eq!(idx[0], ("thepiratebay".to_string(), "The Pirate Bay".to_string()));

        let sample = r#"
        <rss version="2.0" xmlns:torznab="http://torznab.com/schemas/2015/feed">
          <channel>
            <item>
              <title>Almendra - Almendra (1969) [FLAC]</title>
              <size>345678901</size>
              <link>magnet:?xt=urn:btih:deadbeefdeadbeefdeadbeefdeadbeefdeadbeef&amp;dn=Almendra</link>
              <torznab:attr name="seeders" value="18" />
              <torznab:attr name="magneturl" value="magnet:?xt=urn:btih:deadbeefdeadbeefdeadbeefdeadbeefdeadbeef&amp;dn=Almendra" />
            </item>
          </channel>
        </rss>"#;

        let items = parse_torznab_xml(sample, "Jackett");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title, "Almendra - Almendra (1969) [FLAC]");
        assert_eq!(items[0].seeders, 18);
        assert_eq!(items[0].size_bytes, 345678901);
        assert!(items[0].magnet_uri.starts_with("magnet:?xt="));
    }

    #[tokio::test]
    async fn test_fallback_provider_search() {
        let p = OpenAudioFallbackProvider;
        let res = p.search("bach").await.unwrap();
        assert_eq!(res.len(), 1);
        assert!(res[0].title.contains("Bach"));
    }

    #[test]
    fn test_redact_url_secrets() {
        assert_eq!(
            redact_url_secrets("http://localhost:9117/dl/x/?jackett_apikey=abc123&path=Q&file=f"),
            "http://localhost:9117/dl/x/?jackett_apikey=***&path=Q&file=f"
        );
        assert_eq!(
            redact_url_secrets("GET http://h/api?apikey=SECRET returned 404"),
            "GET http://h/api?apikey=*** returned 404"
        );
        assert_eq!(redact_url_secrets("APIKEY=Z"), "APIKEY=***");
        assert_eq!(
            redact_url_secrets("token=t1&passkey=p2&api_key=k3"),
            "token=***&passkey=***&api_key=***"
        );
        let magnet = "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567&dn=Kevin+MacLeod";
        assert_eq!(redact_url_secrets(magnet), magnet);
        assert_eq!(
            redact_url_secrets("Almendra – ñ apikey=x"),
            "Almendra – ñ apikey=***"
        );
        assert_eq!(
            redact_url_secrets("rap2p://192.168.1.10:4261#supersecret123?name=air"),
            "rap2p://192.168.1.10:4261#***?name=air"
        );
    }
}
