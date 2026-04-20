// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use egui;
use freminal_common::keybindings::KeyAction;

use super::TabBarAction;
use super::tabs::Tab;
use super::window::PerWindowState;

impl super::FreminalGui {
    /// Create a `Button` with the shortcut label for `action` from the active
    /// binding map, using platform-canonical modifier symbols.
    fn menu_button_for(&self, label: &str, action: KeyAction) -> egui::Button<'_> {
        let btn = egui::Button::new(label);
        if let Some(combo) = self.binding_map.combo_for(action) {
            btn.shortcut_text(combo.display_platform())
        } else {
            btn
        }
    }
    /// Show the top menu bar.
    ///
    /// Contains a "Freminal" menu with Settings and Quit entries, a "Tab"
    /// menu with tab management actions.
    ///
    /// Returns `(action, any_menu_open)` — the second element is `true`
    /// when any dropdown menu is currently expanded, so the caller can
    /// suppress terminal input and prevent the dismiss-click from leaking
    /// through to the PTY.
    pub(super) fn show_menu_bar(
        &mut self,
        ui: &mut egui::Ui,
        win: &mut PerWindowState,
        window_id: super::WindowId,
    ) -> (TabBarAction, bool) {
        let mut menu_action = TabBarAction::None;
        let mut any_menu_open = false;
        egui::MenuBar::new().ui(ui, |ui| {
            let freminal_resp = ui.menu_button("Freminal", |ui| {
                if ui
                    .add(self.menu_button_for("Settings...", KeyAction::OpenSettings))
                    .clicked()
                {
                    if self.settings_window_id.is_some() {
                        // Settings window already exists — focus it.
                        self.pending_focus_settings = true;
                    } else if !self.settings_modal.is_open && !self.pending_settings_window {
                        let families = win.terminal_widget.monospace_families();
                        self.settings_modal
                            .open(&self.config, families, win.os_dark_mode);
                        self.settings_modal
                            .set_base_font_defs(win.terminal_widget.base_font_defs().clone());
                        self.settings_owner = Some(window_id);
                        self.pending_settings_window = true;
                    }
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
                menu_action = self.show_tab_menu(ui, win);
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
                if ui
                    .add(self.menu_button_for("New Window", KeyAction::NewWindow))
                    .clicked()
                {
                    win.pending_new_window = true;
                    ui.close();
                }
            });
            if window_resp.inner.is_some() {
                any_menu_open = true;
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

    /// Render the "Tab" dropdown menu contents.
    ///
    /// Returns a `TabBarAction` if the user clicked a tab management item.
    fn show_tab_menu(&self, ui: &mut egui::Ui, win: &PerWindowState) -> TabBarAction {
        if ui
            .add(self.menu_button_for("New Tab", KeyAction::NewTab))
            .clicked()
        {
            ui.close();
            return TabBarAction::NewTab;
        }

        let active = win.tabs.active_index();
        let can_close = win.tabs.tab_count() > 1;
        if ui
            .add_enabled(
                can_close,
                self.menu_button_for("Close Tab", KeyAction::CloseTab),
            )
            .clicked()
        {
            ui.close();
            return TabBarAction::Close(active);
        }

        ui.separator();

        if ui
            .add(self.menu_button_for("Next Tab", KeyAction::NextTab))
            .clicked()
        {
            let next = (active + 1) % win.tabs.tab_count();
            ui.close();
            return TabBarAction::SwitchTo(next);
        }

        if ui
            .add(self.menu_button_for("Previous Tab", KeyAction::PrevTab))
            .clicked()
        {
            let count = win.tabs.tab_count();
            let prev = if active == 0 { count - 1 } else { active - 1 };
            ui.close();
            return TabBarAction::SwitchTo(prev);
        }

        TabBarAction::None
    }

    /// Render the "Pane" dropdown menu contents.
    ///
    /// Extracted from `show_menu_bar` to keep that function under the
    /// `too_many_lines` clippy limit.
    pub(super) fn show_pane_menu(&self, ui: &mut egui::Ui, win: &mut PerWindowState) {
        if ui
            .add(self.menu_button_for("Split Vertical (Left | Right)", KeyAction::SplitVertical))
            .clicked()
        {
            self.spawn_split_pane(win, super::panes::SplitDirection::Horizontal);
            ui.close();
        }
        if ui
            .add(self.menu_button_for(
                "Split Horizontal (Top / Bottom)",
                KeyAction::SplitHorizontal,
            ))
            .clicked()
        {
            self.spawn_split_pane(win, super::panes::SplitDirection::Vertical);
            ui.close();
        }

        ui.separator();

        let can_close_pane = win.tabs.active_tab().pane_tree.pane_count().unwrap_or(1) > 1;

        if ui
            .add_enabled(
                can_close_pane,
                self.menu_button_for("Close Pane", KeyAction::ClosePane),
            )
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
            .add_enabled(
                can_zoom,
                self.menu_button_for(zoom_label, KeyAction::ZoomPane),
            )
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
}
