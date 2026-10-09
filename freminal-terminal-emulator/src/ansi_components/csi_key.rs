// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! The identity of a CSI sequence.
//!
//! A CSI sequence is identified by three things together: an optional private
//! marker prefix (`?`, `>`, `<`, `=`), an optional intermediate byte
//! (`0x20..=0x2F`), and the final byte. [`CsiKey`] captures exactly that
//! triple so a router can match on the whole identity instead of testing the
//! final byte alone.
//!
//! [`AnsiCsiParser`](super::csi::AnsiCsiParser) stores the private-marker
//! bytes *inside* `params` (parameter bytes span `0x30..=0x3F`) and the
//! intermediates separately in `intermediates`. [`CsiKey::classify`] reads
//! those two buffers plus the final byte and produces the key. It is pure and
//! does not interpret parameter values.

/// A private-marker byte leading the parameter string.
///
/// Named by the byte rather than by meaning, because what a marker means
/// depends on the final byte (`CSI ? ... h` and `CSI ? ... J` are unrelated
/// uses of the same marker).
// TODO(128.3): consumed by the strict router
#[cfg_attr(not(test), expect(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CsiPrefix {
    /// No private marker.
    None,
    /// `?` (0x3F).
    Question,
    /// `>` (0x3E).
    Greater,
    /// `<` (0x3C).
    Less,
    /// `=` (0x3D).
    Equals,
}

/// The intermediate byte (`0x20..=0x2F`) of a CSI sequence.
///
/// One variant per byte, in byte order. [`CsiIntermediate::Multiple`] means two
/// or more intermediates were present; no route accepts it.
// TODO(128.3): consumed by the strict router
#[cfg_attr(not(test), expect(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CsiIntermediate {
    /// No intermediate byte.
    None,
    /// Space (0x20).
    Space,
    /// `!` (0x21).
    Bang,
    /// `"` (0x22).
    DoubleQuote,
    /// `#` (0x23).
    Hash,
    /// `$` (0x24).
    Dollar,
    /// `%` (0x25).
    Percent,
    /// `&` (0x26).
    Ampersand,
    /// `'` (0x27).
    Apostrophe,
    /// `(` (0x28).
    LeftParen,
    /// `)` (0x29).
    RightParen,
    /// `*` (0x2A).
    Star,
    /// `+` (0x2B).
    Plus,
    /// `,` (0x2C).
    Comma,
    /// `-` (0x2D).
    Minus,
    /// `.` (0x2E).
    Dot,
    /// `/` (0x2F).
    Slash,
    /// Two or more intermediate bytes.
    Multiple,
}

/// Why a CSI sequence could not be given an identity.
// TODO(128.3): consumed by the strict router
#[cfg_attr(not(test), expect(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CsiKeyError {
    /// A private-marker byte (`<`, `=`, `>`, `?`) appeared in the parameter
    /// string at a position other than the first.
    MisplacedPrivateMarker {
        /// Index into `params` of the first misplaced marker.
        index: usize,
    },
}

/// The identity of a CSI sequence: prefix, intermediate, and final byte.
// TODO(128.3): consumed by the strict router
#[cfg_attr(not(test), expect(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CsiKey {
    pub prefix: CsiPrefix,
    pub intermediate: CsiIntermediate,
    pub final_byte: u8,
}

/// The first and last bytes of the private-marker range within parameter bytes.
const PRIVATE_MARKER_RANGE: std::ops::RangeInclusive<u8> = 0x3C..=0x3F;

impl CsiPrefix {
    /// Map a leading parameter byte to a prefix, or `None` if it is not a marker.
    const fn from_byte(b: u8) -> Self {
        match b {
            b'?' => Self::Question,
            b'>' => Self::Greater,
            b'<' => Self::Less,
            b'=' => Self::Equals,
            _ => Self::None,
        }
    }
}

impl CsiIntermediate {
    /// Map a single intermediate byte to its variant.
    ///
    /// The parser only stores bytes in `0x20..=0x2F` as intermediates, so the
    /// fallback arm is unreachable in practice. Should it ever be reached, the
    /// byte maps to [`CsiIntermediate::Multiple`], which no route accepts, so
    /// the sequence is rejected rather than misrouted.
    const fn from_byte(b: u8) -> Self {
        match b {
            0x20 => Self::Space,
            0x21 => Self::Bang,
            0x22 => Self::DoubleQuote,
            0x23 => Self::Hash,
            0x24 => Self::Dollar,
            0x25 => Self::Percent,
            0x26 => Self::Ampersand,
            0x27 => Self::Apostrophe,
            0x28 => Self::LeftParen,
            0x29 => Self::RightParen,
            0x2A => Self::Star,
            0x2B => Self::Plus,
            0x2C => Self::Comma,
            0x2D => Self::Minus,
            0x2E => Self::Dot,
            0x2F => Self::Slash,
            _ => Self::Multiple,
        }
    }
}

// TODO(128.3): consumed by the strict router
#[cfg_attr(not(test), expect(dead_code))]
impl CsiKey {
    /// Classify a CSI sequence from its parameter bytes, intermediate bytes, and
    /// final byte.
    ///
    /// `params` is the raw parameter string as stored by the parser, including
    /// any leading private marker. `;` and `:` separators are ordinary
    /// parameter bytes and do not affect classification.
    ///
    /// # Errors
    ///
    /// Returns [`CsiKeyError::MisplacedPrivateMarker`] if a `<`, `=`, `>` or `?`
    /// byte appears in `params` at any index other than 0.
    pub fn classify(
        params: &[u8],
        intermediates: &[u8],
        final_byte: u8,
    ) -> Result<Self, CsiKeyError> {
        let prefix = params
            .first()
            .map_or(CsiPrefix::None, |&b| CsiPrefix::from_byte(b));

        if let Some(index) = params
            .iter()
            .enumerate()
            .skip(1)
            .find_map(|(i, b)| PRIVATE_MARKER_RANGE.contains(b).then_some(i))
        {
            return Err(CsiKeyError::MisplacedPrivateMarker { index });
        }

        let intermediate = match intermediates {
            [] => CsiIntermediate::None,
            [b] => CsiIntermediate::from_byte(*b),
            _ => CsiIntermediate::Multiple,
        };

        Ok(Self {
            prefix,
            intermediate,
            final_byte,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(prefix: CsiPrefix, intermediate: CsiIntermediate, final_byte: u8) -> CsiKey {
        CsiKey {
            prefix,
            intermediate,
            final_byte,
        }
    }

    #[test]
    fn classify_prefixes() {
        let cases: [(&[u8], CsiPrefix); 5] = [
            (b"1;2", CsiPrefix::None),
            (b"?1049", CsiPrefix::Question),
            (b">1", CsiPrefix::Greater),
            (b"<1", CsiPrefix::Less),
            (b"=1", CsiPrefix::Equals),
        ];
        for (params, expected) in cases {
            assert_eq!(
                CsiKey::classify(params, b"", b'h'),
                Ok(key(expected, CsiIntermediate::None, b'h')),
                "params {params:?}"
            );
        }
    }

    #[test]
    fn classify_prefix_without_other_params() {
        assert_eq!(
            CsiKey::classify(b"?", b"", b'h'),
            Ok(key(CsiPrefix::Question, CsiIntermediate::None, b'h'))
        );
    }

    #[test]
    fn classify_empty_params_has_no_prefix() {
        assert_eq!(
            CsiKey::classify(b"", b"", b'm'),
            Ok(key(CsiPrefix::None, CsiIntermediate::None, b'm'))
        );
    }

    #[test]
    fn classify_each_single_intermediate() {
        let expected = [
            CsiIntermediate::Space,
            CsiIntermediate::Bang,
            CsiIntermediate::DoubleQuote,
            CsiIntermediate::Hash,
            CsiIntermediate::Dollar,
            CsiIntermediate::Percent,
            CsiIntermediate::Ampersand,
            CsiIntermediate::Apostrophe,
            CsiIntermediate::LeftParen,
            CsiIntermediate::RightParen,
            CsiIntermediate::Star,
            CsiIntermediate::Plus,
            CsiIntermediate::Comma,
            CsiIntermediate::Minus,
            CsiIntermediate::Dot,
            CsiIntermediate::Slash,
        ];
        for (offset, want) in (0u8..).zip(expected) {
            let byte = 0x20 + offset;
            assert_eq!(
                CsiKey::classify(b"1", &[byte], b'q'),
                Ok(key(CsiPrefix::None, want, b'q')),
                "intermediate byte {byte:#04x}"
            );
        }
    }

    #[test]
    fn classify_no_intermediate() {
        assert_eq!(
            CsiKey::classify(b"1", b"", b'q'),
            Ok(key(CsiPrefix::None, CsiIntermediate::None, b'q'))
        );
    }

    #[test]
    fn classify_two_or_more_intermediates_is_multiple() {
        let cases: [&[u8]; 3] = [b" !", b"$$", b" !\""];
        for intermediates in cases {
            assert_eq!(
                CsiKey::classify(b"1", intermediates, b'q'),
                Ok(key(CsiPrefix::None, CsiIntermediate::Multiple, b'q')),
                "intermediates {intermediates:?}"
            );
        }
    }

    #[test]
    fn classify_out_of_range_single_intermediate_is_rejectable() {
        // The parser never stores such a byte; the defensive mapping must not
        // alias it onto a real intermediate.
        for byte in [0x00u8, 0x1F, 0x30, 0x40, 0x7E, 0xFF] {
            assert_eq!(
                CsiKey::classify(b"", &[byte], b'q'),
                Ok(key(CsiPrefix::None, CsiIntermediate::Multiple, b'q')),
                "byte {byte:#04x}"
            );
        }
    }

    #[test]
    fn classify_prefix_and_intermediate_together() {
        assert_eq!(
            CsiKey::classify(b">1", b" ", b'q'),
            Ok(key(CsiPrefix::Greater, CsiIntermediate::Space, b'q'))
        );
        assert_eq!(
            CsiKey::classify(b"?1;2", b"$", b'p'),
            Ok(key(CsiPrefix::Question, CsiIntermediate::Dollar, b'p'))
        );
    }

    #[test]
    fn classify_misplaced_marker_reports_first_index() {
        let cases: [(&[u8], usize); 8] = [
            (b"1?", 1),
            (b"1;?2", 2),
            (b"12;3>", 4),
            (b"1<2", 1),
            (b"1=", 1),
            (b"?1?", 2),
            (b">1;2<3=4", 4),
            (b"??", 1),
        ];
        for (params, index) in cases {
            assert_eq!(
                CsiKey::classify(params, b"", b'h'),
                Err(CsiKeyError::MisplacedPrivateMarker { index }),
                "params {params:?}"
            );
        }
    }

    #[test]
    fn classify_misplaced_marker_wins_over_other_fields() {
        assert_eq!(
            CsiKey::classify(b"1?", b" ", b'q'),
            Err(CsiKeyError::MisplacedPrivateMarker { index: 1 })
        );
    }

    #[test]
    fn classify_colon_and_semicolon_do_not_affect_classification() {
        let cases: [(&[u8], CsiPrefix); 4] = [
            (b"4:3", CsiPrefix::None),
            (b"38:2::255:0:0;1", CsiPrefix::None),
            (b"?1:2;3", CsiPrefix::Question),
            (b":", CsiPrefix::None),
        ];
        for (params, expected) in cases {
            assert_eq!(
                CsiKey::classify(params, b"", b'm'),
                Ok(key(expected, CsiIntermediate::None, b'm')),
                "params {params:?}"
            );
        }
    }

    #[test]
    fn classify_preserves_final_byte() {
        for final_byte in *b"@Ah~" {
            assert_eq!(
                CsiKey::classify(b"", b"", final_byte).map(|k| k.final_byte),
                Ok(final_byte)
            );
        }
    }
}
