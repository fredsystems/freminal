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
use std::hash::{DefaultHasher, Hasher};

use freminal_common::buffer_states::osc99_notification::Osc99Sound;

use super::icon_decode::decode_icon_rgba;

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

/// Decode transmitted icon `bytes` (PNG, JPEG, GIF or WebP) to the
/// `image-data` hint's layout.
///
/// The bounded decode, orientation and downscale to 256x256 are
/// [`decode_icon_rgba`]'s (shared with the macOS and Windows leg). `None` when
/// the bytes do not decode (or break a limit) or the dimensions do not fit the
/// hint's `i32` fields; the caller falls back to the next icon source.
#[must_use]
pub fn decode_icon_image(bytes: &[u8]) -> Option<Osc99IconImage> {
    let rgba = decode_icon_rgba(bytes)?;
    let (width, height) = rgba.dimensions();
    let rowstride = width.checked_mul(4)?;
    Some(Osc99IconImage {
        width: i32::try_from(width).ok()?,
        height: i32::try_from(height).ok()?,
        rowstride: i32::try_from(rowstride).ok()?,
        rgba: rgba.into_raw(),
    })
}

/// What identifies a transmitted icon: a hash of its length followed by its
/// bytes, and the length itself.
///
/// The length goes into the hash first, written explicitly rather than left to
/// the slice's `Hash` impl, so the framing is part of this key's definition:
/// two byte strings of different lengths can never feed the hasher the same
/// stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct IconKey {
    hash: u64,
    len: usize,
}

impl IconKey {
    fn of(bytes: &[u8]) -> Self {
        let mut hasher = DefaultHasher::new();
        hasher.write_usize(bytes.len());
        hasher.write(bytes);
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

    fn encode_png(pixels: &image::RgbaImage) -> Result<Vec<u8>, image::ImageError> {
        let mut bytes = Vec::new();
        pixels.write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )?;
        Ok(bytes)
    }

    /// A `width` x `height` PNG of one opaque colour.
    fn png(width: u32, height: u32) -> Result<Vec<u8>, image::ImageError> {
        encode_png(&image::RgbaImage::from_pixel(
            width,
            height,
            image::Rgba([10, 20, 30, 255]),
        ))
    }

    // The decoder's limits, orientation and alpha handling are tested with it
    // in `icon_decode`; these cover the `image-data` layout it is packed into.

    #[test]
    fn a_small_png_decodes_to_the_hint_layout() -> Result<(), Box<dyn std::error::Error>> {
        let image = decode_icon_image(&png(3, 2)?).ok_or("did not decode")?;

        assert_eq!((image.width, image.height, image.rowstride), (3, 2, 12));
        assert_eq!(image.rgba.len(), 3 * 2 * 4);
        assert_eq!(image.rgba[..4], [10, 20, 30, 255]);
        Ok(())
    }

    #[test]
    fn a_downscaled_image_has_the_rowstride_of_its_new_width()
    -> Result<(), Box<dyn std::error::Error>> {
        let image = decode_icon_image(&png(257, 257)?).ok_or("did not decode")?;
        assert_eq!(
            (image.width, image.height, image.rowstride),
            (256, 256, 1024)
        );

        let image = decode_icon_image(&png(2000, 1000)?).ok_or("did not decode")?;
        assert_eq!(
            (image.width, image.height, image.rowstride),
            (256, 128, 1024)
        );
        assert_eq!(image.rgba.len(), 256 * 128 * 4);
        Ok(())
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

    // ── the decoded-icon cache ──────────────────────────────────────────

    /// A 2x2 PNG whose colour is `shade`: a different `shade` is a different
    /// icon.
    fn shaded_png(shade: u8) -> Result<Vec<u8>, image::ImageError> {
        encode_png(&image::RgbaImage::from_pixel(
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
    fn icon_keys_of_different_lengths_differ_even_when_the_bytes_are_all_zero() {
        let keys: Vec<IconKey> = (0..8).map(|len| IconKey::of(&vec![0u8; len])).collect();

        for (index, key) in keys.iter().enumerate() {
            assert_eq!(key.len, index);
            assert!(
                keys[index + 1..].iter().all(|other| other != key),
                "length {index} collides with a longer run of zeros"
            );
        }
        assert_eq!(IconKey::of(b"icon"), IconKey::of(b"icon"));
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
