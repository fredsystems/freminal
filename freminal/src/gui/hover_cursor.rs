// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Hover-cursor affordances for freminal's egui chrome.
//!
//! egui does not change the mouse cursor for clickable widgets on its own: a
//! `Button`, `SelectableLabel` or menu entry shows the ordinary arrow unless
//! the caller explicitly asks otherwise via [`egui::Response::on_hover_cursor`].
//! Without that, nothing in the menu bar, tab strip, settings window or any
//! modal tells the user an element can be clicked (issue #493).
//!
//! This module provides the vocabulary for saying so once per widget, rather
//! than repeating a bare `CursorIcon` at every call site. Naming the *intent*
//! ("this is a clickable affordance") instead of the *appearance* ("pointing
//! hand") keeps the choice of icon in one place.
//!
//! # Scope
//!
//! These helpers are for **chrome only**. The terminal surface resolves its own
//! cursor through `PointerHover`/`PointerTarget` in
//! `crate::gui::terminal::widget`, which additionally has to arbitrate against
//! the application's OSC 22 pointer shape. Do not use these there.
//!
//! # What deliberately does not get a hand
//!
//! - **Text fields.** egui already gives a `TextEdit` an I-beam, which
//!   correctly signals "type here" rather than "click here".
//! - **Drag handles.** Pane dividers use a directional resize cursor so the
//!   drag axis is visible; see `SplitBorderHover`.
//! - **Disabled widgets.** A pointing hand on something that cannot be
//!   activated is a lie. Use [`HoverAffordance::disabled_affordance`].

use egui::{CursorIcon, Response};

/// Attaches a hover cursor describing what a chrome widget does.
///
/// Deliberately not `#[must_use]`. `egui::Response` is not `must_use` itself,
/// and many of the widgets these methods decorate are legitimately used as
/// bare statements (`ui.checkbox(&mut flag, "Label").clickable();` -- the flag
/// is mutated in place and the response is not needed). A `must_use` here
/// would fire on every one of those and teach readers to ignore it.
#[allow(clippy::return_self_not_must_use)]
pub trait HoverAffordance {
    /// Mark this widget as a clickable affordance: buttons, menu entries,
    /// tab labels, checkboxes, radio buttons, links, and anything else that
    /// performs an action when clicked.
    fn clickable(self) -> Self;

    /// Mark this widget as present but not currently actionable, so the
    /// cursor says "no" instead of inviting a click that will do nothing.
    fn disabled_affordance(self) -> Self;

    /// Mark this widget as clickable only when `enabled` is true, otherwise as
    /// disabled.
    ///
    /// Convenience for the common `add_enabled(cond, ...)` pattern, so callers
    /// do not have to branch at the call site.
    fn clickable_when(self, enabled: bool) -> Self;

    /// Mark this widget as a draggable handle: open hand on hover, closed
    /// hand while actually being dragged.
    ///
    /// For controls whose primary interaction is picking something up and
    /// moving it -- sliders, in practice. Deliberately distinct from the
    /// directional resize arrow used by pane dividers, where the drag *axis*
    /// is the useful information, and from `egui::DragValue`'s
    /// `ResizeEast`/`ResizeWest` arrows, which additionally signal when the
    /// value is clamped at a limit.
    fn draggable(self) -> Self;
}

impl HoverAffordance for Response {
    fn clickable(self) -> Self {
        self.on_hover_cursor(CursorIcon::PointingHand)
    }

    fn disabled_affordance(self) -> Self {
        self.on_hover_cursor(CursorIcon::NotAllowed)
    }

    fn clickable_when(self, enabled: bool) -> Self {
        if enabled {
            self.clickable()
        } else {
            self.disabled_affordance()
        }
    }

    fn draggable(self) -> Self {
        if self.dragged() {
            // `on_hover_cursor` only fires while the pointer is inside the
            // widget, and a drag routinely travels outside it -- past the end
            // of a slider's rail, most obviously. Write directly so the closed
            // hand survives the whole gesture.
            self.ctx.set_cursor_icon(CursorIcon::Grabbing);
            self
        } else {
            self.on_hover_cursor(CursorIcon::Grab)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::HoverAffordance;
    use egui::{CursorIcon, Event, Pos2, RawInput, Rect, Sense, Vec2};

    /// Run egui frames with the pointer parked over a widget decorated by
    /// `add`, and report the cursor the last frame requested.
    ///
    /// A real frame with a real pointer is required: `on_hover_cursor` only
    /// applies while the widget is hovered, so there is nothing to observe
    /// otherwise. Two passes because egui resolves hover against the previous
    /// frame's widget rects -- the first registers the widget, the second is
    /// the frame where it is hovered.
    fn cursor_over(add: impl Fn(egui::Response) -> egui::Response) -> CursorIcon {
        let ctx = egui::Context::default();
        let mut input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(300.0, 300.0))),
            ..Default::default()
        };
        input
            .events
            .push(Event::PointerMoved(Pos2::new(50.0, 50.0)));

        let mut icon = CursorIcon::Default;
        for _ in 0..2 {
            let mut out = ctx.run_ui(input.clone(), |ui| {
                // Large enough to contain the pointer wherever the layout
                // starts it.
                let response =
                    ui.allocate_response(Vec2::new(250.0, 250.0), Sense::click_and_drag());
                let _ = add(response);
            });
            icon = out.platform_output.cursor_icon;
            // epaint asserts on a dropped `TexturesDelta` with unapplied
            // entries; nothing here rasterises, so discard them explicitly.
            out.textures_delta.clear();
        }
        icon
    }

    /// Guards the harness itself: an undecorated widget leaves the cursor
    /// alone, so the assertions below are measuring the affordance and not an
    /// egui default.
    #[test]
    fn an_undecorated_widget_leaves_the_cursor_alone() {
        assert_eq!(cursor_over(|r| r), CursorIcon::Default);
    }

    #[test]
    fn clickable_asks_for_the_pointing_hand() {
        assert_eq!(
            cursor_over(HoverAffordance::clickable),
            CursorIcon::PointingHand
        );
    }

    /// A disabled control must actively say it cannot be used, rather than
    /// merely omitting the hand and looking inert.
    #[test]
    fn disabled_affordance_asks_for_not_allowed() {
        assert_eq!(
            cursor_over(HoverAffordance::disabled_affordance),
            CursorIcon::NotAllowed
        );
    }

    #[test]
    fn clickable_when_picks_the_matching_affordance() {
        assert_eq!(
            cursor_over(|r| r.clickable_when(true)),
            CursorIcon::PointingHand
        );
        assert_eq!(
            cursor_over(|r| r.clickable_when(false)),
            CursorIcon::NotAllowed
        );
    }

    /// Hovering a drag handle offers the open hand. The closed-hand branch
    /// needs an in-flight drag, which this harness does not simulate; it is
    /// covered by the `dragged()` arm being the only other path.
    #[test]
    fn draggable_offers_the_open_hand_on_hover() {
        assert_eq!(cursor_over(HoverAffordance::draggable), CursorIcon::Grab);
    }
}
