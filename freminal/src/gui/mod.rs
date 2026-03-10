// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use std::sync::Arc;

use crate::gui::colors::internal_color_to_egui;
use anyhow::Result;
use arc_swap::ArcSwap;
use conv2::ConvUtil;
use crossbeam_channel::{Receiver, Sender};
use eframe::egui::{self, CentralPanel, Pos2, Vec2, ViewportCommand};
use fonts::get_char_size;
use freminal_common::buffer_states::window_manipulation::WindowManipulation;
use freminal_common::config::Config;
use freminal_common::pty_write::PtyWrite;
use freminal_terminal_emulator::io::{InputEvent, WindowCommand};
use freminal_terminal_emulator::snapshot::TerminalSnapshot;
use terminal::FreminalTerminalWidget;
use view_state::ViewState;

pub mod colors;
pub mod fonts;
pub mod mouse;
pub mod terminal;
pub mod view_state;

fn set_egui_options(ctx: &egui::Context) {
    ctx.style_mut(|style| {
        style.visuals.window_fill = internal_color_to_egui(
            freminal_common::colors::TerminalColor::DefaultBackground,
            false,
        );
        style.visuals.panel_fill = internal_color_to_egui(
            freminal_common::colors::TerminalColor::DefaultBackground,
            false,
        );
    });
    ctx.options_mut(|options| {
        options.zoom_with_keyboard = false;
    });
}

struct FreminalGui {
    /// The latest terminal snapshot published by the PTY consumer thread.
    /// Loaded lock-free via a single atomic pointer swap.
    arc_swap: Arc<ArcSwap<TerminalSnapshot>>,

    terminal_widget: FreminalTerminalWidget,
    view_state: ViewState,
    window_title_stack: Vec<String>,
    _config: Config,

    /// Channel sender used to deliver input events (key, resize, focus) to the
    /// PTY consumer thread.
    input_tx: Sender<InputEvent>,

    /// Sender used to write raw bytes back to the PTY (for Report* responses).
    pty_write_tx: Sender<PtyWrite>,

    /// Receiver for window manipulation commands produced by the PTY thread.
    window_cmd_rx: Receiver<WindowCommand>,
}

impl FreminalGui {
    fn new(
        cc: &eframe::CreationContext<'_>,
        arc_swap: Arc<ArcSwap<TerminalSnapshot>>,
        config: Config,
        input_tx: Sender<InputEvent>,
        pty_write_tx: Sender<PtyWrite>,
        window_cmd_rx: Receiver<WindowCommand>,
    ) -> Self {
        set_egui_options(&cc.egui_ctx);

        Self {
            arc_swap,
            terminal_widget: FreminalTerminalWidget::new(&cc.egui_ctx, &config),
            view_state: ViewState::new(),
            window_title_stack: Vec::new(),
            _config: config,
            input_tx,
            pty_write_tx,
            window_cmd_rx,
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
    snap: &TerminalSnapshot,
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
            WindowManipulation::ReportCharacterSizeInPixels => {
                send_pty_response(pty_write_tx, &format!("\x1b[6;{font_height};{font_width}t"));
            }
            WindowManipulation::ReportTerminalSizeInCharacters => {
                let (width, height) = (snap.term_width, snap.term_height);
                send_pty_response(pty_write_tx, &format!("\x1b[8;{height};{width}t"));
            }
            WindowManipulation::ReportRootWindowSizeInCharacters => {
                let (width, height) = (snap.term_width, snap.term_height);
                send_pty_response(pty_write_tx, &format!("\x1b[9;{height};{width}t"));
            }
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
        }
    }
}

impl eframe::App for FreminalGui {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
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

        let panel_response = CentralPanel::default().show(ctx, |ui| {
            // Compute char size once and reuse for both PTY sizing and widget layout.
            let (char_w, char_h) =
                get_char_size(ui.ctx(), &self.terminal_widget.get_terminal_fonts());

            let font_width = char_w.round().approx_as::<usize>().unwrap_or_else(|e| {
                error!("Failed to convert font width to usize: {e}. Using 12 as default");
                12
            });

            let font_height = char_h.round().approx_as::<usize>().unwrap_or_else(|e| {
                error!("Failed to convert font height to usize: {e}. Using 12 as default");
                12
            });

            let available = ui.available_size();
            let width_chars = (available.x / char_w)
                .floor()
                .approx_as::<usize>()
                .unwrap_or_else(|e| {
                    error!("Failed to calculate width chars: {e}");
                    10
                });
            let height_chars = ((available.y / char_h).floor())
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

            let window_width = ctx.input(|i: &egui::InputState| i.content_rect());

            handle_window_manipulation(
                ui,
                &self.window_cmd_rx,
                &self.pty_write_tx,
                font_width,
                font_height,
                window_width,
                &mut self.window_title_stack,
                &snap,
            );

            // Update background color based on whether the terminal is in
            // normal (non-inverted) display mode.
            if snap.is_normal_display {
                ui.ctx().style_mut(|style| {
                    style.visuals.window_fill = internal_color_to_egui(
                        freminal_common::colors::TerminalColor::DefaultBackground,
                        false,
                    );
                    style.visuals.panel_fill = internal_color_to_egui(
                        freminal_common::colors::TerminalColor::DefaultBackground,
                        false,
                    );
                });
            } else {
                ui.ctx().style_mut(|style| {
                    style.visuals.window_fill = egui::Color32::WHITE;
                    style.visuals.panel_fill = egui::Color32::WHITE;
                });
            }

            self.terminal_widget.show(
                ui,
                &snap,
                &mut self.view_state,
                &self.input_tx,
                &self.pty_write_tx,
            );

            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(16));
        });

        panel_response.response.context_menu(|ui| {
            self.terminal_widget.show_options(ui);
        });

        let elapsed = now.elapsed();
        let frame_time = if elapsed.as_millis() > 0 {
            format!("Frame time={}ms", elapsed.as_millis())
        } else {
            format!("Frame time={}μs", elapsed.as_micros())
        };

        debug!("{}", frame_time);
    }
}

/// Run the GUI
///
/// # Errors
/// Will return an error if the GUI fails to run
pub fn run(
    arc_swap: Arc<ArcSwap<TerminalSnapshot>>,
    config: Config,
    input_tx: Sender<InputEvent>,
    pty_write_tx: Sender<PtyWrite>,
    window_cmd_rx: Receiver<WindowCommand>,
) -> Result<()> {
    let native_options = eframe::NativeOptions::default();

    match eframe::run_native(
        "Freminal",
        native_options,
        Box::new(move |cc| {
            Ok(Box::new(FreminalGui::new(
                cc,
                arc_swap,
                config,
                input_tx,
                pty_write_tx,
                window_cmd_rx,
            )))
        }),
    ) {
        Ok(()) => Ok(()),
        Err(e) => Err(anyhow::anyhow!(e.to_string())),
    }
}
