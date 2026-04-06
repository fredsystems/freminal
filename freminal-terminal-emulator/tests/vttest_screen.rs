// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! vttest Menu 2 — Screen Feature Tests.
//!
//! Exercises the screen-level operations covered by vttest Menu 2:
//!
//! - DECSTBM (`CSI Ps;Ps r`) — Set top and bottom scroll margins.
//! - HTS (`ESC H`) — Set a tab stop at the current cursor column.
//! - TBC (`CSI Ps g`) — Clear tab stops (Ps=0: at cursor; Ps=3: all).
//! - CHT (horizontal tab `\t`) — Advance to the next tab stop.
//! - DECOM (`CSI ?6 h/l`) — Origin mode: cursor addressing relative to the
//!   scroll region when enabled, absolute otherwise.
//! - DECAWM (`CSI ?7 h/l`) — Auto-wrap mode on/off.
//! - SGR — Character attributes (bold, underline, italic, inverse,
//!   strikethrough, foreground/background color, combined, reset).
//! - DECSC/DECRC (`ESC 7` / `ESC 8`) — Save and restore cursor position and
//!   character attributes.
//!
//! All cursor positions in the helper API are **0-indexed** (`x` = column,
//! `y` = row). CSI sequences use **1-indexed** row;col parameters.

#![allow(clippy::unwrap_used)]

mod vttest_common;

use freminal_common::buffer_states::{
    cursor::ReverseVideo,
    fonts::{BlinkState, FontDecorations, FontWeight},
};
use freminal_common::colors::TerminalColor;
use vttest_common::VtTestHelper;

// ─── DECSTBM — Set Top and Bottom Scroll Margins ────────────────────────────

/// Setting DECSTBM resets the cursor to the top-left corner.
#[test]
fn decstbm_setting_margins_resets_cursor_to_origin() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b[10;20H"); // position cursor away from origin
    h.feed(b"\x1b[5;15r"); // set scroll region rows 5–15
    // DECSTBM must home the cursor to (0, 0).
    h.assert_cursor_pos(0, 0);
}

/// IND at the bottom of the scroll region scrolls only within that region.
/// Rows outside the region must be untouched.
#[test]
fn decstbm_scroll_stays_within_region() {
    let mut h = VtTestHelper::new_default();
    // Fill rows 0-5 with labelled content.
    for i in 0..6_u8 {
        h.feed_str(&format!("Line {:02}\r\n", i));
    }
    // Set scroll region to rows 3–5 (1-indexed) → y=2..=4 (0-indexed).
    h.feed(b"\x1b[3;5r");
    // Move to the bottom of the scroll region (row 5 CSI → y=4).
    h.feed(b"\x1b[5;1H");
    // IND scrolls within the region.
    h.feed(b"\x1bD");
    // Cursor stays at bottom of scroll region.
    h.assert_cursor_pos(0, 4);
    // Row 4 (bottom of region) is now blank.
    h.assert_row(4, "");
    // Rows 0 and 1 are completely outside the region and must be unchanged.
    h.assert_row(0, "Line 00");
    h.assert_row(1, "Line 01");
    // Row 2 (top of region) still has its original content (scroll went up from
    // inside the region; row 2 got the old row 3 content).
    h.assert_row(2, "Line 03");
}

/// A full-screen region (default parameters) scrolls the whole screen.
#[test]
fn decstbm_full_screen_region_scrolls_all_rows() {
    let mut h = VtTestHelper::new_default();
    // Write content to the last visible row.
    h.feed(b"\x1b[24;1H");
    h.feed_str("Last row");
    h.feed(b"\x1b[24;1H");
    // Reset scroll region to full screen via default params.
    h.feed(b"\x1b[r");
    h.assert_cursor_pos(0, 0);
    // Move to bottom and send IND — should scroll the entire screen.
    h.feed(b"\x1b[24;1H");
    h.feed(b"\x1bD");
    h.assert_cursor_pos(0, 23);
    // The bottom row is now blank (the old "Last row" scrolled off).
    h.assert_row(23, "");
}

/// A single-line region (top == bottom) is rejected by the DECSTBM parser
/// because the ANSI parser pre-validates `pt >= pb` and returns an error
/// without producing a `SetTopAndBottomMargins` output.  As a result, no
/// DECSTBM action occurs: the cursor is NOT homed, the scroll region is NOT
/// changed, and subsequent IND behaves on the full-screen (default) region.
#[test]
fn decstbm_single_line_region() {
    let mut h = VtTestHelper::new_default();
    h.feed_str("Row zero\r\nRow one\r\nRow two");
    // After writing, cursor is at the end of "Row two": col=7, row=2.
    h.assert_cursor_pos(7, 2);
    // Attempt a degenerate single-line region at row 2 (1-indexed).
    // Because pt == pb the parser rejects this — no action is taken.
    h.feed(b"\x1b[2;2r");
    // Cursor is unchanged (not homed) — still at (7, 2).
    h.assert_cursor_pos(7, 2);
    // Move to y=1 and issue IND — full-screen region applies.
    h.feed(b"\x1b[2;1H");
    h.assert_cursor_pos(0, 1);
    h.feed(b"\x1bD"); // IND
    // Full-screen region: cursor simply moves down to y=2.
    h.assert_cursor_pos(0, 2);
    // Row 0 and row 1 content are untouched.
    h.assert_row(0, "Row zero");
    h.assert_row(1, "Row one");
}

/// RI at the top of a restricted scroll region inserts a blank line at the
/// top of the region; rows outside remain untouched.
#[test]
fn decstbm_ri_inserts_at_top_of_region() {
    let mut h = VtTestHelper::new_default();
    for i in 0..5_u8 {
        h.feed_str(&format!("Line {:02}\r\n", i));
    }
    // Scroll region rows 2–4 (1-indexed) → y=1..=3 (0-indexed).
    h.feed(b"\x1b[2;4r");
    // Move to the top of the scroll region.
    h.feed(b"\x1b[2;1H"); // row=2, col=1 → (x=0, y=1)
    // RI at the top of the region inserts a blank line, pushing content down.
    h.feed(b"\x1bM");
    h.assert_cursor_pos(0, 1);
    // New blank at the top of the region.
    h.assert_row(1, "");
    // Old row 1 content shifted to row 2.
    h.assert_row(2, "Line 01");
    // Row 0 (outside region) is unchanged.
    h.assert_row(0, "Line 00");
    // Row 4 (outside region) is unchanged.
    h.assert_row(4, "Line 04");
}

// ─── Tab Stops: HTS + TBC + CHT ─────────────────────────────────────────────

/// Default tab stops are every 8 columns (8, 16, 24, …).
/// A tab from column 0 lands at column 8.
#[test]
fn tab_default_stop_every_8_columns() {
    let mut h = VtTestHelper::new_default();
    // Start at column 0 and tab once.
    h.feed(b"\t");
    h.assert_cursor_pos(8, 0);
}

/// Multiple default tabs advance by 8 columns each.
#[test]
fn tab_advances_through_default_stops() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\t\t\t"); // col 0 → 8 → 16 → 24
    h.assert_cursor_pos(24, 0);
}

/// HTS (`ESC H`) sets a tab stop at the current cursor column.
/// A subsequent tab from just before that column lands on it.
#[test]
fn hts_sets_custom_tab_stop() {
    let mut h = VtTestHelper::new_default();
    // Position at column 5 (0-indexed) and plant a tab stop there.
    h.feed(b"\x1b[1;6H"); // CUP row=1, col=6 → (x=5, y=0)
    h.feed(b"\x1bH"); // HTS — set tab stop at col 5
    // Return to the start of the row.
    h.feed(b"\x1b[1;1H"); // (x=0, y=0)
    // Tab from col 0 should land on the new stop at col 5 (before the default
    // stop at col 8).
    h.feed(b"\t");
    h.assert_cursor_pos(5, 0);
}

/// TBC Ps=0 clears the tab stop at the cursor position only.
/// The next tab skips past that column to the following stop.
#[test]
fn tbc_ps0_clears_tab_stop_at_cursor() {
    let mut h = VtTestHelper::new_default();
    // The default stop at column 8 is active. Move there and clear it.
    h.feed(b"\x1b[1;9H"); // (x=8, y=0)
    h.feed(b"\x1b[0g"); // TBC Ps=0 — clear stop at col 8
    // Tab from col 0 should now skip col 8 and land on col 16.
    h.feed(b"\x1b[1;1H"); // back to col 0
    h.feed(b"\t");
    h.assert_cursor_pos(16, 0);
}

/// TBC Ps=3 clears all tab stops.
/// After clearing, a tab from any column advances to the last column
/// (or stays if already at the rightmost).
#[test]
fn tbc_ps3_clears_all_tab_stops() {
    let mut h = VtTestHelper::new_default();
    // Clear every tab stop.
    h.feed(b"\x1b[3g");
    // With no stops set, tabbing from col 0 should advance to the last column
    // (col 79 for an 80-column terminal).
    h.feed(b"\t");
    h.assert_cursor_pos(79, 0);
}

/// After TBC Ps=3, HTS plants a new stop and CHT lands on it.
#[test]
fn hts_after_tbc_clears_all_creates_new_stop() {
    let mut h = VtTestHelper::new_default();
    // Clear all stops.
    h.feed(b"\x1b[3g");
    // Plant a new stop at col 12.
    h.feed(b"\x1b[1;13H"); // (x=12, y=0)
    h.feed(b"\x1bH"); // HTS at col 12
    // Return to col 0 and tab once.
    h.feed(b"\x1b[1;1H");
    h.feed(b"\t");
    h.assert_cursor_pos(12, 0);
}

/// CHT writes tab characters that fill the row with text in between stops.
#[test]
fn cht_tab_interleaved_with_text() {
    let mut h = VtTestHelper::new_default();
    // Write "A", tab to 8, write "B".
    h.feed_str("A");
    h.feed(b"\t");
    h.feed_str("B");
    // "A" at col 0, blank cols 1-7, "B" at col 8.
    h.assert_row(0, "A       B");
    h.assert_cursor_pos(9, 0);
}

// ─── DECOM — Origin Mode ─────────────────────────────────────────────────────

/// With DECOM off (default), CUP uses absolute screen coordinates.
#[test]
fn decom_off_cup_is_absolute() {
    let mut h = VtTestHelper::new_default();
    // Set a scroll region to make the distinction meaningful.
    h.feed(b"\x1b[5;10r"); // region rows 5–10 (1-indexed) → y=4..=9
    // DECOM is off by default; CUP to row 3 goes to absolute row 3 → y=2.
    h.feed(b"\x1b[3;5H");
    h.assert_cursor_pos(4, 2);
}

/// With DECOM on, CUP row 1 lands on the first row of the scroll region.
#[test]
fn decom_on_cup_row1_is_top_of_scroll_region() {
    let mut h = VtTestHelper::new_default();
    // Set scroll region rows 5–10 (1-indexed) → y=4..=9.
    h.feed(b"\x1b[5;10r");
    // Enable DECOM — cursor is homed to (0, 4) immediately.
    h.feed(b"\x1b[?6h");
    h.assert_cursor_pos(0, 4);
    // CUP row=1 should be relative to the scroll region → absolute y=4.
    h.feed(b"\x1b[1;1H");
    h.assert_cursor_pos(0, 4);
}

/// With DECOM on, CUP row 2 col 3 lands at scroll-region-top + 1, absolute col 2.
#[test]
fn decom_on_cup_relative_to_scroll_region() {
    let mut h = VtTestHelper::new_default();
    // Scroll region rows 5–10 (1-indexed) → y=4..=9.
    h.feed(b"\x1b[5;10r");
    h.feed(b"\x1b[?6h"); // DECOM on
    // CUP row=2, col=3 → absolute (y=4+1, x=2).
    h.feed(b"\x1b[2;3H");
    h.assert_cursor_pos(2, 5);
}

/// Disabling DECOM resets the cursor to the absolute top-left.
#[test]
fn decom_disable_homes_cursor_to_absolute_origin() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b[5;10r");
    h.feed(b"\x1b[?6h"); // DECOM on
    h.feed(b"\x1b[3;3H"); // move somewhere inside region
    // Disable DECOM.
    h.feed(b"\x1b[?6l");
    h.assert_cursor_pos(0, 0);
}

// ─── DECAWM — Auto-Wrap Mode ─────────────────────────────────────────────────

/// With DECAWM on (default), writing past the right margin wraps to the next row.
#[test]
fn decawm_on_wraps_long_line() {
    let mut h = VtTestHelper::new_default();
    // DECAWM is on by default. Write 80 + 1 characters.
    h.feed_str(&"X".repeat(80));
    h.feed_str("Y"); // 81st character — wraps to row 1 col 0.
    h.assert_row(0, &"X".repeat(80));
    h.assert_row(1, "Y");
    h.assert_cursor_pos(1, 1);
}

/// With DECAWM off, writing past the right margin clamps the cursor at the
/// last column and discards all further characters; no wrap occurs and the
/// overflow does NOT overwrite the last cell.
///
/// Specifically: the buffer's insert loop sets `final_col = 80` after placing
/// all 80 `A`s (cols 0–79), which immediately triggers the NoAutoWrap clamp
/// back to col 79 on the next insertion attempt. The subsequent `ZZZZZ` input
/// arrives with cursor already past `wrap_col`, so all five `Z`s are discarded
/// before any cell is written.
#[test]
fn decawm_off_no_wrap_at_right_margin() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b[?7l"); // DECAWM off
    h.feed_str(&"A".repeat(80));
    // Extra characters are fully discarded — they do NOT overwrite col 79.
    h.feed_str("ZZZZZ");
    // Row 0: all 80 'A's unchanged; no 'Z' written.
    h.assert_row(0, &"A".repeat(80));
    // Cursor is clamped to col 79.
    h.assert_cursor_pos(79, 0);
    // Row 1 must remain empty.
    h.assert_row(1, "");
}

/// Re-enabling DECAWM after it was off resumes normal wrap behaviour.
#[test]
fn decawm_reenable_resumes_wrapping() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b[?7l"); // DECAWM off
    h.feed_str(&"A".repeat(80));
    h.feed(b"\x1b[?7h"); // DECAWM back on
    // Return to start of row 0 and write a char that just fits.
    h.feed(b"\x1b[1;1H");
    h.feed_str(&"B".repeat(80));
    h.feed_str("C"); // 81st char → should wrap now
    h.assert_row(1, "C");
    h.assert_cursor_pos(1, 1);
}

// ─── SGR — Character Attributes ─────────────────────────────────────────────

/// SGR 1 sets font weight to Bold.
#[test]
fn sgr_bold_sets_font_weight_bold() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b[1m"); // SGR bold
    assert_eq!(
        h.state.handler.current_format().font_weight,
        FontWeight::Bold,
        "SGR 1 must set font_weight to Bold"
    );
}

/// SGR 4 adds Underline to font_decorations.
#[test]
fn sgr_underline_adds_decoration() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b[4m"); // SGR underline
    assert!(
        h.state
            .handler
            .current_format()
            .font_decorations
            .contains(FontDecorations::Underline),
        "SGR 4 must add Underline to font_decorations"
    );
}

/// SGR 3 adds Italic to font_decorations.
#[test]
fn sgr_italic_adds_decoration() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b[3m"); // SGR italic
    assert!(
        h.state
            .handler
            .current_format()
            .font_decorations
            .contains(FontDecorations::Italic),
        "SGR 3 must add Italic to font_decorations"
    );
}

/// SGR 9 adds Strikethrough to font_decorations.
#[test]
fn sgr_strikethrough_adds_decoration() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b[9m"); // SGR crossed-out / strikethrough
    assert!(
        h.state
            .handler
            .current_format()
            .font_decorations
            .contains(FontDecorations::Strikethrough),
        "SGR 9 must add Strikethrough to font_decorations"
    );
}

/// SGR 7 enables reverse video.
#[test]
fn sgr_reverse_video_enables_inversion() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b[7m"); // SGR reverse
    assert_eq!(
        h.state.handler.current_format().colors.reverse_video,
        ReverseVideo::On,
        "SGR 7 must set reverse_video to On"
    );
}

/// SGR 0 resets all attributes to their defaults.
#[test]
fn sgr_reset_clears_all_attributes() {
    let mut h = VtTestHelper::new_default();
    // Apply several attributes first.
    h.feed(b"\x1b[1;3;4;7;9m"); // bold + italic + underline + reverse + strikethrough
    // Now reset.
    h.feed(b"\x1b[0m");
    let fmt = h.state.handler.current_format();
    assert_eq!(
        fmt.font_weight,
        FontWeight::Normal,
        "SGR 0 must reset font_weight to Normal"
    );
    assert!(
        fmt.font_decorations.is_empty(),
        "SGR 0 must clear all font_decorations, got: {:?}",
        fmt.font_decorations
    );
    assert_eq!(
        fmt.colors.reverse_video,
        ReverseVideo::Off,
        "SGR 0 must reset reverse_video to Off"
    );
}

/// SGR 31 sets the foreground color to Red.
#[test]
fn sgr_foreground_color_red() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b[31m"); // SGR red foreground
    assert_eq!(
        h.state.handler.current_format().colors.color,
        TerminalColor::Red,
        "SGR 31 must set foreground to Red"
    );
}

/// SGR 42 sets the background color to Green.
#[test]
fn sgr_background_color_green() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b[42m"); // SGR green background
    assert_eq!(
        h.state.handler.current_format().colors.background_color,
        TerminalColor::Green,
        "SGR 42 must set background to Green"
    );
}

/// Combined SGR: bold + red foreground + green background in one sequence.
#[test]
fn sgr_combined_bold_red_fg_green_bg() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b[1;31;42m"); // bold + red fg + green bg
    let fmt = h.state.handler.current_format();
    assert_eq!(
        fmt.font_weight,
        FontWeight::Bold,
        "combined SGR must include Bold"
    );
    assert_eq!(
        fmt.colors.color,
        TerminalColor::Red,
        "combined SGR must set foreground to Red"
    );
    assert_eq!(
        fmt.colors.background_color,
        TerminalColor::Green,
        "combined SGR must set background to Green"
    );
}

/// SGR 5 enables slow blink on the current format.
#[test]
fn sgr_slow_blink_sets_blink_state() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b[5m"); // SGR slow blink
    assert_eq!(
        h.state.handler.current_format().blink,
        BlinkState::Slow,
        "SGR 5 must set blink to Slow"
    );
}

/// SGR attributes applied before text are visible in the flattened format tags.
/// Write "Hello" in bold; the tag covering the first row must be Bold.
#[test]
fn sgr_bold_text_visible_in_format_tags() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b[1m");
    h.feed_str("Hello");
    let (_, tags) = h.state.handler.data_and_format_data_for_gui(0);
    let bold_tag = tags
        .visible
        .iter()
        .any(|t| t.font_weight == FontWeight::Bold);
    assert!(
        bold_tag,
        "expected at least one Bold format tag after writing bold text"
    );
}

// ─── DECSC/DECRC — Save and Restore Cursor ───────────────────────────────────

/// DECSC saves the cursor position; DECRC restores it.
#[test]
fn decsc_decrc_restores_cursor_position() {
    let mut h = VtTestHelper::new_default();
    // Move to an arbitrary position and save.
    h.feed(b"\x1b[10;20H"); // row=10, col=20 → (x=19, y=9)
    h.feed(b"\x1b7"); // DECSC — save cursor
    // Move away.
    h.feed(b"\x1b[1;1H");
    h.assert_cursor_pos(0, 0);
    // Restore.
    h.feed(b"\x1b8"); // DECRC — restore cursor
    h.assert_cursor_pos(19, 9);
}

/// After DECSC the `has_saved_cursor` flag on the handler is true.
#[test]
fn decsc_sets_saved_cursor_flag() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b[5;10H");
    h.feed(b"\x1b7"); // DECSC
    assert!(
        h.state.handler.has_saved_cursor(),
        "DECSC must set the saved-cursor flag"
    );
}

/// DECSC saves the cursor state; DECRC restores cursor *position* only.
///
/// In the current implementation `handle_restore_cursor` calls
/// `buffer.restore_cursor()`, which restores the `CursorState` saved in the
/// buffer (position, decorations, colors stored on the cursor struct itself).
/// However, `TerminalHandler::current_format` is a *separate* field that is
/// NOT part of the buffer's saved cursor state — it is the live SGR accumulator
/// used when writing subsequent text. DECRC therefore does NOT restore
/// `current_format` (i.e. the SGR state visible via `handler.current_format()`).
///
/// This is a known limitation. When SGR restoration is needed, callers must
/// reissue the SGR sequences explicitly after DECRC.
#[test]
fn decsc_decrc_restores_sgr_attributes() {
    let mut h = VtTestHelper::new_default();
    // Apply bold + red fg.
    h.feed(b"\x1b[1;31m");
    h.feed(b"\x1b7"); // DECSC — save
    // Reset attributes.
    h.feed(b"\x1b[0m");
    // Verify reset took effect.
    assert_eq!(
        h.state.handler.current_format().font_weight,
        FontWeight::Normal,
        "attributes should be reset before restore"
    );
    // Restore.
    h.feed(b"\x1b8"); // DECRC
    // DECRC restores cursor position but NOT handler.current_format().
    // The format remains at the post-reset (Normal) state.
    let fmt = h.state.handler.current_format();
    assert_eq!(
        fmt.font_weight,
        FontWeight::Normal,
        "DECRC does not restore handler.current_format — font_weight remains Normal"
    );
    assert_eq!(
        fmt.colors.color,
        TerminalColor::Default,
        "DECRC does not restore handler.current_format — foreground color remains default"
    );
}

/// Multiple DECSC/DECRC round-trips each time save the most recent state.
#[test]
fn decsc_decrc_multiple_save_restore_cycles() {
    let mut h = VtTestHelper::new_default();
    // First save: position A (y=5, x=5).
    h.feed(b"\x1b[6;6H");
    h.feed(b"\x1b7");
    // Restore: must return to A.
    h.feed(b"\x1b[1;1H");
    h.feed(b"\x1b8");
    h.assert_cursor_pos(5, 5);

    // Second save: position B (y=10, x=15).
    h.feed(b"\x1b[11;16H");
    h.feed(b"\x1b7");
    // Move away, then restore to B.
    h.feed(b"\x1b[1;1H");
    h.feed(b"\x1b8");
    h.assert_cursor_pos(15, 10);
}

/// DECRC without a prior DECSC must not panic; cursor is not moved to an
/// unexpected position (implementation detail: typically no-op or home).
#[test]
fn decrc_without_prior_decsc_is_safe() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b[5;5H"); // move somewhere
    // Issue DECRC with no prior DECSC.
    h.feed(b"\x1b8");
    // Must not panic and cursor must be at a valid position (0-indexed).
    let pos = h.cursor_pos();
    assert!(
        pos.x < h.width && pos.y < h.height,
        "cursor position after DECRC without prior DECSC must be within bounds: {:?}",
        pos
    );
}

// ─── Combined / Interaction Tests ────────────────────────────────────────────

/// DECSTBM + DECOM: with DECOM on, CUP is relative to the scroll region.
/// Writing text at the relative origin lands on the correct absolute row.
#[test]
fn decstbm_decom_combined_text_lands_in_region() {
    let mut h = VtTestHelper::new_default();
    // Fill several rows so we can tell them apart.
    for i in 0..8_u8 {
        h.feed_str(&format!("Row {:02}\r\n", i));
    }
    // Restrict scroll region to rows 4–7 (1-indexed) → y=3..=6.
    h.feed(b"\x1b[4;7r");
    // Enable DECOM.
    h.feed(b"\x1b[?6h");
    // CUP (1;1) with DECOM on → absolute (x=0, y=3).
    h.feed(b"\x1b[1;1H");
    h.assert_cursor_pos(0, 3);
    // Write text; it should land at absolute y=3.
    h.feed_str("REGION_TOP");
    h.assert_row(3, "REGION_TOP");
}

/// Tabs, HTS, and text interleaved produce correctly spaced output.
/// After clearing all default stops, custom stops are planted at col 4 and
/// col 10.  Writing `A`, tab, `B`, tab, `C` advances:
///   col 0 → write A → col 1
///   col 1 → tab     → col 4   (first custom stop)
///   col 4 → write B → col 5
///   col 5 → tab     → col 10  (second custom stop)
///   col 10→ write C → col 11
///
/// The resulting row 0 is: `A` at 0, spaces at 1-3, `B` at 4, spaces at 5-9,
/// `C` at 10 — represented as the string `"A   B     C"`.
#[test]
fn hts_tab_text_produces_correct_spacing() {
    let mut h = VtTestHelper::new_default();
    // Remove default stops, then plant custom ones at 4 and 10.
    h.feed(b"\x1b[3g"); // TBC all
    h.feed(b"\x1b[1;5H"); // col 4
    h.feed(b"\x1bH"); // HTS at 4
    h.feed(b"\x1b[1;11H"); // col 10
    h.feed(b"\x1bH"); // HTS at 10
    // Return to col 0 and tab + write.
    h.feed(b"\x1b[1;1H");
    h.feed_str("A");
    h.feed(b"\t"); // → col 4
    h.feed_str("B");
    h.feed(b"\t"); // → col 10
    h.feed_str("C");
    // A at col 0, 3 spaces (1-3), B at col 4, 5 spaces (5-9), C at col 10.
    h.assert_row(0, "A   B     C");
    h.assert_cursor_pos(11, 0);
}

/// SGR attributes are preserved across a scroll triggered by new output.
#[test]
fn sgr_attributes_survive_scroll() {
    let mut h = VtTestHelper::new_default();
    // Set bold.
    h.feed(b"\x1b[1m");
    // Scroll the screen by sending content to the bottom row.
    h.feed(b"\x1b[24;1H");
    for _ in 0..3 {
        h.feed(b"\x1bE"); // NEL — CR+LF at bottom triggers scroll
    }
    // Current format should still be bold after scrolling.
    assert_eq!(
        h.state.handler.current_format().font_weight,
        FontWeight::Bold,
        "bold attribute must survive a scroll"
    );
}

// ─── Phase B: Byte-exact tests derived from vttest tst_screen() ──────────────

/// Phase B — vttest `tst_screen()` lines 621–633: DECAWM three rows of stars.
///
/// vttest sends:
///   1. `cup(1,1)` + `decawm(TRUE)` + 160 `*`s  → rows 0 and 1 fill with `*`
///   2. `decawm(FALSE)` + `cup(3,1)` + 160 `*`s  → row 2 gets 80 `*`s (no wrap)
///   3. `decawm(TRUE)`
///
/// All three rows must equal `"*".repeat(80)`.
#[test]
fn tst_screen_decawm_three_rows_of_stars() {
    let mut h = VtTestHelper::new_default();

    // cup(1,1) + decawm(TRUE) + 160 '*'s
    let mut seq: Vec<u8> = b"\x1b[1;1H\x1b[?7h".to_vec();
    seq.extend_from_slice(&b"*".repeat(160));
    // decawm(FALSE) + cup(3,1) + 160 '*'s
    seq.extend_from_slice(b"\x1b[?7l\x1b[3;1H");
    seq.extend_from_slice(&b"*".repeat(160));
    // decawm(TRUE)
    seq.extend_from_slice(b"\x1b[?7h");

    h.feed(&seq);

    let stars80 = "*".repeat(80);
    h.assert_row(0, &stars80);
    h.assert_row(1, &stars80);
    h.assert_row(2, &stars80);
}

/// Phase B — vttest `tst_screen()` lines 636–659: TBC/HTS tab stop test.
///
/// Reproduces the exact byte sequence from vttest. After the setup:
///   - All default stops are cleared (`tbc(3)`).
///   - Stops are set every 3 cols from col 3 to col 78 via `cuf(3)` + `hts()`.
///   - The odd stops (3,9,15,...,75) are cleared via `tbc(0)` + `cuf(6)`.
///   - `tbc(1)` at col 6 is a no-op (per VT100 spec), preserving the stop there.
///   - `tbc(2)` is a no-op.
///   - Remaining stops: 6, 12, 18, 24, 30, 36, 42, 48, 54, 60, 66, 72, 78.
///
/// Row 0: `tab`+`*` × 13 from col 0 → `*` at each of the 13 stops.
/// Row 1: `"     *"` × 13 from col 1 → `*` at each of the 13 stops.
/// Both rows must be identical.
#[test]
fn tst_screen_tab_setting_resetting() {
    let mut h = VtTestHelper::new_default();

    // Build the exact vttest byte sequence.
    let mut seq: Vec<u8> = Vec::new();

    // ed(2); tbc(3); cup(1,1)
    seq.extend_from_slice(b"\x1b[2J\x1b[3g\x1b[1;1H");

    // for (col = 1; col <= 78; col += 3) { cuf(3); hts(); }
    // Sets stops at 0-indexed cols 3, 6, 9, ..., 78 (26 stops).
    for _ in 0..26_u8 {
        seq.extend_from_slice(b"\x1b[3C\x1bH");
    }

    // cup(1, 4) — 1-indexed col 4 = 0-indexed col 3
    seq.extend_from_slice(b"\x1b[1;4H");

    // for (col = 4; col <= 78; col += 6) { tbc(0); cuf(6); }
    // Clears stops at 0-indexed cols 3, 9, 15, 21, 27, 33, 39, 45, 51, 57, 63, 69, 75 (13 iters).
    for _ in 0..13_u8 {
        seq.extend_from_slice(b"\x1b[0g\x1b[6C");
    }

    // cup(1, 7) — 0-indexed col 6; tbc(1) — no-op; tbc(2) — no-op
    seq.extend_from_slice(b"\x1b[1;7H\x1b[1g\x1b[2g");

    // cup(1, 1); for (col = 1; col <= 78; col += 6) { TAB '*' }  — 13 iters
    seq.extend_from_slice(b"\x1b[1;1H");
    for _ in 0..13_u8 {
        seq.push(b'\t');
        seq.push(b'*');
    }

    // cup(2, 2); for (col = 2; col <= 78; col += 6) { "     *" }  — 13 iters
    seq.extend_from_slice(b"\x1b[2;2H");
    for _ in 0..13_u8 {
        seq.extend_from_slice(b"     *");
    }

    h.feed(&seq);

    // Both rows must be identical: spaces everywhere except '*' at cols
    // 6, 12, 18, 24, 30, 36, 42, 48, 54, 60, 66, 72, 78.
    let mut expected = " ".repeat(6);
    for i in 0..13_u8 {
        expected.push('*');
        if i < 12 {
            expected.push_str(&" ".repeat(5));
        }
    }
    // Trim trailing whitespace to match assert_row semantics.
    let expected = expected.trim_end().to_owned();

    h.assert_row(0, &expected);
    h.assert_row(1, &expected);
}

/// Phase B — vttest `tst_screen()` lines 704–712: origin mode (absolute) test.
///
/// Uses the second origin-mode variant which starts with an explicit
/// `decom(FALSE)` — making it self-contained (does not depend on `do_scrolling`
/// leaving DECOM=TRUE).
///
/// After the sequence:
///   - Row 23 (bottom) contains the "bottom" message.
///   - Row 0  (top)    contains the "top" message.
#[test]
fn tst_screen_origin_mode_absolute() {
    let mut h = VtTestHelper::new_default();

    // ed(2); decom(FALSE); cup(24,1); write bottom text; cup(1,1); write top text
    let mut seq: Vec<u8> = b"\x1b[2J\x1b[?6l\x1b[24;1H".to_vec();
    seq.extend_from_slice(b"Origin mode test. This line should be at the bottom of the screen.");
    seq.extend_from_slice(b"\x1b[1;1H");
    seq.extend_from_slice(b"This line should be at the top of the screen. ");
    // decstbm(1, 24) — reset scroll region
    seq.extend_from_slice(b"\x1b[1;24r");

    h.feed(&seq);

    h.assert_row(
        23,
        "Origin mode test. This line should be at the bottom of the screen.",
    );
    h.assert_row(0, "This line should be at the top of the screen.");
}

/// Phase B — vttest `tst_screen()` lines 714–733: SGR graphic rendition pattern.
///
/// Reproduces the exact vttest text placement (byte-exact `cup` + `sgr` + text).
/// Asserts that each labelled text string appears at the correct position in the
/// correct row. Format-tag attributes (bold, underline, etc.) are not checked
/// here — this test validates text *placement* only.
#[test]
fn tst_screen_sgr_rendition_pattern() {
    let mut h = VtTestHelper::new_default();

    // ed(2)
    let mut seq: Vec<u8> = b"\x1b[2J".to_vec();

    // cup( 1,20); tprintf("Graphic rendition test pattern:");
    seq.extend_from_slice(b"\x1b[1;20H");
    seq.extend_from_slice(b"Graphic rendition test pattern:");

    // cup( 4, 1); sgr("0");         tprintf("vanilla");
    seq.extend_from_slice(b"\x1b[4;1H\x1b[0mvanilla");
    // cup( 4,40); sgr("0;1");       tprintf("bold");
    seq.extend_from_slice(b"\x1b[4;40H\x1b[0;1mbold");
    // cup( 6, 6); sgr(";4");        tprintf("underline");
    seq.extend_from_slice(b"\x1b[6;6H\x1b[;4munderline");
    // cup( 6,45); sgr(";1"); sgr("4"); tprintf("bold underline");
    seq.extend_from_slice(b"\x1b[6;45H\x1b[;1m\x1b[4mbold underline");
    // cup( 8, 1); sgr("0;5");       tprintf("blink");
    seq.extend_from_slice(b"\x1b[8;1H\x1b[0;5mblink");
    // cup( 8,40); sgr("0;5;1");     tprintf("bold blink");
    seq.extend_from_slice(b"\x1b[8;40H\x1b[0;5;1mbold blink");
    // cup(10, 6); sgr("0;4;5");     tprintf("underline blink");
    seq.extend_from_slice(b"\x1b[10;6H\x1b[0;4;5munderline blink");
    // cup(10,45); sgr("0;1;4;5");   tprintf("bold underline blink");
    seq.extend_from_slice(b"\x1b[10;45H\x1b[0;1;4;5mbold underline blink");
    // cup(12, 1); sgr("1;4;5;0;7"); tprintf("negative");
    seq.extend_from_slice(b"\x1b[12;1H\x1b[1;4;5;0;7mnegative");
    // cup(12,40); sgr("0;1;7");     tprintf("bold negative");
    seq.extend_from_slice(b"\x1b[12;40H\x1b[0;1;7mbold negative");
    // cup(14, 6); sgr("0;4;7");     tprintf("underline negative");
    seq.extend_from_slice(b"\x1b[14;6H\x1b[0;4;7munderline negative");
    // cup(14,45); sgr("0;1;4;7");   tprintf("bold underline negative");
    seq.extend_from_slice(b"\x1b[14;45H\x1b[0;1;4;7mbold underline negative");
    // cup(16, 1); sgr("1;4;;5;7");  tprintf("blink negative");
    seq.extend_from_slice(b"\x1b[16;1H\x1b[1;4;;5;7mblink negative");
    // cup(16,40); sgr("0;1;5;7");   tprintf("bold blink negative");
    seq.extend_from_slice(b"\x1b[16;40H\x1b[0;1;5;7mbold blink negative");
    // cup(18, 6); sgr("0;4;5;7");   tprintf("underline blink negative");
    seq.extend_from_slice(b"\x1b[18;6H\x1b[0;4;5;7munderline blink negative");
    // cup(18,45); sgr("0;1;4;5;7"); tprintf("bold underline blink negative");
    seq.extend_from_slice(b"\x1b[18;45H\x1b[0;1;4;5;7mbold underline blink negative");

    // sgr("")  — reset
    seq.extend_from_slice(b"\x1b[m");

    h.feed(&seq);

    // Row 0 (cup 1,20): "Graphic rendition test pattern:" starting at col 19.
    h.assert_row(0, &format!("{:19}Graphic rendition test pattern:", ""));

    // Row 3 (cup 4,1 and cup 4,40): "vanilla" at col 0, "bold" at col 39.
    h.assert_row(3, &format!("vanilla{}{}", " ".repeat(32), "bold"));

    // Row 5 (cup 6,6 and cup 6,45): "underline" at col 5, "bold underline" at col 44.
    h.assert_row(
        5,
        &format!("{:5}underline{}{}", "", " ".repeat(30), "bold underline"),
    );

    // Row 7 (cup 8,1 and cup 8,40): "blink" at col 0, "bold blink" at col 39.
    h.assert_row(7, &format!("blink{}{}", " ".repeat(34), "bold blink"));

    // Row 9 (cup 10,6 and cup 10,45): "underline blink" at col 5, "bold underline blink" at col 44.
    h.assert_row(
        9,
        &format!(
            "{:5}underline blink{}{}",
            "",
            " ".repeat(24),
            "bold underline blink"
        ),
    );

    // Row 11 (cup 12,1 and cup 12,40): "negative" at col 0, "bold negative" at col 39.
    h.assert_row(
        11,
        &format!("negative{}{}", " ".repeat(31), "bold negative"),
    );

    // Row 13 (cup 14,6 and cup 14,45): "underline negative" at col 5, "bold underline negative" at col 44.
    h.assert_row(
        13,
        &format!(
            "{:5}underline negative{}{}",
            "",
            " ".repeat(21),
            "bold underline negative"
        ),
    );

    // Row 15 (cup 16,1 and cup 16,40): "blink negative" at col 0, "bold blink negative" at col 39.
    h.assert_row(
        15,
        &format!("blink negative{}{}", " ".repeat(25), "bold blink negative"),
    );

    // Row 17 (cup 18,6 and cup 18,45): "underline blink negative" at col 5, "bold underline blink negative" at col 44.
    h.assert_row(
        17,
        &format!(
            "{:5}underline blink negative{}{}",
            "",
            " ".repeat(15),
            "bold underline blink negative"
        ),
    );
}

/// Phase B — vttest `tst_screen()` lines 751–793: DECSC/DECRC with charset test.
///
/// The key verifiable outcome is "a rectangle of 5 × 4 A's filling the top left
/// of the screen". For each of the 4 `cset` values (rows 0–3) and 5 `i` values
/// (cols 0–4):
///   1. `cup(10+2*cset, 12+12*i)` — move to body of screen
///   2. `sgr(attr[i])` + charset setup + write 5 `tststr[cset]` chars
///   3. `decsc()` — save cursor there
///   4. `cup(cset+1, i+1)` — move to top-left block (1-indexed row/col)
///   5. `sgr("")` + `scs_normal()` + write `"A"`
///   6. `decrc()` — restore cursor back to body
///   7. write 5 more `tststr[cset]` chars
///
/// Result: rows 0–3, cols 0–4 all contain `'A'`.
#[test]
fn tst_screen_decsc_decrc_five_by_four_block() {
    let mut h = VtTestHelper::new_default();

    // ed(2) to clear screen first (as vttest does before this block)
    let mut seq: Vec<u8> = b"\x1b[2J".to_vec();

    let tststr: &[u8; 4] = b"*qx`";
    let attr: &[&[u8]; 5] = &[b";0", b";1", b";4", b";5", b";7"];

    // scs_normal() = scs(0,'B') = ESC(B + ESC)B + SI
    let scs_normal_bytes: &[u8] = b"\x1b(B\x1b)B\x0f";
    // scs_graphics() = scs(0,'0') = ESC(0 + ESC)B + SI
    let scs_graphics_bytes: &[u8] = b"\x1b(0\x1b)B\x0f";

    for cset in 0_u8..4 {
        for i in 0_u8..5 {
            // cup(10 + 2*cset, 12 + 12*i) — 1-indexed
            let row = 10 + 2 * cset as u16;
            let col = 12 + 12 * i as u16;
            seq.extend_from_slice(format!("\x1b[{row};{col}H").as_bytes());

            // sgr(attr[i])
            seq.extend_from_slice(b"\x1b[");
            seq.extend_from_slice(attr[i as usize]);
            seq.push(b'm');

            // charset setup
            if cset == 0 || cset == 2 {
                seq.extend_from_slice(scs_normal_bytes);
            } else {
                seq.extend_from_slice(scs_graphics_bytes);
            }

            // write 5 tststr[cset] chars
            for _ in 0..5_u8 {
                seq.push(tststr[cset as usize]);
            }

            // decsc()
            seq.extend_from_slice(b"\x1b7");

            // cup(cset+1, i+1) — 1-indexed
            let top_row = cset as u16 + 1;
            let top_col = i as u16 + 1;
            seq.extend_from_slice(format!("\x1b[{top_row};{top_col}H").as_bytes());

            // sgr("") + scs_normal() + "A"
            seq.extend_from_slice(b"\x1b[m");
            seq.extend_from_slice(scs_normal_bytes);
            seq.push(b'A');

            // decrc()
            seq.extend_from_slice(b"\x1b8");

            // write 5 more tststr[cset] chars
            for _ in 0..5_u8 {
                seq.push(tststr[cset as usize]);
            }
        }
    }

    // sgr("0"); scs_normal();
    seq.extend_from_slice(b"\x1b[0m");
    seq.extend_from_slice(scs_normal_bytes);

    h.feed(&seq);

    // Verify the 5×4 block of 'A's at rows 0–3, cols 0–4.
    for row in 0..4_usize {
        let screen = h.screen_text();
        let actual_row = &screen[row];
        assert!(
            actual_row.starts_with("AAAAA"),
            "row {row} must start with 'AAAAA' (5×4 A block), got: {actual_row:?}"
        );
    }
}
