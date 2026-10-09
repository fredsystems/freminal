// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Accumulating a chunked payload under size caps.
//!
//! Several escape-sequence protocols (kitty graphics, OSC 99, iTerm2 multipart
//! files) deliver one logical payload as many chunks.  [`BoundedChunkAssembler`]
//! is the single place that concatenates those chunks, optionally base64
//! decoding them as one continuous stream, while enforcing a per-chunk and a
//! total cap.  It never pre-allocates from a sender-supplied size.

use freminal_common::base64::{Base64Error, StreamDecoder, StreamPosition};

/// Size caps applied by a [`BoundedChunkAssembler`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkLimits {
    /// Largest single chunk accepted, measured as received (encoded length).
    pub max_chunk_bytes: usize,
    /// Largest assembled payload accepted, measured after decoding.
    pub max_total_bytes: usize,
}

/// How a chunk passed to [`BoundedChunkAssembler::push`] is encoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkEncoding {
    /// The chunk's bytes are appended verbatim.
    Raw,
    /// The chunk is part of one continuous base64 stream.
    Base64,
}

/// Why a [`BoundedChunkAssembler`] rejected input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ChunkError {
    /// A single chunk exceeded [`ChunkLimits::max_chunk_bytes`].
    #[error("chunk of {len} bytes exceeds the per-chunk limit of {max} bytes")]
    ChunkTooLarge {
        /// Length of the offending chunk as received.
        len: usize,
        /// The per-chunk limit.
        max: usize,
    },
    /// The assembled payload exceeded [`ChunkLimits::max_total_bytes`].
    #[error("assembled payload exceeds the total limit of {max} bytes")]
    TotalTooLarge {
        /// The total limit.
        max: usize,
    },
    /// The base64 stream was invalid, or a raw chunk interrupted it.
    #[error(transparent)]
    Base64(#[from] Base64Error),
}

/// Concatenates chunks of a payload under [`ChunkLimits`].
///
/// An error poisons the assembler: the buffer is released and every later
/// [`push`](Self::push) and [`finish`](Self::finish) returns the same error.
/// Abandoning a transfer is the owner's job: it drops the assembler.
#[derive(Debug)]
pub struct BoundedChunkAssembler {
    /// The caps this assembler enforces.
    limits: ChunkLimits,
    /// Assembled (decoded) bytes so far.
    data: Vec<u8>,
    /// The one base64 stream shared by every `Base64` chunk.
    decoder: StreamDecoder,
    /// The error that poisoned the assembler, if any.
    failure: Option<ChunkError>,
}

impl BoundedChunkAssembler {
    /// Create an empty assembler.  Allocates nothing.
    #[must_use]
    pub fn new(limits: ChunkLimits) -> Self {
        Self {
            limits,
            data: Vec::new(),
            decoder: StreamDecoder::new(),
            failure: None,
        }
    }

    /// Number of assembled (decoded) bytes so far.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.data.len()
    }

    /// Whether no bytes have been assembled yet.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Allocated capacity of the assembled buffer; lets other modules' tests
    /// prove nothing is pre-allocated from a sender-supplied size.
    #[cfg(test)]
    #[must_use]
    pub const fn capacity(&self) -> usize {
        self.data.capacity()
    }

    /// Append one chunk.
    ///
    /// # Errors
    ///
    /// Returns [`ChunkError::ChunkTooLarge`] if `chunk` is longer than the
    /// per-chunk cap, [`ChunkError::TotalTooLarge`] if the assembled payload
    /// would exceed the total cap, and [`ChunkError::Base64`] for invalid
    /// base64 or for a `Raw` chunk arriving mid-quantum.  Any error poisons the
    /// assembler, after which this returns that same error.
    pub fn push(&mut self, chunk: &[u8], encoding: ChunkEncoding) -> Result<(), ChunkError> {
        if let Some(err) = self.failure {
            return Err(err);
        }
        self.try_push(chunk, encoding)
            .map_err(|err| self.poison(err))
    }

    /// Finish the payload, flushing any pending base64 quantum.
    ///
    /// # Errors
    ///
    /// Returns the poisoning error if one occurred, a [`ChunkError::Base64`]
    /// if a single dangling base64 character is pending, or
    /// [`ChunkError::TotalTooLarge`] if the flushed bytes exceed the total cap.
    pub fn finish(mut self) -> Result<Vec<u8>, ChunkError> {
        if let Some(err) = self.failure {
            return Err(err);
        }
        self.decoder
            .finish(&mut self.data)
            .map_err(ChunkError::from)?;
        if self.data.len() > self.limits.max_total_bytes {
            return Err(ChunkError::TotalTooLarge {
                max: self.limits.max_total_bytes,
            });
        }
        Ok(self.data)
    }

    /// Validate and append `chunk`; the caller poisons on error.
    fn try_push(&mut self, chunk: &[u8], encoding: ChunkEncoding) -> Result<(), ChunkError> {
        if chunk.len() > self.limits.max_chunk_bytes {
            return Err(ChunkError::ChunkTooLarge {
                len: chunk.len(),
                max: self.limits.max_chunk_bytes,
            });
        }

        match encoding {
            ChunkEncoding::Raw => {
                if self.decoder.position() == StreamPosition::MidQuantum {
                    return Err(Base64Error::InvalidLength {
                        len: self.decoder.bytes_fed(),
                    }
                    .into());
                }
                if self.data.len().saturating_add(chunk.len()) > self.limits.max_total_bytes {
                    return Err(self.total_too_large());
                }
                self.data.extend_from_slice(chunk);
            }
            ChunkEncoding::Base64 => {
                // The chunk is at most `max_chunk_bytes` long, so decoding it
                // overshoots the total cap by a bounded amount before the check.
                self.decoder.feed(chunk, &mut self.data)?;
                if self.data.len() > self.limits.max_total_bytes {
                    return Err(self.total_too_large());
                }
            }
        }
        Ok(())
    }

    /// The error for exceeding the total cap.
    const fn total_too_large(&self) -> ChunkError {
        ChunkError::TotalTooLarge {
            max: self.limits.max_total_bytes,
        }
    }

    /// Record `err` as the poisoning error, release the buffer, and return it.
    fn poison(&mut self, err: ChunkError) -> ChunkError {
        self.failure = Some(err);
        self.data = Vec::new();
        err
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    const LIMITS: ChunkLimits = ChunkLimits {
        max_chunk_bytes: 8,
        max_total_bytes: 12,
    };

    fn assembler() -> BoundedChunkAssembler {
        BoundedChunkAssembler::new(LIMITS)
    }

    #[test]
    fn new_allocates_nothing() {
        let a = assembler();
        assert_eq!(a.data.capacity(), 0);
        assert!(a.is_empty());
        assert_eq!(a.len(), 0);
    }

    #[test]
    fn len_and_is_empty_track_decoded_bytes() {
        let mut a = assembler();
        a.push(b"abc", ChunkEncoding::Raw).unwrap();
        assert_eq!(a.len(), 3);
        assert!(!a.is_empty());
        a.push(b"YWJj", ChunkEncoding::Base64).unwrap();
        assert_eq!(a.len(), 6);
    }

    #[test]
    fn raw_chunks_concatenate() {
        let mut a = assembler();
        a.push(b"hello", ChunkEncoding::Raw).unwrap();
        a.push(b" you", ChunkEncoding::Raw).unwrap();
        assert_eq!(a.finish().unwrap(), b"hello you");
    }

    #[test]
    fn empty_assembler_finishes_empty() {
        assert_eq!(assembler().finish().unwrap(), b"");
    }

    #[test]
    fn raw_chunk_at_cap_is_accepted_and_over_by_one_rejected() {
        let mut a = assembler();
        a.push(&[1; 8], ChunkEncoding::Raw).unwrap();

        let mut b = assembler();
        assert_eq!(
            b.push(&[1; 9], ChunkEncoding::Raw),
            Err(ChunkError::ChunkTooLarge { len: 9, max: 8 })
        );
    }

    #[test]
    fn base64_chunk_cap_applies_to_encoded_length() {
        // 8 encoded bytes (6 decoded) is at the cap.
        let mut a = assembler();
        a.push(b"YWJjZGVm", ChunkEncoding::Base64).unwrap();
        assert_eq!(a.len(), 6);

        // 9 encoded bytes is over by one, although it decodes to < 8 bytes.
        let mut b = assembler();
        assert_eq!(
            b.push(b"YWJjZGVmZ", ChunkEncoding::Base64),
            Err(ChunkError::ChunkTooLarge { len: 9, max: 8 })
        );
    }

    #[test]
    fn raw_total_at_cap_is_accepted_and_over_by_one_rejected() {
        let mut a = assembler();
        a.push(&[1; 8], ChunkEncoding::Raw).unwrap();
        a.push(&[1; 4], ChunkEncoding::Raw).unwrap();
        assert_eq!(a.len(), 12);
        assert_eq!(a.finish().unwrap().len(), 12);

        let mut b = assembler();
        b.push(&[1; 8], ChunkEncoding::Raw).unwrap();
        assert_eq!(
            b.push(&[1; 5], ChunkEncoding::Raw),
            Err(ChunkError::TotalTooLarge { max: 12 })
        );
    }

    #[test]
    fn base64_total_at_cap_is_accepted_and_over_by_one_rejected() {
        // 6 + 6 decoded bytes is exactly the cap of 12.
        let mut a = assembler();
        a.push(b"YWJjZGVm", ChunkEncoding::Base64).unwrap();
        a.push(b"YWJjZGVm", ChunkEncoding::Base64).unwrap();
        assert_eq!(a.finish().unwrap(), b"abcdefabcdef");

        // One more decoded byte goes over.
        let mut b = assembler();
        b.push(b"YWJjZGVm", ChunkEncoding::Base64).unwrap();
        b.push(b"YWJjZGVm", ChunkEncoding::Base64).unwrap();
        assert_eq!(
            b.push(b"YQ==", ChunkEncoding::Base64),
            Err(ChunkError::TotalTooLarge { max: 12 })
        );
    }

    #[test]
    fn total_cap_is_enforced_on_the_bytes_flushed_by_finish() {
        let mut a = assembler();
        a.push(b"YWJjZGVm", ChunkEncoding::Base64).unwrap();
        a.push(b"YWJjZGVm", ChunkEncoding::Base64).unwrap();
        // Two unpadded characters stay pending (one more decoded byte on finish).
        a.push(b"YQ", ChunkEncoding::Base64).unwrap();
        assert_eq!(a.len(), 12);
        assert_eq!(a.finish(), Err(ChunkError::TotalTooLarge { max: 12 }));
    }

    #[test]
    fn exceeding_the_total_frees_the_buffer() {
        let mut a = assembler();
        a.push(&[1; 8], ChunkEncoding::Raw).unwrap();
        assert!(a.push(&[1; 5], ChunkEncoding::Raw).is_err());
        assert_eq!(a.data.capacity(), 0);
        assert!(a.is_empty());
    }

    #[test]
    fn base64_chunks_decode() {
        let mut a = assembler();
        a.push(b"SGVs", ChunkEncoding::Base64).unwrap();
        a.push(b"bG8=", ChunkEncoding::Base64).unwrap();
        assert_eq!(a.finish().unwrap(), b"Hello");
    }

    #[test]
    fn base64_mid_quantum_split_decodes() {
        let mut a = assembler();
        a.push(b"SGVsb", ChunkEncoding::Base64).unwrap();
        a.push(b"G8=", ChunkEncoding::Base64).unwrap();
        assert_eq!(a.finish().unwrap(), b"Hello");
    }

    #[test]
    fn base64_per_chunk_padding_concatenates() {
        let mut a = assembler();
        a.push(b"YQ==", ChunkEncoding::Base64).unwrap();
        a.push(b"Yg==", ChunkEncoding::Base64).unwrap();
        assert_eq!(a.finish().unwrap(), b"ab");
    }

    #[test]
    fn base64_partial_tail_is_flushed_by_finish() {
        let mut a = assembler();
        a.push(b"YWI", ChunkEncoding::Base64).unwrap();
        assert_eq!(a.finish().unwrap(), b"ab");
    }

    #[test]
    fn base64_dangling_character_errors_at_finish() {
        let mut a = assembler();
        a.push(b"YWJjY", ChunkEncoding::Base64).unwrap();
        assert_eq!(
            a.finish(),
            Err(ChunkError::Base64(Base64Error::InvalidLength { len: 5 }))
        );
    }

    #[test]
    fn invalid_base64_byte_is_an_error() {
        let mut a = assembler();
        assert_eq!(
            a.push(b"YW!j", ChunkEncoding::Base64),
            Err(ChunkError::Base64(Base64Error::InvalidByte {
                offset: 2,
                byte: b'!'
            }))
        );
    }

    #[test]
    fn raw_then_base64_on_a_quantum_boundary_works() {
        let mut a = assembler();
        a.push(b"raw:", ChunkEncoding::Raw).unwrap();
        a.push(b"YWJj", ChunkEncoding::Base64).unwrap();
        a.push(b"-raw", ChunkEncoding::Raw).unwrap();
        assert_eq!(a.finish().unwrap(), b"raw:abc-raw");
    }

    #[test]
    fn raw_after_pending_padding_is_not_mid_quantum() {
        let mut a = assembler();
        a.push(b"YQ=", ChunkEncoding::Base64).unwrap();
        a.push(b"!", ChunkEncoding::Raw).unwrap();
        assert_eq!(a.finish().unwrap(), b"a!");
    }

    #[test]
    fn raw_after_partial_base64_quantum_errors_with_bytes_fed() {
        let mut a = assembler();
        a.push(b"YWJjYW", ChunkEncoding::Base64).unwrap();
        assert_eq!(
            a.push(b"x", ChunkEncoding::Raw),
            Err(ChunkError::Base64(Base64Error::InvalidLength { len: 6 }))
        );
    }

    #[test]
    fn error_poisons_pushes_and_finish() {
        let mut a = assembler();
        a.push(b"abc", ChunkEncoding::Raw).unwrap();
        let err = a.push(&[1; 9], ChunkEncoding::Raw).unwrap_err();
        assert_eq!(err, ChunkError::ChunkTooLarge { len: 9, max: 8 });

        assert_eq!(a.push(b"ok", ChunkEncoding::Raw), Err(err));
        assert_eq!(a.push(b"YQ==", ChunkEncoding::Base64), Err(err));
        assert_eq!(a.push(b"", ChunkEncoding::Raw), Err(err));
        assert_eq!(a.finish(), Err(err));
    }

    #[test]
    fn poisoning_releases_the_buffer() {
        let mut a = assembler();
        a.push(b"abc", ChunkEncoding::Raw).unwrap();
        assert!(a.push(&[1; 9], ChunkEncoding::Raw).is_err());
        assert!(a.is_empty());
        assert_eq!(a.data.capacity(), 0);
    }

    #[test]
    fn base64_error_poisons_with_the_same_error() {
        let mut a = assembler();
        let err = a.push(b"!!!!", ChunkEncoding::Base64).unwrap_err();
        assert_eq!(a.push(b"YQ==", ChunkEncoding::Base64), Err(err));
        assert_eq!(a.finish(), Err(err));
    }

    #[test]
    fn error_display_is_descriptive() {
        assert_eq!(
            ChunkError::ChunkTooLarge { len: 9, max: 8 }.to_string(),
            "chunk of 9 bytes exceeds the per-chunk limit of 8 bytes"
        );
        assert_eq!(
            ChunkError::TotalTooLarge { max: 12 }.to_string(),
            "assembled payload exceeds the total limit of 12 bytes"
        );
        assert_eq!(
            ChunkError::Base64(Base64Error::InvalidLength { len: 5 }).to_string(),
            "invalid base64 length 5"
        );
    }
}
