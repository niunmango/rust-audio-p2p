use std::path::Path;

use eframe::egui::{self, Color32, CornerRadius, RichText, Stroke};
use rust_audio_core::{LanguagePreference, ScanStatus, Track, TrackSource};
use rust_audio_player::{AudioBackendKind, PlaybackState};
use rust_audio_source_torrent::redact_url_secrets;

use crate::state::{GuiState, GuiTab, LibrarySourceFilter, PlaylistPickerModal};

const BG_BASE: Color32 = Color32::from_rgb(17, 17, 27);
const BG_SIDEBAR: Color32 = Color32::from_rgb(20, 20, 32);
const BG_SURFACE: Color32 = Color32::from_rgb(24, 24, 37);
const BG_CARD: Color32 = Color32::from_rgb(30, 30, 46);
const BG_ROW_ALT: Color32 = Color32::from_rgb(24, 25, 38);
const BG_ROW_ACTIVE: Color32 = Color32::from_rgb(35, 45, 64);
const BORDER_SUBTLE: Color32 = Color32::from_rgb(49, 50, 68);
const TEXT_MAIN: Color32 = Color32::from_rgb(205, 214, 244);
const TEXT_MUTED: Color32 = Color32::from_rgb(147, 153, 178);
const ACCENT_CYAN: Color32 = Color32::from_rgb(137, 220, 235);
const ACCENT_GREEN: Color32 = Color32::from_rgb(166, 227, 161);
const ACCENT_YELLOW: Color32 = Color32::from_rgb(249, 226, 175);
const ACCENT_RED: Color32 = Color32::from_rgb(243, 139, 168);

enum TrackRowAction {
    Play(usize),
    Enqueue(Track),
    TogglePin(Track),
    OpenPlaylistPicker(Track),
    RemoveFromPlaylist(Track),
}

pub fn apply_catppuccin_dark(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = BG_BASE;
    visuals.window_fill = BG_SURFACE;
    visuals.extreme_bg_color = BG_CARD;
    visuals.faint_bg_color = BG_ROW_ALT;
    visuals.widgets.noninteractive.bg_fill = BG_SURFACE;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, BORDER_SUBTLE);
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, TEXT_MAIN);
    visuals.widgets.noninteractive.corner_radius = CornerRadius::same(7);
    visuals.widgets.inactive.bg_fill = BG_CARD;
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, BORDER_SUBTLE);
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, TEXT_MAIN);
    visuals.widgets.inactive.corner_radius = CornerRadius::same(6);
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(49, 50, 68);
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, ACCENT_CYAN);
    visuals.widgets.hovered.corner_radius = CornerRadius::same(6);
    visuals.widgets.active.bg_fill = Color32::from_rgb(56, 68, 92);
    visuals.widgets.active.bg_stroke = Stroke::new(1.0_f32, ACCENT_CYAN);
    visuals.widgets.active.corner_radius = CornerRadius::same(6);
    visuals.selection.bg_fill = Color32::from_rgb(43, 89, 112);
    visuals.selection.stroke = Stroke::new(1.0_f32, ACCENT_CYAN);
    visuals.window_corner_radius = CornerRadius::same(10);
    visuals.window_stroke = Stroke::new(1.0_f32, ACCENT_CYAN);
    ctx.set_visuals(visuals);

    ctx.style_mut(|s| {
        s.spacing.item_spacing = egui::vec2(8.0, 6.0);
        s.spacing.button_padding = egui::vec2(9.0, 5.0);
        s.spacing.scroll.bar_width = 8.0;
    });
}

fn fmt_time(secs: f64) -> String {
    let total = secs.max(0.0) as u64;
    format!("{:02}:{:02}", total / 60, total % 60)
}

fn fmt_dur_opt(secs: Option<u64>) -> String {
    match secs {
        Some(s) if s > 0 => format!("{:02}:{:02}", s / 60, s % 60),
        _ => "--:--".to_string(),
    }
}

fn fmt_mb(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
}

fn card_frame() -> egui::Frame {
    egui::Frame::NONE
        .fill(BG_SURFACE)
        .stroke(Stroke::new(1.0_f32, BORDER_SUBTLE))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin::same(12))
}

pub fn render_gui(ctx: &egui::Context, state: &mut GuiState) {
    handle_gui_shortcuts(ctx, state);
    draw_player_bar(ctx, state);
    draw_sidebar(ctx, state);

    egui::CentralPanel::default()
        .frame(
            egui::Frame::NONE
                .fill(BG_BASE)
                .inner_margin(egui::Margin::symmetric(14, 10)),
        )
        .show(ctx, |ui| match state.active_tab {
            GuiTab::Library => draw_library(ui, state),
            GuiTab::Search => draw_search(ui, state),
            GuiTab::Playlists => draw_playlists(ui, state),
            GuiTab::Torrents => draw_torrents(ui, state),
            GuiTab::CacheSettings => draw_cache_settings(ui, state),
            GuiTab::Friends => draw_friends(ui, state),
        });

    draw_playlist_picker_modal(ctx, state);
    draw_help_modal(ctx, state);
}

fn handle_gui_shortcuts(ctx: &egui::Context, state: &mut GuiState) {
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        if state.show_help_modal {
            state.show_help_modal = false;
        } else if state.playlist_picker.is_some() {
            state.playlist_picker = None;
        } else if state.rename_playlist_id.is_some() {
            state.rename_playlist_id = None;
            state.rename_playlist_input.clear();
        } else {
            state.marked_track_ids.clear();
        }
        return;
    }

    if ctx.input(|i| i.key_pressed(egui::Key::F1)) {
        state.show_help_modal = !state.show_help_modal;
        return;
    }

    if ctx.wants_keyboard_input() {
        return;
    }

    ctx.input(|i| {
        if i.key_pressed(egui::Key::Questionmark) {
            state.show_help_modal = !state.show_help_modal;
        } else if i.key_pressed(egui::Key::Space) {
            let _ = state.player.toggle_pause();
        } else if i.key_pressed(egui::Key::ArrowLeft) {
            let _ = state.player.seek_relative(-10.0);
        } else if i.key_pressed(egui::Key::ArrowRight) {
            let _ = state.player.seek_relative(10.0);
        } else if i.key_pressed(egui::Key::ArrowUp) {
            state.selected_row_idx = state.selected_row_idx.saturating_sub(1);
        } else if i.key_pressed(egui::Key::ArrowDown) {
            state.selected_row_idx = state.selected_row_idx.saturating_add(1);
        } else if i.key_pressed(egui::Key::N) {
            let _ = state.next_track();
        } else if i.key_pressed(egui::Key::B) {
            let _ = state.prev_track();
        } else if i.key_pressed(egui::Key::W) {
            let _ = state.enqueue_marked_tracks();
        } else if i.key_pressed(egui::Key::R) {
            state.trigger_rescan();
        } else if i.key_pressed(egui::Key::L) {
            let next = state.i18n.pref.cycle();
            state.set_language_preference(next);
        } else if i.key_pressed(egui::Key::Num1) {
            state.active_tab = GuiTab::Library;
            state.selected_row_idx = 0;
        } else if i.key_pressed(egui::Key::Num2) {
            state.active_tab = GuiTab::Search;
            state.selected_row_idx = 0;
        } else if i.key_pressed(egui::Key::Num3) {
            state.active_tab = GuiTab::Playlists;
            state.selected_row_idx = 0;
        } else if i.key_pressed(egui::Key::Num4) {
            state.active_tab = GuiTab::Torrents;
            state.selected_row_idx = 0;
        } else if i.key_pressed(egui::Key::Num5) {
            state.active_tab = GuiTab::CacheSettings;
            state.selected_row_idx = 0;
        } else if i.key_pressed(egui::Key::Num6) {
            state.active_tab = GuiTab::Friends;
            state.selected_row_idx = 0;
        }
    });
}

fn draw_sidebar(ctx: &egui::Context, state: &mut GuiState) {
    let ticket = state.local_p2p_ticket();
    let web_url = format!("http://{}", ticket.endpoint);
    let scan_status = state.scan_rx.borrow().clone();
    let p_status = state.player.status();
    let backend_str = match p_status.backend {
        AudioBackendKind::NativeAlsaPipeWire => state.i18n.audio_backend_native(),
        AudioBackendKind::HeadlessNullSink => state.i18n.audio_backend_headless(),
    };

    egui::SidePanel::left("nav_sidebar")
        .resizable(false)
        .exact_width(224.0)
        .frame(
            egui::Frame::NONE
                .fill(BG_SIDEBAR)
                .stroke(Stroke::new(1.0_f32, BORDER_SUBTLE))
                .inner_margin(egui::Margin::same(12)),
        )
        .show(ctx, |ui| {
            // Brand header
            ui.horizontal(|ui| {
                ui.colored_label(ACCENT_CYAN, RichText::new("♪").size(20.0).strong());
                ui.vertical(|ui| {
                    ui.colored_label(TEXT_MAIN, RichText::new("rust-audio-p2p").size(15.0).strong());
                    ui.colored_label(TEXT_MUTED, RichText::new("Local • Torrent • P2P").size(10.5));
                });
            });
            ui.add_space(6.0);
            ui.separator();
            ui.add_space(4.0);

            // Navigation items with counts
            let nav_items = [
                (GuiTab::Library, state.i18n.tab_library(), Some(state.tracks.len())),
                (GuiTab::Search, state.i18n.tab_search(), Some(state.search_results.len())),
                (GuiTab::Playlists, state.i18n.tab_playlists(), Some(state.playlists.len())),
                (GuiTab::Torrents, state.i18n.tab_torrents(), Some(state.active_torrents.len())),
                (GuiTab::CacheSettings, state.i18n.tab_cache_settings(), None),
                (GuiTab::Friends, state.i18n.tab_friends(), Some(state.paired_peers.len())),
            ];

            for (tab, label, count_opt) in nav_items {
                let selected = state.active_tab == tab;
                let fill = if selected {
                    Color32::from_rgb(36, 56, 74)
                } else {
                    Color32::TRANSPARENT
                };
                let stroke = if selected {
                    Stroke::new(1.0_f32, ACCENT_CYAN)
                } else {
                    Stroke::NONE
                };
                let text_col = if selected { ACCENT_CYAN } else { TEXT_MAIN };

                let resp = egui::Frame::NONE
                    .fill(fill)
                    .stroke(stroke)
                    .corner_radius(CornerRadius::same(7))
                    .inner_margin(egui::Margin::symmetric(8, 6))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.colored_label(text_col, RichText::new(label).size(13.0).strong());
                            if let Some(cnt) = count_opt {
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    ui.colored_label(
                                        if selected { ACCENT_CYAN } else { TEXT_MUTED },
                                        RichText::new(format!("{cnt}")).size(11.0),
                                    );
                                });
                            }
                        });
                    })
                    .response
                    .interact(egui::Sense::click());
                if resp.clicked() {
                    state.active_tab = tab;
                }
            }

            ui.add_space(6.0);
            if ui
                .add_sized(
                    [ui.available_width(), 30.0],
                    egui::Button::new(
                        RichText::new(state.i18n.help_btn_label())
                            .color(ACCENT_YELLOW)
                            .strong(),
                    )
                    .fill(BG_CARD),
                )
                .clicked()
            {
                state.show_help_modal = true;
            }

            // Bottom node telemetry card
            ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                card_frame()
                    .fill(BG_CARD)
                    .inner_margin(egui::Margin::same(10))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        // Language row
                        ui.horizontal(|ui| {
                            let mut pref = state.i18n.pref;
                            egui::ComboBox::from_id_salt("sidebar_lang_combo")
                                .selected_text(match pref {
                                    LanguagePreference::Auto => "Auto (ES/EN)",
                                    LanguagePreference::Es => "Español",
                                    LanguagePreference::En => "English",
                                })
                                .width(128.0)
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(&mut pref, LanguagePreference::Auto, "Auto (ES/EN)");
                                    ui.selectable_value(&mut pref, LanguagePreference::Es, "Español");
                                    ui.selectable_value(&mut pref, LanguagePreference::En, "English");
                                });
                            if pref != state.i18n.pref {
                                state.set_language_preference(pref);
                            }
                            if ui
                                .button("↻")
                                .on_hover_text(match state.i18n.lang {
                                    rust_audio_core::Language::Es => "Re-escanear música",
                                    rust_audio_core::Language::En => "Rescan music",
                                })
                                .clicked()
                            {
                                state.trigger_rescan();
                            }
                        });

                        ui.add_space(4.0);
                        // Web UI URL row ([Copy] right-aligned so it never clips)
                        ui.horizontal(|ui| {
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.small_button("Copy").clicked() {
                                    ui.ctx().copy_text(web_url.clone());
                                    state.status_message = format!("Copied {web_url}");
                                }
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(&web_url)
                                            .color(ACCENT_CYAN)
                                            .size(11.0)
                                            .monospace(),
                                    )
                                    .truncate(),
                                );
                            });
                        });

                        ui.add_space(2.0);
                        // Cache mini gauge
                        let cstats = &state.cache_stats;
                        let frac = if cstats.limit_bytes > 0 {
                            (cstats.used_bytes as f32 / cstats.limit_bytes as f32).clamp(0.0, 1.0)
                        } else {
                            0.0
                        };
                        ui.add(
                            egui::ProgressBar::new(frac)
                                .desired_height(13.0)
                                .text(RichText::new(format!("LRU {} / {}", fmt_mb(cstats.used_bytes), fmt_mb(cstats.limit_bytes))).size(10.0)),
                        );

                        ui.add_space(2.0);
                        ui.colored_label(TEXT_MUTED, RichText::new(backend_str).size(10.5));

                        match &scan_status {
                            ScanStatus::Idle => {
                                ui.colored_label(TEXT_MUTED, RichText::new(state.i18n.scan_idle()).size(10.5));
                            }
                            ScanStatus::Scanning {
                                scanned_dirs,
                                audio_files_found,
                                ..
                            } => {
                                ui.colored_label(
                                    ACCENT_YELLOW,
                                    RichText::new(state.i18n.scan_running(*scanned_dirs, *audio_files_found)).size(10.5),
                                );
                            }
                            ScanStatus::Completed { indexed, .. } => {
                                ui.colored_label(
                                    ACCENT_GREEN,
                                    RichText::new(state.i18n.scan_completed(*indexed)).size(10.5),
                                );
                            }
                            ScanStatus::Error(e) => {
                                ui.colored_label(ACCENT_RED, RichText::new(format!("Err: {e}")).size(10.5));
                            }
                        }
                    });
            });
        });
}

fn draw_album_badge(ui: &mut egui::Ui, playing: bool) {
    let size = egui::vec2(46.0, 46.0);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let p = ui.painter_at(rect);
    let accent = if playing { ACCENT_CYAN } else { BORDER_SUBTLE };
    p.rect_filled(rect, CornerRadius::same(8), BG_CARD);
    p.rect_stroke(rect, CornerRadius::same(8), Stroke::new(1.5_f32, accent), egui::StrokeKind::Inside);
    let c = rect.center();
    p.circle_stroke(c, 15.0, Stroke::new(1.2_f32, accent));
    p.circle_stroke(c, 7.0, Stroke::new(1.0_f32, TEXT_MUTED));
    p.text(
        c,
        egui::Align2::CENTER_CENTER,
        "♪",
        egui::FontId::proportional(15.0),
        if playing { ACCENT_CYAN } else { TEXT_MAIN },
    );
}

fn draw_player_bar(ctx: &egui::Context, state: &mut GuiState) {
    let pst = state.player.status();
    let is_playing = pst.state == PlaybackState::Playing;
    let state_label = match pst.state {
        PlaybackState::Playing => state.i18n.status_playing(),
        PlaybackState::Paused => state.i18n.status_paused(),
        PlaybackState::Stopped => state.i18n.status_stopped(),
    };
    let cur_track = pst.current_track.clone();
    let (title_str, sub_str) = match &cur_track {
        Some(t) => (
            t.title.clone(),
            format!("{} • {}", t.artist, t.album),
        ),
        None => (
            state.i18n.no_track_playing().to_string(),
            state.i18n.prefetch_label(pst.prefetch_track.as_ref().map(|t| t.title.as_str())),
        ),
    };

    egui::TopBottomPanel::bottom("player_bar")
        .frame(
            egui::Frame::NONE
                .fill(BG_SURFACE)
                .stroke(Stroke::new(1.0_f32, BORDER_SUBTLE))
                .inner_margin(egui::Margin::symmetric(14, 8)),
        )
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                // Left: Album disc badge + current track details + quick ★/+PL actions
                draw_album_badge(ui, is_playing);
                ui.vertical(|ui| {
                    ui.set_max_width(340.0);
                    ui.add(
                        egui::Label::new(RichText::new(&title_str).size(14.0).strong().color(TEXT_MAIN))
                            .truncate(),
                    );
                    ui.add(
                        egui::Label::new(RichText::new(&sub_str).size(11.5).color(TEXT_MUTED))
                            .truncate(),
                    );
                });
                if let Some(ref tr) = cur_track {
                    if ui
                        .button(if tr.pinned { "★" } else { "☆" })
                        .on_hover_text("Pin Offline")
                        .clicked()
                    {
                        let _ = state.toggle_track_pin(tr);
                    }
                    if ui.button("+PL").clicked() {
                        state.playlist_picker = Some(PlaylistPickerModal {
                            tracks: vec![tr.clone()],
                            new_name: String::new(),
                        });
                    }
                }

                ui.add_space(8.0);
                // Center: Transport buttons + state badge + queue badge
                ui.colored_label(
                    if is_playing { ACCENT_CYAN } else { TEXT_MUTED },
                    RichText::new(state_label).size(11.0).strong(),
                );
                if !state.queue.is_empty() {
                    let cur = state.queue.current_index().map(|idx| idx + 1).unwrap_or(0);
                    let total = state.queue.len();
                    ui.colored_label(
                        ACCENT_YELLOW,
                        RichText::new(state.i18n.queue_badge(cur, total)).size(11.0),
                    );
                }
                if ui.button("|◀").clicked() {
                    let _ = state.prev_track();
                }
                let play_text = if is_playing { "⏸ Pause" } else { "▶ Play" };
                let play_btn = egui::Button::new(
                    RichText::new(play_text)
                        .color(if is_playing { BG_BASE } else { TEXT_MAIN })
                        .strong(),
                )
                .fill(if is_playing { ACCENT_CYAN } else { Color32::from_rgb(46, 56, 78) });
                if ui.add(play_btn).clicked() {
                    let _ = state.player.toggle_pause();
                }
                if ui.button("▶|").clicked() {
                    let _ = state.next_track();
                }
                if ui
                    .selectable_label(state.auto_next, state.i18n.auto_next_label(state.auto_next))
                    .clicked()
                {
                    state.toggle_auto_next();
                }

                // Right: Volume slider
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let mut vol = pst.volume;
                    ui.spacing_mut().slider_width = 96.0;
                    let vol_resp = ui.add(
                        egui::Slider::new(&mut vol, 0.0..=1.5)
                            .show_value(false),
                    );
                    if vol_resp.changed() {
                        let _ = state.player.set_volume(vol);
                    }
                    ui.colored_label(TEXT_MUTED, RichText::new(format!("Vol {:.0}%", pst.volume * 100.0)).size(11.5));
                });
            });

            ui.add_space(3.0);
            let dur = pst.duration_secs.unwrap_or(0.0).max(0.0);
            let mut pos = state.seek_preview.unwrap_or(pst.position_secs).clamp(0.0, dur.max(1.0));
            ui.horizontal(|ui| {
                ui.colored_label(TEXT_MUTED, RichText::new(fmt_time(pos)).size(11.0).monospace());
                let slider_w = (ui.available_width() - 62.0).max(80.0);
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
                ui.colored_label(TEXT_MUTED, RichText::new(fmt_time(dur)).size(11.0).monospace());
            });

            if !state.status_message.is_empty() {
                ui.colored_label(ACCENT_GREEN, RichText::new(&state.status_message).size(11.0));
            }
        });
}

fn draw_library(ui: &mut egui::Ui, state: &mut GuiState) {
    let q = state.library_filter.trim().to_ascii_lowercase();
    let list: Vec<Track> = state
        .tracks
        .iter()
        .filter(|t| match state.library_source_filter {
            LibrarySourceFilter::All => true,
            LibrarySourceFilter::PinnedOnly => t.pinned,
            LibrarySourceFilter::PeerOnly => matches!(t.source, TrackSource::Peer { .. }),
        })
        .filter(|t| {
            q.is_empty()
                || t.title.to_ascii_lowercase().contains(&q)
                || t.artist.to_ascii_lowercase().contains(&q)
                || t.album.to_ascii_lowercase().contains(&q)
                || t.format.to_ascii_lowercase().contains(&q)
        })
        .cloned()
        .collect();

    card_frame().inner_margin(egui::Margin::symmetric(10, 8)).show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.colored_label(ACCENT_CYAN, RichText::new(state.i18n.tab_library()).size(16.0).strong());
            ui.colored_label(TEXT_MUTED, RichText::new(format!("({})", list.len())).size(12.0));
            ui.add_space(8.0);
            ui.add(
                egui::TextEdit::singleline(&mut state.library_filter)
                    .desired_width(220.0)
                    .hint_text(state.i18n.search_prompt().trim_end_matches(": ")),
            );
            if !state.library_filter.is_empty() && ui.small_button("×").clicked() {
                state.library_filter.clear();
            }

            ui.separator();
            for (kind, lbl) in [
                (LibrarySourceFilter::All, state.i18n.filter_all()),
                (LibrarySourceFilter::PinnedOnly, state.i18n.filter_pinned()),
                (LibrarySourceFilter::PeerOnly, state.i18n.filter_p2p()),
            ] {
                if ui.selectable_label(state.library_source_filter == kind, lbl).clicked() {
                    state.library_source_filter = kind;
                }
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if !list.is_empty() && ui.button("▶ Play All").clicked() {
                    let _ = state.play_list_at(&list, 0);
                }
                if !list.is_empty() && ui.button(state.i18n.select_all_visible()).clicked() {
                    for tr in &list {
                        state.marked_track_ids.insert(tr.id);
                    }
                }
            });
        });
    });
    ui.add_space(6.0);

    draw_track_table(ui, state, "lib_grid", &list, false);
}

fn draw_search(ui: &mut egui::Ui, state: &mut GuiState) {
    let list = state.search_results.clone();

    card_frame().inner_margin(egui::Margin::symmetric(10, 8)).show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.colored_label(ACCENT_CYAN, RichText::new(state.i18n.tab_search()).size(16.0).strong());
            ui.colored_label(TEXT_MUTED, RichText::new(format!("({})", list.len())).size(12.0));
            ui.add_space(8.0);
            ui.label(state.i18n.search_prompt());
            let resp = ui.add(
                egui::TextEdit::singleline(&mut state.search_query)
                    .desired_width(280.0),
            );
            if resp.changed() {
                let _ = state.refresh_from_db();
            }
            if !state.search_query.is_empty() && ui.small_button("×").clicked() {
                state.search_query.clear();
                let _ = state.refresh_from_db();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if !list.is_empty() && ui.button(state.i18n.select_all_visible()).clicked() {
                    for tr in &list {
                        state.marked_track_ids.insert(tr.id);
                    }
                }
            });
        });
    });
    ui.add_space(6.0);

    draw_track_table(ui, state, "search_grid", &list, false);
}

fn draw_playlists(ui: &mut egui::Ui, state: &mut GuiState) {
    card_frame().inner_margin(egui::Margin::symmetric(10, 8)).show(ui, |ui| {
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
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.colored_label(ACCENT_YELLOW, state.i18n.rename_btn());
                let resp = ui.text_edit_singleline(&mut state.rename_playlist_input);
                if (resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                    || ui.button(state.i18n.save_btn()).clicked()
                {
                    do_save = true;
                }
                if ui.button(state.i18n.cancel_btn()).clicked() {
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
    });
    ui.add_space(6.0);

    let mut select_pl_idx: Option<usize> = None;
    let mut toggle_pl_pin: Option<(i64, String)> = None;
    let mut start_rename: Option<(i64, String)> = None;
    let mut delete_pl: Option<(i64, String)> = None;

    ui.columns(2, |cols| {
        cols[0].colored_label(
            ACCENT_CYAN,
            RichText::new(format!(
                "{} ({})",
                state.i18n.playlists_header(),
                state.playlists.len()
            ))
            .size(14.0)
            .strong(),
        );
        egui::ScrollArea::vertical()
            .id_salt("pl_list_scroll")
            .max_height(220.0)
            .show(&mut cols[0], |ui| {
                for (i, pl) in state.playlists.iter().enumerate() {
                    let selected = state.selected_playlist_idx == Some(i);
                    egui::Frame::NONE
                        .fill(if selected { BG_ROW_ACTIVE } else { BG_SURFACE })
                        .stroke(Stroke::new(1.0_f32, if selected { ACCENT_CYAN } else { BORDER_SUBTLE }))
                        .corner_radius(CornerRadius::same(6))
                        .inner_margin(egui::Margin::symmetric(8, 5))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                if ui.small_button(if pl.pinned { "★" } else { "☆" }).clicked() {
                                    toggle_pl_pin = Some((pl.id, pl.name.clone()));
                                }
                                if ui
                                    .selectable_label(selected, format!("{} ({})", pl.name, pl.track_count))
                                    .clicked()
                                {
                                    select_pl_idx = Some(i);
                                }
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if ui.small_button(state.i18n.delete_btn()).clicked() {
                                        delete_pl = Some((pl.id, pl.name.clone()));
                                    }
                                    if ui.small_button(state.i18n.rename_btn()).clicked() {
                                        start_rename = Some((pl.id, pl.name.clone()));
                                    }
                                });
                            });
                        });
                }
            });

        cols[0].add_space(6.0);
        cols[0].colored_label(
            ACCENT_YELLOW,
            RichText::new(format!(
                "{} ({})",
                state.i18n.pinned_offline_header(),
                state.pinned_tracks.len()
            ))
            .size(14.0)
            .strong(),
        );
        let pinned = state.pinned_tracks.clone();
        draw_track_table(&mut cols[0], state, "pinned_grid", &pinned, false);

        let active_pl = state
            .selected_playlist_idx
            .and_then(|i| state.playlists.get(i))
            .cloned();
        let pl_name = active_pl.as_ref().map(|p| p.name.as_str()).unwrap_or("—");
        cols[1].horizontal_wrapped(|ui| {
            ui.colored_label(
                ACCENT_CYAN,
                RichText::new(state.i18n.playlist_tracks_header(pl_name)).size(14.0).strong(),
            );
            if let Some(ref pl) = active_pl {
                if ui.small_button(state.i18n.rename_btn()).clicked() {
                    start_rename = Some((pl.id, pl.name.clone()));
                }
                if ui.small_button(state.i18n.delete_btn()).clicked() {
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
    card_frame().show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(state.i18n.torrents_search_prompt());
            let resp = ui.text_edit_singleline(&mut state.torrent_query);
            if (resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                || ui.button("Search").clicked()
            {
                state.trigger_torrent_search();
            }
        });
        ui.horizontal(|ui| {
            ui.label(state.i18n.torrents_magnet_prompt());
            ui.text_edit_singleline(&mut state.torrent_magnet_input);
            if ui.button("+ Magnet").clicked() {
                let uri = state.torrent_magnet_input.trim().to_string();
                if !uri.is_empty() {
                    state.add_torrent_uri(uri);
                }
            }
        });
        ui.horizontal(|ui| {
            ui.label(state.i18n.torrents_jackett_prompt());
            ui.text_edit_singleline(&mut state.jackett_url_input);
            if ui.button(state.i18n.save_btn()).clicked() {
                state.save_jackett_url_override();
            }
        });

        if state.torrent_searching {
            ui.add_space(4.0);
            ui.add(
                egui::ProgressBar::new(state.torrent_search_ticks)
                    .text(state.i18n.torrents_searching_progress(&state.torrent_query)),
            );
        }
    });
    ui.add_space(6.0);

    let mut chosen_magnet: Option<String> = None;
    card_frame().show(ui, |ui| {
        egui::ScrollArea::vertical()
            .id_salt("torrent_results_scroll")
            .max_height(210.0)
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
                            ui.add(egui::Label::new(&res.title).truncate());
                            ui.label(format!("{}", res.seeders));
                            ui.label(fmt_mb(res.size_bytes));
                            if ui.button("Download").clicked() {
                                chosen_magnet = Some(res.magnet_uri.clone());
                            }
                            ui.end_row();
                        }
                    });
            });
    });
    if let Some(uri) = chosen_magnet {
        state.torrent_magnet_input = uri.clone();
        state.add_torrent_uri(uri);
    }

    ui.add_space(6.0);
    let mut play_snap = None;
    ui.columns(2, |cols| {
        card_frame().show(&mut cols[0], |ui| {
            ui.colored_label(ACCENT_CYAN, RichText::new(state.i18n.torrents_active_header()).strong());
            egui::ScrollArea::vertical()
                .id_salt("torrent_active_scroll")
                .show(ui, |ui| {
                    for (_u, name, started) in &state.pending_magnets {
                        ui.colored_label(
                            ACCENT_YELLOW,
                            format!("Resolving DHT ({}s): {name}", started.elapsed().as_secs()),
                        );
                    }
                    for snap in &state.active_torrents {
                        let frac = if snap.total_bytes > 0 {
                            (snap.progress_bytes as f32 / snap.total_bytes as f32).clamp(0.0, 1.0)
                        } else {
                            0.0
                        };
                        ui.horizontal(|ui| {
                            ui.add(egui::ProgressBar::new(frac).desired_width(120.0).show_percentage());
                            ui.add(egui::Label::new(format!(
                                "{} ({:.2} MB/s, p:{})",
                                snap.name, snap.download_speed_mbps, snap.peers_live
                            )).truncate());
                            if ui.button("▶").clicked() {
                                play_snap = Some(snap.clone());
                            }
                        });
                    }
                });
        });

        card_frame().show(&mut cols[1], |ui| {
            ui.colored_label(ACCENT_CYAN, RichText::new(state.i18n.torrents_log_header()).strong());
            egui::ScrollArea::vertical()
                .id_salt("torrent_logs_scroll")
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    for line in &state.torrent_search_logs {
                        ui.label(redact_url_secrets(line));
                    }
                });
        });
    });
    if let Some(snap) = play_snap {
        let _ = state.play_torrent_snapshot(&snap);
    }
}

fn draw_cache_settings(ui: &mut egui::Ui, state: &mut GuiState) {
    ui.colored_label(ACCENT_CYAN, RichText::new(state.i18n.cache_header()).size(16.0).strong());
    ui.add_space(6.0);

    card_frame().show(ui, |ui| {
        let stats = &state.cache_stats;
        let frac = if stats.limit_bytes > 0 {
            (stats.used_bytes as f32 / stats.limit_bytes as f32).clamp(0.0, 1.0)
        } else {
            0.0
        };
        ui.add(egui::ProgressBar::new(frac).text(format!(
            "LRU: {} / {} (Pinned: {} • {} entries)",
            fmt_mb(stats.used_bytes),
            fmt_mb(stats.limit_bytes),
            fmt_mb(stats.pinned_bytes),
            stats.pinned_count
        )));
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button("-512 MB Limit").clicked() {
                let _ = state.adjust_cache_limit_mb(-512);
            }
            if ui.button("+512 MB Limit").clicked() {
                let _ = state.adjust_cache_limit_mb(512);
            }
            if ui.button("Purge Unpinned Cache").clicked() {
                let _ = state.purge_cache();
            }
        });
    });

    ui.add_space(8.0);
    card_frame().show(ui, |ui| {
        ui.colored_label(ACCENT_CYAN, RichText::new("Paths & Storage").strong());
        ui.label(format!("DB: {}", state.paths.db_path.display()));
        ui.label(format!("Cache: {}", state.paths.cache_dir.display()));
        ui.label(format!("Offline Pins: {}", state.paths.offline_pins_dir.display()));
        ui.label(format!("Torrents: {}", state.paths.torrent_download_dir.display()));
    });

    ui.add_space(8.0);
    card_frame().show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(state.i18n.language_label());
            if ui.button("ES / EN / Auto").clicked() {
                let next = state.i18n.pref.cycle();
                state.set_language_preference(next);
            }
        });
    });
}

fn draw_friends(ui: &mut egui::Ui, state: &mut GuiState) {
    let ticket = state.local_p2p_ticket();
    let ticket_uri = ticket.to_uri();
    let web_url = format!("http://{}", ticket.endpoint);
    let lan_enabled = state.is_lan_share_enabled();

    card_frame().show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.set_min_width((ui.available_width() - 180.0).max(260.0));
                ui.colored_label(
                    ACCENT_CYAN,
                    RichText::new(state.i18n.friends_local_header()).size(15.0).strong(),
                );
                ui.colored_label(
                    ACCENT_YELLOW,
                    state.i18n.friends_web_url_label(&web_url),
                );
                ui.horizontal_wrapped(|ui| {
                    ui.colored_label(ACCENT_CYAN, RichText::new(&ticket_uri).monospace().size(11.5));
                    if ui.button("Copy Token").clicked() {
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

            draw_qr_matrix(ui, &ticket.render_unicode_qr(), 132.0);
        });
    });

    ui.add_space(6.0);
    let mut resync_target: Option<String> = None;
    ui.columns(2, |cols| {
        cols[0].colored_label(
            ACCENT_CYAN,
            RichText::new(format!(
                "{} ({})",
                state.i18n.friends_peers_header(),
                state.paired_peers.len()
            ))
            .strong(),
        );
        if state.paired_peers.is_empty() {
            cols[0].colored_label(TEXT_MUTED, state.i18n.friends_no_peers_hint());
        } else {
            for p in &state.paired_peers {
                cols[0].horizontal(|ui| {
                    ui.label(format!(
                        "• {} ({}) • {} trk / {} pl",
                        p.node_name, p.endpoint, p.track_count, p.playlist_count
                    ));
                    if ui.small_button("↻").clicked() {
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
        cols[0].colored_label(ACCENT_CYAN, RichText::new(state.i18n.friends_log_header()).strong());
        egui::ScrollArea::vertical()
            .id_salt("p2p_logs_scroll")
            .stick_to_bottom(true)
            .show(&mut cols[0], |ui| {
                for line in &state.p2p_logs {
                    ui.label(line);
                }
            });

        cols[1].colored_label(
            ACCENT_CYAN,
            RichText::new(format!(
                "{} ({})",
                state.i18n.friends_tracks_header(),
                state.peer_tracks.len()
            ))
            .strong(),
        );
        let remote_list = state.peer_tracks.clone();
        draw_track_table(&mut cols[1], state, "peer_tracks_grid", &remote_list, false);
    });

    if let Some(uri) = resync_target {
        state.spawn_peer_sync(uri);
    }
}

fn draw_qr_matrix(ui: &mut egui::Ui, lines: &[String], size_px: f32) {
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
    painter.rect_filled(rect, 6.0, Color32::WHITE);

    let cell_w = rect.width() / n_cols as f32;
    let cell_h = rect.height() / n_rows as f32;
    for (r, row) in rows.iter().enumerate() {
        for (c, &dark) in row.iter().enumerate() {
            if dark {
                let x = rect.min.x + c as f32 * cell_w;
                let y = rect.min.y + r as f32 * cell_h;
                let cell = egui::Rect::from_min_size(
                    egui::pos2(x, y),
                    egui::vec2(cell_w + 0.3, cell_h + 0.3),
                );
                painter.rect_filled(cell, 0.0, Color32::BLACK);
            }
        }
    }
}

fn trunc_cell(ui: &mut egui::Ui, width: f32, text: RichText) {
    ui.allocate_ui_with_layout(
        egui::vec2(width.max(40.0), 22.0),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.set_max_width(width.max(40.0));
            ui.add(egui::Label::new(text).truncate());
        },
    );
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
        egui::Frame::NONE
            .fill(Color32::from_rgb(35, 35, 56))
            .stroke(Stroke::new(1.0_f32, ACCENT_CYAN))
            .corner_radius(CornerRadius::same(6))
            .inner_margin(egui::Margin::symmetric(10, 5))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.colored_label(
                        ACCENT_CYAN,
                        RichText::new(state.i18n.marked_tracks_badge(state.marked_track_ids.len())).strong(),
                    );
                    if ui.button(state.i18n.batch_enqueue_btn()).clicked() {
                        let _ = state.enqueue_marked_tracks();
                    }
                    if ui.button("+ Playlist").clicked() {
                        state.open_batch_playlist_picker();
                    }
                    if ui.button(state.i18n.clear_selection()).clicked() {
                        state.marked_track_ids.clear();
                    }
                });
            });
        ui.add_space(4.0);
    }

    const W_CHK: f32 = 24.0;
    const W_PLAY: f32 = 28.0;
    const W_PIN: f32 = 26.0;
    const W_Q: f32 = 52.0;
    const W_PL: f32 = 38.0;
    const W_DUR: f32 = 46.0;
    const W_FMT: f32 = 64.0;
    const W_DEL: f32 = 26.0;
    const COL_GAP: f32 = 6.0;

    let avail_w = ui.available_width().max(420.0);
    let num_gaps = if show_remove { 9.0 } else { 8.0 };
    let fixed_w = W_CHK
        + W_PLAY
        + W_PIN
        + W_Q
        + W_PL
        + W_DUR
        + W_FMT
        + if show_remove { W_DEL } else { 0.0 };
    let chrome_w = num_gaps * COL_GAP + 22.0;
    let text_w = (avail_w - fixed_w - chrome_w).max(120.0);
    let title_w = text_w * 0.44;
    let artist_w = text_w * 0.30;
    let album_w = (text_w - title_w - artist_w).max(40.0);

    // Table header
    egui::Frame::NONE
        .fill(BG_SURFACE)
        .stroke(Stroke::new(1.0_f32, BORDER_SUBTLE))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(egui::Margin::symmetric(6, 5))
        .show(ui, |ui| {
            ui.set_width((avail_w - 14.0).max(380.0));
            ui.spacing_mut().item_spacing.x = COL_GAP;
            ui.horizontal(|ui| {
                ui.add_sized([W_CHK, 18.0], egui::Label::new(RichText::new("Sel").color(ACCENT_YELLOW).size(11.0).strong()));
                ui.add_sized([W_PLAY, 18.0], egui::Label::new(""));
                ui.add_sized([W_PIN, 18.0], egui::Label::new(RichText::new("★").color(ACCENT_YELLOW).strong()));
                ui.add_sized([W_Q, 18.0], egui::Label::new(RichText::new(state.i18n.enqueue_btn()).color(ACCENT_CYAN).size(11.0).strong()));
                ui.add_sized([W_PL, 18.0], egui::Label::new(RichText::new("+PL").color(TEXT_MUTED).size(11.0).strong()));
                trunc_cell(ui, title_w, RichText::new(state.i18n.col_title()).color(ACCENT_YELLOW).strong());
                trunc_cell(ui, artist_w, RichText::new(state.i18n.col_artist()).color(ACCENT_YELLOW).strong());
                trunc_cell(ui, album_w, RichText::new(state.i18n.col_album()).color(ACCENT_YELLOW).strong());
                ui.add_sized([W_DUR, 18.0], egui::Label::new(RichText::new(state.i18n.col_duration()).color(ACCENT_YELLOW).strong()));
                ui.add_sized([W_FMT, 18.0], egui::Label::new(RichText::new(state.i18n.col_format()).color(ACCENT_YELLOW).strong()));
                if show_remove {
                    ui.add_sized([W_DEL, 18.0], egui::Label::new(""));
                }
            });
        });

    let row_h = 28.0;
    egui::ScrollArea::vertical()
        .id_salt(id_salt)
        .auto_shrink([false; 2])
        .show_rows(ui, row_h, tracks.len(), |ui, row_range| {
            for idx in row_range {
                let tr = &tracks[idx];
                let is_cur = current_key.as_deref() == Some(tr.source.unique_key().as_str());
                let is_selected_row = id_salt != "pinned_grid"
                    && !tracks.is_empty()
                    && idx == state.selected_row_idx.min(tracks.len() - 1);
                let is_marked = state.marked_track_ids.contains(&tr.id);
                let bg = if is_cur {
                    BG_ROW_ACTIVE
                } else if is_selected_row {
                    Color32::from_rgb(33, 35, 52)
                } else if idx % 2 == 1 {
                    BG_ROW_ALT
                } else {
                    BG_BASE
                };

                egui::Frame::NONE
                    .fill(bg)
                    .corner_radius(CornerRadius::same(4))
                    .inner_margin(egui::Margin::symmetric(6, 2))
                    .show(ui, |ui| {
                        ui.set_width((avail_w - 14.0).max(380.0));
                        ui.set_height(24.0);
                        ui.spacing_mut().item_spacing.x = COL_GAP;
                        ui.horizontal(|ui| {
                            let mut checked = is_marked;
                            if ui.add_sized([W_CHK, 20.0], egui::Checkbox::without_text(&mut checked)).changed() {
                                if checked {
                                    state.marked_track_ids.insert(tr.id);
                                } else {
                                    state.marked_track_ids.remove(&tr.id);
                                }
                            }
                            if ui
                                .add_sized([W_PLAY, 20.0], egui::Button::new(if is_cur { "⏸" } else { "▶" }))
                                .clicked()
                            {
                                action = Some(TrackRowAction::Play(idx));
                            }
                            if ui
                                .add_sized([W_PIN, 20.0], egui::Button::new(if tr.pinned { "★" } else { "☆" }))
                                .clicked()
                            {
                                action = Some(TrackRowAction::TogglePin(tr.clone()));
                            }
                            if ui
                                .add_sized([W_Q, 20.0], egui::Button::new(RichText::new(state.i18n.enqueue_btn()).size(11.0)))
                                .clicked()
                            {
                                action = Some(TrackRowAction::Enqueue(tr.clone()));
                            }
                            if ui
                                .add_sized([W_PL, 20.0], egui::Button::new(RichText::new("+PL").size(11.0)))
                                .clicked()
                            {
                                action = Some(TrackRowAction::OpenPlaylistPicker(tr.clone()));
                            }

                            let title_col = if is_cur {
                                ACCENT_CYAN
                            } else if is_marked {
                                ACCENT_YELLOW
                            } else {
                                TEXT_MAIN
                            };
                            let title_resp = ui.allocate_ui_with_layout(
                                egui::vec2(title_w.max(40.0), 22.0),
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    ui.set_max_width(title_w.max(40.0));
                                    ui.add(
                                        egui::Label::new(RichText::new(&tr.title).color(title_col))
                                            .truncate()
                                            .sense(egui::Sense::click()),
                                    )
                                },
                            );
                            if title_resp.inner.clicked() {
                                state.selected_row_idx = idx;
                            }
                            if title_resp.inner.double_clicked() {
                                action = Some(TrackRowAction::Play(idx));
                            }

                            trunc_cell(ui, artist_w, RichText::new(&tr.artist).color(TEXT_MUTED));
                            trunc_cell(ui, album_w, RichText::new(&tr.album).color(TEXT_MUTED));
                            ui.add_sized(
                                [W_DUR, 18.0],
                                egui::Label::new(
                                    RichText::new(fmt_dur_opt(tr.duration_secs))
                                        .color(TEXT_MUTED)
                                        .size(11.0)
                                        .monospace(),
                                ),
                            );

                            let (fmt_label, fmt_col) = match &tr.source {
                                TrackSource::Peer { cached_path, .. } => {
                                    let ok = cached_path
                                        .as_deref()
                                        .map(|p| Path::new(p).exists())
                                        .unwrap_or(false);
                                    if ok {
                                        (format!("P2P {}✓", tr.format.to_uppercase()), ACCENT_GREEN)
                                    } else {
                                        (format!("P2P {}", tr.format.to_uppercase()), ACCENT_CYAN)
                                    }
                                }
                                _ => (tr.format.to_uppercase(), TEXT_MUTED),
                            };
                            ui.add_sized(
                                [W_FMT, 18.0],
                                egui::Label::new(RichText::new(fmt_label).color(fmt_col).size(11.0)).truncate(),
                            );
                            if show_remove
                                && ui
                                    .add_sized([W_DEL, 20.0], egui::Button::new("×"))
                                    .clicked()
                            {
                                action = Some(TrackRowAction::RemoveFromPlaylist(tr.clone()));
                            }
                        });
                    });
            }
        });

    if let Some(act) = action {
        match act {
            TrackRowAction::Play(idx) => {
                let _ = state.play_list_at(tracks, idx);
            }
            TrackRowAction::Enqueue(tr) => {
                let _ = state.enqueue_tracks(std::slice::from_ref(&tr));
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
            ui.colored_label(ACCENT_YELLOW, RichText::new(picker_summary).strong());
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

fn draw_help_modal(ctx: &egui::Context, state: &mut GuiState) {
    if !state.show_help_modal {
        return;
    }
    let mut open = true;
    let sections = state.i18n.help_modal_sections();

    egui::Window::new(state.i18n.help_modal_title())
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .fixed_size([760.0, 395.0])
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.columns(2, |cols| {
                for (i, (sec_title, rows)) in sections.iter().enumerate() {
                    let target_col = &mut cols[i % 2];
                    card_frame().fill(BG_CARD).show(target_col, |ui| {
                        ui.set_width(ui.available_width());
                        ui.colored_label(ACCENT_CYAN, RichText::new(*sec_title).size(13.5).strong());
                        ui.add_space(4.0);
                        for (key_txt, desc_txt) in rows {
                            ui.horizontal(|ui| {
                                ui.colored_label(
                                    ACCENT_YELLOW,
                                    RichText::new(format!("[{key_txt}]")).size(11.5).monospace().strong(),
                                );
                                ui.colored_label(TEXT_MAIN, RichText::new(*desc_txt).size(12.0));
                            });
                        }
                    });
                    target_col.add_space(6.0);
                }
            });
            ui.separator();
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(format!("{} (Esc)", state.i18n.cancel_btn())).clicked() {
                    state.show_help_modal = false;
                }
            });
        });

    if !open {
        state.show_help_modal = false;
    }
}
