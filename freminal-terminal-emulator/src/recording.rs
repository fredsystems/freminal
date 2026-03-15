// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Timestamped recording format for terminal session capture and playback.
//!
//! # Format
//!
//! ```text
//! Header:  b"FREC" (4 bytes magic) + 0x01 (1 byte version)
//! Frame:   [u64 LE timestamp_us] [u32 LE data_length] [data bytes]
//! ```
//!
//! `timestamp_us` is elapsed microseconds since the recording started.
//! Frames appear in chronological order.

use std::io::Write;

use thiserror::Error;

/// 4-byte magic identifying a Freminal recording file.
pub const MAGIC: &[u8; 4] = b"FREC";

/// Current format version.
pub const VERSION: u8 = 1;

/// Size of the file header: 4 bytes magic + 1 byte version.
pub const HEADER_SIZE: usize = 5;

/// Size of a frame header: 8 bytes timestamp + 4 bytes length.
pub const FRAME_HEADER_SIZE: usize = 12;

/// A single recorded PTY read with its timestamp.
#[derive(Debug, Clone)]
pub struct PlaybackFrame {
    /// Elapsed microseconds since the recording started.
    pub timestamp_us: u64,
    /// Raw bytes read from the PTY at this point in time.
    pub data: Vec<u8>,
}

/// Errors that can occur when parsing a recording file.
#[derive(Error, Debug)]
pub enum RecordingError {
    /// File is too short to contain the header.
    #[error("file too short for header (need {HEADER_SIZE} bytes, got {0})")]
    HeaderTooShort(usize),

    /// Magic bytes do not match `FREC`.
    #[error("invalid magic bytes (expected FREC, got {0:?})")]
    InvalidMagic([u8; 4]),

    /// Unsupported format version.
    #[error("unsupported version {0} (expected {VERSION})")]
    UnsupportedVersion(u8),

    /// File is truncated mid-frame (header says more data than is available).
    #[error("truncated frame at offset {offset}: need {need} bytes, have {have}")]
    TruncatedFrame {
        offset: usize,
        need: usize,
        have: usize,
    },
}

/// Write the recording file header to `w`.
///
/// # Errors
///
/// Returns an `io::Error` if the write fails.
pub fn write_header(w: &mut impl Write) -> std::io::Result<()> {
    w.write_all(MAGIC)?;
    w.write_all(&[VERSION])?;
    Ok(())
}

/// Write a single frame to `w`.
///
/// # Errors
///
/// Returns an `io::Error` if the write fails.
pub fn write_frame(w: &mut impl Write, timestamp_us: u64, data: &[u8]) -> std::io::Result<()> {
    w.write_all(&timestamp_us.to_le_bytes())?;
    let len = u32::try_from(data.len()).unwrap_or(u32::MAX);
    w.write_all(&len.to_le_bytes())?;
    w.write_all(data)?;
    Ok(())
}

/// Parse a complete recording file into a list of frames.
///
/// # Errors
///
/// Returns a `RecordingError` if the file is malformed or truncated.
pub fn parse_recording(data: &[u8]) -> Result<Vec<PlaybackFrame>, RecordingError> {
    if data.len() < HEADER_SIZE {
        return Err(RecordingError::HeaderTooShort(data.len()));
    }

    let magic: [u8; 4] = [data[0], data[1], data[2], data[3]];
    if &magic != MAGIC {
        return Err(RecordingError::InvalidMagic(magic));
    }

    let version = data[4];
    if version != VERSION {
        return Err(RecordingError::UnsupportedVersion(version));
    }

    let mut frames = Vec::new();
    let mut pos = HEADER_SIZE;

    while pos < data.len() {
        if pos + FRAME_HEADER_SIZE > data.len() {
            return Err(RecordingError::TruncatedFrame {
                offset: pos,
                need: FRAME_HEADER_SIZE,
                have: data.len() - pos,
            });
        }

        let timestamp_us = u64::from_le_bytes([
            data[pos],
            data[pos + 1],
            data[pos + 2],
            data[pos + 3],
            data[pos + 4],
            data[pos + 5],
            data[pos + 6],
            data[pos + 7],
        ]);

        let data_len =
            u32::from_le_bytes([data[pos + 8], data[pos + 9], data[pos + 10], data[pos + 11]])
                as usize;

        pos += FRAME_HEADER_SIZE;

        if pos + data_len > data.len() {
            return Err(RecordingError::TruncatedFrame {
                offset: pos - FRAME_HEADER_SIZE,
                need: FRAME_HEADER_SIZE + data_len,
                have: data.len() - (pos - FRAME_HEADER_SIZE),
            });
        }

        frames.push(PlaybackFrame {
            timestamp_us,
            data: data[pos..pos + data_len].to_vec(),
        });

        pos += data_len;
    }

    Ok(frames)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_empty() {
        let mut buf = Vec::new();
        write_header(&mut buf).unwrap();
        let frames = parse_recording(&buf).unwrap();
        assert!(frames.is_empty());
    }

    #[test]
    fn round_trip_single_frame() {
        let mut buf = Vec::new();
        write_header(&mut buf).unwrap();
        write_frame(&mut buf, 1_000_000, b"hello").unwrap();

        let frames = parse_recording(&buf).unwrap();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].timestamp_us, 1_000_000);
        assert_eq!(frames[0].data, b"hello");
    }

    #[test]
    fn round_trip_multiple_frames() {
        let mut buf = Vec::new();
        write_header(&mut buf).unwrap();
        write_frame(&mut buf, 0, b"first").unwrap();
        write_frame(&mut buf, 500_000, b"second").unwrap();
        write_frame(&mut buf, 1_000_000, b"third").unwrap();

        let frames = parse_recording(&buf).unwrap();
        assert_eq!(frames.len(), 3);
        assert_eq!(frames[0].timestamp_us, 0);
        assert_eq!(frames[0].data, b"first");
        assert_eq!(frames[1].timestamp_us, 500_000);
        assert_eq!(frames[1].data, b"second");
        assert_eq!(frames[2].timestamp_us, 1_000_000);
        assert_eq!(frames[2].data, b"third");
    }

    #[test]
    fn error_header_too_short() {
        let err = parse_recording(b"FR").unwrap_err();
        assert!(matches!(err, RecordingError::HeaderTooShort(2)));
    }

    #[test]
    fn error_invalid_magic() {
        let err = parse_recording(b"XREC\x01").unwrap_err();
        assert!(matches!(err, RecordingError::InvalidMagic(_)));
    }

    #[test]
    fn error_unsupported_version() {
        let err = parse_recording(b"FREC\x02").unwrap_err();
        assert!(matches!(err, RecordingError::UnsupportedVersion(2)));
    }

    #[test]
    fn error_truncated_frame_header() {
        let mut buf = Vec::new();
        write_header(&mut buf).unwrap();
        // Write partial frame header (only 6 bytes of the 12-byte frame header)
        buf.extend_from_slice(&[0u8; 6]);

        let err = parse_recording(&buf).unwrap_err();
        assert!(matches!(err, RecordingError::TruncatedFrame { .. }));
    }

    #[test]
    fn error_truncated_frame_data() {
        let mut buf = Vec::new();
        write_header(&mut buf).unwrap();
        // Write frame header claiming 100 bytes of data, but provide none
        write_frame(&mut Vec::new(), 0, b"").ok(); // just for reference
        buf.extend_from_slice(&0u64.to_le_bytes()); // timestamp
        buf.extend_from_slice(&100u32.to_le_bytes()); // claims 100 bytes
        // but no data follows

        let err = parse_recording(&buf).unwrap_err();
        assert!(matches!(err, RecordingError::TruncatedFrame { .. }));
    }

    #[test]
    fn round_trip_empty_frame() {
        let mut buf = Vec::new();
        write_header(&mut buf).unwrap();
        write_frame(&mut buf, 42, b"").unwrap();

        let frames = parse_recording(&buf).unwrap();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].timestamp_us, 42);
        assert!(frames[0].data.is_empty());
    }
}
