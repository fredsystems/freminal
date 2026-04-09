// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Pane model for built-in terminal multiplexing.
//!
//! Each `Pane` owns an independent set of channels to its PTY consumer thread
//! and a shared snapshot handle — the same fields that currently live on `Tab`.
//! When split panes are introduced (subtask 58.3), `Tab` will delegate to a
//! `PaneTree` of `Pane` instances rather than holding these fields directly.
//!
//! `PaneId` is a monotonic, unique identifier analogous to `TabId`.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use arc_swap::ArcSwap;
use crossbeam_channel::{Receiver, Sender};
use freminal_common::buffer_states::tchar::TChar;
use freminal_common::pty_write::PtyWrite;
use freminal_terminal_emulator::io::{InputEvent, WindowCommand};
use freminal_terminal_emulator::snapshot::TerminalSnapshot;

use super::view_state::ViewState;

/// A unique, monotonically increasing identifier for each pane.
///
/// Used to track pane identity within a tab's pane tree without relying
/// on tree structure or indices, which change when panes are split or closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PaneId(u64);

impl PaneId {
    /// The initial `PaneId` used for the first pane (id 0).
    #[must_use]
    pub const fn first() -> Self {
        Self(0)
    }
}

impl std::fmt::Display for PaneId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Pane({})", self.0)
    }
}

/// Counter for generating unique `PaneId` values.
///
/// Global to the application — every pane ever created gets a unique id,
/// even across different tabs. This avoids id collisions when panes are
/// moved between tabs in the future.
#[derive(Debug)]
pub struct PaneIdGenerator {
    next: u64,
}

impl PaneIdGenerator {
    /// Create a new generator. The first id it produces will be `PaneId(start)`.
    #[must_use]
    pub const fn new(start: u64) -> Self {
        Self { next: start }
    }

    /// Allocate the next unique `PaneId`.
    pub const fn next_id(&mut self) -> PaneId {
        let id = PaneId(self.next);
        self.next += 1;
        id
    }
}

impl Default for PaneIdGenerator {
    fn default() -> Self {
        Self::new(0)
    }
}

/// A single terminal pane.
///
/// Each pane owns an independent set of channels to its PTY consumer thread
/// and a shared snapshot handle. These are the same fields that currently
/// live on `Tab`; in subtask 58.3, `Tab` will be refactored to hold a
/// `PaneTree` of `Pane` instances instead.
///
/// The pane tree lives on the GUI thread. PTY threads are unaware of the
/// tree structure — they just own their `TerminalEmulator` and publish
/// snapshots via `ArcSwap`.
pub struct Pane {
    /// Unique identifier for this pane.
    pub id: PaneId,

    /// The latest terminal snapshot published by this pane's PTY consumer thread.
    pub arc_swap: Arc<ArcSwap<TerminalSnapshot>>,

    /// Channel sender for input events (key, resize, focus) to this pane's PTY thread.
    pub input_tx: Sender<InputEvent>,

    /// Sender for raw bytes back to this pane's PTY (for Report* responses).
    pub pty_write_tx: Sender<PtyWrite>,

    /// Receiver for window manipulation commands from this pane's PTY thread.
    pub window_cmd_rx: Receiver<WindowCommand>,

    /// Receiver for clipboard text extraction responses from this pane's PTY thread.
    pub clipboard_rx: Receiver<String>,

    /// Receiver for full-buffer search content from this pane's PTY thread.
    ///
    /// When the GUI sends `InputEvent::RequestSearchBuffer`, the PTY thread
    /// concatenates scrollback + visible `TChar` data and sends it here.
    /// The first element of the tuple is `total_rows` at the time the buffer
    /// was captured, used by the GUI to detect stale responses.
    pub search_buffer_rx: Receiver<(usize, Vec<TChar>)>,

    /// Signals that this pane's PTY process has exited.
    ///
    /// The PTY consumer thread sends `()` when the child exits or the PTY read
    /// channel closes. The GUI polls this to close the pane (or the whole
    /// tab when it is the last remaining pane).
    pub pty_dead_rx: Receiver<()>,

    /// Pane title, set by OSC 0/2 escape sequences.
    ///
    /// When empty, the visible label is supplied by pane creation or tab-bar UI
    /// fallback logic rather than a guaranteed model-level default title.
    pub title: String,

    /// Whether a bell has fired in this pane and not yet been cleared.
    pub bell_active: bool,

    /// Per-pane title stack for `SaveWindowTitleToStack` /
    /// `RestoreWindowTitleFromStack` (CSI 22/23 t). Each pane maintains its
    /// own stack so that background shells pushing/popping titles do not
    /// interfere with the active pane.
    pub title_stack: Vec<String>,

    /// Per-pane GUI view state (scroll offset, selection, blink, mouse).
    pub view_state: ViewState,

    /// Shared atomic flag reflecting whether the PTY slave currently has
    /// `ECHO` disabled (i.e. a password prompt is active).
    ///
    /// Read directly every frame by the GUI (cheap `Relaxed` atomic load)
    /// instead of going through `TerminalSnapshot`, because snapshots are
    /// only published on PTY output — if the shell is idle waiting for a
    /// password, the snapshot would be stale.
    pub echo_off: Arc<AtomicBool>,
}

impl std::fmt::Debug for Pane {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pane")
            .field("id", &self.id)
            .field("title", &self.title)
            .field("bell_active", &self.bell_active)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    // ── PaneId tests ─────────────────────────────────────────────────

    #[test]
    fn pane_id_first_is_zero() {
        let id = PaneId::first();
        assert_eq!(id, PaneId(0));
    }

    #[test]
    fn pane_id_equality() {
        assert_eq!(PaneId(42), PaneId(42));
        assert_ne!(PaneId(1), PaneId(2));
    }

    #[test]
    fn pane_id_display() {
        let id = PaneId(7);
        assert_eq!(format!("{id}"), "Pane(7)");
    }

    #[test]
    fn pane_id_hash_works() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(PaneId(0));
        set.insert(PaneId(1));
        set.insert(PaneId(0)); // duplicate
        assert_eq!(set.len(), 2);
    }

    // ── PaneIdGenerator tests ────────────────────────────────────────

    #[test]
    fn generator_default_starts_at_zero() {
        let mut id_gen = PaneIdGenerator::default();
        assert_eq!(id_gen.next_id(), PaneId(0));
        assert_eq!(id_gen.next_id(), PaneId(1));
        assert_eq!(id_gen.next_id(), PaneId(2));
    }

    #[test]
    fn generator_custom_start() {
        let mut id_gen = PaneIdGenerator::new(100);
        assert_eq!(id_gen.next_id(), PaneId(100));
        assert_eq!(id_gen.next_id(), PaneId(101));
    }

    #[test]
    fn generator_ids_are_unique() {
        let mut id_gen = PaneIdGenerator::default();
        let ids: Vec<PaneId> = (0..50).map(|_| id_gen.next_id()).collect();
        let unique: std::collections::HashSet<PaneId> = ids.iter().copied().collect();
        assert_eq!(ids.len(), unique.len(), "all generated ids must be unique");
    }

    // ── Pane struct tests ────────────────────────────────────────────

    /// Create a dummy `Pane` for testing.
    ///
    /// Uses disconnected channels that will fail on send/recv, which is fine
    /// for testing the pane data model.
    fn dummy_pane(id: PaneId, title: &str) -> Pane {
        let arc_swap = Arc::new(ArcSwap::from_pointee(TerminalSnapshot::empty()));
        let (input_tx, _input_rx) = crossbeam_channel::unbounded();
        let (pty_write_tx, _pty_write_rx) = crossbeam_channel::unbounded();
        let (_window_cmd_tx, window_cmd_rx) = crossbeam_channel::unbounded();
        let (_clipboard_tx, clipboard_rx) = crossbeam_channel::bounded(1);
        let (_search_buffer_tx, search_buffer_rx) =
            crossbeam_channel::bounded::<(usize, Vec<TChar>)>(1);
        let (_pty_dead_tx, pty_dead_rx) = crossbeam_channel::bounded(1);

        Pane {
            id,
            arc_swap,
            input_tx,
            pty_write_tx,
            window_cmd_rx,
            clipboard_rx,
            search_buffer_rx,
            pty_dead_rx,
            title: title.to_owned(),
            bell_active: false,
            title_stack: Vec::new(),
            view_state: ViewState::new(),
            echo_off: Arc::new(AtomicBool::new(false)),
        }
    }

    #[test]
    fn pane_debug_includes_id_and_title() {
        let pane = dummy_pane(PaneId(42), "test pane");
        let debug = format!("{pane:?}");
        assert!(debug.contains("42"));
        assert!(debug.contains("test pane"));
    }

    #[test]
    fn pane_fields_are_accessible() {
        let pane = dummy_pane(PaneId(5), "my pane");
        assert_eq!(pane.id, PaneId(5));
        assert_eq!(pane.title, "my pane");
        assert!(!pane.bell_active);
        assert!(pane.title_stack.is_empty());
        assert_eq!(pane.view_state.scroll_offset, 0);
        assert!(!pane.echo_off.load(std::sync::atomic::Ordering::Relaxed));
    }

    #[test]
    fn pane_view_state_is_independent() {
        let mut pane1 = dummy_pane(PaneId(0), "pane 1");
        let pane2 = dummy_pane(PaneId(1), "pane 2");

        pane1.view_state.scroll_offset = 42;
        assert_eq!(pane1.view_state.scroll_offset, 42);
        assert_eq!(pane2.view_state.scroll_offset, 0);
    }
}
