// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Tab model and manager for multi-terminal support.
//!
//! Each `Tab` owns its own PTY thread channels and snapshot handle.
//! `TabManager` provides an API for creating, closing, switching, and
//! reordering tabs.  It is owned exclusively by `FreminalGui` and never
//! shared with the PTY thread.

use std::sync::Arc;

use arc_swap::ArcSwap;
use crossbeam_channel::{Receiver, Sender};
use freminal_common::buffer_states::tchar::TChar;
use freminal_common::pty_write::PtyWrite;
use freminal_terminal_emulator::io::{InputEvent, WindowCommand};
use freminal_terminal_emulator::snapshot::TerminalSnapshot;

use super::view_state::ViewState;

/// A unique, monotonically increasing identifier for each tab.
///
/// Used to track tab identity across reorders and closes without relying
/// on vector indices, which shift when tabs are removed or moved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TabId(u64);

impl TabId {
    /// The initial `TabId` used for the first tab (id 0).
    #[must_use]
    pub const fn first() -> Self {
        Self(0)
    }
}

/// A single terminal tab.
///
/// Each tab owns an independent set of channels to its PTY consumer thread
/// and a shared snapshot handle.  The GUI reads from `arc_swap` and sends
/// user input through `input_tx`.
pub struct Tab {
    /// Unique identifier for this tab.
    pub id: TabId,

    /// The latest terminal snapshot published by this tab's PTY consumer thread.
    pub arc_swap: Arc<ArcSwap<TerminalSnapshot>>,

    /// Channel sender for input events (key, resize, focus) to this tab's PTY thread.
    pub input_tx: Sender<InputEvent>,

    /// Sender for raw bytes back to this tab's PTY (for Report* responses).
    pub pty_write_tx: Sender<PtyWrite>,

    /// Receiver for window manipulation commands from this tab's PTY thread.
    pub window_cmd_rx: Receiver<WindowCommand>,

    /// Receiver for clipboard text extraction responses from this tab's PTY thread.
    pub clipboard_rx: Receiver<String>,

    /// Receiver for full-buffer search content from this tab's PTY thread.
    ///
    /// When the GUI sends `InputEvent::RequestSearchBuffer`, the PTY thread
    /// concatenates scrollback + visible `TChar` data and sends it here.
    pub search_buffer_rx: Receiver<Vec<TChar>>,

    /// Signals that this tab's PTY process has exited.
    ///
    /// The PTY consumer thread sends `()` when the child exits or the PTY read
    /// channel closes.  The GUI polls this to close the tab (or the whole
    /// application when it is the last remaining tab).
    pub pty_dead_rx: Receiver<()>,

    /// Tab title, set by OSC 0/2 escape sequences.
    ///
    /// When empty, the visible label is supplied by tab creation or tab-bar UI
    /// fallback logic rather than a guaranteed model-level default title.
    pub title: String,

    /// Whether a bell has fired in this tab and not yet been cleared.
    pub bell_active: bool,

    /// Per-tab title stack for `SaveWindowTitleToStack` /
    /// `RestoreWindowTitleFromStack` (CSI 22/23 t).  Each tab maintains its
    /// own stack so that background shells pushing/popping titles do not
    /// interfere with the active tab.
    pub title_stack: Vec<String>,

    /// Per-tab GUI view state (scroll offset, selection, blink, mouse).
    pub view_state: ViewState,
}

impl std::fmt::Debug for Tab {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tab")
            .field("id", &self.id)
            .field("title", &self.title)
            .field("bell_active", &self.bell_active)
            .finish_non_exhaustive()
    }
}

/// Error type for `TabManager` operations.
#[derive(Debug, thiserror::Error)]
pub enum TabError {
    /// The requested tab index is out of bounds.
    #[error("tab index {index} is out of bounds (have {count} tabs)")]
    IndexOutOfBounds {
        /// The invalid index that was requested.
        index: usize,
        /// The number of tabs currently open.
        count: usize,
    },

    /// Attempted to close the last remaining tab.
    #[error("cannot close the last tab")]
    CannotCloseLastTab,

    /// The source and destination indices for a move are equal.
    #[error("source and destination indices are equal ({index})")]
    MoveToSelf {
        /// The index that was both source and destination.
        index: usize,
    },
}

/// Manages a collection of terminal tabs.
///
/// Owns a `Vec<Tab>` and tracks the currently active tab index.
/// All mutations go through `TabManager` methods so invariants are
/// enforced in one place.
#[derive(Debug)]
pub struct TabManager {
    /// All open tabs, in display order.
    tabs: Vec<Tab>,

    /// Index of the currently active (visible) tab.
    active: usize,

    /// Counter for generating unique `TabId` values.
    next_id: u64,
}

impl TabManager {
    /// Create a new `TabManager` with a single initial tab.
    #[must_use]
    pub fn new(initial: Tab) -> Self {
        Self {
            tabs: vec![initial],
            active: 0,
            next_id: 1,
        }
    }

    /// Allocate the next unique `TabId`.
    pub const fn next_tab_id(&mut self) -> TabId {
        let id = TabId(self.next_id);
        self.next_id += 1;
        id
    }

    /// Return a reference to the currently active tab.
    #[must_use]
    pub fn active_tab(&self) -> &Tab {
        // SAFETY: `active` is always kept in bounds by all mutating methods.
        // This index is maintained as an invariant — if it were ever out of
        // bounds, that would be a bug in TabManager, not a runtime condition
        // the caller should handle.  We use indexing (not `.get()`) to make
        // the invariant visible.
        &self.tabs[self.active]
    }

    /// Return a mutable reference to the currently active tab.
    #[must_use]
    pub fn active_tab_mut(&mut self) -> &mut Tab {
        &mut self.tabs[self.active]
    }

    /// Return the index of the currently active tab.
    #[must_use]
    pub const fn active_index(&self) -> usize {
        self.active
    }

    /// Return the number of open tabs.
    #[must_use]
    pub const fn tab_count(&self) -> usize {
        self.tabs.len()
    }

    /// Return an iterator over all tabs (in display order).
    #[must_use]
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &Tab> + ExactSizeIterator {
        self.tabs.iter()
    }

    /// Return a mutable iterator over all tabs (in display order).
    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut Tab> {
        self.tabs.iter_mut()
    }

    /// Add a new tab at the end and switch to it.
    pub fn add_tab(&mut self, tab: Tab) {
        self.tabs.push(tab);
        self.active = self.tabs.len() - 1;
    }

    /// Close the tab at `index`, returning the removed `Tab`.
    ///
    /// # Errors
    ///
    /// Returns `TabError::IndexOutOfBounds` if `index >= tab_count()`.
    /// Returns `TabError::CannotCloseLastTab` if only one tab remains.
    pub fn close_tab(&mut self, index: usize) -> Result<Tab, TabError> {
        if index >= self.tabs.len() {
            return Err(TabError::IndexOutOfBounds {
                index,
                count: self.tabs.len(),
            });
        }

        if self.tabs.len() == 1 {
            return Err(TabError::CannotCloseLastTab);
        }

        let removed = self.tabs.remove(index);

        // Adjust active index after removal.
        if self.active >= self.tabs.len() {
            // Was on the last tab and it got removed — move to new last.
            self.active = self.tabs.len() - 1;
        } else if self.active > index {
            // A tab before the active one was removed — shift left.
            self.active -= 1;
        }
        // If active < index, no adjustment needed.
        // If active == index and index < len, we now point at the tab that
        // slid into this slot, which is the natural successor — no change.

        Ok(removed)
    }

    /// Switch to the tab at `index`.
    ///
    /// # Errors
    ///
    /// Returns `TabError::IndexOutOfBounds` if `index >= tab_count()`.
    pub const fn switch_to(&mut self, index: usize) -> Result<(), TabError> {
        if index >= self.tabs.len() {
            return Err(TabError::IndexOutOfBounds {
                index,
                count: self.tabs.len(),
            });
        }
        self.active = index;
        Ok(())
    }

    /// Switch to the next tab (wrapping around to the first).
    pub const fn next_tab(&mut self) {
        self.active = (self.active + 1) % self.tabs.len();
    }

    /// Switch to the previous tab (wrapping around to the last).
    pub const fn prev_tab(&mut self) {
        if self.active == 0 {
            self.active = self.tabs.len() - 1;
        } else {
            self.active -= 1;
        }
    }

    /// Move a tab from `from` to `to` in display order.
    ///
    /// The active tab index is updated to follow the moved tab if it was
    /// the active one, or adjusted if another tab moved past it.
    ///
    /// # Errors
    ///
    /// Returns `TabError::IndexOutOfBounds` if either index is invalid.
    /// Returns `TabError::MoveToSelf` if `from == to`.
    pub fn move_tab(&mut self, from: usize, to: usize) -> Result<(), TabError> {
        let count = self.tabs.len();

        if from >= count {
            return Err(TabError::IndexOutOfBounds { index: from, count });
        }
        if to >= count {
            return Err(TabError::IndexOutOfBounds { index: to, count });
        }
        if from == to {
            return Err(TabError::MoveToSelf { index: from });
        }

        let tab = self.tabs.remove(from);
        self.tabs.insert(to, tab);

        // Update active index to follow the moved tab or adjust for shifts.
        if self.active == from {
            self.active = to;
        } else if from < self.active && self.active <= to {
            // Tab moved right past the active one — active shifts left.
            self.active -= 1;
        } else if to <= self.active && self.active < from {
            // Tab moved left past the active one — active shifts right.
            self.active += 1;
        }

        Ok(())
    }

    /// Move the active tab one position to the left.
    ///
    /// Does nothing if the active tab is already the first.
    pub fn move_active_left(&mut self) {
        if self.active > 0 {
            self.tabs.swap(self.active, self.active - 1);
            self.active -= 1;
        }
    }

    /// Move the active tab one position to the right.
    ///
    /// Does nothing if the active tab is already the last.
    pub fn move_active_right(&mut self) {
        if self.active + 1 < self.tabs.len() {
            self.tabs.swap(self.active, self.active + 1);
            self.active += 1;
        }
    }

    /// Close the currently active tab, returning the removed `Tab`.
    ///
    /// # Errors
    ///
    /// Returns `TabError::CannotCloseLastTab` if only one tab remains.
    pub fn close_active_tab(&mut self) -> Result<Tab, TabError> {
        self.close_tab(self.active)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// Create a dummy `Tab` for testing.
    ///
    /// Uses disconnected channels that will fail on send/recv, which is fine
    /// for testing the `TabManager` data model.
    fn dummy_tab(id: TabId, title: &str) -> Tab {
        let arc_swap = Arc::new(ArcSwap::from_pointee(TerminalSnapshot::empty()));
        let (input_tx, _input_rx) = crossbeam_channel::unbounded();
        let (pty_write_tx, _pty_write_rx) = crossbeam_channel::unbounded();
        let (_window_cmd_tx, window_cmd_rx) = crossbeam_channel::unbounded();
        let (_clipboard_tx, clipboard_rx) = crossbeam_channel::bounded(1);
        let (_search_buffer_tx, search_buffer_rx) = crossbeam_channel::bounded::<Vec<TChar>>(1);
        let (_pty_dead_tx, pty_dead_rx) = crossbeam_channel::bounded(1);

        Tab {
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
        }
    }

    #[test]
    fn new_manager_has_one_tab() {
        let tab = dummy_tab(TabId(0), "Tab 1");
        let mgr = TabManager::new(tab);

        assert_eq!(mgr.tab_count(), 1);
        assert_eq!(mgr.active_index(), 0);
        assert_eq!(mgr.active_tab().title, "Tab 1");
    }

    #[test]
    fn next_tab_id_increments() {
        let tab = dummy_tab(TabId(0), "Tab 1");
        let mut mgr = TabManager::new(tab);

        let id1 = mgr.next_tab_id();
        let id2 = mgr.next_tab_id();
        let id3 = mgr.next_tab_id();

        assert_eq!(id1, TabId(1));
        assert_eq!(id2, TabId(2));
        assert_eq!(id3, TabId(3));
    }

    #[test]
    fn add_tab_switches_to_new() {
        let tab1 = dummy_tab(TabId(0), "Tab 1");
        let mut mgr = TabManager::new(tab1);

        let tab2 = dummy_tab(TabId(1), "Tab 2");
        mgr.add_tab(tab2);

        assert_eq!(mgr.tab_count(), 2);
        assert_eq!(mgr.active_index(), 1);
        assert_eq!(mgr.active_tab().title, "Tab 2");
    }

    #[test]
    fn close_tab_removes_and_adjusts_active() {
        let tab1 = dummy_tab(TabId(0), "Tab 1");
        let mut mgr = TabManager::new(tab1);
        let tab2 = dummy_tab(TabId(1), "Tab 2");
        let tab3 = dummy_tab(TabId(2), "Tab 3");
        mgr.add_tab(tab2);
        mgr.add_tab(tab3);
        // active = 2 (Tab 3)

        // Close Tab 3 (active, last).
        let removed = mgr.close_tab(2).unwrap();
        assert_eq!(removed.title, "Tab 3");
        assert_eq!(mgr.tab_count(), 2);
        assert_eq!(mgr.active_index(), 1); // moved to new last
        assert_eq!(mgr.active_tab().title, "Tab 2");
    }

    #[test]
    fn close_tab_before_active_shifts_left() {
        let tab1 = dummy_tab(TabId(0), "Tab 1");
        let mut mgr = TabManager::new(tab1);
        let tab2 = dummy_tab(TabId(1), "Tab 2");
        let tab3 = dummy_tab(TabId(2), "Tab 3");
        mgr.add_tab(tab2);
        mgr.add_tab(tab3);
        // active = 2 (Tab 3)

        // Close Tab 1 (before active).
        mgr.close_tab(0).unwrap();
        assert_eq!(mgr.tab_count(), 2);
        assert_eq!(mgr.active_index(), 1); // shifted left
        assert_eq!(mgr.active_tab().title, "Tab 3");
    }

    #[test]
    fn close_tab_at_active_selects_successor() {
        let tab1 = dummy_tab(TabId(0), "Tab 1");
        let mut mgr = TabManager::new(tab1);
        let tab2 = dummy_tab(TabId(1), "Tab 2");
        let tab3 = dummy_tab(TabId(2), "Tab 3");
        mgr.add_tab(tab2);
        mgr.add_tab(tab3);

        // Switch to Tab 2 (index 1) and close it.
        mgr.switch_to(1).unwrap();
        assert_eq!(mgr.active_tab().title, "Tab 2");

        mgr.close_tab(1).unwrap();
        assert_eq!(mgr.tab_count(), 2);
        assert_eq!(mgr.active_index(), 1); // successor (Tab 3 slid into slot 1)
        assert_eq!(mgr.active_tab().title, "Tab 3");
    }

    #[test]
    fn close_last_remaining_tab_fails() {
        let tab = dummy_tab(TabId(0), "Only tab");
        let mut mgr = TabManager::new(tab);

        let err = mgr.close_tab(0).unwrap_err();
        assert!(matches!(err, TabError::CannotCloseLastTab));
    }

    #[test]
    fn close_out_of_bounds_fails() {
        let tab = dummy_tab(TabId(0), "Tab 1");
        let mut mgr = TabManager::new(tab);

        let err = mgr.close_tab(5).unwrap_err();
        assert!(matches!(
            err,
            TabError::IndexOutOfBounds { index: 5, count: 1 }
        ));
    }

    #[test]
    fn switch_to_valid_index() {
        let tab1 = dummy_tab(TabId(0), "Tab 1");
        let mut mgr = TabManager::new(tab1);
        let tab2 = dummy_tab(TabId(1), "Tab 2");
        mgr.add_tab(tab2);

        mgr.switch_to(0).unwrap();
        assert_eq!(mgr.active_index(), 0);
        assert_eq!(mgr.active_tab().title, "Tab 1");
    }

    #[test]
    fn switch_to_out_of_bounds_fails() {
        let tab = dummy_tab(TabId(0), "Tab 1");
        let mut mgr = TabManager::new(tab);

        let err = mgr.switch_to(3).unwrap_err();
        assert!(matches!(
            err,
            TabError::IndexOutOfBounds { index: 3, count: 1 }
        ));
    }

    #[test]
    fn next_tab_wraps_around() {
        let tab1 = dummy_tab(TabId(0), "Tab 1");
        let mut mgr = TabManager::new(tab1);
        let tab2 = dummy_tab(TabId(1), "Tab 2");
        let tab3 = dummy_tab(TabId(2), "Tab 3");
        mgr.add_tab(tab2);
        mgr.add_tab(tab3);
        // active = 2

        mgr.next_tab();
        assert_eq!(mgr.active_index(), 0); // wrapped

        mgr.next_tab();
        assert_eq!(mgr.active_index(), 1);
    }

    #[test]
    fn prev_tab_wraps_around() {
        let tab1 = dummy_tab(TabId(0), "Tab 1");
        let mut mgr = TabManager::new(tab1);
        let tab2 = dummy_tab(TabId(1), "Tab 2");
        let tab3 = dummy_tab(TabId(2), "Tab 3");
        mgr.add_tab(tab2);
        mgr.add_tab(tab3);

        mgr.switch_to(0).unwrap();
        mgr.prev_tab();
        assert_eq!(mgr.active_index(), 2); // wrapped to last
    }

    #[test]
    fn move_tab_forward() {
        let tab1 = dummy_tab(TabId(0), "A");
        let mut mgr = TabManager::new(tab1);
        let tab2 = dummy_tab(TabId(1), "B");
        let tab3 = dummy_tab(TabId(2), "C");
        mgr.add_tab(tab2);
        mgr.add_tab(tab3);
        // [A, B, C], active=2 (C)

        // Move A from 0 to 2.
        mgr.move_tab(0, 2).unwrap();
        let titles: Vec<&str> = mgr.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titles, vec!["B", "C", "A"]);
        // C was at active=2, tab moved right past it → active shifts left.
        assert_eq!(mgr.active_index(), 1);
    }

    #[test]
    fn move_tab_backward() {
        let tab1 = dummy_tab(TabId(0), "A");
        let mut mgr = TabManager::new(tab1);
        let tab2 = dummy_tab(TabId(1), "B");
        let tab3 = dummy_tab(TabId(2), "C");
        mgr.add_tab(tab2);
        mgr.add_tab(tab3);

        // Switch to A (index 0).
        mgr.switch_to(0).unwrap();
        // [A, B, C], active=0 (A)

        // Move C from 2 to 0.
        mgr.move_tab(2, 0).unwrap();
        let titles: Vec<&str> = mgr.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titles, vec!["C", "A", "B"]);
        // A was at active=0, tab moved left past it → active shifts right.
        assert_eq!(mgr.active_index(), 1);
    }

    #[test]
    fn move_active_tab() {
        let tab1 = dummy_tab(TabId(0), "A");
        let mut mgr = TabManager::new(tab1);
        let tab2 = dummy_tab(TabId(1), "B");
        let tab3 = dummy_tab(TabId(2), "C");
        mgr.add_tab(tab2);
        mgr.add_tab(tab3);

        // Switch to B (index 1) and move it to 0.
        mgr.switch_to(1).unwrap();
        mgr.move_tab(1, 0).unwrap();
        let titles: Vec<&str> = mgr.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titles, vec!["B", "A", "C"]);
        assert_eq!(mgr.active_index(), 0); // followed the moved tab
    }

    #[test]
    fn move_tab_out_of_bounds_fails() {
        let tab = dummy_tab(TabId(0), "Tab 1");
        let mut mgr = TabManager::new(tab);

        let err = mgr.move_tab(0, 5).unwrap_err();
        assert!(matches!(err, TabError::IndexOutOfBounds { index: 5, .. }));

        let err = mgr.move_tab(5, 0).unwrap_err();
        assert!(matches!(err, TabError::IndexOutOfBounds { index: 5, .. }));
    }

    #[test]
    fn move_tab_to_self_fails() {
        let tab1 = dummy_tab(TabId(0), "Tab 1");
        let mut mgr = TabManager::new(tab1);
        let tab2 = dummy_tab(TabId(1), "Tab 2");
        mgr.add_tab(tab2);

        let err = mgr.move_tab(1, 1).unwrap_err();
        assert!(matches!(err, TabError::MoveToSelf { index: 1 }));
    }

    #[test]
    fn move_active_left_at_start_is_noop() {
        let tab1 = dummy_tab(TabId(0), "A");
        let mut mgr = TabManager::new(tab1);
        let tab2 = dummy_tab(TabId(1), "B");
        mgr.add_tab(tab2);

        mgr.switch_to(0).unwrap();
        mgr.move_active_left(); // already leftmost

        assert_eq!(mgr.active_index(), 0);
        let titles: Vec<&str> = mgr.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titles, vec!["A", "B"]);
    }

    #[test]
    fn move_active_left_swaps() {
        let tab1 = dummy_tab(TabId(0), "A");
        let mut mgr = TabManager::new(tab1);
        let tab2 = dummy_tab(TabId(1), "B");
        mgr.add_tab(tab2);
        // active = 1 (B)

        mgr.move_active_left();
        assert_eq!(mgr.active_index(), 0);
        let titles: Vec<&str> = mgr.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titles, vec!["B", "A"]);
    }

    #[test]
    fn move_active_right_at_end_is_noop() {
        let tab1 = dummy_tab(TabId(0), "A");
        let mut mgr = TabManager::new(tab1);
        let tab2 = dummy_tab(TabId(1), "B");
        mgr.add_tab(tab2);
        // active = 1 (B, last)

        mgr.move_active_right(); // already rightmost

        assert_eq!(mgr.active_index(), 1);
        let titles: Vec<&str> = mgr.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titles, vec!["A", "B"]);
    }

    #[test]
    fn move_active_right_swaps() {
        let tab1 = dummy_tab(TabId(0), "A");
        let mut mgr = TabManager::new(tab1);
        let tab2 = dummy_tab(TabId(1), "B");
        mgr.add_tab(tab2);

        mgr.switch_to(0).unwrap();
        mgr.move_active_right();
        assert_eq!(mgr.active_index(), 1);
        let titles: Vec<&str> = mgr.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titles, vec!["B", "A"]);
    }

    #[test]
    fn iter_returns_all_tabs_in_order() {
        let tab1 = dummy_tab(TabId(0), "X");
        let mut mgr = TabManager::new(tab1);
        let tab2 = dummy_tab(TabId(1), "Y");
        let tab3 = dummy_tab(TabId(2), "Z");
        mgr.add_tab(tab2);
        mgr.add_tab(tab3);

        let titles: Vec<&str> = mgr.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titles, vec!["X", "Y", "Z"]);
    }

    #[test]
    fn active_tab_mut_allows_mutation() {
        let tab = dummy_tab(TabId(0), "Original");
        let mut mgr = TabManager::new(tab);

        mgr.active_tab_mut().title = "Modified".to_owned();
        assert_eq!(mgr.active_tab().title, "Modified");
    }

    #[test]
    fn tab_id_equality() {
        assert_eq!(TabId(42), TabId(42));
        assert_ne!(TabId(1), TabId(2));
    }

    #[test]
    fn close_middle_tab_while_active_is_first() {
        let tab1 = dummy_tab(TabId(0), "A");
        let mut mgr = TabManager::new(tab1);
        let tab2 = dummy_tab(TabId(1), "B");
        let tab3 = dummy_tab(TabId(2), "C");
        mgr.add_tab(tab2);
        mgr.add_tab(tab3);

        // Switch to first tab.
        mgr.switch_to(0).unwrap();
        assert_eq!(mgr.active_index(), 0);

        // Close middle tab (index 1) — active (0) is before it, no shift.
        mgr.close_tab(1).unwrap();
        assert_eq!(mgr.active_index(), 0);
        assert_eq!(mgr.active_tab().title, "A");
        assert_eq!(mgr.tab_count(), 2);
    }

    #[test]
    fn single_tab_next_prev_stays() {
        let tab = dummy_tab(TabId(0), "Only");
        let mut mgr = TabManager::new(tab);

        mgr.next_tab();
        assert_eq!(mgr.active_index(), 0);

        mgr.prev_tab();
        assert_eq!(mgr.active_index(), 0);
    }

    #[test]
    fn tab_debug_includes_id_and_title() {
        let tab = dummy_tab(TabId(42), "Debug test");
        let debug = format!("{tab:?}");
        assert!(debug.contains("42"));
        assert!(debug.contains("Debug test"));
    }

    #[test]
    fn tab_error_display_messages() {
        let e1 = TabError::IndexOutOfBounds { index: 5, count: 3 };
        assert!(e1.to_string().contains('5'));
        assert!(e1.to_string().contains('3'));

        let e2 = TabError::CannotCloseLastTab;
        assert!(e2.to_string().contains("last tab"));

        let e3 = TabError::MoveToSelf { index: 2 };
        assert!(e3.to_string().contains('2'));
    }

    // ── 36.8: Per-tab ViewState isolation ────────────────────────────────

    #[test]
    fn view_state_isolated_across_tab_switch() {
        let tab1 = dummy_tab(TabId(0), "Tab 1");
        let mut mgr = TabManager::new(tab1);
        let tab2 = dummy_tab(TabId(1), "Tab 2");
        mgr.add_tab(tab2);

        // Modify Tab 2's scroll offset.
        mgr.active_tab_mut().view_state.scroll_offset = 42;

        // Switch to Tab 1 — its scroll offset should still be 0 (default).
        mgr.switch_to(0).unwrap();
        assert_eq!(mgr.active_tab().view_state.scroll_offset, 0);

        // Switch back to Tab 2 — its scroll offset should still be 42.
        mgr.switch_to(1).unwrap();
        assert_eq!(mgr.active_tab().view_state.scroll_offset, 42);
    }

    #[test]
    fn view_state_preserved_after_close() {
        let tab1 = dummy_tab(TabId(0), "Tab 1");
        let mut mgr = TabManager::new(tab1);
        let tab2 = dummy_tab(TabId(1), "Tab 2");
        let tab3 = dummy_tab(TabId(2), "Tab 3");
        mgr.add_tab(tab2);
        mgr.add_tab(tab3);

        // Set distinct scroll offsets.
        mgr.switch_to(0).unwrap();
        mgr.active_tab_mut().view_state.scroll_offset = 10;
        mgr.switch_to(1).unwrap();
        mgr.active_tab_mut().view_state.scroll_offset = 20;
        mgr.switch_to(2).unwrap();
        mgr.active_tab_mut().view_state.scroll_offset = 30;

        // Close Tab 2 (index 1) — Tab 1 and Tab 3 should keep their offsets.
        mgr.close_tab(1).unwrap();
        // After close, Tab 3 slid to index 1 and is still active.
        assert_eq!(mgr.active_tab().view_state.scroll_offset, 30);
        mgr.switch_to(0).unwrap();
        assert_eq!(mgr.active_tab().view_state.scroll_offset, 10);
    }

    #[test]
    fn view_state_preserved_after_move() {
        let tab1 = dummy_tab(TabId(0), "A");
        let mut mgr = TabManager::new(tab1);
        let tab2 = dummy_tab(TabId(1), "B");
        let tab3 = dummy_tab(TabId(2), "C");
        mgr.add_tab(tab2);
        mgr.add_tab(tab3);

        // Set distinct scroll offsets.
        mgr.switch_to(0).unwrap();
        mgr.active_tab_mut().view_state.scroll_offset = 100;
        mgr.switch_to(1).unwrap();
        mgr.active_tab_mut().view_state.scroll_offset = 200;
        mgr.switch_to(2).unwrap();
        mgr.active_tab_mut().view_state.scroll_offset = 300;

        // Move C (index 2) to index 0.  Order becomes [C, A, B].
        mgr.move_tab(2, 0).unwrap();

        // Verify each tab still has its own scroll offset.
        mgr.switch_to(0).unwrap();
        assert_eq!(mgr.active_tab().title, "C");
        assert_eq!(mgr.active_tab().view_state.scroll_offset, 300);

        mgr.switch_to(1).unwrap();
        assert_eq!(mgr.active_tab().title, "A");
        assert_eq!(mgr.active_tab().view_state.scroll_offset, 100);

        mgr.switch_to(2).unwrap();
        assert_eq!(mgr.active_tab().title, "B");
        assert_eq!(mgr.active_tab().view_state.scroll_offset, 200);
    }

    #[test]
    fn new_tab_starts_with_default_view_state() {
        let tab1 = dummy_tab(TabId(0), "Tab 1");
        let mut mgr = TabManager::new(tab1);

        // Modify Tab 1's view state.
        mgr.active_tab_mut().view_state.scroll_offset = 999;

        // Add a new tab — it should have a fresh ViewState.
        let tab2 = dummy_tab(TabId(1), "Tab 2");
        mgr.add_tab(tab2);
        assert_eq!(mgr.active_tab().view_state.scroll_offset, 0);
    }
}
