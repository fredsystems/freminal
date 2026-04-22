// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use tracing::error;

use super::FreminalGui;
use super::window;

impl FreminalGui {
    /// Return the path used for auto-save/restore of the last session.
    pub(super) fn last_session_path() -> Option<std::path::PathBuf> {
        freminal_common::config::layout_library_dir().map(|d| d.join("last_session.toml"))
    }

    /// Write the current ephemeral UI window state (currently just the
    /// Settings window geometry) to `window_state.toml`.  Failures are
    /// logged but never fatal — the worst case is that the settings window
    /// reopens at its default size and position next time.
    pub(super) fn persist_window_state(&self) {
        let Some(path) = freminal_common::window_state::window_state_path() else {
            tracing::debug!("persist_window_state: cannot determine window state path");
            return;
        };
        if let Err(e) = self.window_state.save(&path) {
            tracing::warn!(
                "persist_window_state: failed to save {}: {e}",
                path.display()
            );
        }
    }

    /// Populate `self.window_state.main_windows` from the currently-open
    /// terminal windows' tracked geometry (`last_known_size` /
    /// `last_known_position`).  The settings window is excluded.
    ///
    /// If `prioritize` is `Some(id)`, that window's geometry is placed
    /// first so it seeds the primary window on the next launch.  Used
    /// when a window is closing — the closing window is the one the user
    /// most recently interacted with, so its geometry is the best seed.
    ///
    /// Windows with no tracked geometry (e.g. freshly created, never
    /// resized on a platform where seeding is unavailable) are skipped.
    pub(super) fn snapshot_main_window_geometry(
        &mut self,
        prioritize: Option<freminal_windowing::WindowId>,
    ) {
        let settings_id = self.settings_window_id;
        let terminal_entry = |wid: freminal_windowing::WindowId, win: &window::PerWindowState| {
            if Some(wid) == settings_id {
                return None;
            }
            let size = win.last_known_size;
            let position = win.last_known_position;
            if size.is_none() && position.is_none() {
                return None;
            }
            Some(freminal_common::window_state::WindowGeometry { size, position })
        };

        let mut main_windows = Vec::with_capacity(self.windows.len());
        if let Some(first_id) = prioritize
            && let Some(win) = self.windows.get(&first_id)
            && let Some(geom) = terminal_entry(first_id, win)
        {
            main_windows.push(geom);
        }
        for (wid, win) in &self.windows {
            if prioritize == Some(*wid) {
                continue;
            }
            if let Some(geom) = terminal_entry(*wid, win) {
                main_windows.push(geom);
            }
        }
        self.window_state.main_windows = main_windows;
    }

    /// Save the current session to `last_session.toml` in the layout library.
    ///
    /// Called automatically when the last terminal window closes.  Runs
    /// regardless of `restore_last_session` so the on-disk session stays
    /// fresh; the flag only controls whether the saved session is
    /// reapplied on next launch.  Failures are logged but not fatal.
    pub(super) fn auto_save_session(&self) {
        let Some(path) = Self::last_session_path() else {
            error!("auto_save_session: cannot determine layout library path");
            return;
        };
        if let Some(parent) = path.parent()
            && let Err(e) = std::fs::create_dir_all(parent)
        {
            error!("auto_save_session: cannot create layout library dir: {e}");
            return;
        }
        match self.save_layout(&path, "Last Session", None) {
            Ok(()) => {
                tracing::info!("Session auto-saved to {}", path.display());
            }
            Err(e) => {
                error!("auto_save_session: failed: {e}");
            }
        }
    }

    /// Apply the last-session layout if `restore_last_session` is enabled and
    /// the file exists.  Called once after the first window is ready.
    ///
    /// Only called when no `--layout` CLI flag was provided.
    pub(super) fn maybe_restore_last_session(
        &mut self,
        window_id: freminal_windowing::WindowId,
        handle: &freminal_windowing::WindowHandle<'_>,
    ) {
        if !self.config.startup.restore_last_session {
            return;
        }
        let Some(path) = Self::last_session_path() else {
            return;
        };
        if !path.exists() {
            return;
        }
        match freminal_common::layout::Layout::from_file(&path).and_then(|l| {
            l.apply_variables(&[], &std::collections::HashMap::new())
                .resolve()
        }) {
            Ok(resolved) => {
                let commands = self.apply_layout(&resolved, window_id, handle);
                self.inject_layout_commands(&commands);
            }
            Err(e) => {
                error!(
                    "restore_last_session: failed to apply {}: {e}",
                    path.display()
                );
            }
        }
    }
}
