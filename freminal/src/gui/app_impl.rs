// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use std::sync::{Arc, Mutex, OnceLock};

use conv2::{ApproxFrom, ConvUtil, ValueFrom};
use egui::{self, CentralPanel, Panel, ViewportCommand};
use egui_glow::CallbackFn;
use freminal_common::buffer_states::window_manipulation::Osc99ControlKind;
use freminal_common::config::ThemeMode;
use freminal_common::pty_write::PtyWrite;
use freminal_common::send_or_log;
use freminal_terminal_emulator::io::InputEvent;
use freminal_windowing::WindowId;
use glow::HasContext;
use tracing::{debug, error, trace, warn};

use super::chrome_damage;
use super::frame_damage;
use super::panes;
use super::renderer::WindowPostRenderer;
use super::rendering;
use super::tabs::{Tab, TabManager};
use super::terminal::FreminalTerminalWidget;
use super::view_state;
use super::window::PerWindowState;
use super::{FreminalGui, PaneBorderDrag};

/// What `on_close_requested` should do about a window that may own an open
/// Settings window (issue #401).
///
/// Pure decision over an already-computed boolean (rather than `WindowId`s
/// or `&FreminalGui` directly) so it is unit-testable without constructing
/// the windowing layer or a full `FreminalGui`.
///
/// There is no "already resolved, proceed normally" variant: the
/// `WindowUnsavedSettings` Force Close handler clears `settings_owner` to
/// `None` *before* re-issuing the window's close, so `is_owner` is already
/// `false` by the time `on_close_requested` runs again for the retry —
/// tracking a separate "confirmed" flag would be dead weight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SettingsOwnerCloseDecision {
    /// This window does not own the settings modal — no special handling.
    NotOwner,
    /// No unsaved settings edits (or none open) — close the settings window
    /// now, alongside this window.
    CloseNow,
    /// Unsaved settings edits — veto this window's close and surface the
    /// close-guard confirmation dialog.
    VetoWithPrompt,
}

/// Decide what `on_close_requested` should do about the settings window when
/// `window_id` (the window being closed) may own it.
///
/// `is_owner` is `self.settings_owner == Some(window_id)`. `has_unsaved` is
/// `self.settings_modal.has_unsaved_changes()`.
const fn settings_owner_close_decision(
    is_owner: bool,
    has_unsaved: bool,
) -> SettingsOwnerCloseDecision {
    if !is_owner {
        return SettingsOwnerCloseDecision::NotOwner;
    }
    if has_unsaved {
        return SettingsOwnerCloseDecision::VetoWithPrompt;
    }
    SettingsOwnerCloseDecision::CloseNow
}

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
            // `settings_owner` already holds the *terminal* window that
            // requested this settings window (set by the menu/keybind
            // action before `handle.create_window()` was called). Do NOT
            // overwrite it with this settings window's own id here — doing
            // so used to make `settings_owner == settings_window_id`
            // always, which broke the owning-window-close guard (issue
            // #401: the guard compares `settings_owner` against a real
            // terminal window id, which could then never match) and the
            // "Test Paste" routing / os_dark_mode lookup, both of which key
            // off `settings_owner` to find the owning terminal window.
            // Don't create a PerWindowState — the settings window renders
            // only the settings UI via show_standalone().
            return;
        }

        let os_dark_mode = ctx.global_style().visuals.dark_mode;

        if let Some(initial) = self.initial_state.take() {
            // Start the periodic session auto-save timer, bound to the first
            // window's repaint handle so it can wake the (otherwise sleeping)
            // event loop when a save is due.  Spawned exactly once, here at
            // first-window creation.
            self.spawn_session_autosave_timer(Arc::clone(&initial.repaint_handle));

            // First window — spawn the initial PTY tab now, or if a
            // startup layout/session-restore applies, delegate to the
            // layout machinery (which will build the tabs itself and
            // avoid a throwaway PTY spawn).
            if self.will_layout_or_restore_apply() {
                self.create_first_window_from_layout_or_restore(
                    window_id,
                    ctx,
                    handle,
                    inner_size,
                    os_dark_mode,
                    initial.repaint_handle,
                    initial.window_post,
                );
            } else {
                self.create_first_window_with_default_pty(
                    window_id,
                    ctx,
                    handle,
                    inner_size,
                    os_dark_mode,
                    initial.repaint_handle,
                    initial.window_post,
                );
            }

            // Emit WindowCreate recording event.
            let rec_wid = self.recording_window_id(window_id);
            if let Some(h) = self.recording_swap.load_full() {
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
                    None,
                ) {
                    self.inject_layout_commands(&cmds);
                }
                return;
            }

            // Subsequent window — spawn a new PTY tab.
            let theme =
                freminal_common::themes::by_slug(self.config.theme.active_slug(os_dark_mode))
                    .unwrap_or(&freminal_common::themes::CATPPUCCIN_MOCHA);
            rendering::set_egui_options(
                ctx,
                theme,
                self.config.ui.background_opacity,
                &self.gui_theme,
            );

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
                super::pty::PtyTabInitialState {
                    theme,
                    auto_detect_urls: self.config.ui.auto_detect_urls,
                    cursor_style: freminal_common::cursor::CursorVisualStyle::from_config(
                        &self.config.cursor.shape,
                        self.config.cursor.blink,
                    ),
                },
                &repaint_handle,
                initial_size,
                super::pty::PtyTabConfig {
                    cwd: None,
                    shell_override: None,
                    extra_env: None,
                    recording_swap: self.recording_swap.clone(),
                    recording_pane_id: pane_id.raw().try_into().unwrap_or(u32::MAX),
                    set_term_program: self.config.shell_integration.set_term_program,
                },
            ) {
                Ok(channels) => {
                    let pane = panes::Pane::from_channels(
                        pane_id,
                        channels,
                        Arc::clone(&window_post),
                        "Terminal".to_owned(),
                    );
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
                        renaming_tab: None,
                        rename_buffer: String::new(),
                        dragging_tab: None,
                        last_tab_rects: Vec::new(),
                        pending_menu_actions: Vec::new(),
                        paste_dialog: super::paste_guard::PasteDialog::default(),
                        broadcast_dialog: super::broadcast_guard::BroadcastConfirmDialog::default(),
                        close_dialog: super::close_guard::CloseGuardDialog::default(),
                        pending_force_close: false,
                        pending_raw_keys: Vec::new(),
                        pending_frame_damage: freminal_windowing::FrameDamage::Full,
                        pending_terminal_band_range: 0..0,
                        present_is_partial: std::sync::Arc::new(
                            std::sync::atomic::AtomicBool::new(false),
                        ),
                        previous_active_pane_key: None,
                        pending_chrome_damage: freminal_windowing::ChromeDamage::Changed,
                        pending_chrome_signals: chrome_damage::ChromeSignals::default(),
                        chrome_settle_pending: false,
                        prev_dismissible_presence: chrome_damage::DismissiblePresence::default(),
                        prev_chrome_tab_snapshot: chrome_damage::ChromeTabSnapshot::default(),
                        prev_window_focused: false,
                        chrome_frames_rendered: 0,
                        pending_terminal_requested_delay: None,
                        cached_central_rect: None,
                        chrome_head_rects: None,
                        chrome_border_rects: Vec::new(),
                    };
                    self.windows.insert(window_id, win);

                    // Emit WindowCreate recording event.
                    let rec_wid = self.recording_window_id(window_id);
                    if let Some(h) = self.recording_swap.load_full() {
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
                    self.push_error_toast(
                        "Failed to open new window",
                        Some(format!("The shell could not be started: {e}")),
                    );
                }
            }
        }
    }

    /// Called when a window close is requested.
    ///
    /// Removes the window's state — its PTY threads will be dropped when
    /// the channels close. Returns `false` to veto the close: either the
    /// settings modal has unsaved edits to confirm (this window's own
    /// close, or the owning terminal window's — issue #401), or the window
    /// has a running foreground command pending confirmation (Task 98).
    fn on_close_requested(&mut self, window_id: WindowId) -> bool {
        // Settings window closed (via OS close button).
        if self.settings_window_id == Some(window_id) {
            // Consult the unsaved-changes guard.  When dirty, the modal
            // surfaces a confirm prompt on its next frame; veto the OS close
            // so the window stays open long enough for the user to decide.
            if !self.settings_modal.request_close() {
                return false;
            }
            self.settings_modal.is_open = false;
            self.settings_window_id = None;
            self.settings_owner = None;
            self.persist_window_state();
            return true;
        }
        // If this window owns an open Settings window, decide what to do
        // about it before this window closes (issue #401: the settings
        // window used to be orphaned — left open with its internal `is_open`
        // flag force-cleared but the actual OS window never told to close,
        // freezing it, and never counted against the "all windows closed"
        // quit check).
        match settings_owner_close_decision(
            self.settings_owner == Some(window_id),
            self.settings_modal.has_unsaved_changes(),
        ) {
            SettingsOwnerCloseDecision::NotOwner => {}
            SettingsOwnerCloseDecision::CloseNow => {
                self.settings_modal.is_open = false;
                self.settings_owner = None;
                // Nothing else will wake the settings window once its owner
                // is gone — force a repaint so its own next `update()` call
                // notices `is_open == false` and closes the OS window
                // itself via the existing self-close path (see `update()`'s
                // settings-window branch).
                if let Some(sid) = self.settings_window_id
                    && let Some((proxy, _)) = self
                        .windows
                        .get(&window_id)
                        .and_then(|w| w.repaint_handle.get())
                {
                    proxy.request_repaint(sid);
                }
            }
            SettingsOwnerCloseDecision::VetoWithPrompt => {
                if let Some(win) = self.windows.get_mut(&window_id) {
                    win.close_dialog.open(super::close_guard::PendingClose {
                        scope: super::close_guard::CloseScope::WindowUnsavedSettings,
                        running: Vec::new(),
                    });
                }
                return false;
            }
        }

        // Close-on-running-command guard (Task 98.7).  If the user already
        // confirmed a force-close for this window, let it through and clear
        // the flag.  Otherwise, if any pane in the window has a running
        // foreground command, open the confirmation dialog and veto the OS
        // close (return false); the dialog's Force Close re-issues the close
        // with this flag set.
        if self.force_close_windows.remove(&window_id) {
            // User-confirmed force close — fall through to the close logic.
        } else if let Some(win) = self.windows.get(&window_id) {
            let running = self.window_close_running(win);
            if !running.is_empty()
                && let Some(win) = self.windows.get_mut(&window_id)
            {
                win.close_dialog.open(super::close_guard::PendingClose {
                    scope: super::close_guard::CloseScope::Window,
                    running,
                });
                return false;
            }
        }

        // Auto-save session before the last terminal window is removed.
        // We check *before* remove so we still have access to the window's tabs.
        //
        // Saving is independent of `restore_last_session` — the flag only
        // controls whether the saved session is *applied* on next launch.
        // Saving keeps `last_session.toml` fresh so users can toggle the flag
        // on at any time and get their real last session back, rather than
        // whatever stale state happened to be on disk when they last had the
        // flag enabled.
        //
        // `maybe_auto_save_session` skips the write when nothing changed since
        // the periodic timer last persisted, and skips entirely for ad-hoc
        // command launches (`freminal -- vim foo`).  In the common case the
        // periodic save already wrote the current state, so this shutdown call
        // is a no-op — by design, so we no longer depend on a write surviving
        // an abrupt teardown.
        let remaining_terminal_windows = self
            .windows
            .keys()
            .filter(|&&wid| Some(wid) != self.settings_window_id)
            .count();
        if remaining_terminal_windows == 1 {
            self.maybe_auto_save_session();
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
            && let Some(h) = self.recording_swap.load_full()
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
            // Honor the live preview override so the window background tracks a
            // theme being previewed in Settings, not just the committed config.
            let os_dark_mode = self.windows.get(&window_id).is_some_and(|w| w.os_dark_mode);
            let theme = self.preview_theme.unwrap_or_else(|| {
                freminal_common::themes::by_slug(self.config.theme.active_slug(os_dark_mode))
                    .unwrap_or(&freminal_common::themes::CATPPUCCIN_MOCHA)
            });
            let (r, g, b) = theme.background;
            let color = egui::Color32::from_rgb(r, g, b);
            color.to_normalized_gamma_f32()
        }
    }

    fn present_partial_flag(
        &self,
        window_id: WindowId,
    ) -> Option<std::sync::Arc<std::sync::atomic::AtomicBool>> {
        self.windows
            .get(&window_id)
            .map(|win| std::sync::Arc::clone(&win.present_is_partial))
    }

    fn is_chrome_interactive_at(&self, window_id: WindowId, pos: egui::Pos2) -> bool {
        self.windows.get(&window_id).is_none_or(|win| {
            crate::gui::chrome_damage::point_in_chrome_rects(
                pos,
                win.chrome_head_rects.as_deref(),
                &win.chrome_border_rects,
            )
        })
    }

    fn take_frame_damage(&mut self, window_id: WindowId) -> freminal_windowing::FrameDamage {
        // Drain the damage computed during `update()` for this window, leaving
        // `Full` behind so a stale value can never be reused on a later frame
        // that does not recompute it.
        self.windows
            .get_mut(&window_id)
            .map_or(freminal_windowing::FrameDamage::Full, |win| {
                std::mem::replace(
                    &mut win.pending_frame_damage,
                    freminal_windowing::FrameDamage::Full,
                )
            })
    }

    fn take_terminal_band_range(&mut self, window_id: WindowId) -> std::ops::Range<usize> {
        // Drain the terminal-band range captured during `update()` for this
        // window, leaving `0..0` behind so a stale frame's range can never
        // be reused by a later caller that does not recompute it.
        self.windows.get_mut(&window_id).map_or(0..0, |win| {
            std::mem::replace(&mut win.pending_terminal_band_range, 0..0)
        })
    }

    fn take_chrome_damage(&mut self, window_id: WindowId) -> freminal_windowing::ChromeDamage {
        // Drain the chrome-damage decision computed during `update()` for
        // this window, leaving `Changed` behind so a stale `Unchanged` can
        // never be reused by a later frame that does not recompute it.
        self.windows
            .get_mut(&window_id)
            .map_or(freminal_windowing::ChromeDamage::Changed, |win| {
                std::mem::replace(
                    &mut win.pending_chrome_damage,
                    freminal_windowing::ChromeDamage::Changed,
                )
            })
    }

    fn take_terminal_requested_delay(
        &mut self,
        window_id: WindowId,
    ) -> Option<std::time::Duration> {
        // Drain the delay `update()` itself requested via
        // `ctx.request_repaint_after` this window's most recent frame,
        // leaving `None` behind so a stale delay can never be reused by a
        // later frame that does not recompute it.
        self.windows
            .get_mut(&window_id)
            .and_then(|win| win.pending_terminal_requested_delay.take())
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
        chrome_mode: freminal_windowing::ChromeMode,
    ) {
        trace!("Starting new frame");
        let now = std::time::Instant::now();

        // ── Settings window rendering ────────────────────────────────────────
        // If this update is for the settings window, render settings directly
        // and return — no terminal state to process.
        if self.settings_window_id == Some(window_id) {
            // OS dark/light preference (used to pick the auto-mode theme slug).
            // The settings window has no `PerWindowState`, so source it from
            // the owning terminal window's stable `os_dark_mode`. We must NOT
            // read it back from `ctx.global_style().visuals.dark_mode`, because
            // we overwrite the visuals below with a palette-derived `dark_mode`
            // — reading that back next frame would be self-referential.
            let os_dark = self
                .settings_owner
                .and_then(|owner| self.windows.get(&owner))
                .map_or_else(|| ctx.global_style().visuals.dark_mode, |w| w.os_dark_mode);

            // Apply the centralized themed chrome `Visuals` to the settings
            // window's own egui context. The settings window is a separate OS
            // window with its own `ctx`, and this branch returns before the
            // per-frame style hook in the terminal render path runs — so
            // without this the settings window stays on egui's default visuals
            // and ignores the active theme + profile (112.7 follow-up).
            //
            // Style from the *draft* (unsaved) theme so selecting a new theme
            // in the picker repaints the settings window live, instead of
            // staying on the committed theme until Apply + re-open.
            let active_slug = self.settings_modal.draft_active_theme_slug(os_dark);
            let theme = freminal_common::themes::by_slug(&active_slug)
                .unwrap_or(&freminal_common::themes::CATPPUCCIN_MOCHA);
            let visuals = crate::gui::chrome_style::build_visuals(
                &self.gui_theme,
                theme,
                self.config.ui.background_opacity,
            );
            let gui_theme = self.gui_theme;
            ctx.global_style_mut(|style| {
                style.visuals = visuals;
                crate::gui::chrome_style::apply_chrome_spacing(style, &gui_theme);
            });

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
                // Drop the live chrome preview override: the session is over.
                // On Apply the committed theme now flows via the snapshot; on
                // Cancel the RevertTheme broadcast restored it. Clearing also
                // re-enables per-window Auto-mode theming, which a pinned global
                // override cannot represent. The follow-up repaints scheduled by
                // the Apply / Revert dispatch cover the snapshot catch-up.
                self.preview_theme = None;
                self.persist_window_state();
                self.settings_window_id = None;
                self.settings_owner = None;
                handle.close_window(window_id);
            }
            return;
        }

        // ── Periodic session auto-save ───────────────────────────────────────
        // The background timer latches `session_save_due`; drain it here so a
        // due save runs on the terminal-window update path (the settings
        // window returned above).  Cheap no-op when not due.
        self.poll_session_autosave();

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
            // This window has no PerWindowState — normally a transient state
            // during teardown, but if the only/last shell failed to spawn it
            // is permanent.  Rather than leave a blank surface, render the
            // fatal-error panel (with an Exit button) when one is set.
            if self.fatal_error.is_some() {
                self.render_fatal_error(ctx);
            }
            return;
        };

        // ── Chrome-damage (#436.3): §3.5 "before" sample + warm-up counter ───
        //
        // Sampled as early as possible in `update()` — before ANY dialog's
        // `.show(ctx)` this frame (including the toast stack's, which runs
        // after `win` is reinserted at the end of this function) — so it
        // reflects presence strictly BEFORE this frame's rendering can
        // mutate it. Compared against the "after" sample taken once
        // everything dismissible has shown (see the end of this function) to
        // catch a self-dismissal that happens DURING this frame's rendering
        // (adversarial finding 1; see `chrome_damage`'s module doc).
        let chrome_dismissible_before = self.sample_dismissible_presence(&win);
        // #436 §7 warm-up: force `Changed` for the first few frames after
        // window creation, while font atlas / layout / PanelState id-maps
        // are still settling.
        let chrome_warming_up = chrome_damage::is_chrome_warming_up(win.chrome_frames_rendered);
        win.chrome_frames_rendered = win.chrome_frames_rendered.saturating_add(1);

        // ── Drain shader/renderer errors stashed by last frame's PaintCallback ──
        // PaintCallbacks run on the render thread and can't access `self`, so
        // they stash compile/init errors in `WindowPostRenderer::last_error`.
        // Drained here every frame (71.4 bug fix): previously only ran in the
        // subsequent-window branch of `on_window_created`, which never fires
        // for the first/only window and never re-runs after window creation.
        {
            let err = {
                let mut wpr = win
                    .window_post
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                wpr.last_error.take()
            };
            if let Some(msg) = err {
                self.push_error_toast("Shader error", Some(msg));
            }
        }

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
        //
        // `chrome_size_before` is captured for the #436.3 §3.3 "window
        // resize" chrome signal: compared against `win.last_known_size`
        // after this block updates it.
        let chrome_size_before = win.last_known_size;
        if let Some(geom) = handle.window_geometry(window_id) {
            if let Some(size) = geom.size {
                win.last_known_size = Some(<[u32; 2]>::from(size));
            }
            if let Some(pos) = geom.position {
                win.last_known_position = Some(<[i32; 2]>::from(pos));
            }
        }
        let chrome_size_changed = win.last_known_size != chrome_size_before;

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
                    rendering::update_egui_theme(
                        ctx,
                        theme,
                        self.config.ui.background_opacity,
                        &self.gui_theme,
                    );
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

        // ── Drain CommandFinishedEvent from each pane (Task 72.9) ─────────────
        // The PTY consumer thread forwards completed CommandBlocks here via a
        // dedicated channel. Append each block to the owning pane's
        // recent_commands ring (cap RECENT_COMMANDS_CAP) and set the tab's
        // has_pending_event flag if the event arrived on a non-active tab.
        //
        // Command-finished notifications (Task 76.4) are collected here and
        // routed after the loop, where `self.config` / the toast stack are
        // borrowable without conflicting with `win.tabs`.
        let active_tab_idx = win.tabs.active_index();
        let cmd_window_focused = win
            .tabs
            .active_tab()
            .active_pane()
            .is_some_and(|p| p.view_state.window_focused);
        let mut command_notifications: Vec<crate::gui::notifications::NotificationRequest> =
            Vec::new();
        let tab_title_policy = self.config.tab_title.policy;
        let tab_title_separator = self.config.tab_title.separator.clone();
        for (tab_idx, tab) in win.tabs.iter_mut().enumerate() {
            let mut tab_received_event = false;
            // Resolve the tab display name up front: `iter_panes_mut` borrows
            // `tab` mutably, so `display_name` cannot be called inside the
            // inner loop.  Used for the `{tab_name}` notification template
            // token (Task 76.5).
            let tab_name = tab
                .display_name(tab_title_policy, &tab_title_separator)
                .into_owned();
            if let Ok(panes) = tab.pane_tree.iter_panes_mut() {
                for pane in panes {
                    while let Ok(event) = pane.command_event_rx.try_recv() {
                        // Extract the command text from the current
                        // snapshot before the rows scroll out of the
                        // visible window. Used by the Quick Command
                        // History Palette to replay live entries.  Cache
                        // entries whose rows have already left the
                        // visible window will be silently absent — the
                        // seed half of the palette still works in that
                        // case.
                        let snap = pane.arc_swap.load();
                        let command_text =
                            crate::gui::command_history::extract_command_text(&snap, &event.block);
                        if let Some(text) = &command_text {
                            pane.record_command_text(event.block.id, text.clone());
                        }
                        drop(snap);

                        // Build a command-finished notification request (the
                        // builder applies the enable + threshold gates) before
                        // the block is moved into the recent-commands ring.
                        if let Some(req) = crate::gui::notifications::command_finished_request(
                            &event.block,
                            command_text.as_deref().unwrap_or(""),
                            &tab_name,
                            &self.config.notifications,
                        ) {
                            command_notifications.push(req);
                        }

                        // Ring the bell on command completion when
                        // `[bell] on_command_finished` is set (Task 76.5).
                        // Uses the configured bell mode, mirroring the
                        // `WindowManipulation::Bell` path in `rendering`.
                        if self.config.bell.on_command_finished {
                            use freminal_common::config::BellMode;
                            let mode = self.config.bell.mode;
                            if matches!(mode, BellMode::Visual | BellMode::Both) {
                                pane.bell_active = true;
                                pane.view_state.bell_since = Some(std::time::Instant::now());
                            }
                            if matches!(mode, BellMode::Audio | BellMode::Both) {
                                crate::gui::platform::system_beep();
                            }
                        }

                        pane.push_recent_command(event.block);
                        tab_received_event = true;
                    }
                }
            }
            if tab_received_event && tab_idx != active_tab_idx {
                tab.has_pending_event = true;
            }
        }

        // Route command-finished notifications collected above (Task 76.4).
        if !command_notifications.is_empty()
            && let Ok(mut toasts) = self.toasts.try_borrow_mut()
        {
            for req in &command_notifications {
                crate::gui::notifications::NotificationRouter::route(
                    req,
                    &self.config.notifications,
                    cmd_window_focused,
                    &mut toasts,
                );
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

            // Capture the originally-active tab's stable id so we can restore
            // focus to *that* tab afterwards. Restoring by index is wrong:
            // closing a tab at a lower index shifts the active tab left, and
            // the dead pane's `tab_idx` is not the user's active tab.
            let original_active_tab_id = win.tabs.active_tab().id;

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
                    if let Some(h) = self.recording_swap.load_full() {
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

            // Restore the originally-active tab if we switched away. Look it
            // up by stable id rather than by index, since a tab close during
            // this iteration may have shifted indices. If the originally-active
            // tab was itself closed, leave the active index where `close_tab`
            // placed it.
            if !is_active_tab {
                let restore_idx = win.tabs.iter().position(|t| t.id == original_active_tab_id);
                if let Some(restore_idx) = restore_idx {
                    let _ = win.tabs.switch_to(restore_idx);
                }
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

        // Apply the full palette-derived chrome `Visuals` (112.4) BEFORE any
        // chrome is drawn this frame.  The menu bar and tab bar are rendered
        // immediately below, so the style must be in place first — applying it
        // after them (as it was) left the bars styled by the *previous* frame's
        // palette, so a live theme change did not reach the menu/tab bar until
        // a later frame happened to repaint them (the "bars don't update"
        // symptom).
        //
        // Gated: only call `global_style_mut` when the inputs have changed.
        // `global_style_mut` triggers `Arc::make_mut` on the egui `Style`,
        // which clones every frame unless skipped — and `build_visuals` itself
        // is non-trivial, so the cache short-circuits the rebuild on the
        // steady-state (unchanged) path.
        //
        // Chrome styles from the live preview theme when one is active
        // (Settings theme picker), falling back to the snapshot's theme at
        // steady state.  The preview override makes chrome re-theme immediately
        // and deterministically — it does not depend on the GUI happening to
        // read the post-`ThemeChange` snapshot (a race that left the
        // background/chrome stale until a mouseover repaint).
        let bg_opacity = self.config.ui.background_opacity;
        // Hoisted out of the block below (rather than a plain `let` inside
        // it) so the #436.3 chrome-damage signal computed further down can
        // read this frame's style-change verdict too.
        let chrome_style_changed;
        {
            let gui_theme = self.gui_theme;
            let chrome_theme = self.preview_theme.unwrap_or(snap.theme);
            chrome_style_changed = match win.style_cache {
                Some((prev_theme, prev_opacity, prev_gui_theme)) => {
                    !std::ptr::eq(prev_theme, chrome_theme)
                        || prev_opacity.to_bits() != bg_opacity.to_bits()
                        || prev_gui_theme != gui_theme
                }
                None => true,
            };
            if chrome_style_changed {
                let visuals =
                    crate::gui::chrome_style::build_visuals(&gui_theme, chrome_theme, bg_opacity);
                ctx.global_style_mut(|style| {
                    style.visuals = visuals;
                    crate::gui::chrome_style::apply_chrome_spacing(style, &gui_theme);
                });
                win.style_cache = Some((chrome_theme, bg_opacity, gui_theme));
            }
        }

        // ── #436.4b: FULL vs REPLAY chrome construction ──────────────────────
        //
        // On `ChromeMode::Full` the root Ui, menu bar, and tab bar are built
        // exactly as before, and `CentralPanel` reserves the remaining space
        // for `central_body` below. On `ChromeMode::Replay` the windowing
        // layer has already proven chrome (including window size) is
        // unchanged since the last FULL frame, so none of that is rebuilt —
        // `central_body` runs directly against a `Ui` constructed at the
        // cached content rect instead, in the SAME background layer chrome
        // uses (so the terminal band's shapes land exactly where `run_frame`
        // expects them). `any_menu_open` (read inside `central_body` to
        // compute `ui_overlay_open`) is `false` on Replay: with no menu bar
        // built, no menu can be open.
        let (any_menu_open, chrome_root_ui): (bool, Option<egui::Ui>) = if chrome_mode
            == freminal_windowing::ChromeMode::Full
        {
            // Create a root Ui covering the full available area.  Panels
            // reserve space from this Ui via `show` (the non-deprecated
            // API; `show_inside` was renamed to `show` in egui 0.35).
            let mut root_ui = egui::Ui::new(
                ctx.clone(),
                egui::Id::new("freminal_root"),
                egui::UiBuilder::default(),
            );

            // #436.8: menu-bar / tab-bar rects, captured for the
            // region-aware pointer chrome-gate (`is_chrome_interactive_at`).
            // Only ever populated on a FULL frame — a REPLAY frame builds
            // neither panel, so `win.chrome_head_rects` is left untouched
            // (stale-but-still-correct: chrome hasn't moved since the
            // FULL frame that last set it, by the same invariant that
            // makes REPLAY safe at all).
            let mut head_rects: Vec<egui::Rect> = Vec::new();

            // Menu bar at the top of the window.
            let menu_open = if self.config.ui.hide_menu_bar {
                false
            } else {
                let menu_response = Panel::top("menu_bar").show(&mut root_ui, |ui| {
                    self.show_menu_bar(ui, &mut win, window_id)
                });
                head_rects.push(menu_response.response.rect);
                let (menu_action, menu_open) = menu_response.inner;
                self.dispatch_tab_bar_action(menu_action, &mut win);
                menu_open
            };

            // Tab bar: shown when multiple tabs are open, or when the
            // config option `tabs.show_single_tab` is enabled.
            let show_tab_bar = win.tabs.tab_count() > 1 || self.config.tabs.show_single_tab;

            if show_tab_bar {
                let panel = match self.config.tabs.position {
                    freminal_common::config::TabBarPosition::Top => Panel::top("tab_bar"),
                    freminal_common::config::TabBarPosition::Bottom => Panel::bottom("tab_bar"),
                };
                let tab_response = panel.show(&mut root_ui, |ui| self.show_tab_bar(&mut win, ui));
                head_rects.push(tab_response.response.rect);
                let tab_action = tab_response.inner;
                self.dispatch_tab_bar_action(tab_action, &mut win);
            }

            win.chrome_head_rects = Some(head_rects);

            (menu_open, Some(root_ui))
        } else {
            (false, None)
        };

        // Help menu → "Keybindings..." routes here.  Opens the Settings
        // Modal with the Keybindings tab preselected, or focuses the
        // existing settings window if one is already open.  Mirrors the
        // Settings menu item in `show_menu_bar`, but jumps to the
        // Keybindings tab instead of the default Font tab. Independent of
        // `chrome_mode`: it only mutates modal-open state (no painting), so
        // it runs every frame regardless of whether chrome was rebuilt.
        if self.pending_open_keybindings {
            self.pending_open_keybindings = false;
            if self.settings_window_id.is_some() {
                self.pending_focus_settings = true;
                self.settings_modal
                    .set_active_tab(crate::gui::settings::SettingsTab::Keybindings);
            } else if !self.settings_modal.is_open && !self.pending_settings_window {
                let families = win.terminal_widget.monospace_families();
                self.settings_modal.open_to_tab(
                    &self.config,
                    families,
                    win.os_dark_mode,
                    crate::gui::settings::SettingsTab::Keybindings,
                );
                self.settings_modal
                    .set_base_font_defs(win.terminal_widget.base_font_defs().clone());
                self.settings_owner = Some(window_id);
                self.pending_settings_window = true;
            }
        }

        // Copy the cached content rect (`egui::Rect` is `Copy`) out of `win`
        // BEFORE `central_body` captures `win` by mutable reference below —
        // reading `win.cached_central_rect` after that point (e.g. inside
        // the REPLAY arm further down) would conflict with the closure's
        // borrow. Only actually used on a REPLAY frame; harmless (and cheap)
        // to compute unconditionally otherwise. Falls back to egui's own
        // content rect in the unreachable case described where it is used.
        let cached_central_rect_for_replay = win
            .cached_central_rect
            .unwrap_or_else(|| ctx.input(egui::InputState::content_rect));

        // The terminal band + (on Full only) chrome dialogs/overlays. Shared
        // between the FULL path (called via `CentralPanel::show`, below) and
        // the REPLAY path (called directly against a `Ui` built at the
        // cached content rect) so the band's rendering logic — the per-pane
        // loop, borders, broadcast label, band-range capture, chrome-damage
        // signal staging, and repaint scheduling — is defined exactly once.
        let mut central_body = |ui: &mut egui::Ui| {
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

            // Command-block gutter inset, in logical points.  This is reserved
            // on the left edge of every pane's content rect when the gutter is
            // enabled.  It is subtracted from the available width BEFORE the
            // column count is computed (below) so the column count reported to
            // the PTY matches the rendered cell-grid width — the renderer
            // shifts its terminal rect right by the same inset.  Zero when the
            // feature is disabled or the gutter is set to `Off`.
            let gutter_inset_logical = if self.config.command_blocks.enabled {
                self.config.command_blocks.gutter.total_inset_px() / ppp
            } else {
                0.0
            };

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
            // #436.3 §3.3 "Window focus change" chrome signal.
            let chrome_focus_changed = window_focused != win.prev_window_focused;
            win.prev_window_focused = window_focused;
            // OSC 9 / OSC 777 notifications collected from every pane this
            // frame, routed after the loop (Task 76.4).
            let mut osc_notifications: Vec<crate::gui::notifications::NotificationRequest> =
                Vec::new();
            // OSC 99 stateful notifications collected from every pane this
            // frame, routed after the loop (Task 99.5a) alongside
            // `osc_notifications`. Each item is paired with a clone of the
            // originating pane's `pty_write_tx` (Task 99.5c Gap 2) so future
            // reverse-path writes (Task 99.6) can target the right pane.
            let mut osc99_notifications: Vec<(
                freminal_common::buffer_states::window_manipulation::Notification99Data,
                crossbeam_channel::Sender<freminal_common::pty_write::PtyWrite>,
            )> = Vec::new();
            // OSC 99 app→terminal control sequences (p=close/p=alive/p=?)
            // collected from every pane this frame (Task 99.5c). Inert for
            // now — logged after the loop, not yet answered (Tasks 99.6/99.7).
            let mut osc99_controls: Vec<(
                crate::gui::notifications::Osc99Control,
                crossbeam_channel::Sender<freminal_common::pty_write::PtyWrite>,
            )> = Vec::new();
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
                    let mut tab_shell_set_title = false;
                    for pane in panes {
                        let is_fully_active = is_active_tab && pane.id == active_pane_id_for_drain;
                        let shell_set = rendering::handle_window_manipulation(
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
                            &mut osc_notifications,
                            &mut osc99_notifications,
                            &mut osc99_controls,
                        );
                        if shell_set {
                            tab_shell_set_title = true;
                        }
                    }
                    // The title policy decides whether a shell-asserted OSC
                    // 0/1/2 title clears the user-pinned custom name (only
                    // under `OscWins`); see `Tab::apply_osc_title_policy`.
                    tab.apply_osc_title_policy(self.config.tab_title.policy, tab_shell_set_title);
                }
            }

            // Route OSC 9 / OSC 777 notifications collected above (Task 76.4).
            // Done after the pane loop so `self.config` and the toast stack
            // are borrowable without conflicting with the `win.tabs` borrow.
            if !osc_notifications.is_empty()
                && let Ok(mut toasts) = self.toasts.try_borrow_mut()
            {
                for req in &osc_notifications {
                    crate::gui::notifications::NotificationRouter::route(
                        req,
                        &self.config.notifications,
                        window_focused,
                        &mut toasts,
                    );
                }
            }

            // Route OSC 99 stateful notifications collected above (Task
            // 99.5a). Done after the pane loop so `self.config`, the toast
            // stack, and the OSC 99 session maps are borrowable without
            // conflicting with the `win.tabs` borrow.
            if !osc99_notifications.is_empty() {
                let window_minimized = ui.ctx().input(|i| i.viewport().minimized.unwrap_or(false));
                if let (Ok(mut toasts), Ok(mut icon_cache), Ok(mut live)) = (
                    self.toasts.try_borrow_mut(),
                    self.osc99_icon_cache.try_borrow_mut(),
                    self.osc99_live.try_borrow_mut(),
                ) {
                    let ctx = crate::gui::notifications::Osc99DisplayContext {
                        window_focused,
                        window_minimized,
                    };
                    // `tx` (the originating pane's `pty_write_tx` clone) is
                    // threaded into the reverse-write path (Task 99.6): the
                    // notification thread uses it to write activation/close
                    // reports back to the pane that produced this OSC 99
                    // sequence.
                    for (data, tx) in &osc99_notifications {
                        crate::gui::notifications::NotificationRouter::route_osc99(
                            data,
                            &self.config.notifications,
                            ctx,
                            &mut toasts,
                            &mut icon_cache,
                            &mut live,
                            tx,
                        );
                    }
                }
            }

            // Answer OSC 99 control sequences collected above (Task 99.6 for
            // Close/Alive; the Query capability handshake is Task 99.7). Run
            // after the display-routing block above so its `osc99_live`
            // borrow has already been released.
            for (control, tx) in &osc99_controls {
                match control.kind {
                    Osc99ControlKind::Alive => {
                        // Answer the poll with the current live notification ids.
                        if let Ok(live) = self.osc99_live.try_borrow() {
                            let ids = crate::gui::notifications::live_ids_sorted(&live);
                            let bytes = crate::gui::notifications::osc99_alive_report(
                                control.id.as_deref(),
                                &ids,
                            );
                            send_or_log!(
                                tx,
                                PtyWrite::Write(bytes),
                                "Failed to send OSC 99 alive report"
                            );
                        }
                    }
                    Osc99ControlKind::Close => {
                        // App-driven close request: prune the live entry.
                        // freminal cannot programmatically close an OS
                        // notification it already delegated to the desktop
                        // environment, so this only reconciles our liveness
                        // map — no report is sent here (the close report is
                        // emitted only when WE observe a close on the
                        // notification thread with `c=1`).
                        if let (Some(id), Ok(mut live)) =
                            (control.id.as_deref(), self.osc99_live.try_borrow_mut())
                        {
                            crate::gui::notifications::forget_osc99(&mut live, id);
                        }
                    }
                    Osc99ControlKind::Query => {
                        // OSC 99 p=? capability handshake (Task 99.7): answer
                        // with freminal's truthfully-advertised OSC 99
                        // capabilities.
                        let bytes =
                            crate::gui::notifications::osc99_query_response(control.id.as_deref());
                        send_or_log!(
                            tx,
                            PtyWrite::Write(bytes),
                            "Failed to send OSC 99 capability response"
                        );
                    }
                }
            }

            // ── Multi-pane rendering loop ────────────────────────────
            //
            // Compute layout rects for every leaf pane in the active tab's
            // pane tree, then render each one into its allocated rect.
            // Collect deferred key actions from all panes for dispatch after
            // the loop.

            let available_rect = ui.available_rect_before_wrap();

            // #436.4b: cache the content rect so a later REPLAY frame can
            // reconstruct an equivalent `Ui` without rebuilding the menu
            // bar / tab bar / `CentralPanel` chrome that produced it.
            // Idempotent on a REPLAY frame itself (`available_rect` is then
            // already exactly the cached value).
            win.cached_central_rect = Some(available_rect);

            let active_pane_id = win.tabs.active_tab().active_pane;
            let zoomed_pane = win.tabs.active_tab().zoomed_pane;
            let has_multiple_panes = win.tabs.active_tab().pane_tree.pane_count().unwrap_or(1) > 1;

            // Re-anchor the cursor blink phase when the active pane changes —
            // by a pane switch within the tab OR a tab switch (both change the
            // "which pane is active-and-visible" key). This makes the newly
            // active pane's cursor appear immediately instead of inheriting the
            // global blink cycle's current half (the cursor-appear lag). The
            // flag is captured on that pane's next render, when the egui input
            // clock is available. Only reset on an actual change so we don't
            // re-anchor (and cause a spurious extra blink) every frame.
            let active_pane_key = (win.tabs.active_tab().id, active_pane_id);
            let active_pane_changed = win.previous_active_pane_key != Some(active_pane_key);
            if active_pane_changed {
                if let Some(pane) = win.tabs.active_tab_mut().pane_tree.find_mut(active_pane_id) {
                    pane.view_state.cursor_blink_reset_pending = true;
                }
                win.previous_active_pane_key = Some(active_pane_key);
            }

            // Broadcast input (Task 74): when the active tab has broadcast
            // enabled, collect the (pane id, input sender) of every leaf pane
            // up front. Senders are cheap to clone. The active pane's render
            // call mirrors its keyboard input to every *other* pane in this
            // list. Empty when broadcast is off (the common case).
            let broadcast_senders: Vec<(panes::PaneId, crossbeam_channel::Sender<InputEvent>)> =
                if win.tabs.active_tab().broadcast_input {
                    win.tabs
                        .active_tab()
                        .pane_tree
                        .iter_panes()
                        .map(|panes| {
                            panes
                                .into_iter()
                                .map(|p| (p.id, p.input_tx.clone()))
                                .collect()
                        })
                        .unwrap_or_default()
                } else {
                    Vec::new()
                };

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

            // ── #436.4b: chrome dialogs/overlays — FULL only ──────────────
            //
            // These are all cached TAIL chrome (each uses its own
            // `egui::Window`/`Area`, a distinct layer from the terminal
            // band's `LayerId::background()`). A REPLAY frame is only ever
            // entered when the PREVIOUS frame proved `ui_overlay_open` was
            // `false` (any dialog open forces `ChromeDamage::Changed` every
            // frame it is — see `ChromeSignals::any_fired`) and no chrome
            // input landed this frame, so by construction none of these can
            // be open (or becoming open) on a REPLAY frame. Skipping their
            // `.show()` calls is therefore safe; running them would be
            // wasted work whose freshly-painted shapes `run_frame` would
            // discard anyway (REPLAY reuses the cached tail primitives, not
            // this frame's own tail shapes).
            if chrome_mode == freminal_windowing::ChromeMode::Full {
                // Floating "Save Layout" name-entry prompt.  Shown whenever the
                // user clicked "Save Layout" in the Layouts menu.  Returns true
                // exactly once (the frame the user confirms), at which point we
                // enqueue the SaveLayout action for dispatch.
                if self.show_save_layout_prompt(ctx) {
                    all_deferred_actions.push(freminal_common::keybindings::KeyAction::SaveLayout);
                }

                // Smart paste guard confirm dialog (Task 77).  Shown whenever a
                // flagged paste is pending for this window.  On confirm, the
                // resolved (possibly edited) payload is sent to the active pane;
                // on cancel the paste is discarded.
                match win.paste_dialog.show(ctx) {
                    super::paste_guard::PasteDialogOutcome::Paste { payload, target } => {
                        // Route to the pane captured when the dialog opened, not
                        // the currently-active pane: focus-follows-mouse can change
                        // the active pane when the cursor moves onto the dialog
                        // buttons (Task 106 bug).
                        Self::send_paste_to_target(&mut win, target, payload);
                    }
                    super::paste_guard::PasteDialogOutcome::Cancelled
                    | super::paste_guard::PasteDialogOutcome::Idle => {}
                }

                // Broadcast-input confirm dialog (Task 74.5).  Shown when the user
                // tried to enable broadcast and `[tabs] confirm_broadcast` is set.
                // On confirm, broadcast is enabled on the dialog's target tab.
                match win.broadcast_dialog.show(ctx) {
                    super::broadcast_guard::BroadcastDialogOutcome::Confirmed(tab_id) => {
                        if let Some(tab) = win.tabs.iter_mut().find(|t| t.id == tab_id) {
                            tab.broadcast_input = true;
                            let pane_count = tab.pane_tree.iter_panes().map_or(1, |p| p.len());
                            self.push_info_toast(
                                "Broadcast input enabled",
                                Some(format!(
                                    "Keyboard input is now sent to all {pane_count} pane(s) in this tab."
                                )),
                            );
                        }
                    }
                    super::broadcast_guard::BroadcastDialogOutcome::Cancelled
                    | super::broadcast_guard::BroadcastDialogOutcome::Idle => {}
                }

                // Close-on-running-command guard dialog (Task 98).  Shown while a
                // pane / tab / window close is suspended pending confirmation.  On
                // Force Close the original close is executed with the guard
                // bypassed; on Cancel the close is abandoned.
                // A pending ForceClose key action resolves an open close-guard
                // dialog as Force Close; harmless no-op when nothing is open.
                let force_close_requested = std::mem::take(&mut win.pending_force_close);
                if let Some(scope) = win.close_dialog.scope() {
                    let outcome = if force_close_requested {
                        win.close_dialog.force_close_now();
                        super::close_guard::CloseDialogOutcome::ForceClose
                    } else {
                        win.close_dialog.show(ctx)
                    };
                    match outcome {
                        super::close_guard::CloseDialogOutcome::ForceClose => match scope {
                            super::close_guard::CloseScope::Pane => {
                                Self::close_focused_pane(ui, &mut win);
                            }
                            super::close_guard::CloseScope::Tab(index) => {
                                win.close_tab(index);
                            }
                            super::close_guard::CloseScope::Window => {
                                // Mark this window as user-confirmed so the
                                // on_close_requested guard lets the resulting
                                // ViewportCommand::Close through without re-prompting.
                                self.force_close_windows.insert(window_id);
                                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            }
                            super::close_guard::CloseScope::WindowUnsavedSettings => {
                                // User chose to discard the unsaved settings
                                // edits. Close the settings OS window directly —
                                // `handle` is available right here, unlike in
                                // `on_close_requested` — then re-issue this
                                // window's close. Clearing `settings_owner` here
                                // (rather than a separate "confirmed" flag) is
                                // what makes the retry's `on_close_requested`
                                // call see `is_owner == false` and skip the
                                // guard without re-prompting (issue #401).
                                self.settings_modal.is_open = false;
                                self.settings_owner = None;
                                if let Some(sid) = self.settings_window_id.take() {
                                    handle.close_window(sid);
                                }
                                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            }
                        },
                        super::close_guard::CloseDialogOutcome::Cancelled
                        | super::close_guard::CloseDialogOutcome::Idle => {}
                    }
                }

                // Floating "About Freminal" dialog.  Shown whenever the user
                // clicked "About Freminal" in the Help menu.  Self-dismissing
                // via its own Close button or title-bar X.
                self.show_about_window(ctx);

                // First-run welcome overlay (subtask 71.20).  Opened on first
                // launch or from Help -> Show Welcome; persists
                // `first_run_complete = true` on dismissal.
                self.show_welcome_overlay(ctx);
            }

            // Drain pending menu actions (Edit menu clicks: Copy, Paste,
            // Select All, Find...).  These were queued during
            // `show_menu_bar` above, which does not have mutable access to
            // the active pane's ViewState / input_tx.  Menu-local actions
            // (Copy, Paste, Select All) are applied directly to the active
            // pane; others are routed through the deferred-action pipeline.
            for action in std::mem::take(&mut win.pending_menu_actions) {
                Self::dispatch_menu_action(&mut win, action, &mut all_deferred_actions);
            }

            // Track repaint needs across all panes.
            let mut shortest_repaint_delay: Option<std::time::Duration> = None;

            // Tab rename is treated as an overlay: while the inline rename
            // TextEdit is active, the terminal widget must release keyboard
            // focus and stop consuming pointer events, or keystrokes would
            // be forwarded to the PTY instead of the edit buffer.
            let ui_overlay_open = any_menu_open
                || self.pending_save_layout.is_some()
                || self.about_window_open
                || self.welcome.is_open()
                || win.renaming_tab.is_some()
                || win.paste_dialog.is_open()
                || win.broadcast_dialog.is_open()
                || win.close_dialog.is_open();

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

                // #436.8: split-border drag-sensor rects, rebuilt fresh every
                // frame this branch runs, for the region-aware pointer
                // chrome-gate (`is_chrome_interactive_at`).
                let mut border_rects: Vec<egui::Rect> = Vec::with_capacity(borders.len());

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

                    border_rects.push(sensor_rect);

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

                win.chrome_border_rects = border_rects;
            } else {
                // No sensors built this frame (single pane / zoomed / overlay
                // open): clear any stale rects from a since-changed layout so
                // they can't keep classifying terminal content as chrome
                // (#436.8).
                win.chrome_border_rects.clear();
            }

            // ── Terminal band: shape-index range capture (#436.2a, range
            // exposed via `App::take_terminal_band_range` as of #436.4a) ───
            //
            // Everything from here through the broadcast label that paints
            // via `ui` — pane content (GL callbacks), pane borders, and the
            // per-pane decorations — lands in the SAME `LayerId::background()`
            // layer chrome (menu bar, tab bar) already paints into, and is
            // therefore captured in the shape-index range below. (Per-pane
            // pop-ups reached from inside this region — the context menu,
            // command-history palette, and search bar — deliberately use
            // their own `Order::Foreground` `egui::Area`s, so they live in a
            // different layer and are correctly excluded from the captured
            // background range.) An earlier version of this subtask routed
            // the band into a dedicated second `Order::Background` layer
            // instead, but that
            // trips egui 0.35's cross-layer hit-test "hidden" rule
            // (`hit_test.rs:145`): a widget is hidden from hover/click/drag
            // if a later widget on a DIFFERENT layer contains its rect, and
            // two untracked same-`Order` layers tie-break by hash iteration
            // order — nondeterministically hiding every `ui.interact()`
            // widget in the band (e.g. the command-block gutter hover
            // highlight). Staying in the shared background layer keeps both
            // paint topology and hit-test topology identical to `main`; we
            // instead remember where the band's shapes start and end within
            // that single `PaintList` and hand back that `[start, end)` range
            // for `App::take_terminal_band_range`, which `run_frame` uses to
            // slice `full_output.shapes` into head/band/tail and paint each
            // separately (#436.4a).
            //
            // Capture point justification: nothing between `available_rect`
            // above and here paints into the background layer. The
            // intervening code only reads window-manipulation state, shows
            // dialogs (save-layout prompt, paste/broadcast/close guards,
            // about window, welcome overlay — all `egui::Window`, which is
            // backed by an `Area` with its own distinct `LayerId`, never
            // `LayerId::background()`), dispatches queued menu actions (no
            // painting), and registers pane-border drag sensors via
            // `ui.interact()` (which does not append any shape). So the
            // background layer's `PaintList` has not grown since the menu
            // bar / tab bar chrome painted (before this `CentralPanel`
            // closure began); capturing the count here bounds the range to
            // exactly the band.
            let band_shape_start = ctx.graphics(|g| {
                g.get(egui::LayerId::background())
                    .map_or(0, |list| list.all_entries().len())
            });

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

            // Clone the per-window partial-present flag once, before the pane
            // loop, so each pane's `show()` can pass it into its PaintCallback
            // without re-borrowing `win` while `win` is mutably borrowed in
            // the loop (#435).
            let present_is_partial_for_panes = std::sync::Arc::clone(&win.present_is_partial);

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
                // The gutter inset is removed from the available width first so
                // the column count matches the rendered cell grid (the widget
                // shifts its terminal rect right by the same inset).
                let pane_content_width = (content_rect.width() - gutter_inset_logical).max(0.0);
                let pane_width_chars = (pane_content_width / logical_char_w)
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

                // OSC 1338 HISTFILE reload trigger (Task 72.15).  When the
                // shell-integration scripts publish a new HISTFILE path
                // through `OSC 1338 ; HISTFILE=<path> ST`, the snapshot's
                // `shell_histfile` will diverge from the last value we
                // observed for this pane.  On change, spawn an
                // OSC-priority loader (`SEED_SEQ_OSC=1`) which CAS-wins
                // over the env-derived load published earlier at spawn
                // time.  The decision is factored into a pure function
                // (`classify_osc_reload`) so the comparison logic is
                // exhaustively unit-tested independently of egui.
                {
                    use crate::gui::shell_history::OscReloadDecision;
                    let decision = crate::gui::shell_history::classify_osc_reload(
                        pane.shell_program.as_deref(),
                        pane_snap.shell_histfile.as_deref(),
                        pane.shell_histfile_last_seen.as_deref(),
                    );
                    match decision {
                        OscReloadDecision::NoChange => {}
                        OscReloadDecision::SpawnLoad { program, path } => {
                            tracing::debug!(
                                "shell_history: pane {pane_id} OSC 1338 reload \
                                 (program={program:?}, path={path:?})"
                            );
                            crate::gui::shell_history::spawn_loader_with_path(
                                program,
                                path,
                                std::sync::Arc::clone(&pane.history_seed),
                            );
                            pane.shell_histfile_last_seen
                                .clone_from(&pane_snap.shell_histfile);
                        }
                        OscReloadDecision::NoProgramAvailable { new_path } => {
                            tracing::trace!(
                                "shell_history: pane {pane_id} OSC 1338 \
                                 received but no resolved shell program \
                                 (new_path={new_path:?}); skipping reload"
                            );
                            pane.shell_histfile_last_seen
                                .clone_from(&pane_snap.shell_histfile);
                        }
                        OscReloadDecision::Cleared => {
                            tracing::trace!(
                                "shell_history: pane {pane_id} OSC 1338 \
                                 HISTFILE cleared; leaving existing seed in place"
                            );
                            pane.shell_histfile_last_seen = None;
                        }
                    }
                }

                let is_echo_off = self.config.security.password_indicator
                    && pane.echo_off.load(std::sync::atomic::Ordering::Relaxed);
                let is_active = pane_id == active_pane_id;

                // Broadcast input (Task 74): only the active pane fans out its
                // keyboard input, and only to the *other* panes. Non-active
                // panes and the broadcast-off case pass an empty slice.
                let key_broadcast_targets: Vec<crossbeam_channel::Sender<InputEvent>> = if is_active
                {
                    broadcast_senders
                        .iter()
                        .filter(|(id, _)| *id != pane_id)
                        .map(|(_, tx)| tx.clone())
                        .collect()
                } else {
                    Vec::new()
                };

                // Build a RecordingContext for this pane if recording is active.
                // Hold the Arc locally so the borrow in `RecordingContext.handle`
                // remains valid for the lifetime of `rec_ctx`.
                let rec_window_id = self.recording_window_id(window_id);
                let rec_handle = self.recording_swap.load_full();
                let rec_ctx = rec_handle.as_ref().map(|h| {
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
                            &self.config.command_blocks,
                            gutter_inset_logical,
                            &self.binding_map,
                            is_echo_off,
                            is_active,
                            pane_id,
                            rec_ctx.as_ref(),
                            &mut pane.pending_copy,
                            &key_broadcast_targets,
                            &present_is_partial_for_panes,
                        )
                    });
                let (left_clicked, deferred_actions) = show_result.inner;
                all_deferred_actions.extend(deferred_actions);

                // Task 114.7: drain any egui-blocked raw key events queued
                // this frame (keypad operators/directional, media,
                // print/pause/menu keys) for the active pane. Must run
                // here, after `show()` returned above, so
                // `pane.render_cache.super_pressed()` reflects the current
                // frame — draining earlier (or inside `on_raw_key_event`
                // itself) risks encoding against a stale Super state.
                if is_active && !win.pending_raw_keys.is_empty() {
                    // Per-pane overlays (search, command-history palette,
                    // right-click context menu) also own keyboard input and
                    // suppress normal terminal input; queued raw keys must be
                    // gated the same way so they cannot bypass those overlays.
                    let pane_input_suppressed = pane.view_state.search_state.is_open
                        || pane.view_state.command_history.is_open
                        || pane.view_state.context_menu_pos.is_some();
                    if ui_overlay_open || pane_input_suppressed {
                        // An overlay (rename/paste/close/broadcast dialog,
                        // menu, welcome/about window, save-layout, or a
                        // per-pane search/history/context menu) owns keyboard
                        // input this frame — the same gate that suppresses
                        // normal terminal input. Drop the queued raw keys
                        // instead of forwarding them to the PTY, so
                        // intercepted keys cannot bypass the overlay.
                        win.pending_raw_keys.clear();
                    } else {
                        let super_pressed = pane.render_cache.super_pressed();
                        crate::gui::terminal::input::drain_pending_raw_keys(
                            &mut win.pending_raw_keys,
                            &pane.input_tx,
                            &pane_snap,
                            super_pressed,
                            &key_broadcast_targets,
                        );
                    }
                }

                // ── Command history palette overlay (Ctrl+Shift+M) ───
                // Rendered here (not in `widget.show`) because the palette
                // needs `Pane`-owned data — `recent_commands`,
                // `history_seed`, and the `command_texts` cache — that the
                // widget does not have access to.  The palette is an
                // `egui::Area` overlay so its render order relative to the
                // widget body does not matter; what matters is that
                // `Pane` is in scope here.
                if pane.view_state.command_history.is_open {
                    use crate::gui::command_history::PaletteAction;
                    // Hold the Arc for the duration of the palette call
                    // so the borrow into `entries` remains valid.
                    let seed_arc = pane.history_seed.load_full();
                    let seed: Option<&Vec<String>> = if seed_arc.entries.is_empty() {
                        None
                    } else {
                        Some(seed_arc.entries.as_ref())
                    };
                    let action = crate::gui::command_history::show_command_history_palette(
                        ui,
                        &mut pane.view_state.command_history,
                        content_rect,
                        pane_id,
                        seed,
                        &pane.recent_commands,
                        &pane.command_texts,
                    );
                    match action {
                        PaletteAction::None => {}
                        PaletteAction::Close => {
                            pane.view_state.command_history.close();
                            crate::gui::command_history::log_close(pane_id);
                        }
                        PaletteAction::Submit(text) => {
                            let len = text.len();
                            let ok = crate::gui::command_history::send_command_text(
                                &pane.input_tx,
                                &text,
                            );
                            if !ok {
                                crate::gui::command_history::log_submit_failure(pane_id, len);
                            }
                            pane.view_state.command_history.close();
                            crate::gui::command_history::log_close(pane_id);
                        }
                    }
                }

                // Focus transfer (Task 110): a non-active pane is focused either
                // by an explicit left-click or (when focus-follows-mouse is
                // enabled) by the mouse hovering it. Following the mouse only
                // changes the *focused* (keyboard target) pane; it does not
                // retarget in-flight mouse input. Tab switching is unaffected.
                // The pointer is over at most one pane at a time, so this cannot
                // flicker between panes within a frame.
                let pointer_over_content = ui
                    .ctx()
                    .pointer_hover_pos()
                    .is_some_and(|pos| content_rect.contains(pos));
                let should_focus = !is_active
                    && crate::gui::panes::should_focus_inactive_pane(
                        left_clicked,
                        self.config.tabs.focus_follows_mouse,
                        pointer_over_content,
                    );
                if should_focus {
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

            // ── Frame-damage aggregation (#435) ───────────────────────
            //
            // Decide whether this whole frame was a pure cursor-only update,
            // so the windowing layer may skip the full-framebuffer clear and
            // present only the changed cursor region. This is deliberately
            // conservative: `Partial` is emitted ONLY when every condition
            // below positively proves nothing but the cursor blinked/moved.
            // Any doubt falls through to `Full` (a normal clear + present),
            // which is always correct. The windowing layer additionally
            // requires `buffer_age() == 1` before honoring `Partial`, so a
            // false positive here still cannot corrupt the frame on a fresh
            // or aged back buffer — but we avoid false positives regardless.
            //
            // A window-post shader recomposites the entire window every frame,
            // so it forces `Full`.
            let shader_recomposites = {
                let wpr = win
                    .window_post
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                wpr.is_active() || wpr.pending_shader.is_some()
            };
            // The pane-border active-pane highlight, menu/tab-bar hover tints,
            // and other chrome are painted by the plain egui painter every
            // frame — outside the per-pane damage tracking. They only *change*
            // pixels when the active pane changes (border moves) or the
            // pointer moves (hover). Presenting only the cursor rect on such a
            // frame would leave that chrome stale, so both force `Full`.
            let pointer_moving = ctx.input(|i| i.pointer.is_moving());
            let force_full =
                ui_overlay_open || shader_recomposites || active_pane_changed || pointer_moving;
            // A toast being visible animates its own region each frame.
            let toast_active = self
                .toasts
                .try_borrow()
                .is_ok_and(|stack| !stack.is_empty());
            // Inspect only the panes actually rendered this frame — the
            // entries in `pane_layout`. Under zoom, only the zoomed pane is
            // rendered; iterating the whole tree would read stale
            // `last_frame_cursor_damage` from non-rendered siblings and
            // wrongly force `Full` every frame. The per-pane -> `FrameDamage`
            // decision itself (and its full case-by-case rationale) is
            // extracted into `decide_frame_damage` (#436.2b) so both this
            // path and the future REPLAY path compute it identically.
            let active_tab = win.tabs.active_tab();
            let mut unresolved_pane = false;
            // OR-accumulated across every rendered pane: does ANY of them
            // have an open overlay that paints ABOVE the terminal band —
            // the `Order::Foreground` context menu, in-terminal search bar,
            // or command-history palette, OR the `Order::Tooltip` URL-hover
            // tooltip? All of these paint as TAIL chrome outside the captured
            // terminal-band range, so a REPLAY frame (which reuses the stale
            // cached tail) must not be permitted while one is open, or it
            // would vanish/ghost (#436.4b fix — see
            // `ChromeSignals::foreground_overlay_open`). The URL tooltip is
            // driven by `render_cache.cached_hovered_url`, which is recomputed
            // even under a STATIONARY mouse when PTY output scrolls new
            // content under the cursor — i.e. it can change on a frame with
            // no window input event, exactly the frame a REPLAY would
            // otherwise be chosen.
            let mut foreground_overlay_open = false;
            let mut per_pane_damage: Vec<frame_damage::PaneDamageInput> =
                Vec::with_capacity(pane_layout.len());
            for (pane_id, _) in &pane_layout {
                let Some(pane) = active_tab.pane_tree.find(*pane_id) else {
                    // A pane in the layout we cannot resolve -> be safe. This
                    // also aborts the `foreground_overlay_open` scan before
                    // every pane has been inspected, so conservatively treat
                    // an unresolved pane as if a foreground overlay were open
                    // — it already forces `FrameDamage::Full` below, and the
                    // chrome decision must be at least as conservative.
                    unresolved_pane = true;
                    foreground_overlay_open = true;
                    break;
                };
                foreground_overlay_open |= pane.view_state.context_menu_pos.is_some()
                    || pane.view_state.search_state.is_open
                    || pane.view_state.command_history.is_open
                    || pane.render_cache.hover_tooltip_active();
                per_pane_damage.push(frame_damage::PaneDamageInput {
                    bell_active: pane.view_state.bell_since.is_some(),
                    cursor_damage: pane.render_cache.last_frame_cursor_damage,
                });
            }
            win.pending_frame_damage = frame_damage::decide_frame_damage(
                force_full || unresolved_pane,
                toast_active,
                &per_pane_damage,
            );

            // ── Chrome-damage signals (#436.3 §3.3) ───────────────────
            //
            // Stage the individual §3.3 signals this frame, computed from
            // values already available here (several — `ui_overlay_open`,
            // `active_pane_changed`, `shader_recomposites`, `ppp_changed`,
            // `chrome_focus_changed`, per-pane bell state — only exist inside
            // this `CentralPanel` closure). The final `ChromeDamage` decision
            // additionally needs the after-toast-render dismissible-presence
            // sample, which can only be taken once this closure returns (the
            // toast overlay renders after it) — so `win.pending_chrome_signals`
            // is a staging value, combined into `win.pending_chrome_damage`
            // near the end of `update()`.
            let chrome_tab_snapshot = chrome_damage::ChromeTabSnapshot {
                tab_ids: win.tabs.iter().map(|t| t.id).collect(),
                active_tab_id: Some(win.tabs.active_tab().id),
                tab_titles: win
                    .tabs
                    .iter()
                    .map(|t| {
                        t.display_name(
                            self.config.tab_title.policy,
                            &self.config.tab_title.separator,
                        )
                        .into_owned()
                    })
                    .collect(),
                pane_ids: pane_layout.iter().map(|(id, _)| *id).collect(),
                zoomed_pane,
                broadcast_input: win.tabs.active_tab().broadcast_input,
            };
            let chrome_tab_diff = chrome_damage::diff_tab_snapshots(
                &win.prev_chrome_tab_snapshot,
                &chrome_tab_snapshot,
            );
            win.prev_chrome_tab_snapshot = chrome_tab_snapshot;

            win.pending_chrome_signals = chrome_damage::ChromeSignals {
                any_overlay_open: ui_overlay_open,
                style_changed: chrome_style_changed,
                active_pane_changed,
                tab_set_changed: chrome_tab_diff.tab_set_changed,
                tab_title_changed: chrome_tab_diff.tab_title_changed,
                pane_layout_changed: chrome_tab_diff.pane_layout_changed,
                broadcast_state_changed: chrome_tab_diff.broadcast_state_changed,
                shader_active: shader_recomposites,
                bell_active: per_pane_damage.iter().any(|p| p.bell_active),
                toast_active,
                size_changed: chrome_size_changed,
                ppp_changed,
                focus_changed: chrome_focus_changed,
                warming_up: chrome_warming_up,
                foreground_overlay_open,
            };

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
                                wpr.last_error = Some(format!("Renderer init failed: {e}"));
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
                                        wpr.last_error =
                                            Some(format!("Shader compile failed: {e}"));
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
            // Draw "surround the active pane" highlighted borders (Task 109).
            // Every edge of the active pane that is an interior divider is
            // highlighted full-length in the active color; the rest of each
            // divider is inactive. Outer window edges are never dividers, so
            // they are never highlighted. Each pane's own edges light up, so
            // stacked / nested panes stay distinguishable (a middle stacked
            // pane lights its top AND bottom; its neighbours light only the
            // shared edge).
            //
            // The one exception is a tab with EXACTLY two panes: they share a
            // single full-span divider, so surrounding either pane lights the
            // same line and the focused pane is indistinguishable. In that
            // case the divider is half-filled on the active pane's side
            // (the classic tmux behaviour).
            let broadcast_active = win.tabs.active_tab().broadcast_input;
            if has_multiple_panes && zoomed_pane.is_none() {
                let painter = ui.painter();
                // Broadcast mode (Task 74) tints every split border yellow so
                // the user has a constant visual reminder that keystrokes are
                // fanning out to every pane.  Otherwise the active pane's
                // edges use the theme's bright-blue (ansi[12]) — the themed
                // equivalent of the original hardcoded blue, distinct from the
                // command-block status-gutter colors (green/red/yellow) — and
                // the rest are gray.
                let (inactive_color, active_color) = if broadcast_active {
                    (
                        egui::Color32::from_rgb(180, 150, 40),
                        egui::Color32::from_rgb(240, 200, 60),
                    )
                } else {
                    let theme = freminal_common::themes::by_slug(
                        self.config.theme.active_slug(win.os_dark_mode),
                    )
                    .unwrap_or(&freminal_common::themes::CATPPUCCIN_MOCHA);
                    let (br, bg, bb) = theme.ansi[12];
                    (
                        egui::Color32::from_gray(80),
                        egui::Color32::from_rgb(br, bg, bb),
                    )
                };

                // Rect of the currently focused pane; used to decide which
                // divider segments border it.
                let active_rect = pane_layout
                    .iter()
                    .find(|(id, _)| *id == active_pane_id)
                    .map(|(_, r)| *r);

                let border_rects = win
                    .tabs
                    .active_tab()
                    .pane_tree
                    .split_borders(available_rect, active_pane_id)
                    .unwrap_or_default();

                // Tolerance for matching a divider coordinate to a pane edge.
                let edge_epsilon: f32 = 1.0;

                // Exactly-two-pane tabs share a single divider; half-fill it
                // on the active pane's side rather than surrounding (which
                // would be ambiguous). `pane_layout` holds every leaf rect.
                let exactly_two_panes = pane_layout.len() == 2;

                let stroke = |from, to, color| {
                    painter.line_segment([from, to], egui::Stroke::new(border_width, color));
                };

                for border in &border_rects {
                    let r = border.rect;

                    if exactly_two_panes {
                        // Half-fill: the active pane's side gets the active
                        // color; the other half stays inactive. `active_in_first`
                        // is true when the active pane is the first child
                        // (top for a vertical line, left for a horizontal line).
                        let (first_color, second_color) = match border.active_in_first {
                            Some(true) => (active_color, inactive_color),
                            Some(false) => (inactive_color, active_color),
                            None => (inactive_color, inactive_color),
                        };
                        match border.direction {
                            panes::SplitDirection::Horizontal => {
                                // Vertical line — split top/bottom.
                                let mid_y = f32::midpoint(r.min.y, r.max.y);
                                stroke(r.left_top(), egui::pos2(r.min.x, mid_y), first_color);
                                stroke(egui::pos2(r.min.x, mid_y), r.left_bottom(), second_color);
                            }
                            panes::SplitDirection::Vertical => {
                                // Horizontal line — split left/right.
                                let mid_x = f32::midpoint(r.min.x, r.max.x);
                                stroke(r.left_top(), egui::pos2(mid_x, r.min.y), first_color);
                                stroke(egui::pos2(mid_x, r.min.y), r.right_top(), second_color);
                            }
                        }
                        continue;
                    }

                    // 3+ panes: surround. The whole divider is drawn inactive
                    // first…
                    match border.direction {
                        panes::SplitDirection::Horizontal => {
                            stroke(r.left_top(), r.left_bottom(), inactive_color);
                        }
                        panes::SplitDirection::Vertical => {
                            stroke(r.left_top(), r.right_top(), inactive_color);
                        }
                    }

                    // …then the segment along the active pane's edge is
                    // redrawn full-length in the active color.
                    if let Some(seg) = active_rect
                        .and_then(|ar| panes::active_highlight_segment(border, ar, edge_epsilon))
                    {
                        match border.direction {
                            panes::SplitDirection::Horizontal => {
                                stroke(seg.left_top(), seg.left_bottom(), active_color);
                            }
                            panes::SplitDirection::Vertical => {
                                stroke(seg.left_top(), seg.right_top(), active_color);
                            }
                        }
                    }
                }
            }

            // Broadcast label (Task 74): when broadcast is active, paint a
            // small "BROADCAST" tag in the top-right corner of every visible
            // pane.  Top-right is chosen so it never collides with the
            // password-prompt lock icon (which lives in the tab/menu bar, not
            // the pane area).  Drawn for the zoomed pane too.
            if broadcast_active {
                let painter = ui.painter();
                let label_color = egui::Color32::from_rgb(240, 200, 60);
                let bg = egui::Color32::from_rgba_unmultiplied(0, 0, 0, 160);
                for (_pane_id, pane_rect) in &pane_layout {
                    let anchor = egui::pos2(pane_rect.max.x - 4.0, pane_rect.min.y + 4.0);
                    let galley = painter.layout_no_wrap(
                        "BROADCAST".to_owned(),
                        egui::FontId::monospace(10.0),
                        label_color,
                    );
                    let text_rect = egui::Align2::RIGHT_TOP
                        .anchor_size(anchor, galley.size())
                        .expand(2.0);
                    painter.rect_filled(text_rect, 2.0, bg);
                    painter.galley(
                        text_rect.left_top() + egui::vec2(2.0, 2.0),
                        galley,
                        label_color,
                    );
                }
            }
            // ── Terminal band: capture the end of the shape range (#436.4a) ──
            //
            // Read the background layer's current shape count as
            // `band_shape_end`, so `[band_shape_start, band_shape_end)` is
            // exactly the band's range within `LayerId::background()`'s
            // `PaintList`. `run_frame` slices `full_output.shapes` by this
            // range directly (the background layer drains first into
            // `full_output.shapes`), so — unlike 436.2a's approach — nothing
            // is cloned here: this is a pure read of the shape count, and
            // the real shapes are left in place in
            // `LayerId::background()`'s `PaintList` and drain into
            // `FullOutput.shapes` exactly as before. `run_frame`'s 3-way
            // split (head/band/tail, each tessellated and painted
            // separately, in that order) reconstructs the same total shape
            // set with the same paint order as the pre-#436.4a single-call
            // path, so this frame renders byte-identically to before.
            //
            // Capture point justification (end): nothing between the
            // broadcast-label loop above and here paints into the background
            // layer either — this is the very next statement — so
            // `band_shape_end` is exactly the count after the pre-clear
            // callback through the broadcast label, i.e. exactly the end of
            // the band.
            let band_shape_end = ctx.graphics(|g| {
                g.get(egui::LayerId::background())
                    .map_or(band_shape_start, |list| list.all_entries().len())
            });
            win.pending_terminal_band_range = band_shape_start..band_shape_end;

            // Handle key actions that couldn't be dispatched at the input
            // layer because they require full GUI state.
            for action in all_deferred_actions {
                self.dispatch_deferred_action(action, &mut win, window_id, handle);
            }

            // Drain a pending paste injected by the windowing layer's
            // `Event::Paste` (Task 77). Use the already-read text; do not
            // re-read the clipboard here. `bypass_guard` (the PasteUnsafe
            // action, Ctrl+Shift+Alt+V) sends directly without analysis.
            let pending_paste = win
                .tabs
                .active_tab_mut()
                .active_pane_mut()
                .and_then(|pane| pane.view_state.pending_paste.take());
            if let Some(pending) = pending_paste {
                if pending.bypass_guard {
                    Self::send_paste_to_active_pane(&mut win, pending.text);
                } else {
                    self.guarded_paste_text(&mut win, pending.text);
                }
            }

            // Handle deferred close-pane (needs `ui` for ViewportCommand::Close).
            // Routed through the close guard (Task 98): a running foreground
            // command opens the confirmation dialog instead of closing.
            if win.pending_close_pane {
                win.pending_close_pane = false;
                self.guarded_close_pane(ui, &mut win);
            }

            // Handle deferred directional focus (needs layout rects).
            if let Some(dir) = win.pending_focus_direction.take() {
                Self::focus_pane_in_direction(dir, available_rect, &mut win);
            }

            // Keep the window title bar in sync with the active tab's title.
            // This handles tab switches, OSC 0/2 title changes, and restore
            // from the title stack — all in one place.
            //
            // The window title is resolved under the configured tab-title
            // policy (`[tab_title] policy`), combining the user-assigned
            // custom name with the shell-asserted OSC title.  Under the
            // `OscWins` policy a shell title clears the custom name; under
            // every other policy the custom name persists.
            //
            // Only issue the viewport command when the title actually changed;
            // calling `send_viewport_cmd` unconditionally every frame triggers
            // an infinite repaint loop (~3 % idle CPU).
            let active_tab = win.tabs.active_tab();
            let active_title = active_tab.display_name(
                self.config.tab_title.policy,
                &self.config.tab_title.separator,
            );
            let window_title = if active_title.is_empty() {
                "Freminal"
            } else {
                active_title.as_ref()
            };
            if window_title != win.last_window_title {
                window_title.clone_into(&mut win.last_window_title);
                ctx.send_viewport_cmd(egui::ViewportCommand::Title(win.last_window_title.clone()));
            }

            // Stash this frame's own repaint request for #436.4b's
            // `chrome_repaint_settled` gate (drained by
            // `App::take_terminal_requested_delay`): the NEXT frame's replay
            // decision needs to know what delay THIS frame itself asked for,
            // to distinguish "only our own blink/content scheduling wants a
            // wake" from "something egui-internal also wants one sooner".
            win.pending_terminal_requested_delay = shortest_repaint_delay;

            // Schedule a repaint at the shortest interval needed by any pane.
            if let Some(delay) = shortest_repaint_delay {
                ctx.request_repaint_after(delay);
            }
        };

        if let Some(mut root_ui) = chrome_root_ui {
            let _panel_response = CentralPanel::default().show(&mut root_ui, central_body);
        } else {
            // REPLAY: construct the band's `Ui` directly at the cached
            // content rect, in the SAME background layer chrome uses, so the
            // terminal band's shapes land where a FULL frame's `CentralPanel`
            // content would have put them. The id is NOT the same, though:
            // this uses `Id::new("freminal_root")` directly (the root Ui's
            // own id), while the FULL path's `CentralPanel::show` allocates
            // its content `Ui` via `root_ui.new_child(..)` with no explicit
            // id salt, which egui auto-derives from `root_ui`'s id plus a
            // per-frame child-index counter — a different, and not
            // necessarily stable, id. This is a known accepted limitation:
            // any widget that keys persistent state off its `Ui`-derived id
            // (e.g. collapsing-header open state) could in principle churn
            // that state across a Full<->Replay mode toggle. In practice
            // this is inert, because real user interaction with such a
            // widget forces `ChromeMode::Full` on the same frame (via
            // `ui_overlay_open`/pointer-motion/etc.), so REPLAY is only ever
            // entered while nothing is interacting with mismatched-id
            // widgets. Tracked as a follow-up if a concrete widget is ever
            // found to rely on cross-mode id stability.
            // `decide_chrome_mode` only chooses `Replay` when the chrome
            // cache is valid at this frame's size/ppp, which is only ever
            // populated on a FULL frame — and every FULL frame sets
            // `cached_central_rect` (via `central_body`) before that cache is
            // populated — so falling back to egui's own content rect
            // (`cached_central_rect_for_replay`'s fallback, computed above)
            // should be unreachable in practice.
            let mut band_ui = egui::Ui::new(
                ctx.clone(),
                egui::Id::new("freminal_root"),
                egui::UiBuilder::new()
                    .layer_id(egui::LayerId::background())
                    .max_rect(cached_central_rect_for_replay),
            );
            central_body(&mut band_ui);
        }

        // Render the app-level toast stack as an overlay on top of all panels.
        // Toasts are shared across every window, so they appear consistently
        // regardless of which window the user is looking at. TAIL chrome
        // (#436.4b): skipped on REPLAY for the same reason as the dialogs
        // above — a toast being visible forces `ChromeDamage::Changed` every
        // frame it is (`ChromeSignals::toast_active`), so a REPLAY frame can
        // only ever be entered while the stack is provably empty, making
        // `.show()` a no-op here anyway.
        if chrome_mode == freminal_windowing::ChromeMode::Full
            && let Ok(mut stack) = self.toasts.try_borrow_mut()
        {
            stack.show(ctx);
        }

        // ── Chrome-damage (#436.3): §3.5 "after" sample + final decision ─────
        //
        // Taken here — after every dismissible element's `.show(ctx)` this
        // frame, including the toast stack's above — and diffed against
        // `chrome_dismissible_before` (sampled at the very top of this
        // function, before any of them showed) to catch a self-dismissal
        // that happened DURING this frame's rendering (adversarial finding
        // 1: e.g. a toast expiring in its own `.show()` and requesting no
        // further repaint because the stack is now empty). Also diffed
        // against `win.prev_dismissible_presence` (last frame's own "after"
        // sample) to catch a transition caused by something other than the
        // element's own self-dismissal (e.g. a menu action closing a
        // dialog). Either comparison finding a difference counts as a
        // transition — see `chrome_damage::dismissible_presence_transitioned`'s
        // doc for why the intra-frame comparison is the load-bearing one.
        let chrome_dismissible_after = self.sample_dismissible_presence(&win);
        let chrome_presence_transitioned = chrome_damage::dismissible_presence_transitioned(
            chrome_dismissible_before,
            chrome_dismissible_after,
        ) || chrome_damage::dismissible_presence_transitioned(
            win.prev_dismissible_presence,
            chrome_dismissible_after,
        );
        win.prev_dismissible_presence = chrome_dismissible_after;

        // §3.5's "+ next frame FULL" half: read last frame's pending flag as
        // this frame's settle input, then reassign it to THIS frame's own
        // transition result for the next frame to read (no separate "reset"
        // step — see `decide_chrome_damage`'s doc).
        let chrome_settle_frame_pending = win.chrome_settle_pending;
        win.chrome_settle_pending = chrome_presence_transitioned;

        win.pending_chrome_damage = chrome_damage::decide_chrome_damage(
            &win.pending_chrome_signals,
            chrome_presence_transitioned,
            chrome_settle_frame_pending,
        );

        // ── #435/#436 composition (§6): chrome change forces FrameDamage::Full ──
        //
        // The #435 partial-present decision (`pending_frame_damage`, computed
        // in `central_body`) and the #436 chrome-cache decision
        // (`pending_chrome_damage`, just computed) are separate but MUST
        // agree. See `frame_damage::compose_with_chrome_damage` for the full
        // rationale; in short, a frame that changed chrome pixels must not be
        // presented `Partial` (it would leave chrome outside the cursor rect
        // stale under #435's `buffer_age() == 1` assumption). Reconciled here,
        // after both decisions are final, via the pure helper.
        win.pending_frame_damage = frame_damage::compose_with_chrome_damage(
            std::mem::replace(
                &mut win.pending_frame_damage,
                freminal_windowing::FrameDamage::Full,
            ),
            win.pending_chrome_damage,
        );

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

    fn on_raw_key_event(
        &mut self,
        window_id: WindowId,
        event: freminal_windowing::RawKeyEvent,
        mods: freminal_windowing::RawKeyMods,
    ) {
        // Task 114.7: queue only -- do NOT encode here. This callback fires
        // at winit-event time, outside the render/`update()` path where the
        // active pane, its snapshot, and the true per-pane `super_pressed`
        // are in scope. The queue is drained and encoded on the render path
        // in `update()` (see `terminal::input::drain_pending_raw_keys`).
        //
        // No explicit repaint request is needed here: `event_loop.rs`'s
        // `KeyboardInput` intercept already sets `state.repaint_at =
        // Some(Instant::now())` immediately after calling this method, so a
        // repaint (and therefore a drain) is already guaranteed this cycle.
        let Some(win) = self.windows.get_mut(&window_id) else {
            trace!(?window_id, "raw key event for unknown window; dropping");
            return;
        };
        win.pending_raw_keys.push((event, mods));
    }
}

impl FreminalGui {
    /// Sample the presence of every dismissible chrome element (#436 §3.5).
    ///
    /// Called twice per `update()` for the window being rendered — once as
    /// early as possible (before any dialog's `.show(ctx)` this frame) and
    /// once after all of them (including the shared toast stack's `.show`,
    /// which runs after `win` is reinserted — see the call site) — so the
    /// two samples can be diffed to catch a self-dismissal that happens
    /// DURING a `.show()` call this same frame (adversarial finding 1).
    fn sample_dismissible_presence(
        &self,
        win: &PerWindowState,
    ) -> chrome_damage::DismissiblePresence {
        chrome_damage::DismissiblePresence {
            about: self.about_window_open,
            welcome: self.welcome.is_open(),
            paste_dialog: win.paste_dialog.is_open(),
            broadcast_dialog: win.broadcast_dialog.is_open(),
            close_dialog: win.close_dialog.is_open(),
            save_layout_prompt: self.pending_save_layout.is_some(),
            any_toast: self
                .toasts
                .try_borrow()
                .is_ok_and(|stack| !stack.is_empty()),
        }
    }

    /// First-window spawn path when no layout or session restore will apply.
    ///
    /// Spawns a default single-pane PTY.  PTY-spawn failures surface as a
    /// user-visible toast (the window still opens, empty) rather than
    /// aborting the application.  This mirrors the subsequent-window
    /// branch's error handling.
    #[allow(clippy::too_many_arguments)] // Helper inherits all of on_window_created's context.
    fn create_first_window_with_default_pty(
        &mut self,
        window_id: WindowId,
        ctx: &egui::Context,
        handle: &freminal_windowing::WindowHandle<'_>,
        inner_size: (u32, u32),
        os_dark_mode: bool,
        repaint_handle: Arc<std::sync::OnceLock<(freminal_windowing::RepaintProxy, WindowId)>>,
        window_post: Arc<Mutex<WindowPostRenderer>>,
    ) {
        let proxy = handle.event_loop_proxy();
        let _ = repaint_handle.set((proxy, window_id));

        let theme = freminal_common::themes::by_slug(self.config.theme.active_slug(os_dark_mode))
            .unwrap_or(&freminal_common::themes::CATPPUCCIN_MOCHA);
        rendering::set_egui_options(
            ctx,
            theme,
            self.config.ui.background_opacity,
            &self.gui_theme,
        );

        let terminal_widget = FreminalTerminalWidget::new(ctx, &self.config).unwrap_or_else(|e| {
            tracing::error!("fatal: failed to initialise terminal widget (font manager): {e}");
            std::process::exit(1);
        });
        let (cell_w, cell_h) = terminal_widget.cell_size();
        let initial_size = Self::compute_initial_size(inner_size.0, inner_size.1, cell_w, cell_h);

        let pane_id = self
            .pane_id_gen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .next_id();

        let channels = match super::pty::spawn_pty_tab(
            &self.args,
            self.config.scrollback.limit,
            super::pty::PtyTabInitialState {
                theme,
                auto_detect_urls: self.config.ui.auto_detect_urls,
                cursor_style: freminal_common::cursor::CursorVisualStyle::from_config(
                    &self.config.cursor.shape,
                    self.config.cursor.blink,
                ),
            },
            &repaint_handle,
            initial_size,
            super::pty::PtyTabConfig {
                cwd: None,
                shell_override: None,
                extra_env: None,
                recording_swap: self.recording_swap.clone(),
                recording_pane_id: pane_id.raw().try_into().unwrap_or(u32::MAX),
                set_term_program: self.config.shell_integration.set_term_program,
            },
        ) {
            Ok(channels) => channels,
            Err(e) => {
                error!("Failed to spawn initial PTY: {e}");
                // This is the first/only window and it has no other live
                // panes — a failed spawn here is fatal.  Record it so the
                // window renders the fatal-error panel (with an Exit button)
                // instead of a blank surface.  We deliberately do NOT close
                // the window: closing the only window would quit the app
                // before the user can read why.
                self.set_fatal_error(
                    "Failed to start shell",
                    format!("The shell could not be started:\n\n{e}"),
                );
                return;
            }
        };

        let pane = panes::Pane::from_channels(
            pane_id,
            channels,
            Arc::clone(&window_post),
            "Terminal".to_owned(),
        );

        let tab = Tab::new(super::tabs::TabId::first(), pane);

        // Inform the initial tab about the configured theme mode and real
        // OS dark/light preference so DECRPM ?2031 responses are correct.
        if let Some(active) = tab.active_pane()
            && let Err(e) =
                active
                    .input_tx
                    .send(freminal_terminal_emulator::io::InputEvent::ThemeModeUpdate(
                        self.config.theme.mode,
                        os_dark_mode,
                    ))
        {
            error!("Failed to send ThemeModeUpdate to initial tab: {e}");
        }

        // Apply initial background image from config (if set).
        let initial_bg_path = self.config.ui.background_image.clone();
        if initial_bg_path.is_some()
            && let Ok(panes_list) = tab.pane_tree.iter_panes()
        {
            for p in panes_list {
                if let Ok(mut rs) = p.render_state.lock() {
                    rs.set_pending_bg_image(initial_bg_path.clone());
                }
            }
        }

        let win = Self::new_per_window_state(
            tab,
            terminal_widget,
            os_dark_mode,
            window_post,
            repaint_handle,
        );
        self.windows.insert(window_id, win);
    }

    /// Render the fatal-error panel for a window that has no
    /// [`PerWindowState`] because the only/last shell failed to spawn.
    ///
    /// Shows the stored title, the underlying error detail, and a single
    /// "Exit" button that quits the application.  Replaces what would
    /// otherwise be a blank, unrecoverable window.
    fn render_fatal_error(&self, ctx: &egui::Context) {
        let Some((title, detail)) = self.fatal_error.as_ref() else {
            return;
        };
        // Match the rest of the GUI: build a root Ui covering the window and
        // reserve space from it via `show` (the non-deprecated API; `show_inside`
        // was renamed to `show` in egui 0.35).
        let mut root_ui = egui::Ui::new(
            ctx.clone(),
            egui::Id::new("freminal_fatal_error_root"),
            egui::UiBuilder::default(),
        );
        CentralPanel::default().show(&mut root_ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(48.0);
                ui.heading(title);
                ui.add_space(12.0);
                ui.label(detail);
                ui.add_space(24.0);
                if ui.button("Exit").clicked() {
                    std::process::exit(1);
                }
            });
        });
    }

    /// Construct a `PerWindowState` with default field values for all
    /// transient UI state.  Extracted to keep
    /// `create_first_window_with_default_pty` under the line limit.
    fn new_per_window_state(
        tab: Tab,
        terminal_widget: FreminalTerminalWidget,
        os_dark_mode: bool,
        window_post: Arc<Mutex<WindowPostRenderer>>,
        repaint_handle: Arc<std::sync::OnceLock<(freminal_windowing::RepaintProxy, WindowId)>>,
    ) -> PerWindowState {
        PerWindowState {
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
            renaming_tab: None,
            rename_buffer: String::new(),
            dragging_tab: None,
            last_tab_rects: Vec::new(),
            pending_menu_actions: Vec::new(),
            paste_dialog: super::paste_guard::PasteDialog::default(),
            broadcast_dialog: super::broadcast_guard::BroadcastConfirmDialog::default(),
            close_dialog: super::close_guard::CloseGuardDialog::default(),
            pending_force_close: false,
            pending_raw_keys: Vec::new(),
            pending_frame_damage: freminal_windowing::FrameDamage::Full,
            pending_terminal_band_range: 0..0,
            present_is_partial: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            previous_active_pane_key: None,
            pending_chrome_damage: freminal_windowing::ChromeDamage::Changed,
            pending_chrome_signals: chrome_damage::ChromeSignals::default(),
            chrome_settle_pending: false,
            prev_dismissible_presence: chrome_damage::DismissiblePresence::default(),
            prev_chrome_tab_snapshot: chrome_damage::ChromeTabSnapshot::default(),
            prev_window_focused: false,
            chrome_frames_rendered: 0,
            pending_terminal_requested_delay: None,
            cached_central_rect: None,
            chrome_head_rects: None,
            chrome_border_rects: Vec::new(),
        }
    }

    /// First-window spawn path when a startup layout or session restore
    /// will populate the window's tabs.
    ///
    /// Resolves the layout (from `--layout`, `startup.layout`, or
    /// `last_session.toml`), pushes every resolved window into
    /// `pending_layout_windows`, builds the first `PerWindowState` by
    /// popping the first entry, and creates OS windows for the rest.
    ///
    /// If resolution fails, pushes an error toast and falls back to
    /// spawning a default PTY so the user still gets a usable terminal.
    #[allow(clippy::too_many_arguments)] // Helper inherits all of on_window_created's context.
    fn create_first_window_from_layout_or_restore(
        &mut self,
        window_id: WindowId,
        ctx: &egui::Context,
        handle: &freminal_windowing::WindowHandle<'_>,
        inner_size: (u32, u32),
        os_dark_mode: bool,
        repaint_handle: Arc<std::sync::OnceLock<(freminal_windowing::RepaintProxy, WindowId)>>,
        window_post: Arc<Mutex<WindowPostRenderer>>,
    ) {
        let Some(resolved) = self.resolve_startup_layout_or_session() else {
            // Resolution failed and a toast was already pushed.  Fall
            // back to a default PTY so the window is still useful.
            self.create_first_window_with_default_pty(
                window_id,
                ctx,
                handle,
                inner_size,
                os_dark_mode,
                repaint_handle,
                window_post,
            );
            return;
        };

        // Queue all resolved windows.  The first is consumed below for
        // this window; subsequent ones trigger fresh
        // `on_window_created` callbacks that will pop and build their
        // own `PerWindowState`.
        for w in &resolved.windows {
            self.pending_layout_windows.push_back(w.clone());
        }

        // Build this first window by popping the first queued entry.
        let cmds_opt = self.build_window_from_pending_layout(
            window_id,
            ctx,
            handle,
            inner_size,
            os_dark_mode,
            Some((repaint_handle, window_post)),
        );

        // Create OS windows for any remaining pending layout windows.
        // Their sizes/positions are taken from the layout.
        let remaining: Vec<_> = self.pending_layout_windows.iter().cloned().collect();
        for extra_window in remaining {
            handle.create_window(freminal_windowing::WindowConfig {
                title: "Freminal".to_owned(),
                inner_size: extra_window.size.map(<[u32; 2]>::into),
                position: extra_window.position.map(<[i32; 2]>::into),
                transparent: true,
                icon: self.icon.clone(),
                app_id: Some("freminal".into()),
            });
        }

        if let Some(cmds) = cmds_opt {
            self.inject_layout_commands(&cmds);
        } else if !self.has_live_window() {
            // The first window's tabs could not be built (every pane spawn
            // failed) and no other window holds a live pane.  Without this
            // the window would be left blank and unrecoverable.  Record a
            // fatal error so the next frame renders the Exit panel.  A more
            // specific per-pane spawn error has already been surfaced as a
            // toast by `spawn_pane_from_leaf`; this is the catch-all that
            // guarantees a visible, actionable failure state.
            self.set_fatal_error(
                "Failed to start session",
                "No shell could be started for the restored session or \
                 layout.\n\nThis usually means the shell program could not \
                 be launched. Check your shell configuration, or try \
                 launching with shell integration disabled \
                 ([shell_integration] set_term_program = false).",
            );
        }
    }

    /// Resolve the startup layout or session-restore source to a
    /// `ResolvedLayout`, if any applies.
    ///
    /// Tries in priority order:
    /// 1. `--layout` CLI flag
    /// 2. `startup.layout` in config
    /// 3. `last_session.toml` when `startup.restore_last_session` is on
    ///    and no positional command was supplied.
    ///
    /// Returns `None` if no source applies or if loading/resolution
    /// fails.  On failure, pushes an error toast so the caller can fall
    /// back to a default PTY.
    fn resolve_startup_layout_or_session(&self) -> Option<freminal_common::layout::ResolvedLayout> {
        // Priority 1 + 2: --layout / startup.layout.
        if let Some(name_or_path) = self
            .args
            .layout
            .clone()
            .or_else(|| self.config.startup.layout.clone())
        {
            let path = Self::resolve_startup_layout_path(&name_or_path);
            let positional: Vec<String> = self
                .args
                .layout_vars
                .iter()
                .filter(|s| !s.contains('='))
                .cloned()
                .collect();
            let var_map = self.args.layout_var_map();
            return match freminal_common::layout::Layout::from_file(&path) {
                Ok(layout) => match layout.apply_variables(&positional, &var_map).resolve() {
                    Ok(resolved) if resolved.windows.is_empty() => {
                        // A structurally-valid but empty layout (no windows /
                        // no panes) cannot produce a usable window.  Treat it
                        // as "no layout applies" so the caller falls back to a
                        // default shell rather than rendering a blank/fatal
                        // window.
                        error!("Layout '{}' contains no windows", path.display());
                        self.push_error_toast(
                            "Layout is empty",
                            Some(format!(
                                "{} defines no windows or panes; starting a default shell.",
                                path.display()
                            )),
                        );
                        None
                    }
                    Ok(resolved) => Some(resolved),
                    Err(e) => {
                        error!("Failed to resolve layout '{}': {e}", path.display());
                        self.push_error_toast(
                            "Failed to resolve layout",
                            Some(format!("{}: {e}", path.display())),
                        );
                        None
                    }
                },
                Err(e) => {
                    error!("Failed to load layout '{}': {e}", path.display());
                    self.push_error_toast(
                        "Failed to load layout",
                        Some(format!("{}: {e}", path.display())),
                    );
                    None
                }
            };
        }

        // Priority 3: session restore.
        let path = Self::last_session_path()?;
        if !path.exists() {
            return None;
        }
        match freminal_common::layout::Layout::from_file(&path).and_then(|l| {
            l.apply_variables(&[], &std::collections::HashMap::new())
                .resolve()
        }) {
            // A blank or zero-window `last_session.toml` deserializes to a
            // structurally-valid but empty `Layout` (every field is
            // `#[serde(default)]`), so parsing *succeeds* and we land here with
            // no windows.  This is exactly the corruption case observed when a
            // previous run was killed mid-write (e.g. an aggressive reboot
            // truncated the file): without this guard the empty layout produced
            // a blank window and a fatal-error panel at startup.  Treat it like
            // a parse failure — warn the user via a non-blocking toast and fall
            // back to a default shell so the terminal still starts.
            Ok(resolved) if resolved.windows.is_empty() => {
                error!(
                    "restore_last_session: {} contains no windows (blank/corrupt session)",
                    path.display()
                );
                self.push_error_toast(
                    "Could not restore last session",
                    Some(
                        "The saved session was empty or corrupt; starting a default shell."
                            .to_owned(),
                    ),
                );
                None
            }
            Ok(resolved) => Some(resolved),
            Err(e) => {
                error!(
                    "restore_last_session: failed to apply {}: {e}",
                    path.display()
                );
                self.push_error_toast(
                    "Failed to restore last session",
                    Some(format!("{}: {e}", path.display())),
                );
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{SettingsOwnerCloseDecision, settings_owner_close_decision};

    #[test]
    fn not_owner_ignores_other_state() {
        assert_eq!(
            settings_owner_close_decision(false, false),
            SettingsOwnerCloseDecision::NotOwner
        );
        assert_eq!(
            settings_owner_close_decision(false, true),
            SettingsOwnerCloseDecision::NotOwner,
            "a non-owner window closing must never touch the settings guard, \
             even if `has_unsaved` happens to be set for some other window"
        );
    }

    #[test]
    fn clean_owner_closes_now() {
        assert_eq!(
            settings_owner_close_decision(true, false),
            SettingsOwnerCloseDecision::CloseNow
        );
    }

    #[test]
    fn dirty_owner_is_vetoed_with_prompt() {
        assert_eq!(
            settings_owner_close_decision(true, true),
            SettingsOwnerCloseDecision::VetoWithPrompt
        );
    }

    // ── Terminal band shape-index-range extraction (#436.2a / #436.4a) ──
    //
    // These tests pin the underlying mechanism `update()` uses to bound the
    // terminal band's shape-index range: paint the band into the SAME
    // `LayerId::background()` layer chrome already uses (no dedicated
    // layer), remember the shape count before ("`band_shape_start`"), and
    // read `all_entries().skip(start)` to identify exactly the range
    // appended since. Production (as of #436.4a) captures `band_shape_end`
    // the same way and hands back `[band_shape_start, band_shape_end)` as a
    // range via `App::take_terminal_band_range`, rather than cloning the
    // shapes out here — but the boundary-counting primitive these tests
    // exercise is identical either way. A full `FreminalGui`/`PerWindowState`
    // cannot be constructed headlessly (`freminal_windowing::WindowId` has
    // no public constructor outside the real winit event loop), so this
    // validates the extraction primitive directly against a bare
    // `egui::Context`, independent of the app.

    #[test]
    fn band_shape_range_extraction_finds_only_shapes_painted_after_start() {
        let ctx = egui::Context::default();

        // A shape painted *before* `band_shape_start` is captured (mirrors
        // chrome — menu bar, tab bar — painting into the background layer
        // earlier in the same pass) must NOT be included in the extracted
        // range.
        let mut extracted: Vec<egui::epaint::ClippedShape> = Vec::new();
        let mut chrome_shape_count = 0usize;
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            // "Chrome" shape, painted before the band region starts.
            ui.painter().rect_filled(
                egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(5.0, 5.0)),
                0.0,
                egui::Color32::BLUE,
            );
            chrome_shape_count = ui.ctx().graphics(|g| {
                g.get(egui::LayerId::background())
                    .map_or(0, |list| list.all_entries().len())
            });

            // Capture point, exactly as production does immediately before
            // the band region.
            let band_shape_start = ui.ctx().graphics(|g| {
                g.get(egui::LayerId::background())
                    .map_or(0, |list| list.all_entries().len())
            });

            // Band shapes, painted into the same `ui` (background layer).
            ui.painter().rect_filled(
                egui::Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(10.0, 10.0)),
                0.0,
                egui::Color32::RED,
            );
            ui.painter().rect_filled(
                egui::Rect::from_min_size(egui::pos2(30.0, 30.0), egui::vec2(10.0, 10.0)),
                0.0,
                egui::Color32::GREEN,
            );

            extracted = ui.ctx().graphics(|g| {
                g.get(egui::LayerId::background())
                    .map_or_else(Vec::new, |list| {
                        list.all_entries().skip(band_shape_start).cloned().collect()
                    })
            });
        });

        assert_eq!(
            chrome_shape_count, 1,
            "sanity: exactly one chrome shape painted before the band"
        );
        assert_eq!(
            extracted.len(),
            2,
            "expected exactly the two band shapes, none of the chrome shape painted \
             before `band_shape_start`"
        );
    }

    #[test]
    fn band_shape_range_extraction_is_a_clone_not_a_drain() {
        // 436.2a's correctness requirement: extraction must NOT remove the
        // shapes from the background layer (that is deferred to 436.4,
        // alongside separate band painting). Confirms the real shapes are
        // still present after our clone-only extraction, and that egui's
        // own `end_pass` still drains them into `FullOutput.shapes` — i.e.
        // rendering is unaffected by the extraction seam existing.
        let ctx = egui::Context::default();

        let full_output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let band_shape_start = ui.ctx().graphics(|g| {
                g.get(egui::LayerId::background())
                    .map_or(0, |list| list.all_entries().len())
            });

            ui.painter().rect_filled(
                egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(10.0, 10.0)),
                0.0,
                egui::Color32::RED,
            );

            // Clone-only extraction (no removal from the layer), exactly as
            // production does in this subtask.
            let extracted: Vec<egui::epaint::ClippedShape> = ui.ctx().graphics(|g| {
                g.get(egui::LayerId::background())
                    .map_or_else(Vec::new, |list| {
                        list.all_entries().skip(band_shape_start).cloned().collect()
                    })
            });
            assert!(!extracted.is_empty());
        });

        assert!(
            !full_output.shapes.is_empty(),
            "the background layer's shapes must still drain into FullOutput.shapes \
             (byte-identical rendering) because this subtask does not remove them \
             — that is 436.4's job, done together with separate band painting"
        );
    }

    // ── Regression test: same-layer widgets are not cross-layer-hidden ──
    //
    // This is the blocker the adversarial review caught in the FIRST attempt
    // at 436.2a: routing the terminal band into a *second*
    // `Order::Background` layer (distinct from `LayerId::background()`,
    // which chrome — menu bar, tab bar, and the `CentralPanel`'s own root
    // widget rect — paints into) trips egui 0.35's cross-layer hit-test
    // "hidden" rule (`egui-0.35.0/src/hit_test.rs:145-148`): a widget is
    // hidden from hover/click/drag if a LATER widget on a DIFFERENT layer
    // contains its rect. `CentralPanel`'s content-area widget rect covers
    // the whole band, so once the band moved to its own layer, every
    // `ui.interact()` widget inside the band (e.g. the command-block gutter
    // hover highlight) was liable to be permanently hidden — the two
    // untracked `Order::Background` layers tie-break by `IdMap` (hash)
    // iteration order (`nohash_hasher::IntMap`), which the second test below
    // reproduces deterministically for the exact `band_layer_id` scheme the
    // first attempt used, and is NOT controlled by paint call order.
    //
    // This is a real behavioral test (not merely structural): it drives two
    // full `egui::Context::run_ui` passes with an injected
    // `Event::PointerMoved`, exactly as a real frame would, and reads
    // `Response::hovered()` — which is the same mechanism the failing
    // command-block gutter hover highlight relies on.
    #[test]
    fn same_layer_widget_is_not_hidden_by_containing_widget() {
        let big_rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(200.0, 200.0));
        let small_rect = egui::Rect::from_min_size(egui::pos2(50.0, 50.0), egui::vec2(20.0, 20.0));
        let pointer_pos = small_rect.center();

        let ctx = egui::Context::default();

        // Frame 1: register both widgets on the SAME layer as `ui`
        // (`LayerId::background()`) — mirroring the fixed `update()`, where
        // the band paints directly into `ui` rather than a dedicated
        // `band_layer_id`. `big` mimics `CentralPanel`'s content-area
        // widget rect (which fully contains the band); `small` mimics a
        // band-region interactive widget (e.g. the command-block gutter).
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            let _big = ui.interact(
                big_rect,
                egui::Id::new("root_content_area"),
                egui::Sense::hover(),
            );
            let _small = ui.interact(
                small_rect,
                egui::Id::new("band_widget"),
                egui::Sense::click(),
            );
        });

        // Frame 2: pointer is over `small_rect`. Hit-testing (computed at
        // the start of this frame from frame 1's registered widget rects)
        // must find `band_widget` hovered — same-layer widgets are never
        // subject to the cross-layer "hidden" rule, regardless of paint
        // order, since `hit_test.rs` only hides a widget when
        // `current.layer_id != next.layer_id`.
        let raw_input = egui::RawInput {
            events: vec![egui::Event::PointerMoved(pointer_pos)],
            ..Default::default()
        };
        let mut small_hovered = false;
        let _ = ctx.run_ui(raw_input, |ui| {
            let _big = ui.interact(
                big_rect,
                egui::Id::new("root_content_area"),
                egui::Sense::hover(),
            );
            let small_response = ui.interact(
                small_rect,
                egui::Id::new("band_widget"),
                egui::Sense::click(),
            );
            small_hovered = small_response.hovered();
        });

        assert!(
            small_hovered,
            "a widget fully contained by another widget on the SAME layer must \
             still be hoverable — this is the invariant the terminal band relies \
             on by staying in `LayerId::background()` rather than a dedicated layer"
        );
    }

    #[test]
    fn dedicated_background_layer_hides_contained_widget_cross_layer() {
        // Reproduces the actual blocker: the FIRST 436.2a attempt routed the
        // band into `band_layer_id` (a second `Order::Background` layer,
        // keyed by window id, exactly as constructed below) instead of
        // `LayerId::background()`. This deterministically demonstrates that
        // scheme hides a band widget behind the root content-area widget,
        // for this pinned egui/ahash version (the `IdMap` hash tie-break
        // ordering between two `Order::Background` layers is fixed for a
        // given ahash seed/version but is an internal implementation detail
        // — NOT something application code controls or should rely on,
        // which is precisely why the band must not use a second layer at
        // all, in either tie-break order).
        let big_rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(200.0, 200.0));
        let small_rect = egui::Rect::from_min_size(egui::pos2(50.0, 50.0), egui::vec2(20.0, 20.0));
        let pointer_pos = small_rect.center();
        let band_layer_id = egui::LayerId::new(
            egui::Order::Background,
            egui::Id::new("freminal_terminal_band").with(egui::Id::new("probe_window")),
        );

        let ctx = egui::Context::default();
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            let _big = ui.interact(
                big_rect,
                egui::Id::new("root_content_area"),
                egui::Sense::hover(),
            );
            let band_ui = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(big_rect)
                    .layer_id(band_layer_id),
            );
            let _small = band_ui.interact(
                small_rect,
                egui::Id::new("band_widget"),
                egui::Sense::click(),
            );
        });

        let raw_input = egui::RawInput {
            events: vec![egui::Event::PointerMoved(pointer_pos)],
            ..Default::default()
        };
        let mut small_hovered = false;
        let _ = ctx.run_ui(raw_input, |ui| {
            let _big = ui.interact(
                big_rect,
                egui::Id::new("root_content_area"),
                egui::Sense::hover(),
            );
            let band_ui = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(big_rect)
                    .layer_id(band_layer_id),
            );
            let small_response = band_ui.interact(
                small_rect,
                egui::Id::new("band_widget"),
                egui::Sense::click(),
            );
            small_hovered = small_response.hovered();
        });

        assert!(
            !small_hovered,
            "expected the dedicated-layer scheme to reproduce the cross-layer \
             hidden-widget blocker (this pins down WHY that approach was reverted)"
        );
    }
}
