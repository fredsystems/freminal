// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Row flattening and text extraction operations for [`Buffer`].
//!
//! Converts buffer rows into flat `(Vec<TChar>, Vec<FormatTag>)` pairs
//! for the GUI renderer (`visible_as_tchars_and_tags`,
//! `scrollback_as_tchars_and_tags`, `rows_as_tchars_and_tags_cached`),
//! and provides plain-text extraction (`extract_text`, `extract_block_text`).
//!
//! ## Per-row flatten cache
//!
//! Each entry in [`Buffer::row_cache`](super::Buffer::row_cache) is a
//! [`RowCacheEntry`] that bundles:
//!
//! - `chars`: flat per-row `TChar` sequence (wide-continuation cells skipped)
//! - `tags`: per-row `FormatTag`s with row-relative offsets
//! - `bytes`: UTF-8 byte buffer mirroring `chars`, used for byte-based URL
//!   regex matching without per-row `String` allocation
//! - `byte_to_char`: parallel map from byte offset in `bytes` to character
//!   index in `chars`
//! - `auto_urls`: ranges (in character indices) where plain-text URLs were
//!   auto-detected, with the canonical URL string ready to be spliced into
//!   `FormatTag.url` at merge time
//!
//! All five fields are produced in a single pass over the row's cells. The
//! merge step in [`Buffer::rows_as_tchars_and_tags_cached`] rebases tag
//! offsets to global indices and splices auto-URL ranges into the merged
//! `FormatTag` vec. When an existing `FormatTag.url` is already `Some(_)`
//! (e.g. an OSC 8 hyperlink), auto-detected URLs are suppressed within that
//! range — OSC 8 always wins.

use std::sync::Arc;

use freminal_common::buffer_states::{
    buffer_type::BufferType, format_tag::FormatTag, tchar::TChar, url::Url,
};

use crate::row::{Row, RowJoin};
use crate::url_detect;

use super::tags_same_format;
use crate::buffer::Buffer;

/// A single auto-detected URL range within one row's flat character stream.
///
/// Offsets are character indices into the row's `chars` vec (half-open
/// `[char_start, char_end)`), not byte offsets. The `url` field holds the
/// canonical URL string (already stripped of trailing punctuation) behind
/// an `Arc` so that tag splicing at merge time is a cheap refcount bump.
#[derive(Debug, Clone)]
pub struct AutoUrlRange {
    /// Inclusive start character index into the row's `chars`.
    pub char_start: usize,
    /// Exclusive end character index into the row's `chars`.
    pub char_end: usize,
    /// The detected URL, wrapped for cheap splicing into multiple tags.
    pub url: Arc<Url>,
    /// Mirrors [`url_detect::UrlMatch::touches_buffer_end`]: `true` when the
    /// raw (pre-trim) match reached the end of the row's byte buffer, i.e.
    /// this range might be a DECAWM-wrapped URL continuing onto the next
    /// row. Used as the cheap, precise signal for whether a soft-wrapped
    /// group of rows needs the group-level URL redetection in
    /// [`Buffer::refresh_row_cache_and_refine_wrapped_urls`].
    pub touches_row_end: bool,
}

/// Per-row flatten cache entry.
///
/// Produced by [`Buffer::flatten_row`] and consumed by
/// [`Buffer::rows_as_tchars_and_tags_cached`] at merge time. See the module
/// docs for field semantics.
#[derive(Debug, Clone)]
pub struct RowCacheEntry {
    /// Flat per-row character sequence (wide-continuation cells skipped).
    pub chars: Vec<TChar>,
    /// Per-row format tags with **row-relative** offsets into `chars`.
    pub tags: Vec<FormatTag>,
    /// UTF-8 byte representation of `chars`, used for byte-based URL regex
    /// matching. Empty when `auto_detect_urls` was disabled at flatten time.
    pub bytes: Vec<u8>,
    /// Parallel map from byte offset in `bytes` to character index in `chars`.
    /// `byte_to_char[i]` is the character index for the character that starts
    /// at byte `i` (entries for continuation bytes of a multi-byte codepoint
    /// repeat the starting character index). Empty when `auto_detect_urls`
    /// was disabled.
    pub byte_to_char: Vec<u32>,
    /// Auto-detected URL ranges (character indices). Empty when detection
    /// was disabled or no URLs were found.
    pub auto_urls: Vec<AutoUrlRange>,
}

impl RowCacheEntry {
    /// Create an empty cache entry. Used by `Default` and for test scaffolding.
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            chars: Vec::new(),
            tags: Vec::new(),
            bytes: Vec::new(),
            byte_to_char: Vec::new(),
            auto_urls: Vec::new(),
        }
    }
}

impl Default for RowCacheEntry {
    fn default() -> Self {
        Self::empty()
    }
}

impl Buffer {
    /// Convert the currently visible rows into a flat `(Vec<TChar>, Vec<FormatTag>)` pair
    /// Convert visible rows (with the given `scroll_offset`) into flat
    /// `(Vec<TChar>, Vec<FormatTag>)` suitable for the GUI renderer.
    ///
    /// Pass `scroll_offset = 0` when calling from the PTY thread (which always
    /// operates at the live bottom).
    ///
    /// Takes `&mut self` because it updates the per-row cache and clears dirty
    /// flags on rows that are freshly flattened.
    #[must_use]
    pub fn visible_as_tchars_and_tags(
        &mut self,
        scroll_offset: usize,
    ) -> (Vec<TChar>, Vec<FormatTag>, Vec<usize>, Vec<usize>) {
        self.visible_as_tchars_and_tags_extended(scroll_offset, 0)
    }

    /// Like [`Self::visible_as_tchars_and_tags`] but extends the flatten window
    /// upward by `extra_rows` (see
    /// [`Buffer::visible_window_bounds`](super::Buffer::visible_window_bounds)).
    ///
    /// The GUI passes a non-zero `extra_rows` when command-block folds collapse
    /// rows in the visible window: the extra rows above the normal window
    /// provide real content to fill the screen once the folds are collapsed,
    /// so the live bottom stays pinned instead of leaving a blank gap.
    ///
    /// When `extra_rows == 0` this is identical to
    /// [`Self::visible_as_tchars_and_tags`].
    #[must_use]
    pub fn visible_as_tchars_and_tags_extended(
        &mut self,
        scroll_offset: usize,
        extra_rows: usize,
    ) -> (Vec<TChar>, Vec<FormatTag>, Vec<usize>, Vec<usize>) {
        let (visible_start, visible_end) = self.visible_window_bounds(scroll_offset, extra_rows);
        // Task 119: when the user scrolls back, this window can reach into
        // compressed scrollback (a nonzero `scroll_offset` lowers
        // `visible_window_start`). Decompress any evicted rows in the window
        // before reading their cells, so `flatten_row` never touches an
        // evicted placeholder. A no-op when nothing in the window is
        // compressed (the common live-view case).
        self.ensure_decompressed(visible_start..visible_end);
        let auto_detect = self.auto_detect_urls;
        Self::rows_as_tchars_and_tags_cached(
            &mut self.rows[visible_start..visible_end],
            &mut self.row_cache[visible_start..visible_end],
            auto_detect,
        )
    }

    /// Flatten all scrollback rows (everything before the visible window) into
    /// a linear `(Vec<TChar>, Vec<FormatTag>, Vec<usize>, Vec<usize>)` tuple
    /// using the same algorithm as [`Self::visible_as_tchars_and_tags`].
    ///
    /// Returns `(vec![], vec![], vec![], vec![])` for the alternate screen
    /// buffer, which never accumulates scrollback.
    ///
    /// Pass `scroll_offset = 0` when calling from the PTY thread.
    pub fn scrollback_as_tchars_and_tags(
        &mut self,
        scroll_offset: usize,
    ) -> (Vec<TChar>, Vec<FormatTag>, Vec<usize>, Vec<usize>) {
        // Alternate buffer has no scrollback.
        if self.kind == BufferType::Alternate {
            return (vec![], vec![], vec![], vec![]);
        }

        let visible_start = self.visible_window_start(scroll_offset);

        if visible_start == 0 {
            // No scrollback rows exist yet.
            return (vec![], vec![], vec![], vec![]);
        }

        // Task 119.4: restore any deep-cold compressed rows in this range
        // back to real (Task-118 `Compact`) content before flattening. This
        // is the decompress-on-read seam — the visible window (never
        // reached here) never needs it.
        self.ensure_decompressed(0..visible_start);

        let auto_detect = self.auto_detect_urls;
        let result = Self::rows_as_tchars_and_tags_cached(
            &mut self.rows[..visible_start],
            &mut self.row_cache[..visible_start],
            auto_detect,
        );

        // Task 118.4: a full-scrollback flatten (the Ctrl-F search-buffer
        // path) is the only caller that reads cold scrollback history in
        // bulk. `rows_as_tchars_and_tags_cached` just built (or reused) a
        // `RowCacheEntry` per row, and reading a compact row's cells along
        // the way (in `flatten_row`) also warmed its `OnceCell` decompaction
        // memo — so every compact row now momentarily holds three
        // representations at once (its `CompactRow`, the memoized
        // `Vec<Cell>`, and the `RowCacheEntry`). Cold scrollback rows are
        // rarely re-read, so we drop the two larger, cheaply-rebuildable
        // copies here and keep only the small `CompactRow`. The next
        // scrollback flatten rebuilds an identical `RowCacheEntry` from the
        // `CompactRow` (`entry.is_none()` forces a rebuild in Step 1 of
        // `rows_as_tchars_and_tags_cached`), so output is unaffected — only
        // resident memory changes.
        //
        // Visible rows are never in this slice (`..visible_start` excludes
        // them), so their cache is untouched: they re-render every frame and
        // must stay warm.
        for (row, entry) in self.rows[..visible_start]
            .iter_mut()
            .zip(self.row_cache[..visible_start].iter_mut())
        {
            if row.is_compact() {
                *entry = None;
                row.release_decompacted_cache();
            }
        }

        result
    }

    /// Shared helper: flatten a slice of [`Row`]s into `(Vec<TChar>,
    /// Vec<FormatTag>, Vec<usize>)`, using a per-row cache to skip rows that
    /// have not changed since the last snapshot.
    ///
    /// For each row:
    /// - If `row.dirty` or the cache entry is `None`, flatten the row, populate
    ///   the cache entry, and call `row.mark_clean()`.
    /// - Otherwise reuse the cached per-row `RowCacheEntry` directly.
    ///
    /// Per-row tag offsets are stored relative to each row's own character
    /// slice (starting at 0).  The merge step below re-computes global offsets
    /// each time, so the cache never stores stale absolute positions.
    ///
    /// `auto_detect` controls whether the per-row byte buffer and auto-URL
    /// detection are populated at flatten time. When `false`, `bytes`,
    /// `byte_to_char`, and `auto_urls` on each cache entry are empty.
    ///
    /// At merge time, any `AutoUrlRange` whose covered character range does
    /// not already carry a `FormatTag.url` is spliced into the merged tag
    /// stream: covering tags are split into (pre, overlap-with-url, post)
    /// segments. OSC 8 links always win over auto-detected ones.
    ///
    /// The returned tuple contains:
    /// - `Vec<TChar>` — flat character data
    /// - `Vec<FormatTag>` — merged format tags with global offsets
    /// - `Vec<usize>` — row offsets (`row_offsets[r]` is the flat index where
    ///   row `r` begins)
    /// - `Vec<usize>` — URL tag indices (indices into the tags vec where
    ///   `url.is_some()`)
    fn rows_as_tchars_and_tags_cached(
        rows: &mut [Row],
        cache: &mut [Option<RowCacheEntry>],
        auto_detect: bool,
    ) -> (Vec<TChar>, Vec<FormatTag>, Vec<usize>, Vec<usize>) {
        let row_count = rows.len();

        // ── Step 1 (+ 1.5): ensure every row has an up-to-date cache entry,
        // and fix up auto-detected URLs that wrap across rows. See
        // `refresh_row_cache_and_refine_wrapped_urls`'s doc comment.
        let refined_auto_urls =
            Self::refresh_row_cache_and_refine_wrapped_urls(rows, cache, auto_detect);
        let mut refined_cursor = 0usize;

        // ── Step 2: merge per-row results into the global flat vectors ───────
        // Per-row tags have offsets relative to the start of that row's chars.
        // We accumulate a running `global_offset` and re-base each tag.
        let mut chars: Vec<TChar> = Vec::new();
        let mut tags: Vec<FormatTag> = Vec::new();
        let mut row_offsets: Vec<usize> = Vec::with_capacity(row_count);

        for (row_idx, entry) in cache.iter().enumerate() {
            // Step 1 populated every entry unconditionally, so `None` cannot
            // occur here.  We use `if let` to satisfy the no-unwrap/expect rule;
            // the `else` branch is unreachable in practice.
            if let Some(row_entry) = entry.as_ref() {
                let global_offset = chars.len();

                // Record the flat index where this row begins.
                row_offsets.push(global_offset);

                // Rebase per-row tags into the global index space, then splice
                // any auto-detected URL ranges on top. Splicing is done in
                // row-local character space first (cheap) and the result is
                // then rebased. Prefer the Step 1.5 group-corrected ranges
                // (whole-URL text, wrap-boundary-safe) when present — `refined_auto_urls`
                // is sorted by `row_idx`, so a single forward cursor finds the
                // match (if any) in amortised O(1).
                while refined_auto_urls
                    .get(refined_cursor)
                    .is_some_and(|(idx, _)| *idx < row_idx)
                {
                    refined_cursor += 1;
                }
                let row_auto_urls = match refined_auto_urls.get(refined_cursor) {
                    Some((idx, ranges)) if *idx == row_idx => ranges.as_slice(),
                    _ => &row_entry.auto_urls,
                };
                let spliced = splice_auto_urls(&row_entry.tags, row_auto_urls);

                // Append this row's characters, adjusting tag offsets.
                for row_tag in &spliced {
                    let rebased = FormatTag {
                        start: global_offset + row_tag.start,
                        end: global_offset + row_tag.end,
                        colors: row_tag.colors,
                        font_weight: row_tag.font_weight,
                        font_decorations: row_tag.font_decorations,
                        url: row_tag.url.clone(),
                        blink: row_tag.blink,
                    };

                    // Merge with the previous tag when format is identical and
                    // the ranges are contiguous (same logic as the original helper).
                    if let Some(last) = tags.last_mut() {
                        if last.end == rebased.start && tags_same_format(last, &rebased) {
                            last.end = rebased.end;
                        } else {
                            tags.push(rebased);
                        }
                    } else {
                        tags.push(rebased);
                    }
                }

                chars.extend_from_slice(&row_entry.chars);
            }

            // Append a NewLine separator after every row except the last.
            let is_last_row = row_idx + 1 == row_count;
            if !is_last_row {
                let byte_pos = chars.len();
                chars.push(TChar::NewLine);
                if let Some(last) = tags.last_mut() {
                    if last.end == byte_pos {
                        last.end += 1;
                    } else {
                        tags.push(FormatTag {
                            start: byte_pos,
                            end: byte_pos + 1,
                            ..FormatTag::default()
                        });
                    }
                } else {
                    tags.push(FormatTag {
                        start: byte_pos,
                        end: byte_pos + 1,
                        ..FormatTag::default()
                    });
                }
            }
        }

        // Guarantee at least one tag covering the full range.
        if tags.is_empty() {
            tags.push(FormatTag {
                start: 0,
                end: if chars.is_empty() {
                    usize::MAX
                } else {
                    chars.len()
                },
                ..FormatTag::default()
            });
        } else if let Some(last) = tags.last_mut() {
            last.end = chars.len();
        }

        let url_tag_indices = Self::collect_url_tag_indices(&tags);
        (chars, tags, row_offsets, url_tag_indices)
    }

    /// Step 1 (+ 1.5) of [`Self::rows_as_tchars_and_tags_cached`]: ensure
    /// every row has an up-to-date [`RowCacheEntry`], and fix up
    /// auto-detected URLs that wrap across rows, in one pass over `rows`.
    ///
    /// [`Self::flatten_row`] detects URLs using only that row's own bytes, so
    /// a URL that DECAWM soft-wraps across two or more physical rows is seen
    /// as several independent, truncated matches. This pass finds contiguous
    /// runs of rows joined by `RowJoin::ContinueLogicalLine` (soft-wrap
    /// continuations of one logical line) and, only when the run actually
    /// contains URL-looking content, re-runs URL detection on the rows'
    /// concatenated bytes via [`redetect_urls_for_group`] so wrap boundaries
    /// stop being treated as the end of the URL (both for truncation and for
    /// the trailing sentence-punctuation heuristic in
    /// `url_detect::trim_trailing`, which otherwise misfires when a wrap
    /// boundary lands right before stripped punctuation).
    ///
    /// The grouping scan is fused into the same loop that rebuilds dirty row
    /// caches — rather than a second full pass over `rows` — so the common
    /// case (nothing wraps, or a wrap has no URL content) costs only a few
    /// extra cheap comparisons per row instead of a whole extra traversal.
    ///
    /// Returns a **sparse** list of `(row_idx, ranges)` pairs, sorted by
    /// ascending `row_idx`, covering only rows whose `auto_urls` were
    /// replaced by a group-level redetection. A row not present means "no
    /// change; use the row's own cached `auto_urls`". It stays empty (no heap
    /// allocation) whenever no row needed correction — the overwhelmingly
    /// common case.
    fn refresh_row_cache_and_refine_wrapped_urls(
        rows: &mut [Row],
        cache: &mut [Option<RowCacheEntry>],
        auto_detect: bool,
    ) -> Vec<(usize, Vec<AutoUrlRange>)> {
        let row_count = rows.len();
        let mut refined_auto_urls: Vec<(usize, Vec<AutoUrlRange>)> = Vec::new();
        let mut group_start = 0usize;
        let mut group_has_url_signal = false;

        for row_idx in 0..row_count {
            // Invalidate cache entries that were built with a different
            // `auto_detect` mode than the one currently in effect. When
            // auto_detect is true we need `bytes` populated; when false the
            // cache entry may still have them but that is harmless — we keep
            // the entry in that case.
            {
                let row = &mut rows[row_idx];
                let needs_rebuild = row.dirty
                    || cache[row_idx].is_none()
                    || (auto_detect
                        && cache[row_idx]
                            .as_ref()
                            .is_some_and(|e| e.bytes.is_empty() && !e.chars.is_empty()));
                if needs_rebuild {
                    cache[row_idx] = Some(Self::flatten_row(row, auto_detect));
                    row.mark_clean();
                }
            }

            if !auto_detect {
                continue;
            }

            // A new logical-line group starts at row 0, or wherever a row is
            // not a soft-wrap continuation of the previous one. Runs of
            // `RowJoin::ContinueLogicalLine` rows are one DECAWM-wrapped
            // logical line; see `redetect_urls_for_group`'s doc comment.
            let starts_new_group =
                row_idx == 0 || rows[row_idx].join != RowJoin::ContinueLogicalLine;
            if starts_new_group {
                if row_idx - group_start > 1 && group_has_url_signal {
                    redetect_urls_for_group(cache, group_start, row_idx, &mut refined_auto_urls);
                }
                group_start = row_idx;
                group_has_url_signal = false;
            }
            // Only a match whose raw (pre-trim) end reached this row's raw
            // byte end is a candidate continuation — a URL that ends
            // naturally mid-row (the common case: followed by whitespace or
            // more prose) can never be split by a wrap, no matter how many
            // other rows in this soft-wrapped run happen to also contain
            // unrelated, fully self-contained URLs. Only the last match in a
            // row can possibly reach the row's end (matches are found in
            // increasing order), so checking `.last()` suffices.
            if cache[row_idx]
                .as_ref()
                .is_some_and(|e| e.auto_urls.last().is_some_and(|r| r.touches_row_end))
            {
                group_has_url_signal = true;
            }
        }
        // Finalize the last group (the loop above only finalizes a group
        // once it sees the *next* group start).
        if auto_detect && row_count - group_start > 1 && group_has_url_signal {
            redetect_urls_for_group(cache, group_start, row_count, &mut refined_auto_urls);
        }

        refined_auto_urls
    }

    /// Collect the indices of tags in `tags` that carry a URL.
    ///
    /// This is a cheap post-pass over the already-built tag vector — typically
    /// O(tags) where tags is small.  The result enables the GUI to iterate
    /// only URL-bearing tags instead of scanning all tags during hover
    /// detection.
    fn collect_url_tag_indices(tags: &[FormatTag]) -> Vec<usize> {
        tags.iter()
            .enumerate()
            .filter_map(|(i, tag)| tag.url.as_ref().map(|_| i))
            .collect()
    }

    /// Flatten a single [`Row`] into a [`RowCacheEntry`].
    ///
    /// Tag offsets are **row-relative** (start at 0 for the first character in
    /// this row).  The caller is responsible for re-basing them into global
    /// offsets when merging multiple rows.
    ///
    /// When `auto_detect` is `true`, also builds the UTF-8 byte buffer and
    /// the byte→char map in the same cell loop, and runs
    /// [`url_detect::find_urls_bytes`] to populate `auto_urls`.
    fn flatten_row(row: &Row, auto_detect: bool) -> RowCacheEntry {
        let mut chars: Vec<TChar> = Vec::new();
        let mut tags: Vec<FormatTag> = Vec::new();
        let mut bytes: Vec<u8> = Vec::new();
        let mut byte_to_char: Vec<u32> = Vec::new();

        for cell in row.characters() {
            // Skip wide-glyph continuation cells.
            if cell.is_continuation() {
                continue;
            }

            let char_idx = chars.len();
            let tc = *cell.tchar();
            chars.push(tc);

            // Build the byte mirror in the same pass when auto-detect is on.
            if auto_detect {
                let tc_bytes = tc.as_bytes();
                // `char_idx` fits in u32 for any reasonable row width; saturate
                // defensively. Rows never exceed a few thousand cells.
                let char_idx_u32 = u32::try_from(char_idx).unwrap_or(u32::MAX);
                for &b in tc_bytes {
                    bytes.push(b);
                    byte_to_char.push(char_idx_u32);
                }
            }

            let cell_tag = cell.tag();
            if let Some(last) = tags.last_mut() {
                if last.end == char_idx && tags_same_format(last, cell_tag) {
                    last.end += 1;
                } else {
                    tags.push(FormatTag {
                        start: char_idx,
                        end: char_idx + 1,
                        colors: cell_tag.colors,
                        font_weight: cell_tag.font_weight,
                        font_decorations: cell_tag.font_decorations,
                        url: cell_tag.url.clone(),
                        blink: cell_tag.blink,
                    });
                }
            } else {
                tags.push(FormatTag {
                    start: char_idx,
                    end: char_idx + 1,
                    colors: cell_tag.colors,
                    font_weight: cell_tag.font_weight,
                    font_decorations: cell_tag.font_decorations,
                    url: cell_tag.url.clone(),
                    blink: cell_tag.blink,
                });
            }
        }

        // Guarantee at least one tag even for an empty row.
        if tags.is_empty() {
            tags.push(FormatTag {
                start: 0,
                end: 0,
                ..FormatTag::default()
            });
        }

        // Run URL detection on the byte buffer, translate byte offsets into
        // character indices via `byte_to_char`.
        let auto_urls = if auto_detect && !bytes.is_empty() {
            build_auto_urls(&bytes, &byte_to_char)
        } else {
            Vec::new()
        };

        RowCacheEntry {
            chars,
            tags,
            bytes,
            byte_to_char,
            auto_urls,
        }
    }

    /// Return `true` when the alternate screen is currently active.
    #[must_use]
    pub const fn is_alternate_screen(&self) -> bool {
        matches!(self.kind, BufferType::Alternate)
    }

    /// Return `true` when a cursor has been saved via DECSC (ESC 7 / `\x1b[?1048h`).
    #[must_use]
    pub const fn has_saved_cursor(&self) -> bool {
        self.saved_cursor.is_some()
    }

    /// Return the terminal width (columns).
    #[must_use]
    pub const fn terminal_width(&self) -> usize {
        self.width
    }

    /// Return the terminal height (rows).
    #[must_use]
    pub const fn terminal_height(&self) -> usize {
        self.height
    }

    /// Extract the text content of a selection range from the buffer.
    ///
    /// Coordinates are buffer-absolute row indices (0 = first row in the full
    /// buffer including scrollback). Columns are 0-indexed cell positions.
    /// The range is inclusive on both ends: `[start_row, start_col]` through
    /// `[end_row, end_col]`.
    ///
    /// Trailing whitespace on each row is trimmed (standard terminal behaviour).
    /// Rows are separated by `'\n'`.
    #[must_use]
    pub fn extract_text(
        &self,
        start_row: usize,
        start_col: usize,
        end_row: usize,
        end_col: usize,
    ) -> String {
        use std::fmt::Write as _;

        if start_row >= self.rows.len() {
            return String::new();
        }
        let end_row = end_row.min(self.rows.len().saturating_sub(1));

        let mut result = String::new();

        for row_idx in start_row..=end_row {
            // Task 119.4: `extract_text` takes `&self`, so it cannot call
            // `ensure_decompressed` (which needs `&mut self`). A row
            // evicted to a compressed block is resolved via a transient,
            // non-mutating peek instead — see `row_cells_for_read`.
            let cells = self.row_cells_for_read(row_idx);

            let col_begin = if row_idx == start_row { start_col } else { 0 };
            let col_end = if row_idx == end_row {
                end_col
            } else {
                cells.len().saturating_sub(1)
            };

            let mut row_text = String::new();
            for col in col_begin..=col_end {
                if col >= cells.len() {
                    break;
                }
                let cell = &cells[col];
                if cell.is_continuation() {
                    continue;
                }
                let tc = cell.tchar();
                if matches!(tc, TChar::NewLine) {
                    break;
                }
                write!(&mut row_text, "{tc}").unwrap_or_default();
            }

            let trimmed = row_text.trim_end();
            result.push_str(trimmed);

            if row_idx < end_row {
                result.push('\n');
            }
        }

        result
    }

    /// Extract a rectangular block of text from the buffer.
    ///
    /// Every row from `start_row` to `end_row` (inclusive) is sampled between
    /// the same `col_min`..=`col_max` column range, where
    /// `col_min = start_col.min(end_col)` and `col_max = start_col.max(end_col)`.
    /// Rows are joined with `\n`.  Trailing whitespace is trimmed per row.
    ///
    /// This is the copy behaviour for Alt+drag (block/rectangular) selections.
    #[must_use]
    pub fn extract_block_text(
        &self,
        start_row: usize,
        start_col: usize,
        end_row: usize,
        end_col: usize,
    ) -> String {
        use std::fmt::Write as _;

        if start_row >= self.rows.len() {
            return String::new();
        }
        let end_row = end_row.min(self.rows.len().saturating_sub(1));
        let col_min = start_col.min(end_col);
        let col_max = start_col.max(end_col);

        let mut result = String::new();

        for row_idx in start_row..=end_row {
            // Task 119.4: see the matching comment in `extract_text` — this
            // also takes `&self` and must resolve an evicted row without
            // mutating `Buffer` state.
            let cells = self.row_cells_for_read(row_idx);

            let mut row_text = String::new();
            for col in col_min..=col_max {
                if col >= cells.len() {
                    break;
                }
                let cell = &cells[col];
                if cell.is_continuation() {
                    continue;
                }
                let tc = cell.tchar();
                if matches!(tc, TChar::NewLine) {
                    break;
                }
                write!(&mut row_text, "{tc}").unwrap_or_default();
            }

            let trimmed = row_text.trim_end();
            result.push_str(trimmed);

            if row_idx < end_row {
                result.push('\n');
            }
        }

        result
    }
}

/// Convert a byte range into a character range using a `byte_to_char` map.
///
/// `byte_to_char[i]` is the character index for the character that starts at
/// byte `i` of the buffer `byte_to_char` was built for (`bytes.len()` bytes
/// long). Returns `None` when the map is malformed (should never happen, but
/// this is production code so we cannot unwrap) or when the resulting range
/// is empty.
fn byte_range_to_char_range(
    byte_start: usize,
    byte_end: usize,
    bytes_len: usize,
    byte_to_char: &[u32],
) -> Option<(usize, usize)> {
    let &start_u32 = byte_to_char.get(byte_start)?;
    // `byte_end` is exclusive; we want the character index *after* the last
    // included character. If `byte_end` reaches the end of the buffer, use
    // one past the last character index (inferred from the last byte's char
    // index).
    let end_char_u32 = if byte_end >= bytes_len {
        byte_to_char
            .last()
            .copied()
            .map_or(0, |c| c.saturating_add(1))
    } else {
        *byte_to_char.get(byte_end)?
    };

    let char_start = usize::try_from(start_u32).ok()?;
    let char_end = usize::try_from(end_char_u32).ok()?;

    if char_end <= char_start {
        return None;
    }

    Some((char_start, char_end))
}

/// Convert the detected URL byte ranges into row-local character ranges.
///
/// `bytes` is the row's UTF-8 byte buffer; `byte_to_char` maps each byte
/// offset to the starting character index of the character at that byte
/// position. The returned [`AutoUrlRange`]s carry character indices and the
/// URL string as an `Arc<Url>` ready to splice into `FormatTag`s.
fn build_auto_urls(bytes: &[u8], byte_to_char: &[u32]) -> Vec<AutoUrlRange> {
    let matches = url_detect::find_urls_bytes(bytes);
    let mut out = Vec::with_capacity(matches.len());

    for m in matches {
        let Some((char_start, char_end)) =
            byte_range_to_char_range(m.byte_start, m.byte_end, bytes.len(), byte_to_char)
        else {
            continue;
        };

        // Build the URL string from the matched byte range. This is the only
        // per-match string allocation in the pipeline; it is amortised by the
        // row-level cache.
        let url_str = match std::str::from_utf8(&bytes[m.byte_start..m.byte_end]) {
            Ok(s) => s.to_string(),
            Err(_) => continue,
        };

        out.push(AutoUrlRange {
            char_start,
            char_end,
            url: Arc::new(Url {
                id: None,
                url: url_str,
            }),
            touches_row_end: m.touches_buffer_end,
        });
    }

    out
}

/// Re-detect auto URLs across a run of rows joined by
/// `RowJoin::ContinueLogicalLine` (`rows[group_start..group_end)`).
///
/// Concatenates the already-cached per-row byte buffers into one buffer and
/// runs [`url_detect::find_urls_bytes`] once on the result, so that a URL
/// wrap boundary is no longer mistaken for the URL's real end — this both
/// stops truncation and avoids `trim_trailing` misfiring on a wrap boundary
/// that happens to land right before stripped punctuation. Each match's byte
/// range is then mapped back across the contributing rows (via each row's own
/// `byte_to_char` map) into per-row [`AutoUrlRange`]s that all share one
/// `Arc<Url>` holding the full, untruncated URL text.
///
/// Appends an entry for every row in the group (an empty `Vec` when no match
/// touches that row), replacing whatever the row's own single-row detection
/// found. Caller is expected to have already checked that the group actually
/// contains URL-looking content before calling this (this function does not
/// check `auto_detect` or the join flags itself — it operates purely on the
/// cache), and to call groups in ascending `group_start` order so `refined`
/// stays sorted by `row_idx`.
fn redetect_urls_for_group(
    cache: &[Option<RowCacheEntry>],
    group_start: usize,
    group_end: usize,
    refined: &mut Vec<(usize, Vec<AutoUrlRange>)>,
) {
    let mut group_bytes: Vec<u8> = Vec::new();
    let mut row_byte_start: Vec<usize> = Vec::with_capacity(group_end - group_start);
    for entry in &cache[group_start..group_end] {
        row_byte_start.push(group_bytes.len());
        if let Some(entry) = entry.as_ref() {
            group_bytes.extend_from_slice(&entry.bytes);
        }
    }
    let group_total_len = group_bytes.len();

    let matches = url_detect::find_urls_bytes(&group_bytes);

    // Every row in the group gets an authoritative (possibly empty) refined
    // entry, superseding its own single-row `auto_urls` — the group
    // redetection reproduces equivalent matches for URLs fully contained in
    // one row too, so nothing is lost by replacing wholesale.
    let first_new_idx = refined.len();
    refined.extend((group_start..group_end).map(|row_idx| (row_idx, Vec::new())));

    if matches.is_empty() {
        return;
    }

    for m in matches {
        let Ok(url_str) = std::str::from_utf8(&group_bytes[m.byte_start..m.byte_end]) else {
            continue;
        };
        let shared_url = Arc::new(Url {
            id: None,
            url: url_str.to_string(),
        });

        for offset_idx in 0..(group_end - group_start) {
            let row_idx = group_start + offset_idx;
            let row_byte_lo = row_byte_start[offset_idx];
            let row_byte_hi = row_byte_start
                .get(offset_idx + 1)
                .copied()
                .unwrap_or(group_total_len);

            // No overlap between the match and this row's byte span.
            if m.byte_end <= row_byte_lo || m.byte_start >= row_byte_hi {
                continue;
            }

            let Some(entry) = cache[row_idx].as_ref() else {
                continue;
            };
            if entry.bytes.is_empty() {
                continue;
            }

            let local_start = m.byte_start.max(row_byte_lo) - row_byte_lo;
            let local_end = m.byte_end.min(row_byte_hi) - row_byte_lo;

            let Some((char_start, char_end)) = byte_range_to_char_range(
                local_start,
                local_end,
                entry.bytes.len(),
                &entry.byte_to_char,
            ) else {
                continue;
            };

            let (_, row_ranges) = &mut refined[first_new_idx + offset_idx];
            row_ranges.push(AutoUrlRange {
                char_start,
                char_end,
                url: shared_url.clone(),
                touches_row_end: local_end >= entry.bytes.len(),
            });
        }
    }
}

/// Splice auto-detected URL ranges into a row's per-row tag vec.
///
/// For each [`AutoUrlRange`], covering tags (those whose
/// `[start, end)` overlaps `[char_start, char_end)`) are split into up to
/// three pieces: pre-range (unchanged), overlapping (inheriting the base
/// tag's visual attributes but with `url = Some(range.url)`), and post-range
/// (unchanged).
///
/// **OSC 8 precedence**: when a covering tag already has `url.is_some()`,
/// the auto-URL is suppressed within that tag — the OSC 8 link wins. This
/// check happens per-tag, so a range that starts inside an OSC 8 link and
/// extends past it is still partially spliced into the non-OSC 8 segments.
///
/// The returned vec is sorted by `start` and has no overlapping tags, the
/// same invariants the merge step downstream expects.
fn splice_auto_urls(tags: &[FormatTag], ranges: &[AutoUrlRange]) -> Vec<FormatTag> {
    if ranges.is_empty() {
        return tags.to_vec();
    }

    // Accumulator for output tags. We splice one range at a time against the
    // current accumulator, which keeps invariants simple.
    let mut current: Vec<FormatTag> = tags.to_vec();

    for range in ranges {
        let mut next: Vec<FormatTag> = Vec::with_capacity(current.len() + 2);
        for tag in &current {
            // No overlap → keep as-is.
            if tag.end <= range.char_start || tag.start >= range.char_end {
                next.push(tag.clone());
                continue;
            }

            // OSC 8 precedence: tag already has a URL → keep entire tag
            // unchanged within the range's span.
            if tag.url.is_some() {
                next.push(tag.clone());
                continue;
            }

            // Compute split points, clamped into `tag`'s own bounds.
            let mid_start = range.char_start.max(tag.start);
            let mid_end = range.char_end.min(tag.end);

            // Pre-overlap segment (if any).
            if tag.start < mid_start {
                next.push(FormatTag {
                    start: tag.start,
                    end: mid_start,
                    colors: tag.colors,
                    font_weight: tag.font_weight,
                    font_decorations: tag.font_decorations,
                    url: tag.url.clone(),
                    blink: tag.blink,
                });
            }

            // Overlap segment with the auto-URL attached.
            next.push(FormatTag {
                start: mid_start,
                end: mid_end,
                colors: tag.colors,
                font_weight: tag.font_weight,
                font_decorations: tag.font_decorations,
                url: Some(range.url.clone()),
                blink: tag.blink,
            });

            // Post-overlap segment (if any).
            if mid_end < tag.end {
                next.push(FormatTag {
                    start: mid_end,
                    end: tag.end,
                    colors: tag.colors,
                    font_weight: tag.font_weight,
                    font_decorations: tag.font_decorations,
                    url: tag.url.clone(),
                    blink: tag.blink,
                });
            }
        }
        current = next;
    }

    current
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod extended_window_tests {
    use crate::buffer::Buffer;
    use freminal_common::buffer_states::tchar::TChar;

    fn t(s: &str) -> Vec<TChar> {
        s.chars().map(TChar::from).collect()
    }

    /// Build a 4-wide, 3-tall buffer containing 6 rows of distinct content
    /// ("r0".."r5"), so rows 0..=2 are scrollback and rows 3..=5 are the live
    /// visible window.
    fn buffer_with_scrollback() -> Buffer {
        let mut buf = Buffer::new(4, 3);
        for i in 0..6 {
            buf.insert_text(&t(&format!("r{i}")));
            if i < 5 {
                buf.handle_lf();
                buf.handle_cr();
            }
        }
        assert_eq!(buf.rows().len(), 6, "expected 6 total rows");
        buf
    }

    #[test]
    fn bounds_no_extra_is_normal_window() {
        let buf = buffer_with_scrollback();
        // height = 3, total = 6, scroll 0 → normal window [3, 6).
        let (start, end) = buf.visible_window_bounds(0, 0);
        assert_eq!((start, end), (3, 6));
    }

    #[test]
    fn bounds_extra_extends_window_upward() {
        let buf = buffer_with_scrollback();
        // Pull in 2 extra rows above the window: [1, 6).
        let (start, end) = buf.visible_window_bounds(0, 2);
        assert_eq!((start, end), (1, 6));
    }

    #[test]
    fn bounds_extra_clamped_at_row_zero() {
        let buf = buffer_with_scrollback();
        // Request more extra rows than exist above the window: clamps to 0.
        let (start, end) = buf.visible_window_bounds(0, 99);
        assert_eq!((start, end), (0, 6));
    }

    #[test]
    fn bounds_extra_with_scroll_offset() {
        let buf = buffer_with_scrollback();
        // Scrolled back 1 row → normal window [2, 5); plus 1 extra → [1, 5).
        let (start, end) = buf.visible_window_bounds(1, 1);
        assert_eq!((start, end), (1, 5));
    }

    #[test]
    fn extended_flatten_has_more_rows() {
        let mut buf = buffer_with_scrollback();
        let (_c0, _t0, ro0, _u0) = buf.visible_as_tchars_and_tags_extended(0, 0);
        let (_c2, _t2, ro2, _u2) = buf.visible_as_tchars_and_tags_extended(0, 2);
        assert_eq!(ro0.len(), 3, "normal window has term_height rows");
        assert_eq!(ro2.len(), 5, "extended window has term_height + extra rows");
    }

    #[test]
    fn extended_flatten_starts_earlier() {
        let mut buf = buffer_with_scrollback();
        // The extended window's first row should be buffer row 1 ("r1").
        let (chars, _tags, row_offsets, _url) = buf.visible_as_tchars_and_tags_extended(0, 2);
        // First row's first char is 'r'.
        assert_eq!(chars[row_offsets[0]], TChar::from('r'));
        // Second char of first row is '1' (buffer row 1).
        assert_eq!(chars[row_offsets[0] + 1], TChar::from('1'));
    }

    #[test]
    fn extended_line_widths_match_extended_window() {
        let buf = buffer_with_scrollback();
        assert_eq!(buf.visible_line_widths_extended(0, 0).len(), 3);
        assert_eq!(buf.visible_line_widths_extended(0, 2).len(), 5);
    }

    #[test]
    fn extended_dirty_check_covers_extra_rows() {
        let mut buf = buffer_with_scrollback();
        // Flatten the extended window to clear dirty flags across all 5 rows.
        let _ = buf.visible_as_tchars_and_tags_extended(0, 2);
        assert!(
            !buf.any_visible_dirty_extended(0, 2),
            "freshly flattened extended window must be clean"
        );
    }
}

/// Task 118.4: cold scrollback rows must not retain the second (row-cache)
/// and third (decompaction-memo) copies of their cell data after a
/// full-scrollback flatten, while output stays byte-identical across
/// repeated flattens and visible-row caches stay warm.
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod scrollback_eviction_tests {
    use crate::buffer::Buffer;
    use crate::image_store::{ImagePlacement, ImageProtocol};
    use crate::row::Row;
    use freminal_common::buffer_states::{format_tag::FormatTag, tchar::TChar};

    fn text(s: &str) -> Vec<TChar> {
        s.chars().map(TChar::from).collect()
    }

    /// Push `n` numbered lines (LF+CR terminated, matching real PTY output)
    /// into `buf`. Mirrors the helper in
    /// `crate::buffer::scrollback_compaction_tests`. This is a hot-path
    /// fill only: compaction is deferred (Task 118 follow-up), so pushing
    /// lines past the visible window does NOT compact anything on its own
    /// — callers that need compacted scrollback must call
    /// `Buffer::compact_idle_scrollback` explicitly afterward.
    fn push_numbered_lines(buf: &mut Buffer, n: usize) {
        for i in 0..n {
            buf.insert_text(&text(&format!("line{i:04}content")));
            buf.handle_lf();
            buf.handle_cr();
        }
    }

    fn buffer_with_compacted_scrollback() -> Buffer {
        let mut buf = Buffer::new(20, 3).with_scrollback_limit(50);
        push_numbered_lines(&mut buf, 20);
        let _ = buf.compact_idle_scrollback(usize::MAX);
        let visible_start = buf.visible_window_start(0);
        assert!(visible_start > 0, "test needs scrollback rows to exist");
        assert!(
            buf.rows[..visible_start].iter().any(Row::is_compact),
            "expected scrollback compaction to have engaged"
        );
        buf
    }

    #[test]
    fn two_consecutive_scrollback_flattens_are_byte_identical() {
        let mut buf = buffer_with_compacted_scrollback();

        // First flatten: populates row_cache + decompaction memos, then
        // (per Task 118.4) evicts both for every compact row.
        let (chars1, tags1, offsets1, urls1) = buf.scrollback_as_tchars_and_tags(0);

        // Second flatten: must rebuild from the still-resident `CompactRow`
        // data and produce byte-identical output.
        let (chars2, tags2, offsets2, urls2) = buf.scrollback_as_tchars_and_tags(0);

        assert_eq!(chars1, chars2, "flattened characters must be identical");
        assert_eq!(tags1, tags2, "flattened format tags must be identical");
        assert_eq!(offsets1, offsets2, "row offsets must be identical");
        assert_eq!(urls1, urls2, "url tag indices must be identical");
    }

    #[test]
    fn scrollback_flatten_evicts_compact_row_cache_and_memo() {
        let mut buf = buffer_with_compacted_scrollback();
        let visible_start = buf.visible_window_start(0);

        let _ = buf.scrollback_as_tchars_and_tags(0);

        for (row, entry) in buf.rows[..visible_start]
            .iter()
            .zip(buf.row_cache[..visible_start].iter())
        {
            if row.is_compact() {
                assert!(
                    entry.is_none(),
                    "a compact scrollback row's RowCacheEntry must be evicted after a scrollback flatten"
                );
            }
        }

        // Rows must remain compact — eviction drops the cache/memo, not the
        // compact representation itself.
        assert!(
            buf.rows[..visible_start].iter().any(Row::is_compact),
            "rows must remain compact after cache eviction"
        );

        buf.debug_assert_invariants();
    }

    #[test]
    fn scrollback_flatten_does_not_evict_visible_row_cache() {
        let mut buf = buffer_with_compacted_scrollback();
        let visible_start = buf.visible_window_start(0);

        // Warm the visible row cache first.
        let _ = buf.visible_as_tchars_and_tags(0);
        assert!(
            buf.row_cache[visible_start..].iter().all(Option::is_some),
            "sanity: visible row cache should be populated before the scrollback flatten"
        );

        let _ = buf.scrollback_as_tchars_and_tags(0);

        assert!(
            buf.row_cache[visible_start..].iter().all(Option::is_some),
            "a scrollback flatten must not evict the visible window's row cache"
        );
    }

    #[test]
    fn url_in_compacted_scrollback_row_detected_after_eviction_rebuild() {
        let mut buf = Buffer::new(40, 3).with_scrollback_limit(50);
        assert!(buf.auto_detect_urls(), "test relies on default auto-detect");

        buf.insert_text(&text("see http://example.com for info"));
        buf.handle_lf();
        buf.handle_cr();
        push_numbered_lines(&mut buf, 20);
        let _ = buf.compact_idle_scrollback(usize::MAX);

        let visible_start = buf.visible_window_start(0);
        assert!(
            buf.rows[..visible_start].iter().any(Row::is_compact),
            "expected scrollback compaction to have engaged"
        );

        // First flatten: builds the RowCacheEntry (running URL detection),
        // then evicts it (and the decompaction memo) per Task 118.4.
        let (_chars1, tags1, _offsets1, url_indices1) = buf.scrollback_as_tchars_and_tags(0);
        assert!(
            !url_indices1.is_empty(),
            "URL must be auto-detected on the first (populating) flatten"
        );

        // Second flatten: RowCacheEntry is `None` (evicted), so the row is
        // rebuilt via `flatten_row`, re-running URL detection from scratch.
        let (_chars2, tags2, _offsets2, url_indices2) = buf.scrollback_as_tchars_and_tags(0);
        assert!(
            !url_indices2.is_empty(),
            "URL must still be auto-detected on the second flatten after eviction"
        );
        assert_eq!(
            tags1, tags2,
            "re-detected URL tags must match the original detection exactly"
        );
    }

    #[test]
    fn image_opt_out_row_is_not_evicted_by_scrollback_flatten() {
        let mut buf = Buffer::new(10, 3).with_scrollback_limit(50);

        // Stamp an image cell into row 0 before it scrolls into history —
        // image rows opt out of compaction (`CompactRow::from_row`), so this
        // row must stay `Live` all the way through.
        let placement = ImagePlacement {
            image_id: 1,
            col_in_image: 0,
            row_in_image: 0,
            protocol: ImageProtocol::Sixel,
            image_number: None,
            placement_id: None,
            z_index: 0,
            source_crop: None,
            placement_instance: 1,
            subcell_offset: None,
        };
        buf.set_image_cell_at(0, 0, placement, FormatTag::default());
        buf.handle_lf();
        buf.handle_cr();
        push_numbered_lines(&mut buf, 20);

        let visible_start = buf.visible_window_start(0);
        assert!(visible_start > 0, "test needs the image row in scrollback");
        assert!(
            !buf.rows[0].is_compact(),
            "an image row must never be compacted, even in scrollback"
        );

        let _ = buf.scrollback_as_tchars_and_tags(0);

        // Eviction only targets `is_compact()` rows; a Live (opt-out) row's
        // cache is left untouched.
        assert!(
            buf.row_cache[0].is_some(),
            "a non-compact (image) scrollback row's cache must not be evicted"
        );
        assert!(!buf.rows[0].is_compact());
        assert!(
            buf.rows[0].cells().iter().any(crate::cell::Cell::has_image),
            "image cell data must survive the scrollback flatten"
        );

        // A second flatten must still work without panicking and keep
        // reporting the image row intact.
        let _ = buf.scrollback_as_tchars_and_tags(0);
        assert!(buf.rows[0].cells().iter().any(crate::cell::Cell::has_image));
    }
}

/// Regression coverage for GitHub issue #418: a plain-text URL that
/// DECAWM-wraps across two or more physical rows must be auto-detected as
/// one full, untruncated URL — not as several independent per-row fragments.
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod wrapped_url_tests {
    use crate::buffer::Buffer;
    use crate::row::RowJoin;
    use freminal_common::buffer_states::tchar::TChar;

    fn text(s: &str) -> Vec<TChar> {
        s.chars().map(TChar::from).collect()
    }

    /// Collect the distinct URL strings carried by the tags at `url_indices`.
    fn url_strings(
        tags: &[freminal_common::buffer_states::format_tag::FormatTag],
        url_indices: &[usize],
    ) -> Vec<String> {
        url_indices
            .iter()
            .filter_map(|&i| tags[i].url.as_ref().map(|u| u.url.clone()))
            .collect()
    }

    #[test]
    fn url_wrapping_across_two_rows_is_detected_in_full() {
        // Width chosen so the URL itself (not just surrounding prose) spans
        // the row boundary: "see " (4) + first part of the URL fills row 0,
        // the remainder wraps onto row 1.
        let mut buf = Buffer::new(20, 3);
        assert!(buf.auto_detect_urls());
        let url = "https://example.com/a/very/long/path";
        buf.insert_text(&text(&format!("see {url} end")));

        assert_eq!(
            buf.rows()[1].join,
            RowJoin::ContinueLogicalLine,
            "test setup: row 1 must be a soft-wrap continuation of row 0"
        );

        let (_chars, tags, _row_offsets, url_indices) = buf.visible_as_tchars_and_tags(0);
        assert!(!url_indices.is_empty(), "URL must be auto-detected");

        let urls = url_strings(&tags, &url_indices);
        assert!(
            urls.iter().all(|u| u == url),
            "every URL-tagged fragment must carry the full, untruncated URL; got {urls:?}"
        );
    }

    #[test]
    fn url_wrapping_across_three_rows_is_detected_in_full() {
        // A long URL that wraps twice (spans three physical rows), to
        // exercise the multi-row chaining (not just a single adjacent pair).
        let mut buf = Buffer::new(15, 5);
        assert!(buf.auto_detect_urls());
        let url = "https://example.com/a/very/long/path/that/keeps/going/and/going";
        buf.insert_text(&text(url));

        assert_eq!(buf.rows()[1].join, RowJoin::ContinueLogicalLine);
        assert_eq!(buf.rows()[2].join, RowJoin::ContinueLogicalLine);

        let (_chars, tags, _row_offsets, url_indices) = buf.visible_as_tchars_and_tags(0);
        assert!(!url_indices.is_empty(), "URL must be auto-detected");

        let urls = url_strings(&tags, &url_indices);
        assert!(
            urls.iter().all(|u| u == url),
            "every URL-tagged fragment must carry the full, untruncated URL; got {urls:?}"
        );
    }

    #[test]
    fn wrap_boundary_on_trailing_punctuation_char_is_not_stripped() {
        // Width chosen so the wrap boundary lands exactly on the '.' in
        // "index.html" — a single-row detector's `trim_trailing` heuristic
        // would misidentify that '.' as sentence punctuation and strip it,
        // even though it is a real path separator continued on the next row.
        let mut buf = Buffer::new(26, 3);
        assert!(buf.auto_detect_urls());
        let url = "https://example.com/index.html";
        buf.insert_text(&text(url));

        assert_eq!(
            buf.rows()[0].cells().len(),
            26,
            "row 0 must be exactly full width"
        );
        assert_eq!(buf.rows()[1].join, RowJoin::ContinueLogicalLine);

        let (_chars, tags, _row_offsets, url_indices) = buf.visible_as_tchars_and_tags(0);
        assert!(!url_indices.is_empty(), "URL must be auto-detected");

        let urls = url_strings(&tags, &url_indices);
        assert!(
            urls.iter().all(|u| u == url),
            "the '.' before the wrap must be preserved, not stripped; got {urls:?}"
        );
    }

    #[test]
    fn hard_break_after_full_width_url_does_not_merge_with_next_line() {
        // Row 0 is filled exactly by a URL with no trailing content (so its
        // per-row match already reaches the row's raw end), but row 1 is a
        // genuine new logical line (hard break, not a soft wrap) containing
        // an unrelated URL starting at column 0. These must NOT be merged
        // into one URL.
        let url_a = "https://a.example.com/xxxxx"; // 28 chars
        let mut buf = Buffer::new(28, 3);
        assert!(buf.auto_detect_urls());
        buf.insert_text(&text(url_a));
        buf.handle_lf();
        buf.handle_cr();
        buf.insert_text(&text("https://b.example.com"));

        assert_eq!(
            buf.rows()[1].join,
            RowJoin::NewLogicalLine,
            "test setup: row 1 must be a hard break, not a soft wrap"
        );

        let (_chars, tags, _row_offsets, url_indices) = buf.visible_as_tchars_and_tags(0);
        let urls = url_strings(&tags, &url_indices);
        assert!(urls.contains(&url_a.to_string()), "got {urls:?}");
        assert!(
            urls.contains(&"https://b.example.com".to_string()),
            "got {urls:?}"
        );
        assert!(
            urls.iter()
                .all(|u| u != &format!("{url_a}https://b.example.com")),
            "unrelated URLs on a hard-broken next line must not be merged; got {urls:?}"
        );
    }

    #[test]
    fn wrapped_line_without_any_url_is_unaffected() {
        // A long wrapped line with no URL content at all must not produce
        // any URL tags (and must not panic in the group-redetect pre-check
        // skip path).
        let mut buf = Buffer::new(10, 5);
        assert!(buf.auto_detect_urls());
        buf.insert_text(&text(
            "the quick brown fox jumps over the lazy dog again and again",
        ));
        assert_eq!(buf.rows()[1].join, RowJoin::ContinueLogicalLine);

        let (_chars, _tags, _row_offsets, url_indices) = buf.visible_as_tchars_and_tags(0);
        assert!(
            url_indices.is_empty(),
            "a wrapped line with no URL content must not produce URL tags"
        );
    }
}
