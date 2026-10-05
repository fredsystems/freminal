// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Task 125.C14 tests: the cell / column / row placement clears delete the
//! placements they intersect, and only those.
//!
//! A placement is one display of an image (`image_id` + `placement_instance`).
//! Kitty's `d=p`, `d=c`, `d=x` and `d=y` delete "all placements that intersect"
//! a cell, column or row, so a second placement of the same image elsewhere
//! must survive.

use std::sync::Arc;

use crate::{
    buffer::Buffer,
    image_store::{AnimationControl, ImageProtocol, ImageSizeMode, InlineImage},
    row::Row,
};

fn alt_buf(width: usize, height: usize) -> Buffer {
    let mut buf = Buffer::new(width, height);
    buf.enter_alternate(0);
    buf
}

/// Place a `cols` x `rows` display of image `id` as placement `instance` with
/// its top-left at `(col, row)`.
fn place(
    buf: &mut Buffer,
    protocol: ImageProtocol,
    id: u64,
    instance: u64,
    (col, row): (usize, usize),
    (cols, rows): (usize, usize),
) {
    let image = InlineImage {
        id,
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
    buf.set_cursor_pos(Some(col), Some(row));
    let _ = buf.place_image(image, 0, protocol, None, None, 0, None, instance, None);
}

fn actual_image_cells(buf: &Buffer) -> usize {
    buf.rows.iter().map(Row::count_image_cells).sum()
}

fn assert_counted(buf: &Buffer, expected: usize) {
    assert_eq!(actual_image_cells(buf), expected, "real image cell count");
    assert_eq!(buf.image_cell_count, expected, "image_cell_count counter");
    buf.debug_assert_invariants();
}

fn has_image(buf: &Buffer, row: usize, col: usize) -> bool {
    buf.rows[row]
        .cells_for_image_scan()
        .get(col)
        .is_some_and(crate::cell::Cell::has_image)
}

/// Image 7 displayed twice: placement 1 (2x2 at the top-left) and placement 2
/// (2x1 at columns 6-7 of row 4).
fn two_placements_of_one_image() -> Buffer {
    let mut buf = alt_buf(10, 6);
    place(&mut buf, ImageProtocol::Kitty, 7, 1, (0, 0), (2, 2));
    place(&mut buf, ImageProtocol::Kitty, 7, 2, (6, 4), (2, 1));
    assert_counted(&buf, 6);
    buf
}

#[test]
fn at_cell_clears_only_the_placement_under_the_cell() {
    let mut buf = two_placements_of_one_image();

    buf.clear_image_placements_at_cell(1, 1);

    assert_counted(&buf, 2);
    assert!(
        !has_image(&buf, 0, 0) && !has_image(&buf, 1, 1),
        "whole placement 1"
    );
    assert!(
        has_image(&buf, 4, 6) && has_image(&buf, 4, 7),
        "placement 2 kept"
    );
    assert!(buf.image_store().contains(7), "kitty data is kept");
}

#[test]
fn at_cell_on_an_empty_or_out_of_range_cell_clears_nothing() {
    let mut buf = two_placements_of_one_image();

    buf.clear_image_placements_at_cell(3, 3);
    buf.clear_image_placements_at_cell(0, 9);
    buf.clear_image_placements_at_cell(99, 0);

    assert_counted(&buf, 6);
}

#[test]
fn in_column_clears_every_placement_with_a_cell_in_the_column() {
    let mut buf = alt_buf(10, 6);
    place(&mut buf, ImageProtocol::Kitty, 7, 1, (0, 0), (2, 2)); // cols 0-1
    place(&mut buf, ImageProtocol::Kitty, 8, 2, (1, 4), (3, 1)); // cols 1-3
    place(&mut buf, ImageProtocol::Kitty, 9, 3, (6, 2), (2, 1)); // cols 6-7
    assert_counted(&buf, 9);

    buf.clear_image_placements_in_column(1);

    assert_counted(&buf, 2);
    assert!(
        has_image(&buf, 2, 6) && has_image(&buf, 2, 7),
        "col 6-7 kept"
    );
}

#[test]
fn in_column_spares_another_placement_of_the_same_image() {
    let mut buf = two_placements_of_one_image();

    buf.clear_image_placements_in_column(0);

    assert_counted(&buf, 2);
    assert!(has_image(&buf, 4, 6), "placement 2 does not touch column 0");
}

#[test]
fn in_row_clears_every_placement_with_a_cell_on_the_row() {
    let mut buf = alt_buf(10, 6);
    place(&mut buf, ImageProtocol::Kitty, 7, 1, (0, 0), (2, 2)); // rows 0-1
    place(&mut buf, ImageProtocol::Kitty, 8, 2, (5, 1), (2, 2)); // rows 1-2
    place(&mut buf, ImageProtocol::Kitty, 9, 3, (0, 4), (2, 1)); // row 4
    assert_counted(&buf, 10);

    // Row 1 holds cells of both of the first two placements; each goes whole,
    // including its cells on rows 0 and 2.
    buf.clear_image_placements_in_row(1);

    assert_counted(&buf, 2);
    assert!(has_image(&buf, 4, 0) && has_image(&buf, 4, 1));
}

#[test]
fn in_row_spares_another_placement_of_the_same_image() {
    let mut buf = two_placements_of_one_image();

    buf.clear_image_placements_in_row(0);

    assert_counted(&buf, 2);
    assert!(has_image(&buf, 4, 6) && has_image(&buf, 4, 7));
}

#[test]
fn in_row_out_of_range_clears_nothing() {
    let mut buf = two_placements_of_one_image();
    buf.clear_image_placements_in_row(99);
    assert_counted(&buf, 6);
}

#[test]
fn clearing_a_cell_owned_images_only_placement_frees_it() {
    let mut buf = alt_buf(10, 6);
    place(&mut buf, ImageProtocol::Sixel, 21, 1, (0, 0), (2, 1));
    place(&mut buf, ImageProtocol::Sixel, 22, 2, (5, 2), (2, 1));
    assert_counted(&buf, 4);

    buf.clear_image_placements_at_cell(0, 0);

    assert_counted(&buf, 2);
    assert!(!buf.image_store().contains(21), "no cell left: freed");
    assert!(buf.image_store().contains(22));
}
