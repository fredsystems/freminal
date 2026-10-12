// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Toast button rows: flow (wrapping), animated quad/rect/run layout, and the
//! hover/pressed fill states.
//!
//! This module depends only on [`super::input`]; measurement (which needs
//! `layout::fit_text`) lives in `layout`.

use super::ToastKind;
use super::input::{ToastButtonInput, ToastLayoutInput, ToastLayoutMetrics, text_block_height};
use crate::gui::renderer::{ToastQuad, ToastTextRun};

/// Horizontal padding inside a button, on each side of its label.
pub(super) const BUTTON_PAD_X_PTS: f32 = 10.0;
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
pub(super) const MAX_BUTTON_WIDTH_PTS: f32 = 160.0;

/// Shift a button's fill moves away from the pill in its normal state.
const BUTTON_SHIFT_NORMAL: f32 = 0.08;
/// Shift while the pointer is over the button.
const BUTTON_SHIFT_HOVERED: f32 = 0.16;
/// Shift while the button is held down.
const BUTTON_SHIFT_PRESSED: f32 = 0.24;

/// Shift each color channel of `rgba` by `amount` (positive lightens,
/// negative darkens), clamped to `0..=1`. Alpha is left untouched.
pub(super) fn lighten(rgba: [f32; 4], amount: f32) -> [f32; 4] {
    [
        (rgba[0] + amount).clamp(0.0, 1.0),
        (rgba[1] + amount).clamp(0.0, 1.0),
        (rgba[2] + amount).clamp(0.0, 1.0),
        rgba[3],
    ]
}

/// Which way a button's fill moves away from the pill it sits on.
///
/// A lighter button on a light pill vanishes into it, so a light pill darkens
/// its buttons instead (the same luma test that picks dark text,
/// [`ToastKind::prefers_dark_text`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ButtonTint {
    /// The fill is lighter than the pill (dark pills).
    Lighten,
    /// The fill is darker than the pill (light pills).
    Darken,
}

impl ButtonTint {
    /// The direction that keeps buttons visible on a pill filled with
    /// `background`.
    pub(super) fn for_background(background: egui::Color32) -> Self {
        if ToastKind::prefers_dark_text(background) {
            Self::Darken
        } else {
            Self::Lighten
        }
    }

    /// `amount` with this tint's sign.
    fn signed(self, amount: f32) -> f32 {
        match self {
            Self::Lighten => amount,
            Self::Darken => -amount,
        }
    }
}

/// How a button currently looks to the pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ButtonState {
    /// Not under the pointer.
    Normal,
    /// Under the pointer, no press in progress elsewhere.
    Hovered,
    /// Pressed on this button and still held over it.
    Pressed,
}

impl ButtonState {
    /// The size of the fill shift for this state.
    const fn shift(self) -> f32 {
        match self {
            Self::Normal => BUTTON_SHIFT_NORMAL,
            Self::Hovered => BUTTON_SHIFT_HOVERED,
            Self::Pressed => BUTTON_SHIFT_PRESSED,
        }
    }
}

/// A button's `(color_top, color_bottom)` fill for `state`, derived from the
/// pill's own two gradient stops so the button keeps the pill's gradient.
pub(super) fn button_fill(
    pill_top: [f32; 4],
    pill_bottom: [f32; 4],
    tint: ButtonTint,
    state: ButtonState,
) -> ([f32; 4], [f32; 4]) {
    let shift = tint.signed(state.shift());
    (lighten(pill_top, shift), lighten(pill_bottom, shift))
}

/// The paint inputs a button row needs beyond geometry.
#[derive(Debug, Clone, Copy)]
pub(super) struct ButtonPaint {
    /// The label colour.
    pub text_color: [f32; 4],
    /// Which way the fills move off the pill.
    pub tint: ButtonTint,
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
/// `paint` the label colour and fill direction. Quads are produced in the
/// [`ButtonState::Normal`] fill; the hover/pressed fills are applied after
/// layout (see `hit::apply_button_states`), once the pointer is known.
pub(super) fn layout_buttons(
    input: &ToastLayoutInput,
    settled: [f32; 4],
    pill: &ToastQuad,
    hit_rect: [f32; 4],
    metrics: &ToastLayoutMetrics,
    paint: ButtonPaint,
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

    let (fill_top, fill_bottom) = button_fill(
        pill.color_top,
        pill.color_bottom,
        paint.tint,
        ButtonState::Normal,
    );
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
            color_top: fill_top,
            color_bottom: fill_bottom,
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
            color: paint.text_color,
        });
    }
    layout
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::gui::icons::ChromeIcon;
    use crate::gui::toast::test_support::{
        DetailLine, ENTRY_ANIM, SETTLED_ANIM, button_input, buttons_output, rect_inside,
    };
    // -----------------------------------------------------------------
    //  Buttons: layout, hit-testing, events, cap
    // -----------------------------------------------------------------

    #[test]
    fn three_short_buttons_share_one_row_in_order_below_the_title() {
        let labels = ["One", "Two", "Three"];
        let buttons = labels.iter().map(|l| button_input(l, 40.0)).collect();
        let out = buttons_output(buttons, DetailLine::Absent, SETTLED_ANIM, 1.0);

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
        let out = buttons_output(buttons, DetailLine::Absent, ENTRY_ANIM, 1.0);
        for rect in &out.button_rects_logical {
            assert!(rect_inside(*rect, out.hit_rect_logical), "{rect:?}");
        }
    }

    #[test]
    fn a_toast_without_buttons_keeps_its_original_geometry() {
        // Numbers from the pre-button formula at ppp 1: width is
        // 2*14 + icon 16 + 8 + label 100 + 8 + close 18 = 178; height is
        // max(40, 2*10 + 20) = 40, or 2*10 + 20 + 4 + 16 = 60 with a detail.
        for (detail_line, height) in [(DetailLine::Absent, 40.0_f32), (DetailLine::Present, 60.0)] {
            let out = buttons_output(Vec::new(), detail_line, SETTLED_ANIM, 1.0);
            assert!((out.pill.width - 178.0).abs() < 0.01);
            assert!((out.pill.height - height).abs() < 0.01);
            assert!(out.button_quads.is_empty());
            assert_eq!(out.button_rects_logical.len(), 0);
            assert_eq!(
                out.runs.len(),
                if detail_line == DetailLine::Present {
                    4
                } else {
                    3
                }
            );
            assert_eq!(
                out.runs.last().map(|r| r.text.clone()),
                Some(ChromeIcon::Close.glyph())
            );
        }
    }

    #[test]
    fn lighten_shifts_rgb_and_clamps_leaving_alpha_untouched() {
        let base = [0.5, 0.5, 0.5, 0.9];
        let lighter = lighten(base, 0.4);
        assert!((lighter[0] - 0.9).abs() < f32::EPSILON);
        assert!((lighter[3] - 0.9).abs() < f32::EPSILON);

        let clamped = lighten(base, 10.0);
        assert!((clamped[0] - 1.0).abs() < f32::EPSILON);

        let darker = lighten(base, -10.0);
        assert!(darker[0].abs() < f32::EPSILON);
    }

    #[test]
    fn the_tint_follows_the_pill_brightness() {
        assert_eq!(
            ButtonTint::for_background(egui::Color32::from_gray(30)),
            ButtonTint::Lighten
        );
        assert_eq!(
            ButtonTint::for_background(egui::Color32::from_gray(245)),
            ButtonTint::Darken
        );
    }

    #[test]
    fn hover_and_pressed_fills_move_further_from_the_pill_than_normal() {
        let pill = [0.5, 0.5, 0.5, 1.0];
        for tint in [ButtonTint::Lighten, ButtonTint::Darken] {
            let distance = |state| {
                let (top, bottom) = button_fill(pill, pill, tint, state);
                assert_eq!(top, bottom, "same stops in, same stops out");
                (top[0] - pill[0]).abs()
            };
            let (normal, hovered, pressed) = (
                distance(ButtonState::Normal),
                distance(ButtonState::Hovered),
                distance(ButtonState::Pressed),
            );
            assert!(
                normal > 0.0 && hovered > normal && pressed > hovered,
                "{tint:?}"
            );
        }
        let (lighter, _) = button_fill(pill, pill, ButtonTint::Lighten, ButtonState::Hovered);
        let (darker, _) = button_fill(pill, pill, ButtonTint::Darken, ButtonState::Hovered);
        assert!(lighter[0] > pill[0] && darker[0] < pill[0]);
        assert!((lighter[3] - pill[3]).abs() < f32::EPSILON, "alpha is kept");
    }
}
