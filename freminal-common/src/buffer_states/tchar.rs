// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use core::fmt;

use crate::buffer_states::error::TCharError;
use anyhow::Result;
use unicode_segmentation::UnicodeSegmentation;

/// Maximum byte length of a UTF-8 grapheme that `TChar` can store inline.
///
/// 16 bytes covers all Unicode scalars (max 4 bytes) as well as Kitty
/// placeholder graphemes (U+10EEEE plus up to 3 combining diacritics,
/// each up to 4 bytes = 16 bytes maximum).
pub const TCHAR_MAX_UTF8_LEN: usize = 16;

/// A single terminal character.
///
/// This type is `Copy` — it stores UTF-8 bytes inline (no heap allocation)
/// in a fixed-size `[u8; 16]` buffer with a length byte. The enum can be
/// moved/copied via `memcpy` without any allocator interaction.
#[derive(Debug, Clone, Copy, Eq)]
pub enum TChar {
    Ascii(u8),
    /// Inline UTF-8 bytes: `([u8; 16], len)`. Only the first `len` bytes
    /// are meaningful; the rest are zero-filled padding.
    Utf8([u8; TCHAR_MAX_UTF8_LEN], u8),
    Space,
    NewLine,
}

impl TChar {
    #[must_use]
    pub const fn new_from_single_char(c: u8) -> Self {
        match c {
            32 => Self::Space,
            10 => Self::NewLine,
            _ => Self::Ascii(c),
        }
    }

    /// Return the active UTF-8 byte slice for the `Utf8` variant, or a
    /// single-byte/empty slice for the other variants.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        match self {
            Self::Ascii(c) => core::slice::from_ref(c),
            Self::Utf8(buf, len) => &buf[..*len as usize],
            Self::Space => b" ",
            Self::NewLine => b"\n",
        }
    }

    #[must_use]
    pub fn display_width(&self) -> usize {
        match self {
            Self::Ascii(_) | Self::Space => 1,

            // newline is not a cell; it causes a line break handled by Buffer
            Self::NewLine => 0,

            Self::Utf8(buf, len) => {
                let bytes = &buf[..*len as usize];
                // Try to interpret as UTF-8; fallback to width 1 for invalid sequences
                std::str::from_utf8(bytes).map_or(1, unicode_width::UnicodeWidthStr::width)
            }
        }
    }

    /// Create a new `TChar::Utf8` from a byte slice.
    ///
    /// No UTF-8 validation is performed. That is assumed to have happened
    /// before this function is called.
    ///
    /// # Errors
    /// Returns an error if the slice is empty or longer than
    /// [`TCHAR_MAX_UTF8_LEN`] bytes.
    pub fn new_from_many_chars(v: &[u8]) -> Result<Self> {
        if v.is_empty() {
            return Err(TCharError::EmptyTChar.into());
        }
        if v.len() > TCHAR_MAX_UTF8_LEN {
            return Err(TCharError::TooLong(v.len()).into());
        }
        let mut buf = [0u8; TCHAR_MAX_UTF8_LEN];
        buf[..v.len()].copy_from_slice(v);
        #[allow(clippy::cast_possible_truncation)]
        Ok(Self::Utf8(buf, v.len() as u8))
    }

    #[must_use]
    pub const fn to_u8(&self) -> u8 {
        match self {
            Self::Ascii(c) => *c,
            _ => 0,
        }
    }

    /// Convert a byte slice to a vector of `TChar`s by splitting on
    /// grapheme cluster boundaries.
    ///
    /// # Errors
    /// Returns an error if the slice is not valid UTF-8, or if any grapheme
    /// cluster exceeds [`TCHAR_MAX_UTF8_LEN`] bytes.
    pub fn from_vec(v: &[u8]) -> Result<Vec<Self>> {
        // Fast path: if every byte is ASCII (< 0x80), each byte is its own
        // grapheme cluster and we can skip unicode_segmentation entirely.
        if v.iter().all(|&b| b < 0x80) {
            return Ok(v.iter().map(|&b| Self::new_from_single_char(b)).collect());
        }

        let data_as_string = std::str::from_utf8(v)?;
        let graphemes = data_as_string.graphemes(true).collect::<Vec<&str>>();

        Self::from_vec_of_graphemes(&graphemes)
    }

    /// Convert a slice of grapheme strings to a vector of `TChar`.
    ///
    /// # Errors
    /// Returns an error if any grapheme exceeds [`TCHAR_MAX_UTF8_LEN`] bytes.
    pub fn from_vec_of_graphemes(v: &[&str]) -> Result<Vec<Self>> {
        v.iter()
            .map(|s| {
                Ok(if s.len() == 1 {
                    Self::new_from_single_char(s.as_bytes()[0])
                } else {
                    Self::new_from_many_chars(s.as_bytes())?
                })
            })
            .collect::<Result<Vec<Self>>>()
    }

    /// Convert a string to a vector of `TChar` by splitting on grapheme
    /// cluster boundaries.
    ///
    /// # Errors
    /// Returns an error if any grapheme exceeds [`TCHAR_MAX_UTF8_LEN`] bytes.
    pub fn from_string(s: &str) -> Result<Vec<Self>> {
        // Fast path: if the string is pure ASCII, each byte is one grapheme.
        if s.is_ascii() {
            return Ok(s.bytes().map(Self::new_from_single_char).collect());
        }

        let graphemes = s.graphemes(true).collect::<Vec<&str>>();

        graphemes
            .iter()
            .map(|s| {
                Ok(if s.len() == 1 {
                    Self::new_from_single_char(s.as_bytes()[0])
                } else {
                    Self::new_from_many_chars(s.as_bytes())?
                })
            })
            .collect::<Result<Vec<Self>>>()
    }
}

impl From<u8> for TChar {
    fn from(c: u8) -> Self {
        Self::new_from_single_char(c)
    }
}

impl From<char> for TChar {
    fn from(c: char) -> Self {
        if c.is_ascii() {
            // single-byte fast path
            Self::new_from_single_char(c as u8)
        } else {
            // non-ASCII: encode as UTF-8 scalar into the inline buffer
            let mut buf = [0u8; TCHAR_MAX_UTF8_LEN];
            let s = c.encode_utf8(&mut buf[..4]); // max 4 bytes for a single scalar
            let len = s.len();
            // Zero-fill the rest (already zero from array init)
            #[allow(clippy::cast_possible_truncation)]
            Self::Utf8(buf, len as u8)
        }
    }
}

impl TryFrom<&[u8]> for TChar {
    type Error = anyhow::Error;

    fn try_from(v: &[u8]) -> Result<Self> {
        Self::new_from_many_chars(v)
    }
}

impl PartialEq<u8> for TChar {
    fn eq(&self, other: &u8) -> bool {
        match self {
            Self::Ascii(c) => c == other,
            Self::Space => *other == 32,
            Self::NewLine => *other == 10,
            Self::Utf8(..) => false,
        }
    }
}

impl PartialEq<Vec<u8>> for TChar {
    fn eq(&self, other: &Vec<u8>) -> bool {
        match self {
            Self::Utf8(buf, len) => &buf[..*len as usize] == other.as_slice(),
            _ => false,
        }
    }
}

impl PartialEq<Self> for TChar {
    fn eq(&self, other: &Self) -> bool {
        match self {
            Self::Ascii(c) => match other {
                Self::Ascii(o) => c == o,
                _ => false,
            },
            Self::Utf8(buf, len) => match other {
                Self::Utf8(obuf, olen) => {
                    len == olen && buf[..*len as usize] == obuf[..*len as usize]
                }
                _ => false,
            },
            Self::Space => matches!(other, Self::Space),
            Self::NewLine => matches!(other, Self::NewLine),
        }
    }
}

impl fmt::Display for TChar {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ascii(c) => match c {
                0x00..=0x1F => write!(f, "0x{c:02X}"),
                _ => write!(f, "{}", *c as char),
            },
            Self::Utf8(buf, len) => {
                let bytes = &buf[..*len as usize];
                write!(f, "{}", std::str::from_utf8(bytes).unwrap_or(""))
            }
            Self::Space => write!(f, " "),
            Self::NewLine => writeln!(f),
        }
    }
}

// Compile-time assertion: TChar must remain 18 bytes (16-byte buffer + 1 length
// byte + 1 discriminant, with alignment 1). If a code change causes the layout
// to grow, this assertion will fail at compile time.
const _: () = assert!(
    core::mem::size_of::<TChar>() == 18,
    "TChar size changed — expected 18 bytes"
);

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    // ── From<char> ──────────────────────────────────────────────────

    #[test]
    fn from_non_ascii_char_creates_utf8() {
        let t = TChar::from('é');
        // 'é' is U+00E9, encoded as 0xC3 0xA9 in UTF-8
        assert!(matches!(t, TChar::Utf8(_, _)));
        assert_eq!(t.as_bytes(), "é".as_bytes());
    }

    #[test]
    fn from_ascii_char_creates_ascii_variant() {
        let t = TChar::from('A');
        assert_eq!(t, TChar::Ascii(b'A'));
    }

    #[test]
    fn from_space_char_creates_space() {
        let t = TChar::from(' ');
        assert_eq!(t, TChar::Space);
    }

    #[test]
    fn from_newline_char_creates_newline() {
        let t = TChar::from('\n');
        assert_eq!(t, TChar::NewLine);
    }

    // ── display_width() ─────────────────────────────────────────────

    #[test]
    fn display_width_ascii() {
        assert_eq!(TChar::Ascii(b'A').display_width(), 1);
    }

    #[test]
    fn display_width_space() {
        assert_eq!(TChar::Space.display_width(), 1);
    }

    #[test]
    fn display_width_newline_is_zero() {
        assert_eq!(TChar::NewLine.display_width(), 0);
    }

    #[test]
    fn display_width_utf8_ascii_equivalent() {
        let t = TChar::from('A');
        assert_eq!(t.display_width(), 1);
    }

    #[test]
    fn display_width_star_unicode() {
        // '★' (U+2605 BLACK STAR) is a single-width character
        let t = TChar::from('★');
        assert!(matches!(t, TChar::Utf8(_, _)));
        assert_eq!(t.display_width(), 1);
    }

    #[test]
    fn display_width_wide_cjk_char() {
        // '中' (U+4E2D) is a full-width (2) CJK character
        let t = TChar::from('中');
        assert_eq!(t.display_width(), 2);
    }

    // ── to_u8() ─────────────────────────────────────────────────────

    #[test]
    fn to_u8_ascii_returns_byte() {
        assert_eq!(TChar::Ascii(b'Z').to_u8(), b'Z');
    }

    #[test]
    fn to_u8_space_returns_zero() {
        assert_eq!(TChar::Space.to_u8(), 0);
    }

    #[test]
    fn to_u8_newline_returns_zero() {
        assert_eq!(TChar::NewLine.to_u8(), 0);
    }

    #[test]
    fn to_u8_utf8_returns_zero() {
        let t = TChar::from('é');
        assert_eq!(t.to_u8(), 0);
    }

    // ── Display ─────────────────────────────────────────────────────

    #[test]
    fn display_control_char() {
        // ASCII 0x01 should display as "0x01"
        let t = TChar::Ascii(0x01);
        assert_eq!(format!("{t}"), "0x01");
    }

    #[test]
    fn display_printable_ascii() {
        let t = TChar::Ascii(b'H');
        assert_eq!(format!("{t}"), "H");
    }

    #[test]
    fn display_utf8_char() {
        let t = TChar::from('é');
        assert_eq!(format!("{t}"), "é");
    }

    #[test]
    fn display_space() {
        let t = TChar::Space;
        assert_eq!(format!("{t}"), " ");
    }

    #[test]
    fn display_newline() {
        let t = TChar::NewLine;
        assert_eq!(format!("{t}"), "\n");
    }

    // ── PartialEq<Self> for Utf8 ────────────────────────────────────

    #[test]
    fn partial_eq_same_utf8() {
        let a = TChar::from('é');
        let b = TChar::from('é');
        assert_eq!(a, b);
    }

    #[test]
    fn partial_eq_different_utf8() {
        let a = TChar::from('é');
        let b = TChar::from('ü');
        assert_ne!(a, b);
    }

    #[test]
    fn partial_eq_utf8_vs_ascii_is_false() {
        let a = TChar::from('é');
        let b = TChar::Ascii(b'e');
        assert_ne!(a, b);
    }

    // ── PartialEq<Vec<u8>> ──────────────────────────────────────────

    #[test]
    fn partial_eq_vec_u8_utf8_matching() {
        let t = TChar::from('é');
        let bytes = "é".as_bytes().to_vec();
        assert!(t == bytes);
    }

    #[test]
    fn partial_eq_vec_u8_utf8_non_matching() {
        let t = TChar::from('é');
        let bytes = "ü".as_bytes().to_vec();
        assert!(t != bytes);
    }

    #[test]
    fn partial_eq_vec_u8_ascii_is_false() {
        let t = TChar::Ascii(b'A');
        let bytes = vec![b'A'];
        assert!(t != bytes, "Ascii variant never matches Vec<u8>");
    }

    #[test]
    fn partial_eq_vec_u8_space_is_false() {
        let t = TChar::Space;
        let bytes = vec![b' '];
        assert!(t != bytes, "Space variant never matches Vec<u8>");
    }

    #[test]
    fn partial_eq_vec_u8_newline_is_false() {
        let t = TChar::NewLine;
        let bytes = vec![b'\n'];
        assert!(t != bytes, "NewLine variant never matches Vec<u8>");
    }

    // ── as_bytes() ──────────────────────────────────────────────────

    #[test]
    fn as_bytes_space() {
        assert_eq!(TChar::Space.as_bytes(), b" ");
    }

    #[test]
    fn as_bytes_newline() {
        assert_eq!(TChar::NewLine.as_bytes(), b"\n");
    }

    #[test]
    fn as_bytes_ascii() {
        assert_eq!(TChar::Ascii(b'Z').as_bytes(), b"Z");
    }

    #[test]
    fn as_bytes_utf8() {
        let t = TChar::from('é');
        assert_eq!(t.as_bytes(), "é".as_bytes());
    }

    // ── new_from_many_chars() error paths ───────────────────────────

    #[test]
    fn new_from_many_chars_empty_returns_error() {
        let result = TChar::new_from_many_chars(b"");
        assert!(result.is_err(), "empty slice should return error");
    }

    #[test]
    fn new_from_many_chars_too_long_returns_error() {
        let long = vec![b'a'; TCHAR_MAX_UTF8_LEN + 1];
        let result = TChar::new_from_many_chars(&long);
        assert!(result.is_err(), "oversized slice should return error");
    }

    #[test]
    fn new_from_many_chars_max_len_succeeds() {
        let data = vec![b'a'; TCHAR_MAX_UTF8_LEN];
        let result = TChar::new_from_many_chars(&data);
        assert!(result.is_ok(), "exactly max-length slice should succeed");
    }

    // ── TryFrom<&[u8]> ──────────────────────────────────────────────

    #[test]
    fn try_from_slice_ok() {
        let bytes: &[u8] = "é".as_bytes();
        let t = TChar::try_from(bytes);
        assert!(t.is_ok());
    }

    #[test]
    fn try_from_empty_slice_err() {
        let bytes: &[u8] = b"";
        let t = TChar::try_from(bytes);
        assert!(t.is_err());
    }

    // ── from_vec() ──────────────────────────────────────────────────

    #[test]
    fn from_vec_ascii_fast_path() {
        let bytes = b"Hello";
        let tchars = TChar::from_vec(bytes).unwrap();
        assert_eq!(tchars.len(), 5);
        assert_eq!(tchars[0], TChar::Ascii(b'H'));
        assert_eq!(tchars[4], TChar::Ascii(b'o'));
    }

    #[test]
    fn from_vec_unicode_path() {
        let bytes = "héllo".as_bytes();
        let tchars = TChar::from_vec(bytes).unwrap();
        // 'h', 'é' (2 bytes), 'l', 'l', 'o' → 5 grapheme clusters
        assert_eq!(tchars.len(), 5);
    }

    #[test]
    fn from_vec_space_in_ascii() {
        let bytes = b"a b";
        let tchars = TChar::from_vec(bytes).unwrap();
        assert_eq!(tchars[1], TChar::Space);
    }

    #[test]
    fn from_vec_newline_in_ascii() {
        let bytes = b"a\nb";
        let tchars = TChar::from_vec(bytes).unwrap();
        assert_eq!(tchars[1], TChar::NewLine);
    }

    #[test]
    fn from_vec_invalid_utf8_returns_err() {
        let bytes = &[0x80, 0x81]; // invalid UTF-8
        let result = TChar::from_vec(bytes);
        assert!(result.is_err(), "invalid UTF-8 should return error");
    }

    // ── from_string() ───────────────────────────────────────────────

    #[test]
    fn from_string_ascii_fast_path() {
        let tchars = TChar::from_string("Hi!").unwrap();
        assert_eq!(tchars.len(), 3);
        assert_eq!(tchars[0], TChar::Ascii(b'H'));
    }

    #[test]
    fn from_string_unicode_path() {
        let tchars = TChar::from_string("héllo").unwrap();
        assert_eq!(tchars.len(), 5);
    }

    #[test]
    fn from_string_space() {
        let tchars = TChar::from_string(" ").unwrap();
        assert_eq!(tchars.len(), 1);
        assert_eq!(tchars[0], TChar::Space);
    }

    // ── from_vec_of_graphemes() ──────────────────────────────────────

    #[test]
    fn from_vec_of_graphemes_multi_byte() {
        let graphemes: Vec<&str> = vec!["é", "a"];
        let tchars = TChar::from_vec_of_graphemes(&graphemes).unwrap();
        assert_eq!(tchars.len(), 2);
        assert!(matches!(tchars[0], TChar::Utf8(_, _)));
        assert_eq!(tchars[1], TChar::Ascii(b'a'));
    }

    // ── PartialEq<u8>: Utf8 arm returns false ───────────────────────

    #[test]
    fn partial_eq_u8_utf8_is_false() {
        let t = TChar::from('é');
        assert!(!(t == b'e'), "Utf8 variant should never equal a u8");
    }

    #[test]
    fn partial_eq_u8_space_matches_32() {
        assert!(TChar::Space == 32u8);
    }

    #[test]
    fn partial_eq_u8_newline_matches_10() {
        assert!(TChar::NewLine == 10u8);
    }

    #[test]
    fn partial_eq_u8_space_does_not_match_other() {
        assert!(!(TChar::Space == b'A'));
    }

    #[test]
    fn partial_eq_u8_newline_does_not_match_other() {
        assert!(!(TChar::NewLine == b'A'));
    }

    // ── PartialEq<Self>: cross-variant comparisons ───────────────────

    #[test]
    fn partial_eq_self_ascii_vs_space_is_false() {
        assert_ne!(TChar::Ascii(b' '), TChar::Space);
    }

    #[test]
    fn partial_eq_self_space_vs_newline_is_false() {
        assert_ne!(TChar::Space, TChar::NewLine);
    }

    #[test]
    fn partial_eq_self_utf8_vs_space_is_false() {
        let t = TChar::from('é');
        assert_ne!(t, TChar::Space);
    }

    #[test]
    fn partial_eq_self_utf8_different_lengths() {
        // Two Utf8 variants with different lengths should not be equal.
        let a = TChar::new_from_many_chars("é".as_bytes()).unwrap();
        let b = TChar::new_from_many_chars("中".as_bytes()).unwrap();
        assert_ne!(a, b);
    }
}
