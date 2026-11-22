use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Gauge, Paragraph, Row, Table, Tabs};
use ratatui::Frame;
use rust_audio_core::ScanStatus;
use rust_audio_player::{AudioBackendKind, PlaybackState};

use crate::app::{App, Tab, TorrentInputMode};

fn fmt_time(secs: f64) -> String {
    let total = secs.max(0.0) as u64;
    format!("{:02}:{:02}", total / 60, total % 60)
}

fn fmt_mb(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
}

pub fn draw(frame: &mut Frame, app: &App) {
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Top header + scan + backend + lang
            Constraint::Length(3), // Tabs
            Constraint::Min(8),    // Main content
            Constraint::Length(5), // Player bar
            Constraint::Length(2), // Help footer
        ])
        .split(frame.area());

    draw_header(frame, app, root[0]);
    draw_tabs(frame, app, root[1]);

    match app.active_tab {
        Tab::Library => draw_track_table(frame, app, &app.tracks, app.library_selected, app.i18n.tab_library(), root[2]),
        Tab::Search => draw_search(frame, app, root[2]),
        Tab::Playlists => draw_playlists(frame, app, root[2]),
        Tab::Torrents => draw_torrents(frame, app, root[2]),
        Tab::CacheSettings => draw_cache_settings(frame, app, root[2]),
    }

    draw_player_bar(frame, app, root[3]);
    draw_footer(frame, app, root[4]);
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

fn draw_tabs(frame: &mut Frame, app: &App, area: Rect) {
    let titles: Vec<Line> = vec![
        Line::from(app.i18n.tab_library()),
        Line::from(app.i18n.tab_search()),
        Line::from(app.i18n.tab_playlists()),
        Line::from(app.i18n.tab_torrents()),
        Line::from(app.i18n.tab_cache_settings()),
    ];
    let idx = Tab::all()
        .iter()
        .position(|t| *t == app.active_tab)
        .unwrap_or(0);
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
) {
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
            let style = if i == selected {
                Style::default()
                    .bg(Color::DarkGray)
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
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
    .block(Block::default().borders(Borders::ALL).title(format!(" {title} ({}) ", tracks.len())));
    frame.render_widget(table, area);
}

fn draw_search(frame: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(5)])
        .split(area);

    let prompt_style = if app.search_editing {
        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    let input = Paragraph::new(format!("{}{}", app.i18n.search_prompt(), app.search_query))
        .style(prompt_style)
        .block(Block::default().borders(Borders::ALL).title(app.i18n.search_hint()));
    frame.render_widget(input, chunks[0]);

    draw_track_table(
        frame,
        app,
        &app.search_results,
        app.search_selected,
        app.i18n.tab_search(),
        chunks[1],
    );
}

fn draw_playlists(frame: &mut Frame, app: &App, area: Rect) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(area);

    let left_rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(4)])
        .split(cols[0]);

    let new_pl = Paragraph::new(format!(
        "{}: {}",
        if app.playlist_editing { "Nueva / New [*]" } else { "[n] Nueva / New" },
        app.playlist_input
    ))
    .block(Block::default().borders(Borders::ALL).title(app.i18n.playlists_header()));
    frame.render_widget(new_pl, left_rows[0]);

    let pl_lines: Vec<Line> = app
        .playlists
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let pin = if p.pinned { "★ " } else { "  " };
            let st = if i == app.playlist_selected {
                Style::default().bg(Color::DarkGray).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            Line::styled(format!("{pin}{} ({})", p.name, p.track_count), st)
        })
        .collect();
    let pl_widget = Paragraph::new(pl_lines).block(
        Block::default()
            .borders(Borders::ALL)
            .title(format!(" {} ({}) ", app.i18n.playlists_header(), app.playlists.len())),
    );
    frame.render_widget(pl_widget, left_rows[1]);

    draw_track_table(
        frame,
        app,
        &app.pinned_tracks,
        0,
        app.i18n.pinned_offline_header(),
        cols[1],
    );
}

fn draw_torrents(frame: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Percentage(50),
            Constraint::Percentage(45),
        ])
        .split(area);

    let search_line = Line::from(vec![
        Span::styled("[s] ", Style::default().fg(Color::Yellow)),
        Span::raw(app.i18n.torrents_search_prompt()),
        Span::styled(
            &app.torrent_query,
            if app.torrent_input_mode == TorrentInputMode::Search {
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            },
        ),
    ]);
    let magnet_line = Line::from(vec![
        Span::styled("[m] ", Style::default().fg(Color::Yellow)),
        Span::raw(app.i18n.torrents_magnet_prompt()),
        Span::styled(
            &app.torrent_magnet_input,
            if app.torrent_input_mode == TorrentInputMode::Magnet {
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            },
        ),
    ]);
    let input_box = Paragraph::new(vec![search_line, magnet_line])
        .block(Block::default().borders(Borders::ALL).title(app.search_provider.provider_names().join(" + ")));
    frame.render_widget(input_box, chunks[0]);

    let res_rows = app.torrent_results.iter().enumerate().map(|(i, r)| {
        let st = if i == app.torrent_result_selected {
            Style::default().bg(Color::DarkGray).add_modifier(Modifier::BOLD)
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
    .block(Block::default().borders(Borders::ALL).title(" Results (Enter = Add) "));
    frame.render_widget(res_table, chunks[1]);

    let active_lines: Vec<Line> = app
        .active_torrents
        .iter()
        .map(|t| {
            let pct = if t.total_bytes > 0 {
                (t.progress_bytes as f64 / t.total_bytes as f64) * 100.0
            } else {
                0.0
            };
            Line::from(format!(
                "#{} {} | {} | {:.1}% | {:.2} MB/s | peers: {}",
                t.id, t.name, t.state, pct, t.download_speed_mbps, t.peers_live
            ))
        })
        .collect();
    let active_box = Paragraph::new(active_lines)
        .block(Block::default().borders(Borders::ALL).title(app.i18n.torrents_active_header()));
    frame.render_widget(active_box, chunks[2]);
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
        Line::from("Keys: [c] Purge unpinned LRU cache | [ / ] Adjust limit (-/+ 512 MB) | [l] Switch language"),
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

    frame.render_widget(
        Block::default().borders(Borders::ALL).title(format!(" {state_label} | Vol: {:.0}% ", st.volume * 100.0)),
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
}

fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
    let footer = Paragraph::new(app.i18n.help_footer())
        .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(footer, area);
}
