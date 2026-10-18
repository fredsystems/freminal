//! Rasterises a [`ChromeIcon`] glyph to a PNG (macOS and Windows).
//!
//! The macOS and Windows notification APIs take an icon as an image file, and
//! have no notion of a freedesktop icon name. The OSC 99 standard icon names
//! (`error`, `warn`, `info`, ...) are therefore drawn from the same bundled
//! `CaskaydiaCove` glyphs the in-app toasts use, rendered once with `swash` to
//! a square RGBA image and encoded as PNG for the icon temp-file store.
//!
//! So the icon reads on any notification background, the glyph is white
//! ([`GLYPH_RGB`], coverage as alpha) on a fully opaque mid-grey rounded square
//! tile ([`TILE_RGB`]) rather than a bare glyph on the platform's own colour.

use std::io::Cursor;

use conv2::{ApproxFrom, RoundToNearest};
use image::{ImageBuffer, ImageFormat, Rgba};
use swash::FontRef;
use swash::scale::image::Content;
use swash::scale::{Render, ScaleContext, Source};
use swash::zeno::Format;

use crate::gui::font_manager::bundled_regular_font_bytes;
use crate::gui::icons::ChromeIcon;

/// The tile colour. The tile is fully opaque inside its rounded outline.
const TILE_RGB: [u8; 3] = [0x5A, 0x5A, 0x5A];

/// The glyph colour. Coverage is the blend weight over the tile.
const GLYPH_RGB: [u8; 3] = [0xFF, 0xFF, 0xFF];

/// The margin left clear around the tile on every side, as a fraction of the
/// image size: `size_px / MARGIN_DIVISOR`.
const MARGIN_DIVISOR: u32 = 16;

/// The tile's corner radius, as a fraction of the image size:
/// `size_px / CORNER_DIVISOR`.
const CORNER_DIVISOR: u32 = 6;

/// The padding between the tile edge and the glyph, as a fraction of the tile
/// side: `tile_side / GLYPH_PADDING_DIVISOR`.
const GLYPH_PADDING_DIVISOR: u32 = 8;

/// The largest image [`rasterize_chrome_icon_png`] will draw, in pixels per
/// side. The only callers ask for 256; the cap keeps a future caller from
/// allocating an unbounded canvas.
pub const MAX_RASTER_SIZE_PX: u32 = 1024;

/// Render `icon` into a `size_px` by `size_px` RGBA PNG.
///
/// A rounded square tile fills the image inside a margin of `size_px / 16`
/// (corner radius `size_px / 6`), so the outermost pixels are transparent. The
/// white glyph is scaled to fit the tile's interior with its own padding and
/// is centred on it. Returns `None` if the size is zero or over
/// [`MAX_RASTER_SIZE_PX`], the bundled face has no glyph for the icon, or
/// rendering or encoding fails; the reason is logged at debug level.
#[must_use]
pub fn rasterize_chrome_icon_png(icon: ChromeIcon, size_px: u32) -> Option<Vec<u8>> {
    let result = render_png(icon, size_px);
    if result.is_none() {
        tracing::debug!("OSC 99: could not rasterise {icon:?} at {size_px} px");
    }
    result
}

/// [`rasterize_chrome_icon_png`] without the logging.
fn render_png(icon: ChromeIcon, size_px: u32) -> Option<Vec<u8>> {
    if size_px == 0 || size_px > MAX_RASTER_SIZE_PX {
        return None;
    }
    let font = FontRef::from_index(bundled_regular_font_bytes(), 0)?;
    let glyph_id = font.charmap().map(icon.codepoint());
    if glyph_id == 0 {
        return None;
    }

    let mut context = ScaleContext::new();
    let mut render_at = |ppem: f32| {
        let mut scaler = context.builder(font).size(ppem).hint(false).build();
        Render::new(&[Source::Outline])
            .format(Format::Alpha)
            .render(&mut scaler, glyph_id)
    };

    let margin = size_px / MARGIN_DIVISOR;
    let tile_side = size_px.saturating_sub(2 * margin);
    let padding = tile_side / GLYPH_PADDING_DIVISOR;
    let target = tile_side.saturating_sub(2 * padding);

    // Probe at a size equal to the image, then rescale so the bitmap's longer
    // side fills the tile's interior inside the glyph padding.
    let probe_ppem = f32::approx_from(size_px).ok()?;
    let probe = render_at(probe_ppem)?;
    let longest = probe.placement.width.max(probe.placement.height);
    if longest == 0 {
        return None;
    }
    let ppem = probe_ppem * f32::approx_from(target).ok()? / f32::approx_from(longest).ok()?;

    let glyph = render_at(ppem)?;
    if glyph.content != Content::Mask {
        return None;
    }
    let (width, height) = (glyph.placement.width, glyph.placement.height);
    if width == 0 || height == 0 || width > size_px || height > size_px {
        return None;
    }

    let mut canvas = ImageBuffer::<Rgba<u8>, Vec<u8>>::new(size_px, size_px);
    let tile = TileShape::new(size_px, margin, size_px / CORNER_DIVISOR)?;
    for (x, y, pixel) in canvas.enumerate_pixels_mut() {
        let alpha = tile.alpha_at(x, y)?;
        *pixel = Rgba([TILE_RGB[0], TILE_RGB[1], TILE_RGB[2], alpha]);
    }

    let x_origin = (size_px - width) / 2;
    let y_origin = (size_px - height) / 2;
    let rows = glyph.data.chunks_exact(usize::try_from(width).ok()?);
    for (y, row) in (y_origin..).zip(rows) {
        for (x, &coverage) in (x_origin..).zip(row) {
            let pixel = canvas.get_pixel_mut_checked(x, y)?;
            for channel in 0..3 {
                pixel.0[channel] = blend(TILE_RGB[channel], GLYPH_RGB[channel], coverage);
            }
        }
    }

    let mut png = Cursor::new(Vec::new());
    canvas.write_to(&mut png, ImageFormat::Png).ok()?;
    Some(png.into_inner())
}

/// `below` with `above` laid over it at `coverage / 255` weight.
fn blend(below: u8, above: u8, coverage: u8) -> u8 {
    let coverage = u32::from(coverage);
    let mixed = (u32::from(below) * (255 - coverage) + u32::from(above) * coverage + 127) / 255;
    u8::try_from(mixed).unwrap_or(u8::MAX)
}

/// The rounded square tile, as a signed-distance outline.
struct TileShape {
    centre: f32,
    /// Half the tile side minus the corner radius: the straight-edge reach of
    /// the shape's inner rectangle.
    inner_half: f32,
    radius: f32,
}

impl TileShape {
    /// A tile inset by `margin` from a `size_px` square, with corner `radius`.
    fn new(size_px: u32, margin: u32, radius: u32) -> Option<Self> {
        let side = size_px.saturating_sub(2 * margin);
        let radius = radius.min(side / 2);
        Some(Self {
            centre: f32::approx_from(size_px).ok()? / 2.0,
            inner_half: f32::approx_from(side / 2 - radius).ok()?,
            radius: f32::approx_from(radius).ok()?,
        })
    }

    /// The tile's alpha at pixel `(x, y)`: 255 inside, 0 outside, and a one
    /// pixel anti-aliased ramp across the curved corners. Straight edges fall
    /// on pixel boundaries, so they stay crisp.
    fn alpha_at(&self, x: u32, y: u32) -> Option<u8> {
        let dx = (f32::approx_from(x).ok()? + 0.5 - self.centre).abs() - self.inner_half;
        let dy = (f32::approx_from(y).ok()? + 0.5 - self.centre).abs() - self.inner_half;
        let distance = dx.max(0.0).hypot(dy.max(0.0)) + dx.max(dy).min(0.0) - self.radius;
        let coverage = (0.5 - distance).clamp(0.0, 1.0);
        <u8 as ApproxFrom<f32, RoundToNearest>>::approx_from(coverage * 255.0).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STANDARD_ICONS: [ChromeIcon; 8] = [
        ChromeIcon::Error,
        ChromeIcon::Warning,
        ChromeIcon::Info,
        ChromeIcon::Question,
        ChromeIcon::Help,
        ChromeIcon::FileManager,
        ChromeIcon::SystemMonitor,
        ChromeIcon::TextEditor,
    ];

    fn decode(png: &[u8]) -> image::RgbaImage {
        image::load_from_memory_with_format(png, ImageFormat::Png)
            .unwrap()
            .to_rgba8()
    }

    #[test]
    fn every_standard_icon_rasterises_to_a_256_px_png() {
        for icon in STANDARD_ICONS {
            let png = rasterize_chrome_icon_png(icon, 256)
                .unwrap_or_else(|| panic!("{icon:?} did not rasterise"));
            let image = decode(&png);
            assert_eq!(image.dimensions(), (256, 256), "{icon:?}");
            assert!(
                image.pixels().any(|pixel| pixel.0[3] > 0),
                "{icon:?} has no visible pixels"
            );
        }
    }

    #[test]
    fn pixels_outside_the_tile_are_transparent() {
        for icon in STANDARD_ICONS {
            let image = decode(&rasterize_chrome_icon_png(icon, 256).unwrap());
            for (x, y, pixel) in image.enumerate_pixels() {
                let in_margin = x < 16 || y < 16 || x >= 240 || y >= 240;
                if in_margin {
                    assert_eq!(pixel.0[3], 0, "{icon:?} margin pixel ({x}, {y})");
                }
            }
            // The rounded corners cut the tile's own corner pixels away.
            assert_eq!(image.get_pixel(16, 16).0[3], 0, "{icon:?} tile corner");
            assert_eq!(image.get_pixel(239, 239).0[3], 0, "{icon:?} tile corner");
        }
    }

    #[test]
    fn the_tile_is_opaque_grey_wherever_the_glyph_is_absent() {
        for icon in STANDARD_ICONS {
            let image = decode(&rasterize_chrome_icon_png(icon, 256).unwrap());
            // Inside the tile and outside the glyph's padded box, straight
            // along each edge's midpoint and at the curve's start.
            for (x, y) in [(16, 128), (239, 128), (128, 16), (128, 239), (60, 20)] {
                assert_eq!(
                    image.get_pixel(x, y).0,
                    [TILE_RGB[0], TILE_RGB[1], TILE_RGB[2], 255],
                    "{icon:?} tile pixel ({x}, {y})"
                );
            }
            // No pixel inside the tile is translucent or darker than the tile.
            for (x, y, pixel) in image.enumerate_pixels() {
                let deep_inside = (40..216).contains(&x) && (40..216).contains(&y);
                if deep_inside {
                    assert_eq!(pixel.0[3], 255, "{icon:?} pixel ({x}, {y})");
                    assert!(pixel.0[0] >= TILE_RGB[0], "{icon:?} pixel ({x}, {y})");
                }
            }
        }
    }

    #[test]
    fn the_glyph_is_lighter_than_the_tile() {
        for icon in STANDARD_ICONS {
            let image = decode(&rasterize_chrome_icon_png(icon, 256).unwrap());
            assert!(
                image.pixels().any(|pixel| pixel.0[0] > 0xC0),
                "{icon:?} has no light glyph pixels"
            );
            assert!(
                image
                    .pixels()
                    .any(|pixel| pixel.0[3] == 255 && pixel.0[0] > TILE_RGB[0]),
                "{icon:?} has no pixel lighter than the tile"
            );
        }
    }

    #[test]
    fn the_glyph_is_centred_and_padded_inside_the_tile() {
        let image = decode(&rasterize_chrome_icon_png(ChromeIcon::Info, 256).unwrap());
        let lit: Vec<(u32, u32)> = image
            .enumerate_pixels()
            .filter(|(_, _, pixel)| pixel.0[0] > TILE_RGB[0])
            .map(|(x, y, _)| (x, y))
            .collect();
        let min_x = lit.iter().map(|p| p.0).min().unwrap();
        let max_x = lit.iter().map(|p| p.0).max().unwrap();
        let min_y = lit.iter().map(|p| p.1).min().unwrap();
        let max_y = lit.iter().map(|p| p.1).max().unwrap();

        // Left/right and top/bottom clearances match to within a pixel.
        assert!(min_x.abs_diff(255 - max_x) <= 1, "{min_x} {max_x}");
        assert!(min_y.abs_diff(255 - max_y) <= 1, "{min_y} {max_y}");
        // The glyph sits inside the tile's 28 px padding (224 / 8), and its
        // longer side fills the area inside that padding.
        assert!(min_x.min(min_y) >= 16 + 28 - 1);
        assert!((max_x - min_x).max(max_y - min_y) >= 224 - 2 * 28 - 2);
    }

    #[test]
    fn the_size_is_honoured() {
        let image = decode(&rasterize_chrome_icon_png(ChromeIcon::Warning, 64).unwrap());
        assert_eq!(image.dimensions(), (64, 64));
        assert!(image.pixels().any(|pixel| pixel.0[3] > 0));
    }

    #[test]
    fn a_zero_size_has_no_image() {
        assert_eq!(rasterize_chrome_icon_png(ChromeIcon::Info, 0), None);
    }

    #[test]
    fn a_size_over_the_cap_has_no_image_and_the_cap_itself_is_drawn() {
        assert_eq!(
            rasterize_chrome_icon_png(ChromeIcon::Info, MAX_RASTER_SIZE_PX + 1),
            None
        );
        assert_eq!(rasterize_chrome_icon_png(ChromeIcon::Info, u32::MAX), None);
        let at_cap = rasterize_chrome_icon_png(ChromeIcon::Info, MAX_RASTER_SIZE_PX).unwrap();
        assert_eq!(
            decode(&at_cap).dimensions(),
            (MAX_RASTER_SIZE_PX, MAX_RASTER_SIZE_PX)
        );
    }
}
