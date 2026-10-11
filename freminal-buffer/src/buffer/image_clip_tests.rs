// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Task 125.C15 regression tests: clipping rows to a narrower width on the
//! alternate screen must account for the image cells it drops.
//!
//! `Row::truncate_cells_to_width` removes image cells without any bookkeeping,
//! so `Buffer::set_size` has to decrement `image_cell_count` for them and free
//! a cell-owned (Sixel/iTerm2) image that no longer has a cell. A Kitty image's
//! data is protocol-retained and stays.

use std::sync::Arc;

use crate::{
    buffer::Buffer,
    image_store::{AnimationControl, ImageProtocol, ImageSizeMode, InlineImage, next_image_id},
    row::Row,
};

fn alt_buf(width: usize, height: usize) -> Buffer {
    let mut buf = Buffer::new(width, height);
    buf.enter_fresh_alternate();
    buf
}

/// Place a `cols` x `rows` image with its top-left at screen `(col, row)`.
fn place_at(
    buf: &mut Buffer,
    protocol: ImageProtocol,
    col: usize,
    row: usize,
    cols: usize,
    rows: usize,
) -> u64 {
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
    let _ = buf.place_image(image, 0, protocol, None, None, 0, None, 1, None);
    id
}

fn actual_image_cells(buf: &Buffer) -> usize {
    buf.rows.iter().map(Row::count_image_cells).sum()
}

fn assert_counted(buf: &Buffer, expected: usize) {
    assert_eq!(actual_image_cells(buf), expected, "real image cell count");
    assert_eq!(buf.image_cell_count, expected, "image_cell_count counter");
    buf.debug_assert_invariants();
}

#[test]
fn width_shrink_counts_the_clipped_cells_of_a_partly_clipped_image() {
    let mut buf = alt_buf(10, 5);
    let id = place_at(&mut buf, ImageProtocol::Sixel, 0, 0, 6, 2);
    assert_counted(&buf, 12);

    // Columns 4 and 5 of both image rows are clipped: 4 cells.
    let _ = buf.set_size(4, 5, 0);

    assert_counted(&buf, 8);
    assert!(
        buf.image_store().contains(id),
        "an image that still has cells is kept"
    );
}

#[test]
fn width_shrink_frees_a_cell_owned_image_clipped_away_entirely() {
    let mut buf = alt_buf(10, 5);
    let id = place_at(&mut buf, ImageProtocol::Sixel, 6, 0, 3, 2);
    assert_counted(&buf, 6);
    assert!(buf.image_store().contains(id));

    let _ = buf.set_size(5, 5, 0);

    assert_counted(&buf, 0);
    assert!(
        !buf.image_store().contains(id),
        "a Sixel image with no cell left must be freed"
    );
}

#[test]
fn width_shrink_frees_only_the_images_that_lost_every_cell() {
    let mut buf = alt_buf(10, 5);
    let gone = place_at(&mut buf, ImageProtocol::ITerm2, 7, 0, 2, 1);
    let kept = place_at(&mut buf, ImageProtocol::ITerm2, 0, 2, 2, 1);
    assert_counted(&buf, 4);

    let _ = buf.set_size(6, 5, 0);

    assert_counted(&buf, 2);
    assert!(!buf.image_store().contains(gone));
    assert!(buf.image_store().contains(kept));
}

#[test]
fn width_shrink_keeps_the_data_of_a_kitty_image_clipped_away_entirely() {
    let mut buf = alt_buf(10, 5);
    let id = place_at(&mut buf, ImageProtocol::Kitty, 6, 0, 3, 1);
    assert_counted(&buf, 3);

    let _ = buf.set_size(5, 5, 0);

    assert_counted(&buf, 0);
    assert!(
        buf.image_store().contains(id),
        "kitty image data outlives its cells"
    );
}

#[test]
fn width_grow_and_unchanged_width_leave_image_cells_alone() {
    let mut buf = alt_buf(10, 5);
    let id = place_at(&mut buf, ImageProtocol::Sixel, 7, 0, 3, 1);
    assert_counted(&buf, 3);

    let _ = buf.set_size(20, 5, 0);
    assert_counted(&buf, 3);

    let _ = buf.set_size(20, 8, 0);
    assert_counted(&buf, 3);
    assert!(buf.image_store().contains(id));
}

#[test]
fn width_shrink_with_no_images_is_unaffected() {
    let mut buf = alt_buf(10, 5);
    let _ = buf.set_size(4, 5, 0);
    assert_counted(&buf, 0);
}
