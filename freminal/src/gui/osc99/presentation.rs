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

use freminal_common::buffer_states::osc99_notification::Osc99Sound;
use image::imageops::FilterType;

/// An icon wider or taller than this is downscaled before it is sent.
///
/// The `image-data` hint reaches zbus as one `Value` per byte, so its transient
/// cost is a large multiple of the pixel data. Capping the icon at 256x256
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

/// Zero width non-joiner, inserted after the characters kitty escapes in a
/// body for daemons that interpret markup.
const ZERO_WIDTH_NON_JOINER: char = '\u{200c}';

/// What the icon scan and the `.desktop` scan know about the system.
pub trait Osc99IconLookup {
    /// Whether the icon scan found an icon called `name` (case-insensitive).
    fn icon_exists(&self, name: &str) -> bool;

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
///   name; any other name is used when `lookup` finds it.
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
        if lookup.icon_exists(name) {
            return Some(Osc99IconChoice::Named(name.clone()));
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
/// Decoding is bounded by [`MAX_DECODE_DIMENSION`] and
/// [`MAX_DECODE_ALLOC_BYTES`]: an image over a limit is a decode failure. An
/// image wider or taller than 256 pixels is downscaled to fit within 256x256,
/// keeping its aspect ratio. `None` when the bytes do not decode (or break a
/// limit) or the dimensions do not fit the hint's `i32` fields; the caller
/// falls back to the next icon source.
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
    let decoded = reader.decode().ok()?.to_rgba8();
    let (width, height) = decoded.dimensions();
    let rgba = if width > DOWNSCALE_THRESHOLD || height > DOWNSCALE_THRESHOLD {
        let (target_width, target_height) = fit_within_target(width, height);
        image::imageops::resize(&decoded, target_width, target_height, FilterType::Triangle)
    } else {
        decoded
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
    use std::collections::{HashMap, HashSet};

    use super::*;

    /// A lookup over fixed sets, touching no filesystem.
    #[derive(Default)]
    struct FakeLookup {
        icons: HashSet<String>,
        desktop: HashMap<String, String>,
    }

    impl Osc99IconLookup for FakeLookup {
        fn icon_exists(&self, name: &str) -> bool {
            self.icons.contains(name)
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
            icons: icons.iter().map(|icon| (*icon).to_owned()).collect(),
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
}
