// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use std::hash::{Hash as _, Hasher as _};

use tracing::error;

use super::FreminalGui;
use super::window;

/// How often the background timer asks the GUI to re-evaluate the session for
/// auto-save.  The check is cheap when nothing changed (build + hash + compare,
/// no write), so a tight-ish minute keeps the on-disk session fresh without
/// being chatty.  Not user-configurable by design — see the module docs on
/// `last_session_fingerprint`.
pub(super) const SESSION_AUTOSAVE_INTERVAL: std::time::Duration = std::time::Duration::from_mins(1);

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

    /// Auto-save the current session to `last_session.toml`, but only if it
    /// differs from what was last written.
    ///
    /// Runs regardless of `restore_last_session` so the on-disk session stays
    /// fresh; the flag only controls whether the saved session is reapplied on
    /// next launch.  Invoked from two places:
    /// * the periodic background timer (every [`SESSION_AUTOSAVE_INTERVAL`]),
    ///   and
    /// * window close / shutdown.
    ///
    /// Because both paths converge here, most shutdown saves are no-ops: the
    /// timer has usually already persisted the current state, so the on-disk
    /// fingerprint matches and we skip the write entirely.  That deliberately
    /// makes the shutdown write non-load-bearing — the resilience win is that
    /// we no longer depend on a synchronous write surviving a hostile teardown.
    ///
    /// Skips saving when the user launched with an ad-hoc command
    /// (`freminal -- vim foo`): those panes run a one-shot program and are not
    /// meaningfully restorable.  Failures are logged but never fatal.
    pub(super) fn maybe_auto_save_session(&mut self) {
        if !self.args.command.is_empty() {
            return;
        }

        let Some(path) = Self::last_session_path() else {
            error!("maybe_auto_save_session: cannot determine layout library path");
            return;
        };

        // Build the session layout and serialize it in memory so we can
        // fingerprint the exact bytes we would write.  No disk read-back: the
        // fingerprint compares against the last value *we* wrote this run.
        let layout = self.build_layout("Last Session", None);
        let toml_str = match layout.to_toml_string() {
            Ok(s) => s,
            Err(e) => {
                error!("maybe_auto_save_session: serialize failed: {e}");
                return;
            }
        };

        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        toml_str.hash(&mut hasher);
        let fingerprint = hasher.finish();

        if self.last_session_fingerprint == Some(fingerprint) {
            // Nothing changed since the last write — skip the I/O entirely.
            tracing::trace!("session unchanged since last save; skipping write");
            return;
        }

        if let Some(parent) = path.parent()
            && let Err(e) = std::fs::create_dir_all(parent)
        {
            error!("maybe_auto_save_session: cannot create layout library dir: {e}");
            return;
        }

        match Self::atomic_write(&path, &toml_str) {
            Ok(()) => {
                self.last_session_fingerprint = Some(fingerprint);
                tracing::info!("Session auto-saved to {}", path.display());
            }
            Err(e) => {
                error!(
                    "maybe_auto_save_session: failed to write {}: {e}",
                    path.display()
                );
            }
        }
    }

    /// Spawn the background thread that periodically asks `update()` to
    /// re-evaluate the session for auto-save.
    ///
    /// The GUI event loop sleeps on `ControlFlow::Wait` at idle, so a timer
    /// living on the GUI thread (a per-frame elapsed check) would never fire
    /// without traffic.  Instead we mirror the PTY consumer pattern: a
    /// dedicated thread sleeps for [`SESSION_AUTOSAVE_INTERVAL`], flips the
    /// shared `session_save_due` flag, and pokes the window's
    /// [`RepaintProxy`](freminal_windowing::RepaintProxy) to wake the loop.
    /// `update()` observes the flag and calls
    /// [`Self::maybe_auto_save_session`].
    ///
    /// The thread holds only an `Arc<OnceLock<(RepaintProxy, WindowId)>>` and
    /// an `Arc<AtomicBool>` — no GUI state — so it cannot violate the
    /// single-threaded GUI invariant.  It runs for the life of the process
    /// (the proxy clone keeps the loop reachable); on process exit it is torn
    /// down with everything else.
    pub(super) fn spawn_session_autosave_timer(
        &self,
        repaint_handle: std::sync::Arc<
            std::sync::OnceLock<(
                freminal_windowing::RepaintProxy,
                freminal_windowing::WindowId,
            )>,
        >,
    ) {
        use std::sync::atomic::Ordering;

        let due = std::sync::Arc::clone(&self.session_save_due);
        std::thread::Builder::new()
            .name("session-autosave".to_owned())
            .spawn(move || {
                loop {
                    std::thread::sleep(SESSION_AUTOSAVE_INTERVAL);
                    due.store(true, Ordering::Relaxed);
                    // Wake the event loop so `update()` runs and sees the flag.
                    // If the handle is not yet initialised (first window still
                    // coming up), skip this tick's wake — the next one will
                    // catch it, and the flag is already latched.
                    if let Some((proxy, wid)) = repaint_handle.get() {
                        proxy.request_repaint(*wid);
                    }
                }
            })
            .map_or_else(
                |e| error!("failed to spawn session-autosave timer: {e}"),
                |_handle| (),
            );
    }

    /// If the background timer has requested it, re-evaluate and auto-save the
    /// session.  Called once near the top of each `update()`; cheap when the
    /// flag is clear.
    pub(super) fn poll_session_autosave(&mut self) {
        if self
            .session_save_due
            .swap(false, std::sync::atomic::Ordering::Relaxed)
        {
            self.maybe_auto_save_session();
        }
    }

    /// Resolve the path of a `--layout` or `startup.layout` name-or-path
    /// string against the layout library directory.
    ///
    /// If the value has a `.toml` extension or contains a path separator it
    /// is treated as a literal path; otherwise it is looked up inside the
    /// layout library as `<name>.toml`.
    pub(super) fn resolve_startup_layout_path(name_or_path: &str) -> std::path::PathBuf {
        let p = std::path::Path::new(name_or_path);
        if p.extension().is_some_and(|e| e == "toml") || p.components().count() > 1 {
            p.to_path_buf()
        } else {
            freminal_common::config::layout_library_dir().map_or_else(
                || p.to_path_buf(),
                |d| d.join(format!("{name_or_path}.toml")),
            )
        }
    }

    /// Whether the first window will immediately have its tabs replaced by
    /// a layout or session-restore apply.
    ///
    /// When this returns `true`, spawning a default PTY before window
    /// creation is wasteful: the pre-spawned tab would be dropped the
    /// moment `apply_layout` runs.  Callers use this to gate whether to
    /// spawn a default PTY at all for the first window.
    pub(super) fn will_layout_or_restore_apply(&self) -> bool {
        if self.args.layout.is_some() || self.config.startup.layout.is_some() {
            return true;
        }
        if !self.config.startup.restore_last_session {
            return false;
        }
        if !self.args.command.is_empty() {
            return false;
        }
        Self::last_session_path().is_some_and(|p| p.exists())
    }
}
