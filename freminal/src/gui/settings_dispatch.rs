// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use freminal_common::config::Config;
use freminal_common::send_or_log;
use freminal_terminal_emulator::io::InputEvent;
use tracing::{error, warn};

use super::FreminalGui;
use super::settings::SettingsAction;
use super::visual_preview::{
    self, FONT_PREVIEW_DEBOUNCE, FontPreview, PATH_PREVIEW_DEBOUNCE, PreviewTrigger, VisualPreview,
    VisualPreviewDiff,
};

impl FreminalGui {
    /// Replace `self.config` with `new_cfg` and broadcast every derived
    /// state change — theme, font, keybindings, URL detection, background
    /// image, shader source, opacity, and theme-mode updates — to every
    /// pane in every window.
    ///
    /// Called from both the Settings "Apply" path (`SettingsAction::Applied`)
    /// and the "Reload Config" menu action (subtask 71.17).  A single
    /// definition keeps the two paths in lock-step; any new config-derived
    /// side-effect only needs to be added here.
    ///
    /// The caller is responsible for having already produced `new_cfg`
    /// (either from the settings draft or by re-reading `config.toml`).
    #[allow(clippy::too_many_lines)] // Broadcasts 7 distinct config dimensions.
    pub(super) fn apply_new_config(
        &mut self,
        new_cfg: Config,
        handle: &freminal_windowing::WindowHandle<'_>,
    ) {
        // The theme is being committed: drop the live preview override so
        // chrome styling returns to the snapshot's theme (which now carries the
        // committed theme via the broadcast below). Keeping the override pinned
        // would break per-window Auto-mode (OS dark/light) theming, which the
        // global override cannot represent.
        self.preview_theme = None;

        // Apply theme change to all windows.
        for win in self.windows.values_mut() {
            if new_cfg.theme.active_slug(win.os_dark_mode)
                != self.config.theme.active_slug(win.os_dark_mode)
                && let Some(theme) =
                    freminal_common::themes::by_slug(new_cfg.theme.active_slug(win.os_dark_mode))
            {
                for tab in &win.tabs {
                    match tab.pane_tree.iter_panes() {
                        Ok(panes) => {
                            for pane in panes {
                                send_or_log!(
                                    pane.input_tx,
                                    InputEvent::ThemeChange(theme),
                                    "Failed to send ThemeChange to PTY thread"
                                );
                            }
                        }
                        Err(e) => {
                            error!(
                                "iter_panes() failed on tab during theme apply: {e}; \
                                 skipping theme broadcast for this tab"
                            );
                        }
                    }
                }
                for tab in win.tabs.iter_mut() {
                    match tab.pane_tree.iter_panes_mut() {
                        Ok(panes) => {
                            for pane in panes {
                                pane.render_cache.invalidate_theme_cache();
                            }
                        }
                        Err(e) => {
                            error!(
                                "iter_panes_mut() failed on tab during theme \
                                 cache invalidation: {e}; skipping this tab"
                            );
                        }
                    }
                }
            }
        }

        // Apply font changes to all windows.
        for win in self.windows.values_mut() {
            let font_changed = win
                .terminal_widget
                .apply_config_changes_no_ctx(&self.config, &new_cfg);
            if font_changed {
                win.invalidate_all_pane_atlases();
            }
        }

        self.binding_map = new_cfg.build_binding_map().unwrap_or_else(|e| {
            error!("Failed to rebuild binding map after config apply: {e}. Using defaults.");
            freminal_common::keybindings::BindingMap::default()
        });

        // Broadcast auto URL detection toggle to all panes when changed.
        if new_cfg.ui.auto_detect_urls != self.config.ui.auto_detect_urls {
            let enabled = new_cfg.ui.auto_detect_urls;
            for win in self.windows.values() {
                for tab in &win.tabs {
                    match tab.pane_tree.iter_panes() {
                        Ok(panes) => {
                            for pane in panes {
                                send_or_log!(
                                    pane.input_tx,
                                    InputEvent::AutoDetectUrls(enabled),
                                    "Failed to send AutoDetectUrls to PTY thread"
                                );
                            }
                        }
                        Err(e) => {
                            error!(
                                "iter_panes() failed on tab during auto URL \
                                 apply: {e}; skipping this tab"
                            );
                        }
                    }
                }
            }
        }

        // Broadcast cursor shape/blink changes to all panes (issue #406).
        // Like the initial pane-spawn seed, this only supplies the value in
        // effect until a running program's own DECSCUSR / `XTCBlink`
        // request overrides it, exactly as on a real terminal.
        if new_cfg.cursor.shape != self.config.cursor.shape
            || new_cfg.cursor.blink != self.config.cursor.blink
        {
            let style = freminal_common::cursor::CursorVisualStyle::from_config(
                &new_cfg.cursor.shape,
                new_cfg.cursor.blink,
            );
            for win in self.windows.values() {
                for tab in &win.tabs {
                    match tab.pane_tree.iter_panes() {
                        Ok(panes) => {
                            for pane in panes {
                                send_or_log!(
                                    pane.input_tx,
                                    InputEvent::CursorConfigChange(style.clone()),
                                    "Failed to send CursorConfigChange to PTY thread"
                                );
                            }
                        }
                        Err(e) => {
                            error!(
                                "iter_panes() failed on tab during cursor config \
                                 apply: {e}; skipping this tab"
                            );
                        }
                    }
                }
            }
        }

        self.config = new_cfg;

        // Adopt the persisted chrome style profile (Task 112.13). A previewed
        // profile may have set `gui_theme` ephemerally; on Apply we re-derive it
        // from the now-saved config so it persists. On a cancelled preview, the
        // saved config still carries the original profile, so this also reverts
        // an un-applied preview to the persisted value.
        self.gui_theme = self.config.chrome.profile.defaults();

        // Keep the previewer's "currently applied" baseline in sync with the
        // just-committed config (issue #452 phase A). Every draft edit before
        // Apply already flows through `apply_visual_preview` via
        // `SettingsAction::Preview`, so this is normally a no-op; it matters
        // when a config changes without going through that flow at all --
        // e.g. "Reload Config", which also calls this function.
        self.applied_preview =
            VisualPreview::from_config(&self.config, self.settings_modal.os_dark_mode());

        // Discard any pending font-preview debounce stash and re-baseline it
        // to the just-committed font (issue #452 phase C). `font_manager`
        // already reflects `new_cfg.font` for real via
        // `apply_config_changes_no_ctx` above; without this, a stash left
        // over from an in-progress edit could settle later (via the
        // scheduled `request_repaint_after` wake) and silently overwrite the
        // just-committed font with a stale, uncommitted value.
        self.font_preview_debounce
            .apply_immediately(self.applied_preview.font.clone());

        // Same re-baselining for the background-image-path and shader-path
        // debounce holders (issue #452 phase D), and reset the shader error
        // route back to the default (toast): whatever compile result the
        // committed shader below produces is a real, committed failure, not
        // a live-preview one, regardless of what was being previewed a
        // moment ago.
        self.background_image_preview_debounce
            .apply_immediately(self.applied_preview.background_image_path.clone());
        self.shader_preview_debounce
            .apply_immediately(self.applied_preview.shader_path.clone());
        self.shader_error_route = visual_preview::ShaderErrorRoute::default();

        // Rebuild the paste-guard pattern cache from the new config and report
        // any patterns that fail to compile (skipped at match time).
        let invalid = self.paste_guard.rebuild(&self.config.paste_guard);
        for (pattern, err) in invalid {
            error!("Paste guard: ignoring invalid pattern `{pattern}`: {err}");
            self.push_error_toast(
                "Invalid paste-guard pattern",
                Some(format!("`{pattern}` — {err}")),
            );
        }

        // Apply background image to all panes in all windows.
        let new_bg_path = self.config.ui.background_image.clone();
        for win in self.windows.values() {
            for tab in &win.tabs {
                match tab.pane_tree.iter_panes() {
                    Ok(panes) => {
                        for pane in panes {
                            if let Ok(mut rs) = pane.render_state.lock() {
                                rs.set_pending_bg_image(new_bg_path.clone());
                            }
                        }
                    }
                    Err(e) => {
                        error!(
                            "iter_panes() failed on tab during background \
                             image apply: {e}; skipping this tab"
                        );
                    }
                }
            }
        }

        // Apply shader changes to all windows.
        let has_shader_path = self.config.shader.path.is_some();
        if !has_shader_path {
            for win in self.windows.values() {
                if let Ok(mut wpr) = win.window_post.lock() {
                    wpr.pending_shader = Some(None);
                }
            }
        } else if let Some(ref p) = self.config.shader.path {
            match std::fs::read_to_string(p) {
                Ok(src) => {
                    for win in self.windows.values() {
                        if let Ok(mut wpr) = win.window_post.lock() {
                            wpr.pending_shader = Some(Some(src.clone()));
                        }
                    }
                }
                Err(e) => {
                    error!(
                        "Failed to read shader file '{}': {e}; keeping current shader",
                        p.display()
                    );
                }
            }
        }

        // Notify all panes of theme mode update.
        for win in self.windows.values() {
            for tab in &win.tabs {
                match tab.pane_tree.iter_panes() {
                    Ok(panes) => {
                        for pane in panes {
                            send_or_log!(
                                pane.input_tx,
                                InputEvent::ThemeModeUpdate(
                                    self.config.theme.mode,
                                    win.os_dark_mode,
                                ),
                                "Failed to send ThemeModeUpdate after config apply"
                            );
                        }
                    }
                    Err(e) => {
                        error!(
                            "iter_panes() failed on tab during theme-mode \
                             broadcast: {e}; skipping this tab"
                        );
                    }
                }
            }
        }

        // Request repaint on all terminal windows so changes are visible.
        // The theme reaches the terminal chrome via the PTY round-trip
        // (snap.theme), so an immediate repaint may re-read a stale snapshot;
        // a short follow-up repaint guarantees the restyle lands even on an
        // idle terminal.
        for &wid in self.windows.keys() {
            handle.request_repaint(wid);
            handle.request_repaint_after(wid, std::time::Duration::from_millis(50));
        }
    }

    /// Re-read `config.toml` from disk and apply every change live.
    ///
    /// Invoked by the "Reload Config" menu entry (subtask 71.17).  If the
    /// current session has no configured path (i.e. freminal was launched
    /// before any config existed and no `--config` was supplied) this is a
    /// no-op with a user-visible toast.  Parse errors are logged and a
    /// toast is shown; `self.config` is left unchanged.
    pub(super) fn reload_config_from_disk(
        &mut self,
        handle: &freminal_windowing::WindowHandle<'_>,
    ) {
        let Some(path) = self.config_path.clone() else {
            self.push_error_toast(
                "Reload Config",
                Some("No config file is associated with this session.".to_string()),
            );
            return;
        };
        let (new_cfg, config_warnings) =
            match freminal_common::config::load_config_with_warnings(Some(&path)) {
                Ok(loaded) => loaded,
                Err(e) => {
                    error!("Reload Config: failed to load '{}': {e}", path.display());
                    self.push_error_toast("Reload Config failed", Some(e.to_string()));
                    return;
                }
            };
        self.apply_new_config(new_cfg, handle);
        // Re-sync the Settings modal's draft so opening Settings after a
        // reload shows the now-live values, not a stale draft.
        self.settings_modal.sync_from_config(&self.config);
        // Surface any config deprecation warnings as a toast so an
        // interactive reload makes them visible (they are also logged).
        for warning in &config_warnings {
            warn!("{warning}");
            self.push_info_toast("Config deprecation", Some(warning.clone()));
        }
        self.route_freminal_toast(
            freminal_common::config::FreminalToastCategory::ConfigReload,
            crate::gui::toast::ToastKind::Info,
            "Config reloaded",
            Some(format!("From {}", path.display())),
            crate::gui::toast::ToastPlacement::WINDOW_CENTERED,
        );
    }

    /// Diff `next` against [`FreminalGui::applied_preview`] (the previewable
    /// state most recently pushed to the running app) and perform only the
    /// work each changed field needs, then store `next` as the new applied
    /// state.
    ///
    /// This is the single applier for `SettingsAction::Preview` — both
    /// "preview a change" and "revert by re-previewing the committed config"
    /// go through here identically, since [`VisualPreview`] carries no
    /// distinction between the two. See `visual_preview`'s module doc for why
    /// that unification is the point. `trigger` (issue #452 phase C) is the
    /// one place that distinction actually matters: the debounced font
    /// fields must apply immediately and bypass the debounce entirely on a
    /// revert, whereas every other field here already applies immediately
    /// regardless of `trigger`.
    ///
    /// The per-field decision of *what* changed is the pure
    /// [`VisualPreview::diff_from`]; each `apply_preview_*` helper below only
    /// carries out the side effects its changed field(s) imply.
    fn apply_visual_preview(
        &mut self,
        next: &VisualPreview,
        trigger: PreviewTrigger,
        handle: &freminal_windowing::WindowHandle<'_>,
    ) {
        let diff = next.diff_from(&self.applied_preview);

        self.apply_preview_theme(&diff, handle);
        self.apply_preview_background_opacity(&diff, handle);
        self.apply_preview_profile(&diff, handle);
        self.apply_preview_cursor(&diff, next, handle);
        self.apply_preview_ligatures(&diff, handle);
        self.apply_preview_background_image(&diff, handle);
        self.apply_preview_hide_menu_bar(&diff, handle);
        self.apply_preview_gutter(&diff, handle);
        self.apply_preview_tab_bar_position(&diff, handle);
        self.apply_preview_progress_enabled(&diff, handle);

        match trigger {
            PreviewTrigger::Edit => {
                self.apply_preview_font_debounced(&diff, next, handle);
                self.apply_preview_background_image_path_debounced(&diff, next, handle);
                self.apply_preview_shader_path_debounced(&diff, next, handle);
            }
            PreviewTrigger::Revert => {
                self.apply_preview_font_immediate(next, handle);
                self.apply_preview_background_image_path_immediate(next, handle);
                self.apply_preview_shader_path_immediate(next, handle);
            }
        }

        self.applied_preview = next.clone();
    }

    /// End a Settings session: put every live preview back to the committed
    /// config and drop the session-scoped preview state.
    ///
    /// Every close path must run this, whichever way the session ended:
    /// Apply/OK, Cancel, the Settings window's own close button, closing the
    /// terminal window that owns Settings, and that window's Discard
    /// prompt. Several of those paths never render another Settings frame,
    /// so the revert `SettingsAction::Preview` that `show_standalone` emits
    /// on Cancel is not enough on its own. Without this, a Discard left the
    /// discarded previews written into `self.config` (opacity, gutter, tab
    /// bar position and so on), where the next Settings session would load
    /// them into its draft and a later Apply would save them.
    ///
    /// Reverting unconditionally is safe and cheap. After Apply the
    /// committed preview is what was just applied, and after Cancel the
    /// revert has already run; either way `apply_visual_preview` finds
    /// nothing left to change, and the debounced fields compare against
    /// their own baselines (see `debounced_revert_needs_push`).
    pub(super) fn end_settings_session(&mut self, handle: &freminal_windowing::WindowHandle<'_>) {
        let committed = self.settings_modal.committed_preview().clone();
        self.apply_visual_preview(&committed, PreviewTrigger::Revert, handle);
        // Drop the live chrome preview override. After Apply the committed
        // theme flows via the snapshot; after a revert the broadcast above
        // restored it. Clearing also re-enables per-window Auto-mode
        // theming, which a pinned global override cannot represent.
        self.preview_theme = None;
        // Reset the shader-error route unconditionally. The shader revert
        // can legitimately skip the push that would reset it (its early
        // return when the debounce baseline already matches the committed
        // path), which used to leave `SettingsStatus` stuck with the window
        // gone, silently dropping a later hot-reload compile error (issue
        // #452 post-review Blocker 2). See
        // `shader_error_route_on_settings_close`.
        self.shader_error_route = visual_preview::shader_error_route_on_settings_close();
    }

    /// Request an immediate repaint plus a short follow-up on every window.
    ///
    /// Shared by every preview change whose effect only becomes visible
    /// after a PTY round trip (`InputEvent::ThemeChange`,
    /// `InputEvent::CursorConfigChange`): the PTY thread schedules its own
    /// repaint after rebuilding the snapshot, but a quiet terminal (no
    /// cursor blink, no output) would otherwise only refresh on the next
    /// external event (mouseover). The 50ms follow-up guarantees the GUI
    /// re-reads the updated snapshot even when idle.
    fn schedule_pty_roundtrip_repaint(&self, handle: &freminal_windowing::WindowHandle<'_>) {
        for &wid in self.windows.keys() {
            handle.request_repaint(wid);
            handle.request_repaint_after(wid, std::time::Duration::from_millis(50));
        }
    }

    /// Request an immediate repaint on every window.
    ///
    /// Used for preview changes applied entirely GUI-side (no PTY round
    /// trip), where a single repaint is enough to make the change visible.
    fn request_repaint_all_windows(&self, handle: &freminal_windowing::WindowHandle<'_>) {
        for &wid in self.windows.keys() {
            handle.request_repaint(wid);
        }
    }

    /// Preview a theme change: drive chrome styling immediately via
    /// `preview_theme` and broadcast `InputEvent::ThemeChange` so the
    /// terminal buffer re-themes too.
    fn apply_preview_theme(
        &mut self,
        diff: &VisualPreviewDiff,
        handle: &freminal_windowing::WindowHandle<'_>,
    ) {
        let Some(slug) = &diff.theme_slug else {
            return;
        };
        let Some(theme) = freminal_common::themes::by_slug(slug) else {
            error!("Settings preview: unknown theme slug '{slug}'; leaving chrome unchanged");
            return;
        };
        // Drive chrome styling from this preview theme immediately and
        // deterministically (the per-frame style hook reads
        // `self.preview_theme`), independent of the PTY round-trip.
        self.preview_theme = Some(theme);
        // Send the theme to all panes in all windows so the terminal
        // *buffer* re-themes too (its renderer reads the snapshot's theme,
        // set on the PTY side).
        for win in self.windows.values() {
            for tab in &win.tabs {
                match tab.pane_tree.iter_panes() {
                    Ok(panes) => {
                        for pane in panes {
                            send_or_log!(
                                pane.input_tx,
                                InputEvent::ThemeChange(theme),
                                "Failed to send theme preview to PTY thread"
                            );
                        }
                    }
                    Err(e) => {
                        error!(
                            "iter_panes() failed on tab during theme \
                             preview: {e}; skipping this tab"
                        );
                    }
                }
            }
        }
        self.schedule_pty_roundtrip_repaint(handle);
    }

    /// Preview a window background opacity change (GUI-side only).
    fn apply_preview_background_opacity(
        &mut self,
        diff: &VisualPreviewDiff,
        handle: &freminal_windowing::WindowHandle<'_>,
    ) {
        let Some(opacity) = diff.background_opacity else {
            return;
        };
        self.config.ui.background_opacity = opacity;
        self.request_repaint_all_windows(handle);
    }

    /// Preview a chrome style profile change (GUI-side only).
    fn apply_preview_profile(
        &mut self,
        diff: &VisualPreviewDiff,
        handle: &freminal_windowing::WindowHandle<'_>,
    ) {
        let Some(profile) = diff.profile else {
            return;
        };
        // Live chrome re-style: update the runtime GuiTheme the per-frame
        // style hook (112.4) reads. Not persisted here (Apply does that via
        // `apply_new_config`). The style_cache keys on GuiTheme, so the next
        // frame rebuilds and re-applies the visuals across all chrome.
        self.gui_theme = profile.defaults();
        self.request_repaint_all_windows(handle);
    }

    /// Preview cursor shape/blink (broadcast to every pane, same as Apply)
    /// and cursor trail/duration (GUI-side, per-window `terminal_widget`
    /// state) — issue #452 phase B.
    fn apply_preview_cursor(
        &mut self,
        diff: &VisualPreviewDiff,
        next: &VisualPreview,
        handle: &freminal_windowing::WindowHandle<'_>,
    ) {
        if diff.cursor_shape.is_some() || diff.cursor_blink.is_some() {
            // Like the initial pane-spawn seed and `apply_new_config`, this
            // only supplies the value in effect until a running program's
            // own DECSCUSR / XTCBlink request overrides it.
            let style = freminal_common::cursor::CursorVisualStyle::from_config(
                &next.cursor.shape,
                next.cursor.blink,
            );
            for win in self.windows.values() {
                for tab in &win.tabs {
                    match tab.pane_tree.iter_panes() {
                        Ok(panes) => {
                            for pane in panes {
                                send_or_log!(
                                    pane.input_tx,
                                    InputEvent::CursorConfigChange(style.clone()),
                                    "Failed to send cursor preview to PTY thread"
                                );
                            }
                        }
                        Err(e) => {
                            error!(
                                "iter_panes() failed on tab during cursor \
                                 preview: {e}; skipping this tab"
                            );
                        }
                    }
                }
            }
            self.schedule_pty_roundtrip_repaint(handle);
        }

        if diff.cursor_trail.is_some() || diff.cursor_trail_duration_ms.is_some() {
            for win in self.windows.values_mut() {
                win.terminal_widget
                    .set_cursor_trail_preview(next.cursor.trail, next.cursor.trail_duration_ms);
            }
            self.request_repaint_all_windows(handle);
        }
    }

    /// Preview the ligature toggle (GUI-side cached flag on each window's
    /// `terminal_widget`, no `FontManager::rebuild`) — issue #452 phase B.
    ///
    /// A ligature change alters shaping output, so each window whose toggle
    /// actually changed needs its pane atlases invalidated to re-shape
    /// already-rendered lines.
    fn apply_preview_ligatures(
        &mut self,
        diff: &VisualPreviewDiff,
        handle: &freminal_windowing::WindowHandle<'_>,
    ) {
        let Some(enabled) = diff.ligatures else {
            return;
        };
        for win in self.windows.values_mut() {
            if win.terminal_widget.set_ligatures_preview(enabled) {
                win.invalidate_all_pane_atlases();
            }
        }
        self.request_repaint_all_windows(handle);
    }

    /// Preview background image opacity/mode (GUI-side only, read directly
    /// from `self.config` at render time) — issue #452 phase B.
    fn apply_preview_background_image(
        &mut self,
        diff: &VisualPreviewDiff,
        handle: &freminal_windowing::WindowHandle<'_>,
    ) {
        let changed =
            diff.background_image_opacity.is_some() || diff.background_image_mode.is_some();
        if let Some(opacity) = diff.background_image_opacity {
            self.config.ui.background_image_opacity = opacity;
        }
        if let Some(mode) = diff.background_image_mode {
            self.config.ui.background_image_mode = mode;
        }
        if changed {
            self.request_repaint_all_windows(handle);
        }
    }

    /// Preview the "hide menu bar" toggle (GUI-side chrome layout only,
    /// read directly from `self.config` at render time) — issue #452 phase B.
    fn apply_preview_hide_menu_bar(
        &mut self,
        diff: &VisualPreviewDiff,
        handle: &freminal_windowing::WindowHandle<'_>,
    ) {
        let Some(hidden) = diff.hide_menu_bar else {
            return;
        };
        self.config.ui.hide_menu_bar = hidden;
        self.request_repaint_all_windows(handle);
    }

    /// Preview the command-block gutter position (GUI-side write; issue
    /// #452 phase D).
    ///
    /// Unlike every other GUI-side preview field, this changes the
    /// terminal's usable width: `GutterPosition::total_inset_px()` feeds
    /// `pane_width_chars` in the per-pane resize computation in
    /// `app_impl.rs`. No resize is hand-rolled here -- the existing
    /// reactive resize detection there already re-derives `pane_width_chars`
    /// from `self.config.command_blocks.gutter` every frame and sends
    /// `InputEvent::Resize` on a mismatch, exactly as a font-size change
    /// does for cell-size changes (see `apply_font_zoom`'s doc). A repaint
    /// is all that is needed to make that detection run, including on
    /// revert -- `PreviewTrigger::Revert` restores the committed gutter
    /// through this same helper, so the PTY resize back happens
    /// automatically too.
    fn apply_preview_gutter(
        &mut self,
        diff: &VisualPreviewDiff,
        handle: &freminal_windowing::WindowHandle<'_>,
    ) {
        let Some(gutter) = diff.gutter else {
            return;
        };
        self.config.command_blocks.gutter = gutter;
        self.request_repaint_all_windows(handle);
    }

    /// Preview the OSC 9;4 progress-bar display toggle (GUI-side only, a
    /// per-window widget toggle `show()` reads each frame) -- issue #507.
    fn apply_preview_progress_enabled(
        &mut self,
        diff: &VisualPreviewDiff,
        handle: &freminal_windowing::WindowHandle<'_>,
    ) {
        let Some(enabled) = diff.progress_enabled else {
            return;
        };
        for win in self.windows.values_mut() {
            win.terminal_widget.set_progress_enabled_preview(enabled);
        }
        self.request_repaint_all_windows(handle);
    }

    /// Preview the tab bar position (GUI-side chrome layout only, read
    /// directly from `self.config` at render time) -- issue #452 phase D.
    fn apply_preview_tab_bar_position(
        &mut self,
        diff: &VisualPreviewDiff,
        handle: &freminal_windowing::WindowHandle<'_>,
    ) {
        let Some(position) = diff.tab_bar_position else {
            return;
        };
        self.config.tabs.position = position;
        self.request_repaint_all_windows(handle);
    }

    /// Preview an in-progress edit to the font family/size/line-height
    /// triplet (issue #452 phase C) through the time-based debounce.
    ///
    /// Unlike every other `apply_preview_*` helper, this does not act on
    /// `diff` directly beyond checking whether *anything* font-related
    /// changed -- the whole point of debouncing is to *not* redo a font
    /// rebuild on every intermediate slider/keystroke value. The changed
    /// snapshot is instead fed to `self.font_preview_debounce`, which only
    /// returns a value once it has settled (unchanged for
    /// [`FONT_PREVIEW_DEBOUNCE`]). See [`super::visual_preview::DebouncedPreview`].
    fn apply_preview_font_debounced(
        &mut self,
        diff: &VisualPreviewDiff,
        next: &VisualPreview,
        handle: &freminal_windowing::WindowHandle<'_>,
    ) {
        let font_changed = diff.font_family.is_some()
            || diff.font_size.is_some()
            || diff.font_line_height.is_some();
        if !font_changed {
            return;
        }

        let now = std::time::Instant::now();
        let previous_applied = self.font_preview_debounce.applied().clone();
        if let Some(settled) =
            self.font_preview_debounce
                .note(next.font.clone(), now, FONT_PREVIEW_DEBOUNCE)
        {
            self.apply_settled_font_preview(&previous_applied, &settled, handle);
        } else if let Some(settings_window_id) = self.settings_window_id {
            // A stable draft produces no further `SettingsAction::Preview`
            // on its own, so the debounce would otherwise never get a
            // chance to actually apply once the user stops editing.
            // `tick_font_preview_debounce` (invoked from `app_impl.rs` on
            // every settings-window frame) checks on this wake whether the
            // stash has settled.
            handle.request_repaint_after(settings_window_id, FONT_PREVIEW_DEBOUNCE);
        }
    }

    /// Check whether any stashed debounced-preview value (font, background
    /// image path, or shader path) has settled, without a new
    /// `SettingsAction::Preview` having arrived this frame (issue #452
    /// phases C and D).
    ///
    /// Called on every settings-window frame regardless of what
    /// `SettingsAction` (if any) that frame produced -- see the doc on
    /// [`Self::apply_preview_font_debounced`] for why this is the only path
    /// that ever applies a settled value once the user stops editing. Named
    /// generically (phase C called this `tick_font_preview_debounce`)
    /// because phase D adds two more debounce holders that need the exact
    /// same per-frame poll.
    pub(super) fn tick_visual_preview_debounces(
        &mut self,
        handle: &freminal_windowing::WindowHandle<'_>,
    ) {
        let now = std::time::Instant::now();

        let previous_applied = self.font_preview_debounce.applied().clone();
        if let Some(settled) = self.font_preview_debounce.poll(now, FONT_PREVIEW_DEBOUNCE) {
            self.apply_settled_font_preview(&previous_applied, &settled, handle);
        }

        if let Some(settled) = self
            .background_image_preview_debounce
            .poll(now, PATH_PREVIEW_DEBOUNCE)
        {
            self.push_background_image_to_all_panes(settled.as_ref(), handle);
        }

        if let Some(settled) = self
            .shader_preview_debounce
            .poll(now, PATH_PREVIEW_DEBOUNCE)
        {
            self.push_shader_to_all_windows(settled, PreviewTrigger::Edit, handle);
        }
    }

    /// Revert the font family/size/line-height triplet immediately, bypassing
    /// the debounce entirely and discarding any pending stash (issue #452
    /// phase C).
    ///
    /// Used only for [`PreviewTrigger::Revert`]: closing Settings without
    /// applying must not leave a previewed font lingering for the debounce
    /// window, and a stash left over from a cancelled edit must never
    /// surface later via the scheduled wake.
    fn apply_preview_font_immediate(
        &mut self,
        next: &VisualPreview,
        handle: &freminal_windowing::WindowHandle<'_>,
    ) {
        // Restore the committed font in `self.config` before the early
        // return below: the per-frame zoom sync reads `config.font.size`, so
        // the config must hold the committed value even when nothing needs
        // pushing to the font manager.
        self.write_font_preview_into_config(&next.font);
        let previous_applied = self.font_preview_debounce.applied().clone();
        let had_pending = self.font_preview_debounce.is_pending();
        self.font_preview_debounce
            .apply_immediately(next.font.clone());

        if !visual_preview::debounced_revert_needs_push(&previous_applied, &next.font, had_pending)
        {
            // The font was never previewed away from what's already live;
            // nothing to do.
            return;
        }

        // Always route through the full rebuild path on revert: this is a
        // rare, one-shot event (closing Settings without applying), so
        // correctness -- always landing on the right family/size/line-height
        // regardless of which subset drifted during the cancelled preview --
        // matters more than reusing the cheaper `set_font_size` path here.
        for win in self.windows.values_mut() {
            let changed = win.terminal_widget.apply_font_preview_rebuild(
                next.font.family.as_deref(),
                next.font.size,
                next.font.line_height,
            );
            if changed {
                win.invalidate_all_pane_atlases();
            }
        }
        self.request_repaint_all_windows(handle);
    }

    /// Make a font preview the value in `self.config.font`, and refresh
    /// every window's egui chrome fonts when the family or size changed.
    ///
    /// The per-frame font zoom sync in `app_impl.rs` re-applies
    /// `config.font.size` to each window's font manager, so a preview held
    /// only in the font manager would be undone on the next terminal frame.
    /// See [`FontPreview::write_into`] for why the chrome refresh is needed.
    fn write_font_preview_into_config(&mut self, font: &FontPreview) {
        if font.write_into(&mut self.config.font) {
            for win in self.windows.values_mut() {
                win.terminal_widget.mark_egui_fonts_dirty();
            }
        }
    }

    /// Apply a settled font-preview value to every window's `FontManager`,
    /// choosing the cheaper `set_font_size` path when only the size changed
    /// and reserving the full `rebuild` for a family or line-height change
    /// (issue #452 phase C).
    fn apply_settled_font_preview(
        &mut self,
        previous: &FontPreview,
        settled: &FontPreview,
        handle: &freminal_windowing::WindowHandle<'_>,
    ) {
        self.write_font_preview_into_config(settled);
        let needs_rebuild = settled.family != previous.family
            || (settled.line_height - previous.line_height).abs() > f32::EPSILON;
        for win in self.windows.values_mut() {
            let changed = if needs_rebuild {
                win.terminal_widget.apply_font_preview_rebuild(
                    settled.family.as_deref(),
                    settled.size,
                    settled.line_height,
                )
            } else {
                win.terminal_widget.apply_font_zoom(settled.size)
            };
            if changed {
                win.invalidate_all_pane_atlases();
            }
        }
        self.request_repaint_all_windows(handle);
    }

    /// Preview an in-progress edit to the background image path (issue #452
    /// phase D) through the same generic time-based debounce as the font
    /// fields.
    ///
    /// The path is a text field with real filesystem I/O and a GPU texture
    /// decode/upload behind it (`RenderState::set_pending_bg_image`,
    /// consumed inside a `PaintCallback`), so redoing that on every
    /// keystroke would be wasteful and can spam decode-failure logs for a
    /// path that is still being typed. Reuses
    /// [`super::visual_preview::DebouncedPreview`] unchanged rather than a
    /// second bespoke debounce -- see [`Self::background_image_preview_debounce`]'s
    /// doc.
    fn apply_preview_background_image_path_debounced(
        &mut self,
        diff: &VisualPreviewDiff,
        next: &VisualPreview,
        handle: &freminal_windowing::WindowHandle<'_>,
    ) {
        if diff.background_image_path.is_none() {
            return;
        }

        let now = std::time::Instant::now();
        if let Some(settled) = self.background_image_preview_debounce.note(
            next.background_image_path.clone(),
            now,
            PATH_PREVIEW_DEBOUNCE,
        ) {
            self.push_background_image_to_all_panes(settled.as_ref(), handle);
        } else if let Some(settings_window_id) = self.settings_window_id {
            // Mirrors `apply_preview_font_debounced`'s wake: a stable draft
            // produces no further `SettingsAction::Preview` on its own, so
            // `tick_visual_preview_debounces` needs this scheduled wake to
            // ever apply a settled value once the user stops editing.
            handle.request_repaint_after(settings_window_id, PATH_PREVIEW_DEBOUNCE);
        }
    }

    /// Revert the background image path immediately, bypassing the debounce
    /// entirely and discarding any pending stash (issue #452 phase D).
    ///
    /// Used only for [`PreviewTrigger::Revert`] -- same shape as
    /// [`Self::apply_preview_font_immediate`].
    fn apply_preview_background_image_path_immediate(
        &mut self,
        next: &VisualPreview,
        handle: &freminal_windowing::WindowHandle<'_>,
    ) {
        let previous_applied = self.background_image_preview_debounce.applied().clone();
        let had_pending = self.background_image_preview_debounce.is_pending();
        self.background_image_preview_debounce
            .apply_immediately(next.background_image_path.clone());

        if !visual_preview::debounced_revert_needs_push(
            &previous_applied,
            &next.background_image_path,
            had_pending,
        ) {
            // The path was never previewed away from what's already live;
            // nothing to do.
            return;
        }
        self.push_background_image_to_all_panes(next.background_image_path.as_ref(), handle);
    }

    /// Push a settled (or reverted) background image path to every pane in
    /// every window (issue #452 phase D).
    ///
    /// Mirrors the per-pane loop `apply_new_config` already runs on Apply --
    /// same `RenderState::set_pending_bg_image` call, same per-tab error
    /// handling -- so a settled preview and a committed change go through
    /// identical GPU-side plumbing. `self.config.ui.background_image` is
    /// kept in sync too, so a pane spawned while the preview is still in
    /// effect (a new tab, a new split) seeds from the previewed value rather
    /// than a stale committed one.
    fn push_background_image_to_all_panes(
        &mut self,
        path: Option<&std::path::PathBuf>,
        handle: &freminal_windowing::WindowHandle<'_>,
    ) {
        self.config.ui.background_image = path.cloned();
        for win in self.windows.values() {
            for tab in &win.tabs {
                match tab.pane_tree.iter_panes() {
                    Ok(panes) => {
                        for pane in panes {
                            if let Ok(mut rs) = pane.render_state.lock() {
                                rs.set_pending_bg_image(path.cloned());
                            }
                        }
                    }
                    Err(e) => {
                        error!(
                            "iter_panes() failed on tab during background \
                             image preview: {e}; skipping this tab"
                        );
                    }
                }
            }
        }
        self.request_repaint_all_windows(handle);
    }

    /// Preview an in-progress edit to the shader path (issue #452 phase D)
    /// through the same generic time-based debounce as the background image
    /// path.
    ///
    /// A shader path is backed by a GLSL compile that can additionally
    /// fail -- see [`visual_preview::ShaderErrorRoute`] and
    /// [`Self::push_shader_to_all_windows`] for how a compile failure
    /// discovered later (inside the `PaintCallback`, on the render thread)
    /// is routed away from the terminal windows' toast stack while this
    /// preview is still a live edit.
    fn apply_preview_shader_path_debounced(
        &mut self,
        diff: &VisualPreviewDiff,
        next: &VisualPreview,
        handle: &freminal_windowing::WindowHandle<'_>,
    ) {
        if diff.shader_path.is_none() {
            return;
        }

        let now = std::time::Instant::now();
        if let Some(settled) =
            self.shader_preview_debounce
                .note(next.shader_path.clone(), now, PATH_PREVIEW_DEBOUNCE)
        {
            self.push_shader_to_all_windows(settled, PreviewTrigger::Edit, handle);
        } else if let Some(settings_window_id) = self.settings_window_id {
            handle.request_repaint_after(settings_window_id, PATH_PREVIEW_DEBOUNCE);
        }
    }

    /// Revert the shader path immediately, bypassing the debounce entirely
    /// and discarding any pending stash (issue #452 phase D).
    ///
    /// Used only for [`PreviewTrigger::Revert`] -- same shape as
    /// [`Self::apply_preview_font_immediate`]. Routed through
    /// [`Self::push_shader_to_all_windows`] with
    /// [`PreviewTrigger::Revert`] so a (very unlikely) compile failure while
    /// reverting to the committed shader still lands on the toast: the
    /// Settings window is closing at this point, so its own status message
    /// would never be seen.
    fn apply_preview_shader_path_immediate(
        &mut self,
        next: &VisualPreview,
        handle: &freminal_windowing::WindowHandle<'_>,
    ) {
        let previous_applied = self.shader_preview_debounce.applied().clone();
        let had_pending = self.shader_preview_debounce.is_pending();
        self.shader_preview_debounce
            .apply_immediately(next.shader_path.clone());

        if !visual_preview::debounced_revert_needs_push(
            &previous_applied,
            &next.shader_path,
            had_pending,
        ) {
            // The shader path was never previewed away from what's already
            // live; nothing to do. Note this early return does NOT reset
            // `shader_error_route` -- that reset is unconditional at the
            // `app_impl.rs` settings-window close site (issue #452
            // post-review Blocker 2), independent of whether this function
            // pushes anything.
            return;
        }
        self.push_shader_to_all_windows(next.shader_path.clone(), PreviewTrigger::Revert, handle);
    }

    /// Push a settled (or reverted) shader path to every window's
    /// `WindowPostRenderer`, reading the file from disk if a path is set
    /// (issue #452 phase D).
    ///
    /// A missing or unreadable file degrades gracefully: the read failure is
    /// logged here and no `pending_shader` write happens at all, leaving
    /// whatever shader is already active (or inactive) untouched --
    /// identical to `apply_new_config`'s handling of a bad committed path.
    /// A *compile* failure (a readable file with invalid GLSL) is a
    /// different failure surfaced later, asynchronously, inside the
    /// `PaintCallback` on the render thread; `origin` decides where that
    /// later failure is routed by updating [`FreminalGui::shader_error_route`]
    /// via [`visual_preview::shader_error_route_for`] here, before the
    /// render thread ever gets a chance to observe it.
    fn push_shader_to_all_windows(
        &mut self,
        path: Option<std::path::PathBuf>,
        origin: PreviewTrigger,
        handle: &freminal_windowing::WindowHandle<'_>,
    ) {
        self.config.shader.path.clone_from(&path);
        let Some(p) = path else {
            for win in self.windows.values() {
                if let Ok(mut wpr) = win.window_post.lock() {
                    wpr.pending_shader = Some(None);
                }
            }
            self.shader_error_route = visual_preview::shader_error_route_for(origin);
            self.request_repaint_all_windows(handle);
            return;
        };
        match std::fs::read_to_string(&p) {
            Ok(src) => {
                for win in self.windows.values() {
                    if let Ok(mut wpr) = win.window_post.lock() {
                        wpr.pending_shader = Some(Some(src.clone()));
                    }
                }
                self.shader_error_route = visual_preview::shader_error_route_for(origin);
            }
            Err(e) => {
                error!(
                    "Failed to read shader file '{}' during preview: {e}; \
                     keeping current shader",
                    p.display()
                );
            }
        }
        self.request_repaint_all_windows(handle);
    }

    /// Handle a `SettingsAction` from the standalone settings window.
    ///
    /// Unlike the inline modal path (which operates on a single `win`), this
    /// applies changes across ALL terminal windows in `self.windows`.
    // One branch per SettingsAction variant; each branch carries the
    // broadcast logic for that action class.  Splitting would scatter
    // related per-variant handlers across opaque helpers.
    pub(super) fn handle_settings_action(
        &mut self,
        action: &SettingsAction,
        handle: &freminal_windowing::WindowHandle<'_>,
        _settings_window_id: freminal_windowing::WindowId,
    ) {
        match action {
            SettingsAction::Applied => {
                let new_cfg = self.settings_modal.applied_config().clone();
                self.apply_new_config(new_cfg, handle);
            }
            SettingsAction::Preview(next, trigger) => {
                self.apply_visual_preview(next, *trigger, handle);
            }
            SettingsAction::TestNotification => {
                // Route a sample notification through the draft `[notifications]`
                // config so the user sees exactly what their current (unsaved)
                // settings produce.  The Settings window is focused when the
                // button is clicked, so route as focused.
                let config = self.settings_modal.draft_notifications().clone();
                let request = crate::gui::notifications::NotificationRequest::sample();
                if let Ok(mut toasts) = self.toasts.try_borrow_mut() {
                    crate::gui::notifications::NotificationRouter::route_test(
                        &request,
                        &config,
                        true,
                        &mut toasts,
                    );
                }
            }
            SettingsAction::TestPaste => {
                // Open the confirm dialog with sample content using the draft
                // `[paste_guard]` config, so the user previews exactly what
                // their current (unsaved) settings produce. Routed to the
                // terminal window that owns the Settings window.
                const SAMPLE: &str = "echo first line\nsudo rm -rf /tmp/example\necho third line";
                let cfg = self.settings_modal.draft_paste_guard().clone();
                let guard = crate::gui::paste_guard::PasteGuard::new(&cfg);
                let analysis = guard.analyze(SAMPLE, &cfg);
                if analysis.is_safe() {
                    self.push_info_toast(
                        "Test Paste",
                        Some(
                            "With these settings the sample paste would NOT be \
                             intercepted."
                                .to_owned(),
                        ),
                    );
                } else if let Some(owner) = self.settings_owner
                    && let Some(win) = self.windows.get_mut(&owner)
                {
                    // Test Paste is a preview only; target the window's active
                    // pane so a confirm would route there like a real paste.
                    let tab = win.tabs.active_tab();
                    let target = crate::gui::paste_guard::PasteTarget {
                        tab_id: tab.id,
                        pane_id: tab.active_pane,
                    };
                    win.paste_dialog.open(SAMPLE.to_owned(), analysis, target);
                    handle.request_repaint(owner);
                } else {
                    self.push_error_toast(
                        "Test Paste",
                        Some("No terminal window available to show the dialog.".to_owned()),
                    );
                }
            }
            SettingsAction::None => {}
            SettingsAction::DeleteLayout(path) => {
                if let Err(e) = std::fs::remove_file(path) {
                    error!("Failed to delete layout file '{}': {e}", path.display());
                }
                // Refresh the layout list regardless (file may already be gone).
                self.discovered_layouts = freminal_common::config::layout_library_dir()
                    .map(|dir| freminal_common::layout::discover_layouts(&dir))
                    .unwrap_or_default();
                self.settings_modal.discovered_layouts = self.discovered_layouts.clone();
            }
        }
    }
}
