// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use std::sync::{Arc, Mutex, OnceLock, atomic::Ordering};

use conv2::{ApproxFrom, ConvUtil, ValueFrom};
use eframe::egui::{self, CentralPanel, Panel, ViewportBuilder, ViewportCommand, ViewportId};
use eframe::egui_glow::CallbackFn;
use freminal_common::args::Args;
use freminal_common::config::{Config, TabBarPosition};
use freminal_terminal_emulator::io::InputEvent;
use glow::HasContext;
use tracing::{debug, error, trace};

use super::rendering::handle_window_manipulation;
use super::{
    FreminalGui, NEXT_VIEWPORT_ID, PaneBorderDrag, TabBarAction, panes,
    renderer::WindowPostRenderer,
    search, tabs, terminal,
    terminal::{FreminalTerminalWidget, new_render_state},
    view_state,
};
use crate::gui::colors::internal_color_to_egui_with_alpha;

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
pub(super) struct SecondaryWindowState {
    /// All open terminal tabs for this window.
    pub(super) tabs: tabs::TabManager,

    /// Last title string sent to the OS window via `ViewportCommand::Title`.
    pub(super) last_window_title: String,

    /// Cached egui style inputs — prevents redundant `global_style_mut` calls.
    pub(super) style_cache: Option<(bool, &'static freminal_common::themes::ThemePalette, f32)>,

    /// Set `true` by `ClosePane` action; consumed at the end of the frame.
    pub(super) pending_close_pane: bool,

    /// Pending directional focus change; consumed at the end of the frame.
    pub(super) pending_focus_direction: Option<freminal_common::keybindings::KeyAction>,

    /// Active pane border drag state (mouse drag-to-resize).
    pub(super) border_drag: Option<PaneBorderDrag>,

    /// Lazily initialised on the first frame (needs `egui::Context`).
    /// `None` until the deferred viewport closure runs for the first time.
    pub(super) terminal_widget: Option<FreminalTerminalWidget>,

    /// Set to `true` when the OS close button is clicked or the window is
    /// programmatically closed.  The root window's pruning loop checks this
    /// flag and stops calling `show_viewport_deferred` for this entry,
    /// causing egui to destroy the OS window and drop the `Arc`, cleaning
    /// up all PTY threads and resources.
    pub(super) closed: bool,

    /// Set to `true` by the `NewWindow` key action or "Window → New Window"
    /// menu inside this secondary window.  The root window's pruning loop
    /// consumes this flag and sets `self.pending_new_window = true` to
    /// trigger spawning of a new OS window from the root context.
    pub(super) pending_new_window: bool,

    // ── Shared resources (cloned / Arc'd from the root window) ──────────────
    /// A snapshot of the root window's `Config` at the time this window was
    /// opened.  Updated when the root applies settings changes.
    pub(super) config: Config,

    /// CLI arguments (needed for `spawn_pty_tab`).
    pub(super) args: Args,

    /// Binding map used to resolve key combos in this window.
    pub(super) binding_map: freminal_common::keybindings::BindingMap,

    /// Globally unique pane ID generator — shared so IDs are unique
    /// across all windows in the process.
    pub(super) pane_id_gen: Arc<Mutex<panes::PaneIdGenerator>>,

    /// Per-window post-processing renderer (FBO + custom shader).
    ///
    /// Each secondary window owns its own `WindowPostRenderer` so that pane
    /// `PaintCallback`s write into this window's FBO — not the root window's.
    /// Sharing the root's renderer would cause FBO corruption when both windows
    /// are visible simultaneously.
    pub(super) window_post: Arc<Mutex<WindowPostRenderer>>,

    /// Shared egui context handle (same `Arc<OnceLock<>>` as the root).
    /// Used when spawning new PTY tabs so their threads can request repaints.
    pub(super) egui_ctx: Arc<OnceLock<egui::Context>>,
}

impl SecondaryWindowState {
    /// Returns the terminal widget, initialising it from `ctx` on first call.
    pub(super) fn terminal_widget(&mut self, ctx: &egui::Context) -> &mut FreminalTerminalWidget {
        self.terminal_widget
            .get_or_insert_with(|| FreminalTerminalWidget::new(ctx, &self.config))
    }
}

impl super::FreminalGui {
    /// Open a new OS window with an initial PTY-backed tab.
    ///
    /// Creates a fresh [`SecondaryWindowState`] with a clone of the current
    /// `Config`, `BindingMap`, and `Args`, a shared `pane_id_gen` and
    /// `window_post`, and a new `TabManager` backed by a fresh PTY. The
    /// window is registered via [`egui::Context::show_viewport_deferred`]
    /// and will appear on the next frame. Does nothing in playback mode.
    pub(super) fn spawn_new_window(&mut self, ctx: &egui::Context) {
        // New windows are not supported in playback mode.
        #[cfg(feature = "playback")]
        if self.is_playback {
            return;
        }

        let theme =
            freminal_common::themes::by_slug(self.config.theme.active_slug(self.os_dark_mode))
                .unwrap_or(&freminal_common::themes::CATPPUCCIN_MOCHA);

        let channels = match super::pty::spawn_pty_tab(
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
        let initial_tab = tabs::Tab::new(tab_id, pane);

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
            tabs: tabs::TabManager::new(initial_tab),
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
}

/// Spawn a split pane in a secondary window's active tab.
///
/// Mirrors [`FreminalGui::spawn_split_pane`] but operates on a
/// [`SecondaryWindowState`] rather than the root GUI instance.
pub(super) fn spawn_split_pane_in_secondary(
    win: &mut SecondaryWindowState,
    direction: panes::SplitDirection,
    ui: &egui::Ui,
) {
    let os_dark_mode = ui.ctx().global_style().visuals.dark_mode;
    let theme = freminal_common::themes::by_slug(win.config.theme.active_slug(os_dark_mode))
        .unwrap_or(&freminal_common::themes::CATPPUCCIN_MOCHA);

    let channels = match super::pty::spawn_pty_tab(
        &win.args,
        win.config.scrollback.limit,
        theme,
        &win.egui_ctx,
    ) {
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
pub(super) fn run_secondary_window_frame(win: &mut SecondaryWindowState, ui: &mut egui::Ui) {
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
                        match super::pty::spawn_pty_tab(
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
                                let new_tab = tabs::Tab::new(tab_id, pane);
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
                match super::pty::spawn_pty_tab(
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
                        let new_tab = tabs::Tab::new(tab_id, pane);
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
                    match super::pty::spawn_pty_tab(
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
                            let new_tab = tabs::Tab::new(tab_id, pane);
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
