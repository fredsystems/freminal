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

/// Largest repeat count REP (CSI Ps b) will honour.
///
/// xterm caps every CSI numeric parameter at `MAX_I_PARAM` (65535,
/// `ptyx.h`), so its effective REP maximum is 65535 repeats; `CASE_REP` in
/// `charproc.c` then prints the character through the normal text path.
/// Larger counts are clamped to this, as in xterm; unclamped, a hostile stream
/// could request an arbitrarily large allocation and amount of work
/// (GHSA-2746-x3cj-58c7).
const REP_MAX_COUNT: usize = 65_535;

/// Number of characters written per `Buffer::insert_text` call while
/// servicing REP. Bounds the working buffer, independent of the repeat count.
const REP_CHUNK_LEN: usize = 256;

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
    ///
    /// `count` is clamped to [`REP_MAX_COUNT`], and the repeats are written in
    /// fixed-size chunks of [`REP_CHUNK_LEN`] so memory use is O(chunk), never
    /// O(count). Successive `Buffer::insert_text` calls continue exactly where
    /// the previous one stopped (the cursor, including a pending wrap, is the
    /// only state carried between calls), so the result is identical to a
    /// single call with all `count` characters.
    pub(super) fn handle_repeat_character(&mut self, count: usize) {
        let Some(ch) = self.last_graphic_char else {
            return;
        };

        let chunk = [ch; REP_CHUNK_LEN];
        let mut remaining = count.min(REP_MAX_COUNT);
        while remaining > 0 {
            let n = remaining.min(REP_CHUNK_LEN);
            self.buffer.insert_text(&chunk[..n]);
            remaining -= n;
        }
    }
}
