// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Task 131.5 tests: parked screens and the screen-switch primitives
//! (`switch_to_alternate`, `switch_to_primary`, `clear_alternate_screen`).
//!
//! Both screens are real stores that are moved in and out of the `Buffer`.
//! A switch keeps the cursor's screen position and its attributes, leaves the
//! (shared) margins alone, gives each screen its own DECSC slot, and keeps the
//! alternate screen's contents for the next visit; only
//! `clear_alternate_screen` blanks it, and it does so with fresh row numbers.

use std::sync::Arc;

use freminal_common::buffer_states::{
    buffer_type::BufferType, fonts::FontWeight, modes::decom::Decom, row_number::RowNumber,
    tchar::TChar,
};

use crate::{
    buffer::{Buffer, PlaceImageResult},
    cell::Cell,
    image_store::{AnimationControl, ImageProtocol, ImageSizeMode, InlineImage, next_image_id},
    row::Row,
};

fn text(s: &str) -> Vec<TChar> {
    s.chars().map(|c| TChar::Ascii(c as u8)).collect()
}

fn row_text(buf: &Buffer, index: usize) -> String {
    let text: String = buf.rows()[index]
        .cells()
        .iter()
        .map(Cell::into_utf8)
        .collect();
    text.trim_end().to_owned()
}

/// The text of 0-based live-window screen row `screen_row`.
fn screen_text(buf: &Buffer, screen_row: usize) -> String {
    let index = buf.screen_row_index(screen_row).expect("screen row exists");
    row_text(buf, index)
}

/// Write `s` on the cursor row and move to the start of the next row.
fn line(buf: &mut Buffer, s: &str) {
    buf.insert_text(&text(s));
    buf.handle_lf();
    buf.handle_cr();
}

/// Place a `cols` x `rows` Sixel image with its top-left at screen
/// `(col, row)` of the active screen; returns its id.
fn place_image_at(buf: &mut Buffer, col: usize, row: usize, cols: usize, rows: usize) -> u64 {
    let image = InlineImage {
        id: next_image_id(),
        pixels: Arc::new(vec![0u8; cols * rows * 4]),
        width_px: u32::try_from(cols * 8).unwrap(),
        height_px: u32::try_from(rows * 16).unwrap(),
        display_cols: cols,
        display_rows: rows,
        size_mode: ImageSizeMode::NativePixels,
        frames: Vec::new(),
        root_gap_ms: 0,
        animation: AnimationControl::default(),
    };
    let id = image.id;
    buf.set_cursor_pos(Some(col), Some(row));
    let result = buf.place_image(image, 0, ImageProtocol::Sixel, None, None, 0, None, 1, None);
    assert!(matches!(result, PlaceImageResult { .. }));
    id
}

/// `Buffer::new(20, 4)` with a visit to the alternate screen that left `ALT`
/// on its first row and a 3x1 image at screen (5, 2); back on the primary,
/// which holds `P0`..`P5` (so it has scrollback).
fn buffer_with_parked_alternate() -> (Buffer, u64) {
    let mut buf = Buffer::new(20, 4).with_scrollback_limit(50);
    for i in 0..6 {
        line(&mut buf, &format!("P{i}"));
    }
    buf.switch_to_alternate();
    buf.clear_alternate_screen();
    buf.set_cursor_pos(Some(0), Some(0));
    buf.insert_text(&text("ALT"));
    let id = place_image_at(&mut buf, 5, 2, 3, 1);
    buf.set_cursor_pos(Some(0), Some(0));
    buf.switch_to_primary();
    (buf, id)
}

// ── Persistence ──────────────────────────────────────────────────────────

#[test]
fn alternate_rows_and_images_persist_across_switches() {
    let (mut buf, id) = buffer_with_parked_alternate();
    assert_eq!(buf.kind(), BufferType::Primary);
    assert_eq!(buf.image_cell_count, 0, "the primary has no image cells");
    assert!(
        !buf.image_store().contains(id),
        "image is on the alt screen"
    );

    buf.switch_to_alternate();

    assert_eq!(screen_text(&buf, 0), "ALT");
    assert_eq!(buf.image_cell_count, 3);
    assert!(buf.image_store().contains(id));
    buf.debug_assert_invariants();

    // And once more: contents survive every visit, not only the first.
    buf.switch_to_primary();
    buf.switch_to_alternate();
    assert_eq!(screen_text(&buf, 0), "ALT");
    assert_eq!(buf.image_cell_count, 3);
}

#[test]
fn primary_rows_and_images_persist_across_an_alternate_visit() {
    let mut buf = Buffer::new(20, 4).with_scrollback_limit(50);
    for i in 0..6 {
        line(&mut buf, &format!("P{i}"));
    }
    let id = place_image_at(&mut buf, 0, 0, 2, 1);
    let rows_before = buf.rows().len();
    let first_visible = buf.screen_row_index(0).unwrap();
    let first_visible_text = row_text(&buf, first_visible);

    buf.switch_to_alternate();
    assert_eq!(buf.image_cell_count, 0, "alt screen starts without images");
    assert!(!buf.image_store().contains(id));
    buf.switch_to_primary();

    assert_eq!(buf.rows().len(), rows_before);
    assert_eq!(row_text(&buf, first_visible), first_visible_text);
    assert_eq!(buf.image_cell_count, 2);
    assert!(buf.image_store().contains(id));
    buf.debug_assert_invariants();
}

#[test]
fn a_first_ever_alternate_screen_is_blank_height_rows_in_the_alt_namespace() {
    let mut buf = Buffer::new(20, 4);
    buf.switch_to_alternate();

    assert_eq!(buf.rows().len(), 4);
    assert_eq!(buf.row_base(), RowNumber::ALTERNATE_BASE);
    for i in 0..4 {
        assert_eq!(row_text(&buf, i), "");
    }
}

// ── clear_alternate_screen ───────────────────────────────────────────────

#[test]
fn clear_alternate_screen_gives_blank_rows_with_fresh_higher_numbers() {
    let (mut buf, id) = buffer_with_parked_alternate();
    buf.switch_to_alternate();
    let old_base = buf.row_base();
    let old_next = buf.next_row_number();

    buf.clear_alternate_screen();

    assert_eq!(buf.rows().len(), 4);
    for i in 0..4 {
        assert_eq!(row_text(&buf, i), "", "row {i} is blank");
    }
    assert!(
        buf.row_base() >= old_next,
        "blank rows must be numbered past every earlier alt row ({} < {old_next})",
        buf.row_base()
    );
    assert_eq!(
        buf.row_index_of(old_base),
        None,
        "old numbers don't resolve"
    );
    let numbers: Vec<RowNumber> = (0..4).map(|i| buf.row_number_at(i)).collect();
    assert!(
        numbers.windows(2).all(|pair| pair[0] < pair[1]),
        "row numbers are unique and ascending: {numbers:?}"
    );
    assert_eq!(buf.image_cell_count, 0);
    assert!(!buf.image_store().contains(id), "alt image store emptied");
    buf.debug_assert_invariants();
}

#[test]
fn repeated_clears_never_reissue_a_row_number() {
    let mut buf = Buffer::new(20, 3);
    buf.switch_to_alternate();
    let mut previous_next = buf.next_row_number();
    for _ in 0..4 {
        buf.clear_alternate_screen();
        assert!(buf.row_base() >= previous_next);
        previous_next = buf.next_row_number();
    }
}

#[test]
fn clear_alternate_screen_does_not_move_the_cursor() {
    let mut buf = Buffer::new(20, 4);
    buf.switch_to_alternate();
    buf.set_cursor_pos(Some(7), Some(2));
    buf.cursor.font_weight = FontWeight::Bold;

    buf.clear_alternate_screen();

    let pos = buf.cursor_screen_pos();
    assert_eq!((pos.x, pos.y), (7, 2));
    assert_eq!(buf.cursor().font_weight, FontWeight::Bold);
}

#[test]
fn clear_alternate_screen_drops_alternate_marks_but_not_primary_ones() {
    let mut buf = Buffer::new(20, 4);
    buf.mark_prompt_row();
    buf.switch_to_alternate();
    buf.mark_prompt_row();
    assert_eq!(buf.prompt_rows().len(), 2);

    buf.clear_alternate_screen();

    assert_eq!(buf.prompt_rows().len(), 1);
    assert!(!buf.prompt_rows()[0].is_alternate());
}

#[test]
fn clear_alternate_screen_on_the_primary_screen_is_a_no_op() {
    let mut buf = Buffer::new(20, 4);
    line(&mut buf, "keep me");
    let rows_before = buf.rows().len();

    buf.clear_alternate_screen();

    assert_eq!(buf.kind(), BufferType::Primary);
    assert_eq!(buf.rows().len(), rows_before);
    assert_eq!(row_text(&buf, 0), "keep me");
}

// ── The cursor across a switch ───────────────────────────────────────────

#[test]
fn cursor_screen_position_is_kept_in_both_directions_with_scrollback() {
    let mut buf = Buffer::new(20, 4).with_scrollback_limit(50);
    for i in 0..10 {
        line(&mut buf, &format!("P{i}"));
    }
    assert!(buf.screen_row_index(0).unwrap() > 0, "setup: scrollback");
    buf.set_cursor_pos(Some(7), Some(2));

    buf.switch_to_alternate();
    let on_alt = buf.cursor_screen_pos();
    assert_eq!((on_alt.x, on_alt.y), (7, 2));
    assert_eq!(buf.cursor().pos.y, 2, "alt has no scrollback: index == row");

    buf.set_cursor_pos(Some(3), Some(1));
    buf.switch_to_primary();
    let on_primary = buf.cursor_screen_pos();
    assert_eq!((on_primary.x, on_primary.y), (3, 1));
    assert_eq!(buf.cursor().pos.y, buf.screen_row_index(1).unwrap());
}

#[test]
fn cursor_screen_position_is_kept_with_a_one_row_primary() {
    let mut buf = Buffer::new(20, 5);
    assert_eq!(buf.rows().len(), 1, "setup: a fresh primary has one row");

    buf.switch_to_alternate();
    buf.set_cursor_pos(Some(4), Some(3));
    buf.switch_to_primary();

    let pos = buf.cursor_screen_pos();
    assert_eq!((pos.x, pos.y), (4, 3));
    assert!(buf.rows().len() > 3, "the primary grew to hold the cursor");
    buf.debug_assert_invariants();
}

#[test]
fn cursor_attributes_are_kept_across_a_switch() {
    let mut buf = Buffer::new(20, 4);
    buf.cursor.font_weight = FontWeight::Bold;

    buf.switch_to_alternate();
    assert_eq!(buf.cursor().font_weight, FontWeight::Bold);
    buf.cursor.font_weight = FontWeight::Normal;
    buf.switch_to_primary();
    assert_eq!(buf.cursor().font_weight, FontWeight::Normal);
}

// ── Margins ──────────────────────────────────────────────────────────────

#[test]
fn margins_are_untouched_by_a_switch_in_either_direction() {
    let mut buf = Buffer::new(20, 8);
    buf.set_scroll_region(3, 6);
    assert_eq!(buf.scroll_region(), (2, 5));

    buf.switch_to_alternate();
    assert_eq!(buf.scroll_region(), (2, 5), "primary region survives entry");

    buf.set_scroll_region(2, 4);
    buf.switch_to_primary();
    assert_eq!(buf.scroll_region(), (1, 3), "margins are one shared pair");
}

// ── DECSC per screen ─────────────────────────────────────────────────────

#[test]
fn the_decsc_slot_is_per_screen() {
    let mut buf = Buffer::new(20, 8);
    buf.set_cursor_pos(Some(2), Some(3));
    buf.save_cursor();

    buf.switch_to_alternate();
    assert!(buf.saved_cursor.is_none(), "alt does not inherit the slot");
    buf.set_cursor_pos(Some(5), Some(6));
    buf.save_cursor();

    buf.switch_to_primary();
    buf.restore_cursor();
    let on_primary = buf.cursor_screen_pos();
    assert_eq!((on_primary.x, on_primary.y), (2, 3));

    buf.switch_to_alternate();
    buf.restore_cursor();
    let on_alt = buf.cursor_screen_pos();
    assert_eq!((on_alt.x, on_alt.y), (5, 6));
}

#[test]
fn decrc_with_nothing_saved_on_the_other_screen_homes_the_cursor() {
    let mut buf = Buffer::new(20, 8);
    buf.set_cursor_pos(Some(2), Some(3));
    buf.save_cursor();

    buf.switch_to_alternate();
    buf.set_cursor_pos(Some(9), Some(1));
    buf.restore_cursor();

    let pos = buf.cursor_screen_pos();
    assert_eq!(
        (pos.x, pos.y),
        (0, 0),
        "the alt slot is empty, so DECRC homes the cursor (131.C2)"
    );
}

#[test]
fn decrc_with_nothing_saved_turns_decom_off_without_a_save() {
    let mut buf = Buffer::new(20, 8);
    buf.set_scroll_region(3, 6);
    buf.set_decom(Decom::OriginMode);
    buf.set_cursor_pos(Some(4), Some(1));

    buf.restore_cursor();

    assert_eq!(buf.is_decom_enabled(), Decom::NormalCursor);
    let pos = buf.cursor_screen_pos();
    assert_eq!(
        (pos.x, pos.y),
        (0, 0),
        "home is screen home, not the margin"
    );
}

#[test]
fn decrc_restores_the_saved_decom_state_without_homing() {
    let mut buf = Buffer::new(20, 8);
    buf.set_scroll_region(3, 6);
    buf.set_decom(Decom::OriginMode);
    buf.set_cursor_pos(Some(4), Some(1));
    buf.save_cursor();
    let saved = buf.cursor_screen_pos();

    buf.set_decom(Decom::NormalCursor);
    buf.set_cursor_pos(Some(0), Some(7));
    buf.restore_cursor();

    assert_eq!(buf.is_decom_enabled(), Decom::OriginMode);
    assert_eq!(buf.cursor_screen_pos(), saved, "DECRC did not home");
}

// ── Idempotence ──────────────────────────────────────────────────────────

#[test]
fn double_switch_to_alternate_is_a_no_op() {
    let mut buf = Buffer::new(20, 4);
    buf.switch_to_alternate();
    buf.insert_text(&text("x"));
    let rows = buf.rows().len();
    let base = buf.row_base();
    let cursor = buf.cursor().clone();

    buf.switch_to_alternate();

    assert_eq!(buf.kind(), BufferType::Alternate);
    assert_eq!(buf.rows().len(), rows);
    assert_eq!(buf.row_base(), base);
    assert_eq!(buf.cursor().pos, cursor.pos);
    assert_eq!(row_text(&buf, 0), "x");
    assert!(buf.parked_primary.is_some(), "primary is still parked");
    buf.debug_assert_invariants();
}

#[test]
fn double_switch_to_primary_is_a_no_op() {
    let (mut buf, _) = buffer_with_parked_alternate();
    let rows = buf.rows().len();
    let cursor = buf.cursor().clone();

    buf.switch_to_primary();

    assert_eq!(buf.kind(), BufferType::Primary);
    assert_eq!(buf.rows().len(), rows);
    assert_eq!(buf.cursor().pos, cursor.pos);
    assert!(buf.parked_alternate.is_some(), "alt is still parked");
    assert!(buf.parked_primary.is_none());
    buf.debug_assert_invariants();
}

#[test]
fn switch_to_primary_without_ever_entering_the_alternate_is_a_no_op() {
    let mut buf = Buffer::new(20, 4);
    line(&mut buf, "p");
    buf.switch_to_primary();
    assert_eq!(buf.kind(), BufferType::Primary);
    assert!(buf.parked_alternate.is_none());
    assert_eq!(row_text(&buf, 0), "p");
}

// ── Marks ────────────────────────────────────────────────────────────────

#[test]
fn alternate_marks_are_dropped_on_leave_even_though_the_rows_persist() {
    let mut buf = Buffer::new(30, 4);
    buf.mark_prompt_row();
    buf.switch_to_alternate();
    buf.mark_prompt_row();
    buf.insert_text(&text("kept"));

    buf.switch_to_primary();

    assert_eq!(buf.prompt_rows().len(), 1, "alt mark dropped on leave");
    buf.switch_to_alternate();
    assert_eq!(screen_text(&buf, 0), "kept", "the rows persisted");
    assert_eq!(buf.prompt_rows().len(), 1, "stale mark not resurrected");
}

// ── Resizing a parked screen ─────────────────────────────────────────────

/// A 10x4 buffer whose parked alternate screen holds a full first row and a
/// 3x1 image at screen (7, 1), with the alternate cursor left on row 0.
fn buffer_with_wide_parked_alternate() -> Buffer {
    let mut buf = Buffer::new(10, 4);
    buf.switch_to_alternate();
    buf.clear_alternate_screen();
    buf.set_cursor_pos(Some(0), Some(0));
    buf.insert_text(&text("ABCDEFGHIJ"));
    let _ = place_image_at(&mut buf, 7, 1, 3, 1);
    buf.set_cursor_pos(Some(0), Some(0));
    buf.switch_to_primary();
    buf
}

#[test]
fn resizing_the_primary_clips_the_parked_alternate_width() {
    let mut buf = buffer_with_wide_parked_alternate();

    let _ = buf.set_size(6, 4, 0);

    let parked = buf.parked_alternate.as_ref().unwrap();
    assert_eq!(parked.rows.len(), 4);
    assert!(parked.rows.iter().all(|row| row.max_width() == 6));
    assert_eq!(parked.image_cell_count, 0, "the clipped image cells went");
    buf.debug_assert_invariants();

    buf.switch_to_alternate();
    assert_eq!(screen_text(&buf, 0), "ABCDEF");
    assert_eq!(buf.rows().len(), buf.terminal_height());
}

#[test]
fn resizing_the_primary_shrinks_the_parked_alternate_height() {
    let mut buf = buffer_with_wide_parked_alternate();
    buf.switch_to_alternate();
    buf.set_cursor_pos(Some(0), Some(3));
    buf.insert_text(&text("TAIL"));
    buf.switch_to_primary();

    let _ = buf.set_size(10, 2, 0);

    let parked = buf.parked_alternate.as_ref().unwrap();
    assert_eq!(parked.rows.len(), 2);
    assert_eq!(parked.image_cell_count, 0, "the image row was trimmed");
    buf.debug_assert_invariants();

    // An alternate screen keeps its bottom rows on a shrink.
    buf.switch_to_alternate();
    assert_eq!(buf.rows().len(), 2);
    assert_eq!(screen_text(&buf, 1), "TAIL");
}

#[test]
fn resizing_the_primary_grows_the_parked_alternate() {
    let mut buf = buffer_with_wide_parked_alternate();

    let _ = buf.set_size(14, 7, 0);

    let parked = buf.parked_alternate.as_ref().unwrap();
    assert_eq!(parked.rows.len(), 7);
    assert!(parked.rows.iter().all(|row| row.max_width() == 14));
    buf.debug_assert_invariants();

    buf.switch_to_alternate();
    assert_eq!(buf.rows().len(), 7);
    assert_eq!(screen_text(&buf, 0), "ABCDEFGHIJ");
    assert_eq!(screen_text(&buf, 6), "");
    assert_eq!(buf.image_cell_count, 3, "the image survived a grow");
    buf.debug_assert_invariants();
}

#[test]
fn resizing_the_alternate_resizes_the_parked_primary_and_alt_stays_consistent() {
    let (mut buf, _) = buffer_with_parked_alternate();
    buf.switch_to_alternate();

    let _ = buf.set_size(30, 6, 0);
    buf.switch_to_primary();
    buf.switch_to_alternate();

    assert_eq!(buf.rows().len(), 6);
    assert_eq!(screen_text(&buf, 0), "ALT");
    buf.switch_to_primary();
    assert_eq!(buf.terminal_width(), 30);
    assert!(buf.rows().iter().all(|row| row.max_width() == 30));
}

#[test]
fn a_parked_alternate_is_resized_even_when_only_the_height_changes() {
    let mut buf = buffer_with_wide_parked_alternate();

    let _ = buf.set_size(10, 5, 0);

    assert_eq!(buf.parked_alternate.as_ref().unwrap().rows.len(), 5);
    buf.debug_assert_invariants();
}

// ── RIS ──────────────────────────────────────────────────────────────────

#[test]
fn full_reset_from_the_primary_drops_the_parked_alternate_and_keeps_numbering() {
    let (mut buf, _) = buffer_with_parked_alternate();
    buf.set_cursor_pos(Some(1), Some(1));
    buf.save_cursor();
    let primary_next = buf.next_row_number();
    let parked_alt_next = buf.parked_alternate.as_ref().unwrap().rows.next_number();

    buf.full_reset();

    assert!(buf.parked_alternate.is_none());
    assert!(buf.parked_primary.is_none());
    assert!(buf.saved_cursor.is_none());
    assert!(
        buf.row_base() >= primary_next,
        "primary numbering continues"
    );
    buf.switch_to_alternate();
    assert!(
        buf.row_base() >= parked_alt_next,
        "alternate numbering continues past the discarded parked screen"
    );
    assert_eq!(screen_text(&buf, 0), "", "the old alt contents are gone");
    assert!(buf.saved_cursor.is_none(), "the alt DECSC slot is reset");
}

#[test]
fn full_reset_from_the_alternate_keeps_both_namespaces_monotonic() {
    let (mut buf, _) = buffer_with_parked_alternate();
    let primary_next = buf.next_row_number();
    buf.switch_to_alternate();
    let alt_next = buf.next_row_number();

    buf.full_reset();

    assert_eq!(buf.kind(), BufferType::Primary);
    assert!(buf.parked_primary.is_none());
    assert!(buf.parked_alternate.is_none());
    assert!(buf.row_base() >= primary_next);
    buf.switch_to_alternate();
    assert!(buf.row_base() >= alt_next);
    buf.debug_assert_invariants();
}

// ── Parking is a move, not a copy ────────────────────────────────────────

#[test]
fn parking_the_primary_moves_its_image_store() {
    let mut buf = Buffer::new(20, 4);
    let id = place_image_at(&mut buf, 0, 0, 2, 1);

    buf.switch_to_alternate();

    let parked = buf.parked_primary.as_ref().unwrap();
    assert!(parked.image_store.contains(id));
    assert_eq!(parked.image_cell_count, 2);
    assert!(
        !buf.image_store().contains(id),
        "the active store is the alternate's, not a clone of the primary's"
    );
    let blank = Row::new(20);
    assert_eq!(buf.rows()[0].characters(), blank.characters());
}
