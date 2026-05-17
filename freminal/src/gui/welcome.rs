// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! First-run onboarding overlay (subtask 71.20).
//!
//! A three-panel modal dialog shown on the first launch of Freminal (or
//! whenever the user invokes `Help -> Show Welcome`).  The panels introduce
//! the three most frequently-missed UI affordances identified during the
//! UX audit:
//!
//! 1. The menu bar (orientation + toggle shortcut).
//! 2. The Settings dialog and its default shortcut.
//! 3. The layouts directory and how to save/load layouts.
//!
//! The overlay stores no user-facing configuration of its own — the only
//! persistent bit is the `first_run_complete` flag in `state.toml` (see
//! [`freminal_common::app_state`]), which is flipped to `true` on Skip,
//! Finish, or close.

use std::fmt;

use super::FreminalGui;

/// Which of the three onboarding panels is currently displayed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Panel {
    /// Panel 1: menu bar orientation and the Toggle Menu Bar shortcut.
    MenuBar,
    /// Panel 2: opening the Settings dialog.
    Settings,
    /// Panel 3: the layouts directory and save/load workflow.
    Layouts,
}

impl Panel {
    /// The first panel shown when the overlay opens.
    const fn first() -> Self {
        Self::MenuBar
    }

    /// Next panel in the forward direction, or `None` if this is the last
    /// panel (in which case the caller should finish the overlay).
    const fn next(self) -> Option<Self> {
        match self {
            Self::MenuBar => Some(Self::Settings),
            Self::Settings => Some(Self::Layouts),
            Self::Layouts => None,
        }
    }

    /// Previous panel, or `None` if this is the first panel (in which case
    /// the Back button is disabled).
    const fn prev(self) -> Option<Self> {
        match self {
            Self::MenuBar => None,
            Self::Settings => Some(Self::MenuBar),
            Self::Layouts => Some(Self::Settings),
        }
    }

    /// 1-based index for the "Step N of 3" header.
    const fn step_number(self) -> u8 {
        match self {
            Self::MenuBar => 1,
            Self::Settings => 2,
            Self::Layouts => 3,
        }
    }
}

impl fmt::Display for Panel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MenuBar => f.write_str("Menu Bar"),
            Self::Settings => f.write_str("Settings"),
            Self::Layouts => f.write_str("Layouts"),
        }
    }
}

/// Signal returned from `WelcomeOverlay::show()` each frame.
///
/// The overlay itself mutates its own `is_open`/`current` state; the only
/// information the surrounding GUI needs is whether the overlay was just
/// dismissed (so the config flag can be flipped and persisted).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WelcomeAction {
    /// No state change this frame.
    None,
    /// The overlay was dismissed — either by Skip, Finish, or the window
    /// close button.  The caller should set `first_run_complete = true`
    /// and persist the config to disk.
    Dismissed,
}

/// Stateful wrapper around the three-panel welcome dialog.
///
/// Owned by `FreminalGui` and rendered each frame via `show()`.  The
/// overlay is hidden by default and is opened via `open()` — either at
/// startup (when the config flag is false) or on demand from the
/// Help menu.
pub(super) struct WelcomeOverlay {
    /// `true` when the modal is currently being rendered.
    is_open: bool,
    /// Which panel is currently displayed.  Always set to `Panel::first()`
    /// when `open()` is called so that re-opening from the Help menu
    /// always starts from the beginning.
    current: Panel,
}

impl WelcomeOverlay {
    /// Construct a new, closed overlay.
    pub(super) const fn new() -> Self {
        Self {
            is_open: false,
            current: Panel::first(),
        }
    }

    /// `true` if the overlay is currently open.  Used by the GUI to decide
    /// whether terminal input should be suppressed while the dialog is up.
    pub(super) const fn is_open(&self) -> bool {
        self.is_open
    }

    /// Open (or re-open) the overlay starting at panel 1.
    pub(super) const fn open(&mut self) {
        self.is_open = true;
        self.current = Panel::first();
    }

    /// Advance to the next panel.  If already on the last panel, close the
    /// overlay and signal dismissal.
    pub(super) const fn advance(&mut self) -> WelcomeAction {
        if let Some(next) = self.current.next() {
            self.current = next;
            WelcomeAction::None
        } else {
            self.is_open = false;
            WelcomeAction::Dismissed
        }
    }

    /// Move back one panel.  No-op on the first panel.
    pub(super) const fn go_back(&mut self) {
        if let Some(prev) = self.current.prev() {
            self.current = prev;
        }
    }

    /// Dismiss the overlay (Skip button or close-X).  Always signals
    /// dismissal even if the overlay is already closed, because callers
    /// only invoke this in response to an explicit user action.
    pub(super) const fn dismiss(&mut self) -> WelcomeAction {
        self.is_open = false;
        WelcomeAction::Dismissed
    }

    /// Which panel is currently displayed.  Used by the rendering code.
    pub(super) const fn current_panel(&self) -> Panel {
        self.current
    }
}

impl Default for WelcomeOverlay {
    fn default() -> Self {
        Self::new()
    }
}

/// Title shown in the `egui::Window` header.
pub(super) const WINDOW_TITLE: &str = "Welcome to Freminal";

/// Render the body of the current panel (heading + description text).
///
/// Kept as a pure function so it can be unit-tested (by string inspection)
/// without needing an egui context.  Returns `(heading, body_lines)`.
pub(super) const fn panel_content(panel: Panel) -> (&'static str, &'static [&'static str]) {
    match panel {
        Panel::MenuBar => (
            "Menu Bar",
            &[
                "Freminal keeps most commands in the menu bar at the top of the window.",
                "",
                "Press Ctrl+Shift+M (or Cmd+Shift+M on macOS) to hide or show the menu bar at any time.",
                "",
                "You can also right-click the title area of a tab for quick access to tab actions.",
            ],
        ),
        Panel::Settings => (
            "Settings",
            &[
                "All preferences live in the Settings dialog: fonts, colors, themes, key bindings, and more.",
                "",
                "Open it from the Edit menu, or press Ctrl+, (Cmd+, on macOS).",
                "",
                "Changes apply live as you edit, and are saved to your config.toml when you click Apply.",
            ],
        ),
        Panel::Layouts => (
            "Layouts",
            &[
                "A layout is a saved arrangement of tabs and split panes — handy for restoring a project workspace.",
                "",
                "Layouts live in ~/.config/freminal/layouts/ as plain TOML files you can edit or share.",
                "",
                "Use the Layouts menu to save the current window, or to load a saved layout into a new tab.",
            ],
        ),
    }
}

impl FreminalGui {
    /// Render the first-run welcome overlay as a centered `egui::Window`.
    ///
    /// Does nothing when the overlay is closed.  When the user clicks
    /// Skip, Finish, or the title-bar close button, the overlay is
    /// hidden and the `first_run_complete` flag in `state.toml` is set
    /// to `true` and persisted (best-effort: a save error surfaces as a
    /// toast but does not block dismissal).
    pub(super) fn show_welcome_overlay(&mut self, ctx: &egui::Context) {
        if !self.welcome.is_open() {
            return;
        }

        let mut window_open = true;
        let mut action = WelcomeAction::None;
        let panel = self.welcome.current_panel();
        let (heading, body) = panel_content(panel);

        egui::Window::new(WINDOW_TITLE)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .open(&mut window_open)
            .show(ctx, |ui| {
                ui.set_min_width(420.0);
                ui.vertical_centered(|ui| {
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(format!("Step {} of 3", panel.step_number()))
                            .small()
                            .weak(),
                    );
                    ui.heading(heading);
                    ui.add_space(6.0);
                });
                ui.separator();
                ui.add_space(6.0);
                for line in body {
                    if line.is_empty() {
                        ui.add_space(6.0);
                    } else {
                        ui.label(*line);
                    }
                }
                ui.add_space(10.0);
                ui.separator();
                ui.add_space(6.0);

                ui.horizontal(|ui| {
                    if ui.button("Skip").clicked() {
                        action = self.welcome.dismiss();
                    }
                    let spacer = (ui.available_width() - 160.0).max(0.0);
                    ui.add_space(spacer);
                    let has_prev = matches!(panel, Panel::Settings | Panel::Layouts);
                    if ui
                        .add_enabled(has_prev, egui::Button::new("Back"))
                        .clicked()
                    {
                        self.welcome.go_back();
                    }
                    let next_label = if matches!(panel, Panel::Layouts) {
                        "Finish"
                    } else {
                        "Next"
                    };
                    if ui.button(next_label).clicked() {
                        action = self.welcome.advance();
                    }
                });
            });

        // The window's own close-X is equivalent to Skip.
        if !window_open && self.welcome.is_open() {
            action = self.welcome.dismiss();
        }

        if matches!(action, WelcomeAction::Dismissed) {
            self.mark_onboarding_complete();
        }
    }

    /// Set `first_run_complete = true` in `state.toml` and persist it.
    ///
    /// State lives in `$XDG_STATE_HOME/freminal/state.toml` (Linux), not
    /// in `config.toml`, specifically so that read-only/managed configs
    /// (NixOS home-manager, system-wide installs, dotfile managers
    /// locking permissions) can still record the dismissal.  See
    /// [`freminal_common::app_state`] for the format and rationale.
    ///
    /// A save failure surfaces as an error toast but does not re-open
    /// the overlay — the user has already dismissed it and should not be
    /// pestered again.  In the rare case that `$XDG_STATE_HOME` itself
    /// is unwritable, the user will see the overlay again on next
    /// launch; that is preferable to a silent infinite loop.
    fn mark_onboarding_complete(&mut self) {
        if self.app_state.first_run_complete {
            return;
        }
        self.app_state.first_run_complete = true;

        let Some(path) = self.app_state_path.as_deref() else {
            tracing::warn!(
                "Cannot persist onboarding flag: state path is unavailable on this platform"
            );
            return;
        };
        if let Err(e) = self.app_state.save(path) {
            tracing::error!(
                "Failed to persist onboarding flag to {}: {e}",
                path.display()
            );
            self.push_error_toast("Could not save onboarding flag", Some(format!("{e}")));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_overlay_is_closed_on_first_panel() {
        let w = WelcomeOverlay::new();
        assert!(!w.is_open());
        assert_eq!(w.current_panel(), Panel::MenuBar);
    }

    #[test]
    fn open_sets_flag_and_resets_to_first_panel() {
        let mut w = WelcomeOverlay::new();
        w.current = Panel::Layouts;
        w.open();
        assert!(w.is_open());
        assert_eq!(w.current_panel(), Panel::MenuBar);
    }

    #[test]
    fn advance_moves_forward_through_panels() {
        let mut w = WelcomeOverlay::new();
        w.open();
        assert_eq!(w.advance(), WelcomeAction::None);
        assert_eq!(w.current_panel(), Panel::Settings);
        assert_eq!(w.advance(), WelcomeAction::None);
        assert_eq!(w.current_panel(), Panel::Layouts);
    }

    #[test]
    fn advance_on_last_panel_dismisses() {
        let mut w = WelcomeOverlay::new();
        w.open();
        w.advance();
        w.advance();
        // Now on Layouts.
        assert_eq!(w.advance(), WelcomeAction::Dismissed);
        assert!(!w.is_open());
    }

    #[test]
    fn go_back_moves_through_panels() {
        let mut w = WelcomeOverlay::new();
        w.open();
        w.advance();
        w.advance();
        assert_eq!(w.current_panel(), Panel::Layouts);
        w.go_back();
        assert_eq!(w.current_panel(), Panel::Settings);
        w.go_back();
        assert_eq!(w.current_panel(), Panel::MenuBar);
    }

    #[test]
    fn go_back_on_first_panel_is_noop() {
        let mut w = WelcomeOverlay::new();
        w.open();
        w.go_back();
        assert_eq!(w.current_panel(), Panel::MenuBar);
        assert!(w.is_open());
    }

    #[test]
    fn dismiss_from_any_panel_closes_and_signals() {
        for start in [Panel::MenuBar, Panel::Settings, Panel::Layouts] {
            let mut w = WelcomeOverlay::new();
            w.open();
            w.current = start;
            assert_eq!(w.dismiss(), WelcomeAction::Dismissed);
            assert!(!w.is_open(), "dismiss from {start:?} should close overlay");
        }
    }

    #[test]
    fn reopen_after_dismiss_resets_to_first_panel() {
        let mut w = WelcomeOverlay::new();
        w.open();
        w.advance();
        w.advance();
        w.dismiss();
        w.open();
        assert_eq!(w.current_panel(), Panel::MenuBar);
        assert!(w.is_open());
    }

    #[test]
    fn step_numbers_match_panel_order() {
        assert_eq!(Panel::MenuBar.step_number(), 1);
        assert_eq!(Panel::Settings.step_number(), 2);
        assert_eq!(Panel::Layouts.step_number(), 3);
    }

    #[test]
    fn panel_content_is_non_empty_for_all_panels() {
        for panel in [Panel::MenuBar, Panel::Settings, Panel::Layouts] {
            let (heading, body) = panel_content(panel);
            assert!(!heading.is_empty(), "heading empty for {panel:?}");
            assert!(!body.is_empty(), "body empty for {panel:?}");
            // Every body must contain at least one non-blank line.
            assert!(
                body.iter().any(|line| !line.is_empty()),
                "body has no real text for {panel:?}"
            );
        }
    }
}
