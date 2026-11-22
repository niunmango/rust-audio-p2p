pub mod engine;
pub mod search;

pub use engine::{
    extract_magnet_display_name, is_torrent_uri, sanitize_torrent_uri, TorrentEngine,
    TorrentFileEntry, TorrentStatusSnapshot, DEFAULT_PORT_RANGE_END, DEFAULT_PORT_RANGE_START,
};
pub use search::{
    parse_torznab_indexers, parse_torznab_xml, redact_url_secrets, CompositeSearchProvider,
    InternetArchiveAudioProvider, OpenAudioFallbackProvider, SearchProgressEvent,
    TorrentSearchProvider, TorrentSearchResult, TorznabSearchProvider,
};
