// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Scroll, margin, line-feed, and resize dispatch for [`TerminalHandler`].
//!
//! Covers scroll up/down, DECSTBM/DECSLRM margins, IND/RI/NEL, LF/BS/TAB,
//! alternate screen entry/exit, scroll-back offset helpers, and terminal resize.

use conv2::ValueFrom;
use freminal_common::{
    buffer_states::{
        buffer_type::BufferType,
        modes::{
            decawm::Decawm, declrmm::Declrmm, in_band_resize_mode::InBandResizeMode, lnm::Lnm,
        },
    },
    pty_write::{FreminalTerminalSize, PtyWrite},
    send_or_log,
};

use freminal_buffer::buffer::Buffer;

use super::TerminalHandler;

/// Which DEC private mode drives an alternate-screen transition.
///
/// The three modes share the screen switch but differ in what accompanies it;
/// see [`TerminalHandler::handle_alternate_screen`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AltScreenMode {
    /// `?47` -- switch only.
    Legacy47,
    /// `?1047` -- switch; clear the alternate screen on leave.
    Clearing1047,
    /// `?1049` -- DECSC, switch and clear on enter; switch and DECRC on leave.
    SaveClear1049,
}

/// Direction of an alternate-screen transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AltScreenAction {
    /// Set the mode (`CSI ? Pm h`).
    Enter,
    /// Reset the mode (`CSI ? Pm l`).
    Leave,
}

impl TerminalHandler {
    /// Handle LF (Line Feed) — advance cursor to the next line, scrolling if needed.
    pub fn handle_newline(&mut self) {
        self.buffer.handle_lf();
    }

    /// Handle CR (Carriage Return) — move cursor to column 0 of the current row.
    pub const fn handle_carriage_return(&mut self) {
        self.buffer.handle_cr();
    }

    /// Handle BS (Backspace) — move cursor one column to the left, respecting reverse-wrap modes.
    pub fn handle_backspace(&mut self) {
        self.buffer
            .handle_backspace(self.reverse_wrap, self.xt_rev_wrap2);
    }

    /// Handle HT (Horizontal Tab) — advance cursor to the next tab stop.
    pub fn handle_tab(&mut self) {
        self.buffer.advance_to_next_tab_stop();
    }

    /// Handle set top and bottom margins (DECSTBM)
    ///
    /// `top` and `bottom` are **1-based inclusive** row numbers, exactly as the
    /// ANSI parser delivers them.  `Buffer::set_scroll_region` already converts
    /// 1-based → 0-based internally, so we must NOT subtract here.
    pub fn handle_set_scroll_region(&mut self, top: usize, bottom: usize) {
        self.buffer.set_scroll_region(top, bottom);
    }

    /// Set DECSLRM left/right margins.
    ///
    /// `left` and `right` are **1-based inclusive** column numbers as delivered
    /// by the parser.  Only effective when DECLRMM (`?69`) is active.
    pub fn handle_set_left_right_margins(&mut self, left: usize, right: usize) {
        if self.buffer.is_declrmm_enabled() == Declrmm::Enabled {
            self.buffer.set_left_right_margins(left, right);
        }
    }

    /// Handle IND — Index: move cursor down one row, scrolling the scroll region up if at the bottom margin.
    pub fn handle_index(&mut self) {
        self.buffer.handle_ind();
    }

    /// Handle RI — Reverse Index: move cursor up one row, scrolling the scroll region down if at the top margin.
    pub fn handle_reverse_index(&mut self) {
        self.buffer.handle_ri();
    }

    /// Handle NEL — Next Line: perform a carriage return followed by an index (move to start of next line).
    pub fn handle_next_line(&mut self) {
        self.buffer.handle_nel();
    }

    /// Handle SU — Scroll Up `n` lines within the scroll region.
    /// Content moves up; blank lines appear at the bottom of the region.
    pub fn handle_scroll_up(&mut self, n: usize) {
        self.buffer.scroll_region_up_n(n);
    }

    /// Handle SD — Scroll Down `n` lines within the scroll region.
    /// Content moves down; blank lines appear at the top of the region.
    pub fn handle_scroll_down(&mut self, n: usize) {
        self.buffer.scroll_region_down_n(n);
    }

    /// Handle an alternate-screen mode transition.
    ///
    /// The three DEC modes differ in what accompanies the screen switch, and
    /// follow the consensus of xterm, Ghostty, `WezTerm` and kitty:
    ///
    /// | Mode / action | Effect |
    /// | --- | --- |
    /// | `?47` enter | switch to the alternate screen; nothing else |
    /// | `?47` leave | switch to the primary screen; nothing else |
    /// | `?1047` enter | same as `?47` enter |
    /// | `?1047` leave | if on the alternate screen, clear it, then switch to primary |
    /// | `?1049` enter | DECSC on the *current* screen, switch to alternate, clear it |
    /// | `?1049` leave | switch to primary, then DECRC unconditionally |
    ///
    /// Every switch is idempotent and keeps the cursor's screen position and
    /// attributes; the scroll margins are shared and never touched.  The
    /// alternate screen's contents persist across sessions: only a `?1049`
    /// enter and a `?1047` leave clear them.  Because `?1049` enter saves on
    /// the current screen, a second `?1049h` while already on the alternate
    /// screen re-saves into the alternate screen's DECSC slot (the primary's
    /// save is untouched) and clears the alternate screen again.  `?1049l`
    /// while already on the primary screen performs a DECRC from the primary
    /// slot.
    ///
    /// Whenever this actually moves from the alternate screen to the primary
    /// one, the kitty real-placement map is filtered of alternate-screen
    /// placements and re-pruned (replaced by per-screen maps in 131.7).
    pub(super) fn handle_alternate_screen(&mut self, mode: AltScreenMode, action: AltScreenAction) {
        match (mode, action) {
            (AltScreenMode::Legacy47 | AltScreenMode::Clearing1047, AltScreenAction::Enter) => {
                self.buffer.switch_to_alternate();
            }
            (AltScreenMode::Legacy47, AltScreenAction::Leave) => {
                self.switch_to_primary_screen();
            }
            (AltScreenMode::Clearing1047, AltScreenAction::Leave) => {
                // `clear_alternate_screen` is only valid while the alternate
                // screen is active; leaving from the primary is a no-op.
                if self.buffer.kind() == BufferType::Alternate {
                    self.buffer.clear_alternate_screen();
                    self.switch_to_primary_screen();
                }
            }
            (AltScreenMode::SaveClear1049, AltScreenAction::Enter) => {
                self.handle_save_cursor();
                self.buffer.switch_to_alternate();
                self.buffer.clear_alternate_screen();
            }
            (AltScreenMode::SaveClear1049, AltScreenAction::Leave) => {
                self.switch_to_primary_screen();
                self.handle_restore_cursor();
            }
        }
    }

    /// Switch to the primary screen (idempotent) and, if the alternate
    /// screen was actually active, drop the placements recorded against it.
    fn switch_to_primary_screen(&mut self) {
        let was_alternate = self.buffer.kind() == BufferType::Alternate;
        self.buffer.switch_to_primary();
        if was_alternate {
            // The alternate screen's placements are dropped on leave (the
            // buffer drops its own alternate-screen marks the same way).
            self.real_placements
                .retain(|_, placement| !placement.origin_row.is_alternate());
            // A resize while the alternate screen was up may have trimmed the
            // parked primary store's tail; judge the primary placements
            // against the primary store now that it is active again.
            self.prune_unissued_real_placements();
        }
    }

    /// Handle DECAWM — enable or disable soft-wrapping.
    pub const fn handle_set_wrap(&mut self, mode: Decawm) {
        self.buffer.set_wrap(mode);
    }

    /// Handle LNM — enable or disable Line Feed Mode.
    pub const fn handle_set_lnm(&mut self, mode: Lnm) {
        self.buffer.set_lnm(mode);
    }

    /// Compute new `scroll_offset` after scrolling back by `lines`.
    ///
    /// The caller must pass the current offset and store the returned value
    /// into `ViewState::scroll_offset`.
    #[must_use]
    pub fn handle_scroll_back(&self, scroll_offset: usize, lines: usize) -> usize {
        self.buffer.scroll_back(scroll_offset, lines)
    }

    /// Compute new `scroll_offset` after scrolling forward by `lines`.
    ///
    /// The caller must pass the current offset and store the returned value
    /// into `ViewState::scroll_offset`.
    #[must_use]
    pub fn handle_scroll_forward(&self, scroll_offset: usize, lines: usize) -> usize {
        self.buffer.scroll_forward(scroll_offset, lines)
    }

    /// Returns 0 — the scroll offset for the live bottom view.
    ///
    /// The caller should store this into `ViewState::scroll_offset`.
    #[must_use]
    pub const fn handle_scroll_to_bottom() -> usize {
        Buffer::scroll_to_bottom()
    }

    /// Resize the terminal grid to `width` × `height` characters.
    ///
    /// Also updates the stored pixel-per-cell dimensions used for building
    /// `PtyWrite::Resize` payloads.  Zero values for the pixel dimensions are
    /// ignored (the stored value is not overwritten).
    ///
    /// `scroll_offset` is **always `0`** here — it is owned by `ViewState` on
    /// the GUI side.  The PTY thread never holds a scroll offset.  `set_size`
    /// returns the post-reflow offset (which may differ when scrollback rows
    /// are removed), but we discard it because the GUI's `ViewState` will
    /// clamp its own offset the next time it sends a snapshot request.
    ///
    /// The underlying `Buffer::set_size` call triggers `reflow_to_width` when
    /// the column count changes, and adjusts the row count by appending blank
    /// rows or truncating from the live bottom when the height changes.
    pub fn handle_resize(
        &mut self,
        width: usize,
        height: usize,
        cell_pixel_width: u32,
        cell_pixel_height: u32,
    ) {
        let (old_width, old_height) = self.win_size();

        if cell_pixel_width > 0 {
            self.cell_pixel_width = cell_pixel_width;
        }
        if cell_pixel_height > 0 {
            self.cell_pixel_height = cell_pixel_height;
        }
        // scroll_offset is owned by ViewState on the GUI side; the PTY thread
        // always passes 0 when resizing.
        let _new_offset = self.buffer.set_size(width, height, 0);
        // A width change reflows the buffer; carry the row numbers the
        // handler itself holds (kitty placements) through the same remap.
        self.apply_buffer_reflow_remap();

        if self.in_band_resize_enabled == InBandResizeMode::Set
            && (old_width != width || old_height != height)
        {
            self.send_in_band_resize();
        }
    }

    /// Send an in-band resize notification to the PTY.
    /// Format: `CSI 48 ; height_chars ; width_chars ; height_pixels ; width_pixels t`
    pub(super) fn send_in_band_resize(&self) {
        let (width, height) = self.win_size();
        let Ok(width_u32) = u32::value_from(width) else {
            return;
        };
        let Ok(height_u32) = u32::value_from(height) else {
            return;
        };
        let px_w = width_u32 * self.cell_pixel_width;
        let px_h = height_u32 * self.cell_pixel_height;
        self.write_csi_response(&format!("48;{height_u32};{width_u32};{px_h};{px_w}t"));
    }

    /// Notify the PTY of a column-mode resize (DECCOLM).
    ///
    /// Sends a `PtyWrite::Resize` with the new width and the current height.
    /// Pixel dimensions are set to 0 — the PTY thread will use the character
    /// dimensions to compute the actual pixel size.
    pub(super) fn send_pty_resize(&self, new_width: usize) {
        let height = self.buffer.terminal_height();
        if let Some(tx) = &self.write_tx {
            let size = FreminalTerminalSize {
                width: new_width,
                height,
                pixel_width: 0,
                pixel_height: 0,
            };
            send_or_log!(tx, PtyWrite::Resize(size), "Failed to send PTY resize");
        }
    }
}
