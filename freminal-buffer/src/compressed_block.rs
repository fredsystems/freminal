// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! LZ4-compressed blocks of [`CompactRow`]s (Task 119 — Scrollback
//! Compression).
//!
//! [`CompactRow`] (Task 118) is the only thing safe to byte-compress —
//! a raw [`crate::cell::Cell`] holds `Arc`/`Box` pointers that cannot
//! survive a byte-for-byte round trip. [`CompressedBlock`] serializes a run
//! of `CompactRow`s (via [`CompactRow::to_bytes`]/[`CompactRow::from_bytes`])
//! into one contiguous buffer and LZ4-compresses it using `lz4_flex`'s
//! **block format** (not the frame format — the block API is used because
//! it needs no extra frame header/checksum bookkeeping and the exact
//! decompressed length is already known and cheap to store alongside the
//! block).
//!
//! This module is a pure data transform with no knowledge of `Buffer`, the
//! LRU cache, or the idle-driven compression tick — those are wired in by
//! later Task 119 subtasks (119.4–119.6). A `CompressedBlock` round-trips
//! losslessly to the exact `CompactRow`s it was built from (see the
//! module's test suite), and thence to `Row`/`Cell` via
//! [`CompactRow::to_row`].

use conv2::ValueFrom;

use crate::compact_row::CompactRow;

/// A single LZ4-compressed block of serialized [`CompactRow`]s.
///
/// Construct via [`CompressedBlock::from_rows`] and recover the original
/// rows via [`CompressedBlock::decompress`] (allocates a fresh scratch
/// buffer) or [`CompressedBlock::decompress_into`] (reuses a
/// caller-provided scratch buffer — the allocation-churn-avoiding path a
/// later subtask wires into the read-on-scroll-into-view path).
#[derive(Debug, Clone)]
pub struct CompressedBlock {
    /// The LZ4 block-format compressed payload.
    compressed: Vec<u8>,
    /// Byte length of the pre-compression buffer (the `u32` row-count
    /// prefix plus every row's [`CompactRow::to_bytes`] output,
    /// concatenated). `lz4_flex`'s block-format API needs the exact
    /// decompressed length up front to size its output buffer — unlike
    /// the frame format, the block format does not self-describe its own
    /// uncompressed size.
    decompressed_len: u32,
    /// Number of rows contained in this block. Stored redundantly
    /// alongside the row-count prefix baked into the compressed payload
    /// itself, so callers can read it via [`CompressedBlock::row_count`]
    /// without paying for a decompress.
    row_count: u32,
}

impl CompressedBlock {
    /// Build a compressed block from a run of `CompactRow`s.
    ///
    /// Serializes `rows` (via [`CompactRow::to_bytes`]) into one
    /// contiguous buffer prefixed with a little-endian `u32` row count,
    /// then LZ4-compresses that buffer (`lz4_flex::block::compress`).
    #[must_use]
    pub fn from_rows(rows: &[CompactRow]) -> Self {
        // A block spans a bounded number of logical scrollback rows (Task
        // 119 design: ~128–256, tuned in 119.5) — nowhere near `u32::MAX`.
        // Degrade to `u32::MAX` rather than panicking if it somehow were
        // not, mirroring the `unwrap_or` convention already established in
        // `compact_row.rs`.
        let row_count = u32::value_from(rows.len()).unwrap_or(u32::MAX);

        let mut plain = Vec::new();
        plain.extend_from_slice(&row_count.to_le_bytes());
        for row in rows {
            plain.extend_from_slice(&row.to_bytes());
        }

        let decompressed_len = u32::value_from(plain.len()).unwrap_or(u32::MAX);
        let compressed = lz4_flex::block::compress(&plain);

        Self {
            compressed,
            decompressed_len,
            row_count,
        }
    }

    /// Decompress this block back into its original `CompactRow`s,
    /// allocating a fresh scratch buffer for the decompressed bytes.
    ///
    /// Prefer [`CompressedBlock::decompress_into`] when decompressing
    /// repeatedly (e.g. on every scroll-into-view) to reuse one scratch
    /// buffer instead of allocating on every call.
    ///
    /// Returns `None` if the compressed payload is malformed/corrupt —
    /// never panics on bad input.
    #[must_use]
    pub fn decompress(&self) -> Option<Vec<CompactRow>> {
        let mut scratch = Vec::new();
        self.decompress_into(&mut scratch)
    }

    /// Like [`CompressedBlock::decompress`], but decompresses into
    /// `scratch` (cleared and resized as needed) instead of allocating a
    /// fresh buffer on every call. This is the allocation-churn-avoiding
    /// path a later subtask wires into `Buffer`'s decompress-on-scroll
    /// path.
    ///
    /// Returns `None` if the compressed payload is malformed/corrupt —
    /// never panics on bad input.
    #[must_use]
    pub fn decompress_into(&self, scratch: &mut Vec<u8>) -> Option<Vec<CompactRow>> {
        // `decompressed_len` was itself derived from a real `Vec<u8>`'s
        // `len()` in `from_rows`, so this conversion cannot fail in
        // practice; degrade to `usize::MAX` (which will simply fail the
        // subsequent `decompress_into` call against a too-small/garbage
        // buffer) rather than panicking.
        let len = usize::value_from(self.decompressed_len).unwrap_or(usize::MAX);
        scratch.clear();
        scratch.resize(len, 0);

        let written = lz4_flex::block::decompress_into(&self.compressed, scratch).ok()?;
        if written != len {
            // A truncated/corrupt block decompressed to fewer bytes than
            // recorded; the payload cannot be trusted.
            return None;
        }

        let count_bytes = scratch.get(..4)?;
        let count_arr: [u8; 4] = count_bytes.try_into().ok()?;
        let stored_row_count = u32::from_le_bytes(count_arr);
        if stored_row_count != self.row_count {
            // The self-described row count inside the decompressed
            // payload disagrees with the count recorded alongside the
            // compressed bytes — the payload has been corrupted.
            return None;
        }

        let row_count = usize::value_from(self.row_count).unwrap_or(0);
        let mut rows = Vec::with_capacity(row_count.min(scratch.len()));
        let mut offset = 4usize;
        for _ in 0..row_count {
            let remaining = scratch.get(offset..)?;
            let (row, consumed) = CompactRow::from_bytes(remaining)?;
            rows.push(row);
            offset = offset.checked_add(consumed)?;
        }

        Some(rows)
    }

    /// Number of rows contained in this block.
    #[must_use]
    pub fn row_count(&self) -> usize {
        // `u32 -> usize` is lossless on every platform this crate targets
        // (desktop, `usize` is at least 32 bits there); see `expand_runs`
        // in `compact_row.rs` for the same reasoning. Uses the checked
        // `conv2` conversion (rather than `as`) per the workspace's
        // numeric-conversion convention, degrading to `usize::MAX` in the
        // unreachable failure case.
        usize::value_from(self.row_count).unwrap_or(usize::MAX)
    }

    /// Compressed size in bytes — the actual heap allocation size
    /// (`len()`, not `capacity()`) of the LZ4-compressed payload. Useful
    /// for reporting the ratio achieved over the pre-compression
    /// (Task-118 flat compact) size.
    #[must_use]
    pub const fn compressed_bytes(&self) -> usize {
        self.compressed.len()
    }

    /// Heap bytes retained by this block's backing allocation, computed
    /// from allocation *capacity* (the real resident cost), not `len()`.
    /// Mirrors the accounting style of [`CompactRow::heap_bytes`] and
    /// `Buffer::heap_bytes`.
    #[must_use]
    pub const fn heap_bytes(&self) -> usize {
        self.compressed.capacity()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use freminal_common::{
        buffer_states::{fonts::FontWeight, tchar::TChar},
        colors::TerminalColor,
    };

    use crate::row::{LineWidth, Row, RowJoin, RowOrigin};

    use super::*;

    fn ascii_row(width: usize, text: &str) -> Row {
        let mut row = Row::new(width);
        let chars: Vec<TChar> = text.bytes().map(TChar::Ascii).collect();
        row.insert_text(
            0,
            &chars,
            &freminal_common::buffer_states::format_tag::FormatTag::default(),
        );
        row
    }

    /// Round-trip `rows` through a `CompressedBlock` and assert every row
    /// comes back byte-for-byte identical (compared via the rebuilt `Row`,
    /// mirroring `compact_row.rs`'s `assert_round_trip_exact`).
    fn assert_block_round_trip_exact(rows: &[Row]) {
        let compact_rows: Vec<CompactRow> = rows
            .iter()
            .map(|r| CompactRow::from_row(r).expect("row should be compactable"))
            .collect();

        let block = CompressedBlock::from_rows(&compact_rows);
        assert_eq!(block.row_count(), rows.len());

        let decompressed = block.decompress().expect("block should decompress");
        assert_eq!(decompressed.len(), rows.len());

        for (original, decoded) in rows.iter().zip(decompressed.iter()) {
            let rebuilt = decoded.to_row();
            assert_eq!(rebuilt.cells(), original.cells());
            assert_eq!(rebuilt.max_width(), original.max_width());
            assert_eq!(rebuilt.origin, original.origin);
            assert_eq!(rebuilt.join, original.join);
            assert_eq!(rebuilt.line_width, original.line_width);
        }
    }

    #[test]
    fn round_trip_plain_rows() {
        let rows = vec![
            ascii_row(20, "hello world"),
            ascii_row(20, "the quick brown fox"),
            ascii_row(20, "jumps over the lazy dog"),
        ];
        assert_block_round_trip_exact(&rows);
    }

    #[test]
    fn round_trip_colored_runs() {
        let mut row_a = Row::new(10);
        let mut red_tag = freminal_common::buffer_states::format_tag::FormatTag::default();
        red_tag.colors.set_color(TerminalColor::Red);
        row_a.insert_text(0, &[TChar::Ascii(b'a'), TChar::Ascii(b'b')], &red_tag);

        let mut row_b = Row::new(10);
        let bold_tag = freminal_common::buffer_states::format_tag::FormatTag {
            font_weight: FontWeight::Bold,
            ..freminal_common::buffer_states::format_tag::FormatTag::default()
        };
        row_b.insert_text(0, &[TChar::Ascii(b'c'), TChar::Ascii(b'd')], &bold_tag);

        assert_block_round_trip_exact(&[row_a, row_b]);
    }

    #[test]
    fn round_trip_wide_chars() {
        let mut row = Row::new(10);
        let tag = freminal_common::buffer_states::format_tag::FormatTag::default();
        row.insert_text(0, &[TChar::from('中'), TChar::from('文')], &tag);
        assert_block_round_trip_exact(&[row]);
    }

    #[test]
    fn round_trip_url_tags() {
        use std::sync::Arc;

        let mut row = Row::new(10);
        let url_tag = freminal_common::buffer_states::format_tag::FormatTag {
            url: Some(Arc::new(freminal_common::buffer_states::url::Url {
                id: Some("id1".to_string()),
                url: "https://example.com".to_string(),
            })),
            ..freminal_common::buffer_states::format_tag::FormatTag::default()
        };
        row.insert_text(0, &[TChar::Ascii(b'a'), TChar::Ascii(b'b')], &url_tag);
        assert_block_round_trip_exact(&[row]);
    }

    #[test]
    fn round_trip_blank_and_sparse_rows() {
        let rows = vec![Row::new(80), Row::new(80), ascii_row(80, "x")];
        assert_block_round_trip_exact(&rows);
    }

    #[test]
    fn round_trip_block_boundary_many_rows() {
        // A representative block-sized run (256 logical scrollback rows,
        // per the Task 119 design decision of ~128–256 rows/block),
        // asserting every row survives the round trip.
        let rows: Vec<Row> = (0..256)
            .map(|i| ascii_row(80, &format!("scrollback line number {i}")))
            .collect();
        assert_block_round_trip_exact(&rows);
    }

    #[test]
    fn round_trip_high_entropy_block() {
        // Pseudo-random-looking content (varied colors, varied text, no
        // shared structure) — the pessimistic "high-entropy colored"
        // bracket from the Task 119 feasibility spike. Still must
        // round-trip exactly even though the compression ratio is worse.
        let mut rows = Vec::new();
        let mut seed: u32 = 0x1234_5678;
        for i in 0..64 {
            let mut row = Row::new(40);
            for col in 0..40 {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let byte = u8::try_from(32 + (seed >> 24) % 95).unwrap();
                let mut tag = freminal_common::buffer_states::format_tag::FormatTag::default();
                tag.colors.set_color(TerminalColor::Custom(
                    u8::try_from((seed >> 16) & 0xFF).unwrap(),
                    u8::try_from((seed >> 8) & 0xFF).unwrap(),
                    u8::try_from(seed & 0xFF).unwrap(),
                ));
                row.insert_text(col, &[TChar::Ascii(byte)], &tag);
            }
            rows.push(row);
            let _ = i;
        }
        assert_block_round_trip_exact(&rows);
    }

    #[test]
    fn round_trip_every_line_width_and_origin() {
        let mut rows = Vec::new();
        for origin in [
            RowOrigin::HardBreak,
            RowOrigin::SoftWrap,
            RowOrigin::ScrollFill,
        ] {
            for line_width in [
                LineWidth::Normal,
                LineWidth::DoubleWidth,
                LineWidth::DoubleHeightTop,
                LineWidth::DoubleHeightBottom,
            ] {
                let tag = freminal_common::buffer_states::format_tag::FormatTag::default();
                let cells = vec![crate::cell::Cell::new(TChar::Ascii(b'x'), tag)];
                let mut row = Row::from_cells(10, origin, RowJoin::NewLogicalLine, cells);
                row.line_width = line_width;
                rows.push(row);
            }
        }
        assert_block_round_trip_exact(&rows);
    }

    #[test]
    fn compressed_bytes_smaller_than_sum_of_compact_row_heap_bytes_for_compressible_block() {
        // A representative compressible block: many rows sharing the same
        // uniform (non-default) formatting and repetitive text — the
        // "shell session (typical)" bracket from the Task 119 feasibility
        // spike, where compression on top of the Task-118 flat form is
        // expected to help substantially.
        let mut tag = freminal_common::buffer_states::format_tag::FormatTag::default();
        tag.colors.set_color(TerminalColor::Green);

        let compact_rows: Vec<CompactRow> = (0..200)
            .map(|_| {
                let mut row = Row::new(120);
                let text = "the quick brown fox jumps over the lazy dog ".repeat(2);
                let chars: Vec<TChar> = text.bytes().map(TChar::Ascii).collect();
                row.insert_text(0, &chars, &tag);
                CompactRow::from_row(&row).expect("row should be compactable")
            })
            .collect();

        let sum_compact_heap_bytes: usize = compact_rows.iter().map(CompactRow::heap_bytes).sum();

        let block = CompressedBlock::from_rows(&compact_rows);

        assert!(
            block.compressed_bytes() < sum_compact_heap_bytes,
            "compressed size ({}) should be smaller than the sum of the rows' \
             CompactRow heap bytes ({})",
            block.compressed_bytes(),
            sum_compact_heap_bytes
        );
    }

    #[test]
    fn decompress_into_reuses_scratch_buffer() {
        let rows = [ascii_row(20, "hello world"), ascii_row(20, "goodbye world")];
        let compact_rows: Vec<CompactRow> = rows
            .iter()
            .map(|r| CompactRow::from_row(r).unwrap())
            .collect();
        let block = CompressedBlock::from_rows(&compact_rows);

        let mut scratch = Vec::new();
        let first = block.decompress_into(&mut scratch).unwrap();
        assert_eq!(first.len(), 2);

        // Reuse the same scratch buffer for a second decompress call —
        // must still decode correctly (scratch is cleared/resized inside
        // `decompress_into`, not assumed empty on entry).
        scratch.extend_from_slice(&[0xAA; 64]); // pollute the buffer
        let second = block.decompress_into(&mut scratch).unwrap();
        assert_eq!(second.len(), 2);
        assert_eq!(second[0].to_row().cells(), first[0].to_row().cells());
    }

    #[test]
    fn empty_block_round_trips() {
        let block = CompressedBlock::from_rows(&[]);
        assert_eq!(block.row_count(), 0);
        let decompressed = block.decompress().unwrap();
        assert!(decompressed.is_empty());
    }

    #[test]
    fn malformed_compressed_bytes_returns_none_not_panic() {
        let rows = [ascii_row(20, "hello world")];
        let compact_rows: Vec<CompactRow> = rows
            .iter()
            .map(|r| CompactRow::from_row(r).unwrap())
            .collect();
        let mut block = CompressedBlock::from_rows(&compact_rows);

        // Corrupt the compressed payload in place.
        for byte in &mut block.compressed {
            *byte ^= 0xFF;
        }

        assert!(block.decompress().is_none());
    }

    #[test]
    fn truncated_compressed_bytes_returns_none_not_panic() {
        let rows: Vec<Row> = (0..8).map(|i| ascii_row(40, &format!("row {i}"))).collect();
        let compact_rows: Vec<CompactRow> = rows
            .iter()
            .map(|r| CompactRow::from_row(r).unwrap())
            .collect();
        let mut block = CompressedBlock::from_rows(&compact_rows);

        block.compressed.truncate(block.compressed.len() / 2);

        // Must not panic; either decodes to something wrong-but-safe or
        // returns `None`. We only assert it never panics — the call
        // itself completing is the assertion.
        let _ = block.decompress();
    }

    #[test]
    fn row_count_mismatch_after_corruption_returns_none() {
        // Build a block, then hand-construct a `CompressedBlock` whose
        // recorded `row_count` disagrees with what's actually baked into
        // the compressed payload, to exercise the consistency check.
        let rows = [ascii_row(20, "hello"), ascii_row(20, "world")];
        let compact_rows: Vec<CompactRow> = rows
            .iter()
            .map(|r| CompactRow::from_row(r).unwrap())
            .collect();
        let mut block = CompressedBlock::from_rows(&compact_rows);
        block.row_count = 99; // now disagrees with the baked-in prefix

        assert!(block.decompress().is_none());
    }
}
