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

/// Maximum number of palette entries supported.
pub const MAX_PALETTE: usize = 256;

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
pub enum SixelBackground {
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
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
const fn f64_to_u8(val: f64) -> u8 {
    val.clamp(0.0, 255.0) as u8
}

/// Convert a `u32` to `usize` without truncation risk on 32/64-bit platforms.
#[allow(clippy::cast_possible_truncation)]
const fn usize_from_u32(val: u32) -> usize {
    val as usize
}

/// Convert RGB percentage values (0..=100) to byte values (0..=255).
fn pct_to_rgb(rp: u32, gp: u32, bp: u32) -> (u8, u8, u8) {
    let convert = |pct: u32| -> u8 {
        #[allow(clippy::cast_possible_truncation)]
        let val = ((pct.min(100) * 255 + 50) / 100) as u8;
        val
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
                    for pixel in buf.chunks_exact_mut(4) {
                        pixel[0] = r;
                        pixel[1] = g;
                        pixel[2] = b;
                        pixel[3] = 255;
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
                for pixel in buf.chunks_exact_mut(4) {
                    pixel[0] = r;
                    pixel[1] = g;
                    pixel[2] = b;
                    pixel[3] = 255;
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
            let trimmed_size = self.declared_width.max(self.width) * self.declared_height * 4;
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

        #[allow(clippy::cast_possible_truncation)]
        Some(SixelImage {
            pixels: self.pixels,
            width: self.width as u32,
            height: self.height as u32,
        })
    }

    /// Finalise and return the decoded image together with the final palette.
    ///
    /// Used when `?1070` is reset (shared color registers): the caller can
    /// persist the returned palette and pass it to the next image via
    /// `parse_sixel_with_shared_palette`.
    fn finish_into_parts(mut self) -> (Option<SixelImage>, [(u8, u8, u8); MAX_PALETTE]) {
        let palette = self.palette;

        if self.width == 0 || self.height == 0 {
            return (None, palette);
        }

        if self.declared_height > 0 && self.declared_height < self.height {
            let trimmed_size = self.declared_width.max(self.width) * self.declared_height * 4;
            self.pixels.truncate(trimmed_size);
            self.height = self.declared_height;
        }

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

        #[allow(clippy::cast_possible_truncation)]
        let image = SixelImage {
            pixels: self.pixels,
            width: self.width as u32,
            height: self.height as u32,
        };
        (Some(image), palette)
    }
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
#[allow(clippy::unwrap_used, clippy::expect_used)]
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
}
