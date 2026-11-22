//! Lightweight serverless friend-to-friend & LAN P2P audio sharing + Web UI / PWA.

pub mod client;
pub mod protocol;
pub mod server;
pub mod webui;

pub use client::FriendNodeClient;
pub use protocol::{
    detect_lan_ip, sanitize_display_text, track_to_remote_dto, validate_peer_endpoint,
    verify_token_ct, NodeIdentity, NodeInfoDto, PeerTicket, RemoteCatalogDto, RemotePlaylistDto,
    RemoteTrackDto, WebPeerSummary, WebPlayerSnapshot, WebPlaylistSummary, WebRemoteCommand,
    WebStateDto, DEFAULT_P2P_PORT, LAN_DISCOVERY_PORT,
};
pub use server::FriendNodeServer;

pub fn crate_name() -> &'static str {
    "rust-audio-net-friends"
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_audio_core::{Database, Track, TrackSource};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    #[test]
    fn test_peer_ticket_parse_roundtrip_and_qr() {
        let ticket = PeerTicket {
            node_id: "0123456789abcdef".to_string(),
            node_name: "air-server".to_string(),
            endpoint: "192.168.1.44:4261".to_string(),
            auth_token: "deadbeef01020304".to_string(),
        };
        let uri = ticket.to_uri();
        let parsed = PeerTicket::parse(&uri).unwrap();
        assert_eq!(parsed, ticket);

        let qr_lines = ticket.render_unicode_qr();
        assert!(qr_lines.len() >= 14);
        assert!(qr_lines.iter().any(|l| l.contains('█') || l.contains('▀') || l.contains('▄')));

        // Shorthand host:port#token
        let short = PeerTicket::parse("10.0.0.5:4262#secret99").unwrap();
        assert_eq!(short.endpoint, "10.0.0.5:4262");
        assert_eq!(short.auth_token, "secret99");

        // Bare IP defaults port 4261 and empty token (LAN discovery mode)
        let bare = PeerTicket::parse("192.168.1.99").unwrap();
        assert_eq!(bare.endpoint, "192.168.1.99:4261");
        assert!(bare.auth_token.is_empty());
    }

    #[tokio::test]
    async fn test_p2p_two_node_sync_auth_and_audio_stream() {
        let temp = std::env::temp_dir().join(format!(
            "rust_audio_p2p_e2e_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&temp);
        let src_dir = temp.join("server_music");
        let cache_dir = temp.join("client_cache");
        std::fs::create_dir_all(&src_dir).unwrap();
        std::fs::create_dir_all(&cache_dir).unwrap();

        let audio_file = src_dir.join("01 - Prueba.flac");
        let payload = b"fLaC_mock_audio_payload_for_p2p_streaming_test_1234567890";
        std::fs::write(&audio_file, payload).unwrap();

        let db_a = Arc::new(Mutex::new(Database::open_in_memory().unwrap()));
        let identity_a = NodeIdentity::load_or_create(&db_a.lock().unwrap()).unwrap();
        {
            let guard = db_a.lock().unwrap();
            let tid = guard
                .upsert_track(&Track {
                    id: 0,
                    title: "Prueba P2P".to_string(),
                    artist: "Invisible".to_string(),
                    album: "El Jardin".to_string(),
                    year: Some(1976),
                    track_number: Some(1),
                    duration_secs: Some(210),
                    file_size_bytes: payload.len() as u64,
                    format: "flac".to_string(),
                    cover_path: None,
                    source: TrackSource::LocalFile {
                        path: audio_file.to_string_lossy().to_string(),
                    },
                    pinned: false,
                })
                .unwrap();
            let pl_id = guard.create_playlist("Viaje Server").unwrap();
            guard.add_track_to_playlist(pl_id, tid).unwrap();
        }

        let server = FriendNodeServer::start_with_options(
            Arc::clone(&db_a),
            identity_a.clone(),
            Some(0),
            None,
        )
        .await
        .unwrap();

        let client = FriendNodeClient::new();

        // Wrong token must fail with 403
        let bad_ticket = format!("rap2p://127.0.0.1:{}#wrongtoken", server.port);
        let bad_res = client.resolve_and_sync_peer(&bad_ticket).await;
        assert!(bad_res.is_err());

        // LAN Share OFF must block bare IP lan-ticket
        server.set_lan_share_enabled(&db_a.lock().unwrap(), false);
        let bare_addr = format!("127.0.0.1:{}", server.port);
        assert!(client.resolve_and_sync_peer(&bare_addr).await.is_err());

        // Even with LAN Share OFF, full ticket with token still succeeds
        let full_uri = server.ticket().to_uri();
        let (peer, catalog) = client.resolve_and_sync_peer(&full_uri).await.unwrap();
        assert_eq!(peer.node_id, identity_a.node_id);
        assert_eq!(catalog.tracks.len(), 1);
        assert_eq!(catalog.tracks[0].title, "Prueba P2P");
        assert_eq!(catalog.playlists.len(), 1);
        assert_eq!(catalog.playlists[0].name, "Viaje Server");

        // LAN Share ON allows connecting with bare IP:port
        server.set_lan_share_enabled(&db_a.lock().unwrap(), true);
        let (peer_lan, _) = client.resolve_and_sync_peer(&bare_addr).await.unwrap();
        assert_eq!(peer_lan.auth_token, identity_a.auth_token);

        // Stream audio file to client cache and verify exact bytes + Range request
        let cached = client
            .download_peer_track_to_cache(&peer, catalog.tracks[0].id, &catalog.tracks[0].format, &cache_dir)
            .await
            .unwrap();
        assert_eq!(std::fs::read(&cached).unwrap(), payload);

        // Verify byte-range streaming endpoint works directly
        let range_resp = reqwest::Client::new()
            .get(format!("http://127.0.0.1:{}/api/v1/stream/{}", server.port, catalog.tracks[0].id))
            .header("X-Peer-Token", &identity_a.auth_token)
            .header("Range", "bytes=5-14")
            .timeout(Duration::from_secs(5))
            .send()
            .await
            .unwrap();
        assert_eq!(range_resp.status().as_u16(), 206);
        assert_eq!(range_resp.bytes().await.unwrap().as_ref(), &payload[5..15]);

        server.shutdown();
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[tokio::test]
    async fn test_webui_pwa_search_query_token_and_remote_control() {
        let temp = std::env::temp_dir().join(format!(
            "rust_audio_webui_test_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(&temp).unwrap();

        let audio_file = temp.join("track.flac");
        let payload = b"fLaC_web_stream_test_bytes";
        std::fs::write(&audio_file, payload).unwrap();

        let db = Arc::new(Mutex::new(Database::open_in_memory().unwrap()));
        let identity = NodeIdentity::load_or_create(&db.lock().unwrap()).unwrap();
        let tid = db
            .lock()
            .unwrap()
            .upsert_track(&Track {
                id: 0,
                title: "Web Song".to_string(),
                artist: "Web Artist".to_string(),
                album: "Web Album".to_string(),
                year: Some(2025),
                track_number: Some(1),
                duration_secs: Some(180),
                file_size_bytes: payload.len() as u64,
                format: "flac".to_string(),
                cover_path: None,
                source: TrackSource::LocalFile {
                    path: audio_file.to_string_lossy().to_string(),
                },
                pinned: false,
            })
            .unwrap();

        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::unbounded_channel();
        let server = FriendNodeServer::start_with_full_options(
            Arc::clone(&db),
            identity.clone(),
            Some(0),
            None,
            Some(cmd_tx),
        )
        .await
        .unwrap();

        let base = format!("http://127.0.0.1:{}", server.port);
        let http = reqwest::Client::new();

        // 1. Public Web UI & PWA assets
        let index_res = http.get(format!("{base}/")).send().await.unwrap();
        assert_eq!(index_res.status().as_u16(), 200);
        let html = index_res.text().await.unwrap();
        assert!(html.contains("rust-audio-p2p"));
        assert!(html.contains("/manifest.json"));

        let manifest_res = http.get(format!("{base}/manifest.json")).send().await.unwrap();
        assert_eq!(manifest_res.status().as_u16(), 200);
        assert!(manifest_res.text().await.unwrap().contains("rust-audio"));

        // 2. Query-param token auth for browser <audio>, search, and cover
        let search_res = http
            .get(format!("{base}/api/v1/search?q=Web&token={}", identity.auth_token))
            .send()
            .await
            .unwrap();
        assert_eq!(search_res.status().as_u16(), 200);
        let found: Vec<RemoteTrackDto> = search_res.json().await.unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, tid);

        let stream_res = http
            .get(format!("{base}/api/v1/stream/{tid}?token={}", identity.auth_token))
            .send()
            .await
            .unwrap();
        assert_eq!(stream_res.status().as_u16(), 200);
        assert_eq!(stream_res.headers().get("content-type").unwrap(), "audio/flac");
        assert_eq!(stream_res.bytes().await.unwrap().as_ref(), payload);

        let cover_res = http
            .get(format!("{base}/api/v1/cover/{tid}?token={}", identity.auth_token))
            .send()
            .await
            .unwrap();
        assert_eq!(cover_res.status().as_u16(), 200);

        // 3. POST /api/v1/control mutates DB + forwards playback command
        let ctrl_pl = http
            .post(format!("{base}/api/v1/control"))
            .header("X-Peer-Token", &identity.auth_token)
            .json(&WebRemoteCommand::CreatePlaylist {
                name: "From Web".to_string(),
                track_id: Some(tid),
            })
            .send()
            .await
            .unwrap();
        assert_eq!(ctrl_pl.status().as_u16(), 200);

        let ctrl_play = http
            .post(format!("{base}/api/v1/control"))
            .header("X-Peer-Token", &identity.auth_token)
            .json(&WebRemoteCommand::PlayTrack { track_id: tid })
            .send()
            .await
            .unwrap();
        assert_eq!(ctrl_play.status().as_u16(), 200);

        let state_res = http
            .get(format!("{base}/api/v1/state"))
            .header("X-Peer-Token", &identity.auth_token)
            .send()
            .await
            .unwrap();
        assert_eq!(state_res.status().as_u16(), 200);
        let state_dto: WebStateDto = state_res.json().await.unwrap();
        assert_eq!(state_dto.playlists.len(), 1);
        assert_eq!(state_dto.playlists[0].name, "From Web");
        assert_eq!(state_dto.playlists[0].track_ids, vec![tid]);

        let ev1 = cmd_rx.try_recv().unwrap();
        assert!(matches!(ev1, WebRemoteCommand::SyncRefresh { .. }));
        let ev2 = cmd_rx.try_recv().unwrap();
        assert_eq!(ev2, WebRemoteCommand::PlayTrack { track_id: tid });

        server.shutdown();
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[tokio::test]
    async fn test_security_ssrf_dns_rebinding_csrf_and_headers() {
        // 1. SSRF port & link-local checks in PeerTicket::parse
        assert!(PeerTicket::parse("127.0.0.1:9117").is_err());
        assert!(PeerTicket::parse("127.0.0.1:8080").is_err());
        assert!(PeerTicket::parse("169.254.169.254:80").is_err());
        assert!(PeerTicket::parse("0.0.0.0:4261").is_err());
        assert!(PeerTicket::parse("[fe80::1]:4261").is_err());

        // 2. DNS rebinding (Host), cross-site Sec-Fetch-Site, and HTTP security headers
        let db = Arc::new(Mutex::new(Database::open_in_memory().unwrap()));
        let identity = NodeIdentity::load_or_create(&db.lock().unwrap()).unwrap();
        let server = FriendNodeServer::start_with_options(
            Arc::clone(&db),
            identity.clone(),
            Some(0),
            None,
        )
        .await
        .unwrap();

        let base = format!("http://127.0.0.1:{}", server.port);
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();

        // Security headers must be present on Web UI responses
        let ok_res = http.get(&base).send().await.unwrap();
        assert_eq!(ok_res.status().as_u16(), 200);
        assert_eq!(
            ok_res.headers().get("x-content-type-options").unwrap(),
            "nosniff"
        );
        assert!(ok_res.headers().contains_key("content-security-policy"));

        // External Host header simulating DNS rebinding must return 403 Forbidden
        let rebind_res = http
            .get(format!("{base}/api/v1/lan-ticket"))
            .header("Host", format!("evil.attacker.com:{}", server.port))
            .send()
            .await
            .unwrap();
        assert_eq!(rebind_res.status().as_u16(), 403);

        // Cross-site request on API must return 403 Forbidden
        let csrf_res = http
            .get(format!("{base}/api/v1/lan-ticket"))
            .header("Sec-Fetch-Site", "cross-site")
            .send()
            .await
            .unwrap();
        assert_eq!(csrf_res.status().as_u16(), 403);

        // Untrusted browser Origin must return 403 Forbidden
        let origin_res = http
            .get(format!("{base}/api/v1/lan-ticket"))
            .header("Origin", "https://evil.example")
            .send()
            .await
            .unwrap();
        assert_eq!(origin_res.status().as_u16(), 403);

        server.shutdown();
    }

    #[tokio::test]
    async fn test_webui_p2p_library_search_stream_and_playlist_management() {
        let temp = std::env::temp_dir().join(format!(
            "rust_audio_webui_p2p_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(&temp).unwrap();

        let audio_file = temp.join("remote_track.flac");
        let payload = b"fLaC_remote_p2p_web_stream_payload_123456";
        std::fs::write(&audio_file, payload).unwrap();

        // Node A holds a local track
        let db_a = Arc::new(Mutex::new(Database::open_in_memory().unwrap()));
        let mut id_a = NodeIdentity::load_or_create(&db_a.lock().unwrap()).unwrap();
        id_a.node_name = "server-air".to_string();
        let remote_tid = db_a
            .lock()
            .unwrap()
            .upsert_track(&Track {
                id: 0,
                title: "Samba Remota".to_string(),
                artist: "Spinetta".to_string(),
                album: "Nodos".to_string(),
                year: Some(1977),
                track_number: Some(1),
                duration_secs: Some(195),
                file_size_bytes: payload.len() as u64,
                format: "flac".to_string(),
                cover_path: None,
                source: TrackSource::LocalFile {
                    path: audio_file.to_string_lossy().to_string(),
                },
                pinned: false,
            })
            .unwrap();
        let server_a =
            FriendNodeServer::start_with_options(Arc::clone(&db_a), id_a.clone(), Some(0), None)
                .await
                .unwrap();

        // Node B pairs Node A with uncached TrackSource::Peer + has one local track
        let db_b = Arc::new(Mutex::new(Database::open_in_memory().unwrap()));
        let id_b = NodeIdentity::load_or_create(&db_b.lock().unwrap()).unwrap();
        let local_b_file = temp.join("local_b.flac");
        std::fs::write(&local_b_file, b"fLaC_local_b").unwrap();
        let (local_b_tid, peer_tid_on_b) = {
            let guard = db_b.lock().unwrap();
            guard
                .save_paired_peers(&[rust_audio_core::PairedPeer {
                    node_id: id_a.node_id.clone(),
                    node_name: id_a.node_name.clone(),
                    endpoint: format!("127.0.0.1:{}", server_a.port),
                    auth_token: id_a.auth_token.clone(),
                    track_count: 1,
                    playlist_count: 0,
                }])
                .unwrap();
            let t_local = guard
                .upsert_track(&Track {
                    id: 0,
                    title: "Local Track B".to_string(),
                    artist: "Local".to_string(),
                    album: "Kubuntu".to_string(),
                    year: Some(2025),
                    track_number: Some(1),
                    duration_secs: Some(100),
                    file_size_bytes: 12,
                    format: "flac".to_string(),
                    cover_path: None,
                    source: TrackSource::LocalFile {
                        path: local_b_file.to_string_lossy().to_string(),
                    },
                    pinned: false,
                })
                .unwrap();
            let t_peer = guard
                .upsert_track(&Track {
                    id: 0,
                    title: "Samba Remota".to_string(),
                    artist: "Spinetta".to_string(),
                    album: "Nodos".to_string(),
                    year: Some(1977),
                    track_number: Some(1),
                    duration_secs: Some(195),
                    file_size_bytes: payload.len() as u64,
                    format: "flac".to_string(),
                    cover_path: None,
                    source: TrackSource::Peer {
                        node_id: id_a.node_id.clone(),
                        node_name: id_a.node_name.clone(),
                        remote_track_id: remote_tid,
                        cached_path: None,
                    },
                    pinned: false,
                })
                .unwrap();
            (t_local, t_peer)
        };
        let server_b =
            FriendNodeServer::start_with_options(Arc::clone(&db_b), id_b.clone(), Some(0), None)
                .await
                .unwrap();

        let base_b = format!("http://127.0.0.1:{}", server_b.port);
        let http = reqwest::Client::new();

        // 1. Catalog WITHOUT include_peer must NOT re-export Node A's peer track
        let cat_no_peer: RemoteCatalogDto = http
            .get(format!("{base_b}/api/v1/catalog"))
            .header("X-Peer-Token", &id_b.auth_token)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(cat_no_peer.tracks.len(), 1);
        assert_eq!(cat_no_peer.tracks[0].id, local_b_tid);

        // 2. Catalog WITH include_peer=1 (Web UI 1. Library) MUST include uncached P2P track + badge
        let cat_with_peer: RemoteCatalogDto = http
            .get(format!("{base_b}/api/v1/catalog?include_peer=1"))
            .header("X-Peer-Token", &id_b.auth_token)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(cat_with_peer.tracks.len(), 2);
        let remote_dto = cat_with_peer
            .tracks
            .iter()
            .find(|t| t.id == peer_tid_on_b)
            .unwrap();
        assert_eq!(remote_dto.peer_node.as_deref(), Some("server-air"));

        // 3. /api/v1/search?q=Samba MUST find the uncached P2P track too
        let search_hits: Vec<RemoteTrackDto> = http
            .get(format!(
                "{base_b}/api/v1/search?q=Samba&token={}",
                id_b.auth_token
            ))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(search_hits.len(), 1);
        assert_eq!(search_hits[0].id, peer_tid_on_b);
        assert_eq!(search_hits[0].peer_node.as_deref(), Some("server-air"));

        // 4. /api/v1/stream/<peer_tid_on_b> transparently fetches from Node A & streams bytes
        let stream_resp = http
            .get(format!(
                "{base_b}/api/v1/stream/{peer_tid_on_b}?token={}",
                id_b.auth_token
            ))
            .send()
            .await
            .unwrap();
        assert_eq!(stream_resp.status().as_u16(), 200);
        assert_eq!(stream_resp.bytes().await.unwrap().as_ref(), payload);

        // Clean up cached file written during on-demand fetch
        if let Ok(Some(updated)) = db_b.lock().unwrap().get_track(peer_tid_on_b) {
            if let Some(p) = updated.source.playable_path() {
                let _ = std::fs::remove_file(p);
            }
        }

        // 5. Batch playlist creation, rename, remove track, delete playlist
        let create_res = http
            .post(format!("{base_b}/api/v1/control"))
            .header("X-Peer-Token", &id_b.auth_token)
            .json(&WebRemoteCommand::CreatePlaylistWithTracks {
                name: "Mix Nodos".to_string(),
                track_ids: vec![local_b_tid, peer_tid_on_b],
            })
            .send()
            .await
            .unwrap();
        assert_eq!(create_res.status().as_u16(), 200);

        let st1: WebStateDto = http
            .get(format!("{base_b}/api/v1/state"))
            .header("X-Peer-Token", &id_b.auth_token)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(st1.playlists.len(), 1);
        let pl_id = st1.playlists[0].id;
        assert_eq!(st1.playlists[0].track_ids.len(), 2);

        http.post(format!("{base_b}/api/v1/control"))
            .header("X-Peer-Token", &id_b.auth_token)
            .json(&WebRemoteCommand::RenamePlaylist {
                playlist_id: pl_id,
                new_name: "Mix Final".to_string(),
            })
            .send()
            .await
            .unwrap();

        http.post(format!("{base_b}/api/v1/control"))
            .header("X-Peer-Token", &id_b.auth_token)
            .json(&WebRemoteCommand::RemoveFromPlaylist {
                playlist_id: pl_id,
                track_id: local_b_tid,
            })
            .send()
            .await
            .unwrap();

        let st2: WebStateDto = http
            .get(format!("{base_b}/api/v1/state"))
            .header("X-Peer-Token", &id_b.auth_token)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(st2.playlists[0].name, "Mix Final");
        assert_eq!(st2.playlists[0].track_ids, vec![peer_tid_on_b]);

        http.post(format!("{base_b}/api/v1/control"))
            .header("X-Peer-Token", &id_b.auth_token)
            .json(&WebRemoteCommand::DeletePlaylist { playlist_id: pl_id })
            .send()
            .await
            .unwrap();

        let st3: WebStateDto = http
            .get(format!("{base_b}/api/v1/state"))
            .header("X-Peer-Token", &id_b.auth_token)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert!(st3.playlists.is_empty());

        server_b.shutdown();
        server_a.shutdown();
        let _ = std::fs::remove_dir_all(&temp);
    }
}
