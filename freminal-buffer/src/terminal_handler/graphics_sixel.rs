// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Sixel graphics protocol handler for [`TerminalHandler`].
//!
//! This module implements DCS Sixel sequence detection and decoding:
//!
//! - [`TerminalHandler::is_sixel_sequence`] — detect whether a DCS payload is a
//!   Sixel stream (format: `<digits;semicolons>q<data>`)
//! - [`TerminalHandler::handle_sixel`] — decode and place a Sixel image in the
//!   terminal buffer, respecting DECSDM display vs. scrolling mode

use conv2::ValueFrom;
use freminal_common::buffer_states::{
    modes::decsdm::Decsdm, modes::private_color_registers::PrivateColorRegisters,
};

use crate::image_store::{ImageProtocol, InlineImage, next_image_id};

use super::TerminalHandler;

impl TerminalHandler {
    /// Check whether a DCS inner payload is a Sixel sequence.
    ///
    /// The format is `<optional digits and semicolons>q<sixel data>`.
    /// This returns `true` when the data up to the first `q` consists only
    /// of digits and semicolons (the P1;P2;P3 parameters).
    pub(super) fn is_sixel_sequence(inner: &[u8]) -> bool {
        let Some(q_pos) = inner.iter().position(|&b| b == b'q') else {
            return false;
        };
        // Everything before `q` must be digits or semicolons (DCS params).
        inner[..q_pos]
            .iter()
            .all(|&b| b.is_ascii_digit() || b == b';')
    }

    /// Handle a Sixel graphics DCS sequence.
    ///
    /// `inner` is the stripped DCS payload: `<P1;P2;P3>q<sixel-data>`.
    pub(super) fn handle_sixel(&mut self, inner: &[u8]) {
        use freminal_common::buffer_states::sixel::{
            default_sixel_palette, parse_sixel, parse_sixel_with_shared_palette,
        };

        let sixel_image = if self.private_color_registers == PrivateColorRegisters::Private {
            // Private mode (default): each image gets the default palette.
            let Some(img) = parse_sixel(inner) else {
                tracing::warn!("Sixel: failed to decode image from DCS payload");
                return;
            };
            img
        } else {
            // Shared mode: use and update the persistent palette.
            let palette = self
                .sixel_shared_palette
                .as_deref()
                .copied()
                .unwrap_or(default_sixel_palette());
            let (maybe_img, updated_palette) = parse_sixel_with_shared_palette(inner, palette);
            self.sixel_shared_palette = Some(Box::new(updated_palette));
            let Some(img) = maybe_img else {
                tracing::warn!("Sixel (shared palette): failed to decode image from DCS payload");
                return;
            };
            img
        };

        if sixel_image.width == 0 || sixel_image.height == 0 {
            tracing::warn!("Sixel: decoded image has zero dimensions");
            return;
        }

        let (term_width, term_height) = self.get_win_size();

        // Compute display size in terminal cells using actual cell pixel dimensions.
        let display_cols = {
            let cols =
                usize::value_from(sixel_image.width.div_ceil(self.cell_pixel_width)).unwrap_or(0);
            cols.min(term_width).max(1)
        };
        let display_rows = {
            let rows =
                usize::value_from(sixel_image.height.div_ceil(self.cell_pixel_height)).unwrap_or(0);
            rows.min(term_height).max(1)
        };

        let image_id = next_image_id();

        let inline_image = InlineImage {
            id: image_id,
            pixels: std::sync::Arc::new(sixel_image.pixels),
            width_px: sixel_image.width,
            height_px: sixel_image.height,
            display_cols,
            display_rows,
        };

        // In DECSDM display mode (?80 h), the cursor does not advance past
        // the image.  Save the cursor position so we can restore it after
        // place_image (which always moves the cursor below the image).
        let saved_cursor = if self.sixel_display_mode == Decsdm::DisplayMode {
            Some(self.buffer.get_cursor().pos)
        } else {
            None
        };

        let _new_offset =
            self.buffer
                .place_image(inline_image, 0, ImageProtocol::Sixel, None, None, 0);

        // Restore cursor position for DECSDM display mode.
        if let Some(pos) = saved_cursor {
            self.buffer.set_cursor_pos_raw(pos);
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use freminal_common::pty_write::PtyWrite;

    use super::TerminalHandler;

    // -----------------------------------------------------------------------
    // Sixel integration tests
    // -----------------------------------------------------------------------

    /// Build a DCS Sixel payload with `P` prefix and `ESC \` suffix.
    /// `params` is the "P1;P2;P3" part (may be empty), `sixel_body` is
    /// everything after the `q` introducer.
    fn build_sixel_dcs(params: &[u8], sixel_body: &[u8]) -> Vec<u8> {
        // Format: P <params> q <sixel_body> ESC '\'
        let mut v = vec![b'P'];
        v.extend_from_slice(params);
        v.push(b'q');
        v.extend_from_slice(sixel_body);
        v.extend_from_slice(b"\x1b\\");
        v
    }

    #[test]
    fn sixel_simple_red_pixel_places_image() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, _rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // Define color 1 as red (RGB 100,0,0) and paint one sixel column.
        // '#1;2;100;0;0' defines color 1, '#1' selects it, '~' = 0x7E = 0b111111
        // encodes 6 pixels all set (1 column x 6 rows of red).
        let sixel_body = b"#1;2;100;0;0#1~";
        let dcs = build_sixel_dcs(b"0;0;0", sixel_body);
        handler.handle_device_control_string(&dcs);

        let has_image = handler.buffer().has_any_image_cell();
        assert!(
            has_image,
            "Sixel DCS should place image cells in the buffer"
        );
    }

    #[test]
    fn sixel_image_stored_in_image_store() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, _rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        let initial_len = handler.buffer().image_store().len();

        let sixel_body = b"#1;2;100;0;0#1~";
        let dcs = build_sixel_dcs(b"0;0;0", sixel_body);
        handler.handle_device_control_string(&dcs);

        assert_eq!(
            handler.buffer().image_store().len(),
            initial_len + 1,
            "Image store should contain one more image after Sixel"
        );
    }

    #[test]
    fn sixel_empty_body_no_image() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, _rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // Just the `q` introducer with no sixel data at all → zero-dimension image.
        let dcs = build_sixel_dcs(b"", b"");
        handler.handle_device_control_string(&dcs);

        let has_image = handler.buffer().has_any_image_cell();
        assert!(
            !has_image,
            "Empty Sixel body should not produce image cells"
        );
    }

    #[test]
    fn sixel_with_repeat_expands_width() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, _rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // Color 0 = red, then repeat '~' 16 times → 16 columns, 6 rows.
        // At 8 px/col that is 2 cells wide; at 16 px/row that is 1 cell tall.
        let sixel_body = b"#0;2;100;0;0#0!16~";
        let dcs = build_sixel_dcs(b"", sixel_body);
        handler.handle_device_control_string(&dcs);

        let has_image = handler.buffer().has_any_image_cell();
        assert!(
            has_image,
            "Sixel with repeat operator should place image cells"
        );

        // The image should be 16 px wide, 6 px tall.
        // Find it via the image store iterator (there should be exactly one).
        let store = handler.buffer().image_store();
        let (_, img) = store
            .iter()
            .next()
            .expect("image store should contain an image");
        assert_eq!(img.width_px, 16, "Image width should be 16 pixels");
        assert_eq!(img.height_px, 6, "Image height should be 6 pixels");
    }

    #[test]
    fn sixel_multicolor_two_bands() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, _rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // Define red and blue. Paint one column of red, newline, one column of blue.
        // Total: 1 px wide, 12 px tall (2 bands of 6).
        let sixel_body = b"#1;2;100;0;0#2;2;0;0;100#1~-#2~";
        let dcs = build_sixel_dcs(b"", sixel_body);
        handler.handle_device_control_string(&dcs);

        let store = handler.buffer().image_store();
        let (_, img) = store
            .iter()
            .next()
            .expect("image store should contain an image");
        assert_eq!(img.width_px, 1, "Image width should be 1 pixel");
        assert_eq!(
            img.height_px, 12,
            "Image height should be 12 pixels (2 bands)"
        );
    }

    #[test]
    fn sixel_with_raster_attributes() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, _rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // Raster attributes: "1;1;8;12" = aspect 1:1, declared 8 wide x 12 tall.
        // Then paint 2 bands of 8 columns each.
        let sixel_body = b"\"1;1;8;12#1;2;0;100;0#1!8~-#1!8~";
        let dcs = build_sixel_dcs(b"", sixel_body);
        handler.handle_device_control_string(&dcs);

        let store = handler.buffer().image_store();
        let (_, img) = store
            .iter()
            .next()
            .expect("image store should contain an image");
        assert_eq!(img.width_px, 8, "Raster-declared width should be 8 pixels");
        assert_eq!(
            img.height_px, 12,
            "Raster-declared height should be 12 pixels"
        );
    }

    #[test]
    fn sixel_transparent_background() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, _rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // P2=1 means transparent background.
        // Paint only the top pixel of a single column (bit 0 set = '?' + 1 = '@').
        // The remaining 5 pixels in the column should be transparent (alpha=0).
        let sixel_body = b"#1;2;100;0;0#1@";
        let dcs = build_sixel_dcs(b"0;1;0", sixel_body);
        handler.handle_device_control_string(&dcs);

        let store = handler.buffer().image_store();
        let (_, img) = store
            .iter()
            .next()
            .expect("image store should contain an image");

        // 1 pixel wide, 6 pixels tall → 6 * 4 = 24 bytes RGBA.
        assert_eq!(img.pixels.len(), 24);

        // First pixel (row 0): red, fully opaque — '@' = 0x40 - 0x3F = 1 = bit 0 set.
        assert_eq!(img.pixels[0], 255, "R channel of top pixel");
        assert_eq!(img.pixels[1], 0, "G channel of top pixel");
        assert_eq!(img.pixels[2], 0, "B channel of top pixel");
        assert_eq!(img.pixels[3], 255, "A channel of top pixel (opaque)");

        // Second pixel (row 1): transparent — bit 1 not set.
        assert_eq!(img.pixels[7], 0, "A channel of second pixel (transparent)");
    }

    #[test]
    fn sixel_carriage_return_overlays_same_band() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, _rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // Paint red in column 0, then CR ('$'), then paint blue in column 0.
        // Blue should overwrite red in the overlapping pixels.
        // '~' = all 6 bits set.
        let sixel_body = b"#1;2;100;0;0#1~$#2;2;0;0;100#2~";
        let dcs = build_sixel_dcs(b"0;1;0", sixel_body);
        handler.handle_device_control_string(&dcs);

        let store = handler.buffer().image_store();
        let (_, img) = store
            .iter()
            .next()
            .expect("image store should contain an image");

        // Column 0, all 6 pixels should be blue (last writer wins).
        // First pixel: RGBA at offset 0..4.
        assert_eq!(img.pixels[0], 0, "R of overwritten pixel");
        assert_eq!(img.pixels[1], 0, "G of overwritten pixel");
        assert_eq!(img.pixels[2], 255, "B of overwritten pixel");
        assert_eq!(
            img.pixels[3], 255,
            "A of overwritten pixel (opaque, set by blue)"
        );
    }

    #[test]
    fn is_sixel_sequence_detection() {
        // Valid sixel sequences.
        assert!(
            TerminalHandler::is_sixel_sequence(b"q~"),
            "bare 'q' + data is sixel"
        );
        assert!(
            TerminalHandler::is_sixel_sequence(b"0;0;0q~"),
            "params before q is sixel"
        );
        assert!(
            TerminalHandler::is_sixel_sequence(b"0q"),
            "single param before q is sixel"
        );

        // Not sixel.
        assert!(
            !TerminalHandler::is_sixel_sequence(b"$qm"),
            "DECRQSS ($q) is not sixel"
        );
        assert!(
            !TerminalHandler::is_sixel_sequence(b"+qHEX"),
            "XTGETTCAP (+q) is not sixel"
        );
        assert!(
            !TerminalHandler::is_sixel_sequence(b"no_q_here"),
            "no q at all is not sixel"
        );
        assert!(
            !TerminalHandler::is_sixel_sequence(b"abcq~"),
            "letters before q is not sixel"
        );
    }

    // -----------------------------------------------------------------------
    // DECSDM (?80) — Sixel Display Mode behavior tests
    // -----------------------------------------------------------------------

    #[test]
    fn sixel_scrolling_mode_cursor_advances() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, _rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // Move cursor to a non-zero column so we can detect the x→0 reset.
        handler.handle_data(b"Hello");
        let cursor_before = handler.buffer.get_cursor().pos;
        assert_eq!(
            cursor_before.x, 5,
            "cursor should be at col 5 after 'Hello'"
        );

        // Build a multi-band sixel (3 bands = 18px > 16px cell height = 2 cell rows).
        // Each `-` advances to the next sixel band (6 pixels down).
        // Default cell_pixel_height=16, so 18px → display_rows=2.
        let sixel_body = b"#1;2;100;0;0#1~-#1~-#1~";
        let dcs = build_sixel_dcs(b"0;0;0", sixel_body);
        handler.handle_device_control_string(&dcs);

        let cursor_after = handler.buffer.get_cursor().pos;
        // In scrolling mode, cursor moves below the image and x resets to 0.
        assert!(
            cursor_after.y > cursor_before.y,
            "Scrolling mode: cursor row should advance past sixel image, \
             before=({},{}) after=({},{})",
            cursor_before.x,
            cursor_before.y,
            cursor_after.x,
            cursor_after.y
        );
        assert_eq!(
            cursor_after.x, 0,
            "Scrolling mode: cursor column should reset to 0 after sixel"
        );
    }

    #[test]
    fn sixel_display_mode_cursor_does_not_advance() {
        use freminal_common::buffer_states::{
            mode::{Mode, SetMode},
            modes::decsdm::Decsdm,
            terminal_output::TerminalOutput,
        };

        let mut handler = TerminalHandler::new(80, 24);
        let (tx, _rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // Enable DECSDM display mode.
        handler.process_outputs(&[TerminalOutput::Mode(Mode::Decsdm(Decsdm::new(
            &SetMode::DecSet,
        )))]);

        // Move cursor to a non-zero position.
        handler.handle_data(b"Hello");
        let cursor_before = handler.buffer.get_cursor().pos;
        assert_eq!(
            cursor_before.x, 5,
            "cursor should be at col 5 after 'Hello'"
        );

        // Build a multi-band sixel (3 bands = 18px > 16px cell height = 2 cell rows).
        let sixel_body = b"#1;2;100;0;0#1~-#1~-#1~";
        let dcs = build_sixel_dcs(b"0;0;0", sixel_body);
        handler.handle_device_control_string(&dcs);

        let cursor_after = handler.buffer.get_cursor().pos;
        // In display mode, cursor should be restored to its pre-image position.
        assert_eq!(
            cursor_before, cursor_after,
            "Display mode: cursor should NOT advance past sixel image, \
             before=({},{}) after=({},{})",
            cursor_before.x, cursor_before.y, cursor_after.x, cursor_after.y
        );
    }

    // -----------------------------------------------------------------------
    // Shared palette mode (?1070 reset) — exercises the shared-palette code path
    // -----------------------------------------------------------------------

    #[test]
    fn sixel_shared_palette_mode_places_image() {
        use freminal_common::buffer_states::{
            mode::{Mode, SetMode},
            modes::private_color_registers::PrivateColorRegisters,
            terminal_output::TerminalOutput,
        };

        let mut handler = TerminalHandler::new(80, 24);
        let (tx, _rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // Switch to shared color register mode (CSI ? 1070 l → DecRst)
        handler.process_outputs(&[TerminalOutput::Mode(Mode::PrivateColorRegisters(
            PrivateColorRegisters::new(&SetMode::DecRst),
        ))]);

        let initial_len = handler.buffer().image_store().len();

        // Paint a simple red image in shared palette mode
        let sixel_body = b"#1;2;100;0;0#1~";
        let dcs = build_sixel_dcs(b"0;0;0", sixel_body);
        handler.handle_device_control_string(&dcs);

        assert_eq!(
            handler.buffer().image_store().len(),
            initial_len + 1,
            "Sixel (shared palette) should place an image in the image store"
        );
        assert!(
            handler.buffer().has_any_image_cell(),
            "Sixel (shared palette) should place image cells"
        );
    }

    #[test]
    fn sixel_shared_palette_persists_across_images() {
        use freminal_common::buffer_states::{
            mode::{Mode, SetMode},
            modes::private_color_registers::PrivateColorRegisters,
            terminal_output::TerminalOutput,
        };

        let mut handler = TerminalHandler::new(80, 24);
        let (tx, _rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // Enable shared palette mode
        handler.process_outputs(&[TerminalOutput::Mode(Mode::PrivateColorRegisters(
            PrivateColorRegisters::new(&SetMode::DecRst),
        ))]);

        let initial_len = handler.buffer().image_store().len();

        // First image: defines color 5 as green and paints it
        let sixel_body1 = b"#5;2;0;100;0#5~";
        let dcs1 = build_sixel_dcs(b"", sixel_body1);
        handler.handle_device_control_string(&dcs1);

        // Second image: uses color 5 without redefining it (relies on shared palette)
        let sixel_body2 = b"#5~";
        let dcs2 = build_sixel_dcs(b"", sixel_body2);
        handler.handle_device_control_string(&dcs2);

        // Both images should have been stored
        assert_eq!(
            handler.buffer().image_store().len(),
            initial_len + 2,
            "Two images should be stored in shared palette mode"
        );
    }

    #[test]
    fn sixel_shared_palette_empty_body_no_image() {
        use freminal_common::buffer_states::{
            mode::{Mode, SetMode},
            modes::private_color_registers::PrivateColorRegisters,
            terminal_output::TerminalOutput,
        };

        let mut handler = TerminalHandler::new(80, 24);
        let (tx, _rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // Enable shared palette mode
        handler.process_outputs(&[TerminalOutput::Mode(Mode::PrivateColorRegisters(
            PrivateColorRegisters::new(&SetMode::DecRst),
        ))]);

        // Empty sixel body → zero-dimension image → no cells placed
        let dcs = build_sixel_dcs(b"", b"");
        handler.handle_device_control_string(&dcs);

        assert!(
            !handler.buffer().has_any_image_cell(),
            "Empty Sixel body (shared palette) should not produce image cells"
        );
    }
}
