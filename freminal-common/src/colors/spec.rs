// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Parsing of X11 colour specification strings (`rgb:R/G/B`, `#RGB`, `#RRGGBB`)
//! as used by OSC colour-setting sequences.

/// Parse an X11 color spec string to an RGB triple.
///
/// Supported formats:
/// - `rgb:R/G/B` where R, G, B are 1–4 hex digits each (`XParseColor` format)
/// - `#RRGGBB` (6-digit hex)
/// - `#RGB` (3-digit hex, expanded to 6)
///
/// Returns `None` if the string is not a recognised color spec.
#[must_use]
pub fn parse_color_spec(spec: &str) -> Option<(u8, u8, u8)> {
    if let Some(rest) = spec.strip_prefix("rgb:") {
        let parts: Vec<&str> = rest.split('/').collect();
        if parts.len() != 3 {
            return None;
        }
        // Every channel must be pure ASCII hex digits.  This also rejects
        // signs (`+f`) that `from_str_radix` would otherwise accept.
        if !parts
            .iter()
            .all(|p| p.bytes().all(|b| b.is_ascii_hexdigit()))
        {
            return None;
        }
        let r = scale_hex_channel(parts[0])?;
        let g = scale_hex_channel(parts[1])?;
        let b = scale_hex_channel(parts[2])?;
        Some((r, g, b))
    } else if let Some(hex) = spec.strip_prefix('#') {
        // Validate before any byte-offset slicing: the input is
        // attacker-controlled, and slicing inside a multi-byte character
        // would panic.  ASCII-only guarantees every byte is a char boundary.
        if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        match hex.len() {
            6 => {
                let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
                let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
                let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
                Some((r, g, b))
            }
            3 => {
                let r = u8::from_str_radix(&hex[0..1], 16).ok()?;
                let g = u8::from_str_radix(&hex[1..2], 16).ok()?;
                let b = u8::from_str_radix(&hex[2..3], 16).ok()?;
                // Expand: 0xA → 0xAA
                Some((r * 17, g * 17, b * 17))
            }
            _ => None,
        }
    } else {
        None
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
        assert_eq!(parse_color_spec("#ff8000"), Some((0xff, 0x80, 0x00)));
        assert_eq!(parse_color_spec("#abc"), Some((0xaa, 0xbb, 0xcc)));
        assert_eq!(parse_color_spec("rgb:ff/80/00"), Some((0xff, 0x80, 0x00)));
        assert_eq!(parse_color_spec("rgb:f/8/0"), Some((0xff, 0x88, 0x00)));
        assert_eq!(
            parse_color_spec("rgb:ffff/8000/0000"),
            Some((0xff, 0x80, 0x00))
        );
    }

    #[test]
    fn parse_color_spec_non_ascii_hash_three_bytes_returns_none() {
        // "aé" is 3 bytes; slicing [0..1], [1..2] would cut inside 'é'.
        let spec = "#a\u{e9}";
        assert_eq!(spec.len() - 1, 3);
        assert_eq!(parse_color_spec(spec), None);
    }

    #[test]
    fn parse_color_spec_non_ascii_hash_six_bytes_returns_none() {
        // The OSC 11 panic case: "aéaé" is 6 bytes.
        let spec = "#a\u{e9}a\u{e9}";
        assert_eq!(spec.len() - 1, 6);
        assert_eq!(parse_color_spec(spec), None);
    }

    #[test]
    fn parse_color_spec_non_ascii_hash_twelve_bytes_returns_none() {
        let spec = "#a\u{e9}a\u{e9}a\u{e9}a\u{e9}a\u{e9}a\u{e9}";
        assert_eq!(spec.len() - 1, 18);
        assert_eq!(parse_color_spec(spec), None);
        // Exactly 12 bytes: 6 x 2-byte characters.
        let spec = "#\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}";
        assert_eq!(spec.len() - 1, 12);
        assert_eq!(parse_color_spec(spec), None);
    }

    #[test]
    fn parse_color_spec_non_ascii_rgb_channel_returns_none() {
        assert_eq!(parse_color_spec("rgb:\u{e9}/00/00"), None);
        assert_eq!(parse_color_spec("rgb:ff/a\u{e9}/00"), None);
        assert_eq!(parse_color_spec("rgb:ff/00/\u{e9}\u{e9}"), None);
    }

    #[test]
    fn parse_color_spec_rejects_signs_and_non_hex() {
        assert_eq!(parse_color_spec("#+a+a+a"), None);
        assert_eq!(parse_color_spec("#gggggg"), None);
        assert_eq!(parse_color_spec("rgb:+f/00/00"), None);
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
