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

use crate::row::Row;
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
        let visible_start = self.visible_window_start(scroll_offset);
        let visible_end = (visible_start + self.height).min(self.rows.len());
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

        let auto_detect = self.auto_detect_urls;
        Self::rows_as_tchars_and_tags_cached(
            &mut self.rows[..visible_start],
            &mut self.row_cache[..visible_start],
            auto_detect,
        )
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
        // ── Step 1: ensure every row has an up-to-date cache entry ──────────
        for (row, entry) in rows.iter_mut().zip(cache.iter_mut()) {
            // Invalidate cache entries that were built with a different
            // `auto_detect` mode than the one currently in effect. When
            // auto_detect is true we need `bytes` populated; when false the
            // cache entry may still have them but that is harmless — we keep
            // the entry in that case.
            let needs_rebuild = row.dirty
                || entry.is_none()
                || (auto_detect
                    && entry
                        .as_ref()
                        .is_some_and(|e| e.bytes.is_empty() && !e.chars.is_empty()));
            if needs_rebuild {
                *entry = Some(Self::flatten_row(row, auto_detect));
                row.mark_clean();
            }
        }

        // ── Step 2: merge per-row results into the global flat vectors ───────
        // Per-row tags have offsets relative to the start of that row's chars.
        // We accumulate a running `global_offset` and re-base each tag.
        let row_count = rows.len();
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
                // then rebased.
                let spliced = splice_auto_urls(&row_entry.tags, &row_entry.auto_urls);

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
            let row = &self.rows[row_idx];
            let cells = row.characters();

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
            let row = &self.rows[row_idx];
            let cells = row.characters();

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
        // Guard against malformed `byte_to_char` (should never happen but we
        // cannot unwrap in production code).
        let Some(&start_u32) = byte_to_char.get(m.byte_start) else {
            continue;
        };
        // `byte_end` is exclusive; we want the character index *after* the
        // last included character. If `byte_end == bytes.len()`, use
        // `chars.len()` (the maximum character index + 1 = byte_to_char's
        // highest value + 1, inferred from the last byte's char index).
        let end_char_u32 = if m.byte_end >= bytes.len() {
            // Last character spans to end of byte buffer.
            byte_to_char
                .last()
                .copied()
                .map_or(0, |c| c.saturating_add(1))
        } else if let Some(&next) = byte_to_char.get(m.byte_end) {
            next
        } else {
            continue;
        };

        let Ok(char_start) = usize::try_from(start_u32) else {
            continue;
        };
        let Ok(char_end) = usize::try_from(end_char_u32) else {
            continue;
        };

        if char_end <= char_start {
            continue;
        }

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
        });
    }

    out
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
