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

use freminal_common::buffer_states::{format_tag::FormatTag, modes::declrmm::Declrmm};

use crate::{
    buffer::Buffer,
    image_store::{ImagePlacement, ImageProtocol},
    row::Row,
};

fn alt_buf(width: usize, height: usize) -> Buffer {
    let mut buf = Buffer::new(width, height);
    buf.enter_alternate(0);
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
