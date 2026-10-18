// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Sixel graphics decoder.
//!
//! Sixel is a bitmap graphics format used in DCS sequences:
//! `ESC P P1;P2;P3 q <sixel-data> ESC \`
//!
//! Each data byte in `?`..`~` (0x3F..0x7E) encodes a vertical column of
//! 6 pixels. Control characters within the data stream manage the palette,
//! cursor position, and repetition.
//!
//! Reference: <https://vt100.net/docs/vt3xx-gp/chapter14.html>

use conv2::ValueFrom;

/// Maximum number of palette entries supported.
pub const MAX_PALETTE: usize = 256;

/// Maximum width or height, in pixels, of a decoded Sixel image.
///
/// Matches kitty's `MAX_IMAGE_DIMENSION` (`kitty/graphics.c` line 28, 10000).
/// Raster-attribute declarations beyond this are ignored; data that would
/// grow the canvas beyond it is clipped.
pub const MAX_SIXEL_DIMENSION: usize = 10_000;

/// Maximum total pixel count of a decoded Sixel image.
///
/// 16,777,216 pixels is 64 MiB of RGBA, which fits a full 4K screen and is
/// one fifth of freminal's 320 MiB kitty image-store base quota
/// (`KITTY_IMAGE_BASE_QUOTA_BYTES` in `freminal-buffer`, which
/// `freminal-common` cannot depend on, hence this named constant).
pub const MAX_SIXEL_PIXELS: usize = 4096 * 4096;

/// Smallest allocated canvas dimension, in pixels, when the decoder
/// (re)allocates.  Avoids a burst of tiny reallocations while an image is
/// still a few columns or bands in; the allocation is zeroed lazily by the
/// allocator, so the slack costs address space, not work.
const MIN_ALLOC_DIMENSION: usize = 64;

/// Return a freshly initialised VT340-compatible default palette.
///
/// Indices 0–15 are set to the standard VT340 default colours; higher indices
/// default to black.  This is the same palette that `SixelDecoder::new()`
/// starts with.
#[must_use]
pub const fn default_sixel_palette() -> [(u8, u8, u8); MAX_PALETTE] {
    let mut palette = [(0u8, 0u8, 0u8); MAX_PALETTE];
    let mut i = 0;
    while i < DEFAULT_PALETTE_16.len() {
        palette[i] = DEFAULT_PALETTE_16[i];
        i += 1;
    }
    palette
}

/// Default palette (VT340-compatible 16 colors).
///
/// These are the standard VT340 default palette entries.  Indices 0-15 are
/// defined; higher indices default to black until explicitly set.
const DEFAULT_PALETTE_16: [(u8, u8, u8); 16] = [
    (0, 0, 0),       // 0: black
    (51, 51, 204),   // 1: blue
    (204, 33, 33),   // 2: red
    (51, 204, 51),   // 3: green
    (204, 51, 204),  // 4: magenta
    (51, 204, 204),  // 5: cyan
    (204, 204, 51),  // 6: yellow
    (135, 135, 135), // 7: grey 53%
    (68, 68, 68),    // 8: grey 27%
    (85, 85, 255),   // 9: light blue
    (255, 85, 85),   // 10: light red
    (85, 255, 85),   // 11: light green
    (255, 85, 255),  // 12: light magenta
    (85, 255, 255),  // 13: light cyan
    (255, 255, 85),  // 14: light yellow
    (255, 255, 255), // 15: white
];

/// Background handling mode from P2 parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SixelBackground {
    /// P2=0 or P2=2: the background of the image is painted with palette
    /// colour 0 before any sixel data is applied.
    Paint,
    /// P2=1: the background is transparent (left unchanged).
    Transparent,
}

impl SixelBackground {
    /// Parse the P2 parameter value.
    #[must_use]
    pub const fn from_param(p2: u32) -> Self {
        match p2 {
            1 => Self::Transparent,
            _ => Self::Paint,
        }
    }
}

/// Result of decoding a Sixel image.
#[derive(Debug, Clone)]
pub struct SixelImage {
    /// RGBA pixel data (4 bytes per pixel), row-major, top-to-bottom.
    pub pixels: Vec<u8>,
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
}

/// Parsed DCS Sixel parameters (`P1;P2;P3`).
#[cfg_attr(not(test), allow(dead_code))]
struct DcsParams {
    p1: u32,
    p2: u32,
    p3: u32,
}

/// Parse DCS Sixel parameters (`P1;P2;P3`) from the data preceding `q`.
///
/// `params_bytes` should be the bytes between the DCS introducer and `q`
/// (exclusive).
fn parse_dcs_params(params_bytes: &[u8]) -> DcsParams {
    let mut params = [0u32; 3];
    let mut idx = 0;
    let mut acc: u32 = 0;
    let mut has_digit = false;

    for &byte in params_bytes {
        if byte.is_ascii_digit() {
            acc = acc
                .saturating_mul(10)
                .saturating_add(u32::from(byte - b'0'));
            has_digit = true;
        } else if byte == b';' {
            if has_digit && idx < 3 {
                params[idx] = acc;
            }
            idx += 1;
            acc = 0;
            has_digit = false;
        }
    }
    // Final param after last (or only) value.
    if has_digit && idx < 3 {
        params[idx] = acc;
    }

    DcsParams {
        p1: params[0],
        p2: params[1],
        p3: params[2],
    }
}

/// Convert HLS (Hue, Lightness, Saturation) to RGB.
///
/// All values are in the range 0..=100 as specified by the Sixel protocol.
/// Hue is in degrees 0..=360.
fn hls_to_rgb(hue: u32, lightness: u32, saturation: u32) -> (u8, u8, u8) {
    let light = f64::from(lightness.min(100)) / 100.0;
    let sat = f64::from(saturation.min(100)) / 100.0;

    if saturation == 0 {
        let grey = f64_to_u8(light * 255.0);
        return (grey, grey, grey);
    }

    let hue_sector = f64::from(hue % 360) / 60.0;

    let chroma = (1.0 - (2.0_f64.mul_add(light, -1.0)).abs()) * sat;
    let secondary = chroma * (1.0 - (hue_sector % 2.0 - 1.0).abs());
    let match_val = light - chroma / 2.0;

    let (rc, gc, bc) = if hue_sector < 1.0 {
        (chroma, secondary, 0.0)
    } else if hue_sector < 2.0 {
        (secondary, chroma, 0.0)
    } else if hue_sector < 3.0 {
        (0.0, chroma, secondary)
    } else if hue_sector < 4.0 {
        (0.0, secondary, chroma)
    } else if hue_sector < 5.0 {
        (secondary, 0.0, chroma)
    } else {
        (chroma, 0.0, secondary)
    };

    (
        f64_to_u8((rc + match_val).mul_add(255.0, 0.5)),
        f64_to_u8((gc + match_val).mul_add(255.0, 0.5)),
        f64_to_u8((bc + match_val).mul_add(255.0, 0.5)),
    )
}

/// Clamp and convert an `f64` to `u8`, saturating at 0 and 255.
// SAFETY: `clamp(0.0, 255.0)` guarantees the value is in [0.0, 255.0] before
// the cast, so neither truncation nor sign loss can occur.  `conv2` traits are
// not available in `const fn`, so the `as` cast is the only option here.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
const fn f64_to_u8(val: f64) -> u8 {
    val.clamp(0.0, 255.0) as u8
}

/// Convert a `u32` to `usize` without truncation risk on 32/64-bit platforms.
// SAFETY: Freminal targets 32-bit and 64-bit platforms only, where `usize` is
// always at least 32 bits wide, so a `u32 → usize` cast is lossless.
// `usize::from(u32)` is not available in stable Rust, and `conv2` traits cannot
// be used in `const fn`, so the `as` cast is the only option here.
#[allow(clippy::cast_possible_truncation)]
const fn usize_from_u32(val: u32) -> usize {
    val as usize
}

/// Convert RGB percentage values (0..=100) to byte values (0..=255).
fn pct_to_rgb(rp: u32, gp: u32, bp: u32) -> (u8, u8, u8) {
    let convert = |pct: u32| -> u8 {
        // pct.min(100) * 255 + 50 ≤ 100 * 255 + 50 = 25_550, which always fits in u8
        // after dividing by 100 (≤ 255).
        u8::value_from((pct.min(100) * 255 + 50) / 100).unwrap_or(0)
    };
    (convert(rp), convert(gp), convert(bp))
}

/// Internal state for the Sixel decoder.
struct SixelDecoder {
    /// Palette: 256 entries of (R, G, B).
    palette: [(u8, u8, u8); MAX_PALETTE],
    /// Currently selected palette index.
    current_color: usize,
    /// Pixel x position within the current band.
    x: usize,
    /// Current band index (each band is 6 pixels tall).
    band: usize,
    /// Logical image width in pixels (may grow as data arrives).  This is
    /// the extent that `set_pixel` bounds-checks against and that `finish`
    /// crops to.
    width: usize,
    /// Logical image height in pixels (may grow as bands are added).
    height: usize,
    /// Allocated canvas width in pixels; the row stride of `pixels`.
    /// Always `>= width`.  Grows geometrically so repeated one-column or
    /// one-band growth is amortised O(1) per cell instead of re-copying the
    /// whole image each time.
    cap_width: usize,
    /// Allocated canvas height in pixels.  Always `>= height`.
    cap_height: usize,
    /// Declared image width from raster attributes (0 = not set).
    declared_width: usize,
    /// Declared image height from raster attributes (0 = not set).
    declared_height: usize,
    /// Background mode.
    background: SixelBackground,
    /// RGBA pixel buffer, row-major, `cap_width * cap_height * 4` bytes.
    /// Cells outside the logical extent are never written and stay zero.
    pixels: Vec<u8>,
}

impl SixelDecoder {
    fn new(background: SixelBackground) -> Self {
        let mut palette = [(0u8, 0u8, 0u8); MAX_PALETTE];
        for (i, &(r, g, b)) in DEFAULT_PALETTE_16.iter().enumerate() {
            palette[i] = (r, g, b);
        }

        Self {
            palette,
            current_color: 0,
            x: 0,
            band: 0,
            width: 0,
            height: 0,
            cap_width: 0,
            cap_height: 0,
            declared_width: 0,
            declared_height: 0,
            background,
            pixels: Vec::new(),
        }
    }

    /// Create a decoder that starts with the given persistent palette.
    ///
    /// Used when `?1070` is reset (shared color registers): the caller provides
    /// the palette left over from the previous Sixel image.
    const fn with_palette(
        background: SixelBackground,
        palette: &[(u8, u8, u8); MAX_PALETTE],
    ) -> Self {
        Self {
            palette: *palette,
            current_color: 0,
            x: 0,
            band: 0,
            width: 0,
            height: 0,
            cap_width: 0,
            cap_height: 0,
            declared_width: 0,
            declared_height: 0,
            background,
            pixels: Vec::new(),
        }
    }

    /// Whether the decoder invariant holds:
    ///
    /// - the logical extent fits inside the allocation
    ///   (`width <= cap_width`, `height <= cap_height`);
    /// - the allocation is within [`MAX_SIXEL_DIMENSION`] per axis and
    ///   [`MAX_SIXEL_PIXELS`] in total (the logical extent is therefore too);
    /// - `pixels` is exactly `cap_width * cap_height * 4` bytes long.
    fn invariant_holds(&self) -> bool {
        self.width <= self.cap_width
            && self.height <= self.cap_height
            && self.cap_width <= MAX_SIXEL_DIMENSION
            && self.cap_height <= MAX_SIXEL_DIMENSION
            && self
                .cap_width
                .checked_mul(self.cap_height)
                .is_some_and(|n| {
                    n <= MAX_SIXEL_PIXELS && n.checked_mul(4) == Some(self.pixels.len())
                })
    }

    /// Fill the cells newly exposed by growing the logical extent from
    /// `old_w` x `old_h` to `new_w` x `new_h` with the background: `Paint`
    /// uses the *current* palette entry 0 (alpha 255), `Transparent` uses
    /// zeroes.  Cells inside the old extent are untouched.
    ///
    /// Done on every logical growth, reallocating or not, because
    /// over-allocated cells may hold an older fill.
    fn fill_exposed(&mut self, old_w: usize, old_h: usize, new_w: usize, new_h: usize) {
        let (r, g, b) = self.palette[0];
        let fill: [u8; 4] = match self.background {
            SixelBackground::Paint => [r, g, b, 255],
            SixelBackground::Transparent => [0; 4],
        };
        let stride = self.cap_width;
        let mut fill_span = |y: usize, x_start: usize, x_end: usize| {
            let row_base = y.saturating_mul(stride);
            let start = row_base.saturating_add(x_start).saturating_mul(4);
            let end = row_base.saturating_add(x_end).saturating_mul(4);
            if let Some(span) = self.pixels.get_mut(start..end) {
                for pixel in span.as_chunks_mut::<4>().0 {
                    *pixel = fill;
                }
            }
        };
        // Rows inside the old extent: only the columns to the right of it.
        if new_w > old_w {
            for y in 0..old_h {
                fill_span(y, old_w, new_w);
            }
        }
        // Rows below the old extent: exposed in full.
        for y in old_h..new_h {
            fill_span(y, 0, new_w);
        }
    }

    /// Reallocate the canvas to `new_cap_w` x `new_cap_h`, keeping only the
    /// logical region's rows.  The new allocation is zeroed; the caller
    /// fills newly exposed cells afterwards.
    fn reallocate(&mut self, new_cap_w: usize, new_cap_h: usize) -> bool {
        let Some(size) = new_cap_w
            .checked_mul(new_cap_h)
            .and_then(|n| n.checked_mul(4))
        else {
            return false;
        };
        let mut new_pixels = vec![0u8; size];

        if let Some(old_row_bytes) = self.cap_width.checked_mul(4).filter(|&n| n > 0)
            && let Some(new_row_bytes) = new_cap_w.checked_mul(4)
            && let Some(copy_bytes) = self.width.checked_mul(4)
        {
            for (old_row, new_row) in self
                .pixels
                .chunks_exact(old_row_bytes)
                .zip(new_pixels.chunks_exact_mut(new_row_bytes))
                .take(self.height)
            {
                if let (Some(src), Some(dst)) =
                    (old_row.get(..copy_bytes), new_row.get_mut(..copy_bytes))
                {
                    dst.copy_from_slice(src);
                }
            }
        }

        self.pixels = new_pixels;
        self.cap_width = new_cap_w;
        self.cap_height = new_cap_h;
        true
    }

    /// Grow the logical image extent towards the requested dimensions.
    ///
    /// The logical size is clamped by [`clamped_canvas_size`], so existing
    /// dimensions never shrink and growth beyond the caps is clipped.  If it
    /// no longer fits the allocation, the allocation grows geometrically
    /// (see [`grown_capacity`]).  Newly exposed cells are then filled with
    /// the background.
    fn ensure_size(&mut self, new_width: usize, new_height: usize) {
        if new_width <= self.width && new_height <= self.height {
            return;
        }

        let old_width = self.width;
        let old_height = self.height;
        let (final_width, final_height) =
            clamped_canvas_size(old_width, old_height, new_width, new_height);

        if final_width == old_width && final_height == old_height {
            return;
        }
        if final_width == 0 || final_height == 0 {
            return;
        }

        if final_width > self.cap_width || final_height > self.cap_height {
            let (cap_w, cap_h) =
                grown_capacity(self.cap_width, self.cap_height, final_width, final_height);
            if !self.reallocate(cap_w, cap_h) {
                return;
            }
        }

        self.fill_exposed(old_width, old_height, final_width, final_height);
        self.width = final_width;
        self.height = final_height;
        debug_assert!(self.invariant_holds());
    }

    /// Set a single pixel at (px, py) to the current palette colour.
    fn set_pixel(&mut self, px: usize, py: usize) {
        if px >= self.width || py >= self.height {
            return;
        }
        let (red, green, blue) = self.palette[self.current_color];
        let offset = (py * self.cap_width + px) * 4;
        if offset + 3 < self.pixels.len() {
            self.pixels[offset] = red;
            self.pixels[offset + 1] = green;
            self.pixels[offset + 2] = blue;
            self.pixels[offset + 3] = 255;
        }
    }

    /// Apply a single sixel data byte (encodes 6 vertical pixels).
    ///
    /// Only the columns that land inside the (capped) canvas are visited, so a
    /// huge `!` repeat count costs O(width), not O(count).
    fn apply_sixel(&mut self, sixel_value: u8, count: usize) {
        let band_y = self.band.saturating_mul(6);
        let needed_height = band_y.saturating_add(6);
        let needed_width = self.x.saturating_add(count);

        self.ensure_size(needed_width, needed_height);

        if band_y < self.height {
            let end = needed_width.min(self.width);
            for px in self.x..end {
                for bit in 0..6u8 {
                    if sixel_value & (1 << bit) != 0 {
                        self.set_pixel(px, band_y.saturating_add(usize::from(bit)));
                    }
                }
            }
        }

        self.x = needed_width;
    }

    /// Apply a raster-attributes size declaration (`Ph` x `Pv`).
    ///
    /// Like xterm, a declaration that is zero in either dimension, or that
    /// exceeds [`MAX_SIXEL_DIMENSION`] / [`MAX_SIXEL_PIXELS`], is ignored
    /// rather than allocated for.
    fn declare_raster(&mut self, ph: u32, pv: u32) {
        let width = usize_from_u32(ph);
        let height = usize_from_u32(pv);
        if ph > 0
            && pv > 0
            && width <= MAX_SIXEL_DIMENSION
            && height <= MAX_SIXEL_DIMENSION
            && width
                .checked_mul(height)
                .is_some_and(|n| n <= MAX_SIXEL_PIXELS)
        {
            self.declared_width = width;
            self.declared_height = height;
            self.ensure_size(width, height);
        }
    }

    /// Parse a numeric parameter from the data stream, returning `(value, bytes_consumed)`.
    fn parse_number(data: &[u8]) -> (u32, usize) {
        let mut val: u32 = 0;
        let mut len = 0;
        for &b in data {
            if b.is_ascii_digit() {
                val = val.saturating_mul(10).saturating_add(u32::from(b - b'0'));
                len += 1;
            } else {
                break;
            }
        }
        (val, len)
    }

    /// Decode the sixel data stream (everything after `q`).
    fn decode(&mut self, data: &[u8]) {
        let mut i = 0;
        let len = data.len();

        // If raster attributes declared dimensions, pre-allocate.
        // We'll check for them in the first pass of the data.

        while i < len {
            let b = data[i];
            match b {
                // Sixel data character: ? (0x3F) through ~ (0x7E)
                0x3F..=0x7E => {
                    let sixel_value = b - 0x3F;
                    self.apply_sixel(sixel_value, 1);
                    i += 1;
                }
                // `!` — repeat introducer: !<count><sixel-char>
                b'!' => {
                    i += 1;
                    let (count, consumed) = Self::parse_number(&data[i..]);
                    i += consumed;
                    if i < len {
                        let ch = data[i];
                        if (0x3F..=0x7E).contains(&ch) {
                            let sixel_value = ch - 0x3F;
                            let repeat = if count == 0 { 1 } else { usize_from_u32(count) };
                            self.apply_sixel(sixel_value, repeat);
                        }
                        i += 1;
                    }
                }
                // `"` — raster attributes: "Pan;Pad;Ph;Pv
                b'"' => {
                    i += 1;
                    // Pan (pixel aspect numerator)
                    let (_pan, c1) = Self::parse_number(&data[i..]);
                    i += c1;
                    if i < len && data[i] == b';' {
                        i += 1;
                    }
                    // Pad (pixel aspect denominator)
                    let (_pad, c2) = Self::parse_number(&data[i..]);
                    i += c2;
                    if i < len && data[i] == b';' {
                        i += 1;
                    }
                    // Ph (width)
                    let (ph, c3) = Self::parse_number(&data[i..]);
                    i += c3;
                    if i < len && data[i] == b';' {
                        i += 1;
                    }
                    // Pv (height)
                    let (pv, c4) = Self::parse_number(&data[i..]);
                    i += c4;

                    self.declare_raster(ph, pv);
                }
                // `#` — colour introducer
                b'#' => {
                    i += 1;
                    // Pc (palette index)
                    let (pc, c1) = Self::parse_number(&data[i..]);
                    i += c1;

                    if i < len && data[i] == b';' {
                        // Colour definition: #Pc;Pu;Px;Py;Pz
                        i += 1;
                        let (pu, c2) = Self::parse_number(&data[i..]);
                        i += c2;
                        if i < len && data[i] == b';' {
                            i += 1;
                        }
                        let (px, c3) = Self::parse_number(&data[i..]);
                        i += c3;
                        if i < len && data[i] == b';' {
                            i += 1;
                        }
                        let (py, c4) = Self::parse_number(&data[i..]);
                        i += c4;
                        if i < len && data[i] == b';' {
                            i += 1;
                        }
                        let (pz, c5) = Self::parse_number(&data[i..]);
                        i += c5;

                        let idx = usize_from_u32(pc).min(MAX_PALETTE - 1);
                        self.palette[idx] = if pu == 1 {
                            hls_to_rgb(px, py, pz)
                        } else {
                            // pu == 2 (RGB) or default to RGB
                            pct_to_rgb(px, py, pz)
                        };
                        self.current_color = idx;
                    } else {
                        // Colour selection only: #Pc
                        self.current_color = usize_from_u32(pc).min(MAX_PALETTE - 1);
                    }
                }
                // `$` — graphics carriage return (x = 0, same band)
                b'$' => {
                    self.x = 0;
                    i += 1;
                }
                // `-` — graphics new line (x = 0, next band)
                b'-' => {
                    self.x = 0;
                    self.band = self.band.saturating_add(1);
                    i += 1;
                }
                // Skip any other bytes (whitespace, etc.)
                _ => {
                    i += 1;
                }
            }
        }
    }

    /// Crop the allocation to the logical extent, leaving a tight
    /// `width * height * 4` buffer whose stride is `width`.
    fn crop_to_logical(&mut self) {
        if let Some(old_row_bytes) = self.cap_width.checked_mul(4).filter(|&n| n > 0)
            && let Some(row_bytes) = self.width.checked_mul(4)
        {
            if self.cap_width == self.width {
                // Rows are already contiguous; just drop the spare rows.
                self.pixels.truncate(row_bytes.saturating_mul(self.height));
            } else {
                let mut tight = Vec::with_capacity(row_bytes.saturating_mul(self.height));
                for row in self.pixels.chunks_exact(old_row_bytes).take(self.height) {
                    if let Some(head) = row.get(..row_bytes) {
                        tight.extend_from_slice(head);
                    }
                }
                self.pixels = tight;
            }
        }
        self.cap_width = self.width;
        self.cap_height = self.height;
    }

    /// Trim the (already cropped, tight) image to the raster-attribute
    /// dimensions when the declared size is smaller than the image.
    ///
    /// Requires `cap_width == width`, `cap_height == height` (see
    /// [`Self::crop_to_logical`]) and keeps that true.
    fn trim_to_declared(&mut self) {
        // Trim height to declared height if raster attributes specified it and
        // actual content didn't exceed it.
        if self.declared_height > 0
            && self.declared_height < self.height
            && let Some(trimmed_size) = self
                .width
                .checked_mul(self.declared_height)
                .and_then(|n| n.checked_mul(4))
        {
            self.pixels.truncate(trimmed_size);
            self.height = self.declared_height;
        }

        // Trim width if declared and narrower than buffer.
        if self.declared_width > 0
            && self.declared_width < self.width
            && let Some(old_row_bytes) = self.width.checked_mul(4)
            && let Some(new_row_bytes) = self.declared_width.checked_mul(4)
        {
            let mut trimmed = Vec::with_capacity(new_row_bytes.saturating_mul(self.height));
            for row in self.pixels.chunks_exact(old_row_bytes) {
                if let Some(head) = row.get(..new_row_bytes) {
                    trimmed.extend_from_slice(head);
                }
            }
            self.pixels = trimmed;
            self.width = self.declared_width;
        }

        self.cap_width = self.width;
        self.cap_height = self.height;
    }

    /// Finalise and return the decoded image.
    ///
    /// If declared dimensions are larger than actual content, the buffer was
    /// already sized to the declared dimensions.  If actual content exceeded
    /// them, the buffer grew dynamically (within the caps).  Either way the
    /// final dimensions are `self.width` x `self.height` after trimming.
    fn finish(mut self) -> Option<SixelImage> {
        if self.width == 0 || self.height == 0 {
            return None;
        }

        self.crop_to_logical();
        self.trim_to_declared();
        debug_assert!(self.invariant_holds());

        Some(SixelImage {
            pixels: self.pixels,
            width: u32::value_from(self.width).ok()?,
            height: u32::value_from(self.height).ok()?,
        })
    }

    /// Finalise and return the decoded image together with the final palette.
    ///
    /// Used when `?1070` is reset (shared color registers): the caller can
    /// persist the returned palette and pass it to the next image via
    /// `parse_sixel_with_shared_palette`.
    fn finish_into_parts(self) -> (Option<SixelImage>, [(u8, u8, u8); MAX_PALETTE]) {
        let palette = self.palette;
        (self.finish(), palette)
    }
}

/// Choose the allocation size for a logical extent of `width` x `height`
/// that no longer fits the current `cap_width` x `cap_height` allocation.
///
/// Each axis that does not fit grows to
/// `max(needed, 2 * old_capacity, MIN_ALLOC_DIMENSION)`, so a sequence of
/// small growths reallocates only O(log n) times.  The result is then clamped
/// to [`MAX_SIXEL_DIMENSION`] per axis and reduced, never below the logical
/// extent, until `cap_width * cap_height <= MAX_SIXEL_PIXELS`.  The logical
/// extent always satisfies the pixel cap (see [`clamped_canvas_size`]), so
/// the reduction can always succeed: only over-allocation is trimmed, never
/// the logical size.
fn grown_capacity(
    cap_width: usize,
    cap_height: usize,
    width: usize,
    height: usize,
) -> (usize, usize) {
    let grow = |cap: usize, needed: usize| -> usize {
        if needed <= cap {
            cap
        } else {
            needed
                .max(cap.saturating_mul(2))
                .clamp(MIN_ALLOC_DIMENSION, MAX_SIXEL_DIMENSION)
        }
    };
    let mut cap_w = grow(cap_width, width).max(width);
    let mut cap_h = grow(cap_height, height).max(height);

    // Both are `<= MAX_SIXEL_DIMENSION` here, so the product cannot overflow.
    if cap_w * cap_h > MAX_SIXEL_PIXELS && cap_h > 0 {
        cap_w = cap_w.min((MAX_SIXEL_PIXELS / cap_h).max(width));
        if cap_w * cap_h > MAX_SIXEL_PIXELS && cap_w > 0 {
            cap_h = cap_h.min((MAX_SIXEL_PIXELS / cap_w).max(height));
        }
    }
    (cap_w, cap_h)
}

/// Compute the canvas size to grow to when `want_width` x `want_height` is
/// requested and the canvas is currently `cur_width` x `cur_height`.
///
/// Assuming the current size already satisfies the invariant
/// (`<= MAX_SIXEL_DIMENSION` each, product `<= MAX_SIXEL_PIXELS`), the result:
///
/// 1. never shrinks either current dimension;
/// 2. is clamped to [`MAX_SIXEL_DIMENSION`] in each dimension;
/// 3. if the product still exceeds [`MAX_SIXEL_PIXELS`], first limits the width
///    (when it is growing) to `MAX_SIXEL_PIXELS / height`, then, if that was
///    not enough, limits the height (when it is growing) to
///    `MAX_SIXEL_PIXELS / width`.  Neither limit goes below the current
///    dimension, so if neither can grow the current size is kept.
const fn clamped_canvas_size(
    cur_width: usize,
    cur_height: usize,
    want_width: usize,
    want_height: usize,
) -> (usize, usize) {
    let mut width = if want_width > cur_width {
        want_width
    } else {
        cur_width
    };
    let mut height = if want_height > cur_height {
        want_height
    } else {
        cur_height
    };
    if width > MAX_SIXEL_DIMENSION {
        width = MAX_SIXEL_DIMENSION;
    }
    if height > MAX_SIXEL_DIMENSION {
        height = MAX_SIXEL_DIMENSION;
    }

    // Both are now <= 10_000, so the product cannot overflow.
    if width * height > MAX_SIXEL_PIXELS {
        if width > cur_width {
            // `height > 0` because the product exceeds the cap.
            let limit = MAX_SIXEL_PIXELS / height;
            width = if limit > cur_width { limit } else { cur_width };
        }
        if width * height > MAX_SIXEL_PIXELS && height > cur_height {
            // `width > 0` because the product exceeds the cap.
            let limit = MAX_SIXEL_PIXELS / width;
            height = if limit > cur_height {
                limit
            } else {
                cur_height
            };
        }
    }

    (width, height)
}

/// Parse Sixel data using a shared (persistent) palette.
///
/// Used when `?1070` is reset (shared color registers).  The `palette` argument
/// is the palette left over from the previous Sixel image; it will be updated
/// with any colour-definition commands in `inner` and returned to the caller
/// so it can be persisted for the next image.
///
/// Returns the decoded image (or `None` for empty/invalid data) together with
/// the updated palette.
#[must_use]
pub fn parse_sixel_with_shared_palette(
    inner: &[u8],
    palette: [(u8, u8, u8); MAX_PALETTE],
) -> (Option<SixelImage>, [(u8, u8, u8); MAX_PALETTE]) {
    let Some(q_pos) = inner.iter().position(|&b| b == b'q') else {
        return (None, palette);
    };

    let params_bytes = &inner[..q_pos];
    let sixel_data = &inner[q_pos + 1..];

    let params = parse_dcs_params(params_bytes);
    let background = SixelBackground::from_param(params.p2);

    let mut decoder = SixelDecoder::with_palette(background, &palette);
    decoder.decode(sixel_data);
    decoder.finish_into_parts()
}

/// Parse Sixel data from the inner content of a DCS sequence.
///
/// `inner` should be the bytes after stripping the DCS envelope (leading `P`
/// and trailing `ESC \`), i.e. `P1;P2;P3 q <sixel-data>`.
///
/// Returns `None` if the data is empty, contains no `q` introducer, or
/// produces a zero-dimension image.
#[must_use]
pub fn parse_sixel(inner: &[u8]) -> Option<SixelImage> {
    // Find the `q` that separates DCS params from sixel data.
    let q_pos = inner.iter().position(|&b| b == b'q')?;

    let params_bytes = &inner[..q_pos];
    let sixel_data = &inner[q_pos + 1..];

    let params = parse_dcs_params(params_bytes);
    let background = SixelBackground::from_param(params.p2);

    let mut decoder = SixelDecoder::new(background);
    decoder.decode(sixel_data);
    decoder.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_dcs_params_full() {
        let p = parse_dcs_params(b"0;1;0");
        assert_eq!((p.p1, p.p2, p.p3), (0, 1, 0));
        let p = parse_dcs_params(b"9;0;2");
        assert_eq!((p.p1, p.p2, p.p3), (9, 0, 2));
    }

    #[test]
    fn test_parse_dcs_params_empty() {
        let p = parse_dcs_params(b"");
        assert_eq!((p.p1, p.p2, p.p3), (0, 0, 0));
    }

    #[test]
    fn test_parse_dcs_params_partial() {
        let p = parse_dcs_params(b"5");
        assert_eq!((p.p1, p.p2, p.p3), (5, 0, 0));
        let p = parse_dcs_params(b"5;3");
        assert_eq!((p.p1, p.p2, p.p3), (5, 3, 0));
    }

    #[test]
    fn test_sixel_background_from_param() {
        assert_eq!(SixelBackground::from_param(0), SixelBackground::Paint);
        assert_eq!(SixelBackground::from_param(1), SixelBackground::Transparent);
        assert_eq!(SixelBackground::from_param(2), SixelBackground::Paint);
        assert_eq!(SixelBackground::from_param(99), SixelBackground::Paint);
    }

    #[test]
    fn test_hls_to_rgb_achromatic() {
        // Saturation 0 → greyscale
        let (r, g, b) = hls_to_rgb(0, 50, 0);
        assert_eq!(r, g);
        assert_eq!(g, b);
        assert!((i16::from(r) - 128).unsigned_abs() < 3);
    }

    #[test]
    fn test_hls_to_rgb_red() {
        // Hue 0, full saturation, 50% lightness → red
        let (r, g, b) = hls_to_rgb(0, 50, 100);
        assert!(r > 200, "r={r} should be near 255");
        assert!(g < 30, "g={g} should be near 0");
        assert!(b < 30, "b={b} should be near 0");
    }

    #[test]
    fn test_hls_to_rgb_green() {
        let (r, g, b) = hls_to_rgb(120, 50, 100);
        assert!(r < 30, "r={r}");
        assert!(g > 200, "g={g}");
        assert!(b < 30, "b={b}");
    }

    #[test]
    fn test_hls_to_rgb_blue() {
        let (r, g, b) = hls_to_rgb(240, 50, 100);
        assert!(r < 30, "r={r}");
        assert!(g < 30, "g={g}");
        assert!(b > 200, "b={b}");
    }

    #[test]
    fn test_simple_sixel_single_pixel_column() {
        // A single '?' (0x3F) encodes sixel value 0 (no pixels set).
        // A single '@' (0x40) encodes sixel value 1 (top pixel set).
        // Use a 1-pixel wide image: #0;2;100;0;0q@ (red, top pixel)
        let inner = b"0;0;0q#0;2;100;0;0@";
        let img = parse_sixel(inner).unwrap();
        assert_eq!(img.width, 1);
        assert_eq!(img.height, 6);
        // Top pixel should be red (255, 0, 0, 255)
        assert_eq!(img.pixels[0], 255); // R
        assert_eq!(img.pixels[1], 0); // G
        assert_eq!(img.pixels[2], 0); // B
        assert_eq!(img.pixels[3], 255); // A
    }

    #[test]
    fn test_sixel_all_bits_set() {
        // '~' (0x7E) = sixel value 63 = all 6 bits set
        let inner = b"q#0;2;0;100;0~";
        let img = parse_sixel(inner).unwrap();
        assert_eq!(img.width, 1);
        assert_eq!(img.height, 6);
        // All 6 pixels should be green
        for y in 0..6 {
            let offset = y * 4;
            assert_eq!(img.pixels[offset], 0, "pixel {y} R");
            assert_eq!(img.pixels[offset + 1], 255, "pixel {y} G");
            assert_eq!(img.pixels[offset + 2], 0, "pixel {y} B");
            assert_eq!(img.pixels[offset + 3], 255, "pixel {y} A");
        }
    }

    #[test]
    fn test_sixel_repeat() {
        // !3~ means repeat '~' 3 times → 3 columns, all 6 bits set
        let inner = b"q#0;2;100;100;100!3~";
        let img = parse_sixel(inner).unwrap();
        assert_eq!(img.width, 3);
        assert_eq!(img.height, 6);
        // All pixels should be white (255, 255, 255)
        for x in 0..3 {
            for y in 0..6 {
                let offset = (y * 3 + x) * 4;
                assert_eq!(img.pixels[offset], 255, "({x},{y}) R");
                assert_eq!(img.pixels[offset + 1], 255, "({x},{y}) G");
                assert_eq!(img.pixels[offset + 2], 255, "({x},{y}) B");
            }
        }
    }

    #[test]
    fn test_sixel_carriage_return() {
        // Two colours on the same band:
        // #0;2;100;0;0~ (red, all bits)
        // $  (carriage return)
        // #1;2;0;100;0~ (green, all bits, overwrites same position)
        let inner = b"q#0;2;100;0;0~$#1;2;0;100;0~";
        let img = parse_sixel(inner).unwrap();
        assert_eq!(img.width, 1);
        assert_eq!(img.height, 6);
        // The green should overwrite the red (both paint on same pixels)
        assert_eq!(img.pixels[0], 0, "R");
        assert_eq!(img.pixels[1], 255, "G");
        assert_eq!(img.pixels[2], 0, "B");
    }

    #[test]
    fn test_sixel_newline() {
        // Two bands:
        // #0;2;100;0;0~ (band 0, red)
        // -              (new line → band 1)
        // #0~ (band 1, same red color)
        let inner = b"q#0;2;100;0;0~-~";
        let img = parse_sixel(inner).unwrap();
        assert_eq!(img.width, 1);
        assert_eq!(img.height, 12); // 2 bands x 6 pixels
        // Band 0, pixel 0 → red
        assert_eq!(img.pixels[0], 255);
        // Band 1, pixel 0 (y=6) → red
        let offset = 6 * 4; // y=6, width=1
        assert_eq!(img.pixels[offset], 255);
    }

    #[test]
    fn test_sixel_raster_attributes() {
        // "1;1;4;3q means aspect 1:1, image is 4x3
        let inner = b"q\"1;1;4;3#0;2;100;0;0!4~";
        let img = parse_sixel(inner).unwrap();
        // Raster attributes declared 4x3, but one band of ~'s produces 6 rows
        // The declared height (3) trims it.
        assert_eq!(img.width, 4);
        assert_eq!(img.height, 3);
    }

    #[test]
    fn test_sixel_transparent_background() {
        // P2=1 → transparent background
        let inner = b"0;1;0q#0;2;100;0;0@";
        let img = parse_sixel(inner).unwrap();
        // Top pixel is red (painted), but remaining 5 pixels should be transparent
        assert_eq!(img.pixels[0], 255); // R
        assert_eq!(img.pixels[3], 255); // A = opaque (painted pixel)
        // Second pixel (y=1) should be transparent
        assert_eq!(img.pixels[4], 0); // R
        assert_eq!(img.pixels[5], 0); // G
        assert_eq!(img.pixels[6], 0); // B
        assert_eq!(img.pixels[7], 0); // A = transparent
    }

    #[test]
    fn test_sixel_paint_background() {
        // P2=0 → paint background with palette[0]
        // Set palette[0] to blue, then paint one pixel with colour 1 (red)
        let inner = b"0;0;0q#0;2;0;0;100#1;2;100;0;0#1@";
        let img = parse_sixel(inner).unwrap();
        // Top pixel (y=0) = red (colour 1, '@' = bit 0 set)
        assert_eq!(img.pixels[0], 255, "top R");
        assert_eq!(img.pixels[1], 0, "top G");
        assert_eq!(img.pixels[2], 0, "top B");
        assert_eq!(img.pixels[3], 255, "top A");
        // Second pixel (y=1) = blue (background, palette[0])
        assert_eq!(img.pixels[4], 0, "bg R");
        assert_eq!(img.pixels[5], 0, "bg G");
        assert_eq!(img.pixels[6], 255, "bg B");
        assert_eq!(img.pixels[7], 255, "bg A");
    }

    #[test]
    fn test_sixel_empty_data_returns_none() {
        assert!(parse_sixel(b"").is_none());
        assert!(parse_sixel(b"0;0;0").is_none()); // no 'q'
    }

    #[test]
    fn test_sixel_no_data_after_q_returns_none() {
        assert!(parse_sixel(b"q").is_none());
    }

    #[test]
    fn test_sixel_multiple_colours() {
        // 2 pixels wide: first is red, second is blue
        let inner = b"q#0;2;100;0;0@#1;2;0;0;100@";
        let img = parse_sixel(inner).unwrap();
        assert_eq!(img.width, 2);
        // Pixel (0, 0) = red
        assert_eq!(img.pixels[0], 255);
        assert_eq!(img.pixels[1], 0);
        assert_eq!(img.pixels[2], 0);
        // Pixel (1, 0) = blue
        assert_eq!(img.pixels[4], 0);
        assert_eq!(img.pixels[5], 0);
        assert_eq!(img.pixels[6], 255);
    }

    #[test]
    fn test_sixel_colour_selection_without_definition() {
        // #5 selects colour 5 without defining it; should use default palette
        let inner = b"q#5~";
        let img = parse_sixel(inner).unwrap();
        assert_eq!(img.width, 1);
        let (r, g, b) = DEFAULT_PALETTE_16[5];
        assert_eq!(img.pixels[0], r);
        assert_eq!(img.pixels[1], g);
        assert_eq!(img.pixels[2], b);
    }

    #[test]
    fn test_sixel_hls_colour_definition() {
        // #0;1;0;50;100 → HLS: hue=0 (red), lightness=50, saturation=100
        let inner = b"q#0;1;0;50;100~";
        let img = parse_sixel(inner).unwrap();
        // Should be a red-ish colour
        assert!(img.pixels[0] > 200, "R={}", img.pixels[0]);
    }

    #[test]
    fn test_sixel_large_repeat_count() {
        // !100~ → 100 columns of all-set pixels
        let inner = b"q#0;2;50;50;50!100~";
        let img = parse_sixel(inner).unwrap();
        assert_eq!(img.width, 100);
        assert_eq!(img.height, 6);
    }

    #[test]
    fn test_sixel_mixed_data_and_control() {
        // A realistic sequence: define colour, draw, CR, different colour overlay
        // Band 0: 3 red columns, then overlay middle with green
        let inner = b"q#0;2;100;0;0~~~$#1;2;0;100;0?A?";
        let img = parse_sixel(inner).unwrap();
        assert_eq!(img.width, 3);
        // Position (0, 0): red (~=all bits, then ?=no bits overlay) → red stays
        assert_eq!(img.pixels[0], 255, "R at (0,0)");
        // Position (1, 0): red from ~, then A (bit 1) overlay in green
        // Green only paints bit 1 (y=1), so (1,0) stays red from ~
        assert_eq!(img.pixels[4], 255, "R at (1,0)");
        // Position (1, 1): originally red, then green A paints bit 1 → green
        let offset_1_1 = (3 + 1) * 4;
        assert_eq!(img.pixels[offset_1_1], 0, "R at (1,1)");
        assert_eq!(img.pixels[offset_1_1 + 1], 255, "G at (1,1)");
    }

    #[test]
    fn test_parse_number_basic() {
        assert_eq!(SixelDecoder::parse_number(b"123;"), (123, 3));
        assert_eq!(SixelDecoder::parse_number(b"0"), (0, 1));
        assert_eq!(SixelDecoder::parse_number(b""), (0, 0));
        assert_eq!(SixelDecoder::parse_number(b"abc"), (0, 0));
    }

    #[test]
    fn test_sixel_width_grows_dynamically() {
        // First band: 2 columns. Then newline, then 5 columns.
        // Width should be max(2, 5) = 5.
        let inner = b"q#0;2;100;0;0@@-!5@";
        let img = parse_sixel(inner).unwrap();
        assert_eq!(img.width, 5);
        assert_eq!(img.height, 12);
    }

    // --- finish() trim branches ---

    #[test]
    fn test_sixel_height_trim_via_raster_attributes() {
        // Raster: declared height = 3, but sixel data fills 2 bands (12 rows).
        // Since declared_height (3) < actual height (12), finish() must trim.
        // "\"1;1;2;3 means aspect 1:1, width=2, height=3
        let inner = b"q\"1;1;2;3#0;2;100;0;0!2~-!2~";
        let img = parse_sixel(inner).unwrap();
        // height trimmed to declared 3
        assert_eq!(img.height, 3);
        assert_eq!(img.width, 2);
    }

    #[test]
    fn test_sixel_width_trim_via_raster_attributes() {
        // Raster: declared width = 1, actual data is 3 columns wide.
        // Since declared_width (1) < actual width (3), finish() must trim columns.
        // "\"1;1;1;6" means aspect 1:1, declared width=1, declared height=6
        let inner = b"q\"1;1;1;6#0;2;100;0;0~~~";
        let img = parse_sixel(inner).unwrap();
        // width trimmed to declared 1
        assert_eq!(img.width, 1);
        assert_eq!(img.height, 6);
        // The remaining pixel should still be red
        assert_eq!(img.pixels[0], 255, "R");
        assert_eq!(img.pixels[1], 0, "G");
        assert_eq!(img.pixels[2], 0, "B");
    }

    // --- parse_sixel_with_shared_palette: None branch (no 'q') ---

    #[test]
    fn test_parse_sixel_with_shared_palette_no_q_returns_none() {
        let palette = [(0u8, 0u8, 0u8); MAX_PALETTE];
        // No 'q' byte in the input → the `let Some(q_pos)` guard returns (None, palette)
        let (img, _returned_palette) = parse_sixel_with_shared_palette(b"nodatahere", palette);
        assert!(img.is_none());
    }

    #[test]
    fn test_parse_sixel_with_shared_palette_basic() {
        // Normal shared-palette parse: single red pixel
        let palette = [(0u8, 0u8, 0u8); MAX_PALETTE];
        let inner = b"q#0;2;100;0;0@";
        let (img, _) = parse_sixel_with_shared_palette(inner, palette);
        let img = img.unwrap();
        assert_eq!(img.width, 1);
        assert_eq!(img.pixels[0], 255);
    }

    // --- hls_to_rgb remaining sectors ---

    #[test]
    fn test_hls_to_rgb_sector2_yellow_green() {
        // Hue 90 → sector 1.5 (sector < 2.0): (secondary, chroma, 0.0)
        let (r, g, b) = hls_to_rgb(90, 50, 100);
        // Yellow-green: mostly green, some red, no blue
        assert!(g > r, "green should dominate: r={r} g={g} b={b}");
        assert!(b < 10, "blue should be near 0: b={b}");
    }

    #[test]
    fn test_hls_to_rgb_sector3_cyan() {
        // Hue 150 → sector 2.5 (sector < 3.0): (0.0, chroma, secondary)
        let (r, g, b) = hls_to_rgb(150, 50, 100);
        // Cyan-ish: mostly green, some blue, no red
        assert!(r < 10, "red should be near 0: r={r}");
        assert!(g > b, "green should exceed blue: g={g} b={b}");
    }

    #[test]
    fn test_hls_to_rgb_sector4_cyan_blue() {
        // Hue 210 → sector 3.5 (sector < 4.0): (0.0, secondary, chroma)
        let (r, g, b) = hls_to_rgb(210, 50, 100);
        // Cyan-blue: mostly blue, some green, no red
        assert!(r < 10, "red should be near 0: r={r}");
        assert!(b > g, "blue should exceed green: g={g} b={b}");
    }

    #[test]
    fn test_hls_to_rgb_sector5_magenta() {
        // Hue 270 → sector 4.5 (sector < 5.0): (secondary, 0.0, chroma)
        let (r, g, b) = hls_to_rgb(270, 50, 100);
        // Violet/magenta: blue and some red, no green
        assert!(g < 10, "green should be near 0: g={g}");
        assert!(b > r, "blue should exceed red: r={r} b={b}");
    }

    #[test]
    fn test_hls_to_rgb_sector6_red_magenta() {
        // Hue 330 → sector 5.5 (sector >= 5.0): (chroma, 0.0, secondary)
        let (r, g, b) = hls_to_rgb(330, 50, 100);
        // Red-magenta: mostly red, some blue, no green
        assert!(g < 10, "green should be near 0: g={g}");
        assert!(r > b, "red should exceed blue: r={r} b={b}");
    }

    // --- skip-other byte branch (line ~511) ---

    #[test]
    fn test_sixel_skip_unrecognized_bytes() {
        // Whitespace and other non-control bytes should be silently skipped.
        // Embed a space and a tab in a valid sixel stream.
        let inner = b"q#0;2;100;0;0 \t~";
        let img = parse_sixel(inner).unwrap();
        // Despite the garbage bytes, one column of all-set pixels should decode.
        assert_eq!(img.width, 1);
        assert_eq!(img.height, 6);
    }

    // --- repeat with count==0 (should use 1) ---

    #[test]
    fn test_sixel_repeat_count_zero_uses_one() {
        // !0@ → repeat 0 times → treated as 1 repetition
        let inner = b"q#0;2;100;0;0!0@";
        let img = parse_sixel(inner).unwrap();
        assert_eq!(img.width, 1);
    }

    // --- non-sixel char after ! (skipped) ---

    #[test]
    fn test_sixel_repeat_non_sixel_char_skipped() {
        // !5; → ';' is not in the sixel range 0x3F..=0x7E, the draw is skipped
        // but the rest of the stream (a valid pixel '@') still decodes.
        let inner = b"q#0;2;100;0;0!5;@";
        let img = parse_sixel(inner).unwrap();
        // '@' (sixel value 1 → bit 0 = top pixel) is still processed.
        assert_eq!(img.width, 1);
    }

    // --- finish_into_parts height/width trim (via parse_sixel_with_shared_palette) ---

    #[test]
    fn test_finish_into_parts_height_trim() {
        // Two bands of data (12 rows total) but declared height = 3.
        // finish_into_parts should trim to 3.
        let palette = [(0u8, 0u8, 0u8); MAX_PALETTE];
        let inner = b"q\"1;1;2;3#0;2;100;0;0!2~-!2~";
        let (img, _) = parse_sixel_with_shared_palette(inner, palette);
        let img = img.unwrap();
        assert_eq!(img.height, 3);
        assert_eq!(img.width, 2);
    }

    #[test]
    fn test_finish_into_parts_width_trim() {
        // 3 columns of data but declared width = 1.
        // finish_into_parts should trim columns to 1.
        let palette = [(0u8, 0u8, 0u8); MAX_PALETTE];
        let inner = b"q\"1;1;1;6#0;2;100;0;0~~~";
        let (img, _) = parse_sixel_with_shared_palette(inner, palette);
        let img = img.unwrap();
        assert_eq!(img.width, 1);
        assert_eq!(img.height, 6);
    }

    // --- Task 148.2: bounded allocation (GHSA-w475-m9wf-775r) ---

    /// Every image the decoder returns must satisfy the documented caps and
    /// be internally consistent.
    fn assert_within_caps(img: &SixelImage) {
        let width = usize::value_from(img.width).unwrap();
        let height = usize::value_from(img.height).unwrap();
        assert!(width <= MAX_SIXEL_DIMENSION, "width {width}");
        assert!(height <= MAX_SIXEL_DIMENSION, "height {height}");
        assert!(width * height <= MAX_SIXEL_PIXELS, "{width}x{height}");
        assert_eq!(img.pixels.len(), width * height * 4);
    }

    #[test]
    fn test_sixel_advisory_input_does_not_allocate_unbounded() {
        // Inner DCS content from GHSA-w475-m9wf-775r.
        let inner: &[u8] = b"(q\";\";;56;1\x1fq\"05;;2;26221056;1;";
        if let Some(img) = parse_sixel(inner) {
            assert_within_caps(&img);
        }
    }

    #[test]
    fn test_sixel_oversize_raster_declaration_is_ignored() {
        // Ph = 10001 exceeds MAX_SIXEL_DIMENSION: the declaration is ignored,
        // so the image is sized from the data alone (1 column, 1 band).
        let img = parse_sixel(b"q\"1;1;10001;10#0;2;100;0;0~").unwrap();
        assert_eq!((img.width, img.height), (1, 6));
        assert_within_caps(&img);

        // Pv = 10001 likewise.
        let img = parse_sixel(b"q\"1;1;10;10001#0;2;100;0;0~").unwrap();
        assert_eq!((img.width, img.height), (1, 6));

        // Each dimension is in range but the product exceeds MAX_SIXEL_PIXELS.
        let img = parse_sixel(b"q\"1;1;5000;5000#0;2;100;0;0~").unwrap();
        assert_eq!((img.width, img.height), (1, 6));

        // u32::MAX in both.
        let img = parse_sixel(b"q\"1;1;4294967295;4294967295~").unwrap();
        assert_eq!((img.width, img.height), (1, 6));
    }

    #[test]
    fn test_sixel_declaration_at_dimension_cap_is_accepted() {
        // 10000 x 1 is exactly at the dimension cap and well inside the pixel
        // cap, so the declaration is honoured.
        let img = parse_sixel(b"q\"1;1;10000;1#0;2;100;0;0~").unwrap();
        assert_eq!((img.width, img.height), (10_000, 1));
        assert_within_caps(&img);
    }

    #[test]
    fn test_sixel_declaration_at_pixel_cap_boundary_is_accepted() {
        // 4096 x 4096 is exactly MAX_SIXEL_PIXELS; draw a single band.
        let img = parse_sixel(b"q\"1;1;4096;4096#0;2;100;0;0~").unwrap();
        assert_eq!((img.width, img.height), (4096, 4096));
        assert_eq!(img.pixels.len(), MAX_SIXEL_PIXELS * 4);
        // The single band painted the top-left pixel red.
        assert_eq!(&img.pixels[..4], &[255, 0, 0, 255]);
    }

    #[test]
    fn test_sixel_huge_repeat_is_clipped_and_fast() {
        let img = parse_sixel(b"q#0;2;100;0;0!4294967295~").unwrap();
        assert_eq!(usize::value_from(img.width).unwrap(), MAX_SIXEL_DIMENSION);
        assert_eq!(img.height, 6);
        assert_within_caps(&img);
        // Clipped, not skipped: the drawn columns are red.
        assert_eq!(&img.pixels[..4], &[255, 0, 0, 255]);
    }

    #[test]
    fn test_sixel_many_bands_are_clipped_to_dimension_cap() {
        let mut inner = b"q#0;2;100;0;0".to_vec();
        inner.extend(std::iter::repeat_n(b'-', 3000));
        inner.push(b'~');
        let img = parse_sixel(&inner).unwrap();
        assert_eq!(usize::value_from(img.height).unwrap(), MAX_SIXEL_DIMENSION);
        assert_eq!(img.width, 1);
        assert_within_caps(&img);
    }

    #[test]
    fn test_sixel_repeated_huge_repeats_do_not_wrap_position() {
        // Two u32::MAX repeats push `x` far past the cap; must not overflow
        // or panic, and the canvas stays within the caps.
        let img = parse_sixel(b"q!4294967295~!4294967295~!4294967295~").unwrap();
        assert_within_caps(&img);
    }

    #[test]
    fn test_sixel_overflowing_declaration_after_data_does_not_panic() {
        // Data first (allocates a 1x6 canvas), then a declaration whose
        // `width * height * 4` overflows `usize`: the old resize path built an
        // empty buffer and panicked slicing it.
        let img = parse_sixel(b"q~\"1;1;4294967295;4294967295~").unwrap();
        assert_within_caps(&img);
        // One column from the leading `~`, one from the trailing `~`.
        assert_eq!((img.width, img.height), (2, 6));
    }

    #[test]
    fn test_sixel_overflowing_declaration_before_data_does_not_panic() {
        let img = parse_sixel(b"q\"1;1;4294967295;4294967295~~~").unwrap();
        assert_within_caps(&img);
        // The declaration is ignored; the three `~` each draw one column.
        assert_eq!((img.width, img.height), (3, 6));
    }

    #[test]
    fn test_sixel_growth_beyond_pixel_cap_keeps_product_within_cap() {
        // 4000 x 4000 is accepted (16,000,000 px).  Drawing at x = 9000 would
        // need 9001 x 4000 = 36M px, so the width growth is limited to what
        // fits under the pixel cap.
        let mut decoder = SixelDecoder::new(SixelBackground::Paint);
        decoder.decode(b"\"1;1;4000;4000#0;2;100;0;0!9000?~");
        assert!(decoder.invariant_holds());
        assert_eq!(decoder.height, 4000);
        assert_eq!(decoder.width, MAX_SIXEL_PIXELS / 4000);
        // The finished image is trimmed to the declared 4000 x 4000.
        let img = decoder.finish().unwrap();
        assert_within_caps(&img);
        assert_eq!((img.width, img.height), (4000, 4000));
    }

    #[test]
    fn test_sixel_shared_palette_path_is_bounded() {
        let palette = default_sixel_palette();
        let (img, _) =
            parse_sixel_with_shared_palette(b"q\"1;1;4294967295;4294967295!4294967295~", palette);
        assert_within_caps(&img.unwrap());
    }

    #[test]
    fn test_clamped_canvas_size_properties() {
        // Never shrinks, never exceeds the caps, product stays within the cap.
        let samples = [
            0,
            1,
            6,
            100,
            4095,
            4096,
            4194,
            9999,
            10_000,
            10_001,
            usize::MAX,
        ];
        for &cw in &samples {
            for &ch in &samples {
                // Only valid current sizes (the decoder invariant).
                if cw > MAX_SIXEL_DIMENSION
                    || ch > MAX_SIXEL_DIMENSION
                    || cw * ch > MAX_SIXEL_PIXELS
                    || (cw == 0) != (ch == 0)
                {
                    continue;
                }
                for &ww in &samples {
                    for &wh in &samples {
                        let (w, h) = clamped_canvas_size(cw, ch, ww, wh);
                        assert!(w >= cw && h >= ch, "shrunk {cw}x{ch} -> {w}x{h}");
                        assert!(w <= MAX_SIXEL_DIMENSION && h <= MAX_SIXEL_DIMENSION);
                        assert!(
                            w * h <= MAX_SIXEL_PIXELS,
                            "{cw}x{ch} want {ww}x{wh} -> {w}x{h}"
                        );
                    }
                }
            }
        }
    }

    // --- Amortised growth (Task 148.2) ---

    /// Interleaved one-character-at-a-time growth.  Each `~` grows the
    /// logical width by one column on a canvas that is already 10000 rows
    /// tall; reallocating and copying the whole image per column cost ~18 s
    /// (release) on the previous implementation, far beyond nextest's
    /// slow-test threshold even in a debug build.  With geometric capacity
    /// growth the work is proportional to the final image area.
    #[test]
    fn test_sixel_one_column_growth_on_tall_image_is_amortised() {
        let mut inner = b"q".to_vec();
        inner.extend(std::iter::repeat_n(b'-', 1666));
        inner.extend(std::iter::repeat_n(b'~', 1677));
        let img = parse_sixel(&inner).unwrap();
        assert_within_caps(&img);
        assert_eq!(
            (
                usize::value_from(img.width).unwrap(),
                usize::value_from(img.height).unwrap()
            ),
            (1677, MAX_SIXEL_DIMENSION)
        );
    }

    /// Wide band built one column at a time, then one band at a time
    /// underneath it.  Each new band grows the height of a 9000-column image
    /// by 6 rows; copying the image per band is ~10 GB of memcpy in total.
    #[test]
    fn test_sixel_one_band_growth_on_wide_image_is_amortised() {
        let mut inner = b"q".to_vec();
        inner.extend(std::iter::repeat_n(b'~', 9000));
        for _ in 0..1600 {
            inner.extend_from_slice(b"-~");
        }
        let img = parse_sixel(&inner).unwrap();
        assert_within_caps(&img);
        // 9000 columns leave room for 16,777,216 / 9000 = 1864 rows.
        assert_eq!(img.width, 9000);
        assert_eq!(img.height, 1864);
    }

    #[test]
    fn test_sixel_paint_background_uses_palette0_at_each_growth() {
        // P2 = 0: Paint.  Each growth exposes new cells painted with the
        // palette entry 0 in force *at that moment*; cells already inside the
        // image keep their earlier fill.
        let inner = b"0;0;0q#0;2;100;0;0#1;2;100;100;100@\
                      #0;2;0;100;0#1@\
                      #0;2;0;0;100#1-@";
        let inner: Vec<u8> = inner
            .iter()
            .copied()
            .filter(|&b| b != b' ' && b != b'\n')
            .collect();
        let img = parse_sixel(&inner).unwrap();
        assert_eq!((img.width, img.height), (2, 12));
        let px = |x: usize, y: usize| -> [u8; 4] {
            let o = (y * 2 + x) * 4;
            [
                img.pixels[o],
                img.pixels[o + 1],
                img.pixels[o + 2],
                img.pixels[o + 3],
            ]
        };
        let white = [255, 255, 255, 255];
        let red = [255, 0, 0, 255];
        let green = [0, 255, 0, 255];
        let blue = [0, 0, 255, 255];
        for y in 0..12 {
            for x in 0..2 {
                let expected = match (x, y) {
                    (0 | 1, 0) | (0, 6) => white,
                    (0, 1..=5) => red,
                    (1, 1..=5) => green,
                    _ => blue,
                };
                assert_eq!(px(x, y), expected, "pixel ({x},{y})");
            }
        }
    }

    #[test]
    fn test_sixel_transparent_background_never_fills_growth() {
        // P2 = 1: Transparent.  Redefining palette 0 must not leak into the
        // exposed area.
        let img = parse_sixel(b"0;1;0q#0;2;100;0;0#1;2;100;100;100@#0;2;0;100;0#1@-@").unwrap();
        assert_eq!((img.width, img.height), (2, 12));
        for y in 0..12usize {
            for x in 0..2usize {
                let o = (y * 2 + x) * 4;
                let painted = matches!((x, y), (0 | 1, 0) | (0, 6));
                let expected: [u8; 4] = if painted { [255; 4] } else { [0; 4] };
                assert_eq!(img.pixels[o..o + 4], expected, "pixel ({x},{y})");
            }
        }
    }

    #[test]
    fn test_grown_capacity_properties() {
        let samples = [0, 1, 6, 63, 64, 65, 1000, 4096, 4194, 9999, 10_000];
        for &cw in &samples {
            for &ch in &samples {
                if cw * ch > MAX_SIXEL_PIXELS {
                    continue;
                }
                for &w in &samples {
                    for &h in &samples {
                        // Logical extent must itself satisfy the invariant.
                        if w == 0 || h == 0 || w * h > MAX_SIXEL_PIXELS {
                            continue;
                        }
                        let (nw, nh) = grown_capacity(cw, ch, w, h);
                        assert!(nw >= w && nh >= h, "{cw}x{ch} for {w}x{h} -> {nw}x{nh}");
                        assert!(nw <= MAX_SIXEL_DIMENSION && nh <= MAX_SIXEL_DIMENSION);
                        assert!(
                            nw * nh <= MAX_SIXEL_PIXELS,
                            "{cw}x{ch} for {w}x{h} -> {nw}x{nh}"
                        );
                    }
                }
            }
        }
    }

    /// Deterministic pseudo-random sixel streams (both backgrounds, raster
    /// attributes, palette redefinition, repeats, `$` and `-`) must decode
    /// byte-identically to the pre-148.2 decoder.
    #[test]
    fn test_sixel_matches_reference_decoder_on_small_inputs() {
        let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = move |bound: u64| -> u64 {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (state >> 33) % bound
        };

        for case in 0..4000 {
            let mut inner = format!("0;{};0q", next(3)).into_bytes();
            let tokens = 1 + next(40);
            for _ in 0..tokens {
                let tok = match next(12) {
                    0..=3 => {
                        let ch = u8::try_from(0x3F + next(64)).unwrap();
                        vec![ch]
                    }
                    4 | 5 => {
                        let ch = u8::try_from(0x3F + next(64)).unwrap();
                        let mut t = format!("!{}", next(21)).into_bytes();
                        t.push(ch);
                        t
                    }
                    6 => b"-".to_vec(),
                    7 => b"$".to_vec(),
                    8 => format!("#{};2;{};{};{}", next(4), next(101), next(101), next(101))
                        .into_bytes(),
                    9 => format!("#{}", next(4)).into_bytes(),
                    10 => format!("#{};1;{};{};{}", next(4), next(361), next(101), next(101))
                        .into_bytes(),
                    _ => format!("\"1;1;{};{}", 1 + next(25), 1 + next(25)).into_bytes(),
                };
                inner.extend_from_slice(&tok);
            }

            let new = parse_sixel(&inner);
            let old = reference::parse(&inner);
            match (new, old) {
                (None, None) => {}
                (Some(n), Some(o)) => {
                    assert_eq!((n.width, n.height), (o.width, o.height), "case {case}");
                    assert_eq!(
                        n.pixels,
                        o.pixels,
                        "case {case}: {}",
                        String::from_utf8_lossy(&inner)
                    );
                }
                (n, o) => panic!(
                    "case {case}: new={:?} old={:?}",
                    n.map(|i| (i.width, i.height)),
                    o.map(|i| (i.width, i.height))
                ),
            }
        }
    }

    /// Verbatim copy of the pre-148.2 decoder (uncapped, reallocating on every
    /// growth, tight buffer).  Kept in the test module only, for small inputs,
    /// to pin that the amortised canvas is byte-identical with the behaviour
    /// shipped on `main`.
    mod reference {
        use super::super::*;

        /// Internal state for the Sixel decoder.
        struct RefDecoder {
            /// Palette: 256 entries of (R, G, B).
            palette: [(u8, u8, u8); MAX_PALETTE],
            /// Currently selected palette index.
            current_color: usize,
            /// Pixel x position within the current band.
            x: usize,
            /// Current band index (each band is 6 pixels tall).
            band: usize,
            /// Image width in pixels (may grow as data arrives).
            width: usize,
            /// Image height in pixels (may grow as bands are added).
            height: usize,
            /// Declared image width from raster attributes (0 = not set).
            declared_width: usize,
            /// Declared image height from raster attributes (0 = not set).
            declared_height: usize,
            /// Background mode.
            background: SixelBackground,
            /// RGBA pixel buffer, row-major.
            pixels: Vec<u8>,
        }

        impl RefDecoder {
            fn new(background: SixelBackground) -> Self {
                let mut palette = [(0u8, 0u8, 0u8); MAX_PALETTE];
                for (i, &(r, g, b)) in DEFAULT_PALETTE_16.iter().enumerate() {
                    palette[i] = (r, g, b);
                }

                Self {
                    palette,
                    current_color: 0,
                    x: 0,
                    band: 0,
                    width: 0,
                    height: 0,
                    declared_width: 0,
                    declared_height: 0,
                    background,
                    pixels: Vec::new(),
                }
            }

            /// Ensure the pixel buffer is large enough to hold the given dimensions.
            fn ensure_size(&mut self, new_width: usize, new_height: usize) {
                if new_width <= self.width && new_height <= self.height {
                    return;
                }

                let old_width = self.width;
                let old_height = self.height;
                let final_width = new_width.max(self.width);
                let final_height = new_height.max(self.height);

                if old_width == 0 || old_height == 0 {
                    // First allocation.
                    self.width = final_width;
                    self.height = final_height;
                    let size = final_width
                        .checked_mul(final_height)
                        .and_then(|n| n.checked_mul(4))
                        .unwrap_or(0);
                    self.pixels = match self.background {
                        SixelBackground::Paint => {
                            let (r, g, b) = self.palette[0];
                            let mut buf = vec![0u8; size];
                            // `as_chunks_mut` yields fixed-size arrays, so the four
                            // component writes are bounds-check-free (clippy 1.98's
                            // `chunks_exact_to_as_chunks`). `size` is a multiple of 4
                            // by construction, so the remainder is always empty.
                            for pixel in buf.as_chunks_mut::<4>().0 {
                                *pixel = [r, g, b, 255];
                            }
                            buf
                        }
                        SixelBackground::Transparent => vec![0u8; size],
                    };
                    return;
                }

                // Resize existing buffer.
                let new_total = final_width
                    .checked_mul(final_height)
                    .and_then(|n| n.checked_mul(4))
                    .unwrap_or(0);
                let mut new_pixels = match self.background {
                    SixelBackground::Paint => {
                        let (r, g, b) = self.palette[0];
                        let mut buf = vec![0u8; new_total];
                        // See the comment on the initial-allocation path above.
                        for pixel in buf.as_chunks_mut::<4>().0 {
                            *pixel = [r, g, b, 255];
                        }
                        buf
                    }
                    SixelBackground::Transparent => vec![0u8; new_total],
                };

                // Copy old rows into the new buffer.
                let copy_width = old_width.min(final_width);
                for y in 0..old_height.min(final_height) {
                    let old_row_start = y * old_width * 4;
                    let new_row_start = y * final_width * 4;
                    let copy_bytes = copy_width * 4;
                    new_pixels[new_row_start..new_row_start + copy_bytes]
                        .copy_from_slice(&self.pixels[old_row_start..old_row_start + copy_bytes]);
                }

                self.pixels = new_pixels;
                self.width = final_width;
                self.height = final_height;
            }

            /// Set a single pixel at (px, py) to the current palette colour.
            fn set_pixel(&mut self, px: usize, py: usize) {
                if px >= self.width || py >= self.height {
                    return;
                }
                let (red, green, blue) = self.palette[self.current_color];
                let offset = (py * self.width + px) * 4;
                if offset + 3 < self.pixels.len() {
                    self.pixels[offset] = red;
                    self.pixels[offset + 1] = green;
                    self.pixels[offset + 2] = blue;
                    self.pixels[offset + 3] = 255;
                }
            }

            /// Apply a single sixel data byte (encodes 6 vertical pixels).
            fn apply_sixel(&mut self, sixel_value: u8, count: usize) {
                let band_y = self.band * 6;
                let needed_height = band_y + 6;
                let needed_width = self.x + count;

                self.ensure_size(needed_width, needed_height);

                for dx in 0..count {
                    let px = self.x + dx;
                    for bit in 0..6u8 {
                        if sixel_value & (1 << bit) != 0 {
                            self.set_pixel(px, band_y + usize::from(bit));
                        }
                    }
                }

                self.x += count;
            }

            /// Parse a numeric parameter from the data stream, returning `(value, bytes_consumed)`.
            fn parse_number(data: &[u8]) -> (u32, usize) {
                let mut val: u32 = 0;
                let mut len = 0;
                for &b in data {
                    if b.is_ascii_digit() {
                        val = val.saturating_mul(10).saturating_add(u32::from(b - b'0'));
                        len += 1;
                    } else {
                        break;
                    }
                }
                (val, len)
            }

            /// Decode the sixel data stream (everything after `q`).
            fn decode(&mut self, data: &[u8]) {
                let mut i = 0;
                let len = data.len();

                // If raster attributes declared dimensions, pre-allocate.
                // We'll check for them in the first pass of the data.

                while i < len {
                    let b = data[i];
                    match b {
                        // Sixel data character: ? (0x3F) through ~ (0x7E)
                        0x3F..=0x7E => {
                            let sixel_value = b - 0x3F;
                            self.apply_sixel(sixel_value, 1);
                            i += 1;
                        }
                        // `!` — repeat introducer: !<count><sixel-char>
                        b'!' => {
                            i += 1;
                            let (count, consumed) = Self::parse_number(&data[i..]);
                            i += consumed;
                            if i < len {
                                let ch = data[i];
                                if (0x3F..=0x7E).contains(&ch) {
                                    let sixel_value = ch - 0x3F;
                                    let repeat = if count == 0 { 1 } else { usize_from_u32(count) };
                                    self.apply_sixel(sixel_value, repeat);
                                }
                                i += 1;
                            }
                        }
                        // `"` — raster attributes: "Pan;Pad;Ph;Pv
                        b'"' => {
                            i += 1;
                            // Pan (pixel aspect numerator)
                            let (_pan, c1) = Self::parse_number(&data[i..]);
                            i += c1;
                            if i < len && data[i] == b';' {
                                i += 1;
                            }
                            // Pad (pixel aspect denominator)
                            let (_pad, c2) = Self::parse_number(&data[i..]);
                            i += c2;
                            if i < len && data[i] == b';' {
                                i += 1;
                            }
                            // Ph (width)
                            let (ph, c3) = Self::parse_number(&data[i..]);
                            i += c3;
                            if i < len && data[i] == b';' {
                                i += 1;
                            }
                            // Pv (height)
                            let (pv, c4) = Self::parse_number(&data[i..]);
                            i += c4;

                            if ph > 0 && pv > 0 {
                                self.declared_width = usize_from_u32(ph);
                                self.declared_height = usize_from_u32(pv);
                                self.ensure_size(self.declared_width, self.declared_height);
                            }
                        }
                        // `#` — colour introducer
                        b'#' => {
                            i += 1;
                            // Pc (palette index)
                            let (pc, c1) = Self::parse_number(&data[i..]);
                            i += c1;

                            if i < len && data[i] == b';' {
                                // Colour definition: #Pc;Pu;Px;Py;Pz
                                i += 1;
                                let (pu, c2) = Self::parse_number(&data[i..]);
                                i += c2;
                                if i < len && data[i] == b';' {
                                    i += 1;
                                }
                                let (px, c3) = Self::parse_number(&data[i..]);
                                i += c3;
                                if i < len && data[i] == b';' {
                                    i += 1;
                                }
                                let (py, c4) = Self::parse_number(&data[i..]);
                                i += c4;
                                if i < len && data[i] == b';' {
                                    i += 1;
                                }
                                let (pz, c5) = Self::parse_number(&data[i..]);
                                i += c5;

                                let idx = usize_from_u32(pc).min(MAX_PALETTE - 1);
                                self.palette[idx] = if pu == 1 {
                                    hls_to_rgb(px, py, pz)
                                } else {
                                    // pu == 2 (RGB) or default to RGB
                                    pct_to_rgb(px, py, pz)
                                };
                                self.current_color = idx;
                            } else {
                                // Colour selection only: #Pc
                                self.current_color = usize_from_u32(pc).min(MAX_PALETTE - 1);
                            }
                        }
                        // `$` — graphics carriage return (x = 0, same band)
                        b'$' => {
                            self.x = 0;
                            i += 1;
                        }
                        // `-` — graphics new line (x = 0, next band)
                        b'-' => {
                            self.x = 0;
                            self.band += 1;
                            i += 1;
                        }
                        // Skip any other bytes (whitespace, etc.)
                        _ => {
                            i += 1;
                        }
                    }
                }
            }

            /// Finalise and return the decoded image.
            fn finish(mut self) -> Option<SixelImage> {
                if self.width == 0 || self.height == 0 {
                    return None;
                }

                // If declared dimensions are larger than actual content, the buffer was
                // already sized to declared dimensions.  If actual content exceeded the
                // declared dimensions, the buffer grew dynamically.  Either way, the
                // final dimensions are `self.width` x `self.height`.

                // Trim height to declared height if raster attributes specified it and
                // actual content didn't exceed it.
                if self.declared_height > 0 && self.declared_height < self.height {
                    let trimmed_size =
                        self.declared_width.max(self.width) * self.declared_height * 4;
                    self.pixels.truncate(trimmed_size);
                    self.height = self.declared_height;
                }

                // Trim width if declared and narrower than buffer.
                if self.declared_width > 0 && self.declared_width < self.width {
                    let new_width = self.declared_width;
                    let mut trimmed = vec![0u8; new_width * self.height * 4];
                    for y in 0..self.height {
                        let src_start = y * self.width * 4;
                        let dst_start = y * new_width * 4;
                        let copy_bytes = new_width * 4;
                        trimmed[dst_start..dst_start + copy_bytes]
                            .copy_from_slice(&self.pixels[src_start..src_start + copy_bytes]);
                    }
                    self.pixels = trimmed;
                    self.width = new_width;
                }

                Some(SixelImage {
                    pixels: self.pixels,
                    width: u32::value_from(self.width).ok()?,
                    height: u32::value_from(self.height).ok()?,
                })
            }
        }

        pub fn parse(inner: &[u8]) -> Option<SixelImage> {
            let q_pos = inner.iter().position(|&b| b == b'q')?;
            let params = parse_dcs_params(&inner[..q_pos]);
            let background = SixelBackground::from_param(params.p2);
            let mut decoder = RefDecoder::new(background);
            decoder.decode(&inner[q_pos + 1..]);
            decoder.finish()
        }
    }
}
