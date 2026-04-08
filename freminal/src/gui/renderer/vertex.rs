// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Pure CPU vertex builders for the terminal rendering pipeline.
//!
//! All functions in this module are fully testable without a GL context.
//! They build flat `Vec<f32>` buffers that are subsequently uploaded to the
//! GPU by the [`super::gpu`] module.

use conv2::{ApproxFrom, ConvUtil};
use freminal_common::buffer_states::fonts::{BlinkState, FontDecorations, UnderlineStyle};
use freminal_common::cursor::CursorVisualStyle;
use freminal_common::themes::ThemePalette;
use freminal_terminal_emulator::{ImagePlacement, InlineImage};
use std::sync::Arc;

use super::super::{
    atlas::{GlyphAtlas, GlyphKey},
    colors::{
        cursor_f, internal_color_to_gl, search_current_bg_f, search_match_bg_f, selection_bg_f,
        selection_fg_f,
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
        }
    }
}

/// Per-image tracking: pixel bounding box and cell-grid extent within the
/// image.  The cell-grid extent (min/max `col_in_image`, `row_in_image`) tells
/// us which portion of the texture is visible, so we can compute UV
/// coordinates that preserve aspect ratio even when the image is partially
/// clipped by the terminal edge.
struct ImageBounds {
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    min_col_in_image: usize,
    min_row_in_image: usize,
    max_col_in_image: usize,
    max_row_in_image: usize,
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
#[must_use]
// All parameters are required geometric and style inputs for GPU instance data generation.
// Inherently large: iterates all shaped lines, resolving background color for every cell.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub fn build_background_instances(
    shaped_lines: &[Arc<ShapedLine>],
    cell_width: u32,
    cell_height: u32,
    ascent: f32,
    underline_offset: f32,
    strikeout_offset: f32,
    stroke_size: f32,
    show_cursor: bool,
    cursor_blink_on: bool,
    cursor_pixel_pos: (f32, f32),
    cursor_visual_style: &CursorVisualStyle,
    selection: Option<(usize, usize, usize, usize)>,
    selection_is_block: bool,
    match_highlights: &[MatchHighlight],
    theme: &ThemePalette,
    cursor_color_override: Option<(u8, u8, u8)>,
) -> (Vec<f32>, Vec<f32>) {
    let mut instances: Vec<f32> = Vec::new();
    let mut deco: Vec<f32> = Vec::new();

    for (row_idx, line) in shaped_lines.iter().enumerate() {
        let y_top = gl_f32(row_idx) * gl_f32_u32(cell_height);

        // --- Per-cell background instances ---
        for run in &line.runs {
            let is_faint = run.font_decorations.contains(FontDecorations::Faint);
            let bg_color_raw = run.colors.get_background_color();

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
            let col_count = run_col_count(run);
            for c in 0..col_count {
                let col = run.col_start + c;
                instances.push(gl_f32(col));
                instances.push(gl_f32(row_idx));
                instances.push(r);
                instances.push(g);
                instances.push(b);
                instances.push(a);
            }
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
            let x0 = gl_f32(run.col_start) * gl_f32_u32(cell_width);
            let x1 = gl_f32(col_end) * gl_f32_u32(cell_width);

            if underline_style.is_active() {
                // Use underline color if set, otherwise fall back to foreground.
                let ul_color_raw = run.colors.get_underline_color();
                let ul_color = if matches!(
                    ul_color_raw,
                    freminal_common::colors::TerminalColor::DefaultUnderlineColor
                ) {
                    internal_color_to_gl(run.colors.get_color(), is_faint, theme)
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
                    &mut deco,
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
                let fg_color = internal_color_to_gl(run.colors.get_color(), is_faint, theme);
                // strikeout_offset from OS/2 is positive (above baseline in font
                // coords).  In top-down pixel coords, subtracting it from the
                // baseline places the line above the baseline (middle of cell).
                let st_top = y_top + ascent - strikeout_offset;
                let st_bot = st_top + stroke_size.max(1.0);
                push_quad(&mut deco, x0, st_top, x1, st_bot, fg_color);
            }
        }
    }

    // --- Selection highlight quads (decoration pass) ---
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

            let x0 = gl_f32(col_begin) * cw;
            let x1 = gl_f32(col_end + 1) * cw;
            let y0 = gl_f32(row) * ch;
            let y1 = y0 + ch;

            push_quad(&mut deco, x0, y0, x1, y1, selection_bg_f(theme));
        }
    }

    // --- Search match highlight quads ---
    for m in match_highlights {
        if m.row >= shaped_lines.len() || m.col_start > m.col_end {
            continue;
        }
        let cw = gl_f32_u32(cell_width);
        let ch = gl_f32_u32(cell_height);
        let x0 = gl_f32(m.col_start) * cw;
        let x1 = gl_f32(m.col_end + 1) * cw;
        let y0 = gl_f32(m.row) * ch;
        let y1 = y0 + ch;
        let color = if m.is_current {
            search_current_bg_f()
        } else {
            search_match_bg_f()
        };
        push_quad(&mut deco, x0, y0, x1, y1, color);
    }

    // --- Cursor quad (always last in deco so cursor-only patches work) ---
    if show_cursor && cursor_blink_is_visible(cursor_visual_style, cursor_blink_on) {
        let (cx, cy) = cursor_pixel_pos;
        let cw = gl_f32_u32(cell_width);
        let ch = gl_f32_u32(cell_height);

        let color = cursor_f(theme, cursor_color_override);

        match cursor_visual_style {
            CursorVisualStyle::BlockCursorBlink | CursorVisualStyle::BlockCursorSteady => {
                push_quad(&mut deco, cx, cy, cx + cw, cy + ch, color);
            }
            CursorVisualStyle::UnderlineCursorBlink | CursorVisualStyle::UnderlineCursorSteady => {
                let bar_h = (ch * 0.1).max(2.0);
                push_quad(&mut deco, cx, cy + ch - bar_h, cx + cw, cy + ch, color);
            }
            CursorVisualStyle::VerticalLineCursorBlink
            | CursorVisualStyle::VerticalLineCursorSteady => {
                let bar_w = (cw * 0.1).max(1.0);
                push_quad(&mut deco, cx, cy, cx + bar_w, cy + ch, color);
            }
        }
    }

    (instances, deco)
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
    cursor_visual_style: &CursorVisualStyle,
    theme: &ThemePalette,
    cursor_color_override: Option<(u8, u8, u8)>,
) -> Vec<f32> {
    let mut verts = Vec::new();

    if show_cursor && cursor_blink_is_visible(cursor_visual_style, cursor_blink_on) {
        let (cx, cy) = cursor_pixel_pos;
        let cw = gl_f32_u32(cell_width);
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
/// Returns a flat `Vec<f32>` with `FG_INSTANCE_FLOATS` floats per glyph instance.
#[must_use]
pub fn build_foreground_instances(
    shaped_lines: &[Arc<ShapedLine>],
    atlas: &mut GlyphAtlas,
    font_manager: &FontManager,
    cell_height: u32,
    ascent: f32,
    opts: &FgRenderOptions,
    theme: &ThemePalette,
) -> Vec<f32> {
    let mut instances: Vec<f32> = Vec::new();

    for (row_idx, line) in shaped_lines.iter().enumerate() {
        let row_f = gl_f32(row_idx);
        let cell_h_f = gl_f32_u32(cell_height);
        let baseline_y = row_f.mul_add(cell_h_f, ascent);

        // Cell vertical extent for this row (used to clip oversized glyphs).
        let cell_top = row_f * cell_h_f;
        let cell_bottom = cell_top + cell_h_f;

        for run in &line.runs {
            let is_faint = run.font_decorations.contains(FontDecorations::Faint);
            let normal_fg = internal_color_to_gl(run.colors.get_color(), is_faint, theme);

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
                        &mut instances,
                        glyph,
                        atlas,
                        font_manager,
                        baseline_y,
                        fg_color,
                        [cell_top, cell_bottom],
                    );
                }

                col += glyph.cell_width;
            }
        }
    }

    instances
}

// ---------------------------------------------------------------------------
//  Build image verts
// ---------------------------------------------------------------------------

/// Build the image vertex buffer from the visible placements.
///
/// Emits one textured quad per unique image ID found in `placements`.
/// Images are sorted by ID before emission so that `draw_images` (which
/// iterates `snap_images.keys()` in the same sorted order) draws the
/// correct texture for each quad.
///
/// Each quad covers the union of all cells that belong to a given image in
/// the current visible window (i.e. the full image bounding box).
///
/// `placements` is parallel to `visible_chars`: one entry per cell in
/// row-major order.  `term_width` is the number of columns per row.
/// `cell_width` and `cell_height` are integer pixel sizes.
///
/// Returns a flat `Vec<f32>` with `IMG_VERTEX_FLOATS` floats per vertex,
/// `VERTS_PER_QUAD` vertices per image quad.
#[must_use]
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
) -> Vec<f32> {
    if placements.is_empty() || snap_images.is_empty() {
        return Vec::new();
    }

    let mut bounds: std::collections::HashMap<u64, ImageBounds> = std::collections::HashMap::new();

    for (cell_idx, placement) in placements.iter().enumerate() {
        let Some(p) = placement else { continue };
        let col = if term_width == 0 {
            0
        } else {
            cell_idx % term_width
        };
        let row = if term_width == 0 {
            0
        } else {
            cell_idx / term_width
        };

        let x0 = gl_f32(col) * gl_f32_u32(cell_width);
        let y0 = gl_f32(row) * gl_f32_u32(cell_height);
        let x1 = x0 + gl_f32_u32(cell_width);
        let y1 = y0 + gl_f32_u32(cell_height);

        let id = p.image_id;
        let entry = bounds.entry(id).or_insert(ImageBounds {
            x0,
            y0,
            x1,
            y1,
            min_col_in_image: p.col_in_image,
            min_row_in_image: p.row_in_image,
            max_col_in_image: p.col_in_image,
            max_row_in_image: p.row_in_image,
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

    // Emit quads sorted by image ID (must match draw_images iteration order).
    let mut sorted_ids: Vec<u64> = bounds.keys().copied().collect();
    sorted_ids.sort_unstable();

    let mut verts = Vec::with_capacity(sorted_ids.len() * VERTS_PER_QUAD * IMG_VERTEX_FLOATS);

    for id in &sorted_ids {
        let Some(b) = bounds.get(id) else {
            continue;
        };

        let quad = compute_image_quad(b, snap_images.get(id));
        verts.extend_from_slice(&quad);
    }

    verts
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
/// The quad pixel size is determined by the bounding box `b`, which is already
/// computed in renderer cell pixels.  This means the image is scaled to fill
/// exactly the declared cell grid — matching the Kitty protocol intent.
fn compute_image_quad(b: &ImageBounds, img: Option<&InlineImage>) -> [f32; 4 * VERTS_PER_QUAD] {
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

    [
        // Triangle 1
        b.x0, b.y0, u0, v0, b.x1, b.y0, u1, v0, b.x0, b.y1, u0, v1, // Triangle 2
        b.x1, b.y0, u1, v0, b.x1, b.y1, u1, v1, b.x0, b.y1, u0, v1,
    ]
}

/// Emit a single foreground glyph instance (13 floats).
///
/// Looks up (or rasterises) the atlas entry for the glyph, then pushes one
/// instance into `instances`.  Glyphs that extend beyond the cell's vertical
/// extent (`cell_y_range[0]`..`cell_y_range[1]`) are clipped, and their UV
/// and position are adjusted proportionally so the visible portion of the
/// atlas texture is correct.
fn emit_glyph_instance(
    instances: &mut Vec<f32>,
    glyph: &ShapedGlyph,
    atlas: &mut GlyphAtlas,
    font_manager: &FontManager,
    baseline_y: f32,
    fg_color: [f32; 4],
    cell_y_range: [f32; 2],
) {
    use conv2::ValueFrom;

    // Determine pixel size from the atlas key.
    // We use the font manager's cell height as the size_px for rasterisation.
    let size_px = u16::value_from(font_manager.cell_height()).unwrap_or(u16::MAX);

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

    // Pixel position: cell-grid x + bearing, baseline_y - bearing_y.
    let x0 = glyph.x_px + f32::from(entry.bearing_x);
    let raw_y0 = baseline_y - f32::from(entry.bearing_y);
    let x1 = x0 + f32::from(entry.width);
    let raw_y1 = raw_y0 + f32::from(entry.height);

    // --- Cell-boundary clipping ---
    //
    // Oversized glyphs (e.g. powerline symbols in Nerd Fonts) may extend
    // above or below the cell.  Clamp the quad to the cell's vertical
    // extent and adjust the UV coordinates proportionally so only the
    // visible portion of the atlas texture is sampled.
    let cell_top = cell_y_range[0];
    let cell_bottom = cell_y_range[1];
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

    let is_color_f: f32 = if glyph.is_color { 1.0 } else { 0.0 };

    // One instance: glyph_x, glyph_y, glyph_w, glyph_h, u0, v0, u1, v1, r, g, b, a, is_color
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
        is_color_f,
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
    let stride = (atlas_size as usize) * 4;
    let row_bytes = (rect.width as usize) * 4;
    let mut out = Vec::with_capacity((rect.height as usize) * row_bytes);

    for row in 0..rect.height {
        let y = (rect.y + row) as usize;
        let x = rect.x as usize;
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
    use super::*;
    use freminal_common::buffer_states::cursor::CursorPos;
    use freminal_common::buffer_states::fonts::FontDecorationFlags;
    use freminal_common::config::Config;
    use freminal_common::themes;

    use crate::gui::font_manager::FontManager;
    use crate::gui::shaping::{ShapedGlyph, ShapedLine, ShapedRun};
    use freminal_common::buffer_states::cursor::StateColors;
    use freminal_common::buffer_states::fonts::FontWeight;
    use freminal_common::colors::TerminalColor;

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
        build_background_instances(
            lines,
            cell_width,
            cell_height,
            14.0, // ascent (approximate for test font)
            13.0,
            8.0,
            1.0,
            show_cursor,
            cursor_blink_on,
            cursor_pixel_pos,
            cursor_style,
            None,
            false,
            &[],
            &themes::CATPPUCCIN_MOCHA,
            None,
        )
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

    #[test]
    fn fg_instances_empty_on_empty_lines() {
        let instances = build_foreground_instances(
            &[],
            &mut GlyphAtlas::default(),
            &FontManager::new(&Config::default(), 1.0),
            16,
            13.0,
            &FgRenderOptions::all_visible(None),
            &themes::CATPPUCCIN_MOCHA,
        );
        assert_eq!(instances.len(), 0);
    }

    #[test]
    fn fg_instances_produces_data_for_ascii_glyphs() {
        let mut fm = FontManager::new(&Config::default(), 1.0);
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
        let lines = cache.shape_visible(&chars, &tags, 80, &mut fm, cell_w, false);

        let instances = build_foreground_instances(
            &lines,
            &mut atlas,
            &fm,
            cell_h,
            ascent,
            &FgRenderOptions::all_visible(None),
            &themes::CATPPUCCIN_MOCHA,
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
}
