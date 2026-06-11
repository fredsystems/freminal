//! Broadcast-input confirmation dialog (Task 74.5).
//!
//! When the user has set `[tabs] confirm_broadcast = true`, enabling
//! broadcast input for a tab pops a confirmation modal listing how many
//! panes the keystrokes will fan out to. Disabling broadcast never prompts.
//!
//! The dialog lives on `PerWindowState`, mirroring the paste-guard dialog
//! pattern: opened by the `ToggleBroadcastInput` dispatch, rendered every
//! frame while open via [`BroadcastConfirmDialog::show`], and resolved when
//! the user confirms or cancels. Like every modal on the terminal surface it
//! must be registered in `ui_overlay_open` (see `app_impl.rs`) so Escape /
//! Enter / clicks do not leak to the terminal.

use super::tabs::TabId;

/// The result of rendering the broadcast-confirm dialog for one frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::gui) enum BroadcastDialogOutcome {
    /// The dialog is closed, or open and awaiting a decision. Nothing to do.
    Idle,
    /// The user cancelled. Broadcast must remain off.
    Cancelled,
    /// The user confirmed. Broadcast should be enabled on the carried tab.
    Confirmed(TabId),
}

/// State for a single open broadcast-confirm dialog.
#[derive(Debug, Clone, Copy)]
struct BroadcastConfirmState {
    /// The tab whose broadcast flag should be turned on if confirmed.
    tab_id: TabId,
    /// Number of panes the broadcast would reach (for the banner text).
    pane_count: usize,
}

/// The broadcast-confirm modal dialog (Task 74.5).
#[derive(Debug, Default)]
pub(in crate::gui) struct BroadcastConfirmDialog {
    state: Option<BroadcastConfirmState>,
}

impl BroadcastConfirmDialog {
    /// Open the dialog for `tab_id`, which currently holds `pane_count` panes.
    pub(in crate::gui) const fn open(&mut self, tab_id: TabId, pane_count: usize) {
        self.state = Some(BroadcastConfirmState { tab_id, pane_count });
    }

    /// Whether the dialog is currently open.
    pub(in crate::gui) const fn is_open(&self) -> bool {
        self.state.is_some()
    }

    /// Render the dialog for one frame and return the resulting outcome.
    ///
    /// Returns [`BroadcastDialogOutcome::Idle`] when the dialog is closed or
    /// still awaiting a decision. On `Cancelled` or `Confirmed`, the dialog
    /// closes itself before returning.
    ///
    /// Keyboard shortcuts: `Escape` cancels; `Enter` confirms.
    pub(in crate::gui) fn show(&mut self, ctx: &egui::Context) -> BroadcastDialogOutcome {
        let Some(state) = self.state else {
            return BroadcastDialogOutcome::Idle;
        };

        let mut outcome = BroadcastDialogOutcome::Idle;

        let escape = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        let enter = ctx.input(|i| i.key_pressed(egui::Key::Enter));

        egui::Window::new("Enable Broadcast Input")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.set_max_width(420.0);
                ui.label(
                    egui::RichText::new(format!(
                        "You are about to broadcast keyboard input to {} pane(s) \
                         in this tab. Continue?",
                        state.pane_count
                    ))
                    .strong(),
                );
                ui.add_space(8.0);
                ui.colored_label(
                    egui::Color32::GRAY,
                    "Every keystroke will be sent to all panes at once until you \
                     toggle broadcast off.",
                );
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        outcome = BroadcastDialogOutcome::Cancelled;
                    }
                    if ui.button("Enable Broadcast").clicked() {
                        outcome = BroadcastDialogOutcome::Confirmed(state.tab_id);
                    }
                });
            });

        // Keyboard shortcuts resolve only if a button click did not already.
        if outcome == BroadcastDialogOutcome::Idle {
            if escape {
                outcome = BroadcastDialogOutcome::Cancelled;
            } else if enter {
                outcome = BroadcastDialogOutcome::Confirmed(state.tab_id);
            }
        }

        if outcome != BroadcastDialogOutcome::Idle {
            self.state = None;
        }

        outcome
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn new_dialog_is_closed() {
        let dialog = BroadcastConfirmDialog::default();
        assert!(!dialog.is_open());
    }

    #[test]
    fn open_marks_dialog_open() {
        let mut dialog = BroadcastConfirmDialog::default();
        dialog.open(TabId::offset(3), 4);
        assert!(dialog.is_open());
    }

    #[test]
    fn outcome_equality() {
        assert_eq!(BroadcastDialogOutcome::Idle, BroadcastDialogOutcome::Idle);
        assert_eq!(
            BroadcastDialogOutcome::Confirmed(TabId::offset(1)),
            BroadcastDialogOutcome::Confirmed(TabId::offset(1))
        );
        assert_ne!(
            BroadcastDialogOutcome::Confirmed(TabId::offset(1)),
            BroadcastDialogOutcome::Confirmed(TabId::offset(2))
        );
        assert_ne!(
            BroadcastDialogOutcome::Cancelled,
            BroadcastDialogOutcome::Idle
        );
    }
}
