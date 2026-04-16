// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use egui;
#[cfg(feature = "playback")]
use freminal_terminal_emulator::io::{PlaybackCommand, PlaybackMode};
use freminal_terminal_emulator::snapshot::TerminalSnapshot;

use super::TabBarAction;
use super::tabs::Tab;
use super::window::PerWindowState;

impl super::FreminalGui {
    /// Show the top menu bar.
    ///
    /// Contains a "Freminal" menu with Settings and Quit entries, a "Tab"
    /// menu with tab management actions, and playback controls when
    /// running in playback mode.
    ///
    /// Returns `(action, any_menu_open)` — the second element is `true`
    /// when any dropdown menu is currently expanded, so the caller can
    /// suppress terminal input and prevent the dismiss-click from leaking
    /// through to the PTY.
    #[cfg_attr(not(feature = "playback"), allow(unused_variables))]
    pub(super) fn show_menu_bar(
        &mut self,
        ui: &mut egui::Ui,
        snap: &TerminalSnapshot,
        win: &mut PerWindowState,
    ) -> (TabBarAction, bool) {
        let mut menu_action = TabBarAction::None;
        let mut any_menu_open = false;
        egui::MenuBar::new().ui(ui, |ui| {
            let freminal_resp = ui.menu_button("Freminal", |ui| {
                if ui.button("Settings...").clicked() {
                    let families = win.terminal_widget.monospace_families();
                    self.settings_modal
                        .open(&self.config, families, win.os_dark_mode);
                    self.settings_modal
                        .set_base_font_defs(win.terminal_widget.base_font_defs().clone());
                    ui.close();
                }

                ui.separator();

                if ui.button("Quit").clicked() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
            if freminal_resp.inner.is_some() {
                any_menu_open = true;
            }

            let tab_resp = ui.menu_button("Tab", |ui| {
                if ui.button("New Tab").clicked() {
                    menu_action = TabBarAction::NewTab;
                    ui.close();
                }

                let active = win.tabs.active_index();
                let can_close = win.tabs.tab_count() > 1;
                if ui
                    .add_enabled(can_close, egui::Button::new("Close Tab"))
                    .clicked()
                {
                    menu_action = TabBarAction::Close(active);
                    ui.close();
                }

                ui.separator();

                if ui.button("Next Tab").clicked() {
                    let next = (active + 1) % win.tabs.tab_count();
                    menu_action = TabBarAction::SwitchTo(next);
                    ui.close();
                }

                if ui.button("Previous Tab").clicked() {
                    let count = win.tabs.tab_count();
                    let prev = if active == 0 { count - 1 } else { active - 1 };
                    menu_action = TabBarAction::SwitchTo(prev);
                    ui.close();
                }
            });
            if tab_resp.inner.is_some() {
                any_menu_open = true;
            }

            let pane_resp = ui.menu_button("Pane", |ui| {
                self.show_pane_menu(ui, win);
            });
            if pane_resp.inner.is_some() {
                any_menu_open = true;
            }

            let window_resp = ui.menu_button("Window", |ui| {
                if ui.button("New Window").clicked() {
                    win.pending_new_window = true;
                    ui.close();
                }
            });
            if window_resp.inner.is_some() {
                any_menu_open = true;
            }

            // Playback controls: only shown when running in playback mode.
            #[cfg(feature = "playback")]
            if self.is_playback {
                self.show_playback_controls(ui, snap, win);
            }

            // Password-prompt lock indicator: shown in the menu bar (which is
            // always visible) so it works regardless of tab bar visibility.
            if self.config.security.password_indicator
                && win
                    .tabs
                    .active_tab()
                    .active_pane()
                    .echo_off
                    .load(std::sync::atomic::Ordering::Relaxed)
            {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new("\u{1F512}")
                            .color(egui::Color32::from_rgb(255, 200, 50)),
                    );
                });
            }
        });
        (menu_action, any_menu_open)
    }

    /// Render the "Pane" dropdown menu contents.
    ///
    /// Extracted from `show_menu_bar` to keep that function under the
    /// `too_many_lines` clippy limit.
    pub(super) fn show_pane_menu(&self, ui: &mut egui::Ui, win: &mut PerWindowState) {
        if ui.button("Split Vertical (Left | Right)").clicked() {
            self.spawn_split_pane(win, super::panes::SplitDirection::Horizontal);
            ui.close();
        }
        if ui.button("Split Horizontal (Top / Bottom)").clicked() {
            self.spawn_split_pane(win, super::panes::SplitDirection::Vertical);
            ui.close();
        }

        ui.separator();

        let can_close_pane = win.tabs.active_tab().pane_tree.pane_count().unwrap_or(1) > 1;

        if ui
            .add_enabled(can_close_pane, egui::Button::new("Close Pane"))
            .clicked()
        {
            win.pending_close_pane = true;
            ui.close();
        }

        let is_zoomed = win.tabs.active_tab().zoomed_pane.is_some();
        let zoom_label = if is_zoomed {
            "Un-Zoom Pane"
        } else {
            "Zoom Pane"
        };
        let can_zoom = win.tabs.active_tab().pane_tree.pane_count().unwrap_or(1) > 1;

        if ui
            .add_enabled(can_zoom, egui::Button::new(zoom_label))
            .clicked()
        {
            let tab = win.tabs.active_tab_mut();
            let current = tab.active_pane;
            if tab.zoomed_pane == Some(current) {
                tab.zoomed_pane = None;
            } else {
                tab.zoomed_pane = Some(current);
            }
            ui.close();
        }
    }

    /// Render the tab bar between the menu bar and the terminal area.
    ///
    /// Shows one button per open tab (active tab visually distinguished
    /// with a colored underline), a close button (x) on each tab when
    /// more than one tab is open, and a "+" button at the end to create
    /// new tabs. Tabs are separated by thin vertical dividers.
    ///
    /// Returns a `TabBarAction` describing what the user did (if anything).
    pub(super) fn show_tab_bar(&self, win: &PerWindowState, ui: &mut egui::Ui) -> TabBarAction {
        ui.horizontal(|ui| {
            let active = win.tabs.active_index();
            let count = win.tabs.tab_count();
            let mut action = TabBarAction::None;

            for (i, tab) in win.tabs.iter().enumerate() {
                // Thin vertical separator between tabs (skip before first).
                if i > 0 {
                    ui.separator();
                }

                // Read the echo-off state directly from the live atomic flag on
                // the Tab, not from the snapshot.  Snapshots are only published
                // when new PTY output arrives, so they go stale when the shell
                // is idle at a password prompt.  The atomic is updated by the
                // writer thread every 250 ms regardless of PTY activity.
                let is_echo_off = self.config.security.password_indicator
                    && tab
                        .active_pane()
                        .echo_off
                        .load(std::sync::atomic::Ordering::Relaxed);

                let tab_action = Self::show_single_tab(ui, tab, i, i == active, count, is_echo_off);
                if !matches!(tab_action, TabBarAction::None) {
                    action = tab_action;
                }
            }

            ui.separator();

            // "+" button to create a new tab.
            if ui.button("+").clicked() {
                action = TabBarAction::NewTab;
            }

            action
        })
        .inner
    }

    /// Render a single tab element with label, optional close button,
    /// and a distinct background color for the active tab.
    ///
    /// Inactive tabs with an unacknowledged bell are drawn with an amber
    /// text color and a warm-tinted background to make them more prominent.
    ///
    /// A lock icon is prepended to the label when `is_echo_off` is `true`,
    /// indicating that the foreground process has disabled terminal echo (i.e.
    /// a password prompt such as `sudo` or `ssh` is waiting for input).
    pub(super) fn show_single_tab(
        ui: &mut egui::Ui,
        tab: &Tab,
        index: usize,
        is_active: bool,
        count: usize,
        is_echo_off: bool,
    ) -> TabBarAction {
        let mut action = TabBarAction::None;
        let pane = tab.active_pane();
        let label = if pane.title.is_empty() {
            "Shell"
        } else {
            &pane.title
        };

        let has_bell = pane.bell_active && !is_active;

        // Build the display label: prepend a lock indicator when echo is disabled
        // (password prompt active), and a bell indicator when the tab has an
        // unacknowledged bell and is not the active (focused) tab.
        let display_label = match (is_echo_off, has_bell) {
            (true, true) => format!("\u{1f510} \u{1f514} {label}"),
            (true, false) => format!("\u{1f510} {label}"),
            (false, true) => format!("\u{1f514} {label}"),
            (false, false) => label.to_owned(),
        };

        // Tab frame: active gets a gray fill, bell-active inactive tabs
        // get a warm amber tint, others use a transparent frame.
        let frame = if is_active {
            egui::Frame::NONE
                .fill(egui::Color32::from_gray(100))
                .corner_radius(4.0)
                .inner_margin(0.0)
        } else if has_bell {
            egui::Frame::NONE
                .fill(egui::Color32::from_rgba_unmultiplied(180, 120, 30, 40))
                .corner_radius(4.0)
                .inner_margin(0.0)
        } else {
            egui::Frame::NONE
        };

        frame.show(ui, |ui| {
            ui.horizontal(|ui| {
                // Bell-active tabs use amber text for visibility.
                let rich_label = if has_bell {
                    egui::RichText::new(&display_label)
                        .size(13.0)
                        .color(egui::Color32::from_rgb(255, 180, 50))
                } else {
                    egui::RichText::new(&display_label).size(13.0)
                };

                let response = ui.selectable_label(is_active, rich_label);
                if response.clicked() && !is_active {
                    action = TabBarAction::SwitchTo(index);
                }

                // Show close button when more than one tab is open.
                if count > 1 && ui.small_button("\u{00d7}").clicked() {
                    action = TabBarAction::Close(index);
                }
            });
        });

        action
    }

    /// Render the playback toolbar controls (mode selector, play/pause, next, progress).
    #[cfg(feature = "playback")]
    pub(super) fn show_playback_controls(
        &mut self,
        ui: &mut egui::Ui,
        snap: &TerminalSnapshot,
        win: &PerWindowState,
    ) {
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
                win.send_playback_cmd(PlaybackCommand::SetMode(mode));
            }
        });

        ui.separator();

        // Play / Pause toggle button.
        let is_playing = info.is_some_and(|i| i.playing);
        let is_complete = info.is_some_and(|i| i.current_frame >= i.total_frames);
        let has_mode = self.selected_playback_mode.is_some();

        if is_playing {
            if ui.button("Pause").clicked() {
                win.send_playback_cmd(PlaybackCommand::Pause);
            }
        } else {
            let play_btn = ui.add_enabled(!is_complete && has_mode, egui::Button::new("Play"));
            if play_btn.clicked() {
                win.send_playback_cmd(PlaybackCommand::Play);
            }
        }

        // Next button: only active in frame-stepping mode.
        let is_frame_stepping = self.selected_playback_mode == Some(PlaybackMode::FrameStepping);
        let next_btn = ui.add_enabled(is_frame_stepping && !is_complete, egui::Button::new("Next"));
        if next_btn.clicked() {
            win.send_playback_cmd(PlaybackCommand::NextFrame);
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
    #[cfg(feature = "playback")]
    pub(super) const fn playback_mode_label(&self) -> &'static str {
        match self.selected_playback_mode {
            None => "Mode",
            Some(PlaybackMode::Instant) => "Instant",
            Some(PlaybackMode::RealTime) => "Real-Time",
            Some(PlaybackMode::FrameStepping) => "Frame Stepping",
        }
    }
}
