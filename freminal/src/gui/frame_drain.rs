// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Per-frame pane-event draining (Task 122, subtask 122.11a).
//!
//! Everything in this module runs once per frame and walks `win.tabs` (or a
//! `TabManager` slice of it) to drain a channel and stage the results for
//! later handling: [`drain_command_finished_events`] drains
//! `command_event_rx`, [`process_dead_panes`] drains `pty_dead_rx`, and
//! [`drain_window_manipulation_commands`] drains `window_cmd_rx`. That is
//! one coherent concept — "per-frame pane-event draining" — independently
//! identified by two separate reviewers while auditing `app_impl.rs`'s
//! growth across Group B (Tasks 122.5a through 122.8), even though each
//! drain was extracted from `App::update` in a different subtask (122.7 for
//! the first two, 122.8 for the third).
//!
//! This is a **pure move**: no logic changed, no test assertion changed,
//! and the test count is identical to before the move. See
//! `Documents/PLAN_122_ORCHESTRATION_EXTRACTION.md` subtask 122.11a for the
//! rationale and the line-count history that motivated it.
//!
//! [`FreminalGui::route_window_manipulation_events`] (`app_impl.rs`)
//! deliberately stays behind: it takes `&self` and touches enough
//! `FreminalGui` fields (`toasts`, `config`, `osc99_icon_cache`,
//! `osc99_live`, plus the `route_freminal_toast` method) that converting it
//! to a free function taking explicit references would either balloon the
//! signature or require threading `&FreminalGui` through anyway, which
//! defeats the point. `WindowManipulationEvents` (the struct it reads) and
//! `drain_window_manipulation_commands` (the drain that produces it) still
//! move here, since a struct is not "trait-impl-shaped" and only their
//! *fields'* visibility needed to widen (to `pub(super)`) for the
//! cross-module read.
//!
//! ## What deliberately does NOT live here
//!
//! `stage_frame_damage` / `FrameDamageInputs` / `FrameDamageObservations`
//! (Task 122.9) and `stage_chrome_signals` / `ChromeSignalInputs` (Task
//! 122.10) stay in `app_impl.rs`. They are frame-damage/chrome-signal
//! *staging* — deciding what changed and packaging it for a downstream
//! decision — not channel draining, and they do not walk `win.tabs` to
//! drain anything. Folding them in here merely because they are also
//! per-frame `central_body` helpers would turn this module into a
//! grab-bag instead of the one real concept above.

use freminal_common::config::{BellConfig, Config, NotificationsConfig, TabTitlePolicy};
use freminal_common::pty_write::PtyWrite;
use freminal_terminal_emulator::io::InputEvent;
use freminal_terminal_emulator::recording::RecordingSwap;
use tracing::{error, trace};

use super::panes;
use super::tabs::TabManager;
use super::window::PerWindowState;
use super::{rendering, toast};

/// Whether the OS window that owns the terminal surface has input focus
/// this frame, for command-finished notification routing.
///
/// Named per `freminal-state-representation`: this value crosses the call
/// boundary into [`drain_command_finished_events`], so a bare `bool`
/// parameter is disallowed even though the value has exactly two states.
/// [`crate::gui::notifications::NotificationRouter::route`] itself still
/// takes a `bool` (that file is out of scope here), so call sites inside
/// `drain_command_finished_events` and (across the module boundary)
/// `FreminalGui::route_window_manipulation_events` convert back via
/// [`Self::is_focused`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WindowFocus {
    /// The window has input focus this frame.
    Focused,
    /// The window does not have input focus this frame.
    Unfocused,
}

impl WindowFocus {
    /// Build from the raw `bool` read off `egui::InputState::focused`.
    pub(super) const fn from_bool(focused: bool) -> Self {
        if focused {
            Self::Focused
        } else {
            Self::Unfocused
        }
    }

    /// The `bool` form expected by `NotificationRouter::route`.
    pub(super) const fn is_focused(self) -> bool {
        matches!(self, Self::Focused)
    }
}

/// Outcome of [`process_dead_panes`]'s PTY-death poll for one frame.
///
/// Named per `freminal-state-representation`: `process_dead_panes` has no
/// access to `self.windows` or `ctx`, so it cannot itself close the OS
/// window when a dead pane empties the window's last tab. It hands that
/// decision back to the caller as a named enum rather than a bare `bool`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DeadPaneOutcome {
    /// No dead pane emptied the window's last tab this frame; `update`
    /// continues processing the frame as normal.
    Continue,
    /// The last pane in the window's last tab died, so that tab was
    /// closed and the window itself must now close. The caller owns
    /// `self.windows` and `ctx`, so it must reinsert `win` into
    /// `self.windows` and issue `ViewportCommand::Close` before returning
    /// — `process_dead_panes` touches neither.
    CloseWindow,
}

/// Drain `CommandFinishedEvent`s from every pane's `command_event_rx`,
/// append each finished block to its owning pane's `recent_commands` ring
/// (Task 72.9), flag non-active tabs that received an event, ring the bell
/// when `[bell] on_command_finished` is set (Task 76.5), and route any
/// resulting command-finished notifications (Task 76.4).
///
/// Extracted from `App::update` as a zero-egui helper (Task 122.7): the
/// only egui touch in the original inline block —
/// `ctx.input(|i| i.focused)` — is hoisted to the caller and passed in as
/// `window_focus`.
///
/// The notification routing pass deliberately stays inside this function,
/// running *after* the per-tab drain loop over `tabs` and never
/// interleaved with it: `toasts` needs a mutable borrow that must not
/// overlap the loop's mutable borrow of `tabs`. Do not reorder the two
/// phases or interleave routing into the loop.
pub(super) fn drain_command_finished_events(
    tabs: &mut TabManager,
    tab_title_policy: TabTitlePolicy,
    tab_title_separator: &str,
    notifications_config: &NotificationsConfig,
    bell_config: &BellConfig,
    window_focus: WindowFocus,
    toasts: &std::cell::RefCell<toast::ToastStack>,
) {
    let active_tab_idx = tabs.active_index();
    let mut command_notifications: Vec<crate::gui::notifications::NotificationRequest> = Vec::new();

    for (tab_idx, tab) in tabs.iter_mut().enumerate() {
        let mut tab_received_event = false;
        // Resolve the tab display name up front: `iter_panes_mut` borrows
        // `tab` mutably, so `display_name` cannot be called inside the
        // inner loop. Used for the `{tab_name}` notification template
        // token (Task 76.5).
        let tab_name = tab
            .display_name(tab_title_policy, tab_title_separator)
            .into_owned();
        if let Ok(panes) = tab.pane_tree.iter_panes_mut() {
            for pane in panes {
                while let Ok(event) = pane.command_event_rx.try_recv() {
                    // Extract the command text from the current snapshot
                    // before the rows scroll out of the visible window.
                    // Used by the Quick Command History Palette to replay
                    // live entries. Cache entries whose rows have already
                    // left the visible window will be silently absent —
                    // the seed half of the palette still works in that
                    // case.
                    let snap = pane.arc_swap.load();
                    let command_text =
                        crate::gui::command_history::extract_command_text(&snap, &event.block);
                    if let Some(text) = &command_text {
                        pane.record_command_text(event.block.id, text.clone());
                    }
                    drop(snap);

                    // Build a command-finished notification request (the
                    // builder applies the enable + threshold gates) before
                    // the block is moved into the recent-commands ring.
                    if let Some(req) = crate::gui::notifications::command_finished_request(
                        &event.block,
                        command_text.as_deref().unwrap_or(""),
                        &tab_name,
                        notifications_config,
                    ) {
                        command_notifications.push(req);
                    }

                    // Ring the bell on command completion when
                    // `[bell] on_command_finished` is set (Task 76.5).
                    // Uses the configured bell mode, mirroring the
                    // `WindowManipulation::Bell` path in `rendering`.
                    if bell_config.on_command_finished {
                        use freminal_common::config::BellMode;
                        let mode = bell_config.mode;
                        if matches!(mode, BellMode::Visual | BellMode::Both) {
                            pane.bell_active = true;
                            pane.view_state.bell_since = Some(std::time::Instant::now());
                        }
                        if matches!(mode, BellMode::Audio | BellMode::Both) {
                            crate::gui::platform::system_beep();
                        }
                    }

                    pane.push_recent_command(event.block);
                    tab_received_event = true;
                }
            }
        }
        if tab_received_event && tab_idx != active_tab_idx {
            tab.has_pending_event = true;
        }
    }

    // Route command-finished notifications collected above (Task 76.4).
    if !command_notifications.is_empty()
        && let Ok(mut toasts) = toasts.try_borrow_mut()
    {
        for req in &command_notifications {
            crate::gui::notifications::NotificationRouter::route(
                req,
                notifications_config,
                window_focus.is_focused(),
                &mut toasts,
            );
        }
    }
}

/// Poll every pane in `win` for a PTY-death signal and close the dead
/// panes.
///
/// Dead `(tab_index, pane_id)` pairs are collected first, then processed
/// in reverse order so closing one does not shift the tab indices of the
/// others still pending. For each dead pane: close just the pane; if it
/// was the tab's last pane, close the tab; if that tab was the window's
/// last tab, return [`DeadPaneOutcome::CloseWindow`] immediately and stop
/// processing further dead panes — the caller owns `self.windows` and
/// `ctx`, so it performs the reinsert-and-close dance itself.
///
/// Extracted from `App::update` as a zero-egui, zero-`self.windows`
/// helper (Task 122.7): it never calls `ctx.send_viewport_cmd` and never
/// touches `self.windows`.
pub(super) fn process_dead_panes(
    win: &mut PerWindowState,
    recording_swap: &RecordingSwap,
) -> DeadPaneOutcome {
    // Collect (tab_index, pane_id) pairs for dead panes, then process them
    // in reverse order to avoid index shifting issues.
    let mut dead_panes: Vec<(usize, panes::PaneId)> = Vec::new();
    for (tab_idx, tab) in win.tabs.iter().enumerate() {
        if let Ok(panes) = tab.pane_tree.iter_panes() {
            for pane in panes {
                if pane.pty_dead_rx.try_recv().is_ok() {
                    dead_panes.push((tab_idx, pane.id));
                }
            }
        }
    }

    for (tab_idx, pane_id) in dead_panes.into_iter().rev() {
        // Try to close just the dead pane within its tab.
        let is_active_tab = tab_idx == win.tabs.active_index();

        // Capture the originally-active tab's stable id so we can restore
        // focus to *that* tab afterwards. Restoring by index is wrong:
        // closing a tab at a lower index shifts the active tab left, and
        // the dead pane's `tab_idx` is not the user's active tab.
        let original_active_tab_id = win.tabs.active_tab().id;

        // Switch to the dead pane's tab temporarily if needed so we can
        // operate on it.
        if !is_active_tab && let Err(e) = win.tabs.switch_to(tab_idx) {
            error!("Failed to switch to tab {tab_idx} for dead pane cleanup: {e}");
            continue;
        }

        let tab = win.tabs.active_tab_mut();
        // If the dead pane was the zoomed pane, un-zoom first.
        if tab.zoomed_pane == Some(pane_id) {
            tab.zoomed_pane = None;
        }

        match tab.pane_tree.close(pane_id) {
            Ok(_closed) => {
                // Emit PaneClose recording event.
                if let Some(h) = recording_swap.load_full() {
                    // Saturating `u64 -> u32`: pane IDs are monotonic from
                    // 0 and will never realistically exceed u32::MAX.
                    h.emit(
                        freminal_terminal_emulator::recording::EventPayload::PaneClose {
                            pane_id: u32::try_from(pane_id.raw()).unwrap_or(u32::MAX),
                        },
                    );
                }

                // Reset last_sent_size on all surviving panes so the
                // next frame's resize check fires with the new layout.
                let tab = win.tabs.active_tab_mut();
                if let Ok(panes) = tab.pane_tree.iter_panes_mut() {
                    for pane in panes {
                        pane.view_state.last_sent_size = (0, 0);
                    }
                }
                // If the active pane was the one that died, pick a new active pane
                // and notify it that it gained focus.
                let tab = win.tabs.active_tab_mut();
                if tab.active_pane == pane_id
                    && let Ok(panes) = tab.pane_tree.iter_panes()
                    && let Some(first) = panes.first()
                {
                    let new_id = first.id;
                    if let Err(e) = first.input_tx.send(InputEvent::FocusChange(true)) {
                        error!("Failed to send FocusChange(true) to pane {new_id}: {e}");
                    }
                    tab.active_pane = new_id;
                }
            }
            Err(panes::PaneError::CannotCloseLastPane) => {
                // Last pane in tab — close the entire tab.
                if win.tabs.tab_count() <= 1 {
                    // Last tab in this window — report to the caller so it
                    // can close the window.
                    return DeadPaneOutcome::CloseWindow;
                }
                win.close_tab(tab_idx);
            }
            Err(e) => {
                error!("Failed to close dead pane {pane_id}: {e}");
            }
        }

        // Restore the originally-active tab if we switched away. Look it
        // up by stable id rather than by index, since a tab close during
        // this iteration may have shifted indices. If the originally-active
        // tab was itself closed, leave the active index where `close_tab`
        // placed it.
        if !is_active_tab {
            let restore_idx = win.tabs.iter().position(|t| t.id == original_active_tab_id);
            if let Some(restore_idx) = restore_idx {
                let _ = win.tabs.switch_to(restore_idx);
            }
        }
    }

    DeadPaneOutcome::Continue
}

/// OSC-derived events collected by [`drain_window_manipulation_commands`]
/// during its per-tab, per-pane `handle_window_manipulation` drain, staged
/// for routing by [`FreminalGui::route_window_manipulation_events`] once the
/// drain loop's mutable borrow of the tab list has ended.
///
/// [`FreminalGui::route_window_manipulation_events`]: super::FreminalGui::route_window_manipulation_events
pub(super) struct WindowManipulationEvents {
    /// OSC 9 / OSC 777 notifications collected from every pane this frame,
    /// routed after the drain loop (Task 76.4).
    pub(super) osc_notifications: Vec<crate::gui::notifications::NotificationRequest>,
    /// OSC 99 stateful notifications collected from every pane this frame,
    /// routed after the drain loop (Task 99.5a) alongside
    /// `osc_notifications`. Each item is paired with a clone of the
    /// originating pane's `pty_write_tx` (Task 99.5c Gap 2) so the
    /// reverse-path write (Task 99.6) can target the right pane.
    pub(super) osc99_notifications: Vec<(
        freminal_common::buffer_states::window_manipulation::Notification99Data,
        crossbeam_channel::Sender<PtyWrite>,
    )>,
    /// OSC 99 app→terminal control sequences (p=close/p=alive/p=?) collected
    /// from every pane this frame (Task 99.5c), answered after the drain
    /// loop.
    pub(super) osc99_controls: Vec<(
        crate::gui::notifications::Osc99Control,
        crossbeam_channel::Sender<PtyWrite>,
    )>,
    /// OSC 52 clipboard events (remote write / blocked read) collected from
    /// every pane this frame, routed to a toast after the drain loop (issue
    /// #433).
    pub(super) osc52_events: Vec<rendering::Osc52ToastEvent>,
}

/// Drain pending `WindowCommand`s for every pane in every tab of `tabs`,
/// calling `rendering::handle_window_manipulation` per pane, and collect the
/// OSC 9/777, OSC 99 (notification + control), and OSC 52 events it produces
/// for [`FreminalGui::route_window_manipulation_events`] to route once this
/// function returns.
///
/// **Discard-rule contract**, preserved exactly from the pre-extraction
/// inline block: this drains ALL tabs and ALL panes, not just the active
/// one, but active and non-active panes are handled differently. The active
/// tab's active pane gets full handling (viewport commands, reports, title
/// updates, clipboard). Every other pane gets reports answered, titles
/// updated, and clipboard handled — but viewport-mutating commands (resize,
/// move, minimize, fullscreen) are discarded, since a non-active pane must
/// not alter the shared window geometry. See
/// `rendering::handle_window_manipulation`'s own doc for the full
/// per-variant breakdown (including the split-pane resize suppression via
/// `is_only_pane`). Changing this branching changes which panes may
/// resize/move/minimize the shared OS window — do not touch it casually.
///
/// Extracted from `App::update`'s `central_body` closure (Task 122.8).
/// Unlike the zero-egui helpers extracted in Task 122.7, this is **not** an
/// egui-freeing extraction: `rendering::handle_window_manipulation` itself
/// needs `ui` (OSC 52 clipboard access, the window content rect for
/// Report* responses), so `ui` is threaded straight through. `window_focus`
/// is computed by the caller (`ui.input(|i| i.focused)`) and passed in as a
/// named enum per `freminal-state-representation`, since it crosses this
/// function boundary.
///
/// Takes the whole `config` rather than its `security`, `bell`, and
/// `tab_title` sections individually to stay under clippy's
/// `too_many_arguments` threshold; the config-toggle fields it reads
/// (`security.allow_clipboard_read`) are exempt from
/// `freminal-state-representation`'s bool-parameter rule per that skill's
/// "config toggles deserialised from TOML" case.
///
/// [`FreminalGui::route_window_manipulation_events`]: super::FreminalGui::route_window_manipulation_events
pub(super) fn drain_window_manipulation_commands(
    ui: &egui::Ui,
    tabs: &mut TabManager,
    font_width: usize,
    font_height: usize,
    window_focus: WindowFocus,
    config: &Config,
) -> WindowManipulationEvents {
    let window_content_rect = ui.input(|i: &egui::InputState| i.content_rect());
    let active_idx = tabs.active_index();
    let active_pane_id_for_drain = tabs.active_tab().active_pane;

    let mut events = WindowManipulationEvents {
        osc_notifications: Vec::new(),
        osc99_notifications: Vec::new(),
        osc99_controls: Vec::new(),
        osc52_events: Vec::new(),
    };

    for (idx, tab) in tabs.iter_mut().enumerate() {
        let is_active_tab = idx == active_idx;
        let is_only_pane = match tab.pane_tree.pane_count() {
            Ok(count) => count == 1,
            Err(e) => {
                trace!("pane_count error (treating as split): {e}");
                false
            }
        };
        if let Ok(panes) = tab.pane_tree.iter_panes_mut() {
            let mut tab_shell_set_title = false;
            for pane in panes {
                let is_fully_active = is_active_tab && pane.id == active_pane_id_for_drain;
                let shell_set = rendering::handle_window_manipulation(
                    ui,
                    &pane.window_cmd_rx,
                    &pane.pty_write_tx,
                    font_width,
                    font_height,
                    window_content_rect,
                    &mut pane.title_stack,
                    &mut pane.title,
                    &mut pane.bell_active,
                    &mut pane.view_state.bell_since,
                    config.bell.mode,
                    &rendering::WindowManipFlags {
                        allow_clipboard_read: config.security.allow_clipboard_read,
                        is_active: is_fully_active,
                        window_focused: window_focus.is_focused(),
                        is_only_pane,
                    },
                    &mut events.osc_notifications,
                    &mut events.osc99_notifications,
                    &mut events.osc99_controls,
                    &mut events.osc52_events,
                );
                if shell_set {
                    tab_shell_set_title = true;
                }
            }
            // The title policy decides whether a shell-asserted OSC
            // 0/1/2 title clears the user-pinned custom name (only
            // under `OscWins`); see `Tab::apply_osc_title_policy`.
            tab.apply_osc_title_policy(config.tab_title.policy, tab_shell_set_title);
        }
    }

    events
}

#[cfg(test)]
mod tests {
    use super::{
        DeadPaneOutcome, WindowFocus, drain_command_finished_events,
        drain_window_manipulation_commands,
    };
    use crate::gui::panes::{Pane, PaneId, PaneIdGenerator};
    use crate::gui::pty::CommandFinishedEvent;
    use crate::gui::tabs::{Tab, TabId, TabManager};
    use freminal_common::buffer_states::command_block::{CommandBlock, CommandBlockId};
    use freminal_common::buffer_states::window_manipulation::WindowManipulation;
    use freminal_common::config::{BellConfig, Config, NotificationsConfig, TabTitlePolicy};
    use freminal_terminal_emulator::io::WindowCommand;
    use freminal_terminal_emulator::snapshot::TerminalSnapshot;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    use std::time::{Duration, SystemTime};

    // ── WindowFocus / DeadPaneOutcome (Task 122.7) ───────────────────────
    //
    // `process_dead_panes` itself is NOT unit-tested here: it takes
    // `&mut PerWindowState`, and `PerWindowState` cannot be constructed
    // headlessly in this test module — its `terminal_widget` field is a
    // `FreminalTerminalWidget`, which (like the `FreminalGui`/`WindowId`
    // combination already documented above, at the band-shape-range tests)
    // this codebase deliberately does not attempt to fake for unit tests.
    // The reverse-order / stable-tab-id-restore invariant it relies on is
    // covered at the `TabManager` level by `tabs.rs`'s
    // `close_tab_before_active_shifts_left` and
    // `close_tab_at_active_selects_successor`. Extracting a further "pure"
    // helper purely to make `process_dead_panes` unit-testable would
    // re-implement the same control flow being pinned rather than test it,
    // which is the anti-pattern this subtask's instructions call out.

    #[test]
    fn window_focus_round_trips_through_bool() {
        assert_eq!(WindowFocus::from_bool(true), WindowFocus::Focused);
        assert_eq!(WindowFocus::from_bool(false), WindowFocus::Unfocused);
        assert!(WindowFocus::from_bool(true).is_focused());
        assert!(!WindowFocus::from_bool(false).is_focused());
    }

    #[test]
    fn dead_pane_outcome_variants_are_distinct() {
        assert_ne!(DeadPaneOutcome::Continue, DeadPaneOutcome::CloseWindow);
    }

    // ── drain_command_finished_events (Task 122.7) ───────────────────────
    //
    // Unlike `process_dead_panes`, this function takes only `&mut
    // TabManager` plus plain config values — no `PerWindowState`, no
    // `FreminalTerminalWidget`, no `WindowId` — so a real (if minimal)
    // `TabManager`/`Tab`/`Pane` fixture can exercise it directly.

    /// Build a `Pane` with real, connected channels, given both the
    /// `CommandFinishedEvent` and `WindowCommand` receivers.
    ///
    /// Shared by [`test_pane`] and [`test_window_manip_pane`], which each
    /// supply the receiver their tests actually push events into and
    /// manufacture a throwaway sender/receiver pair for the other channel.
    /// Mirrors `panes::mod`'s private `dummy_pane` test helper, but keeps
    /// the caller-supplied channel's sender alive by taking it as a
    /// receiver built by the caller (that helper drops its sender
    /// immediately, which is fine for its own pane-data-model tests but
    /// unusable here, where tests need to push events in).
    fn test_pane_with(
        id: PaneId,
        command_event_rx: crossbeam_channel::Receiver<CommandFinishedEvent>,
        window_cmd_rx: crossbeam_channel::Receiver<WindowCommand>,
    ) -> Pane {
        let arc_swap = Arc::new(arc_swap::ArcSwap::from_pointee(TerminalSnapshot::empty()));
        let (input_tx, _input_rx) = crossbeam_channel::unbounded();
        let (pty_write_tx, _pty_write_rx) = crossbeam_channel::unbounded();
        let (_clipboard_tx, clipboard_rx) = crossbeam_channel::bounded(1);
        let (_search_buffer_tx, search_buffer_rx) = crossbeam_channel::bounded(1);
        let (_pty_dead_tx, pty_dead_rx) = crossbeam_channel::bounded(1);
        Pane {
            id,
            arc_swap,
            input_tx,
            pty_write_tx,
            window_cmd_rx,
            clipboard_rx,
            search_buffer_rx,
            pty_dead_rx,
            title: String::new(),
            bell_active: false,
            pending_copy: false,
            title_stack: Vec::new(),
            view_state: crate::gui::view_state::ViewState::new(),
            echo_off: Arc::new(AtomicBool::new(false)),
            child_pid: None,
            render_state: crate::gui::terminal::new_render_state(Arc::new(std::sync::Mutex::new(
                crate::gui::renderer::WindowPostRenderer::new(),
            ))),
            render_cache: crate::gui::terminal::PaneRenderCache::new(),
            command_event_rx,
            recent_commands: std::collections::VecDeque::new(),
            history_seed: crate::gui::shell_history::new_seeded_history(),
            shell_program: None,
            shell_histfile_last_seen: None,
            command_texts: std::collections::HashMap::new(),
        }
    }

    /// Build a `Pane` with real, connected channels for
    /// `drain_command_finished_events` tests.
    ///
    /// Wraps [`test_pane_with`], manufacturing a throwaway `WindowCommand`
    /// sender/receiver pair since these tests never push window commands.
    fn test_pane(
        id: PaneId,
        command_event_rx: crossbeam_channel::Receiver<CommandFinishedEvent>,
    ) -> Pane {
        let (_window_cmd_tx, window_cmd_rx) = crossbeam_channel::unbounded();
        test_pane_with(id, command_event_rx, window_cmd_rx)
    }

    /// A finished, successfully-executed `CommandBlock` with a known
    /// duration, mirroring `notifications.rs`'s private `finished_block`
    /// test helper (not reusable from here: it is private to that file).
    fn finished_block(exit_code: Option<i32>, dur_secs: u64) -> CommandBlock {
        let executed = SystemTime::now();
        CommandBlock {
            id: CommandBlockId::next(),
            fid: "t".to_owned(),
            prompt_start_row: 0,
            command_start_row: Some(0),
            output_start_row: Some(0),
            end_row: Some(1),
            exit_code,
            cwd: None,
            started_at: executed,
            executed_at: Some(executed),
            finished_at: Some(executed + Duration::from_secs(dur_secs)),
        }
    }

    #[test]
    fn drain_command_finished_events_flags_only_the_non_active_tab() {
        let mut ids = PaneIdGenerator::new(0);
        let (active_tx, active_rx) = crossbeam_channel::unbounded();
        let (bg_tx, bg_rx) = crossbeam_channel::unbounded();
        let active_pane = test_pane(ids.next_id(), active_rx);
        let bg_pane = test_pane(ids.next_id(), bg_rx);

        let mut tabs = TabManager::new(Tab::new(TabId::first(), active_pane));
        tabs.add_tab(Tab::new(TabId::offset(1), bg_pane));
        // `add_tab` switches to the new tab; restore tab 0 as active so the
        // second tab is the non-active one under test.
        if let Err(e) = tabs.switch_to(0) {
            panic!("switch back to tab 0: {e}");
        }

        // Only the background (non-active) tab's pane receives an event.
        if let Err(e) = bg_tx.send(CommandFinishedEvent {
            pane_id: 0,
            block: finished_block(Some(0), 1),
        }) {
            panic!("send to bg pane: {e}");
        }
        drop(active_tx); // unused; keeps the active pane's channel alive until here

        let notifications_config = NotificationsConfig::default(); // disabled: no routing side effects
        let bell_config = BellConfig::default(); // on_command_finished: false
        let toasts = std::cell::RefCell::new(crate::gui::toast::ToastStack::default());

        drain_command_finished_events(
            &mut tabs,
            TabTitlePolicy::default(),
            "",
            &notifications_config,
            &bell_config,
            WindowFocus::Unfocused,
            &toasts,
        );

        let Some(active_tab) = tabs.iter().next() else {
            panic!("tab 0 must exist");
        };
        assert!(
            !active_tab.has_pending_event,
            "the active tab must never be flagged, even though it's tab 0"
        );
        let Some(bg_tab) = tabs.iter().nth(1) else {
            panic!("tab 1 must exist");
        };
        assert!(
            bg_tab.has_pending_event,
            "the non-active tab that received an event must be flagged"
        );
        let Ok(bg_panes) = bg_tab.pane_tree.iter_panes() else {
            panic!("bg pane tree must resolve");
        };
        assert_eq!(
            bg_panes[0].recent_commands.len(),
            1,
            "the finished block must be appended to the receiving pane's ring"
        );
    }

    #[test]
    fn drain_command_finished_events_routes_a_toast_when_focused_and_enabled() {
        let mut ids = PaneIdGenerator::new(0);
        let (tx, rx) = crossbeam_channel::unbounded();
        let pane = test_pane(ids.next_id(), rx);
        let mut tabs = TabManager::new(Tab::new(TabId::first(), pane));

        if let Err(e) = tx.send(CommandFinishedEvent {
            pane_id: 0,
            block: finished_block(Some(0), 30),
        }) {
            panic!("send finished event: {e}");
        }

        let notifications_config = NotificationsConfig {
            enabled: true,
            on_command_finished: true,
            command_finished_threshold_secs: 1.0,
            ..NotificationsConfig::default()
        };
        let bell_config = BellConfig::default();
        let toasts = std::cell::RefCell::new(crate::gui::toast::ToastStack::default());

        // Default `routing_command_finished` is `SystemWhenUnfocused`, which
        // wants a toast only while focused — exercise that branch directly
        // rather than also overriding the routing policy.
        drain_command_finished_events(
            &mut tabs,
            TabTitlePolicy::default(),
            "",
            &notifications_config,
            &bell_config,
            WindowFocus::Focused,
            &toasts,
        );

        assert_eq!(
            toasts.borrow().last_kind(),
            Some(crate::gui::toast::ToastKind::Info),
            "a successful command-finished notification must land as an Info toast"
        );
    }

    #[test]
    fn drain_command_finished_events_rings_visual_bell_when_configured() {
        use freminal_common::config::BellMode;

        let mut ids = PaneIdGenerator::new(0);
        let (tx, rx) = crossbeam_channel::unbounded();
        let pane = test_pane(ids.next_id(), rx);
        let mut tabs = TabManager::new(Tab::new(TabId::first(), pane));

        if let Err(e) = tx.send(CommandFinishedEvent {
            pane_id: 0,
            block: finished_block(Some(0), 1),
        }) {
            panic!("send finished event: {e}");
        }

        let notifications_config = NotificationsConfig::default();
        let bell_config = BellConfig {
            on_command_finished: true,
            mode: BellMode::Visual,
        };
        let toasts = std::cell::RefCell::new(crate::gui::toast::ToastStack::default());

        drain_command_finished_events(
            &mut tabs,
            TabTitlePolicy::default(),
            "",
            &notifications_config,
            &bell_config,
            WindowFocus::Unfocused,
            &toasts,
        );

        let Ok(panes) = tabs.active_tab().pane_tree.iter_panes() else {
            panic!("pane tree must resolve");
        };
        assert!(
            panes[0].bell_active,
            "on_command_finished + BellMode::Visual must set bell_active"
        );
    }

    // ── drain_window_manipulation_commands (Task 122.8) ──────────────────
    //
    // Unlike `drain_command_finished_events` (Task 122.7), this function is
    // not zero-egui: `rendering::handle_window_manipulation` needs `ui` to
    // send `ViewportCommand`s. `egui::Context::run_ui` (already used above
    // for the band-shape-range tests) supplies a real `Ui`, so the
    // active/non-active discard rule can be exercised through the actual
    // production call rather than a re-implementation of its branching.

    /// Build a `Pane` with a caller-supplied `window_cmd_rx`, for
    /// `drain_window_manipulation_commands` tests. Wraps [`test_pane_with`],
    /// manufacturing a throwaway `CommandFinishedEvent` sender/receiver
    /// pair since these tests push `WindowCommand`s in rather than
    /// `CommandFinishedEvent`s.
    fn test_window_manip_pane(
        id: PaneId,
        window_cmd_rx: crossbeam_channel::Receiver<WindowCommand>,
    ) -> Pane {
        let (_command_event_tx, command_event_rx) = crossbeam_channel::unbounded();
        test_pane_with(id, command_event_rx, window_cmd_rx)
    }

    /// Build a two-tab `TabManager` (tab 0 active, tab 1 non-active) whose
    /// panes have caller-visible `WindowCommand` senders, for the
    /// discard-rule tests below.
    fn two_tab_manager_with_window_cmd_senders() -> (
        TabManager,
        crossbeam_channel::Sender<WindowCommand>,
        crossbeam_channel::Sender<WindowCommand>,
    ) {
        let mut ids = PaneIdGenerator::new(0);
        let (active_tx, active_rx) = crossbeam_channel::unbounded();
        let (bg_tx, bg_rx) = crossbeam_channel::unbounded();
        let active_pane = test_window_manip_pane(ids.next_id(), active_rx);
        let bg_pane = test_window_manip_pane(ids.next_id(), bg_rx);

        let mut tabs = TabManager::new(Tab::new(TabId::first(), active_pane));
        tabs.add_tab(Tab::new(TabId::offset(1), bg_pane));
        // `add_tab` switches to the new tab; restore tab 0 as active so tab
        // 1 is the non-active tab under test, mirroring
        // `drain_command_finished_events_flags_only_the_non_active_tab`.
        if let Err(e) = tabs.switch_to(0) {
            panic!("switch back to tab 0: {e}");
        }

        (tabs, active_tx, bg_tx)
    }

    #[test]
    fn drain_window_manipulation_commands_only_broadcasts_title_for_the_active_pane() {
        let (mut tabs, active_tx, bg_tx) = two_tab_manager_with_window_cmd_senders();

        // Every pane's own `title` field updates regardless of active
        // state (`rendering::handle_window_manipulation` always does
        // `tab_title.clone_from(&title)`) — that is NOT the discriminator.
        // What must differ is whether the shared OS window title bar is
        // asserted via `ViewportCommand::Title`, which only the active
        // tab's active pane may do.
        if let Err(e) = active_tx.send(WindowCommand::Viewport(
            WindowManipulation::SetTitleBarText("active-title".to_owned()),
        )) {
            panic!("send to active pane: {e}");
        }
        if let Err(e) = bg_tx.send(WindowCommand::Viewport(
            WindowManipulation::SetTitleBarText("bg-title".to_owned()),
        )) {
            panic!("send to bg pane: {e}");
        }

        let config = Config::default();
        let ctx = egui::Context::default();
        let mut events = None;
        // No painter here, so the egui 0.36 `TexturesDelta` drop-bomb (#8356)
        // must be defused explicitly -- see A2 in EGUI_UPGRADE_ASSUMPTIONS.md.
        let mut full_output = ctx.run_ui(egui::RawInput::default(), |ui| {
            events = Some(drain_window_manipulation_commands(
                ui,
                &mut tabs,
                8,
                16,
                WindowFocus::Focused,
                &config,
            ));
        });
        full_output.textures_delta.clear();
        let Some(events) = events else {
            panic!("closure runs synchronously inside run_ui");
        };

        let title_commands: Vec<String> = full_output
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .map(|vp| {
                vp.commands
                    .iter()
                    .filter_map(|c| match c {
                        egui::ViewportCommand::Title(t) => Some(t.clone()),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default();

        assert_eq!(
            title_commands,
            vec!["active-title".to_owned()],
            "only the active tab's active pane may assert the shared OS window title"
        );

        // This scenario produces no OSC 9/99/52 events; a title-only frame
        // must not spuriously populate any of the routed vectors.
        assert!(events.osc_notifications.is_empty());
        assert!(events.osc99_notifications.is_empty());
        assert!(events.osc99_controls.is_empty());
        assert!(events.osc52_events.is_empty());
    }

    #[test]
    fn drain_window_manipulation_commands_discards_viewport_mutation_for_non_active_pane() {
        let (mut tabs, active_tx, bg_tx) = two_tab_manager_with_window_cmd_senders();

        // `MinimizeWindow` is one of the viewport-mutating commands that
        // `rendering::handle_window_manipulation` discards outright
        // (`{}`, no title/report/clipboard side effect at all) when
        // `!flags.is_active` — a non-active pane must not be able to
        // minimize the shared OS window.
        if let Err(e) = active_tx.send(WindowCommand::Viewport(WindowManipulation::MinimizeWindow))
        {
            panic!("send to active pane: {e}");
        }
        if let Err(e) = bg_tx.send(WindowCommand::Viewport(WindowManipulation::MinimizeWindow)) {
            panic!("send to bg pane: {e}");
        }

        let config = Config::default();
        let ctx = egui::Context::default();
        // No painter here, so the egui 0.36 `TexturesDelta` drop-bomb (#8356)
        // must be defused explicitly -- see A2 in EGUI_UPGRADE_ASSUMPTIONS.md.
        let mut full_output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let _ = drain_window_manipulation_commands(
                ui,
                &mut tabs,
                8,
                16,
                WindowFocus::Focused,
                &config,
            );
        });
        full_output.textures_delta.clear();

        let minimize_commands: usize = full_output
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .map_or(0, |vp| {
                vp.commands
                    .iter()
                    .filter(|c| matches!(c, egui::ViewportCommand::Minimized(true)))
                    .count()
            });

        assert_eq!(
            minimize_commands, 1,
            "exactly one Minimized(true) command must reach the OS window — from the \
             active pane only, never from the non-active tab's discarded copy"
        );
    }
}
