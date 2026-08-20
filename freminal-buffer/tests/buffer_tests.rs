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

/// A line feed that creates a brand-new row at the bottom must NOT apply BCE:
/// LF only moves the active position, it is not an explicit erase.  The new
/// row must carry the default background regardless of the active SGR — even a
/// visually-inert attribute like bold.  Regression test for the "extra blank
/// line on narrower resize" bug: a lingering `ESC[1m` (bold) used to make the
/// new row materialise full-width bold-blank cells, which then survived reflow
/// as a spurious trailing continuation row.
#[test]
fn bce_line_feed_new_row_ignores_active_bold() {
    // Bold, no color — visually inert on a blank cell.
    let bold_tag = FormatTag {
        font_weight: freminal_common::buffer_states::fonts::FontWeight::Bold,
        ..FormatTag::default()
    };

    let mut buf = Buffer::new(10, 3);
    buf.insert_text(&[ascii('A'), ascii('B')]);
    buf.set_format(bold_tag);
    // Advance past the last row so handle_lf must create a fresh row.
    buf.handle_lf();
    buf.handle_lf();

    let rows = buf.rows();
    let new_row = rows.last().expect("buffer has rows");
    assert!(
        new_row.characters().is_empty(),
        "line-feed-created row under active bold must stay sparse (default bg), \
         got {} stored cells",
        new_row.characters().len()
    );
}

/// Same as above but for a real background color: this is legitimate content
/// state, yet a LINE FEED still must not BCE-paint the new row (BCE applies
/// only to explicit erases — ED/EL — not to cursor movement / scrolling).
#[test]
fn bce_line_feed_new_row_ignores_active_bg_color() {
    let mut buf = Buffer::new(10, 3);
    buf.insert_text(&[ascii('A'), ascii('B')]);
    buf.set_format(red_bg_tag());
    buf.handle_lf();
    buf.handle_lf();

    let rows = buf.rows();
    let new_row = rows.last().expect("buffer has rows");
    for col in 0..10 {
        let cell = new_row.resolve_cell(col);
        assert_eq!(
            cell.tag(),
            &FormatTag::default(),
            "col {col}: line-feed-created row must have default background (no BCE)"
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

//
// ─── autowrap must not erase the row it wraps onto (issue #491) ──────────────
//

/// Read a row's cells as a `String`, blanks included.
fn row_text(buf: &Buffer, row: usize, width: usize) -> String {
    let row = &buf.rows()[row];
    (0..width)
        .map(|col| row.resolve_cell(col).tchar().to_string())
        .collect()
}

/// Autowrap is defined as cursor movement. Moving onto an already-populated
/// row must leave that row's cells alone; only the cells actually written to
/// may change.
///
/// This previously cleared the whole destination row, which destroyed content
/// the wrapping text never wrote to.
#[test]
fn autowrap_onto_existing_row_preserves_unwritten_cells() {
    let mut buf = Buffer::new(5, 10);

    // Row 0 = AAAAA (fills the row, leaving the cursor in pending wrap),
    // row 1 = BBBBB.
    buf.insert_text(&[ascii('A'); 5]);
    buf.insert_text(&[ascii('B'); 5]);
    assert_eq!(row_text(&buf, 0, 5), "AAAAA");
    assert_eq!(row_text(&buf, 1, 5), "BBBBB");

    // Put the cursor at the end of row 0 and write one character, which leaves
    // the cursor in the pending-wrap state.
    buf.set_cursor_pos(Some(4), Some(0));
    buf.insert_text(&[ascii('Z')]);
    assert_eq!(row_text(&buf, 0, 5), "AAAAZ");

    // The next character autowraps onto row 1 and writes a single cell there.
    buf.insert_text(&[ascii('Q')]);
    assert_eq!(buf.cursor().pos.y, 1);
    assert_eq!(buf.cursor().pos.x, 1);
    assert_eq!(
        row_text(&buf, 1, 5),
        "QBBBB",
        "autowrap must overwrite only the cell it writes, not clear the row"
    );
}

/// The wrapped row is still marked as a continuation of the logical line
/// above, even though its pre-existing cells survive.
///
/// The destination row is built through a **hard break** (`handle_lf`) and
/// asserted to be one before the wrap. Reaching it via an earlier wrap would
/// have left it already `SoftWrap`/`ContinueLogicalLine`, making the
/// post-wrap assertions pass no matter what `reuse_row_as_softwrap` did.
#[test]
fn autowrap_onto_existing_row_still_marks_it_a_continuation() {
    use freminal_buffer::row::{RowJoin, RowOrigin};

    let mut buf = Buffer::new(5, 10);

    // Fill row 0, then reach row 1 with a line feed rather than a wrap, so
    // the destination row genuinely starts its own logical line.
    buf.insert_text(&[ascii('A'); 5]);
    buf.handle_lf();
    buf.set_cursor_pos(Some(0), Some(1));
    buf.insert_text(&[ascii('B'); 5]);

    assert_eq!(
        buf.rows()[1].origin,
        RowOrigin::HardBreak,
        "precondition: the destination row starts as a hard break"
    );
    assert_eq!(
        buf.rows()[1].join,
        RowJoin::NewLogicalLine,
        "precondition: the destination row starts its own logical line"
    );

    // Fill row 0 to the right margin, then print one more character so
    // autowrap lands on row 1.
    buf.set_cursor_pos(Some(4), Some(0));
    buf.insert_text(&[ascii('Z')]);
    buf.insert_text(&[ascii('Q')]);

    let row = &buf.rows()[1];
    assert_eq!(
        row.origin,
        RowOrigin::SoftWrap,
        "the wrap must re-mark the destination row as a continuation"
    );
    assert_eq!(row.join, RowJoin::ContinueLogicalLine);
    assert_eq!(
        row_text(&buf, 1, 5),
        "QBBBB",
        "and it must still preserve the cells it did not write"
    );
}

/// Text that wraps and then keeps writing must still overwrite the cells it
/// actually covers -- the fix must not turn wrapping into a no-op paint.
#[test]
fn autowrap_still_overwrites_cells_the_text_covers() {
    let mut buf = Buffer::new(5, 10);
    buf.insert_text(&[ascii('A'); 5]);
    buf.insert_text(&[ascii('B'); 5]);

    buf.set_cursor_pos(Some(0), Some(0));
    // 8 characters: 5 fill row 0, the remaining 3 wrap onto row 1.
    buf.insert_text(&[ascii('C'); 8]);

    assert_eq!(row_text(&buf, 0, 5), "CCCCC");
    assert_eq!(
        row_text(&buf, 1, 5),
        "CCCBB",
        "the wrapped text overwrites the cells it covers and no more"
    );
}
