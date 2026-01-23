// Copyright (C) 2024-2025 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use core::fmt;

use crate::buffer_states::error::TCharError;
use anyhow::Result;
use std::fmt::Write;
use unicode_segmentation::UnicodeSegmentation;

#[derive(Debug, Clone, Eq)]
pub enum TChar {
    Ascii(u8),
    Utf8(Vec<u8>),
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

    #[must_use]
    pub fn display_width(&self) -> usize {
        match self {
            Self::Ascii(_) | Self::Space => 1,

            // newline is not a cell; it causes a line break handled by Buffer
            Self::NewLine => 0,

            Self::Utf8(v) => {
                // Try to interpret as UTF-8; fallback to width 1 for invalid sequences
                std::str::from_utf8(v)
                    .map(unicode_width::UnicodeWidthStr::width)
                    .unwrap_or(1)
            }
        }
    }

    /// Create a new `TChar` from a vector of u8
    ///
    /// There is no UTF8 validation. That is assumed to have happened before this function is called.
    ///
    /// # Errors
    /// Will return an error if the vector is empty or is not a valid utf8 string
    pub fn new_from_many_chars(v: Vec<u8>) -> Result<Self> {
        // verify the vector is not empty
        //
        if !v.is_empty() {
            return Ok(Self::Utf8(v));
        }

        Err(TCharError::InvalidTChar(v).into())
    }

    #[must_use]
    pub const fn to_u8(&self) -> u8 {
        match self {
            Self::Ascii(c) => *c,
            _ => 0,
        }
    }

    /// Convert a vector of u8s to a vector of `TChars`
    /// The assumption here is that the vector of u8s will contain one or more `TChars`.
    /// If the byte vector is known to contain a single `TChar`, then use `TChar::from` instead.
    ///
    /// # Errors
    /// Will return an error if the vector is not a valid utf8 string, or if the vector contains characters that are not valid `TChar`
    ///
    pub fn from_vec(v: &[u8]) -> Result<Vec<Self>> {
        let data_as_string = std::str::from_utf8(v)?;
        let graphemes = data_as_string
            .graphemes(true)
            .collect::<Vec<&str>>()
            .clone();

        Self::from_vec_of_graphemes(&graphemes)
    }

    /// Convert a vector of graphemes to a vector of `TChar`
    /// The assumption here is that the vector of graphemes will contain one or more `TChars`.
    ///
    /// # Errors
    /// Will return an error if the vector contains characters that are not valid `TChar`
    ///
    pub fn from_vec_of_graphemes(v: &[&str]) -> Result<Vec<Self>> {
        v.iter()
            .map(|s| {
                Ok(if s.len() == 1 {
                    Self::new_from_single_char(s.as_bytes()[0])
                } else {
                    match Self::new_from_many_chars(s.as_bytes().to_vec()) {
                        Ok(c) => c,
                        Err(e) => {
                            return Err(e);
                        }
                    }
                })
            })
            .collect::<Result<Vec<Self>>>()
    }

    /// Convert a String to a vector of `TChar`
    ///
    /// # Errors
    /// Will return an error if the string is not valid utf8 or the string contains characters that are not valid `TChar`
    pub fn from_string(s: &str) -> Result<Vec<Self>> {
        let graphemes = s.graphemes(true).collect::<Vec<&str>>();

        graphemes
            .iter()
            .map(|s| {
                Ok(if s.len() == 1 {
                    Self::new_from_single_char(s.as_bytes()[0])
                } else {
                    match Self::new_from_many_chars(s.as_bytes().to_vec()) {
                        Ok(c) => c,
                        Err(e) => {
                            return Err(e);
                        }
                    }
                })
            })
            .collect::<Result<Vec<Self>>>()
    }
}

#[must_use]
pub fn display_vec_tchar_as_string(v: &[TChar]) -> String {
    v.iter().fold(String::new(), |mut acc, c| {
        write!(&mut acc, "{c}").unwrap_or_default();
        acc
    })
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
            // non-ASCII: encode as UTF-8 scalar
            let mut buf = [0u8; 4];
            let s = c.encode_utf8(&mut buf); // &str
                                             // we know this is valid UTF-8 by construction, so we can skip Result
            Self::Utf8(s.as_bytes().to_vec())
        }
    }
}

impl From<Vec<u8>> for TChar {
    fn from(v: Vec<u8>) -> Self {
        match Self::new_from_many_chars(v) {
            Ok(c) => c,
            Err(e) => {
                // FIXME: We should probably propagate the error instead of ignoring it
                error!("Error: {}. Will use ascii 0 character", e);
                Self::Ascii(0)
            }
        }
    }
}

// FIXME: Ideally this should be a generic implementation for all types instead of one for each type

impl PartialEq<u8> for TChar {
    fn eq(&self, other: &u8) -> bool {
        match self {
            Self::Ascii(c) => c == other,
            Self::Space => *other == 32,
            Self::NewLine => *other == 10,
            Self::Utf8(_) => false,
        }
    }
}

impl PartialEq<Vec<u8>> for TChar {
    fn eq(&self, other: &Vec<u8>) -> bool {
        match self {
            Self::Utf8(v) => v == other,
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
            Self::Utf8(v) => match other {
                Self::Utf8(o) => v == o,
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
            Self::Utf8(v) => write!(f, "{}", std::str::from_utf8(v).unwrap_or("")),
            Self::Space => write!(f, " "),
            Self::NewLine => writeln!(f),
        }
    }
}
