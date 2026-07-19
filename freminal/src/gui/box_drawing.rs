//! Procedural box-drawing and block-element glyph rendering.
//!
//! Unicode box-drawing (`U+2500`–`U+257F`) and block-element
//! (`U+2580`–`U+259F`) glyphs are designed to tile edge-to-edge across cells
//! with no seam. Rendered from a font they only tile if the font glyph
//! happens to exactly fill the em box *and* the cell equals the em box, which
//! is generally false (the cell height is inflated by the OS/2 win-metrics
//! floor for Nerd Fonts; the cell width comes from the `'0'` advance). The
//! result is hairline gaps in block art and TUI borders.
//!
//! We therefore draw these glyphs **procedurally**, filling the exact cell
//! pixel rectangle, independent of the loaded font. This matches `kitty`,
//! `WezTerm` and `foot`.
//!
//! The module is two layers:
//!
//! * **Layer B** — a small generic rasteriser ([`fill_rect`], [`stroke_hline`],
//!   [`stroke_vline`], [`fill_subcells`], [`fill_uniform`]) that writes a
//!   white-plus-alpha RGBA bitmap. Written once; every glyph goes through it.
//! * **Layer A** — [`generate_alpha`], which decodes a codepoint into calls
//!   to Layer B. Block sub-cell patterns and eighths are **arithmetic** (a
//!   fill fraction or a sub-cell mask derived from the codepoint), and box
//!   lines use a four-arm model (`[LineStyle; 4]` per codepoint) drawn by
//!   shared arm-stroking code — no per-codepoint pixel loops.

use conv2::{ApproxFrom, ValueInto};

/// Bytes per pixel in the generated RGBA bitmap.
const BPP: usize = 4;

/// Returns `true` if `c` is a codepoint this module renders procedurally.
///
/// Currently the box-drawing and block-elements blocks (`U+2500`–`U+259F`).
/// The braille (`U+2800`–`U+28FF`), sextant (`U+1FB00`–) and octant
/// (`U+1CD00`–) blocks are natural extensions using the same [`fill_subcells`]
/// primitive, but are out of the initial scope.
#[must_use]
pub fn is_procedural(c: char) -> bool {
    matches!(u32::from(c), 0x2500..=0x259F)
}

// ---------------------------------------------------------------------------
//  Layer B — generic rasteriser
// ---------------------------------------------------------------------------

/// Write a fully-opaque white pixel at `(x, y)` in the `w`-wide RGBA `buf`.
fn set_px(buf: &mut [u8], w: usize, x: usize, y: usize, alpha: u8) {
    let idx = (y * w + x) * BPP;
    if let Some(px) = buf.get_mut(idx..idx + BPP) {
        px[0] = 255;
        px[1] = 255;
        px[2] = 255;
        px[3] = alpha;
    }
}

/// Fill the axis-aligned rectangle `[x0, x1) × [y0, y1)` (pixel coordinates,
/// float) with opaque white, by pixel-centre sampling.
///
/// Pixel-centre sampling (`px + 0.5` tested against the float bounds) is what
/// makes adjacent rectangles — and adjacent *cells* — meet with neither a gap
/// nor an overlap: a boundary at an integer pixel edge assigns that column/row
/// to exactly one side.
fn fill_rect(buf: &mut [u8], w: usize, h: usize, x0: f32, y0: f32, x1: f32, y1: f32) {
    for y in 0..h {
        let yc = value_to_f32(y) + 0.5;
        if yc < y0 || yc >= y1 {
            continue;
        }
        for x in 0..w {
            let xc = value_to_f32(x) + 0.5;
            if xc >= x0 && xc < x1 {
                set_px(buf, w, x, y, 255);
            }
        }
    }
}

/// Fill the whole cell with a uniform `alpha` (for the shade glyphs).
fn fill_uniform(buf: &mut [u8], alpha: u8) {
    for px in buf.chunks_exact_mut(BPP) {
        px[0] = 255;
        px[1] = 255;
        px[2] = 255;
        px[3] = alpha;
    }
}

/// Stroke a horizontal line of the given `thickness` centred on `y_center`,
/// spanning `[x0, x1)`.
fn stroke_hline(
    buf: &mut [u8],
    w: usize,
    h: usize,
    x0: f32,
    x1: f32,
    y_center: f32,
    thickness: f32,
) {
    let half = thickness / 2.0;
    fill_rect(buf, w, h, x0, y_center - half, x1, y_center + half);
}

/// Stroke a vertical line of the given `thickness` centred on `x_center`,
/// spanning `[y0, y1)`.
fn stroke_vline(
    buf: &mut [u8],
    w: usize,
    h: usize,
    y0: f32,
    y1: f32,
    x_center: f32,
    thickness: f32,
) {
    let half = thickness / 2.0;
    fill_rect(buf, w, h, x_center - half, y0, x_center + half, y1);
}

/// Divide the cell into a `cols × rows` grid and fill the sub-cells whose bit
/// is set in `mask` (bit index `= row * cols + col`, row 0 = top, col 0 =
/// left). This single primitive powers quadrants (2×2) and — when the block is
/// extended — sextants (2×3), octants (2×4) and braille (2×4).
fn fill_subcells(buf: &mut [u8], w: usize, h: usize, cols: u32, rows: u32, mask: u32) {
    let w_f = value_to_f32(w);
    let h_f = value_to_f32(h);
    let cols_f = u32_to_f32(cols);
    let rows_f = u32_to_f32(rows);
    for row in 0..rows {
        for col in 0..cols {
            let bit = row * cols + col;
            if mask & (1 << bit) == 0 {
                continue;
            }
            let x0 = w_f * u32_to_f32(col) / cols_f;
            let x1 = w_f * u32_to_f32(col + 1) / cols_f;
            let y0 = h_f * u32_to_f32(row) / rows_f;
            let y1 = h_f * u32_to_f32(row + 1) / rows_f;
            fill_rect(buf, w, h, x0, y0, x1, y1);
        }
    }
}

/// A quarter-circle arc: centre `(cx, cy)`, `radius`, sweeping into the
/// quadrant selected by the `(dx, dy)` sign (each ±1).
struct Arc {
    cx: f32,
    cy: f32,
    radius: f32,
    dx: f32,
    dy: f32,
}

/// Draw a quarter-circle `arc` with the given `thickness`.
///
/// Used for the rounded box corners (`U+256D`–`U+2570`). Points are sampled
/// along the arc densely enough to avoid gaps at any cell size.
fn stroke_arc(buf: &mut [u8], w: usize, h: usize, arc: &Arc, thickness: f32) {
    let steps: u16 = 64;
    let half = thickness / 2.0;
    for i in 0..=steps {
        let t = f32::from(i) / f32::from(steps);
        let angle = t * std::f32::consts::FRAC_PI_2;
        let px = (arc.dx * arc.radius).mul_add(angle.cos(), arc.cx);
        let py = (arc.dy * arc.radius).mul_add(angle.sin(), arc.cy);
        fill_rect(buf, w, h, px - half, py - half, px + half, py + half);
    }
}

// ---------------------------------------------------------------------------
//  Layer A — codepoint decoding
// ---------------------------------------------------------------------------

/// The stroke style of one arm of a box-drawing glyph.
#[derive(Clone, Copy, PartialEq, Eq)]
enum LineStyle {
    None,
    Light,
    Heavy,
    Double,
}

/// The four arms of a box-drawing glyph, in `[up, right, down, left]` order.
type Arms = [LineStyle; 4];

const N: LineStyle = LineStyle::None;
const L: LineStyle = LineStyle::Light;
const H: LineStyle = LineStyle::Heavy;
const D: LineStyle = LineStyle::Double;

/// Generate the RGBA (white + alpha) bitmap for a procedural glyph at the
/// exact cell size `w × h`. Uncovered pixels are fully transparent.
///
/// Returns a buffer of length `w * h * 4`. Codepoints outside the supported
/// set (or not yet implemented) return a fully-transparent buffer.
#[must_use]
pub fn generate_alpha(c: char, w: usize, h: usize) -> Vec<u8> {
    let mut buf = vec![0u8; w * h * BPP];
    if w == 0 || h == 0 {
        return buf;
    }
    let cp = u32::from(c);

    match cp {
        0x2580..=0x259F => draw_block(&mut buf, w, h, cp),
        0x2500..=0x257F => draw_box(&mut buf, w, h, cp),
        _ => {}
    }
    buf
}

/// Draw a block-element glyph (`U+2580`–`U+259F`) parametrically.
fn draw_block(buf: &mut [u8], w: usize, h: usize, cp: u32) {
    let w_f = value_to_f32(w);
    let h_f = value_to_f32(h);
    match cp {
        // Upper half.
        0x2580 => fill_rect(buf, w, h, 0.0, 0.0, w_f, h_f / 2.0),
        // Lower one-eighth .. full block: 2581 = 1/8 lower .. 2588 = full.
        0x2581..=0x2588 => {
            let frac = eighth(cp - 0x2580);
            fill_rect(buf, w, h, 0.0, h_f * (1.0 - frac), w_f, h_f);
        }
        // Left seven-eighths (2589) .. left one-eighth (258F).
        0x2589..=0x258F => {
            let frac = eighth(0x2590 - cp);
            fill_rect(buf, w, h, 0.0, 0.0, w_f * frac, h_f);
        }
        // Right half.
        0x2590 => fill_rect(buf, w, h, w_f / 2.0, 0.0, w_f, h_f),
        // Shades: uniform 25% / 50% / 75%.
        0x2591 => fill_uniform(buf, 64),
        0x2592 => fill_uniform(buf, 128),
        0x2593 => fill_uniform(buf, 191),
        // Upper one-eighth.
        0x2594 => fill_rect(buf, w, h, 0.0, 0.0, w_f, h_f / 8.0),
        // Right one-eighth.
        0x2595 => fill_rect(buf, w, h, w_f * 7.0 / 8.0, 0.0, w_f, h_f),
        // Quadrants: a 2x2 sub-cell mask (bit = row*2 + col; row 0 = top).
        0x2596..=0x259F => {
            if let Some(mask) = quadrant_mask(cp) {
                fill_subcells(buf, w, h, 2, 2, mask);
            }
        }
        _ => {}
    }
}

/// Fraction (in eighths) for the lower/left eighth-block glyphs.
fn eighth(n: u32) -> f32 {
    u32_to_f32(n) / 8.0
}

/// The 2×2 quadrant fill mask for `U+2596`–`U+259F`.
///
/// Bit layout: bit 0 = upper-left, bit 1 = upper-right, bit 2 = lower-left,
/// bit 3 = lower-right (matching [`fill_subcells`] `row*cols + col`).
const fn quadrant_mask(cp: u32) -> Option<u32> {
    const UL: u32 = 1 << 0;
    const UR: u32 = 1 << 1;
    const LL: u32 = 1 << 2;
    const LR: u32 = 1 << 3;
    Some(match cp {
        0x2596 => LL,
        0x2597 => LR,
        0x2598 => UL,
        0x2599 => UL | LL | LR,
        0x259A => UL | LR,
        0x259B => UL | UR | LL,
        0x259C => UL | UR | LR,
        0x259D => UR,
        0x259E => UR | LL,
        0x259F => UR | LL | LR,
        _ => return None,
    })
}

/// Draw a box-drawing glyph (`U+2500`–`U+257F`) via the four-arm model.
fn draw_box(buf: &mut [u8], w: usize, h: usize, cp: u32) {
    // Rounded corners and diagonals are special-cased; everything else is a
    // combination of the four arms.
    match cp {
        // Rounded corners.
        0x256D..=0x2570 => draw_rounded_corner(buf, w, h, cp),
        // Diagonals and cross.
        0x2571 => draw_diagonal(buf, w, h, true, false),
        0x2572 => draw_diagonal(buf, w, h, false, true),
        0x2573 => draw_diagonal(buf, w, h, true, true),
        _ => {
            if let Some(arms) = box_arms(cp) {
                draw_arms(buf, w, h, arms);
            }
        }
    }
}

/// Stroke the four arms of a box glyph from the cell centre to each edge.
fn draw_arms(buf: &mut [u8], w: usize, h: usize, arms: Arms) {
    let w_f = value_to_f32(w);
    let h_f = value_to_f32(h);
    let cx = w_f / 2.0;
    let cy = h_f / 2.0;
    let light = light_thickness(h);
    let heavy = light * 2.0;
    let dbl_off = light; // spacing of the two strands of a double line

    // [up, right, down, left]
    let [up, right, down, left] = arms;

    // Vertical arms (up: from top edge to centre; down: centre to bottom edge).
    draw_v_arm(buf, w, h, cx, 0.0, cy, up, light, heavy, dbl_off);
    draw_v_arm(buf, w, h, cx, cy, h_f, down, light, heavy, dbl_off);
    // Horizontal arms (left: from left edge to centre; right: centre to right).
    draw_h_arm(buf, w, h, cy, 0.0, cx, left, light, heavy, dbl_off);
    draw_h_arm(buf, w, h, cy, cx, w_f, right, light, heavy, dbl_off);
}

#[allow(clippy::too_many_arguments)]
fn draw_v_arm(
    buf: &mut [u8],
    w: usize,
    h: usize,
    x_center: f32,
    y0: f32,
    y1: f32,
    style: LineStyle,
    light: f32,
    heavy: f32,
    dbl_off: f32,
) {
    match style {
        LineStyle::None => {}
        LineStyle::Light => stroke_vline(buf, w, h, y0, y1, x_center, light),
        LineStyle::Heavy => stroke_vline(buf, w, h, y0, y1, x_center, heavy),
        LineStyle::Double => {
            // For a double vertical the two strands are offset horizontally,
            // and each strand extends slightly past the centre so the two
            // arms join cleanly at a junction.
            stroke_vline(buf, w, h, y0, y1, x_center - dbl_off, light);
            stroke_vline(buf, w, h, y0, y1, x_center + dbl_off, light);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_h_arm(
    buf: &mut [u8],
    w: usize,
    h: usize,
    y_center: f32,
    x0: f32,
    x1: f32,
    style: LineStyle,
    light: f32,
    heavy: f32,
    dbl_off: f32,
) {
    match style {
        LineStyle::None => {}
        LineStyle::Light => stroke_hline(buf, w, h, x0, x1, y_center, light),
        LineStyle::Heavy => stroke_hline(buf, w, h, x0, x1, y_center, heavy),
        LineStyle::Double => {
            stroke_hline(buf, w, h, x0, x1, y_center - dbl_off, light);
            stroke_hline(buf, w, h, x0, x1, y_center + dbl_off, light);
        }
    }
}

/// A rounded corner (`U+256D`–`U+2570`): a quarter arc plus straight stubs to
/// the two connected edges.
fn draw_rounded_corner(buf: &mut [u8], w: usize, h: usize, cp: u32) {
    let w_f = value_to_f32(w);
    let h_f = value_to_f32(h);
    let cx = w_f / 2.0;
    let cy = h_f / 2.0;
    let light = light_thickness(h);
    let radius = (w_f.min(h_f)) / 3.0;

    // Which two edges does this corner connect?
    // 256D: down + right, 256E: down + left, 256F: up + left, 2570: up + right.
    let (v_down, h_right) = match cp {
        0x256E => (true, false),
        0x256F => (false, false),
        0x2570 => (false, true),
        // 0x256D (down + right) is the default.
        _ => (true, true),
    };

    // Arc centre is offset toward the connected edges by `radius`.
    let arc = Arc {
        cx: if h_right { cx + radius } else { cx - radius },
        cy: if v_down { cy + radius } else { cy - radius },
        radius,
        dx: if h_right { -1.0 } else { 1.0 },
        dy: if v_down { -1.0 } else { 1.0 },
    };
    stroke_arc(buf, w, h, &arc, light);

    // Straight stubs from the arc endpoints to the cell edges.
    if v_down {
        stroke_vline(buf, w, h, arc.cy, h_f, cx, light);
    } else {
        stroke_vline(buf, w, h, 0.0, arc.cy, cx, light);
    }
    if h_right {
        stroke_hline(buf, w, h, arc.cx, w_f, cy, light);
    } else {
        stroke_hline(buf, w, h, 0.0, arc.cx, cy, light);
    }
}

/// A diagonal (`U+2571` forward `/`, `U+2572` back `\`, `U+2573` cross `X`).
fn draw_diagonal(buf: &mut [u8], w: usize, h: usize, forward: bool, back: bool) {
    let w_f = value_to_f32(w);
    let h_f = value_to_f32(h);
    let light = light_thickness(h);
    let half = light / 2.0;
    let steps = (w.max(h)) * 2;
    for step in 0..=steps {
        let frac = u32_to_f32(u32_saturating(step)) / u32_to_f32(u32_saturating(steps).max(1));
        let px = frac * w_f;
        if forward {
            // bottom-left to top-right
            let py = frac.mul_add(-h_f, h_f);
            fill_rect(buf, w, h, px - half, py - half, px + half, py + half);
        }
        if back {
            // top-left to bottom-right
            let py = frac * h_f;
            fill_rect(buf, w, h, px - half, py - half, px + half, py + half);
        }
    }
}

/// The light-line thickness in pixels for a cell of height `h`.
///
/// Derived from the cell height so lines scale with font size; at least one
/// physical pixel so a line never vanishes.
fn light_thickness(h: usize) -> f32 {
    (value_to_f32(h) / 8.0).max(1.0)
}

/// The four-arm decomposition of the line/junction/corner box glyphs.
///
/// This is *data* (the arm styles per codepoint), not geometry — the drawing
/// is done by the shared [`draw_arms`]. Dashed lines (`U+2504`–`U+250B`,
/// `U+254C`–`U+254F`) return `None` (not yet implemented); everything else in
/// the single/heavy/double line, corner, tee and cross set is covered.
const fn box_arms(cp: u32) -> Option<Arms> {
    // Split by sub-range to keep each decoding table small. Arms are
    // [up, right, down, left].
    match cp {
        0x2500..=0x254B => single_heavy_arms(cp),
        0x2550..=0x256C => double_arms(cp),
        0x2574..=0x257F => half_arms(cp),
        _ => None, // dashes (2504-250B, 254C-254F) and anything else
    }
}

/// Solid light/heavy lines, corners, tees and crosses (`U+2500`–`U+254B`).
#[allow(clippy::match_same_arms)]
const fn single_heavy_arms(cp: u32) -> Option<Arms> {
    Some(match cp {
        // Straight lines.
        0x2500 => [N, L, N, L], // light horizontal
        0x2501 => [N, H, N, H], // heavy horizontal
        0x2502 => [L, N, L, N], // light vertical
        0x2503 => [H, N, H, N], // heavy vertical

        // Light/heavy corners.
        0x250C => [N, L, L, N], // down + right
        0x250D => [N, H, L, N],
        0x250E => [N, L, H, N],
        0x250F => [N, H, H, N],
        0x2510 => [N, N, L, L], // down + left
        0x2511 => [N, N, L, H],
        0x2512 => [N, N, H, L],
        0x2513 => [N, N, H, H],
        0x2514 => [L, L, N, N], // up + right
        0x2515 => [L, H, N, N],
        0x2516 => [H, L, N, N],
        0x2517 => [H, H, N, N],
        0x2518 => [L, N, N, L], // up + left
        0x2519 => [L, N, N, H],
        0x251A => [H, N, N, L],
        0x251B => [H, N, N, H],

        // Vertical + right (tee ├).
        0x251C => [L, L, L, N],
        0x251D => [L, H, L, N],
        0x251E => [H, L, L, N],
        0x251F => [L, L, H, N],
        0x2520 => [H, L, H, N],
        0x2521 => [H, H, L, N],
        0x2522 => [L, H, H, N],
        0x2523 => [H, H, H, N],

        // Vertical + left (tee ┤).
        0x2524 => [L, N, L, L],
        0x2525 => [L, N, L, H],
        0x2526 => [H, N, L, L],
        0x2527 => [L, N, H, L],
        0x2528 => [H, N, H, L],
        0x2529 => [H, N, L, H],
        0x252A => [L, N, H, H],
        0x252B => [H, N, H, H],

        // Horizontal + down (tee ┬).
        0x252C => [N, L, L, L],
        0x252D => [N, L, L, H],
        0x252E => [N, H, L, L],
        0x252F => [N, H, L, H],
        0x2530 => [N, L, H, L],
        0x2531 => [N, L, H, H],
        0x2532 => [N, H, H, L],
        0x2533 => [N, H, H, H],

        // Horizontal + up (tee ┴).
        0x2534 => [L, L, N, L],
        0x2535 => [L, L, N, H],
        0x2536 => [L, H, N, L],
        0x2537 => [L, H, N, H],
        0x2538 => [H, L, N, L],
        0x2539 => [H, L, N, H],
        0x253A => [H, H, N, L],
        0x253B => [H, H, N, H],

        // Cross ┼.
        0x253C => [L, L, L, L],
        0x253D => [L, L, L, H],
        0x253E => [L, H, L, L],
        0x253F => [L, H, L, H],
        0x2540 => [H, L, L, L],
        0x2541 => [L, L, H, L],
        0x2542 => [H, L, H, L],
        0x2543 => [H, L, L, H],
        0x2544 => [H, H, L, L],
        0x2545 => [L, L, N, L],
        0x2546 => [L, H, L, L],
        0x2547 => [H, H, L, H],
        0x2548 => [L, H, H, H],
        0x2549 => [H, L, H, H],
        0x254A => [H, H, L, L],
        0x254B => [H, H, H, H],

        _ => return None, // dashes 2504-250B
    })
}

/// Double-line box glyphs (`U+2550`–`U+256C`).
const fn double_arms(cp: u32) -> Option<Arms> {
    Some(match cp {
        0x2550 => [N, D, N, D], // double horizontal
        0x2551 => [D, N, D, N], // double vertical
        0x2552 => [N, D, L, N],
        0x2553 => [N, L, D, N],
        0x2554 => [N, D, D, N], // double down + right
        0x2555 => [N, N, L, D],
        0x2556 => [N, N, D, L],
        0x2557 => [N, N, D, D], // double down + left
        0x2558 => [L, D, N, N],
        0x2559 => [D, L, N, N],
        0x255A => [D, D, N, N], // double up + right
        0x255B => [L, N, N, D],
        0x255C => [D, N, N, L],
        0x255D => [D, N, N, D], // double up + left
        0x255E => [L, D, L, N],
        0x255F => [D, L, D, N],
        0x2560 => [D, D, D, N], // double vertical + right
        0x2561 => [L, N, L, D],
        0x2562 => [D, N, D, L],
        0x2563 => [D, N, D, D], // double vertical + left
        0x2564 => [N, D, L, D],
        0x2565 => [N, L, D, L],
        0x2566 => [N, D, D, D], // double horizontal + down
        0x2567 => [L, D, N, D],
        0x2568 => [D, L, N, L],
        0x2569 => [D, D, N, D], // double horizontal + up
        0x256A => [L, D, L, D],
        0x256B => [D, L, D, L],
        0x256C => [D, D, D, D], // double cross
        _ => return None,
    })
}

/// Half-line and mixed-weight-half glyphs (`U+2574`–`U+257F`).
const fn half_arms(cp: u32) -> Option<Arms> {
    Some(match cp {
        0x2574 => [N, N, N, L], // left
        0x2575 => [L, N, N, N], // up
        0x2576 => [N, L, N, N], // right
        0x2577 => [N, N, L, N], // down
        0x2578 => [N, N, N, H], // heavy left
        0x2579 => [H, N, N, N], // heavy up
        0x257A => [N, H, N, N], // heavy right
        0x257B => [N, N, H, N], // heavy down
        0x257C => [N, H, N, L], // light left + heavy right
        0x257D => [L, N, H, N], // light up + heavy down
        0x257E => [N, L, N, H], // heavy left + light right
        0x257F => [H, N, L, N], // heavy up + light down
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
//  Small numeric helpers (conv2, no raw `as`)
// ---------------------------------------------------------------------------

fn value_to_f32(v: usize) -> f32 {
    let n: u32 = v.value_into().unwrap_or(u32::MAX);
    u32_to_f32(n)
}

fn u32_to_f32(v: u32) -> f32 {
    f32::approx_from(v).unwrap_or(0.0)
}

fn u32_saturating(v: usize) -> u32 {
    v.value_into().unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alpha_at(buf: &[u8], w: usize, x: usize, y: usize) -> u8 {
        buf[(y * w + x) * BPP + 3]
    }

    #[test]
    fn is_procedural_covers_box_and_block() {
        assert!(is_procedural('\u{2500}'));
        assert!(is_procedural('\u{2588}')); // full block
        assert!(is_procedural('\u{259F}'));
        assert!(!is_procedural('A'));
        assert!(!is_procedural('\u{2800}')); // braille, out of scope
    }

    #[test]
    fn full_block_is_all_opaque() {
        let (w, h) = (10, 19);
        let buf = generate_alpha('\u{2588}', w, h);
        for y in 0..h {
            for x in 0..w {
                assert_eq!(alpha_at(&buf, w, x, y), 255, "pixel ({x},{y}) not opaque");
            }
        }
    }

    #[test]
    fn two_full_blocks_share_no_vertical_seam() {
        // Every row of a full block must be fully opaque, so stacking two
        // vertically leaves no transparent gap between them.
        let (w, h) = (12, 20);
        let buf = generate_alpha('\u{2588}', w, h);
        // Top and bottom rows opaque (the row-gap failure mode).
        for x in 0..w {
            assert_eq!(alpha_at(&buf, w, x, 0), 255, "top row transparent at {x}");
            assert_eq!(
                alpha_at(&buf, w, x, h - 1),
                255,
                "bottom row transparent at {x}"
            );
        }
    }

    #[test]
    fn two_full_blocks_share_no_horizontal_seam() {
        let (w, h) = (12, 20);
        let buf = generate_alpha('\u{2588}', w, h);
        for y in 0..h {
            assert_eq!(alpha_at(&buf, w, 0, y), 255, "left col transparent at {y}");
            assert_eq!(
                alpha_at(&buf, w, w - 1, y),
                255,
                "right col transparent at {y}"
            );
        }
    }

    #[test]
    fn light_horizontal_is_a_centered_band() {
        let (w, h) = (12, 20);
        let buf = generate_alpha('\u{2500}', w, h);
        // Top and bottom rows are transparent; the middle has opaque pixels.
        for x in 0..w {
            assert_eq!(alpha_at(&buf, w, x, 0), 0);
            assert_eq!(alpha_at(&buf, w, x, h - 1), 0);
        }
        let mid = h / 2;
        assert_eq!(alpha_at(&buf, w, w / 2, mid), 255);
    }

    #[test]
    fn light_cross_reaches_all_four_edges() {
        let (w, h) = (13, 21);
        let buf = generate_alpha('\u{253C}', w, h); // ┼
        let cx = w / 2;
        let cy = h / 2;
        assert_eq!(alpha_at(&buf, w, cx, 0), 255, "no top reach");
        assert_eq!(alpha_at(&buf, w, cx, h - 1), 255, "no bottom reach");
        assert_eq!(alpha_at(&buf, w, 0, cy), 255, "no left reach");
        assert_eq!(alpha_at(&buf, w, w - 1, cy), 255, "no right reach");
    }

    #[test]
    fn shade_50_percent_is_half_alpha_everywhere() {
        let (w, h) = (8, 8);
        let buf = generate_alpha('\u{2592}', w, h);
        for y in 0..h {
            for x in 0..w {
                assert_eq!(alpha_at(&buf, w, x, y), 128);
            }
        }
    }

    #[test]
    fn lower_left_quadrant_fills_only_lower_left() {
        let (w, h) = (10, 10);
        let buf = generate_alpha('\u{2596}', w, h); // ▖ lower-left
        // lower-left opaque
        assert_eq!(alpha_at(&buf, w, 1, h - 1), 255);
        // upper-right transparent
        assert_eq!(alpha_at(&buf, w, w - 1, 1), 0);
        // upper-left transparent
        assert_eq!(alpha_at(&buf, w, 1, 1), 0);
    }

    #[test]
    fn fill_subcells_fills_expected_cells() {
        let (w, h) = (8, 8);
        let mut buf = vec![0u8; w * h * BPP];
        // 2x2 grid, fill only bit 0 (upper-left).
        fill_subcells(&mut buf, w, h, 2, 2, 1);
        assert_eq!(alpha_at(&buf, w, 1, 1), 255); // upper-left
        assert_eq!(alpha_at(&buf, w, w - 1, 1), 0); // upper-right
        assert_eq!(alpha_at(&buf, w, 1, h - 1), 0); // lower-left
    }

    #[test]
    fn lower_eighth_fills_bottom_only() {
        let (w, h) = (8, 16);
        let buf = generate_alpha('\u{2581}', w, h); // lower one-eighth
        // bottom row opaque
        assert_eq!(alpha_at(&buf, w, 0, h - 1), 255);
        // top row transparent
        assert_eq!(alpha_at(&buf, w, 0, 0), 0);
    }

    #[test]
    fn unsupported_codepoint_is_transparent() {
        let (w, h) = (8, 8);
        let buf = generate_alpha('\u{2505}', w, h); // dashed — not implemented
        assert!(buf.iter().all(|&b| b == 0));
    }

    #[test]
    fn zero_size_is_empty() {
        assert!(generate_alpha('\u{2588}', 0, 10).is_empty());
        assert!(generate_alpha('\u{2588}', 10, 0).iter().all(|&b| b == 0));
    }
}
