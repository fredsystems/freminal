// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! A glyph wider than the line must not make the wrap loop spin (Task 148.5,
//! issue #558).
//!
//! `Buffer::insert_text` wraps when a glyph does not fit on the current row.
//! A double-width glyph that is wider than the span a *fresh* row offers can
//! never fit, so wrapping to a new row never helps: the old loop wrapped,
//! wrote nothing, wrapped again, and pushed a row per iteration until the
//! allocator gave up.
//!
//! The decided behaviour matches kitty (`screen_draw_text`: "discard too wide
//! character") and xterm (`dotext`: "give up - it is too big"): the glyph is
//! discarded -- not written, cursor not moved, no wrap -- and the rest of the
//! text carries on.
//!
//! Every test that exercises a glyph wider than the fresh-row span hangs (and
//! exhausts memory) on the code before the fix, so none of them can be
//! satisfied by a retry or a longer timeout.

use freminal_buffer::buffer::Buffer;
use freminal_buffer::row::LineWidth;
use freminal_common::buffer_states::modes::{decawm::Decawm, declrmm::Declrmm};
use freminal_common::buffer_states::tchar::TChar;

/// A double-width glyph (CJK ideograph, display width 2).
fn wide() -> TChar {
    TChar::from('\u{4e2d}')
}

fn narrow(c: char) -> TChar {
    TChar::from(c)
}

/// The glyph in `cells()[col]` of `row`, or `Space` when the cell was never
/// written.
fn glyph_at(buf: &Buffer, row: usize, col: usize) -> TChar {
    buf.rows()
        .get(row)
        .and_then(|r| r.cells().get(col))
        .map_or(TChar::Space, |c| *c.tchar())
}

fn assert_blank_row(buf: &Buffer, row: usize) {
    let width = buf.terminal_width();
    for col in 0..width {
        assert_eq!(
            glyph_at(buf, row, col),
            TChar::Space,
            "row {row} col {col} should be blank"
        );
    }
}

// ── Shape 1: a 1-column terminal (the fuzz reproducer) ──────────────────

#[test]
fn one_column_wide_glyph_alone_is_discarded() {
    let mut buf = Buffer::new(1, 4);
    let rows_before = buf.rows().len();
    buf.insert_text(&[wide()]);

    assert_eq!(
        buf.rows().len(),
        rows_before,
        "no row may be pushed for a discard"
    );
    assert_blank_row(&buf, 0);
    let cursor = buf.cursor().pos;
    assert_eq!((cursor.x, cursor.y), (0, 0), "cursor must not move");
}

#[test]
fn one_column_wide_glyph_then_narrow_text_writes_the_narrow_text() {
    let mut buf = Buffer::new(1, 4);
    let rows_before = buf.rows().len();
    buf.insert_text(&[wide(), narrow('a'), narrow('b')]);

    // Only the real second character needed a second row.
    assert_eq!(buf.rows().len(), rows_before + 1);
    assert_eq!(glyph_at(&buf, 0, 0), narrow('a'));
    assert_eq!(glyph_at(&buf, 1, 0), narrow('b'));
}

#[test]
fn one_column_text_before_and_after_the_wide_glyph_is_unaffected() {
    let mut buf = Buffer::new(1, 4);
    let rows_before = buf.rows().len();
    buf.insert_text(&[narrow('a'), wide(), narrow('b')]);

    // Only the real second character needed a second row.
    assert_eq!(buf.rows().len(), rows_before + 1);
    assert_eq!(glyph_at(&buf, 0, 0), narrow('a'));
    // The discarded glyph did not force a wrap of its own: 'b' is the first
    // thing to need the second row.
    assert_eq!(glyph_at(&buf, 1, 0), narrow('b'));
    assert_blank_row(&buf, 2);
}

#[test]
fn one_column_many_wide_glyphs_terminate_without_growing_the_buffer() {
    let mut buf = Buffer::new(1, 4);
    let rows_before = buf.rows().len();
    buf.insert_text(&vec![wide(); 10_000]);

    assert_eq!(buf.rows().len(), rows_before);
    assert_blank_row(&buf, 0);
}

#[test]
fn one_column_wide_glyph_discarded_with_autowrap_off() {
    let mut buf = Buffer::new(1, 4);
    buf.set_wrap(Decawm::NoAutoWrap);
    let rows_before = buf.rows().len();
    buf.insert_text(&[wide(), narrow('a')]);

    assert_eq!(buf.rows().len(), rows_before);
    assert_eq!(glyph_at(&buf, 0, 0), narrow('a'));
}

#[test]
fn one_column_alternate_screen_wide_glyph_is_discarded() {
    let mut buf = Buffer::new(1, 4);
    buf.switch_to_alternate();
    let rows_before = buf.rows().len();
    buf.insert_text(&[wide(), narrow('a')]);

    assert_eq!(
        buf.rows().len(),
        rows_before,
        "the alternate screen has no scrollback"
    );
    assert_eq!(glyph_at(&buf, 0, 0), narrow('a'));
}

#[test]
fn one_column_with_declrmm_enabled_wide_glyph_is_discarded() {
    // A 1-column buffer cannot hold a valid margin pair (left < right), so
    // DECSLRM resets to the full width and the span stays 1.
    let mut buf = Buffer::new(1, 4);
    buf.set_declrmm(Declrmm::Enabled);
    buf.set_left_right_margins(1, 2);
    let rows_before = buf.rows().len();
    buf.insert_text(&[wide(), narrow('a')]);

    assert_eq!(buf.rows().len(), rows_before);
    assert_eq!(glyph_at(&buf, 0, 0), narrow('a'));
}

// ── Shape 2: DECDWL / DECDHL rows halve the span ────────────────────────

#[test]
fn double_width_row_of_three_columns_discards_a_wide_glyph() {
    // Span (3 / 2).max(1) == 1, so a width-2 glyph can never fit.
    let mut buf = Buffer::new(3, 3);
    buf.set_cursor_line_width(LineWidth::DoubleWidth);
    let rows_before = buf.rows().len();
    buf.insert_text(&[wide(), narrow('a')]);

    assert_eq!(buf.rows().len(), rows_before);
    assert_eq!(glyph_at(&buf, 0, 0), narrow('a'));
}

#[test]
fn double_height_rows_discard_a_wide_glyph_too() {
    for lw in [LineWidth::DoubleHeightTop, LineWidth::DoubleHeightBottom] {
        let mut buf = Buffer::new(3, 3);
        buf.set_cursor_line_width(lw);
        let rows_before = buf.rows().len();
        buf.insert_text(&[wide(), narrow('a')]);

        assert_eq!(buf.rows().len(), rows_before, "{lw:?}");
        assert_eq!(glyph_at(&buf, 0, 0), narrow('a'), "{lw:?}");
    }
}

#[test]
fn double_width_row_of_four_columns_still_writes_a_wide_glyph() {
    // Span (4 / 2) == 2: a width-2 glyph exactly fits, so it must be written.
    let mut buf = Buffer::new(4, 3);
    buf.set_cursor_line_width(LineWidth::DoubleWidth);
    buf.insert_text(&[wide()]);

    assert_eq!(glyph_at(&buf, 0, 0), wide());
    assert_eq!(buf.cursor().pos.x, 2);
}

// ── Shape 3: DECSLRM margins ────────────────────────────────────────────

#[test]
fn two_column_margins_still_write_a_wide_glyph() {
    // Margins 1-based 3..4 -> columns 2..=3, a span of 2: the glyph fits.
    let mut buf = Buffer::new(10, 3);
    buf.set_declrmm(Declrmm::Enabled);
    buf.set_left_right_margins(3, 4);
    buf.set_cursor_pos(Some(2), Some(0));
    buf.insert_text(&[wide(), wide()]);

    assert_eq!(glyph_at(&buf, 0, 2), wide());
    // The second glyph does not fit beside the first, so it wraps to the
    // left margin of the next row.
    assert_eq!(glyph_at(&buf, 1, 2), wide());
    assert_blank_row_outside_margins(&buf, 0);
}

fn assert_blank_row_outside_margins(buf: &Buffer, row: usize) {
    for col in (0..2).chain(4..buf.terminal_width()) {
        assert_eq!(
            glyph_at(buf, row, col),
            TChar::Space,
            "row {row} col {col} is outside the margins"
        );
    }
}

#[test]
fn two_column_margins_on_a_double_width_row_discard_a_wide_glyph() {
    // The margin span is 2; DECDWL halves it to 1, so the glyph cannot fit.
    let mut buf = Buffer::new(10, 3);
    buf.set_declrmm(Declrmm::Enabled);
    buf.set_left_right_margins(3, 4);
    buf.set_cursor_pos(Some(2), Some(0));
    buf.set_cursor_line_width(LineWidth::DoubleWidth);
    let rows_before = buf.rows().len();
    buf.insert_text(&[wide(), narrow('a')]);

    assert_eq!(buf.rows().len(), rows_before);
    assert_eq!(glyph_at(&buf, 0, 2), narrow('a'));
}

// ── Unchanged behaviour ─────────────────────────────────────────────────

#[test]
fn wide_glyph_with_one_column_left_still_wraps() {
    // Width 7, glyph at column 6: only one column remains on this row, but a
    // fresh row offers seven, so the glyph wraps rather than being discarded.
    let mut buf = Buffer::new(7, 3);
    buf.set_cursor_pos(Some(6), Some(0));
    buf.insert_text(&[wide()]);

    assert_eq!(
        glyph_at(&buf, 0, 6),
        TChar::Space,
        "row 0 col 6 stays blank"
    );
    assert_eq!(glyph_at(&buf, 1, 0), wide());
    assert_eq!(buf.cursor().pos.y, 1);
    assert_eq!(buf.cursor().pos.x, 2);
}

#[test]
fn wide_glyph_fitting_exactly_in_a_two_column_terminal_is_written() {
    let mut buf = Buffer::new(2, 3);
    buf.insert_text(&[wide(), wide()]);

    assert_eq!(glyph_at(&buf, 0, 0), wide());
    assert_eq!(glyph_at(&buf, 1, 0), wide());
}
