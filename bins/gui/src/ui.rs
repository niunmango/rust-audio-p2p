use std::path::Path;

use eframe::egui::{self, Color32, RichText};
use rust_audio_core::{LanguagePreference, ScanStatus, Track, TrackSource};
use rust_audio_player::{AudioBackendKind, PlaybackState};
use rust_audio_source_torrent::redact_url_secrets;

use crate::state::{GuiState, GuiTab, PlaylistPickerModal};

enum TrackRowAction {
    Play(usize),
    TogglePin(Track),
    OpenPlaylistPicker(Track),
    RemoveFromPlaylist(Track),
}

pub fn apply_catppuccin_dark(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = Color32::from_rgb(17, 17, 27);
    visuals.window_fill = Color32::from_rgb(24, 24, 37);
    visuals.extreme_bg_color = Color32::from_rgb(30, 30, 46);
    visuals.widgets.noninteractive.bg_fill = Color32::from_rgb(24, 24, 37);
    visuals.widgets.inactive.bg_fill = Color32::from_rgb(30, 30, 46);
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(49, 50, 68);
    visuals.selection.bg_fill = Color32::from_rgb(137, 220, 235);
    visuals.selection.stroke.color = Color32::from_rgb(17, 17, 27);
    ctx.set_visuals(visuals);
}

fn fmt_time(secs: f64) -> String {
    let total = secs.max(0.0) as u64;
    format!("{:02}:{:02}", total / 60, total % 60)
}

fn fmt_mb(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
}

pub fn render_gui(ctx: &egui::Context, state: &mut GuiState) {
    draw_top_bar(ctx, state);
    draw_player_bar(ctx, state);

    egui::CentralPanel::default().show(ctx, |ui| match state.active_tab {
        GuiTab::Library => draw_library(ui, state),
        GuiTab::Search => draw_search(ui, state),
        GuiTab::Playlists => draw_playlists(ui, state),
        GuiTab::Torrents => draw_torrents(ui, state),
        GuiTab::CacheSettings => draw_cache_settings(ui, state),
        GuiTab::Friends => draw_friends(ui, state),
    });

    draw_playlist_picker_modal(ctx, state);
}

fn draw_top_bar(ctx: &egui::Context, state: &mut GuiState) {
    let ticket = state.local_p2p_ticket();
    let web_url = format!("http://{}", ticket.endpoint);
    let scan_status = state.scan_rx.borrow().clone();
    let p_status = state.player.status();
    let backend_str = match p_status.backend {
        AudioBackendKind::NativeAlsaPipeWire => state.i18n.audio_backend_native(),
        AudioBackendKind::HeadlessNullSink => state.i18n.audio_backend_headless(),
    };

    egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new("♪ rust-audio-p2p")
                    .strong()
                    .size(16.0)
                    .color(Color32::from_rgb(137, 220, 235)),
            );
            ui.separator();

            match &scan_status {
                ScanStatus::Idle => {
                    ui.colored_label(Color32::GRAY, state.i18n.scan_idle());
                }
                ScanStatus::Scanning {
                    scanned_dirs,
                    audio_files_found,
                    ..
                } => {
                    ui.colored_label(
                        Color32::from_rgb(249, 226, 175),
                        state.i18n.scan_running(*scanned_dirs, *audio_files_found),
                    );
                }
                ScanStatus::Completed { indexed, .. } => {
                    ui.colored_label(
                        Color32::from_rgb(166, 227, 161),
                        state.i18n.scan_completed(*indexed),
                    );
                }
                ScanStatus::Error(e) => {
                    ui.colored_label(Color32::from_rgb(243, 139, 168), format!("Err: {e}"));
                }
            }

            ui.separator();
            ui.colored_label(Color32::from_rgb(249, 226, 175), backend_str);
            ui.separator();
            ui.colored_label(
                Color32::from_rgb(137, 220, 235),
                state.i18n.friends_web_url_label(&web_url),
            );

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let mut pref = state.i18n.pref;
                egui::ComboBox::from_id_salt("top_lang_combo")
                    .selected_text(match pref {
                        LanguagePreference::Auto => "Auto",
                        LanguagePreference::Es => "Español",
                        LanguagePreference::En => "English",
                    })
                    .width(90.0)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut pref, LanguagePreference::Auto, "Auto");
                        ui.selectable_value(&mut pref, LanguagePreference::Es, "Español");
                        ui.selectable_value(&mut pref, LanguagePreference::En, "English");
                    });
                if pref != state.i18n.pref {
                    state.set_language_preference(pref);
                }

                if ui.button("↻").clicked() {
                    state.trigger_rescan();
                }
            });
        });

        if let ScanStatus::Scanning {
            scanned_dirs,
            audio_files_found,
            indexed,
            ..
        } = &scan_status
        {
            ui.colored_label(
                Color32::from_rgb(249, 226, 175),
                state
                    .i18n
                    .scan_banner_running(*scanned_dirs, *audio_files_found, *indexed),
            );
        }

        ui.separator();
        ui.horizontal_wrapped(|ui| {
            let labels = [
                (GuiTab::Library, state.i18n.tab_library()),
                (GuiTab::Search, state.i18n.tab_search()),
                (GuiTab::Playlists, state.i18n.tab_playlists()),
                (GuiTab::Torrents, state.i18n.tab_torrents()),
                (GuiTab::CacheSettings, state.i18n.tab_cache_settings()),
                (GuiTab::Friends, state.i18n.tab_friends()),
            ];
            for (tab, label) in labels {
                let selected = state.active_tab == tab;
                if ui.selectable_label(selected, RichText::new(label).strong()).clicked() {
                    state.active_tab = tab;
                }
            }
        });
        ui.add_space(2.0);
    });
}

fn draw_player_bar(ctx: &egui::Context, state: &mut GuiState) {
    let pst = state.player.status();
    let state_label = match pst.state {
        PlaybackState::Playing => state.i18n.status_playing(),
        PlaybackState::Paused => state.i18n.status_paused(),
        PlaybackState::Stopped => state.i18n.status_stopped(),
    };
    let track_label = pst
        .current_track
        .as_ref()
        .map(|t| format!("{} — {} ({})", t.artist, t.title, t.album))
        .unwrap_or_else(|| state.i18n.no_track_playing().to_string());
    let prefetch_str = state
        .i18n
        .prefetch_label(pst.prefetch_track.as_ref().map(|t| t.title.as_str()));

    egui::TopBottomPanel::bottom("player_bar").show(ctx, |ui| {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.colored_label(Color32::from_rgb(137, 220, 235), RichText::new(state_label).strong());
            ui.label(RichText::new(track_label).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let mut vol = pst.volume;
                let vol_resp = ui.add(
                    egui::Slider::new(&mut vol, 0.0..=1.5)
                        .show_value(false)
                        .custom_formatter(|v, _| format!("{:.0}%", v * 100.0)),
                );
                if vol_resp.changed() {
                    let _ = state.player.set_volume(vol);
                }
                ui.label(format!("Vol {:.0}%", pst.volume * 100.0));

                if ui
                    .selectable_label(state.auto_next, state.i18n.auto_next_label(state.auto_next))
                    .clicked()
                {
                    state.toggle_auto_next();
                }

                if ui.button("⏭").clicked() {
                    let _ = state.next_track();
                }
                let play_icon = if pst.state == PlaybackState::Playing {
                    "⏸"
                } else {
                    "▶"
                };
                if ui.button(play_icon).clicked() {
                    let _ = state.player.toggle_pause();
                }
                if ui.button("⏮").clicked() {
                    let _ = state.prev_track();
                }
            });
        });

        let dur = pst.duration_secs.unwrap_or(0.0).max(0.0);
        let mut pos = state.seek_preview.unwrap_or(pst.position_secs).clamp(0.0, dur.max(1.0));
        ui.horizontal(|ui| {
            ui.label(fmt_time(pos));
            let slider_w = (ui.available_width() - 60.0).max(80.0);
            ui.spacing_mut().slider_width = slider_w;
            let resp = ui.add(egui::Slider::new(&mut pos, 0.0..=dur.max(1.0)).show_value(false));
            if resp.dragged() {
                state.seek_preview = Some(pos);
            } else if resp.drag_stopped() || resp.changed() {
                state.seek_preview = None;
                if dur > 0.0 {
                    let _ = state.player.seek_to(pos);
                }
            }
            ui.label(fmt_time(dur));
        });

        ui.horizontal(|ui| {
            ui.colored_label(Color32::GRAY, prefetch_str);
            if !state.status_message.is_empty() {
                ui.separator();
                ui.colored_label(Color32::from_rgb(166, 227, 161), &state.status_message);
            }
        });
        ui.add_space(4.0);
    });
}

fn draw_library(ui: &mut egui::Ui, state: &mut GuiState) {
    ui.horizontal(|ui| {
        ui.heading(state.i18n.tab_library());
        ui.add_space(12.0);
        ui.label("🔎");
        ui.text_edit_singleline(&mut state.library_filter);
        if ui.button("✕").clicked() {
            state.library_filter.clear();
        }
    });
    ui.separator();

    let q = state.library_filter.trim().to_ascii_lowercase();
    let list: Vec<Track> = if q.is_empty() {
        state.tracks.clone()
    } else {
        state
            .tracks
            .iter()
            .filter(|t| {
                t.title.to_ascii_lowercase().contains(&q)
                    || t.artist.to_ascii_lowercase().contains(&q)
                    || t.album.to_ascii_lowercase().contains(&q)
            })
            .cloned()
            .collect()
    };

    draw_track_table(ui, state, "lib_grid", &list, false);
}

fn draw_search(ui: &mut egui::Ui, state: &mut GuiState) {
    ui.horizontal(|ui| {
        ui.heading(state.i18n.tab_search());
        ui.add_space(12.0);
        ui.label(state.i18n.search_prompt());
        let resp = ui.text_edit_singleline(&mut state.search_query);
        if resp.changed() {
            let _ = state.refresh_from_db();
        }
    });
    ui.separator();

    let list = state.search_results.clone();
    draw_track_table(ui, state, "search_grid", &list, false);
}

fn draw_playlists(ui: &mut egui::Ui, state: &mut GuiState) {
    ui.horizontal(|ui| {
        ui.label(format!("{}:", state.i18n.playlists_header()));
        let resp = ui.text_edit_singleline(&mut state.new_playlist_input);
        if (resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
            || ui.button("+ Playlist").clicked()
        {
            let _ = state.create_playlist_from_input();
        }
    });

    if let Some(ren_id) = state.rename_playlist_id {
        let mut do_save = false;
        let mut do_cancel = false;
        ui.horizontal(|ui| {
            ui.label("✏");
            let resp = ui.text_edit_singleline(&mut state.rename_playlist_input);
            if (resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                || ui.button("💾").clicked()
            {
                do_save = true;
            }
            if ui.button("✖").clicked() {
                do_cancel = true;
            }
        });
        if do_save {
            let new_name = state.rename_playlist_input.clone();
            let _ = state.rename_playlist(ren_id, &new_name);
        } else if do_cancel {
            state.rename_playlist_id = None;
            state.rename_playlist_input.clear();
        }
    }
    ui.separator();

    let mut select_pl_idx: Option<usize> = None;
    let mut toggle_pl_pin: Option<(i64, String)> = None;
    let mut start_rename: Option<(i64, String)> = None;
    let mut delete_pl: Option<(i64, String)> = None;

    ui.columns(2, |cols| {
        cols[0].heading(format!(
            "{} ({})",
            state.i18n.playlists_header(),
            state.playlists.len()
        ));
        egui::ScrollArea::vertical()
            .id_salt("pl_list_scroll")
            .max_height(260.0)
            .show(&mut cols[0], |ui| {
                for (i, pl) in state.playlists.iter().enumerate() {
                    ui.horizontal(|ui| {
                        let pin_icon = if pl.pinned { "★" } else { "☆" };
                        if ui.button(pin_icon).clicked() {
                            toggle_pl_pin = Some((pl.id, pl.name.clone()));
                        }
                        if ui.button("✏").clicked() {
                            start_rename = Some((pl.id, pl.name.clone()));
                        }
                        if ui.button("🗑").clicked() {
                            delete_pl = Some((pl.id, pl.name.clone()));
                        }
                        let selected = state.selected_playlist_idx == Some(i);
                        if ui
                            .selectable_label(selected, format!("{} ({})", pl.name, pl.track_count))
                            .clicked()
                        {
                            select_pl_idx = Some(i);
                        }
                    });
                }
            });

        cols[0].separator();
        cols[0].heading(format!(
            "{} ({})",
            state.i18n.pinned_offline_header(),
            state.pinned_tracks.len()
        ));
        let pinned = state.pinned_tracks.clone();
        draw_track_table(&mut cols[0], state, "pinned_grid", &pinned, false);

        let active_pl = state
            .selected_playlist_idx
            .and_then(|i| state.playlists.get(i))
            .cloned();
        let pl_name = active_pl
            .as_ref()
            .map(|p| p.name.as_str())
            .unwrap_or("—");
        cols[1].horizontal(|ui| {
            ui.heading(state.i18n.playlist_tracks_header(pl_name));
            if let Some(ref pl) = active_pl {
                if ui.button("✏").clicked() {
                    start_rename = Some((pl.id, pl.name.clone()));
                }
                if ui.button("🗑").clicked() {
                    delete_pl = Some((pl.id, pl.name.clone()));
                }
            }
        });
        let pl_tracks = state.selected_playlist_tracks.clone();
        if !pl_tracks.is_empty() && cols[1].button("▶ Play All").clicked() {
            let _ = state.play_list_at(&pl_tracks, 0);
        }
        draw_track_table(&mut cols[1], state, "pl_tracks_grid", &pl_tracks, true);
    });

    if let Some(i) = select_pl_idx {
        state.selected_playlist_idx = Some(i);
        let _ = state.refresh_from_db();
    }
    if let Some((pl_id, name)) = toggle_pl_pin {
        let _ = state.toggle_playlist_pin(pl_id, &name);
    }
    if let Some((pl_id, name)) = start_rename {
        state.rename_playlist_id = Some(pl_id);
        state.rename_playlist_input = name;
    }
    if let Some((pl_id, name)) = delete_pl {
        let _ = state.delete_playlist(pl_id, &name);
    }
}

fn draw_torrents(ui: &mut egui::Ui, state: &mut GuiState) {
    ui.horizontal(|ui| {
        ui.label(state.i18n.torrents_search_prompt());
        let resp = ui.text_edit_singleline(&mut state.torrent_query);
        if (resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
            || ui.button("🔎 Search").clicked()
        {
            state.trigger_torrent_search();
        }
    });

    ui.horizontal(|ui| {
        ui.label(state.i18n.torrents_magnet_prompt());
        ui.text_edit_singleline(&mut state.torrent_magnet_input);
        if ui.button("⬇ Add Magnet").clicked() {
            let uri = state.torrent_magnet_input.trim().to_string();
            if !uri.is_empty() {
                state.add_torrent_uri(uri);
            }
        }
    });

    ui.horizontal(|ui| {
        ui.label(state.i18n.torrents_jackett_prompt());
        ui.text_edit_singleline(&mut state.jackett_url_input);
        if ui.button("Save URL").clicked() {
            state.save_jackett_url_override();
        }
    });

    if state.torrent_searching {
        ui.add(
            egui::ProgressBar::new(state.torrent_search_ticks)
                .text(state.i18n.torrents_searching_progress(&state.torrent_query)),
        );
    }
    ui.separator();

    let mut chosen_magnet: Option<String> = None;
    egui::ScrollArea::vertical()
        .id_salt("torrent_results_scroll")
        .max_height(240.0)
        .show(ui, |ui| {
            egui::Grid::new("torrent_results_grid")
                .striped(true)
                .num_columns(4)
                .show(ui, |ui| {
                    ui.strong("Torrent");
                    ui.strong("Seeds");
                    ui.strong("Size");
                    ui.strong("Action");
                    ui.end_row();
                    for res in &state.torrent_results {
                        ui.label(&res.title);
                        ui.label(format!("{}", res.seeders));
                        ui.label(fmt_mb(res.size_bytes));
                        if ui.button("⬇ Download").clicked() {
                            chosen_magnet = Some(res.magnet_uri.clone());
                        }
                        ui.end_row();
                    }
                });
        });
    if let Some(uri) = chosen_magnet {
        state.torrent_magnet_input = uri.clone();
        state.add_torrent_uri(uri);
    }

    ui.separator();
    let mut play_snap = None;
    ui.columns(2, |cols| {
        cols[0].heading(state.i18n.torrents_active_header());
        egui::ScrollArea::vertical()
            .id_salt("torrent_active_scroll")
            .show(&mut cols[0], |ui| {
                for (_u, name, started) in &state.pending_magnets {
                    ui.colored_label(
                        Color32::from_rgb(249, 226, 175),
                        format!("⏳ Resolving DHT ({}s): {name}", started.elapsed().as_secs()),
                    );
                }
                for snap in &state.active_torrents {
                    let frac = if snap.total_bytes > 0 {
                        (snap.progress_bytes as f32 / snap.total_bytes as f32).clamp(0.0, 1.0)
                    } else {
                        0.0
                    };
                    ui.horizontal(|ui| {
                        ui.add(egui::ProgressBar::new(frac).desired_width(140.0).show_percentage());
                        ui.label(format!(
                            "{} ({:.2} MB/s, p:{})",
                            snap.name, snap.download_speed_mbps, snap.peers_live
                        ));
                        if ui.button("▶").clicked() {
                            play_snap = Some(snap.clone());
                        }
                    });
                }
            });

        cols[1].heading(state.i18n.torrents_log_header());
        egui::ScrollArea::vertical()
            .id_salt("torrent_logs_scroll")
            .stick_to_bottom(true)
            .show(&mut cols[1], |ui| {
                for line in &state.torrent_search_logs {
                    ui.label(redact_url_secrets(line));
                }
            });
    });
    if let Some(snap) = play_snap {
        let _ = state.play_torrent_snapshot(&snap);
    }
}

fn draw_cache_settings(ui: &mut egui::Ui, state: &mut GuiState) {
    ui.heading(state.i18n.cache_header());
    ui.separator();

    let stats = &state.cache_stats;
    let frac = if stats.limit_bytes > 0 {
        (stats.used_bytes as f32 / stats.limit_bytes as f32).clamp(0.0, 1.0)
    } else {
        0.0
    };
    ui.add(egui::ProgressBar::new(frac).text(format!(
        "LRU: {} / {} (Pinned: {} · {} entries)",
        fmt_mb(stats.used_bytes),
        fmt_mb(stats.limit_bytes),
        fmt_mb(stats.pinned_bytes),
        stats.pinned_count
    )));

    ui.label(format!("DB: {}", state.paths.db_path.display()));
    ui.label(format!("Cache: {}", state.paths.cache_dir.display()));
    ui.label(format!("Pins: {}", state.paths.offline_pins_dir.display()));

    ui.add_space(8.0);
    ui.horizontal(|ui| {
        if ui.button("-512 MB Limit").clicked() {
            let _ = state.adjust_cache_limit_mb(-512);
        }
        if ui.button("+512 MB Limit").clicked() {
            let _ = state.adjust_cache_limit_mb(512);
        }
        if ui.button("🗑 Purge Unpinned Cache").clicked() {
            let _ = state.purge_cache();
        }
    });

    ui.separator();
    ui.horizontal(|ui| {
        ui.label(state.i18n.language_label());
        if ui.button("ES / EN / Auto").clicked() {
            let next = state.i18n.pref.cycle();
            state.set_language_preference(next);
        }
    });
}

fn draw_friends(ui: &mut egui::Ui, state: &mut GuiState) {
    let ticket = state.local_p2p_ticket();
    let ticket_uri = ticket.to_uri();
    let web_url = format!("http://{}", ticket.endpoint);
    let lan_enabled = state.is_lan_share_enabled();

    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.set_min_width((ui.available_width() - 180.0).max(260.0));
            ui.heading(state.i18n.friends_local_header());
            ui.colored_label(
                Color32::from_rgb(249, 226, 175),
                state.i18n.friends_web_url_label(&web_url),
            );
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(Color32::from_rgb(137, 220, 235), &ticket_uri);
                if ui.button("📋 Copy Token").clicked() {
                    ui.ctx().copy_text(ticket_uri.clone());
                    state.status_message = "Copied P2P token".to_string();
                }
            });
            ui.horizontal_wrapped(|ui| {
                if ui.button(state.i18n.friends_scan_btn()).clicked() {
                    state.trigger_lan_discovery();
                }
                if ui
                    .selectable_label(lan_enabled, state.i18n.friends_lan_share_label(lan_enabled))
                    .clicked()
                {
                    state.toggle_lan_share();
                }
            });
            ui.horizontal(|ui| {
                ui.label(state.i18n.friends_connect_prompt());
                let resp = ui.text_edit_singleline(&mut state.peer_connect_input);
                if (resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                    || ui.button("Connect").clicked()
                {
                    state.connect_peer_input();
                }
            });
        });

        draw_qr_matrix(ui, &ticket.render_unicode_qr(), 136.0);
    });

    ui.separator();
    let mut resync_target: Option<String> = None;
    ui.columns(2, |cols| {
        cols[0].heading(format!(
            "{} ({})",
            state.i18n.friends_peers_header(),
            state.paired_peers.len()
        ));
        if state.paired_peers.is_empty() {
            cols[0].colored_label(Color32::GRAY, state.i18n.friends_no_peers_hint());
        } else {
            for p in &state.paired_peers {
                cols[0].horizontal(|ui| {
                    ui.label(format!(
                        "● {} ({}) · {} trk / {} pl",
                        p.node_name, p.endpoint, p.track_count, p.playlist_count
                    ));
                    if ui.button("↻").clicked() {
                        let t = rust_audio_net_friends::PeerTicket {
                            node_id: p.node_id.clone(),
                            node_name: p.node_name.clone(),
                            endpoint: p.endpoint.clone(),
                            auth_token: p.auth_token.clone(),
                        };
                        resync_target = Some(t.to_uri());
                    }
                });
            }
        }

        cols[0].separator();
        cols[0].heading(state.i18n.friends_log_header());
        egui::ScrollArea::vertical()
            .id_salt("p2p_logs_scroll")
            .stick_to_bottom(true)
            .show(&mut cols[0], |ui| {
                for line in &state.p2p_logs {
                    ui.label(line);
                }
            });

        cols[1].heading(format!(
            "{} ({})",
            state.i18n.friends_tracks_header(),
            state.peer_tracks.len()
        ));
        let remote_list = state.peer_tracks.clone();
        draw_track_table(&mut cols[1], state, "peer_tracks_grid", &remote_list, false);
    });

    if let Some(uri) = resync_target {
        state.spawn_peer_sync(uri);
    }
}

fn draw_qr_matrix(ui: &mut egui::Ui, lines: &[String], size_px: f32) {
    // Decode Unicode half-block lines ('█', '▀', '▄', ' ') back into a clean 2D boolean matrix.
    let mut rows: Vec<Vec<bool>> = Vec::new();
    for line in lines {
        let mut top = Vec::new();
        let mut bot = Vec::new();
        for ch in line.chars() {
            match ch {
                '█' => {
                    top.push(true);
                    bot.push(true);
                }
                '▀' => {
                    top.push(true);
                    bot.push(false);
                }
                '▄' => {
                    top.push(false);
                    bot.push(true);
                }
                _ => {
                    top.push(false);
                    bot.push(false);
                }
            }
        }
        rows.push(top);
        rows.push(bot);
    }

    let n_rows = rows.len().max(1);
    let n_cols = rows.first().map(|r| r.len()).unwrap_or(1).max(1);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size_px, size_px), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, Color32::WHITE);

    let cell_w = rect.width() / n_cols as f32;
    let cell_h = rect.height() / n_rows as f32;
    for (r, row) in rows.iter().enumerate() {
        for (c, &dark) in row.iter().enumerate() {
            if dark {
                let x = rect.min.x + c as f32 * cell_w;
                let y = rect.min.y + r as f32 * cell_h;
                let cell = egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(cell_w + 0.3, cell_h + 0.3));
                painter.rect_filled(cell, 0.0, Color32::BLACK);
            }
        }
    }
}

fn draw_track_table(
    ui: &mut egui::Ui,
    state: &mut GuiState,
    id_salt: &str,
    tracks: &[Track],
    show_remove: bool,
) {
    let mut action: Option<TrackRowAction> = None;
    let current_key = state.queue.current().map(|t| t.source.unique_key());

    if !state.marked_track_ids.is_empty() {
        ui.horizontal(|ui| {
            ui.strong(state.i18n.marked_tracks_badge(state.marked_track_ids.len()));
            if ui.button("+ Playlist").clicked() {
                state.open_batch_playlist_picker();
            }
            if ui.button("✖").clicked() {
                state.marked_track_ids.clear();
            }
        });
    }

    egui::ScrollArea::vertical()
        .id_salt(id_salt)
        .auto_shrink([false; 2])
        .show(ui, |ui| {
            egui::Grid::new(format!("{id_salt}_grid"))
                .striped(true)
                .num_columns(if show_remove { 8 } else { 7 })
                .show(ui, |ui| {
                    ui.strong("✓");
                    ui.strong("");
                    ui.strong(state.i18n.col_pin());
                    ui.strong(state.i18n.col_title());
                    ui.strong(state.i18n.col_artist());
                    ui.strong(state.i18n.col_album());
                    ui.strong(state.i18n.col_format());
                    if show_remove {
                        ui.strong("");
                    }
                    ui.end_row();

                    for (idx, tr) in tracks.iter().enumerate() {
                        let is_cur = current_key.as_deref() == Some(tr.source.unique_key().as_str());
                        let mut marked = state.marked_track_ids.contains(&tr.id);
                        if ui.checkbox(&mut marked, "").changed() {
                            if marked {
                                state.marked_track_ids.insert(tr.id);
                            } else {
                                state.marked_track_ids.remove(&tr.id);
                            }
                        }
                        if ui.button(if is_cur { "🔊" } else { "▶" }).clicked() {
                            action = Some(TrackRowAction::Play(idx));
                        }
                        ui.horizontal(|ui| {
                            if ui.button(if tr.pinned { "★" } else { "☆" }).clicked() {
                                action = Some(TrackRowAction::TogglePin(tr.clone()));
                            }
                            if ui.button("+").clicked() {
                                action = Some(TrackRowAction::OpenPlaylistPicker(tr.clone()));
                            }
                        });
                        if ui
                            .selectable_label(is_cur, &tr.title)
                            .double_clicked()
                        {
                            action = Some(TrackRowAction::Play(idx));
                        }
                        ui.label(&tr.artist);
                        ui.label(&tr.album);
                        let fmt_label = match &tr.source {
                            TrackSource::Peer { cached_path, .. } => {
                                let ok = cached_path
                                    .as_deref()
                                    .map(|p| Path::new(p).exists())
                                    .unwrap_or(false);
                                if ok {
                                    format!("{} ✓", tr.format.to_uppercase())
                                } else {
                                    format!("{} ☁", tr.format.to_uppercase())
                                }
                            }
                            _ => tr.format.to_uppercase(),
                        };
                        ui.label(fmt_label);
                        if show_remove && ui.button("✕").clicked() {
                            action = Some(TrackRowAction::RemoveFromPlaylist(tr.clone()));
                        }
                        ui.end_row();
                    }
                });
        });

    if let Some(act) = action {
        match act {
            TrackRowAction::Play(idx) => {
                let _ = state.play_list_at(tracks, idx);
            }
            TrackRowAction::TogglePin(tr) => {
                let _ = state.toggle_track_pin(&tr);
            }
            TrackRowAction::OpenPlaylistPicker(tr) => {
                state.playlist_picker = Some(PlaylistPickerModal {
                    tracks: vec![tr],
                    new_name: String::new(),
                });
            }
            TrackRowAction::RemoveFromPlaylist(tr) => {
                let _ = state.remove_track_from_active_playlist(&tr);
            }
        }
    }
}

fn draw_playlist_picker_modal(ctx: &egui::Context, state: &mut GuiState) {
    let Some(mut picker) = state.playlist_picker.take() else {
        return;
    };
    let mut keep_open = true;
    let mut add_existing: Option<(i64, String)> = None;
    let mut create_and_add: Option<String> = None;

    let picker_summary = if picker.tracks.len() == 1 {
        format!("♪ {} — {}", picker.tracks[0].artist, picker.tracks[0].title)
    } else {
        format!("♪ {} pistas seleccionadas / tracks selected", picker.tracks.len())
    };

    egui::Window::new(state.i18n.playlist_picker_title())
        .open(&mut keep_open)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.label(RichText::new(picker_summary).strong());
            ui.separator();
            for pl in &state.playlists {
                if ui
                    .button(format!("{} ({})", pl.name, pl.track_count))
                    .clicked()
                {
                    add_existing = Some((pl.id, pl.name.clone()));
                }
            }
            ui.separator();
            ui.horizontal(|ui| {
                ui.label(state.i18n.playlist_picker_new_option());
                ui.text_edit_singleline(&mut picker.new_name);
                if ui.button("OK").clicked() && !picker.new_name.trim().is_empty() {
                    create_and_add = Some(picker.new_name.trim().to_string());
                }
            });
        });

    if let Some((pl_id, pl_name)) = add_existing {
        let _ = state.add_tracks_to_playlist(pl_id, &pl_name, &picker.tracks);
    } else if let Some(name) = create_and_add {
        let _ = state.create_playlist_and_add_tracks(&name, &picker.tracks);
    } else if keep_open {
        state.playlist_picker = Some(picker);
    }
}
