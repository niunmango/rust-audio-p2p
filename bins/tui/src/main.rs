mod app;
mod ui;

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::{CrosstermBackend, TestBackend};
use ratatui::Terminal;
use rust_audio_core::{
    detect_system_language, AppPaths, Database, Language, LanguagePreference,
    DEFAULT_CACHE_LIMIT_BYTES,
};
use rust_audio_source_local::{default_scan_root, scan_directory_sync};

use crate::app::{App, Tab, TorrentInputMode};

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
            other => {
                anyhow::bail!("Unknown argument: {other}. Try --help.");
            }
        }
    }
    Ok(out)
}

fn print_help(lang: Language) {
    match lang {
        Language::Es => {
            println!("rust-audio-tui v{} — Reproductor y red de audio P2P en Rust", env!("CARGO_PKG_VERSION"));
            println!("\nUSO:\n  rust-audio-tui [OPCIONES]\n");
            println!("OPCIONES:");
            println!("  --lang <auto|es|en>   Idioma de interfaz (por defecto: autodetectado)");
            println!("  --scan-dir <ruta>     Directorio raíz para auto-escaneo inicial (por defecto: $HOME)");
            println!("  --headless            Forzar backend de audio NullSink sin hardware ALSA");
            println!("  --self-test           Ejecutar verificación automática integral y salir");
            println!("  -V, --version         Mostrar versión");
            println!("  -h, --help            Mostrar esta ayuda");
        }
        Language::En => {
            println!("rust-audio-tui v{} — Distributed Rust Audio Player & P2P TUI", env!("CARGO_PKG_VERSION"));
            println!("\nUSAGE:\n  rust-audio-tui [OPTIONS]\n");
            println!("OPTIONS:");
            println!("  --lang <auto|es|en>   UI language (default: auto-detected)");
            println!("  --scan-dir <path>     Root directory for initial auto-scan (default: $HOME)");
            println!("  --headless            Force NullSink audio backend without ALSA hardware");
            println!("  --self-test           Run non-interactive end-to-end verification and exit");
            println!("  -V, --version         Print version");
            println!("  -h, --help            Print this help");
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = parse_args()?;
    if cli.version {
        println!("rust-audio-tui {}", env!("CARGO_PKG_VERSION"));
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
    if cli.self_test {
        return run_self_test(cli.scan_dir).await;
    }

    let paths = AppPaths::resolve_default();
    paths.ensure_dirs()?;
    let db = Database::open(&paths.db_path)?;
    let scan_root = cli.scan_dir.unwrap_or_else(default_scan_root);
    let mut app = App::new(paths, db, cli.lang, scan_root, cli.headless, true)?;

    run_tui_loop(&mut app).await
}

async fn run_tui_loop(app: &mut App) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let res = event_loop(&mut terminal, app).await;

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    if let Some(engine) = &app.torrent_engine {
        engine.stop().await;
    }
    res
}

async fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
) -> Result<()> {
    loop {
        app.poll_async_events();
        terminal.draw(|f| ui::draw(f, app))?;

        if event::poll(Duration::from_millis(120))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    handle_key(app, key)?;
                }
            }
        }
        if app.should_quit {
            break;
        }
    }
    Ok(())
}

fn handle_key(app: &mut App, key: KeyEvent) -> Result<()> {
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        app.should_quit = true;
        return Ok(());
    }

    // Handle active text input modes first
    if app.search_editing {
        match key.code {
            KeyCode::Esc => app.search_editing = false,
            KeyCode::Enter => {
                app.search_editing = false;
                app.play_selected_from_active_list()?;
            }
            KeyCode::Backspace => {
                app.search_query.pop();
                app.refresh_from_db()?;
            }
            KeyCode::Char(c) => {
                app.search_query.push(c);
                app.refresh_from_db()?;
            }
            _ => {}
        }
        return Ok(());
    }

    if app.playlist_editing {
        match key.code {
            KeyCode::Esc => app.playlist_editing = false,
            KeyCode::Enter => app.create_playlist_from_input()?,
            KeyCode::Backspace => {
                app.playlist_input.pop();
            }
            KeyCode::Char(c) => app.playlist_input.push(c),
            _ => {}
        }
        return Ok(());
    }

    if app.torrent_input_mode != TorrentInputMode::None {
        match key.code {
            KeyCode::Esc => app.torrent_input_mode = TorrentInputMode::None,
            KeyCode::Enter => {
                match app.torrent_input_mode {
                    TorrentInputMode::Search => app.trigger_torrent_search(),
                    TorrentInputMode::Magnet => {
                        let uri = app.torrent_magnet_input.trim().to_string();
                        if !uri.is_empty() {
                            app.add_torrent_uri(uri);
                        }
                    }
                    TorrentInputMode::None => {}
                }
                app.torrent_input_mode = TorrentInputMode::None;
            }
            KeyCode::Backspace => match app.torrent_input_mode {
                TorrentInputMode::Search => {
                    app.torrent_query.pop();
                }
                TorrentInputMode::Magnet => {
                    app.torrent_magnet_input.pop();
                }
                TorrentInputMode::None => {}
            },
            KeyCode::Char(c) => match app.torrent_input_mode {
                TorrentInputMode::Search => app.torrent_query.push(c),
                TorrentInputMode::Magnet => app.torrent_magnet_input.push(c),
                TorrentInputMode::None => {}
            },
            _ => {}
        }
        return Ok(());
    }

    // Global navigation and playback shortcuts
    match key.code {
        KeyCode::Char('q') => app.should_quit = true,
        KeyCode::Char('1') => app.active_tab = Tab::Library,
        KeyCode::Char('2') => app.active_tab = Tab::Search,
        KeyCode::Char('3') => app.active_tab = Tab::Playlists,
        KeyCode::Char('4') => app.active_tab = Tab::Torrents,
        KeyCode::Char('5') => app.active_tab = Tab::CacheSettings,
        KeyCode::Char('/') => {
            app.active_tab = Tab::Search;
            app.search_editing = true;
        }
        KeyCode::Char(' ') => app.player.toggle_pause()?,
        KeyCode::Left => app.player.seek_relative(-10.0)?,
        KeyCode::Right => app.player.seek_relative(10.0)?,
        KeyCode::Char('+') | KeyCode::Char('=') => {
            let v = app.player.status().volume + 0.05;
            app.player.set_volume(v)?;
        }
        KeyCode::Char('-') => {
            let v = app.player.status().volume - 0.05;
            app.player.set_volume(v)?;
        }
        KeyCode::Char('n') => {
            if app.active_tab == Tab::Playlists {
                app.playlist_editing = true;
            } else {
                app.next_track()?;
            }
        }
        KeyCode::Char('b') => app.prev_track()?,
        KeyCode::Char('p') => app.toggle_pin_selected()?,
        KeyCode::Char('a') => app.add_selected_to_default_playlist()?,
        KeyCode::Char('r') => app.trigger_rescan(),
        KeyCode::Char('l') => app.cycle_language(),
        KeyCode::Char('c') if app.active_tab == Tab::CacheSettings => app.purge_cache()?,
        KeyCode::Char('[') if app.active_tab == Tab::CacheSettings => app.adjust_cache_limit_mb(-512)?,
        KeyCode::Char(']') if app.active_tab == Tab::CacheSettings => app.adjust_cache_limit_mb(512)?,
        KeyCode::Char('s') if app.active_tab == Tab::Torrents => {
            app.torrent_input_mode = TorrentInputMode::Search;
        }
        KeyCode::Char('m') if app.active_tab == Tab::Torrents => {
            app.torrent_input_mode = TorrentInputMode::Magnet;
        }
        KeyCode::Up | KeyCode::Char('k') => move_selection(app, -1),
        KeyCode::Down | KeyCode::Char('j') => move_selection(app, 1),
        KeyCode::Enter => match app.active_tab {
            Tab::Library | Tab::Search => app.play_selected_from_active_list()?,
            Tab::Torrents => {
                if let Some(res) = app.torrent_results.get(app.torrent_result_selected) {
                    app.torrent_magnet_input = res.magnet_uri.clone();
                    app.add_torrent_uri(res.magnet_uri.clone());
                }
            }
            Tab::Playlists => {
                if let Some(pl) = app.playlists.get(app.playlist_selected) {
                    let tracks = app
                        .db
                        .lock()
                        .expect("db mutex poisoned")
                        .get_playlist_tracks(pl.id)?;
                    if !tracks.is_empty() {
                        app.queue.set_queue(tracks, 0);
                        app.play_current_in_queue()?;
                    }
                }
            }
            Tab::CacheSettings => {}
        },
        _ => {}
    }

    Ok(())
}

fn move_selection(app: &mut App, delta: isize) {
    let (sel, len) = match app.active_tab {
        Tab::Library => (&mut app.library_selected, app.tracks.len()),
        Tab::Search => (&mut app.search_selected, app.search_results.len()),
        Tab::Playlists => (&mut app.playlist_selected, app.playlists.len()),
        Tab::Torrents => (&mut app.torrent_result_selected, app.torrent_results.len()),
        Tab::CacheSettings => return,
    };
    if len == 0 {
        *sel = 0;
        return;
    }
    let next = (*sel as isize + delta).clamp(0, len as isize - 1) as usize;
    *sel = next;
}

async fn run_self_test(scan_override: Option<PathBuf>) -> Result<()> {
    println!("=== rust-audio-tui --self-test ===");

    let temp_base = std::env::temp_dir().join(format!("rust_audio_selftest_{}", std::process::id()));
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
        let local_music = Path::new("/home/ramiro/media-stack/data/media/music");
        if local_music.exists() {
            local_music.to_path_buf()
        } else {
            default_scan_root()
        }
    });

    let mut app = App::new(
        paths,
        db,
        Some(LanguagePreference::Auto),
        scan_root.clone(),
        true,
        false,
    )?;

    println!("[1/6] i18n detected language: {} ({})", app.i18n.lang.display_name(), app.i18n.lang.code());
    assert_eq!(app.cache.limit_bytes, DEFAULT_CACHE_LIMIT_BYTES);

    let indexed = scan_directory_sync(&scan_root, &app.db, Some(&app.scan_tx))?;
    app.refresh_from_db()?;
    println!("[2/6] Local scan ({}) indexed {} tracks", scan_root.display(), indexed);
    assert!(!app.tracks.is_empty(), "Expected at least 1 local audio track during self-test");

    app.search_query = app.tracks[0].artist.split_whitespace().next().unwrap_or("").to_string();
    app.refresh_from_db()?;
    println!("[3/6] FTS5 search for '{}' returned {} matches", app.search_query, app.search_results.len());
    assert!(!app.search_results.is_empty());

    app.playlist_input = "Travel SelfTest".to_string();
    app.create_playlist_from_input()?;
    app.toggle_pin_selected()?;
    println!("[4/6] Created playlist & pinned track offline (pinned count: {})", app.pinned_tracks.len());
    assert!(!app.pinned_tracks.is_empty());

    let (torrent_res, torrent_errs) = app.search_provider.search_all("bach").await;
    println!(
        "[5/6] TorrentSearchProvider ({}) returned {} result(s) (warnings: {})",
        app.search_provider.provider_names().join(", "),
        torrent_res.len(),
        torrent_errs.len()
    );
    assert!(!torrent_res.is_empty());
    app.torrent_results = torrent_res;

    app.play_selected_from_active_list()?;
    std::thread::sleep(Duration::from_millis(60));
    app.player.seek_to(30.0)?;
    std::thread::sleep(Duration::from_millis(40));
    let pst = app.player.status();
    println!(
        "[6/6] Player state={:?}, pos={:.1}s, prefetch={:?} ({} bytes)",
        pst.state,
        pst.position_secs,
        pst.prefetch_track.as_ref().map(|t| t.title.as_str()),
        pst.prefetched_bytes
    );

    // Render all 5 tabs in Spanish and English using TestBackend
    let mut terminal = Terminal::new(TestBackend::new(120, 35))?;
    for pref in [LanguagePreference::Es, LanguagePreference::En] {
        app.i18n.set_preference(pref);
        for tab in Tab::all() {
            app.active_tab = tab;
            terminal.draw(|f| ui::draw(f, &app))?;
        }
    }
    println!("[OK] Rendered all 5 TUI tabs in Es and En cleanly via ratatui::TestBackend.");

    let _ = std::fs::remove_dir_all(&temp_base);
    Ok(())
}
