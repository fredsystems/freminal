//! Close-on-running-command guard (Task 98).
//!
//! When the user attempts to close a pane, tab, or window — or quit the
//! application — while a shell in the affected scope has a running foreground
//! command (per OSC 133 markers), a confirmation dialog lists what is running
//! and lets the user cancel or force-close.
//!
//! This module provides:
//!
//! - The pure detection helpers ([`pane_running_command`],
//!   [`panes_with_running_commands`], [`unknown_command_panes`]) that read the
//!   latest [`TerminalSnapshot`] via the pane's `ArcSwap` without locking or
//!   mutating emulator state.
//! - The [`RunningCommandInfo`] descriptor surfaced to the dialog.
//! - The [`CloseGuardDialog`] modal (98.4) and the [`PendingClose`] intent
//!   carried while the dialog is open (98.5–98.7).
//!
//! "Running" means a [`CommandStatus::Running`] block that has additionally
//! received its `OSC 133 C` (output-start) marker — i.e. a command actually
//! began executing. A block that only saw `OSC 133 A` (the prompt was drawn
//! but nothing was run, then Ctrl-C) is *not* counted as running, which
//! avoids spurious "command running" prompts on an idle prompt.

use std::time::{Duration, SystemTime};

use freminal_common::buffer_states::command_block::{CommandBlock, CommandStatus};

use super::panes::{Pane, PaneId};
use super::tabs::TabId;

/// Information about one pane that has a running foreground command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::gui) struct RunningCommandInfo {
    /// The pane running the command.
    pub pane_id: PaneId,
    /// The tab the pane lives in (for display).
    pub tab_id: TabId,
    /// Display name of the tab (for the dialog list).
    pub tab_name: String,
    /// A human label for the running command. Freminal's OSC 133 markers do
    /// not carry the command text, so this is the pane title when set (often
    /// the running program's name via OSC 0/2) or `"<unknown command>"`.
    pub command: String,
    /// Wall-clock time the command has been running, best-effort. `None` if
    /// the timestamp is missing or clock skew makes the math fail.
    pub elapsed: Option<Duration>,
}

/// The label used when no better command name is available.
const UNKNOWN_COMMAND: &str = "<unknown command>";

/// Returns `true` if `blocks`' most recent entry is an actually-executing
/// command (status `Running` *and* an `OSC 133 C` output-start marker seen).
///
/// Pure over the snapshot's command-block slice so it is unit-testable without
/// constructing a real [`Pane`].
fn blocks_have_running_command(blocks: &[CommandBlock]) -> bool {
    blocks
        .last()
        .is_some_and(|b| b.status() == CommandStatus::Running && b.output_start_row.is_some())
}

/// Returns `true` if `blocks` is empty — the pane has never received an OSC 133
/// prompt marker, so its command status is unknown.
const fn blocks_are_unknown(blocks: &[CommandBlock]) -> bool {
    blocks.is_empty()
}

/// Compute the elapsed runtime of the most recent (running) block in `blocks`,
/// measured from its execution start (`OSC 133 C`) to `now`.
fn running_elapsed(blocks: &[CommandBlock], now: SystemTime) -> Option<Duration> {
    let last = blocks.last()?;
    let anchor = last.executed_at.unwrap_or(last.started_at);
    now.duration_since(anchor).ok()
}

/// Build a [`RunningCommandInfo`] for `pane` if it has a running command.
///
/// Reads the pane's latest snapshot via `ArcSwap` (lock-free). Returns `None`
/// when the pane has no actively-running command.
pub(in crate::gui) fn pane_running_command(
    pane: &Pane,
    tab_id: TabId,
    tab_name: &str,
) -> Option<RunningCommandInfo> {
    let snap = pane.arc_swap.load();
    if !blocks_have_running_command(&snap.command_blocks) {
        return None;
    }

    let command = if pane.title.trim().is_empty() {
        UNKNOWN_COMMAND.to_owned()
    } else {
        pane.title.clone()
    };

    Some(RunningCommandInfo {
        pane_id: pane.id,
        tab_id,
        tab_name: tab_name.to_owned(),
        command,
        elapsed: running_elapsed(&snap.command_blocks, SystemTime::now()),
    })
}

/// Returns `true` if `pane` has never received an OSC 133 prompt marker (its
/// command status is unknown). Used when `[close_guard] unknown_blocks = true`.
pub(in crate::gui) fn pane_is_unknown(pane: &Pane) -> bool {
    blocks_are_unknown(&pane.arc_swap.load().command_blocks)
}

/// Collect the running (and optionally unknown) commands across `panes`, all
/// of which belong to the tab identified by `tab_id` / `tab_name`.
///
/// `unknown_blocks` mirrors `[close_guard] unknown_blocks`: when `true`, a
/// pane that has never seen an OSC 133 prompt also blocks close and is listed
/// with the `"<unknown command>"` label.
pub(in crate::gui) fn gather_tab_running(
    panes: &[&Pane],
    tab_id: TabId,
    tab_name: &str,
    unknown_blocks: bool,
) -> Vec<RunningCommandInfo> {
    let mut out = Vec::new();
    for pane in panes {
        if let Some(info) = pane_running_command(pane, tab_id, tab_name) {
            out.push(info);
        } else if unknown_blocks && pane_is_unknown(pane) {
            out.push(RunningCommandInfo {
                pane_id: pane.id,
                tab_id,
                tab_name: tab_name.to_owned(),
                command: UNKNOWN_COMMAND.to_owned(),
                elapsed: None,
            });
        }
    }
    out
}

/// What close action a [`CloseGuardDialog`] is currently guarding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::gui) enum CloseScope {
    /// Close the focused pane.
    Pane,
    /// Close the tab at the given index.
    Tab(usize),
    /// Close this window (and, if it is the last, quit).
    Window,
}

/// A close action suspended while the guard dialog is open.
#[derive(Debug, Clone)]
pub(in crate::gui) struct PendingClose {
    /// The scope being closed.
    pub scope: CloseScope,
    /// Panes (and their commands) blocking the close, for the dialog list.
    pub running: Vec<RunningCommandInfo>,
}

/// The result of rendering the close-guard dialog for one frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::gui) enum CloseDialogOutcome {
    /// Closed, or open and awaiting a decision. Nothing to do.
    Idle,
    /// The user cancelled. The close must NOT proceed.
    Cancelled,
    /// The user chose "Force Close". The close should proceed.
    ForceClose,
}

/// The close-guard confirmation modal (Task 98.4).
///
/// Lives on `PerWindowState`, mirroring the paste-guard / broadcast-guard
/// dialogs. Registered in `ui_overlay_open` so its keys do not leak to the
/// terminal; rendered every frame via [`CloseGuardDialog::show`].
#[derive(Debug, Default)]
pub(in crate::gui) struct CloseGuardDialog {
    pending: Option<PendingClose>,
}

impl CloseGuardDialog {
    /// Open the dialog for a suspended close.
    pub(in crate::gui) fn open(&mut self, pending: PendingClose) {
        self.pending = Some(pending);
    }

    /// Whether the dialog is currently open.
    pub(in crate::gui) const fn is_open(&self) -> bool {
        self.pending.is_some()
    }

    /// The scope being guarded, if open.
    pub(in crate::gui) fn scope(&self) -> Option<CloseScope> {
        self.pending.as_ref().map(|p| p.scope)
    }

    /// Close the dialog immediately, as if the user chose Force Close. Used by
    /// the `ForceClose` key action; the caller performs the actual close based
    /// on the [`scope`](Self::scope) it captured before calling this.
    pub(in crate::gui) fn force_close_now(&mut self) {
        self.pending = None;
    }

    /// Render the dialog for one frame and return the outcome. On a resolved
    /// outcome (`Cancelled` / `ForceClose`) the dialog closes itself.
    ///
    /// `Escape` cancels; `Ctrl+Enter` force-closes (a deliberately awkward
    /// chord so it is not pressed by reflex).
    pub(in crate::gui) fn show(&mut self, ctx: &egui::Context) -> CloseDialogOutcome {
        let Some(pending) = self.pending.as_ref() else {
            return CloseDialogOutcome::Idle;
        };

        let mut outcome = CloseDialogOutcome::Idle;

        let escape = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        let force = ctx.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::Enter));

        let count = pending.running.len();
        let banner = format!(
            "{count} pane{} {} a running command. Close anyway?",
            if count == 1 { "" } else { "s" },
            if count == 1 { "has" } else { "have" },
        );

        egui::Window::new("Close — Running Commands")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.set_max_width(480.0);
                ui.label(egui::RichText::new(banner).strong());
                ui.add_space(8.0);

                egui::ScrollArea::vertical()
                    .max_height(180.0)
                    .show(ui, |ui| {
                        for info in &pending.running {
                            ui.label(format_running_entry(info));
                        }
                    });

                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        outcome = CloseDialogOutcome::Cancelled;
                    }
                    if ui.button("Force Close").clicked() {
                        outcome = CloseDialogOutcome::ForceClose;
                    }
                });
                ui.add_space(4.0);
                ui.colored_label(
                    egui::Color32::GRAY,
                    "Esc to cancel · Ctrl+Enter to force close",
                );
            });

        if outcome == CloseDialogOutcome::Idle {
            if escape {
                outcome = CloseDialogOutcome::Cancelled;
            } else if force {
                outcome = CloseDialogOutcome::ForceClose;
            }
        }

        if outcome != CloseDialogOutcome::Idle {
            self.pending = None;
        }

        outcome
    }
}

/// Format one running-command entry for the dialog list:
/// `"<tab name> · <command> (<elapsed>)"`.
fn format_running_entry(info: &RunningCommandInfo) -> String {
    let elapsed = info
        .elapsed
        .map_or_else(|| "running".to_owned(), format_elapsed);
    format!("{} · {} ({elapsed})", info.tab_name, info.command)
}

/// Format a duration as a compact `"3s"` / `"2m15s"` / `"1h3m"` label.
fn format_elapsed(d: Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        return format!("{secs}s");
    }
    if secs < 3600 {
        let (m, s) = (secs / 60, secs % 60);
        return if s == 0 {
            format!("{m}m")
        } else {
            format!("{m}m{s}s")
        };
    }
    let (h, m) = (secs / 3600, (secs % 3600) / 60);
    if m == 0 {
        format!("{h}h")
    } else {
        format!("{h}h{m}m")
    }
}

// ---------------------------------------------------------------------------
//  Orchestration (FreminalGui methods)
// ---------------------------------------------------------------------------

impl super::FreminalGui {
    /// Gather running commands across one tab (by index), honoring config.
    /// Returns an empty vec when the guard is disabled or nothing is running.
    fn gather_running_for_tab(
        &self,
        win: &super::window::PerWindowState,
        tab_index: usize,
    ) -> Vec<RunningCommandInfo> {
        if !self.config.close_guard.enabled {
            return Vec::new();
        }
        let Some(tab) = win.tabs.iter().nth(tab_index) else {
            return Vec::new();
        };
        let tab_name = tab
            .display_name(
                self.config.tab_title.policy,
                &self.config.tab_title.separator,
            )
            .into_owned();
        let panes = tab.pane_tree.iter_panes().unwrap_or_default();
        gather_tab_running(
            &panes,
            tab.id,
            &tab_name,
            self.config.close_guard.unknown_blocks,
        )
    }

    /// Gather running commands across every tab in the window.
    fn gather_running_for_window(
        &self,
        win: &super::window::PerWindowState,
    ) -> Vec<RunningCommandInfo> {
        if !self.config.close_guard.enabled {
            return Vec::new();
        }
        let mut out = Vec::new();
        for index in 0..win.tabs.tab_count() {
            out.extend(self.gather_running_for_tab(win, index));
        }
        out
    }

    /// Gather the running command in the active tab's focused pane only.
    fn gather_running_for_focused_pane(
        &self,
        win: &super::window::PerWindowState,
    ) -> Vec<RunningCommandInfo> {
        if !self.config.close_guard.enabled {
            return Vec::new();
        }
        let tab = win.tabs.active_tab();
        let tab_name = tab
            .display_name(
                self.config.tab_title.policy,
                &self.config.tab_title.separator,
            )
            .into_owned();
        let target = tab.active_pane;
        let Ok(panes) = tab.pane_tree.iter_panes() else {
            return Vec::new();
        };
        let Some(pane) = panes.iter().find(|p| p.id == target) else {
            return Vec::new();
        };
        pane_running_command(pane, tab.id, &tab_name).map_or_else(
            || {
                if self.config.close_guard.unknown_blocks && pane_is_unknown(pane) {
                    vec![RunningCommandInfo {
                        pane_id: pane.id,
                        tab_id: tab.id,
                        tab_name,
                        command: UNKNOWN_COMMAND.to_owned(),
                        elapsed: None,
                    }]
                } else {
                    Vec::new()
                }
            },
            |info| vec![info],
        )
    }

    /// Close the focused pane, guarding on running commands.  When a running
    /// command is present, opens the confirmation dialog and suspends the
    /// close; otherwise closes immediately.
    pub(in crate::gui) fn guarded_close_pane(
        &self,
        ui: &egui::Ui,
        win: &mut super::window::PerWindowState,
    ) {
        let running = self.gather_running_for_focused_pane(win);
        if running.is_empty() {
            Self::close_focused_pane(ui, win);
        } else {
            win.close_dialog.open(PendingClose {
                scope: CloseScope::Pane,
                running,
            });
        }
    }

    /// Close the tab at `index`, guarding on running commands.
    pub(in crate::gui) fn guarded_close_tab(
        &self,
        win: &mut super::window::PerWindowState,
        index: usize,
    ) {
        let running = self.gather_running_for_tab(win, index);
        if running.is_empty() {
            win.close_tab(index);
        } else {
            win.close_dialog.open(PendingClose {
                scope: CloseScope::Tab(index),
                running,
            });
        }
    }

    /// Returns the running commands across the whole window for a window-close
    /// guard.  Empty when the guard is disabled or nothing is running.
    pub(in crate::gui) fn window_close_running(
        &self,
        win: &super::window::PerWindowState,
    ) -> Vec<RunningCommandInfo> {
        self.gather_running_for_window(win)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use freminal_common::buffer_states::command_block::CommandBlock;

    /// A block in the `Running` state that has executed (saw `C`).
    fn running_executed() -> CommandBlock {
        let mut b = CommandBlock::new_running(0, None, "fid".to_owned());
        b.output_start_row = Some(1);
        b.executed_at = Some(SystemTime::now());
        b
    }

    /// A block that saw only `A` (prompt drawn, nothing executed).
    fn running_not_executed() -> CommandBlock {
        CommandBlock::new_running(0, None, "fid".to_owned())
    }

    /// A finished, successful block.
    fn finished_success() -> CommandBlock {
        let mut b = CommandBlock::new_running(0, None, "fid".to_owned());
        b.output_start_row = Some(1);
        b.executed_at = Some(SystemTime::now());
        b.end_row = Some(2);
        b.exit_code = Some(0);
        b.finished_at = Some(SystemTime::now());
        b
    }

    #[test]
    fn no_blocks_means_no_running_command() {
        assert!(!blocks_have_running_command(&[]));
    }

    #[test]
    fn executing_block_is_running() {
        assert!(blocks_have_running_command(&[running_executed()]));
    }

    #[test]
    fn prompt_only_block_is_not_running() {
        // A→(Ctrl-C) with no C must NOT count as a running command, so the
        // user is not nagged on an idle prompt.
        assert!(!blocks_have_running_command(&[running_not_executed()]));
    }

    #[test]
    fn finished_block_is_not_running() {
        assert!(!blocks_have_running_command(&[finished_success()]));
    }

    #[test]
    fn only_the_most_recent_block_matters() {
        // An earlier running block followed by a finished block ⇒ not running.
        let blocks = vec![running_executed(), finished_success()];
        assert!(!blocks_have_running_command(&blocks));
        // A finished block followed by a running one ⇒ running.
        let blocks = vec![finished_success(), running_executed()];
        assert!(blocks_have_running_command(&blocks));
    }

    #[test]
    fn empty_blocks_are_unknown() {
        assert!(blocks_are_unknown(&[]));
        assert!(!blocks_are_unknown(&[finished_success()]));
        assert!(!blocks_are_unknown(&[running_executed()]));
    }

    #[test]
    fn running_elapsed_measures_from_execution_start() {
        let mut b = running_executed();
        let start = SystemTime::now() - Duration::from_secs(5);
        b.executed_at = Some(start);
        let now = start + Duration::from_secs(5);
        let elapsed = running_elapsed(&[b], now).expect("elapsed");
        assert_eq!(elapsed.as_secs(), 5);
    }

    #[test]
    fn running_elapsed_falls_back_to_started_at() {
        let mut b = running_executed();
        let start = SystemTime::now() - Duration::from_secs(3);
        b.started_at = start;
        b.executed_at = None;
        let now = start + Duration::from_secs(3);
        let elapsed = running_elapsed(&[b], now).expect("elapsed");
        assert_eq!(elapsed.as_secs(), 3);
    }

    #[test]
    fn running_elapsed_none_for_empty() {
        assert!(running_elapsed(&[], SystemTime::now()).is_none());
    }

    fn info(tab_name: &str, command: &str, elapsed: Option<Duration>) -> RunningCommandInfo {
        RunningCommandInfo {
            pane_id: PaneId::first(),
            tab_id: TabId::first(),
            tab_name: tab_name.to_owned(),
            command: command.to_owned(),
            elapsed,
        }
    }

    #[test]
    fn format_elapsed_compact_units() {
        assert_eq!(format_elapsed(Duration::from_secs(3)), "3s");
        assert_eq!(format_elapsed(Duration::from_secs(59)), "59s");
        assert_eq!(format_elapsed(Duration::from_mins(1)), "1m");
        assert_eq!(format_elapsed(Duration::from_secs(135)), "2m15s");
        assert_eq!(format_elapsed(Duration::from_hours(1)), "1h");
        assert_eq!(format_elapsed(Duration::from_mins(63)), "1h3m");
    }

    #[test]
    fn format_running_entry_includes_tab_command_and_elapsed() {
        let entry = format_running_entry(&info("Work", "vim", Some(Duration::from_secs(5))));
        assert_eq!(entry, "Work · vim (5s)");
    }

    #[test]
    fn format_running_entry_missing_elapsed_reads_running() {
        let entry = format_running_entry(&info("T", "<unknown command>", None));
        assert_eq!(entry, "T · <unknown command> (running)");
    }

    #[test]
    fn dialog_open_and_scope() {
        let mut dialog = CloseGuardDialog::default();
        assert!(!dialog.is_open());
        assert!(dialog.scope().is_none());
        dialog.open(PendingClose {
            scope: CloseScope::Tab(2),
            running: vec![info("T", "cmd", None)],
        });
        assert!(dialog.is_open());
        assert_eq!(dialog.scope(), Some(CloseScope::Tab(2)));
    }
}
