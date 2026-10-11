// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Front eviction of rows from a [`Buffer`] (Task 125.15).
//!
//! Every path that removes rows from the front of the row store --
//! `enforce_scrollback_limit`, `scroll_up`, `erase_scrollback`, the
//! alternate-screen height shrink -- goes through [`Buffer::evict_front_rows`],
//! which owns the bookkeeping that must follow:
//!
//! - the image-cell counter,
//! - the compressed blocks' live-row counts (a block is freed the moment its
//!   last row is evicted),
//! - the images whose every row is now gone (via the per-image stamp horizon,
//!   see [`crate::image_store::ImageStore::stamp_row`]),
//! - the prompt marks and command blocks that point at evicted rows.
//!
//! Each of those is proportional to the rows (or images, or marks) evicted,
//! never to the rows retained: eviction cost is independent of how much
//! scrollback is kept.

use crate::row::Row;

use super::Buffer;
use super::compression::release_block_rows;

impl Buffer {
    /// Evict the first `n` rows (clamped to the number stored) and settle all
    /// bookkeeping keyed on them. Returns the number of rows actually evicted.
    ///
    /// Callers still own the adjustments that depend on *why* rows were
    /// evicted: the cursor row, the scroll offset, and the merge cache.
    pub(in crate::buffer) fn evict_front_rows(&mut self, n: usize) -> usize {
        let n = n.min(self.rows.len());
        if n == 0 {
            return 0;
        }

        // Image cells in the rows about to go. `Row::count_image_cells`
        // short-circuits for compact and evicted (compressed) rows without
        // decompacting or decompressing them, since neither can hold an image
        // cell; the scan is skipped outright when no image cell exists.
        if self.image_cell_count > 0 {
            let drained_images: usize = self.rows[..n].iter().map(Row::count_image_cells).sum();
            self.image_cell_count -= drained_images;
        }

        // `BlockRowRef::offset_in_block` is block-relative, so surviving rows
        // need no remapping; each evicted compressed row only decrements its
        // block's live-row count, freeing the block at zero.
        let blocks = &mut self.blocks;
        let evicted = self
            .rows
            .evict_front(n, |id, count| release_block_rows(blocks, id, count))
            .rows;

        // Images whose stamp horizon is now below the base have no cell left.
        self.image_store.evict_below(self.rows.base());
        self.prune_evicted_marks();
        evicted
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::sync::Arc;

    use freminal_common::buffer_states::{
        format_tag::FormatTag, modes::declrmm::Declrmm, row_number::RowNumber, tchar::TChar,
    };

    use crate::{
        image_store::{
            AnimationControl, ImagePlacement, ImageProtocol, ImageSizeMode, InlineImage,
            next_image_id,
        },
        row::Row,
    };

    use super::*;

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

    /// Evict `n` rows from the front and keep the cursor on its row, as every
    /// production caller does for itself.
    fn evict(buf: &mut Buffer, n: usize) -> usize {
        let evicted = buf.evict_front_rows(n);
        buf.cursor.pos.y = buf.cursor.pos.y.saturating_sub(evicted);
        evicted
    }

    fn ensure_rows(buf: &mut Buffer, n: usize) {
        while buf.rows.len() < n {
            buf.rows.push(Row::new(buf.width));
        }
    }

    /// Number of cells across the buffer that belong to image `id`.
    fn cells_of(buf: &Buffer, id: u64) -> usize {
        buf.rows
            .iter()
            .flat_map(Row::cells_for_image_scan)
            .filter(|c| c.image_placement().is_some_and(|p| p.image_id == id))
            .count()
    }

    fn text(s: &str) -> Vec<TChar> {
        s.chars().map(TChar::from).collect()
    }

    fn push_lines(buf: &mut Buffer, n: usize) {
        for i in 0..n {
            buf.insert_text(&text(&format!("line{i:04}")));
            buf.handle_lf();
            buf.handle_cr();
        }
    }

    // ── Eviction bookkeeping ────────────────────────────────────────────

    #[test]
    fn evict_front_rows_clamps_and_reports_the_actual_count() {
        let mut buf = Buffer::new(10, 3).with_scrollback_limit(50);
        push_lines(&mut buf, 8);
        let len = buf.rows.len();
        assert_eq!(evict(&mut buf, 1_000), len);
        assert_eq!(evict(&mut buf, 5), 0);
    }

    #[test]
    fn evicting_rows_deducts_their_image_cells() {
        let mut buf = Buffer::new(10, 3).with_scrollback_limit(50);
        let id = place(&mut buf, ImageProtocol::Kitty, 2, 3);
        assert_eq!(buf.image_cell_count, 6);

        assert_eq!(evict(&mut buf, 2), 2);

        assert_eq!(buf.image_cell_count, 2, "only the third image row is left");
        assert_eq!(cells_of(&buf, id), 2);
        buf.debug_assert_invariants();
    }

    // ── Image stamp horizon ─────────────────────────────────────────────

    #[test]
    fn an_image_survives_while_any_of_its_cells_is_retained() {
        let mut buf = Buffer::new(10, 3).with_scrollback_limit(50);
        let id = place(&mut buf, ImageProtocol::Sixel, 2, 4);
        assert_eq!(buf.rows.base(), RowNumber::ZERO);

        // Rows 0..3 evicted; the image's last row (3) is the oldest retained.
        assert_eq!(evict(&mut buf, 3), 3);
        assert!(buf.image_store().contains(id), "row 3 still holds a cell");
        assert_eq!(cells_of(&buf, id), 2);
        buf.debug_assert_invariants();

        // Now its last row goes too.
        assert_eq!(evict(&mut buf, 1), 1);
        assert!(!buf.image_store().contains(id), "no cell left: freed");
        assert_eq!(buf.image_cell_count, 0);
        buf.debug_assert_invariants();
    }

    #[test]
    fn an_image_is_freed_through_the_scrollback_limit_path() {
        let mut buf = Buffer::new(10, 3).with_scrollback_limit(20);
        let id = place(&mut buf, ImageProtocol::Sixel, 2, 2);
        assert!(buf.image_store().contains(id));

        push_lines(&mut buf, 60);

        assert!(
            !buf.image_store().contains(id),
            "scrolled out of the retained scrollback"
        );
        assert_eq!(buf.image_cell_count, 0);
        assert!(buf.image_store().is_empty());
    }

    #[test]
    fn a_younger_image_outlives_an_older_one() {
        let mut buf = Buffer::new(10, 3).with_scrollback_limit(50);
        let old = place(&mut buf, ImageProtocol::Sixel, 1, 2);
        push_lines(&mut buf, 3);
        let young = place(&mut buf, ImageProtocol::Sixel, 1, 2);

        // Evict through the old image's rows and a little beyond.
        let old_last = buf.image_store().horizon_of(old).unwrap();
        let to_evict = old_last.rows_after(buf.rows.base()).unwrap() + 1;
        assert_eq!(evict(&mut buf, to_evict), to_evict);

        assert!(!buf.image_store().contains(old));
        assert!(buf.image_store().contains(young));
        buf.debug_assert_invariants();
    }

    #[test]
    fn kitty_images_keep_their_data_when_every_row_is_evicted() {
        let mut buf = Buffer::new(10, 3).with_scrollback_limit(50);
        let id = place(&mut buf, ImageProtocol::Kitty, 2, 2);
        let len = buf.rows.len();

        assert_eq!(evict(&mut buf, len), len);

        assert!(
            buf.image_store().contains(id),
            "Kitty data is addressable until an explicit free"
        );
        assert_eq!(buf.image_store().horizon_of(id), None, "but not placed");
        assert_eq!(buf.image_cell_count, 0);
    }

    #[test]
    fn a_kitty_image_can_be_placed_again_after_its_rows_were_evicted() {
        let mut buf = Buffer::new(10, 3).with_scrollback_limit(50);
        let id = place(&mut buf, ImageProtocol::Kitty, 2, 1);
        let len = buf.rows.len();
        let _ = evict(&mut buf, len);
        ensure_rows(&mut buf, 3);
        buf.cursor.pos.y = 1;

        buf.place_image_at(
            id,
            1,
            0,
            2,
            1,
            ImageProtocol::Kitty,
            None,
            None,
            0,
            None,
            2,
            None,
        );

        assert_eq!(cells_of(&buf, id), 2);
        assert!(buf.image_store().horizon_of(id).is_some());
        buf.debug_assert_invariants();
    }

    #[test]
    fn scroll_slice_down_widens_the_horizon_so_the_moved_image_survives() {
        let mut buf = Buffer::new(10, 5).with_scrollback_limit(50);
        ensure_rows(&mut buf, 5);
        buf.cursor.pos.y = 1;
        let id = place(&mut buf, ImageProtocol::Sixel, 2, 1);
        assert_eq!(
            buf.image_store().horizon_of(id),
            Some(RowNumber::new(1)),
            "stamped on row 1"
        );

        // The image's cells move from row 1 to row 2.
        buf.scroll_slice_down_confined(1, 4);
        assert_eq!(buf.rows[1].count_image_cells(), 0);
        assert_eq!(buf.rows[2].count_image_cells(), 2);
        assert_eq!(buf.image_store().horizon_of(id), Some(RowNumber::new(2)));

        // Rows 0 and 1 go. Row 2, which holds the image, is the oldest
        // retained; without the widening the image (horizon 1) would be freed.
        assert_eq!(evict(&mut buf, 2), 2);
        assert!(buf.image_store().contains(id));
        assert_eq!(cells_of(&buf, id), 2);
        buf.debug_assert_invariants();
    }

    #[test]
    fn scroll_slice_down_columns_widens_the_horizon_too() {
        let mut buf = Buffer::new(10, 5).with_scrollback_limit(50);
        ensure_rows(&mut buf, 5);
        buf.cursor.pos.y = 1;
        // Kitty cells survive the column sweep and are moved by the shift.
        let id = place(&mut buf, ImageProtocol::Kitty, 2, 1);

        buf.set_declrmm(Declrmm::Enabled);
        buf.set_left_right_margins(1, 5);
        buf.scroll_slice_down_confined(1, 4);
        assert_eq!(buf.rows[2].count_image_cells(), 2);
        assert_eq!(buf.image_store().horizon_of(id), Some(RowNumber::new(2)));

        let _ = evict(&mut buf, 2);
        assert!(buf.image_store().contains(id));
        assert_eq!(cells_of(&buf, id), 2);
        buf.debug_assert_invariants();
    }

    #[test]
    fn scroll_slice_up_keeps_the_image_alive_without_widening() {
        let mut buf = Buffer::new(10, 5).with_scrollback_limit(50);
        ensure_rows(&mut buf, 5);
        buf.cursor.pos.y = 3;
        let id = place(&mut buf, ImageProtocol::Sixel, 2, 1);
        assert_eq!(buf.image_store().horizon_of(id), Some(RowNumber::new(3)));

        // Cells move up to row 2: still at or below the horizon.
        buf.scroll_slice_up(1, 4);
        assert_eq!(buf.rows[2].count_image_cells(), 2);
        assert_eq!(buf.image_store().horizon_of(id), Some(RowNumber::new(3)));
        buf.debug_assert_invariants();

        let _ = evict(&mut buf, 3);
        assert!(buf.image_store().contains(id), "row 3's number is retained");
    }

    #[test]
    fn insert_lines_moves_image_cells_down_and_keeps_them_alive() {
        let mut buf = Buffer::new(10, 6).with_scrollback_limit(50);
        ensure_rows(&mut buf, 6);
        buf.cursor.pos.y = 2;
        let id = place(&mut buf, ImageProtocol::Kitty, 1, 1);
        buf.cursor.pos.y = 2;

        buf.insert_lines(2);

        assert_eq!(buf.rows[4].count_image_cells(), 1);
        assert_eq!(buf.image_store().horizon_of(id), Some(RowNumber::new(4)));
        buf.debug_assert_invariants();
    }

    #[test]
    fn reflow_recomputes_horizons_for_the_renumbered_rows() {
        let mut buf = Buffer::new(10, 4).with_scrollback_limit(50);
        push_lines(&mut buf, 3);
        let id = place(&mut buf, ImageProtocol::Sixel, 2, 2);
        let before = buf.image_store().horizon_of(id).unwrap();

        buf.reflow_to_width(6);

        // Reflow numbers its rows from the old next_number, so the old
        // horizon is stale (below the new base); the recomputed one is not.
        let after = buf.image_store().horizon_of(id).unwrap();
        assert!(after > before);
        assert!(buf.rows.index_of(after).is_some(), "names a retained row");
        let last_image_row = buf
            .rows
            .iter()
            .rposition(|r| r.count_image_cells() > 0)
            .unwrap();
        assert_eq!(buf.rows.number_of(last_image_row), after);
        buf.debug_assert_invariants();

        // The image survives eviction up to (not including) its last row.
        let to_evict = last_image_row;
        let _ = evict(&mut buf, to_evict);
        assert!(buf.image_store().contains(id));
        let _ = evict(&mut buf, 1);
        assert!(!buf.image_store().contains(id));
    }

    #[test]
    fn reflow_frees_a_cell_owned_image_that_nothing_references() {
        let mut buf = Buffer::new(10, 4).with_scrollback_limit(50);
        push_lines(&mut buf, 2);
        let orphan = make_image(1, 1);
        let orphan_id = orphan.id;
        buf.image_store_mut().insert(orphan);

        buf.reflow_to_width(7);

        assert!(!buf.image_store().contains(orphan_id));
    }

    #[test]
    fn reflow_keeps_kitty_data_without_cells() {
        let mut buf = Buffer::new(10, 4).with_scrollback_limit(50);
        push_lines(&mut buf, 2);
        let kitty = make_image(1, 1);
        let kitty_id = kitty.id;
        buf.image_store_mut().insert_protocol_retained(kitty);

        buf.reflow_to_width(7);

        assert!(buf.image_store().contains(kitty_id));
    }

    #[test]
    fn erase_scrollback_frees_scrollback_images_and_keeps_visible_ones() {
        let mut buf = Buffer::new(10, 3).with_scrollback_limit(50);
        let old = place(&mut buf, ImageProtocol::Sixel, 1, 1);
        push_lines(&mut buf, 8);
        let visible = place(&mut buf, ImageProtocol::Sixel, 1, 1);

        buf.erase_scrollback();

        assert!(!buf.image_store().contains(old), "ED 3 evicts scrollback");
        assert!(buf.image_store().contains(visible));
        assert_eq!(cells_of(&buf, visible), 1);
        buf.debug_assert_invariants();
    }

    #[test]
    fn scroll_up_on_the_alternate_screen_frees_an_image_whose_row_left() {
        let mut buf = Buffer::new(10, 3);
        buf.enter_fresh_alternate();
        buf.cursor.pos.y = 0;
        let id = place(&mut buf, ImageProtocol::Sixel, 1, 1);
        assert!(buf.image_store().contains(id));
        buf.cursor.pos.y = 0;

        buf.scroll_up();

        assert!(!buf.image_store().contains(id));
        assert_eq!(buf.image_cell_count, 0);
        buf.debug_assert_invariants();
    }

    #[test]
    fn alternate_screen_height_shrink_evicts_images_through_the_same_path() {
        let mut buf = Buffer::new(10, 4);
        buf.enter_fresh_alternate();
        buf.cursor.pos.y = 0;
        let id = place(&mut buf, ImageProtocol::Sixel, 1, 1);
        assert!(buf.image_store().contains(id));

        let _ = buf.set_size(10, 2, 0);

        assert!(!buf.image_store().contains(id), "row 0 was drained");
        buf.debug_assert_invariants();
    }

    #[test]
    fn leaving_the_alternate_screen_restores_the_primary_horizons() {
        let mut buf = Buffer::new(10, 3).with_scrollback_limit(50);
        let id = place(&mut buf, ImageProtocol::Sixel, 1, 2);
        let horizon = buf.image_store().horizon_of(id);
        assert!(horizon.is_some());

        buf.enter_fresh_alternate();
        assert!(buf.image_store().is_empty(), "alt screen has its own store");
        buf.switch_to_primary();

        assert_eq!(buf.image_store().horizon_of(id), horizon);
        buf.debug_assert_invariants();
    }

    #[test]
    fn a_placement_with_no_room_does_not_leak_a_cell_owned_image() {
        let mut buf = Buffer::new(10, 3);
        buf.cursor.pos.x = 10; // past the last column: zero cells fit
        let id = place(&mut buf, ImageProtocol::Sixel, 2, 1);
        assert_eq!(cells_of(&buf, id), 0);
        assert!(
            !buf.image_store().contains(id),
            "nothing could ever free it"
        );
    }

    #[test]
    fn a_placement_with_no_room_keeps_kitty_data() {
        let mut buf = Buffer::new(10, 3);
        buf.cursor.pos.x = 10;
        let id = place(&mut buf, ImageProtocol::Kitty, 2, 1);
        assert!(buf.image_store().contains(id));
    }

    #[test]
    fn set_image_cell_at_stamps_the_horizon() {
        let mut buf = Buffer::new(10, 4).with_scrollback_limit(50);
        ensure_rows(&mut buf, 4);
        let image = make_image(1, 1);
        let id = image.id;
        buf.image_store_mut().insert_protocol_retained(image);
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

        buf.set_image_cell_at(2, 0, placement.clone(), FormatTag::default());
        assert_eq!(buf.image_store().horizon_of(id), Some(RowNumber::new(2)));
        buf.set_image_cell_at(1, 0, placement, FormatTag::default());
        assert_eq!(
            buf.image_store().horizon_of(id),
            Some(RowNumber::new(2)),
            "a lower row never lowers it"
        );
        buf.debug_assert_invariants();
    }

    #[test]
    fn place_image_at_stamps_the_greatest_row_of_the_stamped_cells() {
        let mut buf = Buffer::new(10, 6).with_scrollback_limit(50);
        ensure_rows(&mut buf, 6);
        let image = make_image(2, 3);
        let id = image.id;
        buf.image_store_mut().insert_protocol_retained(image);

        buf.place_image_at(
            id,
            1,
            0,
            2,
            3,
            ImageProtocol::Kitty,
            None,
            None,
            0,
            None,
            1,
            None,
        );

        assert_eq!(buf.image_store().horizon_of(id), Some(RowNumber::new(3)));
        buf.debug_assert_invariants();
    }

    #[test]
    fn horizon_numbers_survive_eviction_unchanged() {
        let mut buf = Buffer::new(10, 3).with_scrollback_limit(50);
        push_lines(&mut buf, 6);
        let id = place(&mut buf, ImageProtocol::Sixel, 1, 1);
        let horizon = buf.image_store().horizon_of(id).unwrap();

        let _ = evict(&mut buf, 2);

        assert_eq!(buf.image_store().horizon_of(id), Some(horizon));
        assert!(buf.rows.index_of(horizon).is_some());
    }

    // ── Cell-owned images are freed as soon as nothing references them ──

    #[test]
    fn clearing_an_image_by_id_frees_a_cell_owned_image() {
        let mut buf = Buffer::new(10, 4).with_scrollback_limit(50);
        let id = place(&mut buf, ImageProtocol::Sixel, 2, 2);

        buf.clear_image_placements_by_id(id);

        assert!(!buf.image_store().contains(id));
        assert_eq!(buf.image_store().horizon_of(id), None);
        assert_eq!(buf.image_cell_count, 0);
        buf.debug_assert_invariants();
    }

    #[test]
    fn clearing_an_image_by_id_keeps_kitty_data() {
        let mut buf = Buffer::new(10, 4).with_scrollback_limit(50);
        let id = place(&mut buf, ImageProtocol::Kitty, 2, 2);

        buf.clear_image_placements_by_id(id);

        assert!(buf.image_store().contains(id), "addressable without cells");
        assert_eq!(buf.image_cell_count, 0);
        buf.debug_assert_invariants();
    }

    #[test]
    fn overwriting_a_sixel_with_text_frees_its_image() {
        let mut buf = Buffer::new(10, 4).with_scrollback_limit(50);
        let id = place(&mut buf, ImageProtocol::Sixel, 2, 1);
        buf.cursor.pos.y = 0;
        buf.cursor.pos.x = 0;

        buf.insert_text(&text("hi"));

        assert!(!buf.image_store().contains(id));
        buf.debug_assert_invariants();
    }

    /// In-place animation: every frame is a new image placed over the last at
    /// the same spot, and nothing scrolls, so no row is ever evicted. The
    /// store must not accumulate the dead frames.
    #[test]
    fn in_place_sixel_replacement_keeps_the_store_bounded() {
        let mut buf = Buffer::new(10, 5).with_scrollback_limit(50);
        for frame in 0..60 {
            buf.cursor.pos.x = 0;
            buf.cursor.pos.y = 0;
            let _ = place(&mut buf, ImageProtocol::Sixel, 2, 2);
            assert_eq!(buf.image_store().len(), 1, "frame {frame}");
        }
        assert_eq!(buf.image_cell_count, 4);
        buf.debug_assert_invariants();
    }

    /// The same animation next to a static image: the buffer still holds image
    /// cells after the pre-clear, so the cheap "no cells at all" answer is not
    /// available and the cells are scanned.
    #[test]
    fn in_place_replacement_beside_a_static_image_keeps_the_store_bounded() {
        let mut buf = Buffer::new(10, 8).with_scrollback_limit(50);
        buf.cursor.pos.x = 6;
        buf.cursor.pos.y = 5;
        let kitty = place(&mut buf, ImageProtocol::Kitty, 2, 1);
        buf.cursor.pos.x = 6;
        buf.cursor.pos.y = 2;
        let static_sixel = place(&mut buf, ImageProtocol::Sixel, 2, 1);

        for frame in 0..60 {
            buf.cursor.pos.x = 0;
            buf.cursor.pos.y = 0;
            let _ = place(&mut buf, ImageProtocol::Sixel, 2, 2);
            assert_eq!(buf.image_store().len(), 3, "frame {frame}");
        }
        assert!(buf.image_store().contains(kitty));
        assert!(buf.image_store().contains(static_sixel), "untouched");
        assert_eq!(cells_of(&buf, static_sixel), 2);
        buf.debug_assert_invariants();
    }

    /// Replacing part of an image leaves the rest of it on screen; it is still
    /// referenced and must survive.
    #[test]
    fn a_partly_overwritten_image_survives_the_replacement() {
        let mut buf = Buffer::new(10, 6).with_scrollback_limit(50);
        let big = place(&mut buf, ImageProtocol::Sixel, 4, 2);
        buf.cursor.pos.x = 0;
        buf.cursor.pos.y = 0;

        let small = place(&mut buf, ImageProtocol::Sixel, 2, 1);

        assert!(buf.image_store().contains(small));
        assert!(buf.image_store().contains(big), "4 of its 8 cells remain");
        assert_eq!(cells_of(&buf, big), 4);
        buf.debug_assert_invariants();
    }

    // ── Kitty images and the horizon assertion ──────────────────────────

    fn kitty_placement(id: u64) -> ImagePlacement {
        ImagePlacement {
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
        }
    }

    /// Unicode-placeholder cells can be written before the image is
    /// transmitted. They get no horizon (the image is not stored yet); the
    /// invariant check must not treat the later transmission as a violation.
    #[test]
    fn cells_stamped_before_a_kitty_image_is_transmitted_pass_the_invariants() {
        let mut buf = Buffer::new(10, 4).with_scrollback_limit(50);
        ensure_rows(&mut buf, 4);
        let image = make_image(1, 1);
        let id = image.id;
        buf.set_image_cell_at(3, 0, kitty_placement(id), FormatTag::default());
        assert_eq!(buf.image_store().horizon_of(id), None);

        buf.image_store_mut().insert_protocol_retained(image);

        buf.debug_assert_invariants();
        // Eviction never frees the retained data.
        let len = buf.rows.len();
        let _ = evict(&mut buf, len);
        assert!(buf.image_store().contains(id));
    }

    #[test]
    fn a_retransmitted_kitty_id_with_live_cells_passes_the_invariants() {
        let mut buf = Buffer::new(10, 4).with_scrollback_limit(50);
        let id = place(&mut buf, ImageProtocol::Kitty, 2, 1);
        let again = buf.image_store_mut().remove(id).unwrap();
        assert_eq!(buf.image_store().horizon_of(id), None);

        // The id comes back while its old cells are still on screen.
        buf.image_store_mut().insert_protocol_retained(again);

        assert_eq!(cells_of(&buf, id), 2);
        buf.debug_assert_invariants();
    }

    // ── Sweeps, compression and resize interplay ────────────────────────

    #[test]
    fn a_sixel_swept_by_a_column_confined_scroll_is_freed_cleanly() {
        let mut buf = Buffer::new(10, 5).with_scrollback_limit(50);
        ensure_rows(&mut buf, 5);
        buf.cursor.pos.y = 1;
        let id = place(&mut buf, ImageProtocol::Sixel, 2, 1);
        buf.set_declrmm(Declrmm::Enabled);
        buf.set_left_right_margins(1, 5);

        // The sweep clears the whole (non-Kitty) image before the shift.
        buf.scroll_slice_down_confined(1, 4);

        assert_eq!(cells_of(&buf, id), 0);
        assert_eq!(buf.image_cell_count, 0);
        assert!(!buf.image_store().contains(id), "nothing left to keep it");
        buf.debug_assert_invariants();
    }

    #[test]
    fn horizons_survive_compression_and_decompression_around_an_image_row() {
        let mut buf = Buffer::new(20, 3).with_scrollback_limit(200);
        push_lines(&mut buf, 6);
        let id = place(&mut buf, ImageProtocol::Sixel, 2, 1);
        push_lines(&mut buf, 10);
        let horizon = buf.image_store().horizon_of(id).unwrap();

        let _ = buf.compact_idle_scrollback(usize::MAX);
        let compressed = buf.compress_idle_scrollback(usize::MAX);
        assert!(compressed > 0 && !buf.blocks.is_empty(), "rows compressed");
        // The image row opts out of compaction, so it is never compressed.
        assert_eq!(cells_of(&buf, id), 2);
        buf.debug_assert_invariants();

        // Reading the scrollback decompresses every block.
        let _ = buf.scrollback_as_tchars_and_tags(0);
        assert!(buf.blocks.is_empty());
        assert_eq!(buf.image_store().horizon_of(id), Some(horizon));
        buf.debug_assert_invariants();

        // Eviction still releases the image exactly when its row goes.
        let image_index = buf.rows.index_of(horizon).unwrap();
        let _ = evict(&mut buf, image_index);
        assert!(buf.image_store().contains(id));
        let _ = evict(&mut buf, 1);
        assert!(!buf.image_store().contains(id));
        buf.debug_assert_invariants();
    }

    #[test]
    fn resizing_the_saved_primary_screen_carries_the_horizons() {
        let mut buf = Buffer::new(10, 4).with_scrollback_limit(50);
        push_lines(&mut buf, 3);
        let id = place(&mut buf, ImageProtocol::Sixel, 2, 2);
        buf.enter_fresh_alternate();

        // Reflows the parked primary screen, renumbering its rows.
        let _ = buf.set_size(6, 4, 0);
        buf.switch_to_primary();

        let horizon = buf.image_store().horizon_of(id).expect("image kept");
        let index = buf.rows.index_of(horizon).expect("names a retained row");
        let last_image_row = buf
            .rows
            .iter()
            .rposition(|r| r.count_image_cells() > 0)
            .unwrap();
        assert_eq!(index, last_image_row);
        buf.debug_assert_invariants();

        // The image is released when, and only when, its last row is evicted.
        let _ = evict(&mut buf, index);
        assert!(buf.image_store().contains(id));
        let _ = evict(&mut buf, 1);
        assert!(!buf.image_store().contains(id));
    }

    // ── Restamping is skipped when nothing could be exceeded ────────────

    #[test]
    fn insert_lines_restamps_once_for_a_multi_line_shift() {
        let mut buf = Buffer::new(10, 8).with_scrollback_limit(50);
        ensure_rows(&mut buf, 8);
        buf.cursor.pos.y = 2;
        let id = place(&mut buf, ImageProtocol::Kitty, 1, 1);
        buf.cursor.pos.y = 2;

        buf.insert_lines(3);

        assert_eq!(buf.rows[5].count_image_cells(), 1);
        assert_eq!(buf.image_store().horizon_of(id), Some(RowNumber::new(5)));
        buf.debug_assert_invariants();
    }

    #[test]
    fn a_scroll_down_below_every_horizon_leaves_the_horizons_alone() {
        let mut buf = Buffer::new(10, 8).with_scrollback_limit(50);
        ensure_rows(&mut buf, 8);
        buf.cursor.pos.y = 1;
        let id = place(&mut buf, ImageProtocol::Sixel, 1, 1);
        assert_eq!(buf.image_store().horizon_of(id), Some(RowNumber::new(1)));

        // The shifted rows 4..=7 lie wholly below the image.
        buf.scroll_slice_down_confined_n(4, 7, 2);

        assert_eq!(buf.image_store().horizon_of(id), Some(RowNumber::new(1)));
        assert_eq!(cells_of(&buf, id), 1);
    }
}
