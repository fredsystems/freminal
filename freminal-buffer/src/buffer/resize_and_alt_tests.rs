// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Regression tests for `resize_and_alt.rs` found by the Task 125 review:
//! the throwaway buffer an alternate-screen resize reflows the parked primary
//! screen in must honour the configured scrollback limit, and a height grow
//! must restore the compressed scrollback rows it re-exposes.

use freminal_common::buffer_states::{row_number::RowNumber, tchar::TChar};

use crate::{
    buffer::{Buffer, RowStore},
    cell::Cell,
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

// ── resize_saved_primary honours the configured scrollback limit ─────────

/// A primary buffer holding `rows` blank rows under a configured limit
/// larger than the compiled-in default of 10 000, with its cursor on the last
/// row. Assigned directly: filling 11 000 rows through the parser would spend
/// the test's time in debug-build invariant checks.
fn large_primary(rows: usize, limit: usize) -> Buffer {
    let mut buf = Buffer::new(20, 3).with_scrollback_limit(limit);
    buf.rows = (0..rows).map(|_| Row::new(20)).collect::<RowStore>();
    buf.cursor.pos.y = rows - 1;
    buf
}

#[test]
fn alt_screen_resize_keeps_primary_scrollback_up_to_the_configured_limit() {
    // 11 000 retained rows is past the old hard-coded 10 000-row placeholder
    // but well inside the configured 12 000.
    let mut buf = large_primary(11_000, 12_000);
    buf.enter_alternate(0);

    let _ = buf.set_size(20, 5, 0);

    let saved = buf.saved_primary.as_ref().expect("primary is parked");
    assert_eq!(
        saved.rows.len(),
        11_000,
        "the resize must not trim the parked primary to a default limit"
    );
    assert_eq!(
        saved.rows.base(),
        RowNumber::ZERO,
        "no primary row may be evicted by an alternate-screen resize"
    );
}

#[test]
fn alt_screen_resize_still_trims_the_parked_primary_to_the_configured_limit() {
    // The limit is honoured, not ignored: past it, the oldest rows go.
    let mut buf = large_primary(300, 100);
    buf.enter_alternate(0);

    let _ = buf.set_size(20, 5, 0);

    let saved = buf.saved_primary.as_ref().expect("primary is parked");
    assert_eq!(
        saved.rows.len(),
        100 + 5,
        "trimmed to configured limit + height"
    );
    assert!(saved.rows.base() > RowNumber::ZERO);
}

// ── height grow restores the compressed rows it re-exposes ───────────────

/// A 20x3 buffer with `n` numbered scrollback lines, all compacted and then
/// compressed into one block.
fn buffer_with_compressed_scrollback(n: usize) -> Buffer {
    let mut buf = Buffer::new(20, 3).with_scrollback_limit(200);
    for i in 0..n {
        buf.insert_text(&text(&format!("line{i:04}content")));
        buf.handle_lf();
        buf.handle_cr();
    }
    let _ = buf.compact_idle_scrollback(usize::MAX);
    let visible_start = buf.visible_window_start(0);
    assert!(visible_start > 4, "setup: scrollback to compress");
    assert!(buf.compress_scrollback_block(0, visible_start));
    assert!(buf.rows[0].is_evicted(), "setup: row 0 is compressed");
    buf
}

#[test]
fn height_grow_restores_the_compressed_rows_it_re_exposes() {
    let mut buf = buffer_with_compressed_scrollback(30);
    let old_start = buf.visible_window_start(0);

    let _ = buf.set_size(20, 5, 0);

    let new_start = buf.visible_window_start(0);
    assert_eq!(new_start, old_start - 2, "setup: two rows re-exposed");
    for i in new_start..buf.rows().len() {
        assert!(
            !buf.rows[i].is_evicted(),
            "row {i} of the visible window is still a compressed placeholder"
        );
        assert!(buf.rows.block_map()[i].is_none());
    }
    // The restored rows hold their original text.
    assert_eq!(
        row_text(&buf, new_start),
        format!("line{new_start:04}content")
    );
    // (Decompression is per block: the whole block that held the re-exposed
    // rows is restored, so rows above the window may be live again too.)
}

#[test]
fn writing_into_a_re_exposed_row_keeps_the_rest_of_its_text() {
    let mut buf = buffer_with_compressed_scrollback(30);
    let _ = buf.set_size(20, 5, 0);
    let start = buf.visible_window_start(0);
    let original = row_text(&buf, start);
    assert!(original.starts_with("line"), "got {original:?}");

    // Address the window's first row and overwrite its first two cells.
    buf.set_cursor_pos(Some(0), Some(0));
    buf.insert_text(&text("ZZ"));

    assert_eq!(row_text(&buf, start), format!("ZZ{}", &original[2..]));
}

#[test]
fn rotating_a_re_exposed_row_moves_its_text_intact() {
    let mut buf = buffer_with_compressed_scrollback(30);
    let _ = buf.set_size(20, 5, 0);
    let start = buf.visible_window_start(0);
    let first = row_text(&buf, start);
    let second = row_text(&buf, start + 1);

    // IL at the window's top row rotates the rows of the scroll region down.
    buf.set_cursor_pos(Some(0), Some(0));
    buf.insert_lines(1);

    assert_eq!(row_text(&buf, start), "", "a blank row was inserted");
    assert_eq!(
        row_text(&buf, start + 1),
        first,
        "the old top row moved down"
    );
    assert_eq!(row_text(&buf, start + 2), second);
}
