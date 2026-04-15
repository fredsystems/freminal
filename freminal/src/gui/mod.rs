// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicU64, Ordering},
};
use std::time::Instant;

use crate::gui::colors::internal_color_to_egui_with_alpha;
use anyhow::Result;
use conv2::{ApproxFrom, ConvUtil, ValueFrom};
use crossbeam_channel::{Receiver, Sender};
use eframe::egui::{
    self, CentralPanel, Panel, Pos2, Vec2, ViewportBuilder, ViewportCommand, ViewportId,
};
use eframe::egui_glow::CallbackFn;
use freminal_common::args::Args;
use freminal_common::buffer_states::window_manipulation::WindowManipulation;
use freminal_common::config::{Config, TabBarPosition, ThemeMode};
use freminal_common::pty_write::PtyWrite;
use freminal_terminal_emulator::io::{InputEvent, WindowCommand};
#[cfg(feature = "playback")]
use freminal_terminal_emulator::io::{PlaybackCommand, PlaybackMode};
use freminal_terminal_emulator::snapshot::TerminalSnapshot;
use glow::HasContext;
use renderer::WindowPostRenderer;
use settings::{SettingsAction, SettingsModal};
use tabs::{Tab, TabManager};
use terminal::{FreminalTerminalWidget, new_render_state};

pub mod atlas;
pub mod colors;
pub mod font_manager;
pub mod fonts;
pub mod mouse;
pub mod panes;
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

/// Tracks an in-progress mouse drag on a pane split border.
///
/// Created when the user starts dragging a border sensor rect and
/// cleared when the drag ends. While active, mouse movement deltas
/// are converted to ratio deltas and fed to [`panes::PaneTree::resize_split`].
#[derive(Debug, Clone, Copy)]
struct PaneBorderDrag {
    /// A pane id in the first child of the split being resized.
    /// Used as `target_id` for `resize_split()`.
    target_pane: panes::PaneId,

    /// The direction of the split being resized.
    direction: panes::SplitDirection,

    /// The extent of the parent split node along the split axis,
    /// used to accurately convert pixel drag distance into a ratio delta.
    parent_extent: f32,
}

/// Monotonically increasing counter for generating unique `ViewportId` values
/// for secondary windows. Uses u64 to avoid collisions with egui's own IDs.
static NEXT_VIEWPORT_ID: AtomicU64 = AtomicU64::new(1);

/// State owned exclusively by one secondary OS window.
///
/// Each additional window spawned via `Ctrl+Shift+N` (or the "Window → New
/// Window" menu) gets one of these. Secondary windows share the same
/// `egui::Context`, `Config`, and `BindingMap` as the root window (snapshot-
/// cloned at creation time), but own their own `TabManager`, title tracking,
/// `WindowPostRenderer`, and render state — exactly like a second `FreminalGui`
/// instance without the settings modal, playback controls, or app lifecycle
/// management.
// `closed`, `pending_close_pane`, and `pending_new_window` are distinct boolean
// flags with independent semantics; grouping them would obscure their purpose.
#[allow(clippy::struct_excessive_bools)]
struct SecondaryWindowState {
    /// All open terminal tabs for this window.
    tabs: TabManager,

    /// Last title string sent to the OS window via `ViewportCommand::Title`.
    last_window_title: String,

    /// Cached egui style inputs — prevents redundant `global_style_mut` calls.
    style_cache: Option<(bool, &'static freminal_common::themes::ThemePalette, f32)>,

    /// Set `true` by `ClosePane` action; consumed at the end of the frame.
    pending_close_pane: bool,

    /// Pending directional focus change; consumed at the end of the frame.
    pending_focus_direction: Option<freminal_common::keybindings::KeyAction>,

    /// Active pane border drag state (mouse drag-to-resize).
    border_drag: Option<PaneBorderDrag>,

    /// Lazily initialised on the first frame (needs `egui::Context`).
    /// `None` until the deferred viewport closure runs for the first time.
    terminal_widget: Option<FreminalTerminalWidget>,

    /// Set to `true` when the OS close button is clicked or the window is
    /// programmatically closed.  The root window's pruning loop checks this
    /// flag and stops calling `show_viewport_deferred` for this entry,
    /// causing egui to destroy the OS window and drop the `Arc`, cleaning
    /// up all PTY threads and resources.
    closed: bool,

    /// Set to `true` by the `NewWindow` key action or "Window → New Window"
    /// menu inside this secondary window.  The root window's pruning loop
    /// consumes this flag and sets `self.pending_new_window = true` to
    /// trigger spawning of a new OS window from the root context.
    pending_new_window: bool,

    // ── Shared resources (cloned / Arc'd from the root window) ──────────────
    /// A snapshot of the root window's `Config` at the time this window was
    /// opened.  Updated when the root applies settings changes.
    config: Config,

    /// CLI arguments (needed for `spawn_pty_tab`).
    args: Args,

    /// Binding map used to resolve key combos in this window.
    binding_map: freminal_common::keybindings::BindingMap,

    /// Globally unique pane ID generator — shared so IDs are unique
    /// across all windows in the process.
    pane_id_gen: Arc<Mutex<panes::PaneIdGenerator>>,

    /// Per-window post-processing renderer (FBO + custom shader).
    ///
    /// Each secondary window owns its own `WindowPostRenderer` so that pane
    /// `PaintCallback`s write into this window's FBO — not the root window's.
    /// Sharing the root's renderer would cause FBO corruption when both windows
    /// are visible simultaneously.
    window_post: Arc<Mutex<WindowPostRenderer>>,

    /// Shared egui context handle (same `Arc<OnceLock<>>` as the root).
    /// Used when spawning new PTY tabs so their threads can request repaints.
    egui_ctx: Arc<OnceLock<egui::Context>>,
}

impl SecondaryWindowState {
    /// Returns the terminal widget, initialising it from `ctx` on first call.
    fn terminal_widget(&mut self, ctx: &egui::Context) -> &mut FreminalTerminalWidget {
        self.terminal_widget
            .get_or_insert_with(|| FreminalTerminalWidget::new(ctx, &self.config))
    }
}

// `os_dark_mode`, `pending_close_pane`, and `pending_new_window` are distinct
// boolean flags with independent semantics; grouping them into an enum or
// sub-struct would obscure their purpose without adding clarity.
#[allow(clippy::struct_excessive_bools)]
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

    /// Monotonic generator for `PaneId` values.
    ///
    /// All panes across all tabs and all windows draw from this single generator
    /// so that pane ids are globally unique within the process lifetime.
    /// Wrapped in `Arc<Mutex<>>` so secondary windows can share it.
    pane_id_gen: Arc<Mutex<panes::PaneIdGenerator>>,

    /// Set to `true` by the `ClosePane` key action dispatch; consumed after
    /// the render loop where the `ui` reference is available.
    pending_close_pane: bool,

    /// Set by directional focus key actions; consumed after the render loop
    /// where the pane layout rects are available.
    pending_focus_direction: Option<freminal_common::keybindings::KeyAction>,

    /// Tracks an in-progress mouse drag on a pane split border.
    /// `None` when no border drag is active.
    border_drag: Option<PaneBorderDrag>,

    /// Last modified time of the shader file, used for hot-reload detection.
    /// `None` when no shader is configured or hot-reload is disabled.
    shader_last_mtime: Option<std::time::SystemTime>,

    /// Shared window-level post-processing renderer.
    ///
    /// All panes across all tabs share one `WindowPostRenderer` (via `Arc<Mutex<…>>`).
    /// When a user GLSL shader is active, each pane's `PaintCallback` renders its content
    /// into the shared window FBO.  A single window-level `PaintCallback` registered after
    /// the pane loop applies the post pass to egui's framebuffer.
    window_post: Arc<Mutex<WindowPostRenderer>>,

    /// Secondary OS windows spawned via "New Window".
    ///
    /// Each entry is a `(ViewportId, Arc<Mutex<SecondaryWindowState>>)` pair.
    /// The root window registers each secondary window every frame via
    /// `ctx.show_viewport_deferred`. When a secondary window requests close,
    /// `run_secondary_window_frame` sets `win.closed = true` and the pruning
    /// loop stops re-registering that entry, causing egui to destroy the
    /// viewport and dropping the `Arc` to clean up resources.
    secondary_windows: Vec<(ViewportId, Arc<Mutex<SecondaryWindowState>>)>,

    /// Set to `true` by the `NewWindow` key action or "Window → New Window" menu;
    /// consumed at the start of `ui()` where `egui::Context` is available.
    pending_new_window: bool,

    /// When `true`, the root window has been hidden because the user closed it
    /// while secondary windows were still open.  The root stays alive as a
    /// headless coordinator, re-registering secondary viewports each frame.
    /// When the last secondary window closes, the app exits.
    root_hidden: bool,

    /// Whether this instance is running in playback mode.
    #[cfg(feature = "playback")]
    is_playback: bool,

    /// The playback mode currently selected in the GUI dropdown.
    /// Only meaningful when `is_playback` is true.
    #[cfg(feature = "playback")]
    selected_playback_mode: Option<PlaybackMode>,
}

impl FreminalGui {
    #[allow(clippy::too_many_arguments)] // Constructor naturally needs all initialization params.
    fn new(
        cc: &eframe::CreationContext<'_>,
        initial_tab: Tab,
        config: Config,
        args: Args,
        egui_ctx: Arc<OnceLock<egui::Context>>,
        config_path: Option<std::path::PathBuf>,
        window_post: Arc<Mutex<WindowPostRenderer>>,
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
            // Start at 1: the initial pane (spawned in main.rs) was assigned
            // PaneId(0) = PaneId::first(). All subsequent panes get ids ≥ 1.
            pane_id_gen: Arc::new(Mutex::new(panes::PaneIdGenerator::new(1))),
            pending_close_pane: false,
            pending_focus_direction: None,
            border_drag: None,
            shader_last_mtime: None,
            window_post,
            secondary_windows: Vec::new(),
            pending_new_window: false,
            root_hidden: false,
            #[cfg(feature = "playback")]
            is_playback,
            #[cfg(feature = "playback")]
            selected_playback_mode: None,
        };

        // Inform the initial tab about the configured theme mode and current OS
        // dark/light preference so DECRPM ?2031 responses are correct from the start.
        if let Err(e) =
            gui.tabs
                .active_tab()
                .active_pane()
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
                .active_pane()
                .input_tx
                .send(InputEvent::ThemeChange(theme))
        {
            error!("Failed to send initial ThemeChange to tab: {e}");
        }

        // Apply initial background image and shader from config (if set).
        {
            let initial_bg_path = gui.config.ui.background_image.clone();
            let initial_shader_src: Option<String> =
                gui.config.shader.path.as_ref().and_then(|p| {
                    std::fs::read_to_string(p)
                        .map_err(|e| {
                            error!("Failed to read initial shader file '{}': {e}", p.display());
                        })
                        .ok()
                });
            // Push pending background image to each pane's RenderState.
            if initial_bg_path.is_some() {
                for tab in gui.tabs.iter() {
                    if let Ok(panes) = tab.pane_tree.iter_panes() {
                        for pane in panes {
                            if let Ok(mut rs) = pane.render_state.lock() {
                                rs.set_pending_bg_image(initial_bg_path.clone());
                            }
                        }
                    }
                }
            }
            // Push pending shader to the shared WindowPostRenderer.
            if let Some(src) = initial_shader_src
                && let Ok(mut wpr) = gui.window_post.lock()
            {
                wpr.pending_shader = Some(Some(src));
            }
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
                    self.close_or_hide_root(ui.ctx());
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

            let pane_resp = ui.menu_button("Pane", |ui| {
                self.show_pane_menu(ui);
            });
            if pane_resp.inner.is_some() {
                any_menu_open = true;
            }

            let window_resp = ui.menu_button("Window", |ui| {
                if ui.button("New Window").clicked() {
                    self.pending_new_window = true;
                    ui.close();
                }
            });
            if window_resp.inner.is_some() {
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
    fn show_pane_menu(&mut self, ui: &mut egui::Ui) {
        if ui.button("Split Vertical (Left | Right)").clicked() {
            self.spawn_split_pane(panes::SplitDirection::Horizontal);
            ui.close();
        }
        if ui.button("Split Horizontal (Top / Bottom)").clicked() {
            self.spawn_split_pane(panes::SplitDirection::Vertical);
            ui.close();
        }

        ui.separator();

        let can_close_pane = self.tabs.active_tab().pane_tree.pane_count().unwrap_or(1) > 1;

        if ui
            .add_enabled(can_close_pane, egui::Button::new("Close Pane"))
            .clicked()
        {
            self.pending_close_pane = true;
            ui.close();
        }

        let is_zoomed = self.tabs.active_tab().zoomed_pane.is_some();
        let zoom_label = if is_zoomed {
            "Un-Zoom Pane"
        } else {
            "Zoom Pane"
        };
        let can_zoom = self.tabs.active_tab().pane_tree.pane_count().unwrap_or(1) > 1;

        if ui
            .add_enabled(can_zoom, egui::Button::new(zoom_label))
            .clicked()
        {
            let tab = self.tabs.active_tab_mut();
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
                let pane_id = self
                    .pane_id_gen
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .next_id();
                let pane = panes::Pane {
                    id: pane_id,
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
                    render_state: new_render_state(Arc::clone(&self.window_post)),
                    render_cache: terminal::PaneRenderCache::new(),
                };
                let tab = Tab::new(id, pane);
                // Inform the new tab of the current theme mode so DECRPM
                // ?2031 queries return the correct locked/dynamic status.
                if let Err(e) = tab.active_pane().input_tx.send(InputEvent::ThemeModeUpdate(
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

    /// Spawn a new PTY-backed pane and insert it into the active tab's pane tree,
    /// splitting the currently focused pane.
    ///
    /// The focused pane becomes the `first` child of the new split; the new pane
    /// becomes the `second` child. Focus is transferred to the new pane after
    /// insertion. The split ratio starts at 0.5 (equal halves).
    ///
    /// Does nothing in playback mode (no PTY to spawn).
    // The mutex guard for `pane_id_gen` must stay alive across the `split` call
    // because `id_gen` borrows from it. Clippy cannot see through the borrow and
    // suggests an impossible inline form; suppressed here with justification.
    #[allow(clippy::significant_drop_tightening)]
    fn spawn_split_pane(&mut self, direction: panes::SplitDirection) {
        // Split panes are not supported in playback mode.
        #[cfg(feature = "playback")]
        if self.is_playback {
            return;
        }

        let theme =
            freminal_common::themes::by_slug(self.config.theme.active_slug(self.os_dark_mode))
                .unwrap_or(&freminal_common::themes::CATPPUCCIN_MOCHA);

        // Spawn the new PTY before touching `self.tabs` so there is no borrow conflict.
        let channels = match pty::spawn_pty_tab(
            &self.args,
            self.config.scrollback.limit,
            theme,
            &self.egui_ctx,
        ) {
            Ok(ch) => ch,
            Err(e) => {
                error!("Failed to spawn split pane: {e}");
                return;
            }
        };

        // Read the focused pane id before mutably borrowing the tab.
        let target_id = self.tabs.active_tab().active_pane;

        // Insert the new pane into the tree.
        // The mutex guard is held only for the split call and dropped immediately
        // after so the lock is not contended during the subsequent focus/resize work.
        let new_pane_id = {
            let mut guard = self
                .pane_id_gen
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let id_gen = &mut *guard;
            let tab = self.tabs.active_tab_mut();
            match tab
                .pane_tree
                .split(target_id, direction, id_gen, |new_id| panes::Pane {
                    id: new_id,
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
                    render_state: new_render_state(Arc::clone(&self.window_post)),
                    render_cache: terminal::PaneRenderCache::new(),
                }) {
                Ok(id) => id,
                Err(e) => {
                    error!("Failed to insert split pane into tree: {e}");
                    return;
                }
            }
        };
        let tab = self.tabs.active_tab_mut();

        // Transfer terminal focus from the old pane to the new one so
        // applications that track focus (DEC mode 1004) see the transition.
        if let Some(old_pane) = tab.pane_tree.find(target_id)
            && let Err(e) = old_pane.input_tx.send(InputEvent::FocusChange(false))
        {
            error!("Failed to send FocusChange(false) to previous pane {target_id}: {e}");
        }

        // Move keyboard focus to the newly created pane.
        tab.active_pane = new_pane_id;

        if let Some(new_pane) = tab.pane_tree.find(new_pane_id) {
            if let Err(e) = new_pane.input_tx.send(InputEvent::FocusChange(true)) {
                error!("Failed to send FocusChange(true) to new pane {new_pane_id}: {e}");
            }

            // Notify the new pane of the current theme mode so DECRPM ?2031
            // responses are correct from the start.
            if let Err(e) = new_pane.input_tx.send(InputEvent::ThemeModeUpdate(
                self.config.theme.mode,
                self.os_dark_mode,
            )) {
                error!("Failed to send ThemeModeUpdate to split pane: {e}");
            }

            // Propagate any active background image to the new pane so it
            // renders consistently with existing panes.  The post-process
            // shader is window-level (shared via WindowPostRenderer) and
            // does not need per-pane propagation.
            let new_bg_path = self.config.ui.background_image.clone();
            if new_bg_path.is_some()
                && let Ok(mut rs) = new_pane.render_state.lock()
            {
                rs.set_pending_bg_image(new_bg_path);
            }
        }
    }

    /// Open a new OS window with an initial PTY-backed tab.
    ///
    /// Creates a fresh [`SecondaryWindowState`] with a clone of the current
    /// `Config`, `BindingMap`, and `Args`, a shared `pane_id_gen` and
    /// `window_post`, and a new `TabManager` backed by a fresh PTY. The
    /// window is registered via [`egui::Context::show_viewport_deferred`]
    /// and will appear on the next frame. Does nothing in playback mode.
    fn spawn_new_window(&mut self, ctx: &egui::Context) {
        // New windows are not supported in playback mode.
        #[cfg(feature = "playback")]
        if self.is_playback {
            return;
        }

        let theme =
            freminal_common::themes::by_slug(self.config.theme.active_slug(self.os_dark_mode))
                .unwrap_or(&freminal_common::themes::CATPPUCCIN_MOCHA);

        let channels = match pty::spawn_pty_tab(
            &self.args,
            self.config.scrollback.limit,
            theme,
            &self.egui_ctx,
        ) {
            Ok(ch) => ch,
            Err(e) => {
                error!("Failed to spawn PTY for new window: {e}");
                return;
            }
        };

        let pane_id = self
            .pane_id_gen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .next_id();
        let tab_id = tabs::TabId::first();

        // Create a fresh WindowPostRenderer for this window.  Secondary windows
        // must not share the root window's FBO — concurrent pane callbacks from
        // different OS windows would corrupt each other's framebuffer.
        let win_post: Arc<Mutex<WindowPostRenderer>> =
            Arc::new(Mutex::new(WindowPostRenderer::new()));
        self.copy_root_shader_to(&win_post);

        let pane = panes::Pane {
            id: pane_id,
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
            // Each secondary window gets its own WindowPostRenderer so its pane
            // PaintCallbacks write into this window's FBO — not the root's.
            render_state: new_render_state(Arc::clone(&win_post)),
            render_cache: terminal::PaneRenderCache::new(),
        };
        let initial_tab = Tab::new(tab_id, pane);

        // Push the background image to the initial pane's render state so it
        // loads on the first frame (mirrors what FreminalGui::new() does for
        // the root window's initial panes).
        if self.config.ui.background_image.is_some()
            && let Ok(mut rs) = initial_tab.active_pane().render_state.lock()
        {
            rs.set_pending_bg_image(self.config.ui.background_image.clone());
        }

        // Notify the new pane of the current theme mode.
        if let Err(e) = initial_tab
            .active_pane()
            .input_tx
            .send(InputEvent::ThemeModeUpdate(
                self.config.theme.mode,
                self.os_dark_mode,
            ))
        {
            error!("Failed to send ThemeModeUpdate to new window's initial pane: {e}");
        }

        let state = Arc::new(Mutex::new(SecondaryWindowState {
            tabs: TabManager::new(initial_tab),
            last_window_title: String::from("Freminal"),
            style_cache: None,
            pending_close_pane: false,
            pending_focus_direction: None,
            border_drag: None,
            terminal_widget: None, // lazily initialised on first frame
            closed: false,
            pending_new_window: false,
            config: self.config.clone(),
            args: self.args.clone(),
            binding_map: self.binding_map.clone(),
            pane_id_gen: Arc::clone(&self.pane_id_gen),
            window_post: win_post,
            egui_ctx: Arc::clone(&self.egui_ctx),
        }));

        let viewport_id =
            ViewportId::from_hash_of(NEXT_VIEWPORT_ID.fetch_add(1, Ordering::Relaxed));

        let builder = ViewportBuilder::default()
            .with_title("Freminal")
            .with_app_id("freminal")
            .with_transparent(true);

        // Clone the Arc so the closure owns a strong reference.
        let state_clone = Arc::clone(&state);
        ctx.show_viewport_deferred(viewport_id, builder, move |ui, _class| {
            // Prune happens in the root update loop; inside the closure we
            // only run if the lock succeeds.
            let Ok(mut win) = state_clone.try_lock() else {
                return;
            };
            run_secondary_window_frame(&mut win, ui);
        });

        self.secondary_windows.push((viewport_id, state));
    }

    /// Copy the root window's active/pending shader into a new
    /// `WindowPostRenderer` so it compiles its own copy on the next frame.
    ///
    /// If the root has a pending shader change, that pending value is copied.
    /// Otherwise, if the root already has an active (compiled) shader, the
    /// source is re-read from the config path.
    fn copy_root_shader_to(&self, target: &Arc<Mutex<WindowPostRenderer>>) {
        let root_guard = self
            .window_post
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let shader_src: Option<Option<String>> = root_guard.pending_shader.as_ref().map_or_else(
            || {
                if root_guard.is_active() {
                    // Root shader is already compiled — read the source from disk.
                    self.config.shader.path.as_ref().and_then(|p| {
                        std::fs::read_to_string(p)
                            .map_err(|e| {
                                error!(
                                    "Failed to read shader for new window from '{}': {e}",
                                    p.display()
                                );
                            })
                            .ok()
                            .map(Some)
                    })
                } else {
                    None
                }
            },
            |pending| Some(pending.clone()),
        );
        drop(root_guard);
        if let Some(src) = shader_src {
            target
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .pending_shader = Some(src);
        }
    }

    /// Close or hide the root window.
    ///
    /// If secondary windows are still open, the root is hidden instead of
    /// destroyed — eframe ties app lifetime to `ViewportId::ROOT`, so
    /// destroying it would kill the entire process (including all secondary
    /// windows).  When no secondaries exist the window closes normally.
    fn close_or_hide_root(&mut self, ctx: &egui::Context) {
        if self.secondary_windows.is_empty() {
            ctx.send_viewport_cmd(ViewportCommand::Close);
        } else {
            // We cannot destroy ViewportId::ROOT while secondary viewports
            // are alive — eframe ties the application lifecycle to the root,
            // so closing it would kill the entire process.  Minimize the root
            // instead so it gets out of the user's way.
            ctx.send_viewport_cmd(ViewportCommand::Minimized(true));
            self.root_hidden = true;
        }
    }

    /// Close the focused pane in the active tab.
    ///
    /// If the pane is the last one in its tab, the tab itself is closed.
    /// If the tab is the last tab, the application exits.
    /// Otherwise, focus transfers to a sibling pane.
    fn close_focused_pane(&mut self, ui: &egui::Ui) {
        let tab = self.tabs.active_tab_mut();
        let target = tab.active_pane;

        // Cancel zoom if the zoomed pane is being closed.
        if tab.zoomed_pane == Some(target) {
            tab.zoomed_pane = None;
        }

        match tab.pane_tree.close(target) {
            Ok(_closed) => {
                // Focus transfers to the first pane remaining in the tree.
                // Reset last_sent_size on all surviving panes so the next
                // frame's resize check fires — the layout rects change after
                // a close and the PTY must learn the new dimensions.
                let tab = self.tabs.active_tab_mut();
                if let Ok(panes) = tab.pane_tree.iter_panes_mut() {
                    for pane in panes {
                        pane.view_state.last_sent_size = (0, 0);
                    }
                }
                let tab = self.tabs.active_tab_mut();
                if let Ok(panes) = tab.pane_tree.iter_panes()
                    && let Some(first) = panes.first()
                {
                    let new_id = first.id;
                    // Notify the new active pane that it gained focus so
                    // applications tracking DEC mode 1004 see the transition.
                    if let Err(e) = first.input_tx.send(InputEvent::FocusChange(true)) {
                        error!("Failed to send FocusChange(true) to pane {new_id}: {e}");
                    }
                    tab.active_pane = new_id;
                }
            }
            Err(panes::PaneError::CannotCloseLastPane) => {
                // Last pane in tab — close the tab instead.
                if self.tabs.tab_count() <= 1 {
                    self.close_or_hide_root(ui.ctx());
                    return;
                }
                let idx = self.tabs.active_index();
                self.close_tab(idx);
            }
            Err(e) => {
                error!("Failed to close pane {target}: {e}");
            }
        }
    }

    /// Find the nearest pane in the given direction relative to the focused
    /// pane and transfer focus to it.
    ///
    /// Uses the center-point of each pane's layout rect to determine spatial
    /// relationships. "Left" means the candidate's center X is less than the
    /// current pane's center X, etc.
    fn focus_pane_in_direction(
        &mut self,
        direction: freminal_common::keybindings::KeyAction,
        available_rect: egui::Rect,
    ) {
        use freminal_common::keybindings::KeyAction;

        let tab = self.tabs.active_tab();

        // Focus navigation is a no-op while a pane is zoomed — there is only
        // one visible pane so changing active_pane would desync the GUI.
        if tab.zoomed_pane.is_some() {
            return;
        }

        let current_id = tab.active_pane;

        let layout = match tab.pane_tree.layout(available_rect) {
            Ok(l) => l,
            Err(e) => {
                error!("Failed to compute pane layout for navigation: {e}");
                return;
            }
        };

        // Find the current pane's rect.
        let Some(current_rect) = layout
            .iter()
            .find(|(id, _)| *id == current_id)
            .map(|(_, r)| *r)
        else {
            return;
        };
        let current_center = current_rect.center();

        // Filter candidates by direction and pick the closest.
        let best = layout
            .iter()
            .filter(|(id, _)| *id != current_id)
            .filter(|(_, rect)| {
                let c = rect.center();
                match direction {
                    KeyAction::FocusPaneLeft => c.x < current_center.x,
                    KeyAction::FocusPaneRight => c.x > current_center.x,
                    KeyAction::FocusPaneUp => c.y < current_center.y,
                    KeyAction::FocusPaneDown => c.y > current_center.y,
                    _ => false,
                }
            })
            .min_by(|(_, a), (_, b)| {
                let dist_a = a.center().distance(current_center);
                let dist_b = b.center().distance(current_center);
                dist_a
                    .partial_cmp(&dist_b)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });

        if let Some((new_id, _)) = best {
            let new_id = *new_id;
            let tab = self.tabs.active_tab_mut();
            let old_id = tab.active_pane;

            // Notify the old pane it lost focus.
            if let Some(old_pane) = tab.pane_tree.find(old_id)
                && let Err(e) = old_pane.input_tx.send(InputEvent::FocusChange(false))
            {
                error!("Failed to send FocusChange(false) to pane {old_id}: {e}");
            }

            tab.active_pane = new_id;

            // Notify the new pane it gained focus.
            if let Some(new_pane) = tab.pane_tree.find(new_id)
                && let Err(e) = new_pane.input_tx.send(InputEvent::FocusChange(true))
            {
                error!("Failed to send FocusChange(true) to pane {new_id}: {e}");
            }
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
                    self.tabs.active_tab_mut().active_pane_mut().bell_active = false;
                }
            }
            TabBarAction::Close(i) => self.close_tab(i),
            TabBarAction::None => {}
        }
    }

    // Inherently large: routes all key actions that require full GUI state.
    // Each arm is a distinct GUI operation; extracting further would add
    // indirection without improving clarity.
    #[allow(clippy::too_many_lines)]
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
                self.tabs.active_tab_mut().active_pane_mut().bell_active = false;
            }
            KeyAction::PrevTab => {
                self.tabs.prev_tab();
                self.tabs.active_tab_mut().active_pane_mut().bell_active = false;
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
                self.tabs
                    .active_tab_mut()
                    .active_pane_mut()
                    .view_state
                    .reset_zoom();
                self.terminal_widget.apply_font_zoom(self.config.font.size);
                // Zoom reset may change font size — invalidate all panes.
                self.invalidate_all_pane_atlases();
            }

            // -- Search overlay --
            KeyAction::OpenSearch => {
                self.tabs
                    .active_tab_mut()
                    .active_pane_mut()
                    .view_state
                    .search_state
                    .is_open = true;
            }
            KeyAction::SearchNext => {
                let tab = self.tabs.active_tab_mut();
                let pane = tab.active_pane_mut();
                pane.view_state.search_state.next_match();
                let snap = pane.arc_swap.load();
                search::scroll_to_match_and_send(&mut pane.view_state, &snap, &pane.input_tx);
            }
            KeyAction::SearchPrev => {
                let tab = self.tabs.active_tab_mut();
                let pane = tab.active_pane_mut();
                pane.view_state.search_state.prev_match();
                let snap = pane.arc_swap.load();
                search::scroll_to_match_and_send(&mut pane.view_state, &snap, &pane.input_tx);
            }
            KeyAction::PrevCommand => {
                let tab = self.tabs.active_tab_mut();
                let pane = tab.active_pane_mut();
                let snap = pane.arc_swap.load();
                search::jump_to_prev_command(&mut pane.view_state, &snap);
            }
            KeyAction::NextCommand => {
                let tab = self.tabs.active_tab_mut();
                let pane = tab.active_pane_mut();
                let snap = pane.arc_swap.load();
                search::jump_to_next_command(&mut pane.view_state, &snap);
            }

            // -- Window management --
            KeyAction::NewWindow => {
                // Deferred: needs egui::Context, which is not available here.
                // Set a flag; consumed at the top of ui().
                self.pending_new_window = true;
            }

            // -- Not yet implemented --
            // Consumed (not forwarded to PTY) but silently ignored until
            // their respective features land.
            KeyAction::RenameTab => {
                trace!("Unhandled deferred key action: {action:?}");
            }

            // -- Pane management --
            KeyAction::SplitVertical => {
                self.spawn_split_pane(panes::SplitDirection::Horizontal);
            }
            KeyAction::SplitHorizontal => {
                self.spawn_split_pane(panes::SplitDirection::Vertical);
            }
            KeyAction::ClosePane => {
                // Deferred — needs `ui` reference. Stored and handled after
                // the render loop. See the `deferred_close_pane` handling below.
                // For now we call the helper directly since we already have
                // `&mut self`. The `ui` reference is not available here, so
                // we handle last-pane-in-last-tab by checking upfront.
                //
                // We cannot access `ui` from dispatch_deferred_action, so the
                // "close app" path uses a flag instead.
                self.pending_close_pane = true;
            }
            KeyAction::FocusPaneLeft
            | KeyAction::FocusPaneDown
            | KeyAction::FocusPaneUp
            | KeyAction::FocusPaneRight => {
                self.pending_focus_direction = Some(action);
            }
            KeyAction::ResizePaneLeft => {
                let id = self.tabs.active_tab().active_pane;
                // Left resize = shrink horizontal split ratio (move divider left).
                if let Err(e) = self.tabs.active_tab_mut().pane_tree.resize_split(
                    id,
                    panes::SplitDirection::Horizontal,
                    -0.05,
                ) {
                    trace!("Cannot resize pane left: {e}");
                }
            }
            KeyAction::ResizePaneRight => {
                let id = self.tabs.active_tab().active_pane;
                if let Err(e) = self.tabs.active_tab_mut().pane_tree.resize_split(
                    id,
                    panes::SplitDirection::Horizontal,
                    0.05,
                ) {
                    trace!("Cannot resize pane right: {e}");
                }
            }
            KeyAction::ResizePaneUp => {
                let id = self.tabs.active_tab().active_pane;
                if let Err(e) = self.tabs.active_tab_mut().pane_tree.resize_split(
                    id,
                    panes::SplitDirection::Vertical,
                    -0.05,
                ) {
                    trace!("Cannot resize pane up: {e}");
                }
            }
            KeyAction::ResizePaneDown => {
                let id = self.tabs.active_tab().active_pane;
                if let Err(e) = self.tabs.active_tab_mut().pane_tree.resize_split(
                    id,
                    panes::SplitDirection::Vertical,
                    0.05,
                ) {
                    trace!("Cannot resize pane down: {e}");
                }
            }
            KeyAction::ZoomPane => {
                let tab = self.tabs.active_tab_mut();
                let current = tab.active_pane;
                if tab.zoomed_pane == Some(current) {
                    // Un-zoom
                    tab.zoomed_pane = None;
                } else {
                    tab.zoomed_pane = Some(current);
                }
                // Zoom/unzoom changes the effective layout dimensions.
                // Reset last_sent_size on all panes so the resize check
                // fires on the next frame with the correct sizes.
                let tab = self.tabs.active_tab_mut();
                if let Ok(panes) = tab.pane_tree.iter_panes_mut() {
                    for pane in panes {
                        pane.view_state.last_sent_size = (0, 0);
                    }
                }
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
        let vs = &mut self.tabs.active_tab_mut().active_pane_mut().view_state;
        vs.adjust_zoom(base, delta);
        let effective = vs.effective_font_size(base);
        self.terminal_widget.apply_font_zoom(effective);
        // Font size changed — invalidate all panes so atlases are rebuilt.
        self.invalidate_all_pane_atlases();
    }

    /// Clear every pane's GL glyph atlas and dirty-tracking cache.
    ///
    /// Called when the font, font size, ligature config, or pixels-per-point
    /// changes, so that all panes rebuild their vertex buffers and
    /// re-rasterise glyphs at the new metrics.
    fn invalidate_all_pane_atlases(&mut self) {
        for tab in self.tabs.iter_mut() {
            if let Ok(panes) = tab.pane_tree.iter_panes_mut() {
                for pane in panes {
                    pane.render_state
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .clear_atlas();
                    pane.render_cache.invalidate_content();
                }
            }
        }
    }

    /// Push a `pending_shader` change to every secondary window's
    /// `WindowPostRenderer`.
    ///
    /// Called from hot-reload and settings-apply so that secondary windows
    /// recompile the same shader as the root.
    ///
    /// `shader` is `None` to clear the shader, or `Some(source)` to set it.
    fn propagate_shader_to_secondary_windows(&self, shader: Option<&String>) {
        let pending = shader.cloned();
        for (_vid, state) in &self.secondary_windows {
            if let Ok(win) = state.try_lock()
                && let Ok(mut wpr) = win.window_post.lock()
            {
                wpr.pending_shader = Some(pending.clone());
            }
        }
    }

    /// Push a background image change to every pane in every secondary window.
    ///
    /// Called from settings-apply so secondary windows render the same
    /// background image as the root.
    fn propagate_bg_image_to_secondary_windows(&self, path: Option<&std::path::PathBuf>) {
        let owned = path.cloned();
        for (_vid, state) in &self.secondary_windows {
            if let Ok(win) = state.try_lock() {
                for tab in win.tabs.iter() {
                    if let Ok(panes) = tab.pane_tree.iter_panes() {
                        for pane in panes {
                            if let Ok(mut rs) = pane.render_state.lock() {
                                rs.set_pending_bg_image(owned.clone());
                            }
                        }
                    }
                }
            }
        }
    }

    /// Switch to tab N (0-indexed). Silently does nothing if the index
    /// is out of bounds (e.g. user presses Ctrl+Shift+5 with only 3 tabs).
    fn switch_to_tab_n(&mut self, index: usize) {
        if let Err(e) = self.tabs.switch_to(index) {
            trace!("Cannot switch to tab {index}: {e}");
        } else {
            self.tabs.active_tab_mut().active_pane_mut().bell_active = false;
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
            .active_pane()
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

        // ── Root window close intercept ───────────────────────────────────────
        // When the user closes the root window while secondary windows exist,
        // hide the root instead of destroying it — eframe ties app lifecycle to
        // ViewportId::ROOT so we cannot let it close.  The root stays alive as
        // a headless coordinator until all secondaries are gone.
        let close_requested = ui.ctx().input(|i| i.viewport().close_requested());
        if close_requested && !self.secondary_windows.is_empty() {
            // Cancel the OS close and hide the window instead.
            ui.ctx().send_viewport_cmd(ViewportCommand::CancelClose);
            ui.ctx().send_viewport_cmd(ViewportCommand::Visible(false));
            self.root_hidden = true;
        }

        // ── Secondary window management ───────────────────────────────────────
        // Re-register all live secondary windows on every frame.  egui's deferred
        // viewport API requires the closure to be supplied every frame; stopping
        // means the window is destroyed.  We also prune entries whose windows have
        // requested close (detected inside run_secondary_window_frame).
        {
            let ctx = ui.ctx().clone();
            let mut live: Vec<(ViewportId, Arc<Mutex<SecondaryWindowState>>)> = Vec::new();
            for (vid, state) in self.secondary_windows.drain(..) {
                // Check if the window has requested to be closed.  When `closed`
                // is true we stop calling `show_viewport_deferred` for this entry.
                // egui destroys the OS window because the closure is no longer
                // supplied, and the `Arc` is dropped here, cleaning up PTY threads.
                let is_closed = state.try_lock().is_ok_and(|w| w.closed);
                if is_closed {
                    // Do NOT re-register — egui will destroy the viewport.
                    continue;
                }

                let state_clone = Arc::clone(&state);
                ctx.show_viewport_deferred(
                    vid,
                    ViewportBuilder::default()
                        .with_title("Freminal")
                        .with_app_id("freminal")
                        .with_transparent(true),
                    move |ui, _class| {
                        let Ok(mut win) = state_clone.try_lock() else {
                            return;
                        };
                        run_secondary_window_frame(&mut win, ui);
                    },
                );
                live.push((vid, state));
            }
            self.secondary_windows = live;
        }

        // Collect `pending_new_window` requests from secondary windows and
        // consume them into the root's own flag.  We do this with a single
        // `any()` scan to avoid holding a lock while mutating `self`.
        let any_secondary_wants_new_window = self.secondary_windows.iter().any(|(_, state)| {
            state.try_lock().is_ok_and(|mut w| {
                let v = w.pending_new_window;
                if v {
                    w.pending_new_window = false;
                }
                v
            })
        });
        if any_secondary_wants_new_window {
            self.pending_new_window = true;
        }

        // Consume the pending_new_window flag set by dispatch_deferred_action or
        // the "Window → New Window" menu.  Must happen here where ctx is available.
        if self.pending_new_window {
            self.pending_new_window = false;
            self.spawn_new_window(ui.ctx());
        }

        // ── Root-hidden coordinator mode ──────────────────────────────────────
        // When the root window is hidden (user closed it while secondaries were
        // open), skip all root-window rendering.  Just manage secondary windows
        // and exit once they're all gone.
        if self.root_hidden {
            if self.secondary_windows.is_empty() {
                // All secondary windows have closed — exit the app.
                ui.ctx().send_viewport_cmd(ViewportCommand::Close);
            } else {
                // Keep the hidden root alive — repaint periodically to service
                // the secondary window pruning loop above.
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(100));
            }
            return;
        }

        // Detect OS dark/light preference changes and auto-switch theme when
        // `mode = "auto"` is configured.
        let current_os_dark = ui.ctx().global_style().visuals.dark_mode;
        if current_os_dark != self.os_dark_mode {
            self.os_dark_mode = current_os_dark;

            // Only auto-switch when the user has opted in.
            // Always propagate the updated OS preference so DECRPM ?2031
            // reflects the new dark/light state, regardless of ThemeMode.
            for tab in self.tabs.iter() {
                if let Ok(panes) = tab.pane_tree.iter_panes() {
                    for pane in panes {
                        if let Err(e) = pane.input_tx.send(InputEvent::ThemeModeUpdate(
                            self.config.theme.mode,
                            self.os_dark_mode,
                        )) {
                            error!("Failed to send ThemeModeUpdate on OS change to pane: {e}");
                        }
                    }
                }
            }

            if self.config.theme.mode == ThemeMode::Auto {
                let slug = self.config.theme.active_slug(self.os_dark_mode);
                if let Some(theme) = freminal_common::themes::by_slug(slug) {
                    // Notify every pane in every tab so all PTY threads get the new palette.
                    for tab in self.tabs.iter() {
                        if let Ok(panes) = tab.pane_tree.iter_panes() {
                            for pane in panes {
                                if let Err(e) = pane.input_tx.send(
                                    freminal_terminal_emulator::io::InputEvent::ThemeChange(theme),
                                ) {
                                    error!("Failed to send auto ThemeChange to pane: {e}");
                                }
                            }
                        }
                    }
                    update_egui_theme(ui.ctx(), theme, self.config.ui.background_opacity);
                    // Invalidate theme cache on all panes in all tabs so the
                    // next frame forces a full vertex rebuild with the new palette.
                    for tab in self.tabs.iter_mut() {
                        if let Ok(panes) = tab.pane_tree.iter_panes_mut() {
                            for pane in panes {
                                pane.render_cache.invalidate_theme_cache();
                            }
                        }
                    }
                }
            }
        }

        // ── Shader hot-reload ─────────────────────────────────────────────────
        // When hot_reload is enabled and a shader file is configured, check the
        // file's mtime each frame and push a recompile to all panes if it changed.
        if self.config.shader.hot_reload
            && let Some(ref shader_path) = self.config.shader.path.clone()
        {
            let new_mtime = std::fs::metadata(shader_path)
                .and_then(|m| m.modified())
                .ok();
            let changed = match (new_mtime, self.shader_last_mtime) {
                (Some(new), Some(prev)) => new != prev,
                (Some(_), None) => true,
                _ => false,
            };
            if changed {
                self.shader_last_mtime = new_mtime;
                match std::fs::read_to_string(shader_path) {
                    Ok(src) => {
                        if let Ok(mut wpr) = self.window_post.lock() {
                            wpr.pending_shader = Some(Some(src.clone()));
                        }
                        self.propagate_shader_to_secondary_windows(Some(&src));
                    }
                    Err(e) => {
                        error!(
                            "Shader hot-reload: failed to read '{}': {e}",
                            shader_path.display()
                        );
                    }
                }
            }
        }

        // Poll all tabs for PTY death signals.  When a pane's PTY dies,
        // close that pane.  If it was the last pane in the tab, close the
        // tab.  If it was the last tab, close the application.
        //
        // Collect (tab_index, pane_id) pairs for dead panes, then process
        // them in reverse order to avoid index shifting issues.
        let mut dead_panes: Vec<(usize, panes::PaneId)> = Vec::new();
        for (tab_idx, tab) in self.tabs.iter().enumerate() {
            if let Ok(panes) = tab.pane_tree.iter_panes() {
                for pane in panes {
                    if pane.pty_dead_rx.try_recv().is_ok() {
                        dead_panes.push((tab_idx, pane.id));
                    }
                }
            }
        }

        for (tab_idx, pane_id) in dead_panes.into_iter().rev() {
            // Try to close just the dead pane within its tab.
            let is_active_tab = tab_idx == self.tabs.active_index();

            // Switch to the dead pane's tab temporarily if needed so we can
            // operate on it.
            if !is_active_tab && let Err(e) = self.tabs.switch_to(tab_idx) {
                error!("Failed to switch to tab {tab_idx} for dead pane cleanup: {e}");
                continue;
            }

            let tab = self.tabs.active_tab_mut();
            // If the dead pane was the zoomed pane, un-zoom first.
            if tab.zoomed_pane == Some(pane_id) {
                tab.zoomed_pane = None;
            }

            match tab.pane_tree.close(pane_id) {
                Ok(_closed) => {
                    // Reset last_sent_size on all surviving panes so the
                    // next frame's resize check fires with the new layout.
                    let tab = self.tabs.active_tab_mut();
                    if let Ok(panes) = tab.pane_tree.iter_panes_mut() {
                        for pane in panes {
                            pane.view_state.last_sent_size = (0, 0);
                        }
                    }
                    // If the active pane was the one that died, pick a new active pane
                    // and notify it that it gained focus.
                    let tab = self.tabs.active_tab_mut();
                    if tab.active_pane == pane_id
                        && let Ok(panes) = tab.pane_tree.iter_panes()
                        && let Some(first) = panes.first()
                    {
                        let new_id = first.id;
                        if let Err(e) = first.input_tx.send(InputEvent::FocusChange(true)) {
                            error!("Failed to send FocusChange(true) to pane {new_id}: {e}");
                        }
                        tab.active_pane = new_id;
                    }
                }
                Err(panes::PaneError::CannotCloseLastPane) => {
                    // Last pane in tab — close the entire tab.
                    if self.tabs.tab_count() <= 1 {
                        self.close_or_hide_root(ui.ctx());
                        return;
                    }
                    self.close_tab(tab_idx);
                }
                Err(e) => {
                    error!("Failed to close dead pane {pane_id}: {e}");
                }
            }

            // Restore the original active tab if we switched away.
            if !is_active_tab {
                // The tab we were on may have been removed, so saturate.
                let restore_idx = tab_idx.min(self.tabs.tab_count().saturating_sub(1));
                let _ = self.tabs.switch_to(restore_idx);
            }
        }

        // Load the latest snapshot from the PTY thread — no lock, single atomic load.
        let snap = self.tabs.active_tab().active_pane().arc_swap.load();

        // Sync the GUI's scroll offset from the snapshot.  When new PTY output
        // arrives the PTY thread resets its offset to 0, so the snapshot will
        // carry scroll_offset = 0 even if the GUI previously sent a non-zero
        // value.  Adopting the snapshot's value keeps ViewState in sync.
        if self
            .tabs
            .active_tab()
            .active_pane()
            .view_state
            .scroll_offset
            != snap.scroll_offset
        {
            self.tabs
                .active_tab_mut()
                .active_pane_mut()
                .view_state
                .scroll_offset = snap.scroll_offset;
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
            let ppp_changed = self.terminal_widget.sync_pixels_per_point(ppp);

            // Synchronise font zoom for the active tab.  Each tab has its own
            // zoom_delta and the font manager only knows one size at a time.
            // This check fires on every frame but is a single float comparison
            // when no change is needed.
            let effective = self
                .tabs
                .active_tab()
                .active_pane()
                .view_state
                .effective_font_size(self.config.font.size);
            let zoom_changed = self.terminal_widget.apply_font_zoom(effective);

            // When pixels-per-point or font zoom changes, every pane's GL
            // atlas and cached content must be invalidated so glyphs are
            // re-rasterised at the new size.
            if ppp_changed || zoom_changed {
                self.invalidate_all_pane_atlases();
            }

            // Compute char size once — shared across all panes since all panes
            // use the same font at the same size.
            // `cell_size()` returns integer pixel dimensions (physical) from swash
            // font metrics.  egui's coordinate system uses logical points, so we
            // convert with `pixels_per_point` when doing layout math.
            let (cell_w_u, cell_height_u) = self.terminal_widget.cell_size();
            let font_width = usize::value_from(cell_w_u).unwrap_or(0);
            let font_height = usize::value_from(cell_height_u).unwrap_or(0);
            let logical_char_w = f32::approx_from(cell_w_u).unwrap_or(0.0) / ppp;
            let logical_char_h = f32::approx_from(cell_height_u).unwrap_or(0.0) / ppp;

            let window_width = ui.input(|i: &egui::InputState| i.content_rect());

            // Drain window commands for ALL tabs and ALL panes within each tab.
            // The active tab's active pane gets full handling (viewport commands,
            // reports, title updates, clipboard). All other panes get reports
            // answered, titles updated, and clipboard handled — only
            // viewport-mutating commands (resize, move, minimize, fullscreen)
            // are discarded since a non-active pane must not alter the shared
            // window geometry.
            let active_idx = self.tabs.active_index();
            let active_pane_id_for_drain = self.tabs.active_tab().active_pane;
            let window_focused = self
                .tabs
                .active_tab()
                .active_pane()
                .view_state
                .window_focused;
            for (idx, tab) in self.tabs.iter_mut().enumerate() {
                let is_active_tab = idx == active_idx;
                if let Ok(panes) = tab.pane_tree.iter_panes_mut() {
                    for pane in panes {
                        let is_fully_active = is_active_tab && pane.id == active_pane_id_for_drain;
                        handle_window_manipulation(
                            ui,
                            &pane.window_cmd_rx,
                            &pane.pty_write_tx,
                            font_width,
                            font_height,
                            window_width,
                            &mut pane.title_stack,
                            &mut pane.title,
                            &mut pane.bell_active,
                            &mut pane.view_state.bell_since,
                            self.config.bell.mode,
                            self.config.security.allow_clipboard_read,
                            is_fully_active,
                            window_focused,
                        );
                    }
                }
            }

            // Update background color based on the active pane's display mode.
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

            // ── Multi-pane rendering loop ────────────────────────────
            //
            // Compute layout rects for every leaf pane in the active tab's
            // pane tree, then render each one into its allocated rect.
            // Collect deferred key actions from all panes for dispatch after
            // the loop.

            let available_rect = ui.available_rect_before_wrap();
            let active_pane_id = self.tabs.active_tab().active_pane;
            let zoomed_pane = self.tabs.active_tab().zoomed_pane;
            let has_multiple_panes = self.tabs.active_tab().pane_tree.pane_count().unwrap_or(1) > 1;

            // When a pane is zoomed, render only that pane at full size.
            // Borders are hidden during zoom since there is only one visible pane.
            let (pane_layout, border_width) = if let Some(zoomed_id) = zoomed_pane {
                (vec![(zoomed_id, available_rect)], 0.0)
            } else {
                // Width of the border drawn between adjacent panes (logical pixels).
                let bw: f32 = if has_multiple_panes { 1.0 } else { 0.0 };
                let layout = self
                    .tabs
                    .active_tab()
                    .pane_tree
                    .layout(available_rect)
                    .unwrap_or_default();
                (layout, bw)
            };

            let mut all_deferred_actions = Vec::new();

            // Track repaint needs across all panes.
            let mut shortest_repaint_delay: Option<std::time::Duration> = None;

            let ui_overlay_open = self.settings_modal.is_open || any_menu_open;

            // ── Pane border drag-to-resize ───────────────────────────
            //
            // Before rendering panes, place invisible drag sensors on each
            // split border. This must happen before the per-pane
            // `scope_builder` calls so that pointer events on the border
            // are consumed here instead of reaching the terminal widgets.
            if has_multiple_panes && zoomed_pane.is_none() && !ui_overlay_open {
                let borders = self
                    .tabs
                    .active_tab()
                    .pane_tree
                    .split_borders(available_rect, active_pane_id)
                    .unwrap_or_default();

                // Half-width of the invisible drag sensor zone (pixels
                // on each side of the 1px border line).
                let sensor_half: f32 = 3.0;

                for (border_idx, border) in borders.iter().enumerate() {
                    // Expand the thin 1px border rect into a wider sensor rect.
                    let sensor_rect = match border.direction {
                        panes::SplitDirection::Horizontal => {
                            // Vertical divider — expand horizontally.
                            let cx = border.rect.center().x;
                            egui::Rect::from_min_max(
                                egui::pos2(cx - sensor_half, border.rect.min.y),
                                egui::pos2(cx + sensor_half, border.rect.max.y),
                            )
                        }
                        panes::SplitDirection::Vertical => {
                            // Horizontal divider — expand vertically.
                            let cy = border.rect.center().y;
                            egui::Rect::from_min_max(
                                egui::pos2(border.rect.min.x, cy - sensor_half),
                                egui::pos2(border.rect.max.x, cy + sensor_half),
                            )
                        }
                    };

                    let sensor_id = ui.id().with("pane_border_sensor").with(border_idx);
                    let response =
                        ui.interact(sensor_rect, sensor_id, egui::Sense::click_and_drag());

                    // Change cursor when hovering or dragging a border.
                    if response.hovered() || response.dragged() {
                        let cursor = match border.direction {
                            panes::SplitDirection::Horizontal => egui::CursorIcon::ResizeHorizontal,
                            panes::SplitDirection::Vertical => egui::CursorIcon::ResizeVertical,
                        };
                        ui.ctx().set_cursor_icon(cursor);
                    }

                    // On drag start, record which border we're resizing.
                    if response.drag_started() {
                        self.border_drag = Some(PaneBorderDrag {
                            target_pane: border.first_child_pane,
                            direction: border.direction,
                            parent_extent: border.parent_extent,
                        });
                    }

                    // While dragging, convert pixel delta to ratio delta.
                    if response.dragged()
                        && let Some(drag) = &self.border_drag
                    {
                        let delta_px = match drag.direction {
                            panes::SplitDirection::Horizontal => response.drag_delta().x,
                            panes::SplitDirection::Vertical => response.drag_delta().y,
                        };

                        // Convert pixel delta to ratio delta based on
                        // the dragged split parent's extent along the split axis.
                        let total_px = drag.parent_extent;

                        if total_px > 0.0 {
                            let delta_ratio = delta_px / total_px;
                            if let Err(e) = self.tabs.active_tab_mut().pane_tree.resize_split(
                                drag.target_pane,
                                drag.direction,
                                delta_ratio,
                            ) {
                                debug!("Border resize failed: {e}");
                            }
                        }
                    }

                    // Clear drag state when drag ends.
                    if response.drag_stopped() {
                        self.border_drag = None;
                    }
                }
            }

            // ── Pre-clear the window post-processing FBO ──────────
            //
            // When a user GLSL shader is active (or about to become active),
            // all panes render into a shared window FBO.  We clear it once
            // per frame here, before any pane draws into it, so stale content
            // from the previous frame does not bleed through.
            //
            // We also schedule the pre-clear when `pending_shader` is set so
            // that the very first frame after a shader is enabled already has
            // the FBO ready for pane callbacks.  The `ensure_fbo` call inside
            // the callback creates the FBO on-demand if it doesn't exist yet.
            {
                let wpr_guard = self
                    .window_post
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let wpr_active = wpr_guard.is_active();
                let shader_activation_pending = wpr_guard.pending_shader.is_some();
                drop(wpr_guard);

                if wpr_active || shader_activation_pending {
                    let wpr_for_clear = Arc::clone(&self.window_post);
                    ui.painter().add(egui::PaintCallback {
                        rect: available_rect,
                        callback: Arc::new(CallbackFn::new(move |info, painter| {
                            let gl = painter.gl();
                            let vp = info.viewport_in_pixels();
                            let mut wpr = wpr_for_clear
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner);
                            wpr.ensure_fbo(gl, vp.width_px, vp.height_px);
                            if let Some(fbo) = wpr.fbo() {
                                unsafe {
                                    gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
                                    gl.clear_color(0.0, 0.0, 0.0, 0.0);
                                    gl.clear(glow::COLOR_BUFFER_BIT);
                                    // Restore egui's FBO.
                                    gl.bind_framebuffer(
                                        glow::FRAMEBUFFER,
                                        painter.intermediate_fbo(),
                                    );
                                }
                            }
                        })),
                    });
                }
            }

            for (pane_id, pane_rect) in &pane_layout {
                // Shrink the pane rect slightly to leave room for borders.
                // Each pane edge that is interior (shared with another pane)
                // gives up half the border width so the total gap equals
                // `border_width`.
                let content_rect = if has_multiple_panes {
                    let half = border_width / 2.0;
                    let shrink_left = if pane_rect.min.x > available_rect.min.x {
                        half
                    } else {
                        0.0
                    };
                    let shrink_right = if pane_rect.max.x < available_rect.max.x {
                        half
                    } else {
                        0.0
                    };
                    let shrink_top = if pane_rect.min.y > available_rect.min.y {
                        half
                    } else {
                        0.0
                    };
                    let shrink_bottom = if pane_rect.max.y < available_rect.max.y {
                        half
                    } else {
                        0.0
                    };
                    egui::Rect::from_min_max(
                        egui::pos2(pane_rect.min.x + shrink_left, pane_rect.min.y + shrink_top),
                        egui::pos2(
                            pane_rect.max.x - shrink_right,
                            pane_rect.max.y - shrink_bottom,
                        ),
                    )
                } else {
                    *pane_rect
                };

                // Per-pane character dimensions from this pane's content rect.
                let pane_width_chars = (content_rect.width() / logical_char_w)
                    .floor()
                    .approx_as::<usize>()
                    .unwrap_or_else(|e| {
                        error!("Failed to calculate pane width chars: {e}");
                        10
                    });
                let pane_height_chars = (content_rect.height() / logical_char_h)
                    .floor()
                    .approx_as::<usize>()
                    .unwrap_or_else(|e| {
                        error!("Failed to calculate pane height chars: {e}");
                        10
                    })
                    .max(1);

                // Look up the pane mutably for resize + render.
                let pane_id = *pane_id;
                let tab = self.tabs.active_tab_mut();
                let Some(pane) = tab.pane_tree.find_mut(pane_id) else {
                    // Should never happen — layout returned this id.
                    error!("Pane {pane_id} not found in tree during render");
                    continue;
                };

                // Debounced resize: only send when char dims changed.
                let new_size = (pane_width_chars, pane_height_chars);
                if new_size != pane.view_state.last_sent_size {
                    if let Err(e) = pane.input_tx.send(InputEvent::Resize(
                        pane_width_chars,
                        pane_height_chars,
                        font_width,
                        font_height,
                    )) {
                        error!("Failed to send resize event for {pane_id}: {e}");
                    } else {
                        pane.view_state.last_sent_size = new_size;
                    }
                }

                // Load this pane's snapshot and sync scroll offset.
                let pane_snap = pane.arc_swap.load();
                if pane.view_state.scroll_offset != pane_snap.scroll_offset {
                    pane.view_state.scroll_offset = pane_snap.scroll_offset;
                }

                let is_echo_off = self.config.security.password_indicator
                    && pane.echo_off.load(std::sync::atomic::Ordering::Relaxed);
                let is_active = pane_id == active_pane_id;

                // Render this pane into a child UI scoped to its content rect.
                // show() returns (left_clicked, deferred_key_actions).
                // left_clicked is true when a primary left-click was pressed inside
                // this pane's rect — used below for click-to-focus.
                let show_result =
                    ui.scope_builder(egui::UiBuilder::new().max_rect(content_rect), |pane_ui| {
                        self.terminal_widget.show(
                            pane_ui,
                            &pane_snap,
                            &mut pane.view_state,
                            &pane.render_state,
                            &mut pane.render_cache,
                            &pane.input_tx,
                            &pane.clipboard_rx,
                            &pane.search_buffer_rx,
                            ui_overlay_open,
                            bg_opacity,
                            self.config.ui.background_image_opacity,
                            self.config.ui.background_image_mode,
                            &self.binding_map,
                            is_echo_off,
                            is_active,
                        )
                    });
                let (left_clicked, deferred_actions) = show_result.inner;
                all_deferred_actions.extend(deferred_actions);

                // Click-to-focus: if a non-active pane was left-clicked, transfer
                // keyboard focus to it and send FocusChange events to both panes.
                if left_clicked && !is_active {
                    let tab = self.tabs.active_tab_mut();
                    let old_active = tab.active_pane;
                    // Notify the previously-active pane that it lost focus.
                    if let Some(old_pane) = tab.pane_tree.find(old_active)
                        && let Err(e) = old_pane.input_tx.send(InputEvent::FocusChange(false))
                    {
                        error!("Failed to send FocusChange(false) to pane {old_active}: {e}");
                    }
                    // Switch focus.
                    tab.active_pane = pane_id;
                    // Notify the newly-active pane that it gained focus.
                    if let Some(new_pane) = tab.pane_tree.find(pane_id)
                        && let Err(e) = new_pane.input_tx.send(InputEvent::FocusChange(true))
                    {
                        error!("Failed to send FocusChange(true) to pane {pane_id}: {e}");
                    }
                }

                // Advance text blink cycle for this pane if it has blinking text.
                if pane_snap.has_blinking_text {
                    // Re-borrow after the allocate_new_ui closure.
                    let tab = self.tabs.active_tab_mut();
                    if let Some(p) = tab.pane_tree.find_mut(pane_id) {
                        p.view_state.tick_text_blink();
                    }
                }

                // Determine repaint delay for this pane.
                let cursor_is_blinking = matches!(
                    pane_snap.cursor_visual_style,
                    freminal_common::cursor::CursorVisualStyle::BlockCursorBlink
                        | freminal_common::cursor::CursorVisualStyle::UnderlineCursorBlink
                        | freminal_common::cursor::CursorVisualStyle::VerticalLineCursorBlink,
                );
                if pane_snap.content_changed || cursor_is_blinking || pane_snap.has_blinking_text {
                    let delay = if pane_snap.content_changed {
                        std::time::Duration::from_millis(16)
                    } else if pane_snap.has_blinking_text {
                        view_state::TEXT_BLINK_TICK_DURATION
                    } else {
                        std::time::Duration::from_millis(500)
                    };
                    shortest_repaint_delay =
                        Some(shortest_repaint_delay.map_or(delay, |prev| prev.min(delay)));
                }
            }

            // ── Window-level post-processing pass ────────────────────
            //
            // When a user GLSL shader is active, the window FBO now contains
            // the composited terminal content from all panes.  We draw it
            // through the user shader back to egui's framebuffer.
            //
            // This callback is registered BEFORE pane borders so the borders
            // are painted on top of the shader output.
            {
                let wpr_check = self
                    .window_post
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let shader_active = wpr_check.is_active();
                let pending = wpr_check.pending_shader.is_some();
                drop(wpr_check);

                if shader_active || pending {
                    let frame_dt = ui.input(|i| i.stable_dt);
                    let wpr_for_post = Arc::clone(&self.window_post);
                    ui.painter().add(egui::PaintCallback {
                        rect: available_rect,
                        callback: Arc::new(CallbackFn::new(move |info, painter| {
                            let gl = painter.gl();
                            let vp = info.viewport_in_pixels();
                            let mut wpr = wpr_for_post
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner);

                            // Lazy-init GPU resources.
                            if !wpr.initialized()
                                && let Err(e) = wpr.init(gl)
                            {
                                error!("WindowPostRenderer init failed: {e}");
                                return;
                            }

                            // Process any pending shader change.
                            if let Some(pending_shader) = wpr.pending_shader.take() {
                                match pending_shader {
                                    Some(src) => {
                                        if let Err(e) =
                                            wpr.update_shader(gl, &src, vp.width_px, vp.height_px)
                                        {
                                            error!("Shader compilation failed: {e}");
                                        }
                                    }
                                    None => wpr.clear_shader(gl),
                                }
                            }

                            // Apply the post-processing pass if the shader is active.
                            if wpr.is_active() {
                                wpr.ensure_fbo(gl, vp.width_px, vp.height_px);
                                // Bind egui's framebuffer as the render target.
                                unsafe {
                                    gl.bind_framebuffer(
                                        glow::FRAMEBUFFER,
                                        painter.intermediate_fbo(),
                                    );
                                }

                                let vp_w = vp.width_px.approx_as::<f32>().unwrap_or(0.0);
                                let vp_h = vp.height_px.approx_as::<f32>().unwrap_or(0.0);
                                wpr.draw_post_pass(gl, vp_w, vp_h, frame_dt);
                            }
                        })),
                    });

                    // When the shader is active, request continuous repaints so
                    // the `u_time` uniform advances smoothly (~60 fps).
                    if shader_active {
                        let anim_delay = std::time::Duration::from_millis(16);
                        shortest_repaint_delay = Some(
                            shortest_repaint_delay.map_or(anim_delay, |prev| prev.min(anim_delay)),
                        );
                    }
                }
            }

            // ── Pane borders ─────────────────────────────────────────
            //
            // Draw tmux-style half-highlighted borders: each split border is
            // divided at the midpoint along its length. The half adjacent to
            // the active pane's subtree is drawn in the active color; the
            // other half gets the inactive color. This makes it visually
            // clear which pane owns each shared edge.
            if has_multiple_panes && zoomed_pane.is_none() {
                let painter = ui.painter();
                let inactive_color = egui::Color32::from_gray(80);
                let active_color = egui::Color32::from_rgb(100, 160, 255);

                let border_rects = self
                    .tabs
                    .active_tab()
                    .pane_tree
                    .split_borders(available_rect, active_pane_id)
                    .unwrap_or_default();

                for border in &border_rects {
                    let r = border.rect;

                    // Determine which halves are active/inactive.
                    // active_in_first == Some(true)  → first half active
                    // active_in_first == Some(false) → second half active
                    // active_in_first == None        → both inactive
                    let (first_color, second_color) = match border.active_in_first {
                        Some(true) => (active_color, inactive_color),
                        Some(false) => (inactive_color, active_color),
                        None => (inactive_color, inactive_color),
                    };

                    match border.direction {
                        panes::SplitDirection::Horizontal => {
                            // Vertical dividing line — split top/bottom.
                            // First child is left → "first half" = top.
                            let mid_y = f32::midpoint(r.min.y, r.max.y);
                            let top = egui::Rect::from_min_max(r.min, egui::pos2(r.max.x, mid_y));
                            let bot = egui::Rect::from_min_max(egui::pos2(r.min.x, mid_y), r.max);

                            painter.line_segment(
                                [top.left_top(), top.left_bottom()],
                                egui::Stroke::new(border_width, first_color),
                            );
                            painter.line_segment(
                                [bot.left_top(), bot.left_bottom()],
                                egui::Stroke::new(border_width, second_color),
                            );
                        }
                        panes::SplitDirection::Vertical => {
                            // Horizontal dividing line — split left/right.
                            // First child is top → "first half" = left.
                            let mid_x = f32::midpoint(r.min.x, r.max.x);
                            let left = egui::Rect::from_min_max(r.min, egui::pos2(mid_x, r.max.y));
                            let right = egui::Rect::from_min_max(egui::pos2(mid_x, r.min.y), r.max);

                            painter.line_segment(
                                [left.left_top(), left.right_top()],
                                egui::Stroke::new(border_width, first_color),
                            );
                            painter.line_segment(
                                [right.left_top(), right.right_top()],
                                egui::Stroke::new(border_width, second_color),
                            );
                        }
                    }
                }
            }

            // Handle key actions that couldn't be dispatched at the input
            // layer because they require full GUI state.
            for action in all_deferred_actions {
                self.dispatch_deferred_action(action);
            }

            // Handle deferred close-pane (needs `ui` for ViewportCommand::Close).
            if self.pending_close_pane {
                self.pending_close_pane = false;
                self.close_focused_pane(ui);
            }

            // Handle deferred directional focus (needs layout rects).
            if let Some(dir) = self.pending_focus_direction.take() {
                self.focus_pane_in_direction(dir, available_rect);
            }

            // Keep the window title bar in sync with the active tab's title.
            // This handles tab switches, OSC 0/2 title changes, and restore
            // from the title stack — all in one place.
            //
            // Only issue the viewport command when the title actually changed;
            // calling `send_viewport_cmd` unconditionally every frame triggers
            // an infinite repaint loop (~3 % idle CPU).
            let active_title = &self.tabs.active_tab().active_pane().title;
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

            // Schedule a repaint at the shortest interval needed by any pane.
            if let Some(delay) = shortest_repaint_delay {
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
                        .active_pane()
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
                    for tab in self.tabs.iter_mut() {
                        if let Ok(panes) = tab.pane_tree.iter_panes_mut() {
                            for pane in panes {
                                pane.render_cache.invalidate_theme_cache();
                            }
                        }
                    }
                }

                let font_changed =
                    self.terminal_widget
                        .apply_config_changes(ui.ctx(), &self.config, &new_cfg);
                if font_changed {
                    // Font or ligature config changed — clear each pane's GL
                    // atlas and force full vertex rebuilds.
                    self.invalidate_all_pane_atlases();
                }
                self.binding_map = new_cfg.build_binding_map().unwrap_or_else(|e| {
                    error!(
                        "Failed to rebuild binding map after settings apply: {e}. Using defaults."
                    );
                    freminal_common::keybindings::BindingMap::default()
                });
                self.config = new_cfg;

                // Apply background image and shader changes to all panes.
                // The actual GL calls happen in each pane's PaintCallback (needs GL context).
                {
                    let new_bg_path = self.config.ui.background_image.clone();
                    // Per-pane: push background image changes (root window).
                    for tab in self.tabs.iter() {
                        if let Ok(panes) = tab.pane_tree.iter_panes() {
                            for pane in panes {
                                if let Ok(mut rs) = pane.render_state.lock() {
                                    rs.set_pending_bg_image(new_bg_path.clone());
                                }
                            }
                        }
                    }
                    // Per-pane: push background image changes (secondary windows).
                    self.propagate_bg_image_to_secondary_windows(new_bg_path.as_ref());
                    // Window-level: push shader change.
                    // Only update if the path is None (clear shader) or the read
                    // succeeds (new shader source).  On read failure, leave the
                    // current shader in place and log the error.
                    let shader_pending: Option<String> = self
                        .config
                        .shader
                        .path
                        .as_ref()
                        .map_or(Some(None), |p| match std::fs::read_to_string(p) {
                            Ok(src) => Some(Some(src)),
                            Err(e) => {
                                error!(
                                    "Failed to read shader file '{}': {e}; keeping current shader",
                                    p.display()
                                );
                                None
                            }
                        })
                        .flatten();
                    // shader_pending: None means "keep current" (read failed),
                    // need to distinguish from "clear shader" (path was None).
                    // Re-derive: if path is None, clear; if path is Some and
                    // read succeeded, set; if read failed, skip.
                    let has_shader_path = self.config.shader.path.is_some();
                    if !has_shader_path {
                        // Clear shader.
                        if let Ok(mut wpr) = self.window_post.lock() {
                            wpr.pending_shader = Some(None);
                        }
                        self.propagate_shader_to_secondary_windows(None);
                    } else if let Some(ref src) = shader_pending {
                        // Set new shader.
                        if let Ok(mut wpr) = self.window_post.lock() {
                            wpr.pending_shader = Some(Some(src.clone()));
                        }
                        self.propagate_shader_to_secondary_windows(Some(src));
                    }
                    // else: read failed — leave current shader in place.
                }

                // Notify all panes in all tabs of the new theme mode so DECRPM ?2031
                // returns the correct locked/dynamic response after the config change.
                for tab in self.tabs.iter() {
                    if let Ok(panes) = tab.pane_tree.iter_panes() {
                        for pane in panes {
                            if let Err(e) = pane.input_tx.send(InputEvent::ThemeModeUpdate(
                                self.config.theme.mode,
                                self.os_dark_mode,
                            )) {
                                error!("Failed to send ThemeModeUpdate after settings apply: {e}");
                            }
                        }
                    }
                }
            }
            SettingsAction::PreviewTheme(ref slug) => {
                if let Some(theme) = freminal_common::themes::by_slug(slug) {
                    if let Err(e) = self
                        .tabs
                        .active_tab()
                        .active_pane()
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
                        .active_pane()
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

/// Spawn a split pane in a secondary window's active tab.
///
/// Mirrors [`FreminalGui::spawn_split_pane`] but operates on a
/// [`SecondaryWindowState`] rather than the root GUI instance.
fn spawn_split_pane_in_secondary(
    win: &mut SecondaryWindowState,
    direction: panes::SplitDirection,
    ui: &egui::Ui,
) {
    let os_dark_mode = ui.ctx().global_style().visuals.dark_mode;
    let theme = freminal_common::themes::by_slug(win.config.theme.active_slug(os_dark_mode))
        .unwrap_or(&freminal_common::themes::CATPPUCCIN_MOCHA);

    let channels =
        match pty::spawn_pty_tab(&win.args, win.config.scrollback.limit, theme, &win.egui_ctx) {
            Ok(ch) => ch,
            Err(e) => {
                error!("Secondary window: failed to spawn split pane: {e}");
                return;
            }
        };

    let target_id = win.tabs.active_tab().active_pane;

    // Pre-allocate the pane ID so the mutex guard is dropped before we
    // borrow `win.tabs` mutably (avoids significant_drop_tightening lint).
    let new_pane_id_alloc = win
        .pane_id_gen
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .next_id();

    let new_pane_id = {
        let tab = win.tabs.active_tab_mut();
        match tab.pane_tree.split_with_id(
            target_id,
            direction,
            new_pane_id_alloc,
            panes::Pane {
                id: new_pane_id_alloc,
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
                render_state: new_render_state(Arc::clone(&win.window_post)),
                render_cache: terminal::PaneRenderCache::new(),
            },
        ) {
            Ok(id) => id,
            Err(e) => {
                error!("Secondary window: failed to insert split pane: {e}");
                return;
            }
        }
    };

    let tab = win.tabs.active_tab_mut();

    if let Some(old_pane) = tab.pane_tree.find(target_id)
        && let Err(e) = old_pane.input_tx.send(InputEvent::FocusChange(false))
    {
        error!("Secondary window: FocusChange(false) to {target_id}: {e}");
    }

    tab.active_pane = new_pane_id;

    if let Some(new_pane) = tab.pane_tree.find(new_pane_id) {
        if let Err(e) = new_pane.input_tx.send(InputEvent::FocusChange(true)) {
            error!("Secondary window: FocusChange(true) to {new_pane_id}: {e}");
        }
        if let Err(e) = new_pane.input_tx.send(InputEvent::ThemeModeUpdate(
            win.config.theme.mode,
            os_dark_mode,
        )) {
            error!("Secondary window: ThemeModeUpdate to split pane: {e}");
        }
        let new_bg_path = win.config.ui.background_image.clone();
        if new_bg_path.is_some()
            && let Ok(mut rs) = new_pane.render_state.lock()
        {
            rs.set_pending_bg_image(new_bg_path);
        }
    }
}

/// Per-frame rendering for a secondary OS window.
///
/// Called by the deferred viewport closure registered in [`FreminalGui::spawn_new_window`]
/// every frame that the window should remain alive.  Mirrors the core of
/// [`FreminalGui::ui`] but operates on a [`SecondaryWindowState`] instead of the
/// root [`FreminalGui`].
///
/// Close detection: when egui reports a close request for this viewport the function
/// sends `ViewportCommand::Close` so egui actually destroys the window.  The root
/// window prunes the corresponding entry from `secondary_windows` on the following
/// frame because `show_viewport_deferred` stops being called for it.
#[allow(clippy::too_many_lines)] // Mirrors FreminalGui::ui — same justified coupling.
fn run_secondary_window_frame(win: &mut SecondaryWindowState, ui: &mut egui::Ui) {
    // Honor close requests from the OS (alt-F4, window button, etc.).
    let close_requested = ui.ctx().input(|i| i.viewport().close_requested());
    if close_requested {
        // Mark closed so the root pruning loop stops re-registering this window.
        // This causes egui to destroy the viewport and drops the Arc, which
        // cleans up PTY threads and OS resources.
        win.closed = true;
        ui.ctx().send_viewport_cmd(ViewportCommand::Close);
        return;
    }

    // Poll all tabs for PTY death signals.
    let mut dead_panes: Vec<(usize, panes::PaneId)> = Vec::new();
    for (tab_idx, tab) in win.tabs.iter().enumerate() {
        if let Ok(panes_list) = tab.pane_tree.iter_panes() {
            for pane in panes_list {
                if pane.pty_dead_rx.try_recv().is_ok() {
                    dead_panes.push((tab_idx, pane.id));
                }
            }
        }
    }
    for (tab_idx, pane_id) in dead_panes.into_iter().rev() {
        let is_active_tab = tab_idx == win.tabs.active_index();
        if !is_active_tab && let Err(e) = win.tabs.switch_to(tab_idx) {
            error!("Secondary window: failed to switch to tab {tab_idx} for dead pane: {e}");
            continue;
        }
        let tab = win.tabs.active_tab_mut();
        if tab.zoomed_pane == Some(pane_id) {
            tab.zoomed_pane = None;
        }
        match tab.pane_tree.close(pane_id) {
            Ok(_) => {
                let tab = win.tabs.active_tab_mut();
                if let Ok(panes_list) = tab.pane_tree.iter_panes_mut() {
                    for pane in panes_list {
                        pane.view_state.last_sent_size = (0, 0);
                    }
                }
                let tab = win.tabs.active_tab_mut();
                if tab.active_pane == pane_id
                    && let Ok(panes_list) = tab.pane_tree.iter_panes()
                    && let Some(first) = panes_list.first()
                {
                    let new_id = first.id;
                    if let Err(e) = first.input_tx.send(InputEvent::FocusChange(true)) {
                        error!("Secondary window: FocusChange(true) to pane {new_id}: {e}");
                    }
                    tab.active_pane = new_id;
                }
            }
            Err(panes::PaneError::CannotCloseLastPane) => {
                // Last pane in last tab — close the secondary window.
                if win.tabs.tab_count() <= 1 {
                    win.closed = true;
                    ui.ctx().send_viewport_cmd(ViewportCommand::Close);
                    return;
                }
                if let Err(e) = win.tabs.close_active_tab() {
                    error!("Secondary window: failed to close tab: {e}");
                }
            }
            Err(e) => {
                error!("Secondary window: failed to close dead pane {pane_id}: {e}");
            }
        }
        if !is_active_tab {
            let restore_idx = tab_idx.min(win.tabs.tab_count().saturating_sub(1));
            let _ = win.tabs.switch_to(restore_idx);
        }
    }

    let snap = win.tabs.active_tab().active_pane().arc_swap.load();
    if win.tabs.active_tab().active_pane().view_state.scroll_offset != snap.scroll_offset {
        win.tabs
            .active_tab_mut()
            .active_pane_mut()
            .view_state
            .scroll_offset = snap.scroll_offset;
    }

    // ── Menu bar ─────────────────────────────────────────────────────────
    // Show a simplified menu bar for secondary windows.  The root window's
    // menu bar lives in FreminalGui::ui(); here we only expose the actions
    // that make sense for a secondary window (no Settings, no playback).
    let mut any_menu_open = false;
    Panel::top("sec_menu_bar").show_inside(ui, |ui| {
        egui::MenuBar::new().ui(ui, |ui| {
            // ── Freminal menu ────────────────────────────────────────────
            let freminal_resp = ui.menu_button("Freminal", |ui| {
                if ui.button("Quit Window").clicked() {
                    win.closed = true;
                    ui.ctx().send_viewport_cmd(ViewportCommand::Close);
                    ui.close();
                }
            });
            if freminal_resp.inner.is_some() {
                any_menu_open = true;
            }

            // ── Tab menu ─────────────────────────────────────────────────
            let tab_resp =
                ui.menu_button("Tab", |ui| {
                    if ui.button("New Tab").clicked() {
                        // Spawn a new tab directly — we have `win` mutably here.
                        let os_dark = ui.ctx().global_style().visuals.dark_mode;
                        let theme =
                            freminal_common::themes::by_slug(win.config.theme.active_slug(os_dark))
                                .unwrap_or(&freminal_common::themes::CATPPUCCIN_MOCHA);
                        match pty::spawn_pty_tab(
                            &win.args,
                            win.config.scrollback.limit,
                            theme,
                            &win.egui_ctx,
                        ) {
                            Ok(channels) => {
                                let tab_id = win.tabs.next_tab_id();
                                let pane_id = win
                                    .pane_id_gen
                                    .lock()
                                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                                    .next_id();
                                let pane = panes::Pane {
                                    id: pane_id,
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
                                    render_state: new_render_state(Arc::clone(&win.window_post)),
                                    render_cache: terminal::PaneRenderCache::new(),
                                };
                                let new_tab = Tab::new(tab_id, pane);
                                if let Err(e) = new_tab.active_pane().input_tx.send(
                                    InputEvent::ThemeModeUpdate(win.config.theme.mode, os_dark),
                                ) {
                                    error!("Secondary window menu: ThemeModeUpdate: {e}");
                                }
                                win.tabs.add_tab(new_tab);
                            }
                            Err(e) => error!("Secondary window menu: new tab failed: {e}"),
                        }
                        ui.close();
                    }
                    if ui.button("Close Tab").clicked() {
                        if win.tabs.tab_count() > 1 {
                            if let Err(e) = win.tabs.close_active_tab() {
                                error!("Secondary window menu: close tab: {e}");
                            }
                        } else {
                            win.closed = true;
                            ui.ctx().send_viewport_cmd(ViewportCommand::Close);
                        }
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("Next Tab").clicked() {
                        win.tabs.next_tab();
                        win.tabs.active_tab_mut().active_pane_mut().bell_active = false;
                        ui.close();
                    }
                    if ui.button("Previous Tab").clicked() {
                        win.tabs.prev_tab();
                        win.tabs.active_tab_mut().active_pane_mut().bell_active = false;
                        ui.close();
                    }
                });
            if tab_resp.inner.is_some() {
                any_menu_open = true;
            }

            // ── Pane menu ─────────────────────────────────────────────────
            let pane_resp = ui.menu_button("Pane", |ui| {
                if ui.button("Split Vertical").clicked() {
                    spawn_split_pane_in_secondary(win, panes::SplitDirection::Horizontal, ui);
                    ui.close();
                }
                if ui.button("Split Horizontal").clicked() {
                    spawn_split_pane_in_secondary(win, panes::SplitDirection::Vertical, ui);
                    ui.close();
                }
                ui.separator();
                if ui.button("Close Pane").clicked() {
                    win.pending_close_pane = true;
                    ui.close();
                }
                if ui.button("Zoom Pane").clicked() {
                    let tab = win.tabs.active_tab_mut();
                    let current = tab.active_pane;
                    if tab.zoomed_pane == Some(current) {
                        tab.zoomed_pane = None;
                    } else {
                        tab.zoomed_pane = Some(current);
                    }
                    let tab = win.tabs.active_tab_mut();
                    if let Ok(panes_list) = tab.pane_tree.iter_panes_mut() {
                        for pane in panes_list {
                            pane.view_state.last_sent_size = (0, 0);
                        }
                    }
                    ui.close();
                }
            });
            if pane_resp.inner.is_some() {
                any_menu_open = true;
            }

            // ── Window menu ───────────────────────────────────────────────
            let win_resp = ui.menu_button("Window", |ui| {
                if ui.button("New Window").clicked() {
                    win.pending_new_window = true;
                    ui.close();
                }
            });
            if win_resp.inner.is_some() {
                any_menu_open = true;
            }
        });
    });

    // Tab bar (only when multiple tabs open or config requests it).
    let show_tab_bar = win.tabs.tab_count() > 1 || win.config.tabs.show_single_tab;
    if show_tab_bar {
        let panel = match win.config.tabs.position {
            TabBarPosition::Top => Panel::top("sec_tab_bar"),
            TabBarPosition::Bottom => Panel::bottom("sec_tab_bar"),
        };
        let tab_action = panel
            .show_inside(ui, |ui| {
                ui.horizontal(|ui| {
                    let active = win.tabs.active_index();
                    let count = win.tabs.tab_count();
                    let mut action = TabBarAction::None;
                    for (i, tab) in win.tabs.iter().enumerate() {
                        if i > 0 {
                            ui.separator();
                        }
                        let is_echo_off = win.config.security.password_indicator
                            && tab
                                .active_pane()
                                .echo_off
                                .load(std::sync::atomic::Ordering::Relaxed);
                        let tab_action = FreminalGui::show_single_tab(
                            ui,
                            tab,
                            i,
                            i == active,
                            count,
                            is_echo_off,
                        );
                        if !matches!(tab_action, TabBarAction::None) {
                            action = tab_action;
                        }
                    }
                    ui.separator();
                    if ui.button("+").clicked() {
                        action = TabBarAction::NewTab;
                    }
                    action
                })
                .inner
            })
            .inner;
        // Dispatch tab bar actions for the secondary window.
        match tab_action {
            TabBarAction::NewTab => {
                let theme = freminal_common::themes::by_slug(
                    win.config
                        .theme
                        .active_slug(ui.ctx().global_style().visuals.dark_mode),
                )
                .unwrap_or(&freminal_common::themes::CATPPUCCIN_MOCHA);
                match pty::spawn_pty_tab(
                    &win.args,
                    win.config.scrollback.limit,
                    theme,
                    &win.egui_ctx,
                ) {
                    Ok(channels) => {
                        let tab_id = win.tabs.next_tab_id();
                        let pane_id = win
                            .pane_id_gen
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .next_id();
                        let pane = panes::Pane {
                            id: pane_id,
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
                            render_state: new_render_state(Arc::clone(&win.window_post)),
                            render_cache: terminal::PaneRenderCache::new(),
                        };
                        let new_tab = Tab::new(tab_id, pane);
                        if let Err(e) =
                            new_tab
                                .active_pane()
                                .input_tx
                                .send(InputEvent::ThemeModeUpdate(
                                    win.config.theme.mode,
                                    ui.ctx().global_style().visuals.dark_mode,
                                ))
                        {
                            error!("Secondary window: ThemeModeUpdate for new tab: {e}");
                        }
                        win.tabs.add_tab(new_tab);
                    }
                    Err(e) => error!("Secondary window: failed to spawn new tab: {e}"),
                }
            }
            TabBarAction::SwitchTo(idx) => {
                if let Err(e) = win.tabs.switch_to(idx) {
                    trace!("Secondary window: cannot switch to tab {idx}: {e}");
                } else {
                    win.tabs.active_tab_mut().active_pane_mut().bell_active = false;
                }
            }
            TabBarAction::Close(idx) => {
                if win.tabs.tab_count() > 1
                    && let Err(e) = win.tabs.close_tab(idx)
                {
                    trace!("Secondary window: cannot close tab {idx}: {e}");
                }
            }
            TabBarAction::None => {}
        }
    }

    let _panel_response = CentralPanel::default().show_inside(ui, |ui| {
        // Initialise widget if not yet done, then extract the metrics we need before
        // doing any win.tabs / win.config borrows. We structure this in two phases:
        //   Phase 1: init + sync + read metrics (widget borrow lives in a block)
        //   Phase 2: tabs/config access (widget re-borrowed only when calling show())
        let ppp = ui.ctx().pixels_per_point();
        let (
            _cell_w_u,
            _cell_h_u,
            font_width,
            font_height,
            logical_char_w,
            logical_char_h,
            ppp_changed,
        ) = {
            let widget = win.terminal_widget(ui.ctx());
            let ppp_changed = widget.sync_pixels_per_point(ppp);
            let (cw, ch) = widget.cell_size();
            let fw = usize::value_from(cw).unwrap_or(0);
            let fh = usize::value_from(ch).unwrap_or(0);
            let lcw = f32::approx_from(cw).unwrap_or(0.0) / ppp;
            let lch = f32::approx_from(ch).unwrap_or(0.0) / ppp;
            (cw, ch, fw, fh, lcw, lch, ppp_changed)
        };
        // If the window moved to a different-DPI monitor, invalidate all pane
        // atlases so glyphs are re-rasterised at the new pixels-per-point.
        if ppp_changed {
            for tab in win.tabs.iter_mut() {
                if let Ok(panes_list) = tab.pane_tree.iter_panes_mut() {
                    for pane in panes_list {
                        pane.render_state
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .clear_atlas();
                        pane.render_cache.invalidate_content();
                    }
                }
            }
        }
        // Apply font zoom from effective font size (widget borrow ends above).
        {
            let effective = win
                .tabs
                .active_tab()
                .active_pane()
                .view_state
                .effective_font_size(win.config.font.size);
            win.terminal_widget(ui.ctx()).apply_font_zoom(effective);
        }

        let window_width = ui.input(|i: &egui::InputState| i.content_rect());

        // Drain window commands for all panes in all tabs.
        let active_idx = win.tabs.active_index();
        let active_pane_id_for_drain = win.tabs.active_tab().active_pane;
        let window_focused = win
            .tabs
            .active_tab()
            .active_pane()
            .view_state
            .window_focused;
        for (idx, tab) in win.tabs.iter_mut().enumerate() {
            let is_active_tab = idx == active_idx;
            if let Ok(panes_list) = tab.pane_tree.iter_panes_mut() {
                for pane in panes_list {
                    let is_fully_active = is_active_tab && pane.id == active_pane_id_for_drain;
                    handle_window_manipulation(
                        ui,
                        &pane.window_cmd_rx,
                        &pane.pty_write_tx,
                        font_width,
                        font_height,
                        window_width,
                        &mut pane.title_stack,
                        &mut pane.title,
                        &mut pane.bell_active,
                        &mut pane.view_state.bell_since,
                        win.config.bell.mode,
                        win.config.security.allow_clipboard_read,
                        is_fully_active,
                        window_focused,
                    );
                }
            }
        }

        // Style cache (background color).
        let bg_opacity = win.config.ui.background_opacity;
        let style_key = (snap.is_normal_display, snap.theme, bg_opacity);
        let style_changed = match win.style_cache {
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
                    style.visuals.window_fill = internal_color_to_egui_with_alpha(
                        freminal_common::colors::TerminalColor::DefaultBackground,
                        false,
                        snap.theme,
                        1.0,
                    );
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
                    let alpha = (bg_opacity * 255.0)
                        .round()
                        .approx_as::<u8>()
                        .unwrap_or(255);
                    style.visuals.panel_fill =
                        egui::Color32::from_rgba_unmultiplied(255, 255, 255, alpha);
                });
            }
            win.style_cache = Some(style_key);
        }

        // Pane layout and rendering.
        let available_rect = ui.available_rect_before_wrap();
        let active_pane_id = win.tabs.active_tab().active_pane;
        let zoomed_pane = win.tabs.active_tab().zoomed_pane;
        let has_multiple_panes = win.tabs.active_tab().pane_tree.pane_count().unwrap_or(1) > 1;

        let (pane_layout, border_width) = if let Some(zoomed_id) = zoomed_pane {
            (vec![(zoomed_id, available_rect)], 0.0)
        } else {
            let bw: f32 = if has_multiple_panes { 1.0 } else { 0.0 };
            let layout = win
                .tabs
                .active_tab()
                .pane_tree
                .layout(available_rect)
                .unwrap_or_default();
            (layout, bw)
        };

        let mut all_deferred_actions = Vec::new();
        let mut shortest_repaint_delay: Option<std::time::Duration> = None;

        // Pane border drag-to-resize (suppressed while a menu overlay is open).
        if has_multiple_panes && zoomed_pane.is_none() && !any_menu_open {
            let borders = win
                .tabs
                .active_tab()
                .pane_tree
                .split_borders(available_rect, active_pane_id)
                .unwrap_or_default();
            let sensor_half: f32 = 3.0;
            for (border_idx, border) in borders.iter().enumerate() {
                let sensor_rect = match border.direction {
                    panes::SplitDirection::Horizontal => {
                        let cx = border.rect.center().x;
                        egui::Rect::from_min_max(
                            egui::pos2(cx - sensor_half, border.rect.min.y),
                            egui::pos2(cx + sensor_half, border.rect.max.y),
                        )
                    }
                    panes::SplitDirection::Vertical => {
                        let cy = border.rect.center().y;
                        egui::Rect::from_min_max(
                            egui::pos2(border.rect.min.x, cy - sensor_half),
                            egui::pos2(border.rect.max.x, cy + sensor_half),
                        )
                    }
                };
                let sensor_id = ui.id().with("sec_pane_border_sensor").with(border_idx);
                let response = ui.interact(sensor_rect, sensor_id, egui::Sense::click_and_drag());
                if response.hovered() || response.dragged() {
                    let cursor = match border.direction {
                        panes::SplitDirection::Horizontal => egui::CursorIcon::ResizeHorizontal,
                        panes::SplitDirection::Vertical => egui::CursorIcon::ResizeVertical,
                    };
                    ui.ctx().set_cursor_icon(cursor);
                }
                if response.drag_started() {
                    win.border_drag = Some(PaneBorderDrag {
                        target_pane: border.first_child_pane,
                        direction: border.direction,
                        parent_extent: border.parent_extent,
                    });
                }
                if response.dragged()
                    && let Some(drag) = &win.border_drag
                {
                    let delta_px = match drag.direction {
                        panes::SplitDirection::Horizontal => response.drag_delta().x,
                        panes::SplitDirection::Vertical => response.drag_delta().y,
                    };
                    if drag.parent_extent > 0.0 {
                        let delta_ratio = delta_px / drag.parent_extent;
                        if let Err(e) = win.tabs.active_tab_mut().pane_tree.resize_split(
                            drag.target_pane,
                            drag.direction,
                            delta_ratio,
                        ) {
                            debug!("Secondary window border resize failed: {e}");
                        }
                    }
                }
                if response.drag_stopped() {
                    win.border_drag = None;
                }
            }
        }

        // ── Pre-clear the window post-processing FBO ──────────────────────
        // When a user GLSL shader is active (or about to become active),
        // all panes render into this window's FBO.  Clear it once per frame
        // here, before any pane draws into it, so stale content from the
        // previous frame does not bleed through.
        {
            let wpr_guard = win
                .window_post
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let wpr_active = wpr_guard.is_active();
            let shader_activation_pending = wpr_guard.pending_shader.is_some();
            drop(wpr_guard);

            if wpr_active || shader_activation_pending {
                let wpr_for_clear = Arc::clone(&win.window_post);
                ui.painter().add(egui::PaintCallback {
                    rect: available_rect,
                    callback: Arc::new(CallbackFn::new(move |info, painter| {
                        let gl = painter.gl();
                        let vp = info.viewport_in_pixels();
                        let mut wpr = wpr_for_clear
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        wpr.ensure_fbo(gl, vp.width_px, vp.height_px);
                        if let Some(fbo) = wpr.fbo() {
                            unsafe {
                                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
                                gl.clear_color(0.0, 0.0, 0.0, 0.0);
                                gl.clear(glow::COLOR_BUFFER_BIT);
                                // Restore egui's FBO.
                                gl.bind_framebuffer(glow::FRAMEBUFFER, painter.intermediate_fbo());
                            }
                        }
                    })),
                });
            }
        }

        for (pane_id, pane_rect) in &pane_layout {
            let content_rect = if has_multiple_panes {
                let half = border_width / 2.0;
                let shrink_left = if pane_rect.min.x > available_rect.min.x {
                    half
                } else {
                    0.0
                };
                let shrink_right = if pane_rect.max.x < available_rect.max.x {
                    half
                } else {
                    0.0
                };
                let shrink_top = if pane_rect.min.y > available_rect.min.y {
                    half
                } else {
                    0.0
                };
                let shrink_bottom = if pane_rect.max.y < available_rect.max.y {
                    half
                } else {
                    0.0
                };
                egui::Rect::from_min_max(
                    egui::pos2(pane_rect.min.x + shrink_left, pane_rect.min.y + shrink_top),
                    egui::pos2(
                        pane_rect.max.x - shrink_right,
                        pane_rect.max.y - shrink_bottom,
                    ),
                )
            } else {
                *pane_rect
            };

            let pane_width_chars = (content_rect.width() / logical_char_w)
                .floor()
                .approx_as::<usize>()
                .unwrap_or_else(|e| {
                    error!("Sec win pane width chars: {e}");
                    10
                });
            let pane_height_chars = (content_rect.height() / logical_char_h)
                .floor()
                .approx_as::<usize>()
                .unwrap_or_else(|e| {
                    error!("Sec win pane height chars: {e}");
                    10
                })
                .max(1);

            let pane_id = *pane_id;
            let tab = win.tabs.active_tab_mut();
            let Some(pane) = tab.pane_tree.find_mut(pane_id) else {
                error!("Secondary window: pane {pane_id} not found during render");
                continue;
            };

            let new_size = (pane_width_chars, pane_height_chars);
            if new_size != pane.view_state.last_sent_size {
                if let Err(e) = pane.input_tx.send(InputEvent::Resize(
                    pane_width_chars,
                    pane_height_chars,
                    font_width,
                    font_height,
                )) {
                    error!("Secondary window: resize event for {pane_id}: {e}");
                } else {
                    pane.view_state.last_sent_size = new_size;
                }
            }

            let pane_snap = pane.arc_swap.load();
            if pane.view_state.scroll_offset != pane_snap.scroll_offset {
                pane.view_state.scroll_offset = pane_snap.scroll_offset;
            }

            let is_echo_off = win.config.security.password_indicator
                && pane.echo_off.load(std::sync::atomic::Ordering::Relaxed);
            let is_active = pane_id == active_pane_id;

            // Re-borrow widget after the pane borrow ends.
            // (pane borrow ends here before widget borrow begins)
            // Copy config values needed by show() as locals before the closure
            // so they can be read independently of the win.terminal_widget borrow.
            let bg_image_opacity_local = win.config.ui.background_image_opacity;
            let bg_image_mode_local = win.config.ui.background_image_mode;
            // Clone the binding map to allow independent borrows inside the closure.
            // BindingMap is a small structure (a HashMap with a handful of entries),
            // so this clone has negligible cost per pane per frame.
            let binding_map_local = win.binding_map.clone();

            let show_result =
                ui.scope_builder(egui::UiBuilder::new().max_rect(content_rect), |pane_ui| {
                    // Split-borrow win by accessing distinct fields directly:
                    // win.terminal_widget (mutable for show()) and win.tabs (mutable for pane lookup)
                    // are distinct fields so Rust allows simultaneous borrows.
                    let SecondaryWindowState {
                        terminal_widget: ref mut tw_opt,
                        tabs: ref mut tabs_ref,
                        ..
                    } = *win;
                    let Some(widget) = tw_opt.as_mut() else {
                        return (false, Vec::new());
                    };
                    let tab = tabs_ref.active_tab_mut();
                    let Some(pane) = tab.pane_tree.find_mut(pane_id) else {
                        return (false, Vec::new());
                    };
                    widget.show(
                        pane_ui,
                        &pane_snap,
                        &mut pane.view_state,
                        &pane.render_state,
                        &mut pane.render_cache,
                        &pane.input_tx,
                        &pane.clipboard_rx,
                        &pane.search_buffer_rx,
                        any_menu_open, // ui_overlay_open — suppress input while menu is open
                        bg_opacity,
                        bg_image_opacity_local,
                        bg_image_mode_local,
                        &binding_map_local,
                        is_echo_off,
                        is_active,
                    )
                });
            let (left_clicked, deferred_actions) = show_result.inner;
            all_deferred_actions.extend(deferred_actions);

            // Click-to-focus for secondary window panes.
            if left_clicked && !is_active {
                let tab = win.tabs.active_tab_mut();
                let old_active = tab.active_pane;
                if let Some(old_pane) = tab.pane_tree.find(old_active)
                    && let Err(e) = old_pane.input_tx.send(InputEvent::FocusChange(false))
                {
                    error!("Secondary window: FocusChange(false) to {old_active}: {e}");
                }
                tab.active_pane = pane_id;
                if let Some(new_pane) = tab.pane_tree.find(pane_id)
                    && let Err(e) = new_pane.input_tx.send(InputEvent::FocusChange(true))
                {
                    error!("Secondary window: FocusChange(true) to {pane_id}: {e}");
                }
            }

            // Advance blink cycle.
            if pane_snap.has_blinking_text {
                let tab = win.tabs.active_tab_mut();
                if let Some(p) = tab.pane_tree.find_mut(pane_id) {
                    p.view_state.tick_text_blink();
                }
            }

            // Repaint delay.
            let cursor_is_blinking = matches!(
                pane_snap.cursor_visual_style,
                freminal_common::cursor::CursorVisualStyle::BlockCursorBlink
                    | freminal_common::cursor::CursorVisualStyle::UnderlineCursorBlink
                    | freminal_common::cursor::CursorVisualStyle::VerticalLineCursorBlink,
            );
            if pane_snap.content_changed || cursor_is_blinking || pane_snap.has_blinking_text {
                let delay = if pane_snap.content_changed {
                    std::time::Duration::from_millis(16)
                } else if pane_snap.has_blinking_text {
                    view_state::TEXT_BLINK_TICK_DURATION
                } else {
                    std::time::Duration::from_millis(500)
                };
                shortest_repaint_delay =
                    Some(shortest_repaint_delay.map_or(delay, |prev| prev.min(delay)));
            }
        }

        // ── Window-level post-processing pass ─────────────────────────────
        //
        // When a user GLSL shader is active, the window FBO now contains the
        // composited terminal content from all panes.  Draw it through the
        // user shader back to egui's framebuffer.  Registered BEFORE pane
        // borders so borders are painted on top of the shader output.
        {
            let wpr_check = win
                .window_post
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let shader_active = wpr_check.is_active();
            let pending = wpr_check.pending_shader.is_some();
            drop(wpr_check);

            if shader_active || pending {
                let frame_dt = ui.input(|i| i.stable_dt);
                let wpr_for_post = Arc::clone(&win.window_post);
                ui.painter().add(egui::PaintCallback {
                    rect: available_rect,
                    callback: Arc::new(CallbackFn::new(move |info, painter| {
                        let gl = painter.gl();
                        let vp = info.viewport_in_pixels();
                        let mut wpr = wpr_for_post
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);

                        // Lazy-init GPU resources.
                        if !wpr.initialized()
                            && let Err(e) = wpr.init(gl)
                        {
                            error!("Secondary window: WindowPostRenderer init failed: {e}");
                            return;
                        }

                        // Process any pending shader change.
                        if let Some(pending_shader) = wpr.pending_shader.take() {
                            match pending_shader {
                                Some(src) => {
                                    if let Err(e) =
                                        wpr.update_shader(gl, &src, vp.width_px, vp.height_px)
                                    {
                                        error!("Secondary window: shader compilation failed: {e}");
                                    }
                                }
                                None => wpr.clear_shader(gl),
                            }
                        }

                        // Apply the post-processing pass if the shader is active.
                        if wpr.is_active() {
                            wpr.ensure_fbo(gl, vp.width_px, vp.height_px);
                            unsafe {
                                gl.bind_framebuffer(glow::FRAMEBUFFER, painter.intermediate_fbo());
                            }

                            let vp_w = vp.width_px.approx_as::<f32>().unwrap_or(0.0);
                            let vp_h = vp.height_px.approx_as::<f32>().unwrap_or(0.0);
                            wpr.draw_post_pass(gl, vp_w, vp_h, frame_dt);
                        }
                    })),
                });

                // When the shader is active, request continuous repaints so
                // the `u_time` uniform advances smoothly (~60 fps).
                if shader_active {
                    let anim_delay = std::time::Duration::from_millis(16);
                    shortest_repaint_delay = Some(
                        shortest_repaint_delay.map_or(anim_delay, |prev| prev.min(anim_delay)),
                    );
                }
            }
        }

        // Pane borders.
        if has_multiple_panes && zoomed_pane.is_none() {
            let painter = ui.painter();
            let inactive_color = egui::Color32::from_gray(80);
            let active_color = egui::Color32::from_rgb(100, 160, 255);
            let borders = win
                .tabs
                .active_tab()
                .pane_tree
                .split_borders(available_rect, active_pane_id)
                .unwrap_or_default();
            for border in &borders {
                let r = border.rect;
                let (first_color, second_color) = if border.active_in_first == Some(true) {
                    (active_color, inactive_color)
                } else {
                    (inactive_color, active_color)
                };
                match border.direction {
                    panes::SplitDirection::Horizontal => {
                        let mid_y = f32::midpoint(r.min.y, r.max.y);
                        let top = egui::Rect::from_min_max(r.min, egui::pos2(r.max.x, mid_y));
                        let bot = egui::Rect::from_min_max(egui::pos2(r.min.x, mid_y), r.max);
                        painter.line_segment(
                            [top.left_top(), top.left_bottom()],
                            egui::Stroke::new(1.0, first_color),
                        );
                        painter.line_segment(
                            [bot.left_top(), bot.left_bottom()],
                            egui::Stroke::new(1.0, second_color),
                        );
                    }
                    panes::SplitDirection::Vertical => {
                        let mid_x = f32::midpoint(r.min.x, r.max.x);
                        let left = egui::Rect::from_min_max(r.min, egui::pos2(mid_x, r.max.y));
                        let right = egui::Rect::from_min_max(egui::pos2(mid_x, r.min.y), r.max);
                        painter.line_segment(
                            [left.left_top(), left.right_top()],
                            egui::Stroke::new(1.0, first_color),
                        );
                        painter.line_segment(
                            [right.left_top(), right.right_top()],
                            egui::Stroke::new(1.0, second_color),
                        );
                    }
                }
            }
        }

        // Deferred key actions for the secondary window (subset: tabs, panes, scroll).
        for action in all_deferred_actions {
            use freminal_common::keybindings::KeyAction;
            match action {
                KeyAction::NewTab => {
                    let theme = freminal_common::themes::by_slug(
                        win.config
                            .theme
                            .active_slug(ui.ctx().global_style().visuals.dark_mode),
                    )
                    .unwrap_or(&freminal_common::themes::CATPPUCCIN_MOCHA);
                    match pty::spawn_pty_tab(
                        &win.args,
                        win.config.scrollback.limit,
                        theme,
                        &win.egui_ctx,
                    ) {
                        Ok(channels) => {
                            let tab_id = win.tabs.next_tab_id();
                            let pane_id = win
                                .pane_id_gen
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                .next_id();
                            let pane = panes::Pane {
                                id: pane_id,
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
                                render_state: new_render_state(Arc::clone(&win.window_post)),
                                render_cache: terminal::PaneRenderCache::new(),
                            };
                            let new_tab = Tab::new(tab_id, pane);
                            if let Err(e) =
                                new_tab
                                    .active_pane()
                                    .input_tx
                                    .send(InputEvent::ThemeModeUpdate(
                                        win.config.theme.mode,
                                        ui.ctx().global_style().visuals.dark_mode,
                                    ))
                            {
                                error!("Secondary window: ThemeModeUpdate for new tab: {e}");
                            }
                            win.tabs.add_tab(new_tab);
                        }
                        Err(e) => error!("Secondary window: failed to spawn new tab: {e}"),
                    }
                }
                KeyAction::CloseTab => {
                    if win.tabs.tab_count() > 1 {
                        if let Err(e) = win.tabs.close_active_tab() {
                            trace!("Secondary window: cannot close tab: {e}");
                        }
                    } else {
                        win.closed = true;
                        ui.ctx().send_viewport_cmd(ViewportCommand::Close);
                    }
                }
                KeyAction::NextTab => {
                    win.tabs.next_tab();
                    win.tabs.active_tab_mut().active_pane_mut().bell_active = false;
                }
                KeyAction::PrevTab => {
                    win.tabs.prev_tab();
                    win.tabs.active_tab_mut().active_pane_mut().bell_active = false;
                }
                KeyAction::ClosePane => {
                    win.pending_close_pane = true;
                }
                KeyAction::FocusPaneLeft
                | KeyAction::FocusPaneDown
                | KeyAction::FocusPaneUp
                | KeyAction::FocusPaneRight => {
                    win.pending_focus_direction = Some(action);
                }

                // -- Tab switching --
                KeyAction::SwitchToTab1 => {
                    if let Err(e) = win.tabs.switch_to(0) {
                        trace!("Secondary window: cannot switch to tab 0: {e}");
                    } else {
                        win.tabs.active_tab_mut().active_pane_mut().bell_active = false;
                    }
                }
                KeyAction::SwitchToTab2 => {
                    if let Err(e) = win.tabs.switch_to(1) {
                        trace!("Secondary window: cannot switch to tab 1: {e}");
                    } else {
                        win.tabs.active_tab_mut().active_pane_mut().bell_active = false;
                    }
                }
                KeyAction::SwitchToTab3 => {
                    if let Err(e) = win.tabs.switch_to(2) {
                        trace!("Secondary window: cannot switch to tab 2: {e}");
                    } else {
                        win.tabs.active_tab_mut().active_pane_mut().bell_active = false;
                    }
                }
                KeyAction::SwitchToTab4 => {
                    if let Err(e) = win.tabs.switch_to(3) {
                        trace!("Secondary window: cannot switch to tab 3: {e}");
                    } else {
                        win.tabs.active_tab_mut().active_pane_mut().bell_active = false;
                    }
                }
                KeyAction::SwitchToTab5 => {
                    if let Err(e) = win.tabs.switch_to(4) {
                        trace!("Secondary window: cannot switch to tab 4: {e}");
                    } else {
                        win.tabs.active_tab_mut().active_pane_mut().bell_active = false;
                    }
                }
                KeyAction::SwitchToTab6 => {
                    if let Err(e) = win.tabs.switch_to(5) {
                        trace!("Secondary window: cannot switch to tab 5: {e}");
                    } else {
                        win.tabs.active_tab_mut().active_pane_mut().bell_active = false;
                    }
                }
                KeyAction::SwitchToTab7 => {
                    if let Err(e) = win.tabs.switch_to(6) {
                        trace!("Secondary window: cannot switch to tab 6: {e}");
                    } else {
                        win.tabs.active_tab_mut().active_pane_mut().bell_active = false;
                    }
                }
                KeyAction::SwitchToTab8 => {
                    if let Err(e) = win.tabs.switch_to(7) {
                        trace!("Secondary window: cannot switch to tab 7: {e}");
                    } else {
                        win.tabs.active_tab_mut().active_pane_mut().bell_active = false;
                    }
                }
                KeyAction::SwitchToTab9 => {
                    if let Err(e) = win.tabs.switch_to(8) {
                        trace!("Secondary window: cannot switch to tab 8: {e}");
                    } else {
                        win.tabs.active_tab_mut().active_pane_mut().bell_active = false;
                    }
                }
                KeyAction::MoveTabLeft => win.tabs.move_active_left(),
                KeyAction::MoveTabRight => win.tabs.move_active_right(),

                // -- Font zoom --
                KeyAction::ZoomIn => {
                    let base = win.config.font.size;
                    let vs = &mut win.tabs.active_tab_mut().active_pane_mut().view_state;
                    vs.adjust_zoom(base, 1.0);
                    let effective = vs.effective_font_size(base);
                    win.terminal_widget(ui.ctx()).apply_font_zoom(effective);
                    // Invalidate atlases after zoom change.
                    for tab in win.tabs.iter_mut() {
                        if let Ok(panes_list) = tab.pane_tree.iter_panes_mut() {
                            for pane in panes_list {
                                pane.render_state
                                    .lock()
                                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                                    .clear_atlas();
                                pane.render_cache.invalidate_content();
                            }
                        }
                    }
                }
                KeyAction::ZoomOut => {
                    let base = win.config.font.size;
                    let vs = &mut win.tabs.active_tab_mut().active_pane_mut().view_state;
                    vs.adjust_zoom(base, -1.0);
                    let effective = vs.effective_font_size(base);
                    win.terminal_widget(ui.ctx()).apply_font_zoom(effective);
                    // Invalidate atlases after zoom change.
                    for tab in win.tabs.iter_mut() {
                        if let Ok(panes_list) = tab.pane_tree.iter_panes_mut() {
                            for pane in panes_list {
                                pane.render_state
                                    .lock()
                                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                                    .clear_atlas();
                                pane.render_cache.invalidate_content();
                            }
                        }
                    }
                }
                KeyAction::ZoomReset => {
                    let base = win.config.font.size;
                    win.tabs
                        .active_tab_mut()
                        .active_pane_mut()
                        .view_state
                        .reset_zoom();
                    win.terminal_widget(ui.ctx()).apply_font_zoom(base);
                    // Invalidate atlases after zoom reset.
                    for tab in win.tabs.iter_mut() {
                        if let Ok(panes_list) = tab.pane_tree.iter_panes_mut() {
                            for pane in panes_list {
                                pane.render_state
                                    .lock()
                                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                                    .clear_atlas();
                                pane.render_cache.invalidate_content();
                            }
                        }
                    }
                }

                // -- Search --
                KeyAction::OpenSearch => {
                    win.tabs
                        .active_tab_mut()
                        .active_pane_mut()
                        .view_state
                        .search_state
                        .is_open = true;
                }
                KeyAction::SearchNext => {
                    let tab = win.tabs.active_tab_mut();
                    let pane = tab.active_pane_mut();
                    pane.view_state.search_state.next_match();
                    let snap = pane.arc_swap.load();
                    search::scroll_to_match_and_send(&mut pane.view_state, &snap, &pane.input_tx);
                }
                KeyAction::SearchPrev => {
                    let tab = win.tabs.active_tab_mut();
                    let pane = tab.active_pane_mut();
                    pane.view_state.search_state.prev_match();
                    let snap = pane.arc_swap.load();
                    search::scroll_to_match_and_send(&mut pane.view_state, &snap, &pane.input_tx);
                }
                KeyAction::PrevCommand => {
                    let tab = win.tabs.active_tab_mut();
                    let pane = tab.active_pane_mut();
                    let snap = pane.arc_swap.load();
                    search::jump_to_prev_command(&mut pane.view_state, &snap);
                }
                KeyAction::NextCommand => {
                    let tab = win.tabs.active_tab_mut();
                    let pane = tab.active_pane_mut();
                    let snap = pane.arc_swap.load();
                    search::jump_to_next_command(&mut pane.view_state, &snap);
                }

                // -- Split pane management --
                KeyAction::SplitVertical => {
                    spawn_split_pane_in_secondary(win, panes::SplitDirection::Horizontal, ui);
                }
                KeyAction::SplitHorizontal => {
                    spawn_split_pane_in_secondary(win, panes::SplitDirection::Vertical, ui);
                }
                KeyAction::ResizePaneLeft => {
                    let id = win.tabs.active_tab().active_pane;
                    if let Err(e) = win.tabs.active_tab_mut().pane_tree.resize_split(
                        id,
                        panes::SplitDirection::Horizontal,
                        -0.05,
                    ) {
                        trace!("Secondary window: cannot resize pane left: {e}");
                    }
                }
                KeyAction::ResizePaneRight => {
                    let id = win.tabs.active_tab().active_pane;
                    if let Err(e) = win.tabs.active_tab_mut().pane_tree.resize_split(
                        id,
                        panes::SplitDirection::Horizontal,
                        0.05,
                    ) {
                        trace!("Secondary window: cannot resize pane right: {e}");
                    }
                }
                KeyAction::ResizePaneUp => {
                    let id = win.tabs.active_tab().active_pane;
                    if let Err(e) = win.tabs.active_tab_mut().pane_tree.resize_split(
                        id,
                        panes::SplitDirection::Vertical,
                        -0.05,
                    ) {
                        trace!("Secondary window: cannot resize pane up: {e}");
                    }
                }
                KeyAction::ResizePaneDown => {
                    let id = win.tabs.active_tab().active_pane;
                    if let Err(e) = win.tabs.active_tab_mut().pane_tree.resize_split(
                        id,
                        panes::SplitDirection::Vertical,
                        0.05,
                    ) {
                        trace!("Secondary window: cannot resize pane down: {e}");
                    }
                }
                KeyAction::ZoomPane => {
                    let tab = win.tabs.active_tab_mut();
                    let current = tab.active_pane;
                    if tab.zoomed_pane == Some(current) {
                        tab.zoomed_pane = None;
                    } else {
                        tab.zoomed_pane = Some(current);
                    }
                    // Reset last_sent_size so resize fires next frame.
                    let tab = win.tabs.active_tab_mut();
                    if let Ok(panes_list) = tab.pane_tree.iter_panes_mut() {
                        for pane in panes_list {
                            pane.view_state.last_sent_size = (0, 0);
                        }
                    }
                }

                // -- Window management --
                KeyAction::NewWindow => {
                    // Cannot call spawn_new_window directly (no FreminalGui ref).
                    // Set a flag; the root window's pruning loop will consume it.
                    win.pending_new_window = true;
                }

                // -- Not yet implemented in secondary windows --
                KeyAction::OpenSettings | KeyAction::RenameTab => {
                    trace!("Secondary window: no-op deferred action: {action:?}");
                }

                // These actions are handled at the input layer and should never
                // reach the deferred dispatch.
                _ => {
                    trace!("Secondary window: unhandled deferred action: {action:?}");
                }
            }
        }

        // Handle deferred close-pane.
        if win.pending_close_pane {
            win.pending_close_pane = false;
            let tab = win.tabs.active_tab_mut();
            let target = tab.active_pane;
            if tab.zoomed_pane == Some(target) {
                tab.zoomed_pane = None;
            }
            match tab.pane_tree.close(target) {
                Ok(_) => {
                    let tab = win.tabs.active_tab_mut();
                    if let Ok(panes_list) = tab.pane_tree.iter_panes_mut() {
                        for pane in panes_list {
                            pane.view_state.last_sent_size = (0, 0);
                        }
                    }
                    let tab = win.tabs.active_tab_mut();
                    if let Ok(panes_list) = tab.pane_tree.iter_panes()
                        && let Some(first) = panes_list.first()
                    {
                        let new_id = first.id;
                        if let Err(e) = first.input_tx.send(InputEvent::FocusChange(true)) {
                            error!("Secondary window: FocusChange(true) to {new_id}: {e}");
                        }
                        tab.active_pane = new_id;
                    }
                }
                Err(panes::PaneError::CannotCloseLastPane) => {
                    if win.tabs.tab_count() <= 1 {
                        win.closed = true;
                        ui.ctx().send_viewport_cmd(ViewportCommand::Close);
                        return;
                    }
                    if let Err(e) = win.tabs.close_active_tab() {
                        error!("Secondary window: close tab: {e}");
                    }
                }
                Err(e) => {
                    error!("Secondary window: close pane: {e}");
                }
            }
        }

        // Directional focus.
        if let Some(dir) = win.pending_focus_direction.take() {
            use freminal_common::keybindings::KeyAction;
            // Use the same helper used by the root window.
            // We pass available_rect which is captured above.
            let tab = win.tabs.active_tab_mut();
            let active = tab.active_pane;
            let layout = tab.pane_tree.layout(available_rect).unwrap_or_default();
            let target = layout
                .iter()
                .find(|(_, rect)| {
                    let curr_rect = layout.iter().find(|(id, _)| *id == active).map(|(_, r)| *r);
                    curr_rect.is_some_and(|curr| match dir {
                        KeyAction::FocusPaneLeft => {
                            rect.max.x <= curr.min.x + 1.0
                                && rect.min.y < curr.max.y
                                && rect.max.y > curr.min.y
                        }
                        KeyAction::FocusPaneRight => {
                            rect.min.x >= curr.max.x - 1.0
                                && rect.min.y < curr.max.y
                                && rect.max.y > curr.min.y
                        }
                        KeyAction::FocusPaneUp => {
                            rect.max.y <= curr.min.y + 1.0
                                && rect.min.x < curr.max.x
                                && rect.max.x > curr.min.x
                        }
                        KeyAction::FocusPaneDown => {
                            rect.min.y >= curr.max.y - 1.0
                                && rect.min.x < curr.max.x
                                && rect.max.x > curr.min.x
                        }
                        _ => false,
                    })
                })
                .map(|(id, _)| *id);
            if let Some(new_id) = target {
                if let Some(old_pane) = tab.pane_tree.find(active)
                    && let Err(e) = old_pane.input_tx.send(InputEvent::FocusChange(false))
                {
                    error!("Secondary window: FocusChange(false) to {active}: {e}");
                }
                tab.active_pane = new_id;
                if let Some(new_pane) = tab.pane_tree.find(new_id)
                    && let Err(e) = new_pane.input_tx.send(InputEvent::FocusChange(true))
                {
                    error!("Secondary window: FocusChange(true) to {new_id}: {e}");
                }
            }
        }

        // Window title.
        let active_title = &win.tabs.active_tab().active_pane().title;
        let window_title = if active_title.is_empty() {
            "Freminal"
        } else {
            active_title.as_str()
        };
        if window_title != win.last_window_title {
            window_title.clone_into(&mut win.last_window_title);
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::Title(win.last_window_title.clone()));
        }

        // Schedule repaint.
        if let Some(delay) = shortest_repaint_delay {
            ui.ctx().request_repaint_after(delay);
        }
    });
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
    window_post: Arc<Mutex<WindowPostRenderer>>,
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

    // Set the application identifier so that Wayland compositors associate
    // our xdg_toplevel with the "freminal.desktop" entry (matching
    // StartupWMClass=freminal).  On X11 winit already derives WM_CLASS
    // from argv[0], but setting this explicitly ensures consistent behavior
    // across both display servers.
    native_options.viewport.app_id = Some("freminal".into());

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
                window_post,
                #[cfg(feature = "playback")]
                is_playback,
            )))
        }),
    ) {
        Ok(()) => Ok(()),
        Err(e) => Err(anyhow::anyhow!(e.to_string())),
    }
}

#[cfg(test)]
mod secondary_window_tests {
    use std::sync::atomic::Ordering;

    use freminal_common::keybindings::{
        BindingKey, BindingMap, BindingModifiers, KeyAction, KeyCombo,
    };

    use super::NEXT_VIEWPORT_ID;

    // ── NEXT_VIEWPORT_ID counter ────────────────────────────────────────────

    /// Each call to `fetch_add` on `NEXT_VIEWPORT_ID` must return a strictly
    /// increasing value, guaranteeing that concurrent windows never share a
    /// viewport ID.
    #[test]
    fn next_viewport_id_increases_monotonically() {
        let a = NEXT_VIEWPORT_ID.fetch_add(0, Ordering::Relaxed);
        let b = NEXT_VIEWPORT_ID.fetch_add(1, Ordering::Relaxed);
        let c = NEXT_VIEWPORT_ID.fetch_add(1, Ordering::Relaxed);
        // b == a (we only peeked with +0), and c == b + 1
        assert_eq!(b, a);
        assert_eq!(c, b + 1);
    }

    /// Two successive `fetch_add(1)` calls must produce distinct values so
    /// that two windows opened back-to-back get different viewport IDs.
    #[test]
    fn next_viewport_id_successive_calls_are_distinct() {
        let id1 = NEXT_VIEWPORT_ID.fetch_add(1, Ordering::Relaxed);
        let id2 = NEXT_VIEWPORT_ID.fetch_add(1, Ordering::Relaxed);
        assert_ne!(id1, id2);
        assert_eq!(id2, id1 + 1);
    }

    // ── NewWindow keybinding ────────────────────────────────────────────────

    /// `KeyAction::NewWindow` must appear in `KeyAction::ALL` so that the
    /// settings modal and key-binding serialisation can discover it.
    #[test]
    fn new_window_action_is_in_all() {
        assert!(
            KeyAction::ALL.contains(&KeyAction::NewWindow),
            "KeyAction::NewWindow missing from KeyAction::ALL"
        );
    }

    /// The `name()` method must return the canonical TOML key used in
    /// `config_example.toml` and written by the settings modal.
    #[test]
    fn new_window_action_name() {
        assert_eq!(KeyAction::NewWindow.name(), "new_window");
    }

    /// The `display_label()` must be a human-readable string for the UI.
    #[test]
    fn new_window_action_display_label() {
        assert_eq!(KeyAction::NewWindow.display_label(), "New Window");
    }

    /// `FromStr` round-trip: parsing the canonical name must recover the
    /// `NewWindow` variant.
    #[test]
    fn new_window_action_from_str_round_trips() {
        use std::str::FromStr;
        let Ok(parsed) = KeyAction::from_str("new_window") else {
            panic!("parse failed")
        };
        assert_eq!(parsed, KeyAction::NewWindow);
    }

    // ── Default binding ─────────────────────────────────────────────────────

    /// `BindingMap::default()` must bind `Ctrl+Shift+N` to `NewWindow`.
    /// This is the advertised default in `config_example.toml`.
    #[test]
    fn default_binding_map_contains_new_window() {
        let map = BindingMap::default();
        let combo = KeyCombo::new(BindingKey::N, BindingModifiers::CTRL_SHIFT);
        let action = map.lookup(&combo);
        assert_eq!(
            action,
            Some(KeyAction::NewWindow),
            "Ctrl+Shift+N should be bound to NewWindow in the default map"
        );
    }

    /// `NewWindow` must be discoverable by action — the reverse lookup from
    /// action to combo must return a non-empty list so the settings modal can
    /// display the current binding.
    #[test]
    fn default_binding_map_new_window_is_discoverable() {
        let map = BindingMap::default();
        let combos = map.all_combos_for(KeyAction::NewWindow);
        assert!(
            !combos.is_empty(),
            "NewWindow must have at least one combo in the default binding map"
        );
    }

    // ── Args Clone ──────────────────────────────────────────────────────────

    /// `Args` must implement `Clone` so that `SecondaryWindowState` can hold
    /// an independent copy without sharing a reference.  This test is a
    /// compile-time check disguised as a runtime assertion.
    #[test]
    fn args_implements_clone() {
        use clap::Parser;
        use freminal_common::args::Args;
        // Parse from an empty argv (just the program name) to get a default Args.
        let args = Args::parse_from(["freminal"]);
        // Clone into a separate binding to verify the trait is implemented.
        // The clone is used so the compiler doesn't elide it.
        let cloned = args.clone();
        assert_eq!(cloned.show_all_debug, args.show_all_debug);
        // If Args does not derive Clone this file will not compile.
    }
}
