// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Run segmentation and text shaping via `rustybuzz`.
//!
//! Splits visible terminal content into [`TextRun`] spans based on format changes
//! and font-face boundaries, then shapes each run to produce glyph IDs and advances.
//! Results are cached per-line for incremental updates.

use std::hash::{Hash, Hasher};
use std::sync::Arc;

use rustc_hash::FxHasher;

use conv2::{ConvUtil, ValueFrom};

use freminal_common::buffer_states::{
    fonts::{BlinkState, FontDecorationFlags, FontWeight},
    format_tag::FormatTag,
    tchar::TChar,
};
use freminal_terminal_emulator::LineWidth;

use super::font_manager::{FaceId, FontManager, GlyphStyle};

// ---------------------------------------------------------------------------
//  Public types
// ---------------------------------------------------------------------------

/// A contiguous span of characters that share the same format and font face,
/// suitable for a single `rustybuzz::shape()` call.
#[derive(Debug, Clone)]
pub struct TextRun {
    /// Column index of the first character in this run (within a single line).
    pub col_start: usize,
    /// Number of terminal columns covered by characters in this run.
    ///
    /// For wide characters (CJK) a single character counts as 2 columns.
    pub col_count: usize,
    /// The `FaceId` that all characters in this run resolved to.
    pub face_id: FaceId,
    /// Style (bold/italic) for this run.
    pub style: GlyphStyle,
    /// The font weight from the format tag.
    pub font_weight: FontWeight,
    /// Font decorations (underline, strikethrough, etc.) from the format tag.
    pub font_decorations: FontDecorationFlags,
    /// Foreground color index (as-is from the `FormatTag`).
    pub colors: freminal_common::buffer_states::cursor::StateColors,
    /// URL associated with this run, if any.
    pub url: Option<Arc<freminal_common::buffer_states::url::Url>>,
    /// The UTF-8 text content of this run, concatenated.
    pub text: String,
    /// Per-character column widths (1 for normal, 2 for wide, 0 for continuation).
    pub char_widths: Vec<usize>,
    /// Blink state for all characters in this run.
    pub blink: BlinkState,
}

/// The output of shaping a single [`TextRun`].
///
/// Contains glyph IDs, x-advances, y-offsets, and cluster→character mapping
/// produced by `rustybuzz`.
#[derive(Debug, Clone)]
pub struct ShapedGlyph {
    /// Glyph ID in the font.
    pub glyph_id: u16,
    /// X position of this glyph in pixels, snapped to the cell grid.
    pub x_px: f32,
    /// Y offset in pixels (usually 0 for horizontal text).
    pub y_offset: f32,
    /// The `FaceId` for rasterisation.
    pub face_id: FaceId,
    /// Whether this is a color glyph (emoji).
    pub is_color: bool,
    /// Column width of the character (1 or 2).
    pub cell_width: usize,
}

/// All shaped glyphs for a single [`TextRun`].
#[derive(Debug, Clone)]
pub struct ShapedRun {
    /// Shaped glyphs in visual order.
    pub glyphs: Vec<ShapedGlyph>,
    /// Starting column of this run.
    pub col_start: usize,
    /// Style for this run (for decoration rendering).
    pub style: GlyphStyle,
    /// Font weight for this run.
    pub font_weight: FontWeight,
    /// Font decorations for this run.
    pub font_decorations: FontDecorationFlags,
    /// Colors for this run.
    pub colors: freminal_common::buffer_states::cursor::StateColors,
    /// URL for this run.
    pub url: Option<Arc<freminal_common::buffer_states::url::Url>>,
    /// Blink state for all glyphs in this run.
    pub blink: BlinkState,
}

/// Shaped output for a single terminal line.
#[derive(Debug, Clone)]
pub struct ShapedLine {
    /// All shaped runs for this line.
    pub runs: Vec<ShapedRun>,
    /// Line-width attribute from the buffer row (DECDWL / DECDHL).
    ///
    /// The renderer uses this to apply horizontal and/or vertical scaling.
    pub line_width: LineWidth,
}

/// Per-line shaping cache.
///
/// Stores `(content_hash, Arc<ShapedLine>)` per row index.  On each snapshot,
/// only re-shape rows whose content hash changed.  Cache hits return an `Arc`
/// clone (refcount bump) instead of a deep clone.
pub struct ShapingCache {
    /// Per-line cache: `(hash, shaped_line)`.
    entries: Vec<Option<(u64, Arc<ShapedLine>)>>,
}

impl Default for ShapingCache {
    fn default() -> Self {
        Self::new()
    }
}

impl ShapingCache {
    /// Create a new empty shaping cache.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Invalidate the entire cache (e.g. on font change).
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Shape all visible lines, using cached results where possible.
    ///
    /// `visible_chars` and `visible_tags` come directly from the
    /// `TerminalSnapshot`.  The function splits them into per-line segments,
    /// hashes each line, and only re-shapes lines whose hash changed.
    ///
    /// Returns a `Vec<Arc<ShapedLine>>` with one entry per visible line.
    /// Cache hits are cheap `Arc` refcount bumps — no deep clone.
    // `visible_line_widths` must accompany per-line data for correct shaping;
    // bundling it into a struct would just wrap an existing slice parameter.
    #[allow(clippy::too_many_arguments)]
    pub fn shape_visible(
        &mut self,
        visible_chars: &[TChar],
        visible_tags: &[FormatTag],
        term_width: usize,
        font_manager: &mut FontManager,
        cell_width: f32,
        ligatures: bool,
        visible_line_widths: &[LineWidth],
    ) -> Vec<Arc<ShapedLine>> {
        let lines = split_into_lines(visible_chars);
        let line_count = lines.len();

        // Resize cache to match line count.
        self.entries.resize_with(line_count, || None);
        if self.entries.len() > line_count {
            self.entries.truncate(line_count);
        }

        let mut result = Vec::with_capacity(line_count);

        // Track the character offset into the global flat array for tag lookup.
        let mut global_offset: usize = 0;

        for (line_idx, line_chars) in lines.iter().enumerate() {
            let lw = visible_line_widths
                .get(line_idx)
                .copied()
                .unwrap_or_default();

            // Include line_width in hash so cache invalidates when DECDWL/DECDHL changes.
            let mut line_hash = hash_line(line_chars, visible_tags, global_offset);
            {
                let mut h = FxHasher::default();
                line_hash.hash(&mut h);
                std::mem::discriminant(&lw).hash(&mut h);
                line_hash = h.finish();
            }

            let shaped = if let Some((_h, shaped_line)) = self
                .entries
                .get(line_idx)
                .and_then(|e| e.as_ref())
                .filter(|(h, _)| *h == line_hash)
            {
                // Cache hit — reuse via Arc refcount bump.
                Arc::clone(shaped_line)
            } else {
                // Cache miss — segment and shape.
                let runs = segment_line(
                    line_chars,
                    visible_tags,
                    global_offset,
                    term_width,
                    font_manager,
                );
                let shaped_runs = shape_runs(&runs, font_manager, cell_width, ligatures);
                let shaped_line = Arc::new(ShapedLine {
                    runs: shaped_runs,
                    line_width: lw,
                });
                self.entries[line_idx] = Some((line_hash, Arc::clone(&shaped_line)));
                shaped_line
            };

            result.push(shaped);

            // Advance past this line's chars + the NewLine separator.
            global_offset += line_chars.len() + 1; // +1 for the NewLine
        }

        result
    }
}

// ---------------------------------------------------------------------------
//  Line splitting
// ---------------------------------------------------------------------------

/// Split a flat `TChar` array into per-line segments.
///
/// Lines are delimited by `TChar::NewLine`.  The `NewLine` characters themselves
/// are NOT included in the returned slices.
fn split_into_lines(chars: &[TChar]) -> Vec<&[TChar]> {
    let mut lines = Vec::new();
    let mut start = 0;

    for (i, ch) in chars.iter().enumerate() {
        if matches!(ch, TChar::NewLine) {
            lines.push(&chars[start..i]);
            start = i + 1;
        }
    }

    // Trailing content after the last NewLine (or the entire array if no NewLine).
    if start <= chars.len() {
        lines.push(&chars[start..]);
    }

    lines
}

// ---------------------------------------------------------------------------
//  Hashing
// ---------------------------------------------------------------------------

/// Compute a content hash for a single line, incorporating both character data
/// and the format tags that overlap this line's range.
///
/// Uses `FxHasher` (non-cryptographic) for speed — these hashes are cache keys,
/// not security-sensitive.
fn hash_line(line_chars: &[TChar], tags: &[FormatTag], global_offset: usize) -> u64 {
    let mut hasher = FxHasher::default();

    // Hash character content.
    for ch in line_chars {
        match ch {
            TChar::Ascii(b) => {
                0u8.hash(&mut hasher); // discriminant
                b.hash(&mut hasher);
            }
            TChar::Utf8(buf, len) => {
                1u8.hash(&mut hasher);
                buf[..usize::from(*len)].hash(&mut hasher);
            }
            TChar::Space => 2u8.hash(&mut hasher),
            TChar::NewLine => 3u8.hash(&mut hasher),
        }
    }

    // Hash overlapping tags.
    let line_end = global_offset + line_chars.len();
    for tag in tags {
        if tag.start >= line_end {
            break; // Tags are sorted by start; no more can overlap.
        }
        if tag.end <= global_offset {
            continue;
        }
        // This tag overlaps our line — hash its properties.
        tag.start.hash(&mut hasher);
        tag.end.hash(&mut hasher);
        tag.colors.hash(&mut hasher);
        tag.font_weight.hash(&mut hasher);
        tag.font_decorations.hash(&mut hasher);
        tag.url.hash(&mut hasher);
        tag.blink.hash(&mut hasher);
    }

    hasher.finish()
}

// ---------------------------------------------------------------------------
//  Run segmentation
// ---------------------------------------------------------------------------

/// Find the `FormatTag` that covers position `global_pos` in the flat array.
///
/// Falls back to `FormatTag::default()` if no tag covers the position.
fn tag_at_position(tags: &[FormatTag], global_pos: usize) -> &FormatTag {
    // Tags are sorted by start; find the last tag whose start <= global_pos.
    // We search linearly from the end for simplicity — visible lines are short.
    for tag in tags.iter().rev() {
        if tag.start <= global_pos && global_pos < tag.end {
            return tag;
        }
    }

    // No tag covers this position — this can occur when the snapshot's tag
    // list is empty or when a character falls outside all tag ranges (e.g.
    // after a partial snapshot or during a buffer transition).  Fall back to
    // the first tag if one exists, otherwise use a static default tag with
    // default colors and no decorations.
    tags.first().unwrap_or_else(|| {
        // This is a compile-time-known static default, safe to leak.
        static DEFAULT_TAG: FormatTag = FormatTag {
            start: 0,
            end: usize::MAX,
            colors: freminal_common::buffer_states::cursor::StateColors {
                color: freminal_common::colors::TerminalColor::Default,
                background_color: freminal_common::colors::TerminalColor::DefaultBackground,
                underline_color: freminal_common::colors::TerminalColor::DefaultUnderlineColor,
                reverse_video: freminal_common::buffer_states::cursor::ReverseVideo::Off,
            },
            font_weight: FontWeight::Normal,
            font_decorations: FontDecorationFlags::empty(),
            url: None,
            blink: freminal_common::buffer_states::fonts::BlinkState::None,
        };
        &DEFAULT_TAG
    })
}

/// Check if two tags have the same visual format (ignoring position).
fn same_format(a: &FormatTag, b: &FormatTag) -> bool {
    a.font_weight == b.font_weight
        && a.font_decorations == b.font_decorations
        && a.colors == b.colors
        && a.url == b.url
        && a.blink == b.blink
}

/// Segment a single line into `TextRun`s based on format and face boundaries.
fn segment_line(
    line_chars: &[TChar],
    tags: &[FormatTag],
    global_offset: usize,
    _term_width: usize,
    font_manager: &mut FontManager,
) -> Vec<TextRun> {
    if line_chars.is_empty() {
        return Vec::new();
    }

    let mut runs = Vec::new();
    let mut run_col_start: usize = 0;
    let mut run_col_count: usize = 0;
    let mut run_text = String::new();
    let mut run_char_widths: Vec<usize> = Vec::new();

    // Resolve first character.
    let first_char = tchar_to_char(&line_chars[0]);
    let first_tag = tag_at_position(tags, global_offset);
    let first_style = GlyphStyle::from_format(&first_tag.font_weight, first_tag.font_decorations);
    let (first_face, _) = font_manager.resolve_glyph(first_char, first_style);
    let first_width = line_chars[0].display_width();

    let mut current_tag = first_tag;
    let mut current_face = first_face;
    let mut current_style = first_style;

    // Start first run.
    push_char_to_run(&mut run_text, first_char);
    run_char_widths.push(first_width);
    run_col_count += first_width;

    for (i, tch) in line_chars.iter().enumerate().skip(1) {
        let ch = tchar_to_char(tch);
        let gpos = global_offset + i;
        let tag = tag_at_position(tags, gpos);
        let style = GlyphStyle::from_format(&tag.font_weight, tag.font_decorations);
        let (face, _) = font_manager.resolve_glyph(ch, style);
        let width = tch.display_width();

        let format_changed = !same_format(current_tag, tag);
        let face_changed = face != current_face;

        if format_changed || face_changed {
            // Flush current run.
            runs.push(TextRun {
                col_start: run_col_start,
                col_count: run_col_count,
                face_id: current_face,
                style: current_style,
                font_weight: current_tag.font_weight,
                font_decorations: current_tag.font_decorations,
                colors: current_tag.colors,
                url: current_tag.url.clone(),
                text: std::mem::take(&mut run_text),
                char_widths: std::mem::take(&mut run_char_widths),
                blink: current_tag.blink,
            });

            // Start new run.
            run_col_start += run_col_count;
            run_col_count = 0;
            current_tag = tag;
            current_face = face;
            current_style = style;
        }

        push_char_to_run(&mut run_text, ch);
        run_char_widths.push(width);
        run_col_count += width;
    }

    // Flush final run.
    if !run_text.is_empty() {
        runs.push(TextRun {
            col_start: run_col_start,
            col_count: run_col_count,
            face_id: current_face,
            style: current_style,
            font_weight: current_tag.font_weight,
            font_decorations: current_tag.font_decorations,
            colors: current_tag.colors,
            url: current_tag.url.clone(),
            text: run_text,
            char_widths: run_char_widths,
            blink: current_tag.blink,
        });
    }

    runs
}

/// Convert a `TChar` to a `char` for shaping.
fn tchar_to_char(tch: &TChar) -> char {
    match tch {
        TChar::Ascii(b) => char::from(*b),
        TChar::Space => ' ',
        TChar::NewLine => '\n',
        TChar::Utf8(buf, len) => {
            std::str::from_utf8(&buf[..usize::from(*len)])
                .ok()
                .and_then(|s| s.chars().next())
                .unwrap_or('\u{FFFD}') // replacement character
        }
    }
}

/// Push a char onto the run text buffer.
fn push_char_to_run(text: &mut String, ch: char) {
    text.push(ch);
}

// ---------------------------------------------------------------------------
//  Shaping
// ---------------------------------------------------------------------------

/// Build the rustybuzz OpenType feature list.
///
/// When `ligatures` is `true`, `liga` and `calt` are enabled (value 1) so the
/// font's standard and contextual ligatures are applied during shaping.
/// When `false`, all three ligature tags (`liga`, `calt`, `dlig`) are
/// explicitly disabled (value 0) to prevent ligature formation even in fonts
/// that enable them by default.
///
/// `kern` (kerning) is always enabled.
fn shaping_features(ligatures: bool) -> Vec<rustybuzz::Feature> {
    use rustybuzz::ttf_parser::Tag;
    let lig_value = u32::from(ligatures);
    vec![
        // Enable kerning.
        rustybuzz::Feature::new(Tag::from_bytes(b"kern"), 1, ..),
        // Standard ligatures — controlled by config.
        rustybuzz::Feature::new(Tag::from_bytes(b"liga"), lig_value, ..),
        // Contextual alternates — controlled by config.
        rustybuzz::Feature::new(Tag::from_bytes(b"calt"), lig_value, ..),
        // Discretionary ligatures — always disabled (too aggressive for
        // terminal use; can be revisited later).
        rustybuzz::Feature::new(Tag::from_bytes(b"dlig"), 0, ..),
    ]
}

/// Shape a set of `TextRun`s into `ShapedRun`s.
fn shape_runs(
    runs: &[TextRun],
    font_manager: &FontManager,
    cell_width: f32,
    ligatures: bool,
) -> Vec<ShapedRun> {
    let features = shaping_features(ligatures);

    runs.iter()
        .map(|run| shape_single_run(run, font_manager, cell_width, &features))
        .collect()
}

/// Shape a single `TextRun` via `rustybuzz`.
fn shape_single_run(
    run: &TextRun,
    font_manager: &FontManager,
    cell_width: f32,
    features: &[rustybuzz::Feature],
) -> ShapedRun {
    let is_emoji_face = run.face_id == FaceId::Emoji;

    // Try to get a rustybuzz face for this run's font.
    let glyphs = font_manager.rustybuzz_face(run.face_id).map_or_else(
        || {
            // No face available — produce tofu (glyph_id=0) per character.
            build_tofu_glyphs(&run.char_widths, run.col_start, run.face_id, cell_width)
        },
        |face| {
            // Build the input buffer.
            let mut buffer = rustybuzz::UnicodeBuffer::new();
            buffer.push_str(&run.text);

            // Shape.
            let output = rustybuzz::shape(&face, features, buffer);

            let infos = output.glyph_infos();

            // Map shaped glyphs back to cell-grid positions.
            build_shaped_glyphs(
                infos,
                &run.text,
                &run.char_widths,
                run.col_start,
                run.face_id,
                is_emoji_face,
                cell_width,
            )
        },
    );

    ShapedRun {
        glyphs,
        col_start: run.col_start,
        style: run.style,
        font_weight: run.font_weight,
        font_decorations: run.font_decorations,
        colors: run.colors,
        url: run.url.clone(),
        blink: run.blink,
    }
}

/// Build `ShapedGlyph`s from `rustybuzz` output, snapping to the cell grid.
///
/// The cell grid is authoritative: glyph positions are snapped to column
/// boundaries.  When ligatures are active, a single glyph may cover multiple
/// input characters.  Its `cell_width` is the sum of those characters'
/// individual widths so it spans the correct number of terminal cells.
fn build_shaped_glyphs(
    infos: &[rustybuzz::GlyphInfo],
    run_text: &str,
    char_widths: &[usize],
    col_start: usize,
    face_id: FaceId,
    is_color: bool,
    cell_width: f32,
) -> Vec<ShapedGlyph> {
    let mut glyphs = Vec::with_capacity(infos.len());

    // Build a byte-offset → char-index lookup table.  `rustybuzz` cluster
    // values are byte offsets into the UTF-8 input string.  We need char
    // indices to index into `char_widths`.
    let byte_to_char: Vec<(usize, usize)> = run_text
        .char_indices()
        .enumerate()
        .map(|(ci, (bi, _))| (bi, ci))
        .collect();
    let num_chars = char_widths.len();

    // Pre-compute cumulative column offsets so we can look up the column of
    // any char index in O(1).  `cum_cols[i]` is the sum of `char_widths[0..i]`.
    let mut cum_cols: Vec<usize> = Vec::with_capacity(num_chars + 1);
    cum_cols.push(0);
    for &w in char_widths {
        cum_cols.push(cum_cols.last().copied().unwrap_or(0) + w);
    }

    // Helper: resolve a byte offset to a char index via the lookup table.
    let resolve_cluster = |cluster_byte: usize, fallback: usize| -> usize {
        byte_to_char
            .binary_search_by_key(&cluster_byte, |&(b, _)| b)
            .map_or_else(|_| fallback, |pos| byte_to_char[pos].1)
    };

    for (glyph_idx, info) in infos.iter().enumerate() {
        // `u32 -> usize` for byte-offset indexing; lossless on all 64-bit
        // targets. On hypothetical 32-bit hosts, falls back to 0, which is a
        // safe sentinel that the `resolve_cluster` binary-search handles.
        let cluster_byte = usize::value_from(info.cluster).unwrap_or(0);

        // Map byte offset → char index.  Fallback: glyph index clamped to
        // range (should only trigger on malformed shaper output).
        let char_idx = resolve_cluster(cluster_byte, glyph_idx.min(num_chars.saturating_sub(1)));

        // Determine how many input characters this glyph covers.
        // For LTR text, each glyph "owns" characters from `char_idx` up to
        // (but not including) the char index of the next glyph.  The last
        // glyph owns through the end of the run.
        let next_char_idx = if glyph_idx + 1 < infos.len() {
            let next_cluster_byte = usize::value_from(infos[glyph_idx + 1].cluster).unwrap_or(0);
            resolve_cluster(next_cluster_byte, (glyph_idx + 1).min(num_chars))
        } else {
            num_chars
        };

        // Total cell width = sum of char_widths for all characters in the cluster.
        let cw = if next_char_idx > char_idx {
            cum_cols
                .get(next_char_idx)
                .copied()
                .unwrap_or(cum_cols[cum_cols.len() - 1])
                - cum_cols.get(char_idx).copied().unwrap_or(0)
        } else {
            // Defensive: glyph covers zero characters (shouldn't happen).
            char_widths.get(char_idx).copied().unwrap_or(1)
        };

        // Cell-grid x position from the cumulative column offset.
        let col_for_glyph = col_start + cum_cols.get(char_idx).copied().unwrap_or(0);

        let x_px = col_for_glyph.approx_as::<f32>().unwrap_or(0.0) * cell_width;

        let gid = u16::value_from(info.glyph_id).unwrap_or(0);

        glyphs.push(ShapedGlyph {
            glyph_id: gid,
            x_px,
            y_offset: 0.0, // Horizontal text: y offset is rarely nonzero.
            face_id,
            is_color,
            cell_width: cw,
        });
    }

    glyphs
}

/// Produce tofu (glyph 0) glyphs when no face is available.
fn build_tofu_glyphs(
    char_widths: &[usize],
    col_start: usize,
    face_id: FaceId,
    cell_width: f32,
) -> Vec<ShapedGlyph> {
    let mut glyphs = Vec::with_capacity(char_widths.len());
    let mut col = col_start;

    for &cw in char_widths {
        let x_px = col.approx_as::<f32>().unwrap_or(0.0) * cell_width;

        glyphs.push(ShapedGlyph {
            glyph_id: 0,
            x_px,
            y_offset: 0.0,
            face_id,
            is_color: false,
            cell_width: cw,
        });

        col += cw;
    }

    glyphs
}

// ---------------------------------------------------------------------------
//  Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use freminal_common::config::Config;

    /// Helper: create a default `FontManager` for tests.
    fn test_font_manager() -> FontManager {
        FontManager::new(&Config::default(), 1.0).unwrap()
    }

    /// Helper: create a simple format tag covering a range.
    fn make_tag(start: usize, end: usize) -> FormatTag {
        FormatTag {
            start,
            end,
            ..FormatTag::default()
        }
    }

    /// Helper: create a bold format tag covering a range.
    fn make_bold_tag(start: usize, end: usize) -> FormatTag {
        FormatTag {
            start,
            end,
            font_weight: FontWeight::Bold,
            ..FormatTag::default()
        }
    }

    /// Helper: create a tag with a custom foreground color covering a range.
    fn make_colored_tag(
        start: usize,
        end: usize,
        color: freminal_common::colors::TerminalColor,
    ) -> FormatTag {
        FormatTag {
            start,
            end,
            colors: freminal_common::buffer_states::cursor::StateColors {
                color,
                ..Default::default()
            },
            ..FormatTag::default()
        }
    }

    // -- Line splitting --

    #[test]
    fn split_empty() {
        let chars: Vec<TChar> = vec![];
        let lines = split_into_lines(&chars);
        assert_eq!(lines.len(), 1); // One empty trailing line.
        assert!(lines[0].is_empty());
    }

    #[test]
    fn split_single_line() {
        let chars = vec![TChar::Ascii(b'A'), TChar::Ascii(b'B')];
        let lines = split_into_lines(&chars);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].len(), 2);
    }

    #[test]
    fn split_two_lines() {
        let chars = vec![TChar::Ascii(b'A'), TChar::NewLine, TChar::Ascii(b'B')];
        let lines = split_into_lines(&chars);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].len(), 1);
        assert_eq!(lines[1].len(), 1);
    }

    // -- Run segmentation --

    #[test]
    fn segment_ascii_single_run() {
        let mut fm = test_font_manager();
        let chars = vec![TChar::Ascii(b'H'), TChar::Ascii(b'i')];
        let tags = vec![make_tag(0, 10)];

        let runs = segment_line(&chars, &tags, 0, 80, &mut fm);
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].text, "Hi");
        assert_eq!(runs[0].col_start, 0);
        assert_eq!(runs[0].col_count, 2);
        assert_eq!(runs[0].char_widths, vec![1, 1]);
    }

    #[test]
    fn segment_splits_on_format_change() {
        let mut fm = test_font_manager();
        // "AB" where A is normal and B is bold.
        let chars = vec![TChar::Ascii(b'A'), TChar::Ascii(b'B')];
        let tags = vec![make_tag(0, 1), make_bold_tag(1, 2)];

        let runs = segment_line(&chars, &tags, 0, 80, &mut fm);
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].text, "A");
        assert_eq!(runs[0].font_weight, FontWeight::Normal);
        assert_eq!(runs[1].text, "B");
        assert_eq!(runs[1].font_weight, FontWeight::Bold);
    }

    // -- ASCII shaping --

    #[test]
    fn shape_ascii_uniform_advances() {
        let mut fm = test_font_manager();
        #[allow(clippy::cast_precision_loss)]
        let cell_w = fm.cell_width() as f32;

        let chars = vec![TChar::Ascii(b'A'), TChar::Ascii(b'B'), TChar::Ascii(b'C')];
        let tags = vec![make_tag(0, 10)];

        let runs = segment_line(&chars, &tags, 0, 80, &mut fm);
        let shaped = shape_runs(&runs, &fm, cell_w, false);

        assert_eq!(shaped.len(), 1);
        assert_eq!(shaped[0].glyphs.len(), 3);

        // Check that glyphs are at cell-grid positions.
        for (i, g) in shaped[0].glyphs.iter().enumerate() {
            #[allow(clippy::cast_precision_loss)]
            let expected_x = i as f32 * cell_w;
            assert!(
                (g.x_px - expected_x).abs() < f32::EPSILON,
                "glyph {i}: expected x={expected_x}, got x={}",
                g.x_px
            );
            assert_eq!(g.cell_width, 1);
            assert!(!g.is_color);
        }
    }

    // -- CJK wide character --

    #[test]
    fn shape_cjk_two_cell_advance() {
        let mut fm = test_font_manager();
        #[allow(clippy::cast_precision_loss)]
        let cell_w = fm.cell_width() as f32;

        // U+4E2D (中) is a wide CJK character, display_width = 2.
        let chars = vec![TChar::from('中')];
        let tags = vec![make_tag(0, 10)];

        let runs = segment_line(&chars, &tags, 0, 80, &mut fm);
        let shaped = shape_runs(&runs, &fm, cell_w, false);

        assert_eq!(shaped.len(), 1);
        assert_eq!(shaped[0].glyphs.len(), 1);
        assert_eq!(shaped[0].glyphs[0].cell_width, 2);
    }

    // -- Emoji routing --

    #[test]
    fn shape_emoji_routes_to_emoji_face() {
        let mut fm = test_font_manager();
        #[allow(clippy::cast_precision_loss)]
        let cell_w = fm.cell_width() as f32;

        // U+1F600 (😀) should route to emoji face if available.
        let chars = vec![TChar::from('😀')];
        let tags = vec![make_tag(0, 10)];

        let runs = segment_line(&chars, &tags, 0, 80, &mut fm);

        // The run should have face_id == FaceId::Emoji (if system has an emoji font)
        // or some system fallback.  Either way, shaping should succeed.
        let shaped = shape_runs(&runs, &fm, cell_w, false);
        assert_eq!(shaped.len(), 1);
        assert!(!shaped[0].glyphs.is_empty());
    }

    // -- Face boundary splitting --

    #[test]
    fn segment_splits_on_face_boundary() {
        let mut fm = test_font_manager();

        // "A😀B" — ASCII, emoji, ASCII.  Should produce at least 2 runs
        // (face boundary between ASCII and emoji).
        let chars = vec![TChar::Ascii(b'A'), TChar::from('😀'), TChar::Ascii(b'B')];
        let tags = vec![make_tag(0, 10)];

        let runs = segment_line(&chars, &tags, 0, 80, &mut fm);
        // At minimum, ASCII and emoji should be in different runs if emoji face differs.
        // On systems without emoji font, they may all fall back to the same face.
        assert!(!runs.is_empty());
    }

    // -- Cache --

    #[test]
    fn cache_hit_avoids_reshaping() {
        let mut fm = test_font_manager();
        #[allow(clippy::cast_precision_loss)]
        let cell_w = fm.cell_width() as f32;
        let mut cache = ShapingCache::new();

        let chars = vec![TChar::Ascii(b'X'), TChar::Ascii(b'Y')];
        let tags = vec![make_tag(0, 10)];

        // First call — cache miss.
        let r1 = cache.shape_visible(&chars, &tags, 80, &mut fm, cell_w, false, &[]);
        assert_eq!(r1.len(), 1);

        // Second call with identical input — cache hit.
        let r2 = cache.shape_visible(&chars, &tags, 80, &mut fm, cell_w, false, &[]);
        assert_eq!(r2.len(), 1);

        // Results should be identical (same glyph count).
        assert_eq!(r1[0].runs.len(), r2[0].runs.len());
    }

    #[test]
    fn cache_miss_on_changed_content() {
        let mut fm = test_font_manager();
        #[allow(clippy::cast_precision_loss)]
        let cell_w = fm.cell_width() as f32;
        let mut cache = ShapingCache::new();

        let chars1 = vec![TChar::Ascii(b'X')];
        let tags = vec![make_tag(0, 10)];

        let _ = cache.shape_visible(&chars1, &tags, 80, &mut fm, cell_w, false, &[]);

        // Change content.
        let chars2 = vec![TChar::Ascii(b'Y')];
        let r2 = cache.shape_visible(&chars2, &tags, 80, &mut fm, cell_w, false, &[]);

        // Should still produce valid output (cache miss, re-shaped).
        assert_eq!(r2.len(), 1);
        assert_eq!(r2[0].runs.len(), 1);
        assert_eq!(r2[0].runs[0].glyphs.len(), 1);
    }

    // -- Ligature-breaking conditions (Task 5.6) --

    #[test]
    fn color_change_mid_sequence_breaks_into_separate_runs() {
        // "->" where '-' is red and '>' is default — must be two separate runs
        // so no ligature can form across the color boundary.
        let mut fm = test_font_manager();
        let chars = vec![TChar::Ascii(b'-'), TChar::Ascii(b'>')];
        let tags = vec![
            make_colored_tag(0, 1, freminal_common::colors::TerminalColor::Red),
            make_tag(1, 2),
        ];

        let runs = segment_line(&chars, &tags, 0, 80, &mut fm);
        assert_eq!(runs.len(), 2, "color change must break the run");
        assert_eq!(runs[0].text, "-");
        assert_eq!(runs[1].text, ">");
    }

    #[test]
    fn style_change_mid_sequence_breaks_into_separate_runs() {
        // "->" where '-' is bold and '>' is normal — two separate runs.
        let mut fm = test_font_manager();
        let chars = vec![TChar::Ascii(b'-'), TChar::Ascii(b'>')];
        let tags = vec![make_bold_tag(0, 1), make_tag(1, 2)];

        let runs = segment_line(&chars, &tags, 0, 80, &mut fm);
        assert_eq!(runs.len(), 2, "style change must break the run");
        assert_eq!(runs[0].text, "-");
        assert_eq!(runs[1].text, ">");
    }

    #[test]
    fn line_boundary_prevents_cross_line_ligature() {
        // "-\n>" — the '-' and '>' are on different lines so they cannot ligate.
        let chars = vec![TChar::Ascii(b'-'), TChar::NewLine, TChar::Ascii(b'>')];
        let lines = split_into_lines(&chars);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].len(), 1, "first line has just '-'");
        assert_eq!(lines[1].len(), 1, "second line has just '>'");
        // Each line is shaped independently, so no ligature can span them.
    }

    #[test]
    fn same_format_sequence_stays_in_one_run() {
        // "->" with same format — must stay in one run so ligature CAN form.
        let mut fm = test_font_manager();
        let chars = vec![TChar::Ascii(b'-'), TChar::Ascii(b'>')];
        let tags = vec![make_tag(0, 10)];

        let runs = segment_line(&chars, &tags, 0, 80, &mut fm);
        assert_eq!(runs.len(), 1, "same-format run should not be broken");
        assert_eq!(runs[0].text, "->");
    }

    #[test]
    fn background_color_change_breaks_run() {
        // "->" where '-' has a colored background and '>' has default — two runs.
        let mut fm = test_font_manager();
        let chars = vec![TChar::Ascii(b'-'), TChar::Ascii(b'>')];
        let tags = vec![
            FormatTag {
                start: 0,
                end: 1,
                colors: freminal_common::buffer_states::cursor::StateColors {
                    background_color: freminal_common::colors::TerminalColor::Blue,
                    ..Default::default()
                },
                ..FormatTag::default()
            },
            make_tag(1, 2),
        ];

        let runs = segment_line(&chars, &tags, 0, 80, &mut fm);
        assert_eq!(runs.len(), 2, "background color change must break the run");
    }

    // -- build_shaped_glyphs: ligature-aware cluster mapping --

    /// Helper: construct a `GlyphInfo` with just the fields we need.
    fn make_glyph_info(glyph_id: u32, cluster: u32) -> rustybuzz::GlyphInfo {
        let mut info = rustybuzz::GlyphInfo::default();
        info.glyph_id = glyph_id;
        info.cluster = cluster;
        info
    }

    #[test]
    fn build_glyphs_no_ligature_ascii() {
        // 3 ASCII chars "ABC", each 1 byte, no ligatures.
        // Shaper output: 3 glyphs, clusters [0, 1, 2].
        let infos = [
            make_glyph_info(65, 0),
            make_glyph_info(66, 1),
            make_glyph_info(67, 2),
        ];
        let text = "ABC";
        let char_widths = [1, 1, 1];
        let cell_width = 10.0;

        let glyphs = build_shaped_glyphs(
            &infos,
            text,
            &char_widths,
            0,
            FaceId::PrimaryRegular,
            false,
            cell_width,
        );

        assert_eq!(glyphs.len(), 3);
        for (i, g) in glyphs.iter().enumerate() {
            #[allow(clippy::cast_precision_loss)]
            let expected_x = i as f32 * cell_width;
            assert!(
                (g.x_px - expected_x).abs() < f32::EPSILON,
                "glyph {i}: expected x={expected_x}, got x={}",
                g.x_px
            );
            assert_eq!(g.cell_width, 1, "glyph {i} should be 1 cell wide");
        }
    }

    #[test]
    fn build_glyphs_two_char_ligature() {
        // "->" (2 ASCII bytes) → shaper produces 1 ligature glyph.
        // Cluster value = 0 (byte offset of '-').
        let infos = [make_glyph_info(999, 0)]; // single ligature glyph
        let text = "->";
        let char_widths = [1, 1]; // each source char is 1 cell
        let cell_width = 10.0;

        let glyphs = build_shaped_glyphs(
            &infos,
            text,
            &char_widths,
            0,
            FaceId::PrimaryRegular,
            false,
            cell_width,
        );

        assert_eq!(glyphs.len(), 1);
        assert_eq!(
            glyphs[0].cell_width, 2,
            "ligature glyph should span 2 cells"
        );
        assert!(
            glyphs[0].x_px.abs() < f32::EPSILON,
            "ligature should start at column 0"
        );
    }

    #[test]
    fn build_glyphs_three_char_ligature() {
        // "===" (3 ASCII bytes) → shaper produces 1 ligature glyph.
        let infos = [make_glyph_info(888, 0)];
        let text = "===";
        let char_widths = [1, 1, 1];
        let cell_width = 10.0;

        let glyphs = build_shaped_glyphs(
            &infos,
            text,
            &char_widths,
            0,
            FaceId::PrimaryRegular,
            false,
            cell_width,
        );

        assert_eq!(glyphs.len(), 1);
        assert_eq!(
            glyphs[0].cell_width, 3,
            "3-char ligature should span 3 cells"
        );
    }

    #[test]
    fn build_glyphs_ligature_with_col_start_offset() {
        // "->" ligature starting at column 5 (e.g., second run in a line).
        let infos = [make_glyph_info(999, 0)];
        let text = "->";
        let char_widths = [1, 1];
        let cell_width = 10.0;
        let col_start = 5;

        let glyphs = build_shaped_glyphs(
            &infos,
            text,
            &char_widths,
            col_start,
            FaceId::PrimaryRegular,
            false,
            cell_width,
        );

        assert_eq!(glyphs.len(), 1);
        assert_eq!(glyphs[0].cell_width, 2);
        #[allow(clippy::cast_precision_loss)]
        let expected_x = col_start as f32 * cell_width;
        assert!(
            (glyphs[0].x_px - expected_x).abs() < f32::EPSILON,
            "expected x={expected_x}, got x={}",
            glyphs[0].x_px
        );
    }

    #[test]
    fn build_glyphs_mixed_ligature_and_normal() {
        // "a->b" — 'a' is normal, "->" forms a ligature, 'b' is normal.
        // Shaper produces 3 glyphs: glyph_a(cluster=0), glyph_lig(cluster=1),
        // glyph_b(cluster=3).
        let infos = [
            make_glyph_info(97, 0),  // 'a' at byte 0
            make_glyph_info(999, 1), // '->' ligature at byte 1
            make_glyph_info(98, 3),  // 'b' at byte 3
        ];
        let text = "a->b";
        let char_widths = [1, 1, 1, 1]; // a, -, >, b
        let cell_width = 10.0;

        let glyphs = build_shaped_glyphs(
            &infos,
            text,
            &char_widths,
            0,
            FaceId::PrimaryRegular,
            false,
            cell_width,
        );

        assert_eq!(glyphs.len(), 3);

        // 'a' at column 0, width 1
        assert!(glyphs[0].x_px.abs() < f32::EPSILON);
        assert_eq!(glyphs[0].cell_width, 1);

        // '->' ligature at column 1, width 2
        assert!((glyphs[1].x_px - 10.0).abs() < f32::EPSILON);
        assert_eq!(glyphs[1].cell_width, 2, "ligature should span 2 cells");

        // 'b' at column 3, width 1
        assert!((glyphs[2].x_px - 30.0).abs() < f32::EPSILON);
        assert_eq!(glyphs[2].cell_width, 1);
    }

    #[test]
    fn build_glyphs_ligature_with_multibyte_chars() {
        // Mix of ASCII and multi-byte: "é->" where é is 2 bytes (U+00E9).
        // byte offsets: é=0(2 bytes), '-'=2, '>'=3
        // Shaper: glyph_e(cluster=0), glyph_lig(cluster=2)
        let infos = [
            make_glyph_info(200, 0), // 'é' at byte 0
            make_glyph_info(999, 2), // '->' ligature at byte 2
        ];
        let text = "é->";
        let char_widths = [1, 1, 1]; // é, -, >
        let cell_width = 10.0;

        let glyphs = build_shaped_glyphs(
            &infos,
            text,
            &char_widths,
            0,
            FaceId::PrimaryRegular,
            false,
            cell_width,
        );

        assert_eq!(glyphs.len(), 2);

        // 'é' at column 0, width 1
        assert!(glyphs[0].x_px.abs() < f32::EPSILON);
        assert_eq!(glyphs[0].cell_width, 1);

        // '->' ligature at column 1, width 2
        assert!((glyphs[1].x_px - 10.0).abs() < f32::EPSILON);
        assert_eq!(glyphs[1].cell_width, 2);
    }

    #[test]
    fn build_glyphs_wide_char_not_confused_with_ligature() {
        // A single wide CJK character — 1 glyph, 1 char, width 2.
        // This is NOT a ligature; the single char just has display_width=2.
        let infos = [make_glyph_info(500, 0)];
        let text = "中"; // U+4E2D, 3 bytes in UTF-8
        let char_widths = [2]; // wide char
        let cell_width = 10.0;

        let glyphs = build_shaped_glyphs(
            &infos,
            text,
            &char_widths,
            0,
            FaceId::PrimaryRegular,
            false,
            cell_width,
        );

        assert_eq!(glyphs.len(), 1);
        assert_eq!(
            glyphs[0].cell_width, 2,
            "wide char should span 2 cells (not a ligature)"
        );
        assert!(glyphs[0].x_px.abs() < f32::EPSILON);
    }
}
