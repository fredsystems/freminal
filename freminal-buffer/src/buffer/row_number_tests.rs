// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Task 125.14 regression tests: logical row numbers.
//!
//! Every stored row reference that used to be a retained *index* (and so
//! either had to be rewritten on eviction or silently drifted onto another
//! row) is now a stable [`RowNumber`]. These tests pin, per holder, that the
//! reference stays attached to its row across eviction, resize and the
//! alternate screen, and that numbers are never re-issued.

use freminal_common::buffer_states::{row_number::RowNumber, tchar::TChar};

use crate::{
    buffer::{Buffer, PlaceImageResult},
    cell::Cell,
    image_store::{AnimationControl, ImageProtocol, ImageSizeMode, InlineImage, next_image_id},
};

use std::sync::Arc;

fn text(s: &str) -> Vec<TChar> {
    s.chars().map(|c| TChar::Ascii(c as u8)).collect()
}

/// Write `s` on the cursor row and move to the start of the next row.
fn line(buf: &mut Buffer, s: &str) {
    buf.insert_text(&text(s));
    buf.handle_lf();
    buf.handle_cr();
}

/// The text of retained row `index`, trailing blanks trimmed.
fn row_text(buf: &Buffer, index: usize) -> String {
    let text: String = buf.rows()[index]
        .cells()
        .iter()
        .map(Cell::into_utf8)
        .collect();
    text.trim_end().to_owned()
}

/// The text of the row numbered `number`, or `None` if it is not retained.
fn text_of_number(buf: &Buffer, number: RowNumber) -> Option<String> {
    buf.row_index_of(number).map(|i| row_text(buf, i))
}

/// A buffer small enough that a few dozen lines overflow its scrollback:
/// 3 visible rows + 5 scrollback = 8 retained rows at most.
fn small_buffer() -> Buffer {
    Buffer::new(20, 3).with_scrollback_limit(5)
}

fn image(cols: usize, rows: usize) -> InlineImage {
    InlineImage {
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
    }
}

fn place(buf: &mut Buffer, cols: usize, rows: usize) -> PlaceImageResult {
    buf.place_image(
        image(cols, rows),
        0,
        ImageProtocol::Sixel,
        None,
        None,
        0,
        None,
        1,
        None,
    )
}

// ── RowStore numbering through the Buffer API ────────────────────────────

#[test]
fn fresh_buffer_numbers_rows_from_zero() {
    let buf = Buffer::new(20, 3);
    assert_eq!(buf.row_base(), RowNumber::ZERO);
    assert_eq!(buf.next_row_number(), RowNumber::new(1));
    assert_eq!(buf.row_number_at(0), RowNumber::ZERO);
    assert_eq!(buf.cursor_row_number(), RowNumber::ZERO);
    assert_eq!(buf.row_index_of(RowNumber::ZERO), Some(0));
    assert_eq!(buf.row_index_of(RowNumber::new(1)), None, "not created yet");
}

#[test]
fn row_numbers_are_never_reused_after_eviction() {
    let mut buf = small_buffer();
    let first = buf.cursor_row_number();
    line(&mut buf, "first");

    let mut highest_issued = buf.next_row_number();
    let mut last_base = buf.row_base();
    for i in 0..40 {
        line(&mut buf, &format!("line {i}"));
        // Numbers only ever move forward...
        assert!(buf.row_base() >= last_base, "base moved backwards");
        assert!(
            buf.next_row_number() >= highest_issued,
            "a number was re-issued after eviction"
        );
        last_base = buf.row_base();
        highest_issued = buf.next_row_number();
        // ...and number <-> index stay consistent.
        for idx in 0..buf.rows().len() {
            assert_eq!(buf.row_index_of(buf.row_number_at(idx)), Some(idx));
        }
    }

    assert!(
        buf.row_base() > first,
        "40 lines at a limit of 8 rows must have evicted the first row"
    );
    assert_eq!(
        buf.row_index_of(first),
        None,
        "an evicted row's number must not resolve to some other row"
    );
}

#[test]
fn rows_keep_their_number_while_eviction_shifts_their_index() {
    let mut buf = small_buffer();
    for i in 0..4 {
        line(&mut buf, &format!("pad {i}"));
    }
    let marker_row = buf.cursor_row_number();
    line(&mut buf, "marker");
    let index_before = buf.row_index_of(marker_row).unwrap();

    for i in 0..4 {
        line(&mut buf, &format!("more {i}"));
    }

    assert_eq!(
        text_of_number(&buf, marker_row).as_deref(),
        Some("marker"),
        "the number must still name the marker row"
    );
    let index_after = buf.row_index_of(marker_row).unwrap();
    assert!(
        index_after < index_before,
        "eviction must have shifted the marker's retained index down \
         ({index_before} -> {index_after})"
    );
}

// ── DECSC saved cursor ───────────────────────────────────────────────────
//
// Task 125.C6: DECSC saves a *screen* position (xterm `CursorSave` stores
// `screen->cur_row`; the VT510 manual defines it as "cursor position" on a
// terminal without scrollback), so a restore never depends on which content
// currently occupies the saved row, nor on eviction, scrolling or reflow.

/// The first row of the visible (bottom-anchored) window.
fn window_top(buf: &Buffer) -> usize {
    buf.rows().len().saturating_sub(buf.terminal_height())
}

#[test]
fn decsc_restores_the_same_screen_position_after_output_scrolls_the_screen() {
    let mut buf = Buffer::new(20, 3).with_scrollback_limit(50);
    buf.set_cursor_pos(Some(4), Some(1));
    buf.save_cursor();
    let saved_screen = buf.cursor_screen_pos();
    assert_eq!((saved_screen.x, saved_screen.y), (4, 1));

    // Scroll the screen several times: the saved row's content is now in
    // scrollback, retained and not evicted.
    for i in 0..10 {
        line(&mut buf, &format!("out {i}"));
    }
    assert_eq!(buf.row_base(), RowNumber::ZERO, "nothing is evicted");
    assert!(window_top(&buf) > 1, "setup must scroll past the saved row");

    buf.restore_cursor();

    let restored = buf.cursor_screen_pos();
    assert_eq!(
        (restored.x, restored.y),
        (4, 1),
        "DECRC must return to the screen position, not follow the old content \
         into scrollback"
    );
    assert_eq!(buf.cursor().pos.y, window_top(&buf) + 1);
}

#[test]
fn decsc_restores_the_same_screen_position_after_scrollback_eviction() {
    let mut buf = small_buffer();
    buf.set_cursor_pos(Some(2), Some(2));
    buf.save_cursor();
    for i in 0..30 {
        line(&mut buf, &format!("out {i}"));
    }
    assert!(
        buf.row_base() > RowNumber::ZERO,
        "setup must evict rows from the front"
    );
    assert!(
        window_top(&buf) > 0,
        "setup needs scrollback above the window"
    );

    buf.restore_cursor();

    let restored = buf.cursor_screen_pos();
    assert_eq!((restored.x, restored.y), (2, 2));
    assert_eq!(buf.cursor().pos.y, window_top(&buf) + 2);
}

#[test]
fn decsc_at_the_bottom_row_restores_to_the_bottom_row_after_scrolling() {
    let mut buf = Buffer::new(20, 4).with_scrollback_limit(50);
    buf.set_cursor_pos(Some(0), Some(3));
    buf.insert_text(&text("marker"));
    buf.save_cursor();
    for i in 0..9 {
        line(&mut buf, &format!("out {i}"));
    }
    buf.set_cursor_pos(Some(0), Some(0));

    buf.restore_cursor();

    assert_eq!(buf.cursor_screen_pos().y, 3);
    assert_eq!(
        buf.cursor().pos.y,
        buf.rows().len() - 1,
        "bottom screen row is the last retained row"
    );
    assert_ne!(
        row_text(&buf, buf.cursor().pos.y),
        "marker",
        "the restore does not follow the content that was under the cursor"
    );
}

#[test]
fn decsc_does_not_follow_content_when_scrollback_is_evicted_between_save_and_restore() {
    // The pre-C6 behaviour re-attached the cursor to the row that held it at
    // DECSC. The reference behaviour does not: after the screen scrolls, the
    // saved screen row holds different content.
    let mut buf = small_buffer();
    for i in 0..12 {
        line(&mut buf, &format!("pad {i}"));
    }
    buf.insert_text(&text("marker"));
    buf.save_cursor();
    let saved_screen_y = buf.cursor_screen_pos().y;

    buf.handle_lf();
    buf.handle_cr();
    buf.insert_text(&text("later"));
    buf.restore_cursor();

    assert_eq!(buf.cursor_screen_pos().y, saved_screen_y);
    assert_ne!(
        row_text(&buf, buf.cursor().pos.y),
        "marker",
        "the marker row scrolled up by one; the screen position did not"
    );
}

#[test]
fn decsc_restore_clamps_to_the_screen_after_the_height_shrinks() {
    let mut buf = Buffer::new(20, 8);
    buf.set_cursor_pos(Some(15), Some(7));
    buf.save_cursor();

    let _ = buf.set_size(10, 4, 0);
    buf.restore_cursor();

    let restored = buf.cursor_screen_pos();
    assert_eq!(restored.y, 3, "row clamps to the new bottom screen row");
    assert_eq!(restored.x, 9, "column clamps to the new right edge");
    assert!(
        buf.cursor().pos.y >= window_top(&buf),
        "never restored into off-screen scrollback"
    );
}

#[test]
fn decsc_restore_after_a_height_grow_keeps_the_screen_row() {
    let mut buf = Buffer::new(20, 5);
    buf.set_cursor_pos(Some(0), Some(4));
    buf.save_cursor();

    // Moving up and growing reclaims the trailing padding below the cursor.
    buf.set_cursor_pos(Some(0), Some(2));
    let _ = buf.set_size(20, 8, 0);

    buf.restore_cursor();

    assert_eq!(
        buf.cursor_screen_pos().y,
        4,
        "the saved screen row is kept; the rows needed to hold it are created"
    );
    assert!(buf.cursor().pos.y < buf.rows().len());
}

#[test]
fn decsc_survives_reflow_as_a_screen_position() {
    let mut buf = Buffer::new(20, 8);
    line(&mut buf, "head");
    buf.insert_text(&text("0123456789ABCDEFGHIJ0123456789ABCDEFGHIJ"));
    buf.handle_lf();
    buf.handle_cr();
    buf.set_cursor_pos(Some(3), Some(1));
    buf.save_cursor();
    let saved = buf.cursor_screen_pos();

    buf.set_size(10, 8, 0);
    buf.restore_cursor();

    let restored = buf.cursor_screen_pos();
    assert_eq!((restored.x, restored.y), (saved.x, saved.y));
}

#[test]
fn decsc_made_on_the_primary_screen_restores_the_screen_position_on_the_alternate_screen() {
    // The 1049 shape: save on primary, switch, restore. The position is a
    // screen position, so it is meaningful on either screen.
    let mut buf = Buffer::new(20, 5);
    for i in 0..3 {
        line(&mut buf, &format!("row {i}"));
    }
    buf.set_cursor_pos(Some(6), Some(2));
    buf.save_cursor();

    buf.enter_alternate(0);
    buf.restore_cursor();

    let restored = buf.cursor_screen_pos();
    assert_eq!((restored.x, restored.y), (6, 2));
}

#[test]
fn decsc_save_before_1049_and_restore_after_leaving_returns_to_the_primary_position() {
    // ?1049h = save cursor, enter alternate; ?1049l = leave, restore cursor.
    let mut buf = Buffer::new(20, 5).with_scrollback_limit(50);
    for i in 0..9 {
        line(&mut buf, &format!("row {i}"));
    }
    buf.set_cursor_pos(Some(7), Some(3));
    buf.save_cursor();

    buf.enter_alternate(0);
    buf.set_cursor_pos(Some(0), Some(0));
    buf.insert_text(&text("alt"));
    let _ = buf.leave_alternate();
    buf.restore_cursor();

    let restored = buf.cursor_screen_pos();
    assert_eq!((restored.x, restored.y), (7, 3));
    assert_eq!(buf.cursor().pos.y, window_top(&buf) + 3);
}

// ── Image placement origin ───────────────────────────────────────────────

#[test]
fn place_image_origin_stays_attached_to_its_row_across_eviction() {
    let mut buf = small_buffer();
    for i in 0..2 {
        line(&mut buf, &format!("pad {i}"));
    }
    let result = place(&mut buf, 2, 1);

    let index_at_placement = buf.row_index_of(result.origin_row).unwrap();
    assert!(buf.rows()[index_at_placement].cells()[0].has_image());

    for i in 0..5 {
        line(&mut buf, &format!("out {i}"));
    }
    assert!(buf.row_base() > RowNumber::ZERO, "setup must evict rows");

    let index_now = buf
        .row_index_of(result.origin_row)
        .expect("the image's row is still retained");
    assert!(index_now < index_at_placement, "eviction shifted the index");
    assert!(
        buf.rows()[index_now].cells()[0].has_image(),
        "the recorded origin number must still name the image's row"
    );
}

#[test]
fn place_image_origin_is_a_number_not_a_post_drain_index() {
    // Placing an image that overflows the scrollback drains rows DURING the
    // placement. The origin is the number of the row the image was stamped
    // on; it must resolve to that row afterwards, wherever the drain left it.
    let mut buf = small_buffer(); // 3 visible + 5 scrollback = 8 rows max
    for i in 0..6 {
        line(&mut buf, &format!("pad {i}"));
    }
    assert_eq!(buf.row_base(), RowNumber::ZERO, "no eviction yet");
    let expected = buf.cursor_row_number();
    assert_eq!(expected, RowNumber::new(6));

    // 3 rows stamped at 6..9 -> 9 rows (one drained); the row appended below
    // the image drains one more.
    let result = place(&mut buf, 1, 3);

    assert_eq!(result.origin_row, expected);
    assert_eq!(
        buf.row_base(),
        RowNumber::new(2),
        "the placement itself must have evicted two rows"
    );
    let origin_index = buf
        .row_index_of(result.origin_row)
        .expect("the image's top row is still retained");
    assert_eq!(origin_index, 4, "origin number 6 - base 2");
    for i in 0..3 {
        assert!(
            buf.rows()[origin_index + i].cells()[0].has_image(),
            "image row {i} must sit at the row resolved from origin_row"
        );
    }
    assert!(
        !buf.rows()[origin_index + 3]
            .cells()
            .first()
            .is_some_and(Cell::has_image),
        "the row below the image is blank"
    );
    // The cursor is parked below the image, column 0.
    assert_eq!(buf.cursor().pos.y, origin_index + 3);
    assert_eq!(buf.cursor().pos.x, 0);
    assert_eq!(buf.cursor_row_number(), result.origin_row.saturating_add(3));
}

// ── Prompt marks and command blocks ──────────────────────────────────────

/// Start a block at the cursor row, write the prompt text, record the
/// command-start and output-start markers on the following rows and finish it.
fn block(buf: &mut Buffer, fid: &str) {
    buf.mark_prompt_row();
    let _ = buf.start_command_block(None, fid.to_owned());
    line(buf, &format!("{fid}$ cmd"));
    buf.mark_command_start_row(fid);
    buf.mark_output_start_row(fid);
    line(buf, &format!("{fid} out"));
    let _ = buf.finish_command_block(Some(0), fid);
}

#[test]
fn command_block_and_prompt_stay_attached_to_their_rows_across_eviction() {
    let mut buf = Buffer::new(30, 3).with_scrollback_limit(12);
    // A few rows above the block, so that evicting some rows from the front
    // leaves the block's own rows in place.
    for i in 0..4 {
        line(&mut buf, &format!("pad {i}"));
    }
    block(&mut buf, "keep");
    let kept = buf.command_blocks()[0].clone();
    let prompt = buf.prompt_rows()[0];

    for i in 0..10 {
        line(&mut buf, &format!("filler {i}"));
    }
    assert!(buf.row_base() > RowNumber::ZERO, "setup must evict rows");

    assert_eq!(buf.command_blocks().len(), 1, "block survives");
    let b = &buf.command_blocks()[0];
    // The stored numbers are untouched by eviction...
    assert_eq!(b.prompt_start_row, kept.prompt_start_row);
    assert_eq!(b.end_row, kept.end_row);
    assert_eq!(buf.prompt_rows()[0], prompt);
    // ...and still name the rows that hold the block's text.
    assert_eq!(
        text_of_number(&buf, b.prompt_start_row).as_deref(),
        Some("keep$ cmd")
    );
    assert_eq!(
        text_of_number(&buf, b.output_start_row.unwrap()).as_deref(),
        Some("keep out")
    );
}

#[test]
fn evicting_a_blocks_prompt_row_drops_the_block_and_its_mark() {
    let mut buf = small_buffer();
    block(&mut buf, "gone");
    assert_eq!(buf.command_blocks().len(), 1);
    assert_eq!(buf.prompt_rows().len(), 1);

    for i in 0..30 {
        line(&mut buf, &format!("filler {i}"));
    }

    assert!(buf.command_blocks().is_empty(), "evicted block is dropped");
    assert!(
        buf.prompt_rows().is_empty(),
        "evicted prompt mark is dropped"
    );
}

#[test]
fn eviction_pruning_only_trims_the_leading_run_of_stale_marks() {
    let mut buf = small_buffer();
    for i in 0..6 {
        line(&mut buf, &format!("row {i}"));
    }
    // An out-of-order mark list: a high mark first, then a low one.
    buf.cursor.pos.y = 6;
    buf.mark_prompt_row();
    buf.cursor.pos.y = 0;
    buf.mark_prompt_row();
    let high = buf.prompt_rows()[0];
    let low = buf.prompt_rows()[1];
    assert!(high > low);

    for i in 0..20 {
        line(&mut buf, &format!("more {i}"));
    }

    // `low` is evicted but sits behind the (also evicted) `high`; both are
    // below the base, so both are trimmed here. Re-add them in the stale
    // order with a retained mark in front to pin the documented behaviour.
    buf.prompt_rows.clear();
    let retained = buf.row_number_at(buf.rows().len() - 1);
    buf.prompt_rows.push(retained);
    buf.prompt_rows.push(low);
    buf.prune_evicted_marks();

    assert_eq!(
        buf.prompt_rows().len(),
        2,
        "stale mark behind a retained one stays"
    );
    assert_eq!(buf.row_index_of(low), None, "and is filtered by consumers");
    assert!(buf.row_index_of(retained).is_some());
}

// ── Alternate screen ─────────────────────────────────────────────────────

#[test]
fn alternate_screen_uses_its_own_row_namespace() {
    let mut buf = Buffer::new(20, 3);
    line(&mut buf, "primary");
    let primary_row = buf.row_number_at(0);

    buf.enter_alternate(0);
    assert!(buf.row_base().is_alternate());
    assert_eq!(buf.row_base(), RowNumber::ALTERNATE_BASE);
    assert!(buf.cursor_row_number().is_alternate());
    assert_eq!(
        buf.row_index_of(primary_row),
        None,
        "a primary number must not resolve on the alternate screen"
    );

    let _ = buf.leave_alternate();
    assert!(!buf.row_base().is_alternate());
    assert_eq!(
        text_of_number(&buf, primary_row).as_deref(),
        Some("primary")
    );
}

#[test]
fn alternate_row_numbers_are_not_reused_across_sessions() {
    let mut buf = Buffer::new(20, 3);
    buf.enter_alternate(0);
    let first_session_top = buf.row_base();
    let first_session_next = buf.next_row_number();
    let _ = buf.leave_alternate();

    buf.enter_alternate(0);
    assert!(
        buf.row_base() >= first_session_next,
        "second session must start past the first ({} < {})",
        buf.row_base(),
        first_session_next
    );
    assert!(buf.row_base() > first_session_top);
    assert_eq!(buf.row_index_of(first_session_top), None);
}

#[test]
fn alt_screen_ed2_keeps_primary_command_blocks() {
    let mut buf = Buffer::new(30, 4);
    block(&mut buf, "primary");
    assert_eq!(buf.command_blocks().len(), 1);
    assert_eq!(buf.prompt_rows().len(), 1);

    buf.enter_alternate(0);
    buf.erase_display();

    assert_eq!(
        buf.command_blocks().len(),
        1,
        "ED 2 on the alternate screen must not drop a primary-screen block"
    );
    assert_eq!(buf.prompt_rows().len(), 1);
    // The block is still the primary one and resolves once back on primary.
    let _ = buf.leave_alternate();
    assert_eq!(
        text_of_number(&buf, buf.command_blocks()[0].prompt_start_row).as_deref(),
        Some("primary$ cmd")
    );
}

#[test]
fn primary_ed2_still_drops_blocks_anchored_on_the_visible_screen() {
    let mut buf = Buffer::new(30, 4);
    block(&mut buf, "visible");
    buf.erase_display();
    assert!(buf.command_blocks().is_empty());
    assert_eq!(buf.prompt_rows(), &[]);
}

#[test]
fn alt_era_marks_are_dropped_when_the_alternate_screen_is_left() {
    let mut buf = Buffer::new(30, 4);
    block(&mut buf, "primary");

    buf.enter_alternate(0);
    block(&mut buf, "alt");
    assert_eq!(buf.command_blocks().len(), 2);
    assert_eq!(buf.prompt_rows().len(), 2);
    assert!(buf.prompt_rows()[1].is_alternate());

    let _ = buf.leave_alternate();

    assert_eq!(buf.command_blocks().len(), 1, "alt block dropped");
    assert_eq!(buf.command_blocks()[0].fid, "primary");
    assert_eq!(buf.prompt_rows().len(), 1, "alt prompt mark dropped");
    assert!(!buf.prompt_rows()[0].is_alternate());
}

#[test]
fn a_block_that_straddles_the_alternate_screen_loses_only_its_alt_fields() {
    let mut buf = Buffer::new(30, 4);
    buf.mark_prompt_row();
    let _ = buf.start_command_block(None, "straddle".to_owned());
    line(&mut buf, "straddle$ vim");
    buf.mark_command_start_row("straddle");

    buf.enter_alternate(0);
    buf.mark_output_start_row("straddle");
    let _ = buf.finish_command_block(Some(0), "straddle");
    let _ = buf.leave_alternate();

    let b = &buf.command_blocks()[0];
    assert!(!b.prompt_start_row.is_alternate());
    assert!(b.command_start_row.is_some_and(|r| !r.is_alternate()));
    assert_eq!(b.output_start_row, None, "alt-namespace field cleared");
    assert_eq!(b.end_row, None, "alt-namespace field cleared");
}

// ── Resize / reflow ──────────────────────────────────────────────────────

/// A block whose output is `output_len` digits wide, on a 20-column buffer.
fn long_block(buf: &mut Buffer, fid: &str, output_len: usize) {
    buf.mark_prompt_row();
    let _ = buf.start_command_block(None, fid.to_owned());
    line(buf, &format!("{fid}$ cmd"));
    buf.mark_command_start_row(fid);
    buf.mark_output_start_row(fid);
    let output: String = "0123456789".chars().cycle().take(output_len).collect();
    buf.insert_text(&text(&output));
    let _ = buf.finish_command_block(Some(0), fid);
    buf.handle_lf();
    buf.handle_cr();
}

#[test]
fn reflow_renumbers_rows_and_remaps_every_holder() {
    let mut buf = Buffer::new(20, 6);
    long_block(&mut buf, "r", 55);
    buf.save_cursor();
    let old_base = buf.row_base();
    let old_next = buf.next_row_number();
    let old_prompt = buf.prompt_rows()[0];
    let old_end = buf.command_blocks()[0].end_row.unwrap();

    buf.set_size(10, 6, 0);

    // Fresh numbers: everything from before the reflow is below the new base.
    assert!(
        buf.row_base() >= old_next,
        "reflow installs rows at fresh numbers"
    );
    assert_eq!(buf.row_index_of(old_prompt), None);
    assert_eq!(buf.row_index_of(old_end), None);
    assert!(old_base < buf.row_base());

    // Prompt mark -> the row holding the prompt text.
    assert_eq!(
        text_of_number(&buf, buf.prompt_rows()[0]).as_deref(),
        Some("r$ cmd")
    );
    // Block fields -> the rows holding the same content.
    let b = &buf.command_blocks()[0];
    assert_eq!(
        text_of_number(&buf, b.prompt_start_row).as_deref(),
        Some("r$ cmd")
    );
    // (The fixture records `B` after the line feed, so it shares the output
    // row; see `long_block`.)
    assert_eq!(b.command_start_row, b.output_start_row);
    assert!(
        text_of_number(&buf, b.output_start_row.unwrap())
            .unwrap()
            .starts_with("0123456789"),
        "output start follows the first output row"
    );
    // The inclusive end follows the LAST piece of the re-wrapped output (55
    // digits at width 10 end on "45678" / "...").
    let end_text = text_of_number(&buf, b.end_row.unwrap()).unwrap();
    assert!(
        end_text.ends_with('4'),
        "end_row must name the last output row, got {end_text:?}"
    );
    // The saved cursor row is remapped too (it was on the row after the
    // output, i.e. an empty row).
    buf.restore_cursor();
    assert!(buf.cursor().pos.y < buf.rows().len());
}

#[test]
fn take_reflow_remap_translates_old_numbers_and_is_one_shot() {
    let mut buf = Buffer::new(20, 6);
    long_block(&mut buf, "r", 45);
    let old_prompt = buf.prompt_rows()[0];
    let old_end = buf.command_blocks()[0].end_row.unwrap();
    assert!(buf.take_reflow_remap().is_none(), "no reflow yet");

    buf.set_size(10, 6, 0);
    let remap = buf
        .take_reflow_remap()
        .expect("a width change must produce a remap");

    let new_prompt = remap.map_start(old_prompt).unwrap();
    assert_eq!(text_of_number(&buf, new_prompt).as_deref(), Some("r$ cmd"));
    assert_eq!(
        new_prompt,
        buf.prompt_rows()[0],
        "buffer applied the same remap"
    );
    assert_eq!(
        remap.map_end(old_end),
        buf.command_blocks()[0].end_row,
        "end anchors agree"
    );
    assert!(buf.take_reflow_remap().is_none(), "taking is one-shot");
}

#[test]
fn height_only_resize_produces_no_remap() {
    let mut buf = Buffer::new(20, 6);
    long_block(&mut buf, "r", 45);
    buf.set_size(20, 8, 0);
    assert!(buf.take_reflow_remap().is_none());
}

#[test]
fn successive_reflows_chain_into_one_remap() {
    let mut buf = Buffer::new(20, 6);
    long_block(&mut buf, "r", 45);
    let old_prompt = buf.prompt_rows()[0];

    buf.set_size(10, 6, 0);
    buf.set_size(30, 6, 0);
    let remap = buf.take_reflow_remap().unwrap();

    let mapped = remap.map_start(old_prompt).unwrap();
    assert_eq!(mapped, buf.prompt_rows()[0]);
    assert_eq!(text_of_number(&buf, mapped).as_deref(), Some("r$ cmd"));
}

#[test]
fn resize_on_the_alternate_screen_remaps_primary_marks() {
    let mut buf = Buffer::new(20, 6);
    long_block(&mut buf, "r", 55);
    let old_prompt = buf.prompt_rows()[0];

    buf.enter_alternate(0);
    // The alternate screen does not reflow, but the parked primary screen
    // does, and the primary marks held by the buffer must follow it.
    buf.set_size(10, 6, 0);

    let remap = buf
        .take_reflow_remap()
        .expect("reflowing the parked primary screen must be reported");
    assert!(remap.map_start(old_prompt).is_some());
    assert_ne!(
        buf.prompt_rows()[0],
        old_prompt,
        "the primary prompt mark was renumbered"
    );

    let _ = buf.leave_alternate();
    assert_eq!(
        text_of_number(&buf, buf.prompt_rows()[0]).as_deref(),
        Some("r$ cmd")
    );
    let b = &buf.command_blocks()[0];
    assert_eq!(
        text_of_number(&buf, b.prompt_start_row).as_deref(),
        Some("r$ cmd")
    );
    let end_text = text_of_number(&buf, b.end_row.unwrap()).unwrap();
    assert!(
        end_text.ends_with('4'),
        "end_row follows the output: {end_text:?}"
    );
}

#[test]
fn resize_on_the_alternate_screen_leaves_alt_marks_alone() {
    let mut buf = Buffer::new(20, 6);
    long_block(&mut buf, "r", 55);
    buf.enter_alternate(0);
    buf.mark_prompt_row();
    let alt_mark = *buf.prompt_rows().last().unwrap();
    assert!(alt_mark.is_alternate());

    buf.set_size(10, 6, 0);

    assert_eq!(
        *buf.prompt_rows().last().unwrap(),
        alt_mark,
        "a primary reflow must not touch an alternate-screen mark"
    );
}

// ── RIS / ED 3 ───────────────────────────────────────────────────────────

#[test]
fn full_reset_advances_the_base_instead_of_resetting_it() {
    let mut buf = small_buffer();
    for i in 0..20 {
        line(&mut buf, &format!("out {i}"));
    }
    let before = buf.next_row_number();
    let stale = buf.cursor_row_number();

    buf.full_reset();

    assert!(
        buf.row_base() >= before,
        "RIS must continue the numbering ({} < {before})",
        buf.row_base()
    );
    assert_eq!(buf.row_index_of(stale), None, "stale numbers do not alias");
    assert_eq!(buf.rows().len(), 1);
    assert_eq!(buf.prompt_rows(), &[]);
}

#[test]
fn full_reset_from_the_alternate_screen_keeps_both_namespaces_monotonic() {
    let mut buf = Buffer::new(20, 3);
    for i in 0..5 {
        line(&mut buf, &format!("p {i}"));
    }
    let primary_next = buf.next_row_number();
    buf.enter_alternate(0);
    let alt_next = buf.next_row_number();

    buf.full_reset();

    assert!(
        !buf.row_base().is_alternate(),
        "RIS leaves the alternate screen"
    );
    assert!(
        buf.row_base() >= primary_next,
        "primary numbering continues"
    );
    buf.enter_alternate(0);
    assert!(buf.row_base() >= alt_next, "alternate numbering continues");
}

#[test]
fn erase_scrollback_advances_the_base() {
    let mut buf = Buffer::new(20, 3).with_scrollback_limit(50);
    for i in 0..10 {
        line(&mut buf, &format!("out {i}"));
    }
    let old_base = buf.row_base();
    let rows_before = buf.rows().len();
    let visible_start = rows_before - 3;
    assert!(visible_start > 0, "setup needs scrollback");
    let cursor_number = buf.cursor_row_number();

    buf.erase_scrollback();

    assert_eq!(
        buf.row_base(),
        old_base.saturating_add(visible_start),
        "ED 3 evicts the scrollback, advancing the base by exactly that many rows"
    );
    assert_eq!(
        buf.cursor_row_number(),
        cursor_number,
        "the cursor is on the same row, whose number is unchanged"
    );
    assert_eq!(buf.row_index_of(old_base), None);
}

// ── Restoring the cursor to an image origin (Task 125.C7) ────────────────

/// The cell under the cursor, which must be the image's top-left cell.
fn assert_cursor_on_image_origin(buf: &Buffer, x: usize) {
    let pos = buf.cursor().pos;
    assert_eq!(pos.x, x, "cursor column");
    let cell = &buf.rows()[pos.y].cells()[pos.x];
    let placement = cell
        .image_placement()
        .expect("cursor must sit on an image cell");
    assert_eq!(
        (placement.col_in_image, placement.row_in_image),
        (0, 0),
        "cursor must sit on the image's top-left cell"
    );
}

#[test]
fn restore_cursor_to_image_origin_survives_eviction_during_placement() {
    let mut buf = small_buffer(); // 3 visible + 5 scrollback = 8 rows max
    for i in 0..6 {
        line(&mut buf, &format!("pad {i}"));
    }
    buf.set_cursor_pos(Some(4), None);
    let expected = buf.cursor_row_number();

    // Stamps rows 6..9 and appends one below: two rows are evicted DURING
    // the placement, so any index read before it is stale afterwards.
    let result = place(&mut buf, 1, 3);
    assert_eq!(
        buf.row_base(),
        RowNumber::new(2),
        "setup must evict two rows"
    );
    assert_ne!(buf.cursor_row_number(), expected, "place_image moved it");

    buf.restore_cursor_to_image_origin(&result);

    assert_eq!(buf.cursor_row_number(), expected);
    assert_cursor_on_image_origin(&buf, 4);
}

#[test]
fn restore_cursor_to_image_origin_without_eviction_matches_the_pre_placement_cursor() {
    // The screen scrolls (rows are appended below the window) but nothing is
    // evicted: the cursor must come back to where it was in the content, not
    // to the same screen row, so `C=1` still leaves it on the image origin.
    let mut buf = Buffer::new(20, 3);
    for i in 0..4 {
        line(&mut buf, &format!("pad {i}"));
    }
    let before = buf.cursor().pos;
    let before_number = buf.cursor_row_number();

    let result = place(&mut buf, 2, 3);
    buf.restore_cursor_to_image_origin(&result);

    assert_eq!(buf.cursor().pos, before);
    assert_eq!(buf.cursor_row_number(), before_number);
    assert_cursor_on_image_origin(&buf, 0);
}

#[test]
fn restore_cursor_to_image_origin_clamps_an_evicted_origin_to_the_oldest_row() {
    let mut buf = small_buffer();
    for i in 0..6 {
        line(&mut buf, &format!("pad {i}"));
    }

    // 10 rows from row 6 evict well past the origin itself.
    let result = place(&mut buf, 1, 10);
    assert!(
        buf.row_index_of(result.origin_row).is_none(),
        "setup: the origin row must itself be evicted"
    );

    buf.restore_cursor_to_image_origin(&result);

    assert_eq!(buf.cursor().pos.y, 0, "clamped to the oldest retained row");
    assert_eq!(buf.cursor().pos.x, 0);
}

#[test]
fn restore_cursor_to_image_origin_ignores_decom() {
    use freminal_common::buffer_states::modes::decom::Decom;

    let mut buf = Buffer::new(20, 5);
    for i in 0..3 {
        line(&mut buf, &format!("pad {i}"));
    }
    buf.set_cursor_pos(Some(2), None);
    let before = buf.cursor().pos;
    let result = place(&mut buf, 1, 1);

    buf.set_decom(Decom::OriginMode);
    buf.restore_cursor_to_image_origin(&result);

    assert_eq!(buf.cursor().pos, before, "restore is buffer-absolute");
}

// ── Popping trailing padding re-issues numbers (review NIT) ──────────────

/// A 10x5 buffer with the cursor addressed down to screen row `down`, which
/// creates pristine `ScrollFill` padding rows `1..=down` (a line feed would
/// make them `HardBreak`, which the reclaim pass does not touch).
fn buffer_with_padding_down_to(down: usize) -> Buffer {
    let mut buf = Buffer::new(10, 5);
    buf.set_cursor_pos(Some(0), Some(down));
    buf
}

#[test]
fn growing_the_height_forgets_marks_on_popped_padding_rows() {
    let mut buf = buffer_with_padding_down_to(3);
    // Mark the bottom padding row, then return to the top.
    buf.mark_prompt_row();
    let _ = buf.start_command_block(None, "gone".to_owned());
    let marked = buf.cursor_row_number();
    buf.set_cursor_pos(Some(0), Some(0));
    assert_eq!(buf.rows().len(), 4, "setup: three padding rows below row 0");
    assert_eq!(buf.prompt_rows(), [marked]);

    // Growing the window reclaims the padding below the cursor.
    let _ = buf.set_size(10, 8, 0);
    assert_eq!(buf.rows().len(), 1, "setup: the padding was popped");
    assert!(
        buf.prompt_rows().is_empty(),
        "a prompt mark on a popped row must not survive"
    );
    assert!(
        buf.command_blocks().is_empty(),
        "a block that started on a popped row must not survive"
    );

    // The popped numbers are issued again to new rows; no mark may name them.
    buf.set_cursor_pos(Some(0), Some(3));
    assert_eq!(buf.row_index_of(marked), Some(3), "the number is re-issued");
    assert_eq!(buf.prompt_rows(), []);
    assert!(buf.command_blocks().is_empty());
}

#[test]
fn growing_the_height_keeps_marks_on_surviving_rows() {
    let mut buf = Buffer::new(10, 5);
    buf.mark_prompt_row();
    let _ = buf.start_command_block(None, "kept".to_owned());
    buf.mark_command_start_row("kept");
    let kept = buf.cursor_row_number();
    buf.set_cursor_pos(Some(0), Some(3));
    buf.set_cursor_pos(Some(0), Some(0));
    assert_eq!(buf.rows().len(), 4, "setup: padding below the marked row");

    let _ = buf.set_size(10, 8, 0);

    assert_eq!(buf.rows().len(), 1, "setup: the padding was popped");
    assert_eq!(buf.prompt_rows(), [kept]);
    assert_eq!(buf.command_blocks().len(), 1);
    assert_eq!(buf.command_blocks()[0].prompt_start_row, kept);
    assert_eq!(buf.command_blocks()[0].command_start_row, Some(kept));
}

#[test]
fn growing_the_height_clamps_a_surviving_blocks_later_boundaries() {
    let mut buf = Buffer::new(10, 5);
    buf.mark_prompt_row();
    let _ = buf.start_command_block(None, "span".to_owned());
    let top = buf.cursor_row_number();
    // Output and the finish land on padding rows further down.
    buf.set_cursor_pos(Some(0), Some(3));
    buf.mark_output_start_row("span");
    let _ = buf.finish_command_block(Some(0), "span");
    buf.set_cursor_pos(Some(0), Some(0));
    assert_eq!(buf.rows().len(), 4, "setup: padding below the marked row");

    let _ = buf.set_size(10, 8, 0);

    assert_eq!(buf.rows().len(), 1, "setup: the padding was popped");
    let block = &buf.command_blocks()[0];
    assert_eq!(block.prompt_start_row, top);
    assert_eq!(
        block.output_start_row,
        Some(top),
        "clamped to the last row that still exists"
    );
    assert_eq!(block.end_row, Some(top));
    assert!(buf.row_index_of(block.end_row.unwrap()).is_some());
}
