// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use freminal_common::config::Config;
use freminal_common::send_or_log;
use freminal_terminal_emulator::io::InputEvent;
use tracing::error;

use super::FreminalGui;
use super::settings::SettingsAction;

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
        // Apply theme change to all windows.
        for win in self.windows.values_mut() {
            if new_cfg.theme.active_slug(win.os_dark_mode)
                != self.config.theme.active_slug(win.os_dark_mode)
                && let Some(theme) =
                    freminal_common::themes::by_slug(new_cfg.theme.active_slug(win.os_dark_mode))
            {
                for tab in win.tabs.iter() {
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
                for tab in win.tabs.iter() {
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

        self.config = new_cfg;

        // Adopt the persisted chrome style profile (Task 112.13). A previewed
        // profile may have set `gui_theme` ephemerally; on Apply we re-derive it
        // from the now-saved config so it persists. On a cancelled preview, the
        // saved config still carries the original profile, so this also reverts
        // an un-applied preview to the persisted value.
        self.gui_theme = self.config.chrome.profile.defaults();

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
            for tab in win.tabs.iter() {
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
            for tab in win.tabs.iter() {
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
        for &wid in self.windows.keys() {
            handle.request_repaint(wid);
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
        let new_cfg = match freminal_common::config::load_config(Some(&path)) {
            Ok(cfg) => cfg,
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
        self.push_info_toast("Config reloaded", Some(format!("From {}", path.display())));
    }

    /// Handle a `SettingsAction` from the standalone settings window.
    ///
    /// Unlike the inline modal path (which operates on a single `win`), this
    /// applies changes across ALL terminal windows in `self.windows`.
    // Long match arm with one branch per SettingsAction variant; each
    // branch carries the broadcast logic for that action class.  Splitting
    // would scatter related per-variant handlers across opaque helpers.
    #[allow(clippy::too_many_lines)]
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
            SettingsAction::PreviewOpacity(opacity) | SettingsAction::RevertOpacity(opacity) => {
                self.config.ui.background_opacity = *opacity;
                for &wid in self.windows.keys() {
                    handle.request_repaint(wid);
                }
            }
            SettingsAction::PreviewProfile(profile) => {
                // Live chrome re-style: update the runtime GuiTheme the
                // per-frame style hook (112.4) reads. Not persisted (112.13).
                // The style_cache keys on GuiTheme, so the next frame rebuilds
                // and re-applies the visuals across all chrome.
                self.gui_theme = profile.defaults();
                for &wid in self.windows.keys() {
                    handle.request_repaint(wid);
                }
            }
            SettingsAction::PreviewTheme(slug)
                if let Some(theme) = freminal_common::themes::by_slug(slug) =>
            {
                // Send theme preview to all panes in all windows.
                for win in self.windows.values() {
                    for tab in win.tabs.iter() {
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
                for &wid in self.windows.keys() {
                    handle.request_repaint(wid);
                }
            }
            SettingsAction::RevertTheme(slug, original_opacity)
                if let Some(theme) = freminal_common::themes::by_slug(slug) =>
            {
                for win in self.windows.values() {
                    for tab in win.tabs.iter() {
                        match tab.pane_tree.iter_panes() {
                            Ok(panes) => {
                                for pane in panes {
                                    send_or_log!(
                                        pane.input_tx,
                                        InputEvent::ThemeChange(theme),
                                        "Failed to send theme revert to PTY thread"
                                    );
                                }
                            }
                            Err(e) => {
                                error!(
                                    "iter_panes() failed on tab during theme \
                                     revert: {e}; skipping this tab"
                                );
                            }
                        }
                    }
                }
                self.config.ui.background_opacity = *original_opacity;
                for &wid in self.windows.keys() {
                    handle.request_repaint(wid);
                }
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
            SettingsAction::RevertTheme(_, _)
            | SettingsAction::PreviewTheme(_)
            | SettingsAction::None => {}
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
