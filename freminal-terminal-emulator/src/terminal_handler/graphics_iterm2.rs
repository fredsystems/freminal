// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! iTerm2 inline image protocol handler for [`TerminalHandler`].
//!
//! This module implements all iTerm2 `OSC 1337` image command processing:
//!
//! - Single-transfer path (`handle_iterm2_inline_image`) — decodes and places an inline image
//! - Multipart begin (`handle_iterm2_multipart_begin`) — starts a chunked transfer
//! - Multipart chunk (`handle_iterm2_file_part`) — appends bytes to the active transfer
//! - Multipart end (`handle_iterm2_file_end`) — assembles accumulated data and places image
//! - Dimension resolver (`resolve_image_dimension`) — maps iTerm2 dimension specs to cell counts
//! - Aspect-ratio helper (`apply_aspect_ratio`) — scales the auto dimension to preserve ratio

use conv2::ValueFrom;
use freminal_common::buffer_states::osc::{ITerm2InlineImageData, ImageDimension};

use freminal_buffer::image_store::{
    ImageProtocol, ImageSizeMode, InlineImage, next_image_id, next_placement_instance_id,
};

use super::chunk_assembler::{BoundedChunkAssembler, ChunkEncoding, ChunkLimits};
use super::{MultipartImageState, TerminalHandler};
use crate::ansi_components::tracer::lossy_sequence_for_log_bounded;

/// Maximum size of an iTerm2 multipart transfer, per `FilePart=` chunk and in
/// total.
///
/// Matches the OSC 1337 parser cap
/// ([`MAX_OSC_LARGE_BYTES`](crate::ansi_components::osc::MAX_OSC_LARGE_BYTES)):
/// a single `FilePart=` sequence can never be larger than that, and the
/// assembled file is held to the same bound.
const MAX_ITERM2_MULTIPART_BYTES: usize = 64 * 1024 * 1024;

/// The caps applied to an iTerm2 multipart transfer.
const ITERM2_MULTIPART_LIMITS: ChunkLimits = ChunkLimits {
    max_chunk_bytes: MAX_ITERM2_MULTIPART_BYTES,
    max_total_bytes: MAX_ITERM2_MULTIPART_BYTES,
};

impl TerminalHandler {
    /// Handle an iTerm2 `OSC 1337 ; File=` inline image.
    ///
    /// Decodes the raw image bytes into RGBA pixels, computes the display size
    /// in terminal cells from the dimension specs, and places the image into
    /// the buffer at the cursor position.
    pub(super) fn handle_iterm2_inline_image(&mut self, data: &ITerm2InlineImageData) {
        if !data.inline {
            tracing::debug!("OSC 1337 File=: inline=0 (download), ignoring");
            return;
        }

        // Decode the raw image bytes into RGBA pixels.
        let img = match image::load_from_memory(&data.data) {
            Ok(img) => img.to_rgba8(),
            Err(e) => {
                tracing::warn!("OSC 1337 File=: image decode failed");
                tracing::debug!("OSC 1337 File=: image decode failed: {e}");
                return;
            }
        };

        let img_width_px = img.width();
        let img_height_px = img.height();

        if img_width_px == 0 || img_height_px == 0 {
            tracing::warn!("OSC 1337 File=: decoded image has zero dimensions");
            return;
        }

        let (term_width, term_height) = self.win_size();

        // Compute the display size in cells from the iTerm2 dimension specs.
        let display_cols = Self::resolve_image_dimension(
            data.width.as_ref(),
            img_width_px,
            term_width,
            self.cell_pixel_width,
        );
        let display_rows = Self::resolve_image_dimension(
            data.height.as_ref(),
            img_height_px,
            term_height,
            self.cell_pixel_height,
        );

        // Apply aspect-ratio preservation when only one dimension was
        // explicitly specified by the user.
        let (display_cols, display_rows) = if data.preserve_aspect_ratio {
            Self::apply_aspect_ratio(
                data.width.as_ref(),
                data.height.as_ref(),
                display_cols,
                display_rows,
                img_width_px,
                img_height_px,
                term_width,
                term_height,
                self.cell_pixel_width,
                self.cell_pixel_height,
            )
        } else {
            (display_cols, display_rows)
        };

        if display_cols == 0 || display_rows == 0 {
            tracing::warn!("OSC 1337 File=: computed display size is 0x0");
            return;
        }

        // An explicit (non-auto) width or height request means the image is
        // scaled to fill the declared cell grid; `None`/`Auto` on both axes
        // means the grid was derived from the native pixel size (Task
        // 100.17). `Auto` is represented as `Some(ImageDimension::Auto)`, not
        // `None`, so it must be checked explicitly rather than inferred from
        // `Option::is_some()`.
        let width_explicit = !matches!(data.width, None | Some(ImageDimension::Auto));
        let height_explicit = !matches!(data.height, None | Some(ImageDimension::Auto));
        let size_mode = if width_explicit || height_explicit {
            ImageSizeMode::ExplicitCells
        } else {
            ImageSizeMode::NativePixels
        };

        let pixels = img.into_raw();
        let inline_image = InlineImage {
            id: next_image_id(),
            pixels: std::sync::Arc::new(pixels),
            width_px: img_width_px,
            height_px: img_height_px,
            display_cols,
            display_rows,
            size_mode,
            frames: Vec::new(),
            root_gap_ms: 0,
            animation: freminal_buffer::image_store::AnimationControl::default(),
        };

        // Place the image into the buffer. Pass 0 for scroll_offset — the
        // PTY thread always operates at the live bottom.
        //
        // iTerm2 has no placement-id concept and no `X=`/`Y=` sub-cell
        // control key — mint a fresh placement-instance id (Task 100.18)
        // so this placement never merges with an unrelated one, and pass
        // `None` sub-cell offset (Task 100.19, kitty-only).
        let placement_instance = next_placement_instance_id();
        let place_result = self.buffer.place_image(
            inline_image,
            0,
            ImageProtocol::ITerm2,
            None,
            None,
            0,
            None,
            placement_instance,
            None,
        );

        // doNotMoveCursor: the iTerm2 protocol keeps the cursor at its
        // pre-image position, i.e. on the image origin. Locate it by row
        // number so scrollback eviction during placement cannot displace it
        // (Task 125.C7).
        if data.do_not_move_cursor {
            self.buffer.restore_cursor_to_image_origin(&place_result);
        }
    }

    /// Handle `OSC 1337 ; MultipartFile = [args]` — begin a multipart transfer.
    ///
    /// Stores the metadata and initialises an accumulator for incoming `FilePart`
    /// chunks.  If a previous multipart transfer was in progress, it is discarded
    /// with a warning.
    pub(super) fn handle_iterm2_multipart_begin(&mut self, data: &ITerm2InlineImageData) {
        if self.multipart_state.is_some() {
            tracing::warn!(
                "OSC 1337 MultipartFile=: new transfer started while previous was in progress; \
                 discarding incomplete transfer"
            );
        }

        // The sender-supplied `size=` is never used to size an allocation;
        // the assembler grows only as bytes actually arrive.
        self.multipart_state = Some(MultipartImageState {
            metadata: data.clone(),
            data: BoundedChunkAssembler::new(ITERM2_MULTIPART_LIMITS),
        });

        tracing::debug!(
            "OSC 1337 MultipartFile=: started transfer (name={:?}, size={:?})",
            data.name
                .as_deref()
                .map(|n| lossy_sequence_for_log_bounded(n.as_bytes())),
            data.size,
        );
    }

    /// Handle `OSC 1337 ; FilePart = [base64]` — append a chunk to the active
    /// multipart transfer.
    pub(super) fn handle_iterm2_file_part(&mut self, bytes: &[u8]) {
        let Some(state) = &mut self.multipart_state else {
            tracing::warn!("OSC 1337 FilePart=: no active multipart transfer; ignoring chunk");
            return;
        };

        if let Err(err) = state.data.push(bytes, ChunkEncoding::Raw) {
            tracing::warn!("OSC 1337 FilePart=: multipart transfer abandoned: {err}");
            self.multipart_state = None;
            return;
        }
        tracing::debug!(
            "OSC 1337 FilePart=: appended {} bytes (total so far: {})",
            bytes.len(),
            state.data.len(),
        );
    }

    /// Handle `OSC 1337 ; FileEnd` — complete the active multipart transfer.
    ///
    /// Assembles the final `ITerm2InlineImageData` from the accumulated chunks
    /// and delegates to `handle_iterm2_inline_image` for decoding and placement.
    pub(super) fn handle_iterm2_file_end(&mut self) {
        let Some(state) = self.multipart_state.take() else {
            tracing::warn!("OSC 1337 FileEnd: no active multipart transfer; ignoring");
            return;
        };

        if state.data.is_empty() {
            tracing::warn!("OSC 1337 FileEnd: transfer completed with empty payload; ignoring");
            return;
        }

        let total = state.data.len();
        let data = match state.data.finish() {
            Ok(data) => data,
            Err(err) => {
                tracing::warn!("OSC 1337 FileEnd: multipart transfer abandoned: {err}");
                return;
            }
        };

        tracing::debug!("OSC 1337 FileEnd: transfer complete ({total} bytes)");

        // Assemble the final image data from metadata + accumulated bytes.
        let final_data = ITerm2InlineImageData {
            data,
            ..state.metadata
        };

        self.handle_iterm2_inline_image(&final_data);
    }

    /// Resolve an iTerm2 image dimension spec to a cell count.
    ///
    /// `is_width` indicates whether we are computing columns (true) or rows (false).
    /// When the spec is `Auto` or `None`, the full image pixel size is used,
    /// divided by the terminal dimension to get a proportional cell count.
    pub(super) fn resolve_image_dimension(
        spec: Option<&ImageDimension>,
        image_pixels: u32,
        term_cells: usize,
        cell_pixels: u32,
    ) -> usize {
        match spec {
            None | Some(ImageDimension::Auto) => {
                // image_pixels / cell_pixels, rounded up, clamped to term size.
                let cells = image_pixels.saturating_add(cell_pixels - 1) / cell_pixels;
                usize::value_from(cells).unwrap_or(0).min(term_cells).max(1)
            }
            Some(ImageDimension::Cells(n)) => {
                usize::value_from(*n).unwrap_or(0).min(term_cells).max(1)
            }
            Some(ImageDimension::Pixels(px)) => {
                let cells = px.saturating_add(cell_pixels - 1) / cell_pixels;
                usize::value_from(cells).unwrap_or(0).min(term_cells).max(1)
            }
            Some(ImageDimension::Percent(pct)) => {
                let cells = (u64::from(*pct) * u64::value_from(term_cells).unwrap_or(0)) / 100;
                // Safe: cells is bounded by term_cells which fits in usize.
                usize::value_from(cells).unwrap_or(0).min(term_cells).max(1)
            }
        }
    }

    /// Adjust display dimensions to preserve aspect ratio.
    ///
    /// When `preserve_aspect_ratio` is true and one dimension is auto/unspecified,
    /// scale the auto dimension to match the other using real cell pixel
    /// dimensions for correct terminal-cell aspect-ratio compensation.
    // All parameters represent independent geometric inputs. A struct would not improve clarity
    // for this pure geometric helper.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn apply_aspect_ratio(
        width_spec: Option<&ImageDimension>,
        height_spec: Option<&ImageDimension>,
        display_cols: usize,
        display_rows: usize,
        img_width_px: u32,
        img_height_px: u32,
        term_width: usize,
        term_height: usize,
        cell_pixel_width: u32,
        cell_pixel_height: u32,
    ) -> (usize, usize) {
        let width_is_auto = matches!(width_spec, None | Some(ImageDimension::Auto));
        let height_is_auto = matches!(height_spec, None | Some(ImageDimension::Auto));

        // Use actual cell pixel dimensions for aspect-ratio compensation.
        let cpw = u64::from(cell_pixel_width.max(1));
        let cph = u64::from(cell_pixel_height.max(1));

        if width_is_auto && !height_is_auto {
            // Height was explicitly set; scale width to preserve aspect ratio.
            // scaled_cols = display_rows * (img_w / img_h) * (cell_h / cell_w)
            let scaled =
                (u64::from(img_width_px) * u64::value_from(display_rows).unwrap_or(0) * cph)
                    / (u64::from(img_height_px).max(1) * cpw);
            // Safe: scaled is bounded by term_width which fits in usize.
            let cols = usize::value_from(scaled)
                .unwrap_or(0)
                .min(term_width)
                .max(1);
            (cols, display_rows)
        } else if !width_is_auto && height_is_auto {
            // Width was explicitly set; scale height to preserve aspect ratio.
            // scaled_rows = display_cols * (img_h / img_w) * (cell_w / cell_h)
            let scaled =
                (u64::from(img_height_px) * u64::value_from(display_cols).unwrap_or(0) * cpw)
                    / (u64::from(img_width_px).max(1) * cph);
            // Safe: scaled is bounded by term_height which fits in usize.
            let rows = usize::value_from(scaled)
                .unwrap_or(0)
                .min(term_height)
                .max(1);
            (display_cols, rows)
        } else {
            // Both auto or both explicit — no adjustment needed.
            (display_cols, display_rows)
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use freminal_common::buffer_states::osc::{AnsiOscType, ITerm2InlineImageData, ImageDimension};
    use freminal_common::pty_write::PtyWrite;

    use super::*;

    // ------------------------------------------------------------------
    // resolve_image_dimension tests
    // ------------------------------------------------------------------

    #[test]
    fn resolve_auto_uses_image_pixels() {
        // 160px wide image, 8px per cell → 20 cells
        let result = TerminalHandler::resolve_image_dimension(None, 160, 80, 8);
        assert_eq!(result, 20);
    }

    #[test]
    fn resolve_auto_explicit() {
        let result =
            TerminalHandler::resolve_image_dimension(Some(&ImageDimension::Auto), 160, 80, 8);
        assert_eq!(result, 20);
    }

    #[test]
    fn resolve_auto_height() {
        // 320px tall image, 16px per cell → 20 rows
        let result = TerminalHandler::resolve_image_dimension(None, 320, 24, 16);
        assert_eq!(result, 20);
    }

    #[test]
    fn resolve_auto_clamps_to_term_size() {
        // 10000px wide, 8px/cell = 1250 cells, but term is only 80
        let result = TerminalHandler::resolve_image_dimension(None, 10000, 80, 8);
        assert_eq!(result, 80);
    }

    #[test]
    fn resolve_auto_minimum_is_1() {
        // 0px image → would be 0 cells, but minimum is 1
        let result = TerminalHandler::resolve_image_dimension(None, 0, 80, 8);
        assert_eq!(result, 1);
    }

    #[test]
    fn resolve_cells_direct() {
        let result =
            TerminalHandler::resolve_image_dimension(Some(&ImageDimension::Cells(10)), 999, 80, 8);
        assert_eq!(result, 10);
    }

    #[test]
    fn resolve_cells_clamped_to_term() {
        let result =
            TerminalHandler::resolve_image_dimension(Some(&ImageDimension::Cells(200)), 999, 80, 8);
        assert_eq!(result, 80);
    }

    #[test]
    fn resolve_pixels() {
        // 80px wide, 8px/cell → 10 cells
        let result =
            TerminalHandler::resolve_image_dimension(Some(&ImageDimension::Pixels(80)), 999, 80, 8);
        assert_eq!(result, 10);
    }

    #[test]
    fn resolve_pixels_rounds_up() {
        // 81px wide, 8px/cell → ceil(81/8) = 11 cells
        let result =
            TerminalHandler::resolve_image_dimension(Some(&ImageDimension::Pixels(81)), 999, 80, 8);
        assert_eq!(result, 11);
    }

    #[test]
    fn resolve_percent() {
        // 50% of 80 cols = 40
        let result = TerminalHandler::resolve_image_dimension(
            Some(&ImageDimension::Percent(50)),
            999,
            80,
            8,
        );
        assert_eq!(result, 40);
    }

    #[test]
    fn resolve_percent_100() {
        let result = TerminalHandler::resolve_image_dimension(
            Some(&ImageDimension::Percent(100)),
            999,
            80,
            8,
        );
        assert_eq!(result, 80);
    }

    // ------------------------------------------------------------------
    // apply_aspect_ratio tests
    // ------------------------------------------------------------------

    #[test]
    fn aspect_ratio_both_auto_no_adjustment() {
        let (cols, rows) =
            TerminalHandler::apply_aspect_ratio(None, None, 20, 10, 160, 160, 80, 24, 8, 16);
        // Both auto → no change
        assert_eq!(cols, 20);
        assert_eq!(rows, 10);
    }

    #[test]
    fn aspect_ratio_both_explicit_no_adjustment() {
        let (cols, rows) = TerminalHandler::apply_aspect_ratio(
            Some(&ImageDimension::Cells(20)),
            Some(&ImageDimension::Cells(10)),
            20,
            10,
            160,
            160,
            80,
            24,
            8,
            16,
        );
        assert_eq!(cols, 20);
        assert_eq!(rows, 10);
    }

    #[test]
    fn aspect_ratio_width_auto_height_explicit() {
        // height=10 rows, image is square (100x100), cell aspect 2:1 (8w x 16h)
        // scaled_cols = 10 * 100 * 16 / (100 * 8) = 20
        let (cols, rows) = TerminalHandler::apply_aspect_ratio(
            None,
            Some(&ImageDimension::Cells(10)),
            1, // initial cols (ignored, will be recomputed)
            10,
            100,
            100,
            80,
            24,
            8,
            16,
        );
        assert_eq!(rows, 10);
        assert_eq!(cols, 20);
    }

    #[test]
    fn aspect_ratio_height_auto_width_explicit() {
        // width=20 cols, image is square (100x100), cell aspect 2:1 (8w x 16h)
        // scaled_rows = 20 * 100 * 8 / (100 * 16) = 10
        let (cols, rows) = TerminalHandler::apply_aspect_ratio(
            Some(&ImageDimension::Cells(20)),
            None,
            20,
            1, // initial rows (ignored, will be recomputed)
            100,
            100,
            80,
            24,
            8,
            16,
        );
        assert_eq!(cols, 20);
        assert_eq!(rows, 10);
    }

    #[test]
    fn aspect_ratio_clamped_to_term_size() {
        // width=80 cols, image is very tall (100w x 10000h)
        // scaled_rows = 80 * 10000 * 8 / (100 * 16) = 4000, clamped to 24
        let (cols, rows) = TerminalHandler::apply_aspect_ratio(
            Some(&ImageDimension::Cells(80)),
            None,
            80,
            1,
            100,
            10000,
            80,
            24,
            8,
            16,
        );
        assert_eq!(cols, 80);
        assert_eq!(rows, 24);
    }

    #[test]
    fn aspect_ratio_non_square_cells() {
        // Verify the fix: non-2:1 cell aspect (e.g. 10w x 20h).
        // height=10 rows, square image (200x200)
        // scaled_cols = 10 * 200 * 20 / (200 * 10) = 20
        let (cols, rows) = TerminalHandler::apply_aspect_ratio(
            None,
            Some(&ImageDimension::Cells(10)),
            1,
            10,
            200,
            200,
            80,
            24,
            10,
            20,
        );
        assert_eq!(rows, 10);
        assert_eq!(cols, 20);

        // With 12w x 18h cells (1.5:1 ratio), square image
        // scaled_cols = 10 * 200 * 18 / (200 * 12) = 15
        let (cols2, rows2) = TerminalHandler::apply_aspect_ratio(
            None,
            Some(&ImageDimension::Cells(10)),
            1,
            10,
            200,
            200,
            80,
            24,
            12,
            18,
        );
        assert_eq!(rows2, 10);
        assert_eq!(cols2, 15);
    }

    // ------------------------------------------------------------------
    // ImageSizeMode provenance (Task 100.17a)
    // ------------------------------------------------------------------

    #[test]
    fn iterm2_explicit_width_and_height_sets_explicit_cells_mode() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, _rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        let mut png_buf = Vec::new();
        {
            use image::ImageEncoder;
            let encoder = image::codecs::png::PngEncoder::new(&mut png_buf);
            let rgba_data: [u8; 16] = [
                255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255,
            ];
            encoder
                .write_image(&rgba_data, 2, 2, image::ExtendedColorType::Rgba8)
                .unwrap();
        }

        let image_data = ITerm2InlineImageData {
            name: None,
            size: Some(png_buf.len()),
            width: Some(ImageDimension::Cells(4)),
            height: Some(ImageDimension::Cells(2)),
            preserve_aspect_ratio: false,
            inline: true,
            do_not_move_cursor: false,
            data: png_buf,
        };

        handler.handle_iterm2_inline_image(&image_data);

        let store = handler.buffer().image_store();
        let (_, img) = store
            .iter()
            .next()
            .expect("image store should contain an image");
        assert_eq!(
            img.size_mode,
            ImageSizeMode::ExplicitCells,
            "explicit width/height should set ExplicitCells"
        );
    }

    #[test]
    fn iterm2_auto_width_and_height_sets_native_pixels_mode() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, _rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        let mut png_buf = Vec::new();
        {
            use image::ImageEncoder;
            let encoder = image::codecs::png::PngEncoder::new(&mut png_buf);
            let rgba_data: [u8; 16] = [
                255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255,
            ];
            encoder
                .write_image(&rgba_data, 2, 2, image::ExtendedColorType::Rgba8)
                .unwrap();
        }

        let image_data = ITerm2InlineImageData {
            name: None,
            size: Some(png_buf.len()),
            width: Some(ImageDimension::Auto),
            height: None,
            preserve_aspect_ratio: false,
            inline: true,
            do_not_move_cursor: false,
            data: png_buf,
        };

        handler.handle_iterm2_inline_image(&image_data);

        let store = handler.buffer().image_store();
        let (_, img) = store
            .iter()
            .next()
            .expect("image store should contain an image");
        assert_eq!(
            img.size_mode,
            ImageSizeMode::NativePixels,
            "auto/none width and height should default to NativePixels"
        );
    }

    // ------------------------------------------------------------------
    // handle_iterm2_inline_image integration test
    // ------------------------------------------------------------------

    #[test]
    fn handle_iterm2_inline_image_places_image_in_buffer() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, _rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // Create a minimal 2x2 red PNG image in memory.
        let mut png_buf = Vec::new();
        {
            use image::ImageEncoder;
            let encoder = image::codecs::png::PngEncoder::new(&mut png_buf);
            // 2x2 RGBA image: all red
            let rgba_data: [u8; 16] = [
                255, 0, 0, 255, // pixel (0,0)
                255, 0, 0, 255, // pixel (1,0)
                255, 0, 0, 255, // pixel (0,1)
                255, 0, 0, 255, // pixel (1,1)
            ];
            encoder
                .write_image(&rgba_data, 2, 2, image::ExtendedColorType::Rgba8)
                .unwrap();
        }

        let image_data = ITerm2InlineImageData {
            name: Some("red.png".to_string()),
            size: Some(png_buf.len()),
            width: Some(ImageDimension::Cells(4)),
            height: Some(ImageDimension::Cells(2)),
            preserve_aspect_ratio: false,
            inline: true,
            do_not_move_cursor: false,
            data: png_buf,
        };

        handler.handle_iterm2_inline_image(&image_data);

        // After placement, cursor should have moved down by display_rows (2).
        // The buffer should contain image cells.

        // Check that at least one image overlay has been placed.
        let has_image_cell = handler.buffer().has_any_image_cell();
        assert!(has_image_cell, "Expected at least one image cell in buffer");
    }

    #[test]
    fn handle_iterm2_inline_image_non_inline_ignored() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, _rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        let image_data = ITerm2InlineImageData {
            name: None,
            size: None,
            width: None,
            height: None,
            preserve_aspect_ratio: true,
            inline: false, // not inline → should be ignored
            do_not_move_cursor: false,
            data: vec![0xFF; 100],
        };

        // Cursor should not move.
        let cursor_before = handler.cursor_pos();
        handler.handle_iterm2_inline_image(&image_data);
        let cursor_after = handler.cursor_pos();
        assert_eq!(cursor_before, cursor_after);
    }

    #[test]
    fn handle_iterm2_inline_image_do_not_move_cursor() {
        use image::ImageEncoder;

        let mut handler = TerminalHandler::new(80, 24);
        let (tx, _rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // Create a minimal test PNG.
        let mut png_buf = Vec::new();
        {
            let encoder = image::codecs::png::PngEncoder::new(&mut png_buf);
            let rgba_data: [u8; 16] = [
                255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255,
            ];
            encoder
                .write_image(&rgba_data, 2, 2, image::ExtendedColorType::Rgba8)
                .unwrap();
        }

        // Position cursor at a known location.
        handler.buffer_mut().set_cursor_pos(Some(5), Some(3));
        let cursor_before = handler.cursor_pos();

        let image_data = ITerm2InlineImageData {
            name: None,
            size: Some(png_buf.len()),
            width: Some(ImageDimension::Cells(4)),
            height: Some(ImageDimension::Cells(2)),
            preserve_aspect_ratio: false,
            inline: true,
            do_not_move_cursor: true,
            data: png_buf,
        };

        handler.handle_iterm2_inline_image(&image_data);

        // Cursor should NOT have moved because doNotMoveCursor=1.
        let cursor_after = handler.cursor_pos();
        assert_eq!(
            cursor_before, cursor_after,
            "Cursor should be preserved when doNotMoveCursor=1"
        );

        // But the image should still have been placed.
        let has_image = handler.buffer().has_any_image_cell();
        assert!(has_image, "Image should still be placed");
    }

    /// Regression (Task 125.C7): at scrollback capacity the placement evicts
    /// rows, so `doNotMoveCursor` must restore by row number, not by index.
    #[test]
    fn handle_iterm2_inline_image_do_not_move_cursor_at_scrollback_capacity() {
        use image::ImageEncoder;

        let mut handler = TerminalHandler::new(80, 4).with_scrollback_limit(4);
        let (tx, _rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        let mut png_buf = Vec::new();
        {
            let encoder = image::codecs::png::PngEncoder::new(&mut png_buf);
            let rgba_data: [u8; 16] = [
                255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255,
            ];
            encoder
                .write_image(&rgba_data, 2, 2, image::ExtendedColorType::Rgba8)
                .unwrap();
        }

        for _ in 0..20 {
            handler.handle_data(b"pad");
            handler.handle_newline();
            handler.handle_carriage_return();
        }
        handler.handle_data(b"ab");
        let base_before = handler.buffer().row_base();
        let origin = handler.buffer().cursor_row_number();
        let x_before = handler.buffer().cursor().pos.x;

        handler.handle_iterm2_inline_image(&ITerm2InlineImageData {
            name: None,
            size: Some(png_buf.len()),
            width: Some(ImageDimension::Cells(4)),
            height: Some(ImageDimension::Cells(2)),
            preserve_aspect_ratio: false,
            inline: true,
            do_not_move_cursor: true,
            data: png_buf,
        });

        assert!(
            handler.buffer().row_base() > base_before,
            "setup: placement must evict rows"
        );
        assert_eq!(handler.buffer().cursor_row_number(), origin);
        let pos = handler.buffer().cursor().pos;
        assert_eq!(pos.x, x_before);
        let placement = handler.buffer().rows()[pos.y].cells()[pos.x]
            .image_placement()
            .expect("cursor sits on the image origin cell");
        assert_eq!((placement.col_in_image, placement.row_in_image), (0, 0));
    }

    // ------------------------------------------------------------------
    // iTerm2 multipart file transfer integration tests
    // ------------------------------------------------------------------

    /// Create a minimal 2x2 red PNG image as raw bytes.
    fn make_test_png() -> Vec<u8> {
        use image::ImageEncoder;
        let mut png_buf = Vec::new();
        let encoder = image::codecs::png::PngEncoder::new(&mut png_buf);
        // 2x2 RGBA image: all red
        let rgba_data: [u8; 16] = [
            255, 0, 0, 255, // pixel (0,0)
            255, 0, 0, 255, // pixel (1,0)
            255, 0, 0, 255, // pixel (0,1)
            255, 0, 0, 255, // pixel (1,1)
        ];
        encoder
            .write_image(&rgba_data, 2, 2, image::ExtendedColorType::Rgba8)
            .unwrap();
        png_buf
    }

    #[test]
    fn multipart_begin_part_end_places_image() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, _rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        let png_data = make_test_png();

        // Begin: metadata with no payload
        let begin_data = ITerm2InlineImageData {
            name: Some("red.png".to_string()),
            size: Some(png_data.len()),
            width: Some(ImageDimension::Cells(4)),
            height: Some(ImageDimension::Cells(2)),
            preserve_aspect_ratio: false,
            inline: true,
            do_not_move_cursor: false,
            data: Vec::new(),
        };
        handler.handle_osc(&AnsiOscType::ITerm2MultipartBegin(begin_data));

        // Verify no image placed yet
        let has_image_before = handler.buffer().has_any_image_cell();
        assert!(
            !has_image_before,
            "No image should be placed before FileEnd"
        );

        // Send data in two chunks
        let mid = png_data.len() / 2;
        handler.handle_osc(&AnsiOscType::ITerm2FilePart(png_data[..mid].to_vec()));
        handler.handle_osc(&AnsiOscType::ITerm2FilePart(png_data[mid..].to_vec()));

        // End: assemble and place
        handler.handle_osc(&AnsiOscType::ITerm2FileEnd);

        // Verify image was placed
        let has_image_after = handler.buffer().has_any_image_cell();
        assert!(has_image_after, "Image should be placed after FileEnd");

        // Verify multipart state was cleared
        assert!(
            handler.multipart_state.is_none(),
            "multipart_state should be None after FileEnd"
        );
    }

    /// Metadata for a multipart transfer, with an optional declared `size=`.
    fn multipart_begin(size: Option<usize>) -> AnsiOscType {
        AnsiOscType::ITerm2MultipartBegin(ITerm2InlineImageData {
            name: None,
            size,
            width: None,
            height: None,
            preserve_aspect_ratio: true,
            inline: true,
            do_not_move_cursor: false,
            data: Vec::new(),
        })
    }

    /// Run a whole multipart transfer of a real PNG and assert it was placed:
    /// proof the handler is usable after an abandonment.
    fn assert_multipart_transfer_works(handler: &mut TerminalHandler) {
        handler.handle_osc(&multipart_begin(None));
        handler.handle_osc(&AnsiOscType::ITerm2FilePart(make_test_png()));
        handler.handle_osc(&AnsiOscType::ITerm2FileEnd);
        assert!(handler.multipart_state.is_none());
        assert!(handler.buffer().has_any_image_cell());
    }

    #[test]
    fn multipart_over_total_cap_is_abandoned_silently() {
        use super::super::chunk_assembler::{BoundedChunkAssembler, ChunkLimits};

        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // Start through the real path, then swap the assembler for one with
        // small limits so the cap can be crossed cheaply.
        handler.handle_osc(&multipart_begin(None));
        let state = handler.multipart_state.as_mut().unwrap();
        let mut small = BoundedChunkAssembler::new(ChunkLimits {
            max_chunk_bytes: 8,
            max_total_bytes: 12,
        });
        small.push(&[0; 8], ChunkEncoding::Raw).unwrap();
        state.data = small;

        // 8 + 8 = 16 > 12: over the total cap.
        handler.handle_osc(&AnsiOscType::ITerm2FilePart(vec![0; 8]));
        assert!(handler.multipart_state.is_none(), "transfer abandoned");

        // The rest of the abandoned transfer is ignored.
        handler.handle_osc(&AnsiOscType::ITerm2FilePart(vec![0; 4]));
        handler.handle_osc(&AnsiOscType::ITerm2FileEnd);
        assert!(!handler.buffer().has_any_image_cell());
        assert!(rx.try_recv().is_err(), "no reply may be written");

        assert_multipart_transfer_works(&mut handler);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn multipart_chunk_over_cap_is_abandoned_silently() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        handler.handle_osc(&multipart_begin(None));
        handler.handle_osc(&AnsiOscType::ITerm2FilePart(vec![
            0;
            MAX_ITERM2_MULTIPART_BYTES
                + 1
        ]));

        assert!(handler.multipart_state.is_none(), "transfer abandoned");
        handler.handle_osc(&AnsiOscType::ITerm2FileEnd);
        assert!(!handler.buffer().has_any_image_cell());
        assert!(rx.try_recv().is_err(), "no reply may be written");

        assert_multipart_transfer_works(&mut handler);
    }

    #[test]
    fn multipart_huge_declared_size_does_not_preallocate() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, _rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        handler.handle_osc(&multipart_begin(Some(usize::MAX)));

        let state = handler.multipart_state.as_ref().unwrap();
        assert_eq!(state.data.len(), 0);
        assert_eq!(
            state.data.capacity(),
            0,
            "size= must not size an allocation"
        );
    }

    #[test]
    fn multipart_single_chunk_places_image() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, _rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        let png_data = make_test_png();

        let begin_data = ITerm2InlineImageData {
            name: None,
            size: None,
            width: None,
            height: None,
            preserve_aspect_ratio: true,
            inline: true,
            do_not_move_cursor: false,
            data: Vec::new(),
        };
        handler.handle_osc(&AnsiOscType::ITerm2MultipartBegin(begin_data));
        handler.handle_osc(&AnsiOscType::ITerm2FilePart(png_data));
        handler.handle_osc(&AnsiOscType::ITerm2FileEnd);

        let has_image = handler.buffer().has_any_image_cell();
        assert!(
            has_image,
            "Image should be placed after single-chunk transfer"
        );
    }

    #[test]
    fn multipart_file_part_without_begin_ignored() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, _rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // FilePart with no active transfer — should be silently ignored
        let cursor_before = handler.cursor_pos();
        handler.handle_osc(&AnsiOscType::ITerm2FilePart(vec![1, 2, 3]));
        let cursor_after = handler.cursor_pos();
        assert_eq!(cursor_before, cursor_after);

        // Verify no multipart state was created
        assert!(handler.multipart_state.is_none());
    }

    #[test]
    fn multipart_file_end_without_begin_ignored() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, _rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // FileEnd with no active transfer — should be silently ignored
        let cursor_before = handler.cursor_pos();
        handler.handle_osc(&AnsiOscType::ITerm2FileEnd);
        let cursor_after = handler.cursor_pos();
        assert_eq!(cursor_before, cursor_after);
    }

    #[test]
    fn multipart_begin_resets_previous_incomplete_transfer() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, _rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        let png_data = make_test_png();

        // Start first transfer (will be abandoned)
        let begin1 = ITerm2InlineImageData {
            name: Some("first.png".to_string()),
            size: None,
            width: None,
            height: None,
            preserve_aspect_ratio: true,
            inline: true,
            do_not_move_cursor: false,
            data: Vec::new(),
        };
        handler.handle_osc(&AnsiOscType::ITerm2MultipartBegin(begin1));
        handler.handle_osc(&AnsiOscType::ITerm2FilePart(vec![0xDE, 0xAD]));

        // Start second transfer — should discard the first
        let begin2 = ITerm2InlineImageData {
            name: Some("second.png".to_string()),
            size: Some(png_data.len()),
            width: Some(ImageDimension::Cells(4)),
            height: Some(ImageDimension::Cells(2)),
            preserve_aspect_ratio: false,
            inline: true,
            do_not_move_cursor: false,
            data: Vec::new(),
        };
        handler.handle_osc(&AnsiOscType::ITerm2MultipartBegin(begin2));

        // Verify state was replaced (accumulated data from first transfer is gone)
        let state = handler.multipart_state.as_ref().unwrap();
        assert_eq!(state.metadata.name, Some("second.png".to_string()));
        assert!(
            state.data.is_empty(),
            "accumulated data should be empty after new begin"
        );

        // Complete second transfer with real image data
        handler.handle_osc(&AnsiOscType::ITerm2FilePart(png_data));
        handler.handle_osc(&AnsiOscType::ITerm2FileEnd);

        let has_image = handler.buffer().has_any_image_cell();
        assert!(has_image, "Second transfer should produce an image");
    }

    #[test]
    fn multipart_empty_payload_ignored_on_file_end() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, _rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // Begin a transfer but send no FilePart chunks
        let begin_data = ITerm2InlineImageData {
            name: None,
            size: None,
            width: None,
            height: None,
            preserve_aspect_ratio: true,
            inline: true,
            do_not_move_cursor: false,
            data: Vec::new(),
        };
        handler.handle_osc(&AnsiOscType::ITerm2MultipartBegin(begin_data));
        handler.handle_osc(&AnsiOscType::ITerm2FileEnd);

        // No image should be placed (empty payload)
        let has_image = handler.buffer().has_any_image_cell();
        assert!(
            !has_image,
            "Empty multipart transfer should not place an image"
        );

        // State should be cleared
        assert!(handler.multipart_state.is_none());
    }

    #[test]
    fn multipart_non_inline_ignored() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, _rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        let png_data = make_test_png();

        // Begin with inline=false (download, not display)
        let begin_data = ITerm2InlineImageData {
            name: None,
            size: None,
            width: None,
            height: None,
            preserve_aspect_ratio: true,
            inline: false, // not inline
            do_not_move_cursor: false,
            data: Vec::new(),
        };
        handler.handle_osc(&AnsiOscType::ITerm2MultipartBegin(begin_data));
        handler.handle_osc(&AnsiOscType::ITerm2FilePart(png_data));

        let cursor_before = handler.cursor_pos();
        handler.handle_osc(&AnsiOscType::ITerm2FileEnd);
        let cursor_after = handler.cursor_pos();

        // inline=false means the image is a download, not an inline display.
        // handle_iterm2_inline_image returns early for non-inline, so no image placed.
        assert_eq!(
            cursor_before, cursor_after,
            "Cursor should not move for non-inline"
        );

        let has_image = handler.buffer().has_any_image_cell();
        assert!(
            !has_image,
            "Non-inline multipart transfer should not place an image"
        );
    }
}
