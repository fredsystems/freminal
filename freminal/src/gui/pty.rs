// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! PTY tab spawning and consumer thread.
//!
//! Provides [`spawn_pty_tab`] which creates a new `TerminalEmulator`,
//! wires all channels, spawns the PTY consumer thread, and returns the
//! GUI-side channel endpoints as a [`TabChannels`].

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use anyhow::Result;
use arc_swap::ArcSwap;
use crossbeam_channel::{Receiver, Sender, unbounded};
use freminal_common::args::Args;
use freminal_common::buffer_states::command_block::CommandBlock;
use freminal_common::buffer_states::modes::theme::Theming;
use freminal_common::buffer_states::tchar::TChar;
use freminal_common::buffer_states::window_manipulation::WindowManipulation;
use freminal_common::pty_write::{FreminalTerminalSize, PtyWrite};
use freminal_common::send_or_log;
use freminal_terminal_emulator::interface::TerminalEmulator;
use freminal_terminal_emulator::io::{InputEvent, WindowCommand};
use freminal_terminal_emulator::recording::{EventPayload, RecordingSwap};
use freminal_terminal_emulator::snapshot::TerminalSnapshot;
use freminal_windowing::{RepaintProxy, WindowId};

/// A finished-command event delivered from a PTY consumer thread to the GUI.
///
/// Produced by Task 72.3 when the terminal handler sees an `OSC 133 D` marker,
/// queued onto the handler's `pending_command_events` vector, and drained by
/// the PTY consumer thread after each batch (Task 72.9). One event per
/// completed shell command. The GUI uses these to populate per-pane recent
/// command history and to set the unfocused-tab pending-event indicator
/// (visual indicator is rendered in Task 72.10).
///
/// `pane_id` is the `recording_pane_id` (`PaneId.raw() as u32`) of the
/// originating pane, which the GUI maps back to its [`super::panes::PaneId`]
/// to locate the receiving pane.
#[derive(Debug, Clone)]
pub struct CommandFinishedEvent {
    /// The originating pane's `recording_pane_id` (`PaneId.raw() as u32`).
    pub pane_id: u32,
    /// The completed command block produced by the terminal handler.
    pub block: CommandBlock,
}

/// Wrap each `CommandBlock` in a [`CommandFinishedEvent`] tagged with
/// `pane_id` and forward it on `tx`.
///
/// Extracted from the PTY consumer thread's `post_event` closure (Task 72.9)
/// so the transport contract — "drained blocks become events tagged with the
/// originating pane" — is unit-testable without spinning up a real shell.
///
/// Send failures are logged but not propagated; a closed receiver indicates
/// the GUI has already shut down, which is a benign race with the consumer
/// thread's own shutdown path.
pub(crate) fn forward_command_events(
    blocks: Vec<CommandBlock>,
    pane_id: u32,
    tx: &Sender<CommandFinishedEvent>,
) {
    for block in blocks {
        send_or_log!(
            tx,
            CommandFinishedEvent { pane_id, block },
            "Failed to send command-finished event to GUI"
        );
    }
}

/// The GUI-side endpoints needed to communicate with a single PTY tab.
///
/// Returned by [`spawn_pty_tab`] after the PTY consumer thread has been
/// launched.  All fields are consumed by `gui::tabs::Tab` (or by `gui::run()`
/// for the initial single-tab path).
pub struct TabChannels {
    /// Lock-free snapshot handle published by the PTY consumer thread.
    pub arc_swap: Arc<ArcSwap<TerminalSnapshot>>,

    /// Sender for input events (key, resize, focus) to the PTY thread.
    pub input_tx: Sender<InputEvent>,

    /// Sender for raw bytes back to the PTY (Report* responses).
    pub pty_write_tx: Sender<PtyWrite>,

    /// Receiver for window commands from the PTY thread.
    pub window_cmd_rx: Receiver<WindowCommand>,

    /// Receiver for clipboard text extraction responses from the PTY thread.
    pub clipboard_rx: Receiver<String>,

    /// Receiver for full-buffer search content from the PTY thread.
    ///
    /// When the GUI sends `InputEvent::RequestSearchBuffer`, the PTY thread
    /// concatenates scrollback + visible `TChar` data and sends it here.
    /// The first element of the tuple is `total_rows` at the time the buffer
    /// was captured, used by the GUI to detect stale responses.
    pub search_buffer_rx: Receiver<(usize, Vec<TChar>)>,

    /// Signals that the PTY process has exited.
    ///
    /// The PTY consumer thread sends `()` on this channel when the child
    /// process exits or the PTY read channel closes.  The GUI polls this
    /// to close the tab (or the whole app if it was the last tab).
    pub pty_dead_rx: Receiver<()>,

    /// Receiver for [`CommandFinishedEvent`]s produced by OSC 133 D markers.
    ///
    /// The PTY consumer thread drains `TerminalHandler::drain_command_events`
    /// after each batch and forwards every finished `CommandBlock` here,
    /// tagged with this pane's `recording_pane_id`. The GUI uses this to
    /// populate per-pane recent-command history (Task 72.9) and ultimately
    /// to drive Task 76 notifications and Task 72.10 visual indicators.
    pub command_event_rx: Receiver<CommandFinishedEvent>,

    /// Shared atomic flag reflecting whether the PTY slave currently has
    /// `ECHO` disabled (i.e. a password prompt is active).
    ///
    /// The GUI reads this directly every frame (via `Relaxed` atomic load)
    /// instead of going through `TerminalSnapshot`, because snapshots are
    /// only published on PTY output — if the shell is idle waiting for a
    /// password, the snapshot would be stale.
    pub echo_off: Arc<AtomicBool>,

    /// OS process ID of the PTY child shell.
    ///
    /// Used for CWD discovery via [`crate::gui::platform::read_cwd`] when
    /// saving layouts or building recording topology snapshots.
    /// `None` on platforms where `portable_pty` cannot report the PID.
    pub child_pid: Option<u32>,

    /// Per-pane shell-history seed populated asynchronously by
    /// [`crate::gui::shell_history::spawn_loader`] at spawn time.
    ///
    /// `OnceLock` is empty until the loader thread reads and parses the
    /// shell's history file; thereafter it holds at most
    /// [`crate::gui::shell_history::HISTORY_SEED_CAP`] entries.  Consumed
    /// by the Quick Command History Palette (Task 72.15) to surface
    /// historical commands alongside the live `recent_commands` ring.
    /// Empty for non-shell programs and for shells freminal does not
    /// recognise.
    pub history_seed: crate::gui::shell_history::SharedSeededHistory,

    /// Resolved shell program (if any), used by the GUI to re-trigger the
    /// shell-history loader when `OSC 1338 ; HISTFILE=<path>` arrives so
    /// the right parser is selected.  `None` when a positional `command`
    /// was specified or when no shell could be resolved.
    pub shell_program: Option<std::path::PathBuf>,
}

/// Per-pane configuration forwarded to the PTY child process.
///
/// Carries optional overrides from a layout file: shell binary, extra
/// environment variables, and working directory.  All fields are `None`
/// / empty when spawning a regular (non-layout) pane.
pub struct PtyTabConfig<'a> {
    /// Working directory for the child process.
    pub cwd: Option<&'a Path>,
    /// Shell executable override (replaces the global `--shell` / default shell).
    pub shell_override: Option<&'a str>,
    /// Extra environment variables to set on the child process.
    pub extra_env: Option<&'a std::collections::HashMap<String, String>>,
    /// Shared, hot-swappable FREC v2 recording handle. The pane observes
    /// the current `Option<RecordingHandle>` on every event; turning
    /// recording on or off at runtime requires no rewiring.
    pub recording_swap: RecordingSwap,
    /// Pane ID used in FREC v2 recording event payloads.
    pub recording_pane_id: u32,
    /// When `true`, set `TERM_PROGRAM=freminal` on the child.  Forwarded
    /// from `config.shell_integration.set_term_program` (Task 72.6).
    pub set_term_program: bool,
}

/// Spawn a new PTY-backed terminal and its consumer thread.
///
/// Creates a `TerminalEmulator`, sets the given theme, wires all channels,
/// and spawns the PTY consumer thread.  Returns the GUI-side channel
/// endpoints as a [`TabChannels`].
///
/// The `repaint_handle` is shared with the PTY thread so it can request
/// repaints after publishing new snapshots.
///
/// # Errors
///
/// Returns an error if `TerminalEmulator::new` fails (e.g. the shell
/// cannot be started).
pub fn spawn_pty_tab(
    args: &Args,
    scrollback_limit: usize,
    theme: &'static freminal_common::themes::ThemePalette,
    auto_detect_urls: bool,
    repaint_handle: &Arc<OnceLock<(RepaintProxy, WindowId)>>,
    initial_size: FreminalTerminalSize,
    tab_cfg: PtyTabConfig<'_>,
) -> Result<TabChannels> {
    let (mut terminal, pty_read_rx) = TerminalEmulator::new(
        args,
        Some(scrollback_limit),
        initial_size,
        tab_cfg.cwd,
        tab_cfg.extra_env,
        tab_cfg.shell_override,
        tab_cfg.recording_pane_id,
        tab_cfg.set_term_program,
    )?;

    // Apply the configured theme so all snapshots carry the correct palette.
    terminal.internal.handler.set_theme(theme);

    // Apply the auto URL detection flag so the buffer's flatten cache
    // surfaces auto-detected URLs in `FormatTag.url` entries.
    terminal
        .internal
        .handler
        .buffer_mut()
        .set_auto_detect_urls(auto_detect_urls);

    // Shared snapshot (ArcSwap).
    let arc_swap: Arc<ArcSwap<TerminalSnapshot>> =
        Arc::new(ArcSwap::from_pointee(TerminalSnapshot::empty()));
    let arc_swap_gui = Arc::clone(&arc_swap);

    let pty_write_tx = terminal.clone_write_tx();
    let child_exit_rx = terminal.child_exit_rx();
    let echo_off = terminal
        .echo_off_atomic()
        .unwrap_or_else(|| Arc::new(AtomicBool::new(false)));
    let reader_shutdown = terminal
        .reader_shutdown_atomic()
        .unwrap_or_else(|| Arc::new(AtomicBool::new(false)));
    let child_pid = terminal.child_pid();

    let (input_tx, input_rx) = unbounded::<InputEvent>();
    let (window_cmd_tx, window_cmd_rx) = unbounded::<WindowCommand>();
    let (clipboard_tx, clipboard_rx) = crossbeam_channel::bounded::<String>(1);
    let (search_buffer_tx, search_buffer_rx) = crossbeam_channel::bounded::<(usize, Vec<TChar>)>(1);
    let (pty_dead_tx, pty_dead_rx) = crossbeam_channel::bounded::<()>(1);
    let (command_event_tx, command_event_rx) = unbounded::<CommandFinishedEvent>();

    let repaint_handle_pty = Arc::clone(repaint_handle);

    // Resolve the shell program (if any) and kick off the asynchronous
    // shell-history loader for Task 72.15.  Mirrors the resolution logic
    // in `freminal_terminal_emulator::io::pty::resolve_command`:
    // explicit `--command` wins (no shell -> no history), else
    // shell_override, else --shell, else `$SHELL`.  The loader thread
    // writes the parsed history into `history_seed` once; the slot is
    // empty until then and the palette degrades gracefully to
    // "live commands only".
    //
    // The resolved shell program is also forwarded through `TabChannels`
    // so the GUI thread can re-trigger the loader with an explicit
    // `OSC 1338`-supplied HISTFILE path (and pick the right parser).
    let history_seed: crate::gui::shell_history::SharedSeededHistory =
        crate::gui::shell_history::new_seeded_history();
    let shell_program: Option<std::path::PathBuf> = if args.command.is_empty() {
        let resolved_shell: Option<std::path::PathBuf> = tab_cfg
            .shell_override
            .map(std::path::PathBuf::from)
            .or_else(|| args.shell.as_deref().map(std::path::PathBuf::from))
            .or_else(|| std::env::var_os("SHELL").map(std::path::PathBuf::from));
        if let Some(program) = resolved_shell.as_ref() {
            // Snapshot the parent process env once for the loader thread
            // so it sees the same HISTFILE / HOME / XDG_DATA_HOME freminal
            // was launched with.  Runtime rc-file overrides inside the
            // spawned child shell are reported via OSC 1338; see
            // `app_impl::draw` for the reload trigger.
            let env_snapshot: std::collections::HashMap<String, String> =
                std::env::vars().collect();
            crate::gui::shell_history::spawn_loader(
                program.clone(),
                env_snapshot,
                Arc::clone(&history_seed),
            );
        }
        resolved_shell
    } else {
        None
    };

    spawn_pty_consumer_thread(
        terminal,
        pty_read_rx,
        input_rx,
        window_cmd_tx,
        clipboard_tx,
        search_buffer_tx,
        child_exit_rx,
        arc_swap,
        repaint_handle_pty,
        pty_dead_tx,
        tab_cfg.recording_swap,
        tab_cfg.recording_pane_id,
        command_event_tx,
        reader_shutdown,
    );

    Ok(TabChannels {
        arc_swap: arc_swap_gui,
        input_tx,
        pty_write_tx,
        window_cmd_rx,
        clipboard_rx,
        search_buffer_rx,
        pty_dead_rx,
        echo_off,
        child_pid,
        command_event_rx,
        history_seed,
        shell_program,
    })
}

/// Spawn the PTY consumer thread that owns a `TerminalEmulator`.
///
/// This thread:
/// - Receives raw PTY output and feeds it to the emulator
/// - Receives input events from the GUI and forwards them
/// - Publishes snapshots via `ArcSwap` after each batch
/// - Sends window commands back to the GUI
///
/// The thread exits when the input channel closes (GUI exited), the PTY
/// read channel closes (shell exited), or the child-exit signal fires.
// Inherently large: the PTY consumer thread event loop. Each section handles a different
// signal (PTY read, GUI input, child exit) and must remain together for clarity.
#[allow(clippy::too_many_lines, clippy::too_many_arguments)]
fn spawn_pty_consumer_thread(
    terminal: TerminalEmulator,
    pty_read_rx: Receiver<freminal_terminal_emulator::io::PtyRead>,
    input_rx: Receiver<InputEvent>,
    window_cmd_tx: Sender<WindowCommand>,
    clipboard_tx: Sender<String>,
    search_buffer_tx: Sender<(usize, Vec<TChar>)>,
    child_exit_rx: Option<Receiver<()>>,
    arc_swap: Arc<ArcSwap<TerminalSnapshot>>,
    repaint_handle: Arc<OnceLock<(RepaintProxy, WindowId)>>,
    pty_dead_tx: Sender<()>,
    recording_swap: RecordingSwap,
    recording_pane_id: u32,
    command_event_tx: Sender<CommandFinishedEvent>,
    reader_shutdown: Arc<AtomicBool>,
) {
    let thread_name = format!("freminal-pty-consumer-{recording_pane_id}");
    if let Err(e) = std::thread::Builder::new()
        .name(thread_name)
        .spawn(move || {
            let mut emulator = terminal;

            let child_exit = child_exit_rx.unwrap_or_else(crossbeam_channel::never::<()>);

            // Helper closure: drain window commands and command-finished
            // events, publish snapshot, request repaint.
            let post_event =
                |emulator: &mut TerminalEmulator,
                 window_cmd_tx: &crossbeam_channel::Sender<WindowCommand>,
                 arc_swap: &ArcSwap<TerminalSnapshot>,
                 repaint_handle: &OnceLock<(RepaintProxy, WindowId)>| {
                    let cmds: Vec<_> = emulator.internal.window_commands.drain(..).collect();
                    for cmd in cmds {
                        let wc = match &cmd {
                            WindowManipulation::ReportWindowState
                            | WindowManipulation::ReportWindowPositionWholeWindow
                            | WindowManipulation::ReportWindowPositionTextArea
                            | WindowManipulation::ReportWindowSizeInPixels
                            | WindowManipulation::ReportWindowTextAreaSizeInPixels
                            | WindowManipulation::ReportRootWindowSizeInPixels
                            | WindowManipulation::ReportIconLabel
                            | WindowManipulation::ReportTitle
                            | WindowManipulation::QueryClipboard(_)
                            // OSC 99 display and control requests drive reverse writes
                            // back to the originating pane's pty_write_tx (Tasks
                            // 99.5c/99.6/99.7), so they are classified as Report like
                            // the other PTY-response-producing variants above.
                            | WindowManipulation::Notification99(_)
                            | WindowManipulation::Osc99Control { .. } => {
                                WindowCommand::Report(cmd)
                            }
                            _ => WindowCommand::Viewport(cmd),
                        };
                        send_or_log!(window_cmd_tx, wc, "Failed to send window command to GUI");
                    }

                    // Drain finished-command events queued by the FTCS OSC 133 D
                    // handler (Task 72.3) and forward them to the GUI tagged with
                    // this pane's recording_pane_id (Task 72.9).
                    let events = emulator.internal.handler.drain_command_events();
                    forward_command_events(events, recording_pane_id, &command_event_tx);

                    let snap = emulator.build_snapshot();
                    arc_swap.store(Arc::new(snap));

                    if let Some((proxy, wid)) = repaint_handle.get() {
                        proxy.request_repaint_after(*wid, std::time::Duration::from_millis(8));
                    }
                };

            // Helper closure: process a single InputEvent.
            let handle_input =
                |emulator: &mut TerminalEmulator,
                 msg: std::result::Result<InputEvent, crossbeam_channel::RecvError>,
                 clipboard_tx: &crossbeam_channel::Sender<String>,
                 search_buffer_tx: &crossbeam_channel::Sender<(usize, Vec<TChar>)>|
                 -> bool {
                    match msg {
                        Ok(InputEvent::Resize(w, h, pw, ph)) => {
                            if let Some(rec) = recording_swap.load_full() {
                                rec.emit(EventPayload::PaneResize {
                                    pane_id: recording_pane_id,
                                    cols: w.try_into().unwrap_or(u32::MAX),
                                    rows: h.try_into().unwrap_or(u32::MAX),
                                });
                            }
                            emulator.handle_resize_event(w, h, pw, ph);
                        }
                        Ok(InputEvent::Key(bytes)) => {
                            if let Err(e) = emulator.write_raw_bytes(&bytes) {
                                error!("Failed to forward key bytes to PTY: {e}");
                            }
                            if let Some(rec) = recording_swap.load_full() {
                                rec.emit(EventPayload::PtyInput {
                                    pane_id: recording_pane_id,
                                    data: bytes,
                                });
                            }
                        }
                        Ok(InputEvent::FocusChange(focused)) => {
                            emulator.internal.send_focus_event(focused);
                        }
                        Ok(InputEvent::ScrollOffset { offset, extra_rows }) => {
                            emulator.set_gui_scroll_window(offset, extra_rows);
                        }
                        Ok(InputEvent::ThemeChange(theme)) => {
                            emulator.internal.handler.set_theme(theme);
                        }
                        Ok(InputEvent::AutoDetectUrls(enabled)) => {
                            emulator
                                .internal
                                .handler
                                .buffer_mut()
                                .set_auto_detect_urls(enabled);
                        }
                        Ok(InputEvent::ThemeModeUpdate(theme_mode, os_is_dark)) => {
                            emulator.internal.modes.theme_mode = theme_mode;
                            // Sync the live theming state to match the OS preference
                            // so that ?2031 queries reflect reality immediately.
                            if os_is_dark {
                                emulator.internal.modes.theming = Theming::Dark;
                            } else {
                                emulator.internal.modes.theming = Theming::Light;
                            }
                        }
                        Ok(InputEvent::ExtractSelection {
                            start_row,
                            start_col,
                            end_row,
                            end_col,
                            is_block,
                        }) => {
                            let text = emulator.extract_selection_text(
                                start_row, start_col, end_row, end_col, is_block,
                            );
                            let _ = clipboard_tx.send(text);
                        }
                        Ok(InputEvent::RequestSearchBuffer) => {
                            let (chars, _tags) =
                                emulator.internal.handler.data_and_format_data_for_gui(0);
                            let mut combined = chars.scrollback;
                            combined.extend(chars.visible);
                            let total_rows = emulator.internal.handler.buffer().rows().len();
                            let _ = search_buffer_tx.send((total_rows, combined));
                        }
                        Ok(InputEvent::ClearScrollback) => {
                            // Drop every scrollback row; the visible display
                            // is unaffected. Also reset the PTY-side
                            // gui_scroll_offset so snapshots immediately render
                            // from the live view (the GUI resets its local
                            // ViewState::scroll_offset in parallel).
                            emulator.internal.handler.buffer_mut().erase_scrollback();
                            emulator.set_gui_scroll_offset(0);
                        }
                        Err(_) => {
                            info!("Input channel closed; consumer thread exiting");
                            return false;
                        }
                    }
                    true
                };

            // Primary loop: service PTY reads, GUI input events, and child-exit signals.
            loop {
                crossbeam_channel::select! {
                    recv(pty_read_rx) -> msg => {
                        if let Ok(read) = msg {
                            let data = &read.buf[0..read.read_amount];
                            if let Some(rec) = recording_swap.load_full() {
                                rec.emit(EventPayload::PtyOutput {
                                    pane_id: recording_pane_id,
                                    data: data.to_vec(),
                                });
                            }
                            emulator.handle_incoming_data(data);
                        } else {
                            info!("PTY read channel closed; signaling tab death");
                            post_event(&mut emulator, &window_cmd_tx, &arc_swap, &repaint_handle);
                            let _ = pty_dead_tx.send(());
                            if let Some((proxy, wid)) = repaint_handle.get() {
                                proxy.request_repaint(*wid);
                            }
                            return;
                        }
                    }
                    recv(input_rx) -> msg => {
                        if !handle_input(&mut emulator, msg, &clipboard_tx, &search_buffer_tx) {
                            // The GUI dropped the pane's input channel — the
                            // pane/tab/window is being torn down while the
                            // child shell may still be alive. Signal the PTY
                            // reader thread so a subsequent failed `send` (the
                            // receiver we own is about to drop) is treated as
                            // an expected teardown, not an error.
                            reader_shutdown.store(true, Ordering::Release);
                            return;
                        }
                    }
                    recv(child_exit) -> _ => {
                        info!("Child process exited; draining remaining PTY output");
                        let drain_deadline = std::time::Duration::from_millis(200);
                        while let Ok(read) = pty_read_rx.recv_timeout(drain_deadline) {
                            emulator.handle_incoming_data(
                                &read.buf[0..read.read_amount],
                            );
                        }

                        info!("PTY drain complete; signaling tab death");
                        post_event(&mut emulator, &window_cmd_tx, &arc_swap, &repaint_handle);
                        let _ = pty_dead_tx.send(());
                        if let Some((proxy, wid)) = repaint_handle.get() {
                            proxy.request_repaint(*wid);
                        }
                        return;
                    }
                }

                post_event(&mut emulator, &window_cmd_tx, &arc_swap, &repaint_handle);
            }
        })
    {
        error!("Failed to spawn PTY consumer thread: {e}");
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// Helper: build a fresh `CommandBlock` with the given fid.
    fn block_with_fid(fid: &str) -> CommandBlock {
        CommandBlock::new_running(0, None, fid.to_owned())
    }

    #[test]
    fn forward_command_events_empty_input_sends_nothing() {
        let (tx, rx) = crossbeam_channel::unbounded::<CommandFinishedEvent>();
        forward_command_events(Vec::new(), 42, &tx);
        assert!(
            rx.try_recv().is_err(),
            "no events should be sent for an empty input"
        );
    }

    #[test]
    fn forward_command_events_preserves_order_and_pane_id() {
        let (tx, rx) = crossbeam_channel::unbounded::<CommandFinishedEvent>();
        let blocks = vec![
            block_with_fid("a"),
            block_with_fid("b"),
            block_with_fid("c"),
        ];
        let original_ids: Vec<_> = blocks.iter().map(|b| b.id).collect();

        forward_command_events(blocks, 7, &tx);

        // All three events must arrive, in order, tagged with pane_id 7.
        for (i, expected_id) in original_ids.iter().enumerate() {
            let ev = rx
                .try_recv()
                .unwrap_or_else(|_| panic!("expected event #{i}"));
            assert_eq!(ev.pane_id, 7, "event #{i} pane_id mismatch");
            assert_eq!(ev.block.id, *expected_id, "event #{i} block id mismatch");
        }
        assert!(rx.try_recv().is_err(), "no extra events should be sent");
    }

    #[test]
    fn forward_command_events_with_closed_receiver_does_not_panic() {
        // The GUI may have shut down before the consumer thread's final
        // drain. A closed receiver must be a benign no-op (logged, not
        // propagated) — this matches the consumer thread's own shutdown
        // semantics.
        let (tx, rx) = crossbeam_channel::unbounded::<CommandFinishedEvent>();
        drop(rx);
        forward_command_events(vec![block_with_fid("x")], 1, &tx);
        // No assertion needed: not panicking is the contract.
    }
}
