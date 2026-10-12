// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Toast button rows: measurement, flow (wrapping), and animated quad/rect/run layout.

use super::layout::{ToastLayoutInput, ToastLayoutMetrics, fit_text, lighten, text_block_height};
use crate::gui::font_manager::FontManager;
use crate::gui::renderer::{ToastQuad, ToastTextMetrics, ToastTextRenderer, ToastTextRun};

/// Horizontal padding inside a button, on each side of its label.
const BUTTON_PAD_X_PTS: f32 = 10.0;
/// Vertical padding inside a button, above and below its label.
const BUTTON_PAD_Y_PTS: f32 = 4.0;
/// Gap between buttons on one row.
const BUTTON_GAP_PTS: f32 = 6.0;
/// Gap between button rows.
const BUTTON_ROW_GAP_PTS: f32 = 6.0;
/// Gap between the title/detail block and the first button row.
pub(super) const BUTTON_TOP_GAP_PTS: f32 = 8.0;
/// The widest a single button may be; longer labels are fitted with an
/// ellipsis.
const MAX_BUTTON_WIDTH_PTS: f32 = 160.0;
/// How much lighter than the pill a button's fill is.
const BUTTON_LIGHTEN: f32 = 0.08;

/// One measured toast button label, input to [`layout_toasts`](super::layout::layout_toasts).
#[derive(Debug, Clone)]
pub(super) struct ToastButtonInput {
    /// The exact label that was measured, already whitespace-normalized and
    /// fitted to [`MAX_BUTTON_WIDTH_PTS`] (see [`fit_text`]).
    pub label_text: String,
    /// The physical pixel size `label_text` was measured/rasterized at.
    pub size_px: f32,
    /// Measured `label_text`, in physical pixels.
    pub label: ToastTextMetrics,
}

/// The width of one button: its label plus padding, physical pixels.
fn button_width(button: &ToastButtonInput, ppp: f32) -> f32 {
    BUTTON_PAD_X_PTS.mul_add(2.0 * ppp, button.label.width)
}

/// The width of all `buttons` on a single row, physical pixels.
pub(super) fn button_row_width(buttons: &[ToastButtonInput], ppp: f32) -> f32 {
    let gap = BUTTON_GAP_PTS * ppp;
    buttons
        .iter()
        .enumerate()
        .map(|(i, b)| button_width(b, ppp) + if i == 0 { 0.0 } else { gap })
        .sum()
}

/// Where each button sits within the button block, before animation.
pub(super) struct ButtonFlow {
    /// `[x, y, width, height]` per button, physical pixels, relative to the
    /// block's top-left corner.
    rects: Vec<[f32; 4]>,
    /// Total height of all rows, physical pixels (0 with no buttons).
    pub(super) height: f32,
}

/// Flow `buttons` left to right, wrapping to a new row when the next button
/// would pass `inner_width` (physical pixels). A button wider than the row
/// sits alone on its own row. All buttons share the tallest label's height.
pub(super) fn button_flow(buttons: &[ToastButtonInput], inner_width: f32, ppp: f32) -> ButtonFlow {
    let gap = BUTTON_GAP_PTS * ppp;
    let row_gap = BUTTON_ROW_GAP_PTS * ppp;
    let label_height = buttons
        .iter()
        .map(|b| b.label.height)
        .fold(0.0_f32, f32::max);
    let height = BUTTON_PAD_Y_PTS.mul_add(2.0 * ppp, label_height);

    let mut rects = Vec::with_capacity(buttons.len());
    let (mut x, mut y) = (0.0_f32, 0.0_f32);
    for button in buttons {
        let width = button_width(button, ppp);
        if x > 0.0 && x + width > inner_width {
            x = 0.0;
            y += height + row_gap;
        }
        rects.push([x, y, width, height]);
        x += width + gap;
    }
    let total = if rects.is_empty() { 0.0 } else { y + height };
    ButtonFlow {
        rects,
        height: total,
    }
}

/// The animated draw data and hit rects of one toast's buttons.
pub(super) struct ToastButtonLayout {
    /// One quad per button, in label order.
    pub(super) quads: Vec<ToastQuad>,
    /// One hit rect per button, logical points, clamped into the pill's.
    pub(super) rects_logical: Vec<[f32; 4]>,
    /// One label run per button.
    pub(super) runs: Vec<ToastTextRun>,
}

/// Lay out `input`'s buttons under its title/detail block.
///
/// `settled` is the pill's settled `[x, y, width, height]` (physical pixels);
/// the buttons are flowed against that rect and then mapped through the same
/// scale-and-slide transform the pill goes through (see `layout::layout_one_toast`),
/// so they never drift from the pill during the entry animation. `pill` is the
/// already-animated pill quad (its border and opacity are reused),
/// `hit_rect` its logical hit rect (the button rects are clamped into it), and
/// `text_color` the label colour.
pub(super) fn layout_buttons(
    input: &ToastLayoutInput,
    settled: [f32; 4],
    pill: &ToastQuad,
    hit_rect: [f32; 4],
    metrics: &ToastLayoutMetrics,
    text_color: [f32; 4],
) -> ToastButtonLayout {
    let [settled_x, settled_y, width, height] = settled;
    let anim = input.anim;
    let ppp = metrics.ppp;
    let flow = button_flow(&input.buttons, 2.0_f32.mul_add(-metrics.pad_x, width), ppp);
    let block_x = settled_x + metrics.pad_x;
    let block_y = BUTTON_TOP_GAP_PTS.mul_add(
        ppp,
        settled_y + metrics.pad_y + text_block_height(input, metrics.detail_gap),
    );
    let center_x = settled_x + width / 2.0;
    let center_y = settled_y + height / 2.0;

    let mut layout = ToastButtonLayout {
        quads: Vec::with_capacity(input.buttons.len()),
        rects_logical: Vec::with_capacity(input.buttons.len()),
        runs: Vec::with_capacity(input.buttons.len()),
    };
    for (button, rel) in input.buttons.iter().zip(&flow.rects) {
        // The same transform as the pill: scale about the pill's centre, then
        // slide horizontally.
        let x = anim.slide_x.mul_add(
            ppp,
            (block_x + rel[0] - center_x).mul_add(anim.scale, center_x),
        );
        let y = (block_y + rel[1] - center_y).mul_add(anim.scale, center_y);
        let (w, h) = (rel[2] * anim.scale, rel[3] * anim.scale);
        let half_extent = w.min(h) / 2.0;
        layout.quads.push(ToastQuad {
            x,
            y,
            width: w,
            height: h,
            corner_radius: metrics.corner_radius.min(half_extent).max(0.0),
            color_top: lighten(pill.color_top, BUTTON_LIGHTEN),
            color_bottom: lighten(pill.color_bottom, BUTTON_LIGHTEN),
            border_color: pill.border_color,
            border_width: pill.border_width.min(half_extent).max(0.0),
            accent: [0.0; 4],
            opacity: pill.opacity,
        });
        layout.rects_logical.push([
            (x / ppp).max(hit_rect[0]),
            (y / ppp).max(hit_rect[1]),
            ((x + w) / ppp).min(hit_rect[2]),
            ((y + h) / ppp).min(hit_rect[3]),
        ]);
        layout.runs.push(ToastTextRun {
            text: button.label_text.clone(),
            origin_x: x + (w - button.label.width) / 2.0,
            baseline_y: y + (h - button.label.height) / 2.0 + button.label.ascent,
            size_px: button.size_px,
            color: text_color,
        });
    }
    layout
}

/// Measure and fit each of `labels` as a button label at `size_px`, so a
/// single button always fits [`MAX_BUTTON_WIDTH_PTS`].
pub(super) fn measure_buttons(
    labels: &[String],
    renderer: &ToastTextRenderer,
    size_px: f32,
    ppp: f32,
    font_manager: &mut FontManager,
) -> Vec<ToastButtonInput> {
    let budget = (2.0_f32.mul_add(-BUTTON_PAD_X_PTS, MAX_BUTTON_WIDTH_PTS) * ppp).max(1.0);
    labels
        .iter()
        .map(|raw| {
            let (label_text, label) = fit_text(renderer, raw, size_px, budget, font_manager);
            ToastButtonInput {
                label_text,
                size_px,
                label,
            }
        })
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use crate::gui::icons::ChromeIcon;
    use crate::gui::toast::test_support::{
        ENTRY_ANIM, SETTLED_ANIM, button_input, buttons_output, rect_inside,
    };
    // -----------------------------------------------------------------
    //  Buttons: layout, hit-testing, events, cap
    // -----------------------------------------------------------------

    #[test]
    fn three_short_buttons_share_one_row_in_order_below_the_title() {
        let labels = ["One", "Two", "Three"];
        let buttons = labels.iter().map(|l| button_input(l, 40.0)).collect();
        let out = buttons_output(buttons, false, SETTLED_ANIM, 1.0);

        assert_eq!(out.button_rects_logical.len(), 3);
        assert_eq!(out.button_quads.len(), 3);
        let rects = &out.button_rects_logical;
        for rect in rects {
            assert!(rect_inside(*rect, out.hit_rect_logical), "{rect:?}");
        }
        for pair in rects.windows(2) {
            assert!(pair[0][2] <= pair[1][0], "left to right, no overlap");
            assert!((pair[0][1] - pair[1][1]).abs() < 0.01, "one row");
        }
        // The title is the second run; every button starts below its baseline.
        let title_baseline = out.runs[1].baseline_y;
        assert!(rects.iter().all(|r| r[1] > title_baseline));

        // Labels are appended after the dismiss glyph, in order, centred.
        let label_runs = &out.runs[3..];
        assert_eq!(label_runs.len(), 3);
        for ((run, label), rect) in label_runs.iter().zip(labels).zip(rects) {
            assert_eq!(run.text, label);
            assert!(run.origin_x > rect[0] && run.origin_x < rect[2]);
            assert!(run.baseline_y > rect[1] && run.baseline_y < rect[3]);
        }
        // The quads match the rects and carry no accent bar.
        for (quad, rect) in out.button_quads.iter().zip(rects) {
            assert!((quad.x - rect[0]).abs() < 0.01 && (quad.y - rect[1]).abs() < 0.01);
            assert!((quad.x + quad.width - rect[2]).abs() < 0.01);
            assert!(quad.accent[3].abs() < f32::EPSILON);
            assert!((quad.opacity - out.pill.opacity).abs() < f32::EPSILON);
            assert!(
                quad.color_top[0] > out.pill.color_top[0],
                "lighter than the pill"
            );
        }
    }

    #[test]
    fn button_rects_stay_inside_the_hit_rect_mid_animation() {
        let buttons = (0..4)
            .map(|i| button_input(&format!("b{i}"), 60.0))
            .collect();
        let out = buttons_output(buttons, false, ENTRY_ANIM, 1.0);
        for rect in &out.button_rects_logical {
            assert!(rect_inside(*rect, out.hit_rect_logical), "{rect:?}");
        }
    }

    #[test]
    fn a_toast_without_buttons_keeps_its_original_geometry() {
        // Numbers from the pre-button formula at ppp 1: width is
        // 2*14 + icon 16 + 8 + label 100 + 8 + close 18 = 178; height is
        // max(40, 2*10 + 20) = 40, or 2*10 + 20 + 4 + 16 = 60 with a detail.
        for (has_detail, height) in [(false, 40.0_f32), (true, 60.0)] {
            let out = buttons_output(Vec::new(), has_detail, SETTLED_ANIM, 1.0);
            assert!((out.pill.width - 178.0).abs() < 0.01);
            assert!((out.pill.height - height).abs() < 0.01);
            assert!(out.button_quads.is_empty());
            assert_eq!(out.button_rects_logical.len(), 0);
            assert_eq!(out.runs.len(), if has_detail { 4 } else { 3 });
            assert_eq!(
                out.runs.last().map(|r| r.text.clone()),
                Some(ChromeIcon::Close.glyph())
            );
        }
    }
}
