// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! `ViewState` — GUI-local terminal view state.
//!
//! This struct holds all state that is owned exclusively by the GUI render
//! loop and has no business being inside the `TerminalEmulator` or `Buffer`.
//! It is never shared with the PTY thread.
//!
//! See the architecture plan (`Documents/PERFORMANCE_PLAN.md`, Section 4.5).

use std::time::Instant;

use eframe::egui;

use super::mouse::PreviousMouseState;

/// Duration of one text-blink tick (~167 ms).
///
/// At this rate the 6-tick cycle completes in ~1 000 ms:
///   - Slow blink: visible on ticks 0-2 (501 ms), hidden on ticks 3-5 (501 ms) ≈ 1 Hz.
///   - Fast blink: visible on even ticks (0,2,4), hidden on odd ticks (1,3,5) ≈ 3 Hz.
pub const TEXT_BLINK_TICK_DURATION: std::time::Duration = std::time::Duration::from_millis(167);

/// A terminal cell coordinate (column, row), both 0-indexed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellCoord {
    pub col: usize,
    pub row: usize,
}

/// Tracks an in-progress or completed text selection.
///
/// Selection is defined by an *anchor* (where the mouse was pressed) and an
/// *end* (where the mouse currently is or was released).  The anchor stays
/// fixed; the end moves with the pointer.
///
/// The selection is "active" when a drag is in progress (`is_selecting`), and
/// "present" when anchor != end (i.e. something is highlighted).
#[derive(Debug, Clone, Default)]
pub struct SelectionState {
    /// The cell where the mouse button was pressed (fixed during drag).
    pub anchor: Option<CellCoord>,
    /// The cell where the pointer currently is (updated during drag).
    pub end: Option<CellCoord>,
    /// `true` while the primary button is held down and dragging.
    pub is_selecting: bool,
}

impl SelectionState {
    /// Returns the normalised selection range as `(start, end)` where `start`
    /// is always before or equal to `end` in reading order.
    ///
    /// Returns `None` if there is no selection.
    #[must_use]
    pub fn normalised(&self) -> Option<(CellCoord, CellCoord)> {
        let (a, e) = (self.anchor?, self.end?);
        if a.row < e.row || (a.row == e.row && a.col <= e.col) {
            Some((a, e))
        } else {
            Some((e, a))
        }
    }

    /// Clear the selection entirely.
    pub const fn clear(&mut self) {
        self.anchor = None;
        self.end = None;
        self.is_selecting = false;
    }

    /// Returns `true` if there is a visible selection (anchor and end differ).
    #[must_use]
    pub fn has_selection(&self) -> bool {
        match (self.anchor, self.end) {
            (Some(a), Some(e)) => a != e,
            _ => false,
        }
    }
}

/// GUI-local view state for the terminal widget.
///
/// Everything here belongs to the render thread only.  The PTY thread never
/// reads or writes any of these fields.
///
/// Fields are gradually populated as later tasks (5–8) migrate state out of
/// `TerminalEmulator` / `TerminalState`.  For now, only `scroll_offset` is
/// moved here (Task 4).  The remaining fields are stubs that will replace the
/// corresponding fields on `TerminalState` in Task 5.
#[derive(Debug)]
pub struct ViewState {
    /// How many lines the user has scrolled back from the live bottom.
    ///
    /// `0` = live bottom view (normal terminal mode).
    /// `> 0` = user is viewing older scrollback history.
    ///
    /// This field is the single source of truth for the scroll position.
    /// `Buffer` no longer stores or mutates it.  All `Buffer` methods that
    /// operate on visible rows accept this value as a parameter.
    pub scroll_offset: usize,

    /// The last mouse position reported to the terminal, if any.
    ///
    /// Moved here from `TerminalState::mouse_position` (Task 5).
    pub mouse_position: Option<egui::Pos2>,

    /// Whether the terminal window currently has keyboard focus.
    ///
    /// Moved here from `TerminalState::window_focused` (Task 5).
    pub window_focused: bool,

    /// The last `(width, height)` in character cells that was sent to the PTY
    /// as a resize.  Used to debounce resize events so we only send a new
    /// `InputEvent::Resize` when the size actually changes.
    ///
    /// Populated in Task 7.
    pub last_sent_size: (usize, usize),

    /// The most-recently pressed key, used to suppress auto-repeat on the
    /// first frame a key is held down.
    ///
    /// Moved here from the `write_input_to_terminal` call-chain (Task 5/9).
    pub previous_key: Option<egui::Key>,

    /// Accumulated scroll delta (in fractional lines) carried over between
    /// frames so sub-line scroll events are not lost.
    ///
    /// Moved here from the `write_input_to_terminal` call-chain (Task 5/9).
    pub previous_scroll_amount: f32,

    /// The mouse button / position / modifier state from the previous frame,
    /// used to detect button-state transitions and avoid sending redundant
    /// mouse reports to the PTY.
    ///
    /// Moved here from the `write_input_to_terminal` call-chain (Task 5/9).
    pub previous_mouse_state: Option<PreviousMouseState>,

    /// Current text selection state (anchor, end, `is_selecting`).
    ///
    /// Populated when the user clicks and drags with the primary mouse button
    /// while mouse tracking is off (no mouse-aware TUI application running).
    pub selection: SelectionState,

    // ── Text blink state ─────────────────────────────────────────────
    /// Current position in the 6-tick blink cycle (0–5).
    ///
    /// The cycle advances by one tick every ~167 ms when `has_blinking_text`
    /// is true in the snapshot.  At tick 6 it wraps back to 0.
    ///
    /// Slow blink visibility: `cycle < 3` → visible, `cycle >= 3` → hidden.
    /// Fast blink visibility: `cycle % 2 == 0` → visible, `cycle % 2 == 1` → hidden.
    pub text_blink_cycle: u8,

    /// Timestamp of the last blink-cycle tick.
    ///
    /// When the snapshot's `has_blinking_text` transitions from false to true,
    /// this is set to `Instant::now()` so the first visible phase starts
    /// immediately.
    pub text_blink_last_tick: Instant,

    /// Whether slow-blink text should currently be drawn (derived from cycle).
    pub text_blink_slow_visible: bool,

    /// Whether fast-blink text should currently be drawn (derived from cycle).
    pub text_blink_fast_visible: bool,
}

impl Default for ViewState {
    fn default() -> Self {
        Self {
            scroll_offset: 0,
            mouse_position: None,
            window_focused: false,
            last_sent_size: (0, 0),
            previous_key: None,
            previous_scroll_amount: 0.0,
            previous_mouse_state: None,
            selection: SelectionState::default(),
            text_blink_cycle: 0,
            text_blink_last_tick: Instant::now(),
            text_blink_slow_visible: true,
            text_blink_fast_visible: true,
        }
    }
}

impl ViewState {
    /// Create a new `ViewState` with all fields at their default (live-bottom)
    /// values.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Advance the text blink cycle if enough time has elapsed.
    ///
    /// Called from `update()` every frame when `has_blinking_text` is true.
    /// Returns `true` if the visibility flags changed (i.e. the caller should
    /// repaint).
    pub fn tick_text_blink(&mut self) -> bool {
        let now = Instant::now();
        let elapsed = now.duration_since(self.text_blink_last_tick);

        if elapsed < TEXT_BLINK_TICK_DURATION {
            return false;
        }

        // Advance by the number of ticks elapsed (normally 1, but could be
        // more if the frame rate dropped below the tick rate).
        #[allow(clippy::cast_possible_truncation)]
        let ticks = (elapsed.as_millis() / TEXT_BLINK_TICK_DURATION.as_millis()).min(6) as u8;
        self.text_blink_cycle = (self.text_blink_cycle + ticks) % 6;
        self.text_blink_last_tick = now;

        let old_slow = self.text_blink_slow_visible;
        let old_fast = self.text_blink_fast_visible;

        self.text_blink_slow_visible = self.text_blink_cycle < 3;
        self.text_blink_fast_visible = self.text_blink_cycle.is_multiple_of(2);

        old_slow != self.text_blink_slow_visible || old_fast != self.text_blink_fast_visible
    }

    /// Reset the text blink cycle to the beginning (both slow and fast
    /// visible).  Called when `has_blinking_text` transitions from false
    /// to true.
    pub fn reset_text_blink(&mut self) {
        self.text_blink_cycle = 0;
        self.text_blink_last_tick = Instant::now();
        self.text_blink_slow_visible = true;
        self.text_blink_fast_visible = true;
    }

    /// Compute the visibility for a given blink cycle value.
    ///
    /// Returns `(slow_visible, fast_visible)`.
    /// Useful for testing the mapping without an `Instant` dependency.
    #[must_use]
    pub const fn blink_visibility_for_cycle(cycle: u8) -> (bool, bool) {
        (cycle < 3, cycle.is_multiple_of(2))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn blink_cycle_slow_visibility() {
        // Cycles 0, 1, 2 → slow visible; 3, 4, 5 → slow hidden.
        let expected_slow = [true, true, true, false, false, false];
        for (cycle, &expected) in expected_slow.iter().enumerate() {
            #[allow(clippy::cast_possible_truncation)]
            let (slow, _) = ViewState::blink_visibility_for_cycle(cycle as u8);
            assert_eq!(slow, expected, "slow visibility at cycle {cycle}");
        }
    }

    #[test]
    fn blink_cycle_fast_visibility() {
        // Cycles 0, 2, 4 → fast visible; 1, 3, 5 → fast hidden.
        let expected_fast = [true, false, true, false, true, false];
        for (cycle, &expected) in expected_fast.iter().enumerate() {
            #[allow(clippy::cast_possible_truncation)]
            let (_, fast) = ViewState::blink_visibility_for_cycle(cycle as u8);
            assert_eq!(fast, expected, "fast visibility at cycle {cycle}");
        }
    }

    #[test]
    fn blink_cycle_combined_visibility() {
        // Verify combined (slow, fast) pairs for the full cycle.
        let expected = [
            (true, true),   // cycle 0: both visible
            (true, false),  // cycle 1: slow visible, fast hidden
            (true, true),   // cycle 2: both visible
            (false, false), // cycle 3: both hidden
            (false, true),  // cycle 4: slow hidden, fast visible
            (false, false), // cycle 5: both hidden
        ];
        for (cycle, &(slow, fast)) in expected.iter().enumerate() {
            #[allow(clippy::cast_possible_truncation)]
            let result = ViewState::blink_visibility_for_cycle(cycle as u8);
            assert_eq!(result, (slow, fast), "visibility at cycle {cycle}");
        }
    }

    #[test]
    fn reset_text_blink_restores_defaults() {
        let mut vs = ViewState::new();
        vs.text_blink_cycle = 4;
        vs.text_blink_slow_visible = false;
        vs.text_blink_fast_visible = false;

        vs.reset_text_blink();

        assert_eq!(vs.text_blink_cycle, 0);
        assert!(vs.text_blink_slow_visible);
        assert!(vs.text_blink_fast_visible);
    }

    #[test]
    fn tick_does_not_advance_when_insufficient_time() {
        let mut vs = ViewState::new();
        // Just created — last_tick is now(), so elapsed < 167ms.
        let changed = vs.tick_text_blink();
        assert!(!changed, "should not advance within the first tick window");
        assert_eq!(vs.text_blink_cycle, 0);
        assert!(vs.text_blink_slow_visible);
        assert!(vs.text_blink_fast_visible);
    }

    #[test]
    fn tick_advances_after_elapsed_time() {
        let mut vs = ViewState::new();
        // Artificially set last_tick to the past so the next tick will fire.
        vs.text_blink_last_tick = Instant::now()
            .checked_sub(TEXT_BLINK_TICK_DURATION)
            .unwrap();

        let changed = vs.tick_text_blink();
        // cycle goes from 0 → 1.  slow stays true, fast changes true → false.
        assert!(changed, "visibility should change at cycle 0 → 1");
        assert_eq!(vs.text_blink_cycle, 1);
        assert!(vs.text_blink_slow_visible); // cycle 1 < 3
        assert!(!vs.text_blink_fast_visible); // cycle 1 % 2 != 0
    }

    #[test]
    fn new_view_state_starts_both_visible() {
        let vs = ViewState::new();
        assert!(vs.text_blink_slow_visible);
        assert!(vs.text_blink_fast_visible);
        assert_eq!(vs.text_blink_cycle, 0);
    }
}
