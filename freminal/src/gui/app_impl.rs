// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use std::sync::{Arc, Mutex, OnceLock};

use conv2::{ApproxFrom, ConvUtil, ValueFrom};
use egui::{self, CentralPanel, Panel, ViewportCommand};
use egui_glow::CallbackFn;
use freminal_common::config::ThemeMode;
use freminal_common::send_or_log;
use freminal_terminal_emulator::io::InputEvent;
use freminal_windowing::WindowId;
use glow::HasContext;
use tracing::{debug, error, trace, warn};

use super::colors::internal_color_to_egui_with_alpha;
use super::panes;
use super::renderer::WindowPostRenderer;
use super::rendering;
use super::tabs::{Tab, TabManager};
use super::terminal::{FreminalTerminalWidget, new_render_state};
use super::view_state;
use super::window::PerWindowState;
use super::{FreminalGui, PaneBorderDrag};

impl freminal_windowing::App for FreminalGui {
    /// Called when a window is created.
    ///
    /// For the first window, consumes `initial_state` to get the pre-spawned
    /// tab and widget.  For subsequent windows, spawns a fresh PTY tab.
    // Window creation handles two distinct paths (first window with pre-spawned state vs
    // subsequent windows with fresh PTY) that share no logic — splitting would not reduce
    // coupling and would obscure the flow.
    #[allow(clippy::too_many_lines)]
    fn on_window_created(
        &mut self,
        window_id: WindowId,
        ctx: &egui::Context,
        handle: &freminal_windowing::WindowHandle<'_>,
        inner_size: (u32, u32),
    ) {
        // ── Settings window ──────────────────────────────────────────────────
        if self.pending_settings_window {
            self.pending_settings_window = false;
            self.settings_window_id = Some(window_id);
            self.settings_owner = Some(window_id);
            // Don't create a PerWindowState — the settings window renders
            // only the settings UI via show_standalone().
            return;
        }

        let os_dark_mode = ctx.global_style().visuals.dark_mode;

        if let Some(initial) = self.initial_state.take() {
            // First window — use the pre-spawned tab and widget.
            let proxy = handle.event_loop_proxy();
            let _ = initial.repaint_handle.set((proxy, window_id));

            let initial_theme =
                freminal_common::themes::by_slug(self.config.theme.active_slug(os_dark_mode))
                    .unwrap_or(&freminal_common::themes::CATPPUCCIN_MOCHA);
            rendering::set_egui_options(ctx, initial_theme, self.config.ui.background_opacity);

            // Re-create terminal widget with real egui context for correct
            // font registration and DPI scaling.
            let terminal_widget =
                FreminalTerminalWidget::new(ctx, &self.config).unwrap_or_else(|e| {
                    tracing::error!(
                        "fatal: failed to initialise terminal widget (font manager): {e}"
                    );
                    std::process::exit(1);
                });

            // Send an immediate resize to the PTY so the shell starts at the
            // correct dimensions instead of the pre-spawn defaults (100x100).
            let (cell_w, cell_h) = terminal_widget.cell_size();
            let computed_size =
                Self::compute_initial_size(inner_size.0, inner_size.1, cell_w, cell_h);
            let cell_pixel_w = cell_w.value_as::<usize>().unwrap_or(0);
            let cell_pixel_h = cell_h.value_as::<usize>().unwrap_or(0);
            if let Ok(panes) = initial.tab.pane_tree.iter_panes() {
                for pane in panes {
                    send_or_log!(
                        pane.input_tx,
                        InputEvent::Resize(
                            computed_size.width,
                            computed_size.height,
                            cell_pixel_w,
                            cell_pixel_h,
                        ),
                        "Failed to send initial resize to pre-spawned pane"
                    );
                }
            }

            // Correct the theme for auto mode if the real OS preference differs
            // from the assumed-light default used during construction.
            if os_dark_mode
                && let Some(theme) =
                    freminal_common::themes::by_slug(self.config.theme.active_slug(os_dark_mode))
                && let Ok(panes) = initial.tab.pane_tree.iter_panes()
            {
                for pane in panes {
                    send_or_log!(
                        pane.input_tx,
                        InputEvent::ThemeChange(theme),
                        "Failed to send corrective ThemeChange"
                    );
                    send_or_log!(
                        pane.input_tx,
                        InputEvent::ThemeModeUpdate(self.config.theme.mode, os_dark_mode,),
                        "Failed to send ThemeModeUpdate"
                    );
                }
            }

            let win = PerWindowState {
                tabs: TabManager::new(initial.tab),
                terminal_widget,
                last_window_title: String::from("Freminal"),
                os_dark_mode,
                style_cache: None,
                pending_close_pane: false,
                pending_focus_direction: None,
                border_drag: None,
                shader_last_mtime: None,
                window_post: initial.window_post,
                repaint_handle: initial.repaint_handle,
                pending_new_window: false,
                pending_geometry: None,
                last_known_size: None,
                last_known_position: None,
            };
            self.windows.insert(window_id, win);

            // If --layout was given on the CLI, apply it to this first window.
            // Fall through to config.startup.layout when absent.
            let startup_layout_name = self
                .args
                .layout
                .clone()
                .or_else(|| self.config.startup.layout.clone());

            if let Some(ref name_or_path) = startup_layout_name {
                // Resolve bare name (e.g. "dev") to library path; treat anything
                // with a path separator or .toml suffix as a literal path.
                let path = {
                    let p = std::path::Path::new(name_or_path.as_str());
                    if p.extension().is_some_and(|e| e == "toml") || p.components().count() > 1 {
                        p.to_path_buf()
                    } else {
                        freminal_common::config::layout_library_dir().map_or_else(
                            || p.to_path_buf(),
                            |d| d.join(format!("{name_or_path}.toml")),
                        )
                    }
                };
                let positional: Vec<String> = self
                    .args
                    .layout_vars
                    .iter()
                    .filter(|s| !s.contains('='))
                    .cloned()
                    .collect();
                let var_map = self.args.layout_var_map();
                match freminal_common::layout::Layout::from_file(&path) {
                    Ok(layout) => match layout.apply_variables(&positional, &var_map).resolve() {
                        Ok(resolved) => {
                            let cmds = self.apply_layout(&resolved, window_id, handle);
                            self.inject_layout_commands(&cmds);
                        }
                        Err(e) => {
                            error!("Failed to resolve layout '{}': {e}", path.display());
                        }
                    },
                    Err(e) => {
                        error!("Failed to load layout '{}': {e}", path.display());
                    }
                }
            } else {
                // No --layout CLI flag and no startup.layout — try to restore
                // the last session if configured.
                self.maybe_restore_last_session(window_id, handle);
            }

            // Emit WindowCreate recording event.
            let rec_wid = self.recording_window_id(window_id);
            if let Some(ref h) = self.recording_handle {
                h.emit(
                    freminal_terminal_emulator::recording::EventPayload::WindowCreate {
                        window_id: rec_wid,
                        width_px: inner_size.0,
                        height_px: inner_size.1,
                        x: 0,
                        y: 0,
                    },
                );
            }
        } else {
            // Subsequent window — check if a layout window is waiting, otherwise
            // spawn a default single-pane PTY tab.
            if !self.pending_layout_windows.is_empty() {
                if let Some(cmds) = self.build_window_from_pending_layout(
                    window_id,
                    ctx,
                    handle,
                    inner_size,
                    os_dark_mode,
                ) {
                    self.inject_layout_commands(&cmds);
                }
                return;
            }

            // Subsequent window — spawn a new PTY tab.
            let theme =
                freminal_common::themes::by_slug(self.config.theme.active_slug(os_dark_mode))
                    .unwrap_or(&freminal_common::themes::CATPPUCCIN_MOCHA);
            rendering::set_egui_options(ctx, theme, self.config.ui.background_opacity);

            let repaint_handle = Arc::new(OnceLock::new());
            let proxy = handle.event_loop_proxy();
            let _ = repaint_handle.set((proxy, window_id));

            let window_post = Arc::new(Mutex::new(WindowPostRenderer::new()));

            let terminal_widget =
                FreminalTerminalWidget::new(ctx, &self.config).unwrap_or_else(|e| {
                    tracing::error!(
                        "fatal: failed to initialise terminal widget (font manager): {e}"
                    );
                    std::process::exit(1);
                });
            let (cell_w, cell_h) = terminal_widget.cell_size();
            let initial_size =
                Self::compute_initial_size(inner_size.0, inner_size.1, cell_w, cell_h);

            let pane_id = self
                .pane_id_gen
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .next_id();

            match super::pty::spawn_pty_tab(
                &self.args,
                self.config.scrollback.limit,
                theme,
                &repaint_handle,
                initial_size,
                super::pty::PtyTabConfig {
                    cwd: None,
                    shell_override: None,
                    extra_env: None,
                    recording_handle: self.recording_handle.clone(),
                    recording_pane_id: pane_id.raw().try_into().unwrap_or(u32::MAX),
                },
            ) {
                Ok(channels) => {
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
                        child_pid: channels.child_pid,
                        render_state: new_render_state(Arc::clone(&window_post)),
                        render_cache: super::terminal::PaneRenderCache::new(),
                    };
                    let tab_id = super::tabs::TabId::first();
                    let tab = Tab::new(tab_id, pane);

                    if let Some(active) = tab.active_pane() {
                        if let Err(e) = active.input_tx.send(InputEvent::ThemeModeUpdate(
                            self.config.theme.mode,
                            os_dark_mode,
                        )) {
                            error!("Failed to send ThemeModeUpdate to new window tab: {e}");
                        }
                    } else {
                        warn!("new window tab has no active pane when sending ThemeModeUpdate");
                    }

                    // Copy shader from config if present.
                    let shader_src = self
                        .config
                        .shader
                        .path
                        .as_ref()
                        .and_then(|p| std::fs::read_to_string(p).ok());
                    if let Some(src) = shader_src
                        && let Ok(mut wpr) = window_post.lock()
                    {
                        wpr.pending_shader = Some(Some(src));
                    }

                    // Copy bg image if present.
                    let bg_path = self.config.ui.background_image.clone();
                    if bg_path.is_some()
                        && let Ok(panes_list) = tab.pane_tree.iter_panes()
                    {
                        for p in panes_list {
                            if let Ok(mut rs) = p.render_state.lock() {
                                rs.set_pending_bg_image(bg_path.clone());
                            }
                        }
                    }

                    let win = PerWindowState {
                        tabs: TabManager::new(tab),
                        terminal_widget,
                        last_window_title: String::from("Freminal"),
                        os_dark_mode,
                        style_cache: None,
                        pending_close_pane: false,
                        pending_focus_direction: None,
                        border_drag: None,
                        shader_last_mtime: None,
                        window_post,
                        repaint_handle,
                        pending_new_window: false,
                        pending_geometry: None,
                        last_known_size: None,
                        last_known_position: None,
                    };
                    self.windows.insert(window_id, win);

                    // Emit WindowCreate recording event.
                    let rec_wid = self.recording_window_id(window_id);
                    if let Some(ref h) = self.recording_handle {
                        h.emit(
                            freminal_terminal_emulator::recording::EventPayload::WindowCreate {
                                window_id: rec_wid,
                                width_px: inner_size.0,
                                height_px: inner_size.1,
                                x: 0,
                                y: 0,
                            },
                        );
                    }
                }
                Err(e) => {
                    error!("Failed to spawn PTY for new window: {e}");
                }
            }
        }
    }

    /// Called when a window close is requested.
    ///
    /// Removes the window's state — its PTY threads will be dropped when
    /// the channels close.  Always returns `true` to allow the close.
    fn on_close_requested(&mut self, window_id: WindowId) -> bool {
        // Settings window closed (via OS close button).
        if self.settings_window_id == Some(window_id) {
            self.settings_modal.is_open = false;
            self.settings_window_id = None;
            self.settings_owner = None;
            self.persist_window_state();
            return true;
        }
        // If this window owns the settings modal, close it.
        if self.settings_owner == Some(window_id) {
            self.settings_modal.is_open = false;
            self.settings_owner = None;
        }

        // Auto-save session before the last terminal window is removed.
        // We check *before* remove so we still have access to the window's tabs.
        //
        // Saving is independent of `restore_last_session` — the flag only
        // controls whether the saved session is *applied* on next launch.
        // Always writing keeps `_last_session.toml` fresh so users can
        // toggle the flag on at any time and get their real last session
        // back, rather than whatever stale state happened to be on disk
        // when they last had the flag enabled.
        //
        // We still skip saving when the user launched with an ad-hoc
        // command (`freminal -- vim foo`): those panes are running a
        // one-shot program and are not meaningfully restorable.
        let remaining_terminal_windows = self
            .windows
            .keys()
            .filter(|&&wid| Some(wid) != self.settings_window_id)
            .count();
        if remaining_terminal_windows == 1 && self.args.command.is_empty() {
            self.auto_save_session();
        }

        // Capture geometry of every still-open terminal window (including
        // the one being closed) into `window_state.main_windows`, with the
        // closing window first so it seeds the primary window on next
        // launch.  Persist unconditionally — this is independent of
        // `restore_last_session`.
        self.snapshot_main_window_geometry(Some(window_id));
        self.persist_window_state();

        self.windows.remove(&window_id);

        // Emit WindowClose recording event (only for known windows), and clean up the mapping.
        if let Some(rec_wid) = self.recording_window_ids.remove(&window_id)
            && let Some(ref h) = self.recording_handle
        {
            h.emit(
                freminal_terminal_emulator::recording::EventPayload::WindowClose {
                    window_id: rec_wid,
                },
            );
        }

        true
    }

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
    fn clear_color(&self, window_id: WindowId) -> [f32; 4] {
        // Settings window: use a neutral opaque background.
        if self.settings_window_id == Some(window_id) {
            return [0.2, 0.2, 0.2, 1.0];
        }
        if self.config.ui.background_opacity < 1.0 {
            [0.0, 0.0, 0.0, 0.0]
        } else {
            // Fully opaque: use the terminal background color from the theme.
            let os_dark_mode = self.windows.get(&window_id).is_some_and(|w| w.os_dark_mode);
            let theme =
                freminal_common::themes::by_slug(self.config.theme.active_slug(os_dark_mode))
                    .unwrap_or(&freminal_common::themes::CATPPUCCIN_MOCHA);
            let (r, g, b) = theme.background;
            let color = egui::Color32::from_rgb(r, g, b);
            color.to_normalized_gamma_f32()
        }
    }

    // Inherently large: the main per-frame UI function handles menu bar, settings modal, window
    // manipulation drain, terminal widget layout, and resize detection — all in one pass over
    // the shared snapshot. Artificial sub-functions would not reduce the coupling.
    #[allow(clippy::too_many_lines)]
    fn update(
        &mut self,
        window_id: WindowId,
        ctx: &egui::Context,
        _gl: &glow::Context,
        handle: &freminal_windowing::WindowHandle<'_>,
    ) {
        trace!("Starting new frame");
        let now = std::time::Instant::now();

        // ── Settings window rendering ────────────────────────────────────────
        // If this update is for the settings window, render settings directly
        // and return — no terminal state to process.
        if self.settings_window_id == Some(window_id) {
            let os_dark = ctx.global_style().visuals.dark_mode;
            // Sync discovered layout list into the modal each frame so the
            // Startup tab always shows fresh data.
            self.settings_modal.discovered_layouts = self.discovered_layouts.clone();
            let settings_action = self.settings_modal.show_standalone(ctx, os_dark);
            self.handle_settings_action(&settings_action, handle, window_id);

            // Track the settings window's current geometry so we can restore
            // it the next time it is opened.  We query the windowing layer
            // directly rather than `ctx.input().viewport()` because the
            // latter only populates `inner_rect` / `outer_rect` after a
            // Resized / Moved event reaches the window's egui context, which
            // is not guaranteed on the first frame of a freshly created
            // window on every platform.  The windowing layer always tracks
            // live geometry from winit events + direct window queries.
            if let Some(geom) = handle.window_geometry(window_id) {
                if let Some(size) = geom.size {
                    self.window_state.settings.size = Some(<[u32; 2]>::from(size));
                }
                if let Some(pos) = geom.position {
                    self.window_state.settings.position = Some(<[i32; 2]>::from(pos));
                }
            }

            // If the modal closed (Cancel or Apply), close the OS window.
            if !self.settings_modal.is_open {
                self.persist_window_state();
                self.settings_window_id = None;
                self.settings_owner = None;
                handle.close_window(window_id);
            }
            return;
        }

        // ── Focus or create settings window (deferred from menu/keybind) ─────
        if self.pending_focus_settings {
            self.pending_focus_settings = false;
            if let Some(sid) = self.settings_window_id {
                handle.focus_window(sid);
            }
        }
        if self.pending_settings_window && self.settings_window_id.is_none() {
            // Don't clear pending_settings_window here — cleared in on_window_created.
            // Seed inner_size and position from the last-known geometry so the
            // window reopens where the user left it (both within a session and
            // across sessions via window_state.toml).  Falls back to the 600x500
            // default on first open / missing state.
            let settings_geom = self.window_state.settings;
            let inner_size = settings_geom.size.map_or((600_u32, 500_u32), <_>::from);
            let position = settings_geom.position.map(<_>::from);
            handle.create_window(freminal_windowing::WindowConfig {
                title: "Freminal Settings".to_owned(),
                inner_size: Some(inner_size),
                position,
                transparent: false,
                icon: self.icon.clone(),
                app_id: Some("freminal-settings".into()),
            });
        }

        // Remove per-window state for the duration of this frame.
        // All other windows remain in the map, so shader/bg propagation
        // to "other windows" simply iterates self.windows.
        let Some(mut win) = self.windows.remove(&window_id) else {
            return;
        };

        // ── Spawn new window ─────────────────────────────────────────────────
        if win.pending_new_window {
            win.pending_new_window = false;
            self.spawn_new_window(handle);
        }

        // ── Apply pending window geometry from layout engine ─────────────────
        if let Some((size_opt, pos_opt)) = win.pending_geometry.take() {
            use conv2::ConvUtil as _;
            if let Some([w, h]) = size_opt {
                // u32 -> f32 via approx is always Ok for window dimensions.
                let w_f: f32 = w.approx_as().unwrap_or(f32::MAX);
                let h_f: f32 = h.approx_as().unwrap_or(f32::MAX);
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(w_f, h_f)));
            }
            if let Some([x, y]) = pos_opt {
                // i32 -> f32 via approx is always Ok for screen coordinates.
                let x_f: f32 = x.approx_as().unwrap_or(0.0_f32);
                let y_f: f32 = y.approx_as().unwrap_or(0.0_f32);
                ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(x_f, y_f)));
            }
        }

        // ── Track last known window geometry (for save_layout) ───────────────
        // Query the windowing layer directly.  See the settings-window branch
        // above for why `ctx.input().viewport()` is not reliable here.
        if let Some(geom) = handle.window_geometry(window_id) {
            if let Some(size) = geom.size {
                win.last_known_size = Some(<[u32; 2]>::from(size));
            }
            if let Some(pos) = geom.position {
                win.last_known_position = Some(<[i32; 2]>::from(pos));
            }
        }

        // ── Deferred egui font update from standalone settings window ────────
        win.terminal_widget
            .flush_egui_fonts_if_dirty(ctx, &self.config);

        // ── Detect OS dark/light preference changes ───────────────────────────
        let current_os_dark = ctx.global_style().visuals.dark_mode;
        if current_os_dark != win.os_dark_mode {
            win.os_dark_mode = current_os_dark;

            // Always propagate the updated OS preference so DECRPM ?2031
            // reflects the new dark/light state, regardless of ThemeMode.
            for tab in win.tabs.iter() {
                if let Ok(panes) = tab.pane_tree.iter_panes() {
                    for pane in panes {
                        send_or_log!(
                            pane.input_tx,
                            InputEvent::ThemeModeUpdate(self.config.theme.mode, win.os_dark_mode,),
                            "Failed to send ThemeModeUpdate on OS change to pane"
                        );
                    }
                }
            }

            if self.config.theme.mode == ThemeMode::Auto {
                let slug = self.config.theme.active_slug(win.os_dark_mode);
                if let Some(theme) = freminal_common::themes::by_slug(slug) {
                    // Notify every pane in every tab so all PTY threads get the new palette.
                    for tab in win.tabs.iter() {
                        if let Ok(panes) = tab.pane_tree.iter_panes() {
                            for pane in panes {
                                send_or_log!(
                                    pane.input_tx,
                                    freminal_terminal_emulator::io::InputEvent::ThemeChange(theme),
                                    "Failed to send auto ThemeChange to pane"
                                );
                            }
                        }
                    }
                    rendering::update_egui_theme(ctx, theme, self.config.ui.background_opacity);
                    // Invalidate theme cache on all panes in all tabs so the
                    // next frame forces a full vertex rebuild with the new palette.
                    for tab in win.tabs.iter_mut() {
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
            let changed = match (new_mtime, win.shader_last_mtime) {
                (Some(new), Some(prev)) => new != prev,
                (Some(_), None) => true,
                _ => false,
            };
            if changed {
                win.shader_last_mtime = new_mtime;
                match std::fs::read_to_string(shader_path) {
                    Ok(src) => {
                        if let Ok(mut wpr) = win.window_post.lock() {
                            wpr.pending_shader = Some(Some(src.clone()));
                        }
                        // Propagate to all other windows (win is removed from map).
                        for other_win in self.windows.values() {
                            if let Ok(mut wpr) = other_win.window_post.lock() {
                                wpr.pending_shader = Some(Some(src.clone()));
                            }
                        }
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

        // ── Poll all tabs for PTY death signals ───────────────────────────────
        // When a pane's PTY dies, close that pane.  If it was the last pane in
        // the tab, close the tab.  If it was the last tab, close the window.
        //
        // Collect (tab_index, pane_id) pairs for dead panes, then process
        // them in reverse order to avoid index shifting issues.
        let mut dead_panes: Vec<(usize, panes::PaneId)> = Vec::new();
        for (tab_idx, tab) in win.tabs.iter().enumerate() {
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
            let is_active_tab = tab_idx == win.tabs.active_index();

            // Switch to the dead pane's tab temporarily if needed so we can
            // operate on it.
            if !is_active_tab && let Err(e) = win.tabs.switch_to(tab_idx) {
                error!("Failed to switch to tab {tab_idx} for dead pane cleanup: {e}");
                continue;
            }

            let tab = win.tabs.active_tab_mut();
            // If the dead pane was the zoomed pane, un-zoom first.
            if tab.zoomed_pane == Some(pane_id) {
                tab.zoomed_pane = None;
            }

            match tab.pane_tree.close(pane_id) {
                Ok(_closed) => {
                    // Emit PaneClose recording event.
                    if let Some(ref h) = self.recording_handle {
                        // Saturating `u64 -> u32`: pane IDs are monotonic from
                        // 0 and will never realistically exceed u32::MAX.
                        h.emit(
                            freminal_terminal_emulator::recording::EventPayload::PaneClose {
                                pane_id: u32::try_from(pane_id.raw()).unwrap_or(u32::MAX),
                            },
                        );
                    }

                    // Reset last_sent_size on all surviving panes so the
                    // next frame's resize check fires with the new layout.
                    let tab = win.tabs.active_tab_mut();
                    if let Ok(panes) = tab.pane_tree.iter_panes_mut() {
                        for pane in panes {
                            pane.view_state.last_sent_size = (0, 0);
                        }
                    }
                    // If the active pane was the one that died, pick a new active pane
                    // and notify it that it gained focus.
                    let tab = win.tabs.active_tab_mut();
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
                    if win.tabs.tab_count() <= 1 {
                        // Last tab in this window — close the window.
                        self.windows.insert(window_id, win);
                        ctx.send_viewport_cmd(ViewportCommand::Close);
                        return;
                    }
                    win.close_tab(tab_idx);
                }
                Err(e) => {
                    error!("Failed to close dead pane {pane_id}: {e}");
                }
            }

            // Restore the original active tab if we switched away.
            if !is_active_tab {
                // The tab we were on may have been removed, so saturate.
                let restore_idx = tab_idx.min(win.tabs.tab_count().saturating_sub(1));
                let _ = win.tabs.switch_to(restore_idx);
            }
        }

        // Load the latest snapshot from the PTY thread — no lock, single atomic load.
        let (snap, pane_scroll_offset) = {
            let Some(active_pane_ref) = win.tabs.active_tab().active_pane() else {
                warn!("update: active tab has no active pane; skipping render frame");
                return;
            };
            (
                active_pane_ref.arc_swap.load_full(),
                active_pane_ref.view_state.scroll_offset,
            )
        };

        // Sync the GUI's scroll offset from the snapshot.  When new PTY output
        // arrives the PTY thread resets its offset to 0, so the snapshot will
        // carry scroll_offset = 0 even if the GUI previously sent a non-zero
        // value.  Adopting the snapshot's value keeps ViewState in sync.
        if pane_scroll_offset != snap.scroll_offset
            && let Some(p) = win.tabs.active_tab_mut().active_pane_mut()
        {
            p.view_state.scroll_offset = snap.scroll_offset;
        }

        // Create a root Ui covering the full available area.  Panels reserve
        // space from this Ui via `show_inside` (the non-deprecated API).
        let mut root_ui = egui::Ui::new(
            ctx.clone(),
            egui::Id::new("freminal_root"),
            egui::UiBuilder::default(),
        );

        // Menu bar at the top of the window.
        let mut any_menu_open = false;
        if !self.config.ui.hide_menu_bar {
            let (menu_action, menu_open) = Panel::top("menu_bar")
                .show_inside(&mut root_ui, |ui| {
                    self.show_menu_bar(ui, &mut win, window_id)
                })
                .inner;
            any_menu_open = menu_open;
            self.dispatch_tab_bar_action(menu_action, &mut win);
        }

        // Tab bar: shown when multiple tabs are open, or when the config
        // option `tabs.show_single_tab` is enabled.
        let show_tab_bar = win.tabs.tab_count() > 1 || self.config.tabs.show_single_tab;

        if show_tab_bar {
            let panel = match self.config.tabs.position {
                freminal_common::config::TabBarPosition::Top => Panel::top("tab_bar"),
                freminal_common::config::TabBarPosition::Bottom => Panel::bottom("tab_bar"),
            };
            let tab_action = panel
                .show_inside(&mut root_ui, |ui| self.show_tab_bar(&win, ui))
                .inner;
            self.dispatch_tab_bar_action(tab_action, &mut win);
        }

        let _panel_response = CentralPanel::default().show_inside(&mut root_ui, |ui| {
            // Synchronise font metrics with the current display scale *before*
            // reading `cell_size()`.  Without this, the first frame after a DPI
            // change would use stale pixel metrics for the resize calculation.
            let ppp = ctx.pixels_per_point();
            let ppp_changed = win.terminal_widget.sync_pixels_per_point(ppp);

            // Synchronise font zoom for the active tab.  Each tab has its own
            // zoom_delta and the font manager only knows one size at a time.
            // This check fires on every frame but is a single float comparison
            // when no change is needed.
            let effective = win
                .tabs
                .active_tab()
                .active_pane()
                .map_or(self.config.font.size, |p| {
                    p.view_state.effective_font_size(self.config.font.size)
                });
            let zoom_changed = win.terminal_widget.apply_font_zoom(effective);

            // When pixels-per-point or font zoom changes, every pane's GL
            // atlas and cached content must be invalidated so glyphs are
            // re-rasterised at the new size.
            if ppp_changed || zoom_changed {
                win.invalidate_all_pane_atlases();
            }

            // Compute char size once — shared across all panes since all panes
            // use the same font at the same size.
            // `cell_size()` returns integer pixel dimensions (physical) from swash
            // font metrics.  egui's coordinate system uses logical points, so we
            // convert with `pixels_per_point` when doing layout math.
            let (cell_w_u, cell_height_u) = win.terminal_widget.cell_size();
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
            let active_idx = win.tabs.active_index();
            let active_pane_id_for_drain = win.tabs.active_tab().active_pane;
            let window_focused = win
                .tabs
                .active_tab()
                .active_pane()
                .is_some_and(|p| p.view_state.window_focused);
            for (idx, tab) in win.tabs.iter_mut().enumerate() {
                let is_active_tab = idx == active_idx;
                let is_only_pane = match tab.pane_tree.pane_count() {
                    Ok(count) => count == 1,
                    Err(e) => {
                        trace!("pane_count error (treating as split): {e}");
                        false
                    }
                };
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
                            &rendering::WindowManipFlags {
                                allow_clipboard_read: self.config.security.allow_clipboard_read,
                                is_active: is_fully_active,
                                window_focused,
                                is_only_pane,
                            },
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
                    ctx.global_style_mut(|style| {
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
                    ctx.global_style_mut(|style| {
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
                win.style_cache = Some(style_key);
            }

            // ── Multi-pane rendering loop ────────────────────────────
            //
            // Compute layout rects for every leaf pane in the active tab's
            // pane tree, then render each one into its allocated rect.
            // Collect deferred key actions from all panes for dispatch after
            // the loop.

            let available_rect = ui.available_rect_before_wrap();
            let active_pane_id = win.tabs.active_tab().active_pane;
            let zoomed_pane = win.tabs.active_tab().zoomed_pane;
            let has_multiple_panes = win.tabs.active_tab().pane_tree.pane_count().unwrap_or(1) > 1;

            // When a pane is zoomed, render only that pane at full size.
            // Borders are hidden during zoom since there is only one visible pane.
            let (pane_layout, border_width) = if let Some(zoomed_id) = zoomed_pane {
                (vec![(zoomed_id, available_rect)], 0.0)
            } else {
                // Width of the border drawn between adjacent panes (logical pixels).
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

            // Floating "Save Layout" name-entry prompt.  Shown whenever the
            // user clicked "Save Layout" in the Layouts menu.  Returns true
            // exactly once (the frame the user confirms), at which point we
            // enqueue the SaveLayout action for dispatch.
            if self.show_save_layout_prompt(ctx) {
                all_deferred_actions.push(freminal_common::keybindings::KeyAction::SaveLayout);
            }

            // Track repaint needs across all panes.
            let mut shortest_repaint_delay: Option<std::time::Duration> = None;

            let ui_overlay_open = any_menu_open || self.pending_save_layout.is_some();

            // ── Pane border drag-to-resize ───────────────────────────
            //
            // Before rendering panes, place invisible drag sensors on each
            // split border. This must happen before the per-pane
            // `scope_builder` calls so that pointer events on the border
            // are consumed here instead of reaching the terminal widgets.
            if has_multiple_panes && zoomed_pane.is_none() && !ui_overlay_open {
                let borders = win
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
                        ctx.set_cursor_icon(cursor);
                    }

                    // On drag start, record which border we're resizing.
                    if response.drag_started() {
                        win.border_drag = Some(PaneBorderDrag {
                            target_pane: border.first_child_pane,
                            direction: border.direction,
                            parent_extent: border.parent_extent,
                        });
                    }

                    // While dragging, convert pixel delta to ratio delta.
                    if response.dragged()
                        && let Some(drag) = &win.border_drag
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
                            if let Err(e) = win.tabs.active_tab_mut().pane_tree.resize_split(
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
                        win.border_drag = None;
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
                let tab = win.tabs.active_tab_mut();
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

                // Build a RecordingContext for this pane if recording is active.
                let rec_window_id = self.recording_window_id(window_id);
                let rec_ctx = self.recording_handle.as_ref().map(|h| {
                    freminal_terminal_emulator::recording::RecordingContext {
                        handle: h,
                        window_id: rec_window_id,
                        // Saturating `u64 -> u32` for recording: pane IDs are
                        // monotonic from 0 and never approach u32::MAX.
                        pane_id: u32::try_from(pane_id.raw()).unwrap_or(u32::MAX),
                    }
                });

                // Render this pane into a child UI scoped to its content rect.
                // show() returns (left_clicked, deferred_key_actions).
                // left_clicked is true when a primary left-click was pressed inside
                // this pane's rect — used below for click-to-focus.
                let show_result =
                    ui.scope_builder(egui::UiBuilder::new().max_rect(content_rect), |pane_ui| {
                        win.terminal_widget.show(
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
                            pane_id,
                            rec_ctx.as_ref(),
                        )
                    });
                let (left_clicked, deferred_actions) = show_result.inner;
                all_deferred_actions.extend(deferred_actions);

                // Click-to-focus: if a non-active pane was left-clicked, transfer
                // keyboard focus to it and send FocusChange events to both panes.
                if left_clicked && !is_active {
                    let tab = win.tabs.active_tab_mut();
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
                    let tab = win.tabs.active_tab_mut();
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
                                error!("WindowPostRenderer init failed: {e}");
                                return;
                            }

                            // Process any pending shader change.
                            if let Some(pending_shader) = wpr.pending_shader.take() {
                                match pending_shader {
                                    Some(src)
                                        if let Err(e) = wpr.update_shader(
                                            gl,
                                            &src,
                                            vp.width_px,
                                            vp.height_px,
                                        ) =>
                                    {
                                        error!("Shader compilation failed: {e}");
                                    }
                                    Some(_) => {}
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

                let border_rects = win
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
                self.dispatch_deferred_action(action, &mut win, window_id);
            }

            // Handle deferred close-pane (needs `ui` for ViewportCommand::Close).
            if win.pending_close_pane {
                win.pending_close_pane = false;
                Self::close_focused_pane(ui, &mut win);
            }

            // Handle deferred directional focus (needs layout rects).
            if let Some(dir) = win.pending_focus_direction.take() {
                Self::focus_pane_in_direction(dir, available_rect, &mut win);
            }

            // Keep the window title bar in sync with the active tab's title.
            // This handles tab switches, OSC 0/2 title changes, and restore
            // from the title stack — all in one place.
            //
            // Only issue the viewport command when the title actually changed;
            // calling `send_viewport_cmd` unconditionally every frame triggers
            // an infinite repaint loop (~3 % idle CPU).
            let active_title = win
                .tabs
                .active_tab()
                .active_pane()
                .map_or("", |p| p.title.as_str());
            let window_title = if active_title.is_empty() {
                "Freminal"
            } else {
                active_title
            };
            if window_title != win.last_window_title {
                window_title.clone_into(&mut win.last_window_title);
                ctx.send_viewport_cmd(egui::ViewportCommand::Title(win.last_window_title.clone()));
            }

            // Schedule a repaint at the shortest interval needed by any pane.
            if let Some(delay) = shortest_repaint_delay {
                ctx.request_repaint_after(delay);
            }
        });

        let elapsed = now.elapsed();
        let frame_time = if elapsed.as_millis() > 0 {
            format!("Frame time={}ms", elapsed.as_millis())
        } else {
            format!("Frame time={}μs", elapsed.as_micros())
        };

        trace!("{}", frame_time);

        // Reinsert per-window state before returning.
        self.windows.insert(window_id, win);

        // Apply a pending layout (set from the Layouts menu).
        if let Some(resolved) = self.pending_load_layout.take() {
            let commands = self.apply_layout(&resolved, window_id, handle);
            self.inject_layout_commands(&commands);
        }
    }

    fn raw_input_hook(&mut self, _window_id: WindowId, raw_input: &mut egui::RawInput) {
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
