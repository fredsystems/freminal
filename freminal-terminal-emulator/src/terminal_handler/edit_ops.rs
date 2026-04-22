// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Edit and erase dispatch methods for [`TerminalHandler`].
//!
//! Covers erase-in-display, erase-in-line, insert/delete lines,
//! erase/delete characters, insert spaces, and repeat-character.

use super::TerminalHandler;
use crate::ansi_components::csi_commands::ed::EraseDisplayMode;
use crate::ansi_components::csi_commands::el::EraseLineMode;

impl TerminalHandler {
    /// Handle erase in display (ED)
    pub fn handle_erase_in_display(&mut self, mode: EraseDisplayMode) {
        match mode {
            EraseDisplayMode::CursorToEnd => self.buffer.erase_to_end_of_display(),
            EraseDisplayMode::StartToCursor => self.buffer.erase_to_beginning_of_display(),
            EraseDisplayMode::All => self.buffer.erase_display(),
            EraseDisplayMode::AllWithScrollback => self.buffer.erase_scrollback(),
        }
    }

    /// Handle erase in line (EL)
    pub fn handle_erase_in_line(&mut self, mode: EraseLineMode) {
        match mode {
            EraseLineMode::CursorToEnd => self.buffer.erase_line_to_end(),
            EraseLineMode::StartToCursor => self.buffer.erase_line_to_beginning(),
            EraseLineMode::All => self.buffer.erase_line(),
        }
    }

    /// Handle IL — insert `n` blank lines at the cursor row, pushing existing lines down (Insert Lines).
    pub fn handle_insert_lines(&mut self, n: usize) {
        self.buffer.insert_lines(n);
    }

    /// Handle DL — delete `n` lines starting at the cursor row, pulling lines below up (Delete Lines).
    pub fn handle_delete_lines(&mut self, n: usize) {
        self.buffer.delete_lines(n);
    }

    /// Handle ECH (Erase Characters) — erase `n` characters starting at the cursor column.
    pub fn handle_erase_chars(&mut self, n: usize) {
        self.buffer.erase_chars(n);
    }

    /// Handle DCH (Delete Characters) — delete `n` characters at the cursor column, shifting remaining characters left.
    pub fn handle_delete_chars(&mut self, n: usize) {
        self.buffer.delete_chars(n);
    }

    /// Handle ICH (Insert Characters) — insert `n` blank spaces at the cursor column, shifting existing characters right.
    pub fn handle_insert_spaces(&mut self, n: usize) {
        self.buffer.insert_spaces(n);
    }

    /// Handle REP (CSI Ps b) — repeat the last graphic character Ps times.
    pub(super) fn handle_repeat_character(&mut self, count: usize) {
        if let Some(ref ch) = self.last_graphic_char {
            let repeated = vec![*ch; count];
            self.buffer.insert_text(&repeated);
        }
    }
}
