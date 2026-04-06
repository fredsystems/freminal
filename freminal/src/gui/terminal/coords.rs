// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Pure coordinate-utility functions operating over snapshot fields.
//!
//! All functions in this module are stateless mathematical helpers with no
//! side effects.

use conv2::ConvUtil;
use eframe::egui::Pos2;
use freminal_common::buffer_states::tchar::TChar;
use freminal_terminal_emulator::snapshot::TerminalSnapshot;

/// Compute the buffer-absolute row index of the first visible row.
///
/// This is the inverse of screen-relative → buffer-absolute:
///   `buffer_row = visible_window_start + screen_row`
///   `screen_row = buffer_row - visible_window_start`
///
/// The formula mirrors `Buffer::visible_window_start`: the live bottom of the
/// buffer is at `total_rows - term_height`, and scrolling *back* subtracts
/// from that position.
pub(super) const fn visible_window_start(snap: &TerminalSnapshot) -> usize {
    snap.total_rows
        .saturating_sub(snap.term_height)
        .saturating_sub(snap.scroll_offset)
}

/// Convert a screen-relative `(row, col)` — where `col` is a **display
/// column** (a wide character occupies two columns) — to a flat index into
/// the `visible_chars` slice.
///
/// `visible_chars` is produced by `Buffer::flatten_row`, which skips
/// continuation cells.  A CJK character that occupies two display columns
/// produces only one `TChar` entry.  The simple fixed-stride formula
/// `row * (term_width + 1) + col` is therefore wrong whenever wide
/// characters are present.  This function walks the flat vector to find the
/// correct index.
///
/// Returns `None` if `row`/`col` are out of range.
pub(super) fn flat_index_for_cell(
    visible_chars: &[TChar],
    row: usize,
    col: usize,
) -> Option<usize> {
    // Walk through visible_chars, splitting on TChar::NewLine to find the
    // start of the target row.
    let mut current_row: usize = 0;
    let mut idx: usize = 0;

    // Advance past preceding rows.
    while current_row < row {
        if idx >= visible_chars.len() {
            return None; // row is beyond the data
        }
        if matches!(visible_chars[idx], TChar::NewLine) {
            current_row += 1;
        }
        idx += 1;
    }

    // Now `idx` points to the first TChar of the target row (or past the end).
    // Walk through the row's characters, accumulating display columns.
    let mut display_col: usize = 0;
    while idx < visible_chars.len() {
        if matches!(visible_chars[idx], TChar::NewLine) {
            break; // past end of this row
        }
        let w = visible_chars[idx].display_width();
        // The mouse is within this character's display span.
        if col < display_col + w {
            return Some(idx);
        }
        display_col += w;
        idx += 1;
    }

    None // col is beyond the row's content
}

/// Convert an egui pointer position to `(col, row)` terminal-grid coordinates.
///
/// Subtracts the terminal area `origin` so that coordinates are relative to
/// the top-left of the terminal grid, not the top-left of the window.
pub(super) fn encode_egui_mouse_pos_as_usize(
    pos: Pos2,
    character_size: (f32, f32),
    origin: Pos2,
) -> (usize, usize) {
    // Subtract the terminal area origin so that coordinates are relative to
    // the top-left of the terminal grid, not the top-left of the window.
    let rel_x = (pos.x - origin.x).max(0.0);
    let rel_y = (pos.y - origin.y).max(0.0);

    let x = ((rel_x / character_size.0).floor())
        .approx_as::<usize>()
        .unwrap_or_else(|_| {
            if rel_x > 0.0 {
                debug!("Mouse x ({}) out of range, clamping to 255", rel_x);
                255
            } else {
                debug!("Mouse x ({}) out of range, clamping to 0", rel_x);
                0
            }
        });
    let y = ((rel_y / character_size.1).floor())
        .approx_as::<usize>()
        .unwrap_or_else(|_| {
            if rel_y > 0.0 {
                debug!("Mouse y ({}) out of range, clamping to 255", rel_y);
                255
            } else {
                debug!("Mouse y ({}) out of range, clamping to 0", rel_y);
                0
            }
        });

    (x, y)
}

#[cfg(test)]
mod visible_window_start_tests {
    use super::*;
    use freminal_terminal_emulator::snapshot::TerminalSnapshot;

    fn snap_with(total_rows: usize, term_height: usize, scroll_offset: usize) -> TerminalSnapshot {
        let mut s = TerminalSnapshot::empty();
        s.total_rows = total_rows;
        s.term_height = term_height;
        s.scroll_offset = scroll_offset;
        s
    }

    #[test]
    fn live_view_at_bottom() {
        // 100 total rows, 24 visible, scrolled to live bottom.
        let snap = snap_with(100, 24, 0);
        assert_eq!(visible_window_start(&snap), 76);
    }

    #[test]
    fn scrolled_back_fully() {
        // 100 total rows, 24 visible, scrolled back 76 rows (to very top).
        let snap = snap_with(100, 24, 76);
        assert_eq!(visible_window_start(&snap), 0);
    }

    #[test]
    fn scrolled_back_partially() {
        let snap = snap_with(100, 24, 10);
        assert_eq!(visible_window_start(&snap), 66);
    }

    #[test]
    fn no_scrollback() {
        // total_rows == term_height → no scrollback, always 0.
        let snap = snap_with(24, 24, 0);
        assert_eq!(visible_window_start(&snap), 0);
    }

    #[test]
    fn fewer_rows_than_height() {
        // Edge case: buffer has fewer rows than visible height.
        let snap = snap_with(10, 24, 0);
        assert_eq!(visible_window_start(&snap), 0);
    }
}

#[cfg(test)]
mod flat_index_for_cell_tests {
    use super::*;
    use freminal_common::buffer_states::tchar::TChar;

    fn ascii(c: char) -> TChar {
        TChar::Ascii(c as u8)
    }

    /// Build a simple `visible_chars` vec: rows of ASCII chars separated by `NewLine`.
    fn make_visible(rows: &[&str]) -> Vec<TChar> {
        let mut chars = Vec::new();
        for (i, row) in rows.iter().enumerate() {
            for c in row.chars() {
                chars.push(ascii(c));
            }
            if i + 1 < rows.len() {
                chars.push(TChar::NewLine);
            }
        }
        chars
    }

    #[test]
    fn first_cell() {
        let chars = make_visible(&["abcde", "fghij"]);
        assert_eq!(flat_index_for_cell(&chars, 0, 0), Some(0));
    }

    #[test]
    fn middle_of_first_row() {
        let chars = make_visible(&["abcde", "fghij"]);
        assert_eq!(flat_index_for_cell(&chars, 0, 2), Some(2));
    }

    #[test]
    fn start_of_second_row() {
        let chars = make_visible(&["abcde", "fghij"]);
        // Row 0 = 5 chars + 1 NewLine = indices 0..5, NL at 5.
        // Row 1 starts at index 6.
        assert_eq!(flat_index_for_cell(&chars, 1, 0), Some(6));
    }

    #[test]
    fn col_beyond_row() {
        let chars = make_visible(&["abc"]);
        // Row has 3 chars (cols 0, 1, 2). Col 5 is out of range.
        assert_eq!(flat_index_for_cell(&chars, 0, 5), None);
    }

    #[test]
    fn row_beyond_data() {
        let chars = make_visible(&["abc"]);
        assert_eq!(flat_index_for_cell(&chars, 5, 0), None);
    }

    #[test]
    fn wide_character_handling() {
        // Simulate a row with a wide character (display_width=2) followed by
        // a narrow character.  In the flat vec, the wide char is one TChar
        // entry but occupies 2 display columns.
        let wide = TChar::from('Ｗ'); // fullwidth W, width=2
        let chars = vec![wide, ascii('x')];

        // Display columns: 0-1 = 'Ｗ', 2 = 'x'
        assert_eq!(flat_index_for_cell(&chars, 0, 0), Some(0)); // first col of wide char
        assert_eq!(flat_index_for_cell(&chars, 0, 1), Some(0)); // second col of wide char
        assert_eq!(flat_index_for_cell(&chars, 0, 2), Some(1)); // 'x'
        assert_eq!(flat_index_for_cell(&chars, 0, 3), None); // beyond
    }

    #[test]
    fn empty_visible_chars() {
        let chars: Vec<TChar> = Vec::new();
        assert_eq!(flat_index_for_cell(&chars, 0, 0), None);
    }

    #[test]
    fn multiple_wide_chars() {
        let w1 = TChar::from('Ｗ'); // width 2
        let w2 = TChar::from('Ｘ'); // width 2
        let chars = vec![w1, w2, ascii('z')];

        // Display layout: cols 0-1 = Ｗ, cols 2-3 = Ｘ, col 4 = z
        assert_eq!(flat_index_for_cell(&chars, 0, 0), Some(0));
        assert_eq!(flat_index_for_cell(&chars, 0, 1), Some(0));
        assert_eq!(flat_index_for_cell(&chars, 0, 2), Some(1));
        assert_eq!(flat_index_for_cell(&chars, 0, 3), Some(1));
        assert_eq!(flat_index_for_cell(&chars, 0, 4), Some(2));
        assert_eq!(flat_index_for_cell(&chars, 0, 5), None);
    }
}
