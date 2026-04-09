// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use std::sync::{Arc, OnceLock};
use std::time::Instant;

use crate::gui::colors::internal_color_to_egui_with_alpha;
use anyhow::Result;
use conv2::{ApproxFrom, ConvUtil, ValueFrom};
use crossbeam_channel::{Receiver, Sender};
use eframe::egui::{self, CentralPanel, Panel, Pos2, Vec2, ViewportCommand};
use freminal_common::args::Args;
use freminal_common::buffer_states::window_manipulation::WindowManipulation;
use freminal_common::config::{Config, TabBarPosition, ThemeMode};
use freminal_common::pty_write::PtyWrite;
use freminal_terminal_emulator::io::{InputEvent, WindowCommand};
#[cfg(feature = "playback")]
use freminal_terminal_emulator::io::{PlaybackCommand, PlaybackMode};
use freminal_terminal_emulator::snapshot::TerminalSnapshot;
use settings::{SettingsAction, SettingsModal};
use tabs::{Tab, TabManager};
use terminal::FreminalTerminalWidget;

pub mod atlas;
pub mod colors;
pub mod font_manager;
pub mod fonts;
pub mod mouse;
pub mod pty;
pub mod renderer;
pub mod search;
pub mod settings;
pub mod shaping;
pub mod tabs;
pub mod terminal;
pub mod view_state;

fn set_egui_options(
    ctx: &egui::Context,
    theme: &freminal_common::themes::ThemePalette,
    bg_opacity: f32,
) {
    ctx.global_style_mut(|style| {
        // window_fill stays fully opaque so menus, settings modal, and all
        // egui chrome are never affected by background_opacity.
        style.visuals.window_fill = internal_color_to_egui_with_alpha(
            freminal_common::colors::TerminalColor::DefaultBackground,
            false,
            theme,
            1.0,
        );
        // panel_fill gets the opacity — it controls the CentralPanel
        // (terminal area) background, which is the only surface that
        // should be semi-transparent.
        style.visuals.panel_fill = internal_color_to_egui_with_alpha(
            freminal_common::colors::TerminalColor::DefaultBackground,
            false,
            theme,
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

/// Action requested by the tab bar UI.
///
/// Returned by `show_tab_bar()` and consumed by the main `ui()` method
/// after the panel finishes rendering.
#[derive(Clone, Copy)]
enum TabBarAction {
    /// No tab bar interaction this frame.
    None,
    /// User clicked the "+" button — spawn a new tab.
    NewTab,
    /// User clicked a tab label — switch to tab at `index`.
    SwitchTo(usize),
    /// User clicked the "x" close button — close tab at `index`.
    Close(usize),
}

struct FreminalGui {
    /// All open terminal tabs, managed by `TabManager`.
    /// Each tab owns its own PTY channels, snapshot handle, and `ViewState`.
    tabs: TabManager,

    terminal_widget: FreminalTerminalWidget,
    config: Config,

    /// CLI arguments needed for spawning new PTY tabs.
    args: Args,

    /// Shared egui context handle used by PTY consumer threads to request
    /// repaints after publishing new snapshots.
    egui_ctx: Arc<OnceLock<egui::Context>>,

    /// Settings modal state (open/close, draft config, tabs).
    settings_modal: SettingsModal,

    /// Compiled key-binding map from config. Rebuilt when the user applies
    /// new settings. Passed into the terminal widget on every frame so that
    /// bound key combos are intercepted before PTY dispatch.
    binding_map: freminal_common::keybindings::BindingMap,

    /// The last title sent to the OS window title bar via
    /// `ViewportCommand::Title`.  Compared each frame so we only issue
    /// the viewport command when the title actually changes — avoiding
    /// an unconditional `send_viewport_cmd` that would trigger an
    /// infinite repaint loop.
    last_window_title: String,

    /// Cached OS dark/light preference.  `true` = OS is in dark mode.
    ///
    /// Sampled each frame from `egui ctx.style().visuals.dark_mode` and used
    /// to resolve `ThemeMode::Auto` to the correct palette.  When the value
    /// changes, the active theme is re-applied to all tabs.
    os_dark_mode: bool,

    /// Cached inputs to `global_style_mut` from the previous frame:
    /// `(is_normal_display, theme, bg_opacity)`.
    ///
    /// `None` on the first frame forces an unconditional style apply.
    /// Compared each frame; `global_style_mut` is only called when a
    /// value changes.  This eliminates the per-frame `Arc::make_mut`
    /// clone of the egui `Style` during idle mouse movement.
    style_cache: Option<(bool, &'static freminal_common::themes::ThemePalette, f32)>,

    /// Whether this instance is running in playback mode.
    #[cfg(feature = "playback")]
    is_playback: bool,

    /// The playback mode currently selected in the GUI dropdown.
    /// Only meaningful when `is_playback` is true.
    #[cfg(feature = "playback")]
    selected_playback_mode: Option<PlaybackMode>,
}

impl FreminalGui {
    fn new(
        cc: &eframe::CreationContext<'_>,
        initial_tab: Tab,
        config: Config,
        args: Args,
        egui_ctx: Arc<OnceLock<egui::Context>>,
        config_path: Option<std::path::PathBuf>,
        #[cfg(feature = "playback")] is_playback: bool,
    ) -> Self {
        // Sample the OS dark/light preference from egui.
        // `dark_mode` is true when the OS is in dark mode.
        let os_dark_mode = cc.egui_ctx.global_style().visuals.dark_mode;

        let initial_theme =
            freminal_common::themes::by_slug(config.theme.active_slug(os_dark_mode))
                .unwrap_or(&freminal_common::themes::CATPPUCCIN_MOCHA);
        set_egui_options(&cc.egui_ctx, initial_theme, config.ui.background_opacity);

        let gui = Self {
            tabs: TabManager::new(initial_tab),
            terminal_widget: FreminalTerminalWidget::new(&cc.egui_ctx, &config),
            binding_map: config.build_binding_map().unwrap_or_else(|e| {
                error!("Failed to build binding map from config: {e}. Using defaults.");
                freminal_common::keybindings::BindingMap::default()
            }),
            config,
            args,
            egui_ctx,
            settings_modal: SettingsModal::new(config_path),
            last_window_title: String::from("Freminal"),
            os_dark_mode,
            // `None` forces the first frame to unconditionally apply the
            // style.  `set_egui_options` already ran above, so the first
            // snapshot comparison will update the cache without a redundant
            // `global_style_mut` call only when the snapshot differs from
            // what `set_egui_options` established.
            style_cache: None,
            #[cfg(feature = "playback")]
            is_playback,
            #[cfg(feature = "playback")]
            selected_playback_mode: None,
        };

        // Inform the initial tab about the configured theme mode and current OS
        // dark/light preference so DECRPM ?2031 responses are correct from the start.
        if let Err(e) = gui
            .tabs
            .active_tab()
            .input_tx
            .send(InputEvent::ThemeModeUpdate(
                gui.config.theme.mode,
                os_dark_mode,
            ))
        {
            error!("Failed to send initial ThemeModeUpdate to tab: {e}");
        }

        // The initial tab was spawned in main.rs with `active_slug(false)` before
        // egui existed, so when `mode = "auto"` and the OS is actually in dark mode,
        // the PTY thread has the wrong palette.  Correct it now that we know the
        // real OS preference.
        if gui.config.theme.active_slug(os_dark_mode) != gui.config.theme.active_slug(false)
            && let Some(theme) =
                freminal_common::themes::by_slug(gui.config.theme.active_slug(os_dark_mode))
            && let Err(e) = gui
                .tabs
                .active_tab()
                .input_tx
                .send(InputEvent::ThemeChange(theme))
        {
            error!("Failed to send initial ThemeChange to tab: {e}");
        }

        gui
    }

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
    fn show_menu_bar(
        &mut self,
        ui: &mut egui::Ui,
        snap: &TerminalSnapshot,
    ) -> (TabBarAction, bool) {
        let mut menu_action = TabBarAction::None;
        let mut any_menu_open = false;
        egui::MenuBar::new().ui(ui, |ui| {
            let freminal_resp = ui.menu_button("Freminal", |ui| {
                if ui.button("Settings...").clicked() {
                    let families = self.terminal_widget.monospace_families();
                    self.settings_modal
                        .open(&self.config, families, self.os_dark_mode);
                    self.settings_modal
                        .set_base_font_defs(self.terminal_widget.base_font_defs().clone());
                    ui.close();
                }

                ui.separator();

                if ui.button("Quit").clicked() {
                    ui.ctx().send_viewport_cmd(ViewportCommand::Close);
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

                let active = self.tabs.active_index();
                let can_close = self.tabs.tab_count() > 1;
                if ui
                    .add_enabled(can_close, egui::Button::new("Close Tab"))
                    .clicked()
                {
                    menu_action = TabBarAction::Close(active);
                    ui.close();
                }

                ui.separator();

                if ui.button("Next Tab").clicked() {
                    let next = (active + 1) % self.tabs.tab_count();
                    menu_action = TabBarAction::SwitchTo(next);
                    ui.close();
                }

                if ui.button("Previous Tab").clicked() {
                    let count = self.tabs.tab_count();
                    let prev = if active == 0 { count - 1 } else { active - 1 };
                    menu_action = TabBarAction::SwitchTo(prev);
                    ui.close();
                }
            });
            if tab_resp.inner.is_some() {
                any_menu_open = true;
            }

            // Playback controls: only shown when running in playback mode.
            #[cfg(feature = "playback")]
            if self.is_playback {
                self.show_playback_controls(ui, snap);
            }

            // Password-prompt lock indicator: shown in the menu bar (which is
            // always visible) so it works regardless of tab bar visibility.
            if self.config.security.password_indicator
                && self
                    .tabs
                    .active_tab()
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

    /// Render the tab bar between the menu bar and the terminal area.
    ///
    /// Shows one button per open tab (active tab visually distinguished
    /// with a colored underline), a close button (x) on each tab when
    /// more than one tab is open, and a "+" button at the end to create
    /// new tabs. Tabs are separated by thin vertical dividers.
    ///
    /// Returns a `TabBarAction` describing what the user did (if anything).
    fn show_tab_bar(&self, ui: &mut egui::Ui) -> TabBarAction {
        ui.horizontal(|ui| {
            let active = self.tabs.active_index();
            let count = self.tabs.tab_count();
            let mut action = TabBarAction::None;

            for (i, tab) in self.tabs.iter().enumerate() {
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
                    && tab.echo_off.load(std::sync::atomic::Ordering::Relaxed);

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
    /// A 🔐 lock icon is prepended to the label when `is_echo_off` is `true`,
    /// indicating that the foreground process has disabled terminal echo (i.e.
    /// a password prompt such as `sudo` or `ssh` is waiting for input).
    fn show_single_tab(
        ui: &mut egui::Ui,
        tab: &Tab,
        index: usize,
        is_active: bool,
        count: usize,
        is_echo_off: bool,
    ) -> TabBarAction {
        let mut action = TabBarAction::None;
        let label = if tab.title.is_empty() {
            "Shell"
        } else {
            &tab.title
        };

        let has_bell = tab.bell_active && !is_active;

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

    /// Spawn a new PTY-backed tab and add it to the tab manager.
    ///
    /// Uses the stored `Args` and `Config` to configure the new terminal.
    /// Logs an error and does nothing if the PTY fails to start.
    fn spawn_new_tab(&mut self) {
        // Tabs are not supported in playback mode — there is exactly one
        // recording session to replay and no PTY to spawn.
        #[cfg(feature = "playback")]
        if self.is_playback {
            return;
        }

        let theme =
            freminal_common::themes::by_slug(self.config.theme.active_slug(self.os_dark_mode))
                .unwrap_or(&freminal_common::themes::CATPPUCCIN_MOCHA);

        match pty::spawn_pty_tab(
            &self.args,
            self.config.scrollback.limit,
            theme,
            &self.egui_ctx,
        ) {
            Ok(channels) => {
                let id = self.tabs.next_tab_id();
                let tab = Tab {
                    id,
                    arc_swap: channels.arc_swap,
                    input_tx: channels.input_tx,
                    pty_write_tx: channels.pty_write_tx,
                    window_cmd_rx: channels.window_cmd_rx,
                    clipboard_rx: channels.clipboard_rx,
                    search_buffer_rx: channels.search_buffer_rx,
                    pty_dead_rx: channels.pty_dead_rx,
                    title: "Terminal".to_owned(),
                    bell_active: false,
                    title_stack: Vec::new(),
                    view_state: view_state::ViewState::new(),
                    echo_off: channels.echo_off,
                };
                // Inform the new tab of the current theme mode so DECRPM
                // ?2031 queries return the correct locked/dynamic status.
                if let Err(e) = tab.input_tx.send(InputEvent::ThemeModeUpdate(
                    self.config.theme.mode,
                    self.os_dark_mode,
                )) {
                    error!("Failed to send ThemeModeUpdate to new tab: {e}");
                }
                self.tabs.add_tab(tab);
            }
            Err(e) => {
                error!("Failed to spawn new tab: {e}");
            }
        }
    }

    /// Close the tab at `index`. If it is the last tab, does nothing
    /// (the window close is handled by the PTY exit path instead).
    fn close_tab(&mut self, index: usize) {
        if let Err(e) = self.tabs.close_tab(index) {
            trace!("Cannot close tab: {e}");
        }
    }

    /// Dispatch a `TabBarAction` from either the tab bar or the Tab menu.
    fn dispatch_tab_bar_action(&mut self, action: TabBarAction) {
        match action {
            TabBarAction::NewTab => self.spawn_new_tab(),
            TabBarAction::SwitchTo(i) => {
                if let Err(e) = self.tabs.switch_to(i) {
                    error!("Failed to switch tab: {e}");
                } else {
                    // Clear the bell indicator on the newly-active tab —
                    // switching to it acknowledges the bell.
                    self.tabs.active_tab_mut().bell_active = false;
                }
            }
            TabBarAction::Close(i) => self.close_tab(i),
            TabBarAction::None => {}
        }
    }

    /// Dispatch a deferred key action that requires full GUI state.
    ///
    /// Called from the `ui()` method for each action returned by the terminal
    /// widget's input handler. Actions that can be handled at the input layer
    /// (e.g. scrollback, copy/paste) are dispatched there; the remaining
    /// actions (tab management, settings, zoom, search) land here.
    fn dispatch_deferred_action(&mut self, action: freminal_common::keybindings::KeyAction) {
        use freminal_common::keybindings::KeyAction;

        match action {
            // -- Settings --
            KeyAction::OpenSettings => {
                if !self.settings_modal.is_open {
                    let families = self.terminal_widget.monospace_families();
                    self.settings_modal
                        .open(&self.config, families, self.os_dark_mode);
                    self.settings_modal
                        .set_base_font_defs(self.terminal_widget.base_font_defs().clone());
                }
            }

            // -- Tab management --
            KeyAction::NewTab => self.spawn_new_tab(),
            KeyAction::CloseTab => {
                if let Err(e) = self.tabs.close_active_tab() {
                    trace!("Cannot close tab: {e}");
                }
            }
            KeyAction::NextTab => {
                self.tabs.next_tab();
                self.tabs.active_tab_mut().bell_active = false;
            }
            KeyAction::PrevTab => {
                self.tabs.prev_tab();
                self.tabs.active_tab_mut().bell_active = false;
            }
            KeyAction::SwitchToTab1 => self.switch_to_tab_n(0),
            KeyAction::SwitchToTab2 => self.switch_to_tab_n(1),
            KeyAction::SwitchToTab3 => self.switch_to_tab_n(2),
            KeyAction::SwitchToTab4 => self.switch_to_tab_n(3),
            KeyAction::SwitchToTab5 => self.switch_to_tab_n(4),
            KeyAction::SwitchToTab6 => self.switch_to_tab_n(5),
            KeyAction::SwitchToTab7 => self.switch_to_tab_n(6),
            KeyAction::SwitchToTab8 => self.switch_to_tab_n(7),
            KeyAction::SwitchToTab9 => self.switch_to_tab_n(8),
            KeyAction::MoveTabLeft => self.tabs.move_active_left(),
            KeyAction::MoveTabRight => self.tabs.move_active_right(),
            // -- Font zoom --
            KeyAction::ZoomIn => self.apply_zoom(1.0),
            KeyAction::ZoomOut => self.apply_zoom(-1.0),
            KeyAction::ZoomReset => {
                self.tabs.active_tab_mut().view_state.reset_zoom();
                self.terminal_widget.apply_font_zoom(self.config.font.size);
            }

            // -- Search overlay --
            KeyAction::OpenSearch => {
                self.tabs.active_tab_mut().view_state.search_state.is_open = true;
            }
            KeyAction::SearchNext => {
                let tab = self.tabs.active_tab_mut();
                tab.view_state.search_state.next_match();
                let snap = tab.arc_swap.load();
                search::scroll_to_match_and_send(&mut tab.view_state, &snap, &tab.input_tx);
            }
            KeyAction::SearchPrev => {
                let tab = self.tabs.active_tab_mut();
                tab.view_state.search_state.prev_match();
                let snap = tab.arc_swap.load();
                search::scroll_to_match_and_send(&mut tab.view_state, &snap, &tab.input_tx);
            }
            KeyAction::PrevCommand => {
                let tab = self.tabs.active_tab_mut();
                let snap = tab.arc_swap.load();
                search::jump_to_prev_command(&mut tab.view_state, &snap);
            }
            KeyAction::NextCommand => {
                let tab = self.tabs.active_tab_mut();
                let snap = tab.arc_swap.load();
                search::jump_to_next_command(&mut tab.view_state, &snap);
            }

            // -- Not yet implemented --
            // Consumed (not forwarded to PTY) but silently ignored until
            // their respective features land.
            KeyAction::RenameTab => {
                trace!("Unhandled deferred key action: {action:?}");
            }

            // These actions are handled at the input layer and should never
            // reach the deferred dispatch. Log if they somehow do.
            KeyAction::Copy
            | KeyAction::Paste
            | KeyAction::SelectAll
            | KeyAction::ToggleMenuBar
            | KeyAction::ScrollPageUp
            | KeyAction::ScrollPageDown
            | KeyAction::ScrollToTop
            | KeyAction::ScrollToBottom
            | KeyAction::ScrollLineUp
            | KeyAction::ScrollLineDown => {
                trace!(
                    "Unexpected deferred key action (should be handled at input layer): {action:?}"
                );
            }
        }
    }

    /// Adjust font zoom by `delta` points and apply the new effective size
    /// to the terminal widget.
    fn apply_zoom(&mut self, delta: f32) {
        let base = self.config.font.size;
        let vs = &mut self.tabs.active_tab_mut().view_state;
        vs.adjust_zoom(base, delta);
        let effective = vs.effective_font_size(base);
        self.terminal_widget.apply_font_zoom(effective);
    }

    /// Switch to tab N (0-indexed). Silently does nothing if the index
    /// is out of bounds (e.g. user presses Ctrl+Shift+5 with only 3 tabs).
    fn switch_to_tab_n(&mut self, index: usize) {
        if let Err(e) = self.tabs.switch_to(index) {
            trace!("Cannot switch to tab {index}: {e}");
        } else {
            self.tabs.active_tab_mut().bell_active = false;
        }
    }

    /// Render the playback toolbar controls (mode selector, play/pause, next, progress).
    #[cfg(feature = "playback")]
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
    #[cfg(feature = "playback")]
    const fn playback_mode_label(&self) -> &'static str {
        match self.selected_playback_mode {
            None => "Mode",
            Some(PlaybackMode::Instant) => "Instant",
            Some(PlaybackMode::RealTime) => "Real-Time",
            Some(PlaybackMode::FrameStepping) => "Frame Stepping",
        }
    }

    /// Send a playback command to the consumer thread via the input channel.
    #[cfg(feature = "playback")]
    fn send_playback_cmd(&self, cmd: PlaybackCommand) {
        if let Err(e) = self
            .tabs
            .active_tab()
            .input_tx
            .send(InputEvent::PlaybackControl(cmd))
        {
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

/// Read the system clipboard and return its contents as a base64-encoded string.
///
/// Returns an empty string on any error (clipboard unavailable, empty, etc.).
/// This is intentionally infallible — clipboard access is best-effort.
///
/// Clipboard contents beyond [`MAX_CLIPBOARD_BYTES`] are truncated to avoid
/// excessive memory allocation and PTY traffic from a large clipboard.
fn read_clipboard_base64() -> String {
    /// Maximum clipboard payload size (bytes) returned for OSC 52 queries.
    /// 100 KiB matches limits used by other terminal emulators (e.g. xterm).
    const MAX_CLIPBOARD_BYTES: usize = 100 * 1024;

    let Ok(mut clipboard) = arboard::Clipboard::new() else {
        debug!("OSC 52 query: failed to open clipboard");
        return String::new();
    };

    match clipboard.get_text() {
        Ok(text) if !text.is_empty() => {
            let bytes = text.as_bytes();
            if bytes.len() > MAX_CLIPBOARD_BYTES {
                debug!(
                    "OSC 52 query: clipboard truncated from {} to {MAX_CLIPBOARD_BYTES} bytes",
                    bytes.len()
                );
                freminal_common::base64::encode(&bytes[..MAX_CLIPBOARD_BYTES])
            } else {
                freminal_common::base64::encode(bytes)
            }
        }
        Ok(_) => String::new(),
        Err(e) => {
            debug!("OSC 52 query: clipboard read error: {e}");
            String::new()
        }
    }
}

/// Drain and dispatch all pending [`WindowCommand`]s for this frame.
///
/// ## Flow
///
/// 1. **Non-blocking drain** — `window_cmd_rx.try_recv()` is called in a
///    loop until the channel is empty.  All commands queued by the PTY
///    consumer thread since the last frame are processed before rendering.
///
/// 2. **Variant routing** — both `Viewport` and `Report` commands carry
///    the same inner `WindowManipulation` value; the outer tag is not used
///    for routing here (the dispatch is done entirely on the inner value).
///
/// 3. **Viewport operations** — forwarded to egui via
///    `ui.ctx().send_viewport_cmd(ViewportCommand::…)`.  Covers move,
///    resize, minimize/restore, maximize/restore, fullscreen, raise/lower,
///    de-iconify, and resize-to-lines-and-columns.
///
/// 4. **Report queries** — the function measures the current viewport
///    geometry from `ui.ctx()` (pixel positions, sizes) and the font metrics
///    (`font_width`, `font_height`), then builds the appropriate escape
///    sequence response string and sends it directly to the PTY via
///    `pty_write_tx` using `send_pty_response()`.  The emulator is never
///    involved.  Covered variants:
///    - `ReportWindowState` → `ESC [ 1 t` or `ESC [ 2 t`
///    - `ReportWindowPosition*` → `ESC [ 3 ; x ; y t`
///    - `ReportWindowSize*` and `ReportRootWindowSize*` → `ESC [ 4/5/6/7 ; h ; w t`
///    - `ReportIconLabel` and `ReportTitle` → `ESC ] 0 / 1 / 2 ; <title> ST`
///
///    **Not handled here** (no-ops in this function):
///    - `ReportCharacterSizeInPixels`, `ReportTerminalSizeInCharacters`,
///      `ReportRootWindowSizeInCharacters` — these are handled synchronously
///      on the PTY thread by `TerminalHandler::handle_window_manipulation` so
///      that responses arrive in the same batch as DA1.  They never reach the
///      GUI's `window_cmd_rx` stream.
///
/// 5. **Title stack** — `SaveWindowTitleToStack` and
///    `RestoreWindowTitleFromStack` push/pop from `title_stack`; `SetTitleBarText`
///    calls `ViewportCommand::Title`.
///
/// 6. **OSC 52 clipboard** — `SetClipboard` copies decoded text to the system
///    clipboard via `ui.ctx().copy_text()`.  `QueryClipboard` reads the system
///    clipboard via `arboard` when `allow_clipboard_read` is `true`; otherwise
///    it responds with an empty payload (the safe/secure default).
// Inherently large: handles all `WindowCommand` variants — viewport commands, Report* PTY
// responses, title stack, clipboard. Each variant requires distinct context (ui, pty_write_tx,
// title_stack). Splitting further would scatter a cohesive protocol handler.
// All arguments are required context that cannot be easily grouped without obscuring intent.
#[allow(clippy::too_many_lines, clippy::too_many_arguments)]
fn handle_window_manipulation(
    ui: &egui::Ui,
    window_cmd_rx: &Receiver<WindowCommand>,
    pty_write_tx: &Sender<PtyWrite>,
    font_width: usize,
    font_height: usize,
    window_width: egui::Rect,
    title_stack: &mut Vec<String>,
    tab_title: &mut String,
    bell_active: &mut bool,
    bell_since: &mut Option<Instant>,
    bell_mode: freminal_common::config::BellMode,
    allow_clipboard_read: bool,
    is_active: bool,
    window_focused: bool,
) {
    // Drain all pending WindowCommands for this frame.
    while let Ok(wc) = window_cmd_rx.try_recv() {
        let window_event = match wc {
            WindowCommand::Viewport(cmd) | WindowCommand::Report(cmd) => cmd,
        };

        match window_event {
            // ── Viewport-mutating commands: skip for inactive tabs ───
            // An inactive tab must not resize, move, minimize, or fullscreen
            // the shared window.
            WindowManipulation::DeIconifyWindow
            | WindowManipulation::MinimizeWindow
            | WindowManipulation::MoveWindow(_, _)
            | WindowManipulation::ResizeWindow(_, _)
            | WindowManipulation::MaximizeWindow
            | WindowManipulation::RestoreNonMaximizedWindow
            | WindowManipulation::ResizeWindowToLinesAndColumns(_, _)
            | WindowManipulation::NotFullScreen
            | WindowManipulation::FullScreen
            | WindowManipulation::ToggleFullScreen
                if !is_active => {}

            // ── Title: inactive tabs update their own title only ─────
            WindowManipulation::SetTitleBarText(title) if !is_active => {
                tab_title.clone_from(&title);
            }

            // ── Title stack: inactive tabs save their own tab title ──
            WindowManipulation::SaveWindowTitleToStack if !is_active => {
                title_stack.push(tab_title.clone());
            }
            WindowManipulation::RestoreWindowTitleFromStack if !is_active => {
                if let Some(title) = title_stack.pop() {
                    tab_title.clone_from(&title);
                } else {
                    tab_title.clear();
                }
            }
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
                // Update the tab title for the tab bar display.
                tab_title.clone_from(&title);
                // Set the window title bar to the active tab's title.
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
                    tab_title.clone_from(&title);
                    ui.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::Title(title));
                } else {
                    tab_title.clear();
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

            // OSC 52 clipboard query: read the system clipboard when the
            // user has opted in via [security] allow_clipboard_read = true.
            // Otherwise respond with an empty payload (safe default).
            WindowManipulation::QueryClipboard(sel) => {
                let payload = if allow_clipboard_read {
                    read_clipboard_base64()
                } else {
                    tracing::debug!(
                        "OSC 52 query for selection '{sel}' — blocked by security config"
                    );
                    String::new()
                };
                send_pty_response(pty_write_tx, &format!("\x1b]52;{sel};{payload}\x1b\\"));
            }

            // Terminal bell: ignored entirely when bell mode is `None`.
            // Otherwise mark this tab as having an unacknowledged bell and
            // start the visual flash timer.  When the window is unfocused,
            // also request OS-level taskbar attention.
            WindowManipulation::Bell => {
                if bell_mode == freminal_common::config::BellMode::Visual {
                    *bell_active = true;
                    *bell_since = Some(Instant::now());

                    if !window_focused {
                        ui.ctx()
                            .send_viewport_cmd(ViewportCommand::RequestUserAttention(
                                egui::UserAttentionType::Informational,
                            ));
                    }
                }
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

    // Inherently large: the main per-frame UI function handles menu bar, settings modal, window
    // manipulation drain, terminal widget layout, and resize detection — all in one pass over
    // the shared snapshot. Artificial sub-functions would not reduce the coupling.
    #[allow(clippy::too_many_lines)]
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        trace!("Starting new frame");
        let now = std::time::Instant::now();

        // Detect OS dark/light preference changes and auto-switch theme when
        // `mode = "auto"` is configured.
        let current_os_dark = ui.ctx().global_style().visuals.dark_mode;
        if current_os_dark != self.os_dark_mode {
            self.os_dark_mode = current_os_dark;

            // Only auto-switch when the user has opted in.
            // Always propagate the updated OS preference so DECRPM ?2031
            // reflects the new dark/light state, regardless of ThemeMode.
            for tab in self.tabs.iter() {
                if let Err(e) = tab.input_tx.send(InputEvent::ThemeModeUpdate(
                    self.config.theme.mode,
                    self.os_dark_mode,
                )) {
                    error!("Failed to send ThemeModeUpdate on OS change to tab: {e}");
                }
            }

            if self.config.theme.mode == ThemeMode::Auto {
                let slug = self.config.theme.active_slug(self.os_dark_mode);
                if let Some(theme) = freminal_common::themes::by_slug(slug) {
                    // Notify every tab so all PTY threads get the new palette.
                    for tab in self.tabs.iter() {
                        if let Err(e) = tab.input_tx.send(
                            freminal_terminal_emulator::io::InputEvent::ThemeChange(theme),
                        ) {
                            error!("Failed to send auto ThemeChange to tab: {e}");
                        }
                    }
                    update_egui_theme(ui.ctx(), theme, self.config.ui.background_opacity);
                    self.terminal_widget.invalidate_theme_cache();
                }
            }
        }

        // Poll all tabs for PTY death signals.  Dead tabs are collected by
        // index (in reverse order so removal doesn't shift later indices).
        let dead_indices: Vec<usize> = self
            .tabs
            .iter()
            .enumerate()
            .filter(|(_, tab)| tab.pty_dead_rx.try_recv().is_ok())
            .map(|(i, _)| i)
            .rev()
            .collect();

        for idx in dead_indices {
            if self.tabs.tab_count() <= 1 {
                // Last tab died — close the whole application.
                ui.ctx().send_viewport_cmd(ViewportCommand::Close);
                return;
            }
            self.close_tab(idx);
        }

        // Load the latest snapshot from the PTY thread — no lock, single atomic load.
        let snap = self.tabs.active_tab().arc_swap.load();

        // Sync the GUI's scroll offset from the snapshot.  When new PTY output
        // arrives the PTY thread resets its offset to 0, so the snapshot will
        // carry scroll_offset = 0 even if the GUI previously sent a non-zero
        // value.  Adopting the snapshot's value keeps ViewState in sync.
        if self.tabs.active_tab().view_state.scroll_offset != snap.scroll_offset {
            self.tabs.active_tab_mut().view_state.scroll_offset = snap.scroll_offset;
        }

        // Menu bar at the top of the window.
        let mut any_menu_open = false;
        if !self.config.ui.hide_menu_bar {
            let (menu_action, menu_open) = Panel::top("menu_bar")
                .show_inside(ui, |ui| self.show_menu_bar(ui, &snap))
                .inner;
            any_menu_open = menu_open;
            self.dispatch_tab_bar_action(menu_action);
        }

        // Tab bar: shown when multiple tabs are open, or when the config
        // option `tabs.show_single_tab` is enabled.
        let show_tab_bar = self.tabs.tab_count() > 1 || self.config.tabs.show_single_tab;

        if show_tab_bar {
            let panel = match self.config.tabs.position {
                TabBarPosition::Top => Panel::top("tab_bar"),
                TabBarPosition::Bottom => Panel::bottom("tab_bar"),
            };
            let tab_action = panel.show_inside(ui, |ui| self.show_tab_bar(ui)).inner;
            self.dispatch_tab_bar_action(tab_action);
        }

        let _panel_response = CentralPanel::default().show_inside(ui, |ui| {
            // Synchronise font metrics with the current display scale *before*
            // reading `cell_size()`.  Without this, the first frame after a DPI
            // change would use stale pixel metrics for the resize calculation.
            let ppp = ui.ctx().pixels_per_point();
            self.terminal_widget.sync_pixels_per_point(ppp);

            // Synchronise font zoom for the active tab.  Each tab has its own
            // zoom_delta and the font manager only knows one size at a time.
            // This check fires on every frame but is a single float comparison
            // when no change is needed.
            let effective = self
                .tabs
                .active_tab()
                .view_state
                .effective_font_size(self.config.font.size);
            self.terminal_widget.apply_font_zoom(effective);

            // Compute char size once and reuse for both PTY sizing and widget layout.
            // `cell_size()` returns integer pixel dimensions (physical) from swash
            // font metrics.  egui's coordinate system uses logical points, so we
            // convert with `pixels_per_point` when doing layout math.
            let (cell_w_u, cell_height_u) = self.terminal_widget.cell_size();
            let font_width = usize::value_from(cell_w_u).unwrap_or(0);
            let font_height = usize::value_from(cell_height_u).unwrap_or(0);
            let logical_char_w = f32::approx_from(cell_w_u).unwrap_or(0.0) / ppp;
            let logical_char_h = f32::approx_from(cell_height_u).unwrap_or(0.0) / ppp;

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
            if new_size != self.tabs.active_tab().view_state.last_sent_size {
                if let Err(e) = self.tabs.active_tab().input_tx.send(InputEvent::Resize(
                    width_chars,
                    height_chars,
                    font_width,
                    font_height,
                )) {
                    error!("Failed to send resize event: {e}");
                } else {
                    self.tabs.active_tab_mut().view_state.last_sent_size = new_size;
                }
            }

            let window_width = ui.input(|i: &egui::InputState| i.content_rect());

            // Drain window commands for ALL tabs.  The active tab gets full
            // handling (viewport commands, reports, title updates, clipboard).
            // Inactive tabs get reports answered, titles updated, and clipboard
            // handled — only viewport-mutating commands (resize, move, minimize,
            // fullscreen) are discarded since an inactive tab must not alter the
            // shared window geometry.
            let active_idx = self.tabs.active_index();
            let window_focused = self.tabs.active_tab().view_state.window_focused;
            for (idx, tab) in self.tabs.iter_mut().enumerate() {
                handle_window_manipulation(
                    ui,
                    &tab.window_cmd_rx,
                    &tab.pty_write_tx,
                    font_width,
                    font_height,
                    window_width,
                    &mut tab.title_stack,
                    &mut tab.title,
                    &mut tab.bell_active,
                    &mut tab.view_state.bell_since,
                    self.config.bell.mode,
                    self.config.security.allow_clipboard_read,
                    idx == active_idx,
                    window_focused,
                );
            }

            // Update background color based on whether the terminal is in
            // normal (non-inverted) display mode.
            //
            // Gated: only call `global_style_mut` when the inputs have
            // changed.  `global_style_mut` triggers `Arc::make_mut` on
            // the egui `Style`, which clones every frame unless skipped.
            let bg_opacity = self.config.ui.background_opacity;
            let style_key = (snap.is_normal_display, snap.theme, bg_opacity);
            let style_changed = match self.style_cache {
                Some((prev_display, prev_theme, prev_opacity)) => {
                    prev_display != style_key.0
                        || !std::ptr::eq(prev_theme, style_key.1)
                        || prev_opacity.to_bits() != bg_opacity.to_bits()
                }
                None => true,
            };
            if style_changed {
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
                        // window_fill: always opaque (menus, settings, chrome).
                        style.visuals.window_fill =
                            egui::Color32::from_rgba_unmultiplied(255, 255, 255, 255);
                        // panel_fill: respects background_opacity (terminal area only).
                        let alpha = (bg_opacity * 255.0)
                            .round()
                            .approx_as::<u8>()
                            .unwrap_or(255);
                        style.visuals.panel_fill =
                            egui::Color32::from_rgba_unmultiplied(255, 255, 255, alpha);
                    });
                }
                self.style_cache = Some(style_key);
            }

            let tab = self.tabs.active_tab_mut();
            let is_echo_off = self.config.security.password_indicator
                && tab.echo_off.load(std::sync::atomic::Ordering::Relaxed);
            let deferred_actions = self.terminal_widget.show(
                ui,
                &snap,
                &mut tab.view_state,
                &tab.input_tx,
                &tab.clipboard_rx,
                &tab.search_buffer_rx,
                self.settings_modal.is_open || any_menu_open,
                bg_opacity,
                &self.binding_map,
                is_echo_off,
            );

            // Handle key actions that couldn't be dispatched at the input
            // layer because they require full GUI state.
            for action in deferred_actions {
                self.dispatch_deferred_action(action);
            }

            // Keep the window title bar in sync with the active tab's title.
            // This handles tab switches, OSC 0/2 title changes, and restore
            // from the title stack — all in one place.
            //
            // Only issue the viewport command when the title actually changed;
            // calling `send_viewport_cmd` unconditionally every frame triggers
            // an infinite repaint loop (~3 % idle CPU).
            let active_title = &self.tabs.active_tab().title;
            let window_title = if active_title.is_empty() {
                "Freminal"
            } else {
                active_title.as_str()
            };
            if window_title != self.last_window_title {
                window_title.clone_into(&mut self.last_window_title);
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Title(
                    self.last_window_title.clone(),
                ));
            }

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
                self.tabs.active_tab_mut().view_state.tick_text_blink();
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
        let settings_action = self.settings_modal.show(ui.ctx(), self.os_dark_mode);

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

                // If the active theme slug changed (accounting for mode and OS pref),
                // look it up and notify the PTY thread so the next snapshot carries
                // the new palette.
                if new_cfg.theme.active_slug(self.os_dark_mode)
                    != self.config.theme.active_slug(self.os_dark_mode)
                    && let Some(theme) = freminal_common::themes::by_slug(
                        new_cfg.theme.active_slug(self.os_dark_mode),
                    )
                {
                    if let Err(e) = self
                        .tabs
                        .active_tab()
                        .input_tx
                        .send(InputEvent::ThemeChange(theme))
                    {
                        error!("Failed to send ThemeChange to PTY thread: {e}");
                    }
                    update_egui_theme(ui.ctx(), theme, new_cfg.ui.background_opacity);
                    // Force a full vertex rebuild on the next frame so
                    // foreground/background colors are re-resolved against
                    // the new palette.  Without this, the preview's rebuild
                    // may be the last one, and the Apply-frame snapshot
                    // (with content_changed=false) would skip the rebuild.
                    self.terminal_widget.invalidate_theme_cache();
                }

                self.terminal_widget
                    .apply_config_changes(ui.ctx(), &self.config, &new_cfg);
                self.binding_map = new_cfg.build_binding_map().unwrap_or_else(|e| {
                    error!(
                        "Failed to rebuild binding map after settings apply: {e}. Using defaults."
                    );
                    freminal_common::keybindings::BindingMap::default()
                });
                self.config = new_cfg;

                // Notify all tabs of the new theme mode so DECRPM ?2031 returns
                // the correct locked/dynamic response after the config change.
                for tab in self.tabs.iter() {
                    if let Err(e) = tab.input_tx.send(InputEvent::ThemeModeUpdate(
                        self.config.theme.mode,
                        self.os_dark_mode,
                    )) {
                        error!("Failed to send ThemeModeUpdate after settings apply: {e}");
                    }
                }
            }
            SettingsAction::PreviewTheme(ref slug) => {
                if let Some(theme) = freminal_common::themes::by_slug(slug) {
                    if let Err(e) = self
                        .tabs
                        .active_tab()
                        .input_tx
                        .send(InputEvent::ThemeChange(theme))
                    {
                        error!("Failed to send theme preview to PTY thread: {e}");
                    }
                    update_egui_theme(ui.ctx(), theme, self.config.ui.background_opacity);
                }
            }
            SettingsAction::RevertTheme(ref slug, original_opacity) => {
                if let Some(theme) = freminal_common::themes::by_slug(slug) {
                    if let Err(e) = self
                        .tabs
                        .active_tab()
                        .input_tx
                        .send(InputEvent::ThemeChange(theme))
                    {
                        error!("Failed to send theme revert to PTY thread: {e}");
                    }
                    // Restore opacity first so update_egui_theme uses the
                    // correct value for panel_fill.
                    self.config.ui.background_opacity = original_opacity;
                    update_egui_theme(ui.ctx(), theme, original_opacity);
                }
            }
            SettingsAction::PreviewOpacity(opacity) | SettingsAction::RevertOpacity(opacity) => {
                self.config.ui.background_opacity = opacity;
            }
            SettingsAction::None => {}
        }

        let elapsed = now.elapsed();
        let frame_time = if elapsed.as_millis() > 0 {
            format!("Frame time={}ms", elapsed.as_millis())
        } else {
            format!("Frame time={}μs", elapsed.as_micros())
        };

        trace!("{}", frame_time);
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
pub fn run(
    initial_tab: Tab,
    config: Config,
    args: Args,
    config_path: Option<std::path::PathBuf>,
    egui_ctx_lock: Arc<OnceLock<egui::Context>>,
    #[cfg(feature = "playback")] is_playback: bool,
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
                initial_tab,
                config,
                args,
                egui_ctx_lock,
                config_path,
                #[cfg(feature = "playback")]
                is_playback,
            )))
        }),
    ) {
        Ok(()) => Ok(()),
        Err(e) => Err(anyhow::anyhow!(e.to_string())),
    }
}
