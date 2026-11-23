//! Portable Android FFI/C-ABI bridge sharing SQLite/FTS5, playlists, queue, offline travel pins,
//! i18n, battery/Wi-Fi transfer policies, and peer-to-peer catalog synchronization with mobile Kotlin.

use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use rust_audio_core::{
    AppPaths, Database, I18n, Language, LanguagePreference, LruCacheManager, PairedPeer,
    PlaybackQueue, DEFAULT_CACHE_LIMIT_BYTES,
};
use rust_audio_net_friends::{
    track_to_remote_dto, FriendNodeClient, NodeIdentity, PeerTicket, RemoteCatalogDto,
    RemoteTrackDto,
};
use serde::{Deserialize, Serialize};

/// Battery and metered-network safeguards controlling automatic P2P sync and seeding on Android.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkBatteryPolicy {
    pub wifi_only_sync: bool,
    pub charging_only_seed: bool,
    pub is_on_wifi: bool,
    pub is_charging: bool,
}

impl Default for NetworkBatteryPolicy {
    fn default() -> Self {
        Self {
            wifi_only_sync: true,
            charging_only_seed: true,
            is_on_wifi: false,
            is_charging: false,
        }
    }
}

impl NetworkBatteryPolicy {
    /// Returns true if batch track downloads or remote catalog syncing may run.
    pub fn allows_batch_download(&self) -> bool {
        !self.wifi_only_sync || self.is_on_wifi
    }

    /// Returns true if background peer seeding/serving is permitted.
    pub fn allows_seeding(&self) -> bool {
        (!self.wifi_only_sync || self.is_on_wifi)
            && (!self.charging_only_seed || self.is_charging)
    }
}

/// Mobile core session wrapping the shared SQLite library, offline cache, playback queue, and P2P identity.
pub struct AndroidCoreSession {
    pub paths: AppPaths,
    pub db: Arc<Mutex<Database>>,
    pub cache: LruCacheManager,
    pub i18n: I18n,
    pub queue: PlaybackQueue,
    pub identity: NodeIdentity,
    pub policy: NetworkBatteryPolicy,
}

impl AndroidCoreSession {
    /// Initializes SQLite, cache directories, locale selection (`es`/`en`), and saved policy settings.
    pub fn open(app_files_dir: &Path, app_cache_dir: &Path, device_locale: &str) -> Result<Self> {
        let data_dir = app_files_dir.join("data");
        let paths = AppPaths {
            db_path: data_dir.join("library.db"),
            data_dir,
            cache_dir: app_cache_dir.join("media"),
            offline_pins_dir: app_files_dir.join("offline_pins"),
            torrent_download_dir: app_cache_dir.join("torrents"),
        };
        paths.ensure_dirs()?;

        let db = Database::open(&paths.db_path)?;
        let lang_pref = match Language::from_locale_tag(device_locale) {
            Some(Language::Es) => LanguagePreference::Es,
            Some(Language::En) => LanguagePreference::En,
            None => LanguagePreference::Auto,
        };
        let i18n = I18n::new(lang_pref);

        let limit_bytes = db
            .get_setting("cache_limit_bytes")?
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|&b| b >= 128 * 1024 * 1024)
            .unwrap_or(DEFAULT_CACHE_LIMIT_BYTES);
        let cache = LruCacheManager::new(
            paths.cache_dir.clone(),
            paths.offline_pins_dir.clone(),
            limit_bytes,
        );
        let identity = NodeIdentity::load_or_create(&db)?;

        let wifi_only_sync = db
            .get_setting("android_wifi_only_sync")?
            .map(|s| s != "false" && s != "0")
            .unwrap_or(true);
        let charging_only_seed = db
            .get_setting("android_charging_only_seed")?
            .map(|s| s != "false" && s != "0")
            .unwrap_or(true);

        let policy = NetworkBatteryPolicy {
            wifi_only_sync,
            charging_only_seed,
            is_on_wifi: false,
            is_charging: false,
        };

        Ok(Self {
            paths,
            db: Arc::new(Mutex::new(db)),
            cache,
            i18n,
            queue: PlaybackQueue::new(),
            identity,
            policy,
        })
    }

    /// Updates and persists the Android Wi-Fi / battery policy flags in SQLite.
    pub fn set_network_battery_policy(&mut self, policy: NetworkBatteryPolicy) -> Result<()> {
        self.policy = policy;
        let guard = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("Database mutex poisoned"))?;
        guard.set_setting(
            "android_wifi_only_sync",
            if policy.wifi_only_sync { "1" } else { "0" },
        )?;
        guard.set_setting(
            "android_charging_only_seed",
            if policy.charging_only_seed { "1" } else { "0" },
        )?;
        Ok(())
    }

    /// Searches indexed tracks via SQLite FTS5 (or lists all tracks when query is empty).
    pub fn search_tracks(&self, query: &str) -> Result<Vec<RemoteTrackDto>> {
        let guard = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("Database mutex poisoned"))?;
        let tracks = if query.trim().is_empty() {
            guard.list_tracks(1000, 0)?
        } else {
            guard.search_tracks_fts(query, 300)?
        };
        Ok(tracks.iter().map(track_to_remote_dto).collect())
    }

    /// Creates a playlist and adds multiple tracks in a single batch.
    pub fn create_playlist_with_tracks(&self, name: &str, track_ids: &[i64]) -> Result<i64> {
        let clean = name.trim();
        anyhow::ensure!(!clean.is_empty(), "Playlist name cannot be empty");
        let guard = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("Database mutex poisoned"))?;
        let pl_id = guard.create_playlist(clean)?;
        if !track_ids.is_empty() {
            guard.add_tracks_to_playlist(pl_id, track_ids)?;
        }
        Ok(pl_id)
    }

    /// Renames an existing playlist.
    pub fn rename_playlist(&self, playlist_id: i64, new_name: &str) -> Result<()> {
        let guard = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("Database mutex poisoned"))?;
        guard.rename_playlist(playlist_id, new_name)
    }

    /// Deletes an existing playlist and its track associations.
    pub fn delete_playlist(&self, playlist_id: i64) -> Result<()> {
        let guard = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("Database mutex poisoned"))?;
        guard.delete_playlist(playlist_id)
    }

    /// Toggles the offline travel pin on a track.
    pub fn toggle_track_pin(&self, track_id: i64) -> Result<bool> {
        let guard = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("Database mutex poisoned"))?;
        let next = guard.toggle_track_pin(track_id)?;
        Ok(next)
    }

    /// Appends the requested track IDs to the in-memory playback queue and returns total enqueued.
    pub fn enqueue_track_ids(&mut self, track_ids: &[i64]) -> Result<usize> {
        let mut to_add = Vec::new();
        {
            let guard = self
                .db
                .lock()
                .map_err(|_| anyhow::anyhow!("Database mutex poisoned"))?;
            for &id in track_ids {
                if let Some(tr) = guard.get_track(id)? {
                    to_add.push(tr);
                }
            }
        }
        Ok(self.queue.enqueue_many(to_add))
    }

    /// Parses a `rap2p://` ticket URI or QR payload after SSRF endpoint validation.
    pub fn parse_peer_ticket(&self, raw_uri_or_qr: &str) -> Result<PeerTicket> {
        PeerTicket::parse(raw_uri_or_qr)
    }

    /// Connects to a friend P2P node and syncs its catalog only if the Wi-Fi policy permits it.
    pub async fn sync_peer_if_allowed(
        &self,
        raw_ticket: &str,
    ) -> Result<(PairedPeer, RemoteCatalogDto)> {
        if !self.policy.allows_batch_download() {
            let msg = match self.i18n.lang {
                Language::Es => "Sincronización P2P bloqueada por política 'Solo Wi-Fi'",
                Language::En => "P2P sync blocked by 'Wi-Fi only' policy",
            };
            anyhow::bail!("{msg}");
        }
        let client = FriendNodeClient::new();
        client.resolve_and_sync_peer(raw_ticket).await
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum AndroidBridgeRequest {
    Open {
        files_dir: PathBuf,
        cache_dir: PathBuf,
        #[serde(default)]
        locale: String,
    },
    SetPolicy {
        files_dir: PathBuf,
        cache_dir: PathBuf,
        #[serde(default)]
        locale: String,
        policy: NetworkBatteryPolicy,
    },
    Search {
        files_dir: PathBuf,
        cache_dir: PathBuf,
        #[serde(default)]
        locale: String,
        #[serde(default)]
        query: String,
    },
    CreatePlaylistWithTracks {
        files_dir: PathBuf,
        cache_dir: PathBuf,
        #[serde(default)]
        locale: String,
        name: String,
        #[serde(default)]
        track_ids: Vec<i64>,
    },
    RenamePlaylist {
        files_dir: PathBuf,
        cache_dir: PathBuf,
        #[serde(default)]
        locale: String,
        playlist_id: i64,
        new_name: String,
    },
    DeletePlaylist {
        files_dir: PathBuf,
        cache_dir: PathBuf,
        #[serde(default)]
        locale: String,
        playlist_id: i64,
    },
}

fn dispatch_json_request(raw: &str) -> Result<serde_json::Value> {
    let req: AndroidBridgeRequest = serde_json::from_str(raw)?;
    match req {
        AndroidBridgeRequest::Open {
            files_dir,
            cache_dir,
            locale,
        } => {
            let session = AndroidCoreSession::open(&files_dir, &cache_dir, &locale)?;
            Ok(serde_json::json!({
                "ok": true,
                "node_id": session.identity.node_id,
                "node_name": session.identity.node_name,
                "lang": session.i18n.lang.code(),
                "policy": session.policy,
            }))
        }
        AndroidBridgeRequest::SetPolicy {
            files_dir,
            cache_dir,
            locale,
            policy,
        } => {
            let mut session = AndroidCoreSession::open(&files_dir, &cache_dir, &locale)?;
            session.set_network_battery_policy(policy)?;
            Ok(serde_json::json!({ "ok": true, "policy": session.policy }))
        }
        AndroidBridgeRequest::Search {
            files_dir,
            cache_dir,
            locale,
            query,
        } => {
            let session = AndroidCoreSession::open(&files_dir, &cache_dir, &locale)?;
            let tracks = session.search_tracks(&query)?;
            Ok(serde_json::json!({ "ok": true, "tracks": tracks }))
        }
        AndroidBridgeRequest::CreatePlaylistWithTracks {
            files_dir,
            cache_dir,
            locale,
            name,
            track_ids,
        } => {
            let session = AndroidCoreSession::open(&files_dir, &cache_dir, &locale)?;
            let playlist_id = session.create_playlist_with_tracks(&name, &track_ids)?;
            Ok(serde_json::json!({ "ok": true, "playlist_id": playlist_id }))
        }
        AndroidBridgeRequest::RenamePlaylist {
            files_dir,
            cache_dir,
            locale,
            playlist_id,
            new_name,
        } => {
            let session = AndroidCoreSession::open(&files_dir, &cache_dir, &locale)?;
            session.rename_playlist(playlist_id, &new_name)?;
            Ok(serde_json::json!({ "ok": true }))
        }
        AndroidBridgeRequest::DeletePlaylist {
            files_dir,
            cache_dir,
            locale,
            playlist_id,
        } => {
            let session = AndroidCoreSession::open(&files_dir, &cache_dir, &locale)?;
            session.delete_playlist(playlist_id)?;
            Ok(serde_json::json!({ "ok": true }))
        }
    }
}

/// C-ABI JSON bridge entrypoint invoked from Android Kotlin/JNI.
/// Caller must release the returned pointer using [`rap2p_android_free_string`].
#[no_mangle]
pub unsafe extern "C" fn rap2p_android_invoke_json(req_ptr: *const c_char) -> *mut c_char {
    let response = if req_ptr.is_null() {
        serde_json::json!({ "ok": false, "error": "Null JSON request pointer" }).to_string()
    } else {
        let raw = unsafe { CStr::from_ptr(req_ptr) }.to_string_lossy();
        match dispatch_json_request(&raw) {
            Ok(val) => val.to_string(),
            Err(err) => serde_json::json!({ "ok": false, "error": err.to_string() }).to_string(),
        }
    };
    CString::new(response)
        .unwrap_or_else(|_| CString::new(r#"{"ok":false,"error":"CString conversion error"}"#).expect("static json"))
        .into_raw()
}

/// Frees a C string previously allocated by [`rap2p_android_invoke_json`].
#[no_mangle]
pub unsafe extern "C" fn rap2p_android_free_string(ptr: *mut c_char) {
    if !ptr.is_null() {
        unsafe {
            let _ = CString::from_raw(ptr);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_audio_core::{Track, TrackSource};

    fn temp_android_dirs(tag: &str) -> (PathBuf, PathBuf, PathBuf) {
        let base = std::env::temp_dir().join(format!(
            "rust_audio_android_test_{tag}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&base);
        let files = base.join("files");
        let cache = base.join("cache");
        (base, files, cache)
    }

    #[tokio::test]
    async fn test_android_session_locale_policy_playlists_queue_and_c_abi() {
        let (base, files, cache) = temp_android_dirs("core");
        let mut session = AndroidCoreSession::open(&files, &cache, "es-AR").unwrap();
        assert_eq!(session.i18n.lang, Language::Es);
        assert!(session.paths.db_path.exists());

        // Policy defaults to Wi-Fi-only sync and charging-only seeding
        assert!(!session.policy.allows_batch_download());
        assert!(!session.policy.allows_seeding());

        // Attempting peer sync over mobile data while wifi_only_sync = true must fail gracefully
        let blocked = session
            .sync_peer_if_allowed("rap2p://127.0.0.1:4261#deadbeef?name=air")
            .await;
        assert!(blocked.is_err());

        // Turning Wi-Fi & charging ON satisfies both safeguards and persists preferences
        session
            .set_network_battery_policy(NetworkBatteryPolicy {
                wifi_only_sync: true,
                charging_only_seed: true,
                is_on_wifi: true,
                is_charging: true,
            })
            .unwrap();
        assert!(session.policy.allows_batch_download());
        assert!(session.policy.allows_seeding());

        // Insert a track and verify FTS5 search, pin, playlist CRUD, and queue enqueue
        let tid = {
            let guard = session.db.lock().unwrap();
            guard
                .upsert_track(&Track {
                    id: 0,
                    title: "Spinetta Mobile".into(),
                    artist: "Pescado".into(),
                    album: "Artaud".into(),
                    year: Some(1973),
                    track_number: Some(1),
                    duration_secs: Some(215),
                    file_size_bytes: 1024,
                    format: "flac".into(),
                    cover_path: None,
                    source: TrackSource::LocalFile {
                        path: "/tmp/android_dummy.flac".into(),
                    },
                    pinned: false,
                })
                .unwrap()
        };
        assert_eq!(session.search_tracks("Spinetta").unwrap().len(), 1);
        assert!(session.toggle_track_pin(tid).unwrap());

        let pl_id = session
            .create_playlist_with_tracks("Viaje Android", &[tid])
            .unwrap();
        session.rename_playlist(pl_id, "Ruta Sur").unwrap();
        session.delete_playlist(pl_id).unwrap();
        assert_eq!(session.enqueue_track_ids(&[tid]).unwrap(), 1);
        assert_eq!(session.queue.len(), 1);

        // Re-open with en-US locale verifies persisted policy and English language selection
        let session_en = AndroidCoreSession::open(&files, &cache, "en-US").unwrap();
        assert_eq!(session_en.i18n.lang, Language::En);
        assert!(session_en.policy.wifi_only_sync);

        // Verify C ABI JSON bridge entrypoint
        let c_req = CString::new(
            serde_json::json!({
                "op": "search",
                "files_dir": files,
                "cache_dir": cache,
                "locale": "es-AR",
                "query": "Pescado"
            })
            .to_string(),
        )
        .unwrap();
        unsafe {
            let out_ptr = rap2p_android_invoke_json(c_req.as_ptr());
            assert!(!out_ptr.is_null());
            let parsed: serde_json::Value =
                serde_json::from_str(CStr::from_ptr(out_ptr).to_str().unwrap()).unwrap();
            assert_eq!(parsed["ok"], true);
            assert_eq!(parsed["tracks"].as_array().unwrap().len(), 1);
            rap2p_android_free_string(out_ptr);
        }

        let _ = std::fs::remove_dir_all(&base);
    }
}
