// freminal-buffer/tests/buffer_tests.rs

// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use freminal_buffer::buffer::Buffer;
use freminal_common::buffer_states::cursor::StateColors;
use freminal_common::buffer_states::format_tag::FormatTag;
use freminal_common::buffer_states::tchar::TChar;
use freminal_common::colors::TerminalColor;

fn ascii(c: char) -> TChar {
    TChar::Ascii(c as u8)
}

fn emoji(s: &str) -> TChar {
    TChar::new_from_many_chars(s.as_bytes()).unwrap()
}

#[test]
fn insert_simple_text_in_buffer() {
    let mut buf = Buffer::new(10, 10);

    buf.insert_text(&[ascii('H'), ascii('e'), ascii('l'), ascii('l'), ascii('o')]);

    assert_eq!(buf.cursor().pos.x, 5);
    assert_eq!(buf.cursor().pos.y, 0);
}

#[test]
fn insert_wraps_into_next_row() {
    let mut buf = Buffer::new(5, 10);

    buf.insert_text(&[ascii('H'), ascii('e'), ascii('l'), ascii('l'), ascii('o')]); // col=5 -> wrap
    buf.insert_text(&[ascii('!')]);

    assert_eq!(buf.cursor().pos.y, 1);
    assert_eq!(buf.cursor().pos.x, 1);
}

#[test]
fn insert_wide_char_wrap() {
    let mut buf = Buffer::new(4, 10);

    buf.insert_text(&[ascii('A'), emoji("🙂")]); // A takes 1, 🙂 takes 2 → 3 total

    assert_eq!(buf.cursor().pos.x, 3);

    buf.insert_text(&[emoji("🙂")]); // does NOT fit at col 3 → wraps

    assert_eq!(buf.cursor().pos.y, 1);
    assert_eq!(buf.cursor().pos.x, 2);
}

#[test]
fn insert_multiple_wraps() {
    let mut buf = Buffer::new(3, 10);

    buf.insert_text(&[ascii('A'), ascii('B'), ascii('C'), ascii('D'), ascii('E')]);

    assert_eq!(buf.cursor().pos.y, 1);
    assert_eq!(buf.cursor().pos.x, 2);
}

#[test]
fn multi_row_mixed_width_insertion() {
    let mut buf = Buffer::new(4, 10);

    buf.insert_text(&[ascii('A'), emoji("🙂"), ascii('B'), emoji("🙂")]);
    // Expected:
    // Row 0: A 🙂 B → col=4 (wrap)
    // Row 1: 🙂     → col=2

    assert_eq!(buf.cursor().pos.y, 1);
    assert_eq!(buf.cursor().pos.x, 2);
}

//
// ────────────────────────────────────────────────────────────
//  BCE (Background Color Erase) TESTS
// ────────────────────────────────────────────────────────────
//

/// Build a `FormatTag` with a red background (non-default).
fn red_bg_tag() -> FormatTag {
    FormatTag {
        colors: StateColors::default().with_background_color(TerminalColor::Red),
        ..FormatTag::default()
    }
}

#[test]
fn bce_erase_line_to_end_fills_with_current_bg() {
    let mut buf = Buffer::new(10, 5);

    // Write "ABCDE" on row 0
    buf.insert_text(&[ascii('A'), ascii('B'), ascii('C'), ascii('D'), ascii('E')]);
    // Move cursor to col 2
    buf.set_cursor_pos(Some(2), Some(0));
    // Set current format to red background
    buf.set_format(red_bg_tag());
    // Erase from cursor to end of line
    buf.erase_line_to_end();

    let row = &buf.rows()[0];
    // Cols 0-1 should still be A, B
    assert_eq!(row.resolve_cell(0).tchar(), &ascii('A'));
    assert_eq!(row.resolve_cell(1).tchar(), &ascii('B'));
    // Cols 2-9 should be blanks with red background
    for col in 2..10 {
        let cell = row.resolve_cell(col);
        assert_eq!(
            cell.tchar(),
            &TChar::Space,
            "col {col}: expected blank after erase"
        );
        assert_eq!(
            cell.tag(),
            &red_bg_tag(),
            "col {col}: expected red-bg tag from BCE"
        );
    }
}

#[test]
fn bce_erase_line_fills_with_current_bg() {
    let mut buf = Buffer::new(10, 5);

    buf.insert_text(&[ascii('H'), ascii('e'), ascii('l'), ascii('l'), ascii('o')]);
    buf.set_cursor_pos(Some(3), Some(0));
    buf.set_format(red_bg_tag());
    buf.erase_line();

    let row = &buf.rows()[0];
    for col in 0..10 {
        let cell = row.resolve_cell(col);
        assert_eq!(
            cell.tchar(),
            &TChar::Space,
            "col {col}: expected blank after full line erase"
        );
        assert_eq!(
            cell.tag(),
            &red_bg_tag(),
            "col {col}: expected red-bg tag from BCE"
        );
    }
}

#[test]
fn bce_erase_display_fills_with_current_bg() {
    let mut buf = Buffer::new(5, 3);

    buf.insert_text(&[ascii('A'), ascii('B')]);
    buf.set_format(red_bg_tag());
    buf.erase_display();

    // All rows should have blank cells with the red-bg tag
    let rows = buf.rows();
    for (ridx, row) in rows.iter().enumerate() {
        for col in 0..5 {
            let cell = row.resolve_cell(col);
            assert_eq!(
                cell.tchar(),
                &TChar::Space,
                "row {ridx} col {col}: expected blank after display erase"
            );
            assert_eq!(
                cell.tag(),
                &red_bg_tag(),
                "row {ridx} col {col}: expected red-bg tag from BCE"
            );
        }
    }
}

#[test]
fn bce_scroll_does_not_fill_new_row_with_current_bg() {
    let mut buf = Buffer::new(5, 3);

    // Fill all rows with text
    buf.insert_text(&[ascii('A'), ascii('B'), ascii('C'), ascii('D'), ascii('E')]);
    buf.handle_lf();
    buf.insert_text(&[ascii('F'), ascii('G'), ascii('H'), ascii('I'), ascii('J')]);
    buf.handle_lf();
    buf.insert_text(&[ascii('K'), ascii('L'), ascii('M'), ascii('N'), ascii('O')]);

    // Set format to red background, then scroll up
    buf.set_format(red_bg_tag());
    buf.scroll_region_up_n(1);

    // The bottom row (row 2) should be blank with DEFAULT background — scroll
    // operations do not apply BCE.  Only explicit erase operations (ED, EL)
    // fill with the current background color.
    let rows = buf.rows();
    let last_visible_idx = rows.len() - 1;
    let last_row = &rows[last_visible_idx];
    for col in 0..5 {
        let cell = last_row.resolve_cell(col);
        assert_eq!(
            cell.tchar(),
            &TChar::Space,
            "col {col}: new row after scroll should be blank"
        );
        assert_eq!(
            cell.tag(),
            &FormatTag::default(),
            "col {col}: scroll-created row should have default background (no BCE)"
        );
    }
}

#[test]
fn bce_erase_chars_fills_with_current_bg() {
    let mut buf = Buffer::new(10, 5);

    buf.insert_text(&[ascii('A'), ascii('B'), ascii('C'), ascii('D'), ascii('E')]);
    buf.set_cursor_pos(Some(1), Some(0));
    buf.set_format(red_bg_tag());
    buf.erase_chars(2);

    let row = &buf.rows()[0];
    // Col 0 untouched
    assert_eq!(row.resolve_cell(0).tchar(), &ascii('A'));
    // Cols 1-2 erased with BCE
    for col in 1..3 {
        let cell = row.resolve_cell(col);
        assert_eq!(cell.tchar(), &TChar::Space, "col {col}: should be erased");
        assert_eq!(
            cell.tag(),
            &red_bg_tag(),
            "col {col}: should have red-bg from BCE"
        );
    }
    // Cols 3-4 untouched
    assert_eq!(row.resolve_cell(3).tchar(), &ascii('D'));
    assert_eq!(row.resolve_cell(4).tchar(), &ascii('E'));
}

#[test]
fn bce_default_tag_leaves_rows_sparse() {
    let mut buf = Buffer::new(5, 3);

    buf.insert_text(&[ascii('A'), ascii('B')]);
    // Format is default — erase should leave rows sparse
    buf.erase_display();

    // All rows should be sparse (empty cells vector)
    for (ridx, row) in buf.rows().iter().enumerate() {
        assert!(
            row.characters().is_empty(),
            "row {ridx}: should be sparse after erase with default tag"
        );
    }
}
