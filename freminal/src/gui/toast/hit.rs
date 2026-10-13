// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Toast hit-testing, click targeting, and button hover/pressed state.

use std::time::Instant;

use super::ToastStack;
use super::buttons::{ButtonState, button_fill};
use super::layout::ToastLayoutOutput;
use crate::gui::hover_cursor::HoverAffordance;

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

/// A toast part together with the toast's id: what a click can land on.
pub(super) type ToastTarget = (u64, ToastClickPart);

/// Whether the primary pointer button is held at the start of this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PrimaryButton {
    /// Not held (a button released this frame counts as not held).
    Up,
    /// Held.
    Down,
}

/// One frame's pointer state, in logical points (window space), as the toast
/// interaction needs it. Read once per frame with [`Self::read`] so button
/// fills, press tracking and click resolution all see the same values.
#[derive(Debug, Clone, Copy)]
pub(super) struct ToastPointer {
    /// Where the pointer is hovering, if it is over the window.
    pub hover: Option<egui::Pos2>,
    /// The best-known pointer position for resolving a click: the hover
    /// position, else egui's interaction position, else the latest known one.
    /// egui clears the hover position when the pointer leaves the window (a
    /// touch release does the same), which would otherwise lose a release
    /// that is already known to have happened over a toast.
    pub click_pos: Option<egui::Pos2>,
    /// Where a primary press happened this frame, from the frame's own
    /// events. egui clears [`Self::press_origin`] on release, so a press and
    /// release in one frame is only visible here.
    pub pressed_now: Option<egui::Pos2>,
    /// Where the held primary press began (egui's `press_origin`).
    pub press_origin: Option<egui::Pos2>,
    /// Whether the primary button is held.
    pub primary: PrimaryButton,
}

impl ToastPointer {
    /// Read the current frame's pointer state from `ctx`.
    pub(super) fn read(ctx: &egui::Context) -> Self {
        ctx.input(|input| {
            let pointer = &input.pointer;
            let pressed_now = input.events.iter().find_map(|event| match event {
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    ..
                } => Some(*pos),
                _ => None,
            });
            Self {
                hover: pointer.hover_pos(),
                click_pos: pointer
                    .hover_pos()
                    .or_else(|| pointer.interact_pos())
                    .or_else(|| pointer.latest_pos()),
                pressed_now,
                press_origin: pointer.press_origin(),
                primary: if pointer.primary_down() {
                    PrimaryButton::Down
                } else {
                    PrimaryButton::Up
                },
            }
        })
    }
}

/// Whether the pointer is over a toast. See [`ToastStack::interact`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PointerOver {
    /// Over the pill of some laid-out toast.
    Toast,
    /// Over no toast (or not over the window).
    Nothing,
}

impl ToastStack {
    /// Run one frame of pointer interaction against the laid-out `outputs`:
    /// read the pointer once, track the press, recolour the hovered/pressed
    /// button in `outputs`, then hit-test (hover hold, cursor, click).
    ///
    /// Returns whether the pointer is over a toast, as of the layout the
    /// frame was drawn with (a click may remove that toast afterwards).
    ///
    /// The order is load-bearing: a press this frame must already paint as
    /// pressed, and the fills must be in `outputs` before the caller builds
    /// the quads to paint. Split out of [`Self::show`] to keep it under the
    /// line-count limit.
    pub(super) fn interact(
        &mut self,
        ctx: &egui::Context,
        outputs: &mut [ToastLayoutOutput],
        now: Instant,
    ) -> PointerOver {
        let pointer = ToastPointer::read(ctx);
        self.track_press(&pointer, outputs);
        self.apply_button_states(outputs, &pointer);
        let over = if pointer
            .hover
            .is_some_and(|pos| topmost_hit(outputs, pos).is_some())
        {
            PointerOver::Toast
        } else {
            PointerOver::Nothing
        };
        self.hit_test(ctx, outputs, &pointer, now);
        over
    }

    /// Remember which toast part the primary button was pressed on, so a
    /// click is only acted on when the release lands on that same part.
    ///
    /// A press seen this frame is resolved against this frame's layout. A
    /// press that began in an earlier frame was resolved then; if none was
    /// (the toast was not laid out yet), `press_origin` is the fallback.
    fn track_press(&mut self, pointer: &ToastPointer, outputs: &[ToastLayoutOutput]) {
        if let Some(pos) = pointer.pressed_now {
            self.press = click_target(outputs, pos);
        } else if self.press.is_none()
            && let Some(origin) = pointer.press_origin
        {
            self.press = click_target(outputs, origin);
        }
    }

    /// Recolour each button under the pointer for its hover or pressed state
    /// (see [`button_state`]); every other button keeps the fill layout gave
    /// it. Must run after [`Self::track_press`] and before the quads are
    /// painted.
    fn apply_button_states(&self, outputs: &mut [ToastLayoutOutput], pointer: &ToastPointer) {
        let under_pointer = pointer.hover.and_then(|pos| click_target(outputs, pos));
        for out in outputs {
            let (id, tint) = (out.id, out.button_tint);
            let (pill_top, pill_bottom) = (out.pill.color_top, out.pill.color_bottom);
            for (quad, n) in out.button_quads.iter_mut().zip(1_u8..) {
                let state = button_state(under_pointer, self.press, pointer.primary, (id, n));
                if state != ButtonState::Normal {
                    (quad.color_top, quad.color_bottom) =
                        button_fill(pill_top, pill_bottom, tint, state);
                }
            }
        }
    }

    /// Hover/click hit-test: allocate one interactive click-only egui region
    /// per toast, over its `hit_rect_logical` (logical points). Hovering a
    /// toast's pill pauses its expiry (`last_hovered`) and shows the pointing
    /// hand over the whole pill (every part of a toast acts on click, so the
    /// whole pill is a clickable affordance). A primary click removes the
    /// toast; for a keyed toast the click is also classified by
    /// [`click_target`] (dismiss square, button, or body) and reported as
    /// events via [`Self::apply_click`].
    ///
    /// A toast that is fading out after [`Self::close_keyed`] is not held by
    /// hover and its click is ignored (the click is still consumed, so the
    /// terminal beneath never sees it).
    ///
    /// A click is acted on only when the release lands on the same part the
    /// press began on (`self.press`, see [`Self::track_press`]): pressing on a
    /// button and releasing on the body, or on another button, does nothing
    /// (egui still consumes it, so the terminal never sees it).
    ///
    /// The regions live in an interactive `egui::Area` above the terminal so
    /// egui consumes the click (see the call site's finding-#3 comment). At
    /// most one toast is dismissed per frame, and regions are hit-tested
    /// newest-first (newest toasts are drawn on top) so that during the brief
    /// entry slide — when an arriving toast can transiently overlap its
    /// neighbour — a click in the overlap dismisses only the topmost one
    /// (review finding #4). Removes the dismissed toast itself. Split out of
    /// [`Self::show`] to keep it under the line-count limit.
    ///
    /// # Cursor
    ///
    /// The pointing hand is `HoverAffordance::clickable` on each toast
    /// region's response, the sanctioned path for hand-rolled
    /// `allocate_rect` widgets (see the `freminal-cursor-affordances` skill).
    /// This does not add a second `output.cursor_icon` writer: the terminal
    /// pane abstains whenever `rect_contains_pointer` says a higher layer owns
    /// the pointer, and these regions are interactive `Foreground` areas.
    fn hit_test(
        &mut self,
        ctx: &egui::Context,
        outputs: &[ToastLayoutOutput],
        pointer: &ToastPointer,
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
                    ui.allocate_rect(rect, egui::Sense::click())
                        .clickable()
                        .clicked()
                })
                .inner;
            if clicked {
                any_clicked = true;
            }
        }

        if let Some(pos) = pointer.hover
            && let Some(id) = topmost_hit(outputs, pos)
            && let Some(toast) = self
                .entries
                .iter_mut()
                .find(|t| t.id == id && t.closing.is_none())
        {
            toast.last_hovered = Some(now);
        }
        if any_clicked
            && let Some(pos) = pointer.click_pos
            && let Some((id, part)) = click_target(outputs, pos)
            && self.press == Some((id, part))
        {
            self.apply_click(id, part);
        }
        // The press ends with the button: whatever happened above, the next
        // frame starts without one.
        if pointer.primary == PrimaryButton::Up {
            self.press = None;
        }
    }
}

/// How button `n` of toast `id` looks, given what is under the pointer
/// (`under_pointer`), where the current press began (`press`), and whether the
/// primary button is held.
///
/// A button is `Normal` unless the pointer is over it. Over it, it is
/// `Pressed` while held down if the press began on it, `Normal` while held
/// down if the press began elsewhere (egui buttons do not hover-light while
/// something else is being pressed), and `Hovered` otherwise.
pub(super) fn button_state(
    under_pointer: Option<ToastTarget>,
    press: Option<ToastTarget>,
    primary: PrimaryButton,
    (id, n): (u64, u8),
) -> ButtonState {
    let this = Some((id, ToastClickPart::Button(n)));
    if under_pointer != this {
        return ButtonState::Normal;
    }
    match (primary, press) {
        (PrimaryButton::Down, held) if held == this => ButtonState::Pressed,
        (PrimaryButton::Down, Some(_)) => ButtonState::Normal,
        (PrimaryButton::Down | PrimaryButton::Up, _) => ButtonState::Hovered,
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
pub(super) fn topmost_hit(outputs: &[ToastLayoutOutput], pos: egui::Pos2) -> Option<u64> {
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
pub(super) fn click_target(outputs: &[ToastLayoutOutput], pos: egui::Pos2) -> Option<ToastTarget> {
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
    use crate::gui::toast::buttons::ButtonTint;
    use crate::gui::toast::test_support::{
        DetailLine, SETTLED_ANIM, button_input, buttons_output, single_toast_outputs,
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
            button_tint: ButtonTint::Lighten,
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
        let mut outputs = vec![buttons_output(
            buttons,
            DetailLine::Absent,
            SETTLED_ANIM,
            1.0,
        )];

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

    // -----------------------------------------------------------------
    //  138.C14: button state, press tracking, headless egui interaction
    // -----------------------------------------------------------------

    use crate::gui::toast::ToastPosition;
    use crate::gui::toast::buttons::{ButtonState, button_fill};
    use crate::gui::toast::input::ToastLayoutInput;
    use crate::gui::toast::layout::{ToastGeometry, layout_toasts};
    use crate::gui::toast::test_support::{test_input, test_window_rect};
    use crate::gui::toast::{
        ToastContent, ToastEvent, ToastEventKind, ToastKey, ToastKind, ToastLifetime,
        ToastPlacement,
    };

    const BUTTON_1: ToastTarget = (3, ToastClickPart::Button(1));
    const BUTTON_2: ToastTarget = (3, ToastClickPart::Button(2));

    #[test]
    fn button_state_is_normal_unless_the_pointer_is_over_the_button() {
        for primary in [PrimaryButton::Up, PrimaryButton::Down] {
            assert_eq!(
                button_state(None, None, primary, (3, 1)),
                ButtonState::Normal
            );
            assert_eq!(
                button_state(Some(BUTTON_2), Some(BUTTON_2), primary, (3, 1)),
                ButtonState::Normal,
                "over a different button"
            );
            assert_eq!(
                button_state(Some((4, ToastClickPart::Button(1))), None, primary, (3, 1)),
                ButtonState::Normal,
                "over the same button number of a different toast"
            );
            assert_eq!(
                button_state(Some((3, ToastClickPart::Body)), None, primary, (3, 1)),
                ButtonState::Normal,
                "over the body"
            );
        }
    }

    #[test]
    fn button_state_hovered_pressed_and_pressed_elsewhere() {
        assert_eq!(
            button_state(Some(BUTTON_1), None, PrimaryButton::Up, (3, 1)),
            ButtonState::Hovered
        );
        assert_eq!(
            button_state(Some(BUTTON_1), Some(BUTTON_1), PrimaryButton::Down, (3, 1)),
            ButtonState::Pressed
        );
        // Released: the stale press does not keep the pressed look.
        assert_eq!(
            button_state(Some(BUTTON_1), Some(BUTTON_1), PrimaryButton::Up, (3, 1)),
            ButtonState::Hovered
        );
        // Dragged onto this button from another part: no hover-light.
        assert_eq!(
            button_state(Some(BUTTON_1), Some(BUTTON_2), PrimaryButton::Down, (3, 1)),
            ButtonState::Normal
        );
        assert_eq!(
            button_state(
                Some(BUTTON_1),
                Some((3, ToastClickPart::Body)),
                PrimaryButton::Down,
                (3, 1)
            ),
            ButtonState::Normal
        );
    }

    fn pointer_at(pos: Option<egui::Pos2>, primary: PrimaryButton) -> ToastPointer {
        ToastPointer {
            hover: pos,
            click_pos: pos,
            pressed_now: None,
            press_origin: None,
            primary,
        }
    }

    fn three_button_outputs() -> Vec<ToastLayoutOutput> {
        let buttons = ["A", "B", "C"].iter().map(|l| button_input(l, 40.0));
        vec![buttons_output(
            buttons.collect(),
            DetailLine::Absent,
            SETTLED_ANIM,
            1.0,
        )]
    }

    #[test]
    fn apply_button_states_recolours_only_the_button_under_the_pointer() {
        let stack = ToastStack::default();
        let normal = three_button_outputs();
        let mut outputs = normal.clone();
        let over_second = rect_center(outputs[0].button_rects_logical[1]);

        stack.apply_button_states(
            &mut outputs,
            &pointer_at(Some(over_second), PrimaryButton::Up),
        );

        let pill = &outputs[0].pill;
        let tint = outputs[0].button_tint;
        let hovered = button_fill(
            pill.color_top,
            pill.color_bottom,
            tint,
            ButtonState::Hovered,
        );
        assert_eq!(
            (
                outputs[0].button_quads[1].color_top,
                outputs[0].button_quads[1].color_bottom
            ),
            hovered
        );
        assert_ne!(
            outputs[0].button_quads[1].color_top,
            normal[0].button_quads[1].color_top
        );
        for i in [0, 2] {
            assert_eq!(
                outputs[0].button_quads[i].color_top, normal[0].button_quads[i].color_top,
                "button {i} is untouched"
            );
        }
    }

    #[test]
    fn apply_button_states_shows_pressed_only_while_held_on_the_pressed_button() {
        let mut stack = ToastStack::default();
        let base = three_button_outputs();
        let first = rect_center(base[0].button_rects_logical[0]);
        let body = egui::pos2(
            base[0].hit_rect_logical[0] + 2.0,
            base[0].hit_rect_logical[1] + 2.0,
        );
        let pressed = |outputs: &[ToastLayoutOutput]| {
            let pill = &outputs[0].pill;
            button_fill(
                pill.color_top,
                pill.color_bottom,
                outputs[0].button_tint,
                ButtonState::Pressed,
            )
            .0
        };

        stack.press = Some(BUTTON_1);
        let mut held = base.clone();
        stack.apply_button_states(&mut held, &pointer_at(Some(first), PrimaryButton::Down));
        assert_eq!(held[0].button_quads[0].color_top, pressed(&held));

        // Dragged off the button while held: back to the normal fill.
        let mut off = base.clone();
        stack.apply_button_states(&mut off, &pointer_at(Some(body), PrimaryButton::Down));
        assert_eq!(
            off[0].button_quads[0].color_top,
            base[0].button_quads[0].color_top
        );
    }

    #[test]
    fn track_press_resolves_a_press_this_frame_and_overrides_an_old_one() {
        let outputs = three_button_outputs();
        let first = rect_center(outputs[0].button_rects_logical[0]);
        let second = rect_center(outputs[0].button_rects_logical[1]);
        let mut stack = ToastStack::default();

        let mut pointer = pointer_at(Some(first), PrimaryButton::Up);
        pointer.pressed_now = Some(first);
        stack.track_press(&pointer, &outputs);
        assert_eq!(stack.press, Some(BUTTON_1));

        pointer.pressed_now = Some(second);
        stack.track_press(&pointer, &outputs);
        assert_eq!(stack.press, Some(BUTTON_2), "a new press replaces the old");
    }

    #[test]
    fn track_press_falls_back_to_the_press_origin_only_without_a_recorded_press() {
        let outputs = three_button_outputs();
        let first = rect_center(outputs[0].button_rects_logical[0]);
        let second = rect_center(outputs[0].button_rects_logical[1]);
        let mut stack = ToastStack::default();

        let mut pointer = pointer_at(Some(second), PrimaryButton::Down);
        pointer.press_origin = Some(first);
        stack.track_press(&pointer, &outputs);
        assert_eq!(
            stack.press,
            Some(BUTTON_1),
            "origin, not the hover position"
        );

        pointer.press_origin = Some(second);
        stack.track_press(&pointer, &outputs);
        assert_eq!(
            stack.press,
            Some(BUTTON_1),
            "a recorded press is not re-resolved"
        );

        // A press that began outside every toast records nothing.
        let mut stack = ToastStack::default();
        pointer.press_origin = Some(egui::pos2(-50.0, -50.0));
        stack.track_press(&pointer, &outputs);
        assert_eq!(stack.press, None);
    }

    /// A headless egui context driving the real [`ToastStack::interact`]
    /// against a laid-out toast, with the pointer injected through
    /// `RawInput` events.
    struct Harness {
        ctx: egui::Context,
        stack: ToastStack,
        /// The layout as laid out, before any hover/pressed recolouring.
        base: Vec<ToastLayoutOutput>,
        /// The outputs after the last frame's `interact`.
        drawn: Vec<ToastLayoutOutput>,
        /// What the last frame's `interact` reported.
        over: PointerOver,
        time: f64,
    }

    impl Harness {
        fn laid_out(stack: ToastStack, button_labels: &[&str]) -> Self {
            let id = stack.entries[0].id;
            let input = ToastLayoutInput {
                buttons: button_labels
                    .iter()
                    .map(|l| button_input(l, 40.0))
                    .collect(),
                ..test_input(id, 100.0, DetailLine::Absent, SETTLED_ANIM)
            };
            let base = layout_toasts(
                &[input],
                ToastPosition::TopRightStack,
                ToastGeometry {
                    content_rect: test_window_rect(),
                    active_pane_rect: None,
                },
                &egui::Visuals::dark(),
                1.0,
            );
            Self {
                ctx: egui::Context::default(),
                stack,
                drawn: base.clone(),
                base,
                over: PointerOver::Nothing,
                time: 0.0,
            }
        }

        /// A keyed, never-expiring toast with `button_labels`.
        fn keyed(button_labels: &[&str]) -> Self {
            let mut stack = ToastStack::default();
            stack.push_keyed(
                ToastKey(9),
                ToastContent {
                    kind: ToastKind::Info,
                    title: "t".to_owned(),
                    detail: None,
                    buttons: button_labels.iter().map(|l| (*l).to_owned()).collect(),
                    icon: None,
                },
                ToastLifetime::UntilDismissed,
                ToastPlacement::TOP_RIGHT,
            );
            Self::laid_out(stack, button_labels)
        }

        fn unkeyed() -> Self {
            let mut stack = ToastStack::default();
            stack.info("t", None);
            Self::laid_out(stack, &[])
        }

        /// Run one frame with `events`; returns the cursor it asked for.
        fn frame(&mut self, events: Vec<egui::Event>) -> egui::CursorIcon {
            self.time += 0.05;
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800.0, 600.0),
                )),
                time: Some(self.time),
                events,
                ..Default::default()
            };
            self.drawn = self.base.clone();
            let (stack, drawn, over) = (&mut self.stack, &mut self.drawn, &mut self.over);
            let mut out = self.ctx.run_ui(input, |ui| {
                *over = stack.interact(ui.ctx(), drawn, Instant::now());
            });
            let icon = out.platform_output.cursor_icon;
            // epaint asserts on a dropped `TexturesDelta` with unapplied
            // entries; nothing here rasterises.
            out.textures_delta.clear();
            icon
        }

        /// Move the pointer to `pos` and settle. egui resolves hover against
        /// the previous frame's widget rects, and an `Area` only counts as a
        /// visible layer from its second frame, so it takes a few frames.
        fn move_to(&mut self, pos: egui::Pos2) -> egui::CursorIcon {
            self.frame(vec![egui::Event::PointerMoved(pos)]);
            self.frame(Vec::new());
            self.frame(Vec::new())
        }

        fn button_event(pos: egui::Pos2, pressed: bool) -> egui::Event {
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            }
        }

        fn press(&mut self, pos: egui::Pos2) {
            self.frame(vec![Self::button_event(pos, true)]);
        }

        fn release(&mut self, pos: egui::Pos2) {
            self.frame(vec![Self::button_event(pos, false)]);
        }

        fn button_center(&self, index: usize) -> egui::Pos2 {
            rect_center(self.base[0].button_rects_logical[index])
        }

        fn dismiss_center(&self) -> egui::Pos2 {
            rect_center(self.base[0].dismiss_rect_logical)
        }

        /// A point on the pill that is neither the dismiss square nor a button.
        fn body_point(&self) -> egui::Pos2 {
            let hit = self.base[0].hit_rect_logical;
            egui::pos2(hit[0] + 3.0, hit[1] + 3.0)
        }

        fn events(&mut self) -> Vec<ToastEvent> {
            self.stack.take_events()
        }
    }

    fn event(kind: ToastEventKind) -> ToastEvent {
        ToastEvent {
            key: ToastKey(9),
            kind,
        }
    }

    #[test]
    fn the_harness_geometry_has_distinct_body_dismiss_and_button_points() {
        let h = Harness::keyed(&["A", "B"]);
        let target = |pos| click_target(&h.base, pos).map(|(_, part)| part);
        assert_eq!(target(h.body_point()), Some(ToastClickPart::Body));
        assert_eq!(target(h.dismiss_center()), Some(ToastClickPart::Dismiss));
        assert_eq!(target(h.button_center(0)), Some(ToastClickPart::Button(1)));
        assert_eq!(target(h.button_center(1)), Some(ToastClickPart::Button(2)));
    }

    #[test]
    fn a_click_on_a_button_reports_that_button_then_closed() {
        let mut h = Harness::keyed(&["A", "B"]);
        let target = h.button_center(1);
        h.move_to(target);
        h.press(target);
        assert_eq!(h.events(), Vec::new(), "nothing happens on press");
        h.release(target);

        assert_eq!(
            h.events(),
            vec![
                event(ToastEventKind::Button(2)),
                event(ToastEventKind::Closed)
            ]
        );
        assert!(h.stack.is_empty());
    }

    #[test]
    fn a_click_on_the_dismiss_square_is_closed_only() {
        let mut h = Harness::keyed(&["A"]);
        let target = h.dismiss_center();
        h.move_to(target);
        h.press(target);
        h.release(target);

        assert_eq!(h.events(), vec![event(ToastEventKind::Closed)]);
        assert!(h.stack.is_empty());
    }

    #[test]
    fn a_click_on_the_body_is_activated_then_closed() {
        let mut h = Harness::keyed(&["A"]);
        let target = h.body_point();
        h.move_to(target);
        h.press(target);
        h.release(target);

        assert_eq!(
            h.events(),
            vec![
                event(ToastEventKind::Activated),
                event(ToastEventKind::Closed)
            ]
        );
    }

    /// A point just inside button `index`'s right edge, and one just outside
    /// it (in the gap or pill padding, so the body). They are 3 points apart:
    /// close enough that egui still calls press-then-release a click, which is
    /// the case where only the toast's own press/release check can reject it.
    fn right_edge_of_button(h: &Harness, index: usize) -> (egui::Pos2, egui::Pos2) {
        let rect = h.base[0].button_rects_logical[index];
        let y = rect[1].midpoint(rect[3]);
        (egui::pos2(rect[2] - 1.0, y), egui::pos2(rect[2] + 2.0, y))
    }

    #[test]
    fn pressing_on_a_button_and_releasing_just_off_it_does_nothing() {
        let mut h = Harness::keyed(&["A", "B"]);
        let (on_button, off_button) = right_edge_of_button(&h, 0);
        assert_eq!(
            click_target(&h.base, on_button),
            Some((h.base[0].id, ToastClickPart::Button(1)))
        );
        assert_eq!(
            click_target(&h.base, off_button),
            Some((h.base[0].id, ToastClickPart::Body)),
            "premise: the release point is the body"
        );
        h.move_to(on_button);
        h.press(on_button);
        h.frame(vec![egui::Event::PointerMoved(off_button)]);
        h.release(off_button);

        assert_eq!(h.events(), Vec::new(), "no action on a mismatched release");
        assert_eq!(h.stack.len(), 1, "the toast stays");
        assert_eq!(h.stack.press, None, "and the press is forgotten");
    }

    #[test]
    fn pressing_on_the_body_and_releasing_just_on_a_button_does_nothing() {
        let mut h = Harness::keyed(&["A", "B"]);
        let (on_button, off_button) = right_edge_of_button(&h, 0);
        h.move_to(off_button);
        h.press(off_button);
        h.frame(vec![egui::Event::PointerMoved(on_button)]);
        h.release(on_button);

        assert_eq!(h.events(), Vec::new());
        assert_eq!(h.stack.len(), 1);
    }

    #[test]
    fn a_click_that_stays_on_one_button_across_a_small_move_still_counts() {
        let mut h = Harness::keyed(&["A", "B"]);
        let centre = h.button_center(0);
        let (from, to) = (centre - egui::vec2(2.0, 0.0), centre + egui::vec2(2.0, 0.0));
        h.move_to(from);
        h.press(from);
        h.frame(vec![egui::Event::PointerMoved(to)]);
        h.release(to);

        assert_eq!(
            h.events(),
            vec![
                event(ToastEventKind::Button(1)),
                event(ToastEventKind::Closed)
            ]
        );
    }

    #[test]
    fn pressing_on_one_button_and_releasing_on_another_does_nothing() {
        let mut h = Harness::keyed(&["A", "B"]);
        let (first, second) = (h.button_center(0), h.button_center(1));
        h.move_to(first);
        h.press(first);
        h.frame(vec![egui::Event::PointerMoved(second)]);
        h.release(second);

        assert_eq!(h.events(), Vec::new());
        assert_eq!(h.stack.len(), 1);
    }

    #[test]
    fn a_press_and_release_in_one_frame_still_clicks_the_pressed_part() {
        let mut h = Harness::keyed(&["A", "B"]);
        let target = h.button_center(0);
        h.move_to(target);
        // egui clears `press_origin` on release, so only the frame's own
        // events say where this press began.
        h.frame(vec![
            Harness::button_event(target, true),
            Harness::button_event(target, false),
        ]);

        assert_eq!(
            h.events(),
            vec![
                event(ToastEventKind::Button(1)),
                event(ToastEventKind::Closed)
            ]
        );
    }

    #[test]
    fn a_release_is_not_lost_when_the_pointer_leaves_the_window_with_it() {
        let mut h = Harness::keyed(&["A", "B"]);
        let target = h.button_center(0);
        h.move_to(target);
        h.press(target);
        // The release and the pointer leaving arrive in the same frame, so
        // `hover_pos()` is already `None` when the click is resolved.
        h.frame(vec![
            Harness::button_event(target, false),
            egui::Event::PointerGone,
        ]);

        assert_eq!(
            h.events(),
            vec![
                event(ToastEventKind::Button(1)),
                event(ToastEventKind::Closed)
            ]
        );
    }

    #[test]
    fn a_click_that_began_outside_every_toast_does_nothing() {
        let mut h = Harness::keyed(&["A"]);
        let target = h.button_center(0);
        let outside = egui::pos2(5.0, 500.0);
        h.move_to(outside);
        h.press(outside);
        h.frame(vec![egui::Event::PointerMoved(target)]);
        h.release(target);

        assert_eq!(h.events(), Vec::new());
        assert_eq!(h.stack.len(), 1);
    }

    #[test]
    fn an_unkeyed_toast_is_removed_by_a_click_anywhere_with_no_events() {
        let mut h = Harness::unkeyed();
        let target = h.body_point();
        h.move_to(target);
        h.press(target);
        h.release(target);

        assert!(h.stack.is_empty());
        assert_eq!(h.events(), Vec::new());
    }

    #[test]
    fn the_pointer_is_a_hand_over_keyed_and_unkeyed_toasts_and_an_arrow_elsewhere() {
        for mut h in [Harness::keyed(&["A", "B"]), Harness::unkeyed()] {
            let body = h.body_point();
            assert_eq!(h.move_to(body), egui::CursorIcon::PointingHand, "body");
            let dismiss = h.dismiss_center();
            assert_eq!(
                h.move_to(dismiss),
                egui::CursorIcon::PointingHand,
                "dismiss"
            );
            assert_eq!(
                h.move_to(egui::pos2(5.0, 500.0)),
                egui::CursorIcon::Default,
                "away from the toast the hand is gone"
            );
        }
        let mut h = Harness::keyed(&["A", "B"]);
        for index in 0..2 {
            let button = h.button_center(index);
            assert_eq!(
                h.move_to(button),
                egui::CursorIcon::PointingHand,
                "button {index}"
            );
        }
    }

    #[test]
    fn the_button_fill_follows_hover_press_and_leave() {
        let mut h = Harness::keyed(&["A", "B"]);
        let (first, second) = (h.button_center(0), h.button_center(1));
        let fill_of = |h: &Harness, i: usize| h.drawn[0].button_quads[i].color_top;
        let normal = h.base[0].button_quads[0].color_top;

        h.frame(Vec::new());
        assert_eq!(fill_of(&h, 0), normal, "no pointer: normal");

        h.move_to(first);
        let hovered = fill_of(&h, 0);
        assert_ne!(hovered, normal, "hover changes the fill");
        assert_eq!(
            fill_of(&h, 1),
            h.base[0].button_quads[1].color_top,
            "the other button is normal"
        );

        h.press(first);
        let pressed = fill_of(&h, 0);
        assert_ne!(pressed, hovered, "pressed differs from hovered");
        assert_ne!(pressed, normal);

        // Dragged onto the other button while held: neither lights up.
        h.frame(vec![egui::Event::PointerMoved(second)]);
        assert_eq!(fill_of(&h, 0), normal);
        assert_eq!(fill_of(&h, 1), h.base[0].button_quads[1].color_top);

        h.release(second);
        h.move_to(egui::pos2(5.0, 500.0));
        assert_eq!(fill_of(&h, 0), normal, "left: back to normal");
    }

    #[test]
    fn interact_reports_whether_the_pointer_is_over_a_toast() {
        let mut h = Harness::keyed(&["A"]);
        h.frame(Vec::new());
        assert_eq!(h.over, PointerOver::Nothing, "no pointer yet");

        let body = h.body_point();
        h.move_to(body);
        assert_eq!(h.over, PointerOver::Toast);

        h.move_to(egui::pos2(5.0, 500.0));
        assert_eq!(h.over, PointerOver::Nothing);
    }

    #[test]
    fn hovering_a_toast_records_the_hover_time() {
        let mut h = Harness::keyed(&["A"]);
        assert_eq!(h.stack.entries[0].last_hovered, None);
        let target = h.body_point();
        h.move_to(target);
        assert!(h.stack.entries[0].last_hovered.is_some());
    }
}
