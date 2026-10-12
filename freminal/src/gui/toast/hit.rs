// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Toast hit-testing and click targeting.

use std::time::Instant;

use super::ToastStack;
use super::layout::ToastLayoutOutput;

/// Which part of a toast a click landed on. See [`click_target`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ToastClickPart {
    /// The dismiss ("x") square.
    Dismiss,
    /// Button _n_ (1-based, in label order).
    Button(u8),
    /// Anywhere else within the pill.
    Body,
}

impl ToastStack {
    /// Hover/dismiss hit-test: allocate one interactive click-only egui
    /// region per toast, over its `hit_rect_logical` (logical points).
    /// Hovering a toast's pill pauses its expiry (`last_hovered`); clicking
    /// anywhere within a toast's pill dismisses it — the whole pill is the
    /// click target. For a keyed toast the click is also classified by
    /// [`click_target`] (dismiss square vs body) and reported as events via
    /// [`Self::apply_click`], which removes the toast.
    ///
    /// The regions live in an interactive `egui::Area` above the terminal so
    /// egui consumes the click (see the call site's finding-#3 comment). At
    /// most one toast is dismissed per frame, and regions are hit-tested
    /// newest-first (newest toasts are drawn on top) so that during the brief
    /// entry slide — when an arriving toast can transiently overlap its
    /// neighbour — a click in the overlap dismisses only the topmost one
    /// (review finding #4). Removes the dismissed toast itself. Split out of
    /// [`Self::show`] to keep it under the line-count limit.
    pub(super) fn hit_test(
        &mut self,
        ctx: &egui::Context,
        outputs: &[ToastLayoutOutput],
        now: Instant,
    ) {
        // Allocate one interactive `Area` PER TOAST, each sized and positioned
        // exactly to its pill's hit rect, so egui consumes a click that lands
        // on a toast (keeping it off the terminal). Deliberately NOT a single
        // window-spanning area anchored at the origin: egui 0.35's cross-layer
        // hit-test hides a lower-layer widget whose rect is fully contained by
        // a higher-layer one, so a full-window interactive area would suppress
        // interaction with the chrome beneath it. Tight per-toast areas only
        // cover the pills.
        //
        // egui's own arbitration reports the click on whichever area it picked;
        // we additionally derive the hover target and the single, topmost
        // dismiss target from the pointer position via `topmost_hit`, so the
        // overlap-precedence rule has one tested source of truth.
        let mut any_clicked = false;
        for (i, out) in outputs.iter().enumerate() {
            let [min_x, min_y, max_x, max_y] = out.hit_rect_logical;
            let rect = egui::Rect::from_min_max(egui::pos2(min_x, min_y), egui::pos2(max_x, max_y));
            let clicked = egui::Area::new(egui::Id::new(("toast_interaction", i)))
                .order(egui::Order::Foreground)
                .interactable(true)
                .fixed_pos(rect.min)
                .show(ctx, |ui| {
                    ui.allocate_rect(rect, egui::Sense::click()).clicked()
                })
                .inner;
            if clicked {
                any_clicked = true;
            }
        }

        let pointer = ctx.input(|i| i.pointer.hover_pos());
        let Some(pos) = pointer else {
            return;
        };
        if let Some(id) = topmost_hit(outputs, pos)
            && let Some(toast) = self.entries.iter_mut().find(|t| t.id == id)
        {
            toast.last_hovered = Some(now);
        }
        if any_clicked && let Some((id, part)) = click_target(outputs, pos) {
            self.apply_click(id, part);
        }
    }
}

/// Whether logical-point `pos` falls within `rect` (`[min_x, min_y, max_x,
/// max_y]`, logical points — see [`ToastLayoutOutput::hit_rect_logical`]).
pub(super) fn rect_contains(rect: [f32; 4], pos: egui::Pos2) -> bool {
    pos.x >= rect[0] && pos.x <= rect[2] && pos.y >= rect[1] && pos.y <= rect[3]
}

/// The id of the topmost toast whose hit rect contains `pos`, or `None`.
///
/// "Topmost" = newest = last in `outputs` (drawn on top), so this iterates in
/// reverse. Pure and GL-/egui-free so the overlap-precedence rule (review
/// finding #4) is unit-testable; the interactive [`ToastStack::hit_test`]
/// egui path relies on the same newest-first ordering when it allocates its
/// click regions.
fn topmost_hit(outputs: &[ToastLayoutOutput], pos: egui::Pos2) -> Option<u64> {
    topmost_output(outputs, pos).map(|out| out.id)
}

/// The topmost (newest) output whose hit rect contains `pos`.
fn topmost_output(outputs: &[ToastLayoutOutput], pos: egui::Pos2) -> Option<&ToastLayoutOutput> {
    outputs
        .iter()
        .rev()
        .find(|out| rect_contains(out.hit_rect_logical, pos))
}

/// What a click at `pos` hit: the topmost toast (same precedence as
/// [`topmost_hit`]) and which part of it. The dismiss square wins over a
/// button, and a button over the body. The distinction only matters for keyed
/// toasts — [`ToastStack::apply_click`] ignores it for unkeyed ones.
pub(super) fn click_target(
    outputs: &[ToastLayoutOutput],
    pos: egui::Pos2,
) -> Option<(u64, ToastClickPart)> {
    topmost_output(outputs, pos).map(|out| {
        let part = if rect_contains(out.dismiss_rect_logical, pos) {
            ToastClickPart::Dismiss
        } else if let Some((_, n)) = out
            .button_rects_logical
            .iter()
            .zip(1_u8..)
            .find(|(rect, _)| rect_contains(**rect, pos))
        {
            ToastClickPart::Button(n)
        } else {
            ToastClickPart::Body
        };
        (out.id, part)
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::gui::renderer::ToastQuad;
    use crate::gui::toast::test_support::{
        SETTLED_ANIM, button_input, buttons_output, single_toast_outputs,
    };
    // -----------------------------------------------------------------
    //  ToastStack::hit_test — click-anywhere-on-the-pill dismissal
    // -----------------------------------------------------------------

    #[test]
    fn topmost_hit_matches_a_click_anywhere_in_the_pill() {
        let outputs = single_toast_outputs(7);
        let pill = &outputs[0].pill;
        // The pill's center — nowhere near the drawn "x" glyph at the right
        // edge — must still match (the whole pill is the click target).
        let center = egui::pos2(pill.x + pill.width / 2.0, pill.y + pill.height / 2.0);
        assert_eq!(topmost_hit(&outputs, center), Some(7));
    }

    #[test]
    fn topmost_hit_misses_a_click_outside_the_pill() {
        let outputs = single_toast_outputs(7);
        let pill = &outputs[0].pill;
        let outside = egui::pos2(pill.x - 50.0, pill.y - 50.0);
        assert_eq!(topmost_hit(&outputs, outside), None);
    }

    #[test]
    fn topmost_hit_prefers_the_newest_toast_on_overlap() {
        // Two toasts whose hit rects overlap; the newest (last in `outputs`,
        // drawn on top) must win the overlap (review finding #4).
        let older = ToastLayoutOutput {
            id: 1,
            pill: ToastQuad {
                x: 0.0,
                y: 0.0,
                width: 10.0,
                height: 10.0,
                corner_radius: 0.0,
                color_top: [0.0; 4],
                color_bottom: [0.0; 4],
                border_color: [0.0; 4],
                border_width: 0.0,
                accent: [0.0; 4],
                opacity: 1.0,
            },
            runs: Vec::new(),
            hit_rect_logical: [0.0, 0.0, 100.0, 100.0],
            dismiss_rect_logical: [0.0; 4],
            button_quads: Vec::new(),
            button_rects_logical: Vec::new(),
        };
        let mut newer = older.clone();
        newer.id = 2;
        newer.hit_rect_logical = [50.0, 50.0, 150.0, 150.0];
        let outputs = vec![older, newer];
        // A point inside both rects must resolve to the newer toast (id 2).
        assert_eq!(topmost_hit(&outputs, egui::pos2(75.0, 75.0)), Some(2));
        // A point only in the older rect resolves to it.
        assert_eq!(topmost_hit(&outputs, egui::pos2(10.0, 10.0)), Some(1));
    }

    fn rect_center(rect: [f32; 4]) -> egui::Pos2 {
        egui::pos2(rect[0].midpoint(rect[2]), rect[1].midpoint(rect[3]))
    }

    #[test]
    fn click_target_dismiss_rect_wins_over_body() {
        let outputs = single_toast_outputs(7);
        let dismiss_center = rect_center(outputs[0].dismiss_rect_logical);
        assert_eq!(
            click_target(&outputs, dismiss_center),
            Some((7, ToastClickPart::Dismiss))
        );
    }

    #[test]
    fn click_target_body_elsewhere_in_the_pill() {
        let outputs = single_toast_outputs(7);
        let pill_center = rect_center(outputs[0].hit_rect_logical);
        assert!(!rect_contains(outputs[0].dismiss_rect_logical, pill_center));
        assert_eq!(
            click_target(&outputs, pill_center),
            Some((7, ToastClickPart::Body))
        );
    }

    #[test]
    fn click_target_outside_every_pill_is_none() {
        let outputs = single_toast_outputs(7);
        let pill = &outputs[0].pill;
        assert_eq!(
            click_target(&outputs, egui::pos2(pill.x - 50.0, pill.y - 50.0)),
            None
        );
    }

    #[test]
    fn click_target_uses_topmost_toast_precedence() {
        // Newer toast overlaps the older one's dismiss square: the newer
        // toast's body wins, not the older toast's dismiss.
        let base = single_toast_outputs(1).remove(0);
        let mut older = base.clone();
        older.hit_rect_logical = [0.0, 0.0, 100.0, 100.0];
        older.dismiss_rect_logical = [70.0, 10.0, 90.0, 30.0];
        let mut newer = base;
        newer.id = 2;
        newer.hit_rect_logical = [50.0, 0.0, 150.0, 100.0];
        newer.dismiss_rect_logical = [130.0, 10.0, 150.0, 30.0];
        let outputs = vec![older, newer];
        assert_eq!(
            click_target(&outputs, egui::pos2(80.0, 20.0)),
            Some((2, ToastClickPart::Body))
        );
        assert_eq!(
            click_target(&outputs, egui::pos2(10.0, 20.0)),
            Some((1, ToastClickPart::Body))
        );
    }

    #[test]
    fn click_target_prefers_dismiss_over_a_button_and_a_button_over_the_body() {
        let buttons = ["A", "B", "C"]
            .iter()
            .map(|l| button_input(l, 40.0))
            .collect();
        let mut outputs = vec![buttons_output(buttons, false, SETTLED_ANIM, 1.0)];

        let [first, second, third] =
            [0, 1, 2].map(|i| rect_center(outputs[0].button_rects_logical[i]));
        assert_eq!(
            click_target(&outputs, first),
            Some((3, ToastClickPart::Button(1)))
        );
        assert_eq!(
            click_target(&outputs, second),
            Some((3, ToastClickPart::Button(2)))
        );
        assert_eq!(
            click_target(&outputs, third),
            Some((3, ToastClickPart::Button(3)))
        );

        // The pill's top-left padding is body, not a button.
        let hit = outputs[0].hit_rect_logical;
        let body = egui::pos2(hit[0] + 2.0, hit[1] + 2.0);
        assert_eq!(
            click_target(&outputs, body),
            Some((3, ToastClickPart::Body))
        );
        // The gap between two buttons is body too.
        let gap = egui::pos2(
            outputs[0].button_rects_logical[0][2].midpoint(outputs[0].button_rects_logical[1][0]),
            second.y,
        );
        assert_eq!(click_target(&outputs, gap), Some((3, ToastClickPart::Body)));

        // Force a button onto the dismiss square: dismiss wins.
        outputs[0].button_rects_logical[0] = outputs[0].dismiss_rect_logical;
        let dismiss = rect_center(outputs[0].dismiss_rect_logical);
        assert_eq!(
            click_target(&outputs, dismiss),
            Some((3, ToastClickPart::Dismiss))
        );
    }
}
