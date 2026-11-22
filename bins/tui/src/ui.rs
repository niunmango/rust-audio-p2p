use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Clear, Gauge, Paragraph, Row, Table, Tabs, Wrap};
use ratatui::Frame;
use rust_audio_core::{I18n, ScanStatus};
use rust_audio_player::{AudioBackendKind, PlaybackState};
use rust_audio_source_torrent::redact_url_secrets;

use crate::app::{App, PlaylistPanelFocus, Tab, TorrentInputMode};

fn fmt_time(secs: f64) -> String {
    let total = secs.max(0.0) as u64;
    format!("{:02}:{:02}", total / 60, total % 60)
}

fn fmt_mb(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
}

fn ascii_progress_bar(pct: f64, width: usize) -> String {
    let clamped = pct.clamp(0.0, 100.0);
    let filled = ((clamped / 100.0) * width as f64).round() as usize;
    let empty = width.saturating_sub(filled);
    format!("[{}{}] {:5.1}%", "█".repeat(filled), "░".repeat(empty), clamped)
}

pub fn draw_loading_screen(frame: &mut Frame, i18n: &I18n) {
    let area = frame.area();
    let v = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(35),
            Constraint::Length(7),
            Constraint::Percentage(35),
        ])
        .split(area);
    let h = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(15),
            Constraint::Percentage(70),
            Constraint::Percentage(15),
        ])
        .split(v[1]);

    let text = vec![
        Line::from(""),
        Line::styled(
            format!("♪  {}", i18n.loading_title()),
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ),
        Line::from(""),
        Line::styled(
            i18n.loading_subtitle(),
            Style::default().fg(Color::Gray),
        ),
    ];
    let p = Paragraph::new(text)
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: true })
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan))
                .title(" rust-audio-p2p "),
        );
    frame.render_widget(p, h[1]);
}

pub fn draw(frame: &mut Frame, app: &App) {
    let is_scanning = matches!(&*app.scan_rx.borrow(), ScanStatus::Scanning { .. });
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),                         // Top header
            Constraint::Length(3),                         // Tabs
            Constraint::Length(if is_scanning { 3 } else { 0 }), // Prominent scan banner
            Constraint::Min(8),                            // Main content
            Constraint::Length(5),                         // Player bar
            Constraint::Length(2),                         // Help footer
        ])
        .split(frame.area());

    draw_header(frame, app, root[0]);
    draw_tabs(frame, app, root[1]);
    if is_scanning {
        draw_scan_banner(frame, app, root[2]);
    }

    match app.active_tab {
        Tab::Library => {
            let mut l = app.layout.borrow_mut();
            l.library_table = root[3];
            drop(l);
            let scroll = draw_track_table(
                frame,
                app,
                &app.tracks,
                app.library_selected,
                &format!("{} ([a]: +Playlist | [p]: Pin)", app.i18n.tab_library()),
                root[3],
                true,
            );
            app.layout.borrow_mut().library_scroll = scroll;
        }
        Tab::Search => draw_search(frame, app, root[3]),
        Tab::Playlists => draw_playlists(frame, app, root[3]),
        Tab::Torrents => draw_torrents(frame, app, root[3]),
        Tab::CacheSettings => draw_cache_settings(frame, app, root[3]),
    }

    draw_player_bar(frame, app, root[4]);
    draw_footer(frame, app, root[5]);

    if app.playlist_picker.is_some() {
        draw_playlist_picker_modal(frame, app);
    }
}

fn draw_header(frame: &mut Frame, app: &App, area: Rect) {
    let scan_str = match &*app.scan_rx.borrow() {
        ScanStatus::Idle => app.i18n.scan_idle().to_string(),
        ScanStatus::Scanning {
            scanned_dirs,
            audio_files_found,
            ..
        } => app.i18n.scan_running(*scanned_dirs, *audio_files_found),
        ScanStatus::Completed { indexed, .. } => app.i18n.scan_completed(*indexed),
        ScanStatus::Error(e) => format!("Err: {e}"),
    };
    let p_status = app.player.status();
    let backend = match p_status.backend {
        AudioBackendKind::NativeAlsaPipeWire => app.i18n.audio_backend_native(),
        AudioBackendKind::HeadlessNullSink => app.i18n.audio_backend_headless(),
    };
    let line = Line::from(vec![
        Span::styled(
            format!(" {} ", app.i18n.app_title()),
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ),
        Span::raw(" | "),
        Span::styled(scan_str, Style::default().fg(Color::Green)),
        Span::raw(" | "),
        Span::styled(backend, Style::default().fg(Color::Yellow)),
        Span::raw(" | "),
        Span::styled(
            format!("[{}]", app.i18n.lang.display_name()),
            Style::default().fg(Color::Magenta),
        ),
    ]);
    let block = Paragraph::new(line).block(Block::default().borders(Borders::ALL));
    frame.render_widget(block, area);
}

fn draw_scan_banner(frame: &mut Frame, app: &App, area: Rect) {
    if let ScanStatus::Scanning {
        scanned_dirs,
        audio_files_found,
        indexed,
        ..
    } = &*app.scan_rx.borrow()
    {
        let msg = app
            .i18n
            .scan_banner_running(*scanned_dirs, *audio_files_found, *indexed);
        let p = Paragraph::new(Line::styled(
            msg,
            Style::default().fg(Color::Black).bg(Color::Yellow).add_modifier(Modifier::BOLD),
        ))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow)),
        );
        frame.render_widget(p, area);
    }
}

fn draw_tabs(frame: &mut Frame, app: &App, area: Rect) {
    let raw_titles = [
        app.i18n.tab_library(),
        app.i18n.tab_search(),
        app.i18n.tab_playlists(),
        app.i18n.tab_torrents(),
        app.i18n.tab_cache_settings(),
    ];
    let titles: Vec<Line> = raw_titles.iter().map(|t| Line::from(*t)).collect();
    let idx = Tab::all()
        .iter()
        .position(|t| *t == app.active_tab)
        .unwrap_or(0);

    // Record exact clickable bounding rectangles for each tab title
    {
        let mut layout = app.layout.borrow_mut();
        let mut cursor_x = area.x + 1;
        for (i, title) in raw_titles.iter().enumerate() {
            let w = title.chars().count() as u16 + 2;
            layout.tab_rects[i] = Rect::new(cursor_x, area.y, w, area.height);
            cursor_x = cursor_x.saturating_add(w + 2); // divider " │ "
        }
    }

    let tabs = Tabs::new(titles)
        .block(Block::default().borders(Borders::ALL))
        .select(idx)
        .highlight_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        );
    frame.render_widget(tabs, area);
}

fn draw_track_table(
    frame: &mut Frame,
    app: &App,
    tracks: &[rust_audio_core::Track],
    selected: usize,
    title: &str,
    area: Rect,
    focused: bool,
) -> usize {
    let header = Row::new(vec![
        Cell::from(app.i18n.col_pin()),
        Cell::from(app.i18n.col_title()),
        Cell::from(app.i18n.col_artist()),
        Cell::from(app.i18n.col_album()),
        Cell::from(app.i18n.col_format()),
    ])
    .style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD));

    let height = area.height.saturating_sub(4) as usize;
    let start = if height > 0 && selected >= height {
        selected - height + 1
    } else {
        0
    };

    let rows = tracks
        .iter()
        .enumerate()
        .skip(start)
        .take(height.max(1))
        .map(|(i, t)| {
            let pin_mark = if t.pinned { "★" } else { " " };
            let style = if i == selected && focused {
                Style::default()
                    .bg(Color::DarkGray)
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else if i == selected {
                Style::default().fg(Color::Cyan)
            } else {
                Style::default()
            };
            Row::new(vec![
                Cell::from(pin_mark),
                Cell::from(t.title.clone()),
                Cell::from(t.artist.clone()),
                Cell::from(t.album.clone()),
                Cell::from(t.format.to_uppercase()),
            ])
            .style(style)
        });

    let border_style = if focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default()
    };

    let table = Table::new(
        rows,
        [
            Constraint::Length(4),
            Constraint::Percentage(36),
            Constraint::Percentage(26),
            Constraint::Percentage(26),
            Constraint::Length(8),
        ],
    )
    .header(header)
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .title(format!(" {title} ({}) ", tracks.len())),
    );
    frame.render_widget(table, area);
    start
}

fn draw_search(frame: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(5)])
        .split(area);

    let cursor = if app.search_editing { "█" } else { "" };
    let border_style = if app.search_editing {
        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Gray)
    };
    let input = Paragraph::new(format!("{}{}{}", app.i18n.search_prompt(), app.search_query, cursor))
        .style(if app.search_editing {
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        })
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(border_style)
                .title(format!(" {} ", app.i18n.search_hint())),
        );
    frame.render_widget(input, chunks[0]);

    let scroll = draw_track_table(
        frame,
        app,
        &app.search_results,
        app.search_selected,
        app.i18n.tab_search(),
        chunks[1],
        true,
    );

    let mut l = app.layout.borrow_mut();
    l.search_input = chunks[0];
    l.search_table = chunks[1];
    l.search_scroll = scroll;
}

fn draw_playlists(frame: &mut Frame, app: &App, area: Rect) {
    let main_rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(6)])
        .split(area);

    let guide = Paragraph::new(app.i18n.playlists_guide())
        .style(Style::default().fg(Color::Yellow))
        .wrap(Wrap { trim: true })
        .block(Block::default().borders(Borders::ALL));
    frame.render_widget(guide, main_rows[0]);

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(36), Constraint::Percentage(64)])
        .split(main_rows[1]);

    let left_rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(4)])
        .split(cols[0]);

    let cursor = if app.playlist_editing { "█" } else { "" };
    let input_border = if app.playlist_editing {
        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    let new_pl = Paragraph::new(format!(
        "{}: {}{}",
        if app.playlist_editing { "Nombre / Name" } else { "[n / Clic] Nueva / New" },
        app.playlist_input,
        cursor
    ))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(input_border)
            .title(format!(" {} ", app.i18n.playlists_header())),
    );
    frame.render_widget(new_pl, left_rows[0]);

    let pl_focused = app.playlist_focus == PlaylistPanelFocus::Playlists;
    let list_height = left_rows[1].height.saturating_sub(2) as usize;
    let pl_scroll = if list_height > 0 && app.playlist_selected >= list_height {
        app.playlist_selected - list_height + 1
    } else {
        0
    };
    let pl_lines: Vec<Line> = app
        .playlists
        .iter()
        .enumerate()
        .skip(pl_scroll)
        .take(list_height.max(1))
        .map(|(i, p)| {
            let pin = if p.pinned { "★ " } else { "  " };
            let st = if i == app.playlist_selected && pl_focused {
                Style::default().bg(Color::DarkGray).fg(Color::White).add_modifier(Modifier::BOLD)
            } else if i == app.playlist_selected {
                Style::default().fg(Color::Cyan)
            } else {
                Style::default()
            };
            Line::styled(format!("{pin}{} ({})", p.name, p.track_count), st)
        })
        .collect();
    let pl_widget = Paragraph::new(pl_lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(if pl_focused { Style::default().fg(Color::Cyan) } else { Style::default() })
            .title(format!(" {} ({}) ", app.i18n.playlists_header(), app.playlists.len())),
    );
    frame.render_widget(pl_widget, left_rows[1]);

    let right_rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(cols[1]);

    let tracks_focused = app.playlist_focus == PlaylistPanelFocus::PlaylistTracks;
    let pl_name = app
        .playlists
        .get(app.playlist_selected)
        .map(|p| p.name.as_str())
        .unwrap_or("—");

    let pt_scroll = if app.selected_playlist_tracks.is_empty() {
        let empty = Paragraph::new(app.i18n.playlist_empty_hint())
            .style(Style::default().fg(Color::Gray))
            .wrap(Wrap { trim: true })
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(if tracks_focused { Style::default().fg(Color::Cyan) } else { Style::default() })
                    .title(format!(" {} ", app.i18n.playlist_tracks_header(pl_name))),
            );
        frame.render_widget(empty, right_rows[0]);
        0
    } else {
        draw_track_table(
            frame,
            app,
            &app.selected_playlist_tracks,
            app.playlist_track_selected,
            &app.i18n.playlist_tracks_header(pl_name),
            right_rows[0],
            tracks_focused,
        )
    };

    draw_track_table(
        frame,
        app,
        &app.pinned_tracks,
        0,
        app.i18n.pinned_offline_header(),
        right_rows[1],
        false,
    );

    let mut l = app.layout.borrow_mut();
    l.playlist_input = left_rows[0];
    l.playlists_list = left_rows[1];
    l.playlists_scroll = pl_scroll;
    l.playlist_tracks_table = right_rows[0];
    l.playlist_tracks_scroll = pt_scroll;
}

fn draw_torrents(frame: &mut Frame, app: &App, area: Rect) {
    let search_bar_h = if app.torrent_searching { 3 } else { 0 };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Search input
            Constraint::Length(3), // Magnet input
            Constraint::Length(search_bar_h), // Search progress bar
            Constraint::Percentage(50),
            Constraint::Min(5),
        ])
        .split(area);

    let s_cursor = if app.torrent_input_mode == TorrentInputMode::Search { "█" } else { "" };
    let m_cursor = if app.torrent_input_mode == TorrentInputMode::Magnet { "█" } else { "" };
    let u_cursor = if app.torrent_input_mode == TorrentInputMode::JackettUrl { "█" } else { "" };

    let jackett_hint = redact_url_secrets(
        app.search_provider
            .jackett_endpoint_hint()
            .unwrap_or("no-jackett"),
    );
    let search_box = Paragraph::new(format!("[s] {}{}{}", app.i18n.torrents_search_prompt(), app.torrent_query, s_cursor))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(if app.torrent_input_mode == TorrentInputMode::Search {
                    Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                })
                .title(format!(
                    " {} ({jackett_hint}) | [Enter]=Search | [u]=Jackett URL ",
                    app.search_provider.provider_names().join(" + ")
                )),
        );
    frame.render_widget(search_box, chunks[0]);

    let (second_text, second_focused) = if app.torrent_input_mode == TorrentInputMode::JackettUrl {
        let safe_jackett_url = redact_url_secrets(&app.jackett_url_input);
        (
            format!("[u] {}{}{}", app.i18n.torrents_jackett_prompt(), safe_jackett_url, u_cursor),
            true,
        )
    } else {
        let safe_magnet = redact_url_secrets(&app.torrent_magnet_input);
        (
            format!("[m] {}{}{}", app.i18n.torrents_magnet_prompt(), safe_magnet, m_cursor),
            app.torrent_input_mode == TorrentInputMode::Magnet,
        )
    };
    let magnet_box = Paragraph::new(second_text).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(if second_focused {
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            }),
    );
    frame.render_widget(magnet_box, chunks[1]);

    if app.torrent_searching {
        let pct = (app.torrent_search_ticks % 100).max(5);
        let gauge = Gauge::default()
            .block(Block::default().borders(Borders::ALL))
            .gauge_style(Style::default().fg(Color::Yellow))
            .percent(pct)
            .label(app.i18n.torrents_searching_progress(&app.torrent_query));
        frame.render_widget(gauge, chunks[2]);
    }

    let res_height = chunks[3].height.saturating_sub(4) as usize;
    let res_scroll = if res_height > 0 && app.torrent_result_selected >= res_height {
        app.torrent_result_selected - res_height + 1
    } else {
        0
    };
    let res_rows = app
        .torrent_results
        .iter()
        .enumerate()
        .skip(res_scroll)
        .take(res_height.max(1))
        .map(|(i, r)| {
            let st = if i == app.torrent_result_selected {
                Style::default().bg(Color::DarkGray).fg(Color::White).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            Row::new(vec![
                Cell::from(r.title.clone()),
                Cell::from(format!("{}", r.seeders)),
                Cell::from(fmt_mb(r.size_bytes)),
                Cell::from(r.provider.clone()),
            ])
            .style(st)
        });
    let res_table = Table::new(
        res_rows,
        [
            Constraint::Percentage(55),
            Constraint::Length(8),
            Constraint::Length(12),
            Constraint::Percentage(25),
        ],
    )
    .header(Row::new(vec!["Torrent", "Seeds", "Size", "Provider"]).style(Style::default().fg(Color::Yellow)))
    .block(Block::default().borders(Borders::ALL).title(format!(" Results ({}) [Enter / Double-Click = Add] ", app.torrent_results.len())));
    frame.render_widget(res_table, chunks[3]);

    let bottom_cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(chunks[4]);

    let mut active_lines: Vec<Line> = Vec::new();
    for (_uri, name, started_at) in &app.pending_magnets {
        let secs = started_at.elapsed().as_secs();
        let pulse = ((started_at.elapsed().as_millis() / 150) % 100) as f64;
        let bar = ascii_progress_bar(pulse, 12);
        active_lines.push(Line::styled(
            format!("⏳ {bar} DHT/Trackers ({secs}s): {name}"),
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        ));
    }
    for (idx, t) in app.active_torrents.iter().enumerate() {
        let pct = if t.total_bytes > 0 {
            (t.progress_bytes as f64 / t.total_bytes as f64) * 100.0
        } else {
            0.0
        };
        let bar = ascii_progress_bar(pct, 14);
        let style = if idx == app.active_torrent_selected {
            Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        active_lines.push(Line::styled(
            format!(
                "#{} {} {} | {} / {} | {:.2} MB/s | p:{} ({})",
                t.id,
                bar,
                t.name,
                fmt_mb(t.progress_bytes),
                fmt_mb(t.total_bytes),
                t.download_speed_mbps,
                t.peers_live,
                t.state
            ),
            style,
        ));
    }
    let active_box = Paragraph::new(active_lines)
        .wrap(Wrap { trim: true })
        .block(Block::default().borders(Borders::ALL).title(app.i18n.torrents_active_header()));
    frame.render_widget(active_box, bottom_cols[0]);

    let log_cap = bottom_cols[1].height.saturating_sub(2) as usize;
    let log_skip = app.torrent_search_logs.len().saturating_sub(log_cap.max(1));
    let log_lines: Vec<Line> = app
        .torrent_search_logs
        .iter()
        .skip(log_skip)
        .map(|entry| {
            let st = if entry.starts_with("[OK]") {
                Style::default().fg(Color::Green)
            } else if entry.starts_with("[ERR]") {
                Style::default().fg(Color::Red)
            } else if entry.starts_with("[TIMEOUT]") {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::Cyan)
            };
            Line::styled(entry.as_str(), st)
        })
        .collect();
    let log_box = Paragraph::new(log_lines)
        .wrap(Wrap { trim: false })
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" {} ({}) ", app.i18n.torrents_log_header(), app.torrent_search_logs.len())),
        );
    frame.render_widget(log_box, bottom_cols[1]);

    let mut l = app.layout.borrow_mut();
    l.torrent_search_input = chunks[0];
    l.torrent_magnet_input = chunks[1];
    l.torrent_results_table = chunks[3];
    l.torrent_results_scroll = res_scroll;
    l.torrent_downloads_area = bottom_cols[0];
}

fn draw_cache_settings(frame: &mut Frame, app: &App, area: Rect) {
    let stats = &app.cache_stats;
    let lines = vec![
        Line::from(format!(
            "DB: {} | Cache dir: {}",
            app.paths.db_path.display(),
            app.paths.cache_dir.display()
        )),
        Line::from(format!(
            "LRU: {} / {} | Pinned: {} ({} entries) | Evictable: {}",
            fmt_mb(stats.used_bytes),
            fmt_mb(stats.limit_bytes),
            fmt_mb(stats.pinned_bytes),
            stats.pinned_count,
            fmt_mb(stats.evictable_bytes)
        )),
        Line::from(app.i18n.language_label()),
        Line::from(format!(
            "{} | [c] Purge unpinned | [ / ] Limit (-/+ 512 MB) | [l] Language",
            app.i18n.auto_next_label(app.auto_next)
        )),
        Line::from(format!("Status: {}", app.status_message)),
    ];
    let p = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(app.i18n.cache_header()));
    frame.render_widget(p, area);
}

fn draw_player_bar(frame: &mut Frame, app: &App, area: Rect) {
    let st = app.player.status();
    let state_label = match st.state {
        PlaybackState::Playing => app.i18n.status_playing(),
        PlaybackState::Paused => app.i18n.status_paused(),
        PlaybackState::Stopped => app.i18n.status_stopped(),
    };
    let track_label = st
        .current_track
        .as_ref()
        .map(|t| format!("{} — {} ({})", t.artist, t.title, t.album))
        .unwrap_or_else(|| app.i18n.no_track_playing().to_string());

    let dur = st.duration_secs.unwrap_or(0.0);
    let ratio = if dur > 0.0 {
        (st.position_secs / dur).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let prefetch_str = app
        .i18n
        .prefetch_label(st.prefetch_track.as_ref().map(|t| t.title.as_str()));

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1), Constraint::Length(1)])
        .margin(1)
        .split(area);

    let auto_lbl = app.i18n.auto_next_label(app.auto_next);
    let add_pl_lbl = "[+ Playlist (a)]";
    let title_str = format!(
        " {state_label} | Vol: {:.0}% ",
        st.volume * 100.0
    );
    let right_pills = format!(" {add_pl_lbl} | {auto_lbl} ");

    frame.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .title(title_str)
            .title_top(Line::from(right_pills).right_aligned()),
        area,
    );
    frame.render_widget(Paragraph::new(track_label), chunks[0]);
    frame.render_widget(
        Gauge::default()
            .ratio(ratio)
            .label(format!("{} / {}", fmt_time(st.position_secs), fmt_time(dur)))
            .gauge_style(Style::default().fg(Color::Cyan)),
        chunks[1],
    );
    frame.render_widget(
        Paragraph::new(format!("{prefetch_str} | {}", app.status_message))
            .style(Style::default().fg(Color::Gray)),
        chunks[2],
    );

    let mut l = app.layout.borrow_mut();
    l.player_bar = area;
    l.player_gauge = chunks[1];
    let auto_w = auto_lbl.chars().count() as u16 + 1;
    let auto_x = area.x + area.width.saturating_sub(auto_w + 2);
    l.auto_next_pill = Rect::new(auto_x, area.y, auto_w, 1);
    let add_w = add_pl_lbl.chars().count() as u16 + 1;
    let add_x = auto_x.saturating_sub(add_w + 2);
    l.add_playlist_btn = Rect::new(add_x, area.y, add_w, 1);
}

fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
    let footer = Paragraph::new(app.i18n.help_footer())
        .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(footer, area);
}

fn draw_playlist_picker_modal(frame: &mut Frame, app: &App) {
    let Some(picker) = &app.playlist_picker else {
        return;
    };
    let area = frame.area();
    let v = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(20),
            Constraint::Percentage(60),
            Constraint::Percentage(20),
        ])
        .split(area);
    let h = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(20),
            Constraint::Percentage(60),
            Constraint::Percentage(20),
        ])
        .split(v[1]);
    let modal_area = h[1];
    frame.render_widget(Clear, modal_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
        .title(format!(" {} ", app.i18n.playlist_picker_title()));
    frame.render_widget(block, modal_area);

    let inner = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(2), Constraint::Min(4)])
        .margin(1)
        .split(modal_area);

    frame.render_widget(
        Paragraph::new(format!("♪ {} — {}", picker.track.artist, picker.track.title))
            .style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        inner[0],
    );

    let mut lines: Vec<Line> = app
        .playlists
        .iter()
        .enumerate()
        .map(|(i, pl)| {
            let st = if !picker.creating_new && i == picker.selected {
                Style::default().bg(Color::DarkGray).fg(Color::White).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            Line::styled(format!("  {} ({})", pl.name, pl.track_count), st)
        })
        .collect();

    let new_opt = if picker.creating_new {
        format!("  {} {}█", app.i18n.playlist_picker_new_option(), picker.new_name)
    } else {
        format!("  {}", app.i18n.playlist_picker_new_option())
    };
    let new_style = if picker.creating_new || picker.selected == app.playlists.len() {
        Style::default().bg(Color::Cyan).fg(Color::Black).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Green)
    };
    lines.push(Line::styled(new_opt, new_style));

    frame.render_widget(Paragraph::new(lines), inner[1]);

    let mut l = app.layout.borrow_mut();
    l.modal_box = modal_area;
    l.modal_list = inner[1];
}
