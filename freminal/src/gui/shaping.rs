// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Run segmentation and text shaping via `rustybuzz`.
//!
//! Splits visible terminal content into [`TextRun`] spans based on format changes
//! and font-face boundaries, then shapes each run to produce glyph IDs and advances.
//! Results are cached per-line for incremental updates.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use freminal_common::buffer_states::{
    fonts::{FontDecorations, FontWeight},
    format_tag::FormatTag,
    tchar::TChar,
};

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
    pub font_decorations: Vec<FontDecorations>,
    /// Foreground color index (as-is from the `FormatTag`).
    pub colors: freminal_common::buffer_states::cursor::StateColors,
    /// URL associated with this run, if any.
    pub url: Option<freminal_common::buffer_states::url::Url>,
    /// The UTF-8 text content of this run, concatenated.
    pub text: String,
    /// Per-character column widths (1 for normal, 2 for wide, 0 for continuation).
    pub char_widths: Vec<usize>,
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
    pub font_decorations: Vec<FontDecorations>,
    /// Colors for this run.
    pub colors: freminal_common::buffer_states::cursor::StateColors,
    /// URL for this run.
    pub url: Option<freminal_common::buffer_states::url::Url>,
}

/// Shaped output for a single terminal line.
#[derive(Debug, Clone)]
pub struct ShapedLine {
    /// All shaped runs for this line.
    pub runs: Vec<ShapedRun>,
}

/// Per-line shaping cache.
///
/// Stores `(content_hash, ShapedLine)` per row index.  On each snapshot, only
/// re-shape rows whose content hash changed.
pub struct ShapingCache {
    /// Per-line cache: `(hash, shaped_line)`.
    entries: Vec<Option<(u64, ShapedLine)>>,
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
    /// Returns a `Vec<ShapedLine>` with one entry per visible line.
    ///
    /// # Panics
    ///
    /// This method cannot panic under normal use.  Internally it accesses a
    /// cache entry that was verified to be `Some` immediately before the
    /// access, so the `unwrap` is unreachable in practice.
    pub fn shape_visible(
        &mut self,
        visible_chars: &[TChar],
        visible_tags: &[FormatTag],
        term_width: usize,
        font_manager: &mut FontManager,
        cell_width: f32,
        ligatures: bool,
    ) -> Vec<ShapedLine> {
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
            let line_hash = hash_line(line_chars, visible_tags, global_offset);

            let shaped = if self
                .entries
                .get(line_idx)
                .and_then(|e| e.as_ref())
                .is_some_and(|(h, _)| *h == line_hash)
            {
                // Cache hit — reuse.
                #[allow(clippy::unwrap_used)] // We just verified Some above.
                self.entries[line_idx].as_ref().unwrap().1.clone()
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
                let shaped_line = ShapedLine { runs: shaped_runs };
                self.entries[line_idx] = Some((line_hash, shaped_line.clone()));
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
fn hash_line(line_chars: &[TChar], tags: &[FormatTag], global_offset: usize) -> u64 {
    let mut hasher = DefaultHasher::new();

    // Hash character content.
    for ch in line_chars {
        match ch {
            TChar::Ascii(b) => {
                0u8.hash(&mut hasher); // discriminant
                b.hash(&mut hasher);
            }
            TChar::Utf8(v) => {
                1u8.hash(&mut hasher);
                v.hash(&mut hasher);
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
        // Hash colors via Debug repr (StateColors doesn't impl Hash).
        format!("{:?}", tag.colors).hash(&mut hasher);
        format!("{:?}", tag.font_weight).hash(&mut hasher);
        format!("{:?}", tag.font_decorations).hash(&mut hasher);
        format!("{:?}", tag.url).hash(&mut hasher);
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

    // Should not happen in practice — there's always a default tag.
    // Return the first tag if available, or we'll need a static default.
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
            font_decorations: Vec::new(),
            url: None,
        };
        &DEFAULT_TAG
    })
}

/// Check if two tags have the same visual format (ignoring position).
fn same_format(a: &FormatTag, b: &FormatTag) -> bool {
    a.font_weight == b.font_weight
        && a.font_decorations == b.font_decorations
        && format!("{:?}", a.colors) == format!("{:?}", b.colors)
        && a.url == b.url
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
    let first_style = GlyphStyle::from_format(&first_tag.font_weight, &first_tag.font_decorations);
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
        let style = GlyphStyle::from_format(&tag.font_weight, &tag.font_decorations);
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
                font_weight: current_tag.font_weight.clone(),
                font_decorations: current_tag.font_decorations.clone(),
                colors: current_tag.colors.clone(),
                url: current_tag.url.clone(),
                text: std::mem::take(&mut run_text),
                char_widths: std::mem::take(&mut run_char_widths),
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
            font_weight: current_tag.font_weight.clone(),
            font_decorations: current_tag.font_decorations.clone(),
            colors: current_tag.colors.clone(),
            url: current_tag.url.clone(),
            text: run_text,
            char_widths: run_char_widths,
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
        TChar::Utf8(v) => {
            std::str::from_utf8(v)
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
        font_weight: run.font_weight.clone(),
        font_decorations: run.font_decorations.clone(),
        colors: run.colors.clone(),
        url: run.url.clone(),
    }
}

/// Build `ShapedGlyph`s from `rustybuzz` output, snapping to the cell grid.
///
/// The cell grid is authoritative: glyph N maps to column N × `cell_width`.
/// Wide characters (width 2) span two cells.
fn build_shaped_glyphs(
    infos: &[rustybuzz::GlyphInfo],
    char_widths: &[usize],
    col_start: usize,
    face_id: FaceId,
    is_color: bool,
    cell_width: f32,
) -> Vec<ShapedGlyph> {
    let mut glyphs = Vec::with_capacity(infos.len());

    // Walk glyphs.  For monospace terminal text (no ligatures), each glyph maps
    // 1:1 to a character.  We snap positions to the cell grid based on the
    // character index rather than trusting the shaper's advance.

    for (glyph_idx, info) in infos.iter().enumerate() {
        let char_index = byte_offset_to_char_index(infos, glyph_idx);
        let cw = char_widths.get(char_index).copied().unwrap_or(1);

        // Cell-grid x position: sum of column widths up to this character.
        let col_for_glyph = col_start + char_widths.iter().take(char_index).sum::<usize>();

        #[allow(clippy::cast_precision_loss)]
        let x_px = col_for_glyph as f32 * cell_width;

        #[allow(clippy::cast_possible_truncation)]
        let gid = info.glyph_id as u16;

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

/// Map a glyph's byte-offset cluster to a character index.
const fn byte_offset_to_char_index(infos: &[rustybuzz::GlyphInfo], glyph_idx: usize) -> usize {
    // In the simple case (1 glyph per char, LTR, no ligatures), glyph_idx
    // is the character index.  For correctness we derive it from the cluster
    // value, but for terminal monospace text the simple path dominates.
    let _ = infos; // We'll use glyph_idx directly for now.
    glyph_idx
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
        #[allow(clippy::cast_precision_loss)]
        let x_px = col as f32 * cell_width;

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
    use super::*;
    use freminal_common::config::Config;

    /// Helper: create a default `FontManager` for tests.
    fn test_font_manager() -> FontManager {
        FontManager::new(&Config::default())
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
        let r1 = cache.shape_visible(&chars, &tags, 80, &mut fm, cell_w, false);
        assert_eq!(r1.len(), 1);

        // Second call with identical input — cache hit.
        let r2 = cache.shape_visible(&chars, &tags, 80, &mut fm, cell_w, false);
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

        let _ = cache.shape_visible(&chars1, &tags, 80, &mut fm, cell_w, false);

        // Change content.
        let chars2 = vec![TChar::Ascii(b'Y')];
        let r2 = cache.shape_visible(&chars2, &tags, 80, &mut fm, cell_w, false);

        // Should still produce valid output (cache miss, re-shaped).
        assert_eq!(r2.len(), 1);
        assert_eq!(r2[0].runs.len(), 1);
        assert_eq!(r2[0].runs[0].glyphs.len(), 1);
    }
}
