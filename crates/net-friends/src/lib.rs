//! Lightweight serverless friend-to-friend & LAN P2P audio sharing.

pub mod client;
pub mod protocol;
pub mod server;

pub use client::FriendNodeClient;
pub use protocol::{
    detect_lan_ip, verify_token_ct, NodeIdentity, NodeInfoDto, PeerTicket, RemoteCatalogDto,
    RemotePlaylistDto, RemoteTrackDto, DEFAULT_P2P_PORT, LAN_DISCOVERY_PORT,
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
}
