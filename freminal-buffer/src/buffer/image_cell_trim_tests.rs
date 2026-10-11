// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Task 125.C13 regression tests: the sparse-row trailing-blank trim must
//! stop at image cells.
//!
//! An image cell is a default-tag space, so every row-level trim loop used to
//! pop it when it was the last cell of the row — silently, without the owning
//! `Buffer` decrementing `image_cell_count`. Each test places an image cell
//! where the operation leaves it as the row's final stored cell, runs the
//! operation, and asserts the cell survived (or was destroyed *and* counted)
//! and that `image_cell_count` equals the real number of image cells.

use std::sync::Arc;

use freminal_common::buffer_states::{format_tag::FormatTag, modes::declrmm::Declrmm};

use crate::{
    buffer::Buffer,
    image_store::{
        AnimationControl, ImagePlacement, ImageProtocol, ImageSizeMode, InlineImage, next_image_id,
    },
    row::Row,
};

fn alt_buf(width: usize, height: usize) -> Buffer {
    let mut buf = Buffer::new(width, height);
    buf.enter_fresh_alternate();
    buf
}

fn place_image_cell(buf: &mut Buffer, row: usize, col: usize, id: u64) {
    let placement = ImagePlacement {
        image_id: id,
        col_in_image: 0,
        row_in_image: 0,
        protocol: ImageProtocol::Kitty,
        image_number: None,
        placement_id: None,
        z_index: 0,
        source_crop: None,
        placement_instance: 1,
        subcell_offset: None,
    };
    buf.rows[row].set_image_cell(col, placement, FormatTag::default());
    buf.image_cell_count += 1;
}

fn actual_image_cells(buf: &Buffer) -> usize {
    buf.rows.iter().map(Row::count_image_cells).sum()
}

fn assert_counted(buf: &Buffer, expected: usize) {
    assert_eq!(actual_image_cells(buf), expected, "real image cell count");
    assert_eq!(buf.image_cell_count, expected, "image_cell_count counter");
    buf.debug_assert_invariants();
}

fn set_cursor(buf: &mut Buffer, x: usize, y: usize) {
    buf.cursor.pos.x = x;
    buf.cursor.pos.y = y;
}

#[test]
fn ich_shifting_an_image_to_the_row_tail_keeps_it() {
    let mut buf = alt_buf(10, 5);
    place_image_cell(&mut buf, 0, 3, 1);
    set_cursor(&mut buf, 0, 0);

    buf.insert_spaces(2);

    assert!(buf.rows[0].cells()[5].has_image(), "shifted to col 5");
    assert_counted(&buf, 1);
}

#[test]
fn ich_shifting_an_image_off_the_right_edge_is_counted() {
    let mut buf = alt_buf(10, 5);
    place_image_cell(&mut buf, 0, 8, 1);
    set_cursor(&mut buf, 0, 0);

    buf.insert_spaces(3);

    assert_counted(&buf, 0);
}

#[test]
fn ich_with_right_margin_shifting_an_image_to_the_tail_keeps_it() {
    let mut buf = alt_buf(10, 5);
    buf.set_declrmm(Declrmm::Enabled);
    buf.set_left_right_margins(1, 8); // right margin = col 7
    place_image_cell(&mut buf, 0, 3, 1);
    set_cursor(&mut buf, 0, 0);

    buf.insert_spaces(2);

    assert!(buf.rows[0].cells()[5].has_image(), "shifted to col 5");
    assert_counted(&buf, 1);
}

#[test]
fn dch_shifting_an_image_to_the_row_tail_keeps_it() {
    let mut buf = alt_buf(10, 5);
    place_image_cell(&mut buf, 0, 8, 1);
    set_cursor(&mut buf, 0, 0);

    buf.delete_chars(2);

    assert!(buf.rows[0].cells()[6].has_image(), "shifted to col 6");
    assert_counted(&buf, 1);
}

#[test]
fn dch_deleting_an_image_is_counted() {
    let mut buf = alt_buf(10, 5);
    place_image_cell(&mut buf, 0, 2, 1);
    set_cursor(&mut buf, 2, 0);

    buf.delete_chars(1);

    assert_counted(&buf, 0);
}

#[test]
fn dch_with_right_margin_shifting_an_image_to_the_tail_keeps_it() {
    let mut buf = alt_buf(10, 5);
    buf.set_declrmm(Declrmm::Enabled);
    buf.set_left_right_margins(1, 8); // right margin = col 7
    place_image_cell(&mut buf, 0, 6, 1);
    set_cursor(&mut buf, 0, 0);

    buf.delete_chars(2);

    assert!(buf.rows[0].cells()[4].has_image(), "shifted to col 4");
    assert_counted(&buf, 1);
}

#[test]
fn el_to_end_keeps_an_image_left_of_the_cursor() {
    let mut buf = alt_buf(10, 5);
    place_image_cell(&mut buf, 0, 0, 1);
    place_image_cell(&mut buf, 0, 1, 2);
    set_cursor(&mut buf, 1, 0);

    buf.erase_line_to_end();

    assert!(buf.rows[0].cells()[0].has_image(), "col 0 is left of EL 0");
    assert_counted(&buf, 1);
}

#[test]
fn ed_to_end_keeps_an_image_left_of_the_cursor() {
    let mut buf = alt_buf(10, 5);
    place_image_cell(&mut buf, 0, 0, 1);
    place_image_cell(&mut buf, 0, 1, 2);
    place_image_cell(&mut buf, 1, 0, 3);
    set_cursor(&mut buf, 1, 0);

    buf.erase_to_end_of_display();

    assert!(buf.rows[0].cells()[0].has_image(), "col 0 is left of ED 0");
    assert_counted(&buf, 1);
}

#[test]
fn ech_keeps_an_image_left_of_the_erased_range() {
    let mut buf = alt_buf(10, 5);
    place_image_cell(&mut buf, 0, 0, 1);
    place_image_cell(&mut buf, 0, 1, 2);
    set_cursor(&mut buf, 1, 0);

    buf.erase_chars(1);

    assert!(buf.rows[0].cells()[0].has_image());
    assert_counted(&buf, 1);
}

// ---- Row-level probes (no Buffer accounting involved) ------------------

fn image_row(width: usize, cols: &[usize]) -> Row {
    let mut row = Row::new(width);
    for &col in cols {
        row.set_image_cell(
            col,
            ImagePlacement {
                image_id: 9,
                col_in_image: col,
                row_in_image: 0,
                protocol: ImageProtocol::Kitty,
                image_number: None,
                placement_id: None,
                z_index: 0,
                source_crop: None,
                placement_instance: 1,
                subcell_offset: None,
            },
            FormatTag::default(),
        );
    }
    row
}

#[test]
fn row_insert_spaces_at_keeps_the_tail_image() {
    let mut row = image_row(10, &[3]);
    row.insert_spaces_at(0, 2, &FormatTag::default());
    assert_eq!(row.count_image_cells(), 1);
    assert!(row.cells()[5].has_image());
}

#[test]
fn row_insert_spaces_at_with_right_limit_keeps_the_tail_image() {
    let mut row = image_row(10, &[3]);
    row.insert_spaces_at_with_right_limit(0, 2, &FormatTag::default(), 8);
    assert_eq!(row.count_image_cells(), 1);
    assert!(row.cells()[5].has_image());
}

#[test]
fn row_delete_cells_at_keeps_the_tail_image() {
    let mut row = image_row(10, &[8]);
    row.delete_cells_at(0, 2, &FormatTag::default());
    assert_eq!(row.count_image_cells(), 1);
    assert!(row.cells()[6].has_image());
}

#[test]
fn row_delete_cells_at_with_right_limit_keeps_the_tail_image() {
    let mut row = image_row(10, &[6]);
    row.delete_cells_at_with_right_limit(0, 2, 8, &FormatTag::default());
    assert_eq!(row.count_image_cells(), 1);
    assert!(row.cells()[4].has_image());
}

#[test]
fn row_clear_from_keeps_images_left_of_col() {
    let mut row = image_row(10, &[0, 1]);
    row.clear_from(1, &FormatTag::default());
    assert_eq!(row.count_image_cells(), 1);
    assert!(row.cells()[0].has_image());
}

fn make_image(cols: usize, rows: usize) -> InlineImage {
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

/// Place an image of `cols` x `rows` at the cursor, returning its id.
fn place(buf: &mut Buffer, protocol: ImageProtocol, cols: usize, rows: usize) -> u64 {
    let image = make_image(cols, rows);
    let id = image.id;
    let _ = buf.place_image(image, 0, protocol, None, None, 0, None, 1, None);
    id
}

/// Number of cells across the buffer that belong to image `id`.
fn cells_of(buf: &Buffer, id: u64) -> usize {
    buf.rows
        .iter()
        .flat_map(Row::cells_for_image_scan)
        .filter(|c| c.image_placement().is_some_and(|p| p.image_id == id))
        .count()
}

// ── ECH and trailing image cells (Task 125.C11) ─────────────────────────

/// Place a `cols` x 1 image at the start of row 0 and park the cursor at
/// `(x, 0)`.
fn image_at_row_start(protocol: ImageProtocol, cols: usize, x: usize) -> (Buffer, u64) {
    let mut buf = Buffer::new(10, 3);
    buf.cursor.pos.y = 0;
    buf.cursor.pos.x = 0;
    let id = place(&mut buf, protocol, cols, 1);
    assert_eq!(cells_of(&buf, id), cols, "setup: image cells placed");
    assert_eq!(buf.image_cell_count, cols);
    buf.cursor.pos.y = 0;
    buf.cursor.pos.x = x;
    (buf, id)
}

#[test]
fn ech_beyond_an_image_keeps_its_cells_counted_and_the_image_alive() {
    // The erased range [4, 7) extends the row's storage with blanks, and the
    // image cells at [0, 2) become the row's trailing "default blanks". They
    // are content, not blanks: they must neither be trimmed nor miscounted.
    for protocol in [ImageProtocol::Sixel, ImageProtocol::Kitty] {
        let (mut buf, id) = image_at_row_start(protocol, 2, 4);

        buf.erase_chars(3);

        assert_eq!(cells_of(&buf, id), 2, "{protocol:?}: cells survive ECH");
        assert_eq!(buf.image_cell_count, 2, "{protocol:?}: count is exact");
        assert!(
            buf.image_store().contains(id),
            "{protocol:?}: image still referenced"
        );
        // Fails (in debug builds) if the count and the cells disagree.
        buf.debug_assert_invariants();
    }
}

#[test]
fn ech_over_part_of_a_kitty_image_keeps_the_rest_counted() {
    let (mut buf, id) = image_at_row_start(ImageProtocol::Kitty, 3, 2);

    // Blank only column 2; columns 0-1 are the row's trailing cells.
    buf.erase_chars(1);

    assert_eq!(cells_of(&buf, id), 2, "columns 0-1 survive");
    assert_eq!(buf.image_cell_count, 2, "count is exact");
    assert!(buf.image_store().contains(id));
    buf.debug_assert_invariants();
}

#[test]
fn ech_over_part_of_a_sixel_image_sweeps_and_frees_the_whole_image() {
    // Non-Kitty images are cleared as a unit when any of their cells is
    // erased (`collect_and_clear_image_ids_in_rows`); the row trim must
    // not disturb that accounting.
    let (mut buf, id) = image_at_row_start(ImageProtocol::Sixel, 3, 2);

    buf.erase_chars(1);

    assert_eq!(cells_of(&buf, id), 0);
    assert_eq!(buf.image_cell_count, 0);
    assert!(!buf.image_store().contains(id));
    buf.debug_assert_invariants();
}

#[test]
fn ech_over_a_whole_image_clears_its_cells_and_frees_it() {
    for protocol in [ImageProtocol::Sixel, ImageProtocol::Kitty] {
        let (mut buf, id) = image_at_row_start(protocol, 2, 0);

        buf.erase_chars(2);

        assert_eq!(cells_of(&buf, id), 0, "{protocol:?}: cells erased");
        assert_eq!(buf.image_cell_count, 0, "{protocol:?}: count is exact");
        if protocol == ImageProtocol::Sixel {
            assert!(
                !buf.image_store().contains(id),
                "an image with no cells left must be freed"
            );
        }
        buf.debug_assert_invariants();
    }
}
