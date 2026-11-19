// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Underline and strikethrough decoration geometry (Task 140).
//!
//! Pure CPU builders that turn a shaped run's underline style and
//! strikethrough flag into solid-colour quads in the decoration vertex
//! stream. Called from [`super::vertex::build_background_instances`].

use conv2::ConvUtil;
use freminal_common::buffer_states::fonts::{FontDecorations, UnderlineStyle};
use freminal_terminal_emulator::LineWidth;

use super::super::{colors::internal_color_to_gl, shaping::ShapedRun};
use super::vertex::{
    BackgroundFrame, QUAD_FLOATS, QuadBounds, effective_fg, gl_f32, gl_f32_u32, push_quad,
    quad_bounds, quad_vertices, run_col_count, x_scale,
};

/// Vertical layout of a row's decorations.
///
/// On DECDHL rows the glyphs are scaled to twice the cell height from the
/// pair's top, and the bottom half shows the lower half of that doubled box.
/// Decorations are laid out in the same doubled box and then clipped to the
/// row's own cell, so the two halves of a pair show one continuous pattern.
struct RowLayout {
    /// Top edge of the box the decoration is laid out in: the row's top, or
    /// one cell above it for the bottom half of a DECDHL pair.
    box_top: f32,
    /// Vertical scale applied to the box height, ascent, offsets and stroke.
    y_scale: f32,
    /// The row's own cell `[top, bottom]` that quads are clipped to, set only
    /// for DECDHL rows (every other layout already fits inside the cell).
    clip: Option<[f32; 2]>,
}

impl RowLayout {
    fn new(line_width: LineWidth, y_top: f32, cell_height: f32) -> Self {
        let clip = Some([y_top, y_top + cell_height]);
        match line_width {
            LineWidth::Normal | LineWidth::DoubleWidth => Self {
                box_top: y_top,
                y_scale: 1.0,
                clip: None,
            },
            LineWidth::DoubleHeightTop => Self {
                box_top: y_top,
                y_scale: 2.0,
                clip,
            },
            LineWidth::DoubleHeightBottom => Self {
                box_top: y_top - cell_height,
                y_scale: 2.0,
                clip,
            },
        }
    }
}

/// Push the underline and strikethrough quads for one run of the row whose
/// top edge is `y_top` and whose line width is `line_width`.
///
/// The caller has checked that the run has an active underline or a
/// strikethrough.  Kept out of line so the common no-decoration path through
/// [`super::vertex::build_background_instances`] stays a tight loop.
#[inline(never)]
pub(super) fn push_run_decorations(
    deco: &mut Vec<f32>,
    frame: &BackgroundFrame<'_>,
    run: &ShapedRun,
    y_top: f32,
    line_width: LineWidth,
) {
    let cell_width = frame.cell_width;
    let cell_height = gl_f32_u32(frame.cell_height);
    let colors = frame.colors;
    let screen_inversion = frame.screen_inversion;
    let is_faint = run.font_decorations.contains(FontDecorations::Faint);
    let underline_style = run.font_decorations.underline_style();
    let has_strike = run
        .font_decorations
        .contains(FontDecorations::Strikethrough);

    let scale = x_scale(line_width);
    let layout = RowLayout::new(line_width, y_top, cell_height);
    let box_height = cell_height * layout.y_scale;
    let ascent = frame.ascent * layout.y_scale;
    let stroke = frame.stroke_size * layout.y_scale;

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
            internal_color_to_gl(
                effective_fg(&run.colors, screen_inversion),
                is_faint,
                colors,
            )
        } else {
            internal_color_to_gl(ul_color_raw, is_faint, colors)
        };

        // underline_offset from swash is negative (below baseline in font
        // coords).  In top-down pixel coords the baseline is at
        // box_top + ascent, so subtracting the (negative) offset places the
        // line below the baseline.  The raw position is clamped to the
        // cell box by `push_underline_quads`.
        let start = deco.len();
        push_underline_quads(
            deco,
            underline_style,
            &UnderlineParams {
                x0,
                x1,
                top: layout.box_top,
                cell_height: box_height,
                cell_width: gl_f32_u32(cell_width) * scale,
                raw_y: frame
                    .underline_offset
                    .mul_add(-layout.y_scale, layout.box_top + ascent),
                stroke,
                color: ul_color,
            },
        );
        if let Some([row_top, row_bottom]) = layout.clip {
            clip_quads_to_rows(deco, start, row_top, row_bottom);
        }
    }

    if has_strike && box_height > 0.0 {
        // Task 115.2: strikethrough uses the effective (DECSCNM ×
        // SGR-7 composed) foreground so it stays visible under
        // whole-screen reverse video.
        let fg_color = internal_color_to_gl(
            effective_fg(&run.colors, screen_inversion),
            is_faint,
            colors,
        );
        // strikeout_offset from OS/2 is positive (above baseline in font
        // coords).  In top-down pixel coords, subtracting it from the
        // baseline places the line above the baseline (middle of cell).
        let (st_top, thick) = clamp_to_cell(
            layout.box_top,
            box_height,
            frame
                .strikeout_offset
                .mul_add(-layout.y_scale, layout.box_top + ascent),
            stroke,
        );
        let start = deco.len();
        push_quad(deco, x0, st_top, x1, st_top + thick, fg_color);
        if let Some([row_top, row_bottom]) = layout.clip {
            clip_quads_to_rows(deco, start, row_top, row_bottom);
        }
    }
}

/// Clip the quads appended to `deco` from index `start` onwards to the
/// vertical range `[row_top, row_bottom)`, dropping any that end up empty.
fn clip_quads_to_rows(deco: &mut Vec<f32>, start: usize, row_top: f32, row_bottom: f32) {
    let mut read = start;
    let mut write = start;
    while let Some(Ok(quad)) = deco
        .get(read..read + QUAD_FLOATS)
        .map(<[f32; QUAD_FLOATS]>::try_from)
    {
        read += QUAD_FLOATS;
        let QuadBounds {
            x0,
            y0,
            x1,
            y1,
            color,
        } = quad_bounds(&quad);
        let y0 = y0.max(row_top);
        let y1 = y1.min(row_bottom);
        if y1 <= y0 {
            continue;
        }
        if let Some(dst) = deco.get_mut(write..write + QUAD_FLOATS) {
            dst.copy_from_slice(&quad_vertices(x0, y0, x1, y1, color));
        }
        write += QUAD_FLOATS;
    }
    deco.truncate(write);
}

// ---------------------------------------------------------------------------
//  Underline style geometry
// ---------------------------------------------------------------------------

/// Number of vertical-strip quads used to approximate one curly-underline
/// period (one period spans exactly one cell).
const CURLY_SEGMENTS: usize = 8;

/// Inputs for decoration geometry, bundled to stay within the 7-argument
/// lint limit.
///
/// Every decoration is laid out inside the cell box
/// `[top, top + cell_height) x [x0, x1)`; patterns are anchored to the
/// absolute cell grid (`x0` is a whole number of drawn cells from the row
/// origin), so the output does not depend on where text runs split.
struct UnderlineParams {
    x0: f32,
    x1: f32,
    /// Top edge of the row.
    top: f32,
    cell_height: f32,
    /// Drawn cell width: the font cell width times the row's horizontal scale
    /// (2 on DECDWL/DECDHL rows).
    cell_width: f32,
    /// Unclamped top edge of the font's underline position.
    raw_y: f32,
    /// Font stroke thickness before clamping.
    stroke: f32,
    color: [f32; 4],
}

/// Clamp a horizontal line to the cell box.
///
/// Returns `(y, thickness)`: the thickness is `stroke.max(1)` limited to the
/// cell height, and `y` (the line's top edge) is `raw_y` limited so the whole
/// line lies in `[top, top + cell_height)`.  `cell_height` must be positive.
fn clamp_to_cell(top: f32, cell_height: f32, raw_y: f32, stroke: f32) -> (f32, f32) {
    let thick = stroke.max(1.0).min(cell_height);
    let y = raw_y.max(top).min(top + cell_height - thick);
    (y, thick)
}

/// Number of whole cells a run covers.
fn run_cell_count(p: &UnderlineParams) -> usize {
    ((p.x1 - p.x0) / p.cell_width)
        .round()
        .approx_as::<usize>()
        .unwrap_or(0)
}

/// Push decoration quads for the given underline style.
///
/// All styles are clamped to the cell box (see [`clamp_to_cell`]).  Single and
/// double lines are one quad per line across the run; curly, dotted and
/// dashed are laid out per cell.
fn push_underline_quads(deco: &mut Vec<f32>, style: UnderlineStyle, p: &UnderlineParams) {
    if !style.is_active() || p.cell_height <= 0.0 || p.cell_width <= 0.0 || p.x1 <= p.x0 {
        return;
    }
    let (y, thick) = clamp_to_cell(p.top, p.cell_height, p.raw_y, p.stroke);
    match style {
        UnderlineStyle::None => {}
        UnderlineStyle::Single => {
            push_quad(deco, p.x0, y, p.x1, y + thick, p.color);
        }
        UnderlineStyle::Double => push_double_underline(deco, p, y, thick),
        UnderlineStyle::Curly => push_curly_underline(deco, p, y, thick),
        UnderlineStyle::Dotted => push_dotted_underline(deco, p, y, thick),
        UnderlineStyle::Dashed => push_dashed_underline(deco, p, y, thick),
    }
}

/// Push two lines of height `thick` separated by a gap of `thick + 1`.
///
/// If the lower line would pass the bottom of the cell it moves up to sit on
/// the bottom edge and the upper line follows it, never going above the top.
fn push_double_underline(deco: &mut Vec<f32>, p: &UnderlineParams, y: f32, thick: f32) {
    let bot = p.top + p.cell_height;
    let mut upper = y;
    let mut lower = y + thick.mul_add(2.0, 1.0);
    if lower + thick > bot {
        lower = bot - thick;
        upper = lower - (thick + 1.0) - thick;
    }
    let upper = upper.max(p.top);
    push_quad(deco, p.x0, upper, p.x1, upper + thick, p.color);
    push_quad(deco, p.x0, lower, p.x1, lower + thick, p.color);
}

/// Push quads approximating a curly underline.
///
/// One period of `y = centre - amp * cos(2π (x - cx) / cell_width)` spans each
/// cell, so every cell starts and ends at the same height and adjacent cells
/// join.  The amplitude is `2 * thick`, reduced so the stroke stays inside the
/// cell, and `centre` is the clamped underline position moved only as far as
/// needed to fit.  Each cell is [`CURLY_SEGMENTS`] vertical strips.
fn push_curly_underline(deco: &mut Vec<f32>, p: &UnderlineParams, y: f32, thick: f32) {
    let bot = p.top + p.cell_height;
    let amp = (2.0 * thick).min((p.cell_height - thick) / 2.0).max(0.0);
    let centre = y.max(p.top + amp).min(bot - thick - amp);

    let segs = gl_f32(CURLY_SEGMENTS);
    // The last vertex wraps to the first, so adjacent cells join exactly.
    let edge_y: [f32; CURLY_SEGMENTS + 1] = std::array::from_fn(|k| {
        let phase = gl_f32(k % CURLY_SEGMENTS) / segs * std::f32::consts::TAU;
        amp.mul_add(-phase.cos(), centre)
    });
    let bands: [(f32, f32); CURLY_SEGMENTS] = std::array::from_fn(|k| {
        let (a, b) = (edge_y[k], edge_y[k + 1]);
        (a.min(b).max(p.top), (a.max(b) + thick).min(bot))
    });

    for cell in 0..run_cell_count(p) {
        let cx = p.cell_width.mul_add(gl_f32(cell), p.x0);
        for (k, &(y_min, y_max)) in bands.iter().enumerate() {
            let sx = p.cell_width.mul_add(gl_f32(k) / segs, cx);
            let ex = p.cell_width.mul_add(gl_f32(k + 1) / segs, cx);
            push_quad(deco, sx, y_min, ex, y_max, p.color);
        }
    }
}

/// Push quads for a dotted underline.
///
/// Per cell there are `n = max(1, floor(cw / 2t))` dots in slots of `s = cw / n`;
/// each dot is `d = max(1, min(t, floor(s / 2)))` wide and centred in its slot
/// (rounded to a whole pixel).
fn push_dotted_underline(deco: &mut Vec<f32>, p: &UnderlineParams, y: f32, thick: f32) {
    let cw = p.cell_width;
    let n = (cw / (2.0 * thick))
        .floor()
        .approx_as::<usize>()
        .unwrap_or(1)
        .max(1);
    let n_f = gl_f32(n);
    let slot = cw / n_f;
    let dot = (slot / 2.0).floor().min(thick).max(1.0);

    for cell in 0..run_cell_count(p) {
        let cx = cw.mul_add(gl_f32(cell), p.x0);
        let cell_end = cx + cw;
        for i in 0..n {
            let offset = ((gl_f32(i) + 0.5).mul_add(slot, -dot / 2.0)).round();
            let x = cx + offset;
            push_quad(deco, x, y, (x + dot).min(cell_end), y + thick, p.color);
        }
    }
}

/// Push quads for a dashed underline.
///
/// Per cell, with `q = floor(cw / 4)` and `w = cw - 3q`, dashes cover
/// `[cx, cx + w)` and `[cx + 3q, cx + cw)`: the right dash of one cell and the
/// left dash of the next join into one longer dash.  When the two dashes would
/// touch or overlap (a cell under four pixels wide, or `w >= 3q`) the cell is
/// one solid dash instead.
fn push_dashed_underline(deco: &mut Vec<f32>, p: &UnderlineParams, y: f32, thick: f32) {
    let cw = p.cell_width;
    let q = (cw / 4.0).floor();
    let w = 3.0f32.mul_add(-q, cw);

    for cell in 0..run_cell_count(p) {
        let cx = cw.mul_add(gl_f32(cell), p.x0);
        if q < 1.0 || w >= 3.0 * q {
            push_quad(deco, cx, y, cx + cw, y + thick, p.color);
            continue;
        }
        push_quad(deco, cx, y, cx + w, y + thick, p.color);
        push_quad(deco, 3.0f32.mul_add(q, cx), y, cx + cw, y + thick, p.color);
    }
}

// ---------------------------------------------------------------------------
//  Underline / strikethrough geometry tests (Task 140.3)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod decoration_geometry {
    use std::sync::Arc;

    use freminal_common::buffer_states::cursor::StateColors;
    use freminal_common::buffer_states::fonts::{BlinkState, FontDecorationFlags, FontWeight};
    use freminal_common::buffer_states::modes::decscnm::ScreenInversion;
    use freminal_common::colors::{
        ColorPalette, DynamicColorOverrides, ResolvedColors, TerminalColor,
    };
    use freminal_common::themes;
    use freminal_terminal_emulator::LineWidth;

    use super::super::super::{
        colors::cursor_f,
        shaping::{ShapedGlyph, ShapedLine},
        terminal::cursor_appearance::CursorAppearance,
    };
    use super::super::vertex::{
        BackgroundFrame, CursorBlinkPhase, CursorDrawParams, build_background_instances,
    };
    use super::*;

    fn mocha_colors() -> ResolvedColors {
        ResolvedColors::resolve(
            &themes::CATPPUCCIN_MOCHA,
            &ColorPalette::default(),
            &DynamicColorOverrides::default(),
        )
    }

    fn default_colors() -> StateColors {
        StateColors::default()
    }

    fn hidden_cursor() -> CursorDrawParams {
        CursorDrawParams {
            appearance: CursorAppearance::Hidden,
            col: 0.0,
            row: 0.0,
            color: cursor_f(&themes::CATPPUCCIN_MOCHA, None),
            x_scale: 1.0,
            blink_on: CursorBlinkPhase::Off,
        }
    }

    const EPS: f32 = 1e-3;

    const PATTERNED: [UnderlineStyle; 3] = [
        UnderlineStyle::Curly,
        UnderlineStyle::Dotted,
        UnderlineStyle::Dashed,
    ];

    const ALL_STYLES: [UnderlineStyle; 5] = [
        UnderlineStyle::Single,
        UnderlineStyle::Double,
        UnderlineStyle::Curly,
        UnderlineStyle::Dotted,
        UnderlineStyle::Dashed,
    ];

    /// One decoded decoration quad.
    #[derive(Clone, Copy, Debug)]
    struct Quad {
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
    }

    /// Font metrics fed to `build_background_instances`.
    #[derive(Clone, Copy)]
    struct Metrics {
        cell_width: u32,
        cell_height: u32,
        ascent: f32,
        underline_offset: f32,
        strikeout_offset: f32,
        stroke_size: f32,
    }

    const NORMAL: Metrics = Metrics {
        cell_width: 8,
        cell_height: 16,
        ascent: 14.0,
        underline_offset: 13.0,
        strikeout_offset: 8.0,
        stroke_size: 1.0,
    };

    fn decode_quads(deco: &[f32]) -> Vec<Quad> {
        deco.as_chunks::<QUAD_FLOATS>()
            .0
            .iter()
            .map(|q| {
                let b = quad_bounds(q);
                Quad {
                    x0: b.x0,
                    y0: b.y0,
                    x1: b.x1,
                    y1: b.y1,
                }
            })
            .collect()
    }

    fn decorations(style: UnderlineStyle) -> FontDecorationFlags {
        let mut flags = FontDecorationFlags::empty();
        flags.set_underline_style(style);
        flags
    }

    /// A line of `runs.len()` runs; run `i` has `runs[i].0` cells drawn
    /// with `runs[i].1` as its colours.
    fn make_run_line(
        runs: &[(usize, StateColors)],
        flags: FontDecorationFlags,
        lw: LineWidth,
    ) -> Arc<ShapedLine> {
        use crate::gui::font_manager::FaceId;
        let mut col_start = 0;
        let mut out = Vec::new();
        for &(cells, colors) in runs {
            let glyphs = (0..cells)
                .map(|i| ShapedGlyph {
                    glyph_id: 36,
                    x_px: gl_f32(i) * 8.0,
                    y_offset: 0.0,
                    face_id: FaceId::PrimaryRegular,
                    is_color: false,
                    cell_width: 1,
                    source_char: '\0',
                })
                .collect();
            out.push(ShapedRun {
                glyphs,
                col_start,
                style: crate::gui::font_manager::GlyphStyle::new(false, false),
                font_weight: FontWeight::Normal,
                font_decorations: flags,
                colors,
                url: None,
                blink: BlinkState::None,
            });
            col_start += cells;
        }
        Arc::new(ShapedLine {
            runs: out,
            line_width: lw,
        })
    }

    fn deco_for(lines: &[Arc<ShapedLine>], m: Metrics) -> Vec<Quad> {
        let mut instances = Vec::new();
        let mut deco = Vec::new();
        let _cursor_range = build_background_instances(
            &BackgroundFrame {
                shaped_lines: lines,
                cell_width: m.cell_width,
                cell_height: m.cell_height,
                ascent: m.ascent,
                underline_offset: m.underline_offset,
                strikeout_offset: m.strikeout_offset,
                stroke_size: m.stroke_size,
                cursor: &hidden_cursor(),
                selection: None,
                selection_is_block: false,
                match_highlights: &[],
                command_block_hover_rows: None,
                term_width_cols: 0,
                colors: &mocha_colors(),
                screen_inversion: ScreenInversion::Normal,
            },
            &mut instances,
            &mut deco,
        );
        decode_quads(&deco)
    }

    /// Quads of a single decorated line placed on row 1 (so `top` is
    /// non-zero) below an undecorated row 0.
    fn row_one_quads(line: Arc<ShapedLine>, m: Metrics) -> Vec<Quad> {
        let plain = make_run_line(
            &[(1, default_colors())],
            FontDecorationFlags::empty(),
            LineWidth::Normal,
        );
        deco_for(&[plain, line], m)
    }

    fn underline_quads(style: UnderlineStyle, cells: usize, m: Metrics) -> Vec<Quad> {
        let line = make_run_line(
            &[(cells, default_colors())],
            decorations(style),
            LineWidth::Normal,
        );
        row_one_quads(line, m)
    }

    fn assert_inside(quads: &[Quad], x0: f32, x1: f32, top: f32, bot: f32, what: &str) {
        assert!(!quads.is_empty(), "{what}: no quads");
        for q in quads {
            assert!(
                q.x0 >= x0 - EPS && q.x1 <= x1 + EPS && q.y0 >= top - EPS && q.y1 <= bot + EPS,
                "{what}: quad {q:?} outside [{x0},{x1}) x [{top},{bot})"
            );
            assert!(
                q.x1 > q.x0 && q.y1 > q.y0,
                "{what}: quad {q:?} is degenerate"
            );
        }
    }

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < EPS
    }

    #[test]
    fn every_style_stays_inside_the_cell_box() {
        let cases: [(&str, Metrics); 7] = [
            ("normal", NORMAL),
            (
                "offset just below the cell",
                Metrics {
                    underline_offset: -2.0,
                    ..NORMAL
                },
            ),
            (
                "offset far below the cell",
                Metrics {
                    underline_offset: -40.0,
                    ..NORMAL
                },
            ),
            (
                "offset above the cell",
                Metrics {
                    underline_offset: 60.0,
                    ..NORMAL
                },
            ),
            (
                "stroke thicker than the cell",
                Metrics {
                    stroke_size: 40.0,
                    ..NORMAL
                },
            ),
            (
                "stroke a third of the cell",
                Metrics {
                    stroke_size: 6.0,
                    underline_offset: -3.0,
                    ..NORMAL
                },
            ),
            (
                "small odd cell",
                Metrics {
                    cell_width: 7,
                    cell_height: 12,
                    ascent: 10.0,
                    underline_offset: -1.0,
                    ..NORMAL
                },
            ),
        ];
        for (label, m) in cases {
            let ch = gl_f32_u32(m.cell_height);
            let x1 = 6.0 * gl_f32_u32(m.cell_width);
            for style in ALL_STYLES {
                let quads = underline_quads(style, 6, m);
                assert_inside(
                    &quads,
                    0.0,
                    x1,
                    ch,
                    2.0 * ch,
                    &format!("{style:?} / {label}"),
                );
            }
        }
    }

    #[test]
    fn strikethrough_stays_inside_the_cell_box() {
        let mut flags = FontDecorationFlags::empty();
        flags.insert(FontDecorations::Strikethrough);
        let cases = [
            NORMAL,
            Metrics {
                strikeout_offset: -40.0,
                ..NORMAL
            },
            Metrics {
                strikeout_offset: 90.0,
                ..NORMAL
            },
            Metrics {
                stroke_size: 40.0,
                ..NORMAL
            },
        ];
        for m in cases {
            let line = make_run_line(&[(4, default_colors())], flags, LineWidth::Normal);
            let quads = row_one_quads(line, m);
            assert_eq!(quads.len(), 1);
            let ch = gl_f32_u32(m.cell_height);
            assert_inside(&quads, 0.0, 32.0, ch, 2.0 * ch, "strikethrough");
        }
    }

    #[test]
    fn single_underline_is_one_clamped_quad() {
        let quads = underline_quads(UnderlineStyle::Single, 6, NORMAL);
        assert_eq!(quads.len(), 1);
        let q = quads[0];
        // top (16) + ascent (14) - offset (13) = 17, thickness 1.
        assert!(approx(q.x0, 0.0) && approx(q.x1, 48.0), "{q:?}");
        assert!(approx(q.y0, 17.0) && approx(q.y1, 18.0), "{q:?}");

        let thick = underline_quads(
            UnderlineStyle::Single,
            2,
            Metrics {
                stroke_size: 40.0,
                ..NORMAL
            },
        );
        assert!(approx(thick[0].y0, 16.0) && approx(thick[0].y1, 32.0));
    }

    /// Quad positions only, in emission order.
    fn positions(quads: &[Quad]) -> Vec<[f32; 4]> {
        quads.iter().map(|q| [q.x0, q.y0, q.x1, q.y1]).collect()
    }

    /// Metrics whose old dotted pitch (`2 * stroke` = 6) does not divide the
    /// 8 px cell, so a pattern anchored at the run start (the pre-140.3
    /// behaviour) drifts relative to the cell grid when a run splits.
    const STROKE_3: Metrics = Metrics {
        underline_offset: -1.0,
        stroke_size: 3.0,
        ..NORMAL
    };

    #[test]
    fn patterned_styles_ignore_run_boundaries() {
        let red = default_colors().with_color(TerminalColor::Red);
        let green = default_colors().with_color(TerminalColor::Green);
        for (label, m) in [("normal", NORMAL), ("stroke 3", STROKE_3)] {
            for style in PATTERNED {
                let whole = row_one_quads(
                    make_run_line(&[(6, red)], decorations(style), LineWidth::Normal),
                    m,
                );
                let split = row_one_quads(
                    make_run_line(
                        &[(2, red), (4, green)],
                        decorations(style),
                        LineWidth::Normal,
                    ),
                    m,
                );
                assert_eq!(
                    positions(&whole),
                    positions(&split),
                    "{style:?} / {label}: run boundaries changed the pattern"
                );
            }
        }
    }

    #[test]
    fn solid_styles_cover_the_same_area_across_run_boundaries() {
        let red = default_colors().with_color(TerminalColor::Red);
        let green = default_colors().with_color(TerminalColor::Green);
        for style in [UnderlineStyle::Single, UnderlineStyle::Double] {
            let whole = row_one_quads(
                make_run_line(&[(6, red)], decorations(style), LineWidth::Normal),
                NORMAL,
            );
            let split = row_one_quads(
                make_run_line(
                    &[(2, red), (4, green)],
                    decorations(style),
                    LineWidth::Normal,
                ),
                NORMAL,
            );
            let lines_per_run = whole.len();
            assert_eq!(split.len(), 2 * lines_per_run, "{style:?}");
            for (i, w) in whole.iter().enumerate() {
                let first = split[i];
                let second = split[lines_per_run + i];
                assert!(
                    approx(first.y0, w.y0) && approx(first.y1, w.y1),
                    "{style:?}"
                );
                assert!(
                    approx(second.y0, w.y0) && approx(second.y1, w.y1),
                    "{style:?}"
                );
                assert!(approx(first.x0, w.x0) && approx(first.x1, second.x0));
                assert!(approx(second.x1, w.x1), "{style:?}");
            }
        }
    }

    #[test]
    fn double_width_row_scales_the_pattern_with_the_drawn_cell() {
        let normal = |style| {
            row_one_quads(
                make_run_line(
                    &[(3, default_colors())],
                    decorations(style),
                    LineWidth::Normal,
                ),
                NORMAL,
            )
        };
        let wide = |style| {
            row_one_quads(
                make_run_line(
                    &[(3, default_colors())],
                    decorations(style),
                    LineWidth::DoubleWidth,
                ),
                NORMAL,
            )
        };

        // Dashed: the drawn cell is 16 px, so q = 4 and the dashes are
        // [0, 4) and [12, 16) in every cell.
        let dashes = wide(UnderlineStyle::Dashed);
        assert_eq!(dashes.len(), 6);
        for (cell, pair) in dashes.as_chunks::<2>().0.iter().enumerate() {
            let cx = 16.0 * gl_f32(cell);
            assert!(
                approx(pair[0].x0, cx) && approx(pair[0].x1, cx + 4.0),
                "{pair:?}"
            );
            assert!(
                approx(pair[1].x0, cx + 12.0) && approx(pair[1].x1, cx + 16.0),
                "{pair:?}"
            );
        }

        // Dotted: twice the dots per cell on a double-width row.
        assert_eq!(normal(UnderlineStyle::Dotted).len(), 3 * 4);
        assert_eq!(wide(UnderlineStyle::Dotted).len(), 3 * 8);

        // Curly: strip width is cw / CURLY_SEGMENTS, and the wave spans
        // the whole drawn width.
        let curly = wide(UnderlineStyle::Curly);
        assert_eq!(curly.len(), 3 * CURLY_SEGMENTS);
        assert!(approx(curly[0].x1 - curly[0].x0, 2.0));
        assert!(approx(curly[curly.len() - 1].x1, 48.0));
        let curly_normal = normal(UnderlineStyle::Curly);
        assert!(approx(curly_normal[0].x1 - curly_normal[0].x0, 1.0));
    }

    #[test]
    fn double_lines_do_not_overlap_when_they_fit() {
        let quads = underline_quads(UnderlineStyle::Double, 3, NORMAL);
        assert_eq!(quads.len(), 2);
        let (upper, lower) = (quads[0], quads[1]);
        assert!(upper.y1 <= lower.y0, "{upper:?} overlaps {lower:?}");
        // Thickness 1, so the gap is thickness + 1.
        assert!(approx(lower.y0 - upper.y1, 2.0), "{upper:?} {lower:?}");
        assert!(approx(upper.x0, 0.0) && approx(upper.x1, 24.0));
        assert!(approx(lower.x0, 0.0) && approx(lower.x1, 24.0));
    }

    #[test]
    fn double_lower_line_moves_up_to_sit_on_the_cell_bottom() {
        // Raw position 24 (offset -10): the lower line cannot fit below it.
        let m = Metrics {
            underline_offset: -10.0,
            ..NORMAL
        };
        let quads = underline_quads(UnderlineStyle::Double, 3, m);
        let (upper, lower) = (quads[0], quads[1]);
        assert!(
            approx(lower.y1, 32.0),
            "lower line ends at the bottom: {lower:?}"
        );
        assert!(approx(lower.y0 - upper.y1, 2.0), "{upper:?} {lower:?}");
        assert!(upper.y1 <= lower.y0);
    }

    #[test]
    fn curly_is_continuous_across_cell_boundaries() {
        let quads = underline_quads(UnderlineStyle::Curly, 4, NORMAL);
        assert_eq!(quads.len(), 4 * CURLY_SEGMENTS);
        for cell in 0..3 {
            let last = quads[(cell + 1) * CURLY_SEGMENTS - 1];
            let next = quads[(cell + 1) * CURLY_SEGMENTS];
            assert!(
                approx(last.x1, next.x0),
                "gap between cells {cell} and {}",
                cell + 1
            );
            assert!(
                approx(last.y0, next.y0) && approx(last.y1, next.y1),
                "cell {cell} ends at {last:?} but the next starts at {next:?}"
            );
        }
        // Every cell is the same wave, and the wave moves.
        for cell in 1..4 {
            for k in 0..CURLY_SEGMENTS {
                let (a, b) = (quads[k], quads[cell * CURLY_SEGMENTS + k]);
                assert!(approx(a.y0, b.y0) && approx(a.y1, b.y1));
            }
        }
        let lowest = quads.iter().map(|q| q.y0).fold(f32::MAX, f32::min);
        let highest = quads.iter().map(|q| q.y0).fold(f32::MIN, f32::max);
        assert!(highest - lowest > 1.0, "wave is flat: {lowest}..{highest}");
    }

    #[test]
    fn dashed_dashes_join_across_cell_boundaries() {
        let quads = underline_quads(UnderlineStyle::Dashed, 3, NORMAL);
        assert_eq!(quads.len(), 6);
        for (cell, pair) in quads.as_chunks::<2>().0.iter().enumerate() {
            let left = 8.0 * gl_f32(cell);
            assert!(
                approx(pair[0].x0, left),
                "first dash starts at the cell's left edge"
            );
            assert!(
                approx(pair[1].x1, left + 8.0),
                "last dash ends at the cell's right edge"
            );
        }
        for cell in 0..2 {
            assert!(approx(quads[cell * 2 + 1].x1, quads[(cell + 1) * 2].x0));
        }
    }

    #[test]
    fn dotted_dots_are_evenly_spaced_per_cell() {
        let quads = underline_quads(UnderlineStyle::Dotted, 2, NORMAL);
        // cw = 8, t = 1: four 1px dots in 2px slots, at offsets 1, 3, 5, 7.
        assert_eq!(quads.len(), 8);
        for (i, q) in quads.iter().enumerate() {
            let expected = 2.0f32.mul_add(gl_f32(i % 4), 8.0f32.mul_add(gl_f32(i / 4), 1.0));
            assert!(
                approx(q.x0, expected) && approx(q.x1, expected + 1.0),
                "{q:?}"
            );
        }
    }

    /// A line with a single run whose glyphs have the given cell widths.
    fn make_wide_line(
        widths: &[usize],
        flags: FontDecorationFlags,
        lw: LineWidth,
    ) -> Arc<ShapedLine> {
        use crate::gui::font_manager::FaceId;
        let mut col = 0;
        let glyphs = widths
            .iter()
            .map(|&cell_width| {
                let glyph = ShapedGlyph {
                    glyph_id: 36,
                    x_px: gl_f32(col) * 8.0,
                    y_offset: 0.0,
                    face_id: FaceId::PrimaryRegular,
                    is_color: false,
                    cell_width,
                    source_char: '\0',
                };
                col += cell_width;
                glyph
            })
            .collect();
        Arc::new(ShapedLine {
            runs: vec![ShapedRun {
                glyphs,
                col_start: 0,
                style: crate::gui::font_manager::GlyphStyle::new(false, false),
                font_weight: FontWeight::Normal,
                font_decorations: flags,
                colors: default_colors(),
                url: None,
                blink: BlinkState::None,
            }],
            line_width: lw,
        })
    }

    fn strikethrough() -> FontDecorationFlags {
        let mut flags = FontDecorationFlags::empty();
        flags.insert(FontDecorations::Strikethrough);
        flags
    }

    #[test]
    fn zero_cell_height_emits_no_decoration_quads() {
        let m = Metrics {
            cell_height: 0,
            ..NORMAL
        };
        for lw in [
            LineWidth::Normal,
            LineWidth::DoubleWidth,
            LineWidth::DoubleHeightTop,
            LineWidth::DoubleHeightBottom,
        ] {
            for style in ALL_STYLES {
                let line = make_run_line(&[(4, default_colors())], decorations(style), lw);
                assert!(
                    row_one_quads(line, m).is_empty(),
                    "{style:?} on {lw:?} drew with a zero-height cell"
                );
            }
            let line = make_run_line(&[(4, default_colors())], strikethrough(), lw);
            assert!(
                row_one_quads(line, m).is_empty(),
                "strikethrough on {lw:?} drew with a zero-height cell"
            );
        }
    }

    /// Realistic metrics for a 16 px cell: the underline sits two pixels
    /// below the baseline (row y 14) and the strikethrough above the middle.
    const REALISTIC: Metrics = Metrics {
        cell_width: 8,
        cell_height: 16,
        ascent: 12.0,
        underline_offset: -2.0,
        strikeout_offset: 5.0,
        stroke_size: 1.0,
    };

    /// The same font with every vertical metric doubled, as the glyphs are on
    /// a DECDHL pair.
    const REALISTIC_DOUBLED: Metrics = Metrics {
        cell_width: 8,
        cell_height: 32,
        ascent: 24.0,
        underline_offset: -4.0,
        strikeout_offset: 10.0,
        stroke_size: 2.0,
    };

    /// The doubled-box layout clipped to `[lo, hi)`, as expected positions.
    fn clipped(quads: &[Quad], lo: f32, hi: f32) -> Vec<[f32; 4]> {
        quads
            .iter()
            .filter_map(|q| {
                let (y0, y1) = (q.y0.max(lo), q.y1.min(hi));
                (y1 > y0).then_some([q.x0, y0, q.x1, y1])
            })
            .collect()
    }

    fn assert_positions_eq(actual: &[Quad], expected: &[[f32; 4]], what: &str) {
        assert_eq!(actual.len(), expected.len(), "{what}: quad count");
        for (a, e) in actual.iter().zip(expected) {
            assert!(
                approx(a.x0, e[0])
                    && approx(a.y0, e[1])
                    && approx(a.x1, e[2])
                    && approx(a.y1, e[3]),
                "{what}: {a:?} != {e:?}"
            );
        }
    }

    /// Decorations of a DECDHL pair on rows 0 (top half) and 1 (bottom half),
    /// returned as `(top_half, bottom_half)`.
    fn dhl_halves(flags: FontDecorationFlags, m: Metrics) -> (Vec<Quad>, Vec<Quad>) {
        let half = |lw| make_run_line(&[(5, default_colors())], flags, lw);
        let quads = deco_for(
            &[
                half(LineWidth::DoubleHeightTop),
                half(LineWidth::DoubleHeightBottom),
            ],
            m,
        );
        let ch = gl_f32_u32(m.cell_height);
        let (top, bottom): (Vec<Quad>, Vec<Quad>) = quads.into_iter().partition(|q| q.y0 < ch);
        (top, bottom)
    }

    /// The doubled-box reference for `flags`: a double-width row (same
    /// horizontal scale as DECDHL) laid out with doubled vertical metrics.
    fn doubled_box_reference(flags: FontDecorationFlags) -> Vec<Quad> {
        deco_for(
            &[make_run_line(
                &[(5, default_colors())],
                flags,
                LineWidth::DoubleWidth,
            )],
            REALISTIC_DOUBLED,
        )
    }

    #[test]
    fn double_height_halves_are_the_doubled_box_clipped_to_each_half() {
        let ch = 16.0;
        let mut cases: Vec<(String, FontDecorationFlags)> = ALL_STYLES
            .iter()
            .map(|&style| (format!("{style:?}"), decorations(style)))
            .collect();
        cases.push(("strikethrough".to_owned(), strikethrough()));

        for (label, flags) in cases {
            let reference = doubled_box_reference(flags);
            assert!(!reference.is_empty(), "{label}: empty reference");
            let (top, bottom) = dhl_halves(flags, REALISTIC);

            assert_positions_eq(&top, &clipped(&reference, 0.0, ch), &format!("{label} top"));
            assert_positions_eq(
                &bottom,
                &clipped(&reference, ch, 2.0 * ch),
                &format!("{label} bottom"),
            );
            // Nothing leaves its own half, and nothing is degenerate.
            for q in &top {
                assert!(
                    q.y0 >= -EPS && q.y1 <= ch + EPS && q.y1 > q.y0,
                    "{label}: {q:?}"
                );
            }
            for q in &bottom {
                assert!(
                    q.y0 >= ch - EPS && q.y1 <= 2.0f32.mul_add(ch, EPS) && q.y1 > q.y0,
                    "{label}: {q:?}"
                );
            }
        }
    }

    #[test]
    fn double_height_underline_lands_on_the_half_where_the_glyph_underline_falls() {
        // Doubled: baseline at 24, underline 4 px below it, 2 px thick: y 28..30
        // of the pair, which is the bottom half's row y 12..14.
        let (top, bottom) = dhl_halves(decorations(UnderlineStyle::Single), REALISTIC);
        assert!(top.is_empty(), "underline drew on the top half: {top:?}");
        assert_eq!(bottom.len(), 1);
        let q = bottom[0];
        assert!(
            approx(q.y0, 16.0 + 12.0) && approx(q.y1, 16.0 + 14.0),
            "{q:?}"
        );
        assert!(approx(q.x0, 0.0) && approx(q.x1, 5.0 * 16.0), "{q:?}");

        // Doubled strikethrough: y 14..16, entirely in the top half.
        let (top, bottom) = dhl_halves(strikethrough(), REALISTIC);
        assert!(
            bottom.is_empty(),
            "strikethrough drew on the bottom half: {bottom:?}"
        );
        assert_eq!(top.len(), 1);
        assert!(
            approx(top[0].y0, 14.0) && approx(top[0].y1, 16.0),
            "{:?}",
            top[0]
        );
    }

    #[test]
    fn double_height_quad_on_the_boundary_is_split_across_the_halves() {
        // Raw underline y = 24 - 2 * 4.5 = 15, doubled stroke 2: [15, 17).
        let m = Metrics {
            underline_offset: 4.5,
            ..REALISTIC
        };
        let (top, bottom) = dhl_halves(decorations(UnderlineStyle::Single), m);
        assert_eq!(top.len(), 1, "{top:?}");
        assert_eq!(bottom.len(), 1, "{bottom:?}");
        assert!(
            approx(top[0].y0, 15.0) && approx(top[0].y1, 16.0),
            "{:?}",
            top[0]
        );
        assert!(
            approx(bottom[0].y0, 16.0) && approx(bottom[0].y1, 17.0),
            "{:?}",
            bottom[0]
        );
        assert!(approx(top[0].x0, bottom[0].x0) && approx(top[0].x1, bottom[0].x1));
    }

    #[test]
    fn double_height_patterns_are_identical_between_the_halves_except_for_the_clip() {
        // Curly straddles the boundary, so each half carries only the strips
        // of the wave that reach it; together they are the whole wave.
        let m = Metrics {
            underline_offset: 4.5,
            ..REALISTIC
        };
        let flags = decorations(UnderlineStyle::Curly);
        let reference = deco_for(
            &[make_run_line(
                &[(5, default_colors())],
                flags,
                LineWidth::DoubleWidth,
            )],
            Metrics {
                underline_offset: 9.0,
                ..REALISTIC_DOUBLED
            },
        );
        let (top, bottom) = dhl_halves(flags, m);
        assert!(!top.is_empty() && !bottom.is_empty());
        assert_eq!(reference.len(), 5 * CURLY_SEGMENTS);
        for strip in &reference {
            let in_top = top
                .iter()
                .any(|q| approx(q.x0, strip.x0) && approx(q.x1, strip.x1));
            let in_bottom = bottom
                .iter()
                .any(|q| approx(q.x0, strip.x0) && approx(q.x1, strip.x1));
            assert!(in_top || in_bottom, "strip {strip:?} is in neither half");
        }
        let covered = |quads: &[Quad], lo: f32, hi: f32| {
            let want = clipped(&reference, lo, hi);
            assert_positions_eq(quads, &want, "half");
        };
        covered(&top, 0.0, 16.0);
        covered(&bottom, 16.0, 32.0);
    }

    #[test]
    fn two_column_glyphs_get_patterns_anchored_like_single_width_cells() {
        for style in ALL_STYLES {
            let flags = decorations(style);
            for (widths, cells) in [(&[2][..], 2), (&[1, 2][..], 3), (&[2, 1, 2][..], 5)] {
                let wide = row_one_quads(make_wide_line(widths, flags, LineWidth::Normal), NORMAL);
                let narrow = underline_quads(style, cells, NORMAL);
                assert_eq!(
                    positions(&wide),
                    positions(&narrow),
                    "{style:?} {widths:?}: a two-column glyph changed the pattern"
                );
                let right = wide.iter().map(|q| q.x1).fold(f32::MIN, f32::max);
                assert!(
                    approx(right, 8.0 * gl_f32(cells)),
                    "{style:?} {widths:?}: pattern ends at {right}"
                );
            }
        }
    }

    #[test]
    fn tiny_cells_keep_every_style_inside_the_box_without_degenerate_quads() {
        for height in [2_u32, 3, 4] {
            for stroke in [1.0, 3.0, 40.0] {
                let m = Metrics {
                    cell_height: height,
                    ascent: gl_f32_u32(height) - 1.0,
                    underline_offset: -1.0,
                    strikeout_offset: 1.0,
                    stroke_size: stroke,
                    ..NORMAL
                };
                let ch = gl_f32_u32(height);
                for style in ALL_STYLES {
                    let quads = underline_quads(style, 4, m);
                    assert_inside(
                        &quads,
                        0.0,
                        32.0,
                        ch,
                        2.0 * ch,
                        &format!("{style:?} / height {height} / stroke {stroke}"),
                    );
                }
                let line =
                    make_run_line(&[(4, default_colors())], strikethrough(), LineWidth::Normal);
                assert_inside(
                    &row_one_quads(line, m),
                    0.0,
                    32.0,
                    ch,
                    2.0 * ch,
                    &format!("strikethrough / height {height} / stroke {stroke}"),
                );
            }
        }
    }
}
