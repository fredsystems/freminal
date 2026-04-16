// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use std::sync::{Arc, Mutex, OnceLock, atomic::AtomicU64};

use crate::gui::colors::internal_color_to_egui_with_alpha;
use anyhow::Result;
use conv2::{ApproxFrom, ConvUtil, ValueFrom};
use eframe::egui::{self, CentralPanel, Panel, ViewportBuilder, ViewportCommand, ViewportId};
use eframe::egui_glow::CallbackFn;
use freminal_common::args::Args;
use freminal_common::config::{Config, TabBarPosition, ThemeMode};
use freminal_terminal_emulator::io::InputEvent;
#[cfg(feature = "playback")]
use freminal_terminal_emulator::io::PlaybackMode;
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

mod actions;
mod hot_reload;
mod menu;
mod rendering;
pub(crate) mod window;

use tracing::{debug, error, trace};

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

// `os_dark_mode`, `pending_close_pane`, `pending_new_window`,
// `show_close_confirmation`, and `closing_all` are distinct boolean flags
// with independent semantics; grouping them into an enum or sub-struct
// would obscure their purpose without adding clarity.
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
    secondary_windows: Vec<(ViewportId, Arc<Mutex<window::SecondaryWindowState>>)>,

    /// Set to `true` by the `NewWindow` key action or "Window → New Window" menu;
    /// consumed at the start of `ui()` where `egui::Context` is available.
    pending_new_window: bool,

    /// When `true`, a confirmation dialog is shown asking the user whether to
    /// close all windows.  Set by any close path (OS close button, Menu → Quit,
    /// last pane/tab closed) when secondary windows are still open.  The dialog
    /// offers "Close All" (terminates every window) or "Cancel" (dismisses).
    show_close_confirmation: bool,

    /// Set to `true` by the "Close All" confirmation button.  Prevents the
    /// close-intercept logic from re-cancelling the `ViewportCommand::Close`
    /// that was just issued.  Without this, the intercept sees `close_requested`
    /// on the next frame, finds secondary windows still in the list (not yet
    /// pruned), and sends `CancelClose` — defeating the intentional shutdown.
    closing_all: bool,

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
        rendering::set_egui_options(&cc.egui_ctx, initial_theme, config.ui.background_opacity);

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
            show_close_confirmation: false,
            closing_all: false,
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
        // show a confirmation dialog instead of closing immediately — eframe
        // ties app lifecycle to ViewportId::ROOT so closing it would destroy
        // all windows.
        //
        // Skip the intercept when `closing_all` is true — that means the user
        // already confirmed "Close All" and the `ViewportCommand::Close` we
        // sent should be honoured, not re-cancelled.
        let close_requested = ui.ctx().input(|i| i.viewport().close_requested());
        if close_requested && !self.closing_all {
            if self.secondary_windows.is_empty() {
                // No other windows — let the close proceed normally.
            } else {
                // Cancel the OS close and show a confirmation dialog.
                ui.ctx().send_viewport_cmd(ViewportCommand::CancelClose);
                self.show_close_confirmation = true;
            }
        }

        // ── Secondary window management ───────────────────────────────────────
        // Re-register all live secondary windows on every frame.  egui's deferred
        // viewport API requires the closure to be supplied every frame; stopping
        // means the window is destroyed.  We also prune entries whose windows have
        // requested close (detected inside run_secondary_window_frame).
        {
            let ctx = ui.ctx().clone();
            let mut live: Vec<(ViewportId, Arc<Mutex<window::SecondaryWindowState>>)> = Vec::new();
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
                        window::run_secondary_window_frame(&mut win, ui);
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

        // ── Close-all confirmation dialog ─────────────────────────────────────
        // When the user attempted to close the root window (or pressed Quit)
        // while secondary windows are still open, show a modal confirmation.
        if self.show_close_confirmation {
            let mut keep_open = true;
            let mut confirmed_close = false;
            egui::Window::new("Close All Windows?")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ui.ctx(), |ui| {
                    ui.label("Other windows are still open. Close all windows?");
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Close All").clicked() {
                            // Mark all secondary windows as closed so the pruning
                            // loop stops re-registering them.
                            for (_, state) in &self.secondary_windows {
                                if let Ok(mut win) = state.lock() {
                                    win.closed = true;
                                }
                            }
                            confirmed_close = true;
                            keep_open = false;
                        }
                        if ui.button("Cancel").clicked() {
                            keep_open = false;
                        }
                    });
                });
            if confirmed_close {
                // Set `closing_all` BEFORE sending `ViewportCommand::Close` so
                // the close intercept at the top of `ui()` won't re-cancel it
                // on the next frame.
                self.closing_all = true;
                ui.ctx().send_viewport_cmd(ViewportCommand::Close);
            }
            if !keep_open {
                self.show_close_confirmation = false;
            }
            // Request a repaint so the dialog remains interactive.
            ui.ctx().request_repaint();
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
                    rendering::update_egui_theme(
                        ui.ctx(),
                        theme,
                        self.config.ui.background_opacity,
                    );
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
                        rendering::handle_window_manipulation(
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
                    rendering::update_egui_theme(ui.ctx(), theme, new_cfg.ui.background_opacity);
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
                    rendering::update_egui_theme(
                        ui.ctx(),
                        theme,
                        self.config.ui.background_opacity,
                    );
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
                    rendering::update_egui_theme(ui.ctx(), theme, original_opacity);
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

    /// `fetch_add` on `NEXT_VIEWPORT_ID` must return strictly increasing
    /// values, guaranteeing that concurrent windows never share a viewport
    /// ID.  Successive `fetch_add(1)` calls must produce distinct values.
    ///
    /// Combined into a single test because `NEXT_VIEWPORT_ID` is a process-
    /// global `AtomicU64` — two tests asserting exact adjacency would race
    /// when Rust runs tests in parallel.
    #[test]
    fn next_viewport_id_increases_monotonically_and_is_distinct() {
        let a = NEXT_VIEWPORT_ID.fetch_add(0, Ordering::Relaxed);
        let b = NEXT_VIEWPORT_ID.fetch_add(1, Ordering::Relaxed);
        let c = NEXT_VIEWPORT_ID.fetch_add(1, Ordering::Relaxed);
        // b == a (we only peeked with +0), then c == b + 1
        assert_eq!(b, a);
        assert_eq!(c, b + 1);

        // Two successive fetch_add(1) calls must produce distinct values.
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
