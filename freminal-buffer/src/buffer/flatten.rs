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

use freminal_common::buffer_states::{
    buffer_type::BufferType, format_tag::FormatTag, tchar::TChar,
};

use crate::row::Row;

use super::tags_same_format;
use crate::buffer::Buffer;

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
        Self::rows_as_tchars_and_tags_cached(
            &mut self.rows[visible_start..visible_end],
            &mut self.row_cache[visible_start..visible_end],
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

        Self::rows_as_tchars_and_tags_cached(
            &mut self.rows[..visible_start],
            &mut self.row_cache[..visible_start],
        )
    }

    /// Shared helper: flatten a slice of [`Row`]s into `(Vec<TChar>,
    /// Vec<FormatTag>, Vec<usize>)`, using a per-row cache to skip rows that
    /// have not changed since the last snapshot.
    ///
    /// For each row:
    /// - If `row.dirty` or the cache entry is `None`, flatten the row, populate
    ///   the cache entry, and call `row.mark_clean()`.
    /// - Otherwise reuse the cached per-row `(chars, tags)` directly.
    ///
    /// Per-row tag offsets are stored relative to each row's own character
    /// slice (starting at 0).  The merge step below re-computes global offsets
    /// each time, so the cache never stores stale absolute positions.
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
        cache: &mut [Option<(Vec<TChar>, Vec<FormatTag>)>],
    ) -> (Vec<TChar>, Vec<FormatTag>, Vec<usize>, Vec<usize>) {
        // ── Step 1: ensure every row has an up-to-date cache entry ──────────
        for (row, entry) in rows.iter_mut().zip(cache.iter_mut()) {
            if row.dirty || entry.is_none() {
                *entry = Some(Self::flatten_row(row));
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
            if let Some((row_chars, row_tags)) = entry.as_ref() {
                let global_offset = chars.len();

                // Record the flat index where this row begins.
                row_offsets.push(global_offset);

                // Append this row's characters, adjusting tag offsets.
                for row_tag in row_tags {
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

                chars.extend_from_slice(row_chars);
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

    /// Flatten a single [`Row`] into a `(Vec<TChar>, Vec<FormatTag>)` pair.
    ///
    /// Tag offsets are **row-relative** (start at 0 for the first character in
    /// this row).  The caller is responsible for re-basing them into global
    /// offsets when merging multiple rows.
    fn flatten_row(row: &Row) -> (Vec<TChar>, Vec<FormatTag>) {
        let mut chars: Vec<TChar> = Vec::new();
        let mut tags: Vec<FormatTag> = Vec::new();

        for cell in row.get_characters() {
            // Skip wide-glyph continuation cells.
            if cell.is_continuation() {
                continue;
            }

            let byte_pos = chars.len();
            chars.push(*cell.tchar());

            let cell_tag = cell.tag();
            if let Some(last) = tags.last_mut() {
                if last.end == byte_pos && tags_same_format(last, cell_tag) {
                    last.end += 1;
                } else {
                    tags.push(FormatTag {
                        start: byte_pos,
                        end: byte_pos + 1,
                        colors: cell_tag.colors,
                        font_weight: cell_tag.font_weight,
                        font_decorations: cell_tag.font_decorations,
                        url: cell_tag.url.clone(),
                        blink: cell_tag.blink,
                    });
                }
            } else {
                tags.push(FormatTag {
                    start: byte_pos,
                    end: byte_pos + 1,
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

        (chars, tags)
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
            let cells = row.get_characters();

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
            let cells = row.get_characters();

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
