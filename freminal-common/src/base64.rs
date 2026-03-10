// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Minimal base64 encoder/decoder for OSC 52 clipboard support.
//!
//! Uses the standard alphabet (RFC 4648 §4) with optional `=` padding on decode.

const ENCODE_TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Decode a single base64 ASCII character to its 6-bit value.
/// Returns `None` for invalid characters (including `=` padding).
const fn decode_char(c: u8) -> Option<u8> {
    match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

/// Encode arbitrary bytes into a base64 string (with `=` padding).
#[must_use]
pub fn encode(input: &[u8]) -> String {
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);

    for chunk in input.chunks(3) {
        // chunks(3) always yields slices of length 1, 2, or 3 — no other case is possible.
        let (b0, b1, b2) = match *chunk {
            [a] => (a, 0u8, 0u8),
            [a, b] => (a, b, 0u8),
            [a, b, c, ..] => (a, b, c),
            // chunks(3) never yields an empty slice, but the compiler requires exhaustiveness.
            [] => continue,
        };

        let triple = u32::from(b0) << 16 | u32::from(b1) << 8 | u32::from(b2);

        out.push(char::from(ENCODE_TABLE[((triple >> 18) & 0x3F) as usize]));
        out.push(char::from(ENCODE_TABLE[((triple >> 12) & 0x3F) as usize]));

        if chunk.len() > 1 {
            out.push(char::from(ENCODE_TABLE[((triple >> 6) & 0x3F) as usize]));
        } else {
            out.push('=');
        }

        if chunk.len() > 2 {
            out.push(char::from(ENCODE_TABLE[(triple & 0x3F) as usize]));
        } else {
            out.push('=');
        }
    }

    out
}

/// Decode a base64 string into bytes.
///
/// Trailing `=` padding is optional.  Whitespace is **not** stripped — the
/// caller should pre-process if needed.
///
/// # Errors
///
/// Returns `Err` with a human-readable message if the input contains invalid
/// base64 characters (after stripping trailing `=` padding).
pub fn decode(input: &str) -> Result<Vec<u8>, String> {
    // Strip trailing padding.
    let input = input.trim_end_matches('=');
    let bytes = input.as_bytes();

    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    let mut buf: u32 = 0;
    let mut bits: u32 = 0;

    for &b in bytes {
        let val = decode_char(b).ok_or_else(|| format!("invalid base64 character: 0x{b:02x}"))?;
        buf = (buf << 6) | u32::from(val);
        bits += 6;

        if bits >= 8 {
            bits -= 8;
            // buf >> bits is always <= 0xFF because we mask off consumed bits below.
            #[allow(clippy::cast_possible_truncation)]
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }

    Ok(out)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn encode_empty() {
        assert_eq!(encode(b""), "");
    }

    #[test]
    fn encode_single_byte() {
        assert_eq!(encode(b"f"), "Zg==");
    }

    #[test]
    fn encode_two_bytes() {
        assert_eq!(encode(b"ab"), "YWI=");
    }

    #[test]
    fn encode_three_bytes() {
        assert_eq!(encode(b"foo"), "Zm9v");
    }

    #[test]
    fn encode_hello_world() {
        assert_eq!(encode(b"Hello, World!"), "SGVsbG8sIFdvcmxkIQ==");
    }

    #[test]
    fn decode_empty() {
        assert_eq!(decode("").unwrap(), b"");
    }

    #[test]
    fn decode_single_byte() {
        assert_eq!(decode("Zg==").unwrap(), b"f");
    }

    #[test]
    fn decode_without_padding() {
        assert_eq!(decode("Zg").unwrap(), b"f");
    }

    #[test]
    fn decode_two_bytes() {
        assert_eq!(decode("YWI=").unwrap(), b"ab");
    }

    #[test]
    fn decode_three_bytes() {
        assert_eq!(decode("Zm9v").unwrap(), b"foo");
    }

    #[test]
    fn decode_hello_world() {
        assert_eq!(decode("SGVsbG8sIFdvcmxkIQ==").unwrap(), b"Hello, World!");
    }

    #[test]
    fn round_trip() {
        let original = b"The quick brown fox jumps over the lazy dog";
        let encoded = encode(original);
        let decoded = decode(&encoded).unwrap();
        assert_eq!(decoded, original);
    }

    #[test]
    fn round_trip_binary() {
        let original: Vec<u8> = (0..=255).collect();
        let encoded = encode(&original);
        let decoded = decode(&encoded).unwrap();
        assert_eq!(decoded, original);
    }

    #[test]
    fn decode_invalid_char() {
        assert!(decode("abc!def").is_err());
    }

    #[test]
    fn decode_unicode_char() {
        assert!(decode("abc\u{00e9}").is_err());
    }
}
