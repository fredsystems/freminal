// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Kitty graphics protocol handler for [`TerminalHandler`].
//!
//! This module implements all Kitty graphics APC (`_G`) command processing:
//!
//! - Dispatch (`handle_kitty_graphics`) — routes commands by action type
//! - Query (`handle_kitty_query`) — responds to `a=q` capability queries
//! - Chunked transfer (`handle_kitty_chunk_start`, `handle_kitty_chunk`) — assembles
//!   multi-chunk payloads before dispatch
//! - Single-command path (`handle_kitty_single`) — decodes payload and places image
//! - Put (`handle_kitty_put`) — places a previously stored image by ID
//! - Payload decoding (`decode_kitty_payload`, `resolve_kitty_transmission`,
//!   `read_kitty_file`, `require_kitty_dimensions`) — resolves transmission medium
//!   and decodes pixel formats (RGB, RGBA, PNG)
//! - Image placement (`place_kitty_image`) — stores `InlineImage` and places cells
//! - Error response (`send_kitty_error`) — sends error APC to the PTY
//! - Delete (`handle_kitty_delete`) — removes images by various targets

use conv2::ValueFrom;
use freminal_common::buffer_states::kitty_graphics::{
    KittyAction, KittyControlData, KittyGraphicsCommand, format_kitty_response,
};

use crate::image_store::{ImageProtocol, InlineImage, next_image_id};

use super::KittyImageState;
use super::TerminalHandler;

impl TerminalHandler {
    /// Dispatch a parsed Kitty graphics command.
    pub(super) fn handle_kitty_graphics(&mut self, cmd: KittyGraphicsCommand) {
        let action = cmd.control.action.unwrap_or(KittyAction::Transmit);

        // If this is a continuation chunk for an in-progress chunked transfer,
        // append the payload.  Per the Kitty protocol spec, continuation chunks
        // carry ONLY `m` and optionally `q` — no explicit `a=` key.  If the
        // incoming command explicitly sets `a=`, it is a new command and any
        // stale chunked state must be discarded.
        if self.kitty_state.is_some() {
            if cmd.control.action.is_none() {
                // No explicit action → this is a continuation chunk.
                self.handle_kitty_chunk(&cmd);
                return;
            }
            // Explicit action on a new command while a chunked transfer is in
            // progress — discard the stale accumulator.
            tracing::warn!(
                "Kitty graphics: discarding incomplete chunked transfer \
                 (id={:?}) due to new command (a={action:?})",
                self.kitty_state.as_ref().map(|s| s.control.image_id),
            );
            self.kitty_state = None;
        }

        tracing::debug!(
            "Kitty graphics: dispatch a={action:?} i={:?} m={} q={}",
            cmd.control.image_id,
            cmd.control.more_data,
            cmd.control.quiet,
        );

        match action {
            KittyAction::Query => self.handle_kitty_query(&cmd),
            KittyAction::Transmit | KittyAction::TransmitAndDisplay | KittyAction::Put => {
                if cmd.control.more_data {
                    // First chunk of a chunked transfer — start accumulating.
                    self.handle_kitty_chunk_start(cmd);
                } else {
                    // Single-chunk command (or final chunk with no prior state).
                    self.handle_kitty_single(&cmd, action);
                }
            }
            KittyAction::Delete => {
                self.handle_kitty_delete(&cmd);
            }
            KittyAction::AnimationFrame
            | KittyAction::AnimationControl
            | KittyAction::AnimationCompose => {
                tracing::warn!(
                    "Kitty graphics: animation commands not yet supported (a={action:?})"
                );
            }
        }
    }

    /// Handle `a=q` — query whether the terminal supports the Kitty graphics protocol.
    ///
    /// Responds with OK for formats 24 (RGB), 32 (RGBA), and 100 (PNG).
    /// Other formats get an error response.
    fn handle_kitty_query(&self, cmd: &KittyGraphicsCommand) {
        let image_id = cmd.control.image_id.unwrap_or(0);
        let quiet = cmd.control.quiet;

        // We support RGB (f=24), RGBA (f=32), and PNG (f=100).
        let supported = cmd.control.format.is_none_or(|f| {
            matches!(
                f,
                freminal_common::buffer_states::kitty_graphics::KittyFormat::Rgb
                    | freminal_common::buffer_states::kitty_graphics::KittyFormat::Rgba
                    | freminal_common::buffer_states::kitty_graphics::KittyFormat::Png
            )
        });

        // quiet=1 suppresses OK responses; quiet=2 suppresses all responses.
        if quiet >= 2 || (quiet >= 1 && supported) {
            return;
        }

        let response = if supported {
            format_kitty_response(image_id, true, "")
        } else {
            format_kitty_response(image_id, false, "ENOTSUP:unsupported format")
        };

        self.write_to_pty(&response);
    }

    /// Start a chunked Kitty graphics transfer (first chunk, `m=1`).
    fn handle_kitty_chunk_start(&mut self, cmd: KittyGraphicsCommand) {
        if self.kitty_state.is_some() {
            tracing::warn!(
                "Kitty graphics: new chunked transfer started while previous was in progress; \
                 discarding incomplete transfer"
            );
        }

        let capacity = cmd.control.data_size.unwrap_or(0) as usize;
        let mut accumulated_data = Vec::with_capacity(capacity);
        accumulated_data.extend_from_slice(&cmd.payload);

        self.kitty_state = Some(KittyImageState {
            control: cmd.control,
            accumulated_data,
        });

        tracing::debug!(
            "Kitty graphics: started chunked transfer (id={:?})",
            self.kitty_state.as_ref().map(|s| s.control.image_id),
        );
    }

    /// Append a chunk to the in-progress Kitty graphics transfer.
    ///
    /// If `m=0` (final chunk), finalise the transfer and dispatch.
    fn handle_kitty_chunk(&mut self, cmd: &KittyGraphicsCommand) {
        let Some(state) = &mut self.kitty_state else {
            tracing::warn!("Kitty graphics: chunk received with no active transfer; ignoring");
            return;
        };

        state.accumulated_data.extend_from_slice(&cmd.payload);

        if cmd.control.more_data {
            // More chunks to come.
            tracing::debug!(
                "Kitty graphics: appended chunk ({} bytes total so far)",
                state.accumulated_data.len(),
            );
        } else {
            // Final chunk — take ownership and dispatch.
            let final_state = self.kitty_state.take().unwrap_or_else(|| {
                // This branch is unreachable because we checked `is_some` above,
                // but we need to avoid `unwrap()` in production code.
                KittyImageState {
                    control: KittyControlData::default(),
                    accumulated_data: Vec::new(),
                }
            });

            tracing::debug!(
                "Kitty graphics: chunked transfer complete ({} bytes)",
                final_state.accumulated_data.len(),
            );

            let action = final_state.control.action.unwrap_or(KittyAction::Transmit);
            let final_cmd = KittyGraphicsCommand {
                control: final_state.control,
                payload: final_state.accumulated_data,
            };
            self.handle_kitty_single(&final_cmd, action);
        }
    }

    /// Handle a single (non-chunked) Kitty graphics command.
    ///
    /// Decodes the image payload according to the format (`f=24` RGB, `f=32`
    /// RGBA, `f=100` PNG), computes display dimensions, stores as an
    /// `InlineImage`, and places into the buffer.
    fn handle_kitty_single(&mut self, cmd: &KittyGraphicsCommand, action: KittyAction) {
        let image_id_hint = cmd.control.image_id.unwrap_or(0);
        let quiet = cmd.control.quiet;

        // `a=p` (Put) with an empty payload means "display a previously
        // transmitted image by its ID."  Look up the image from the store
        // instead of trying to decode an empty payload.
        if action == KittyAction::Put && cmd.payload.is_empty() {
            self.handle_kitty_put(cmd, image_id_hint, quiet);
            return;
        }

        if cmd.payload.is_empty() {
            tracing::warn!("Kitty graphics: empty payload for a={action:?}; ignoring");
            self.send_kitty_error(image_id_hint, quiet, "ENODATA:no payload");
            return;
        }

        // Decode payload into RGBA pixels + dimensions.
        let Some((rgba_pixels, img_width_px, img_height_px)) =
            self.decode_kitty_payload(cmd, image_id_hint, quiet)
        else {
            return; // Error already sent by decode_kitty_payload.
        };

        // Compute display size in cells and place the image.
        self.place_kitty_image(
            &cmd.control,
            action,
            rgba_pixels,
            img_width_px,
            img_height_px,
            image_id_hint,
            quiet,
        );
    }

    /// Handle `a=p` (Put) — display a previously transmitted image.
    ///
    /// Looks up the image by `i=<image_id>` in the image store, applies any
    /// display-size overrides (`c=`/`r=`) from the control data, and places
    /// the image into the buffer.
    fn handle_kitty_put(&mut self, cmd: &KittyGraphicsCommand, image_id: u32, quiet: u8) {
        let id = u64::from(image_id);

        // Look up the previously transmitted image.
        let Some(stored_image) = self.buffer.image_store().get(id).cloned() else {
            tracing::warn!(
                "Kitty graphics: a=p for unknown image id={image_id} \
                 (store has {} images: {:?})",
                self.buffer.image_store().len(),
                self.buffer
                    .image_store()
                    .iter()
                    .map(|(k, _)| k)
                    .collect::<Vec<_>>(),
            );
            self.send_kitty_error(image_id, quiet, "ENOENT:image not found");
            return;
        };

        // Apply display-size overrides from the Put command, if present.
        let (term_width, term_height) = self.get_win_size();

        let display_cols = cmd
            .control
            .display_cols
            .map_or(stored_image.display_cols, |c| {
                let cols = usize::value_from(c).unwrap_or(0);
                cols.min(term_width).max(1)
            });

        let display_rows = cmd
            .control
            .display_rows
            .map_or(stored_image.display_rows, |r| {
                let rows = usize::value_from(r).unwrap_or(0);
                rows.min(term_height).max(1)
            });

        let image_to_place = InlineImage {
            id: stored_image.id,
            pixels: stored_image.pixels,
            width_px: stored_image.width_px,
            height_px: stored_image.height_px,
            display_cols,
            display_rows,
        };

        // Update the store with the possibly-resized image.
        self.buffer.image_store_mut().insert(image_to_place.clone());

        if cmd.control.unicode_placeholder {
            let pid = cmd.control.placement_id.unwrap_or(0);
            let vp = super::VirtualPlacement {
                image_id: id,
                placement_id: pid,
                cols: u32::try_from(display_cols).unwrap_or(u32::MAX),
                rows: u32::try_from(display_rows).unwrap_or(u32::MAX),
            };
            self.virtual_placements.insert((id, pid), vp);
        } else {
            // Save cursor position if `C=1` (no cursor movement).
            let saved_cursor = if cmd.control.no_cursor_movement {
                Some(self.buffer.get_cursor().pos)
            } else {
                None
            };

            let cursor = self.buffer.get_cursor().pos;
            tracing::debug!(
                "Kitty graphics: a=p placing image id={id} at cursor ({},{}) \
                 {display_cols}x{display_rows} cells, C={}",
                cursor.x,
                cursor.y,
                u8::from(cmd.control.no_cursor_movement),
            );
            let _new_offset = self.buffer.place_image(
                image_to_place,
                0,
                ImageProtocol::Kitty,
                cmd.control.image_number,
                cmd.control.placement_id,
                cmd.control.z_index.unwrap_or(0),
            );

            // Restore cursor if `C=1`.
            if let Some(pos) = saved_cursor {
                self.buffer.set_cursor_pos(Some(pos.x), Some(pos.y));
            }
        }

        // Send OK response unless suppressed.
        if quiet < 1 && id > 0 {
            let response_id = u32::value_from(id).unwrap_or(0);
            let response = format_kitty_response(response_id, true, "");
            self.write_to_pty(&response);
        }
    }

    /// Decode a Kitty graphics payload into RGBA pixel data.
    ///
    /// Handles the transmission medium (`t=d` direct, `t=f` file path,
    /// `t=t` temp file, `t=s` shared memory) to resolve the raw image bytes,
    /// then decodes according to the pixel format (`f=24`, `f=32`, `f=100`).
    ///
    /// Returns `None` if decoding fails (an error response is sent to the PTY).
    fn decode_kitty_payload(
        &self,
        cmd: &KittyGraphicsCommand,
        image_id_hint: u32,
        quiet: u8,
    ) -> Option<(Vec<u8>, u32, u32)> {
        use freminal_common::buffer_states::kitty_graphics::KittyFormat;

        // Resolve the raw image bytes based on transmission medium.
        let image_data = self.resolve_kitty_transmission(
            &cmd.payload,
            cmd.control.transmission,
            image_id_hint,
            quiet,
        )?;

        let format = cmd.control.format.unwrap_or(KittyFormat::Rgba);

        match format {
            KittyFormat::Rgba => {
                let (w, h) = self.require_kitty_dimensions(cmd, image_id_hint, quiet)?;
                let expected = (w as usize) * (h as usize) * 4;
                if image_data.len() != expected {
                    tracing::warn!(
                        "Kitty RGBA: expected {expected} bytes, got {}",
                        image_data.len()
                    );
                    self.send_kitty_error(image_id_hint, quiet, "EINVAL:payload size mismatch");
                    return None;
                }
                Some((image_data, w, h))
            }
            KittyFormat::Rgb => {
                let (w, h) = self.require_kitty_dimensions(cmd, image_id_hint, quiet)?;
                let expected = (w as usize) * (h as usize) * 3;
                if image_data.len() != expected {
                    tracing::warn!(
                        "Kitty RGB: expected {expected} bytes, got {}",
                        image_data.len()
                    );
                    self.send_kitty_error(image_id_hint, quiet, "EINVAL:payload size mismatch");
                    return None;
                }
                let pixel_count = (w as usize) * (h as usize);
                let mut rgba = Vec::with_capacity(pixel_count * 4);
                for chunk in image_data.chunks_exact(3) {
                    rgba.extend_from_slice(chunk);
                    rgba.push(255);
                }
                Some((rgba, w, h))
            }
            KittyFormat::Png => match image::load_from_memory(&image_data) {
                Ok(img) => {
                    let rgba_img = img.to_rgba8();
                    let w = rgba_img.width();
                    let h = rgba_img.height();
                    if w == 0 || h == 0 {
                        self.send_kitty_error(
                            image_id_hint,
                            quiet,
                            "EINVAL:decoded image has zero dimensions",
                        );
                        return None;
                    }
                    Some((rgba_img.into_raw(), w, h))
                }
                Err(e) => {
                    tracing::warn!("Kitty PNG decode failed: {e}");
                    self.send_kitty_error(image_id_hint, quiet, "EINVAL:PNG decode failed");
                    None
                }
            },
        }
    }

    /// Resolve Kitty transmission medium to raw image bytes.
    ///
    /// - `Direct` (default): payload is already the image data.
    /// - `File`: payload is a UTF-8 file path; read the file from disk.
    /// - `TempFile`: same as `File`, but delete the file after reading.
    /// - `SharedMemory`: not supported.
    fn resolve_kitty_transmission(
        &self,
        payload: &[u8],
        transmission: Option<freminal_common::buffer_states::kitty_graphics::KittyTransmission>,
        image_id_hint: u32,
        quiet: u8,
    ) -> Option<Vec<u8>> {
        use freminal_common::buffer_states::kitty_graphics::KittyTransmission;

        match transmission.unwrap_or(KittyTransmission::Direct) {
            KittyTransmission::Direct => Some(payload.to_vec()),
            KittyTransmission::File => self.read_kitty_file(payload, image_id_hint, quiet, false),
            KittyTransmission::TempFile => {
                self.read_kitty_file(payload, image_id_hint, quiet, true)
            }
            KittyTransmission::SharedMemory => {
                tracing::warn!("Kitty graphics: shared memory transmission (t=s) is not supported");
                self.send_kitty_error(
                    image_id_hint,
                    quiet,
                    "ENOTSUP:shared memory transmission not supported",
                );
                None
            }
        }
    }

    /// Read a Kitty graphics file from disk.
    ///
    /// The payload bytes are interpreted as a UTF-8 file path. If `delete_after`
    /// is true, the file is removed after reading (for `t=t` temp file mode).
    fn read_kitty_file(
        &self,
        payload: &[u8],
        image_id_hint: u32,
        quiet: u8,
        delete_after: bool,
    ) -> Option<Vec<u8>> {
        let path_str = match std::str::from_utf8(payload) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("Kitty graphics file path is not valid UTF-8: {e}");
                self.send_kitty_error(image_id_hint, quiet, "EINVAL:invalid file path encoding");
                return None;
            }
        };

        let path = std::path::Path::new(path_str);

        // Security: reject non-absolute paths to prevent relative path traversal.
        if !path.is_absolute() {
            tracing::warn!("Kitty graphics: rejecting non-absolute file path: {path_str:?}");
            self.send_kitty_error(image_id_hint, quiet, "EPERM:file path must be absolute");
            return None;
        }

        tracing::debug!(
            "Kitty graphics: reading image from file: {path_str} (delete_after={delete_after})"
        );

        let data = match std::fs::read(path) {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!("Kitty graphics: failed to read file {path_str:?}: {e}");
                self.send_kitty_error(
                    image_id_hint,
                    quiet,
                    &format!("EIO:failed to read file: {e}"),
                );
                return None;
            }
        };

        tracing::debug!(
            "Kitty graphics: read {} bytes from file: {path_str}",
            data.len(),
        );

        if delete_after {
            if let Err(e) = std::fs::remove_file(path) {
                // Not fatal — we still have the data. Log the failure.
                tracing::warn!("Kitty graphics: failed to delete temp file {path_str:?}: {e}");
            } else {
                tracing::debug!("Kitty graphics: deleted temp file {path_str}");
            }
        }

        Some(data)
    }

    /// Extract required `s` (width) and `v` (height) from Kitty control data.
    ///
    /// Returns `None` and sends an error if either is missing.
    fn require_kitty_dimensions(
        &self,
        cmd: &KittyGraphicsCommand,
        image_id_hint: u32,
        quiet: u8,
    ) -> Option<(u32, u32)> {
        let Some(w) = cmd.control.src_width else {
            self.send_kitty_error(image_id_hint, quiet, "EINVAL:missing width (s)");
            return None;
        };
        let Some(h) = cmd.control.src_height else {
            self.send_kitty_error(image_id_hint, quiet, "EINVAL:missing height (v)");
            return None;
        };
        Some((w, h))
    }

    /// Compute display dimensions, store an `InlineImage`, and optionally place
    /// it into the buffer.
    // All parameters are required image placement inputs; grouping into a struct would obscure
    // the data flow without reducing coupling.
    #[allow(clippy::too_many_arguments)]
    fn place_kitty_image(
        &mut self,
        control: &KittyControlData,
        action: KittyAction,
        rgba_pixels: Vec<u8>,
        img_width_px: u32,
        img_height_px: u32,
        image_id_hint: u32,
        quiet: u8,
    ) {
        if img_width_px == 0 || img_height_px == 0 {
            self.send_kitty_error(image_id_hint, quiet, "EINVAL:zero dimension");
            return;
        }

        let (term_width, term_height) = self.get_win_size();

        let display_cols = control.display_cols.map_or_else(
            || {
                let cols =
                    usize::value_from(img_width_px.div_ceil(self.cell_pixel_width)).unwrap_or(0);
                cols.min(term_width).max(1)
            },
            |c| {
                let cols = usize::value_from(c).unwrap_or(0);
                cols.min(term_width).max(1)
            },
        );

        let display_rows = control.display_rows.map_or_else(
            || {
                let rows =
                    usize::value_from(img_height_px.div_ceil(self.cell_pixel_height)).unwrap_or(0);
                rows.min(term_height).max(1)
            },
            |r| {
                let rows = usize::value_from(r).unwrap_or(0);
                rows.min(term_height).max(1)
            },
        );

        let assigned_id = if image_id_hint > 0 {
            u64::from(image_id_hint)
        } else {
            next_image_id()
        };

        let inline_image = InlineImage {
            id: assigned_id,
            pixels: std::sync::Arc::new(rgba_pixels),
            width_px: img_width_px,
            height_px: img_height_px,
            display_cols,
            display_rows,
        };

        self.buffer.image_store_mut().insert(inline_image.clone());

        // If this is a virtual (Unicode placeholder) placement, store it in
        // the virtual_placements table instead of placing cells in the buffer.
        if control.unicode_placeholder {
            let pid = control.placement_id.unwrap_or(0);
            let vp = super::VirtualPlacement {
                image_id: assigned_id,
                placement_id: pid,
                cols: u32::try_from(display_cols).unwrap_or(u32::MAX),
                rows: u32::try_from(display_rows).unwrap_or(u32::MAX),
            };
            self.virtual_placements.insert((assigned_id, pid), vp);
        } else {
            let should_display =
                matches!(action, KittyAction::TransmitAndDisplay | KittyAction::Put);
            if should_display {
                tracing::debug!(
                    "Kitty graphics: placing image id={assigned_id} at cursor, \
                     {display_cols}x{display_rows} cells, {img_width_px}x{img_height_px} px",
                );
                let _new_offset = self.buffer.place_image(
                    inline_image,
                    0,
                    ImageProtocol::Kitty,
                    control.image_number,
                    control.placement_id,
                    control.z_index.unwrap_or(0),
                );
            } else {
                tracing::debug!(
                    "Kitty graphics: stored image id={assigned_id} (a={action:?}, not placing), \
                     {display_cols}x{display_rows} cells, {img_width_px}x{img_height_px} px",
                );
            }
        }

        // Send OK response unless suppressed.
        if quiet < 1 && assigned_id > 0 {
            let response_id = u32::value_from(assigned_id).unwrap_or(0);
            let response = format_kitty_response(response_id, true, "");
            self.write_to_pty(&response);
        }
    }

    /// Send a Kitty graphics error response, respecting quiet mode.
    fn send_kitty_error(&self, image_id: u32, quiet: u8, message: &str) {
        // quiet=2 suppresses all responses (including errors).
        if quiet >= 2 {
            tracing::debug!("Kitty graphics: error suppressed by q=2: id={image_id} {message}");
            return;
        }
        let response = format_kitty_response(image_id, false, message);
        self.write_to_pty(&response);
    }

    /// Handle `a=d` — delete images.
    ///
    /// Supports the most common delete targets: delete all (`d=a`/`d=A`),
    /// delete by ID (`d=i`/`d=I`), and delete at cursor (`d=c`/`d=C`).
    /// Unsupported targets are logged and ignored.
    fn handle_kitty_delete(&mut self, cmd: &KittyGraphicsCommand) {
        use freminal_common::buffer_states::kitty_graphics::KittyDeleteTarget;

        let target = cmd.control.delete_target.unwrap_or(KittyDeleteTarget::All);

        match target {
            KittyDeleteTarget::All | KittyDeleteTarget::AllIncludingNonVisible => {
                tracing::debug!(
                    "Kitty graphics: deleting ALL images ({} in store)",
                    self.buffer.image_store().len(),
                );
                self.buffer.clear_all_image_placements();
                self.buffer.image_store_mut().clear();
                self.virtual_placements.clear();
            }
            KittyDeleteTarget::ById | KittyDeleteTarget::ByIdCursorOrAfter => {
                if let Some(image_id) = cmd.control.image_id {
                    let id = u64::from(image_id);
                    tracing::debug!("Kitty graphics: deleting image id={id}");
                    self.buffer.clear_image_placements_by_id(id);
                    self.buffer.image_store_mut().remove(id);
                    self.virtual_placements
                        .retain(|&(img_id, _), _| img_id != id);
                }
            }
            KittyDeleteTarget::ByNumber | KittyDeleteTarget::ByNumberCursorOrAfter => {
                if let Some(number) = cmd.control.image_number {
                    tracing::debug!("Kitty graphics: deleting image number={number}");
                    self.buffer.clear_image_placements_by_number(number);
                }
            }
            KittyDeleteTarget::AtCursor => {
                tracing::debug!("Kitty graphics: deleting images at cursor");
                self.buffer.clear_image_placements_at_cursor();
            }
            KittyDeleteTarget::AtCursorAndAfter => {
                tracing::debug!("Kitty graphics: deleting images at cursor and after");
                self.buffer.clear_image_placements_at_cursor_and_after();
            }
            KittyDeleteTarget::AtCellRange | KittyDeleteTarget::AtCellRangeAndAfter => {
                // Uses the x and y from the control data to define the cell range.
                // Per Kitty spec, x/y default to cursor position if not specified.
                let cursor = self.buffer.get_cursor().pos;
                let col = cmd.control.src_x.map_or(cursor.x, |v| v as usize);
                let row = cmd.control.src_y.map_or(cursor.y, |v| v as usize);
                tracing::debug!("Kitty graphics: deleting images at cell ({row},{col})");
                // For the "and after" variant, clear from that position onward.
                if matches!(target, KittyDeleteTarget::AtCellRangeAndAfter) {
                    self.buffer
                        .clear_image_placements_at_cell_and_after(row, col);
                } else {
                    self.buffer.clear_image_placements_at_cell(row, col);
                }
            }
            KittyDeleteTarget::InColumnRange | KittyDeleteTarget::InColumnRangeAndAfter => {
                // Delete images that intersect the specified column.
                let cursor = self.buffer.get_cursor().pos;
                let col = cmd.control.src_x.map_or(cursor.x, |v| v as usize);
                tracing::debug!("Kitty graphics: deleting images in column {col}");
                self.buffer.clear_image_placements_in_column(col);
            }
            KittyDeleteTarget::InRowRange | KittyDeleteTarget::InRowRangeAndAfter => {
                // Delete images that intersect the specified row.
                let cursor = self.buffer.get_cursor().pos;
                let row = cmd.control.src_y.map_or(cursor.y, |v| v as usize);
                tracing::debug!("Kitty graphics: deleting images in row {row}");
                self.buffer.clear_image_placements_in_row(row);
            }
            KittyDeleteTarget::AtZIndex | KittyDeleteTarget::AtZIndexAndAfter => {
                let z = cmd.control.z_index.unwrap_or(0);
                tracing::debug!("Kitty graphics: deleting images at z-index {z}");
                self.buffer.clear_image_placements_by_z_index(z);
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use freminal_common::{
        buffer_states::{
            format_tag::FormatTag,
            kitty_graphics::{KittyAction, KittyGraphicsCommand},
        },
        colors::TerminalColor,
        pty_write::PtyWrite,
    };

    use super::super::TerminalHandler;

    // ------------------------------------------------------------------
    // Kitty graphics direct transfer tests
    // ------------------------------------------------------------------

    /// Helper: create a `TerminalHandler` with a write channel and return
    /// `(handler, rx)` so tests can inspect PTY responses.
    fn kitty_handler() -> (TerminalHandler, crossbeam_channel::Receiver<PtyWrite>) {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);
        (handler, rx)
    }

    /// Helper: build a `KittyGraphicsCommand` with the given control data and
    /// raw RGBA payload for a 2x2 image.
    fn kitty_rgba_2x2_cmd(action: KittyAction) -> KittyGraphicsCommand {
        use freminal_common::buffer_states::kitty_graphics::{KittyControlData, KittyFormat};

        // 2x2 RGBA = 16 bytes.
        let rgba_data: Vec<u8> = vec![
            255, 0, 0, 255, // red
            0, 255, 0, 255, // green
            0, 0, 255, 255, // blue
            255, 255, 0, 255, // yellow
        ];

        KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(action),
                format: Some(KittyFormat::Rgba),
                src_width: Some(2),
                src_height: Some(2),
                image_id: Some(42),
                ..KittyControlData::default()
            },
            payload: rgba_data,
        }
    }

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
    fn kitty_single_rgba_transmit_and_display_places_image() {
        use freminal_common::buffer_states::kitty_graphics::KittyAction;

        let (mut handler, _rx) = kitty_handler();
        let cmd = kitty_rgba_2x2_cmd(KittyAction::TransmitAndDisplay);

        handler.handle_kitty_graphics(cmd);

        // Image should be placed in the buffer.
        let has_image = handler.buffer().has_any_image_cell();
        assert!(
            has_image,
            "Expected image cells after Kitty TransmitAndDisplay"
        );

        // Image should be in the store.
        assert!(
            handler.buffer().image_store().get(42).is_some(),
            "Expected image id=42 in the image store"
        );
    }

    #[test]
    fn kitty_single_rgba_transmit_only_stores_but_does_not_place() {
        use freminal_common::buffer_states::kitty_graphics::KittyAction;

        let (mut handler, _rx) = kitty_handler();
        let cmd = kitty_rgba_2x2_cmd(KittyAction::Transmit);

        handler.handle_kitty_graphics(cmd);

        // Image should be in the store but NOT placed in cells.
        assert!(
            handler.buffer().image_store().get(42).is_some(),
            "Expected image id=42 in the store after Transmit"
        );

        let has_image = handler.buffer().has_any_image_cell();
        assert!(!has_image, "Transmit-only should not place image cells");
    }

    #[test]
    fn kitty_single_rgb_converts_to_rgba() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat,
        };

        let (mut handler, _rx) = kitty_handler();

        // 2x2 RGB = 12 bytes (no alpha channel).
        let rgb_data: Vec<u8> = vec![
            255, 0, 0, // red
            0, 255, 0, // green
            0, 0, 255, // blue
            255, 255, 0, // yellow
        ];

        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Rgb),
                src_width: Some(2),
                src_height: Some(2),
                image_id: Some(99),
                ..KittyControlData::default()
            },
            payload: rgb_data,
        };

        handler.handle_kitty_graphics(cmd);

        // The stored image should have RGBA pixels (16 bytes for 2x2).
        let img = handler.buffer().image_store().get(99).unwrap();
        assert_eq!(
            img.pixels.len(),
            16,
            "RGB should be converted to RGBA (4 bytes/pixel)"
        );
        // Verify alpha was inserted: first pixel should be [255, 0, 0, 255].
        assert_eq!(&img.pixels[0..4], &[255, 0, 0, 255]);
    }

    #[test]
    fn kitty_single_png_decodes_and_places() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat,
        };

        let (mut handler, _rx) = kitty_handler();

        let png_data = make_test_png(); // 2x2 red PNG.

        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Png),
                image_id: Some(77),
                // PNG does not require s/v — dimensions come from the image.
                ..KittyControlData::default()
            },
            payload: png_data,
        };

        handler.handle_kitty_graphics(cmd);

        let img = handler.buffer().image_store().get(77).unwrap();
        assert_eq!(img.width_px, 2);
        assert_eq!(img.height_px, 2);
        // PNG decoded to RGBA: 2*2*4 = 16 bytes.
        assert_eq!(img.pixels.len(), 16);
    }

    #[test]
    fn kitty_single_rgba_missing_dimensions_sends_error() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat,
        };

        let (mut handler, rx) = kitty_handler();

        // RGBA payload but no s/v dimensions → should send error.
        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Rgba),
                image_id: Some(10),
                // No src_width or src_height!
                ..KittyControlData::default()
            },
            payload: vec![0; 16],
        };

        handler.handle_kitty_graphics(cmd);

        // Should NOT be stored.
        assert!(
            handler.buffer().image_store().get(10).is_none(),
            "Image should not be stored when dimensions are missing"
        );

        // Should have sent an error response.
        let response = rx.try_recv().unwrap();
        match response {
            PtyWrite::Write(bytes) => {
                let s = String::from_utf8_lossy(&bytes);
                assert!(s.contains("EINVAL"), "Expected EINVAL error, got: {s}");
            }
            PtyWrite::Resize(_) => panic!("Expected PtyWrite::Write"),
        }
    }

    #[test]
    fn kitty_single_rgba_payload_size_mismatch_sends_error() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat,
        };

        let (mut handler, rx) = kitty_handler();

        // Says 2x2 RGBA (expects 16 bytes) but payload is only 8 bytes.
        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Rgba),
                src_width: Some(2),
                src_height: Some(2),
                image_id: Some(11),
                ..KittyControlData::default()
            },
            payload: vec![0; 8], // too small
        };

        handler.handle_kitty_graphics(cmd);

        assert!(handler.buffer().image_store().get(11).is_none());

        let response = rx.try_recv().unwrap();
        match response {
            PtyWrite::Write(bytes) => {
                let s = String::from_utf8_lossy(&bytes);
                assert!(s.contains("EINVAL"), "Expected EINVAL error, got: {s}");
            }
            PtyWrite::Resize(_) => panic!("Expected PtyWrite::Write"),
        }
    }

    #[test]
    fn kitty_single_empty_payload_sends_enodata() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat,
        };

        let (mut handler, rx) = kitty_handler();

        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Rgba),
                src_width: Some(2),
                src_height: Some(2),
                image_id: Some(12),
                ..KittyControlData::default()
            },
            payload: Vec::new(), // empty
        };

        handler.handle_kitty_graphics(cmd);

        let response = rx.try_recv().unwrap();
        match response {
            PtyWrite::Write(bytes) => {
                let s = String::from_utf8_lossy(&bytes);
                assert!(s.contains("ENODATA"), "Expected ENODATA error, got: {s}");
            }
            PtyWrite::Resize(_) => panic!("Expected PtyWrite::Write"),
        }
    }

    #[test]
    fn kitty_single_ok_response_sent_for_verbose_mode() {
        use freminal_common::buffer_states::kitty_graphics::KittyAction;

        let (mut handler, rx) = kitty_handler();

        // quiet=0 (default) → should send OK.
        let cmd = kitty_rgba_2x2_cmd(KittyAction::TransmitAndDisplay);
        handler.handle_kitty_graphics(cmd);

        let response = rx.try_recv().unwrap();
        match response {
            PtyWrite::Write(bytes) => {
                let s = String::from_utf8_lossy(&bytes);
                assert!(s.contains("OK"), "Expected OK response, got: {s}");
            }
            PtyWrite::Resize(_) => panic!("Expected PtyWrite::Write"),
        }
    }

    #[test]
    fn kitty_single_quiet_1_suppresses_ok() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat,
        };

        let (mut handler, rx) = kitty_handler();

        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Rgba),
                src_width: Some(2),
                src_height: Some(2),
                image_id: Some(50),
                quiet: 1, // suppress OK but not errors
                ..KittyControlData::default()
            },
            payload: vec![
                255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 0, 255,
            ],
        };

        handler.handle_kitty_graphics(cmd);

        // Image should still be placed.
        assert!(handler.buffer().image_store().get(50).is_some());

        // But no OK response should be sent.
        assert!(
            rx.try_recv().is_err(),
            "quiet=1 should suppress OK response"
        );
    }

    #[test]
    fn kitty_single_quiet_2_suppresses_all_responses() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat,
        };

        let (mut handler, rx) = kitty_handler();

        // quiet=2 with a bad payload → error should be suppressed.
        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Rgba),
                src_width: Some(2),
                src_height: Some(2),
                image_id: Some(51),
                quiet: 2,
                ..KittyControlData::default()
            },
            payload: vec![0; 8], // wrong size → EINVAL
        };

        handler.handle_kitty_graphics(cmd);

        // No response at all.
        assert!(
            rx.try_recv().is_err(),
            "quiet=2 should suppress all responses including errors"
        );
    }

    #[test]
    fn kitty_delete_all_clears_images() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyDeleteTarget,
        };

        let (mut handler, _rx) = kitty_handler();

        // First, place an image.
        let cmd = kitty_rgba_2x2_cmd(KittyAction::TransmitAndDisplay);
        handler.handle_kitty_graphics(cmd);
        assert!(handler.buffer().image_store().get(42).is_some());

        // Now delete all.
        let delete_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Delete),
                delete_target: Some(KittyDeleteTarget::All),
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(delete_cmd);

        // Image should be gone from the store.
        assert!(
            handler.buffer().image_store().get(42).is_none(),
            "Delete all should remove all images"
        );

        // No image cells should remain.
        let has_image = handler
            .buffer()
            .get_rows()
            .iter()
            .any(|row| row.cells().iter().any(crate::cell::Cell::has_image));
        assert!(!has_image, "Delete all should clear image cells");
    }

    #[test]
    fn kitty_delete_by_id_removes_only_target() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyDeleteTarget, KittyFormat,
        };

        let (mut handler, _rx) = kitty_handler();

        // Place image id=42.
        let cmd1 = kitty_rgba_2x2_cmd(KittyAction::TransmitAndDisplay);
        handler.handle_kitty_graphics(cmd1);

        // Place image id=99.
        let cmd2 = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Transmit),
                format: Some(KittyFormat::Rgba),
                src_width: Some(2),
                src_height: Some(2),
                image_id: Some(99),
                ..KittyControlData::default()
            },
            payload: vec![
                255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 0, 255,
            ],
        };
        handler.handle_kitty_graphics(cmd2);

        assert!(handler.buffer().image_store().get(42).is_some());
        assert!(handler.buffer().image_store().get(99).is_some());

        // Delete only id=42.
        let delete_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Delete),
                delete_target: Some(KittyDeleteTarget::ById),
                image_id: Some(42),
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(delete_cmd);

        assert!(
            handler.buffer().image_store().get(42).is_none(),
            "id=42 should be removed"
        );
        assert!(
            handler.buffer().image_store().get(99).is_some(),
            "id=99 should survive delete-by-id of 42"
        );
    }

    #[test]
    fn kitty_delete_at_cursor_clears_images_at_cursor_row() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyDeleteTarget,
        };

        let (mut handler, _rx) = kitty_handler();

        // Place an image (will be at row 0).
        let cmd = kitty_rgba_2x2_cmd(KittyAction::TransmitAndDisplay);
        handler.handle_kitty_graphics(cmd);

        // Cursor should now be below the image. Move it back to row 0.
        handler.buffer_mut().set_cursor_pos(Some(0), Some(0));

        // Delete at cursor (row 0 only).
        let delete_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Delete),
                delete_target: Some(KittyDeleteTarget::AtCursor),
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(delete_cmd);

        // Row 0 should have no image cells.
        let row0_has_image = handler.buffer().get_rows()[0]
            .cells()
            .iter()
            .any(crate::cell::Cell::has_image);
        assert!(!row0_has_image, "AtCursor delete should clear row 0 images");
    }

    #[test]
    fn kitty_delete_at_cursor_and_after_clears_remaining_rows() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyDeleteTarget,
        };

        let (mut handler, _rx) = kitty_handler();

        // Place an image at row 0 (2x2 px → 1x1 cell with 8x16 cell size).
        let cmd = kitty_rgba_2x2_cmd(KittyAction::TransmitAndDisplay);
        handler.handle_kitty_graphics(cmd);

        // Move cursor to row 0.
        handler.buffer_mut().set_cursor_pos(Some(0), Some(0));

        // Delete at cursor and after.
        let delete_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Delete),
                delete_target: Some(KittyDeleteTarget::AtCursorAndAfter),
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(delete_cmd);

        // All rows from cursor onward should have no image cells.
        let any_image = handler
            .buffer()
            .get_rows()
            .iter()
            .any(|row| row.cells().iter().any(crate::cell::Cell::has_image));
        assert!(
            !any_image,
            "AtCursorAndAfter should clear all image cells from cursor onward"
        );
    }

    #[test]
    fn kitty_display_cols_and_rows_override() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat,
        };

        let (mut handler, _rx) = kitty_handler();

        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Rgba),
                src_width: Some(2),
                src_height: Some(2),
                image_id: Some(55),
                display_cols: Some(10),
                display_rows: Some(5),
                ..KittyControlData::default()
            },
            payload: vec![
                255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 0, 255,
            ],
        };

        handler.handle_kitty_graphics(cmd);

        let img = handler.buffer().image_store().get(55).unwrap();
        assert_eq!(img.display_cols, 10);
        assert_eq!(img.display_rows, 5);
    }

    #[test]
    fn kitty_query_rgb_format_responds_ok() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat,
        };

        let (mut handler, rx) = kitty_handler();

        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Query),
                format: Some(KittyFormat::Rgb),
                image_id: Some(31),
                ..KittyControlData::default()
            },
            payload: vec![0, 0, 0], // minimal payload for query
        };

        handler.handle_kitty_graphics(cmd);

        let response = rx.try_recv().unwrap();
        match response {
            PtyWrite::Write(bytes) => {
                let s = String::from_utf8_lossy(&bytes);
                assert!(
                    s.contains("OK"),
                    "f=24 (RGB) query should succeed, got: {s}"
                );
                assert!(!s.contains("ENOTSUP"), "f=24 should NOT be unsupported");
            }
            PtyWrite::Resize(_) => panic!("Expected PtyWrite::Write"),
        }
    }

    #[test]
    fn kitty_chunked_transfer_assembles_and_places() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat,
        };

        let (mut handler, _rx) = kitty_handler();

        // Full RGBA payload for 2x2 = 16 bytes, split into two 8-byte chunks.
        let full_payload: Vec<u8> = vec![
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 0, 255,
        ];

        // First chunk (more_data=true).
        let chunk1 = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Rgba),
                src_width: Some(2),
                src_height: Some(2),
                image_id: Some(88),
                more_data: true,
                ..KittyControlData::default()
            },
            payload: full_payload[..8].to_vec(),
        };

        // Last chunk (more_data=false / default).
        let chunk2 = KittyGraphicsCommand {
            control: KittyControlData {
                more_data: false,
                ..KittyControlData::default()
            },
            payload: full_payload[8..].to_vec(),
        };

        handler.handle_kitty_graphics(chunk1);
        // After first chunk, image should NOT be in the store yet.
        assert!(
            handler.buffer().image_store().get(88).is_none(),
            "Image should not appear until final chunk"
        );

        handler.handle_kitty_graphics(chunk2);
        // After final chunk, image should be stored and placed.
        assert!(
            handler.buffer().image_store().get(88).is_some(),
            "Image should appear after final chunk"
        );

        let has_image = handler.buffer().has_any_image_cell();
        assert!(has_image, "Chunked TransmitAndDisplay should place image");
    }

    // ------------------------------------------------------------------
    // Kitty Unicode placeholder tests
    // ------------------------------------------------------------------

    /// Helper: create a Kitty command with `unicode_placeholder = true` for a 2×2 RGBA image.
    fn kitty_virtual_2x2_cmd() -> KittyGraphicsCommand {
        use freminal_common::buffer_states::kitty_graphics::{KittyControlData, KittyFormat};

        let rgba_data: Vec<u8> = vec![
            255, 0, 0, 255, // red
            0, 255, 0, 255, // green
            0, 0, 255, 255, // blue
            255, 255, 0, 255, // yellow
        ];

        KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Rgba),
                src_width: Some(2),
                src_height: Some(2),
                image_id: Some(42),
                placement_id: Some(0),
                unicode_placeholder: true,
                ..KittyControlData::default()
            },
            payload: rgba_data,
        }
    }

    /// Helper: build a `FormatTag` whose foreground encodes an image ID and
    /// whose underline color encodes a placement ID using 24-bit RGB.
    fn format_for_placeholder(image_id: u32, placement_id: u32) -> FormatTag {
        use freminal_common::buffer_states::cursor::StateColors;

        let fg = TerminalColor::Custom(
            u8::try_from((image_id >> 16) & 0xFF).unwrap_or(0),
            u8::try_from((image_id >> 8) & 0xFF).unwrap_or(0),
            u8::try_from(image_id & 0xFF).unwrap_or(0),
        );

        let ul = TerminalColor::Custom(
            u8::try_from((placement_id >> 16) & 0xFF).unwrap_or(0),
            u8::try_from((placement_id >> 8) & 0xFF).unwrap_or(0),
            u8::try_from(placement_id & 0xFF).unwrap_or(0),
        );

        let mut tag = FormatTag::default();
        tag.colors = StateColors {
            color: fg,
            underline_color: ul,
            ..tag.colors
        };
        tag
    }

    /// U+10EEEE without any diacritics (bare placeholder char).
    const PLACEHOLDER_BYTES: &[u8] = &[0xF4, 0x8E, 0xBB, 0xAE];

    /// U+10EEEE followed by one diacritic (row=0 → U+0305).
    fn placeholder_with_row(row_idx: usize) -> Vec<u8> {
        let mut bytes = PLACEHOLDER_BYTES.to_vec();
        let diacritic =
            char::from_u32(DIACRITICS_FOR_TESTS[row_idx]).expect("valid diacritic codepoint");
        let mut buf = [0u8; 4];
        let encoded = diacritic.encode_utf8(&mut buf);
        bytes.extend_from_slice(encoded.as_bytes());
        bytes
    }

    /// U+10EEEE followed by two diacritics (row + col).
    fn placeholder_with_row_col(row_idx: usize, col_idx: usize) -> Vec<u8> {
        let mut bytes = PLACEHOLDER_BYTES.to_vec();
        for &idx in &[row_idx, col_idx] {
            let diacritic =
                char::from_u32(DIACRITICS_FOR_TESTS[idx]).expect("valid diacritic codepoint");
            let mut buf = [0u8; 4];
            let encoded = diacritic.encode_utf8(&mut buf);
            bytes.extend_from_slice(encoded.as_bytes());
        }
        bytes
    }

    /// A few diacritics from the table for test convenience.
    /// Index 0 = U+0305, index 1 = U+030D, index 2 = U+030E, etc.
    const DIACRITICS_FOR_TESTS: &[u32] = &[0x0305, 0x030D, 0x030E, 0x0310, 0x0312, 0x033D];

    #[test]
    fn kitty_virtual_placement_stores_but_does_not_place_cells() {
        let (mut handler, _rx) = kitty_handler();
        let cmd = kitty_virtual_2x2_cmd();

        handler.handle_kitty_graphics(cmd);

        // Image should be in the store.
        assert!(
            handler.buffer().image_store().get(42).is_some(),
            "Image id=42 should be in the image store"
        );

        // But NO image cells should be placed in the buffer.
        let has_image = handler
            .buffer()
            .get_rows()
            .iter()
            .any(|row| row.cells().iter().any(crate::cell::Cell::has_image));
        assert!(
            !has_image,
            "Virtual placement should NOT place image cells directly"
        );

        // Virtual placement should be stored.
        assert!(
            !handler.virtual_placements.is_empty(),
            "Virtual placements table should have an entry"
        );
        assert!(
            handler.virtual_placements.contains_key(&(42, 0)),
            "Should have virtual placement for (image_id=42, placement_id=0)"
        );
    }

    #[test]
    fn kitty_placeholder_chars_create_image_cells() {
        let (mut handler, _rx) = kitty_handler();

        // Step 1: Create a virtual placement with U=1.
        let cmd = kitty_virtual_2x2_cmd();
        handler.handle_kitty_graphics(cmd);

        // Step 2: Set foreground to encode image_id=42, underline to placement_id=0.
        let fmt = format_for_placeholder(42, 0);
        handler.set_format(fmt);

        // Step 3: Send two placeholder chars (row=0, col=0) and (row=0, col=1)
        // with explicit row+col diacritics.
        let mut data = placeholder_with_row_col(0, 0);
        data.extend_from_slice(&placeholder_with_row_col(0, 1));
        handler.handle_data(&data);

        // Step 4: Verify image cells were placed.
        let row0 = &handler.buffer().get_rows()[0];
        assert!(
            row0.cells()[0].has_image(),
            "Cell (0,0) should have an image placement"
        );
        assert!(
            row0.cells()[1].has_image(),
            "Cell (0,1) should have an image placement"
        );

        // Verify the image placements reference the correct image and positions.
        let p0 = row0.cells()[0].image_placement().unwrap();
        assert_eq!(p0.image_id, 42, "Cell (0,0) should reference image_id=42");
        assert_eq!(p0.row_in_image, 0);
        assert_eq!(p0.col_in_image, 0);

        let p1 = row0.cells()[1].image_placement().unwrap();
        assert_eq!(p1.image_id, 42, "Cell (0,1) should reference image_id=42");
        assert_eq!(p1.row_in_image, 0);
        assert_eq!(p1.col_in_image, 1);
    }

    #[test]
    fn kitty_placeholder_inheritance_no_diacritics() {
        let (mut handler, _rx) = kitty_handler();

        // Create virtual placement.
        let cmd = kitty_virtual_2x2_cmd();
        handler.handle_kitty_graphics(cmd);

        // Set foreground = image_id=42, underline = placement_id=0.
        let fmt = format_for_placeholder(42, 0);
        handler.set_format(fmt);

        // Send first placeholder with explicit row=0, col=0.
        let first = placeholder_with_row_col(0, 0);
        handler.handle_data(&first);

        // Send second placeholder with NO diacritics — should inherit row=0, col=1.
        handler.handle_data(PLACEHOLDER_BYTES);

        let row0 = &handler.buffer().get_rows()[0];
        let p0 = row0.cells()[0].image_placement().unwrap();
        assert_eq!(p0.row_in_image, 0);
        assert_eq!(p0.col_in_image, 0);

        let p1 = row0.cells()[1].image_placement().unwrap();
        assert_eq!(p1.row_in_image, 0);
        assert_eq!(p1.col_in_image, 1, "Inherited col should be prev_col + 1");
    }

    #[test]
    fn kitty_placeholder_inheritance_row_only_diacritic() {
        let (mut handler, _rx) = kitty_handler();

        // Create virtual placement.
        let cmd = kitty_virtual_2x2_cmd();
        handler.handle_kitty_graphics(cmd);

        let fmt = format_for_placeholder(42, 0);
        handler.set_format(fmt);

        // First cell: row=0, col=0 (explicit).
        let first = placeholder_with_row_col(0, 0);
        handler.handle_data(&first);

        // Second cell: row=0 only (one diacritic) — should inherit col=1 from prev.
        let second = placeholder_with_row(0);
        handler.handle_data(&second);

        let row0 = &handler.buffer().get_rows()[0];
        let p1 = row0.cells()[1].image_placement().unwrap();
        assert_eq!(p1.row_in_image, 0);
        assert_eq!(p1.col_in_image, 1, "Should inherit col+1 from previous");
    }

    #[test]
    fn kitty_placeholder_new_row_resets_col() {
        let (mut handler, _rx) = kitty_handler();

        // Create virtual placement.
        let cmd = kitty_virtual_2x2_cmd();
        handler.handle_kitty_graphics(cmd);

        let fmt = format_for_placeholder(42, 0);
        handler.set_format(fmt);

        // Row 0, col 0 (explicit).
        handler.handle_data(&placeholder_with_row_col(0, 0));

        // Row 0, col 1 (inherited via bare placeholder).
        handler.handle_data(PLACEHOLDER_BYTES);

        // Now move to next line (simulate \r\n or newline).
        handler.handle_newline();
        handler.handle_carriage_return();

        // Row 1, col 0 with row-only diacritic — different row, so col resets to 0.
        handler.handle_data(&placeholder_with_row(1));

        let row1 = &handler.buffer().get_rows()[1];
        let p = row1.cells()[0].image_placement().unwrap();
        assert_eq!(p.row_in_image, 1, "Should be row 1");
        assert_eq!(p.col_in_image, 0, "New row should start at col 0");
    }

    #[test]
    fn kitty_placeholder_no_virtual_placement_inserts_space() {
        let (mut handler, _rx) = kitty_handler();
        // Do NOT create any virtual placement.

        // Set foreground to encode image_id=99.
        let fmt = format_for_placeholder(99, 0);
        handler.set_format(fmt);

        // Send a placeholder char with row=0, col=0.
        let data = placeholder_with_row_col(0, 0);

        // But wait — the fast path skips placeholder processing when
        // virtual_placements is empty. The char goes through as normal text.
        handler.handle_data(&data);

        // The cell should NOT have an image placement.
        let row0 = &handler.buffer().get_rows()[0];
        assert!(
            !row0.cells()[0].has_image(),
            "Without virtual placements, placeholder should not create image cells"
        );
    }

    #[test]
    fn kitty_delete_all_clears_virtual_placements() {
        use freminal_common::buffer_states::kitty_graphics::{KittyControlData, KittyDeleteTarget};

        let (mut handler, _rx) = kitty_handler();

        // Create a virtual placement.
        let cmd = kitty_virtual_2x2_cmd();
        handler.handle_kitty_graphics(cmd);
        assert!(!handler.virtual_placements.is_empty());

        // Delete all.
        let delete_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Delete),
                delete_target: Some(KittyDeleteTarget::All),
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(delete_cmd);

        assert!(
            handler.virtual_placements.is_empty(),
            "Delete all should clear virtual placements"
        );
    }

    #[test]
    fn kitty_delete_by_id_clears_matching_virtual_placements() {
        use freminal_common::buffer_states::kitty_graphics::{KittyControlData, KittyDeleteTarget};

        let (mut handler, _rx) = kitty_handler();

        // Create a virtual placement for image_id=42.
        let cmd = kitty_virtual_2x2_cmd();
        handler.handle_kitty_graphics(cmd);
        assert!(handler.virtual_placements.contains_key(&(42, 0)));

        // Delete by ID = 42.
        let delete_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Delete),
                delete_target: Some(KittyDeleteTarget::ById),
                image_id: Some(42),
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(delete_cmd);

        assert!(
            handler.virtual_placements.is_empty(),
            "Delete by ID=42 should clear the virtual placement"
        );
    }

    #[test]
    fn kitty_placeholder_fast_path_with_no_virtual_placements() {
        let (mut handler, _rx) = kitty_handler();
        // No virtual placements — fast path should just insert text normally.

        handler.handle_data(b"Hello World");

        // Cursor should have advanced.
        let cursor = handler.buffer().get_cursor();
        assert_eq!(
            cursor.pos.x, 11,
            "Cursor should be at column 11 after 'Hello World'"
        );
    }

    // -----------------------------------------------------------------------
    // Kitty graphics file path transmission tests (t=f, t=t, t=s)
    // -----------------------------------------------------------------------

    #[test]
    fn kitty_file_transmission_reads_png_from_disk() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat, KittyTransmission,
        };

        let (mut handler, _rx) = kitty_handler();

        // Write a minimal valid 1x1 red PNG to a temp file.
        let dir = std::env::temp_dir();
        let path = dir.join("freminal_test_kitty_file.png");
        let img = image::RgbaImage::from_pixel(1, 1, image::Rgba([255, 0, 0, 255]));
        img.save(&path).expect("failed to write test PNG");

        let path_bytes = path
            .to_str()
            .expect("non-UTF-8 temp path")
            .as_bytes()
            .to_vec();

        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Png),
                transmission: Some(KittyTransmission::File),
                image_id: Some(999),
                ..KittyControlData::default()
            },
            payload: path_bytes,
        };

        handler.handle_kitty_graphics(cmd);

        // Image should be in the store.
        assert!(
            handler.buffer().image_store().get(999).is_some(),
            "Expected image id=999 in the store after t=f transmission"
        );

        // Image should be placed in cells.
        let has_image = handler.buffer().has_any_image_cell();
        assert!(
            has_image,
            "Expected image cells after t=f Kitty TransmitAndDisplay"
        );

        // Clean up.
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn kitty_temp_file_transmission_reads_and_deletes() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat, KittyTransmission,
        };

        let (mut handler, _rx) = kitty_handler();

        // Write a minimal valid 1x1 PNG to a temp file.
        let dir = std::env::temp_dir();
        let path = dir.join("freminal_test_kitty_tempfile.png");
        let img = image::RgbaImage::from_pixel(1, 1, image::Rgba([0, 255, 0, 255]));
        img.save(&path).expect("failed to write test PNG");

        let path_bytes = path
            .to_str()
            .expect("non-UTF-8 temp path")
            .as_bytes()
            .to_vec();

        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Png),
                transmission: Some(KittyTransmission::TempFile),
                image_id: Some(998),
                ..KittyControlData::default()
            },
            payload: path_bytes,
        };

        handler.handle_kitty_graphics(cmd);

        // Image should be in the store.
        assert!(
            handler.buffer().image_store().get(998).is_some(),
            "Expected image id=998 in the store after t=t transmission"
        );

        // The temp file should have been deleted.
        assert!(
            !path.exists(),
            "Temp file should be deleted after t=t transmission"
        );
    }

    #[test]
    fn kitty_file_transmission_nonexistent_file_sends_error() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat, KittyTransmission,
        };

        let (mut handler, rx) = kitty_handler();

        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Png),
                transmission: Some(KittyTransmission::File),
                image_id: Some(997),
                ..KittyControlData::default()
            },
            payload: b"/tmp/freminal_this_file_does_not_exist_12345.png".to_vec(),
        };

        handler.handle_kitty_graphics(cmd);

        // Should NOT be in the store.
        assert!(
            handler.buffer().image_store().get(997).is_none(),
            "No image should be stored for a nonexistent file"
        );

        // An error response should have been sent.
        let mut found_error = false;
        while let Ok(msg) = rx.try_recv() {
            if let PtyWrite::Write(bytes) = msg {
                let text = String::from_utf8_lossy(&bytes);
                if text.contains("EIO") {
                    found_error = true;
                }
            }
        }
        assert!(
            found_error,
            "Expected an EIO error response for missing file"
        );
    }

    #[test]
    fn kitty_file_transmission_relative_path_rejected() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat, KittyTransmission,
        };

        let (mut handler, rx) = kitty_handler();

        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Png),
                transmission: Some(KittyTransmission::File),
                image_id: Some(996),
                ..KittyControlData::default()
            },
            payload: b"../../../etc/passwd".to_vec(),
        };

        handler.handle_kitty_graphics(cmd);

        // Should NOT be in the store.
        assert!(
            handler.buffer().image_store().get(996).is_none(),
            "No image should be stored for a relative path"
        );

        // An EPERM error response should have been sent.
        let mut found_error = false;
        while let Ok(msg) = rx.try_recv() {
            if let PtyWrite::Write(bytes) = msg {
                let text = String::from_utf8_lossy(&bytes);
                if text.contains("EPERM") {
                    found_error = true;
                }
            }
        }
        assert!(
            found_error,
            "Expected an EPERM error response for relative path"
        );
    }

    #[test]
    fn kitty_shared_memory_transmission_unsupported() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat, KittyTransmission,
        };

        let (mut handler, rx) = kitty_handler();

        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Rgba),
                transmission: Some(KittyTransmission::SharedMemory),
                src_width: Some(1),
                src_height: Some(1),
                image_id: Some(995),
                ..KittyControlData::default()
            },
            payload: b"shm_name".to_vec(),
        };

        handler.handle_kitty_graphics(cmd);

        // Should NOT be in the store.
        assert!(
            handler.buffer().image_store().get(995).is_none(),
            "No image should be stored for shared memory transmission"
        );

        // An ENOTSUP error response should have been sent.
        let mut found_error = false;
        while let Ok(msg) = rx.try_recv() {
            if let PtyWrite::Write(bytes) = msg {
                let text = String::from_utf8_lossy(&bytes);
                if text.contains("ENOTSUP") {
                    found_error = true;
                }
            }
        }
        assert!(
            found_error,
            "Expected an ENOTSUP error response for shared memory"
        );
    }

    #[test]
    fn kitty_direct_transmission_still_works() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat, KittyTransmission,
        };

        let (mut handler, _rx) = kitty_handler();

        // 1x1 RGBA = 4 bytes, explicit t=d.
        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Rgba),
                transmission: Some(KittyTransmission::Direct),
                src_width: Some(1),
                src_height: Some(1),
                image_id: Some(994),
                ..KittyControlData::default()
            },
            payload: vec![255, 0, 0, 255],
        };

        handler.handle_kitty_graphics(cmd);

        assert!(
            handler.buffer().image_store().get(994).is_some(),
            "Expected image id=994 in the store with explicit t=d"
        );
    }

    #[test]
    fn kitty_file_invalid_utf8_path_sends_error() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat, KittyTransmission,
        };

        let (mut handler, rx) = kitty_handler();

        // Invalid UTF-8 bytes.
        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Png),
                transmission: Some(KittyTransmission::File),
                image_id: Some(993),
                ..KittyControlData::default()
            },
            payload: vec![0xFF, 0xFE, 0x80, 0x00],
        };

        handler.handle_kitty_graphics(cmd);

        assert!(
            handler.buffer().image_store().get(993).is_none(),
            "No image should be stored for invalid UTF-8 path"
        );

        let mut found_error = false;
        while let Ok(msg) = rx.try_recv() {
            if let PtyWrite::Write(bytes) = msg {
                let text = String::from_utf8_lossy(&bytes);
                if text.contains("EINVAL") {
                    found_error = true;
                }
            }
        }
        assert!(
            found_error,
            "Expected an EINVAL error response for invalid UTF-8 path"
        );
    }

    // -----------------------------------------------------------------------
    // Kitty `a=p` (Put) tests — transmit then place by image ID
    // -----------------------------------------------------------------------

    #[test]
    fn kitty_put_places_previously_transmitted_image() {
        use freminal_common::buffer_states::kitty_graphics::{KittyAction, KittyControlData};

        let (mut handler, _rx) = kitty_handler();

        // Step 1: Transmit a 2x2 RGBA image (store only, no display).
        let transmit_cmd = kitty_rgba_2x2_cmd(KittyAction::Transmit);
        handler.handle_kitty_graphics(transmit_cmd);

        // Image should be in the store but not placed.
        assert!(handler.buffer().image_store().get(42).is_some());
        let has_image_before = handler.buffer().has_any_image_cell();
        assert!(
            !has_image_before,
            "Transmit-only should not place image cells"
        );

        // Step 2: Put (display the previously transmitted image).
        let put_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Put),
                image_id: Some(42),
                ..KittyControlData::default()
            },
            payload: Vec::new(), // Put has no payload — references by ID.
        };
        handler.handle_kitty_graphics(put_cmd);

        // Image should now be placed in buffer cells.
        let has_image_after = handler.buffer().has_any_image_cell();
        assert!(
            has_image_after,
            "Put should place previously transmitted image into cells"
        );
    }

    #[test]
    fn kitty_put_with_display_size_overrides() {
        use freminal_common::buffer_states::kitty_graphics::{KittyAction, KittyControlData};

        let (mut handler, _rx) = kitty_handler();

        // Transmit a 2x2 RGBA image.
        let transmit_cmd = kitty_rgba_2x2_cmd(KittyAction::Transmit);
        handler.handle_kitty_graphics(transmit_cmd);

        // Put with display size overrides: c=10 columns, r=5 rows.
        let put_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Put),
                image_id: Some(42),
                display_cols: Some(10),
                display_rows: Some(5),
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(put_cmd);

        // Check the image in the store was updated with new display dimensions.
        let img = handler.buffer().image_store().get(42).unwrap();
        assert_eq!(
            img.display_cols, 10,
            "display_cols should be overridden to 10"
        );
        assert_eq!(
            img.display_rows, 5,
            "display_rows should be overridden to 5"
        );

        // Verify image cells were placed.
        let has_image = handler.buffer().has_any_image_cell();
        assert!(has_image, "Put with overrides should place image cells");
    }

    #[test]
    fn kitty_put_with_no_cursor_movement() {
        use freminal_common::buffer_states::kitty_graphics::{KittyAction, KittyControlData};

        let (mut handler, _rx) = kitty_handler();

        // Transmit a 2x2 RGBA image.
        let transmit_cmd = kitty_rgba_2x2_cmd(KittyAction::Transmit);
        handler.handle_kitty_graphics(transmit_cmd);

        // Move cursor to a known position.
        handler.buffer_mut().set_cursor_pos(Some(5), Some(3));
        let cursor_before = handler.cursor_pos();

        // Put with C=1 (no cursor movement).
        let put_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Put),
                image_id: Some(42),
                no_cursor_movement: true,
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(put_cmd);

        let cursor_after = handler.cursor_pos();
        assert_eq!(
            cursor_before, cursor_after,
            "Cursor should not move when C=1 is set on Put"
        );
    }

    #[test]
    fn kitty_put_nonexistent_image_sends_error() {
        use freminal_common::buffer_states::kitty_graphics::{KittyAction, KittyControlData};

        let (mut handler, rx) = kitty_handler();

        // Put for an image ID that was never transmitted.
        let put_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Put),
                image_id: Some(999),
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(put_cmd);

        // No image cells should be placed.
        let has_image = handler
            .buffer()
            .get_rows()
            .iter()
            .any(|row| row.cells().iter().any(crate::cell::Cell::has_image));
        assert!(
            !has_image,
            "Put for nonexistent image should not place cells"
        );

        // An error response should be sent.
        let mut found_error = false;
        while let Ok(msg) = rx.try_recv() {
            if let PtyWrite::Write(bytes) = msg {
                let text = String::from_utf8_lossy(&bytes);
                if text.contains("ENOENT") {
                    found_error = true;
                }
            }
        }
        assert!(
            found_error,
            "Expected ENOENT error for Put with nonexistent image ID"
        );
    }

    #[test]
    fn kitty_put_quiet_2_suppresses_error_for_missing_image() {
        use freminal_common::buffer_states::kitty_graphics::{KittyAction, KittyControlData};

        let (mut handler, rx) = kitty_handler();

        // Put for nonexistent image with q=2 (suppress all responses).
        let put_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Put),
                image_id: Some(999),
                quiet: 2,
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(put_cmd);

        // With q=2, no error response should be sent.
        let has_response = rx.try_recv().is_ok();
        assert!(
            !has_response,
            "q=2 should suppress all responses including errors"
        );
    }

    #[test]
    fn kitty_put_default_action_is_transmit_not_put() {
        use freminal_common::buffer_states::kitty_graphics::{KittyAction, KittyControlData};

        // When `a=` is not specified, default is Transmit (not Put).
        // This means a command with no `a=` parameter and image data
        // should store but not display.
        let (mut handler, _rx) = kitty_handler();

        let cmd = kitty_rgba_2x2_cmd(KittyAction::Transmit);
        // Re-create with action=None to simulate missing `a=` parameter.
        let cmd_no_action = KittyGraphicsCommand {
            control: KittyControlData {
                action: None,
                ..cmd.control
            },
            payload: cmd.payload,
        };
        handler.handle_kitty_graphics(cmd_no_action);

        // Should be stored.
        assert!(handler.buffer().image_store().get(42).is_some());

        // Should NOT be placed.
        let has_image = handler
            .buffer()
            .get_rows()
            .iter()
            .any(|row| row.cells().iter().any(crate::cell::Cell::has_image));
        assert!(
            !has_image,
            "Default action (None → Transmit) should not place image cells"
        );
    }

    // ------------------------------------------------------------------
    // Additional coverage tests
    // ------------------------------------------------------------------

    #[test]
    fn kitty_animation_commands_not_supported() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat,
        };

        let (mut handler, _rx) = kitty_handler();

        for action in [
            KittyAction::AnimationFrame,
            KittyAction::AnimationControl,
            KittyAction::AnimationCompose,
        ] {
            let cmd = KittyGraphicsCommand {
                control: KittyControlData {
                    action: Some(action),
                    format: Some(KittyFormat::Rgba),
                    src_width: Some(2),
                    src_height: Some(2),
                    image_id: Some(100),
                    ..KittyControlData::default()
                },
                payload: vec![0; 16],
            };
            // Should not panic, just log a warning
            handler.handle_kitty_graphics(cmd);
        }
    }

    #[test]
    fn kitty_stale_chunked_transfer_discarded_on_new_command() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat,
        };

        let (mut handler, _rx) = kitty_handler();

        // Start a chunked transfer (more_data=true)
        let chunk1 = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Transmit),
                format: Some(KittyFormat::Rgba),
                src_width: Some(2),
                src_height: Some(2),
                image_id: Some(200),
                more_data: true,
                ..KittyControlData::default()
            },
            payload: vec![255, 0, 0, 255, 0, 255, 0, 255],
        };
        handler.handle_kitty_graphics(chunk1);
        assert!(
            handler.kitty_state.is_some(),
            "Chunked transfer should be in progress"
        );

        // Now send a NEW command with explicit action → discards the stale accumulator
        let new_cmd = kitty_rgba_2x2_cmd(KittyAction::TransmitAndDisplay);
        handler.handle_kitty_graphics(new_cmd);

        // Stale chunked transfer should be gone
        assert!(
            handler.kitty_state.is_none(),
            "Stale chunked transfer should be discarded"
        );
        // New image should be stored
        assert!(handler.buffer().image_store().get(42).is_some());
    }

    #[test]
    fn kitty_query_unsupported_format_sends_error() {
        use freminal_common::buffer_states::kitty_graphics::{KittyAction, KittyControlData};

        let (mut handler, rx) = kitty_handler();

        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Query),
                // Use a non-standard format value that's not RGB/RGBA/PNG
                // There is no "unsupported" format enum value we can use,
                // so we test with format=None which is supported. Instead
                // let's test quiet suppression on queries.
                format: None,
                image_id: Some(1),
                quiet: 1, // suppress OK response
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(cmd);

        // quiet=1 with a supported format → OK suppressed
        assert!(
            rx.try_recv().is_err(),
            "quiet=1 should suppress OK response for supported query"
        );
    }

    #[test]
    fn kitty_query_quiet_2_suppresses_all() {
        use freminal_common::buffer_states::kitty_graphics::{KittyAction, KittyControlData};

        let (mut handler, rx) = kitty_handler();

        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Query),
                format: None,
                image_id: Some(1),
                quiet: 2, // suppress all responses
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(cmd);

        assert!(
            rx.try_recv().is_err(),
            "quiet=2 should suppress all query responses"
        );
    }

    #[test]
    fn kitty_chunk_with_no_active_transfer_ignored() {
        use freminal_common::buffer_states::kitty_graphics::{KittyControlData, KittyFormat};

        let (mut handler, _rx) = kitty_handler();

        // Send a continuation chunk (no explicit action) with no active transfer
        let chunk = KittyGraphicsCommand {
            control: KittyControlData {
                action: None, // continuation chunk
                format: Some(KittyFormat::Rgba),
                more_data: true,
                ..KittyControlData::default()
            },
            payload: vec![0; 8],
        };
        // Should not panic or crash
        handler.handle_kitty_graphics(chunk);
    }

    #[test]
    fn kitty_chunked_transfer_final_chunk_completes() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat,
        };

        let (mut handler, _rx) = kitty_handler();

        // First chunk (more_data=true)
        let chunk1 = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Rgba),
                src_width: Some(2),
                src_height: Some(2),
                image_id: Some(300),
                more_data: true,
                ..KittyControlData::default()
            },
            payload: vec![255, 0, 0, 255, 0, 255, 0, 255],
        };
        handler.handle_kitty_graphics(chunk1);
        assert!(handler.kitty_state.is_some());

        // Second chunk (continuation, more_data=true)
        let chunk2 = KittyGraphicsCommand {
            control: KittyControlData {
                action: None, // continuation
                more_data: true,
                ..KittyControlData::default()
            },
            payload: vec![0, 0, 255, 255, 255, 255, 0, 255],
        };
        handler.handle_kitty_graphics(chunk2);
        assert!(handler.kitty_state.is_some());

        // Final chunk (more_data=false)
        let chunk3 = KittyGraphicsCommand {
            control: KittyControlData {
                action: None,     // continuation
                more_data: false, // final
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(chunk3);
        assert!(
            handler.kitty_state.is_none(),
            "Chunked transfer should be complete"
        );
        assert!(handler.buffer().image_store().get(300).is_some());
    }

    #[test]
    fn kitty_rgb_payload_size_mismatch_sends_error() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat,
        };

        let (mut handler, rx) = kitty_handler();

        // Says 2x2 RGB (expects 12 bytes) but payload is only 6 bytes.
        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Rgb),
                src_width: Some(2),
                src_height: Some(2),
                image_id: Some(400),
                ..KittyControlData::default()
            },
            payload: vec![0; 6], // too small for 2x2 RGB
        };

        handler.handle_kitty_graphics(cmd);

        assert!(handler.buffer().image_store().get(400).is_none());

        let response = rx.try_recv().unwrap();
        match response {
            PtyWrite::Write(bytes) => {
                let s = String::from_utf8_lossy(&bytes);
                assert!(s.contains("EINVAL"), "Expected EINVAL error, got: {s}");
            }
            PtyWrite::Resize(_) => panic!("Expected PtyWrite::Write"),
        }
    }

    #[test]
    fn kitty_png_decode_failure_sends_error() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat,
        };

        let (mut handler, rx) = kitty_handler();

        // Invalid PNG data
        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Png),
                image_id: Some(500),
                ..KittyControlData::default()
            },
            payload: vec![0xFF, 0xFF, 0xFF, 0xFF], // not valid PNG
        };

        handler.handle_kitty_graphics(cmd);

        let response = rx.try_recv().unwrap();
        match response {
            PtyWrite::Write(bytes) => {
                let s = String::from_utf8_lossy(&bytes);
                assert!(s.contains("EINVAL"), "Expected EINVAL error, got: {s}");
            }
            PtyWrite::Resize(_) => panic!("Expected PtyWrite::Write"),
        }
    }

    #[test]
    fn kitty_delete_by_number() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyDeleteTarget,
        };

        let (mut handler, _rx) = kitty_handler();

        // Place an image
        let cmd = kitty_rgba_2x2_cmd(KittyAction::TransmitAndDisplay);
        handler.handle_kitty_graphics(cmd);

        // Delete by number (this exercises the ByNumber arm)
        let delete_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Delete),
                delete_target: Some(KittyDeleteTarget::ByNumber),
                image_number: Some(1),
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(delete_cmd);
    }

    #[test]
    fn kitty_delete_at_cursor() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyDeleteTarget,
        };

        let (mut handler, _rx) = kitty_handler();

        let cmd = kitty_rgba_2x2_cmd(KittyAction::TransmitAndDisplay);
        handler.handle_kitty_graphics(cmd);

        let delete_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Delete),
                delete_target: Some(KittyDeleteTarget::AtCursor),
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(delete_cmd);
    }

    #[test]
    fn kitty_delete_at_cursor_and_after() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyDeleteTarget,
        };

        let (mut handler, _rx) = kitty_handler();

        let cmd = kitty_rgba_2x2_cmd(KittyAction::TransmitAndDisplay);
        handler.handle_kitty_graphics(cmd);

        let delete_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Delete),
                delete_target: Some(KittyDeleteTarget::AtCursorAndAfter),
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(delete_cmd);
    }

    #[test]
    fn kitty_delete_at_cell_range() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyDeleteTarget,
        };

        let (mut handler, _rx) = kitty_handler();

        let cmd = kitty_rgba_2x2_cmd(KittyAction::TransmitAndDisplay);
        handler.handle_kitty_graphics(cmd);

        // Delete at specific cell
        let delete_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Delete),
                delete_target: Some(KittyDeleteTarget::AtCellRange),
                src_x: Some(0),
                src_y: Some(0),
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(delete_cmd);
    }

    #[test]
    fn kitty_delete_at_cell_range_and_after() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyDeleteTarget,
        };

        let (mut handler, _rx) = kitty_handler();

        let cmd = kitty_rgba_2x2_cmd(KittyAction::TransmitAndDisplay);
        handler.handle_kitty_graphics(cmd);

        let delete_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Delete),
                delete_target: Some(KittyDeleteTarget::AtCellRangeAndAfter),
                src_x: Some(0),
                src_y: Some(0),
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(delete_cmd);
    }

    #[test]
    fn kitty_delete_in_column_range() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyDeleteTarget,
        };

        let (mut handler, _rx) = kitty_handler();

        let cmd = kitty_rgba_2x2_cmd(KittyAction::TransmitAndDisplay);
        handler.handle_kitty_graphics(cmd);

        let delete_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Delete),
                delete_target: Some(KittyDeleteTarget::InColumnRange),
                src_x: Some(0),
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(delete_cmd);
    }

    #[test]
    fn kitty_delete_in_row_range() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyDeleteTarget,
        };

        let (mut handler, _rx) = kitty_handler();

        let cmd = kitty_rgba_2x2_cmd(KittyAction::TransmitAndDisplay);
        handler.handle_kitty_graphics(cmd);

        let delete_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Delete),
                delete_target: Some(KittyDeleteTarget::InRowRange),
                src_y: Some(0),
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(delete_cmd);
    }

    #[test]
    fn kitty_delete_at_z_index() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyDeleteTarget,
        };

        let (mut handler, _rx) = kitty_handler();

        let cmd = kitty_rgba_2x2_cmd(KittyAction::TransmitAndDisplay);
        handler.handle_kitty_graphics(cmd);

        let delete_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Delete),
                delete_target: Some(KittyDeleteTarget::AtZIndex),
                z_index: Some(0),
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(delete_cmd);
    }

    #[test]
    fn kitty_delete_all_including_non_visible() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyDeleteTarget,
        };

        let (mut handler, _rx) = kitty_handler();

        let cmd = kitty_rgba_2x2_cmd(KittyAction::TransmitAndDisplay);
        handler.handle_kitty_graphics(cmd);
        assert!(handler.buffer().image_store().get(42).is_some());

        let delete_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Delete),
                delete_target: Some(KittyDeleteTarget::AllIncludingNonVisible),
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(delete_cmd);

        assert!(
            handler.buffer().image_store().get(42).is_none(),
            "Delete all including non-visible should remove all images"
        );
    }

    #[test]
    fn kitty_delete_by_number_cursor_or_after() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyDeleteTarget,
        };

        let (mut handler, _rx) = kitty_handler();

        let cmd = kitty_rgba_2x2_cmd(KittyAction::TransmitAndDisplay);
        handler.handle_kitty_graphics(cmd);

        let delete_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Delete),
                delete_target: Some(KittyDeleteTarget::ByNumberCursorOrAfter),
                image_number: Some(1),
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(delete_cmd);
    }

    #[test]
    fn kitty_put_missing_image_sends_enoent() {
        use freminal_common::buffer_states::kitty_graphics::{KittyAction, KittyControlData};

        let (mut handler, rx) = kitty_handler();

        // Try to put an image that doesn't exist
        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Put),
                image_id: Some(999),
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(cmd);

        let response = rx.try_recv().unwrap();
        match response {
            PtyWrite::Write(bytes) => {
                let s = String::from_utf8_lossy(&bytes);
                assert!(s.contains("ENOENT"), "Expected ENOENT error, got: {s}");
            }
            PtyWrite::Resize(_) => panic!("Expected PtyWrite::Write"),
        }
    }

    #[test]
    fn kitty_put_existing_image_places_it() {
        use freminal_common::buffer_states::kitty_graphics::{KittyAction, KittyControlData};

        let (mut handler, _rx) = kitty_handler();

        // First transmit (store only)
        let cmd = kitty_rgba_2x2_cmd(KittyAction::Transmit);
        handler.handle_kitty_graphics(cmd);
        assert!(handler.buffer().image_store().get(42).is_some());

        // Now put it
        let put_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Put),
                image_id: Some(42),
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(put_cmd);

        // Image cells should now be placed
        let has_image = handler.buffer().has_any_image_cell();
        assert!(
            has_image,
            "Put should place the previously transmitted image"
        );
    }

    #[test]
    fn kitty_delete_by_id_cursor_or_after() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyDeleteTarget,
        };

        let (mut handler, _rx) = kitty_handler();

        let cmd = kitty_rgba_2x2_cmd(KittyAction::TransmitAndDisplay);
        handler.handle_kitty_graphics(cmd);
        assert!(handler.buffer().image_store().get(42).is_some());

        let delete_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Delete),
                delete_target: Some(KittyDeleteTarget::ByIdCursorOrAfter),
                image_id: Some(42),
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(delete_cmd);

        assert!(
            handler.buffer().image_store().get(42).is_none(),
            "Delete by ID should remove the image"
        );
    }

    #[test]
    fn kitty_delete_in_column_range_and_after() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyDeleteTarget,
        };

        let (mut handler, _rx) = kitty_handler();

        let cmd = kitty_rgba_2x2_cmd(KittyAction::TransmitAndDisplay);
        handler.handle_kitty_graphics(cmd);

        let delete_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Delete),
                delete_target: Some(KittyDeleteTarget::InColumnRangeAndAfter),
                src_x: Some(0),
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(delete_cmd);
    }

    #[test]
    fn kitty_delete_in_row_range_and_after() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyDeleteTarget,
        };

        let (mut handler, _rx) = kitty_handler();

        let cmd = kitty_rgba_2x2_cmd(KittyAction::TransmitAndDisplay);
        handler.handle_kitty_graphics(cmd);

        let delete_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Delete),
                delete_target: Some(KittyDeleteTarget::InRowRangeAndAfter),
                src_y: Some(0),
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(delete_cmd);
    }

    #[test]
    fn kitty_delete_at_z_index_and_after() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyDeleteTarget,
        };

        let (mut handler, _rx) = kitty_handler();

        let cmd = kitty_rgba_2x2_cmd(KittyAction::TransmitAndDisplay);
        handler.handle_kitty_graphics(cmd);

        let delete_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Delete),
                delete_target: Some(KittyDeleteTarget::AtZIndexAndAfter),
                z_index: Some(0),
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(delete_cmd);
    }

    #[test]
    fn kitty_place_image_zero_dimension() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat,
        };

        let (mut handler, rx) = kitty_handler();

        // Transmit with width=0 (zero dimension) — should get error
        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Rgba),
                src_width: Some(0),
                src_height: Some(2),
                image_id: Some(600),
                ..KittyControlData::default()
            },
            payload: Vec::new(), // Empty payload triggers ENODATA first
        };
        handler.handle_kitty_graphics(cmd);

        // Check we got an error
        let response = rx.try_recv().unwrap();
        match response {
            PtyWrite::Write(bytes) => {
                let s = String::from_utf8_lossy(&bytes);
                assert!(
                    s.contains("ENODATA") || s.contains("EINVAL"),
                    "Expected error response, got: {s}"
                );
            }
            PtyWrite::Resize(_) => panic!("Expected PtyWrite::Write"),
        }
    }

    #[test]
    fn kitty_missing_height_dimension_sends_error() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat,
        };

        let (mut handler, rx) = kitty_handler();

        // Has width but missing height
        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Rgba),
                src_width: Some(2),
                src_height: None, // missing height
                image_id: Some(601),
                ..KittyControlData::default()
            },
            payload: vec![0; 16],
        };
        handler.handle_kitty_graphics(cmd);

        let response = rx.try_recv().unwrap();
        match response {
            PtyWrite::Write(bytes) => {
                let s = String::from_utf8_lossy(&bytes);
                assert!(
                    s.contains("EINVAL"),
                    "Expected EINVAL error for missing height, got: {s}"
                );
            }
            PtyWrite::Resize(_) => panic!("Expected PtyWrite::Write"),
        }
    }

    // ── Coverage-gap tests ──────────────────────────────────────────────

    #[test]
    fn kitty_zero_width_rgba_sends_enodata_for_empty_payload() {
        // RGBA with width=0 requires empty payload to match expected=0 bytes,
        // but empty payload triggers ENODATA before decode. This test verifies
        // the ENODATA error path.
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat,
        };

        let (mut handler, rx) = kitty_handler();

        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Rgba),
                src_width: Some(0),
                src_height: Some(2),
                image_id: Some(700),
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(cmd);

        let response = rx.try_recv().unwrap();
        match response {
            PtyWrite::Write(bytes) => {
                let s = String::from_utf8_lossy(&bytes);
                assert!(s.contains("ENODATA"), "Expected ENODATA error, got: {s}");
            }
            PtyWrite::Resize(_) => panic!("Expected PtyWrite::Write"),
        }
    }

    #[test]
    fn kitty_zero_width_rgba_with_payload_sends_size_mismatch() {
        // RGBA with width=0 and non-empty payload triggers size mismatch.
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat,
        };

        let (mut handler, rx) = kitty_handler();

        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Rgba),
                src_width: Some(0),
                src_height: Some(2),
                image_id: Some(701),
                ..KittyControlData::default()
            },
            payload: vec![255; 4], // non-empty but won't match expected=0
        };
        handler.handle_kitty_graphics(cmd);

        let response = rx.try_recv().unwrap();
        match response {
            PtyWrite::Write(bytes) => {
                let s = String::from_utf8_lossy(&bytes);
                assert!(
                    s.contains("EINVAL"),
                    "Expected EINVAL size mismatch error, got: {s}"
                );
            }
            PtyWrite::Resize(_) => panic!("Expected PtyWrite::Write"),
        }
    }

    #[test]
    fn kitty_transmit_and_display_with_unicode_placeholder() {
        // TransmitAndDisplay with unicode_placeholder=true should create a
        // VirtualPlacement instead of placing the image in the buffer.
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat,
        };

        let (mut handler, _rx) = kitty_handler();

        // 2x2 RGBA = 16 bytes
        let rgba_data: Vec<u8> = vec![255; 16];
        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Rgba),
                src_width: Some(2),
                src_height: Some(2),
                image_id: Some(800),
                unicode_placeholder: true,
                placement_id: Some(5),
                ..KittyControlData::default()
            },
            payload: rgba_data,
        };
        handler.handle_kitty_graphics(cmd);

        // Should have a virtual placement registered
        assert!(
            !handler.virtual_placements.is_empty(),
            "Expected virtual placement to be created"
        );
    }

    #[test]
    fn kitty_put_with_unicode_placeholder_creates_virtual_placement() {
        // First transmit an image, then Put with unicode_placeholder=true.
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat,
        };

        let (mut handler, _rx) = kitty_handler();

        // Step 1: Transmit image (a=t)
        let rgba_data: Vec<u8> = vec![255; 16]; // 2x2 RGBA
        let transmit_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Transmit),
                format: Some(KittyFormat::Rgba),
                src_width: Some(2),
                src_height: Some(2),
                image_id: Some(810),
                ..KittyControlData::default()
            },
            payload: rgba_data,
        };
        handler.handle_kitty_graphics(transmit_cmd);

        // Step 2: Put with unicode_placeholder
        let put_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Put),
                image_id: Some(810),
                unicode_placeholder: true,
                placement_id: Some(3),
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(put_cmd);

        assert!(
            !handler.virtual_placements.is_empty(),
            "Expected virtual placement from Put"
        );
    }

    #[test]
    fn kitty_put_with_no_cursor_movement_preserves_position() {
        // Put with C=1 (no_cursor_movement) should restore cursor position after placement.
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat,
        };

        let (mut handler, _rx) = kitty_handler();

        // Step 1: Transmit image
        let rgba_data: Vec<u8> = vec![255; 16]; // 2x2 RGBA
        let transmit_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Transmit),
                format: Some(KittyFormat::Rgba),
                src_width: Some(2),
                src_height: Some(2),
                image_id: Some(820),
                ..KittyControlData::default()
            },
            payload: rgba_data,
        };
        handler.handle_kitty_graphics(transmit_cmd);

        // Record cursor before Put
        let cursor_before = handler.buffer.get_cursor().pos;

        // Step 2: Put with C=1
        let put_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Put),
                image_id: Some(820),
                no_cursor_movement: true,
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(put_cmd);

        // Cursor should be restored to original position
        let cursor_after = handler.buffer.get_cursor().pos;
        assert_eq!(
            cursor_before, cursor_after,
            "Cursor should not move with C=1"
        );
    }

    #[test]
    fn kitty_transmit_and_display_with_no_cursor_movement() {
        // TransmitAndDisplay with C=1 should place image but keep cursor at original position.
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat,
        };

        let (mut handler, _rx) = kitty_handler();

        let cursor_before = handler.buffer.get_cursor().pos;

        let rgba_data: Vec<u8> = vec![255; 16]; // 2x2 RGBA
        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Rgba),
                src_width: Some(2),
                src_height: Some(2),
                image_id: Some(830),
                no_cursor_movement: true,
                ..KittyControlData::default()
            },
            payload: rgba_data,
        };
        handler.handle_kitty_graphics(cmd);

        let cursor_after = handler.buffer.get_cursor().pos;
        assert_eq!(
            cursor_before, cursor_after,
            "Cursor should not move with C=1 on TransmitAndDisplay"
        );
    }

    #[test]
    fn kitty_chunked_transfer_new_command_discards_stale_state() {
        // Start a chunked transfer, then send a new command with a=t.
        // The stale chunk state should be discarded.
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat,
        };

        let (mut handler, _rx) = kitty_handler();

        // Start a chunked transfer (more_data=true)
        let chunk1 = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Transmit),
                format: Some(KittyFormat::Rgba),
                src_width: Some(2),
                src_height: Some(2),
                image_id: Some(900),
                more_data: true,
                ..KittyControlData::default()
            },
            payload: vec![255; 8], // partial chunk
        };
        handler.handle_kitty_graphics(chunk1);
        assert!(
            handler.kitty_state.is_some(),
            "Should have in-progress chunked state"
        );

        // Send a brand-new command with explicit action → discards stale state
        let new_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Rgba),
                src_width: Some(2),
                src_height: Some(2),
                image_id: Some(901),
                ..KittyControlData::default()
            },
            payload: vec![255; 16],
        };
        handler.handle_kitty_graphics(new_cmd);
        assert!(
            handler.kitty_state.is_none(),
            "Stale state should have been discarded"
        );
    }

    #[test]
    fn kitty_query_quiet_2_suppresses_all_responses() {
        // a=q with quiet=2 should produce no response at all.
        use freminal_common::buffer_states::kitty_graphics::{KittyAction, KittyControlData};

        let (mut handler, rx) = kitty_handler();

        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Query),
                quiet: 2,
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(cmd);

        // No response should be sent
        assert!(
            rx.try_recv().is_err(),
            "quiet=2 should suppress all responses"
        );
    }

    #[test]
    fn kitty_query_quiet_1_suppresses_ok_response() {
        // a=q with quiet=1 should suppress OK (supported format) but still send errors.
        use freminal_common::buffer_states::kitty_graphics::{KittyAction, KittyControlData};

        let (mut handler, rx) = kitty_handler();

        // Default format (None → supported) with quiet=1
        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Query),
                quiet: 1,
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(cmd);

        assert!(
            rx.try_recv().is_err(),
            "quiet=1 should suppress OK for supported format"
        );
    }
}
