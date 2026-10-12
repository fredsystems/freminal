// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! The bounded decoder for icon bytes an OSC 99 program transmits.
//!
//! The bytes are untrusted terminal input. Every system leg decodes them
//! here, so the limits are enforced in one place: the Linux/BSD D-Bus leg
//! sends the decoded pixels as the `image-data` hint, and the macOS and
//! Windows untracked leg re-encodes them as a PNG before an OS image decoder
//! ever sees them (the OS decoders have their own, unbounded view of a
//! hostile file).

use image::imageops::FilterType;
use image::metadata::Orientation;
use image::{DynamicImage, ImageDecoder, RgbaImage};

/// An icon wider or taller than this is downscaled.
///
/// Notification daemons show icons at a small size, and the `image-data` hint
/// travels over the bus in every `Notify` call. Capping the icon at 256x256
/// bounds the RGBA at 256 KiB.
const DOWNSCALE_THRESHOLD: u32 = 256;

/// The size a downscaled icon is fitted within.
const DOWNSCALE_TARGET: u32 = 256;

/// The widest and tallest image the decoder will accept, declared in the
/// image's header. The icon bytes are untrusted terminal input, and a few
/// bytes of PNG can declare an image of billions of pixels; decoders check this
/// before allocating.
const MAX_DECODE_DIMENSION: u32 = 4096;

/// The most memory the decoder may allocate at one time, output image
/// included. Non-strict in the `image` crate (some decoders ignore it), so the
/// dimension limits above are the firm bound.
const MAX_DECODE_ALLOC_BYTES: u64 = 64 * 1024 * 1024;

/// Decode transmitted icon `bytes` (PNG, JPEG, GIF or WebP) to RGBA.
///
/// WebP is accepted although kitty's spec names only PNG: it goes through the
/// same bounded decoder, and being lenient about what a sender transmits costs
/// nothing.
///
/// Decoding is bounded by [`MAX_DECODE_DIMENSION`] and
/// [`MAX_DECODE_ALLOC_BYTES`]: an image over a limit is a decode failure. The
/// EXIF orientation, where the format carries one, is applied, so a photo
/// taken sideways is shown upright. An image wider or taller than 256 pixels
/// is downscaled to fit within 256x256, keeping its aspect ratio; the resize
/// runs on premultiplied alpha, so the colour of a fully transparent pixel
/// does not darken the edge of the opaque part. `None` when the bytes do not
/// decode or break a limit; the caller falls back to the next icon source (or
/// to no image).
#[must_use]
pub fn decode_icon_rgba(bytes: &[u8]) -> Option<RgbaImage> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    let mut limits = image::Limits::no_limits();
    limits.max_image_width = Some(MAX_DECODE_DIMENSION);
    limits.max_image_height = Some(MAX_DECODE_DIMENSION);
    limits.max_alloc = Some(MAX_DECODE_ALLOC_BYTES);
    reader.limits(limits);
    // `ImageReader::decode` reserves the output image against `max_alloc`; the
    // decoder route taken here (it exposes the orientation) must do it itself.
    let mut decoder = reader.into_decoder().ok()?;
    if decoder.total_bytes() > MAX_DECODE_ALLOC_BYTES {
        return None;
    }
    // A broken orientation tag is not worth losing the icon over.
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let mut dynamic = DynamicImage::from_decoder(decoder).ok()?;
    dynamic.apply_orientation(orientation);
    let upright = dynamic.into_rgba8();
    let (width, height) = upright.dimensions();
    Some(
        if width > DOWNSCALE_THRESHOLD || height > DOWNSCALE_THRESHOLD {
            downscale(upright)
        } else {
            upright
        },
    )
}

/// Decode transmitted icon `bytes` with [`decode_icon_rgba`] and encode the
/// result as a PNG no larger than 256x256.
///
/// This is what the macOS and Windows untracked leg hands to the OS: the image
/// the OS decodes is one freminal produced, never the transmitted bytes.
/// `None` when the bytes do not decode (or break a limit) or the encode fails;
/// the notification is then shown without an image.
#[cfg(any(target_os = "macos", target_os = "windows", test))]
#[must_use]
pub fn reencode_icon_png(bytes: &[u8]) -> Option<Vec<u8>> {
    let decoded = decode_icon_rgba(bytes)?;
    let mut png = std::io::Cursor::new(Vec::new());
    decoded.write_to(&mut png, image::ImageFormat::Png).ok()?;
    Some(png.into_inner())
}

/// `image` fitted within [`DOWNSCALE_TARGET`] squared, keeping its aspect
/// ratio. The filter averages colour channels without weighting them by
/// alpha, so the image is premultiplied around it: otherwise the colour
/// stored under transparent pixels bleeds into the edge.
fn downscale(mut image: RgbaImage) -> RgbaImage {
    let (width, height) = image.dimensions();
    let (target_width, target_height) = fit_within_target(width, height);
    premultiply_alpha(&mut image);
    let mut resized =
        image::imageops::resize(&image, target_width, target_height, FilterType::Triangle);
    unpremultiply_alpha(&mut resized);
    resized
}

/// Scale every pixel's colour by its alpha, rounding to nearest.
fn premultiply_alpha(image: &mut RgbaImage) {
    for pixel in image.pixels_mut() {
        let alpha = u32::from(pixel.0[3]);
        for channel in &mut pixel.0[..3] {
            let scaled = (u32::from(*channel) * alpha + 127) / 255;
            *channel = u8::try_from(scaled).unwrap_or(u8::MAX);
        }
    }
}

/// Undo [`premultiply_alpha`]. A fully transparent pixel has no colour left.
fn unpremultiply_alpha(image: &mut RgbaImage) {
    for pixel in image.pixels_mut() {
        let alpha = u32::from(pixel.0[3]);
        for channel in &mut pixel.0[..3] {
            let restored = (u32::from(*channel) * 255 + alpha / 2)
                .checked_div(alpha)
                .unwrap_or(0);
            *channel = u8::try_from(restored.min(255)).unwrap_or(u8::MAX);
        }
    }
}

/// `width` x `height` scaled to fit within [`DOWNSCALE_TARGET`] squared,
/// keeping the aspect ratio and never collapsing a side to nothing.
fn fit_within_target(width: u32, height: u32) -> (u32, u32) {
    let scaled = |side: u32, longest: u32| -> u32 {
        let scaled = u64::from(side) * u64::from(DOWNSCALE_TARGET) / u64::from(longest.max(1));
        u32::try_from(scaled).unwrap_or(DOWNSCALE_TARGET).max(1)
    };
    let longest = width.max(height);
    (scaled(width, longest), scaled(height, longest))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode_png(pixels: &RgbaImage) -> Result<Vec<u8>, image::ImageError> {
        let mut bytes = Vec::new();
        pixels.write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )?;
        Ok(bytes)
    }

    /// A `width` x `height` PNG of one opaque colour.
    fn png(width: u32, height: u32) -> Result<Vec<u8>, image::ImageError> {
        encode_png(&RgbaImage::from_pixel(
            width,
            height,
            image::Rgba([10, 20, 30, 255]),
        ))
    }

    #[test]
    fn a_small_png_decodes_to_rgba_at_its_own_size() -> Result<(), Box<dyn std::error::Error>> {
        let image = decode_icon_rgba(&png(3, 2)?).ok_or("did not decode")?;

        assert_eq!(image.dimensions(), (3, 2));
        assert_eq!(image.get_pixel(0, 0).0, [10, 20, 30, 255]);
        Ok(())
    }

    #[test]
    fn an_image_of_exactly_256_is_not_downscaled() -> Result<(), Box<dyn std::error::Error>> {
        let image = decode_icon_rgba(&png(256, 1)?).ok_or("did not decode")?;
        assert_eq!(image.dimensions(), (256, 1));
        let image = decode_icon_rgba(&png(256, 256)?).ok_or("did not decode")?;
        assert_eq!(image.dimensions(), (256, 256));
        Ok(())
    }

    #[test]
    fn an_image_of_257_is_downscaled() -> Result<(), Box<dyn std::error::Error>> {
        let image = decode_icon_rgba(&png(257, 257)?).ok_or("did not decode")?;
        assert_eq!(image.dimensions(), (256, 256));
        let image = decode_icon_rgba(&png(1, 257)?).ok_or("did not decode")?;
        assert_eq!(image.dimensions(), (1, 256));
        Ok(())
    }

    /// `png` with its IHDR dimensions replaced (and the IHDR CRC recomputed),
    /// without any pixel data for them: the bytes stay tiny whatever the
    /// declared size.
    fn with_declared_size(mut png: Vec<u8>, width: u32, height: u32) -> Option<Vec<u8>> {
        // Signature (8), length (4), "IHDR" (4), width, height, ..., CRC at 29.
        png.get_mut(16..20)?.copy_from_slice(&width.to_be_bytes());
        png.get_mut(20..24)?.copy_from_slice(&height.to_be_bytes());
        let crc = crc32(png.get(12..29)?);
        png.get_mut(29..33)?.copy_from_slice(&crc.to_be_bytes());
        Some(png)
    }

    /// The CRC-32 PNG chunks carry.
    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = u32::MAX;
        for byte in bytes {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                crc = if crc & 1 == 1 {
                    (crc >> 1) ^ 0xedb8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }

    #[test]
    fn a_header_declaring_a_huge_image_is_rejected_before_any_allocation()
    -> Result<(), Box<dyn std::error::Error>> {
        let tiny = png(1, 1)?;

        // The patching itself is sound: an unchanged size still decodes.
        let unchanged = with_declared_size(tiny.clone(), 1, 1).ok_or("not a png")?;
        assert!(decode_icon_rgba(&unchanged).is_some());

        for (width, height) in [(8192, 8192), (30_000, 30_000), (8192, 1), (1, 8192)] {
            let bomb = with_declared_size(tiny.clone(), width, height).ok_or("not a png")?;
            assert!(bomb.len() < 100, "the bomb must stay tiny");
            assert!(decode_icon_rgba(&bomb).is_none(), "{width}x{height}");
            assert_eq!(reencode_icon_png(&bomb), None, "{width}x{height}");
        }
        Ok(())
    }

    #[test]
    fn the_dimension_limit_is_4096_per_side() -> Result<(), Box<dyn std::error::Error>> {
        let at_limit = decode_icon_rgba(&png(4096, 1)?).ok_or("did not decode")?;
        assert_eq!(at_limit.dimensions(), (256, 1));

        assert!(decode_icon_rgba(&png(4097, 1)?).is_none());
        assert!(decode_icon_rgba(&png(1, 4097)?).is_none());
        Ok(())
    }

    #[test]
    fn a_large_image_is_downscaled_to_fit_256_keeping_aspect()
    -> Result<(), Box<dyn std::error::Error>> {
        let image = decode_icon_rgba(&png(2000, 1000)?).ok_or("did not decode")?;

        assert_eq!(image.dimensions(), (256, 128));
        Ok(())
    }

    #[test]
    fn a_tall_large_image_is_downscaled_by_its_height() -> Result<(), Box<dyn std::error::Error>> {
        let image = decode_icon_rgba(&png(100, 1100)?).ok_or("did not decode")?;

        assert_eq!(image.dimensions(), (23, 256));
        Ok(())
    }

    #[test]
    fn an_extreme_aspect_ratio_keeps_one_pixel() {
        assert_eq!(fit_within_target(5000, 1), (256, 1));
    }

    #[test]
    fn undecodable_bytes_give_nothing() {
        assert!(decode_icon_rgba(b"not an image").is_none());
        assert!(decode_icon_rgba(&[]).is_none());
        assert_eq!(reencode_icon_png(b"not an image"), None);
        assert_eq!(reencode_icon_png(&[]), None);
    }

    /// A 512x512 image, fully transparent except an opaque white square, with
    /// black stored under the transparent pixels (as many encoders do).
    fn white_square_on_black_transparent() -> RgbaImage {
        let mut pixels = RgbaImage::from_pixel(512, 512, image::Rgba([0, 0, 0, 0]));
        for y in 100..401 {
            for x in 100..401 {
                pixels.put_pixel(x, y, image::Rgba([255, 255, 255, 255]));
            }
        }
        pixels
    }

    #[test]
    fn downscaling_does_not_darken_the_edge_of_a_transparent_image()
    -> Result<(), Box<dyn std::error::Error>> {
        let bytes = encode_png(&white_square_on_black_transparent())?;

        let image = decode_icon_rgba(&bytes).ok_or("did not decode")?;

        assert_eq!(image.dimensions(), (256, 256));
        let mut partly_transparent = 0;
        let mut opaque = 0;
        for pixel in image.pixels() {
            if pixel.0[3] > 0 {
                assert!(
                    pixel.0[0] >= 250 && pixel.0[1] >= 250 && pixel.0[2] >= 250,
                    "a visible pixel {pixel:?} is darker than its source"
                );
                if pixel.0[3] < 255 {
                    partly_transparent += 1;
                } else {
                    opaque += 1;
                }
            }
        }
        // The check above is not vacuous: there is an opaque core and an edge.
        assert!(opaque > 0);
        assert!(partly_transparent > 0);
        Ok(())
    }

    #[test]
    fn premultiplying_and_restoring_keeps_opaque_and_clears_transparent_pixels() {
        let mut pixels = RgbaImage::new(3, 1);
        pixels.put_pixel(0, 0, image::Rgba([10, 20, 30, 255]));
        pixels.put_pixel(1, 0, image::Rgba([200, 100, 50, 0]));
        pixels.put_pixel(2, 0, image::Rgba([255, 255, 255, 128]));

        premultiply_alpha(&mut pixels);

        assert_eq!(pixels.get_pixel(0, 0).0, [10, 20, 30, 255]);
        assert_eq!(pixels.get_pixel(1, 0).0, [0, 0, 0, 0]);
        assert_eq!(pixels.get_pixel(2, 0).0, [128, 128, 128, 128]);

        unpremultiply_alpha(&mut pixels);

        assert_eq!(pixels.get_pixel(0, 0).0, [10, 20, 30, 255]);
        assert_eq!(pixels.get_pixel(1, 0).0, [0, 0, 0, 0]);
        assert_eq!(pixels.get_pixel(2, 0).0, [255, 255, 255, 128]);
    }

    /// The bytes of a little-endian EXIF block that holds only the
    /// orientation tag.
    fn exif_with_orientation(orientation: u16) -> Vec<u8> {
        let mut exif = vec![b'I', b'I', 42, 0];
        exif.extend_from_slice(&8u32.to_le_bytes()); // offset of the first IFD
        exif.extend_from_slice(&1u16.to_le_bytes()); // one entry
        exif.extend_from_slice(&0x0112u16.to_le_bytes()); // Orientation
        exif.extend_from_slice(&3u16.to_le_bytes()); // SHORT
        exif.extend_from_slice(&1u32.to_le_bytes()); // count
        exif.extend_from_slice(&orientation.to_le_bytes());
        exif.extend_from_slice(&0u16.to_le_bytes()); // padding
        exif.extend_from_slice(&0u32.to_le_bytes()); // no next IFD
        exif
    }

    /// A 4x2 PNG carrying `orientation` in an `eXIf` chunk, red at its top
    /// left and blue everywhere else.
    fn png_with_orientation(orientation: u16) -> Result<Vec<u8>, image::ImageError> {
        use image::ImageEncoder;

        let mut pixels = RgbaImage::from_pixel(4, 2, image::Rgba([0, 0, 255, 255]));
        pixels.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));
        let mut bytes = Vec::new();
        let mut encoder = image::codecs::png::PngEncoder::new(&mut bytes);
        encoder
            .set_exif_metadata(exif_with_orientation(orientation))
            .map_err(image::ImageError::Unsupported)?;
        encoder.write_image(pixels.as_raw(), 4, 2, image::ExtendedColorType::Rgba8)?;
        Ok(bytes)
    }

    /// The `(x, y)` of the red pixel of `image`.
    fn red_pixel(image: &RgbaImage) -> Option<(u32, u32)> {
        image
            .enumerate_pixels()
            .find(|(_, _, pixel)| pixel.0 == [255, 0, 0, 255])
            .map(|(x, y, _)| (x, y))
    }

    #[test]
    fn an_image_without_an_orientation_tag_is_left_as_it_is()
    -> Result<(), Box<dyn std::error::Error>> {
        let image = decode_icon_rgba(&png_with_orientation(1)?).ok_or("did not decode")?;

        assert_eq!(image.dimensions(), (4, 2));
        assert_eq!(red_pixel(&image), Some((0, 0)));
        Ok(())
    }

    #[test]
    fn the_exif_orientation_is_applied() -> Result<(), Box<dyn std::error::Error>> {
        // Orientation 6: rotate 90 degrees clockwise. The 4x2 image becomes
        // 2x4 and its top-left pixel moves to the top-right.
        let image = decode_icon_rgba(&png_with_orientation(6)?).ok_or("did not decode")?;
        assert_eq!(image.dimensions(), (2, 4));
        assert_eq!(red_pixel(&image), Some((1, 0)));

        // Orientation 3: rotate 180. The top-left pixel moves to the
        // bottom-right.
        let image = decode_icon_rgba(&png_with_orientation(3)?).ok_or("did not decode")?;
        assert_eq!(image.dimensions(), (4, 2));
        assert_eq!(red_pixel(&image), Some((3, 1)));

        // Orientation 2: mirror horizontally.
        let image = decode_icon_rgba(&png_with_orientation(2)?).ok_or("did not decode")?;
        assert_eq!(red_pixel(&image), Some((3, 0)));
        Ok(())
    }

    #[test]
    fn an_orientation_tag_outside_the_valid_range_is_ignored()
    -> Result<(), Box<dyn std::error::Error>> {
        let image = decode_icon_rgba(&png_with_orientation(9)?).ok_or("did not decode")?;

        assert_eq!(image.dimensions(), (4, 2));
        assert_eq!(red_pixel(&image), Some((0, 0)));
        Ok(())
    }

    // ── re-encoding ─────────────────────────────────────────────────────

    #[test]
    fn reencoding_gives_a_png_of_the_decoded_image() -> Result<(), Box<dyn std::error::Error>> {
        let bytes = reencode_icon_png(&png(3, 2)?).ok_or("did not re-encode")?;

        assert!(bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
        let decoded = image::load_from_memory(&bytes)?.to_rgba8();
        assert_eq!(decoded.dimensions(), (3, 2));
        assert_eq!(decoded.get_pixel(1, 1).0, [10, 20, 30, 255]);
        Ok(())
    }

    #[test]
    fn reencoding_bounds_the_size_to_256() -> Result<(), Box<dyn std::error::Error>> {
        let bytes = reencode_icon_png(&png(2000, 1000)?).ok_or("did not re-encode")?;

        let decoded = image::load_from_memory(&bytes)?;
        assert_eq!((decoded.width(), decoded.height()), (256, 128));
        Ok(())
    }

    #[test]
    fn reencoding_applies_the_orientation_and_drops_the_metadata()
    -> Result<(), Box<dyn std::error::Error>> {
        let bytes = reencode_icon_png(&png_with_orientation(6)?).ok_or("did not re-encode")?;

        let decoded = decode_icon_rgba(&bytes).ok_or("did not decode")?;
        assert_eq!(decoded.dimensions(), (2, 4));
        assert_eq!(red_pixel(&decoded), Some((1, 0)));
        assert!(
            !bytes.windows(4).any(|window| window == b"eXIf"),
            "the EXIF block must not survive the re-encode"
        );
        Ok(())
    }

    #[test]
    fn a_jpeg_is_reencoded_to_png() -> Result<(), Box<dyn std::error::Error>> {
        let mut jpeg = Vec::new();
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(8, 8, image::Rgb([1, 2, 3])))
            .write_to(
                &mut std::io::Cursor::new(&mut jpeg),
                image::ImageFormat::Jpeg,
            )?;

        let bytes = reencode_icon_png(&jpeg).ok_or("did not re-encode")?;

        assert!(bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
        Ok(())
    }
}
