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
/// in a fixed-size `[u8; 16]` buffer with a length byte. The enum is 18
/// bytes on all platforms and can be moved/copied via `memcpy` without any
/// allocator interaction.
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
                std::str::from_utf8(bytes)
                    .map(unicode_width::UnicodeWidthStr::width)
                    .unwrap_or(1)
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
