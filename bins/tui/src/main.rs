mod app;
mod ui;

use std::io;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::{CrosstermBackend, TestBackend};
use ratatui::layout::Rect;
use ratatui::Terminal;
use rust_audio_core::{
    detect_system_language, AppPaths, Database, I18n, Language, LanguagePreference,
    DEFAULT_CACHE_LIMIT_BYTES,
};
use rust_audio_net_friends::{FriendNodeClient, FriendNodeServer, NodeIdentity};
use rust_audio_source_local::{default_scan_root, scan_directory_sync};
use rust_audio_source_torrent::{is_torrent_uri, sanitize_torrent_uri};

use crate::app::{App, PlaylistPanelFocus, Tab, TorrentInputMode};

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

struct TerminalCleanup;

impl Drop for TerminalCleanup {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let mut stdout = io::stdout();
        let _ = execute!(
            stdout,
            LeaveAlternateScreen,
            DisableMouseCapture,
            DisableBracketedPaste
        );
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

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableBracketedPaste
    )?;
    let _cleanup = TerminalCleanup;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Render immediate minimal startup screen before opening SQLite / launching async workers.
    let boot_i18n = I18n::new(cli.lang.unwrap_or(LanguagePreference::Auto));
    terminal.draw(|f| ui::draw_loading_screen(f, &boot_i18n))?;

    let paths = AppPaths::resolve_default();
    paths.ensure_dirs()?;
    let db = Database::open(&paths.db_path)?;
    let scan_root = cli.scan_dir.unwrap_or_else(default_scan_root);
    let mut app = App::new(paths, db, cli.lang, scan_root, cli.headless, true)?;

    let res = event_loop(&mut terminal, &mut app).await;
    terminal.show_cursor()?;
    if let Some(srv) = &app.p2p_server {
        srv.shutdown();
    }
    if let Some(engine) = &app.torrent_engine {
        engine.stop().await;
    }
    res
}

async fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
) -> Result<()> {
    let mut last_click: Option<(Instant, u16, u16)> = None;
    loop {
        app.poll_async_events();
        terminal.draw(|f| ui::draw(f, app))?;

        if event::poll(Duration::from_millis(120))? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    handle_key(app, key)?;
                }
                Event::Mouse(mouse) => {
                    handle_mouse(app, mouse, &mut last_click)?;
                }
                Event::Paste(text) => {
                    handle_paste(app, text)?;
                }
                _ => {}
            }
        }
        if app.should_quit {
            break;
        }
    }
    Ok(())
}

fn handle_paste(app: &mut App, raw: String) -> Result<()> {
    let cleaned: String = raw.chars().filter(|c| *c != '\r' && *c != '\n').collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        return Ok(());
    }

    if is_torrent_uri(trimmed) {
        let fixed = sanitize_torrent_uri(trimmed);
        app.active_tab = Tab::Torrents;
        app.search_editing = false;
        app.playlist_editing = false;
        app.peer_connect_editing = false;
        app.torrent_input_mode = TorrentInputMode::None;
        app.add_torrent_uri(fixed);
        return Ok(());
    }

    if trimmed.to_ascii_lowercase().starts_with("rap2p://") {
        app.active_tab = Tab::Friends;
        app.search_editing = false;
        app.playlist_editing = false;
        app.torrent_input_mode = TorrentInputMode::None;
        app.peer_connect_input = trimmed.to_string();
        app.connect_peer_input();
        return Ok(());
    }

    if let Some(picker) = &mut app.playlist_picker {
        picker.creating_new = true;
        picker.new_name.push_str(trimmed);
        return Ok(());
    }

    match app.active_tab {
        Tab::Search => {
            app.search_editing = true;
            app.search_query.push_str(trimmed);
            app.refresh_from_db()?;
        }
        Tab::Playlists => {
            app.playlist_editing = true;
            app.playlist_input.push_str(trimmed);
        }
        Tab::Torrents => match app.torrent_input_mode {
            TorrentInputMode::Magnet => app.torrent_magnet_input.push_str(trimmed),
            TorrentInputMode::JackettUrl => app.jackett_url_input.push_str(trimmed),
            _ => {
                app.torrent_input_mode = TorrentInputMode::Search;
                app.torrent_query.push_str(trimmed);
            }
        },
        Tab::Friends => {
            app.peer_connect_editing = true;
            app.peer_connect_input.push_str(trimmed);
        }
        _ => {}
    }
    Ok(())
}

fn point_in(rect: Rect, col: u16, row: u16) -> bool {
    rect.width > 0
        && rect.height > 0
        && col >= rect.x
        && col < rect.x + rect.width
        && row >= rect.y
        && row < rect.y + rect.height
}

fn handle_mouse(
    app: &mut App,
    mouse: MouseEvent,
    last_click: &mut Option<(Instant, u16, u16)>,
) -> Result<()> {
    let col = mouse.column;
    let row = mouse.row;
    let layout = app.layout.borrow().clone();

    match mouse.kind {
        MouseEventKind::ScrollUp => {
            if app.playlist_picker.is_some() {
                if let Some(p) = &mut app.playlist_picker {
                    p.selected = p.selected.saturating_sub(1);
                    p.creating_new = p.selected >= app.playlists.len();
                }
            } else {
                move_selection(app, -1)?;
            }
            return Ok(());
        }
        MouseEventKind::ScrollDown => {
            if app.playlist_picker.is_some() {
                if let Some(p) = &mut app.playlist_picker {
                    let max_idx = app.playlists.len();
                    p.selected = (p.selected + 1).min(max_idx);
                    p.creating_new = p.selected >= app.playlists.len();
                }
            } else {
                move_selection(app, 1)?;
            }
            return Ok(());
        }
        MouseEventKind::Down(MouseButton::Left) => {}
        _ => return Ok(()),
    }

    let is_double_click = if let Some((prev_t, pc, pr)) = *last_click {
        prev_t.elapsed() <= Duration::from_millis(450) && pc == col && pr == row
    } else {
        false
    };
    *last_click = Some((Instant::now(), col, row));

    // 1. Handle modal clicks first
    if app.playlist_picker.is_some() {
        if !point_in(layout.modal_box, col, row) {
            app.playlist_picker = None;
            return Ok(());
        }
        if point_in(layout.modal_list, col, row) {
            let rel = (row - layout.modal_list.y) as usize;
            let max_idx = app.playlists.len();
            if rel <= max_idx {
                if let Some(p) = &mut app.playlist_picker {
                    p.selected = rel;
                    p.creating_new = rel == max_idx;
                }
                if rel < max_idx {
                    app.confirm_playlist_picker()?;
                }
            }
        }
        return Ok(());
    }

    if app.show_qr_modal {
        app.show_qr_modal = false;
        return Ok(());
    }

    // 2. Tab bar clicks
    for (i, tab) in Tab::all().iter().enumerate() {
        if point_in(layout.tab_rects[i], col, row) {
            app.switch_tab(*tab);
            return Ok(());
        }
    }

    // 3. Player bar clicks
    if point_in(layout.add_playlist_btn, col, row) {
        app.open_playlist_picker_for_selected();
        return Ok(());
    }
    if point_in(layout.auto_next_pill, col, row) {
        app.toggle_auto_next();
        return Ok(());
    }
    if point_in(layout.player_gauge, col, row) {
        let st = app.player.status();
        if let Some(dur) = st.duration_secs {
            if dur > 0.0 && layout.player_gauge.width > 0 {
                let ratio = (col - layout.player_gauge.x) as f64 / layout.player_gauge.width as f64;
                app.player.seek_to((ratio * dur).clamp(0.0, dur))?;
            }
        }
        return Ok(());
    }
    if point_in(layout.player_bar, col, row) && row == layout.player_bar.y {
        app.player.toggle_pause()?;
        return Ok(());
    }

    // 4. Active tab interactive areas
    match app.active_tab {
        Tab::Library => {
            if point_in(layout.library_table, col, row) && row >= layout.library_table.y + 2 {
                let idx = layout.library_scroll + (row - (layout.library_table.y + 2)) as usize;
                if idx < app.tracks.len() {
                    let already = app.library_selected == idx;
                    app.library_selected = idx;
                    if is_double_click || already {
                        app.play_selected_from_active_list()?;
                    }
                }
            }
        }
        Tab::Search => {
            if point_in(layout.search_input, col, row) {
                app.search_editing = true;
            } else if point_in(layout.search_table, col, row) && row >= layout.search_table.y + 2 {
                let idx = layout.search_scroll + (row - (layout.search_table.y + 2)) as usize;
                if idx < app.search_results.len() {
                    let already = app.search_selected == idx;
                    app.search_selected = idx;
                    if is_double_click || already {
                        app.play_selected_from_active_list()?;
                    }
                }
            }
        }
        Tab::Playlists => {
            if point_in(layout.playlist_input, col, row) {
                app.playlist_editing = true;
            } else if point_in(layout.playlists_list, col, row) && row >= layout.playlists_list.y + 1
            {
                app.playlist_editing = false;
                app.playlist_focus = PlaylistPanelFocus::Playlists;
                let idx = layout.playlists_scroll + (row - (layout.playlists_list.y + 1)) as usize;
                if idx < app.playlists.len() {
                    app.playlist_selected = idx;
                    app.refresh_from_db()?;
                }
            } else if point_in(layout.playlist_tracks_table, col, row)
                && row >= layout.playlist_tracks_table.y + 2
            {
                app.playlist_editing = false;
                app.playlist_focus = PlaylistPanelFocus::PlaylistTracks;
                let idx = layout.playlist_tracks_scroll
                    + (row - (layout.playlist_tracks_table.y + 2)) as usize;
                if idx < app.selected_playlist_tracks.len() {
                    let already = app.playlist_track_selected == idx;
                    app.playlist_track_selected = idx;
                    if is_double_click || already {
                        app.play_selected_from_active_list()?;
                    }
                }
            }
        }
        Tab::Torrents => {
            if point_in(layout.torrent_search_input, col, row) {
                app.torrent_input_mode = TorrentInputMode::Search;
            } else if point_in(layout.torrent_magnet_input, col, row) {
                app.torrent_input_mode = TorrentInputMode::Magnet;
            } else if point_in(layout.torrent_results_table, col, row)
                && row >= layout.torrent_results_table.y + 2
            {
                app.torrent_input_mode = TorrentInputMode::None;
                let idx = layout.torrent_results_scroll
                    + (row - (layout.torrent_results_table.y + 2)) as usize;
                if idx < app.torrent_results.len() {
                    app.torrent_result_selected = idx;
                    if is_double_click {
                        let uri = app.torrent_results[idx].magnet_uri.clone();
                        app.torrent_magnet_input = uri.clone();
                        app.add_torrent_uri(uri);
                    }
                }
            } else if point_in(layout.torrent_downloads_area, col, row)
                && row >= layout.torrent_downloads_area.y + 1
            {
                app.torrent_input_mode = TorrentInputMode::None;
                let rel = (row - (layout.torrent_downloads_area.y + 1)) as usize;
                let pending_count = app.pending_magnets.len();
                if rel >= pending_count {
                    let dl_idx = rel - pending_count;
                    if dl_idx < app.active_torrents.len() {
                        let already = app.active_torrent_selected == dl_idx;
                        app.active_torrent_selected = dl_idx;
                        if is_double_click || already {
                            app.play_selected_active_torrent()?;
                        }
                    }
                }
            }
        }
        Tab::CacheSettings => {}
        Tab::Friends => {
            if point_in(layout.peer_discover_btn, col, row) {
                app.peer_connect_editing = false;
                app.trigger_lan_discovery();
            } else if point_in(layout.peer_lan_share_btn, col, row) {
                app.peer_connect_editing = false;
                app.toggle_lan_share();
            } else if point_in(layout.peer_qr_btn, col, row) {
                app.peer_connect_editing = false;
                app.toggle_qr_modal();
            } else if point_in(layout.peer_connect_input, col, row) {
                app.peer_connect_editing = true;
            } else if point_in(layout.peer_list, col, row) && row >= layout.peer_list.y + 1 {
                app.peer_connect_editing = false;
                let idx = layout.peer_list_scroll + (row - (layout.peer_list.y + 1)) as usize;
                if idx < app.paired_peers.len() {
                    app.peer_selected = idx;
                    if is_double_click {
                        app.resync_selected_peer();
                    }
                }
            } else if point_in(layout.peer_tracks_table, col, row)
                && row >= layout.peer_tracks_table.y + 2
            {
                app.peer_connect_editing = false;
                let idx =
                    layout.peer_tracks_scroll + (row - (layout.peer_tracks_table.y + 2)) as usize;
                if idx < app.peer_tracks.len() {
                    let already = app.peer_track_selected == idx;
                    app.peer_track_selected = idx;
                    if is_double_click || already {
                        app.play_selected_from_active_list()?;
                    }
                }
            }
        }
    }

    Ok(())
}

fn handle_tab_hotkeys(app: &mut App, key: &KeyEvent) -> bool {
    match key.code {
        KeyCode::F(1) => {
            app.switch_tab(Tab::Library);
            true
        }
        KeyCode::F(2) => {
            app.switch_tab(Tab::Search);
            true
        }
        KeyCode::F(3) => {
            app.switch_tab(Tab::Playlists);
            true
        }
        KeyCode::F(4) => {
            app.switch_tab(Tab::Torrents);
            true
        }
        KeyCode::F(5) => {
            app.switch_tab(Tab::CacheSettings);
            true
        }
        KeyCode::F(6) => {
            app.switch_tab(Tab::Friends);
            true
        }
        KeyCode::Char(c @ '1'..='6') if key.modifiers.contains(KeyModifiers::ALT) => {
            let idx = (c as u8 - b'1') as usize;
            app.switch_tab(Tab::all()[idx]);
            true
        }
        _ => false,
    }
}

fn handle_key(app: &mut App, key: KeyEvent) -> Result<()> {
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        app.should_quit = true;
        return Ok(());
    }

    // 1. Playlist Picker Modal takes precedence
    if app.playlist_picker.is_some() {
        let playlists_len = app.playlists.len();
        match key.code {
            KeyCode::Esc => {
                app.playlist_picker = None;
            }
            KeyCode::Up => {
                if let Some(p) = &mut app.playlist_picker {
                    p.selected = p.selected.saturating_sub(1);
                    p.creating_new = p.selected >= playlists_len;
                }
            }
            KeyCode::Down => {
                if let Some(p) = &mut app.playlist_picker {
                    p.selected = (p.selected + 1).min(playlists_len);
                    p.creating_new = p.selected >= playlists_len;
                }
            }
            KeyCode::Backspace => {
                if let Some(p) = &mut app.playlist_picker {
                    if p.creating_new {
                        p.new_name.pop();
                    }
                }
            }
            KeyCode::Char(c) => {
                if let Some(p) = &mut app.playlist_picker {
                    if !p.creating_new {
                        p.selected = playlists_len;
                        p.creating_new = true;
                    }
                    p.new_name.push(c);
                }
            }
            KeyCode::Enter => {
                app.confirm_playlist_picker()?;
            }
            _ => {}
        }
        return Ok(());
    }

    // Always allow F1..F6 or Alt+1..6 even while typing in inputs
    if handle_tab_hotkeys(app, &key) {
        return Ok(());
    }

    if app.show_qr_modal {
        match key.code {
            KeyCode::Esc | KeyCode::Char('v') | KeyCode::Enter | KeyCode::Char('q') => {
                app.show_qr_modal = false;
            }
            _ => {}
        }
        return Ok(());
    }

    // 2. Search input editing (allows live typing + Up/Down navigation + Enter to play)
    if app.search_editing && app.active_tab == Tab::Search {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('a') {
            app.open_playlist_picker_for_selected();
            return Ok(());
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('x') {
            app.toggle_mark_selected_track();
            return Ok(());
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('p') {
            app.toggle_pin_selected()?;
            return Ok(());
        }
        match key.code {
            KeyCode::Esc => app.search_editing = false,
            KeyCode::Up => move_selection(app, -1)?,
            KeyCode::Down => move_selection(app, 1)?,
            KeyCode::Enter => {
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

    // 3. New Playlist / Rename Playlist input editing
    if (app.playlist_editing || app.playlist_renaming) && app.active_tab == Tab::Playlists {
        match key.code {
            KeyCode::Esc => {
                app.playlist_editing = false;
                app.playlist_renaming = false;
                app.playlist_input.clear();
            }
            KeyCode::Enter => {
                if app.playlist_renaming {
                    app.confirm_rename_playlist()?;
                } else {
                    app.create_playlist_from_input()?;
                }
            }
            KeyCode::Backspace => {
                app.playlist_input.pop();
            }
            KeyCode::Char(c) => app.playlist_input.push(c),
            _ => {}
        }
        return Ok(());
    }

    // 4. Torrent search / magnet / Jackett URL input editing
    if app.torrent_input_mode != TorrentInputMode::None && app.active_tab == Tab::Torrents {
        match key.code {
            KeyCode::Esc => app.torrent_input_mode = TorrentInputMode::None,
            KeyCode::Up => move_selection(app, -1)?,
            KeyCode::Down => move_selection(app, 1)?,
            KeyCode::Enter => {
                match app.torrent_input_mode {
                    TorrentInputMode::Search => app.trigger_torrent_search(),
                    TorrentInputMode::Magnet => {
                        let uri = app.torrent_magnet_input.trim().to_string();
                        if !uri.is_empty() {
                            app.add_torrent_uri(uri);
                        }
                    }
                    TorrentInputMode::JackettUrl => {
                        app.save_jackett_url_override();
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
                TorrentInputMode::JackettUrl => {
                    app.jackett_url_input.pop();
                }
                TorrentInputMode::None => {}
            },
            KeyCode::Char(c) => match app.torrent_input_mode {
                TorrentInputMode::Search => app.torrent_query.push(c),
                TorrentInputMode::Magnet => app.torrent_magnet_input.push(c),
                TorrentInputMode::JackettUrl => app.jackett_url_input.push(c),
                TorrentInputMode::None => {}
            },
            _ => {}
        }
        return Ok(());
    }

    // 4b. P2P Connect input editing
    if app.peer_connect_editing && app.active_tab == Tab::Friends {
        match key.code {
            KeyCode::Esc => app.peer_connect_editing = false,
            KeyCode::Enter => app.connect_peer_input(),
            KeyCode::Backspace => {
                app.peer_connect_input.pop();
            }
            KeyCode::Char(c) => app.peer_connect_input.push(c),
            _ => {}
        }
        return Ok(());
    }

    // 5. Global navigation and playback shortcuts
    match key.code {
        KeyCode::Char('q') => app.should_quit = true,
        KeyCode::Char('1') => app.switch_tab(Tab::Library),
        KeyCode::Char('2') => app.switch_tab(Tab::Search),
        KeyCode::Char('3') => app.switch_tab(Tab::Playlists),
        KeyCode::Char('4') => app.switch_tab(Tab::Torrents),
        KeyCode::Char('5') => app.switch_tab(Tab::CacheSettings),
        KeyCode::Char('6') => app.switch_tab(Tab::Friends),
        KeyCode::Char('/') | KeyCode::Char('i') => match app.active_tab {
            Tab::Playlists => app.playlist_editing = true,
            Tab::Torrents => app.torrent_input_mode = TorrentInputMode::Search,
            Tab::Friends => app.peer_connect_editing = true,
            _ => app.switch_tab(Tab::Search),
        },
        KeyCode::Tab if app.active_tab == Tab::Playlists => {
            app.playlist_focus = match app.playlist_focus {
                PlaylistPanelFocus::Playlists => PlaylistPanelFocus::PlaylistTracks,
                PlaylistPanelFocus::PlaylistTracks => PlaylistPanelFocus::Playlists,
            };
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
        KeyCode::Char('A') => app.toggle_auto_next(),
        KeyCode::Char('n') => {
            if app.active_tab == Tab::Playlists {
                app.playlist_editing = true;
            } else {
                app.next_track()?;
            }
        }
        KeyCode::Char('b') => app.prev_track()?,
        KeyCode::Char('p') => app.toggle_pin_selected()?,
        KeyCode::Char('x') => app.toggle_mark_selected_track(),
        KeyCode::Char('X') => app.clear_marked_tracks(),
        KeyCode::Char('a') => app.open_playlist_picker_for_selected(),
        KeyCode::Char('e') | KeyCode::F(2) if app.active_tab == Tab::Playlists => {
            app.start_renaming_selected_playlist();
        }
        KeyCode::Char('d') | KeyCode::Delete if app.active_tab == Tab::Playlists => {
            if app.playlist_focus == PlaylistPanelFocus::Playlists {
                app.delete_selected_playlist()?;
            } else {
                app.remove_selected_from_playlist()?;
            }
        }
        KeyCode::Char('d') if app.active_tab == Tab::Friends => app.trigger_lan_discovery(),
        KeyCode::Char('r') => app.trigger_rescan(),
        KeyCode::Char('l') => app.cycle_language(),
        KeyCode::Char('c') if app.active_tab == Tab::CacheSettings => app.purge_cache()?,
        KeyCode::Char('c') if app.active_tab == Tab::Friends => app.peer_connect_editing = true,
        KeyCode::Char('[') if app.active_tab == Tab::CacheSettings => app.adjust_cache_limit_mb(-512)?,
        KeyCode::Char(']') if app.active_tab == Tab::CacheSettings => app.adjust_cache_limit_mb(512)?,
        KeyCode::Char('s') if app.active_tab == Tab::Torrents => {
            app.torrent_input_mode = TorrentInputMode::Search;
        }
        KeyCode::Char('s') if app.active_tab == Tab::Friends => app.toggle_lan_share(),
        KeyCode::Char('v') if app.active_tab == Tab::Friends => app.toggle_qr_modal(),
        KeyCode::Char('m') if app.active_tab == Tab::Torrents => {
            app.torrent_input_mode = TorrentInputMode::Magnet;
        }
        KeyCode::Char('u') if app.active_tab == Tab::Torrents => {
            app.torrent_input_mode = TorrentInputMode::JackettUrl;
        }
        KeyCode::Char('u') if app.active_tab == Tab::Friends => app.resync_selected_peer(),
        KeyCode::Up | KeyCode::Char('k') => move_selection(app, -1)?,
        KeyCode::Down | KeyCode::Char('j') => move_selection(app, 1)?,
        KeyCode::Enter => match app.active_tab {
            Tab::Library | Tab::Search | Tab::Friends => app.play_selected_from_active_list()?,
            Tab::Torrents => {
                if let Some(res) = app.torrent_results.get(app.torrent_result_selected) {
                    app.torrent_magnet_input = res.magnet_uri.clone();
                    app.add_torrent_uri(res.magnet_uri.clone());
                } else if !app.torrent_query.trim().is_empty() {
                    app.trigger_torrent_search();
                } else {
                    app.torrent_input_mode = TorrentInputMode::Search;
                }
            }
            Tab::Playlists => {
                if app.playlist_focus == PlaylistPanelFocus::PlaylistTracks {
                    app.play_selected_from_active_list()?;
                } else if !app.selected_playlist_tracks.is_empty() {
                    app.queue.set_queue(app.selected_playlist_tracks.clone(), 0);
                    app.play_current_in_queue()?;
                }
            }
            Tab::CacheSettings => {}
        },
        _ => {}
    }

    Ok(())
}

fn move_selection(app: &mut App, delta: isize) -> Result<()> {
    match app.active_tab {
        Tab::Library => {
            let len = app.tracks.len();
            if len > 0 {
                app.library_selected = (app.library_selected as isize + delta).clamp(0, len as isize - 1) as usize;
            }
        }
        Tab::Search => {
            let len = app.search_results.len();
            if len > 0 {
                app.search_selected = (app.search_selected as isize + delta).clamp(0, len as isize - 1) as usize;
            }
        }
        Tab::Playlists => {
            if app.playlist_focus == PlaylistPanelFocus::Playlists {
                let len = app.playlists.len();
                if len > 0 {
                    let next = (app.playlist_selected as isize + delta).clamp(0, len as isize - 1) as usize;
                    if next != app.playlist_selected {
                        app.playlist_selected = next;
                        app.refresh_from_db()?;
                    }
                }
            } else {
                let len = app.selected_playlist_tracks.len();
                if len > 0 {
                    app.playlist_track_selected =
                        (app.playlist_track_selected as isize + delta).clamp(0, len as isize - 1) as usize;
                }
            }
        }
        Tab::Torrents => {
            let len = app.torrent_results.len();
            if len > 0 {
                app.torrent_result_selected =
                    (app.torrent_result_selected as isize + delta).clamp(0, len as isize - 1) as usize;
            }
        }
        Tab::CacheSettings => {}
        Tab::Friends => {
            let len = app.peer_tracks.len();
            if len > 0 {
                app.peer_track_selected =
                    (app.peer_track_selected as isize + delta).clamp(0, len as isize - 1) as usize;
            }
        }
    }
    Ok(())
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
        if let Some(p) = std::env::var_os("HOME")
            .map(PathBuf::from)
            .map(|h| h.join("media-stack/data/media/music"))
            .filter(|p| p.exists())
        {
            return p;
        }
        let sample_dir = temp_base.join("sample_music/TestArtist/TestAlbum (2025)");
        let _ = std::fs::create_dir_all(&sample_dir);
        let _ = std::fs::write(sample_dir.join("01 - Afrika Sample.wav"), b"RIFF....WAVE");
        let _ = std::fs::write(sample_dir.join("02 - Next Sample.wav"), b"RIFF....WAVE");
        temp_base.join("sample_music")
    });

    let mut app = App::new(
        paths,
        db,
        Some(LanguagePreference::Auto),
        scan_root.clone(),
        true,
        false,
    )?;

    println!("[1/8] i18n detected language: {} ({}) | auto_next={}", app.i18n.lang.display_name(), app.i18n.lang.code(), app.auto_next);
    assert_eq!(app.cache.limit_bytes, DEFAULT_CACHE_LIMIT_BYTES);
    assert!(app.auto_next);

    let t0 = Instant::now();
    let indexed = scan_directory_sync(&scan_root, &app.db, Some(&app.scan_tx))?;
    let first_ms = t0.elapsed().as_millis();
    let t1 = Instant::now();
    let indexed_second = scan_directory_sync(&scan_root, &app.db, Some(&app.scan_tx))?;
    let second_ms = t1.elapsed().as_millis();
    app.refresh_from_db()?;
    println!("[2/8] Scan ({}): 1st={} tracks in {}ms | 2nd incremental={} tracks in {}ms", scan_root.display(), indexed, first_ms, indexed_second, second_ms);
    assert!(!app.tracks.is_empty());
    assert_eq!(indexed, indexed_second);

    app.switch_tab(Tab::Search);
    assert!(app.search_editing, "Switching to Search tab must activate typing immediately");
    app.search_query = app.tracks[0].artist.split_whitespace().next().unwrap_or("").to_string();
    app.refresh_from_db()?;
    println!("[3/8] FTS5 live search for '{}' returned {} matches", app.search_query, app.search_results.len());
    assert!(!app.search_results.is_empty());

    app.switch_tab(Tab::Library);
    app.open_playlist_picker_for_selected();
    assert!(app.playlist_picker.is_some());
    if let Some(p) = &mut app.playlist_picker {
        p.creating_new = true;
        p.new_name = "Travel SelfTest".to_string();
    }
    app.confirm_playlist_picker()?;
    app.toggle_pin_selected()?;
    println!("[4/8] Added track via PlaylistPicker modal (playlist tracks={}, pinned={})", app.selected_playlist_tracks.len(), app.pinned_tracks.len());
    assert_eq!(app.selected_playlist_tracks.len(), 1);
    assert!(!app.pinned_tracks.is_empty());

    app.switch_tab(Tab::Torrents);
    app.torrent_searching = true;
    app.torrent_search_ticks = 45;
    let (log_tx, mut log_rx) = tokio::sync::mpsc::unbounded_channel();
    let (torrent_res, torrent_errs) = app.search_provider.search_streaming("beatles", log_tx).await;
    while let Ok(ev) = log_rx.try_recv() {
        if let rust_audio_source_torrent::SearchProgressEvent::Log(line) = ev {
            app.torrent_search_logs.push(line);
        }
    }
    app.torrent_searching = false;
    println!(
        "[5/8] TorrentSearchProvider ({}) returned {} result(s) (logs: {}, warnings: {})",
        app.search_provider.provider_names().join(", "),
        torrent_res.len(),
        app.torrent_search_logs.len(),
        torrent_errs.len()
    );
    for l in &app.torrent_search_logs {
        println!("      {l}");
    }
    assert!(!torrent_res.is_empty());
    app.torrent_results = torrent_res;

    app.switch_tab(Tab::Library);
    app.play_selected_from_active_list()?;
    std::thread::sleep(Duration::from_millis(60));
    app.player.seek_to(30.0)?;
    std::thread::sleep(Duration::from_millis(40));
    let pst = app.player.status();
    println!(
        "[6/8] Player state={:?}, pos={:.1}s, prefetch={:?} ({} bytes)",
        pst.state,
        pst.position_secs,
        pst.prefetch_track.as_ref().map(|t| t.title.as_str()),
        pst.prefetched_bytes
    );

    // [7/8] P2P node server + catalog sync + audio streaming to local cache
    let peer_audio = temp_base.join("peer_shared.wav");
    std::fs::write(&peer_audio, b"RIFF....WAVEfmt_peer_audio")?;
    let peer_db = std::sync::Arc::new(std::sync::Mutex::new(Database::open_in_memory()?));
    let peer_id = NodeIdentity::load_or_create(&peer_db.lock().unwrap())?;
    {
        let g = peer_db.lock().unwrap();
        let tid = g.upsert_track(&rust_audio_core::Track {
            id: 0,
            title: "Remote P2P Song".to_string(),
            artist: "Peer Artist".to_string(),
            album: "Shared Album".to_string(),
            year: Some(2025),
            track_number: Some(1),
            duration_secs: Some(90),
            file_size_bytes: 26,
            format: "wav".to_string(),
            cover_path: None,
            source: rust_audio_core::TrackSource::LocalFile {
                path: peer_audio.to_string_lossy().to_string(),
            },
            pinned: false,
        })?;
        let pid = g.create_playlist("Shared Mix")?;
        g.add_track_to_playlist(pid, tid)?;
    }
    let srv = FriendNodeServer::start_with_options(peer_db, peer_id.clone(), Some(0), None).await?;
    let client = FriendNodeClient::new();
    let (paired_peer, catalog) = client.resolve_and_sync_peer(&srv.ticket().to_uri()).await?;
    app.apply_synced_peer(paired_peer.clone(), catalog.clone())?;
    let cached_peer_file = client
        .download_peer_track_to_cache(
            &paired_peer,
            catalog.tracks[0].id,
            &catalog.tracks[0].format,
            &app.paths.cache_dir,
        )
        .await?;
    assert!(cached_peer_file.exists());
    assert!(!app.peer_tracks.is_empty());

    // Verify Web UI / PWA & Remote State HTTP endpoints return 200 OK
    let web_url = srv.web_url();
    let web_state = client.verify_web_ui(&web_url, &peer_id.auth_token).await?;
    assert_eq!(web_state.playlists.len(), 1);
    srv.shutdown();
    println!(
        "[7/8] P2P sync, audio stream & Web UI/PWA verified (url={}, peers={}, remote_tracks={}, cached={})",
        web_url,
        app.paired_peers.len(),
        app.peer_tracks.len(),
        cached_peer_file.display()
    );

    // Render splash screen, all 6 tabs, playlist picker modal, and QR modal in Spanish and English
    let mut terminal = Terminal::new(TestBackend::new(120, 35))?;
    terminal.draw(|f| ui::draw_loading_screen(f, &app.i18n))?;
    for pref in [LanguagePreference::Es, LanguagePreference::En] {
        app.i18n.set_preference(pref);
        for tab in Tab::all() {
            app.switch_tab(tab);
            terminal.draw(|f| ui::draw(f, &app))?;
        }
        app.switch_tab(Tab::Friends);
        app.toggle_qr_modal();
        terminal.draw(|f| ui::draw(f, &app))?;
        app.show_qr_modal = false;

        app.switch_tab(Tab::Library);
        app.open_playlist_picker_for_selected();
        terminal.draw(|f| ui::draw(f, &app))?;
        app.playlist_picker = None;
    }
    println!("[8/8] Rendered splash screen, all 6 TUI tabs, QR modal, and PlaylistPicker modal in Es & En cleanly.");

    let _ = std::fs::remove_dir_all(&temp_base);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_audio_core::{Track, TrackSource};

    fn make_test_app() -> (App, PathBuf) {
        let temp_base = std::env::temp_dir().join(format!(
            "rust_audio_tui_unit_{}_{}",
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
        for (i, title) in ["Alpha", "Beta"].iter().enumerate() {
            db.upsert_track(&Track {
                id: 0,
                title: title.to_string(),
                artist: "TestArtist".to_string(),
                album: "TestAlbum".to_string(),
                year: Some(2025),
                track_number: Some(i as u32 + 1),
                duration_secs: Some(120),
                file_size_bytes: 1000,
                format: "flac".to_string(),
                cover_path: None,
                source: TrackSource::LocalFile {
                    path: format!("/tmp/fake_{i}.flac"),
                },
                pinned: false,
            })
            .unwrap();
        }
        let app = App::new(
            paths,
            db,
            Some(LanguagePreference::Es),
            temp_base.clone(),
            true,
            false,
        )
        .unwrap();
        (app, temp_base)
    }

    #[test]
    fn test_search_tab_immediate_typing_and_mouse_clicks() {
        let (mut app, temp_base) = make_test_app();
        let mut terminal = Terminal::new(TestBackend::new(120, 35)).unwrap();
        terminal.draw(|f| ui::draw(f, &app)).unwrap();

        // Click tab 2 (Search) via layout coordinates
        let search_tab_rect = app.layout.borrow().tab_rects[1];
        let mut last_click = None;
        handle_mouse(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: search_tab_rect.x + 2,
                row: search_tab_rect.y + 1,
                modifiers: KeyModifiers::NONE,
            },
            &mut last_click,
        )
        .unwrap();
        assert_eq!(app.active_tab, Tab::Search);
        assert!(app.search_editing, "Mouse click on Search tab must enable typing");

        // Type 'B' 'e' 't' directly without pressing '/'
        for ch in ['B', 'e', 't'] {
            handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE),
            )
            .unwrap();
        }
        assert_eq!(app.search_query, "Bet");
        assert_eq!(app.search_results.len(), 1);
        assert_eq!(app.search_results[0].title, "Beta");

        // Click [+ Playlist (a)] button to open modal
        terminal.draw(|f| ui::draw(f, &app)).unwrap();
        let add_btn = app.layout.borrow().add_playlist_btn;
        handle_mouse(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: add_btn.x + 2,
                row: add_btn.y,
                modifiers: KeyModifiers::NONE,
            },
            &mut last_click,
        )
        .unwrap();
        assert!(app.playlist_picker.is_some());
        app.playlist_picker = None;

        // Click Auto-Sig pill to toggle auto_next
        let auto_pill = app.layout.borrow().auto_next_pill;
        assert!(app.auto_next);
        handle_mouse(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: auto_pill.x + 1,
                row: auto_pill.y,
                modifiers: KeyModifiers::NONE,
            },
            &mut last_click,
        )
        .unwrap();
        assert!(!app.auto_next);

        let _ = std::fs::remove_dir_all(&temp_base);
    }

    #[test]
    fn test_paste_magnet_and_broken_agnet_uri() {
        let (mut app, temp_base) = make_test_app();
        // Pasting a magnet link (even missing leading 'm') from anywhere switches to Torrents and queues it immediately.
        let broken_magnet = "agnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567&dn=Spinetta+Test\n";
        handle_paste(&mut app, broken_magnet.to_string()).unwrap();

        assert_eq!(app.active_tab, Tab::Torrents);
        assert!(app.torrent_magnet_input.starts_with("magnet:?xt="));
        assert_eq!(app.pending_magnets.len(), 1);
        assert_eq!(app.pending_magnets[0].1, "Spinetta Test");
        assert_eq!(app.pending_queued_uris.len(), 1);
        assert!(app.torrent_search_logs.iter().any(|l| l.contains("Spinetta Test")));

        let mut terminal = Terminal::new(TestBackend::new(120, 35)).unwrap();
        terminal.draw(|f| ui::draw(f, &app)).unwrap();

        let _ = std::fs::remove_dir_all(&temp_base);
    }

    #[tokio::test]
    async fn test_paste_rap2p_ticket_and_friends_tab() {
        let (mut app, temp_base) = make_test_app();
        handle_paste(&mut app, "rap2p://127.0.0.1:4261#deadbeef?name=air".to_string()).unwrap();
        assert_eq!(app.active_tab, Tab::Friends);
        assert!(app.peer_connect_input.starts_with("rap2p://127.0.0.1:4261"));
        assert!(app.p2p_logs.iter().any(|l| l.contains("Conectando a nodo")));

        let mut terminal = Terminal::new(TestBackend::new(120, 35)).unwrap();
        terminal.draw(|f| ui::draw(f, &app)).unwrap();
        app.toggle_qr_modal();
        assert!(app.show_qr_modal);
        terminal.draw(|f| ui::draw(f, &app)).unwrap();

        let _ = std::fs::remove_dir_all(&temp_base);
    }

    #[test]
    fn test_tui_multi_mark_and_playlist_rename_delete() {
        let (mut app, temp_base) = make_test_app();
        // Mark both Alpha and Beta with 'x'
        handle_key(&mut app, KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE)).unwrap();
        handle_key(&mut app, KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE)).unwrap();
        assert_eq!(app.marked_track_ids.len(), 2);

        // Press 'a' -> batch playlist picker -> create "Ruta"
        handle_key(&mut app, KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE)).unwrap();
        assert!(app.playlist_picker.is_some());
        if let Some(p) = &mut app.playlist_picker {
            p.creating_new = true;
            p.new_name = "Ruta".to_string();
        }
        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)).unwrap();
        assert!(app.marked_track_ids.is_empty());
        assert_eq!(app.playlists.len(), 1);
        assert_eq!(app.playlists[0].track_count, 2);

        // Switch to Playlists tab, rename to "Viaje Largo", then delete
        app.switch_tab(Tab::Playlists);
        app.playlist_focus = PlaylistPanelFocus::Playlists;
        handle_key(&mut app, KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE)).unwrap();
        assert!(app.playlist_renaming);
        app.playlist_input = "Viaje Largo".to_string();
        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)).unwrap();
        assert!(!app.playlist_renaming);
        assert_eq!(app.playlists[0].name, "Viaje Largo");

        handle_key(&mut app, KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE)).unwrap();
        assert!(app.playlists.is_empty());

        let _ = std::fs::remove_dir_all(&temp_base);
    }
}
