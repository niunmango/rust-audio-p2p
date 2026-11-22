use std::path::PathBuf;

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

const MAX_TORZNAB_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const SECRET_PARAM_PREFIXES: &[&str] = &["apikey=", "api_key=", "passkey=", "token="];

/// Redacts sensitive query parameter values (such as Jackett `apikey` / `jackett_apikey`,
/// `api_key`, `passkey`, and `token`) from URLs and error strings before logging,
/// rendering in the UI, or persisting to SQLite.
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

    if pos == 0 {
        return input.to_string();
    }
    out.push_str(&input[pos..]);
    out
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TorrentSearchResult {
    pub title: String,
    pub magnet_uri: String,
    pub seeders: u32,
    pub size_bytes: u64,
    pub provider: String,
}

#[async_trait]
pub trait TorrentSearchProvider: Send + Sync {
    fn provider_name(&self) -> &str;
    async fn search(&self, query: &str) -> Result<Vec<TorrentSearchResult>>;
}

pub struct TorznabSearchProvider {
    pub base_url: String,
    pub api_key: String,
    client: reqwest::Client,
}

impl TorznabSearchProvider {
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key: api_key.into(),
            client,
        }
    }

    pub fn detect_local_jackett() -> Option<Self> {
        let base_url = std::env::var("JACKETT_URL")
            .unwrap_or_else(|_| "http://localhost:9117".to_string());
        if let Ok(key) = std::env::var("JACKETT_API_KEY") {
            if !key.trim().is_empty() {
                return Some(Self::new(base_url, key.trim()));
            }
        }

        let mut candidates: Vec<PathBuf> = Vec::new();
        if let Some(cfg) = std::env::var_os("JACKETT_CONFIG").filter(|v| !v.is_empty()) {
            candidates.push(PathBuf::from(cfg));
        }
        if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
            candidates.push(PathBuf::from(xdg).join("Jackett/ServerConfig.json"));
        } else if let Some(home) = std::env::var_os("HOME").filter(|v| !v.is_empty()) {
            candidates.push(PathBuf::from(home).join(".config/Jackett/ServerConfig.json"));
        }

        for cfg_path in candidates {
            if cfg_path.exists() {
                if let Ok(raw) = std::fs::read_to_string(&cfg_path) {
                    if let Ok(val) = serde_json::from_str::<serde_json::Value>(&raw) {
                        if let Some(key) = val.get("APIKey").and_then(|v| v.as_str()) {
                            if !key.trim().is_empty() {
                                return Some(Self::new(base_url, key.trim()));
                            }
                        }
                    }
                }
            }
        }
        None
    }
}

#[async_trait]
impl TorrentSearchProvider for TorznabSearchProvider {
    fn provider_name(&self) -> &str {
        "Jackett/Torznab"
    }

    async fn search(&self, query: &str) -> Result<Vec<TorrentSearchResult>> {
        let q = urlencoding::encode(query.trim());
        let key = urlencoding::encode(&self.api_key);
        let url = format!(
            "{}/api/v2.0/indexers/all/results/torznab/api?apikey={}&t=search&cat=3000,3010,3040&q={}",
            self.base_url, key, q
        );
        let mut resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("request failed: {}", e.without_url()))?;
        let status = resp.status();
        if !status.is_success() {
            anyhow::bail!("HTTP {}", status);
        }
        if let Some(len) = resp.content_length() {
            if len > MAX_TORZNAB_RESPONSE_BYTES as u64 {
                anyhow::bail!(
                    "Torznab response too large ({len} bytes > {MAX_TORZNAB_RESPONSE_BYTES} byte limit)"
                );
            }
        }
        let mut buf = Vec::new();
        while let Some(chunk) = resp
            .chunk()
            .await
            .map_err(|e| anyhow::anyhow!("read failed: {}", e.without_url()))?
        {
            if buf.len().saturating_add(chunk.len()) > MAX_TORZNAB_RESPONSE_BYTES {
                anyhow::bail!(
                    "Torznab response exceeded {MAX_TORZNAB_RESPONSE_BYTES} byte limit"
                );
            }
            buf.extend_from_slice(&chunk);
        }
        let xml = String::from_utf8_lossy(&buf);
        Ok(parse_torznab_xml(&xml, self.provider_name()))
    }
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
}

pub struct OpenAudioFallbackProvider;

#[async_trait]
impl TorrentSearchProvider for OpenAudioFallbackProvider {
    fn provider_name(&self) -> &str {
        "OpenAudio (CC/Public Domain)"
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
    providers: Vec<Box<dyn TorrentSearchProvider>>,
}

impl CompositeSearchProvider {
    pub fn with_defaults() -> Self {
        let mut providers: Vec<Box<dyn TorrentSearchProvider>> = Vec::new();
        if let Some(jackett) = TorznabSearchProvider::detect_local_jackett() {
            providers.push(Box::new(jackett));
        }
        providers.push(Box::new(OpenAudioFallbackProvider));
        Self { providers }
    }

    pub async fn search_all(&self, query: &str) -> (Vec<TorrentSearchResult>, Vec<String>) {
        let mut merged = Vec::new();
        let mut errors = Vec::new();
        for p in &self.providers {
            match p.search(query).await {
                Ok(mut items) => merged.append(&mut items),
                Err(e) => errors.push(redact_url_secrets(&format!("{}: {}", p.provider_name(), e))),
            }
        }
        (merged, errors)
    }

    pub fn provider_names(&self) -> Vec<&str> {
        self.providers.iter().map(|p| p.provider_name()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_torznab_xml() {
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
    }
}
