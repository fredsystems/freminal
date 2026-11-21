// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Tests: [`ImagePlacementMode::FixedOrigin`] anchors an image at
//! screen row 0, column 0, clips it to the visible screen, and neither scrolls
//! the buffer nor moves the cursor (sixel with scrolling off, DECSDM `?80 h`).

use std::sync::Arc;

use freminal_common::buffer_states::tchar::TChar;

use crate::{
    buffer::{Buffer, ImagePlacementMode, PlaceImageRequest},
    image_store::{AnimationControl, ImageProtocol, ImageSizeMode, InlineImage, next_image_id},
};

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

fn place_fixed(
    buf: &mut Buffer,
    cols: usize,
    rows: usize,
) -> (u64, crate::buffer::PlaceImageResult) {
    let img = image(cols, rows);
    let id = img.id;
    let result = buf.place_image(
        img,
        0,
        PlaceImageRequest {
            protocol: ImageProtocol::Sixel,
            image_number: None,
            placement_id: None,
            z_index: 0,
            source_crop: None,
            placement_instance: 1,
            subcell_offset: None,
            mode: ImagePlacementMode::FixedOrigin,
        },
    );
    (id, result)
}

/// Fill `buf` with enough lines that the screen is full and scrollback exists.
fn fill_with_scrollback(buf: &mut Buffer, lines: usize) {
    for _ in 0..lines {
        buf.insert_text(&[TChar::Ascii(b'x')]);
        buf.handle_lf();
        buf.handle_cr();
    }
}

fn first_visible_row(buf: &Buffer) -> usize {
    buf.rows().len() - buf.height
}

fn image_cell_total(buf: &Buffer) -> usize {
    buf.rows()
        .iter()
        .map(crate::row::Row::count_image_cells)
        .sum()
}

#[test]
fn fixed_origin_anchors_at_screen_top_left_and_leaves_cursor_alone() {
    let mut buf = Buffer::new(20, 6);
    fill_with_scrollback(&mut buf, 15);
    buf.set_cursor_pos(Some(7), Some(3));
    let cursor_before = buf.cursor().pos;
    let rows_before = buf.rows().len();
    let base_before = buf.row_base();
    let top = first_visible_row(&buf);

    let (_, result) = place_fixed(&mut buf, 4, 2);

    assert_eq!(buf.cursor().pos, cursor_before, "cursor must not move");
    assert_eq!(buf.rows().len(), rows_before, "no row may be pushed");
    assert_eq!(buf.row_base(), base_before, "nothing may be evicted");
    assert_eq!(result.scroll_offset, 0);
    assert_eq!(result.origin_col, 0);
    assert_eq!(buf.row_index_of(result.origin_row), Some(top));

    for row in 0..2 {
        for col in 0..4 {
            let p = buf.rows()[top + row].cells()[col]
                .image_placement()
                .expect("image cell inside the image");
            assert_eq!((p.col_in_image, p.row_in_image), (col, row));
        }
    }
    assert_eq!(image_cell_total(&buf), 8, "no cell outside the image");
    buf.debug_assert_invariants();
}

#[test]
fn fixed_origin_image_taller_than_screen_is_clipped_without_scrollback_growth() {
    let mut buf = Buffer::new(20, 4);
    fill_with_scrollback(&mut buf, 12);
    let rows_before = buf.rows().len();
    let base_before = buf.row_base();
    let top = first_visible_row(&buf);

    let (_, _) = place_fixed(&mut buf, 3, 10);

    assert_eq!(buf.rows().len(), rows_before, "scrollback must not grow");
    assert_eq!(buf.row_base(), base_before);
    assert_eq!(image_cell_total(&buf), 3 * 4, "only the 4 visible rows");
    let last = buf.rows()[top + 3].cells()[0]
        .image_placement()
        .expect("last visible row carries the image");
    assert_eq!(last.row_in_image, 3);
    for row in 0..top {
        assert_eq!(
            buf.rows()[row].count_image_cells(),
            0,
            "scrollback row {row} must stay image-free"
        );
    }
    buf.debug_assert_invariants();
}

#[test]
fn fixed_origin_image_wider_than_screen_is_clipped_to_the_width() {
    let mut buf = Buffer::new(5, 4);
    fill_with_scrollback(&mut buf, 6);
    let top = first_visible_row(&buf);

    let (_, _) = place_fixed(&mut buf, 9, 1);

    assert_eq!(buf.rows()[top].count_image_cells(), 5);
    assert_eq!(image_cell_total(&buf), 5);
    buf.debug_assert_invariants();
}

#[test]
fn fixed_origin_on_a_young_buffer_materialises_only_the_screen_rows_it_needs() {
    let mut buf = Buffer::new(10, 6);
    assert_eq!(buf.rows().len(), 1, "setup: a fresh buffer holds one row");

    let (_, _) = place_fixed(&mut buf, 2, 3);

    assert_eq!(buf.rows().len(), 3, "grown to the image height, no more");
    assert_eq!(image_cell_total(&buf), 6);
    assert_eq!(buf.cursor().pos.y, 0);
    buf.debug_assert_invariants();
}

#[test]
fn fixed_origin_on_the_alternate_screen_stays_height_rows() {
    let mut buf = Buffer::new(10, 4);
    buf.enter_fresh_alternate();
    buf.set_cursor_pos(Some(5), Some(2));
    let cursor_before = buf.cursor().pos;

    let (_, _) = place_fixed(&mut buf, 3, 9);

    assert_eq!(buf.rows().len(), 4);
    assert_eq!(buf.cursor().pos, cursor_before);
    assert_eq!(image_cell_total(&buf), 3 * 4);
    buf.debug_assert_invariants();
}

#[test]
fn fixed_origin_replacing_an_image_of_the_same_size_releases_the_old_one() {
    let mut buf = Buffer::new(10, 6);
    fill_with_scrollback(&mut buf, 10);

    let (first, _) = place_fixed(&mut buf, 5, 4);
    let (second, _) = place_fixed(&mut buf, 5, 4);

    assert_eq!(image_cell_total(&buf), 5 * 4, "only the new image remains");
    assert!(
        !buf.image_store().contains(first),
        "the replaced cell-owned image is released"
    );
    assert!(buf.image_store().contains(second));
    buf.debug_assert_invariants();
}

#[test]
fn fixed_origin_clears_only_the_cells_under_the_rows_it_covers() {
    let mut buf = Buffer::new(10, 6);
    fill_with_scrollback(&mut buf, 10);

    let (first, _) = place_fixed(&mut buf, 5, 4);
    let (second, _) = place_fixed(&mut buf, 5, 1);

    // The new image overwrites row 0 of the old one; rows 1..4 of the old
    // image are not under it and stay on screen with their image alive.
    assert_eq!(image_cell_total(&buf), 5 + 5 * 3);
    assert!(buf.image_store().contains(first));
    assert!(buf.image_store().contains(second));
    buf.debug_assert_invariants();
}

#[test]
fn fixed_origin_leaves_an_inline_image_lower_on_the_screen_alone() {
    let mut buf = Buffer::new(20, 8);
    fill_with_scrollback(&mut buf, 20);
    buf.set_cursor_pos(Some(0), Some(5));
    let lower = image(3, 2);
    let lower_id = lower.id;
    let _ = buf.place_image(
        lower,
        0,
        PlaceImageRequest {
            protocol: ImageProtocol::Sixel,
            image_number: None,
            placement_id: None,
            z_index: 0,
            source_crop: None,
            placement_instance: 1,
            subcell_offset: None,
            mode: ImagePlacementMode::Inline,
        },
    );
    assert_eq!(image_cell_total(&buf), 3 * 2, "setup: the inline image");
    let top = first_visible_row(&buf);
    for row in &mut buf.rows {
        row.dirty = false;
    }

    let (fixed_id, _) = place_fixed(&mut buf, 3, 2);

    assert_eq!(image_cell_total(&buf), 3 * 2 + 3 * 2, "both images' cells");
    assert!(
        buf.image_store().contains(lower_id),
        "the lower image is not under the fixed-origin one and must survive"
    );
    assert!(buf.image_store().contains(fixed_id));
    for (idx, row) in buf.rows().iter().enumerate() {
        let in_image = (top..top + 2).contains(&idx);
        assert_eq!(row.dirty, in_image, "row {idx} dirty flag");
    }
    buf.debug_assert_invariants();
}

#[test]
fn fixed_origin_damage_is_bounded_to_the_images_rows() {
    let mut buf = Buffer::new(10, 8);
    fill_with_scrollback(&mut buf, 20);
    let top = first_visible_row(&buf);
    for row in &mut buf.rows {
        row.dirty = false;
    }

    let (_, _) = place_fixed(&mut buf, 3, 2);

    for (idx, row) in buf.rows().iter().enumerate() {
        let in_image = (top..top + 2).contains(&idx);
        assert_eq!(row.dirty, in_image, "row {idx} dirty flag");
    }
}

#[test]
fn inline_mode_still_scrolls_and_moves_the_cursor() {
    let mut buf = Buffer::new(10, 4);
    fill_with_scrollback(&mut buf, 6);
    buf.set_cursor_pos(Some(2), Some(3));
    let rows_before = buf.rows().len();

    let img = image(3, 3);
    let _ = buf.place_image(
        img,
        0,
        PlaceImageRequest {
            protocol: ImageProtocol::Sixel,
            image_number: None,
            placement_id: None,
            z_index: 0,
            source_crop: None,
            placement_instance: 1,
            subcell_offset: None,
            mode: ImagePlacementMode::Inline,
        },
    );

    assert!(
        buf.rows().len() > rows_before,
        "inline placement grows rows"
    );
    assert_eq!(buf.cursor().pos.x, 0, "inline placement resets the column");
}
