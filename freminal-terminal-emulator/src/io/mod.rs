// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

mod pty;
pub use pty::{FreminalPtyInputOutput, PtySpawnConfig};

// Re-export the shared PTY I/O types from freminal-common so that all crates
// can use the same definitions without creating a circular dependency.
pub use freminal_common::pty_write::{FreminalTerminalSize, PtyWrite};

use freminal_common::config::ThemeMode;
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
    /// Update the GUI-configured theme selection mode in the PTY thread.
    ///
    /// Sent at startup and whenever `ThemeConfig::mode` or the OS dark-mode
    /// preference changes.  The PTY thread stores this in `TerminalModes` so
    /// that DECRPM `?2031` responses reflect the correct locked/dynamic state.
    ///
    /// `os_is_dark` is the current OS dark/light preference; it is used to
    /// initialise `theming` when `mode` is `Auto`.
    ThemeModeUpdate(ThemeMode, bool),
    /// Request the full buffer content (scrollback + visible) for search.
    ///
    /// The PTY thread concatenates `scrollback_chars` and `visible_chars`
    /// into a single `Vec<TChar>` and sends it back through the dedicated
    /// search buffer response channel.  The GUI caches this and runs search
    /// across the complete history rather than just the visible window.
    RequestSearchBuffer,
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
        /// When `true` the selection is a rectangular block: every row from
        /// `start_row` to `end_row` is extracted between `start_col` and
        /// `end_col` (the same column range on each row).
        is_block: bool,
    },
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
