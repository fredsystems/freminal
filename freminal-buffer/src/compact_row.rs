// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! A compact, format-run-sharing representation of a scrollback [`Row`].
//!
//! [`Row`] stores one [`Cell`] per occupied column, and every `Cell` carries
//! its own owned [`FormatTag`] (40 bytes). For scrollback rows — which are
//! rarely mutated once pushed out of the visible viewport — that per-cell
//! `FormatTag` duplication is wasteful: long runs of cells sharing identical
//! formatting (the common case) each pay the full `FormatTag` cost.
//!
//! [`CompactRow`] stores the same information with formatting and wide-glyph
//! bookkeeping run-length-encoded instead of duplicated per cell. Conversion
//! is lossless in both directions: [`CompactRow::from_row`] /
//! [`CompactRow::to_row`] round-trip a compactable row exactly (see the
//! module's test suite).
//!
//! This module is a pure data transform. It has no knowledge of `Buffer` and
//! is not wired into the scrollback storage path — that integration is a
//! separate task.

use conv2::ValueFrom;
use freminal_common::buffer_states::{format_tag::FormatTag, tchar::TChar};

use crate::{
    cell::Cell,
    row::{LineWidth, Row, RowJoin, RowOrigin},
};

// Bit flags used to pack a cell's wide-glyph bookkeeping into a single byte
// for run-length encoding.
const WIDE_FLAG_HEAD: u8 = 0b01;
const WIDE_FLAG_CONTINUATION: u8 = 0b10;

// Compile-time assertions capturing the measured sizes of the types this
// module's space accounting depends on. If either of these change, the
// heap-savings claims documented above (and measured in this module's tests)
// need to be re-verified.
const _: () = assert!(
    core::mem::size_of::<Cell>() == 72,
    "Cell size changed — re-measure CompactRow's space savings"
);
const _: () = assert!(
    core::mem::size_of::<FormatTag>() == 40,
    "FormatTag size changed — re-measure CompactRow's space savings"
);

/// Returns `true` if `row` can be losslessly represented as a [`CompactRow`].
///
/// Rows containing any cell with an inline image placement are excluded:
/// image placements are per-cell data that doesn't benefit from format-run
/// sharing, and keeping them out of `CompactRow` keeps its representation
/// simple (no `Option<Box<ImagePlacement>>` bookkeeping to carry through the
/// run-length encoding). Callers that need to compact scrollback rows should
/// check this before calling [`CompactRow::from_row`], though `from_row`
/// itself also performs this check and returns `None` rather than silently
/// dropping image data.
#[must_use]
pub fn is_compactable(row: &Row) -> bool {
    !row.cells().iter().any(Cell::has_image)
}

/// A compact, format-run-sharing representation of a scrollback [`Row`].
///
/// See the module documentation for the motivation. Construct via
/// [`CompactRow::from_row`] and rebuild an equivalent `Row` via
/// [`CompactRow::to_row`].
#[derive(Debug, Clone)]
pub struct CompactRow {
    /// One [`TChar`] per stored cell, in column order. Unlike `Row`, there is
    /// no wide-continuation special-casing here: continuation cells store
    /// whatever `TChar` they actually carried (almost always `TChar::Space`),
    /// and their wide-glyph role is recovered from `wide_runs`.
    chars: Vec<TChar>,
    /// Run-length-encoded formatting: `(tag, run_length)`. Adjacent cells
    /// carrying `==` tags are coalesced into a single run. The sum of all
    /// `run_length`s equals `chars.len()`.
    tag_runs: Vec<(FormatTag, u32)>,
    /// Run-length-encoded wide-glyph bookkeeping: `(flags, run_length)`,
    /// where `flags` packs `is_wide_head` (bit 0) and `is_wide_continuation`
    /// (bit 1). The sum of all `run_length`s equals `chars.len()`.
    wide_runs: Vec<(u8, u32)>,
    width: usize,
    origin: RowOrigin,
    join: RowJoin,
    line_width: LineWidth,
}

/// Pack a cell's wide-glyph flags into a single byte for run-length coding.
const fn wide_flags_of(cell: &Cell) -> u8 {
    let mut flags = 0u8;
    if cell.is_head() {
        flags |= WIDE_FLAG_HEAD;
    }
    if cell.is_continuation() {
        flags |= WIDE_FLAG_CONTINUATION;
    }
    flags
}

/// Append `value` to a run-length-encoded vec, coalescing with the last run
/// if `value` matches it, or starting a new run of length 1 otherwise.
///
/// Run lengths grow one cell at a time (via `saturating_add(1)`), so no
/// `usize -> u32` conversion is ever needed on this path: a run's length can
/// only ever be one more than the previous count, never an arbitrary `usize`
/// total. If a single run somehow exceeded `u32::MAX` cells (not physically
/// possible for a terminal row width), the run length simply saturates at
/// `u32::MAX` rather than panicking or wrapping.
fn push_run<T: PartialEq + Clone>(runs: &mut Vec<(T, u32)>, value: T) {
    if let Some(last) = runs.last_mut()
        && last.0 == value
    {
        last.1 = last.1.saturating_add(1);
        return;
    }
    runs.push((value, 1));
}

/// Expand a run-length-encoded vec back into a flat per-cell iterator.
///
/// `u32 -> usize` is lossless on every platform this crate targets (desktop
/// platforms, where `usize` is at least 32 bits), so the conversion cannot
/// fail in practice. The `unwrap_or(0)` fallback is unreachable there; it
/// degrades a hypothetical failure to "contribute zero cells from this run"
/// rather than panicking or under/over-allocating.
fn expand_runs<T>(runs: &[(T, u32)]) -> impl Iterator<Item = &T> {
    runs.iter().flat_map(|(value, len)| {
        let count = usize::value_from(*len).unwrap_or(0);
        std::iter::repeat_n(value, count)
    })
}

impl CompactRow {
    /// Build a `CompactRow` from `row`, or `None` if `row` contains any
    /// image cell (see [`is_compactable`]).
    #[must_use]
    pub fn from_row(row: &Row) -> Option<Self> {
        let cells = row.cells();
        if cells.iter().any(Cell::has_image) {
            return None;
        }

        let mut chars = Vec::with_capacity(cells.len());
        let mut tag_runs: Vec<(FormatTag, u32)> = Vec::new();
        let mut wide_runs: Vec<(u8, u32)> = Vec::new();

        for cell in cells {
            chars.push(*cell.tchar());
            push_run(&mut tag_runs, cell.tag().clone());
            push_run(&mut wide_runs, wide_flags_of(cell));
        }

        Some(Self {
            chars,
            tag_runs,
            wide_runs,
            width: row.max_width(),
            origin: row.origin,
            join: row.join,
            line_width: row.line_width,
        })
    }

    /// Rebuild an equivalent [`Row`] from this compact representation.
    ///
    /// The rebuilt row's cell contents (value, format, wide-glyph flags),
    /// width, origin, join, and line-width are exactly equal to the source
    /// row's. The rebuilt row's `dirty` flag is *not* part of this identity:
    /// `dirty` is a cache-staleness marker, not row content, and
    /// [`Row::from_cells`] always constructs with `dirty: true`.
    #[must_use]
    pub fn to_row(&self) -> Row {
        let tags = expand_runs(&self.tag_runs);
        let wide_flags = expand_runs(&self.wide_runs);

        let cells: Vec<Cell> = self
            .chars
            .iter()
            .zip(tags)
            .zip(wide_flags)
            .map(|((value, tag), flags)| {
                let is_head = flags & WIDE_FLAG_HEAD != 0;
                let is_continuation = flags & WIDE_FLAG_CONTINUATION != 0;
                Cell::from_parts(*value, tag.clone(), is_head, is_continuation)
            })
            .collect();

        let mut row = Row::from_cells(self.width, self.origin, self.join, cells);
        row.line_width = self.line_width;
        row
    }

    /// Heap bytes retained by this `CompactRow`'s backing allocations,
    /// computed from allocation *capacity* (the real resident cost), not
    /// `len()`. Mirrors the accounting style of `Buffer::heap_bytes`.
    #[must_use]
    pub const fn heap_bytes(&self) -> usize {
        self.chars.capacity() * core::mem::size_of::<TChar>()
            + self.tag_runs.capacity() * core::mem::size_of::<(FormatTag, u32)>()
            + self.wide_runs.capacity() * core::mem::size_of::<(u8, u32)>()
    }

    /// Number of distinct format runs. Exposed for tests/diagnostics that
    /// want to verify run-coalescing behavior directly.
    #[must_use]
    pub const fn tag_run_count(&self) -> usize {
        self.tag_runs.len()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::sync::Arc;

    use freminal_common::{
        buffer_states::{fonts::FontWeight, url::Url},
        colors::TerminalColor,
    };

    use crate::image_store::{ImagePlacement, ImageProtocol};

    use super::*;

    fn ascii_row(width: usize, text: &str, tag: &FormatTag) -> Row {
        let mut row = Row::new(width);
        let chars: Vec<TChar> = text.bytes().map(TChar::Ascii).collect();
        row.insert_text(0, &chars, tag);
        row
    }

    fn assert_round_trip_exact(row: &Row) {
        let compact = CompactRow::from_row(row).expect("row should be compactable");
        let rebuilt = compact.to_row();

        assert_eq!(
            rebuilt.cells(),
            row.cells(),
            "cell contents must match exactly"
        );
        assert_eq!(rebuilt.max_width(), row.max_width());
        assert_eq!(rebuilt.origin, row.origin);
        assert_eq!(rebuilt.join, row.join);
        assert_eq!(rebuilt.line_width, row.line_width);
        // `dirty` is explicitly exempt from the round-trip identity.
    }

    // -----------------------------------------------------------------------
    // Plain ASCII row, default format
    // -----------------------------------------------------------------------

    #[test]
    fn round_trip_plain_ascii_row() {
        let row = ascii_row(20, "hello world", &FormatTag::default());
        assert_round_trip_exact(&row);

        let compact = CompactRow::from_row(&row).unwrap();
        // Uniform default formatting across the whole row -> a single run.
        assert_eq!(compact.tag_run_count(), 1);
    }

    // -----------------------------------------------------------------------
    // Colored runs: multiple FormatTag runs, coalescing verified
    // -----------------------------------------------------------------------

    #[test]
    fn round_trip_colored_runs_and_coalescing() {
        let mut row = Row::new(10);
        let default_tag = FormatTag::default();
        let mut red_tag = FormatTag::default();
        red_tag.colors.set_color(TerminalColor::Red);
        let bold_tag = FormatTag {
            font_weight: FontWeight::Bold,
            ..FormatTag::default()
        };

        // "AA" default, "BB" red, "CC" bold, "DD" red again (not adjacent to
        // the first red run, so it must NOT be coalesced with it).
        row.insert_text(0, &[TChar::Ascii(b'A'), TChar::Ascii(b'A')], &default_tag);
        row.insert_text(2, &[TChar::Ascii(b'B'), TChar::Ascii(b'B')], &red_tag);
        row.insert_text(4, &[TChar::Ascii(b'C'), TChar::Ascii(b'C')], &bold_tag);
        row.insert_text(6, &[TChar::Ascii(b'D'), TChar::Ascii(b'D')], &red_tag);

        assert_round_trip_exact(&row);

        let compact = CompactRow::from_row(&row).unwrap();
        // 4 distinct runs: default(2), red(2), bold(2), red(2) — the two red
        // runs are NOT adjacent, so they are not coalesced into one.
        assert_eq!(compact.tag_run_count(), 4);
    }

    #[test]
    fn uniform_row_has_far_fewer_runs_than_varied_row() {
        let uniform = ascii_row(40, &"x".repeat(40), &FormatTag::default());
        let varied_tag_a = FormatTag {
            font_weight: FontWeight::Bold,
            ..FormatTag::default()
        };

        let mut varied = Row::new(40);
        for i in 0..40 {
            let tag = if i % 2 == 0 {
                FormatTag::default()
            } else {
                varied_tag_a.clone()
            };
            varied.insert_text(i, &[TChar::Ascii(b'x')], &tag);
        }

        let uniform_compact = CompactRow::from_row(&uniform).unwrap();
        let varied_compact = CompactRow::from_row(&varied).unwrap();

        assert_eq!(uniform_compact.tag_run_count(), 1);
        assert_eq!(varied_compact.tag_run_count(), 40);
        assert!(uniform_compact.tag_run_count() < varied_compact.tag_run_count());
    }

    // -----------------------------------------------------------------------
    // Mixed tags with a URL (Arc<Url>): preserved and shared, not deep-copied
    // -----------------------------------------------------------------------

    #[test]
    fn round_trip_preserves_and_shares_url_arc() {
        let mut row = Row::new(10);
        let default_tag = FormatTag::default();
        let url = Arc::new(Url {
            id: None,
            url: "https://example.com".to_string(),
        });
        let url_tag = FormatTag {
            url: Some(Arc::clone(&url)),
            ..FormatTag::default()
        };

        row.insert_text(0, &[TChar::Ascii(b'a')], &default_tag);
        row.insert_text(1, &[TChar::Ascii(b'b'), TChar::Ascii(b'c')], &url_tag);

        assert_round_trip_exact(&row);

        let compact = CompactRow::from_row(&row).unwrap();
        // The url-tagged run should be a single coalesced run (both cells
        // carry `==` tags).
        assert_eq!(compact.tag_run_count(), 2);

        // The Arc for the shared run is the very same allocation (refcount
        // bump only, not a deep copy of the URL string).
        let url_run_tag = &compact.tag_runs[1].0;
        let stored_url = url_run_tag.url.as_ref().expect("expected a url");
        assert!(Arc::ptr_eq(stored_url, &url));
        assert_eq!(stored_url.url, "https://example.com");

        let rebuilt = compact.to_row();
        let rebuilt_url = rebuilt.cells()[1].tag().url.as_ref().expect("url");
        assert_eq!(rebuilt_url.url, "https://example.com");
        assert_eq!(rebuilt_url, &url);
    }

    // -----------------------------------------------------------------------
    // Wide char (head + continuation)
    // -----------------------------------------------------------------------

    #[test]
    fn round_trip_wide_char_head_and_continuation() {
        let mut row = Row::new(10);
        let tag = FormatTag::default();
        row.insert_text(0, &[TChar::from('中')], &tag);

        assert!(row.cells()[0].is_head());
        assert!(row.cells()[1].is_continuation());

        assert_round_trip_exact(&row);
    }

    // -----------------------------------------------------------------------
    // Orphan continuation cell (continuation with no head)
    // -----------------------------------------------------------------------

    #[test]
    fn round_trip_orphan_continuation_cell() {
        let mut row = Row::new(5);
        row.cells_mut_push(Cell::new(TChar::Ascii(b'a'), FormatTag::default()));
        // An orphan continuation: no preceding head cell.
        row.cells_mut_push(Cell::wide_continuation());
        row.cells_mut_push(Cell::new(TChar::Ascii(b'b'), FormatTag::default()));

        assert!(!row.cells()[0].is_continuation());
        assert!(row.cells()[1].is_continuation());
        assert!(!row.cells()[1].is_head());

        assert_round_trip_exact(&row);
    }

    // -----------------------------------------------------------------------
    // Blank/sparse row (empty cells vec)
    // -----------------------------------------------------------------------

    #[test]
    fn round_trip_blank_sparse_row() {
        let row = Row::new(80);
        assert!(row.cells().is_empty());
        assert_round_trip_exact(&row);

        let compact = CompactRow::from_row(&row).unwrap();
        assert!(compact.tag_runs.is_empty());
        assert!(compact.wide_runs.is_empty());
        assert!(compact.chars.is_empty());
    }

    // -----------------------------------------------------------------------
    // Non-default origin/join/line_width
    // -----------------------------------------------------------------------

    #[test]
    fn round_trip_nondefault_origin_join_line_width() {
        let tag = FormatTag::default();
        let chars: Vec<TChar> = b"soft-wrapped".iter().map(|&b| TChar::Ascii(b)).collect();
        let mut cells = Vec::new();
        for c in &chars {
            cells.push(Cell::new(*c, tag.clone()));
        }
        let mut row = Row::from_cells(40, RowOrigin::SoftWrap, RowJoin::ContinueLogicalLine, cells);
        row.line_width = LineWidth::DoubleWidth;

        assert_round_trip_exact(&row);
    }

    // -----------------------------------------------------------------------
    // Image rows opt out
    // -----------------------------------------------------------------------

    fn image_placement() -> ImagePlacement {
        ImagePlacement {
            image_id: 1,
            col_in_image: 0,
            row_in_image: 0,
            protocol: ImageProtocol::Kitty,
            image_number: None,
            placement_id: None,
            z_index: 0,
            source_crop: None,
            placement_instance: 1,
            subcell_offset: None,
        }
    }

    #[test]
    fn image_row_is_not_compactable() {
        let mut row = Row::new(10);
        row.set_image_cell(2, image_placement(), FormatTag::default());

        assert!(!is_compactable(&row));
        assert!(CompactRow::from_row(&row).is_none());
    }

    #[test]
    fn compactable_row_without_images_reports_true() {
        let row = ascii_row(10, "abc", &FormatTag::default());
        assert!(is_compactable(&row));
        assert!(CompactRow::from_row(&row).is_some());
    }

    // -----------------------------------------------------------------------
    // Size / heap-savings assertion
    // -----------------------------------------------------------------------

    #[test]
    fn compact_row_heap_bytes_smaller_than_per_cell_row_for_uniform_formatting() {
        // A representative 200-column row with uniform (non-default, so it's
        // not trivially sparse) formatting: every cell in `Row` pays the full
        // `size_of::<Cell>()` cost, but `CompactRow` collapses the formatting
        // into a single run.
        let width = 200;
        let mut tag = FormatTag::default();
        tag.colors.set_color(TerminalColor::Green);

        let text = "x".repeat(width);
        let row = ascii_row(width, &text, &tag);

        let compact = CompactRow::from_row(&row).unwrap();
        assert_eq!(compact.tag_run_count(), 1);

        let row_per_cell_cost = core::mem::size_of_val(row.cells());
        let compact_cost = compact.heap_bytes();

        assert!(
            compact_cost < row_per_cell_cost,
            "compact heap cost ({compact_cost}) should be materially smaller than \
             the equivalent Row's per-cell cost ({row_per_cell_cost})"
        );
        // Sanity: the saving should not be trivial for a 200-cell uniform row.
        assert!(row_per_cell_cost / compact_cost.max(1) >= 2);
    }

    // -----------------------------------------------------------------------
    // Run-length invariants
    // -----------------------------------------------------------------------

    #[test]
    fn run_lengths_sum_to_chars_len() {
        let row = ascii_row(30, "some text here to test runs!!", &FormatTag::default());
        let compact = CompactRow::from_row(&row).unwrap();

        let tag_total: u32 = compact.tag_runs.iter().map(|(_, len)| *len).sum();
        let wide_total: u32 = compact.wide_runs.iter().map(|(_, len)| *len).sum();

        assert_eq!(usize::try_from(tag_total).unwrap(), compact.chars.len());
        assert_eq!(usize::try_from(wide_total).unwrap(), compact.chars.len());
    }
}
