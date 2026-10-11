// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Parsing of X11 colour specification strings (`rgb:R/G/B`, `rgbi:R/G/B`,
//! `#RGB`, `#RRGGBB`, `#RRRGGGBBB`, `#RRRRGGGGBBBB`) as used by OSC
//! colour-setting sequences.

use conv2::{ConvUtil, RoundToNearest};
use thiserror::Error;

/// Reasons a colour specification string failed to parse.
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum ColorSpecError {
    /// Nothing was left of the spec after trimming whitespace.
    #[error("empty colour specification")]
    Empty,
    /// The spec did not start with `#`, `rgb:` or `rgbi:`.
    #[error("unrecognised colour specification format")]
    UnknownFormat,
    /// A `#` or `rgb:` channel contained a character that is not an ASCII hex digit.
    #[error("colour specification contains a non-hexadecimal digit")]
    InvalidHexDigit,
    /// A `#` spec had a digit count that is not 3, 6, 9 or 12.
    #[error("'#' colour specification has {len} hex digits; expected 3, 6, 9 or 12")]
    InvalidHexLength {
        /// Number of digits found after the `#`.
        len: usize,
    },
    /// An `rgb:` or `rgbi:` spec did not have exactly three `/`-separated channels.
    #[error("colour specification has {found} channels; expected 3")]
    WrongChannelCount {
        /// Number of `/`-separated channels found.
        found: usize,
    },
    /// An `rgb:` channel had zero or more than four hex digits.
    #[error("'rgb:' channel has {len} hex digits; expected 1 to 4")]
    InvalidChannelLength {
        /// Number of digits found in the offending channel.
        len: usize,
    },
    /// An `rgbi:` component was not a valid floating-point intensity.
    #[error("'rgbi:' component is not a valid intensity")]
    InvalidIntensity,
}

/// Parse an X11 color spec string to an RGB triple.
///
/// Leading and trailing ASCII whitespace is ignored, and the `rgb:` / `rgbi:`
/// prefixes and hex digits are matched case-insensitively.
///
/// Supported formats:
/// - `rgb:R/G/B` where R, G, B are 1–4 hex digits each (`XParseColor` format)
/// - `rgbi:R/G/B` where R, G, B are floating-point intensities, clamped to
///   `0.0..=1.0`
/// - `#RGB` (3 digits, each nibble doubled)
/// - `#RRGGBB`, `#RRRGGGBBB`, `#RRRRGGGGBBBB` (6, 9 or 12 digits; the top
///   byte of each channel is kept)
///
/// # Errors
///
/// Returns a [`ColorSpecError`] describing why the string is not a
/// recognised colour spec.
pub fn parse_color_spec(spec: &str) -> Result<(u8, u8, u8), ColorSpecError> {
    let spec = spec.trim_matches(|c: char| c.is_ascii_whitespace());
    if spec.is_empty() {
        return Err(ColorSpecError::Empty);
    }

    // Prefix matching uses `str::get`, which returns `None` rather than
    // panicking when the range does not fall on a character boundary, so
    // attacker-controlled non-ASCII input is safe here.
    if let Some(rest) = strip_prefix_ignore_ascii_case(spec, "rgbi:") {
        return parse_rgbi(rest);
    }
    if let Some(rest) = strip_prefix_ignore_ascii_case(spec, "rgb:") {
        return parse_rgb(rest);
    }
    if let Some(hex) = spec.strip_prefix('#') {
        return parse_hash(hex);
    }
    Err(ColorSpecError::UnknownFormat)
}

/// Strip `prefix` from the start of `s`, comparing ASCII case-insensitively.
fn strip_prefix_ignore_ascii_case<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let head = s.get(..prefix.len())?;
    if head.eq_ignore_ascii_case(prefix) {
        s.get(prefix.len()..)
    } else {
        None
    }
}

/// Parse the part of an `rgb:` spec after the prefix.
fn parse_rgb(rest: &str) -> Result<(u8, u8, u8), ColorSpecError> {
    let parts: Vec<&str> = rest.split('/').collect();
    let [r, g, b] = parts.as_slice() else {
        return Err(ColorSpecError::WrongChannelCount { found: parts.len() });
    };
    Ok((rgb_channel(r)?, rgb_channel(g)?, rgb_channel(b)?))
}

/// Parse one `rgb:` channel of 1–4 hex digits.
fn rgb_channel(s: &str) -> Result<u8, ColorSpecError> {
    // Every byte must be an ASCII hex digit.  This also rejects signs (`+f`)
    // that `from_str_radix` would otherwise accept.
    if !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(ColorSpecError::InvalidHexDigit);
    }
    if !(1..=4).contains(&s.len()) {
        return Err(ColorSpecError::InvalidChannelLength { len: s.len() });
    }
    scale_hex_channel(s).ok_or(ColorSpecError::InvalidHexDigit)
}

/// Parse the part of an `rgbi:` spec after the prefix.
fn parse_rgbi(rest: &str) -> Result<(u8, u8, u8), ColorSpecError> {
    let parts: Vec<&str> = rest.split('/').collect();
    let [r, g, b] = parts.as_slice() else {
        return Err(ColorSpecError::WrongChannelCount { found: parts.len() });
    };
    Ok((
        intensity_channel(r)?,
        intensity_channel(g)?,
        intensity_channel(b)?,
    ))
}

/// Parse one `rgbi:` intensity, clamp to `0.0..=1.0`, and scale to 8 bits.
fn intensity_channel(s: &str) -> Result<u8, ColorSpecError> {
    let value: f64 = s.parse().map_err(|_| ColorSpecError::InvalidIntensity)?;
    if value.is_nan() {
        return Err(ColorSpecError::InvalidIntensity);
    }
    // `f64::round` rounds half away from zero.
    let scaled = (value.clamp(0.0, 1.0) * 255.0).round();
    scaled
        .approx_as_by::<u8, RoundToNearest>()
        .map_err(|_| ColorSpecError::InvalidIntensity)
}

/// Parse the part of a `#` spec after the `#`.
fn parse_hash(hex: &str) -> Result<(u8, u8, u8), ColorSpecError> {
    // Validate before any byte-offset slicing: the input is
    // attacker-controlled, and slicing inside a multi-byte character
    // would panic.  ASCII-only guarantees every byte is a char boundary.
    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(ColorSpecError::InvalidHexDigit);
    }
    let len = hex.len();
    match len {
        3 => {
            let nibble = |i: usize| -> Result<u8, ColorSpecError> {
                let v = hex
                    .get(i..=i)
                    .and_then(|d| u8::from_str_radix(d, 16).ok())
                    .ok_or(ColorSpecError::InvalidHexDigit)?;
                // Expand: 0xA -> 0xAA
                Ok((v << 4) | v)
            };
            Ok((nibble(0)?, nibble(1)?, nibble(2)?))
        }
        6 | 9 | 12 => {
            let width = len / 3;
            // Keep the top byte (first two digits) of each channel.
            let top_byte = |channel: usize| -> Result<u8, ColorSpecError> {
                let start = channel * width;
                hex.get(start..start + 2)
                    .and_then(|d| u8::from_str_radix(d, 16).ok())
                    .ok_or(ColorSpecError::InvalidHexDigit)
            };
            Ok((top_byte(0)?, top_byte(1)?, top_byte(2)?))
        }
        _ => Err(ColorSpecError::InvalidHexLength { len }),
    }
}

/// Scale a 1–4 hex-digit channel value to 8-bit.
///
/// Follows the `XParseColor` convention:
/// - 1 digit:  0xH   → (H << 4) | H  (e.g. `a` → 0xaa)
/// - 2 digits: 0xHH  → HH as-is
/// - 3 digits: 0xHHH → top 8 bits (shift right 4)
/// - 4 digits: 0xHHHH → top 8 bits (shift right 8)
#[must_use]
pub fn scale_hex_channel(s: &str) -> Option<u8> {
    let v = u16::from_str_radix(s, 16).ok()?;
    let scaled = match s.len() {
        1 => (v << 4) | v,
        2 => v,
        3 => v >> 4,
        4 => v >> 8,
        _ => return None,
    };
    u8::try_from(scaled).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn parse_color_spec_valid_formats_still_parse() {
        assert_eq!(parse_color_spec("#ff8000"), Ok((0xff, 0x80, 0x00)));
        assert_eq!(parse_color_spec("#abc"), Ok((0xaa, 0xbb, 0xcc)));
        assert_eq!(parse_color_spec("rgb:ff/80/00"), Ok((0xff, 0x80, 0x00)));
        assert_eq!(parse_color_spec("rgb:f/8/0"), Ok((0xff, 0x88, 0x00)));
        assert_eq!(
            parse_color_spec("rgb:ffff/8000/0000"),
            Ok((0xff, 0x80, 0x00))
        );
    }

    #[test]
    fn parse_color_spec_rgb_three_digit_channels() {
        // 0xabc >> 4 = 0xab, 0x123 >> 4 = 0x12, 0xfff >> 4 = 0xff
        assert_eq!(parse_color_spec("rgb:abc/123/fff"), Ok((0xab, 0x12, 0xff)));
    }

    #[test]
    fn parse_color_spec_hash_three_digits_nibble_doubled() {
        assert_eq!(parse_color_spec("#3a7"), Ok((0x33, 0xaa, 0x77)));
    }

    #[test]
    fn parse_color_spec_hash_nine_digits_keeps_top_byte() {
        assert_eq!(parse_color_spec("#abcabcdef"), Ok((0xab, 0xab, 0xde)));
    }

    #[test]
    fn parse_color_spec_hash_twelve_digits_keeps_top_byte() {
        assert_eq!(parse_color_spec("#123456789abc"), Ok((0x12, 0x56, 0x9a)));
    }

    #[test]
    fn parse_color_spec_hash_bad_lengths() {
        assert_eq!(
            parse_color_spec("#1234"),
            Err(ColorSpecError::InvalidHexLength { len: 4 })
        );
        assert_eq!(
            parse_color_spec("#"),
            Err(ColorSpecError::InvalidHexLength { len: 0 })
        );
        assert_eq!(
            parse_color_spec("#1234567"),
            Err(ColorSpecError::InvalidHexLength { len: 7 })
        );
        assert_eq!(
            parse_color_spec("#1234567890123"),
            Err(ColorSpecError::InvalidHexLength { len: 13 })
        );
    }

    #[test]
    fn parse_color_spec_prefixes_are_case_insensitive() {
        assert_eq!(parse_color_spec("RGB:ff/00/80"), Ok((0xff, 0x00, 0x80)));
        assert_eq!(parse_color_spec("Rgb:ff/00/80"), Ok((0xff, 0x00, 0x80)));
        assert_eq!(parse_color_spec("Rgbi:1/0/0"), Ok((0xff, 0x00, 0x00)));
        assert_eq!(parse_color_spec("RGBI:0/1/0"), Ok((0x00, 0xff, 0x00)));
    }

    #[test]
    fn parse_color_spec_hex_digits_are_case_insensitive() {
        assert_eq!(parse_color_spec("#FF8000"), Ok((0xff, 0x80, 0x00)));
        assert_eq!(parse_color_spec("#aBc"), Ok((0xaa, 0xbb, 0xcc)));
        assert_eq!(parse_color_spec("rgb:FF/Ab/cD"), Ok((0xff, 0xab, 0xcd)));
    }

    #[test]
    fn parse_color_spec_trims_ascii_whitespace() {
        assert_eq!(parse_color_spec("  #ff8000\t"), Ok((0xff, 0x80, 0x00)));
        assert_eq!(
            parse_color_spec("\n rgb:ff/80/00 \r"),
            Ok((0xff, 0x80, 0x00))
        );
        assert_eq!(parse_color_spec(" rgbi:1/0/0 "), Ok((0xff, 0, 0)));
    }

    #[test]
    fn parse_color_spec_empty_and_blank() {
        assert_eq!(parse_color_spec(""), Err(ColorSpecError::Empty));
        assert_eq!(parse_color_spec("   "), Err(ColorSpecError::Empty));
    }

    #[test]
    fn parse_color_spec_unknown_format() {
        assert_eq!(
            parse_color_spec("notacolor"),
            Err(ColorSpecError::UnknownFormat)
        );
        assert_eq!(
            parse_color_spec("rgbx:1/2/3"),
            Err(ColorSpecError::UnknownFormat)
        );
    }

    #[test]
    fn parse_color_spec_rgbi_values() {
        assert_eq!(parse_color_spec("rgbi:1/0/0"), Ok((0xff, 0, 0)));
        assert_eq!(parse_color_spec("rgbi:0.5/0.5/0.5"), Ok((0x80, 0x80, 0x80)));
        assert_eq!(parse_color_spec("rgbi:0/0/0"), Ok((0, 0, 0)));
        assert_eq!(parse_color_spec("rgbi:+0.5/1e-1/0"), Ok((0x80, 0x1a, 0)));
    }

    #[test]
    fn parse_color_spec_rgbi_clamps() {
        assert_eq!(parse_color_spec("rgbi:2/-1/0"), Ok((0xff, 0, 0)));
        assert_eq!(parse_color_spec("rgbi:inf/0/0"), Ok((0xff, 0, 0)));
        assert_eq!(parse_color_spec("rgbi:-inf/0/0"), Ok((0, 0, 0)));
    }

    #[test]
    fn parse_color_spec_rgbi_errors() {
        assert_eq!(
            parse_color_spec("rgbi:nan/0/0"),
            Err(ColorSpecError::InvalidIntensity)
        );
        assert_eq!(
            parse_color_spec("rgbi:abc/0/0"),
            Err(ColorSpecError::InvalidIntensity)
        );
        assert_eq!(
            parse_color_spec("rgbi://0/0"),
            Err(ColorSpecError::WrongChannelCount { found: 4 })
        );
        assert_eq!(
            parse_color_spec("rgbi:1/2"),
            Err(ColorSpecError::WrongChannelCount { found: 2 })
        );
        assert_eq!(
            parse_color_spec("rgbi:1/2/3/4"),
            Err(ColorSpecError::WrongChannelCount { found: 4 })
        );
        assert_eq!(
            parse_color_spec("rgbi:0/\u{e9}/0"),
            Err(ColorSpecError::InvalidIntensity)
        );
    }

    #[test]
    fn parse_color_spec_rgb_channel_errors() {
        assert_eq!(
            parse_color_spec("rgb:12345/0/0"),
            Err(ColorSpecError::InvalidChannelLength { len: 5 })
        );
        assert_eq!(
            parse_color_spec("rgb:/0/0"),
            Err(ColorSpecError::InvalidChannelLength { len: 0 })
        );
        assert_eq!(
            parse_color_spec("rgb:0//0"),
            Err(ColorSpecError::InvalidChannelLength { len: 0 })
        );
        // The second ':' is part of the first channel, leaving two channels.
        assert_eq!(
            parse_color_spec("rgb::0/0"),
            Err(ColorSpecError::WrongChannelCount { found: 2 })
        );
        assert_eq!(
            parse_color_spec("rgb:ff/00"),
            Err(ColorSpecError::WrongChannelCount { found: 2 })
        );
        assert_eq!(
            parse_color_spec("rgb:ff/00/80/aa"),
            Err(ColorSpecError::WrongChannelCount { found: 4 })
        );
        assert_eq!(
            parse_color_spec("rgb:"),
            Err(ColorSpecError::WrongChannelCount { found: 1 })
        );
        assert_eq!(
            parse_color_spec("rgb:zz/00/00"),
            Err(ColorSpecError::InvalidHexDigit)
        );
    }

    #[test]
    fn parse_color_spec_non_ascii_hash_three_bytes_is_err() {
        // "aé" is 3 bytes; slicing [0..1], [1..2] would cut inside 'é'.
        let spec = "#a\u{e9}";
        assert_eq!(spec.len() - 1, 3);
        assert_eq!(parse_color_spec(spec), Err(ColorSpecError::InvalidHexDigit));
    }

    #[test]
    fn parse_color_spec_non_ascii_hash_six_bytes_is_err() {
        // The OSC 11 panic case: "aéaé" is 6 bytes.
        let spec = "#a\u{e9}a\u{e9}";
        assert_eq!(spec.len() - 1, 6);
        assert_eq!(parse_color_spec(spec), Err(ColorSpecError::InvalidHexDigit));
    }

    #[test]
    fn parse_color_spec_non_ascii_hash_twelve_bytes_is_err() {
        let spec = "#a\u{e9}a\u{e9}a\u{e9}a\u{e9}a\u{e9}a\u{e9}";
        assert_eq!(spec.len() - 1, 18);
        assert!(parse_color_spec(spec).is_err());
        // Exactly 12 bytes: 6 x 2-byte characters.
        let spec = "#\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}";
        assert_eq!(spec.len() - 1, 12);
        assert!(parse_color_spec(spec).is_err());
    }

    #[test]
    fn parse_color_spec_non_ascii_rgb_channel_is_err() {
        assert!(parse_color_spec("rgb:\u{e9}/00/00").is_err());
        assert!(parse_color_spec("rgb:ff/a\u{e9}/00").is_err());
        assert!(parse_color_spec("rgb:ff/00/\u{e9}\u{e9}").is_err());
    }

    #[test]
    fn parse_color_spec_non_ascii_prefix_boundary_is_err() {
        // The prefix comparison slices at byte 4/5; a multi-byte character
        // straddling that offset must not panic.
        assert!(parse_color_spec("rg\u{e9}:00/00/00").is_err());
        assert!(parse_color_spec("rgb\u{e9}00/00/00").is_err());
        assert!(parse_color_spec("\u{e9}\u{e9}\u{e9}").is_err());
    }

    #[test]
    fn parse_color_spec_rejects_signs_and_non_hex() {
        assert_eq!(
            parse_color_spec("#+a+a+a"),
            Err(ColorSpecError::InvalidHexDigit)
        );
        assert_eq!(
            parse_color_spec("#gggggg"),
            Err(ColorSpecError::InvalidHexDigit)
        );
        assert_eq!(
            parse_color_spec("rgb:+f/00/00"),
            Err(ColorSpecError::InvalidHexDigit)
        );
    }

    #[test]
    fn color_spec_error_messages_describe_the_failure() {
        assert_eq!(
            ColorSpecError::InvalidHexLength { len: 4 }.to_string(),
            "'#' colour specification has 4 hex digits; expected 3, 6, 9 or 12"
        );
        assert_eq!(
            ColorSpecError::WrongChannelCount { found: 2 }.to_string(),
            "colour specification has 2 channels; expected 3"
        );
    }

    proptest! {
        #[test]
        fn parse_color_spec_never_panics_on_arbitrary_string(s in any::<String>()) {
            let _ = parse_color_spec(&s);
        }

        #[test]
        fn parse_color_spec_never_panics_on_hash_prefixed(s in "#\\PC{0,12}") {
            let _ = parse_color_spec(&s);
        }

        #[test]
        fn parse_color_spec_never_panics_on_rgb_prefixed(
            s in "rgb:\\PC{0,6}/\\PC{0,6}/\\PC{0,6}"
        ) {
            let _ = parse_color_spec(&s);
        }

        #[test]
        fn parse_color_spec_never_panics_on_rgbi_prefixed(
            s in "rgbi:\\PC{0,8}/\\PC{0,8}/\\PC{0,8}"
        ) {
            let _ = parse_color_spec(&s);
        }

        #[test]
        fn parse_color_spec_never_panics_on_rgbi_prefixed_free_form(s in "rgbi:\\PC{0,24}") {
            let _ = parse_color_spec(&s);
        }

        #[test]
        fn parse_color_spec_rgbi_finite_floats_always_ok(
            r in -10.0f64..10.0, g in -10.0f64..10.0, b in -10.0f64..10.0
        ) {
            let spec = format!("rgbi:{r}/{g}/{b}");
            prop_assert!(parse_color_spec(&spec).is_ok());
        }
    }

    #[test]
    fn scale_hex_channel_five_digit_returns_none() {
        // Length 5 is not in 1–4 → the `_ => return None` arm
        assert_eq!(scale_hex_channel("fffff"), None);
    }

    #[test]
    fn scale_hex_channel_empty_returns_none() {
        // Empty string: from_str_radix will fail → None via `?`
        assert_eq!(scale_hex_channel(""), None);
    }

    #[test]
    fn scale_hex_channel_one_digit() {
        // 'a' → 0xaa = 170
        assert_eq!(scale_hex_channel("a"), Some(0xaa));
    }

    #[test]
    fn scale_hex_channel_two_digits() {
        assert_eq!(scale_hex_channel("ff"), Some(0xff));
        assert_eq!(scale_hex_channel("80"), Some(0x80));
    }

    #[test]
    fn scale_hex_channel_three_digits() {
        // 0xABC >> 4 = 0xAB = 171
        assert_eq!(scale_hex_channel("ABC"), Some(0xAB));
    }

    #[test]
    fn scale_hex_channel_four_digits() {
        // 0xABCD >> 8 = 0xAB = 171
        assert_eq!(scale_hex_channel("ABCD"), Some(0xAB));
    }
}
