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
use freminal_common::buffer_states::{format_tag::FormatTag, tchar::TChar};
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

/// Look up the URL (if any) at a given buffer-absolute cell coordinate.
///
/// Converts the buffer-absolute `(cell_row, cell_col)` to a screen-relative
/// position, computes the flat index into `visible_chars`, then searches
/// `visible_tags` for a tag covering that index whose `url` field is `Some`.
///
/// Returns the URL string if found, `None` otherwise.
pub(super) fn url_at_cell(
    cell_row: usize,
    cell_col: usize,
    visible_chars: &[TChar],
    visible_tags: &[FormatTag],
    window_start: usize,
) -> Option<String> {
    let screen_row = cell_row.checked_sub(window_start)?;
    let flat_idx = flat_index_for_cell(visible_chars, screen_row, cell_col)?;
    visible_tags
        .iter()
        .find(|tag| tag.start <= flat_idx && flat_idx < tag.end)
        .and_then(|tag| tag.url.as_ref())
        .map(|u| u.url.clone())
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

#[cfg(test)]
mod url_at_cell_tests {
    use super::*;
    use freminal_common::buffer_states::{format_tag::FormatTag, tchar::TChar, url::Url};
    use std::sync::Arc;

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

    fn url_tag(start: usize, end: usize, url: &str) -> FormatTag {
        FormatTag {
            start,
            end,
            url: Some(Arc::new(Url {
                id: None,
                url: url.to_string(),
            })),
            ..FormatTag::default()
        }
    }

    fn plain_tag(start: usize, end: usize) -> FormatTag {
        FormatTag {
            start,
            end,
            ..FormatTag::default()
        }
    }

    #[test]
    fn cell_inside_url_returns_url() {
        // Row 0: "hello" (indices 0..5)
        // Row 1: "world" (indices 6..11, NL at 5)
        // URL covers indices 1..4 ("ell" in "hello").
        let chars = make_visible(&["hello", "world"]);
        let tags = vec![
            plain_tag(0, 1),
            url_tag(1, 4, "https://example.com"),
            plain_tag(4, 11),
        ];
        let window_start = 0;

        // Cell (row=0, col=2) → flat_idx=2, inside the URL tag [1,4).
        assert_eq!(
            url_at_cell(0, 2, &chars, &tags, window_start),
            Some("https://example.com".to_string())
        );
    }

    #[test]
    fn cell_outside_url_returns_none() {
        let chars = make_visible(&["hello", "world"]);
        let tags = vec![
            plain_tag(0, 1),
            url_tag(1, 4, "https://example.com"),
            plain_tag(4, 11),
        ];
        let window_start = 0;

        // Cell (row=0, col=0) → flat_idx=0, inside plain tag [0,1).
        assert_eq!(url_at_cell(0, 0, &chars, &tags, window_start), None);
    }

    #[test]
    fn cell_on_second_row_with_url() {
        // Row 0: "hello" (indices 0..5), NL at 5
        // Row 1: "world" (indices 6..11)
        // URL covers all of row 1: [6, 11).
        let chars = make_visible(&["hello", "world"]);
        let tags = vec![plain_tag(0, 6), url_tag(6, 11, "https://row2.example.com")];
        let window_start = 0;

        // Cell (row=1, col=3) → screen_row=1, flat_idx=9, inside URL tag.
        assert_eq!(
            url_at_cell(1, 3, &chars, &tags, window_start),
            Some("https://row2.example.com".to_string())
        );
    }

    #[test]
    fn cell_with_nonzero_window_start() {
        // Simulates scrollback: window_start = 10, so buffer row 10 maps
        // to screen row 0.
        let chars = make_visible(&["hello"]);
        let tags = vec![url_tag(0, 5, "https://scroll.example.com")];
        let window_start = 10;

        // Buffer row 10, col 2 → screen_row = 0, flat_idx = 2.
        assert_eq!(
            url_at_cell(10, 2, &chars, &tags, window_start),
            Some("https://scroll.example.com".to_string())
        );
    }

    #[test]
    fn cell_row_before_window_returns_none() {
        // Buffer row 5 is before window_start=10 → checked_sub underflows.
        let chars = make_visible(&["hello"]);
        let tags = vec![url_tag(0, 5, "https://example.com")];
        let window_start = 10;

        assert_eq!(url_at_cell(5, 0, &chars, &tags, window_start), None);
    }

    #[test]
    fn cell_col_beyond_row_returns_none() {
        let chars = make_visible(&["abc"]);
        let tags = vec![url_tag(0, 3, "https://example.com")];
        let window_start = 0;

        // Col 10 is way past the 3-char row.
        assert_eq!(url_at_cell(0, 10, &chars, &tags, window_start), None);
    }

    #[test]
    fn empty_tags_returns_none() {
        let chars = make_visible(&["hello"]);
        let tags: Vec<FormatTag> = Vec::new();
        let window_start = 0;

        assert_eq!(url_at_cell(0, 2, &chars, &tags, window_start), None);
    }

    #[test]
    fn tag_at_boundary_start_is_inclusive() {
        // URL tag covers [2, 5). Cell at col 2 should match.
        let chars = make_visible(&["abcde"]);
        let tags = vec![
            plain_tag(0, 2),
            url_tag(2, 5, "https://boundary.example.com"),
        ];
        let window_start = 0;

        assert_eq!(
            url_at_cell(0, 2, &chars, &tags, window_start),
            Some("https://boundary.example.com".to_string())
        );
    }

    #[test]
    fn tag_at_boundary_end_is_exclusive() {
        // URL tag covers [2, 5). Cell at col 5 (flat_idx=5) should NOT match
        // the URL tag — it's at the exclusive boundary.
        let chars = make_visible(&["abcdefgh"]);
        let tags = vec![
            plain_tag(0, 2),
            url_tag(2, 5, "https://boundary.example.com"),
            plain_tag(5, 8),
        ];
        let window_start = 0;

        // Col 4 → flat_idx 4, inside [2,5) → match.
        assert_eq!(
            url_at_cell(0, 4, &chars, &tags, window_start),
            Some("https://boundary.example.com".to_string())
        );
        // Col 5 → flat_idx 5, NOT inside [2,5) → no match.
        assert_eq!(url_at_cell(0, 5, &chars, &tags, window_start), None);
    }

    #[test]
    fn multiple_urls_returns_correct_one() {
        // Two URLs on the same row.
        // Row: "abc_xyz_end" (11 chars, indices 0..11)
        // URL 1 covers [0,3): "abc" → https://first.example.com
        // URL 2 covers [4,7): "xyz" → https://second.example.com
        let chars = make_visible(&["abc_xyz_end"]);
        let tags = vec![
            url_tag(0, 3, "https://first.example.com"),
            plain_tag(3, 4),
            url_tag(4, 7, "https://second.example.com"),
            plain_tag(7, 11),
        ];
        let window_start = 0;

        assert_eq!(
            url_at_cell(0, 1, &chars, &tags, window_start),
            Some("https://first.example.com".to_string())
        );
        assert_eq!(
            url_at_cell(0, 5, &chars, &tags, window_start),
            Some("https://second.example.com".to_string())
        );
        // Col 3 is the underscore between URLs — plain tag.
        assert_eq!(url_at_cell(0, 3, &chars, &tags, window_start), None);
    }
}
