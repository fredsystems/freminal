// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Cursor positioning and visual style operations for [`TerminalHandler`].
//!
//! Contains all cursor movement, save/restore, position reporting, and
//! cursor visual style (blink/shape) handler methods.

use conv2::ValueFrom;
use freminal_common::{
    buffer_states::modes::{decanm::Decanm, decom::Decom, dectcem::Dectcem, xtcblink::XtCBlink},
    cursor::CursorVisualStyle,
};

use super::TerminalHandler;

impl TerminalHandler {
    /// Handle cursor position (CUP, HVP).
    ///
    /// `x` and `y` are 1-indexed (from the parser).  `None` means "leave this
    /// axis unchanged" (e.g. CHA supplies only `x`).
    ///
    /// **VT52 out-of-bounds row rule** — When the terminal is in VT52
    /// compatibility mode (`Decanm::Vt52`) and the supplied row index exceeds
    /// the screen height, the row coordinate is silently ignored and only the
    /// column is updated.  This matches the behaviour documented in the vttest
    /// source (`vt52.c`, lines 94-107): `vt52cup(max_lines+3, i-1)` is used
    /// deliberately to update only the column.
    pub fn handle_cursor_pos(&mut self, x: Option<usize>, y: Option<usize>) {
        // In VT52 mode, out-of-bounds coordinates are ignored (the axis is
        // left unchanged) rather than clamped.  This matches VT100-emulating-
        // VT52 behaviour and is relied upon by vttest's box-drawing test.
        let (x_zero, y_zero) = if self.vt52_mode == Decanm::Vt52 {
            let x_z = x.and_then(|col_1indexed| {
                if col_1indexed > self.buffer.terminal_width() {
                    None // out-of-bounds — ignore column, keep current position
                } else {
                    Some(col_1indexed.saturating_sub(1))
                }
            });
            let y_z = y.and_then(|row_1indexed| {
                if row_1indexed > self.buffer.terminal_height() {
                    None // out-of-bounds — ignore row, keep current position
                } else {
                    Some(row_1indexed.saturating_sub(1))
                }
            });
            (x_z, y_z)
        } else {
            (
                x.map(|v| v.saturating_sub(1)),
                y.map(|v| v.saturating_sub(1)),
            )
        };

        self.buffer.set_cursor_pos(x_zero, y_zero);
    }

    /// Move the cursor by `(dx, dy)` cells relative to its current position.
    pub fn handle_cursor_relative(&mut self, dx: i32, dy: i32) {
        self.buffer.move_cursor_relative(dx, dy);
    }

    /// Handle CUU (Cursor Up) — move cursor up `n` rows.
    pub fn handle_cursor_up(&mut self, n: usize) {
        let dy = i32::value_from(n).unwrap_or(i32::MAX);
        self.buffer.move_cursor_relative(0, -dy);
    }

    /// Handle CUD (Cursor Down) — move cursor down `n` rows.
    pub fn handle_cursor_down(&mut self, n: usize) {
        let dy = i32::value_from(n).unwrap_or(i32::MAX);
        self.buffer.move_cursor_relative(0, dy);
    }

    /// Handle CUF (Cursor Forward) — move cursor forward `n` columns.
    pub fn handle_cursor_forward(&mut self, n: usize) {
        let dx = i32::value_from(n).unwrap_or(i32::MAX);
        self.buffer.move_cursor_relative(dx, 0);
    }

    /// Handle CUB (Cursor Backward) — move cursor backward `n` columns.
    pub fn handle_cursor_backward(&mut self, n: usize) {
        let dx = i32::value_from(n).unwrap_or(i32::MAX);
        self.buffer.move_cursor_relative(-dx, 0);
    }

    /// Handle DECSC — save the current cursor position, SGR state, and character set.
    pub fn handle_save_cursor(&mut self) {
        self.buffer.save_cursor();
        self.saved_character_replace = Some(self.character_replace.clone());
    }

    /// Handle DECRC — restore the cursor position, SGR state, and character set saved by the most recent DECSC.
    pub fn handle_restore_cursor(&mut self) {
        self.buffer.restore_cursor();
        if let Some(saved) = &self.saved_character_replace {
            self.character_replace = saved.clone();
        }
    }

    /// Handle CPR — Cursor Position Report.
    /// Responds with `CSI <row> ; <col> R` (1-indexed).
    ///
    /// Per DEC VT510: when DECOM is enabled, the reported row is relative to the
    /// scroll region top margin.  When DECOM is disabled, it is relative to the
    /// screen origin.
    pub fn handle_cursor_report(&mut self) {
        let screen_pos = self.buffer.cursor_screen_pos();
        let x = screen_pos.x + 1;
        let y = if self.buffer.is_decom_enabled() == Decom::OriginMode {
            let (region_top, _) = self.buffer.scroll_region();
            screen_pos.y.saturating_sub(region_top) + 1
        } else {
            screen_pos.y + 1
        };
        let body = format!("{y};{x}R");
        self.write_csi_response(&body);
    }

    /// Return `true` when the cursor should be painted.
    #[must_use]
    pub const fn show_cursor(&self) -> bool {
        matches!(self.show_cursor, Dectcem::Show)
    }

    /// Return the current cursor shape / blink style.
    #[must_use]
    pub fn cursor_visual_style(&self) -> CursorVisualStyle {
        self.cursor_visual_style.clone()
    }

    /// Apply an `XtCBlink` blink-mode change to the current `cursor_visual_style`.
    ///
    /// Flips between the blinking and steady variants of whichever shape is active,
    /// matching the behaviour of the old buffer's `set_mode` handler.
    pub(super) fn apply_xtcblink(&mut self, blink: &XtCBlink) {
        match blink {
            XtCBlink::Blinking => {
                self.cursor_visual_style = match self.cursor_visual_style {
                    CursorVisualStyle::BlockCursorSteady => CursorVisualStyle::BlockCursorBlink,
                    CursorVisualStyle::UnderlineCursorSteady => {
                        CursorVisualStyle::UnderlineCursorBlink
                    }
                    CursorVisualStyle::VerticalLineCursorSteady => {
                        CursorVisualStyle::VerticalLineCursorBlink
                    }
                    // Already blinking — leave unchanged.
                    ref other => other.clone(),
                };
            }
            XtCBlink::Steady => {
                self.cursor_visual_style = match self.cursor_visual_style {
                    CursorVisualStyle::BlockCursorBlink => CursorVisualStyle::BlockCursorSteady,
                    CursorVisualStyle::UnderlineCursorBlink => {
                        CursorVisualStyle::UnderlineCursorSteady
                    }
                    CursorVisualStyle::VerticalLineCursorBlink => {
                        CursorVisualStyle::VerticalLineCursorSteady
                    }
                    // Already steady — leave unchanged.
                    ref other => other.clone(),
                };
            }
            // Query is handled at the Mode dispatch level, not here.
            XtCBlink::Query => {}
        }
    }
}
