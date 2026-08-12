// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Live keyboard-modifier state for one window, mirrored from winit.
//!
//! # Why this exists
//!
//! `freminal-windowing` intercepts two narrow classes of keyboard event
//! *before* handing them to `egui-winit` — the Wayland paste work-around and
//! the Task 114 blocked-key routing (both in [`crate::event_loop`]). Both need
//! to know which modifiers are held **at the moment the intercepted key
//! arrives**.
//!
//! Up to egui 0.35.0 that state was readable from `egui-winit`'s
//! `State::egui_input().modifiers`. egui 0.36.0 removed `RawInput::modifiers`
//! (upstream #8336, "Remove `Modifiers` from `RawInput` and make it a
//! `egui::Event`") and moved the running state into a **private** field on
//! `egui_winit::State`, with no accessor. The remaining egui-side reader,
//! `Context::input(|i| i.modifiers)`, is not a substitute: it only advances
//! when a pass runs, so between passes it reports the modifier state of the
//! *previous* frame. A `Ctrl` press and the `V` that follows it can both
//! arrive inside one frame interval, which would make the paste intercept miss
//! the `Ctrl` and fall through to egui as a literal `v`.
//!
//! So this tracker mirrors the state itself, from the same winit events
//! `egui-winit` uses, applying the same rules.
//!
//! # Invariant
//!
//! [`ModifierTracker::current`] returns the modifiers implied by the most
//! recent [`WindowEvent::ModifiersChanged`] this window has received, and is
//! reset to "nothing held" by [`WindowEvent::Focused(false)`]. It is updated
//! by [`ModifierTracker::on_window_event`], which
//! [`crate::egui_integration::EguiWindow::on_window_event`] calls on every
//! event **before** forwarding it to `egui-winit` — so a read during an
//! interception path that runs after that forwarding always sees the current
//! event's effect, never a stale one.
//!
//! The focus reset mirrors `egui-winit` 0.36.1's own (`lib.rs`, the
//! `WindowEvent::Focused` arm): without it, alt-tabbing away while holding a
//! modifier leaves it stuck down, because the release arrives at whichever
//! window has focus by then — not this one.

use winit::event::WindowEvent;
use winit::keyboard::ModifiersState;

/// Mirror of `egui-winit`'s private running modifier state for one window.
///
/// See the module documentation for why this duplication exists and what
/// invariant it maintains.
#[derive(Debug, Clone, Copy, Default)]
pub struct ModifierTracker {
    modifiers: egui::Modifiers,
}

impl ModifierTracker {
    /// The modifiers currently held, as of the last
    /// [`WindowEvent::ModifiersChanged`] (or "none" since the last focus loss).
    pub(crate) const fn current(self) -> egui::Modifiers {
        self.modifiers
    }

    /// Update the tracked state from a window event.
    ///
    /// Ignores every event except [`WindowEvent::ModifiersChanged`] and
    /// [`WindowEvent::Focused`]; safe (and intended) to call unconditionally
    /// for every event the window receives.
    pub(crate) fn on_window_event(&mut self, event: &WindowEvent) {
        match event {
            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = to_egui_modifiers(modifiers.state());
            }
            // See the module doc: dropping the state on focus loss prevents a
            // modifier held across an alt-tab from sticking down forever.
            WindowEvent::Focused(false) => self.modifiers = egui::Modifiers::default(),
            _ => {}
        }
    }
}

/// Map a winit [`ModifiersState`] to [`egui::Modifiers`].
///
/// Mirrors `egui-winit` 0.36.1's `WindowEvent::ModifiersChanged` arm exactly,
/// including the platform split on `command`: on macOS the "command" modifier
/// is the Super/⌘ key (and `mac_cmd` is set alongside it), everywhere else it
/// is Ctrl.
fn to_egui_modifiers(state: ModifiersState) -> egui::Modifiers {
    let ctrl = state.control_key();
    let super_ = state.super_key();

    egui::Modifiers {
        alt: state.alt_key(),
        ctrl,
        shift: state.shift_key(),
        mac_cmd: cfg!(target_os = "macos") && super_,
        command: if cfg!(target_os = "macos") {
            super_
        } else {
            ctrl
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{ModifierTracker, to_egui_modifiers};
    use winit::event::WindowEvent;
    use winit::keyboard::ModifiersState;

    // `ModifiersState` is a bitflags type, so the named constants compose
    // directly at each call site (`CONTROL | SHIFT`). A `state(ctrl, shift,
    // alt, super_)` helper taking four bools would read worse and is
    // forbidden outright -- see the `state-representation` skill on bool
    // parameters.

    fn modifiers_changed(state: ModifiersState) -> WindowEvent {
        WindowEvent::ModifiersChanged(state.into())
    }

    #[test]
    fn empty_state_maps_to_no_modifiers() {
        let m = to_egui_modifiers(ModifiersState::empty());
        assert!(!m.alt);
        assert!(!m.ctrl);
        assert!(!m.shift);
        assert!(!m.mac_cmd);
        assert!(!m.command);
    }

    #[test]
    fn each_plain_modifier_maps_one_to_one() {
        assert!(to_egui_modifiers(ModifiersState::CONTROL).ctrl);
        assert!(to_egui_modifiers(ModifiersState::SHIFT).shift);
        assert!(to_egui_modifiers(ModifiersState::ALT).alt);
    }

    /// The `command` mapping is the whole reason this is a named function
    /// rather than five inline field assignments: it is Ctrl off-macOS and
    /// Super on macOS, and the paste intercept reads `command`.
    #[test]
    fn command_follows_the_platform_convention() {
        let ctrl_only = to_egui_modifiers(ModifiersState::CONTROL);
        let super_only = to_egui_modifiers(ModifiersState::SUPER);

        if cfg!(target_os = "macos") {
            assert!(!ctrl_only.command, "on macOS Ctrl is not `command`");
            assert!(super_only.command, "on macOS Super is `command`");
            assert!(super_only.mac_cmd, "on macOS Super also sets `mac_cmd`");
        } else {
            assert!(ctrl_only.command, "off macOS Ctrl is `command`");
            assert!(!super_only.command, "off macOS Super is not `command`");
            assert!(!super_only.mac_cmd, "`mac_cmd` is macOS-only");
        }
    }

    #[test]
    fn tracker_starts_with_nothing_held() {
        let m = ModifierTracker::default().current();
        assert!(!m.ctrl);
        assert!(!m.shift);
        assert!(!m.alt);
    }

    #[test]
    fn tracker_adopts_the_latest_modifiers_changed() {
        let mut tracker = ModifierTracker::default();

        tracker.on_window_event(&modifiers_changed(
            ModifiersState::CONTROL | ModifiersState::SHIFT,
        ));
        assert!(tracker.current().ctrl);
        assert!(tracker.current().shift);

        // A later event fully replaces the state — it is not merged.
        tracker.on_window_event(&modifiers_changed(ModifiersState::ALT));
        assert!(!tracker.current().ctrl);
        assert!(!tracker.current().shift);
        assert!(tracker.current().alt);
    }

    #[test]
    fn losing_focus_clears_held_modifiers() {
        let mut tracker = ModifierTracker::default();
        tracker.on_window_event(&modifiers_changed(ModifiersState::CONTROL));
        assert!(tracker.current().ctrl);

        tracker.on_window_event(&WindowEvent::Focused(false));
        assert!(
            !tracker.current().ctrl,
            "a modifier held across a focus loss must not stick down"
        );
    }

    #[test]
    fn gaining_focus_does_not_clear_held_modifiers() {
        let mut tracker = ModifierTracker::default();
        tracker.on_window_event(&modifiers_changed(ModifiersState::CONTROL));

        tracker.on_window_event(&WindowEvent::Focused(true));
        assert!(
            tracker.current().ctrl,
            "only focus LOSS resets; `Focused(true)` must leave the state alone"
        );
    }

    #[test]
    fn unrelated_events_leave_the_state_untouched() {
        let mut tracker = ModifierTracker::default();
        tracker.on_window_event(&modifiers_changed(ModifiersState::CONTROL));

        tracker.on_window_event(&WindowEvent::RedrawRequested);
        tracker.on_window_event(&WindowEvent::CloseRequested);
        assert!(tracker.current().ctrl);
    }
}
