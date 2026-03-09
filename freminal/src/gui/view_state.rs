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

use eframe::egui;

use super::mouse::PreviousMouseState;

/// GUI-local view state for the terminal widget.
///
/// Everything here belongs to the render thread only.  The PTY thread never
/// reads or writes any of these fields.
///
/// Fields are gradually populated as later tasks (5–8) migrate state out of
/// `TerminalEmulator` / `TerminalState`.  For now, only `scroll_offset` is
/// moved here (Task 4).  The remaining fields are stubs that will replace the
/// corresponding fields on `TerminalState` in Task 5.
#[derive(Debug, Default)]
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
}

impl ViewState {
    /// Create a new `ViewState` with all fields at their default (live-bottom)
    /// values.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}
