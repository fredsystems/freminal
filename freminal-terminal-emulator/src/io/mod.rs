// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

mod pty;
pub use pty::FreminalPtyInputOutput;

// Re-export the shared PTY I/O types from freminal-common so that all crates
// can use the same definitions without creating a circular dependency.
pub use freminal_common::pty_write::{FreminalTerminalSize, PtyWrite};

use freminal_common::themes::ThemePalette;

pub struct PtyRead {
    pub buf: Vec<u8>,
    pub read_amount: usize,
}

/// Events sent from the GUI thread to the PTY processing thread.
///
/// The GUI sends these through a `crossbeam_channel::Sender<InputEvent>`.
/// The PTY thread receives them in its `select!` loop alongside `PtyRead`.
#[derive(Debug, Clone)]
pub enum InputEvent {
    /// Raw bytes to write to the PTY (keyboard input).
    Key(Vec<u8>),
    /// New terminal dimensions in character cells and font pixel size.
    ///
    /// Fields: `(width_chars, height_chars, font_pixel_width, font_pixel_height)`.
    /// The pixel dimensions are needed by the PTY consumer thread to build the
    /// correct `PtyWrite::Resize(FreminalTerminalSize { … })` payload.
    Resize(usize, usize, usize, usize),
    /// Window focus gained (`true`) or lost (`false`).
    FocusChange(bool),
    /// Desired scroll offset (rows from the bottom, 0 = live view).
    ///
    /// Sent by the GUI when the user scrolls up/down on the primary screen.
    /// The PTY thread stores this and uses it when building the next snapshot
    /// so `visible_as_tchars_and_tags(offset)` renders the correct window.
    ScrollOffset(usize),
    /// The user selected a new color theme in the Settings Modal.
    ///
    /// The PTY thread updates `handler.set_theme()` so subsequent snapshots
    /// carry the new palette. All embedded themes are `'static` so this is
    /// a zero-cost pointer update.
    ThemeChange(&'static ThemePalette),
    /// Request text extraction from the full buffer for clipboard copy.
    ///
    /// Coordinates are buffer-absolute row indices and 0-indexed columns.
    /// The PTY thread extracts the text and sends it back through a dedicated
    /// clipboard response channel.
    ExtractSelection {
        start_row: usize,
        start_col: usize,
        end_row: usize,
        end_col: usize,
    },
    /// A playback control command (mode selection, play/pause, next frame).
    ///
    /// Only sent when the application is running in playback mode.
    #[cfg(feature = "playback")]
    PlaybackControl(PlaybackCommand),
}

/// Playback mode selection.
///
/// Controls how a recorded terminal session is replayed.
#[cfg(feature = "playback")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackMode {
    /// Process the entire recording at once (no timing).
    Instant,
    /// Replay frames with their original inter-frame timing.
    RealTime,
    /// Advance one frame at a time on user command.
    FrameStepping,
}

/// Commands the GUI sends to control playback state.
#[cfg(feature = "playback")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackCommand {
    /// Select a playback mode (always pauses playback; user must press Play).
    SetMode(PlaybackMode),
    /// Begin or resume playback in the current mode.
    Play,
    /// Pause playback (only meaningful in `RealTime` mode).
    Pause,
    /// Advance one frame (only meaningful in `FrameStepping` mode).
    NextFrame,
}

/// Commands sent from the PTY processing thread to the GUI thread.
///
/// The PTY thread sends these through a `crossbeam_channel::Sender<WindowCommand>`
/// when the terminal program requests window-level operations.  The GUI drains
/// them at the start of each `update()` call via non-blocking `try_recv()`.
///
/// No call sites are wired up yet — this is a type definition only.
#[derive(Debug, Clone)]
pub enum WindowCommand {
    /// A viewport-level command that the GUI should execute via
    /// `ctx.send_viewport_cmd(…)`.
    Viewport(freminal_common::buffer_states::window_manipulation::WindowManipulation),
    /// A query that requires the GUI to read viewport geometry and write a
    /// response back to the PTY via its `Sender<PtyWrite>`.
    Report(freminal_common::buffer_states::window_manipulation::WindowManipulation),
}
