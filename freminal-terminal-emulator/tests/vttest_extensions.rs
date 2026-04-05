// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! vttest Menu 11 — Non-VT100 Extension Tests.
//!
//! These tests cover the non-VT100 extensions exercised by vttest Menu 11.
//! Only extensions with deterministic, buffer-verifiable outcomes are included.
//! Mouse tracking and window manipulation tests are excluded (require GUI).
//!
//! ## Coverage
//!
//! - **ECMA-48 cursor commands** — CNL, CPL, HPA/CHA, VPA
//! - **ECMA-48 miscellaneous** — SU (scroll up), SD (scroll down), ECH, REP,
//!   CBT, CHT
//! - **DECSCUSR** — cursor shape: block blink, block steady, underline blink,
//!   underline steady, vertical bar blink, vertical bar steady
//! - **DECTCEM** — show/hide cursor flag
//! - **256-color and RGB** — SGR 38;5;N, SGR 48;5;N, SGR 38;2;R;G;B,
//!   SGR 48;2;R;G;B
//! - **Alternate screen** — enter (`?1049h`), leave (`?1049l`), content
//!   preservation, cursor saved/restored
//! - **Bracketed paste** — `?2004h` / `?2004l` mode flag
//!
//! ## Excluded
//!
//! - **BCE** (background color erase): not yet implemented in Freminal.
//! - **Mouse tracking** (`?1000` etc.): requires GUI interaction.
//! - **Window manipulation**: requires GUI context.
//!
//! All cursor positions in the helper API are **0-indexed** (`x` = column,
//! `y` = row). CSI sequences use **1-indexed** row;col parameters.

#![allow(clippy::unwrap_used)]

mod vttest_common;

use freminal_common::{
    buffer_states::{cursor::ReverseVideo, fonts::FontWeight},
    colors::TerminalColor,
    cursor::CursorVisualStyle,
};
use vttest_common::VtTestHelper;

// ─── ECMA-48 Cursor Commands ─────────────────────────────────────────────────

/// CNL (Cursor Next Line, CSI Ps E) — moves cursor to the beginning of the
/// Ps-th line below the current line.
#[test]
fn ecma48_cnl_cursor_next_line_default() {
    let mut h = VtTestHelper::new_default();
    // Position at row 5, col 10 (1-indexed 6;11 → 0-indexed col=10, row=5).
    h.feed(b"\x1b[6;11H");
    // CNL with no parameter (default 1): move down 1 line, col reset to 0.
    h.feed(b"\x1b[E");
    h.assert_cursor_pos(0, 6);
}

#[test]
fn ecma48_cnl_cursor_next_line_multiple() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b[3;5H"); // row=2, col=4 (0-indexed)
    // CNL(4): move down 4 lines to row 6, col 0.
    h.feed(b"\x1b[4E");
    h.assert_cursor_pos(0, 6);
}

/// CPL (Cursor Preceding Line, CSI Ps F) — moves cursor to the beginning of
/// the Ps-th line above the current line.
#[test]
fn ecma48_cpl_cursor_preceding_line_default() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b[8;20H"); // row=7, col=19 (0-indexed)
    // CPL with no parameter (default 1): move up 1 line, col reset to 0.
    h.feed(b"\x1b[F");
    h.assert_cursor_pos(0, 6);
}

#[test]
fn ecma48_cpl_cursor_preceding_line_multiple() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b[15;40H"); // row=14, col=39 (0-indexed)
    // CPL(5): move up 5 lines to row 9, col 0.
    h.feed(b"\x1b[5F");
    h.assert_cursor_pos(0, 9);
}

/// HPA / CHA (Horizontal Position Absolute, CSI Ps G) — move cursor to the
/// given column on the current row (1-indexed parameter).
#[test]
fn ecma48_hpa_horizontal_position_absolute() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b[10;1H"); // row=9, col=0 (0-indexed)
    // HPA(20): move to column 20 (1-indexed) = col 19 (0-indexed).
    h.feed(b"\x1b[20G");
    h.assert_cursor_pos(19, 9);
}

#[test]
fn ecma48_hpa_default_moves_to_column_zero() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b[5;50H"); // row=4, col=49 (0-indexed)
    // HPA with no parameter (default 1): move to column 1 = col 0 (0-indexed).
    h.feed(b"\x1b[G");
    h.assert_cursor_pos(0, 4);
}

/// VPA (Vertical Position Absolute, CSI Ps d) — move cursor to the given row
/// in the current column (1-indexed parameter).
#[test]
fn ecma48_vpa_vertical_position_absolute() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b[1;30H"); // row=0, col=29 (0-indexed)
    // VPA(15): move to row 15 (1-indexed) = row 14 (0-indexed).
    h.feed(b"\x1b[15d");
    h.assert_cursor_pos(29, 14);
}

#[test]
fn ecma48_vpa_default_moves_to_row_zero() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b[20;10H"); // row=19, col=9 (0-indexed)
    // VPA with no parameter (default 1): move to row 0 (0-indexed).
    h.feed(b"\x1b[d");
    h.assert_cursor_pos(9, 0);
}

// ─── ECMA-48 Miscellaneous Commands ──────────────────────────────────────────

/// SU (Scroll Up, CSI Ps S) — scroll the scroll region up by Ps lines.
/// Lines at the bottom of the region are filled with blanks.
#[test]
fn ecma48_su_scroll_up_default() {
    let mut h = VtTestHelper::new_default();
    // Write content to first 5 rows.
    for i in 0..5_u8 {
        h.feed_str(&format!("Row {:02}\r\n", i));
    }
    // SU(1): scroll up 1 line.  Row 0 ("Row 00") scrolls away.
    h.feed(b"\x1b[S");
    // After scroll, row 0 has "Row 01".
    h.assert_row(0, "Row 01");
    h.assert_row(1, "Row 02");
    h.assert_row(2, "Row 03");
    h.assert_row(3, "Row 04");
    h.assert_row(4, ""); // blank line at bottom of scroll region
}

#[test]
fn ecma48_su_scroll_up_multiple() {
    let mut h = VtTestHelper::new_default();
    for i in 0..8_u8 {
        h.feed_str(&format!("Row {:02}\r\n", i));
    }
    // SU(3): scroll up 3 lines.
    h.feed(b"\x1b[3S");
    h.assert_row(0, "Row 03");
    h.assert_row(4, "Row 07");
    h.assert_row(5, ""); // blank
    h.assert_row(6, ""); // blank
    h.assert_row(7, ""); // blank
}

/// SD (Scroll Down, CSI Ps T) — scroll the scroll region down by Ps lines.
/// Lines at the top of the region are filled with blanks.
#[test]
fn ecma48_sd_scroll_down_default() {
    let mut h = VtTestHelper::new_default();
    for i in 0..5_u8 {
        h.feed_str(&format!("Row {:02}\r\n", i));
    }
    h.feed(b"\x1b[1;5r"); // scroll region rows 1-5 (1-indexed) = 0-4 (0-indexed)
    h.feed(b"\x1b[1;1H"); // home cursor to top of region
    // SD(2): scroll down 2 lines within the region.
    h.feed(b"\x1b[2T");
    // Row 0 and 1 become blank (inserted at top of region).
    h.assert_row(0, "");
    h.assert_row(1, "");
    // Original row 0 ("Row 00") shifts to row 2.
    h.assert_row(2, "Row 00");
}

/// ECH (Erase Characters, CSI Ps X) — erase Ps characters starting at the
/// cursor position, replacing them with spaces. Cursor does not move.
#[test]
fn ecma48_ech_erase_characters_default() {
    let mut h = VtTestHelper::new_default();
    h.feed_str("ABCDEFGHIJ");
    h.feed(b"\x1b[1;4H"); // cursor at col 3, row 0 (0-indexed)
    // ECH(1): erase 1 character at cursor.
    h.feed(b"\x1b[X");
    h.assert_row(0, "ABC EFGHIJ");
    // Cursor must not have moved.
    h.assert_cursor_pos(3, 0);
}

#[test]
fn ecma48_ech_erase_multiple_characters() {
    let mut h = VtTestHelper::new_default();
    h.feed_str("ABCDEFGHIJ");
    h.feed(b"\x1b[1;3H"); // cursor at col 2, row 0
    // ECH(4): erase 4 characters starting at col 2.
    h.feed(b"\x1b[4X");
    h.assert_row(0, "AB    GHIJ");
    h.assert_cursor_pos(2, 0);
}

/// REP (Repeat Last Character, CSI Ps b) — repeat the last printed character
/// Ps times at the current cursor position.
#[test]
fn ecma48_rep_repeat_last_char() {
    let mut h = VtTestHelper::new_default();
    // Write 'A', then REP(4) to get AAAAA total.
    h.feed(b"A");
    h.feed(b"\x1b[4b");
    h.assert_row(0, "AAAAA");
    h.assert_cursor_pos(5, 0);
}

#[test]
fn ecma48_rep_repeat_does_not_wrap_line() {
    let mut h = VtTestHelper::new_default();
    // Move near the end of the line and repeat past the edge.
    h.feed(b"\x1b[1;78H"); // col 77 (0-indexed), row 0
    h.feed(b"X");
    // REP(5): repeat 'X' 5 times — only 2 fit in the row (cols 78,79).
    h.feed(b"\x1b[5b");
    // Row 0 should have 'X' at col 77 and two more at 78-79.
    let row0 = h.screen_text()[0].clone();
    // Count 'X' characters in the row — should be exactly 3 (one written + 2 that fit).
    let x_count = row0.chars().filter(|&c| c == 'X').count();
    assert_eq!(x_count, 3, "REP should fill to EOL; got: {row0:?}");
}

/// CBT (Cursor Backward Tabulation, CSI Ps Z) — move cursor backward Ps tab
/// stops.
#[test]
fn ecma48_cbt_cursor_backward_tab() {
    let mut h = VtTestHelper::new_default();
    // Default tab stops are every 8 columns (0, 8, 16, 24, …).
    // Position cursor at col 20 (0-indexed).
    h.feed(b"\x1b[1;21H"); // 1-indexed col 21 = col 20 (0-indexed)
    // CBT(1): move back 1 tab stop → col 16.
    h.feed(b"\x1b[Z");
    h.assert_cursor_pos(16, 0);
}

#[test]
fn ecma48_cbt_cursor_backward_tab_multiple() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b[1;41H"); // col 40 (0-indexed)
    // CBT(3): move back 3 tab stops → 40 → 32 → 24 → 16.
    h.feed(b"\x1b[3Z");
    h.assert_cursor_pos(16, 0);
}

/// CHT (Cursor Forward Tabulation, CSI Ps I) — move cursor forward Ps tab
/// stops.
#[test]
fn ecma48_cht_cursor_forward_tab() {
    let mut h = VtTestHelper::new_default();
    // Position at col 3 (0-indexed).
    h.feed(b"\x1b[1;4H");
    // CHT(1): advance to next tab stop at col 8.
    h.feed(b"\x1b[I");
    h.assert_cursor_pos(8, 0);
}

#[test]
fn ecma48_cht_cursor_forward_tab_multiple() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b[1;1H"); // col 0
    // CHT(3): advance 3 tab stops → 0 → 8 → 16 → 24.
    h.feed(b"\x1b[3I");
    h.assert_cursor_pos(24, 0);
}

// ─── DECSCUSR — Cursor Shape ─────────────────────────────────────────────────

/// DECSCUSR Ps=1: blinking block cursor.
#[test]
fn decscusr_block_blink() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b[1 q"); // DECSCUSR 1 = blinking block
    let style = h.state.handler.cursor_visual_style();
    assert_eq!(style, CursorVisualStyle::BlockCursorBlink);
}

/// DECSCUSR Ps=2: steady block cursor.
#[test]
fn decscusr_block_steady() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b[2 q"); // DECSCUSR 2 = steady block
    let style = h.state.handler.cursor_visual_style();
    assert_eq!(style, CursorVisualStyle::BlockCursorSteady);
}

/// DECSCUSR Ps=3: blinking underline cursor.
#[test]
fn decscusr_underline_blink() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b[3 q"); // DECSCUSR 3 = blinking underline
    let style = h.state.handler.cursor_visual_style();
    assert_eq!(style, CursorVisualStyle::UnderlineCursorBlink);
}

/// DECSCUSR Ps=4: steady underline cursor.
#[test]
fn decscusr_underline_steady() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b[4 q"); // DECSCUSR 4 = steady underline
    let style = h.state.handler.cursor_visual_style();
    assert_eq!(style, CursorVisualStyle::UnderlineCursorSteady);
}

/// DECSCUSR Ps=5: blinking vertical bar cursor.
#[test]
fn decscusr_vertical_bar_blink() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b[5 q"); // DECSCUSR 5 = blinking vertical bar
    let style = h.state.handler.cursor_visual_style();
    assert_eq!(style, CursorVisualStyle::VerticalLineCursorBlink);
}

/// DECSCUSR Ps=6: steady vertical bar cursor.
#[test]
fn decscusr_vertical_bar_steady() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b[6 q"); // DECSCUSR 6 = steady vertical bar
    let style = h.state.handler.cursor_visual_style();
    assert_eq!(style, CursorVisualStyle::VerticalLineCursorSteady);
}

/// DECSCUSR Ps=0 resets to the default (blinking block).
#[test]
fn decscusr_zero_resets_to_default() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b[4 q"); // set to underline steady first
    h.feed(b"\x1b[0 q"); // reset to default
    let style = h.state.handler.cursor_visual_style();
    assert_eq!(style, CursorVisualStyle::BlockCursorBlink);
}

// ─── DECTCEM — Cursor Visibility ─────────────────────────────────────────────

/// DECTCEM hide (`CSI ?25l`) makes the cursor invisible.
#[test]
fn dectcem_hide_cursor() {
    let mut h = VtTestHelper::new_default();
    // Cursor is visible by default.
    assert!(
        h.state.handler.show_cursor(),
        "cursor must be visible by default"
    );
    h.feed(b"\x1b[?25l"); // DECTCEM hide
    assert!(
        !h.state.handler.show_cursor(),
        "cursor must be hidden after DECTCEM hide"
    );
}

/// DECTCEM show (`CSI ?25h`) restores cursor visibility.
#[test]
fn dectcem_show_cursor() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b[?25l"); // hide
    h.feed(b"\x1b[?25h"); // show
    assert!(
        h.state.handler.show_cursor(),
        "cursor must be visible after DECTCEM show"
    );
}

// ─── 256-Color and RGB Color ─────────────────────────────────────────────────

/// SGR 38;5;N sets the foreground to palette color N (256-color).
///
/// The handler resolves `PaletteIndex(N)` against the default xterm-256 palette
/// immediately — `current_format()` returns the resolved `Custom(r,g,b)` value.
/// Color 196 is pure red (`#ff0000`) in the standard xterm-256 palette.
#[test]
fn sgr_256_color_foreground() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b[38;5;196m"); // foreground = color 196 (pure red in xterm-256)
    let fmt = h.state.handler.current_format();
    assert_eq!(
        fmt.colors.color,
        TerminalColor::Custom(255, 0, 0),
        "SGR 38;5;196 must resolve to Custom(255,0,0) — pure red"
    );
}

/// SGR 48;5;N sets the background to palette color N (256-color).
///
/// Color 82 is `#5fff00` (bright chartreuse) in the standard xterm-256 palette,
/// resolved to `Custom(95, 255, 0)`.
#[test]
fn sgr_256_color_background() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b[48;5;82m"); // background = color 82 (chartreuse)
    let fmt = h.state.handler.current_format();
    assert_eq!(
        fmt.colors.background_color,
        TerminalColor::Custom(95, 255, 0),
        "SGR 48;5;82 must resolve to Custom(95,255,0)"
    );
}

/// SGR 38;2;R;G;B sets the foreground to a true-color RGB value.
#[test]
fn sgr_rgb_color_foreground() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b[38;2;255;128;0m"); // foreground = RGB(255, 128, 0)
    let fmt = h.state.handler.current_format();
    assert_eq!(
        fmt.colors.color,
        TerminalColor::Custom(255, 128, 0),
        "SGR 38;2;255;128;0 must set foreground to Custom(255,128,0)"
    );
}

/// SGR 48;2;R;G;B sets the background to a true-color RGB value.
#[test]
fn sgr_rgb_color_background() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b[48;2;0;64;128m"); // background = RGB(0, 64, 128)
    let fmt = h.state.handler.current_format();
    assert_eq!(
        fmt.colors.background_color,
        TerminalColor::Custom(0, 64, 128),
        "SGR 48;2;0;64;128 must set background to Custom(0,64,128)"
    );
}

/// SGR 0 after color settings must reset both foreground and background.
#[test]
fn sgr_reset_clears_256_color() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b[38;5;100m"); // set fg
    h.feed(b"\x1b[48;5;200m"); // set bg
    h.feed(b"\x1b[0m"); // reset
    let fmt = h.state.handler.current_format();
    assert_eq!(
        fmt.colors.color,
        TerminalColor::Default,
        "SGR 0 must reset foreground"
    );
    assert_eq!(
        fmt.colors.background_color,
        TerminalColor::DefaultBackground,
        "SGR 0 must reset background"
    );
    assert_eq!(
        fmt.font_weight,
        FontWeight::Normal,
        "SGR 0 must reset font weight"
    );
    assert!(
        fmt.font_decorations.is_empty(),
        "SGR 0 must reset decorations"
    );
    assert_eq!(
        fmt.colors.reverse_video,
        ReverseVideo::Off,
        "SGR 0 must clear reverse video"
    );
}

// ─── Alternate Screen ─────────────────────────────────────────────────────────

/// Entering the alternate screen preserves the primary screen content.
/// After leaving, the primary screen is restored.
#[test]
fn alt_screen_primary_content_preserved() {
    let mut h = VtTestHelper::new_default();

    // Write content to primary screen.
    h.feed_str("Primary content\r\n");
    h.feed_str("Second line\r\n");

    // Enter alternate screen (`?1049h`): saves cursor, switches to alt buffer.
    h.feed(b"\x1b[?1049h");

    // Alternate screen starts blank.
    h.assert_row(0, "");
    h.assert_row(1, "");

    // Write something to the alternate screen.
    h.feed_str("Alternate content\r\n");
    h.assert_row(0, "Alternate content");

    // Leave alternate screen (`?1049l`): restores cursor and primary buffer.
    h.feed(b"\x1b[?1049l");

    // Primary screen content must be restored.
    h.assert_row(0, "Primary content");
    h.assert_row(1, "Second line");
}

/// Entering alternate screen restores a blank buffer (no leftover from
/// a previous alternate screen session).
#[test]
fn alt_screen_starts_blank_on_entry() {
    let mut h = VtTestHelper::new_default();

    // Enter, write, leave.
    h.feed(b"\x1b[?1049h");
    h.feed_str("Alt session 1\r\n");
    h.feed(b"\x1b[?1049l");

    // Enter again — the alternate screen must start blank.
    h.feed(b"\x1b[?1049h");
    h.assert_row(0, "");
}

/// The cursor position is saved and restored across alternate screen entry/exit.
#[test]
fn alt_screen_cursor_saved_and_restored() {
    let mut h = VtTestHelper::new_default();

    // Position cursor at a known location on the primary screen.
    h.feed(b"\x1b[5;10H"); // row=4, col=9 (0-indexed)

    // Enter alternate screen — cursor position is saved.
    h.feed(b"\x1b[?1049h");

    // Move to a different position on the alt screen.
    h.feed(b"\x1b[15;20H"); // row=14, col=19

    // Leave alternate screen — cursor must return to the saved position.
    h.feed(b"\x1b[?1049l");

    h.assert_cursor_pos(9, 4);
}

// ─── Bracketed Paste Mode ────────────────────────────────────────────────────

/// `CSI ?2004h` enables bracketed paste mode.
#[test]
fn bracketed_paste_enable() {
    use freminal_common::buffer_states::modes::rl_bracket::RlBracket;

    let mut h = VtTestHelper::new_default();
    // Bracketed paste is disabled by default.
    assert_eq!(
        h.state.modes.bracketed_paste,
        RlBracket::Disabled,
        "bracketed paste must be disabled by default"
    );

    h.feed(b"\x1b[?2004h"); // enable
    assert_eq!(
        h.state.modes.bracketed_paste,
        RlBracket::Enabled,
        "bracketed paste must be enabled after CSI ?2004h"
    );
}

/// `CSI ?2004l` disables bracketed paste mode.
#[test]
fn bracketed_paste_disable() {
    use freminal_common::buffer_states::modes::rl_bracket::RlBracket;

    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b[?2004h"); // enable
    h.feed(b"\x1b[?2004l"); // disable
    assert_eq!(
        h.state.modes.bracketed_paste,
        RlBracket::Disabled,
        "bracketed paste must be disabled after CSI ?2004l"
    );
}
