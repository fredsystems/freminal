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
}
