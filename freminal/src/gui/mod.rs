// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use std::sync::{Arc, OnceLock};

use crate::gui::colors::internal_color_to_egui_with_alpha;
use anyhow::Result;
use arc_swap::ArcSwap;
use conv2::ConvUtil;
use crossbeam_channel::{Receiver, Sender};
use eframe::egui::{self, CentralPanel, Panel, Pos2, Vec2, ViewportCommand};
use freminal_common::buffer_states::window_manipulation::WindowManipulation;
use freminal_common::config::Config;
use freminal_common::pty_write::PtyWrite;
use freminal_terminal_emulator::io::{InputEvent, PlaybackCommand, PlaybackMode, WindowCommand};
use freminal_terminal_emulator::snapshot::TerminalSnapshot;
use settings::{SettingsAction, SettingsModal};
use terminal::FreminalTerminalWidget;
use view_state::ViewState;

pub mod atlas;
pub mod colors;
pub mod font_manager;
pub mod fonts;
pub mod mouse;
pub mod renderer;
pub mod settings;
pub mod shaping;
pub mod terminal;
pub mod view_state;

fn set_egui_options(ctx: &egui::Context, bg_opacity: f32) {
    ctx.global_style_mut(|style| {
        // window_fill stays fully opaque so menus, settings modal, and all
        // egui chrome are never affected by background_opacity.
        style.visuals.window_fill = internal_color_to_egui_with_alpha(
            freminal_common::colors::TerminalColor::DefaultBackground,
            false,
            &freminal_common::themes::CATPPUCCIN_MOCHA,
            1.0,
        );
        // panel_fill gets the opacity — it controls the CentralPanel
        // (terminal area) background, which is the only surface that
        // should be semi-transparent.
        style.visuals.panel_fill = internal_color_to_egui_with_alpha(
            freminal_common::colors::TerminalColor::DefaultBackground,
            false,
            &freminal_common::themes::CATPPUCCIN_MOCHA,
            bg_opacity,
        );
    });
    ctx.options_mut(|options| {
        options.zoom_with_keyboard = false;
    });
}

/// Update egui chrome colors (window/panel fill) to match a new theme.
fn update_egui_theme(
    ctx: &egui::Context,
    theme: &freminal_common::themes::ThemePalette,
    bg_opacity: f32,
) {
    ctx.global_style_mut(|style| {
        // window_fill: always opaque (menus, settings, chrome).
        style.visuals.window_fill = internal_color_to_egui_with_alpha(
            freminal_common::colors::TerminalColor::DefaultBackground,
            false,
            theme,
            1.0,
        );
        // panel_fill: respects background_opacity (terminal area only).
        style.visuals.panel_fill = internal_color_to_egui_with_alpha(
            freminal_common::colors::TerminalColor::DefaultBackground,
            false,
            theme,
            bg_opacity,
        );
    });
}

struct FreminalGui {
    /// The latest terminal snapshot published by the PTY consumer thread.
    /// Loaded lock-free via a single atomic pointer swap.
    arc_swap: Arc<ArcSwap<TerminalSnapshot>>,

    terminal_widget: FreminalTerminalWidget,
    view_state: ViewState,
    window_title_stack: Vec<String>,
    config: Config,

    /// Settings modal state (open/close, draft config, tabs).
    settings_modal: SettingsModal,

    /// Channel sender used to deliver input events (key, resize, focus) to the
    /// PTY consumer thread.
    input_tx: Sender<InputEvent>,

    /// Sender used to write raw bytes back to the PTY (for Report* responses).
    pty_write_tx: Sender<PtyWrite>,

    /// Receiver for window manipulation commands produced by the PTY thread.
    window_cmd_rx: Receiver<WindowCommand>,

    /// Receiver for clipboard text extraction responses from the PTY thread.
    clipboard_rx: Receiver<String>,

    /// Whether this instance is running in playback mode.
    is_playback: bool,

    /// The playback mode currently selected in the GUI dropdown.
    /// Only meaningful when `is_playback` is true.
    selected_playback_mode: Option<PlaybackMode>,
}

impl FreminalGui {
    #[allow(clippy::too_many_arguments)]
    fn new(
        cc: &eframe::CreationContext<'_>,
        arc_swap: Arc<ArcSwap<TerminalSnapshot>>,
        config: Config,
        config_path: Option<std::path::PathBuf>,
        input_tx: Sender<InputEvent>,
        pty_write_tx: Sender<PtyWrite>,
        window_cmd_rx: Receiver<WindowCommand>,
        clipboard_rx: Receiver<String>,
        is_playback: bool,
    ) -> Self {
        set_egui_options(&cc.egui_ctx, config.ui.background_opacity);

        Self {
            arc_swap,
            terminal_widget: FreminalTerminalWidget::new(&cc.egui_ctx, &config),
            view_state: ViewState::new(),
            window_title_stack: Vec::new(),
            config,
            settings_modal: SettingsModal::new(config_path),
            input_tx,
            pty_write_tx,
            window_cmd_rx,
            clipboard_rx,
            is_playback,
            selected_playback_mode: None,
        }
    }

    /// Show the top menu bar.
    ///
    /// Contains a "Terminal" menu with Settings and Quit entries, plus
    /// playback controls when running in playback mode.
    fn show_menu_bar(&mut self, ui: &mut egui::Ui, snap: &TerminalSnapshot) {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("Terminal", |ui| {
                if ui.button("Settings...").clicked() {
                    let families = self.terminal_widget.monospace_families();
                    self.settings_modal.open(&self.config, families);
                    self.settings_modal
                        .set_base_font_defs(self.terminal_widget.base_font_defs().clone());
                    ui.close();
                }

                ui.separator();

                if ui.button("Quit").clicked() {
                    ui.ctx().send_viewport_cmd(ViewportCommand::Close);
                }
            });

            // Playback controls: only shown when running in playback mode.
            if self.is_playback {
                self.show_playback_controls(ui, snap);
            }
        });
    }

    /// Render the playback toolbar controls (mode selector, play/pause, next, progress).
    fn show_playback_controls(&mut self, ui: &mut egui::Ui, snap: &TerminalSnapshot) {
        let info = snap.playback_info.as_ref();

        // Mode selector dropdown.
        ui.menu_button(self.playback_mode_label(), |ui| {
            let mut changed = false;

            if ui
                .selectable_label(
                    self.selected_playback_mode == Some(PlaybackMode::Instant),
                    "Instant",
                )
                .clicked()
            {
                self.selected_playback_mode = Some(PlaybackMode::Instant);
                changed = true;
                ui.close();
            }

            if ui
                .selectable_label(
                    self.selected_playback_mode == Some(PlaybackMode::RealTime),
                    "Real-Time",
                )
                .clicked()
            {
                self.selected_playback_mode = Some(PlaybackMode::RealTime);
                changed = true;
                ui.close();
            }

            if ui
                .selectable_label(
                    self.selected_playback_mode == Some(PlaybackMode::FrameStepping),
                    "Frame Stepping",
                )
                .clicked()
            {
                self.selected_playback_mode = Some(PlaybackMode::FrameStepping);
                changed = true;
                ui.close();
            }

            if changed && let Some(mode) = self.selected_playback_mode {
                self.send_playback_cmd(PlaybackCommand::SetMode(mode));
            }
        });

        ui.separator();

        // Play / Pause toggle button.
        let is_playing = info.is_some_and(|i| i.playing);
        let is_complete = info.is_some_and(|i| i.current_frame >= i.total_frames);
        let has_mode = self.selected_playback_mode.is_some();

        if is_playing {
            if ui.button("Pause").clicked() {
                self.send_playback_cmd(PlaybackCommand::Pause);
            }
        } else {
            let play_btn = ui.add_enabled(!is_complete && has_mode, egui::Button::new("Play"));
            if play_btn.clicked() {
                self.send_playback_cmd(PlaybackCommand::Play);
            }
        }

        // Next button: only active in frame-stepping mode.
        let is_frame_stepping = self.selected_playback_mode == Some(PlaybackMode::FrameStepping);
        let next_btn = ui.add_enabled(is_frame_stepping && !is_complete, egui::Button::new("Next"));
        if next_btn.clicked() {
            self.send_playback_cmd(PlaybackCommand::NextFrame);
        }

        ui.separator();

        // Frame counter label.
        if let Some(info) = info {
            ui.label(format!(
                "Frame {}/{}",
                info.current_frame, info.total_frames
            ));
        } else {
            ui.label("Frame 0/0");
        }
    }

    /// Human-readable label for the current playback mode selector button.
    const fn playback_mode_label(&self) -> &'static str {
        match self.selected_playback_mode {
            None => "Mode",
            Some(PlaybackMode::Instant) => "Instant",
            Some(PlaybackMode::RealTime) => "Real-Time",
            Some(PlaybackMode::FrameStepping) => "Frame Stepping",
        }
    }

    /// Send a playback command to the consumer thread via the input channel.
    fn send_playback_cmd(&self, cmd: PlaybackCommand) {
        if let Err(e) = self.input_tx.send(InputEvent::PlaybackControl(cmd)) {
            error!("Failed to send playback command: {e}");
        }
    }
}

/// Send a raw PTY response string via the write channel.
///
/// Used by `handle_window_manipulation` to respond to Report* queries without
/// going through the emulator.
fn send_pty_response(pty_write_tx: &Sender<PtyWrite>, response: &str) {
    if let Err(e) = pty_write_tx.send(PtyWrite::Write(response.as_bytes().to_vec())) {
        error!("Failed to send PTY response: {e}");
    }
}

#[allow(clippy::too_many_lines, clippy::too_many_arguments)]
fn handle_window_manipulation(
    ui: &egui::Ui,
    window_cmd_rx: &Receiver<WindowCommand>,
    pty_write_tx: &Sender<PtyWrite>,
    font_width: usize,
    font_height: usize,
    window_width: egui::Rect,
    title_stack: &mut Vec<String>,
) {
    // Drain all pending WindowCommands for this frame.
    while let Ok(wc) = window_cmd_rx.try_recv() {
        let window_event = match wc {
            WindowCommand::Viewport(cmd) | WindowCommand::Report(cmd) => cmd,
        };

        match window_event {
            WindowManipulation::DeIconifyWindow => {
                ui.ctx()
                    .send_viewport_cmd(ViewportCommand::Minimized(false));
            }
            WindowManipulation::MinimizeWindow => {
                ui.ctx().send_viewport_cmd(ViewportCommand::Minimized(true));
            }
            WindowManipulation::MoveWindow(x, y) => {
                let x = x.approx_as::<f32>().unwrap_or_default();
                let y = y.approx_as::<f32>().unwrap_or_default();

                ui.ctx()
                    .send_viewport_cmd(ViewportCommand::OuterPosition(Pos2::new(x, y)));
            }
            WindowManipulation::ResizeWindow(width, height) => {
                let width = width.approx_as::<f32>().unwrap_or_default();
                let height = height.approx_as::<f32>().unwrap_or_default();

                ui.ctx()
                    .send_viewport_cmd(ViewportCommand::InnerSize(Vec2::new(width, height)));
            }
            WindowManipulation::MaximizeWindow => {
                ui.ctx().send_viewport_cmd(ViewportCommand::Maximized(true));
            }
            WindowManipulation::RestoreNonMaximizedWindow => {
                ui.ctx()
                    .send_viewport_cmd(ViewportCommand::Maximized(false));
            }
            WindowManipulation::ResizeWindowToLinesAndColumns(input_height, input_width) => {
                let available_height = ui.available_height();
                let available_width = ui.available_width();
                let width_difference = window_width.width() - available_width;
                let height_difference = window_width.height() - available_height;
                let width = input_width * font_width;
                let height = input_height * font_height;

                let width = width.approx_as::<f32>().unwrap_or_default() + width_difference;
                let height = height.approx_as::<f32>().unwrap_or_default() + height_difference;

                ui.ctx()
                    .send_viewport_cmd(ViewportCommand::InnerSize(Vec2::new(width, height)));
            }
            WindowManipulation::NotFullScreen => {
                ui.ctx()
                    .send_viewport_cmd(ViewportCommand::Fullscreen(false));
            }
            WindowManipulation::FullScreen => {
                ui.ctx()
                    .send_viewport_cmd(ViewportCommand::Fullscreen(true));
            }
            WindowManipulation::ToggleFullScreen => {
                let current_status = ui.ctx().input(|i| i.viewport().fullscreen.unwrap_or(false));
                ui.ctx()
                    .send_viewport_cmd(ViewportCommand::Fullscreen(!current_status));
            }
            WindowManipulation::ReportWindowState => {
                let minimized = ui.ctx().input(|i| i.viewport().minimized.unwrap_or(false));
                let response = if minimized { "\x1b[2t" } else { "\x1b[1t" };
                send_pty_response(pty_write_tx, response);
            }
            WindowManipulation::ReportWindowPositionWholeWindow => {
                let position = ui
                    .ctx()
                    .input(|i| {
                        i.raw.viewport().outer_rect.unwrap_or_else(|| {
                            error!("Failed to get viewport position. Using 0 as default");
                            egui::Rect::from_min_size(Pos2::new(0.0, 0.0), Vec2::new(0.0, 0.0))
                        })
                    })
                    .min;

                let pos_x = position.x.approx_as::<usize>().unwrap_or_else(|e| {
                    error!("Failed to convert position x to usize: {e}. Using 0 as default");
                    0
                });
                let pos_y = position.y.approx_as::<usize>().unwrap_or_else(|e| {
                    error!("Failed to convert position y to usize: {e}. Using 0 as default");
                    0
                });

                send_pty_response(pty_write_tx, &format!("\x1b[3;{pos_x};{pos_y}t"));
            }
            WindowManipulation::ReportWindowPositionTextArea => {
                let position = ui
                    .ctx()
                    .input(|i| {
                        i.raw.viewport().outer_rect.unwrap_or_else(|| {
                            error!("Failed to get viewport position. Using 0 as default");
                            egui::Rect::from_min_size(Pos2::new(0.0, 0.0), Vec2::new(0.0, 0.0))
                        })
                    })
                    .min;

                let available_height = ui.available_height();
                let available_width = ui.available_width();
                let width_difference = window_width.width() - available_width;
                let height_difference = window_width.height() - available_height;
                let pos_x = (position.y + height_difference)
                    .approx_as::<usize>()
                    .unwrap_or_else(|e| {
                        error!("Failed to convert position x to usize: {e}. Using 0 as default");
                        0
                    });
                let pos_y = (position.y + width_difference)
                    .approx_as::<usize>()
                    .unwrap_or_else(|e| {
                        error!("Failed to convert position y to usize: {e}. Using 0 as default");
                        0
                    });

                send_pty_response(pty_write_tx, &format!("\x1b[3;{pos_x};{pos_y}t"));
            }
            WindowManipulation::ReportWindowSizeInPixels => {
                let rect = ui.ctx().input(|i| {
                    i.raw.viewport().outer_rect.unwrap_or_else(|| {
                        error!("Failed to get viewport position. Using 0 as default");
                        egui::Rect::from_min_size(Pos2::new(0.0, 0.0), Vec2::new(0.0, 0.0))
                    })
                });

                let width = (rect.max.x - rect.min.x)
                    .approx_as::<usize>()
                    .unwrap_or_else(|e| {
                        error!("Failed to convert width to usize: {e}. Using 0 as default");
                        0
                    });
                let height = (rect.max.y - rect.min.y)
                    .approx_as::<usize>()
                    .unwrap_or_else(|e| {
                        error!("Failed to convert height to usize: {e}. Using 0 as default");
                        0
                    });

                send_pty_response(pty_write_tx, &format!("\x1b[4;{height};{width}t"));
            }
            WindowManipulation::ReportWindowTextAreaSizeInPixels => {
                let size = ui.ctx().content_rect().max;
                let width = size.x.approx_as::<usize>().unwrap_or_else(|e| {
                    error!("Failed to convert width to usize: {e}. Using 0 as default");
                    0
                });
                let height = size.y.approx_as::<usize>().unwrap_or_else(|e| {
                    error!("Failed to convert height to usize: {e}. Using 0 as default");
                    0
                });

                send_pty_response(pty_write_tx, &format!("\x1b[4;{height};{width}t"));
            }
            WindowManipulation::ReportRootWindowSizeInPixels => {
                let rect = ui.ctx().input(|i| {
                    i.raw.viewport().outer_rect.unwrap_or_else(|| {
                        error!("Failed to get viewport position. Using 0 as default");
                        egui::Rect::from_min_size(Pos2::new(0.0, 0.0), Vec2::new(0.0, 0.0))
                    })
                });

                let width = (rect.max.x - rect.min.x)
                    .approx_as::<usize>()
                    .unwrap_or_else(|e| {
                        error!("Failed to convert width to usize: {e}. Using 0 as default");
                        0
                    });
                let height = (rect.max.y - rect.min.y)
                    .approx_as::<usize>()
                    .unwrap_or_else(|e| {
                        error!("Failed to convert height to usize: {e}. Using 0 as default");
                        0
                    });

                send_pty_response(pty_write_tx, &format!("\x1b[5;{height};{width}t"));
            }
            // ReportCharacterSizeInPixels, ReportTerminalSizeInCharacters, and
            // ReportRootWindowSizeInCharacters are handled synchronously by the
            // PTY thread (TerminalHandler::handle_window_manipulation) so that
            // responses arrive in the same batch as DA1.  They never reach here.
            WindowManipulation::ReportCharacterSizeInPixels
            | WindowManipulation::ReportTerminalSizeInCharacters
            | WindowManipulation::ReportRootWindowSizeInCharacters => {}
            WindowManipulation::ReportIconLabel => {
                let title = ui.ctx().input(|r| r.raw.viewport().title.clone());
                let title = title.unwrap_or_else(|| {
                    error!("Failed to get viewport title. Using Freminal");
                    "Freminal".to_string()
                });
                send_pty_response(pty_write_tx, &format!("\x1b]L{title}\x1b\\"));
            }
            WindowManipulation::ReportTitle => {
                let title = ui.ctx().input(|r| r.raw.viewport().title.clone());
                let title = title.unwrap_or_else(|| {
                    error!("Failed to get viewport title. Using Freminal");
                    "Freminal".to_string()
                });
                send_pty_response(pty_write_tx, &format!("\x1b]l{title}\x1b\\"));
            }
            WindowManipulation::SetTitleBarText(title) => {
                ui.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::Title(title));
            }
            WindowManipulation::SaveWindowTitleToStack => {
                let title = ui.ctx().input(|r| r.raw.viewport().title.clone());
                let title = title.unwrap_or_else(|| {
                    error!("Failed to get viewport title. Using Freminal");
                    "Freminal".to_string()
                });
                title_stack.push(title);
            }
            WindowManipulation::RestoreWindowTitleFromStack => {
                if let Some(title) = title_stack.pop() {
                    ui.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::Title(title));
                } else {
                    ui.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::Title("Freminal".to_string()));
                }
            }
            // These are ignored. eGui doesn't give us a stacking order thing (that I can tell).
            // Refresh window is already happening because we ended up here.
            WindowManipulation::RefreshWindow
            | WindowManipulation::LowerWindowToBottomOfStackingOrder
            | WindowManipulation::RaiseWindowToTopOfStackingOrder => (),

            // OSC 52 clipboard set: copy decoded text to the system clipboard.
            WindowManipulation::SetClipboard(_sel, content) => {
                ui.ctx().copy_text(content);
            }

            // OSC 52 clipboard query: we cannot read the clipboard through
            // egui's public API, so respond with an empty payload.  This is
            // the safe/secure default adopted by many terminals.
            WindowManipulation::QueryClipboard(sel) => {
                tracing::debug!("OSC 52 query for selection '{sel}' — responding empty");
                send_pty_response(pty_write_tx, &format!("\x1b]52;{sel};\x1b\\"));
            }
        }
    }
}

impl eframe::App for FreminalGui {
    /// Override the GL framebuffer clear color.
    ///
    /// When `background_opacity < 1.0` the viewport was created with
    /// `transparent = true`, so the compositor can show the desktop through.
    /// For that to work the clear color must have alpha = 0; otherwise the
    /// opaque clear overwrites the transparent framebuffer before egui
    /// paints anything.
    ///
    /// When opacity is 1.0 the clear color matches `panel_fill` (fully
    /// opaque) — there is no visible difference from the default.
    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        if self.config.ui.background_opacity < 1.0 {
            [0.0, 0.0, 0.0, 0.0]
        } else {
            // Fully opaque: use the terminal background color.
            visuals.panel_fill.to_normalized_gamma_f32()
        }
    }

    #[allow(clippy::too_many_lines)]
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        debug!("Starting new frame");
        let now = std::time::Instant::now();

        // Load the latest snapshot from the PTY thread — no lock, single atomic load.
        let snap = self.arc_swap.load();

        // Sync the GUI's scroll offset from the snapshot.  When new PTY output
        // arrives the PTY thread resets its offset to 0, so the snapshot will
        // carry scroll_offset = 0 even if the GUI previously sent a non-zero
        // value.  Adopting the snapshot's value keeps ViewState in sync.
        if self.view_state.scroll_offset != snap.scroll_offset {
            self.view_state.scroll_offset = snap.scroll_offset;
        }

        // Menu bar at the top of the window.
        if !self.config.ui.hide_menu_bar {
            Panel::top("menu_bar").show_inside(ui, |ui| {
                self.show_menu_bar(ui, &snap);
            });
        }

        let _panel_response = CentralPanel::default().show_inside(ui, |ui| {
            // Synchronise font metrics with the current display scale *before*
            // reading `cell_size()`.  Without this, the first frame after a DPI
            // change would use stale pixel metrics for the resize calculation.
            let ppp = ui.ctx().pixels_per_point();
            self.terminal_widget.sync_pixels_per_point(ppp);

            // Compute char size once and reuse for both PTY sizing and widget layout.
            // `cell_size()` returns integer pixel dimensions (physical) from swash
            // font metrics.  egui's coordinate system uses logical points, so we
            // convert with `pixels_per_point` when doing layout math.
            let (cell_w_u, cell_height_u) = self.terminal_widget.cell_size();
            #[allow(clippy::cast_possible_truncation)]
            let font_width = cell_w_u as usize;
            #[allow(clippy::cast_possible_truncation)]
            let font_height = cell_height_u as usize;
            #[allow(clippy::cast_precision_loss)]
            let logical_char_w = cell_w_u as f32 / ppp;
            #[allow(clippy::cast_precision_loss)]
            let logical_char_h = cell_height_u as f32 / ppp;

            let available = ui.available_size();
            let width_chars = (available.x / logical_char_w)
                .floor()
                .approx_as::<usize>()
                .unwrap_or_else(|e| {
                    error!("Failed to calculate width chars: {e}");
                    10
                });
            let height_chars = ((available.y / logical_char_h).floor())
                .approx_as::<usize>()
                .unwrap_or_else(|e| {
                    error!("Failed to calculate height chars: {e}");
                    10
                })
                .max(1);

            // Debounced resize: only send an InputEvent::Resize when the
            // character-cell dimensions actually change.
            let new_size = (width_chars, height_chars);
            if new_size != self.view_state.last_sent_size {
                if let Err(e) = self.input_tx.send(InputEvent::Resize(
                    width_chars,
                    height_chars,
                    font_width,
                    font_height,
                )) {
                    error!("Failed to send resize event: {e}");
                } else {
                    self.view_state.last_sent_size = new_size;
                }
            }

            let window_width = ui.input(|i: &egui::InputState| i.content_rect());

            handle_window_manipulation(
                ui,
                &self.window_cmd_rx,
                &self.pty_write_tx,
                font_width,
                font_height,
                window_width,
                &mut self.window_title_stack,
            );

            // Update background color based on whether the terminal is in
            // normal (non-inverted) display mode.
            let bg_opacity = self.config.ui.background_opacity;
            if snap.is_normal_display {
                ui.ctx().global_style_mut(|style| {
                    // window_fill: always opaque (menus, settings, chrome).
                    style.visuals.window_fill = internal_color_to_egui_with_alpha(
                        freminal_common::colors::TerminalColor::DefaultBackground,
                        false,
                        snap.theme,
                        1.0,
                    );
                    // panel_fill: respects background_opacity (terminal area only).
                    style.visuals.panel_fill = internal_color_to_egui_with_alpha(
                        freminal_common::colors::TerminalColor::DefaultBackground,
                        false,
                        snap.theme,
                        bg_opacity,
                    );
                });
            } else {
                ui.ctx().global_style_mut(|style| {
                    style.visuals.window_fill =
                        egui::Color32::from_rgba_unmultiplied(255, 255, 255, 255);
                    style.visuals.panel_fill =
                        egui::Color32::from_rgba_unmultiplied(255, 255, 255, 255);
                });
            }

            self.terminal_widget.show(
                ui,
                &snap,
                &mut self.view_state,
                &self.input_tx,
                &self.clipboard_rx,
                self.settings_modal.is_open,
            );

            // Only schedule a wakeup when there is work to do:
            //  - new content arrived (`content_changed`)
            //  - cursor is blinking (needs toggling every ~500 ms)
            //  - text is blinking (needs toggling every ~167 ms)
            //  - first frame (buffers still empty — need at least one full draw)
            //
            // A steady cursor with no new content does not need a periodic
            // repaint; egui will wake on the next user input event instead.
            let cursor_is_blinking = matches!(
                snap.cursor_visual_style,
                freminal_common::cursor::CursorVisualStyle::BlockCursorBlink
                    | freminal_common::cursor::CursorVisualStyle::UnderlineCursorBlink
                    | freminal_common::cursor::CursorVisualStyle::VerticalLineCursorBlink,
            );

            // Advance the text blink cycle when blinking text is present.
            if snap.has_blinking_text {
                self.view_state.tick_text_blink();
            }

            if snap.content_changed || cursor_is_blinking || snap.has_blinking_text {
                // Use a 16 ms deadline (~60 fps) for content changes; use the
                // blink half-period (~500 ms) when only the cursor needs to
                // toggle; use the fast-blink tick (~167 ms) when text is
                // blinking.  Pick the shortest applicable interval.
                let delay = if snap.content_changed {
                    std::time::Duration::from_millis(16)
                } else if snap.has_blinking_text {
                    view_state::TEXT_BLINK_TICK_DURATION
                } else {
                    std::time::Duration::from_millis(500)
                };
                ui.ctx().request_repaint_after(delay);
            }
        });

        // Show the settings modal (if open) above everything else.
        let modal_was_open = self.settings_modal.is_open;
        let settings_action = self.settings_modal.show(ui.ctx());

        // After show() processes the dropdown change, load the new font's
        // bytes and register them with egui so the preview renders in the
        // actual selected font on the next frame.
        if self.settings_modal.is_open
            && let Some(family) = self.settings_modal.needed_preview_family()
        {
            let bytes = self.terminal_widget.load_font_bytes(&family);
            let base = self.terminal_widget.base_font_defs();
            self.settings_modal
                .register_preview_font(ui.ctx(), &family, bytes, base);
        }

        // If the modal just closed (any reason), restore the original egui
        // font set to remove the preview font registration.
        if modal_was_open && !self.settings_modal.is_open {
            self.settings_modal.restore_base_fonts(ui.ctx());
        }

        match settings_action {
            SettingsAction::Applied => {
                let new_cfg = self.settings_modal.applied_config().clone();

                // If the theme slug changed, look it up and notify the PTY thread
                // so the next snapshot carries the new palette.
                if new_cfg.theme.name != self.config.theme.name
                    && let Some(theme) = freminal_common::themes::by_slug(&new_cfg.theme.name)
                {
                    if let Err(e) = self.input_tx.send(InputEvent::ThemeChange(theme)) {
                        error!("Failed to send ThemeChange to PTY thread: {e}");
                    }
                    update_egui_theme(ui.ctx(), theme, self.config.ui.background_opacity);
                    // Force a full vertex rebuild on the next frame so
                    // foreground/background colors are re-resolved against
                    // the new palette.  Without this, the preview's rebuild
                    // may be the last one, and the Apply-frame snapshot
                    // (with content_changed=false) would skip the rebuild.
                    self.terminal_widget.invalidate_theme_cache();
                }

                self.terminal_widget
                    .apply_config_changes(ui.ctx(), &self.config, &new_cfg);
                self.config = new_cfg;
            }
            SettingsAction::PreviewTheme(ref slug) | SettingsAction::RevertTheme(ref slug) => {
                if let Some(theme) = freminal_common::themes::by_slug(slug) {
                    if let Err(e) = self.input_tx.send(InputEvent::ThemeChange(theme)) {
                        error!("Failed to send theme preview/revert to PTY thread: {e}");
                    }
                    update_egui_theme(ui.ctx(), theme, self.config.ui.background_opacity);
                }
            }
            SettingsAction::None => {}
        }

        let elapsed = now.elapsed();
        let frame_time = if elapsed.as_millis() > 0 {
            format!("Frame time={}ms", elapsed.as_millis())
        } else {
            format!("Frame time={}μs", elapsed.as_micros())
        };

        debug!("{}", frame_time);
    }

    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        // Override egui's predicted frame time to zero.
        //
        // egui's `request_repaint_after(delay)` subtracts `predicted_dt`
        // (~16.7 ms at the default 1/60) from the requested delay to avoid
        // "overshooting" into the next frame.  With vsync disabled (see the
        // `native_options.vsync = false` below), this subtraction collapses
        // any delay ≤ 16.7 ms to zero — turning every repaint request into
        // an immediate repaint and driving the frame rate to hundreds of FPS
        // during active PTY output.
        //
        // Setting `predicted_dt = 0` disables the subtraction, so our delays
        // are honoured exactly:
        //   - 8 ms  (PTY thread after each batch)  → ~120 FPS cap
        //   - 16 ms (GUI on content_changed)        → ~60 FPS cap
        //   - 500 ms (cursor blink)                 → ~2 FPS
        //   - no request (true idle, steady cursor)  → 0 FPS
        raw_input.predicted_dt = 0.0;
    }
}

/// Run the GUI
///
/// # Errors
/// Will return an error if the GUI fails to run
#[allow(clippy::too_many_arguments)]
pub fn run(
    arc_swap: Arc<ArcSwap<TerminalSnapshot>>,
    config: Config,
    config_path: Option<std::path::PathBuf>,
    input_tx: Sender<InputEvent>,
    pty_write_tx: Sender<PtyWrite>,
    window_cmd_rx: Receiver<WindowCommand>,
    clipboard_rx: Receiver<String>,
    egui_ctx_lock: Arc<OnceLock<egui::Context>>,
    is_playback: bool,
) -> Result<()> {
    let icon = match eframe::icon_data::from_png_bytes(include_bytes!("../../../assets/icon.png")) {
        Ok(icon) => icon,
        Err(e) => {
            return Err(anyhow::anyhow!(
                "Failed to load window icon from bytes: {e}"
            ));
        }
    };

    let mut native_options = eframe::NativeOptions::default();
    native_options.viewport.icon = Some(Arc::new(icon));

    // Always request a framebuffer with an alpha channel so that
    // background_opacity can be changed at runtime without a restart.
    // When opacity is 1.0 the clear_color() override returns a fully
    // opaque color, so there is no visual difference.  On Wayland and
    // macOS this works out of the box; on X11 it requires a running
    // compositor (e.g. picom).
    native_options.viewport.transparent = Some(true);

    // Disable client-side vsync so that eglSwapBuffers is non-blocking.
    //
    // eframe 0.34 does not call winit's pre_present_notify() before
    // swap_buffers(), which means winit's Wayland frame-callback pacing
    // is never activated.  With EGL_SWAP_INTERVAL=1 (the vsync=true
    // default), eglSwapBuffers blocks until the compositor signals a
    // frame — but on a hidden workspace the compositor never signals,
    // so the call blocks indefinitely.  While blocked, the Wayland
    // event loop cannot dispatch protocol events, so xdg_wm_base pings
    // go unanswered and the compositor declares the app hung.
    //
    // With vsync=false the swap returns immediately.  Wayland compositors
    // do their own compositing pass at the display refresh rate, so
    // client-side tearing is not visible.  The `raw_input_hook` override
    // of `predicted_dt = 0.0` (see above) ensures our repaint-request
    // delays are honoured exactly, so the effective frame rate is capped
    // by the repaint intervals (8 ms / 16 ms / 500 ms) rather than
    // spinning at hundreds of FPS.
    native_options.vsync = false;

    match eframe::run_native(
        "Freminal",
        native_options,
        Box::new(move |cc| {
            // Publish the egui::Context so the PTY consumer thread can
            // request repaints after storing new snapshots.
            let _already_set = egui_ctx_lock.set(cc.egui_ctx.clone());

            Ok(Box::new(FreminalGui::new(
                cc,
                arc_swap,
                config,
                config_path,
                input_tx,
                pty_write_tx,
                window_cmd_rx,
                clipboard_rx,
                is_playback,
            )))
        }),
    ) {
        Ok(()) => Ok(()),
        Err(e) => Err(anyhow::anyhow!(e.to_string())),
    }
}
