mod state;
mod ui;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Result;
use eframe::egui;
use rust_audio_core::{
    detect_system_language, AppPaths, Database, Language, LanguagePreference,
    DEFAULT_CACHE_LIMIT_BYTES,
};
use rust_audio_net_friends::{FriendNodeClient, FriendNodeServer, NodeIdentity};
use rust_audio_source_local::{default_scan_root, scan_directory_sync};

use crate::state::{GuiState, GuiTab};

#[derive(Debug)]
struct CliArgs {
    help: bool,
    version: bool,
    self_test: bool,
    headless: bool,
    lang: Option<LanguagePreference>,
    scan_dir: Option<PathBuf>,
}

fn parse_args() -> Result<CliArgs> {
    let mut out = CliArgs {
        help: false,
        version: false,
        self_test: false,
        headless: false,
        lang: None,
        scan_dir: None,
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => out.help = true,
            "-V" | "--version" => out.version = true,
            "--self-test" => out.self_test = true,
            "--headless" => out.headless = true,
            "--lang" => {
                let val = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--lang requires auto|es|en"))?;
                out.lang = Some(LanguagePreference::parse(&val));
            }
            "--scan-dir" => {
                let val = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--scan-dir requires a path"))?;
                out.scan_dir = Some(PathBuf::from(val));
            }
            other => anyhow::bail!("Unknown argument: {other}. Try --help."),
        }
    }
    Ok(out)
}

fn print_help(lang: Language) {
    match lang {
        Language::Es => {
            println!("rust-audio-gui v{} — Interfaz gráfica nativa (eframe/glow)", env!("CARGO_PKG_VERSION"));
            println!("\nUSO:\n  rust-audio-gui [OPCIONES]\n");
            println!("OPCIONES:");
            println!("  --lang <auto|es|en>   Idioma de interfaz (por defecto: autodetectado)");
            println!("  --scan-dir <ruta>     Directorio raíz para auto-escaneo inicial");
            println!("  --headless            Forzar backend NullSink sin hardware de audio");
            println!("  --self-test           Ejecutar verificación automática headless y salir");
            println!("  -V, --version         Mostrar versión");
            println!("  -h, --help            Mostrar esta ayuda");
        }
        Language::En => {
            println!("rust-audio-gui v{} — Native Desktop GUI (eframe/glow)", env!("CARGO_PKG_VERSION"));
            println!("\nUSAGE:\n  rust-audio-gui [OPTIONS]\n");
            println!("OPTIONS:");
            println!("  --lang <auto|es|en>   UI language (default: auto-detected)");
            println!("  --scan-dir <path>     Root directory for initial auto-scan");
            println!("  --headless            Force NullSink audio backend");
            println!("  --self-test           Run non-interactive headless check and exit");
            println!("  -V, --version         Print version");
            println!("  -h, --help            Print this help");
        }
    }
}

struct DesktopApp {
    state: GuiState,
}

impl eframe::App for DesktopApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.state.poll_async_events();
        ui::render_gui(ctx, &mut self.state);
        ctx.request_repaint_after(Duration::from_millis(150));
    }
}

fn main() -> Result<()> {
    let cli = parse_args()?;
    if cli.version {
        println!("rust-audio-gui {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if cli.help {
        let lang = cli
            .lang
            .map(|p| p.resolve())
            .unwrap_or_else(detect_system_language);
        print_help(lang);
        return Ok(());
    }

    let rt = tokio::runtime::Runtime::new()?;
    let _rt_guard = rt.enter();

    if cli.self_test {
        return rt.block_on(run_self_test(cli.scan_dir));
    }

    let paths = AppPaths::resolve_default();
    paths.ensure_dirs()?;
    let db = Database::open(&paths.db_path)?;
    let scan_root = cli.scan_dir.unwrap_or_else(default_scan_root);
    let state = GuiState::new(paths, db, cli.lang, scan_root, cli.headless, true)?;

    let native_options = eframe::NativeOptions {
        renderer: eframe::Renderer::Glow,
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1140.0, 740.0])
            .with_min_inner_size([800.0, 520.0])
            .with_title("rust-audio-p2p"),
        ..Default::default()
    };

    eframe::run_native(
        "rust-audio-p2p",
        native_options,
        Box::new(move |cc| {
            ui::apply_catppuccin_dark(&cc.egui_ctx);
            Ok(Box::new(DesktopApp { state }))
        }),
    )
    .map_err(|e| anyhow::anyhow!("eframe launch failed: {e}"))?;

    Ok(())
}

async fn run_self_test(scan_override: Option<PathBuf>) -> Result<()> {
    println!("=== rust-audio-gui --self-test ===");

    let temp_base =
        std::env::temp_dir().join(format!("rust_audio_gui_selftest_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&temp_base);
    let paths = AppPaths {
        data_dir: temp_base.join("data"),
        cache_dir: temp_base.join("cache/media"),
        offline_pins_dir: temp_base.join("cache/offline"),
        torrent_download_dir: temp_base.join("cache/torrents"),
        db_path: temp_base.join("data/library.db"),
    };
    paths.ensure_dirs()?;

    let db = Database::open(&paths.db_path)?;
    let scan_root = scan_override.unwrap_or_else(|| {
        let sample_dir = temp_base.join("sample_music/TestArtist/TestAlbum (2025)");
        let _ = std::fs::create_dir_all(&sample_dir);
        let _ = std::fs::write(sample_dir.join("01 - Gui Sample.wav"), b"RIFF....WAVE");
        let _ = std::fs::write(sample_dir.join("02 - Next Sample.wav"), b"RIFF....WAVE");
        temp_base.join("sample_music")
    });

    let mut state = GuiState::new(
        paths,
        db,
        Some(LanguagePreference::Auto),
        scan_root.clone(),
        true,
        false,
    )?;

    assert_eq!(state.cache.limit_bytes, DEFAULT_CACHE_LIMIT_BYTES);
    let indexed = scan_directory_sync(&scan_root, &state.db, Some(&state.scan_tx))?;
    state.refresh_from_db()?;
    println!("[1/5] Scan ({}): indexed={} tracks", scan_root.display(), indexed);
    assert!(!state.tracks.is_empty());

    state.search_query = state.tracks[0].title.split_whitespace().next().unwrap_or("").to_string();
    state.refresh_from_db()?;
    println!("[2/5] FTS5 search returned {} matches", state.search_results.len());
    assert!(!state.search_results.is_empty());

    let first_track = state.tracks[0].clone();
    state.create_playlist_and_add_track("GUI Travel", &first_track)?;
    state.toggle_track_pin(&first_track)?;
    println!(
        "[3/5] Playlists & Pins verified (playlists={}, pinned={})",
        state.playlists.len(),
        state.pinned_tracks.len()
    );
    assert_eq!(state.playlists.len(), 1);
    assert!(!state.pinned_tracks.is_empty());

    let peer_audio = temp_base.join("peer_gui.wav");
    std::fs::write(&peer_audio, b"RIFF....WAVE_gui_peer")?;
    let peer_db = std::sync::Arc::new(std::sync::Mutex::new(Database::open_in_memory()?));
    let peer_id = NodeIdentity::load_or_create(&peer_db.lock().unwrap())?;
    {
        let g = peer_db.lock().unwrap();
        g.upsert_track(&rust_audio_core::Track {
            id: 0,
            title: "GUI Remote Track".to_string(),
            artist: "Peer Artist".to_string(),
            album: "Shared Album".to_string(),
            year: Some(2025),
            track_number: Some(1),
            duration_secs: Some(60),
            file_size_bytes: 21,
            format: "wav".to_string(),
            cover_path: None,
            source: rust_audio_core::TrackSource::LocalFile {
                path: peer_audio.to_string_lossy().to_string(),
            },
            pinned: false,
        })?;
    }
    let srv = FriendNodeServer::start_with_options(peer_db, peer_id.clone(), Some(0), None).await?;
    let client = FriendNodeClient::new();
    let (paired_peer, catalog) = client.resolve_and_sync_peer(&srv.ticket().to_uri()).await?;
    state.apply_synced_peer(paired_peer, catalog)?;
    let _web_state = client.verify_web_ui(&srv.web_url(), &peer_id.auth_token).await?;
    srv.shutdown();
    println!("[4/5] P2P sync & Web UI bridge verified (peers={})", state.paired_peers.len());

    // Render all 6 GUI tabs headlessly via egui::Context::run in both Es & En
    let ctx = egui::Context::default();
    ui::apply_catppuccin_dark(&ctx);
    for pref in [LanguagePreference::Es, LanguagePreference::En] {
        state.set_language_preference(pref);
        for tab in GuiTab::all() {
            state.active_tab = tab;
            let _ = ctx.run(egui::RawInput::default(), |c| {
                ui::render_gui(c, &mut state);
            });
        }
    }
    println!("[5/5] Rendered all 6 native egui tabs headlessly in Es & En cleanly.");

    let _ = Instant::now();
    let _ = std::fs::remove_dir_all(&temp_base);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_audio_core::{Track, TrackSource};

    #[test]
    fn test_headless_egui_renders_all_6_tabs_in_es_and_en() {
        let temp_base = std::env::temp_dir().join(format!(
            "rust_audio_gui_unit_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let paths = AppPaths {
            data_dir: temp_base.join("data"),
            cache_dir: temp_base.join("cache/media"),
            offline_pins_dir: temp_base.join("cache/offline"),
            torrent_download_dir: temp_base.join("cache/torrents"),
            db_path: temp_base.join("data/library.db"),
        };
        paths.ensure_dirs().unwrap();
        let db = Database::open(&paths.db_path).unwrap();
        db.upsert_track(&Track {
            id: 0,
            title: "Barro Tal Vez".to_string(),
            artist: "Luis Alberto Spinetta".to_string(),
            album: "Kamikaze".to_string(),
            year: Some(1982),
            track_number: Some(1),
            duration_secs: Some(190),
            file_size_bytes: 1024,
            format: "flac".to_string(),
            cover_path: None,
            source: TrackSource::LocalFile {
                path: "/tmp/dummy_spinetta.flac".to_string(),
            },
            pinned: false,
        })
        .unwrap();

        let mut state = GuiState::new(
            paths,
            db,
            Some(LanguagePreference::Es),
            temp_base.clone(),
            true,
            false,
        )
        .unwrap();

        let ctx = egui::Context::default();
        ui::apply_catppuccin_dark(&ctx);
        for pref in [LanguagePreference::Es, LanguagePreference::En] {
            state.set_language_preference(pref);
            for tab in GuiTab::all() {
                state.active_tab = tab;
                let output = ctx.run(egui::RawInput::default(), |c| {
                    ui::render_gui(c, &mut state);
                });
                assert!(!output.shapes.is_empty());
            }
        }

        let _ = std::fs::remove_dir_all(&temp_base);
    }
}
