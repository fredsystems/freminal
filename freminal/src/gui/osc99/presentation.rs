// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! How an OSC 99 notification is presented on the Linux/BSD system leg.
//!
//! Pure functions that turn what the program asked for (`n=` icon names,
//! transmitted icon data, `s=` sound, `t=` types, the body text) into what the
//! freedesktop notification spec has a place for. The filesystem is reached
//! only through [`Osc99IconLookup`], which tests replace with a fake.

use std::collections::VecDeque;
use std::hash::{DefaultHasher, Hash, Hasher};

use freminal_common::buffer_states::osc99_notification::Osc99Sound;
use image::imageops::FilterType;
use image::metadata::Orientation;
use image::{DynamicImage, ImageDecoder, RgbaImage};

/// An icon wider or taller than this is downscaled before it is sent.
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

/// How many decoded icons [`Osc99DecodedIconCache`] keeps.
const DECODED_ICON_CACHE_ENTRIES: usize = 8;

/// Zero width non-joiner, inserted after the characters kitty escapes in a
/// body for daemons that interpret markup.
const ZERO_WIDTH_NON_JOINER: char = '\u{200c}';

/// What the icon scan and the `.desktop` scan know about the system.
pub trait Osc99IconLookup {
    /// The icon the icon scan found for `name` (case-insensitive), as the
    /// icon file's stem is spelled on disk, or `None` when there is no such
    /// icon. The on-disk spelling is what the daemon must be sent: icon lookup
    /// in a theme is case-sensitive.
    fn matched_icon(&self, name: &str) -> Option<String>;

    /// The `Icon=` of the `.desktop` file whose stem or `Name=` equals
    /// `app_name` (case-insensitive).
    fn desktop_icon(&self, app_name: &str) -> Option<String>;
}

/// The icon the notification is shown with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Osc99IconChoice {
    /// A freedesktop icon name, sent as the `app_icon` argument.
    Named(String),
    /// Transmitted icon data, sent as the `image-data` hint.
    Data(Vec<u8>),
}

/// The sound hint a notification gets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Osc99SoundHint {
    /// No sound hint: the daemon decides.
    None,
    /// `suppress-sound`.
    Suppress,
    /// `sound-name` with this freedesktop sound name.
    Named(String),
}

/// Decoded icon pixels in the layout of the `image-data` hint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Osc99IconImage {
    /// Width in pixels.
    pub width: i32,
    /// Height in pixels.
    pub height: i32,
    /// Bytes per row (`width * 4`; the pixels are tightly packed).
    pub rowstride: i32,
    /// RGBA, 8 bits per channel, row by row.
    pub rgba: Vec<u8>,
}

/// The freedesktop icon name for kitty's standard icon `name`, if it is one.
#[must_use]
pub fn standard_icon_name(name: &str) -> Option<&'static str> {
    match name {
        "error" => Some("dialog-error"),
        "warn" | "warning" => Some("dialog-warning"),
        "info" => Some("dialog-information"),
        "question" => Some("dialog-question"),
        "help" => Some("system-help"),
        "file-manager" => Some("system-file-manager"),
        "system-monitor" => Some("utilities-system-monitor"),
        "text-editor" => Some("utilities-text-editor"),
        _ => None,
    }
}

/// Pick the icon, in kitty's order.
///
/// - Each name in `names`, in order: a standard name maps to its freedesktop
///   name; any other name is used, as `lookup` spells it, when `lookup` finds
///   it.
/// - If no name matched: the transmitted `data`, else the first name verbatim
///   (the daemon may know an icon the scan did not).
/// - With no names: `data`, else the icon of the `.desktop` file matching
///   `app_name`.
///
/// `None` means no rule found an icon: the caller falls back to freminal's own.
#[must_use]
pub fn choose_icon(
    names: &[String],
    data: Option<&[u8]>,
    app_name: Option<&str>,
    lookup: &dyn Osc99IconLookup,
) -> Option<Osc99IconChoice> {
    for name in names {
        if let Some(standard) = standard_icon_name(name) {
            return Some(Osc99IconChoice::Named(standard.to_owned()));
        }
        if let Some(matched) = lookup.matched_icon(name) {
            return Some(Osc99IconChoice::Named(matched));
        }
    }
    if let Some(data) = data {
        return Some(Osc99IconChoice::Data(data.to_vec()));
    }
    if let Some(first) = names.first() {
        return Some(Osc99IconChoice::Named(first.clone()));
    }
    app_name
        .and_then(|app| lookup.desktop_icon(app))
        .map(Osc99IconChoice::Named)
}

/// The sound hint for `sound`.
///
/// Kitty plays named sounds through libcanberra; freminal has no sound
/// library and uses the freedesktop hints instead, so the standard names map
/// to the freedesktop sound names and any other name is sent verbatim.
#[must_use]
pub fn sound_hint(sound: &Osc99Sound) -> Osc99SoundHint {
    match sound {
        Osc99Sound::System => Osc99SoundHint::None,
        Osc99Sound::Silent => Osc99SoundHint::Suppress,
        Osc99Sound::Named(name) => Osc99SoundHint::Named(match name.as_str() {
            "error" => "dialog-error".to_owned(),
            "warn" | "warning" => "dialog-warning".to_owned(),
            "info" => "dialog-information".to_owned(),
            "question" => "dialog-question".to_owned(),
            other => other.to_owned(),
        }),
    }
}

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
/// decode (or break a limit) or the dimensions do not fit the hint's `i32`
/// fields; the caller falls back to the next icon source.
#[must_use]
pub fn decode_icon_image(bytes: &[u8]) -> Option<Osc99IconImage> {
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
    let rgba = if width > DOWNSCALE_THRESHOLD || height > DOWNSCALE_THRESHOLD {
        downscale(upright)
    } else {
        upright
    };
    let (width, height) = rgba.dimensions();
    let rowstride = width.checked_mul(4)?;
    Some(Osc99IconImage {
        width: i32::try_from(width).ok()?,
        height: i32::try_from(height).ok()?,
        rowstride: i32::try_from(rowstride).ok()?,
        rgba: rgba.into_raw(),
    })
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

/// What identifies a transmitted icon: a hash of its bytes and their length.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct IconKey {
    hash: u64,
    len: usize,
}

impl IconKey {
    fn of(bytes: &[u8]) -> Self {
        let mut hasher = DefaultHasher::new();
        bytes.hash(&mut hasher);
        Self {
            hash: hasher.finish(),
            len: bytes.len(),
        }
    }
}

/// The last few decoded icons, so a program that sends the same icon with
/// every notification does not have it decoded and downscaled each time.
///
/// Least recently used first out, keyed by a hash of the transmitted bytes. It
/// is owned by one thread (the dispatcher), so there is no locking. A decode
/// failure is not remembered: it is cheap, and bounded by the decode limits.
#[derive(Debug, Default)]
pub struct Osc99DecodedIconCache {
    /// Least recently used at the front.
    entries: VecDeque<(IconKey, Osc99IconImage)>,
}

impl Osc99DecodedIconCache {
    /// [`decode_icon_image`] of `bytes`, from the cache when it holds them.
    pub fn decode(&mut self, bytes: &[u8]) -> Option<Osc99IconImage> {
        let key = IconKey::of(bytes);
        if let Some(position) = self.entries.iter().position(|(cached, _)| *cached == key)
            && let Some(entry) = self.entries.remove(position)
        {
            let image = entry.1.clone();
            self.entries.push_back(entry);
            return Some(image);
        }
        let image = decode_icon_image(bytes)?;
        if self.entries.len() >= DECODED_ICON_CACHE_ENTRIES {
            self.entries.pop_front();
        }
        self.entries.push_back((key, image.clone()));
        Some(image)
    }

    /// How many icons are cached.
    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the icon transmitted as `bytes` is cached.
    #[cfg(test)]
    fn contains(&self, bytes: &[u8]) -> bool {
        let key = IconKey::of(bytes);
        self.entries.iter().any(|(cached, _)| *cached == key)
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

/// The `category` hint: the first non-empty `t=` type.
#[must_use]
pub fn category_hint(types: &[String]) -> Option<&str> {
    types
        .iter()
        .map(String::as_str)
        .find(|category| !category.is_empty())
}

/// Escape `body` for a daemon that interprets markup, as kitty does: a zero
/// width non-joiner after every `<` and `&` keeps them from forming a tag or
/// an entity without visibly changing the text.
#[must_use]
pub fn escape_body_markup(body: &str) -> String {
    let mut escaped = String::with_capacity(body.len());
    for c in body.chars() {
        escaped.push(c);
        if c == '<' || c == '&' {
            escaped.push(ZERO_WIDTH_NON_JOINER);
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    /// A lookup over fixed maps, touching no filesystem. `icons` maps the
    /// name asked for to the on-disk stem that matches it.
    #[derive(Default)]
    struct FakeLookup {
        icons: HashMap<String, String>,
        desktop: HashMap<String, String>,
    }

    impl Osc99IconLookup for FakeLookup {
        fn matched_icon(&self, name: &str) -> Option<String> {
            self.icons.get(name).cloned()
        }

        fn desktop_icon(&self, app_name: &str) -> Option<String> {
            self.desktop.get(app_name).cloned()
        }
    }

    fn names(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_owned()).collect()
    }

    fn lookup_with_icons(icons: &[&str]) -> FakeLookup {
        FakeLookup {
            icons: icons
                .iter()
                .map(|icon| ((*icon).to_owned(), (*icon).to_owned()))
                .collect(),
            ..FakeLookup::default()
        }
    }

    #[test]
    fn each_standard_name_maps_to_its_freedesktop_name() {
        for (name, expected) in [
            ("error", "dialog-error"),
            ("warn", "dialog-warning"),
            ("warning", "dialog-warning"),
            ("info", "dialog-information"),
            ("question", "dialog-question"),
            ("help", "system-help"),
            ("file-manager", "system-file-manager"),
            ("system-monitor", "utilities-system-monitor"),
            ("text-editor", "utilities-text-editor"),
        ] {
            assert_eq!(standard_icon_name(name), Some(expected), "{name}");
        }
        assert_eq!(standard_icon_name("firefox"), None);
        assert_eq!(standard_icon_name("Error"), None);
    }

    #[test]
    fn a_standard_name_is_chosen_without_asking_the_lookup() {
        let choice = choose_icon(&names(&["warn"]), None, None, &FakeLookup::default());

        assert_eq!(
            choice,
            Some(Osc99IconChoice::Named("dialog-warning".to_owned()))
        );
    }

    #[test]
    fn an_existing_name_beats_transmitted_data() {
        let choice = choose_icon(
            &names(&["mail-unread"]),
            Some(&[1, 2, 3]),
            None,
            &lookup_with_icons(&["mail-unread"]),
        );

        assert_eq!(
            choice,
            Some(Osc99IconChoice::Named("mail-unread".to_owned()))
        );
    }

    #[test]
    fn the_matched_on_disk_stem_is_what_is_chosen() {
        let lookup = FakeLookup {
            icons: HashMap::from([("mailicon".to_owned(), "MailIcon".to_owned())]),
            ..FakeLookup::default()
        };

        let choice = choose_icon(&names(&["mailicon"]), None, None, &lookup);

        assert_eq!(choice, Some(Osc99IconChoice::Named("MailIcon".to_owned())));
    }

    #[test]
    fn names_are_tried_in_order() {
        let choice = choose_icon(
            &names(&["nonexistent", "second", "third"]),
            None,
            None,
            &lookup_with_icons(&["third", "second"]),
        );

        assert_eq!(choice, Some(Osc99IconChoice::Named("second".to_owned())));
    }

    #[test]
    fn data_beats_an_unknown_name() {
        let choice = choose_icon(
            &names(&["nonexistent"]),
            Some(&[1, 2, 3]),
            None,
            &FakeLookup::default(),
        );

        assert_eq!(choice, Some(Osc99IconChoice::Data(vec![1, 2, 3])));
    }

    #[test]
    fn an_unknown_name_with_no_data_is_passed_verbatim() {
        let choice = choose_icon(
            &names(&["nonexistent", "other"]),
            None,
            None,
            &FakeLookup::default(),
        );

        assert_eq!(
            choice,
            Some(Osc99IconChoice::Named("nonexistent".to_owned()))
        );
    }

    #[test]
    fn no_names_and_data_chooses_the_data() {
        let choice = choose_icon(&[], Some(&[7]), Some("app"), &FakeLookup::default());

        assert_eq!(choice, Some(Osc99IconChoice::Data(vec![7])));
    }

    #[test]
    fn no_names_and_no_data_uses_the_desktop_file_icon_of_the_app() {
        let lookup = FakeLookup {
            desktop: HashMap::from([("myapp".to_owned(), "myicon".to_owned())]),
            ..FakeLookup::default()
        };

        let choice = choose_icon(&[], None, Some("myapp"), &lookup);

        assert_eq!(choice, Some(Osc99IconChoice::Named("myicon".to_owned())));
    }

    #[test]
    fn nothing_to_go_on_chooses_nothing() {
        let lookup = FakeLookup::default();

        assert_eq!(choose_icon(&[], None, None, &lookup), None);
        assert_eq!(choose_icon(&[], None, Some("unknown"), &lookup), None);
    }

    #[test]
    fn sounds_map_to_hints() {
        assert_eq!(sound_hint(&Osc99Sound::System), Osc99SoundHint::None);
        assert_eq!(sound_hint(&Osc99Sound::Silent), Osc99SoundHint::Suppress);
        for (name, expected) in [
            ("error", "dialog-error"),
            ("warn", "dialog-warning"),
            ("warning", "dialog-warning"),
            ("info", "dialog-information"),
            ("question", "dialog-question"),
            ("bell", "bell"),
        ] {
            assert_eq!(
                sound_hint(&Osc99Sound::Named(name.to_owned())),
                Osc99SoundHint::Named(expected.to_owned()),
                "{name}"
            );
        }
    }

    /// A `width` x `height` PNG of one opaque colour.
    fn png(width: u32, height: u32) -> Result<Vec<u8>, image::ImageError> {
        let pixels = image::RgbaImage::from_pixel(width, height, image::Rgba([10, 20, 30, 255]));
        let mut bytes = Vec::new();
        pixels.write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )?;
        Ok(bytes)
    }

    #[test]
    fn a_small_png_decodes_to_rgba_at_its_own_size() -> Result<(), Box<dyn std::error::Error>> {
        let image = decode_icon_image(&png(3, 2)?).ok_or("did not decode")?;

        assert_eq!((image.width, image.height, image.rowstride), (3, 2, 12));
        assert_eq!(image.rgba.len(), 3 * 2 * 4);
        assert_eq!(image.rgba[..4], [10, 20, 30, 255]);
        Ok(())
    }

    #[test]
    fn an_image_of_exactly_256_is_not_downscaled() -> Result<(), Box<dyn std::error::Error>> {
        let image = decode_icon_image(&png(256, 1)?).ok_or("did not decode")?;

        assert_eq!((image.width, image.height), (256, 1));
        let image = decode_icon_image(&png(256, 256)?).ok_or("did not decode")?;
        assert_eq!((image.width, image.height), (256, 256));
        Ok(())
    }

    #[test]
    fn an_image_of_257_is_downscaled() -> Result<(), Box<dyn std::error::Error>> {
        let image = decode_icon_image(&png(257, 257)?).ok_or("did not decode")?;

        assert_eq!(
            (image.width, image.height, image.rowstride),
            (256, 256, 1024)
        );
        let image = decode_icon_image(&png(1, 257)?).ok_or("did not decode")?;
        assert_eq!((image.width, image.height), (1, 256));
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
        assert!(decode_icon_image(&unchanged).is_some());

        for (width, height) in [(8192, 8192), (30_000, 30_000), (8192, 1), (1, 8192)] {
            let bomb = with_declared_size(tiny.clone(), width, height).ok_or("not a png")?;
            assert!(bomb.len() < 100, "the bomb must stay tiny");
            assert_eq!(decode_icon_image(&bomb), None, "{width}x{height}");
        }
        Ok(())
    }

    #[test]
    fn the_dimension_limit_is_4096_per_side() -> Result<(), Box<dyn std::error::Error>> {
        let at_limit = decode_icon_image(&png(4096, 1)?).ok_or("did not decode")?;
        assert_eq!((at_limit.width, at_limit.height), (256, 1));

        assert_eq!(decode_icon_image(&png(4097, 1)?), None);
        assert_eq!(decode_icon_image(&png(1, 4097)?), None);
        Ok(())
    }

    #[test]
    fn a_large_image_is_downscaled_to_fit_256_keeping_aspect()
    -> Result<(), Box<dyn std::error::Error>> {
        let image = decode_icon_image(&png(2000, 1000)?).ok_or("did not decode")?;

        assert_eq!(
            (image.width, image.height, image.rowstride),
            (256, 128, 1024)
        );
        assert_eq!(image.rgba.len(), 256 * 128 * 4);
        Ok(())
    }

    #[test]
    fn a_tall_large_image_is_downscaled_by_its_height() -> Result<(), Box<dyn std::error::Error>> {
        let image = decode_icon_image(&png(100, 1100)?).ok_or("did not decode")?;

        assert_eq!((image.width, image.height), (23, 256));
        Ok(())
    }

    #[test]
    fn an_extreme_aspect_ratio_keeps_one_pixel() {
        assert_eq!(fit_within_target(5000, 1), (256, 1));
    }

    #[test]
    fn undecodable_bytes_give_nothing() {
        assert_eq!(decode_icon_image(b"not an image"), None);
        assert_eq!(decode_icon_image(&[]), None);
    }

    #[test]
    fn the_category_is_the_first_non_empty_type() {
        assert_eq!(category_hint(&[]), None);
        assert_eq!(category_hint(&names(&["", ""])), None);
        assert_eq!(
            category_hint(&names(&["", "email.arrived", "im"])),
            Some("email.arrived")
        );
    }

    #[test]
    fn markup_escaping_inserts_a_joiner_only_after_angle_and_ampersand() {
        assert_eq!(
            escape_body_markup("a<b>&amp; c"),
            "a<\u{200c}b>&\u{200c}amp; c"
        );
        assert_eq!(escape_body_markup("plain text > 3"), "plain text > 3");
        assert_eq!(escape_body_markup(""), "");
        assert_eq!(escape_body_markup("<<"), "<\u{200c}<\u{200c}");
    }

    // ── Z9: decoding ────────────────────────────────────────────────────

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

    fn encode_png(pixels: &RgbaImage) -> Result<Vec<u8>, image::ImageError> {
        let mut bytes = Vec::new();
        pixels.write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )?;
        Ok(bytes)
    }

    #[test]
    fn downscaling_does_not_darken_the_edge_of_a_transparent_image()
    -> Result<(), Box<dyn std::error::Error>> {
        let bytes = encode_png(&white_square_on_black_transparent())?;

        let image = decode_icon_image(&bytes).ok_or("did not decode")?;

        assert_eq!((image.width, image.height), (256, 256));
        let mut partly_transparent = 0;
        let mut opaque = 0;
        for pixel in image.rgba.as_chunks::<4>().0 {
            if pixel[3] > 0 {
                assert!(
                    pixel[0] >= 250 && pixel[1] >= 250 && pixel[2] >= 250,
                    "a visible pixel {pixel:?} is darker than its source"
                );
                if pixel[3] < 255 {
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
    fn red_pixel(image: &Osc99IconImage) -> Option<(usize, usize)> {
        let width = usize::try_from(image.width).ok()?;
        image
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .position(|pixel| *pixel == [255, 0, 0, 255])
            .map(|index| (index % width, index / width))
    }

    #[test]
    fn an_image_without_an_orientation_tag_is_left_as_it_is()
    -> Result<(), Box<dyn std::error::Error>> {
        let image = decode_icon_image(&png_with_orientation(1)?).ok_or("did not decode")?;

        assert_eq!((image.width, image.height), (4, 2));
        assert_eq!(red_pixel(&image), Some((0, 0)));
        Ok(())
    }

    #[test]
    fn the_exif_orientation_is_applied() -> Result<(), Box<dyn std::error::Error>> {
        // Orientation 6: rotate 90 degrees clockwise. The 4x2 image becomes
        // 2x4 and its top-left pixel moves to the top-right.
        let image = decode_icon_image(&png_with_orientation(6)?).ok_or("did not decode")?;
        assert_eq!((image.width, image.height), (2, 4));
        assert_eq!(red_pixel(&image), Some((1, 0)));

        // Orientation 3: rotate 180. The top-left pixel moves to the
        // bottom-right.
        let image = decode_icon_image(&png_with_orientation(3)?).ok_or("did not decode")?;
        assert_eq!((image.width, image.height), (4, 2));
        assert_eq!(red_pixel(&image), Some((3, 1)));

        // Orientation 2: mirror horizontally.
        let image = decode_icon_image(&png_with_orientation(2)?).ok_or("did not decode")?;
        assert_eq!(red_pixel(&image), Some((3, 0)));
        Ok(())
    }

    #[test]
    fn an_orientation_tag_outside_the_valid_range_is_ignored()
    -> Result<(), Box<dyn std::error::Error>> {
        let image = decode_icon_image(&png_with_orientation(9)?).ok_or("did not decode")?;

        assert_eq!((image.width, image.height), (4, 2));
        assert_eq!(red_pixel(&image), Some((0, 0)));
        Ok(())
    }

    // ── Z9: the decoded-icon cache ──────────────────────────────────────

    /// A 2x2 PNG whose colour is `shade`: a different `shade` is a different
    /// icon.
    fn shaded_png(shade: u8) -> Result<Vec<u8>, image::ImageError> {
        encode_png(&RgbaImage::from_pixel(
            2,
            2,
            image::Rgba([shade, 0, 0, 255]),
        ))
    }

    #[test]
    fn the_cache_decodes_the_same_as_the_function() -> Result<(), Box<dyn std::error::Error>> {
        let bytes = shaded_png(9)?;
        let mut cache = Osc99DecodedIconCache::default();

        let first = cache.decode(&bytes);
        let second = cache.decode(&bytes);

        assert_eq!(first, decode_icon_image(&bytes));
        assert!(first.is_some());
        assert_eq!(first, second);
        assert_eq!(
            cache.len(),
            1,
            "the second decode is a hit, not a new entry"
        );
        Ok(())
    }

    #[test]
    fn the_cache_does_not_remember_a_decode_failure() {
        let mut cache = Osc99DecodedIconCache::default();

        assert_eq!(cache.decode(b"not an image"), None);
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn the_cache_evicts_the_least_recently_used_icon_past_eight()
    -> Result<(), Box<dyn std::error::Error>> {
        let icons = (0..10).map(shaded_png).collect::<Result<Vec<_>, _>>()?;
        let mut cache = Osc99DecodedIconCache::default();
        for icon in &icons[..8] {
            cache.decode(icon).ok_or("did not decode")?;
        }
        assert_eq!(cache.len(), 8);

        // A hit on the oldest makes it the newest, so the next oldest goes.
        cache.decode(&icons[0]).ok_or("did not decode")?;
        cache.decode(&icons[8]).ok_or("did not decode")?;

        assert_eq!(cache.len(), 8);
        assert!(cache.contains(&icons[0]), "a hit refreshes an entry");
        assert!(!cache.contains(&icons[1]), "the least recently used goes");
        assert!(cache.contains(&icons[8]));

        cache.decode(&icons[9]).ok_or("did not decode")?;
        assert!(!cache.contains(&icons[2]));
        assert_eq!(cache.len(), 8);
        Ok(())
    }
}
