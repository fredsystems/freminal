// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Task 131.C5 regression tests: placing an image at the alternate screen's
//! bottom must scroll the content up rather than grow the store.
//!
//! `place_image` pushes rows to make room for the image and for the cursor row
//! below it. The alternate screen has no scrollback, so it must end every
//! placement with exactly `height` rows, the excess having scrolled off the
//! top as a line feed at the bottom would.

use std::sync::Arc;

use freminal_common::buffer_states::tchar::TChar;

use crate::{
    buffer::Buffer,
    cell::Cell,
    image_store::{AnimationControl, ImageProtocol, ImageSizeMode, InlineImage, next_image_id},
    row::Row,
};

const HEIGHT: usize = 5;

fn alt_buf() -> Buffer {
    let mut buf = Buffer::new(10, HEIGHT);
    buf.enter_fresh_alternate();
    buf
}

fn place(buf: &mut Buffer, cols: usize, rows: usize) -> u64 {
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
    let _ = buf.place_image(image, 0, ImageProtocol::Kitty, None, None, 0, None, 1, None);
    id
}

/// Write `A`..: one letter per row on rows `0..count`, then put the cursor on
/// the last row.
fn fill_letters(buf: &mut Buffer, count: usize) {
    for row in 0..count {
        buf.set_cursor_pos(Some(0), Some(row));
        let letter = b'A' + u8::try_from(row).unwrap();
        buf.insert_text(&[TChar::Ascii(letter)]);
    }
    buf.set_cursor_pos(Some(0), Some(HEIGHT - 1));
}

/// Screen rows (indices into the store) whose first cell is an image cell.
fn image_rows(buf: &Buffer) -> Vec<usize> {
    buf.rows
        .iter()
        .enumerate()
        .filter(|(_, row)| row.cells().first().is_some_and(Cell::has_image))
        .map(|(i, _)| i)
        .collect()
}

fn first_letter(buf: &Buffer, row: usize) -> TChar {
    *buf.rows[row].cells()[0].tchar()
}

fn assert_consistent(buf: &Buffer) {
    assert_eq!(
        buf.rows.len(),
        HEIGHT,
        "alternate store must be height rows"
    );
    let real: usize = buf.rows.iter().map(Row::count_image_cells).sum();
    assert_eq!(buf.image_cell_count, real, "image_cell_count counter");
    buf.debug_assert_invariants();
}

#[test]
fn one_row_image_on_last_row_scrolls_up_by_one() {
    let mut buf = alt_buf();
    fill_letters(&mut buf, HEIGHT);

    let _ = place(&mut buf, 1, 1);

    assert_consistent(&buf);
    // One row was pushed below the image, so content scrolled up by one: the
    // image, which sat on row 4, is now on row 3.
    assert_eq!(image_rows(&buf), vec![3]);
    assert_eq!(buf.cursor.pos.y, HEIGHT - 1);
    assert_eq!(buf.cursor.pos.x, 0);
    // Rows 0..=3 held A..=D before; the top one (A) scrolled off, B-D moved up
    // and the image (replacing E) is on row 3.
    assert_eq!(first_letter(&buf, 0), TChar::Ascii(b'B'));
    assert_eq!(first_letter(&buf, 1), TChar::Ascii(b'C'));
    assert_eq!(first_letter(&buf, 2), TChar::Ascii(b'D'));
}

#[test]
fn three_row_image_on_last_row_scrolls_up_by_three() {
    let mut buf = alt_buf();
    fill_letters(&mut buf, HEIGHT);

    let _ = place(&mut buf, 2, 3);

    assert_consistent(&buf);
    // Image rows 4..=6 plus the row below (7) need 3 rows beyond the screen
    // bottom: content scrolls up by 3, putting the image on rows 1..=3.
    assert_eq!(image_rows(&buf), vec![1, 2, 3]);
    assert_eq!(buf.cursor.pos.y, HEIGHT - 1);
    assert_eq!(buf.cursor.pos.x, 0);
    // D was on row 3 and moved up by three to row 0.
    assert_eq!(first_letter(&buf, 0), TChar::Ascii(b'D'));
}

#[test]
fn image_taller_than_screen_clamps_without_panicking() {
    let mut buf = alt_buf();
    fill_letters(&mut buf, HEIGHT);

    let _ = place(&mut buf, 2, 7);

    assert_consistent(&buf);
    assert_eq!(buf.cursor.pos.y, HEIGHT - 1);
    assert_eq!(buf.cursor.pos.x, 0);
    // The image's origin scrolled off the top; every remaining row is image.
    assert_eq!(image_rows(&buf), vec![0, 1, 2, 3]);
}

#[test]
fn image_mid_screen_does_not_scroll() {
    let mut buf = alt_buf();
    fill_letters(&mut buf, 1);
    buf.set_cursor_pos(Some(0), Some(1));

    let _ = place(&mut buf, 2, 2);

    assert_consistent(&buf);
    assert_eq!(image_rows(&buf), vec![1, 2]);
    assert_eq!(buf.cursor.pos.y, 3);
    assert_eq!(first_letter(&buf, 0), TChar::Ascii(b'A'));
}
