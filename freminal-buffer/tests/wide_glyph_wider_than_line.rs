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

// ── Shape 4: a discard must not leave a wrap behind (review R7) ─────────
//
// Placeable text fills the row, then only unplaceable glyphs remain. The wrap
// between them used to happen anyway -- pushing or scrolling a row -- before
// the discard, so the cursor stayed on the old row while the buffer had
// already grown. At the bottom of the screen the next character then wrapped
// a second time and produced a blank line.

/// Write `a`, then `a` + a wide glyph into two identical buffers and assert the
/// wide glyph changed nothing; then `b` must wrap exactly once.
fn assert_discard_after_full_row_leaves_no_wrap(mut with_wide: Buffer, mut reference: Buffer) {
    reference.insert_text(&[narrow('a')]);
    with_wide.insert_text(&[narrow('a'), wide()]);

    assert_eq!(
        with_wide.rows().len(),
        reference.rows().len(),
        "a discarded glyph must not push a row"
    );
    assert_eq!(
        with_wide.cursor().pos,
        reference.cursor().pos,
        "a discarded glyph must not move the cursor"
    );

    reference.insert_text(&[narrow('b')]);
    with_wide.insert_text(&[narrow('b')]);

    assert_eq!(with_wide.rows().len(), reference.rows().len());
    assert_eq!(with_wide.cursor().pos, reference.cursor().pos);
    for row in 0..reference.rows().len() {
        assert_eq!(
            glyph_at(&with_wide, row, 0),
            glyph_at(&reference, row, 0),
            "row {row}"
        );
    }
}

fn at_bottom_row(mut buf: Buffer) -> Buffer {
    let bottom = buf.terminal_height() - 1;
    buf.set_cursor_pos(Some(0), Some(bottom));
    buf
}

#[test]
fn one_column_primary_bottom_row_discard_after_full_row_does_not_wrap() {
    assert_discard_after_full_row_leaves_no_wrap(
        at_bottom_row(Buffer::new(1, 4)),
        at_bottom_row(Buffer::new(1, 4)),
    );
}

#[test]
fn one_column_primary_bottom_row_then_next_char_wraps_exactly_once() {
    let mut buf = at_bottom_row(Buffer::new(1, 4));
    buf.insert_text(&[narrow('a'), wide()]);
    let rows_after_discard = buf.rows().len();
    buf.insert_text(&[narrow('b')]);

    assert_eq!(buf.rows().len(), rows_after_discard + 1, "exactly one wrap");
    let last = buf.rows().len() - 1;
    assert_eq!(glyph_at(&buf, last, 0), narrow('b'));
    assert_eq!(glyph_at(&buf, last - 1, 0), narrow('a'), "no blank line");
}

#[test]
fn one_column_alternate_bottom_row_discard_after_full_row_does_not_wrap() {
    let alt = || {
        let mut buf = Buffer::new(1, 4);
        buf.switch_to_alternate();
        at_bottom_row(buf)
    };
    assert_discard_after_full_row_leaves_no_wrap(alt(), alt());
}

#[test]
fn double_width_span_one_bottom_row_discard_after_full_row_does_not_wrap() {
    // One column: the DECDWL span (`(1 / 2).max(1)`) is 1, and so is the span
    // of the normal row a wrap would land on, so the wide glyph is
    // unplaceable either way.
    let make = || {
        let mut buf = at_bottom_row(Buffer::new(1, 4));
        buf.set_cursor_line_width(LineWidth::DoubleWidth);
        buf
    };
    assert_discard_after_full_row_leaves_no_wrap(make(), make());
}

// ── Shape 5: the landing row decides the span (148.C8) ──────────────────
//
// A wide glyph that is a leftover on a full row. A normal row would hold it,
// but the row the wrap lands on is DECDWL (span `3 / 2 == 1` on a 3-column
// buffer), so it can never be placed there. The wrap must not happen at all:
// the cursor and row count must be those of writing the prefix alone.

fn assert_wide_after_prefix_changes_nothing(
    mut with_wide: Buffer,
    mut reference: Buffer,
    prefix: &[TChar],
) {
    reference.insert_text(prefix);
    let mut text = prefix.to_vec();
    text.push(wide());
    with_wide.insert_text(&text);

    assert_eq!(
        with_wide.rows().len(),
        reference.rows().len(),
        "an unplaceable glyph must not push or scroll a row"
    );
    assert_eq!(
        with_wide.cursor().pos,
        reference.cursor().pos,
        "an unplaceable glyph must not advance the cursor"
    );
    for row in 0..reference.rows().len() {
        for col in 0..reference.terminal_width() {
            assert_eq!(
                glyph_at(&with_wide, row, col),
                glyph_at(&reference, row, col),
                "row {row} col {col}"
            );
        }
    }
}

/// A 3-wide primary buffer whose row 1 is DECDWL, cursor at the start of row 0.
fn dwl_next_row() -> Buffer {
    let mut buf = Buffer::new(3, 4);
    buf.set_cursor_pos(Some(0), Some(1));
    buf.set_cursor_line_width(LineWidth::DoubleWidth);
    buf.set_cursor_pos(Some(0), Some(0));
    buf
}

#[test]
fn wrap_onto_an_existing_double_width_row_is_not_taken() {
    // `reuse_row_as_softwrap` keeps the row's `line_width`, so row 1 stays
    // DECDWL when the wrap reuses it.
    assert_wide_after_prefix_changes_nothing(
        dwl_next_row(),
        dwl_next_row(),
        &[narrow('a'), narrow('b')],
    );
}

#[test]
fn wrap_onto_an_existing_double_width_row_still_wraps_what_fits() {
    // A narrow glyph fits the one-column DECDWL span, so the wrap is taken.
    let mut buf = dwl_next_row();
    buf.insert_text(&[narrow('a'), narrow('b'), wide(), narrow('c')]);

    assert_eq!(glyph_at(&buf, 1, 0), narrow('c'));
    assert_eq!(buf.cursor().pos.y, 1);
}

/// A 3x3 alternate screen whose last row (the cursor's) is DECDWL, with the
/// scroll region `[top1, bottom1]` (1-based).
fn alt_last_row_dwl(top1: usize, bottom1: usize) -> Buffer {
    let mut buf = Buffer::new(3, 3);
    buf.switch_to_alternate();
    buf.set_scroll_region(top1, bottom1);
    buf.set_cursor_pos(Some(0), Some(2));
    buf.set_cursor_line_width(LineWidth::DoubleWidth);
    buf.set_cursor_pos(Some(0), Some(2));
    buf
}

#[test]
fn stay_on_last_row_below_margin_double_width_row_is_not_wrapped() {
    // Cursor below the bottom margin on the last screen row: a wrap stays on
    // this DECDWL row, so the glyph (span 1) can never be placed.
    assert_wide_after_prefix_changes_nothing(
        alt_last_row_dwl(1, 2),
        alt_last_row_dwl(1, 2),
        &[narrow('a')],
    );
}

#[test]
fn primary_declrmm_column_scroll_keeps_the_double_width_row() {
    // On the primary screen DECLRMM confines the wrap-scroll to columns, so
    // the landing row keeps its DECDWL `line_width` (margin span 3, halved to
    // 1).
    let make = || {
        let mut buf = Buffer::new(3, 3);
        buf.set_declrmm(Declrmm::Enabled);
        buf.set_scroll_region(1, 2);
        buf.set_cursor_pos(Some(0), Some(1));
        buf.set_cursor_line_width(LineWidth::DoubleWidth);
        buf
    };
    assert_wide_after_prefix_changes_nothing(make(), make(), &[narrow('a')]);
}

#[test]
fn whole_row_scroll_lands_on_a_blank_single_width_row() {
    // At the region bottom with a whole-row scroll the landing row is freshly
    // blanked and single-width, so the glyph that does not fit the DECDWL row
    // is placed there.
    let mut buf = alt_last_row_dwl(1, 3);
    buf.insert_text(&[narrow('a'), wide()]);

    assert_eq!(glyph_at(&buf, 2, 0), wide());
    assert_eq!(buf.cursor().pos.y, 2);
    assert_eq!(buf.cursor().pos.x, 2);
}

#[test]
fn wrap_pushing_a_new_row_lands_on_a_single_width_row() {
    // Full-screen primary at the bottom: a pushed row is single-width.
    let mut buf = Buffer::new(3, 2);
    buf.set_cursor_pos(Some(0), Some(1));
    buf.set_cursor_line_width(LineWidth::DoubleWidth);
    buf.insert_text(&[narrow('a'), wide()]);

    let last = buf.rows().len() - 1;
    assert_eq!(glyph_at(&buf, last, 0), wide());
}
