// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Minimal base64 encoder/decoder (OSC 52, kitty graphics, OSC 99, iTerm2).
//!
//! Uses the standard alphabet (RFC 4648 §4).  [`decode`] is lenient (optional
//! padding), [`decode_strict`] requires canonical padding, and
//! [`StreamDecoder`] decodes input delivered in chunks.

use conv2::ValueFrom;

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

        // Masked 6-bit values are always 0..=63, so `value_from` never fails.
        // `unwrap_or(0)` yields `ENCODE_TABLE[0]` ('A') in the impossible failure case.
        out.push(char::from(
            ENCODE_TABLE[usize::value_from((triple >> 18) & 0x3F).unwrap_or(0)],
        ));
        out.push(char::from(
            ENCODE_TABLE[usize::value_from((triple >> 12) & 0x3F).unwrap_or(0)],
        ));

        if chunk.len() > 1 {
            out.push(char::from(
                ENCODE_TABLE[usize::value_from((triple >> 6) & 0x3F).unwrap_or(0)],
            ));
        } else {
            out.push('=');
        }

        if chunk.len() > 2 {
            out.push(char::from(
                ENCODE_TABLE[usize::value_from(triple & 0x3F).unwrap_or(0)],
            ));
        } else {
            out.push('=');
        }
    }

    out
}

/// Encode arbitrary bytes into an unpadded base64 string (no trailing `=`).
#[must_use]
pub fn encode_unpadded(input: &[u8]) -> String {
    let mut out = encode(input);
    out.truncate(out.trim_end_matches('=').len());
    out
}

/// Errors produced by the base64 decoders in this module.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Base64Error {
    /// A byte outside the standard base64 alphabet (and not `=`).
    #[error("invalid base64 byte 0x{byte:02x} at offset {offset}")]
    InvalidByte {
        /// Offset of the offending byte within the input (or `feed` chunk).
        offset: usize,
        /// The offending byte.
        byte: u8,
    },
    /// A dangling single character (a quantum cannot be one character long),
    /// or, for strict input, a length that is not a multiple of 4.
    #[error("invalid base64 length {len}")]
    InvalidLength {
        /// The offending length (total input length for the one-shot decoders).
        len: usize,
    },
    /// `=` anywhere other than the end of a quantum.
    #[error("misplaced base64 padding at offset {offset}")]
    MisplacedPadding {
        /// Offset of the misplaced `=` within the input (or `feed` chunk).
        offset: usize,
    },
}

/// Classify one input byte: a 6-bit value, or the matching error.
const fn symbol_value(byte: u8, offset: usize) -> Result<u8, Base64Error> {
    match decode_char(byte) {
        Some(v) => Ok(v),
        None if byte == b'=' => Err(Base64Error::MisplacedPadding { offset }),
        None => Err(Base64Error::InvalidByte { offset, byte }),
    }
}

/// Accumulator for one (possibly partial) quantum of up to four characters.
#[derive(Debug, Clone, Copy, Default)]
struct Quantum {
    /// The 6-bit values pushed so far, most significant first.
    acc: u32,
    /// Number of characters pushed so far (always `0..=3` between calls).
    pending: u8,
}

impl Quantum {
    /// Append one 6-bit value; emits three bytes when the quantum completes.
    fn push(&mut self, value: u8, out: &mut Vec<u8>) {
        self.acc = (self.acc << 6) | u32::from(value);
        self.pending += 1;
        if self.pending == 4 {
            self.emit(out);
        }
    }

    /// Emit the bytes represented by the pending characters and reset.
    ///
    /// `n` characters carry `n - 1` whole bytes; the remaining low bits are
    /// discarded without a canonicality check.
    fn emit(&mut self, out: &mut Vec<u8>) {
        if self.pending >= 2 {
            let aligned = self.acc << (6 * u32::from(4 - self.pending));
            let bytes = aligned.to_be_bytes();
            // bytes[0] is always zero (24-bit value); bytes[1..] hold the data.
            out.extend_from_slice(&bytes[1..usize::from(self.pending)]);
        }
        *self = Self::default();
    }
}

/// Decode every byte of `data` (which must not contain padding) into `out`,
/// returning the trailing partial quantum.
fn decode_symbols(data: &[u8], out: &mut Vec<u8>) -> Result<Quantum, Base64Error> {
    let mut quantum = Quantum::default();
    for (offset, &byte) in data.iter().enumerate() {
        quantum.push(symbol_value(byte, offset)?, out);
    }
    Ok(quantum)
}

/// Length of `input` without its trailing run of `=`.
fn unpadded_len(input: &[u8]) -> usize {
    input.iter().rposition(|&b| b != b'=').map_or(0, |p| p + 1)
}

/// Decode a base64 string into bytes (lenient).
///
/// Standard alphabet (RFC 4648 §4).  Whitespace is **not** accepted.
///
/// Rules:
///
/// - Trailing `=` padding is optional.  Padding is accepted when it does not
///   exceed what would complete the final quantum: a final quantum of 2 data
///   characters accepts 0, 1 or 2 `=` (so `"YQ="` is accepted), and a final
///   quantum of 3 data characters accepts 0 or 1.  Any other `=` (interior,
///   after a complete quantum, or in excess) is `MisplacedPadding`.
/// - A final partial quantum of 2 or 3 characters is accepted; a dangling
///   single character is `InvalidLength` (RFC 4648 makes it invalid).
/// - Non-zero trailing bits in the final character are ignored (no
///   canonicality check).
///
/// # Errors
///
/// Returns a [`Base64Error`] describing the first problem found.
pub fn decode(input: &[u8]) -> Result<Vec<u8>, Base64Error> {
    let data_len = unpadded_len(input);
    let (data, padding) = input.split_at(data_len);

    let mut out = Vec::with_capacity(data.len() / 4 * 3 + 2);
    let mut quantum = decode_symbols(data, &mut out)?;

    let max_padding = match quantum.pending {
        0 => 0,
        1 => return Err(Base64Error::InvalidLength { len: input.len() }),
        2 => 2,
        _ => 1,
    };
    if padding.len() > max_padding {
        return Err(Base64Error::MisplacedPadding {
            offset: data_len + max_padding,
        });
    }

    quantum.emit(&mut out);
    Ok(out)
}

/// Decode a base64 string into bytes (strict, RFC 4648 §4).
///
/// The length must be a multiple of 4, and `=` padding (0 to 2 characters) is
/// permitted only at the very end of the final quantum, so `"YQ=="` and
/// `"YWI="` are accepted while `"YQ"`, `"YQ="` and `"YQ=A"` are rejected.
/// Non-zero trailing bits are ignored, as in [`decode`].
///
/// # Errors
///
/// Returns a [`Base64Error`] describing the first problem found.
pub fn decode_strict(input: &[u8]) -> Result<Vec<u8>, Base64Error> {
    if !input.len().is_multiple_of(4) {
        return Err(Base64Error::InvalidLength { len: input.len() });
    }

    let padding = (input.len() - unpadded_len(input)).min(2);
    let data = &input[..input.len() - padding];

    let mut out = Vec::with_capacity(input.len() / 4 * 3);
    let mut quantum = decode_symbols(data, &mut out)?;
    quantum.emit(&mut out);
    Ok(out)
}

/// Where a [`StreamDecoder`] stands relative to base64 quantum boundaries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamPosition {
    /// No characters of an unfinished quantum are pending.
    Aligned,
    /// One to three characters of an unfinished quantum are pending.
    MidQuantum,
}

/// Incremental base64 decoder for input delivered in chunks.
///
/// Any chunk may end mid-quantum, or may end with its own padding (both
/// chunking styles of the kitty notification protocol).  Semantics:
///
/// - a partial quantum carries over between [`feed`](Self::feed) calls;
/// - `=` at quantum position 2 or 3 closes the quantum (its bytes are
///   emitted); further `=` up to position 4 are consumed; a following
///   non-`=` byte starts a new quantum;
/// - `=` at position 0 or 1 is `MisplacedPadding`;
/// - [`finish`](Self::finish) accepts a pending partial quantum of 2 or 3
///   characters and rejects a single dangling character;
/// - error offsets are relative to the chunk passed to `feed`.
///
/// Like [`decode`], this ignores non-zero trailing bits.  After an error the
/// decoder's state is unspecified and it should be discarded.
#[derive(Debug, Clone, Copy, Default)]
pub struct StreamDecoder {
    /// The partial quantum carried over between chunks.
    quantum: Quantum,
    /// Number of further `=` still consumable after a closed quantum.
    padding_slots: u8,
    /// Total bytes fed so far, reported by `InvalidLength` from `finish`.
    consumed: usize,
}

impl StreamDecoder {
    /// Create a decoder with no pending state.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Decode `chunk`, appending the decoded bytes to `out`.
    ///
    /// # Errors
    ///
    /// Returns [`Base64Error::InvalidByte`] for a byte outside the alphabet and
    /// [`Base64Error::MisplacedPadding`] for `=` at quantum position 0 (with no
    /// padding slot available) or 1.
    pub fn feed(&mut self, chunk: &[u8], out: &mut Vec<u8>) -> Result<(), Base64Error> {
        self.consumed = self.consumed.saturating_add(chunk.len());

        for (offset, &byte) in chunk.iter().enumerate() {
            if byte == b'=' {
                self.pad(offset, out)?;
            } else {
                self.padding_slots = 0;
                self.quantum.push(symbol_value(byte, offset)?, out);
            }
        }
        Ok(())
    }

    /// Handle one `=` seen at `offset` within the current chunk.
    fn pad(&mut self, offset: usize, out: &mut Vec<u8>) -> Result<(), Base64Error> {
        match self.quantum.pending {
            2 => {
                self.quantum.emit(out);
                self.padding_slots = 1;
                Ok(())
            }
            3 => {
                self.quantum.emit(out);
                self.padding_slots = 0;
                Ok(())
            }
            0 if self.padding_slots > 0 => {
                self.padding_slots -= 1;
                Ok(())
            }
            _ => Err(Base64Error::MisplacedPadding { offset }),
        }
    }

    /// Whether the decoder is between quanta or holds a partial one.
    ///
    /// Padding still consumable after a closed quantum (the second `=` of
    /// `"YQ=="` may arrive in a later chunk) does not count as partial.
    #[must_use]
    pub const fn position(&self) -> StreamPosition {
        if self.quantum.pending == 0 {
            StreamPosition::Aligned
        } else {
            StreamPosition::MidQuantum
        }
    }

    /// Total number of bytes passed to [`feed`](Self::feed) so far.
    #[must_use]
    pub const fn bytes_fed(&self) -> usize {
        self.consumed
    }

    /// Finish decoding, flushing a pending partial quantum into `out`.
    ///
    /// # Errors
    ///
    /// Returns [`Base64Error::InvalidLength`] (carrying the total number of
    /// bytes fed) if a single dangling character is pending.
    pub fn finish(mut self, out: &mut Vec<u8>) -> Result<(), Base64Error> {
        if self.quantum.pending == 1 {
            return Err(Base64Error::InvalidLength { len: self.consumed });
        }
        self.quantum.emit(out);
        Ok(())
    }
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
        assert_eq!(decode(b"").unwrap(), b"");
    }

    #[test]
    fn decode_single_byte() {
        assert_eq!(decode(b"Zg==").unwrap(), b"f");
    }

    #[test]
    fn decode_without_padding() {
        assert_eq!(decode(b"Zg").unwrap(), b"f");
    }

    #[test]
    fn decode_two_bytes() {
        assert_eq!(decode(b"YWI=").unwrap(), b"ab");
    }

    #[test]
    fn decode_two_bytes_without_padding() {
        assert_eq!(decode(b"YWI").unwrap(), b"ab");
    }

    #[test]
    fn decode_three_bytes() {
        assert_eq!(decode(b"Zm9v").unwrap(), b"foo");
    }

    #[test]
    fn decode_hello_world() {
        assert_eq!(decode(b"SGVsbG8sIFdvcmxkIQ==").unwrap(), b"Hello, World!");
    }

    #[test]
    fn round_trip() {
        let original = b"The quick brown fox jumps over the lazy dog";
        let encoded = encode(original);
        let decoded = decode(encoded.as_bytes()).unwrap();
        assert_eq!(decoded, original);
    }

    #[test]
    fn round_trip_binary() {
        let original: Vec<u8> = (0..=255).collect();
        let encoded = encode(&original);
        let decoded = decode(encoded.as_bytes()).unwrap();
        assert_eq!(decoded, original);
    }

    #[test]
    fn decode_invalid_char() {
        assert_eq!(
            decode(b"abc!def"),
            Err(Base64Error::InvalidByte {
                offset: 3,
                byte: b'!'
            })
        );
    }

    #[test]
    fn decode_unicode_char() {
        assert!(matches!(
            decode("abc\u{00e9}".as_bytes()),
            Err(Base64Error::InvalidByte { offset: 3, .. })
        ));
    }

    #[test]
    fn decode_dangling_character_errors() {
        assert_eq!(decode(b"Z"), Err(Base64Error::InvalidLength { len: 1 }));
        assert_eq!(decode(b"Zm9vY"), Err(Base64Error::InvalidLength { len: 5 }));
        assert_eq!(decode(b"Z="), Err(Base64Error::InvalidLength { len: 2 }));
    }

    #[test]
    fn decode_interior_padding_is_misplaced() {
        assert_eq!(
            decode(b"YQ==YQ=="),
            Err(Base64Error::MisplacedPadding { offset: 2 })
        );
        assert_eq!(
            decode(b"ab=c"),
            Err(Base64Error::MisplacedPadding { offset: 2 })
        );
        assert_eq!(
            decode(b"=abc"),
            Err(Base64Error::MisplacedPadding { offset: 0 })
        );
    }

    #[test]
    fn decode_excess_padding_is_misplaced() {
        // Padding after a complete quantum, or beyond what completes one.
        assert_eq!(
            decode(b"Zm9v="),
            Err(Base64Error::MisplacedPadding { offset: 4 })
        );
        assert_eq!(
            decode(b"YQ==="),
            Err(Base64Error::MisplacedPadding { offset: 4 })
        );
        assert_eq!(
            decode(b"YWI=="),
            Err(Base64Error::MisplacedPadding { offset: 4 })
        );
        assert_eq!(
            decode(b"="),
            Err(Base64Error::MisplacedPadding { offset: 0 })
        );
    }

    #[test]
    fn decode_lenient_accepts_partial_padding() {
        assert_eq!(decode(b"YQ=").unwrap(), b"a");
    }

    #[test]
    fn decode_ignores_nonzero_trailing_bits() {
        // "YR" has non-zero trailing bits; the lenient decoder does not care.
        assert_eq!(decode(b"YR==").unwrap(), b"a");
    }

    #[test]
    fn decode_rejects_whitespace() {
        assert!(matches!(
            decode(b"YQ ="),
            Err(Base64Error::InvalidByte {
                offset: 2,
                byte: b' '
            })
        ));
        assert!(matches!(
            decode(b"YWJj\n"),
            Err(Base64Error::InvalidByte {
                offset: 4,
                byte: b'\n'
            })
        ));
    }

    #[test]
    fn strict_accepts_canonical_input() {
        assert_eq!(decode_strict(b"").unwrap(), b"");
        assert_eq!(decode_strict(b"YQ==").unwrap(), b"a");
        assert_eq!(decode_strict(b"YWI=").unwrap(), b"ab");
        assert_eq!(decode_strict(b"Zm9v").unwrap(), b"foo");
        assert_eq!(
            decode_strict(b"SGVsbG8sIFdvcmxkIQ==").unwrap(),
            b"Hello, World!"
        );
    }

    #[test]
    fn strict_rejects_unpadded_input() {
        assert_eq!(
            decode_strict(b"YQ"),
            Err(Base64Error::InvalidLength { len: 2 })
        );
        assert_eq!(
            decode_strict(b"YWI"),
            Err(Base64Error::InvalidLength { len: 3 })
        );
        assert_eq!(
            decode_strict(b"Z"),
            Err(Base64Error::InvalidLength { len: 1 })
        );
    }

    #[test]
    fn strict_rejects_partial_padding() {
        assert_eq!(
            decode_strict(b"YQ="),
            Err(Base64Error::InvalidLength { len: 3 })
        );
    }

    #[test]
    fn strict_rejects_interior_padding() {
        assert_eq!(
            decode_strict(b"YQ=A"),
            Err(Base64Error::MisplacedPadding { offset: 2 })
        );
        assert_eq!(
            decode_strict(b"YQ==YQ=="),
            Err(Base64Error::MisplacedPadding { offset: 2 })
        );
        assert_eq!(
            decode_strict(b"Y==="),
            Err(Base64Error::MisplacedPadding { offset: 1 })
        );
        assert_eq!(
            decode_strict(b"===="),
            Err(Base64Error::MisplacedPadding { offset: 0 })
        );
    }

    #[test]
    fn strict_rejects_invalid_byte() {
        assert_eq!(
            decode_strict(b"YQ!="),
            Err(Base64Error::InvalidByte {
                offset: 2,
                byte: b'!'
            })
        );
    }

    #[test]
    fn encode_unpadded_lengths_0_to_4() {
        assert_eq!(encode_unpadded(b""), "");
        assert_eq!(encode_unpadded(b"a"), "YQ");
        assert_eq!(encode_unpadded(b"ab"), "YWI");
        assert_eq!(encode_unpadded(b"abc"), "YWJj");
        assert_eq!(encode_unpadded(b"abcd"), "YWJjZA");
    }

    #[test]
    fn error_display_is_descriptive() {
        assert_eq!(
            Base64Error::InvalidByte {
                offset: 3,
                byte: 0x21
            }
            .to_string(),
            "invalid base64 byte 0x21 at offset 3"
        );
        assert_eq!(
            Base64Error::InvalidLength { len: 5 }.to_string(),
            "invalid base64 length 5"
        );
        assert_eq!(
            Base64Error::MisplacedPadding { offset: 2 }.to_string(),
            "misplaced base64 padding at offset 2"
        );
    }

    /// Decode `chunks` through a fresh [`StreamDecoder`].
    fn stream_decode(chunks: &[&[u8]]) -> Result<Vec<u8>, Base64Error> {
        let mut decoder = StreamDecoder::new();
        let mut out = Vec::new();
        for chunk in chunks {
            decoder.feed(chunk, &mut out)?;
        }
        decoder.finish(&mut out)?;
        Ok(out)
    }

    #[test]
    fn stream_matches_one_shot_at_every_split_point() {
        let texts: [&[u8]; 6] = [
            b"",
            b"Zg==",
            b"Zg",
            b"YWI=",
            b"Zm9v",
            b"SGVsbG8sIFdvcmxkIQ==",
        ];
        for text in texts {
            let expected = decode(text).unwrap();
            for split in 0..=text.len() {
                assert_eq!(
                    stream_decode(&[&text[..split], &text[split..]]).unwrap(),
                    expected,
                    "text {:?} split at {split}",
                    String::from_utf8_lossy(text)
                );
            }
        }
    }

    #[test]
    fn stream_byte_at_a_time() {
        let text = b"SGVsbG8sIFdvcmxkIQ==";
        let chunks: Vec<&[u8]> = text.chunks(1).collect();
        assert_eq!(stream_decode(&chunks).unwrap(), b"Hello, World!");
    }

    #[test]
    fn stream_per_chunk_padding() {
        assert_eq!(stream_decode(&[b"YQ==", b"Yg=="]).unwrap(), b"ab");
        assert_eq!(stream_decode(&[b"YWI=", b"Yw=="]).unwrap(), b"abc");
        assert_eq!(stream_decode(&[b"YQ=", b"Yg="]).unwrap(), b"ab");
        assert_eq!(
            stream_decode(&[b"Zm9v", b"YQ==", b"Zm9v"]).unwrap(),
            b"fooafoo"
        );
    }

    #[test]
    fn stream_padding_split_across_chunks() {
        assert_eq!(
            stream_decode(&[b"YQ", b"=", b"=", b"Yg", b"=="]).unwrap(),
            b"ab"
        );
    }

    #[test]
    fn stream_single_character_tail_errors_at_finish() {
        let mut decoder = StreamDecoder::new();
        let mut out = Vec::new();
        decoder.feed(b"Zm9vY", &mut out).unwrap();
        assert_eq!(out, b"foo");
        assert_eq!(
            decoder.finish(&mut out),
            Err(Base64Error::InvalidLength { len: 5 })
        );
    }

    #[test]
    fn stream_partial_tail_accepted_at_finish() {
        assert_eq!(stream_decode(&[b"Zm9vYg"]).unwrap(), b"foob");
        assert_eq!(stream_decode(&[b"Zm9vYmE"]).unwrap(), b"fooba");
    }

    #[test]
    fn stream_padding_at_position_zero_or_one_is_misplaced() {
        assert_eq!(
            stream_decode(&[b"="]),
            Err(Base64Error::MisplacedPadding { offset: 0 })
        );
        assert_eq!(
            stream_decode(&[b"Zm9v="]),
            Err(Base64Error::MisplacedPadding { offset: 4 })
        );
        assert_eq!(
            stream_decode(&[b"Z="]),
            Err(Base64Error::MisplacedPadding { offset: 1 })
        );
        // Too many pads after a closed quantum.
        assert_eq!(
            stream_decode(&[b"YQ===", b"A"]),
            Err(Base64Error::MisplacedPadding { offset: 4 })
        );
    }

    #[test]
    fn stream_offsets_are_chunk_relative() {
        let mut decoder = StreamDecoder::new();
        let mut out = Vec::new();
        decoder.feed(b"Zm9v", &mut out).unwrap();
        assert_eq!(
            decoder.feed(b"YQ!", &mut out),
            Err(Base64Error::InvalidByte {
                offset: 2,
                byte: b'!'
            })
        );
    }

    #[test]
    fn stream_position_and_bytes_fed() {
        let mut decoder = StreamDecoder::new();
        let mut out = Vec::new();
        assert_eq!(decoder.position(), StreamPosition::Aligned);
        assert_eq!(decoder.bytes_fed(), 0);
        decoder.feed(b"SGVsb", &mut out).unwrap();
        assert_eq!(decoder.position(), StreamPosition::MidQuantum);
        assert_eq!(decoder.bytes_fed(), 5);
        decoder.feed(b"G8=", &mut out).unwrap();
        assert_eq!(decoder.position(), StreamPosition::Aligned);
        assert_eq!(decoder.bytes_fed(), 8);
        // A closed quantum with one more `=` still consumable is aligned.
        decoder.feed(b"YQ=", &mut out).unwrap();
        assert_eq!(decoder.position(), StreamPosition::Aligned);
    }

    #[test]
    fn stream_default_matches_new() {
        let mut out = Vec::new();
        let mut decoder = StreamDecoder::default();
        decoder.feed(b"Zg==", &mut out).unwrap();
        decoder.finish(&mut out).unwrap();
        assert_eq!(out, b"f");
    }

    mod props {
        use super::*;
        use proptest::prelude::*;

        proptest! {
            #[test]
            fn round_trip_padded(data in proptest::collection::vec(any::<u8>(), 0..256)) {
                let encoded = encode(&data);
                prop_assert_eq!(&decode(encoded.as_bytes()).unwrap(), &data);
                prop_assert_eq!(&decode_strict(encoded.as_bytes()).unwrap(), &data);
            }

            #[test]
            fn round_trip_unpadded(data in proptest::collection::vec(any::<u8>(), 0..256)) {
                let encoded = encode_unpadded(&data);
                prop_assert!(!encoded.contains('='));
                prop_assert_eq!(decode(encoded.as_bytes()).unwrap(), data);
            }

            #[test]
            fn stream_equals_one_shot(
                data in proptest::collection::vec(any::<u8>(), 0..256),
                unpadded in prop_oneof![Just(false), Just(true)],
                mut splits in proptest::collection::vec(any::<usize>(), 0..6),
            ) {
                let encoded = if unpadded { encode_unpadded(&data) } else { encode(&data) };
                let bytes = encoded.as_bytes();
                for s in &mut splits {
                    *s %= bytes.len() + 1;
                }
                splits.sort_unstable();

                let mut chunks: Vec<&[u8]> = Vec::new();
                let mut prev = 0;
                for s in splits {
                    chunks.push(&bytes[prev..s]);
                    prev = s;
                }
                chunks.push(&bytes[prev..]);

                prop_assert_eq!(stream_decode(&chunks).unwrap(), decode(bytes).unwrap());
            }

            #[test]
            fn stream_per_chunk_padding_concatenates(
                a in proptest::collection::vec(any::<u8>(), 0..64),
                b in proptest::collection::vec(any::<u8>(), 0..64),
            ) {
                let (ea, eb) = (encode(&a), encode(&b));
                let mut expected = a;
                expected.extend_from_slice(&b);
                prop_assert_eq!(
                    stream_decode(&[ea.as_bytes(), eb.as_bytes()]).unwrap(),
                    expected
                );
            }

            #[test]
            fn decoders_never_panic(input in proptest::collection::vec(any::<u8>(), 0..256)) {
                let _ = decode(&input);
                let _ = decode_strict(&input);
                let _ = stream_decode(&[&input]);
            }

            #[test]
            fn strict_success_implies_lenient_agrees(
                input in proptest::collection::vec(
                    prop_oneof![
                        Just(b'A'), Just(b'Q'), Just(b'Z'), Just(b'g'), Just(b'+'),
                        Just(b'/'), Just(b'='),
                    ],
                    0..32,
                ),
            ) {
                if let Ok(strict) = decode_strict(&input) {
                    prop_assert_eq!(decode(&input).unwrap(), strict);
                }
            }
        }
    }
}
