use std::path::Path;

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

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
        let default_cfg = Path::new("/home/ramiro/media-stack/config/jackett/Jackett/ServerConfig.json");
        if default_cfg.exists() {
            if let Ok(raw) = std::fs::read_to_string(default_cfg) {
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(&raw) {
                    if let Some(key) = val.get("APIKey").and_then(|v| v.as_str()) {
                        if !key.trim().is_empty() {
                            return Some(Self::new(base_url, key.trim()));
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
        let xml = self.client.get(&url).send().await?.text().await?;
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
                Err(e) => errors.push(format!("{}: {}", p.provider_name(), e)),
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
}
