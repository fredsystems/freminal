// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Run segmentation and text shaping via `rustybuzz`.
//!
//! Splits visible terminal content into [`TextRun`] spans based on format changes
//! and font-face boundaries, then shapes each run to produce glyph IDs and advances.
//! Results are cached per-line for incremental updates.

use std::collections::HashMap;
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
    /// The source Unicode scalar this glyph was shaped from.
    ///
    /// Used to route box-drawing / block-element codepoints to the procedural
    /// renderer (Task #410). `'\0'` when the source char is unavailable (e.g.
    /// ligature clusters spanning multiple chars, or placeholder glyphs).
    pub source_char: char,
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
///
/// Subtask 124.6 (lever 1) adds a second, content-addressed cache
/// ([`RunCache`]) alongside the line cache. The line cache is keyed by line
/// index, so a one-line scroll invalidates every line even though most of
/// their runs are byte-identical to runs shaped a moment ago at a different
/// index (see the 124.16 findings block in
/// `Documents/PLAN_124_RENDER_EFFICIENCY.md`). The run cache is keyed by
/// `(face_id, ligatures, run text)` instead, so it can still hit across that
/// shift.
pub struct ShapingCache {
    /// Per-line cache: `(hash, shaped_line)`.
    entries: Vec<Option<(u64, Arc<ShapedLine>)>>,
    /// Subtask 124.16: cumulative hit/miss tally across every
    /// [`Self::shape_visible`] call since the last [`Self::reset_stats`].
    stats: ShapingCacheStats,
    /// Subtask 124.6: content-addressed run-level glyph-template cache.
    run_cache: RunCache,
}

/// Named ligature-shaping mode, used only as the `ligatures` component of
/// [`RunCacheKey`].
///
/// The `ligatures: bool` flag threaded through every shaping function
/// (`shape_visible`, `shape_runs`, `shape_single_run`, ...) predates subtask
/// 124.6 and is grandfathered as-is at every existing call site. This type
/// exists so that the run-cache *key* — new in 124.6 — never carries a bare
/// `bool`: the pre-existing flag is classified into a named domain value
/// inline at the one point it crosses into the new cache key, rather than
/// threading another bool parameter through new code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum LigatureShaping {
    /// `calt`/`liga` were enabled for this run.
    Enabled,
    /// `calt`/`liga` were disabled for this run.
    Disabled,
}

/// Content-addressed key for [`RunCache`].
///
/// Equality is structural over all three fields — deliberately not a
/// hash-only lookup, so a hash collision can never be mistaken for a real
/// match. `text` is owned because a `TextRun`'s text does not outlive the
/// `shape_visible` call that built it, while cache entries must survive
/// across calls.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct RunCacheKey {
    /// The font face the run resolved to.
    face_id: FaceId,
    /// Whether ligature features were enabled for this run. Part of the key
    /// because the same text shapes to different glyphs with `calt`/`liga`
    /// on versus off.
    ligatures: LigatureShaping,
    /// The run's UTF-8 text content.
    text: String,
}

/// Position-independent shaped-glyph template for one run, cached at the
/// canonical position `col_start = 0` and canonical pixel cell width `1.0`.
///
/// At those canonical values `build_shaped_glyphs`/`build_tofu_glyphs`
/// compute `x_px` as exactly the glyph's column offset *within the run*
/// (`0.0`, `1.0`, `2.0`, ...), independent of where the run actually sits on
/// the line or how wide a cell is in pixels. Rebasing to a concrete
/// `col_start`/`cell_width` is then just
/// `(template_x_px + col_start) * cell_width` — see
/// [`build_shaped_run_from_template`].
///
/// `Arc<[ShapedGlyph]>`, not `Vec<ShapedGlyph>`: a cache hit (current or
/// promoted-from-previous) then costs one refcount bump, never a deep clone
/// of the glyph vector. Only a genuine miss builds the `Vec` (once, in
/// [`shape_run_canonical`]) and converts it into the `Arc` that gets stored.
type GlyphTemplate = Arc<[ShapedGlyph]>;

/// Two-generation, content-addressed cache of [`GlyphTemplate`]s, owned by
/// [`ShapingCache`] (subtask 124.6, lever 1).
///
/// Bounded without an arbitrary capacity: only the current and immediately
/// previous **miss-bearing** `shape_visible` call's generation are kept. A
/// call in which every line hits the line-level cache does not rotate, so a
/// generation survives across any number of quiet calls — letting a later
/// scroll still reuse runs shaped several quiet frames earlier. A call is
/// rotated at most once, on its first line miss; every later miss within
/// the same call shares that call's `current` generation.
#[derive(Debug, Default)]
struct RunCache {
    /// Templates cached during the in-progress (or most recent) miss-bearing
    /// call.
    current: HashMap<RunCacheKey, GlyphTemplate>,
    /// Templates cached during the miss-bearing call before that. Evicted
    /// wholesale the next time [`Self::rotate`] runs.
    previous: HashMap<RunCacheKey, GlyphTemplate>,
}

impl RunCache {
    /// Age `current` into `previous`, discarding whatever was in `previous`
    /// before. Called at most once per miss-bearing `shape_visible` call.
    fn rotate(&mut self) {
        self.previous = std::mem::take(&mut self.current);
    }

    /// Drop every cached template in both generations.
    ///
    /// Needed alongside [`ShapingCache::clear`]: font rebuilds can reuse
    /// `FaceId` values for entirely different font data, and `FaceId` is
    /// part of [`RunCacheKey`] but says nothing about *which* font backs it.
    fn clear(&mut self) {
        self.current.clear();
        self.previous.clear();
    }
}

/// Subtask 124.16: per-line shaping cache hit/miss tally.
///
/// [`ShapingCache::shape_visible`] already decides, per line, whether to
/// reuse an `Arc<ShapedLine>` or re-shape from scratch, but it returned only
/// `Vec<Arc<ShapedLine>>` — the outcome was computed and thrown away. That
/// made it the fifth and last collapse point in the chain Task 124's
/// defect table lists, and the reason 124.6's two levers could not be
/// justified or sized. This surfaces it.
///
/// Counting only. Nothing here influences which branch is taken, and the
/// counters are always compiled (not feature-gated) so a measurement can
/// never diverge from the code path it claims to describe — the same
/// rationale subtask 122.5 recorded for `PaneResolution`'s diagnostic terms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ShapingCacheStats {
    /// Lines served by an `Arc` refcount bump.
    pub hits: u64,
    /// Lines re-segmented and re-shaped through rustybuzz.
    pub misses: u64,
    /// Subtask 124.6: runs served from [`RunCache`] (current or previous
    /// generation) as a template clone, without a `rustybuzz` shape call.
    /// Only counted for runs belonging to a line that missed the line-level
    /// cache — a line hit never touches the run cache at all.
    pub run_hits: u64,
    /// Subtask 124.6: runs that missed [`RunCache`] entirely and were shaped
    /// fresh through `rustybuzz`.
    pub run_misses: u64,
}

impl ShapingCacheStats {
    /// Lines considered — `hits + misses`.
    #[must_use]
    pub const fn total(self) -> u64 {
        self.hits.saturating_add(self.misses)
    }

    /// Fraction of considered lines served from cache, or `None` when no
    /// line has been considered yet.
    ///
    /// `None` rather than `0.0` deliberately: "no data" and "nothing hit"
    /// are different findings, and a measurement that silently reports the
    /// second when it means the first is worse than one that reports
    /// nothing.
    #[must_use]
    pub fn hit_rate(self) -> Option<f64> {
        let total = self.total();
        if total == 0 {
            return None;
        }
        #[allow(clippy::cast_precision_loss)]
        Some(self.hits as f64 / total as f64)
    }

    /// Runs considered by [`RunCache`] — `run_hits + run_misses`.
    ///
    /// Always `<=` the number of runs segmented from missed lines: a run
    /// belonging to a line-level cache hit is never counted here at all.
    #[must_use]
    pub const fn run_total(self) -> u64 {
        self.run_hits.saturating_add(self.run_misses)
    }

    /// Fraction of considered runs served from [`RunCache`], or `None` when
    /// no run has been considered yet. See [`Self::hit_rate`] for why `None`
    /// is distinct from `0.0`.
    #[must_use]
    pub fn run_hit_rate(self) -> Option<f64> {
        let total = self.run_total();
        if total == 0 {
            return None;
        }
        let hits: f64 = self.run_hits.approx_as::<f64>().unwrap_or(0.0);
        let total_f: f64 = total.approx_as::<f64>().unwrap_or(0.0);
        Some(hits / total_f)
    }
}

impl Default for ShapingCache {
    fn default() -> Self {
        Self::new()
    }
}

impl ShapingCache {
    /// Create a new empty shaping cache.
    ///
    /// Not `const` (unlike before subtask 124.6): [`RunCache`]'s `HashMap`
    /// fields have no `const` constructor.
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            stats: ShapingCacheStats::default(),
            run_cache: RunCache::default(),
        }
    }

    /// Invalidate the entire cache (e.g. on font change).
    ///
    /// Deliberately does **not** reset [`Self::stats`]: a font change is one
    /// of the events a hit-rate measurement most wants to see the cost of,
    /// and zeroing the tally here would hide it. Use [`Self::reset_stats`].
    ///
    /// Also clears [`Self::run_cache`] (subtask 124.6): a font rebuild can
    /// reuse the same `FaceId` values for entirely different font data, so
    /// leaving stale run-cache entries keyed on those `FaceId`s behind would
    /// serve glyph templates shaped against the old font.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.run_cache.clear();
    }

    /// Subtask 124.16: the cumulative hit/miss tally.
    #[must_use]
    pub const fn stats(&self) -> ShapingCacheStats {
        self.stats
    }

    /// Subtask 124.16: zero the hit/miss tally, leaving cached lines (and,
    /// per subtask 124.6, cached runs) intact.
    ///
    /// Separate from [`Self::clear`] so a measurement can prime the cache and
    /// then count only the frames it cares about.
    pub const fn reset_stats(&mut self) {
        self.stats = ShapingCacheStats {
            hits: 0,
            misses: 0,
            run_hits: 0,
            run_misses: 0,
        };
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

        // Built once per call rather than once per line: the feature list is
        // identical for every run sharing this `ligatures` setting.
        let features = shaping_features(ligatures);

        let mut result = Vec::with_capacity(line_count);

        // Track the character offset into the global flat array for tag lookup.
        let mut global_offset: usize = 0;

        // Subtask 124.6: whether `self.run_cache` has already rotated for
        // THIS call. A call rotates at most once, on its first line miss —
        // a call where every line hits (e.g. an identical full-screen
        // redraw) never touches this and never rotates, so a generation
        // survives across any number of quiet calls.
        let mut run_cache_rotated = false;

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
                self.stats.hits = self.stats.hits.saturating_add(1);
                Arc::clone(shaped_line)
            } else {
                // Cache miss — segment and shape.
                self.stats.misses = self.stats.misses.saturating_add(1);
                if !run_cache_rotated {
                    self.run_cache.rotate();
                    run_cache_rotated = true;
                }
                let runs = segment_line(
                    line_chars,
                    visible_tags,
                    global_offset,
                    term_width,
                    font_manager,
                );
                let shaped_runs = self.shape_runs_via_run_cache(
                    &runs,
                    font_manager,
                    cell_width,
                    ligatures,
                    &features,
                );
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

    /// Shape `runs` (all belonging to one line-cache-missed line), consulting
    /// [`Self::run_cache`] per run instead of shaping every one from scratch.
    ///
    /// Subtask 124.6, lever 1. Only ever called from the line-cache-miss
    /// branch of [`Self::shape_visible`] — a run belonging to a line that hit
    /// the line-level cache never reaches here, since the whole
    /// `Arc<ShapedLine>` (all its runs included) is reused by refcount bump.
    fn shape_runs_via_run_cache(
        &mut self,
        runs: &[TextRun],
        font_manager: &FontManager,
        cell_width: f32,
        ligatures: bool,
        features: &[rustybuzz::Feature],
    ) -> Vec<ShapedRun> {
        runs.iter()
            .map(|run| {
                self.shape_run_via_run_cache(run, font_manager, cell_width, ligatures, features)
            })
            .collect()
    }

    /// Shape a single run via [`Self::run_cache`], falling back to a fresh
    /// `rustybuzz` shape on a genuine miss.
    fn shape_run_via_run_cache(
        &mut self,
        run: &TextRun,
        font_manager: &FontManager,
        cell_width: f32,
        ligatures: bool,
        features: &[rustybuzz::Feature],
    ) -> ShapedRun {
        let key = RunCacheKey {
            face_id: run.face_id,
            ligatures: if ligatures {
                LigatureShaping::Enabled
            } else {
                LigatureShaping::Disabled
            },
            text: run.text.clone(),
        };

        let template = if let Some(template) = self.run_cache_lookup(&key) {
            template
        } else {
            self.stats.run_misses = self.stats.run_misses.saturating_add(1);
            let glyphs = shape_run_canonical(run, font_manager, ligatures, features);
            let template: GlyphTemplate = Arc::from(glyphs);
            self.run_cache.current.insert(key, Arc::clone(&template));
            template
        };

        build_shaped_run_from_template(run, &template, cell_width)
    }

    /// Look up `key` in [`Self::run_cache`]: current generation first (so
    /// duplicate runs within the same call — e.g. two identical rows in one
    /// frame — hit each other), then the previous generation. A previous-
    /// generation hit is promoted into the current generation (an `Arc`
    /// clone, not a deep copy) so it survives the *next* rotation too, per
    /// [`RunCache`]'s two-generation bound.
    ///
    /// Returns `None` on a genuine miss in both generations; the caller is
    /// responsible for shaping and inserting in that case (kept out of this
    /// method so it doesn't need `font_manager`/`features` just to record a
    /// hit).
    fn run_cache_lookup(&mut self, key: &RunCacheKey) -> Option<GlyphTemplate> {
        if let Some(template) = self.run_cache.current.get(key) {
            self.stats.run_hits = self.stats.run_hits.saturating_add(1);
            return Some(Arc::clone(template));
        }
        if let Some(template) = self.run_cache.previous.get(key) {
            self.stats.run_hits = self.stats.run_hits.saturating_add(1);
            let template = Arc::clone(template);
            self.run_cache
                .current
                .insert(key.clone(), Arc::clone(&template));
            return Some(template);
        }
        None
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
///
/// `features` is built once by the caller (see [`ShapingCache::shape_visible`])
/// rather than once per run, since it is identical for every run sharing the
/// same `ligatures` setting.
fn shape_runs(
    runs: &[TextRun],
    font_manager: &FontManager,
    cell_width: f32,
    ligatures: bool,
    features: &[rustybuzz::Feature],
) -> Vec<ShapedRun> {
    runs.iter()
        .map(|run| shape_single_run(run, font_manager, cell_width, ligatures, features))
        .collect()
}

/// Shape a single line of synthetic text in a uniform color.
///
/// Used by the widget layer to build fold-placeholder rows (Task 72.10b-3):
/// rows that are not present in the snapshot buffer but need to render real
/// glyphs through the same shaping pipeline as buffer text so atlas
/// rasterisation, font fallback, and ligature features behave identically.
///
/// `text` is treated as a single contiguous run with the given foreground
/// color, normal weight, and no decorations.  Per-character cell widths are
/// computed via `char::width_cjk` semantics (ASCII = 1, wide = 2).  Face
/// resolution falls back to the primary face for any character whose
/// natural face cannot be determined — sufficient for the ASCII +
/// triangle (`▶`) + ellipsis (`…`) glyphs used by the placeholder UI.
///
/// Returns a fully-shaped [`ShapedLine`] with one [`ShapedRun`] per
/// face-segmented sub-span, mirroring what `shape_visible` would produce
/// for the same text in the buffer.
#[must_use]
pub fn shape_placeholder_line(
    text: &str,
    fg: freminal_common::colors::TerminalColor,
    font_manager: &mut FontManager,
    cell_width: f32,
    ligatures: bool,
) -> ShapedLine {
    use freminal_common::buffer_states::cursor::{ReverseVideo, StateColors};

    if text.is_empty() {
        return ShapedLine {
            runs: Vec::new(),
            line_width: LineWidth::Normal,
        };
    }

    let colors = StateColors {
        color: fg,
        background_color: freminal_common::colors::TerminalColor::DefaultBackground,
        underline_color: freminal_common::colors::TerminalColor::DefaultUnderlineColor,
        reverse_video: ReverseVideo::Off,
    };

    // Segment the text into runs by font-face boundary so glyph fallback
    // works for non-ASCII placeholder glyphs (▶, …).  All other format
    // attributes are uniform across the line.
    let style = GlyphStyle::from_format(&FontWeight::Normal, FontDecorationFlags::empty());
    let chars: Vec<char> = text.chars().collect();
    let char_widths: Vec<usize> = chars
        .iter()
        .map(|c| {
            use unicode_width::UnicodeWidthChar;
            UnicodeWidthChar::width(*c).unwrap_or(1).max(1)
        })
        .collect();

    let mut runs: Vec<TextRun> = Vec::new();
    let mut run_text = String::new();
    let mut run_char_widths: Vec<usize> = Vec::new();
    let mut run_col_start: usize = 0;
    let mut run_col_count: usize = 0;
    let mut current_face = {
        let (face, _) = font_manager.resolve_glyph(chars[0], style);
        face
    };

    for (i, &ch) in chars.iter().enumerate() {
        let (face, _) = font_manager.resolve_glyph(ch, style);
        if face != current_face && !run_text.is_empty() {
            runs.push(TextRun {
                col_start: run_col_start,
                col_count: run_col_count,
                face_id: current_face,
                style,
                font_weight: FontWeight::Normal,
                font_decorations: FontDecorationFlags::empty(),
                colors,
                url: None,
                text: std::mem::take(&mut run_text),
                char_widths: std::mem::take(&mut run_char_widths),
                blink: BlinkState::None,
            });
            run_col_start += run_col_count;
            run_col_count = 0;
            current_face = face;
        }
        run_text.push(ch);
        run_char_widths.push(char_widths[i]);
        run_col_count += char_widths[i];
    }

    if !run_text.is_empty() {
        runs.push(TextRun {
            col_start: run_col_start,
            col_count: run_col_count,
            face_id: current_face,
            style,
            font_weight: FontWeight::Normal,
            font_decorations: FontDecorationFlags::empty(),
            colors,
            url: None,
            text: run_text,
            char_widths: run_char_widths,
            blink: BlinkState::None,
        });
    }

    let features = shaping_features(ligatures);
    let shaped_runs = shape_runs(&runs, font_manager, cell_width, ligatures, &features);
    ShapedLine {
        runs: shaped_runs,
        line_width: LineWidth::Normal,
    }
}

/// Shape a single `TextRun` via `rustybuzz`.
///
/// Composed from [`shape_run_canonical`] (the actual `rustybuzz` call, at
/// the canonical `col_start = 0` / pixel `cell_width = 1.0` position) and
/// [`build_shaped_run_from_template`] (rebasing to `run`'s real position and
/// attaching its metadata). This is the same composition
/// [`ShapingCache::shape_run_via_run_cache`] uses on a run-cache miss, so the
/// two paths cannot silently diverge — this one just never consults or
/// populates the cache, since callers of this function (placeholder-line
/// shaping, tests) have no `ShapingCache` to share it through (subtask
/// 124.6's point 5: no global/static cache state).
fn shape_single_run(
    run: &TextRun,
    font_manager: &FontManager,
    cell_width: f32,
    ligatures: bool,
    features: &[rustybuzz::Feature],
) -> ShapedRun {
    let template = shape_run_canonical(run, font_manager, ligatures, features);
    build_shaped_run_from_template(run, &template, cell_width)
}

/// Shape `run` via `rustybuzz` at the canonical position `col_start = 0` and
/// canonical pixel cell width `1.0`, producing glyph identity data whose
/// `x_px` is exactly the glyph's column offset *within the run*, independent
/// of where the run actually sits or how wide a cell is in pixels.
///
/// Returns a plain `Vec`, not a [`GlyphTemplate`]: this is the one-time build
/// on a run-cache miss (or the whole computation on the uncached path), and
/// the caller decides whether/how to wrap it in the `Arc` a [`GlyphTemplate`]
/// requires. Converting here unconditionally would force an `Arc` allocation
/// even on [`shape_single_run`]'s uncached path, which never touches the
/// cache at all.
///
/// This is the shaping half used by both the uncached path
/// ([`shape_single_run`]) and the run-cache-miss path
/// ([`ShapingCache::shape_run_via_run_cache`]) — see
/// [`build_shaped_run_from_template`] for the rebasing half.
fn shape_run_canonical(
    run: &TextRun,
    font_manager: &FontManager,
    ligatures: bool,
    features: &[rustybuzz::Feature],
) -> Vec<ShapedGlyph> {
    let is_emoji_face = run.face_id == FaceId::Emoji;

    // Build the input buffer and guess its segment properties (script,
    // direction) exactly as `rustybuzz::shape()` does internally — this is
    // required up front because `shape_cached` needs a concrete
    // script/direction to look up (or build) the matching cached
    // `ShapePlan` before it can shape.
    let mut buffer = rustybuzz::UnicodeBuffer::new();
    buffer.push_str(&run.text);
    buffer.guess_segment_properties();

    // Try to shape via the cached Face + ShapePlan (Task #430).
    font_manager
        .shape_cached(run.face_id, ligatures, features, buffer)
        .map_or_else(
            || {
                // No face available — produce tofu (glyph_id=0) per character.
                build_tofu_glyphs(&run.char_widths, 0, run.face_id, 1.0)
            },
            |output| {
                let infos = output.glyph_infos();

                // Map shaped glyphs back to canonical column offsets.
                build_shaped_glyphs(
                    infos,
                    &run.text,
                    &run.char_widths,
                    0,
                    run.face_id,
                    is_emoji_face,
                    1.0,
                )
            },
        )
}

/// Rebase a canonical [`GlyphTemplate`] (`col_start = 0`, pixel
/// `cell_width = 1.0`) onto `run`'s real column position and pixel cell
/// width, and attach `run`'s current metadata (style, colors, URL, blink,
/// `col_start`).
///
/// The metadata always comes from `run` — the caller — never from whatever
/// run originally produced the template. That is what makes a run-cache hit
/// safe: `RunCacheKey` covers only `(face_id, ligatures, text)`, so a hit can
/// legitimately reuse glyph *shapes* from a differently-styled or
/// differently-positioned prior run, but must never leak that prior run's
/// style/color/URL/blink/position.
fn build_shaped_run_from_template(
    run: &TextRun,
    template: &[ShapedGlyph],
    cell_width: f32,
) -> ShapedRun {
    let col_start_f: f32 = run.col_start.approx_as::<f32>().unwrap_or(0.0);
    let glyphs = template
        .iter()
        .map(|g| ShapedGlyph {
            x_px: (g.x_px + col_start_f) * cell_width,
            ..g.clone()
        })
        .collect();

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

    // Char-index → source char, for routing box-drawing codepoints to the
    // procedural renderer (Task #410).
    let run_chars: Vec<char> = run_text.chars().collect();

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

        // Source char, but only for single-char clusters — a ligature spanning
        // multiple chars has no single source char and must not be treated as
        // procedural.
        let source_char = if next_char_idx == char_idx + 1 {
            run_chars.get(char_idx).copied().unwrap_or('\0')
        } else {
            '\0'
        };

        glyphs.push(ShapedGlyph {
            glyph_id: gid,
            x_px,
            y_offset: 0.0, // Horizontal text: y offset is rarely nonzero.
            face_id,
            is_color,
            cell_width: cw,
            source_char,
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
            source_char: '\0',
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

    /// Helper: shape `runs`, building the feature list internally (mirroring
    /// what `ShapingCache::shape_visible` does once per call in production).
    fn shape_runs_test(
        runs: &[TextRun],
        fm: &FontManager,
        cell_w: f32,
        ligatures: bool,
    ) -> Vec<ShapedRun> {
        let features = shaping_features(ligatures);
        shape_runs(runs, fm, cell_w, ligatures, &features)
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

    /// Helper: build a minimal `TextRun` with default metadata, for the
    /// run-cache unit tests (subtask 124.6). Each character is 1 cell wide.
    fn make_run(text: &str, col_start: usize, face_id: FaceId) -> TextRun {
        let width = text.chars().count();
        TextRun {
            col_start,
            col_count: width,
            face_id,
            style: GlyphStyle::from_format(&FontWeight::Normal, FontDecorationFlags::empty()),
            font_weight: FontWeight::Normal,
            font_decorations: FontDecorationFlags::empty(),
            colors: freminal_common::buffer_states::cursor::StateColors::default(),
            url: None,
            text: text.to_string(),
            char_widths: vec![1; width],
            blink: BlinkState::None,
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
        let shaped = shape_runs_test(&runs, &fm, cell_w, false);

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
        let shaped = shape_runs_test(&runs, &fm, cell_w, false);

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

        // U+1F600 (😀) must route to the emoji face. With the bundled Noto
        // Color Emoji floor (Task #402) there is always an emoji face, so this
        // is now deterministic rather than environment-dependent.
        let chars = vec![TChar::from('😀')];
        let tags = vec![make_tag(0, 10)];

        let runs = segment_line(&chars, &tags, 0, 80, &mut fm);
        assert!(
            runs.iter().any(|r| r.face_id == FaceId::Emoji),
            "emoji must resolve to FaceId::Emoji (bundled Noto floor guarantees one)"
        );

        let shaped = shape_runs_test(&runs, &fm, cell_w, false);
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

    // -- Run cache (subtask 124.6, lever 1) --
    //
    // These drive `ShapingCache::shape_run_via_run_cache` directly (rather
    // than through `shape_visible`) to pin the run cache's key semantics and
    // generation lifecycle precisely, independent of line-level segmentation.
    // The end-to-end integration — that `shape_visible` rotates a generation
    // only on a genuine line miss — is covered separately by
    // `shaping_cache_hit_rate::a_scroll_by_one_line_reuses_unchanged_runs`.

    #[test]
    fn run_cache_hits_on_identical_key() {
        let fm = test_font_manager();
        let features = shaping_features(false);
        let mut cache = ShapingCache::new();
        let run = make_run("hello", 0, FaceId::PrimaryRegular);

        let _ = cache.shape_run_via_run_cache(&run, &fm, 10.0, false, &features);
        assert_eq!(cache.stats().run_misses, 1);
        assert_eq!(cache.stats().run_hits, 0);

        let _ = cache.shape_run_via_run_cache(&run, &fm, 10.0, false, &features);
        let stats = cache.stats();
        assert_eq!(
            stats.run_hits, 1,
            "identical (face_id, ligatures, text) key must hit"
        );
        assert_eq!(stats.run_misses, 1);
    }

    #[test]
    fn run_cache_misses_when_face_id_differs() {
        let fm = test_font_manager();
        let features = shaping_features(false);
        let mut cache = ShapingCache::new();

        let run_a = make_run("hello", 0, FaceId::PrimaryRegular);
        let run_b = make_run("hello", 0, FaceId::PrimaryBold);

        let _ = cache.shape_run_via_run_cache(&run_a, &fm, 10.0, false, &features);
        let _ = cache.shape_run_via_run_cache(&run_b, &fm, 10.0, false, &features);

        let stats = cache.stats();
        assert_eq!(
            stats.run_misses, 2,
            "a different face_id must not hit the other face's entry"
        );
        assert_eq!(stats.run_hits, 0);
    }

    #[test]
    fn run_cache_misses_when_ligatures_differs() {
        let fm = test_font_manager();
        let features_off = shaping_features(false);
        let features_on = shaping_features(true);
        let mut cache = ShapingCache::new();

        let run = make_run("->", 0, FaceId::PrimaryRegular);

        let _ = cache.shape_run_via_run_cache(&run, &fm, 10.0, false, &features_off);
        let _ = cache.shape_run_via_run_cache(&run, &fm, 10.0, true, &features_on);

        let stats = cache.stats();
        assert_eq!(
            stats.run_misses, 2,
            "the ligatures flag must be part of the run-cache key"
        );
        assert_eq!(stats.run_hits, 0);
    }

    #[test]
    fn run_cache_misses_when_text_differs() {
        let fm = test_font_manager();
        let features = shaping_features(false);
        let mut cache = ShapingCache::new();

        let run_a = make_run("hello", 0, FaceId::PrimaryRegular);
        let run_b = make_run("world", 0, FaceId::PrimaryRegular);

        let _ = cache.shape_run_via_run_cache(&run_a, &fm, 10.0, false, &features);
        let _ = cache.shape_run_via_run_cache(&run_b, &fm, 10.0, false, &features);

        let stats = cache.stats();
        assert_eq!(stats.run_misses, 2, "different run text must not hit");
        assert_eq!(stats.run_hits, 0);
    }

    #[test]
    fn run_cache_hit_never_leaks_prior_metadata() {
        use freminal_common::buffer_states::fonts::FontDecorations;

        let fm = test_font_manager();
        let features = shaping_features(false);
        let mut cache = ShapingCache::new();
        let cell_w = 10.0;

        let run_a = make_run("hi", 0, FaceId::PrimaryRegular);
        let _ = cache.shape_run_via_run_cache(&run_a, &fm, cell_w, false, &features);
        assert_eq!(cache.stats().run_misses, 1);

        // Same (face_id, ligatures, text) as `run_a`, but every field NOT
        // covered by `RunCacheKey` differs: style, font_weight, colors,
        // decorations, URL, blink, and col_start.
        let mut decorations = FontDecorationFlags::empty();
        decorations.insert(FontDecorations::Underline);
        let style_b = GlyphStyle::from_format(&FontWeight::Bold, decorations);
        let run_b = TextRun {
            col_start: 7,
            style: style_b,
            font_weight: FontWeight::Bold,
            colors: freminal_common::buffer_states::cursor::StateColors {
                color: freminal_common::colors::TerminalColor::Blue,
                ..Default::default()
            },
            font_decorations: decorations,
            url: Some(Arc::new(freminal_common::buffer_states::url::Url {
                id: None,
                url: "https://example.invalid".to_string(),
            })),
            blink: BlinkState::Slow,
            ..make_run("hi", 7, FaceId::PrimaryRegular)
        };

        let shaped_b = cache.shape_run_via_run_cache(&run_b, &fm, cell_w, false, &features);
        let stats = cache.stats();
        assert_eq!(
            stats.run_hits, 1,
            "same (face_id, ligatures, text) must hit the run cache"
        );
        assert_eq!(
            stats.run_misses, 1,
            "a run-cache hit must not call rustybuzz again"
        );

        // The hit must carry run_b's OWN metadata, never run_a's.
        assert_eq!(shaped_b.style, style_b);
        assert_eq!(shaped_b.font_weight, FontWeight::Bold);
        assert_eq!(
            shaped_b.colors.color,
            freminal_common::colors::TerminalColor::Blue
        );
        assert_eq!(shaped_b.font_decorations, decorations);
        assert_eq!(shaped_b.blink, BlinkState::Slow);
        assert_eq!(
            shaped_b.url.as_ref().map(|u| u.url.as_str()),
            Some("https://example.invalid")
        );
        assert_eq!(shaped_b.col_start, 7);

        // And the glyph position must be rebased to run_b's col_start (7),
        // never reused verbatim from run_a's col_start (0). Exact equality
        // is correct (not a tolerance): this is the identical
        // `(template_x_px + col_start) * cell_width` arithmetic performed
        // in `build_shaped_run_from_template`.
        let expected_x = 7.0 * cell_w;
        #[allow(clippy::float_cmp)]
        {
            assert_eq!(
                shaped_b.glyphs[0].x_px, expected_x,
                "glyph position must rebase to the new col_start"
            );
        }
    }

    // -- Differential regression: cached-hit output vs. uncached
    // `shape_single_run` output, field-by-field (subtask 124.6 review fix) --
    //
    // These compare two independently-computed PRODUCTION outputs rather
    // than reimplementing any shaping math: `expected` always comes from
    // `shape_single_run` (the uncached path, never touches `RunCache`), and
    // `actual` always comes from a genuine run-cache HIT that rebases a
    // template originally built for a DIFFERENT run (different col_start
    // and, where noted, a different pixel `cell_width`). Exact equality
    // (`==`, not a tolerance) is correct here because both paths perform the
    // identical `(template_x_px + col_start) * cell_width` arithmetic in
    // `build_shaped_run_from_template` — any mismatch is a real defect.

    /// Assert two `ShapedGlyph`s are identical in every field.
    fn assert_glyph_fields_eq(actual: &ShapedGlyph, expected: &ShapedGlyph, ctx: &str) {
        assert_eq!(actual.glyph_id, expected.glyph_id, "{ctx}: glyph_id");
        assert_eq!(actual.x_px, expected.x_px, "{ctx}: x_px");
        assert_eq!(actual.y_offset, expected.y_offset, "{ctx}: y_offset");
        assert_eq!(actual.face_id, expected.face_id, "{ctx}: face_id");
        assert_eq!(actual.is_color, expected.is_color, "{ctx}: is_color");
        assert_eq!(actual.cell_width, expected.cell_width, "{ctx}: cell_width");
        assert_eq!(
            actual.source_char, expected.source_char,
            "{ctx}: source_char"
        );
    }

    /// Assert two `ShapedRun`s (produced via different code paths) are
    /// identical: every metadata field, and every glyph in order.
    fn assert_shaped_runs_eq(actual: &ShapedRun, expected: &ShapedRun, ctx: &str) {
        assert_eq!(actual.col_start, expected.col_start, "{ctx}: col_start");
        assert_eq!(actual.style, expected.style, "{ctx}: style");
        assert_eq!(
            actual.font_weight, expected.font_weight,
            "{ctx}: font_weight"
        );
        assert_eq!(
            actual.font_decorations, expected.font_decorations,
            "{ctx}: font_decorations"
        );
        assert_eq!(actual.colors, expected.colors, "{ctx}: colors");
        assert_eq!(actual.url, expected.url, "{ctx}: url");
        assert_eq!(actual.blink, expected.blink, "{ctx}: blink");
        assert_eq!(
            actual.glyphs.len(),
            expected.glyphs.len(),
            "{ctx}: glyph count"
        );
        for (i, (a, e)) in actual.glyphs.iter().zip(expected.glyphs.iter()).enumerate() {
            assert_glyph_fields_eq(a, e, &format!("{ctx}: glyph {i}"));
        }
    }

    #[test]
    fn run_cache_hit_matches_uncached_shape_for_ascii_at_nonzero_position() {
        let fm = test_font_manager();
        let features = shaping_features(false);
        let mut cache = ShapingCache::new();

        // Populate the cache from a run at one position and pixel cell width...
        let run_a = make_run("hello world", 2, FaceId::PrimaryRegular);
        let _ = cache.shape_run_via_run_cache(&run_a, &fm, 8.0, false, &features);

        // ...then hit it from a DIFFERENT run (same face/ligatures/text) at a
        // different col_start and a different (non-1.0) pixel cell_width.
        let run_b = make_run("hello world", 17, FaceId::PrimaryRegular);
        let cell_w_b = 13.5;
        let actual = cache.shape_run_via_run_cache(&run_b, &fm, cell_w_b, false, &features);
        assert_eq!(
            cache.stats().run_hits,
            1,
            "run_b must hit run_a's cached template"
        );

        let expected = shape_single_run(&run_b, &fm, cell_w_b, false, &features);
        assert_shaped_runs_eq(&actual, &expected, "ascii nonzero col_start/cell_width");
    }

    #[test]
    fn run_cache_hit_matches_uncached_shape_for_ligature_text() {
        let fm = test_font_manager();
        let features = shaping_features(true);
        let mut cache = ShapingCache::new();

        let run_a = make_run("->", 0, FaceId::PrimaryRegular);
        let _ = cache.shape_run_via_run_cache(&run_a, &fm, 10.0, true, &features);

        let run_b = make_run("->", 4, FaceId::PrimaryRegular);
        let cell_w_b = 12.0;
        let actual = cache.shape_run_via_run_cache(&run_b, &fm, cell_w_b, true, &features);
        assert_eq!(
            cache.stats().run_hits,
            1,
            "run_b must hit run_a's cached ligature template"
        );

        let expected = shape_single_run(&run_b, &fm, cell_w_b, true, &features);
        assert_shaped_runs_eq(&actual, &expected, "ligature text with ligatures enabled");
    }

    #[test]
    fn run_cache_hit_matches_uncached_shape_for_wide_unicode_text() {
        let fm = test_font_manager();
        let features = shaping_features(false);
        let mut cache = ShapingCache::new();

        // U+4E2D (中): a single wide CJK character, display_width = 2. The
        // shared `make_run` helper assumes 1-wide chars, so build these
        // directly with the correct `char_widths`/`col_count`.
        let run_a = TextRun {
            char_widths: vec![2],
            col_count: 2,
            ..make_run("中", 3, FaceId::PrimaryRegular)
        };
        let _ = cache.shape_run_via_run_cache(&run_a, &fm, 9.0, false, &features);

        let run_b = TextRun {
            char_widths: vec![2],
            col_count: 2,
            ..make_run("中", 11, FaceId::PrimaryRegular)
        };
        let cell_w_b = 14.25;
        let actual = cache.shape_run_via_run_cache(&run_b, &fm, cell_w_b, false, &features);
        assert_eq!(
            cache.stats().run_hits,
            1,
            "run_b must hit run_a's cached wide-glyph template"
        );

        let expected = shape_single_run(&run_b, &fm, cell_w_b, false, &features);
        assert_shaped_runs_eq(&actual, &expected, "wide unicode cluster/cell widths");

        // Sanity: this genuinely exercises the wide-glyph path (not a
        // silent fallback to width 1).
        assert_eq!(expected.glyphs.len(), 1);
        assert_eq!(
            expected.glyphs[0].cell_width, 2,
            "wide char must occupy 2 cells"
        );
    }

    #[test]
    fn run_cache_quiet_lookups_do_not_evict_prior_generation() {
        let fm = test_font_manager();
        let features = shaping_features(false);
        let mut cache = ShapingCache::new();
        let run = make_run("alpha", 0, FaceId::PrimaryRegular);

        // "Call 1": miss-bearing — rotates once, then shapes `run` into the
        // new current generation.
        cache.run_cache.rotate();
        let _ = cache.shape_run_via_run_cache(&run, &fm, 10.0, false, &features);
        assert_eq!(cache.stats().run_misses, 1);

        // "Call 2": quiet. A quiet `shape_visible` call never touches
        // `run_cache` at all (every line hits the line-level cache), so it
        // is modeled here by doing nothing — no `rotate()`, no lookup.

        // "Call 3": miss-bearing again. Because call 2 never rotated, `run`'s
        // template must still be reachable: it becomes `previous` when call
        // 3 rotates `current` (still holding it from call 1).
        cache.run_cache.rotate();
        let _ = cache.shape_run_via_run_cache(&run, &fm, 10.0, false, &features);
        let stats = cache.stats();
        assert_eq!(
            stats.run_hits, 1,
            "a generation must survive an intervening quiet call"
        );
        assert_eq!(stats.run_misses, 1);
    }

    #[test]
    fn run_cache_third_miss_bearing_generation_evicts_the_first() {
        let fm = test_font_manager();
        let features = shaping_features(false);
        let mut cache = ShapingCache::new();
        let run = make_run("alpha", 0, FaceId::PrimaryRegular);
        let other = make_run("beta", 0, FaceId::PrimaryRegular);

        // Generation 1: shape `run`.
        cache.run_cache.rotate();
        let _ = cache.shape_run_via_run_cache(&run, &fm, 10.0, false, &features);

        // Generation 2: shape something else. `run` is not looked up here,
        // so it survives only as the (now) `previous` generation.
        cache.run_cache.rotate();
        let _ = cache.shape_run_via_run_cache(&other, &fm, 10.0, false, &features);

        // Generation 3: rotates again. Generation 1 (which held `run`) is now
        // neither `current` nor `previous` — bounded to two generations, it
        // must be gone.
        cache.run_cache.rotate();
        let _ = cache.shape_run_via_run_cache(&run, &fm, 10.0, false, &features);

        let stats = cache.stats();
        assert_eq!(
            stats.run_misses, 3,
            "generation 1's entry for `run` must be evicted by generation 3's rotation"
        );
        assert_eq!(stats.run_hits, 0);
    }

    #[test]
    fn run_cache_duplicate_runs_in_the_same_call_hit_the_current_generation() {
        let fm = test_font_manager();
        let features = shaping_features(false);
        let mut cache = ShapingCache::new();
        let run_row1 = make_run("same text", 0, FaceId::PrimaryRegular);
        let run_row2 = make_run("same text", 0, FaceId::PrimaryRegular);

        cache.run_cache.rotate();
        let _ = cache.shape_run_via_run_cache(&run_row1, &fm, 10.0, false, &features);
        let _ = cache.shape_run_via_run_cache(&run_row2, &fm, 10.0, false, &features);

        let stats = cache.stats();
        assert_eq!(
            stats.run_hits, 1,
            "the second identical row must hit the first row's entry, \
             freshly inserted into the SAME call's current generation"
        );
        assert_eq!(stats.run_misses, 1);
    }

    #[test]
    fn clear_invalidates_run_cache() {
        let fm = test_font_manager();
        let features = shaping_features(false);
        let mut cache = ShapingCache::new();
        let run = make_run("hello", 0, FaceId::PrimaryRegular);

        cache.run_cache.rotate();
        let _ = cache.shape_run_via_run_cache(&run, &fm, 10.0, false, &features);
        let _ = cache.shape_run_via_run_cache(&run, &fm, 10.0, false, &features);
        assert_eq!(cache.stats().run_hits, 1);

        cache.clear();

        // Subtask 124.6: `clear()` must drop both run-cache generations, not
        // just the line cache — a font rebuild can reuse a `FaceId` for
        // entirely different font data.
        let _ = cache.shape_run_via_run_cache(&run, &fm, 10.0, false, &features);
        assert_eq!(
            cache.stats().run_misses,
            2,
            "clear() must invalidate cached run templates"
        );
    }

    #[test]
    fn reset_stats_preserves_run_cache_entries() {
        let fm = test_font_manager();
        let features = shaping_features(false);
        let mut cache = ShapingCache::new();
        let run = make_run("hello", 0, FaceId::PrimaryRegular);

        cache.run_cache.rotate();
        let _ = cache.shape_run_via_run_cache(&run, &fm, 10.0, false, &features);
        assert_eq!(cache.stats().run_misses, 1);

        cache.reset_stats();
        assert_eq!(cache.stats(), ShapingCacheStats::default());

        // The cached template must still be there — reset_stats zeroes only
        // the tally, per its own contract (mirroring the line cache).
        let _ = cache.shape_run_via_run_cache(&run, &fm, 10.0, false, &features);
        let stats = cache.stats();
        assert_eq!(
            stats.run_hits, 1,
            "reset_stats must not clear cached run templates"
        );
        assert_eq!(stats.run_misses, 0);
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

    // -- Bundled-font ligature smoke test (Task 111.6) --

    /// Shape `text` as a single same-format ASCII run through the REAL bundled
    /// font and return the sequence of shaped glyph IDs.
    ///
    /// Unlike the `build_glyphs_*` tests (which feed synthetic `GlyphInfo`
    /// fixtures), this drives the actual shaping pipeline against the bundled
    /// default face, so it observes whether the font's `calt` table actually
    /// rewrites glyphs for ligating sequences.
    fn bundled_glyph_ids(text: &str, ligatures: bool) -> Vec<u16> {
        let mut fm = test_font_manager();
        #[allow(clippy::cast_precision_loss)]
        let cell_w = fm.cell_width() as f32;

        let chars: Vec<TChar> = text.bytes().map(TChar::Ascii).collect();
        let tags = vec![make_tag(0, text.len())];

        let runs = segment_line(&chars, &tags, 0, 80, &mut fm);
        // A same-format ASCII run must stay a single run so a ligature can form.
        assert_eq!(runs.len(), 1, "`{text}` should be one run");

        let shaped = shape_runs_test(&runs, &fm, cell_w, ligatures);
        assert_eq!(shaped.len(), 1);
        shaped[0].glyphs.iter().map(|g| g.glyph_id).collect()
    }

    /// Regression guard: the bundled default font must actually form ligatures
    /// when `ligatures = true`.
    ///
    /// This is the test that would have caught the `MesloLGS`
    /// "ligature-feature-on-but-font-has-no-ligatures" bug that motivated the
    /// `CaskaydiaCove` swap (Task 111). It loads the *real* bundled face and
    /// asserts that known ligating sequences are shaped to a *different* set of
    /// glyphs with ligatures on than with ligatures off.
    ///
    /// `CaskaydiaCove` (like `Cascadia Code`) implements its coding ligatures via
    /// `calt` chaining-contextual substitution into dedicated "ligature piece"
    /// glyphs rather than a many-to-one ligature collapse — so the glyph
    /// *count* is unchanged but the glyph *IDs* change. Asserting on
    /// "IDs differ" (not on specific IDs, which are font-version-specific, nor
    /// on glyph-count reduction, which this font does not do) is the
    /// non-brittle way to prove the feature fired.
    ///
    /// It fails if the bundled font is ever swapped back to a non-ligating face
    /// such as `MesloLGS`, or to the `CaskaydiaMono` variant (which strips
    /// `calt`): in those fonts the glyphs are identical regardless of the
    /// ligature feature flag.
    #[test]
    fn bundled_font_forms_ligatures() {
        for seq in ["->", "=>", "==="] {
            let with_lig = bundled_glyph_ids(seq, true);
            let without_lig = bundled_glyph_ids(seq, false);

            // The ligature feature must have changed the shaping output. If the
            // bundled font has no `calt` coverage (Meslo, CaskaydiaMono), the
            // two would be identical.
            assert_ne!(
                with_lig, without_lig,
                "`{seq}`: bundled font shaped to identical glyphs with \
                 ligatures ON ({with_lig:?}) and OFF ({without_lig:?}) — the \
                 `calt` feature did not fire. The bundled font may have lost \
                 its ligature coverage."
            );
        }
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

    // --- Task #430: shape_with_plan output-identity vs the old shape() path ---

    /// One shaped glyph's full identity: glyph id, source cluster, and the
    /// positional data (advances + offsets). Positions are included because
    /// `kern` is always force-enabled in the feature list, so a plan-caching
    /// regression could change advances/offsets without changing glyph ids —
    /// comparing ids alone would miss it.
    type GlyphIdentity = (u16, u32, i32, i32, i32, i32);

    /// Collect the full per-glyph identity (id, cluster, x/y advance, x/y
    /// offset) from a shaped `GlyphBuffer`.
    fn glyph_identities(output: &rustybuzz::GlyphBuffer) -> Vec<GlyphIdentity> {
        let infos = output.glyph_infos();
        let positions = output.glyph_positions();
        infos
            .iter()
            .zip(positions.iter())
            .map(|(info, pos)| {
                (
                    u16::value_from(info.glyph_id).unwrap_or(0),
                    info.cluster,
                    pos.x_advance,
                    pos.y_advance,
                    pos.x_offset,
                    pos.y_offset,
                )
            })
            .collect()
    }

    /// Shape `text` as a single same-format run via the OLD `rustybuzz::shape()`
    /// entry point — bypassing `FontManager`'s face/plan cache entirely, by
    /// parsing the face directly from the raw bytes — for comparison against
    /// the new cached `shape_cached` path.
    fn shape_via_old_api(
        text: &str,
        face_id: FaceId,
        fm: &FontManager,
        ligatures: bool,
    ) -> Vec<GlyphIdentity> {
        let bytes = fm.face_data(face_id).expect("face must be loaded");
        let index: u32 = fm
            .face_index(face_id)
            .and_then(|i| u32::value_from(i).ok())
            .expect("face index must be loaded and fit in u32");
        let face = rustybuzz::Face::from_slice(bytes, index).expect("face must parse");

        let features = shaping_features(ligatures);
        let mut buffer = rustybuzz::UnicodeBuffer::new();
        buffer.push_str(text);
        let output = rustybuzz::shape(&face, &features, buffer);
        glyph_identities(&output)
    }

    /// Shape `text` through the NEW cached `shape_cached` path (the same
    /// entry point `shape_single_run` uses in production), returning the
    /// same per-glyph identity sequence for comparison.
    fn shape_via_new_api(
        text: &str,
        face_id: FaceId,
        fm: &FontManager,
        ligatures: bool,
    ) -> Vec<GlyphIdentity> {
        let features = shaping_features(ligatures);
        let mut buffer = rustybuzz::UnicodeBuffer::new();
        buffer.push_str(text);
        buffer.guess_segment_properties();
        let output = fm
            .shape_cached(face_id, ligatures, &features, buffer)
            .expect("shape_cached must succeed for a loaded face");
        glyph_identities(&output)
    }

    /// Regression guard for the core claim of Task #430: caching the parsed
    /// `Face` and the compiled `ShapePlan` and shaping via
    /// `shape_with_plan` must produce IDENTICAL output — glyph ids, source
    /// clusters, AND positional data (advances + offsets) — to the previous
    /// `rustybuzz::shape()` call for every kind of content the terminal
    /// renderer shapes: plain Latin text, ligating/contextual sequences, a
    /// CJK (wide) character, and an emoji (routed to a different face
    /// entirely). Positions are asserted because `kern` is always enabled,
    /// so a plan-caching regression could shift advances without touching
    /// glyph ids. If this test ever fails, the perf change altered
    /// rendering, which is the one thing it must never do.
    #[test]
    fn shape_with_plan_matches_old_shape_for_mixed_content() {
        let fm = test_font_manager();

        let samples: &[(&str, FaceId, bool)] = &[
            ("Hello, world!", FaceId::PrimaryRegular, false),
            ("->", FaceId::PrimaryRegular, true),
            ("!=", FaceId::PrimaryRegular, true),
            ("中", FaceId::PrimaryRegular, false),
        ];

        for &(text, face_id, ligatures) in samples {
            let old = shape_via_old_api(text, face_id, &fm, ligatures);
            let new = shape_via_new_api(text, face_id, &fm, ligatures);
            assert_eq!(
                old, new,
                "`{text}` (ligatures={ligatures}): shape_with_plan output \
                 must match the old rustybuzz::shape() output exactly"
            );
        }

        // Emoji routes to a different face (FaceId::Emoji) entirely — the
        // bundled Noto Color Emoji floor (Task #402) guarantees one is
        // always loaded, so this is deterministic across hosts.
        let old_emoji = shape_via_old_api("😀", FaceId::Emoji, &fm, false);
        let new_emoji = shape_via_new_api("😀", FaceId::Emoji, &fm, false);
        assert_eq!(
            old_emoji, new_emoji,
            "emoji: shape_with_plan output must match the old rustybuzz::shape() \
             output exactly"
        );
    }
}

/// Subtask 124.16: the shaping cache's hit rate on the four workloads that
/// decide 124.6.
///
/// These are a **measurement**, kept as tests so the numbers cannot rot
/// silently the way the 2026-07-29 pointer-motion table did. Each asserts
/// the structural finding rather than a timing, so they are deterministic
/// and platform-independent.
///
/// The headline results are recorded in 124.16's findings block in
/// `Documents/PLAN_124_RENDER_EFFICIENCY.md`.
#[cfg(test)]
mod shaping_cache_hit_rate {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::{FormatTag, ShapingCache, ShapingCacheStats};
    use freminal_common::buffer_states::tchar::TChar;
    use freminal_common::config::Config;

    use crate::gui::font_manager::FontManager;

    /// A `width` x `height` grid of newline-separated ASCII, generated from
    /// cell coordinates so every row is distinct and the content is
    /// reproducible (the same property `SyntheticFrame` relies on in the
    /// Task 123 harness — no randomness, so a hit count is exact).
    /// No trailing newline: `split_into_lines` would read one as a final
    /// empty line, giving `height + 1` lines and quietly shifting every hit
    /// count in this module by one.
    fn grid_chars(width: usize, height: usize) -> Vec<TChar> {
        let mut out = Vec::with_capacity((width + 1) * height);
        for row in 0..height {
            if row > 0 {
                out.push(TChar::NewLine);
            }
            for col in 0..width {
                let b = b'a' + u8::try_from((row * 7 + col * 3) % 26).unwrap_or(0);
                out.push(TChar::Ascii(b));
            }
        }
        out
    }

    /// Drive one frame through a cache and return that frame's tally alone.
    fn frame(
        cache: &mut ShapingCache,
        fm: &mut FontManager,
        chars: &[TChar],
        width: usize,
    ) -> ShapingCacheStats {
        let tags = vec![FormatTag {
            start: 0,
            end: chars.len(),
            ..FormatTag::default()
        }];
        cache.reset_stats();
        #[allow(clippy::cast_precision_loss)]
        let cell_w = fm.cell_width() as f32;
        let _ = cache.shape_visible(chars, &tags, width, fm, cell_w, false, &[]);
        cache.stats()
    }

    fn fixture() -> (FontManager, ShapingCache) {
        (
            FontManager::new(&Config::default(), 1.0).unwrap(),
            ShapingCache::new(),
        )
    }

    /// A full-screen redraw of **identical** content costs nothing.
    ///
    /// This is the workload the whole of Task 124 exists for: a TUI that
    /// rewrites unchanged bytes every tick. The shaping stage already
    /// handles it correctly, which is a genuine finding — it means shaping
    /// is *not* where that workload's cost lives.
    #[test]
    fn identical_full_screen_redraw_is_a_total_hit() {
        let (mut fm, mut cache) = fixture();
        let chars = grid_chars(80, 24);

        let cold = frame(&mut cache, &mut fm, &chars, 80);
        assert_eq!(cold.hits, 0, "the first frame cannot hit");
        assert_eq!(cold.misses, 24);

        let warm = frame(&mut cache, &mut fm, &chars, 80);
        assert_eq!(warm.misses, 0, "an identical redraw must re-shape nothing");
        assert_eq!(warm.hit_rate(), Some(1.0));

        // Subtask 124.6: a 100%-line-hit call never touches the run cache at
        // all (a line hit reuses the whole `Arc<ShapedLine>`, runs included).
        // This is the fact that keeps 124.6 from being justified on this
        // workload — the run cache has nothing to contribute here because
        // the line cache already handles it perfectly.
        assert_eq!(warm.run_hits, 0);
        assert_eq!(warm.run_misses, 0);
        assert_eq!(warm.run_hit_rate(), None);
    }

    /// A single-character edit re-shapes exactly one row.
    ///
    /// Note what this does *not* say. The row is the granule: one changed
    /// character re-shapes every run on its line, which is 124.6's second
    /// lever. But it does not spill into neighbouring rows.
    #[test]
    fn a_single_character_edit_reshapes_exactly_one_row() {
        let (mut fm, mut cache) = fixture();
        let chars = grid_chars(80, 24);
        let _ = frame(&mut cache, &mut fm, &chars, 80);

        let mut edited = chars.clone();
        let target = edited
            .iter()
            .position(|c| matches!(c, TChar::Ascii(_)))
            .expect("an ascii cell");
        edited[target] = TChar::Ascii(b'#');

        let after = frame(&mut cache, &mut fm, &edited, 80);
        assert_eq!(after.misses, 1, "exactly the edited row re-shapes");
        assert_eq!(after.hits, 23);
    }

    /// **A scroll by one line still misses the LINE cache completely, but
    /// subtask 124.6's run cache now reuses all 23 unchanged rows.**
    ///
    /// This is the inverted form of the original
    /// `a_scroll_by_one_line_hits_nothing`, which pinned the pre-124.6
    /// defect as a regression guard (per the idiom Task 123 used for 124.9
    /// and 124.1). `ShapingCache`'s LINE cache is still keyed by line index —
    /// unchanged by this subtask — so scrolling by one row still shifts
    /// every line's content into a different slot and every line still
    /// misses at that layer (asserted below, exactly as before). But this
    /// grid fixture's uniform single-tag format puts every row in exactly
    /// one run, so at the RUN layer the 23 rows that are byte-identical to a
    /// row shaped a moment ago (just at a different line index and thus a
    /// different `RunCacheKey`-irrelevant position) now hit the
    /// content-addressed run cache instead of paying a fresh `rustybuzz`
    /// call. Only the one genuinely new bottom row misses.
    #[test]
    fn a_scroll_by_one_line_reuses_unchanged_runs() {
        let (mut fm, mut cache) = fixture();
        let chars = grid_chars(80, 25);

        // Frame 1: rows 0..24 of the content.
        let top = grid_chars_window(&chars, 0, 24);
        let _ = frame(&mut cache, &mut fm, &top, 80);

        // Frame 2: rows 1..25 — a one-line scroll. 23 of these 24 lines were
        // shaped a moment ago, at a different index.
        let scrolled = grid_chars_window(&chars, 1, 24);
        let after = frame(&mut cache, &mut fm, &scrolled, 80);

        assert_eq!(
            after.hits, 0,
            "the LINE cache is still keyed by index, so it still misses every line"
        );
        assert_eq!(after.misses, 24);
        assert_eq!(after.hit_rate(), Some(0.0));

        assert_eq!(
            after.run_hits, 23,
            "the RUN cache is content-addressed, so the 23 rows byte-identical \
             to a row shaped a moment ago (at a different line index) must hit"
        );
        assert_eq!(
            after.run_misses, 1,
            "only the genuinely new bottom row should miss the run cache"
        );
        assert_eq!(after.run_hit_rate(), Some(23.0 / 24.0));
    }

    /// Steady typing: one row changes per frame, the rest hit.
    #[test]
    fn steady_typing_hits_every_row_but_the_one_being_typed_on() {
        let (mut fm, mut cache) = fixture();
        let mut chars = grid_chars(80, 24);
        let _ = frame(&mut cache, &mut fm, &chars, 80);

        // Type five characters into the last row, one per frame.
        let last_row_start = chars
            .iter()
            .enumerate()
            .filter(|(_, c)| matches!(c, TChar::NewLine))
            .map(|(i, _)| i)
            .nth(22)
            .expect("row boundary")
            + 1;

        let mut totals = ShapingCacheStats::default();
        for k in 0..5 {
            chars[last_row_start + k] = TChar::Ascii(b'z');
            let s = frame(&mut cache, &mut fm, &chars, 80);
            totals.hits += s.hits;
            totals.misses += s.misses;
        }

        assert_eq!(totals.misses, 5, "one row re-shapes per keystroke");
        assert_eq!(totals.hits, 5 * 23);
    }

    /// Take `count` lines starting at line `from` out of a newline-separated
    /// char grid, as its own newline-separated grid.
    fn grid_chars_window(chars: &[TChar], from: usize, count: usize) -> Vec<TChar> {
        let mut out = Vec::new();
        for (i, line) in chars
            .split(|c| matches!(c, TChar::NewLine))
            .skip(from)
            .take(count)
            .enumerate()
        {
            // No trailing newline, for the same reason `grid_chars` omits it.
            if i > 0 {
                out.push(TChar::NewLine);
            }
            out.extend_from_slice(line);
        }
        out
    }
}
