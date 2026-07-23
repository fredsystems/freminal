// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Pure CPU vertex builders for the terminal rendering pipeline.
//!
//! All functions in this module are fully testable without a GL context.
//! They build flat `Vec<f32>` buffers that are subsequently uploaded to the
//! GPU by the [`super::gpu`] module.

use conv2::{ApproxFrom, ConvUtil, ValueFrom};
use freminal_common::buffer_states::fonts::{BlinkState, FontDecorations, UnderlineStyle};
use freminal_common::cursor::CursorVisualStyle;
use freminal_common::themes::ThemePalette;
use freminal_terminal_emulator::LineWidth;
use freminal_terminal_emulator::{
    ImagePlacement, ImageSizeMode, InlineImage, SourceCrop, SubCellOffset,
};
use std::sync::Arc;

use super::super::{
    atlas::{AtlasEntry, GlyphAtlas, GlyphKey},
    colors::{
        command_block_hover_bg_f, cursor_f, internal_color_to_gl, search_current_bg_f,
        search_match_bg_f, selection_bg_f, selection_fg_f,
    },
    font_manager::FontManager,
    shaping::{ShapedGlyph, ShapedLine},
};

// ---------------------------------------------------------------------------
//  GL numeric conversion helpers (used by vertex builders)
// ---------------------------------------------------------------------------
//
// The vertex builders use f32 for GPU coordinate math and need checked
// conversions from usize/u32.  These mirror the helpers in gpu.rs but are
// reproduced here to keep this module self-contained (no cross-submodule
// dependency on gpu.rs).

use tracing::error;

/// Convert a `usize` to `f32` for GPU coordinate math.
#[inline]
pub(super) fn gl_f32(val: usize) -> f32 {
    val.approx_as::<f32>().unwrap_or_else(|_| {
        error!("gl_f32: usize {val} cannot be approximated as f32");
        0.0
    })
}

/// Convert a `u32` to `f32` for GPU cell-dimension math.
#[inline]
pub(super) fn gl_f32_u32(val: u32) -> f32 {
    f32::approx_from(val).unwrap_or_else(|_| {
        error!("gl_f32_u32: u32 {val} cannot be approximated as f32");
        0.0
    })
}

/// Horizontal scale factor for a given `LineWidth`.
///
/// DECDWL and DECDHL rows render at 2× horizontal width; normal rows are 1×.
#[inline]
const fn x_scale(lw: LineWidth) -> f32 {
    match lw {
        LineWidth::Normal => 1.0,
        LineWidth::DoubleWidth | LineWidth::DoubleHeightTop | LineWidth::DoubleHeightBottom => 2.0,
    }
}

// ---------------------------------------------------------------------------
//  DECSCNM (screen reverse video) × SGR-7 compose (Task 115.2)
// ---------------------------------------------------------------------------
//
// `StateColors::color()` / `StateColors::background_color()` already resolve
// per-cell SGR-7 (reverse video) before vertex.rs ever sees a color:
// `color()` returns the SGR-7-effective foreground, `background_color()`
// returns the SGR-7-effective background.
//
// DECSCNM (whole-screen reverse video) must compose with SGR-7 by XOR: a
// cell's fg/bg should appear swapped when *exactly one* of {SGR-7, DECSCNM}
// is active (both active cancels out back to normal).
//
// Rather than re-implement the swap (and duplicate `default_to_regular()`'s
// sentinel handling), we exploit an accessor-swap identity: when DECSCNM is
// active, calling the OPPOSITE accessor reproduces the XOR exactly:
//
// - DECSCNM off -> use the accessor for the property we want (today's
//   behavior: SGR-7 alone determines the swap).
// - DECSCNM on  -> use the OTHER accessor. If SGR-7 is off, this yields the
//   swapped color (DECSCNM alone flips it). If SGR-7 is also on, the two
//   swaps cancel and we get the un-swapped color back (XOR cancels).
//
// All per-cell color reads in this module MUST route through these two
// helpers rather than calling `StateColors::color()` /
// `StateColors::background_color()` directly, so DECSCNM composes
// consistently everywhere colors are resolved.

/// Effective foreground color for a cell: composes per-cell SGR-7 (already
/// resolved inside `colors.color()`/`colors.background_color()`) with
/// whole-screen DECSCNM by XOR, via the opposite-accessor trick documented
/// above (Task 115.2).
#[inline]
const fn effective_fg(
    colors: &freminal_common::buffer_states::cursor::StateColors,
    reverse_screen: bool,
) -> freminal_common::colors::TerminalColor {
    if reverse_screen {
        colors.background_color()
    } else {
        colors.color()
    }
}

/// Effective background color for a cell — the mirror of [`effective_fg`].
#[inline]
const fn effective_bg(
    colors: &freminal_common::buffer_states::cursor::StateColors,
    reverse_screen: bool,
) -> freminal_common::colors::TerminalColor {
    if reverse_screen {
        colors.color()
    } else {
        colors.background_color()
    }
}

// ---------------------------------------------------------------------------
//  Vertex stride constants (in f32 components)
// ---------------------------------------------------------------------------

/// Decoration vertex: `x, y, r, g, b, a` — 6 floats per vertex.
///
/// Used for underlines, strikethrough, cursor, and selection highlight quads.
pub(super) const DECO_VERTEX_FLOATS: usize = 6;

/// Foreground instance: `glyph_x, glyph_y, glyph_w, glyph_h, u0, v0, u1, v1,
/// r, g, b, a, is_color` — 13 floats per glyph instance.
pub(crate) const FG_INSTANCE_FLOATS: usize = 13;

/// Image vertex: `x, y, u, v` — 4 floats per vertex.
pub(super) const IMG_VERTEX_FLOATS: usize = 4;

/// Vertices per quad (2 triangles, 6 vertices).
pub(crate) const VERTS_PER_QUAD: usize = 6;

/// Floats for one cursor quad in the decoration VBO.
pub const CURSOR_QUAD_FLOATS: usize = VERTS_PER_QUAD * DECO_VERTEX_FLOATS;

/// Per-instance data: `col, row, r, g, b, a` — 6 floats per cell instance.
pub(crate) const BG_INSTANCE_FLOATS: usize = 6;

// ---------------------------------------------------------------------------
//  Public vertex-builder types
// ---------------------------------------------------------------------------

/// Options controlling per-glyph foreground rendering.
///
/// Bundled to keep `build_foreground_instances` within the 7-argument lint limit.
// Four independent, short-lived rendering-intent flags (selection shape,
// two blink-visibility phases, DECSCNM state); a state machine would couple
// unrelated concerns and obscure intent.
#[allow(clippy::struct_excessive_bools)]
pub struct FgRenderOptions {
    /// Normalised selection region `(start_col, start_row, end_col, end_row)`,
    /// or `None` when no selection is active.
    pub selection: Option<(usize, usize, usize, usize)>,
    /// `true` when the selection is a rectangular block (Alt+drag).
    ///
    /// When `false` the selection is a linear span.  When `true` every row in
    /// the range uses the same column boundaries (`start_col`..=`end_col`).
    pub selection_is_block: bool,
    /// Whether slow-blink (SGR 5) text is currently in its visible phase.
    pub text_blink_slow_visible: bool,
    /// Whether fast-blink (SGR 6) text is currently in its visible phase.
    pub text_blink_fast_visible: bool,
    /// `true` when DECSCNM (whole-screen reverse video) is active for this
    /// pane. Composed with per-cell SGR-7 by XOR via [`effective_fg`] /
    /// [`effective_bg`] (Task 115.2).
    pub reverse_screen: bool,
}

impl FgRenderOptions {
    /// Convenience constructor for the common case where all text is fully visible
    /// (e.g. internal helper calls and tests that do not exercise blink).
    #[must_use]
    pub const fn all_visible(selection: Option<(usize, usize, usize, usize)>) -> Self {
        Self {
            selection,
            selection_is_block: false,
            text_blink_slow_visible: true,
            text_blink_fast_visible: true,
            reverse_screen: false,
        }
    }
}

/// Per-image-PLACEMENT tracking: pixel bounding box and cell-grid extent
/// within the image.  The cell-grid extent (min/max `col_in_image`,
/// `row_in_image`) tells us which portion of the texture is visible, so we
/// can compute UV coordinates that preserve aspect ratio even when the
/// image is partially clipped by the terminal edge.
///
/// Bucketed by `placement_instance` (Task 100.18), NOT by `image_id` — two
/// independent on-screen placements of the SAME image (e.g. two `a=p` puts
/// with `p=0`/unspecified) are two separate `ImageBounds` entries, each
/// producing its own quad, rather than merging into one oversized bucket.
struct ImageBounds {
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    min_col_in_image: usize,
    min_row_in_image: usize,
    max_col_in_image: usize,
    max_row_in_image: usize,
    /// The underlying image id (texture lookup key) this placement
    /// displays. Captured from the first cell of the bucket — correct
    /// because every cell of one placement instance shares one image id.
    image_id: u64,
    /// Kitty `z=` layering value (Task 100.7b). Read from the first-seen
    /// placement for this placement instance; all cells of one placement
    /// share the same z-index, so later cells must not overwrite it.
    z_index: i32,
    /// Source-crop (kitty `a=p` x/y/w/h); read from the first-seen
    /// placement, like `z_index`. `None` = full image. Bucketing by
    /// `placement_instance` (Task 100.18) means two DIFFERENT placements of
    /// the same image with different crops no longer collapse — each gets
    /// its own bucket and therefore its own crop.
    crop: Option<SourceCrop>,
    /// Sub-cell pixel offset (kitty `a=p`/`a=T` `X=`/`Y=`, Task 100.19);
    /// read from the first-seen placement, like `z_index`/`crop`.
    subcell_offset: Option<SubCellOffset>,
}

/// One entry in the authoritative image draw order (Task 100.18).
///
/// Carries BOTH the placement instance id (the key into the vertex-slab
/// bucketing done by [`build_image_verts`]) and the underlying image id
/// (the key into the GPU texture cache in `gpu.rs`'s `image_textures`
/// map) — the two are no longer the same key, since bucketing moved from
/// `image_id` to `placement_instance`, but texture upload/lookup is still
/// keyed by `image_id` (the pixel data, which may be shared by several
/// placements).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageDrawEntry {
    /// The placement-instance id (Task 100.18) — identifies which vertex
    /// slab in the image VBO this entry corresponds to.
    pub instance_id: u64,
    /// The underlying image id — identifies which GPU texture to bind.
    pub image_id: u64,
}

// ---------------------------------------------------------------------------
//  Build background instances + decoration verts
// ---------------------------------------------------------------------------

/// Returns whether the cursor should be visible given its style and blink state.
const fn cursor_blink_is_visible(style: &CursorVisualStyle, blink_on: bool) -> bool {
    match style {
        CursorVisualStyle::BlockCursorSteady
        | CursorVisualStyle::UnderlineCursorSteady
        | CursorVisualStyle::VerticalLineCursorSteady => true,
        CursorVisualStyle::BlockCursorBlink
        | CursorVisualStyle::UnderlineCursorBlink
        | CursorVisualStyle::VerticalLineCursorBlink => blink_on,
    }
}

/// A search match span for match-highlight rendering.
///
/// Coordinates are in *visible-window* space matching the shaped-line
/// indices passed to [`build_background_instances`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MatchHighlight {
    /// Row index within the visible window (0 = top).
    pub row: usize,
    /// First highlighted column (inclusive).
    pub col_start: usize,
    /// Last highlighted column (inclusive).
    pub col_end: usize,
    /// `true` when this is the current (focused) match.
    pub is_current: bool,
}

/// Groups all frame-level rendering state required by [`build_background_instances`].
///
/// Passing a struct instead of 18 positional parameters keeps call sites
/// readable and eliminates the need for `#[allow(clippy::too_many_arguments)]`.
// Independent, short-lived per-frame rendering-intent flags (cursor
// visibility/blink, selection shape, DECSCNM state); a state machine would
// couple unrelated concerns and obscure intent.
#[allow(clippy::struct_excessive_bools)]
pub struct BackgroundFrame<'a> {
    pub shaped_lines: &'a [Arc<ShapedLine>],
    pub cell_width: u32,
    pub cell_height: u32,
    pub ascent: f32,
    pub underline_offset: f32,
    pub strikeout_offset: f32,
    pub stroke_size: f32,
    pub show_cursor: bool,
    pub cursor_blink_on: bool,
    pub cursor_pixel_pos: (f32, f32),
    pub cursor_width_scale: f32,
    pub cursor_visual_style: &'a CursorVisualStyle,
    pub selection: Option<(usize, usize, usize, usize)>,
    pub selection_is_block: bool,
    pub match_highlights: &'a [MatchHighlight],
    /// Inclusive rendered-row range `[start, end]` for the OSC 133
    /// command block currently under the mouse, if any.  Drawn full
    /// width between search highlights and selection so the underlying
    /// text remains readable.
    pub command_block_hover_rows: Option<(usize, usize)>,
    /// Terminal width in columns, used to draw the hover-tint band the
    /// full row width regardless of how short individual shaped lines
    /// are.  Should match `TerminalSnapshot::term_width` at call time.
    pub term_width_cols: usize,
    pub theme: &'a ThemePalette,
    pub cursor_color_override: Option<(u8, u8, u8)>,
    /// `true` when DECSCNM (whole-screen reverse video) is active for this
    /// pane. Composed with per-cell SGR-7 by XOR via [`effective_fg`] /
    /// [`effective_bg`] (Task 115.2).
    pub reverse_screen: bool,
}

/// Build the two-pass background data: instanced cell BGs + decoration quads.
///
/// Returns `(bg_instances, deco_verts)`:
/// - `bg_instances`: flat `Vec<f32>` with `BG_INSTANCE_FLOATS` (6) floats per
///   cell that has a non-default background.  Each instance is
///   `(col, row, r, g, b, a)`.  Uploaded to the instance VBO and drawn with
///   `draw_arrays_instanced`.
/// - `deco_verts`: flat `Vec<f32>` with `DECO_VERTEX_FLOATS` (6) floats per
///   vertex.  Contains underline, strikethrough, selection highlight, and
///   cursor quads.  Uploaded to the decoration VBO and drawn with a plain
///   `draw_arrays` call.
///
/// The cursor quad (if visible) is always appended **last** in `deco_verts`
/// so that cursor-only partial updates can patch just the tail.
///
/// Returns `true` if a cursor quad was actually appended to `deco_verts`,
/// `false` otherwise. The caller **must** use this return value (not its own
/// copy of `frame.show_cursor`) to compute where the cursor's tail quad
/// begins for later cursor-only patches — `frame.show_cursor` alone does not
/// account for the blink-visibility gate (`cursor_blink_is_visible`) applied
/// here, and recomputing the append decision independently let the two
/// silently disagree whenever a full rebuild happened to run during the
/// cursor's blink-off phase (issue #432): the offset bookkeeping would then
/// assume a cursor quad was appended when it was not, causing a later
/// cursor-only frame to blink the cursor back on by overwriting whatever
/// quad actually occupies that tail position — in practice the bottom-most
/// row's selection highlight quad, since selection quads are appended in
/// top-to-bottom row order and the bottom row's is therefore always the last
/// one pushed before the (absent) cursor quad.
// All parameters are required geometric and style inputs for GPU instance data generation.
// Inherently large: iterates all shaped lines, resolving background color for every cell.
#[allow(clippy::too_many_lines)]
#[must_use]
pub fn build_background_instances(
    frame: &BackgroundFrame<'_>,
    instances: &mut Vec<f32>,
    deco: &mut Vec<f32>,
) -> bool {
    let shaped_lines = frame.shaped_lines;
    let cell_width = frame.cell_width;
    let cell_height = frame.cell_height;
    let ascent = frame.ascent;
    let underline_offset = frame.underline_offset;
    let strikeout_offset = frame.strikeout_offset;
    let stroke_size = frame.stroke_size;
    let show_cursor = frame.show_cursor;
    let cursor_blink_on = frame.cursor_blink_on;
    let cursor_pixel_pos = frame.cursor_pixel_pos;
    let cursor_width_scale = frame.cursor_width_scale;
    let cursor_visual_style = frame.cursor_visual_style;
    let selection = frame.selection;
    let selection_is_block = frame.selection_is_block;
    let match_highlights = frame.match_highlights;
    let command_block_hover_rows = frame.command_block_hover_rows;
    let term_width_cols = frame.term_width_cols;
    let theme = frame.theme;
    let cursor_color_override = frame.cursor_color_override;
    let reverse_screen = frame.reverse_screen;
    // Reuse existing heap allocations — clear but keep capacity.
    instances.clear();
    deco.clear();

    for (row_idx, line) in shaped_lines.iter().enumerate() {
        let y_top = gl_f32(row_idx) * gl_f32_u32(cell_height);
        let lw = line.line_width;
        let scale = x_scale(lw);

        // --- Per-cell background instances ---
        for run in &line.runs {
            let is_faint = run.font_decorations.contains(FontDecorations::Faint);
            // Task 115.2: compute the DECSCNM/SGR-7-composed effective
            // background BEFORE the DefaultBackground skip check below, so
            // the skip decision operates on the post-swap color. This is
            // required for correctness: under DECSCNM a cell whose raw
            // background was DefaultBackground may now have a real
            // (swapped-from-foreground) effective background and must be
            // drawn, while a cell whose effective background becomes
            // DefaultBackground must still be skipped.
            let bg_color_raw = effective_bg(&run.colors, reverse_screen);

            // Skip default backgrounds (transparent — the terminal base color
            // is rendered as a panel clear, not explicit quads).
            if matches!(
                bg_color_raw,
                freminal_common::colors::TerminalColor::DefaultBackground
            ) {
                continue;
            }

            let [r, g, b, a] = internal_color_to_gl(bg_color_raw, is_faint, theme);

            // Emit one instance per cell in this run.
            // For double-width/height rows, each logical column is 2 physical
            // cells wide, so emit 2 adjacent instances per column.
            let col_count = run_col_count(run);
            emit_bg_cells(
                instances,
                run.col_start,
                col_count,
                row_idx,
                scale,
                [r, g, b, a],
            );
        }

        // --- Underline and strikethrough decoration quads ---
        for run in &line.runs {
            let is_faint = run.font_decorations.contains(FontDecorations::Faint);
            let underline_style = run.font_decorations.underline_style();
            let has_strike = run
                .font_decorations
                .contains(FontDecorations::Strikethrough);

            if !underline_style.is_active() && !has_strike {
                continue;
            }

            let col_end = run.col_start + run_col_count(run);
            let x0 = gl_f32(run.col_start) * gl_f32_u32(cell_width) * scale;
            let x1 = gl_f32(col_end) * gl_f32_u32(cell_width) * scale;

            if underline_style.is_active() {
                // Use underline color if set, otherwise fall back to
                // foreground. The underline color ITSELF is independent of
                // fg/bg inversion and is deliberately NOT routed through
                // `effective_fg` — only the fallback-to-foreground path is,
                // so the underline stays visible against a DECSCNM-swapped
                // background (Task 115.2).
                let ul_color_raw = run.colors.underline_color();
                let ul_color = if matches!(
                    ul_color_raw,
                    freminal_common::colors::TerminalColor::DefaultUnderlineColor
                ) {
                    internal_color_to_gl(effective_fg(&run.colors, reverse_screen), is_faint, theme)
                } else {
                    internal_color_to_gl(ul_color_raw, is_faint, theme)
                };

                // underline_offset from swash is negative (below baseline in font
                // coords).  In top-down pixel coords the baseline is at
                // y_top + ascent, so subtracting the (negative) offset places the
                // line below the baseline.
                let ul_y = y_top + ascent - underline_offset;

                let cw = gl_f32_u32(cell_width);
                push_underline_quads(
                    deco,
                    underline_style,
                    &UnderlineParams {
                        x0,
                        x1,
                        ul_y,
                        thick: stroke_size.max(1.0),
                        cell_width: cw,
                        color: ul_color,
                    },
                );
            }

            if has_strike {
                // Task 115.2: strikethrough uses the effective (DECSCNM ×
                // SGR-7 composed) foreground so it stays visible under
                // whole-screen reverse video.
                let fg_color = internal_color_to_gl(
                    effective_fg(&run.colors, reverse_screen),
                    is_faint,
                    theme,
                );
                // strikeout_offset from OS/2 is positive (above baseline in font
                // coords).  In top-down pixel coords, subtracting it from the
                // baseline places the line above the baseline (middle of cell).
                let st_top = y_top + ascent - strikeout_offset;
                let st_bot = st_top + stroke_size.max(1.0);
                push_quad(deco, x0, st_top, x1, st_bot, fg_color);
            }
        }
    }

    // --- Search match highlight quads (rendered first so selection overpaints) ---
    // Render non-current matches first, then current matches, so the focused
    // match is always visible if highlight regions overlap.
    for &is_current_pass in &[false, true] {
        for m in match_highlights
            .iter()
            .filter(|m| m.is_current == is_current_pass)
        {
            if m.row >= shaped_lines.len() || m.col_start > m.col_end {
                continue;
            }
            let cw = gl_f32_u32(cell_width);
            let ch = gl_f32_u32(cell_height);
            let row_scale = x_scale(shaped_lines[m.row].line_width);
            let x0 = gl_f32(m.col_start) * cw * row_scale;
            let x1 = gl_f32(m.col_end + 1) * cw * row_scale;
            let y0 = gl_f32(m.row) * ch;
            let y1 = y0 + ch;
            let color = if m.is_current {
                search_current_bg_f()
            } else {
                search_match_bg_f()
            };
            push_quad(deco, x0, y0, x1, y1, color);
        }
    }

    // --- Command-block hover tint (full-width rows, drawn between search and selection) ---
    //
    // Rendered after search highlights so it overpaints them, but
    // before selection so an active selection inside the hovered block
    // remains crisply visible.  Uses the theme's selection color at
    // 25% alpha so the indicator is recognisable as "the same family"
    // as selection without looking like a real selection.
    //
    // Span is `term_width_cols * cell_width`, matching the snapshot's
    // configured terminal width.  Deriving the span from the last
    // shaped run on each row would visually truncate the tint at the
    // last non-blank glyph -- typically a much narrower band than the
    // full row, because trailing blank cells contribute no glyphs.
    if let Some((hover_start, hover_end)) = command_block_hover_rows
        && hover_start <= hover_end
        && term_width_cols > 0
    {
        let cw = gl_f32_u32(cell_width);
        let ch = gl_f32_u32(cell_height);
        let last_row = shaped_lines.len().saturating_sub(1);
        let clamped_end = hover_end.min(last_row);
        let color = command_block_hover_bg_f(theme);
        for row in hover_start..=clamped_end {
            if row >= shaped_lines.len() {
                break;
            }
            let row_scale = x_scale(shaped_lines[row].line_width);
            let x0 = 0.0;
            let x1 = gl_f32(term_width_cols) * cw * row_scale;
            let y0 = gl_f32(row) * ch;
            let y1 = y0 + ch;
            push_quad(deco, x0, y0, x1, y1, color);
        }
    }

    // --- Selection highlight quads (rendered after search so selection is topmost) ---
    if let Some((sel_start_col, sel_start_row, sel_end_col, sel_end_row)) = selection {
        let cw = gl_f32_u32(cell_width);
        let ch = gl_f32_u32(cell_height);

        // For block selections the same column span applies to every row.
        let block_col_begin = sel_start_col.min(sel_end_col);
        let block_col_end = sel_start_col.max(sel_end_col);

        for (row, line) in shaped_lines
            .iter()
            .enumerate()
            .take(sel_end_row + 1)
            .skip(sel_start_row)
        {
            let (col_begin, col_end) = if selection_is_block {
                // Block selection: same column range on every row.
                (block_col_begin, block_col_end)
            } else {
                // Linear selection: first row starts at anchor col, last row
                // ends at end col, middle rows span the full line width.
                let begin = if row == sel_start_row {
                    sel_start_col
                } else {
                    0
                };
                let end = if row == sel_end_row {
                    sel_end_col
                } else {
                    line.runs
                        .last()
                        .map_or(0, |r| r.col_start + run_col_count(r))
                        .saturating_sub(1)
                };
                (begin, end)
            };

            if col_end < col_begin {
                continue;
            }

            let row_scale = x_scale(line.line_width);
            let x0 = gl_f32(col_begin) * cw * row_scale;
            let x1 = gl_f32(col_end + 1) * cw * row_scale;
            let y0 = gl_f32(row) * ch;
            let y1 = y0 + ch;

            push_quad(deco, x0, y0, x1, y1, selection_bg_f(theme));
        }
    }

    // --- Cursor quad (always last in deco so cursor-only patches work) ---
    let cursor_quad_appended =
        show_cursor && cursor_blink_is_visible(cursor_visual_style, cursor_blink_on);
    if cursor_quad_appended {
        let (cx, cy) = cursor_pixel_pos;
        let cw = gl_f32_u32(cell_width) * cursor_width_scale;
        let ch = gl_f32_u32(cell_height);

        let color = cursor_f(theme, cursor_color_override);

        match cursor_visual_style {
            CursorVisualStyle::BlockCursorBlink | CursorVisualStyle::BlockCursorSteady => {
                push_quad(deco, cx, cy, cx + cw, cy + ch, color);
            }
            CursorVisualStyle::UnderlineCursorBlink | CursorVisualStyle::UnderlineCursorSteady => {
                let bar_h = (ch * 0.1).max(2.0);
                push_quad(deco, cx, cy + ch - bar_h, cx + cw, cy + ch, color);
            }
            CursorVisualStyle::VerticalLineCursorBlink
            | CursorVisualStyle::VerticalLineCursorSteady => {
                let bar_w = (cw * 0.1).max(1.0);
                push_quad(deco, cx, cy, cx + bar_w, cy + ch, color);
            }
        }
    }

    cursor_quad_appended
}

// ---------------------------------------------------------------------------
//  Build cursor-only verts
// ---------------------------------------------------------------------------

/// Build just the cursor quad for the background VBO.
///
/// Returns `CURSOR_QUAD_FLOATS` floats when the cursor is visible, or an
/// empty `Vec` when it should not be painted (cursor hidden, or blink-off).
///
/// This is the "cheap path" used for cursor-only frame updates: instead of
/// rebuilding the entire background VBO, the caller patches only the cursor
/// quad region in-place via `upload_verts_sub`.
#[must_use]
// All parameters are required for cursor geometry: cell dimensions, screen position, cursor
// style, and color. No subset is independently reusable.
#[allow(clippy::too_many_arguments)]
pub fn build_cursor_verts_only(
    cell_width: u32,
    cell_height: u32,
    show_cursor: bool,
    cursor_blink_on: bool,
    cursor_pixel_pos: (f32, f32),
    cursor_width_scale: f32,
    cursor_visual_style: &CursorVisualStyle,
    theme: &ThemePalette,
    cursor_color_override: Option<(u8, u8, u8)>,
) -> Vec<f32> {
    let mut verts = Vec::new();

    if show_cursor && cursor_blink_is_visible(cursor_visual_style, cursor_blink_on) {
        let (cx, cy) = cursor_pixel_pos;
        let cw = gl_f32_u32(cell_width) * cursor_width_scale;
        let ch = gl_f32_u32(cell_height);

        let color = cursor_f(theme, cursor_color_override);

        match cursor_visual_style {
            CursorVisualStyle::BlockCursorBlink | CursorVisualStyle::BlockCursorSteady => {
                push_quad(&mut verts, cx, cy, cx + cw, cy + ch, color);
            }
            CursorVisualStyle::UnderlineCursorBlink | CursorVisualStyle::UnderlineCursorSteady => {
                let bar_h = (ch * 0.1).max(2.0);
                push_quad(&mut verts, cx, cy + ch - bar_h, cx + cw, cy + ch, color);
            }
            CursorVisualStyle::VerticalLineCursorBlink
            | CursorVisualStyle::VerticalLineCursorSteady => {
                let bar_w = (cw * 0.1).max(1.0);
                push_quad(&mut verts, cx, cy, cx + bar_w, cy + ch, color);
            }
        }
    }

    verts
}

// ---------------------------------------------------------------------------
//  Build foreground instances
// ---------------------------------------------------------------------------

/// Build the foreground instance buffer from shaped lines.
///
/// For each shaped glyph: looks up the atlas entry (rasterising on miss) and
/// emits a single instance at the cell-grid position adjusted by the bearing offsets.
///
/// `opts.selection` is `Some((start_col, start_row, end_col, end_row))` in normalised
/// reading order.  Glyphs that fall within the selection use `SELECTION_FG_F`
/// instead of their normal foreground color.
///
/// Writes glyph instances into the caller-supplied `instances` buffer, which
/// is cleared first so that the existing heap allocation is reused across
/// frames (clear+extend pattern).
// All parameters are required: shaped lines, atlas, font manager, cell metrics,
// render options, theme, and the output buffer.
#[allow(clippy::too_many_arguments)]
pub fn build_foreground_instances(
    shaped_lines: &[Arc<ShapedLine>],
    atlas: &mut GlyphAtlas,
    font_manager: &FontManager,
    cell_height: u32,
    ascent: f32,
    opts: &FgRenderOptions,
    theme: &ThemePalette,
    instances: &mut Vec<f32>,
) {
    // Reuse existing heap allocation — clear but keep capacity.
    instances.clear();

    for (row_idx, line) in shaped_lines.iter().enumerate() {
        let cell_h_f = gl_f32_u32(cell_height);
        let row_params = RowGlyphParams::new(line.line_width, cell_h_f, row_idx, ascent);

        for run in &line.runs {
            let is_faint = run.font_decorations.contains(FontDecorations::Faint);
            // Task 115.2: effective foreground composes per-cell SGR-7
            // (already resolved in `colors.color()`) with whole-screen
            // DECSCNM by XOR via `effective_fg`.
            let normal_fg = internal_color_to_gl(
                effective_fg(&run.colors, opts.reverse_screen),
                is_faint,
                theme,
            );

            // Track the current column as we iterate glyphs within the run.
            let mut col = run.col_start;

            // Determine whether this run's glyphs should be visible based on
            // the blink state.  `BlinkState::None` is always visible.
            let run_visible = match run.blink {
                BlinkState::None => true,
                BlinkState::Slow => opts.text_blink_slow_visible,
                BlinkState::Fast => opts.text_blink_fast_visible,
            };

            for glyph in &run.glyphs {
                let fg_color =
                    if is_cell_selected(row_idx, col, opts.selection, opts.selection_is_block) {
                        selection_fg_f(theme)
                    } else {
                        normal_fg
                    };

                if run_visible {
                    emit_glyph_instance(
                        instances,
                        glyph,
                        atlas,
                        font_manager,
                        fg_color,
                        &row_params,
                    );
                }

                col += glyph.cell_width;
            }
        }
    }
}

// ---------------------------------------------------------------------------
//  Build image verts
// ---------------------------------------------------------------------------

/// Writes image vertex data into the caller-supplied `verts` buffer, which
/// is cleared first so that the existing heap allocation is reused across
/// frames (clear+extend pattern).
///
/// `placements` is parallel to `visible_chars`: one entry per cell in
/// row-major order.  `term_width` is the number of columns per row.
/// `cell_width` and `cell_height` are integer pixel sizes.
///
/// Emits `IMG_VERTEX_FLOATS` floats per vertex, `VERTS_PER_QUAD` vertices
/// per image quad.
// All parameters are required for image vertex generation: placements, cell dimensions,
// terminal size, and display area.
// `implicit_hasher`: `build_image_verts` is an internal function; generic hasher adds no value.
#[allow(clippy::too_many_arguments, clippy::implicit_hasher)]
pub fn build_image_verts(
    placements: &[Option<ImagePlacement>],
    snap_images: &std::collections::HashMap<u64, InlineImage>,
    term_width: usize,
    cell_width: u32,
    cell_height: u32,
    verts: &mut Vec<f32>,
    draw_order: &mut Vec<ImageDrawEntry>,
) {
    // Reuse existing heap allocation — clear but keep capacity.
    verts.clear();
    draw_order.clear();

    if placements.is_empty() || snap_images.is_empty() {
        return;
    }

    // Bucketed by `placement_instance` (Task 100.18), NOT `image_id` — two
    // independent placements of the same image id (e.g. two `a=p` puts with
    // `p=0`/unspecified) must land in two separate buckets so they render
    // as two separate quads instead of merging into one oversized quad.
    let mut bounds: std::collections::HashMap<u64, ImageBounds> = std::collections::HashMap::new();

    for (cell_idx, placement) in placements.iter().enumerate() {
        let Some(p) = placement else { continue };
        let col = cell_idx.checked_rem(term_width).unwrap_or(0);
        let row = cell_idx.checked_div(term_width).unwrap_or(0);

        let x0 = gl_f32(col) * gl_f32_u32(cell_width);
        let y0 = gl_f32(row) * gl_f32_u32(cell_height);
        let x1 = x0 + gl_f32_u32(cell_width);
        let y1 = y0 + gl_f32_u32(cell_height);

        let instance_id = p.placement_instance;
        let entry = bounds.entry(instance_id).or_insert(ImageBounds {
            x0,
            y0,
            x1,
            y1,
            min_col_in_image: p.col_in_image,
            min_row_in_image: p.row_in_image,
            max_col_in_image: p.col_in_image,
            max_row_in_image: p.row_in_image,
            image_id: p.image_id,
            z_index: p.z_index,
            crop: p.source_crop,
            subcell_offset: p.subcell_offset,
        });
        entry.x0 = entry.x0.min(x0);
        entry.y0 = entry.y0.min(y0);
        entry.x1 = entry.x1.max(x1);
        entry.y1 = entry.y1.max(y1);
        entry.min_col_in_image = entry.min_col_in_image.min(p.col_in_image);
        entry.min_row_in_image = entry.min_row_in_image.min(p.row_in_image);
        entry.max_col_in_image = entry.max_col_in_image.max(p.col_in_image);
        entry.max_row_in_image = entry.max_row_in_image.max(p.row_in_image);
    }

    // Emit quads in (z_index, instance_id) order so higher z-index
    // placements render above lower ones (ties broken by instance id for
    // determinism). `draw_images` iterates the SAME order (via
    // `draw_order`), so vertex slab N corresponds to `draw_order[N]` — the
    // vertex-emission order and the draw order are guaranteed identical
    // because they share this one authoritative list.
    draw_order.extend(bounds.iter().map(|(&instance_id, b)| ImageDrawEntry {
        instance_id,
        image_id: b.image_id,
    }));
    draw_order.sort_unstable_by_key(|entry| {
        let z = bounds.get(&entry.instance_id).map_or(0, |b| b.z_index);
        (z, entry.instance_id)
    });

    verts.reserve(draw_order.len() * VERTS_PER_QUAD * IMG_VERTEX_FLOATS);

    for entry in draw_order.iter() {
        let Some(b) = bounds.get(&entry.instance_id) else {
            continue;
        };

        let quad = compute_image_quad(b, snap_images.get(&entry.image_id), cell_width, cell_height);
        verts.extend_from_slice(&quad);
    }
}

// ---------------------------------------------------------------------------
//  Internal helpers
// ---------------------------------------------------------------------------

/// Compute the 6-vertex textured quad for a single image.
///
/// UV coordinates are derived from the image's `display_cols` / `display_rows`
/// — the cell-grid extent the Kitty protocol declared for this image.  The
/// visible portion of the image (determined by `ImageBounds` min/max
/// `col_in_image` / `row_in_image`) maps to the corresponding fraction of
/// the texture.  The full texture (UV 0..1) covers the full `display_cols` ×
/// `display_rows` grid; partial visibility (e.g. image clipped at the terminal
/// edge) produces a UV sub-range.
///
/// The quad's PIXEL EXTENT depends on the image's [`ImageSizeMode`]
/// (Task 100.17b):
///
/// - `ExplicitCells` (kitty `c=`/`r=`, iTerm2 explicit `width=`/`height=`),
///   or when image metadata is unavailable: the quad fills the bounding box
///   `b` exactly — the image is scaled to fill the declared cell grid. This
///   is the pre-100.17 behaviour, unchanged.
/// - `NativePixels` (kitty without `c=`/`r=`, iTerm2 `auto`, and ALWAYS for
///   sixel — the spec-mandated default): the quad is anchored at the cell
///   top-left `(b.x0, b.y0)` and sized to the image's *native* pixel
///   dimensions instead of stretching to fill the cell box. See
///   [`compute_image_quad_position`] for how this interacts with
///   source-crop and partial cell-grid visibility.
///
/// If `b.crop` is set (kitty `a=p` source-crop, Task 100.9), the cell-grid
/// UV fractions above are re-mapped into the crop's pixel-space UV window
/// instead of the full texture, so the placement displays only that
/// sub-rectangle of the transmitted image. For `NativePixels` images the
/// crop's pixel dimensions also become the quad's native extent (rather
/// than the full image's), so a cropped native-size image is drawn at the
/// crop's actual pixel size.
///
/// If `b.subcell_offset` is set (kitty `a=p`/`a=T` `X=`/`Y=`, Task 100.19),
/// the quad's position (all four corners) is translated by that many
/// pixels after [`compute_image_quad_position`] resolves the base
/// position — orthogonal to both size mode (a position-only shift) and
/// crop (which only affects UVs, not position). Defensively re-clamped to
/// `< cell_width`/`< cell_height` here as well, in case a future caller
/// constructs an `ImageBounds` directly without going through the
/// resolving handler's own clamp.
fn compute_image_quad(
    b: &ImageBounds,
    img: Option<&InlineImage>,
    cell_width: u32,
    cell_height: u32,
) -> [f32; 4 * VERTS_PER_QUAD] {
    let (u0, v0, u1, v1) = if let Some(img) = img
        && img.display_cols > 0
        && img.display_rows > 0
    {
        let dc = gl_f32(img.display_cols);
        let dr = gl_f32(img.display_rows);

        // Map the visible cell range to the corresponding UV sub-range.
        let u0 = gl_f32(b.min_col_in_image) / dc;
        let v0 = gl_f32(b.min_row_in_image) / dr;
        let u1 = gl_f32(b.max_col_in_image + 1) / dc;
        let v1 = gl_f32(b.max_row_in_image + 1) / dr;

        (u0, v0, u1, v1)
    } else {
        // Fallback: map full texture if image metadata is unavailable.
        (0.0, 0.0, 1.0, 1.0)
    };

    // Compose a kitty `a=p` source-crop (Task 100.9) on top of the
    // cell-visibility UV fractions computed above: the crop defines the
    // outer texture-space UV window `[cu0,cu1]x[cv0,cv1]`, and the
    // cell-visibility fractions `u0..u1`/`v0..v1` are re-mapped INTO that
    // window via lerp. When there's no crop, the UVs are unchanged.
    let (u0, v0, u1, v1) = match (b.crop, img) {
        (Some(crop), Some(img)) if img.width_px > 0 && img.height_px > 0 => {
            let w = gl_f32_u32(img.width_px);
            let h = gl_f32_u32(img.height_px);
            let cu0 = gl_f32_u32(crop.x) / w;
            let cu1 = gl_f32_u32(crop.x.saturating_add(crop.width)) / w;
            let cv0 = gl_f32_u32(crop.y) / h;
            let cv1 = gl_f32_u32(crop.y.saturating_add(crop.height)) / h;
            (
                (cu1 - cu0).mul_add(u0, cu0),
                (cv1 - cv0).mul_add(v0, cv0),
                (cu1 - cu0).mul_add(u1, cu0),
                (cv1 - cv0).mul_add(v1, cv0),
            )
        }
        _ => (u0, v0, u1, v1),
    };

    let (qx0, qy0, qx1, qy1) = compute_image_quad_position(b, img);

    // Apply the kitty `X=`/`Y=` sub-cell pixel offset (Task 100.19) as a
    // pure translation of all four corners — position only, independent of
    // size mode and crop (both already resolved above).
    let (qx0, qy0, qx1, qy1) = b.subcell_offset.map_or((qx0, qy0, qx1, qy1), |offset| {
        let dx = gl_f32_u32(offset.x.min(cell_width.saturating_sub(1)));
        let dy = gl_f32_u32(offset.y.min(cell_height.saturating_sub(1)));
        (qx0 + dx, qy0 + dy, qx1 + dx, qy1 + dy)
    });

    [
        // Triangle 1
        qx0, qy0, u0, v0, qx1, qy0, u1, v0, qx0, qy1, u0, v1, // Triangle 2
        qx1, qy0, u1, v0, qx1, qy1, u1, v1, qx0, qy1, u0, v1,
    ]
}

/// Compute the pixel-space quad rectangle `(x0, y0, x1, y1)` for an image,
/// honoring its [`ImageSizeMode`] (Task 100.17b).
///
/// - `ImageSizeMode::ExplicitCells`, or when `img` is `None` (metadata
///   unavailable): returns the cell bounding box `b` unchanged — the
///   pre-100.17 behaviour.
/// - `ImageSizeMode::NativePixels`: returns a quad anchored at the cell
///   top-left `(b.x0, b.y0)` and sized to the image's native pixel
///   dimensions. If a source-crop (`b.crop`, Task 100.9) is active, the
///   crop's pixel dimensions are used instead of the full image's — the
///   displayed native size is the size of the cropped region, not the
///   original image.
///
///   When only part of the image's cell grid is visible (`b`'s
///   `min`/`max_col_in_image` / `row_in_image` span narrower than the
///   full `display_cols` × `display_rows` grid — e.g. the placement is
///   clipped at the terminal's right/bottom edge or by scroll), the
///   native extent is scaled by that same visible-cell fraction, mirroring
///   the UV sub-range computed in [`compute_image_quad`]. This keeps quad
///   geometry and texture sampling proportionally aligned in both the
///   fully-visible case (fraction == 1, full native size) and the
///   partially-visible case, without needing a separate fallback path.
///
/// Sub-cell X/Y pixel offset (kitty `X=`/`Y=`, Task 100.19) is NOT applied
/// here — this function returns the quad anchored exactly at the cell
/// origin; the offset translation is applied by the caller,
/// [`compute_image_quad`], after this function returns.
fn compute_image_quad_position(b: &ImageBounds, img: Option<&InlineImage>) -> (f32, f32, f32, f32) {
    let Some(img) = img else {
        return (b.x0, b.y0, b.x1, b.y1);
    };

    if img.size_mode == ImageSizeMode::ExplicitCells {
        return (b.x0, b.y0, b.x1, b.y1);
    }

    // NativePixels: the crop's pixel size (if any) takes precedence over
    // the full image's — a cropped native-size image displays at the
    // crop's actual dimensions. `resolve_source_crop` (terminal-emulator
    // side) never stores a zero-sized crop, but we guard defensively
    // rather than trust that invariant across the snapshot boundary.
    let (native_w, native_h) = match b.crop {
        Some(crop) if crop.width > 0 && crop.height > 0 => (crop.width, crop.height),
        _ => (img.width_px, img.height_px),
    };

    if native_w == 0 || native_h == 0 || img.display_cols == 0 || img.display_rows == 0 {
        // Degenerate metadata — fall back to the cell bounding box rather
        // than emit a zero-size (invisible) quad.
        return (b.x0, b.y0, b.x1, b.y1);
    }

    // Fraction of the image's declared cell grid that is actually visible
    // in `b`, in each axis. `max >= min` always holds by construction
    // (bounds are seeded with `min == max` and only ever widened).
    let visible_cols = gl_f32(b.max_col_in_image - b.min_col_in_image + 1);
    let visible_rows = gl_f32(b.max_row_in_image - b.min_row_in_image + 1);
    let display_cols = gl_f32(img.display_cols);
    let display_rows = gl_f32(img.display_rows);

    let w = gl_f32_u32(native_w) * (visible_cols / display_cols);
    let h = gl_f32_u32(native_h) * (visible_rows / display_rows);

    (b.x0, b.y0, b.x0 + w, b.y0 + h)
}

/// Per-row rendering parameters for glyph emission: scaling factors,
/// baseline position, and cell vertical extent.  Bundled to keep
/// [`emit_glyph_instance`] within the 7-argument lint limit.
struct RowGlyphParams {
    /// Horizontal scale factor (1.0 for normal, 2.0 for double-width/height).
    x_scale: f32,
    /// Vertical scale factor (1.0 for normal, 2.0 for DECDHL).
    y_scale: f32,
    /// Vertical offset applied to the baseline before scaling.  For DECDHL
    /// bottom halves this shifts the virtual origin up by one cell height so
    /// the cell-boundary clip keeps the lower half of the 2× glyph.
    y_origin_shift: f32,
    /// Baseline y-coordinate for this row (in pixels).
    baseline_y: f32,
    /// Cell vertical extent `[top, bottom]` for clipping oversized glyphs.
    cell_y_range: [f32; 2],
}

impl RowGlyphParams {
    /// Build row parameters from a `LineWidth`, cell height, and row index.
    fn new(lw: LineWidth, cell_h_f: f32, row_idx: usize, ascent: f32) -> Self {
        let row_f = gl_f32(row_idx);
        let baseline_y = row_f.mul_add(cell_h_f, ascent);
        let cell_top = row_f * cell_h_f;
        let cell_bottom = cell_top + cell_h_f;
        let (y_scale, y_origin_shift) = match lw {
            LineWidth::Normal | LineWidth::DoubleWidth => (1.0, 0.0),
            LineWidth::DoubleHeightTop => (2.0, 0.0),
            LineWidth::DoubleHeightBottom => (2.0, -cell_h_f),
        };
        let x_scale_val = x_scale(lw);

        Self {
            x_scale: x_scale_val,
            y_scale,
            y_origin_shift,
            baseline_y,
            cell_y_range: [cell_top, cell_bottom],
        }
    }
}

/// Fraction of the cell height left as vertical breathing room around a
/// fitted color emoji (split evenly top and bottom).  An emoji is scaled so
/// that its height occupies `1.0 - COLOR_GLYPH_CELL_MARGIN` of the cell,
/// giving the "sized to the line with a small gap" appearance rather than
/// filling the cell edge-to-edge.
const COLOR_GLYPH_CELL_MARGIN: f32 = 0.12;

/// A fitted axis-aligned quad for a color glyph: top-left corner and size in
/// pixels, in the same coordinate space as the row's cell box.
#[derive(Debug, Clone, Copy, PartialEq)]
struct FittedColorRect {
    x0: f32,
    y0: f32,
    width: f32,
    height: f32,
}

/// Scale a color (emoji) glyph to fit the cell box, preserving aspect ratio.
///
/// Color emoji are rasterised at their native bitmap-strike size (swash's
/// `StrikeWith::BestFit` does not downscale to the requested ppem), so the
/// raw glyph dimensions routinely exceed the cell.  Rather than crop the
/// glyph to the cell (which cuts it off and minifies blurrily), we scale it
/// uniformly so its height fits the cell minus [`COLOR_GLYPH_CELL_MARGIN`],
/// then centre it within the glyph's advance box.
///
/// `advance_w` is the glyph's horizontal advance in pixels (the cell box
/// width for a single-width emoji, already scaled for DECDWL).  `cell_top`
/// and `cell_h` describe the cell's vertical extent.  The glyph is centred
/// vertically within the cell.
///
/// Glyphs already small enough are only down-shifted to centre; they are
/// never enlarged (`scale` is clamped to `<= 1.0`), preserving crisp small
/// emoji.
fn fit_color_glyph_rect(
    glyph_w: f32,
    glyph_h: f32,
    x_origin: f32,
    advance_w: f32,
    cell_top: f32,
    cell_h: f32,
) -> FittedColorRect {
    if glyph_w <= 0.0 || glyph_h <= 0.0 || cell_h <= 0.0 {
        return FittedColorRect {
            x0: x_origin,
            y0: cell_top,
            width: glyph_w.max(0.0),
            height: glyph_h.max(0.0),
        };
    }

    let target_h = cell_h * (1.0 - COLOR_GLYPH_CELL_MARGIN);

    // Uniform scale: fit within target height AND the advance width, never
    // enlarging beyond native size.
    let scale_h = target_h / glyph_h;
    let scale_w = if advance_w > 0.0 && glyph_w > advance_w {
        advance_w / glyph_w
    } else {
        1.0
    };
    let scale = scale_h.min(scale_w).min(1.0);

    let width = glyph_w * scale;
    let height = glyph_h * scale;

    // Centre horizontally within the advance box and vertically within the cell.
    let x0 = (advance_w - width).mul_add(0.5, x_origin);
    let y0 = (cell_h - height).mul_add(0.5, cell_top);

    FittedColorRect {
        x0,
        y0,
        width,
        height,
    }
}

/// Emit a procedural box-drawing / block-element glyph (Task #410).
///
/// The bitmap is generated at the exact cell pixel size and the quad spans the
/// cell rectangle **exactly** — `[cell_left, cell_top]` to
/// `[cell_right, cell_bottom]`, with the row's DECDWL/DECDHL scale applied — so
/// consecutive cells tile with no seam. This deliberately bypasses the
/// baseline/bearing/clip path used for font glyphs.
fn emit_procedural_glyph(
    instances: &mut Vec<f32>,
    glyph: &ShapedGlyph,
    atlas: &mut GlyphAtlas,
    font_manager: &FontManager,
    fg_color: [f32; 4],
    row_params: &RowGlyphParams,
) {
    use conv2::{ApproxFrom, RoundToNearest};

    let cell_top = row_params.cell_y_range[0];
    let cell_bottom = row_params.cell_y_range[1];
    let cell_w = font_manager.cell_width();
    let cell_h = font_manager.cell_height();

    // The generated bitmap fills the whole cell, so `bearing_y` (distance from
    // the baseline up to the glyph top) equals the baseline-to-cell-top
    // distance. It is unused by the exact-cell placement below, but the atlas
    // entry records it for completeness.
    let bearing_y: i16 =
        <i16 as ApproxFrom<f32, RoundToNearest>>::approx_from(row_params.baseline_y - cell_top)
            .unwrap_or(0);

    let entry = match atlas.get_or_insert_procedural(glyph.source_char, cell_w, cell_h, bearing_y) {
        Some(e) => e.clone(),
        None => return,
    };
    if entry.width == 0 || entry.height == 0 {
        return;
    }
    let [u0, v0, u1, v1] = entry.uv_rect;

    // Exact cell rectangle, with DECDWL (x_scale) / DECDHL (y_scale,
    // y_origin_shift) applied so double-width/height rows still fill correctly.
    let x0 = glyph.x_px * row_params.x_scale;
    let width = gl_f32_u32(cell_w) * row_params.x_scale;
    let cell_pixel_h = cell_bottom - cell_top;
    let y0 = cell_top.mul_add(1.0, row_params.y_origin_shift);
    let height = cell_pixel_h * row_params.y_scale;

    instances.extend_from_slice(&[
        x0,
        y0,
        width,
        height,
        u0,
        v0,
        u1,
        v1,
        fg_color[0],
        fg_color[1],
        fg_color[2],
        fg_color[3],
        0.0,
    ]);
}

/// Inputs for [`emit_normalized_fallback_glyph`], bundled to stay within the
/// argument-count lint.
struct NormalizedFallbackGlyph {
    /// Cell-grid x origin of the glyph (pixels, pre-`x_scale`).
    x_px: f32,
    /// Horizontal bearing of the rasterised glyph.
    bearing_x: i16,
    /// Vertical bearing (baseline to glyph top, positive = up) of the glyph.
    bearing_y: i16,
    /// Rasterised glyph width in pixels.
    glyph_w: u16,
    /// Rasterised glyph height in pixels.
    glyph_h: u16,
    /// The fallback face's own cell height at the current ppem.
    fb_cell_h: f32,
    /// The fallback face's own baseline (cell-top to baseline) at the ppem.
    fb_baseline: f32,
    /// The fallback face's own cell width at the current ppem.
    fb_cell_w: f32,
    /// The primary face's cell width in pixels (pre-`x_scale`).
    primary_cell_w: f32,
    /// Atlas UV rect `[u0, v0, u1, v1]`.
    uv: [f32; 4],
}

/// Normalise and emit a glyph if it was resolved from a **fallback** face
/// (Task #411); returns `true` if it handled the glyph.
///
/// A glyph from a face other than the user-selected primary (bundled fallback
/// or a system face) was designed against *that* font's cell, not the
/// primary's. Placing it at the primary baseline with the primary metrics
/// mis-sizes and mis-centres it — most visibly, full-cell Nerd Font powerline
/// separators clip at the top. Returns `false` for primary-face glyphs (which
/// drive the grid directly and take the normal placement path).
fn try_emit_fallback_glyph(
    instances: &mut Vec<f32>,
    glyph: &ShapedGlyph,
    entry: &AtlasEntry,
    font_manager: &FontManager,
    fg_color: [f32; 4],
    row_params: &RowGlyphParams,
) -> bool {
    let Some((fb_cell_h, fb_baseline, fb_cell_w)) =
        font_manager.fallback_cell_metrics(glyph.face_id)
    else {
        return false;
    };
    if fb_cell_h <= 0.0 || fb_cell_w <= 0.0 {
        return false;
    }
    emit_normalized_fallback_glyph(
        instances,
        &NormalizedFallbackGlyph {
            x_px: glyph.x_px,
            bearing_x: entry.bearing_x,
            bearing_y: entry.bearing_y,
            glyph_w: entry.width,
            glyph_h: entry.height,
            fb_cell_h,
            fb_baseline,
            fb_cell_w,
            primary_cell_w: gl_f32_u32(font_manager.cell_width()),
            uv: entry.uv_rect,
        },
        fg_color,
        row_params,
    );
    true
}

/// Emit a glyph resolved from a **fallback** face, normalised into the primary
/// cell (Task #411).
///
/// The glyph was designed against the fallback font's own cell
/// (`fb_cell_w` × `fb_cell_h`, baseline at `fb_baseline`). We map its natural
/// box within that cell into the primary cell using **independent** per-axis
/// ratios — `sx = primary_cell_w / fb_cell_w` horizontally and
/// `sy = primary_cell_h / fb_cell_h` vertically. Using the height ratio for
/// both axes (as an earlier version did) mis-sizes the width whenever the
/// fallback font's aspect ratio differs from the primary's, re-introducing the
/// hairline-gap / over-fill class of bug for full-cell fallback glyphs
/// (powerline separators). This makes full-cell glyphs fill the primary cell
/// exactly and keeps partial icons proportional to the cell — with the actual
/// pixel scaling done by the GPU (we only emit a resized quad). DECDWL/DECDHL
/// row scaling is applied on top.
fn emit_normalized_fallback_glyph(
    instances: &mut Vec<f32>,
    g: &NormalizedFallbackGlyph,
    fg_color: [f32; 4],
    row_params: &RowGlyphParams,
) {
    let cell_top = row_params.cell_y_range[0];
    let cell_bottom = row_params.cell_y_range[1];
    let primary_cell_h = cell_bottom - cell_top;
    if primary_cell_h <= 0.0 || g.fb_cell_w <= 0.0 || g.glyph_w == 0 || g.glyph_h == 0 {
        return;
    }

    // Independent per-axis ratios: map the fallback face's cell onto the
    // primary cell without distorting the width by the height ratio.
    let sy = primary_cell_h / g.fb_cell_h;
    let sx = g.primary_cell_w / g.fb_cell_w;

    // The glyph's natural box within the fallback face's own cell.
    let natural_top = g.fb_baseline - f32::from(g.bearing_y);
    let natural_left = f32::from(g.bearing_x);

    // Scale that box into the primary cell, then apply the row's vertical scale
    // and origin shift (DECDHL) and horizontal scale (DECDWL).
    let scaled_h = f32::from(g.glyph_h) * sy * row_params.y_scale;
    let scaled_w = f32::from(g.glyph_w) * sx * row_params.x_scale;

    let y0 = (natural_top * sy).mul_add(row_params.y_scale, cell_top + row_params.y_origin_shift);
    let x0 = natural_left.mul_add(sx, g.x_px) * row_params.x_scale;

    let [u0, v0, u1, v1] = g.uv;
    instances.extend_from_slice(&[
        x0,
        y0,
        scaled_w,
        scaled_h,
        u0,
        v0,
        u1,
        v1,
        fg_color[0],
        fg_color[1],
        fg_color[2],
        fg_color[3],
        0.0,
    ]);
}

/// Emit a single foreground glyph instance (13 floats).
///
/// Looks up (or rasterises) the atlas entry for the glyph, then pushes one
/// instance into `instances`.  Monochrome glyphs that extend beyond the
/// cell's vertical extent are clipped (Nerd Font powerline/icon glyphs are
/// intended to fill or overflow the cell), with UVs adjusted proportionally.
/// Color emoji are instead scaled to fit the cell (see
/// [`fit_color_glyph_rect`]) so they are not cropped or minified.
///
/// `row_params` carries per-row scaling and baseline data for DECDWL / DECDHL.
fn emit_glyph_instance(
    instances: &mut Vec<f32>,
    glyph: &ShapedGlyph,
    atlas: &mut GlyphAtlas,
    font_manager: &FontManager,
    fg_color: [f32; 4],
    row_params: &RowGlyphParams,
) {
    use conv2::{ApproxFrom, RoundToNearest};

    let cell_top = row_params.cell_y_range[0];
    let cell_bottom = row_params.cell_y_range[1];

    // Procedural box-drawing / block-element glyphs (Task #410) are drawn to
    // fill the cell rectangle EXACTLY, so they tile with their neighbours with
    // no seam. They bypass the font-glyph baseline/bearing/clip math entirely
    // (that math accumulates sub-pixel error that shows up as hairline gaps
    // between rows). Only single-cell-wide glyphs qualify.
    if crate::gui::box_drawing::is_procedural(glyph.source_char) && glyph.cell_width == 1 {
        emit_procedural_glyph(instances, glyph, atlas, font_manager, fg_color, row_params);
        return;
    }

    // Rasterize glyphs at the font's actual pixels-per-em — the SAME size the
    // cell metrics (ascent/descent/baseline/cell width) were computed at — not
    // the cell *height*. The cell height can be larger than the font ppem
    // (e.g. Nerd Fonts inflate it via the OS/2 win-metrics floor), and
    // rasterizing at that inflated size scales every glyph by the wrong factor,
    // making text visibly too large and top-heavy within the cell.
    let size_px: u16 =
        <u16 as ApproxFrom<f32, RoundToNearest>>::approx_from(font_manager.rasterization_ppem())
            .unwrap_or(u16::MAX);

    let key = GlyphKey {
        glyph_id: glyph.glyph_id,
        face_id: glyph.face_id,
        size_px,
    };

    let entry = match atlas.get_or_insert(key, font_manager) {
        Some(e) => e.clone(),
        None => return, // Rasterisation failed — skip glyph.
    };

    // Zero-size glyphs (space) have no geometry.
    if entry.width == 0 || entry.height == 0 {
        return;
    }

    let [u0, v0, u1, v1] = entry.uv_rect;

    // Color emoji take a separate geometry path: rather than crop an
    // oversized bitmap to the cell, scale it to fit (see
    // `fit_color_glyph_rect`).  UVs are passed through unmodified because the
    // whole glyph is shown.
    if glyph.is_color {
        // Advance box width: the glyph's cell span times the cell width, with
        // the row's horizontal scale applied (DECDWL).
        let advance_w = gl_f32_u32(font_manager.cell_width())
            * gl_f32(glyph.cell_width.max(1))
            * row_params.x_scale;
        let x_origin = glyph.x_px * row_params.x_scale;
        let cell_h = cell_bottom - cell_top;
        let fitted = fit_color_glyph_rect(
            f32::from(entry.width) * row_params.x_scale,
            f32::from(entry.height) * row_params.y_scale,
            x_origin,
            advance_w,
            cell_top + row_params.y_origin_shift,
            cell_h,
        );
        if fitted.width <= 0.0 || fitted.height <= 0.0 {
            return;
        }
        instances.extend_from_slice(&[
            fitted.x0,
            fitted.y0,
            fitted.width,
            fitted.height,
            u0,
            v0,
            u1,
            v1,
            fg_color[0],
            fg_color[1],
            fg_color[2],
            fg_color[3],
            1.0,
        ]);
        return;
    }

    // Fallback-face glyphs (Task #411) are normalised into the primary cell —
    // see `try_emit_fallback_glyph`.
    if try_emit_fallback_glyph(instances, glyph, &entry, font_manager, fg_color, row_params) {
        return;
    }

    // Pixel position: cell-grid x + bearing, baseline_y - bearing_y.
    // Apply horizontal and vertical scaling for DECDWL / DECDHL rows.
    let x0 = (glyph.x_px + f32::from(entry.bearing_x)) * row_params.x_scale;
    let x1 = f32::from(entry.width).mul_add(row_params.x_scale, x0);

    // Vertical position: scale the glyph's offset from the cell top, then
    // apply `y_origin_shift` (negative for DECDHL bottom half to show the
    // lower half of the 2× glyph).
    let relative_y = row_params.baseline_y - cell_top - f32::from(entry.bearing_y);
    let raw_y0 = row_params
        .y_scale
        .mul_add(relative_y, cell_top + row_params.y_origin_shift);
    let raw_y1 = f32::from(entry.height).mul_add(row_params.y_scale, raw_y0);

    // --- Cell-boundary clipping ---
    //
    // Oversized glyphs (e.g. powerline symbols in Nerd Fonts) may extend
    // above or below the cell.  Clamp the quad to the cell's vertical
    // extent and adjust the UV coordinates proportionally so only the
    // visible portion of the atlas texture is sampled.
    let glyph_h = raw_y1 - raw_y0;
    let (y0, v0_adj) = if raw_y0 < cell_top {
        // Glyph extends above the cell — clip the top.
        let frac = (cell_top - raw_y0) / glyph_h;
        (cell_top, frac.mul_add(v1 - v0, v0))
    } else {
        (raw_y0, v0)
    };
    let (y1, v1_adj) = if raw_y1 > cell_bottom {
        // Glyph extends below the cell — clip the bottom.
        let frac = (raw_y1 - cell_bottom) / glyph_h;
        (cell_bottom, frac.mul_add(-(v1 - v0), v1))
    } else {
        (raw_y1, v1)
    };

    // After clipping the quad may have been fully culled.
    if y0 >= y1 {
        return;
    }

    // One instance: glyph_x, glyph_y, glyph_w, glyph_h, u0, v0, u1, v1, r, g, b, a, is_color.
    // Color emoji took the early-return path above; this is always a
    // monochrome glyph, so `is_color` is 0.0.
    instances.extend_from_slice(&[
        x0,
        y0,
        x1 - x0,
        y1 - y0,
        u0,
        v0_adj,
        u1,
        v1_adj,
        fg_color[0],
        fg_color[1],
        fg_color[2],
        fg_color[3],
        0.0,
    ]);
}

// ---------------------------------------------------------------------------
//  Quad geometry helper
// ---------------------------------------------------------------------------

/// Push a solid-color axis-aligned quad (2 triangles = 6 vertices) into `verts`.
///
/// Vertex layout: `x, y, r, g, b, a`.
pub(super) fn push_quad(verts: &mut Vec<f32>, x0: f32, y0: f32, x1: f32, y1: f32, color: [f32; 4]) {
    let [r, g, b, a] = color;
    let quad = [
        x0, y0, r, g, b, a, x1, y0, r, g, b, a, x0, y1, r, g, b, a, x1, y0, r, g, b, a, x1, y1, r,
        g, b, a, x0, y1, r, g, b, a,
    ];
    verts.extend_from_slice(&quad);
}

// ---------------------------------------------------------------------------
//  Underline style geometry
// ---------------------------------------------------------------------------

/// Number of segments used to approximate a sine wave for curly underlines.
const CURLY_SEGMENTS: u32 = 8;

/// Parameters for underline rendering, bundled to stay within the 7-argument
/// lint limit.
struct UnderlineParams {
    x0: f32,
    x1: f32,
    ul_y: f32,
    /// Clamped stroke thickness: `stroke.max(1.0)`.  Computed once at
    /// construction so style-specific helpers share a single clamp.
    thick: f32,
    cell_width: f32,
    color: [f32; 4],
}

/// Push decoration quads for the given underline style.
///
/// All styles share the same vertical anchor (`ul_y`, the font's underline
/// position below the baseline).  `thick` is the clamped line thickness
/// (`stroke.max(1.0)`) and `cell_width` is the cell width in pixels.
fn push_underline_quads(deco: &mut Vec<f32>, style: UnderlineStyle, p: &UnderlineParams) {
    match style {
        UnderlineStyle::None => {}
        UnderlineStyle::Single => {
            push_quad(deco, p.x0, p.ul_y, p.x1, p.ul_y + p.thick, p.color);
        }
        UnderlineStyle::Double => {
            let gap = p.thick.mul_add(2.0, 1.0);
            push_quad(deco, p.x0, p.ul_y, p.x1, p.ul_y + p.thick, p.color);
            push_quad(
                deco,
                p.x0,
                p.ul_y + gap,
                p.x1,
                p.ul_y + gap + p.thick,
                p.color,
            );
        }
        UnderlineStyle::Curly => {
            push_curly_underline(deco, p);
        }
        UnderlineStyle::Dotted => {
            push_dotted_underline(deco, p);
        }
        UnderlineStyle::Dashed => {
            push_dashed_underline(deco, p);
        }
    }
}

/// Push quads approximating a sine-wave curly underline.
///
/// The wave amplitude is `2 * thick` and the period is one cell width.
/// Each cell is subdivided into [`CURLY_SEGMENTS`] vertical-strip quads whose
/// top and bottom edges follow the sine curve.
fn push_curly_underline(deco: &mut Vec<f32>, p: &UnderlineParams) {
    let amplitude = p.thick * 2.0;
    let span = p.x1 - p.x0;
    if span <= 0.0 || p.cell_width <= 0.0 {
        return;
    }

    let seg_width = p.cell_width / gl_f32_u32(CURLY_SEGMENTS);
    let total_segs_f = (span / seg_width).ceil();
    let total_segs = total_segs_f.approx_as::<usize>().unwrap_or(0);

    for i in 0..total_segs {
        let sx = seg_width.mul_add(gl_f32(i), p.x0);
        let ex = (sx + seg_width).min(p.x1);

        // Phase within the sine period (0..2π per cell width).
        let phase_start = (sx - p.x0) / p.cell_width * std::f32::consts::TAU;
        let phase_end = (ex - p.x0) / p.cell_width * std::f32::consts::TAU;

        let y_start = amplitude.mul_add(phase_start.sin(), p.ul_y);
        let y_end = amplitude.mul_add(phase_end.sin(), p.ul_y);

        let y_min = y_start.min(y_end);
        let y_max = y_start.max(y_end) + p.thick;

        push_quad(deco, sx, y_min, ex, y_max, p.color);
    }
}

/// Push quads for a dotted underline.
///
/// Each dot is a square with side = `thick`, with a gap of `thick` between
/// dots.
fn push_dotted_underline(deco: &mut Vec<f32>, p: &UnderlineParams) {
    let step = p.thick * 2.0;
    if step <= 0.0 {
        return;
    }

    let span = p.x1 - p.x0;
    let dot_count_f = (span / step).ceil();
    let dot_count = dot_count_f.approx_as::<usize>().unwrap_or(0);

    for i in 0..dot_count {
        let x = step.mul_add(gl_f32(i), p.x0);
        let dot_end = (x + p.thick).min(p.x1);
        push_quad(deco, x, p.ul_y, dot_end, p.ul_y + p.thick, p.color);
    }
}

/// Push quads for a dashed underline.
///
/// Each dash is `cell_width / 2` wide with a gap of `cell_width / 4`.
fn push_dashed_underline(deco: &mut Vec<f32>, p: &UnderlineParams) {
    let dash_len = (p.cell_width * 0.5).max(2.0);
    let gap_len = (p.cell_width * 0.25).max(1.0);
    let step = dash_len + gap_len;
    if step <= 0.0 {
        return;
    }

    let span = p.x1 - p.x0;
    let dash_count_f = (span / step).ceil();
    let dash_count = dash_count_f.approx_as::<usize>().unwrap_or(0);

    for i in 0..dash_count {
        let x = step.mul_add(gl_f32(i), p.x0);
        let dash_end = (x + dash_len).min(p.x1);
        push_quad(deco, x, p.ul_y, dash_end, p.ul_y + p.thick, p.color);
    }
}

// ---------------------------------------------------------------------------
//  Small helpers
// ---------------------------------------------------------------------------

/// Return the total column count covered by a `ShapedRun`.
/// Emit background cell instances for a run, applying horizontal scaling.
///
/// For normal rows (`scale == 1.0`), emits one instance per logical column.
/// For double-width rows (`scale == 2.0`), each logical column maps to 2
/// physical cell slots so the background spans the correct width.
fn emit_bg_cells(
    instances: &mut Vec<f32>,
    col_start: usize,
    col_count: usize,
    row_idx: usize,
    scale: f32,
    color: [f32; 4],
) {
    let row_f = gl_f32(row_idx);
    let [r, g, b, a] = color;

    if scale > 1.5 {
        // Double-width: each logical column produces 2 physical instances.
        for c in 0..col_count {
            let base = (col_start + c) * 2;
            for phys in 0..2_usize {
                instances.push(gl_f32(base + phys));
                instances.push(row_f);
                instances.push(r);
                instances.push(g);
                instances.push(b);
                instances.push(a);
            }
        }
    } else {
        for c in 0..col_count {
            instances.push(gl_f32(col_start + c));
            instances.push(row_f);
            instances.push(r);
            instances.push(g);
            instances.push(b);
            instances.push(a);
        }
    }
}

pub(super) fn run_col_count(run: &super::super::shaping::ShapedRun) -> usize {
    run.glyphs.iter().map(|g| g.cell_width).sum()
}

/// Check whether a cell at `(row, col)` falls within the normalised selection
/// `(start_col, start_row, end_col, end_row)`.
///
/// When `is_block` is `true` the selection is rectangular: every row in the
/// range is considered selected between `start_col` and `end_col` (inclusive).
/// When `is_block` is `false` the standard linear (stream) selection logic
/// applies.
pub(super) fn is_cell_selected(
    row: usize,
    col: usize,
    selection: Option<(usize, usize, usize, usize)>,
    is_block: bool,
) -> bool {
    let Some((sel_start_col, sel_start_row, sel_end_col, sel_end_row)) = selection else {
        return false;
    };

    if row < sel_start_row || row > sel_end_row {
        return false;
    }

    if is_block {
        // Block selection: same column range on every row.
        let col_min = sel_start_col.min(sel_end_col);
        let col_max = sel_start_col.max(sel_end_col);
        return col >= col_min && col <= col_max;
    }

    if sel_start_row == sel_end_row {
        // Single-row selection.
        return col >= sel_start_col && col <= sel_end_col;
    }

    if row == sel_start_row {
        col >= sel_start_col
    } else if row == sel_end_row {
        col <= sel_end_col
    } else {
        // Middle rows are fully selected.
        true
    }
}

/// Extract a rectangular region from the full atlas pixel buffer into a
/// contiguous `Vec<u8>` suitable for `gl.tex_sub_image_2d()`.
pub(super) fn extract_atlas_rect(
    pixels: &[u8],
    atlas_size: u32,
    rect: &super::super::atlas::DirtyRect,
) -> Vec<u8> {
    // `u32 -> usize` is lossless on all 64-bit targets; `value_from` degrades
    // gracefully on hypothetical 32-bit hosts by returning an empty region
    // rather than panicking.
    let usize_from_u32 = |v: u32| usize::value_from(v).unwrap_or(0);
    let stride = usize_from_u32(atlas_size).saturating_mul(4);
    let row_bytes = usize_from_u32(rect.width).saturating_mul(4);
    let height_usize = usize_from_u32(rect.height);
    let mut out = Vec::with_capacity(height_usize.saturating_mul(row_bytes));

    for row in 0..rect.height {
        let y = usize_from_u32(rect.y.saturating_add(row));
        let x = usize_from_u32(rect.x);
        let offset = y * stride + x * 4;
        let end = offset + row_bytes;
        if end <= pixels.len() {
            out.extend_from_slice(&pixels[offset..end]);
        }
    }

    out
}

// ---------------------------------------------------------------------------
//  Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use freminal_common::buffer_states::cursor::CursorPos;
    use freminal_common::buffer_states::fonts::FontDecorationFlags;
    use freminal_common::config::Config;
    use freminal_common::themes;

    use crate::gui::font_manager::FontManager;
    use crate::gui::shaping::{ShapedGlyph, ShapedLine, ShapedRun};
    use freminal_common::buffer_states::cursor::{ReverseVideo, StateColors};
    use freminal_common::buffer_states::fonts::FontWeight;
    use freminal_common::colors::TerminalColor;
    use freminal_terminal_emulator::{AnimationControl, ImageProtocol, ImageSizeMode};

    /// Default `StateColors` for test runs.
    fn default_colors() -> StateColors {
        StateColors::default()
    }

    /// Build a single `ShapedLine` with one run containing `n_glyphs` identical
    /// ASCII glyphs.  `cell_width_f32` is the cell width for x positions.
    fn make_line(
        n_glyphs: usize,
        cell_w: f32,
        colors: StateColors,
        decorations: FontDecorationFlags,
    ) -> Arc<ShapedLine> {
        use crate::gui::font_manager::FaceId;
        let glyphs: Vec<ShapedGlyph> = (0..n_glyphs)
            .map(|i| ShapedGlyph {
                glyph_id: 36, // 'A' glyph (approximate)
                #[allow(clippy::cast_precision_loss)]
                x_px: i as f32 * cell_w,
                y_offset: 0.0,
                face_id: FaceId::PrimaryRegular,
                is_color: false,
                cell_width: 1,
                source_char: '\0',
            })
            .collect();
        Arc::new(ShapedLine {
            runs: vec![ShapedRun {
                glyphs,
                col_start: 0,
                style: crate::gui::font_manager::GlyphStyle::new(false, false),
                font_weight: FontWeight::Normal,
                font_decorations: decorations,
                colors,
                url: None,
                blink: BlinkState::None,
            }],
            line_width: LineWidth::Normal,
        })
    }

    // -----------------------------------------------------------------------
    //  TerminalRenderer construction
    // -----------------------------------------------------------------------

    #[test]
    fn renderer_constructs_uninitialized() {
        let r = super::super::gpu::TerminalRenderer::new();
        assert!(!r.initialized());
    }

    #[test]
    fn renderer_default_is_uninitialized() {
        let r = super::super::gpu::TerminalRenderer::default();
        assert!(!r.initialized());
    }

    // -----------------------------------------------------------------------
    //  Background instance + decoration tests
    // -----------------------------------------------------------------------

    /// Shorthand for calling `build_background_instances` with typical test
    /// defaults (no selection, `CATPPUCCIN_MOCHA`, no cursor color override).
    ///
    /// Accepts a `CursorPos` (integer cell coordinates) and converts to the
    /// pixel `(f32, f32)` the production function expects.
    fn bg_instances_test(
        lines: &[Arc<ShapedLine>],
        cell_width: u32,
        cell_height: u32,
        show_cursor: bool,
        cursor_blink_on: bool,
        cursor_pos: CursorPos,
        cursor_style: &CursorVisualStyle,
    ) -> (Vec<f32>, Vec<f32>) {
        let cursor_pixel_pos = (
            gl_f32(cursor_pos.x) * gl_f32_u32(cell_width),
            gl_f32(cursor_pos.y) * gl_f32_u32(cell_height),
        );
        let mut instances = Vec::new();
        let mut deco = Vec::new();
        let _cursor_quad_appended = build_background_instances(
            &BackgroundFrame {
                shaped_lines: lines,
                cell_width,
                cell_height,
                ascent: 14.0, // ascent (approximate for test font)
                underline_offset: 13.0,
                strikeout_offset: 8.0,
                stroke_size: 1.0,
                show_cursor,
                cursor_blink_on,
                cursor_pixel_pos,
                cursor_width_scale: 1.0, // cursor_width_scale (normal for tests)
                cursor_visual_style: cursor_style,
                selection: None,
                selection_is_block: false,
                match_highlights: &[],
                command_block_hover_rows: None,
                term_width_cols: 0,
                theme: &themes::CATPPUCCIN_MOCHA,
                cursor_color_override: None,
                reverse_screen: false,
            },
            &mut instances,
            &mut deco,
        );
        (instances, deco)
    }

    /// Sibling of [`bg_instances_test`] that additionally accepts a
    /// `reverse_screen` flag (Task 115.3), so DECSCNM composition tests can
    /// exercise `build_background_instances` without duplicating every other
    /// `BackgroundFrame` field. All other fields are hardcoded to match
    /// `bg_instances_test`'s defaults exactly (8x16 cells, no cursor, no
    /// selection), so `reverse_screen: false` here must reproduce
    /// `bg_instances_test`'s output byte-for-byte.
    fn bg_instances_test_rev(
        lines: &[Arc<ShapedLine>],
        reverse_screen: bool,
    ) -> (Vec<f32>, Vec<f32>) {
        let mut instances = Vec::new();
        let mut deco = Vec::new();
        let _cursor_quad_appended = build_background_instances(
            &BackgroundFrame {
                shaped_lines: lines,
                cell_width: 8,
                cell_height: 16,
                ascent: 14.0,
                underline_offset: 13.0,
                strikeout_offset: 8.0,
                stroke_size: 1.0,
                show_cursor: false,
                cursor_blink_on: false,
                cursor_pixel_pos: (0.0, 0.0),
                cursor_width_scale: 1.0,
                cursor_visual_style: &CursorVisualStyle::BlockCursorSteady,
                selection: None,
                selection_is_block: false,
                match_highlights: &[],
                command_block_hover_rows: None,
                term_width_cols: 0,
                theme: &themes::CATPPUCCIN_MOCHA,
                cursor_color_override: None,
                reverse_screen,
            },
            &mut instances,
            &mut deco,
        );
        (instances, deco)
    }

    // -----------------------------------------------------------------------
    //  DECSCNM (reverse_screen) / SGR-7 composition tests (Task 115.3)
    //
    //  Truth table exercised below (fg = cell foreground, bg = cell
    //  background):
    //    reverse_screen=false, SGR-7 Off -> bg instance color = bg (control)
    //    reverse_screen=true,  SGR-7 Off -> bg instance color = fg (swapped)
    //    reverse_screen=true,  SGR-7 On  -> bg instance color = bg (XOR cancels)
    //    reverse_screen=false, SGR-7 On  -> bg instance color = fg (SGR-7 alone,
    //                                       covered by pre-existing StateColors
    //                                       tests, not repeated here)
    // -----------------------------------------------------------------------

    #[test]
    fn bg_reverse_screen_off_uses_cell_background() {
        // Control case: reverse_screen=false, SGR-7 Off. The emitted bg
        // instance color must be the cell's own background (today's
        // behavior, unchanged).
        let colors = default_colors()
            .with_color(TerminalColor::Red)
            .with_background_color(TerminalColor::Blue);
        let line = make_line(1, 8.0, colors, FontDecorationFlags::empty());
        let (bg, _deco) = bg_instances_test_rev(&[line], false);
        assert_eq!(bg.len(), BG_INSTANCE_FLOATS, "expected one cell instance");
        let expected = internal_color_to_gl(TerminalColor::Blue, false, &themes::CATPPUCCIN_MOCHA);
        assert_eq!(
            &bg[2..6],
            expected.as_slice(),
            "bg color should be cell background"
        );
    }

    #[test]
    fn bg_reverse_screen_on_uses_cell_foreground() {
        // Defect fix: reverse_screen=true, SGR-7 Off. DECSCNM alone swaps
        // fg/bg, so the emitted bg instance color must be the cell's
        // FOREGROUND color.
        let colors = default_colors()
            .with_color(TerminalColor::Red)
            .with_background_color(TerminalColor::Blue);
        let line = make_line(1, 8.0, colors, FontDecorationFlags::empty());
        let (bg, _deco) = bg_instances_test_rev(&[line], true);
        assert_eq!(bg.len(), BG_INSTANCE_FLOATS, "expected one cell instance");
        let expected = internal_color_to_gl(TerminalColor::Red, false, &themes::CATPPUCCIN_MOCHA);
        assert_eq!(
            &bg[2..6],
            expected.as_slice(),
            "reverse_screen alone should swap in the cell foreground"
        );
    }

    #[test]
    fn bg_reverse_screen_on_with_sgr7_on_cancels() {
        // XOR case: reverse_screen=true AND SGR-7 On. The two swaps cancel,
        // so the emitted bg instance color must be the cell's original
        // background again.
        let colors = default_colors()
            .with_color(TerminalColor::Red)
            .with_background_color(TerminalColor::Blue)
            .with_reverse_video(ReverseVideo::On);
        let line = make_line(1, 8.0, colors, FontDecorationFlags::empty());
        let (bg, _deco) = bg_instances_test_rev(&[line], true);
        assert_eq!(bg.len(), BG_INSTANCE_FLOATS, "expected one cell instance");
        let expected = internal_color_to_gl(TerminalColor::Blue, false, &themes::CATPPUCCIN_MOCHA);
        assert_eq!(
            &bg[2..6],
            expected.as_slice(),
            "SGR-7 On + reverse_screen=true should cancel back to the original background"
        );
    }

    #[test]
    fn bg_reverse_screen_false_matches_baseline() {
        // Proves adding `reverse_screen` with value `false` changed nothing:
        // the pre-115.2 helper (`bg_instances_test`, hardcoded
        // `reverse_screen: false`) and the new helper called with
        // `reverse_screen: false` must produce byte-identical output.
        let colors = default_colors()
            .with_color(TerminalColor::Red)
            .with_background_color(TerminalColor::Blue);
        let line_a = make_line(4, 8.0, colors, FontDecorationFlags::empty());
        let line_b = make_line(4, 8.0, colors, FontDecorationFlags::empty());

        let (bg_baseline, deco_baseline) = bg_instances_test(
            &[line_a],
            8,
            16,
            false,
            false,
            CursorPos::default(),
            &CursorVisualStyle::BlockCursorSteady,
        );
        let (bg_rev_false, deco_rev_false) = bg_instances_test_rev(&[line_b], false);

        assert_eq!(bg_baseline, bg_rev_false, "bg instances must be identical");
        assert_eq!(
            deco_baseline, deco_rev_false,
            "deco verts must be identical"
        );
    }

    #[test]
    fn effective_fg_bg_xor_truth_table() {
        // `build_foreground_instances` requires a real GlyphAtlas/FontManager
        // (GPU-backed) to exercise end-to-end, which is impractical in a
        // hermetic unit test. Instead, this test calls the private
        // `effective_fg`/`effective_bg` helpers directly (reachable via
        // `use super::*` in this module) to prove the full DECSCNM x SGR-7
        // XOR composition truth table that both the background- and
        // foreground-instance builders rely on.
        let colors_off = default_colors()
            .with_color(TerminalColor::Red)
            .with_background_color(TerminalColor::Blue);
        let colors_on = colors_off.with_reverse_video(ReverseVideo::On);

        // SGR-7 Off:
        assert_eq!(effective_fg(&colors_off, false), TerminalColor::Red);
        assert_eq!(effective_bg(&colors_off, false), TerminalColor::Blue);
        assert_eq!(effective_fg(&colors_off, true), TerminalColor::Blue);
        assert_eq!(effective_bg(&colors_off, true), TerminalColor::Red);

        // SGR-7 On:
        assert_eq!(effective_fg(&colors_on, false), TerminalColor::Blue);
        assert_eq!(effective_bg(&colors_on, false), TerminalColor::Red);
        assert_eq!(effective_fg(&colors_on, true), TerminalColor::Red);
        assert_eq!(effective_bg(&colors_on, true), TerminalColor::Blue);
    }

    #[test]
    fn bg_instances_empty_on_default_background() {
        // A line whose cells all have `DefaultBackground` should produce no
        // instances and no decoration verts.
        let line = make_line(5, 8.0, default_colors(), FontDecorationFlags::empty());
        let (bg, deco) = bg_instances_test(
            &[line],
            8,
            16,
            false,
            false,
            CursorPos::default(),
            &CursorVisualStyle::BlockCursorSteady,
        );
        assert_eq!(
            bg.len(),
            0,
            "default background should produce no instances"
        );
        assert_eq!(deco.len(), 0, "no decorations expected");
    }

    #[test]
    fn bg_instances_per_cell_for_colored_run() {
        // A single run with 3 non-default-background cells should produce 3
        // instances (one per cell).
        let colors = StateColors::default().with_background_color(TerminalColor::Red);
        let line = make_line(3, 8.0, colors, FontDecorationFlags::empty());
        let (bg, deco) = bg_instances_test(
            &[line],
            8,
            16,
            false,
            false,
            CursorPos::default(),
            &CursorVisualStyle::BlockCursorSteady,
        );
        assert_eq!(
            bg.len(),
            3 * BG_INSTANCE_FLOATS,
            "expected 3 cell instances"
        );
        assert_eq!(deco.len(), 0, "no decorations expected");
    }

    #[test]
    fn bg_instances_adjacent_same_color_per_cell() {
        // Two adjacent runs with the same background color produce one instance
        // per cell (instanced rendering does not merge — the GPU handles it).
        use crate::gui::font_manager::FaceId;
        let colors = StateColors::default().with_background_color(TerminalColor::Blue);

        let line = Arc::new(ShapedLine {
            runs: vec![
                ShapedRun {
                    glyphs: vec![ShapedGlyph {
                        glyph_id: 36,
                        x_px: 0.0,
                        y_offset: 0.0,
                        face_id: FaceId::PrimaryRegular,
                        is_color: false,
                        cell_width: 1,
                        source_char: '\0',
                    }],
                    col_start: 0,
                    style: crate::gui::font_manager::GlyphStyle::new(false, false),
                    font_weight: FontWeight::Normal,
                    font_decorations: FontDecorationFlags::empty(),
                    colors,
                    url: None,
                    blink: BlinkState::None,
                },
                ShapedRun {
                    glyphs: vec![ShapedGlyph {
                        glyph_id: 37,
                        x_px: 8.0,
                        y_offset: 0.0,
                        face_id: FaceId::PrimaryRegular,
                        is_color: false,
                        cell_width: 1,
                        source_char: '\0',
                    }],
                    col_start: 1,
                    style: crate::gui::font_manager::GlyphStyle::new(false, false),
                    font_weight: FontWeight::Normal,
                    font_decorations: FontDecorationFlags::empty(),
                    colors,
                    url: None,
                    blink: BlinkState::None,
                },
            ],
            line_width: LineWidth::Normal,
        });

        let (bg, deco) = bg_instances_test(
            &[line],
            8,
            16,
            false,
            false,
            CursorPos::default(),
            &CursorVisualStyle::BlockCursorSteady,
        );
        // Two cells → two instances.
        assert_eq!(
            bg.len(),
            2 * BG_INSTANCE_FLOATS,
            "adjacent same-color runs should produce one instance per cell"
        );
        assert_eq!(deco.len(), 0, "no decorations expected");
    }

    #[test]
    fn bg_instances_different_colors_per_cell() {
        // Two runs with different non-default background colors produce one
        // instance per cell.
        use crate::gui::font_manager::FaceId;

        let colors_red = StateColors::default().with_background_color(TerminalColor::Red);
        let colors_blue = StateColors::default().with_background_color(TerminalColor::Blue);

        let line = Arc::new(ShapedLine {
            runs: vec![
                ShapedRun {
                    glyphs: vec![ShapedGlyph {
                        glyph_id: 36,
                        x_px: 0.0,
                        y_offset: 0.0,
                        face_id: FaceId::PrimaryRegular,
                        is_color: false,
                        cell_width: 1,
                        source_char: '\0',
                    }],
                    col_start: 0,
                    style: crate::gui::font_manager::GlyphStyle::new(false, false),
                    font_weight: FontWeight::Normal,
                    font_decorations: FontDecorationFlags::empty(),
                    colors: colors_red,
                    url: None,
                    blink: BlinkState::None,
                },
                ShapedRun {
                    glyphs: vec![ShapedGlyph {
                        glyph_id: 37,
                        x_px: 8.0,
                        y_offset: 0.0,
                        face_id: FaceId::PrimaryRegular,
                        is_color: false,
                        cell_width: 1,
                        source_char: '\0',
                    }],
                    col_start: 1,
                    style: crate::gui::font_manager::GlyphStyle::new(false, false),
                    font_weight: FontWeight::Normal,
                    font_decorations: FontDecorationFlags::empty(),
                    colors: colors_blue,
                    url: None,
                    blink: BlinkState::None,
                },
            ],
            line_width: LineWidth::Normal,
        });

        let (bg, _deco) = bg_instances_test(
            &[line],
            8,
            16,
            false,
            false,
            CursorPos::default(),
            &CursorVisualStyle::BlockCursorSteady,
        );
        assert_eq!(
            bg.len(),
            2 * BG_INSTANCE_FLOATS,
            "different-color runs should produce one instance per cell"
        );
    }

    #[test]
    fn bg_instances_cursor_block_adds_deco_quad() {
        // With `show_cursor = true` and a steady block cursor, one cursor quad
        // should appear in the decoration verts.
        let line = make_line(3, 8.0, default_colors(), FontDecorationFlags::empty());
        let (bg, deco) = bg_instances_test(
            &[line],
            8,
            16,
            true,
            true,
            CursorPos { x: 1, y: 0 },
            &CursorVisualStyle::BlockCursorSteady,
        );
        assert_eq!(bg.len(), 0, "default bg should produce no instances");
        assert_eq!(
            deco.len(),
            VERTS_PER_QUAD * DECO_VERTEX_FLOATS,
            "block cursor should add one deco quad"
        );
    }

    #[test]
    fn bg_instances_cursor_blink_off_no_quad() {
        let line = make_line(3, 8.0, default_colors(), FontDecorationFlags::empty());
        let (bg, deco) = bg_instances_test(
            &[line],
            8,
            16,
            true,
            false, // blink_on = false
            CursorPos { x: 0, y: 0 },
            &CursorVisualStyle::BlockCursorBlink,
        );
        assert_eq!(bg.len(), 0);
        assert_eq!(
            deco.len(),
            0,
            "blinking cursor with blink_on=false should produce no quad"
        );
    }

    #[test]
    fn bg_instances_cursor_steady_ignores_blink_flag() {
        let line = make_line(3, 8.0, default_colors(), FontDecorationFlags::empty());
        let (_bg, deco) = bg_instances_test(
            &[line],
            8,
            16,
            true,
            false, // blink_on = false — irrelevant for steady cursor
            CursorPos { x: 0, y: 0 },
            &CursorVisualStyle::BlockCursorSteady,
        );
        assert_eq!(
            deco.len(),
            VERTS_PER_QUAD * DECO_VERTEX_FLOATS,
            "steady cursor should render even when blink_on=false"
        );
    }

    #[test]
    fn bg_instances_underline_adds_deco_quad() {
        let mut underline_flags = FontDecorationFlags::empty();
        underline_flags.insert(FontDecorations::Underline);
        let line = make_line(3, 8.0, default_colors(), underline_flags);
        let (bg, deco) = bg_instances_test(
            &[line],
            8,
            16,
            false,
            false,
            CursorPos::default(),
            &CursorVisualStyle::BlockCursorSteady,
        );
        assert_eq!(bg.len(), 0, "default bg — no instances");
        assert_eq!(
            deco.len(),
            VERTS_PER_QUAD * DECO_VERTEX_FLOATS,
            "underline run should produce one decoration quad"
        );
    }

    #[test]
    fn bg_instances_strikethrough_adds_deco_quad() {
        let mut strikethrough_flags = FontDecorationFlags::empty();
        strikethrough_flags.insert(FontDecorations::Strikethrough);
        let line = make_line(3, 8.0, default_colors(), strikethrough_flags);
        let (bg, deco) = bg_instances_test(
            &[line],
            8,
            16,
            false,
            false,
            CursorPos::default(),
            &CursorVisualStyle::BlockCursorSteady,
        );
        assert_eq!(bg.len(), 0, "default bg — no instances");
        assert_eq!(
            deco.len(),
            VERTS_PER_QUAD * DECO_VERTEX_FLOATS,
            "strikethrough run should produce one decoration quad"
        );
    }

    #[test]
    fn bg_instances_cursor_position_maps_to_pixel_coords() {
        // Block cursor at (col=2, row=1) with cell_width=10, cell_height=20.
        // Expected x0 = 2*10 = 20, y0 = 1*20 = 20.
        let lines = [
            make_line(5, 10.0, default_colors(), FontDecorationFlags::empty()),
            make_line(5, 10.0, default_colors(), FontDecorationFlags::empty()),
        ];
        let (_bg, deco) = bg_instances_test(
            &lines,
            10,
            20,
            true,
            true,
            CursorPos { x: 2, y: 1 },
            &CursorVisualStyle::BlockCursorSteady,
        );
        // The cursor quad is the last 36 floats (6 verts x 6 floats) in deco.
        assert!(deco.len() >= VERTS_PER_QUAD * DECO_VERTEX_FLOATS);
        let cursor_start = deco.len() - VERTS_PER_QUAD * DECO_VERTEX_FLOATS;
        let x0 = deco[cursor_start];
        let y0 = deco[cursor_start + 1];
        assert!(
            (x0 - 20.0).abs() < f32::EPSILON,
            "cursor x should be col*cell_w = 20, got {x0}"
        );
        assert!(
            (y0 - 20.0).abs() < f32::EPSILON,
            "cursor y should be row*cell_h = 20, got {y0}"
        );
    }

    // -----------------------------------------------------------------------
    //  Foreground instance tests
    // -----------------------------------------------------------------------

    // -----------------------------------------------------------------------
    //  Color-glyph fit-to-cell sizing
    // -----------------------------------------------------------------------

    #[test]
    fn fit_color_glyph_oversized_scales_to_cell_height_with_margin() {
        // A 64x64 native emoji bitmap in a 20px-tall, 2-cell-wide box.
        let cell_h = 20.0;
        let advance_w = 20.0; // 2 cells of 10px each
        let r = fit_color_glyph_rect(64.0, 64.0, 0.0, advance_w, 0.0, cell_h);
        // Height fits within cell minus margin.
        let max_h = cell_h * (1.0 - COLOR_GLYPH_CELL_MARGIN);
        assert!(
            r.height <= max_h + 0.01,
            "height {} should be <= {max_h}",
            r.height
        );
        // Aspect ratio preserved (square in, square out).
        assert!(
            (r.width - r.height).abs() < 0.01,
            "aspect ratio not preserved"
        );
        // Centered vertically within the cell.
        assert!((cell_h - r.height).mul_add(-0.5, r.y0).abs() < 0.01);
        // Stays within the cell bounds vertically.
        assert!(r.y0 >= 0.0);
        assert!(r.y0 + r.height <= cell_h + 0.01);
    }

    #[test]
    fn fit_color_glyph_centered_within_advance_box() {
        // Emoji narrower than its 2-cell advance box must be centered, never
        // shifted left into the previous cell.
        let r = fit_color_glyph_rect(64.0, 64.0, 100.0, 40.0, 0.0, 20.0);
        // Left edge must be at or past the box origin (never to the left).
        assert!(r.x0 >= 100.0, "glyph shifted left of its cell: x0={}", r.x0);
        // Right edge must stay within the advance box.
        assert!(
            r.x0 + r.width <= 100.0 + 40.0 + 0.01,
            "glyph overflows its advance box: x1={}",
            r.x0 + r.width
        );
        // Symmetric centering.
        let left_gap = r.x0 - 100.0;
        let right_gap = (100.0 + 40.0) - (r.x0 + r.width);
        assert!((left_gap - right_gap).abs() < 0.01, "not centered");
    }

    #[test]
    fn fit_color_glyph_small_not_enlarged() {
        // A glyph already smaller than the cell must not be scaled up.
        let r = fit_color_glyph_rect(8.0, 8.0, 0.0, 20.0, 0.0, 20.0);
        assert!((r.width - 8.0).abs() < 0.01, "small glyph was enlarged");
        assert!((r.height - 8.0).abs() < 0.01, "small glyph was enlarged");
    }

    #[test]
    fn fit_color_glyph_degenerate_inputs() {
        let r = fit_color_glyph_rect(0.0, 0.0, 5.0, 10.0, 2.0, 0.0);
        assert!(r.width.abs() < f32::EPSILON);
        assert!(r.height.abs() < f32::EPSILON);
    }

    #[test]
    fn fit_color_glyph_wide_emoji_uses_full_two_cell_box() {
        // The fitted glyph for a width-2 emoji must span (centered) the full
        // 2-cell box, not a single cell — this is the regression guard for the
        // "emoji squeezed into one cell, blank cell inserted" bug.
        let cell_px = 10.0;
        let two_cells = cell_px * 2.0;
        let r = fit_color_glyph_rect(64.0, 64.0, 50.0, two_cells, 0.0, 20.0);
        // The glyph must be wider than a single cell (it fit to 20px height,
        // square, so ~17.6px wide > 10px cell).
        assert!(
            r.width > cell_px,
            "width-2 emoji collapsed into a single cell: width={}",
            r.width
        );
    }

    #[test]
    fn normalized_fallback_full_cell_glyph_fills_primary_cell() {
        // Reproduces the Task #411 case: a full-cell powerline glyph from the
        // bundled CaskaydiaCove fallback (its cell 19px, baseline 15, glyph
        // bearing_y 15 h 19 => fills its own cell) placed into a Courier Prime
        // primary cell (18px). It must fill the primary cell exactly, not clip.
        let primary_cell_h = 18.0_f32;
        // Row 0: cell_top = 0, cell_bottom = primary_cell_h.
        let params = RowGlyphParams::new(LineWidth::Normal, primary_cell_h, 0, 15.0);
        let mut out = Vec::new();
        emit_normalized_fallback_glyph(
            &mut out,
            &NormalizedFallbackGlyph {
                x_px: 0.0,
                bearing_x: 0,
                bearing_y: 15,
                glyph_w: 10,
                glyph_h: 19,
                fb_cell_h: 19.0,
                fb_baseline: 15.0,
                fb_cell_w: 10.0,
                primary_cell_w: 9.0,
                uv: [0.0, 0.0, 1.0, 1.0],
            },
            [1.0, 1.0, 1.0, 1.0],
            &params,
        );
        assert_eq!(out.len(), 13, "one instance of 13 floats");
        let y0 = out[1];
        let width = out[2];
        let height = out[3];
        // natural_top = baseline - bearing_y = 0 -> y0 = cell_top = 0.
        assert!(y0.abs() < 0.01, "expected y0 ~0 (cell top), got {y0}");
        // scaled_h = 19 * (18/19) = 18 -> fills the primary cell, no clip.
        assert!(
            (height - primary_cell_h).abs() < 0.01,
            "expected height ~{primary_cell_h} (fills cell), got {height}"
        );
        // scaled_w = 10 * (primary_cell_w 9 / fb_cell_w 10) = 9 -> fills the
        // primary cell width exactly (independent of the height ratio).
        assert!(
            (width - 9.0).abs() < 0.01,
            "expected width ~9 (fills primary cell width), got {width}"
        );
    }

    #[test]
    fn normalized_fallback_uses_independent_width_scale() {
        // Aspect-ratio guard (Task #411 fix): a full-cell fallback glyph must be
        // scaled to the primary cell's WIDTH via the width ratio, not the height
        // ratio. Here the primary cell is much taller-and-narrower than the
        // fallback cell, so a height-ratio width would wildly over-fill.
        let primary_cell_h = 30.0_f32;
        let params = RowGlyphParams::new(LineWidth::Normal, primary_cell_h, 0, 24.0);
        let mut out = Vec::new();
        emit_normalized_fallback_glyph(
            &mut out,
            &NormalizedFallbackGlyph {
                x_px: 0.0,
                bearing_x: 0,
                bearing_y: 15,
                glyph_w: 10, // fills the fallback cell width
                glyph_h: 19,
                fb_cell_h: 19.0,
                fb_baseline: 15.0,
                fb_cell_w: 10.0,
                primary_cell_w: 8.0, // narrow primary cell
                uv: [0.0, 0.0, 1.0, 1.0],
            },
            [1.0, 1.0, 1.0, 1.0],
            &params,
        );
        let width = out[2];
        // Correct (width ratio): 10 * (8/10) = 8.0 — fills the narrow cell.
        // Bug (height ratio): 10 * (30/19) ≈ 15.8 — nearly double, overflows.
        assert!(
            (width - 8.0).abs() < 0.01,
            "expected width ~8 via width ratio, got {width} (height-ratio bug would give ~15.8)"
        );
    }

    #[test]
    fn normalized_fallback_icon_stays_proportional() {
        // A partial-height icon (h 13 in a 19px fallback cell) must NOT be
        // ballooned to fill the primary cell — it stays proportionally sized.
        let primary_cell_h = 18.0_f32;
        let params = RowGlyphParams::new(LineWidth::Normal, primary_cell_h, 0, 15.0);
        let mut out = Vec::new();
        emit_normalized_fallback_glyph(
            &mut out,
            &NormalizedFallbackGlyph {
                x_px: 0.0,
                bearing_x: 0,
                bearing_y: 12,
                glyph_w: 13,
                glyph_h: 13,
                fb_cell_h: 19.0,
                fb_baseline: 15.0,
                fb_cell_w: 19.0,
                primary_cell_w: 18.0,
                uv: [0.0, 0.0, 1.0, 1.0],
            },
            [1.0, 1.0, 1.0, 1.0],
            &params,
        );
        let height = out[3];
        // scaled_h = 13 * (18/19) ~= 12.3 — much less than the full cell.
        assert!(
            height < primary_cell_h * 0.8,
            "icon should stay proportional, got height {height} (cell {primary_cell_h})"
        );
    }

    #[test]
    fn fg_instances_empty_on_empty_lines() {
        let mut instances = Vec::new();
        build_foreground_instances(
            &[],
            &mut GlyphAtlas::default(),
            &FontManager::new(&Config::default(), 1.0).unwrap(),
            16,
            13.0,
            &FgRenderOptions::all_visible(None),
            &themes::CATPPUCCIN_MOCHA,
            &mut instances,
        );
        assert_eq!(instances.len(), 0);
    }

    #[test]
    fn fg_instances_produces_data_for_ascii_glyphs() {
        let mut fm = FontManager::new(&Config::default(), 1.0).unwrap();
        let mut atlas = GlyphAtlas::new(256, 1024);
        #[allow(clippy::cast_precision_loss)]
        let cell_w = fm.cell_width() as f32;
        let cell_h = fm.cell_height();
        let ascent = fm.ascent();

        // Shape a short ASCII line.
        let mut cache = crate::gui::shaping::ShapingCache::new();
        let chars: Vec<freminal_common::buffer_states::tchar::TChar> = b"ABC"
            .iter()
            .map(|&b| freminal_common::buffer_states::tchar::TChar::Ascii(b))
            .collect();
        let tags = vec![freminal_common::buffer_states::format_tag::FormatTag::default()];
        let lines = cache.shape_visible(&chars, &tags, 80, &mut fm, cell_w, false, &[]);

        let mut instances = Vec::new();
        build_foreground_instances(
            &lines,
            &mut atlas,
            &fm,
            cell_h,
            ascent,
            &FgRenderOptions::all_visible(None),
            &themes::CATPPUCCIN_MOCHA,
            &mut instances,
        );

        // Three ASCII glyphs each produce one instance = FG_INSTANCE_FLOATS floats.
        // Some glyphs may be spaces (zero-size) — so at minimum some instances must exist.
        assert!(
            instances.len() >= FG_INSTANCE_FLOATS,
            "at least one foreground instance expected, got {} floats",
            instances.len()
        );
        assert_eq!(
            instances.len() % FG_INSTANCE_FLOATS,
            0,
            "foreground instance count must be a multiple of one instance ({FG_INSTANCE_FLOATS} floats)",
        );
    }

    // -----------------------------------------------------------------------
    //  Push quad helper
    // -----------------------------------------------------------------------

    #[test]
    fn push_quad_produces_six_vertices() {
        let mut verts = Vec::new();
        push_quad(&mut verts, 0.0, 0.0, 10.0, 10.0, [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(verts.len(), VERTS_PER_QUAD * DECO_VERTEX_FLOATS);
    }

    #[test]
    fn push_quad_corner_positions() {
        let mut verts = Vec::new();
        push_quad(&mut verts, 5.0, 3.0, 15.0, 13.0, [0.0, 1.0, 0.0, 1.0]);

        // Vertex 0 (top-left): x=5, y=3
        assert!((verts[0] - 5.0).abs() < f32::EPSILON);
        assert!((verts[1] - 3.0).abs() < f32::EPSILON);

        // Vertex 1 (top-right): x=15, y=3
        assert!((verts[DECO_VERTEX_FLOATS] - 15.0).abs() < f32::EPSILON);
        assert!((verts[DECO_VERTEX_FLOATS + 1] - 3.0).abs() < f32::EPSILON);
    }

    // -----------------------------------------------------------------------
    //  Extract atlas rect
    // -----------------------------------------------------------------------

    #[test]
    fn extract_atlas_rect_correct_bytes() {
        // Build a 4×4 atlas with distinct pixel values per row.
        let size: u32 = 4;
        let mut pixels = vec![0u8; (size * size * 4) as usize];
        // Row 1 (y=1): fill with 0xFF.
        let row_start = (size * 4) as usize;
        pixels[row_start..row_start + (size * 4) as usize].fill(0xFF);

        let rect = super::super::super::atlas::DirtyRect {
            x: 0,
            y: 1,
            width: 4,
            height: 1,
        };
        let out = extract_atlas_rect(&pixels, size, &rect);
        assert_eq!(out.len(), 16);
        assert!(out.iter().all(|&b| b == 0xFF));
    }

    // -----------------------------------------------------------------------
    //  Blink helper
    // -----------------------------------------------------------------------

    #[test]
    fn blink_visible_steady_always_true() {
        assert!(cursor_blink_is_visible(
            &CursorVisualStyle::BlockCursorSteady,
            false
        ));
        assert!(cursor_blink_is_visible(
            &CursorVisualStyle::BlockCursorSteady,
            true
        ));
        assert!(cursor_blink_is_visible(
            &CursorVisualStyle::UnderlineCursorSteady,
            false
        ));
        assert!(cursor_blink_is_visible(
            &CursorVisualStyle::VerticalLineCursorSteady,
            false
        ));
    }

    #[test]
    fn blink_visible_blinking_follows_flag() {
        assert!(!cursor_blink_is_visible(
            &CursorVisualStyle::BlockCursorBlink,
            false
        ));
        assert!(cursor_blink_is_visible(
            &CursorVisualStyle::BlockCursorBlink,
            true
        ));
        assert!(!cursor_blink_is_visible(
            &CursorVisualStyle::UnderlineCursorBlink,
            false
        ));
        assert!(!cursor_blink_is_visible(
            &CursorVisualStyle::VerticalLineCursorBlink,
            false
        ));
    }

    // -----------------------------------------------------------------------
    //  build_cursor_verts_only
    // -----------------------------------------------------------------------

    /// Test helper: wraps `build_cursor_verts_only`, converting a `CursorPos`
    /// to pixel `(f32, f32)` using the same formula the production caller uses.
    #[allow(clippy::too_many_arguments)]
    fn cursor_verts_test(
        cell_width: u32,
        cell_height: u32,
        show_cursor: bool,
        cursor_blink_on: bool,
        cursor_pos: CursorPos,
        cursor_visual_style: &CursorVisualStyle,
        theme: &ThemePalette,
        cursor_color_override: Option<(u8, u8, u8)>,
    ) -> Vec<f32> {
        let cursor_pixel_pos = (
            gl_f32(cursor_pos.x) * gl_f32_u32(cell_width),
            gl_f32(cursor_pos.y) * gl_f32_u32(cell_height),
        );
        build_cursor_verts_only(
            cell_width,
            cell_height,
            show_cursor,
            cursor_blink_on,
            cursor_pixel_pos,
            1.0, // cursor_width_scale (normal for tests)
            cursor_visual_style,
            theme,
            cursor_color_override,
        )
    }

    /// When the cursor is hidden (`show_cursor = false`), the function must
    /// return an empty vec — no geometry at all.
    #[test]
    fn cursor_verts_only_hidden_returns_empty() {
        let verts = cursor_verts_test(
            8,
            16,
            false,
            true,
            CursorPos { x: 0, y: 0 },
            &CursorVisualStyle::BlockCursorSteady,
            &themes::CATPPUCCIN_MOCHA,
            None,
        );
        assert!(verts.is_empty(), "hidden cursor should produce no verts");
    }

    /// A steady block cursor always produces exactly `CURSOR_QUAD_FLOATS` floats,
    /// regardless of the blink flag.
    #[test]
    fn cursor_verts_only_steady_block_always_visible() {
        for blink_on in [false, true] {
            let verts = cursor_verts_test(
                8,
                16,
                true,
                blink_on,
                CursorPos { x: 1, y: 2 },
                &CursorVisualStyle::BlockCursorSteady,
                &themes::CATPPUCCIN_MOCHA,
                None,
            );
            assert_eq!(
                verts.len(),
                CURSOR_QUAD_FLOATS,
                "steady cursor must always produce one quad (blink_on={blink_on})"
            );
        }
    }

    /// A blinking cursor with `blink_on = false` must return empty verts.
    #[test]
    fn cursor_verts_only_blink_off_returns_empty() {
        let verts = cursor_verts_test(
            8,
            16,
            true,
            false,
            CursorPos { x: 0, y: 0 },
            &CursorVisualStyle::BlockCursorBlink,
            &themes::CATPPUCCIN_MOCHA,
            None,
        );
        assert!(verts.is_empty(), "blink-off cursor should produce no verts");
    }

    /// Partial VBO update: `build_cursor_verts_only` produces exactly
    /// `CURSOR_QUAD_FLOATS` floats and they occupy the correct byte range.
    ///
    /// The test verifies that:
    ///   1. The cursor-only builder produces `CURSOR_QUAD_FLOATS` floats
    ///      (= `VERTS_PER_QUAD * DECO_VERTEX_FLOATS`).
    ///   2. Patching those floats into a pre-built deco VBO at the
    ///      recorded offset produces the expected combined buffer — only the
    ///      cursor region changes, all other floats are untouched.
    #[test]
    fn partial_vbo_update_only_modifies_cursor_region() {
        // Build the full deco VBO for one line + a cursor at (col=0, row=0).
        let line = make_line(3, 8.0, default_colors(), FontDecorationFlags::empty());
        let (_bg, full_deco) = bg_instances_test(
            std::slice::from_ref(&line),
            8,
            16,
            true,
            true, // cursor visible
            CursorPos { x: 0, y: 0 },
            &CursorVisualStyle::BlockCursorSteady,
        );

        // Record where the cursor quad starts (it is appended at the end).
        let cursor_float_offset = full_deco.len() - CURSOR_QUAD_FLOATS;
        let cursor_byte_offset = cursor_float_offset * std::mem::size_of::<f32>();

        // The pre-cursor portion must be unchanged — capture it before mutation.
        let pre_cursor = full_deco[..cursor_float_offset].to_vec();

        // Build cursor-only verts with blink_on=false using a *blinking* style.
        // BlockCursorSteady ignores blink_on (always visible); BlockCursorBlink
        // respects it, so blink_on=false correctly produces an empty vec.
        let cursor_off_verts = cursor_verts_test(
            8,
            16,
            true,
            false, // blink off → empty verts (only true for blinking style)
            CursorPos { x: 0, y: 0 },
            &CursorVisualStyle::BlockCursorBlink,
            &themes::CATPPUCCIN_MOCHA,
            None,
        );

        // Simulate the partial-update patch: mutate full_deco in-place to
        // overwrite the cursor region (matches draw_with_cursor_only_update).
        let mut patched = full_deco;
        if cursor_off_verts.is_empty() {
            // Zero-fill the cursor region (matches what draw_with_cursor_only_update does).
            for f in &mut patched[cursor_float_offset..] {
                *f = 0.0;
            }
        } else {
            patched[cursor_float_offset..].copy_from_slice(&cursor_off_verts);
        }

        // Pre-cursor region must be bit-identical.
        assert_eq!(
            patched[..cursor_float_offset],
            pre_cursor[..],
            "partial update must not modify floats before the cursor quad offset              (byte_offset={cursor_byte_offset})"
        );

        // The cursor region must now be all zeros (blink-off patch).
        assert!(
            patched[cursor_float_offset..].iter().all(|&f| f == 0.0),
            "cursor region must be zeroed after blink-off patch"
        );
    }

    /// Regression for issue #432: a full rebuild that happens to run during
    /// the cursor's blink-*off* phase (a blinking cursor style, not steady)
    /// must report `cursor_quad_appended == false` and must NOT reserve
    /// `CURSOR_QUAD_FLOATS` tail floats for a cursor quad that was never
    /// actually pushed.
    ///
    /// Before the fix, callers computed the cursor's tail offset from
    /// `show_cursor` alone (ignoring blink phase), so this exact scenario —
    /// `show_cursor: true`, a blinking style, `cursor_blink_on: false` —
    /// caused the caller to believe a cursor quad occupied the last
    /// `CURSOR_QUAD_FLOATS` floats of `deco_verts` when in fact NO cursor
    /// quad was appended at all, and those floats actually belonged to the
    /// bottom-most (here: only) selection highlight quad. A later cursor-only
    /// frame, when blink flipped back on, would then overwrite that
    /// mis-identified region — clobbering the selection quad with the
    /// cursor's own geometry and color instead of the cursor's absent quad.
    #[test]
    fn full_rebuild_during_blink_off_does_not_append_cursor_quad_over_selection() {
        let line = make_line(3, 8.0, default_colors(), FontDecorationFlags::empty());
        let mut instances = Vec::new();
        let mut deco = Vec::new();

        let cursor_quad_appended = build_background_instances(
            &BackgroundFrame {
                shaped_lines: std::slice::from_ref(&line),
                cell_width: 8,
                cell_height: 16,
                ascent: 14.0,
                underline_offset: 13.0,
                strikeout_offset: 8.0,
                stroke_size: 1.0,
                // The cursor is structurally supposed to show...
                show_cursor: true,
                // ...but this frame lands on the blink-off half of the cycle...
                cursor_blink_on: false,
                cursor_pixel_pos: (0.0, 0.0),
                cursor_width_scale: 1.0,
                // ...and the style is a *blinking* one, so blink phase matters
                // (a steady style would ignore `cursor_blink_on` entirely).
                cursor_visual_style: &CursorVisualStyle::BlockCursorBlink,
                // A single-row selection spanning the whole shaped line.
                selection: Some((0, 0, 2, 0)),
                selection_is_block: false,
                match_highlights: &[],
                command_block_hover_rows: None,
                term_width_cols: 0,
                theme: &themes::CATPPUCCIN_MOCHA,
                cursor_color_override: None,
                reverse_screen: false,
            },
            &mut instances,
            &mut deco,
        );

        assert!(
            !cursor_quad_appended,
            "blink-off phase with a blinking cursor style must not append a cursor quad"
        );
        assert_eq!(
            deco.len(),
            CURSOR_QUAD_FLOATS,
            "deco_verts should contain exactly the one selection quad — no reserved \
             cursor tail floats — since no cursor quad was actually appended"
        );
        // The one quad present must be the selection's color, not the
        // cursor's — i.e. every float in deco_verts genuinely belongs to the
        // selection quad, confirming there is no separate reserved cursor
        // region hiding at the tail.
        let expected_color = selection_bg_f(&themes::CATPPUCCIN_MOCHA);
        // Layout: vertex 0 = (x, y, r, g, b, a) — color starts at index 2.
        let actual_color = [deco[2], deco[3], deco[4], deco[5]];
        assert!(
            actual_color
                .iter()
                .zip(expected_color.iter())
                .all(|(a, b)| (a - b).abs() < f32::EPSILON),
            "the sole quad in deco_verts must be the selection highlight, not a cursor quad: \
             got {actual_color:?}, expected {expected_color:?}"
        );
    }

    // -----------------------------------------------------------------------
    //  is_cell_selected tests
    // -----------------------------------------------------------------------

    #[test]
    fn is_cell_selected_no_selection() {
        // No selection → always false.
        assert!(!is_cell_selected(0, 0, None, false));
        assert!(!is_cell_selected(5, 5, None, true));
    }

    #[test]
    fn is_cell_selected_linear_single_row() {
        // Linear selection on row 2, cols 3..=6.
        let sel = Some((3, 2, 6, 2));
        assert!(is_cell_selected(2, 3, sel, false));
        assert!(is_cell_selected(2, 5, sel, false));
        assert!(is_cell_selected(2, 6, sel, false));
        assert!(!is_cell_selected(2, 2, sel, false));
        assert!(!is_cell_selected(2, 7, sel, false));
        assert!(!is_cell_selected(1, 4, sel, false));
        assert!(!is_cell_selected(3, 4, sel, false));
    }

    #[test]
    fn is_cell_selected_linear_multirow() {
        // Linear selection: start row 1 col 3, end row 3 col 5.
        let sel = Some((3, 1, 5, 3));
        // Start row: only cols >= 3 are selected.
        assert!(is_cell_selected(1, 3, sel, false));
        assert!(is_cell_selected(1, 9, sel, false));
        assert!(!is_cell_selected(1, 2, sel, false));
        // Middle row: entire row is selected.
        assert!(is_cell_selected(2, 0, sel, false));
        assert!(is_cell_selected(2, 99, sel, false));
        // End row: only cols <= 5 are selected.
        assert!(is_cell_selected(3, 0, sel, false));
        assert!(is_cell_selected(3, 5, sel, false));
        assert!(!is_cell_selected(3, 6, sel, false));
        // Row outside range.
        assert!(!is_cell_selected(0, 0, sel, false));
        assert!(!is_cell_selected(4, 0, sel, false));
    }

    #[test]
    fn is_cell_selected_block_same_cols_every_row() {
        // Block selection: rows 1..=3, cols 2..=5.
        let sel = Some((2, 1, 5, 3));
        for row in 1..=3_usize {
            assert!(is_cell_selected(row, 2, sel, true), "row {row} col 2");
            assert!(is_cell_selected(row, 4, sel, true), "row {row} col 4");
            assert!(is_cell_selected(row, 5, sel, true), "row {row} col 5");
            assert!(!is_cell_selected(row, 1, sel, true), "row {row} col 1");
            assert!(!is_cell_selected(row, 6, sel, true), "row {row} col 6");
        }
        // Row outside range is never selected.
        assert!(!is_cell_selected(0, 3, sel, true));
        assert!(!is_cell_selected(4, 3, sel, true));
    }

    #[test]
    fn is_cell_selected_block_reversed_cols() {
        // Block selection dragged right-to-left: start_col > end_col.
        let sel = Some((7, 0, 3, 2)); // cols 7 to 3 → col_min=3, col_max=7
        for row in 0..=2_usize {
            assert!(is_cell_selected(row, 3, sel, true));
            assert!(is_cell_selected(row, 5, sel, true));
            assert!(is_cell_selected(row, 7, sel, true));
            assert!(!is_cell_selected(row, 2, sel, true));
            assert!(!is_cell_selected(row, 8, sel, true));
        }
    }

    #[test]
    fn is_cell_selected_block_single_row() {
        // Block mode on a single row: column bounds apply like any row.
        let sel = Some((2, 4, 5, 4)); // row 4, cols 2..=5
        assert!(is_cell_selected(4, 2, sel, true));
        assert!(is_cell_selected(4, 5, sel, true));
        assert!(!is_cell_selected(4, 1, sel, true));
        assert!(!is_cell_selected(4, 6, sel, true));
        assert!(!is_cell_selected(3, 3, sel, true));
    }

    #[test]
    fn is_cell_selected_block_middle_rows_respect_col_bounds() {
        // In LINEAR mode, middle rows (between start and end) are fully selected.
        // In BLOCK mode, middle rows respect the column bounds just like edge rows.
        let sel = Some((3, 0, 5, 2));
        // Linear: middle row 1, col 0 → selected (full row).
        assert!(is_cell_selected(1, 0, sel, false));
        // Block: middle row 1, col 0 → NOT selected (col 0 < col_min=3).
        assert!(!is_cell_selected(1, 0, sel, true));
        // Block: middle row 1, col 4 → selected.
        assert!(is_cell_selected(1, 4, sel, true));
    }

    // -----------------------------------------------------------------------
    //  DECDWL / DECDHL — RowGlyphParams and emit_bg_cells
    // -----------------------------------------------------------------------

    /// Build a `ShapedLine` with a specific `LineWidth`.
    fn make_line_with_width(
        n_glyphs: usize,
        cell_w: f32,
        colors: StateColors,
        decorations: FontDecorationFlags,
        lw: LineWidth,
    ) -> Arc<ShapedLine> {
        use crate::gui::font_manager::FaceId;
        let glyphs: Vec<ShapedGlyph> = (0..n_glyphs)
            .map(|i| ShapedGlyph {
                glyph_id: 36,
                #[allow(clippy::cast_precision_loss)]
                x_px: i as f32 * cell_w,
                y_offset: 0.0,
                face_id: FaceId::PrimaryRegular,
                is_color: false,
                cell_width: 1,
                source_char: '\0',
            })
            .collect();
        Arc::new(ShapedLine {
            runs: vec![ShapedRun {
                glyphs,
                col_start: 0,
                style: crate::gui::font_manager::GlyphStyle::new(false, false),
                font_weight: FontWeight::Normal,
                font_decorations: decorations,
                colors,
                url: None,
                blink: BlinkState::None,
            }],
            line_width: lw,
        })
    }

    #[test]
    fn x_scale_normal_is_one() {
        assert!((x_scale(LineWidth::Normal) - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn x_scale_double_variants_are_two() {
        assert!((x_scale(LineWidth::DoubleWidth) - 2.0).abs() < f32::EPSILON);
        assert!((x_scale(LineWidth::DoubleHeightTop) - 2.0).abs() < f32::EPSILON);
        assert!((x_scale(LineWidth::DoubleHeightBottom) - 2.0).abs() < f32::EPSILON);
    }

    #[test]
    fn row_glyph_params_normal_scales_are_one() {
        let p = RowGlyphParams::new(LineWidth::Normal, 16.0, 0, 14.0);
        assert!((p.x_scale - 1.0).abs() < f32::EPSILON);
        assert!((p.y_scale - 1.0).abs() < f32::EPSILON);
        assert!((p.y_origin_shift - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn row_glyph_params_double_width_no_y_scale() {
        let p = RowGlyphParams::new(LineWidth::DoubleWidth, 16.0, 0, 14.0);
        assert!((p.x_scale - 2.0).abs() < f32::EPSILON);
        assert!((p.y_scale - 1.0).abs() < f32::EPSILON);
        assert!((p.y_origin_shift - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn row_glyph_params_double_height_top_scales() {
        let p = RowGlyphParams::new(LineWidth::DoubleHeightTop, 16.0, 0, 14.0);
        assert!((p.x_scale - 2.0).abs() < f32::EPSILON);
        assert!((p.y_scale - 2.0).abs() < f32::EPSILON);
        assert!((p.y_origin_shift - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn row_glyph_params_double_height_bottom_shifts_origin() {
        let cell_h = 16.0_f32;
        let p = RowGlyphParams::new(LineWidth::DoubleHeightBottom, cell_h, 0, 14.0);
        assert!((p.x_scale - 2.0).abs() < f32::EPSILON);
        assert!((p.y_scale - 2.0).abs() < f32::EPSILON);
        assert!((p.y_origin_shift - (-cell_h)).abs() < f32::EPSILON);
    }

    #[test]
    fn row_glyph_params_row_offset_affects_baseline() {
        let cell_h = 16.0_f32;
        let ascent = 14.0_f32;
        let p0 = RowGlyphParams::new(LineWidth::Normal, cell_h, 0, ascent);
        let p1 = RowGlyphParams::new(LineWidth::Normal, cell_h, 1, ascent);
        // Baseline at row 0 = 0 * cell_h + ascent = ascent
        assert!((p0.baseline_y - ascent).abs() < f32::EPSILON);
        // Baseline at row 1 = 1 * cell_h + ascent
        assert!((p1.baseline_y - (cell_h + ascent)).abs() < f32::EPSILON);
    }

    #[test]
    fn emit_bg_cells_normal_produces_n_instances() {
        let mut instances = Vec::new();
        let color = [1.0, 0.0, 0.0, 1.0];
        emit_bg_cells(&mut instances, 0, 5, 0, 1.0, color);
        // Normal scale (1.0): 5 logical cols → 5 instances × 6 floats each.
        assert_eq!(instances.len(), 5 * 6);
    }

    #[test]
    fn emit_bg_cells_double_width_produces_2n_instances() {
        let mut instances = Vec::new();
        let color = [0.0, 1.0, 0.0, 1.0];
        emit_bg_cells(&mut instances, 0, 5, 0, 2.0, color);
        // Double scale (2.0): 5 logical cols → 10 physical instances × 6 floats.
        assert_eq!(instances.len(), 10 * 6);
    }

    #[test]
    fn emit_bg_cells_double_width_physical_col_indices() {
        let mut instances = Vec::new();
        let color = [0.0, 0.0, 1.0, 1.0];
        // 2 logical columns starting at col_start=1 → physical cols 2,3,4,5
        emit_bg_cells(&mut instances, 1, 2, 0, 2.0, color);
        // 4 instances × 6 floats = 24
        assert_eq!(instances.len(), 24);
        // First instance: physical col = (1+0)*2 = 2
        assert!((instances[0] - 2.0).abs() < f32::EPSILON);
        // Second instance: physical col = (1+0)*2 + 1 = 3
        assert!((instances[6] - 3.0).abs() < f32::EPSILON);
        // Third instance: physical col = (1+1)*2 = 4
        assert!((instances[12] - 4.0).abs() < f32::EPSILON);
        // Fourth instance: physical col = (1+1)*2 + 1 = 5
        assert!((instances[18] - 5.0).abs() < f32::EPSILON);
    }

    #[test]
    fn bg_instances_double_width_row_doubles_colored_cells() {
        // A double-width line with colored BG should produce twice as many
        // instances as a normal-width line.
        let colored = StateColors {
            background_color: TerminalColor::Custom(255, 0, 0),
            ..StateColors::default()
        };
        let normal_line = make_line_with_width(
            5,
            8.0,
            colored,
            FontDecorationFlags::empty(),
            LineWidth::Normal,
        );
        let dw_line = make_line_with_width(
            5,
            8.0,
            colored,
            FontDecorationFlags::empty(),
            LineWidth::DoubleWidth,
        );
        let (bg_normal, _) = bg_instances_test(
            &[normal_line],
            8,
            16,
            false,
            false,
            CursorPos::default(),
            &CursorVisualStyle::BlockCursorSteady,
        );
        let (bg_dw, _) = bg_instances_test(
            &[dw_line],
            8,
            16,
            false,
            false,
            CursorPos::default(),
            &CursorVisualStyle::BlockCursorSteady,
        );
        // Each double-width logical column produces 2 physical BG instances,
        // so the total instance count should be double.
        assert_eq!(bg_dw.len(), bg_normal.len() * 2);
    }

    #[test]
    fn hover_tint_spans_full_term_width_not_shaped_run_extent() {
        // Regression for PR #333 review comment 5: the hover-tint
        // band used to derive its width from the last shaped run on
        // each row, which silently truncated the tint at the last
        // non-blank glyph.  After the fix, the band always spans
        // `term_width_cols * cell_width`, regardless of how short the
        // shaped content on the row is.
        let cell_width_px: u32 = 8;
        let cell_height_px: u32 = 16;
        let cell_width_f32 = gl_f32_u32(cell_width_px);
        // Five glyphs of content on an 80-column terminal.  Pre-fix,
        // the tint would have spanned 5 * cell_width.  Post-fix it
        // must span 80 * cell_width.
        let line = make_line(
            5,
            cell_width_f32,
            default_colors(),
            FontDecorationFlags::empty(),
        );
        let mut instances = Vec::new();
        let mut deco = Vec::new();
        let cursor_quad_appended = build_background_instances(
            &BackgroundFrame {
                shaped_lines: &[line],
                cell_width: cell_width_px,
                cell_height: cell_height_px,
                ascent: 14.0,
                underline_offset: 13.0,
                strikeout_offset: 8.0,
                stroke_size: 1.0,
                show_cursor: false,
                cursor_blink_on: false,
                cursor_pixel_pos: (0.0, 0.0),
                cursor_width_scale: 1.0,
                cursor_visual_style: &CursorVisualStyle::BlockCursorSteady,
                selection: None,
                selection_is_block: false,
                match_highlights: &[],
                command_block_hover_rows: Some((0, 0)),
                term_width_cols: 80,
                theme: &themes::CATPPUCCIN_MOCHA,
                cursor_color_override: None,
                reverse_screen: false,
            },
            &mut instances,
            &mut deco,
        );
        assert!(!cursor_quad_appended, "show_cursor was false");

        // The only deco quad produced (no cursor, no underlines, no
        // selection, no search highlights) is the hover-tint quad.
        // push_quad writes 6 vertices × 6 floats = 36 floats per quad.
        assert_eq!(
            deco.len(),
            36,
            "expected exactly one hover-tint quad in deco vec"
        );
        // Layout: vertex 0 = (x0, y0, r, g, b, a), vertex 1 = (x1, y0, ...).
        let x0 = deco[0];
        let x1 = deco[6];
        let span = x1 - x0;
        let expected_span = gl_f32(80usize) * cell_width_f32;
        assert!(
            (span - expected_span).abs() < f32::EPSILON,
            "hover-tint span = {span}, expected {expected_span} \
             (80 cols × {cell_width_f32} px/cell); shaped-line truncation regressed"
        );
    }

    #[test]
    fn hover_tint_skipped_when_term_width_cols_zero() {
        // Defensive: a zero-width terminal (degenerate snapshot, e.g.
        // before first resize) should not produce a hover tint even
        // when hover rows are set.  The fix guards on term_width_cols
        // > 0 so we don't push a zero-width quad.
        let cell_width_px: u32 = 8;
        let cell_height_px: u32 = 16;
        let line = make_line(
            5,
            gl_f32_u32(cell_width_px),
            default_colors(),
            FontDecorationFlags::empty(),
        );
        let mut instances = Vec::new();
        let mut deco = Vec::new();
        let cursor_quad_appended = build_background_instances(
            &BackgroundFrame {
                shaped_lines: &[line],
                cell_width: cell_width_px,
                cell_height: cell_height_px,
                ascent: 14.0,
                underline_offset: 13.0,
                strikeout_offset: 8.0,
                stroke_size: 1.0,
                show_cursor: false,
                cursor_blink_on: false,
                cursor_pixel_pos: (0.0, 0.0),
                cursor_width_scale: 1.0,
                cursor_visual_style: &CursorVisualStyle::BlockCursorSteady,
                selection: None,
                selection_is_block: false,
                match_highlights: &[],
                command_block_hover_rows: Some((0, 0)),
                term_width_cols: 0,
                theme: &themes::CATPPUCCIN_MOCHA,
                cursor_color_override: None,
                reverse_screen: false,
            },
            &mut instances,
            &mut deco,
        );
        assert!(!cursor_quad_appended, "show_cursor was false");
        assert!(
            deco.is_empty(),
            "expected no deco quads when term_width_cols == 0"
        );
    }

    // ── build_image_verts z-order (Task 100.7b) ──────────────────────

    /// Build a minimal 1x1-pixel `InlineImage` for `build_image_verts` tests.
    fn make_inline_image(id: u64) -> InlineImage {
        InlineImage {
            id,
            pixels: Arc::new(vec![0u8; 4]),
            width_px: 1,
            height_px: 1,
            display_cols: 1,
            display_rows: 1,
            size_mode: ImageSizeMode::NativePixels,
            frames: Vec::new(),
            root_gap_ms: 0,
            animation: AnimationControl::default(),
        }
    }

    /// Build a single-cell `ImagePlacement` with the given image id,
    /// z-index, and placement instance id.
    ///
    /// `instance_id` (Task 100.18) must be explicit rather than derived from
    /// `image_id` so tests can build two DISTINCT placements of the SAME
    /// image id and confirm they are bucketed separately.
    fn make_placement(image_id: u64, z_index: i32, instance_id: u64) -> ImagePlacement {
        ImagePlacement {
            image_id,
            col_in_image: 0,
            row_in_image: 0,
            protocol: ImageProtocol::Kitty,
            image_number: None,
            placement_id: None,
            z_index,
            source_crop: None,
            placement_instance: instance_id,
            subcell_offset: None,
        }
    }

    /// Higher z-index images must be emitted (and therefore drawn) after
    /// lower z-index ones, so they render on top (Task 100.7b).
    #[test]
    fn build_image_verts_orders_by_z_index() {
        let term_width = 4;
        // Cell 0 -> image A (id=1, z=5); cell 1 -> image B (id=2, z=1).
        let placements: Vec<Option<ImagePlacement>> = vec![
            Some(make_placement(1, 5, 1)),
            Some(make_placement(2, 1, 2)),
            None,
            None,
        ];
        let mut snap_images = std::collections::HashMap::new();
        snap_images.insert(1, make_inline_image(1));
        snap_images.insert(2, make_inline_image(2));

        let mut verts = Vec::new();
        let mut draw_order = Vec::new();
        build_image_verts(
            &placements,
            &snap_images,
            term_width,
            8,
            16,
            &mut verts,
            &mut draw_order,
        );

        // B (lower z) drawn first = bottom; A (higher z) drawn last = on top.
        assert_eq!(
            draw_order,
            vec![
                ImageDrawEntry {
                    instance_id: 2,
                    image_id: 2
                },
                ImageDrawEntry {
                    instance_id: 1,
                    image_id: 1
                },
            ]
        );
    }

    /// Images with equal z-index are ordered by ascending instance id for
    /// determinism.
    #[test]
    fn build_image_verts_ties_broken_by_id() {
        let term_width = 4;
        let placements: Vec<Option<ImagePlacement>> = vec![
            Some(make_placement(3, 0, 3)),
            Some(make_placement(1, 0, 1)),
            None,
            None,
        ];
        let mut snap_images = std::collections::HashMap::new();
        snap_images.insert(3, make_inline_image(3));
        snap_images.insert(1, make_inline_image(1));

        let mut verts = Vec::new();
        let mut draw_order = Vec::new();
        build_image_verts(
            &placements,
            &snap_images,
            term_width,
            8,
            16,
            &mut verts,
            &mut draw_order,
        );

        assert_eq!(
            draw_order,
            vec![
                ImageDrawEntry {
                    instance_id: 1,
                    image_id: 1
                },
                ImageDrawEntry {
                    instance_id: 3,
                    image_id: 3
                },
            ]
        );
    }

    /// Empty placements must clear `draw_order`, even if it held stale
    /// entries from a previous frame.
    #[test]
    fn build_image_verts_empty_placements_clears_draw_order() {
        let placements: Vec<Option<ImagePlacement>> = vec![None, None];
        let snap_images = std::collections::HashMap::new();

        let mut verts = Vec::new();
        // Pre-fill with stale junk to confirm the early-return path clears it.
        let mut draw_order = vec![
            ImageDrawEntry {
                instance_id: 99,
                image_id: 99,
            },
            ImageDrawEntry {
                instance_id: 98,
                image_id: 98,
            },
        ];
        build_image_verts(
            &placements,
            &snap_images,
            4,
            8,
            16,
            &mut verts,
            &mut draw_order,
        );

        assert!(
            draw_order.is_empty(),
            "draw_order should be cleared on the empty-input path"
        );
        assert!(verts.is_empty());
    }

    /// A single placed image yields exactly one entry in `draw_order`.
    #[test]
    fn build_image_verts_single_image_single_draw_order_entry() {
        let term_width = 4;
        let placements: Vec<Option<ImagePlacement>> = vec![Some(make_placement(7, 3, 7)), None];
        let mut snap_images = std::collections::HashMap::new();
        snap_images.insert(7, make_inline_image(7));

        let mut verts = Vec::new();
        let mut draw_order = Vec::new();
        build_image_verts(
            &placements,
            &snap_images,
            term_width,
            8,
            16,
            &mut verts,
            &mut draw_order,
        );

        assert_eq!(
            draw_order,
            vec![ImageDrawEntry {
                instance_id: 7,
                image_id: 7
            }]
        );
    }

    /// Two independent placements of the SAME image id (e.g. two `a=p`
    /// puts with `p=0`/unspecified) must produce TWO separate `ImageBounds`
    /// buckets/draw-order entries with distinct instance ids and
    /// correctly-sized separate quads — not one merged, oversized quad
    /// (Task 100.18).
    ///
    /// This is the fail-before/pass-after assertion for 100.18: before the
    /// fix, bucketing by `image_id` would have collapsed both placements'
    /// cells into a single `ImageBounds` entry spanning cells from BOTH
    /// placements (an oversized bounding box), yielding exactly one
    /// `draw_order` entry instead of two.
    #[test]
    fn build_image_verts_two_placements_of_same_image_id_stay_separate() {
        let term_width = 4;
        // Placement A: image id 5, instance 100, occupies cell 0 only.
        // Placement B: image id 5 (SAME image), instance 200, occupies
        // cells 2 and 3 (a 1x2 placement, distinct region on screen).
        let mut placement_b_col0 = make_placement(5, 0, 200);
        placement_b_col0.col_in_image = 0;
        let mut placement_b_col1 = make_placement(5, 0, 200);
        placement_b_col1.col_in_image = 1;

        let placements: Vec<Option<ImagePlacement>> = vec![
            Some(make_placement(5, 0, 100)),
            None,
            Some(placement_b_col0),
            Some(placement_b_col1),
        ];
        let mut snap_images = std::collections::HashMap::new();
        snap_images.insert(5, make_inline_image(5));

        let mut verts = Vec::new();
        let mut draw_order = Vec::new();
        build_image_verts(
            &placements,
            &snap_images,
            term_width,
            8,
            16,
            &mut verts,
            &mut draw_order,
        );

        assert_eq!(
            draw_order.len(),
            2,
            "two independent placements of the same image id must yield \
             two separate draw_order entries, not one merged bucket"
        );
        let instance_ids: Vec<u64> = draw_order.iter().map(|e| e.instance_id).collect();
        assert!(instance_ids.contains(&100));
        assert!(instance_ids.contains(&200));
        // Both entries reference the same underlying image id for texture
        // lookup purposes.
        assert!(draw_order.iter().all(|e| e.image_id == 5));
        // Two separate quads => 2 * VERTS_PER_QUAD * IMG_VERTEX_FLOATS floats.
        assert_eq!(verts.len(), 2 * VERTS_PER_QUAD * IMG_VERTEX_FLOATS);
    }

    /// A same NON-ZERO `placement_id` re-put (kitty spec REPLACE
    /// semantics, Task 100.18/100.20) is exercised at the
    /// `Buffer::clear_image_placements_by_placement` level in
    /// `freminal-buffer`; here we only confirm that once the OLD
    /// placement's cells are cleared (as that lower-level call does), only
    /// the NEW placement's instance remains in `draw_order`.
    #[test]
    fn build_image_verts_after_replace_only_new_instance_remains() {
        let term_width = 4;
        // Simulates the buffer state AFTER a same-`p=` re-put replaced the
        // old placement's cells: only the new instance's cell is present.
        let placements: Vec<Option<ImagePlacement>> =
            vec![Some(make_placement(9, 0, 300)), None, None, None];
        let mut snap_images = std::collections::HashMap::new();
        snap_images.insert(9, make_inline_image(9));

        let mut verts = Vec::new();
        let mut draw_order = Vec::new();
        build_image_verts(
            &placements,
            &snap_images,
            term_width,
            8,
            16,
            &mut verts,
            &mut draw_order,
        );

        assert_eq!(
            draw_order.len(),
            1,
            "only the replacement placement remains"
        );
        assert_eq!(draw_order[0].instance_id, 300);
    }

    // ── compute_image_quad UV composition (Task 100.9 source-crop) ────

    /// Build an `InlineImage` with caller-specified pixel dimensions and a
    /// single-cell display grid, for `compute_image_quad` UV tests where the
    /// pixel-space crop math must be exercised against a non-trivial image
    /// size (unlike the 1x1-pixel `make_inline_image` helper above).
    fn make_inline_image_sized(id: u64, width_px: u32, height_px: u32) -> InlineImage {
        InlineImage {
            id,
            pixels: Arc::new(vec![0u8; 4]),
            width_px,
            height_px,
            display_cols: 1,
            display_rows: 1,
            size_mode: ImageSizeMode::NativePixels,
            frames: Vec::new(),
            root_gap_ms: 0,
            animation: AnimationControl::default(),
        }
    }

    /// Like [`make_inline_image_sized`], but with `size_mode` set to
    /// `ExplicitCells` (kitty `c=`/`r=`, iTerm2 explicit `width=`/`height=`),
    /// for Task 100.17b quad-sizing tests that need to lock in the
    /// unchanged "scale to the declared cell grid" behaviour for that mode.
    fn make_inline_image_sized_explicit_cells(
        id: u64,
        width_px: u32,
        height_px: u32,
    ) -> InlineImage {
        InlineImage {
            size_mode: ImageSizeMode::ExplicitCells,
            ..make_inline_image_sized(id, width_px, height_px)
        }
    }

    /// Build a fully-visible single-cell `ImageBounds` (min == max == 0),
    /// with the given optional source-crop, for `compute_image_quad` tests.
    fn make_bounds(crop: Option<SourceCrop>) -> ImageBounds {
        ImageBounds {
            x0: 0.0,
            y0: 0.0,
            x1: 8.0,
            y1: 16.0,
            min_col_in_image: 0,
            min_row_in_image: 0,
            max_col_in_image: 0,
            max_row_in_image: 0,
            image_id: 1,
            z_index: 0,
            crop,
            subcell_offset: None,
        }
    }

    /// Extract the (u, v) pair from vertex `idx` (0-based) of a
    /// `compute_image_quad` result. Each vertex is `[x, y, u, v]` (stride 4).
    fn quad_uv(quad: &[f32; 4 * VERTS_PER_QUAD], idx: usize) -> (f32, f32) {
        (quad[idx * 4 + 2], quad[idx * 4 + 3])
    }

    /// Extract the (x, y) pair from vertex `idx` (0-based) of a
    /// `compute_image_quad` result. Each vertex is `[x, y, u, v]` (stride 4).
    fn quad_pos(quad: &[f32; 4 * VERTS_PER_QUAD], idx: usize) -> (f32, f32) {
        (quad[idx * 4], quad[idx * 4 + 1])
    }

    const UV_EPSILON: f32 = 1e-6;

    /// With no crop, a fully-visible single-cell image (1x1 display grid)
    /// must map to the full 0..1 UV range — this also covers the
    /// previously-untested plain (no-crop) UV path.
    #[test]
    fn compute_image_quad_no_crop_maps_full_uv_range() {
        let bounds = make_bounds(None);
        let img = make_inline_image_sized(1, 100, 100);

        let quad = compute_image_quad(&bounds, Some(&img), 8, 16);

        // Triangle 1: (x0,y0,u0,v0) (x1,y0,u1,v0) (x0,y1,u0,v1)
        let corner_a = quad_uv(&quad, 0);
        let corner_b = quad_uv(&quad, 1);
        let corner_c = quad_uv(&quad, 2);

        assert!(
            (corner_a.0 - 0.0).abs() < UV_EPSILON,
            "u0 should be 0.0, got {}",
            corner_a.0
        );
        assert!(
            (corner_a.1 - 0.0).abs() < UV_EPSILON,
            "v0 should be 0.0, got {}",
            corner_a.1
        );
        assert!(
            (corner_b.0 - 1.0).abs() < UV_EPSILON,
            "u1 should be 1.0, got {}",
            corner_b.0
        );
        assert!(
            (corner_b.1 - 0.0).abs() < UV_EPSILON,
            "v0 should be 0.0, got {}",
            corner_b.1
        );
        assert!(
            (corner_c.0 - 0.0).abs() < UV_EPSILON,
            "u0 should be 0.0, got {}",
            corner_c.0
        );
        assert!(
            (corner_c.1 - 1.0).abs() < UV_EPSILON,
            "v1 should be 1.0, got {}",
            corner_c.1
        );
    }

    /// A source-crop (kitty `a=p` x/y/w/h) on a fully-visible single-cell
    /// image must re-map the UV range into the crop's pixel-space window
    /// instead of the full texture (Task 100.9).
    #[test]
    fn compute_image_quad_with_crop_maps_to_crop_sub_range() {
        let bounds = make_bounds(Some(SourceCrop {
            x: 25,
            y: 25,
            width: 50,
            height: 50,
        }));
        let img = make_inline_image_sized(1, 100, 100);

        let quad = compute_image_quad(&bounds, Some(&img), 8, 16);

        let corner_a = quad_uv(&quad, 0);
        let corner_b = quad_uv(&quad, 1);
        let corner_c = quad_uv(&quad, 2);

        assert!(
            (corner_a.0 - 0.25).abs() < UV_EPSILON,
            "u0 should be 0.25, got {}",
            corner_a.0
        );
        assert!(
            (corner_a.1 - 0.25).abs() < UV_EPSILON,
            "v0 should be 0.25, got {}",
            corner_a.1
        );
        assert!(
            (corner_b.0 - 0.75).abs() < UV_EPSILON,
            "u1 should be 0.75, got {}",
            corner_b.0
        );
        assert!(
            (corner_b.1 - 0.25).abs() < UV_EPSILON,
            "v0 should be 0.25, got {}",
            corner_b.1
        );
        assert!(
            (corner_c.0 - 0.25).abs() < UV_EPSILON,
            "u0 should be 0.25, got {}",
            corner_c.0
        );
        assert!(
            (corner_c.1 - 0.75).abs() < UV_EPSILON,
            "v1 should be 0.75, got {}",
            corner_c.1
        );
    }

    // ── compute_image_quad position sizing (Task 100.17b) ────

    const POS_EPSILON: f32 = 1e-4;

    /// `ImageSizeMode::NativePixels` with a native size SMALLER than the
    /// cell bounding box must produce a quad sized to the native pixels
    /// (anchored at `b.x0`/`b.y0`), NOT stretched to fill the full cell
    /// box. This is the core Task 100.17 fail-before/pass-after
    /// assertion: before this fix, `x1-x0`/`y1-y0` equalled the 8x16 cell
    /// box regardless of the 4x4 native size.
    #[test]
    fn compute_image_quad_native_pixels_sizes_to_native_dimensions_not_cell_box() {
        let bounds = make_bounds(None); // 8x16 cell box (b.x1=8, b.y1=16)
        let img = make_inline_image_sized(1, 4, 4); // native 4x4px, size_mode NativePixels

        let quad = compute_image_quad(&bounds, Some(&img), 8, 16);
        let (x0, y0) = quad_pos(&quad, 0);
        let (x1, _) = quad_pos(&quad, 1);
        let (_, y1) = quad_pos(&quad, 2);

        assert!((x0 - 0.0).abs() < POS_EPSILON, "x0 should be 0.0, got {x0}");
        assert!((y0 - 0.0).abs() < POS_EPSILON, "y0 should be 0.0, got {y0}");
        assert!(
            (x1 - 4.0).abs() < POS_EPSILON,
            "x1-x0 should equal native width_px (4), got x1={x1}"
        );
        assert!(
            (y1 - 4.0).abs() < POS_EPSILON,
            "y1-y0 should equal native height_px (4), got y1={y1}"
        );
    }

    /// `ImageSizeMode::ExplicitCells` must still scale the quad to fill the
    /// full cell bounding box — unchanged pre-100.17 behaviour, locked in
    /// so a future regression can't silently apply native sizing to
    /// explicitly-sized (kitty `c=`/`r=`, iTerm2 explicit width/height)
    /// images.
    #[test]
    fn compute_image_quad_explicit_cells_sizes_to_cell_box() {
        let bounds = make_bounds(None); // 8x16 cell box
        let img = make_inline_image_sized_explicit_cells(1, 4, 4); // native 4x4px, but ExplicitCells

        let quad = compute_image_quad(&bounds, Some(&img), 8, 16);
        let (x0, y0) = quad_pos(&quad, 0);
        let (x1, _) = quad_pos(&quad, 1);
        let (_, y1) = quad_pos(&quad, 2);

        assert!((x0 - 0.0).abs() < POS_EPSILON);
        assert!((y0 - 0.0).abs() < POS_EPSILON);
        assert!(
            (x1 - 8.0).abs() < POS_EPSILON,
            "x1 should equal the full cell box x1 (8), got {x1}"
        );
        assert!(
            (y1 - 16.0).abs() < POS_EPSILON,
            "y1 should equal the full cell box y1 (16), got {y1}"
        );
    }

    /// `NativePixels` composed with a source-crop (kitty `a=p`, Task 100.9)
    /// must size the quad to the CROP's pixel dimensions, not the full
    /// image's — a cropped native-size image displays at the crop's actual
    /// size while the UV window (covered by the existing crop UV test)
    /// still samples only that sub-rectangle of the texture.
    #[test]
    fn compute_image_quad_native_pixels_with_crop_sizes_to_crop_dimensions() {
        let bounds = make_bounds(Some(SourceCrop {
            x: 10,
            y: 10,
            width: 50,
            height: 30,
        }));
        let img = make_inline_image_sized(1, 100, 100); // full image is 100x100, crop is 50x30

        let quad = compute_image_quad(&bounds, Some(&img), 8, 16);
        let (x0, y0) = quad_pos(&quad, 0);
        let (x1, _) = quad_pos(&quad, 1);
        let (_, y1) = quad_pos(&quad, 2);

        assert!((x0 - 0.0).abs() < POS_EPSILON);
        assert!((y0 - 0.0).abs() < POS_EPSILON);
        assert!(
            (x1 - 50.0).abs() < POS_EPSILON,
            "x1-x0 should equal the crop width (50), got x1={x1}"
        );
        assert!(
            (y1 - 30.0).abs() < POS_EPSILON,
            "y1-y0 should equal the crop height (30), got y1={y1}"
        );
    }

    /// `NativePixels` on a partially-visible image (only some of the
    /// image's declared cell grid is within `b`, e.g. clipped at the
    /// terminal's right edge) must scale the native pixel extent by the
    /// same visible-cell fraction used for the UV sub-range, so quad
    /// geometry and texture sampling stay proportionally aligned.
    #[test]
    fn compute_image_quad_native_pixels_partial_visibility_scales_proportionally() {
        // A 20x10px native image declared across a 2x1 cell grid; only the
        // first column (col_in_image == 0) is visible (e.g. the second
        // column was clipped at the terminal edge) => 1 of 2 cols visible.
        let bounds = ImageBounds {
            x0: 0.0,
            y0: 0.0,
            x1: 8.0,
            y1: 16.0,
            min_col_in_image: 0,
            min_row_in_image: 0,
            max_col_in_image: 0,
            max_row_in_image: 0,
            image_id: 1,
            z_index: 0,
            crop: None,
            subcell_offset: None,
        };
        let img = InlineImage {
            id: 1,
            pixels: Arc::new(vec![0u8; 4]),
            width_px: 20,
            height_px: 10,
            display_cols: 2,
            display_rows: 1,
            size_mode: ImageSizeMode::NativePixels,
            frames: Vec::new(),
            root_gap_ms: 0,
            animation: AnimationControl::default(),
        };

        let quad = compute_image_quad(&bounds, Some(&img), 8, 16);
        let (x0, y0) = quad_pos(&quad, 0);
        let (x1, _) = quad_pos(&quad, 1);
        let (_, y1) = quad_pos(&quad, 2);

        assert!((x0 - 0.0).abs() < POS_EPSILON);
        assert!((y0 - 0.0).abs() < POS_EPSILON);
        // 1 of 2 cols visible => half the native 20px width.
        assert!(
            (x1 - 10.0).abs() < POS_EPSILON,
            "x1-x0 should be half the native width (10), got x1={x1}"
        );
        // Full row visible (1 of 1) => full native 10px height.
        assert!(
            (y1 - 10.0).abs() < POS_EPSILON,
            "y1-y0 should equal the full native height (10), got y1={y1}"
        );
    }

    // ── compute_image_quad sub-cell X/Y offset (Task 100.19) ────

    /// A kitty `X=`/`Y=` sub-cell offset must translate the quad's origin
    /// (all four corners) by that many pixels — position only, UVs
    /// unaffected.
    #[test]
    fn compute_image_quad_subcell_offset_translates_quad_position() {
        let mut bounds = make_bounds(None); // 8x16 cell box (x0=0,y0=0,x1=8,y1=16)
        bounds.subcell_offset = Some(SubCellOffset { x: 3, y: 5 });
        let img = make_inline_image_sized_explicit_cells(1, 100, 100);

        let quad = compute_image_quad(&bounds, Some(&img), 8, 16);
        let (x0, y0) = quad_pos(&quad, 0);
        let (x1, _) = quad_pos(&quad, 1);
        let (_, y1) = quad_pos(&quad, 2);

        assert!(
            (x0 - 3.0).abs() < POS_EPSILON,
            "x0 should be translated by the offset (3), got {x0}"
        );
        assert!(
            (y0 - 5.0).abs() < POS_EPSILON,
            "y0 should be translated by the offset (5), got {y0}"
        );
        // ExplicitCells sizes to the full 8x16 cell box; translated by the
        // offset, x1 = 8 + 3 = 11, y1 = 16 + 5 = 21.
        assert!(
            (x1 - 11.0).abs() < POS_EPSILON,
            "x1 should also be translated by the offset, got {x1}"
        );
        assert!(
            (y1 - 21.0).abs() < POS_EPSILON,
            "y1 should also be translated by the offset, got {y1}"
        );
    }

    /// A sub-cell offset at or beyond the cell's pixel dimensions must be
    /// defensively re-clamped to strictly less than `cell_width`/
    /// `cell_height`, even though the resolving handler already clamps.
    #[test]
    fn compute_image_quad_subcell_offset_defensively_clamped_to_cell_size() {
        let mut bounds = make_bounds(None);
        bounds.subcell_offset = Some(SubCellOffset { x: 100, y: 100 });
        let img = make_inline_image_sized_explicit_cells(1, 100, 100);

        let quad = compute_image_quad(&bounds, Some(&img), 8, 16);
        let (x0, y0) = quad_pos(&quad, 0);

        assert!(
            (x0 - 7.0).abs() < POS_EPSILON,
            "x offset should clamp to cell_width - 1 (7), got {x0}"
        );
        assert!(
            (y0 - 15.0).abs() < POS_EPSILON,
            "y offset should clamp to cell_height - 1 (15), got {y0}"
        );
    }

    /// No sub-cell offset (`None`) must leave the quad position exactly as
    /// `compute_image_quad_position` computed it — no translation applied.
    #[test]
    fn compute_image_quad_no_subcell_offset_is_untranslated() {
        let bounds = make_bounds(None);
        let img = make_inline_image_sized_explicit_cells(1, 100, 100);

        let quad = compute_image_quad(&bounds, Some(&img), 8, 16);
        let (x0, y0) = quad_pos(&quad, 0);

        assert!((x0 - 0.0).abs() < POS_EPSILON);
        assert!((y0 - 0.0).abs() < POS_EPSILON);
    }
}
