// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Runtime recording toggle.
//!
//! The GUI-side entry point for starting and stopping FREC v2 session
//! recordings at runtime. The recording stream itself is owned by a
//! writer thread spawned by [`freminal_terminal_emulator::recording::start_recording`];
//! this module wires the toggle (`KeyAction::ToggleRecording` and the
//! Session menu entry) into the application state.
//!
//! # Why two fields on `FreminalGui`
//!
//! `recording_swap` is shared with every PTY thread via `RecordingSwap`
//! (an `ArcSwap`). Each pane checks the swap on every event it emits; a
//! non-`None` value means "record this event". The GUI flips recording
//! on or off by storing a new handle (or clearing it) into the swap —
//! no per-pane plumbing required.
//!
//! `recording_join` lives solely on the GUI side so that stopping a
//! recording can deterministically block until the writer thread has
//! finalized the output file. Without it the file would only be flushed
//! when the *last* `RecordingHandle` clone is dropped, which happens at
//! process shutdown — unsuitable for a runtime toggle.

use std::sync::Arc;

use freminal_terminal_emulator::recording::{
    PaneNodeSnapshot, RecordingMetadata, TabSnapshot, TopologySnapshot, WindowSnapshot,
    start_recording,
};

impl super::FreminalGui {
    /// Build a full [`TopologySnapshot`] describing every open window,
    /// tab, and pane at this instant.
    ///
    /// Used when a recording is started mid-session so that the FREC
    /// header reflects the actual state of the application rather than
    /// an empty topology.
    ///
    /// - Window geometry is taken from each window's cached
    ///   `last_known_size` / `last_known_position`.
    /// - Per-pane CWDs are read best-effort via `/proc` on Linux (see
    ///   [`Self::read_cwd_for_pane_with_extra`]).
    /// - Shell commands are not tracked today and are always `None`.
    #[must_use]
    pub(super) fn build_topology_snapshot(&mut self) -> TopologySnapshot {
        // Collect window ids first so we can mutably assign recording ids
        // without holding an immutable borrow across the `recording_window_id`
        // call.
        let os_window_ids: Vec<super::WindowId> = self.windows.keys().copied().collect();

        let mut windows = Vec::with_capacity(os_window_ids.len());
        for os_wid in os_window_ids {
            let rec_wid = self.recording_window_id(os_wid);

            // Re-borrow after the mutable call above.
            let Some(win) = self.windows.get(&os_wid) else {
                continue;
            };

            let size = win.last_known_size.map_or((0, 0), <(u32, u32)>::from);
            let position = win.last_known_position.map(<(i32, i32)>::from);

            let active_tab_idx = win.tabs.active_index();
            let mut tabs = Vec::with_capacity(win.tabs.tab_count());
            let mut active_tab_rec_id: u32 = 0;

            for (idx, tab) in win.tabs.iter().enumerate() {
                let tab_rec_id = u32::try_from(tab.id.raw()).unwrap_or(u32::MAX);
                if idx == active_tab_idx {
                    active_tab_rec_id = tab_rec_id;
                }

                let active_pane_rec_id = u32::try_from(tab.active_pane.raw()).unwrap_or(u32::MAX);
                let zoomed_pane_rec_id = tab
                    .zoomed_pane
                    .map(|id| u32::try_from(id.raw()).unwrap_or(u32::MAX));

                // Build the pane tree snapshot. If the tree is empty
                // (should be impossible for an active tab), fall back to
                // a single empty leaf so the recording stays well-formed.
                let pane_tree = tab
                    .pane_tree
                    .to_recording_snapshot(
                        |pid| self.read_cwd_for_pane_with_extra(pid, None),
                        |_| None,
                    )
                    .unwrap_or_else(|| {
                        tracing::warn!(
                            "build_topology_snapshot: tab {tab_rec_id} has empty pane tree"
                        );
                        freminal_terminal_emulator::recording::PaneTreeSnapshot {
                            node: PaneNodeSnapshot::Leaf {
                                pane_id: 0,
                                cols: 0,
                                rows: 0,
                                cwd: None,
                                shell: None,
                                title: String::new(),
                            },
                        }
                    });

                tabs.push(TabSnapshot {
                    tab_id: tab_rec_id,
                    window_id: rec_wid,
                    pane_tree,
                    active_pane: active_pane_rec_id,
                    zoomed_pane: zoomed_pane_rec_id,
                });
            }

            windows.push(WindowSnapshot {
                window_id: rec_wid,
                position,
                size,
                tabs,
                active_tab: active_tab_rec_id,
            });
        }

        TopologySnapshot { windows }
    }

    /// Return `true` when a FREC v2 recording is currently being written.
    #[must_use]
    pub(super) fn is_recording(&self) -> bool {
        self.recording_swap.load().is_some()
    }

    /// Toggle the FREC v2 recording on or off.
    ///
    /// When starting, a new output file is created under the platform
    /// recording library directory (see
    /// [`freminal_common::config::recording_library_dir`]) with a
    /// timestamped filename. The current topology is snapshotted and
    /// embedded in the FREC header so the recording can be replayed
    /// without reconstructing the pre-existing layout from events.
    ///
    /// When stopping, the swap is cleared (so panes stop emitting
    /// events) and the writer thread is joined so callers can be
    /// confident the file is fully flushed before this returns.
    ///
    /// Errors are surfaced via toast notifications; the toggle is
    /// best-effort and never panics.
    pub(super) fn toggle_recording(&mut self) {
        if self.is_recording() {
            self.stop_recording();
        } else {
            self.start_recording();
        }
    }

    fn start_recording(&mut self) {
        let Some(dir) = freminal_common::config::recording_library_dir() else {
            tracing::error!("Cannot start recording: no recording library directory available");
            self.push_error_toast(
                "Cannot start recording",
                Some("No recording library directory is available on this platform.".to_owned()),
            );
            return;
        };

        // Timestamped filename: freminal-YYYYMMDD-HHMMSS.frec
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let filename = format!("freminal-{now}.frec");
        let path = dir.join(&filename);

        let metadata = RecordingMetadata {
            freminal_version: env!("CARGO_PKG_VERSION").to_string(),
            created_at: now,
            term: "xterm-256color".to_string(),
            initial_topology: self.build_topology_snapshot(),
            scrollback_limit: self.config.scrollback.limit.try_into().unwrap_or(u32::MAX),
        };

        match start_recording(&path, metadata, 4096) {
            Ok((handle, join)) => {
                tracing::info!("Started recording to {}", path.display());
                self.recording_swap.store(Some(Arc::new(handle)));
                self.recording_join = Some(join);
                self.recording_path = Some(path);
            }
            Err(e) => {
                tracing::error!("Failed to start recording to {}: {e}", path.display());
                self.push_error_toast(
                    "Failed to start recording",
                    Some(format!("{}: {e}", path.display())),
                );
            }
        }
    }

    fn stop_recording(&mut self) {
        // Clear the swap first so PTY threads immediately stop emitting
        // events. Dropping the handle(s) they may have cloned inside
        // `RecordingSwap` closes the sender side of the channel, which
        // causes the writer thread to exit its loop and finalize.
        self.recording_swap.store(None);

        if let Some(mut join) = self.recording_join.take() {
            // Block until the writer has flushed and finalized the file.
            // The writer is I/O-bound but the file size is bounded by the
            // session length, so this returns quickly in practice.
            join.join();
        }

        if let Some(path) = self.recording_path.take() {
            tracing::info!("Stopped recording; file saved to {}", path.display());
        } else {
            tracing::info!("Stopped recording");
        }
    }
}
