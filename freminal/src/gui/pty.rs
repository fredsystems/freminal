// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! PTY tab spawning and consumer thread.
//!
//! Provides [`spawn_pty_tab`] which creates a new `TerminalEmulator`,
//! wires all channels, spawns the PTY consumer thread, and returns the
//! GUI-side channel endpoints as a [`TabChannels`].

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, OnceLock};

use anyhow::Result;
use arc_swap::ArcSwap;
use crossbeam_channel::{Receiver, Sender, unbounded};
use freminal_common::args::Args;
use freminal_common::buffer_states::tchar::TChar;
use freminal_common::buffer_states::window_manipulation::WindowManipulation;
use freminal_common::pty_write::PtyWrite;
use freminal_terminal_emulator::interface::TerminalEmulator;
use freminal_terminal_emulator::io::{InputEvent, WindowCommand};
use freminal_terminal_emulator::snapshot::TerminalSnapshot;

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

    /// Shared atomic flag reflecting whether the PTY slave currently has
    /// `ECHO` disabled (i.e. a password prompt is active).
    ///
    /// The GUI reads this directly every frame (via `Relaxed` atomic load)
    /// instead of going through `TerminalSnapshot`, because snapshots are
    /// only published on PTY output — if the shell is idle waiting for a
    /// password, the snapshot would be stale.
    pub echo_off: Arc<AtomicBool>,
}

/// Spawn a new PTY-backed terminal and its consumer thread.
///
/// Creates a `TerminalEmulator`, sets the given theme, wires all channels,
/// and spawns the PTY consumer thread.  Returns the GUI-side channel
/// endpoints as a [`TabChannels`].
///
/// The `egui_ctx` handle is shared with the PTY thread so it can request
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
    egui_ctx: &Arc<OnceLock<eframe::egui::Context>>,
) -> Result<TabChannels> {
    let (mut terminal, pty_read_rx) = TerminalEmulator::new(args, Some(scrollback_limit))?;

    // Apply the configured theme so all snapshots carry the correct palette.
    terminal.internal.handler.set_theme(theme);

    // Shared snapshot (ArcSwap).
    let arc_swap: Arc<ArcSwap<TerminalSnapshot>> =
        Arc::new(ArcSwap::from_pointee(TerminalSnapshot::empty()));
    let arc_swap_gui = Arc::clone(&arc_swap);

    let pty_write_tx = terminal.clone_write_tx();
    let child_exit_rx = terminal.child_exit_rx();
    let echo_off = terminal
        .echo_off_atomic()
        .unwrap_or_else(|| Arc::new(AtomicBool::new(false)));

    let (input_tx, input_rx) = unbounded::<InputEvent>();
    let (window_cmd_tx, window_cmd_rx) = unbounded::<WindowCommand>();
    let (clipboard_tx, clipboard_rx) = crossbeam_channel::bounded::<String>(1);
    let (search_buffer_tx, search_buffer_rx) = crossbeam_channel::bounded::<(usize, Vec<TChar>)>(1);
    let (pty_dead_tx, pty_dead_rx) = crossbeam_channel::bounded::<()>(1);

    let egui_ctx_pty = Arc::clone(egui_ctx);

    spawn_pty_consumer_thread(
        terminal,
        pty_read_rx,
        input_rx,
        window_cmd_tx,
        clipboard_tx,
        search_buffer_tx,
        child_exit_rx,
        arc_swap,
        egui_ctx_pty,
        pty_dead_tx,
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
    egui_ctx_pty: Arc<OnceLock<eframe::egui::Context>>,
    pty_dead_tx: Sender<()>,
) {
    std::thread::spawn(move || {
        let mut emulator = terminal;

        let child_exit = child_exit_rx.unwrap_or_else(crossbeam_channel::never::<()>);

        // Helper closure: drain window commands, publish snapshot, request repaint.
        let post_event = |emulator: &mut TerminalEmulator,
                          window_cmd_tx: &crossbeam_channel::Sender<WindowCommand>,
                          arc_swap: &ArcSwap<TerminalSnapshot>,
                          egui_ctx_pty: &OnceLock<eframe::egui::Context>| {
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
                    | WindowManipulation::QueryClipboard(_) => WindowCommand::Report(cmd),
                    _ => WindowCommand::Viewport(cmd),
                };
                if let Err(e) = window_cmd_tx.send(wc) {
                    error!("Failed to send window command to GUI: {e}");
                }
            }

            let snap = emulator.build_snapshot();
            arc_swap.store(Arc::new(snap));

            if let Some(ctx) = egui_ctx_pty.get() {
                ctx.request_repaint_after(std::time::Duration::from_millis(8));
            }
        };

        // Helper closure: process a single InputEvent.
        let handle_input = |emulator: &mut TerminalEmulator,
                            msg: std::result::Result<InputEvent, crossbeam_channel::RecvError>,
                            clipboard_tx: &crossbeam_channel::Sender<String>,
                            search_buffer_tx: &crossbeam_channel::Sender<(usize, Vec<TChar>)>|
         -> bool {
            match msg {
                Ok(InputEvent::Resize(w, h, pw, ph)) => {
                    emulator.handle_resize_event(w, h, pw, ph);
                }
                Ok(InputEvent::Key(bytes)) => {
                    if let Err(e) = emulator.write_raw_bytes(&bytes) {
                        error!("Failed to forward key bytes to PTY: {e}");
                    }
                }
                Ok(InputEvent::FocusChange(focused)) => {
                    emulator.internal.send_focus_event(focused);
                }
                Ok(InputEvent::ScrollOffset(offset)) => {
                    emulator.set_gui_scroll_offset(offset);
                }
                Ok(InputEvent::ThemeChange(theme)) => {
                    emulator.internal.handler.set_theme(theme);
                }
                Ok(InputEvent::ExtractSelection {
                    start_row,
                    start_col,
                    end_row,
                    end_col,
                    is_block,
                }) => {
                    let text = emulator
                        .extract_selection_text(start_row, start_col, end_row, end_col, is_block);
                    let _ = clipboard_tx.send(text);
                }
                Ok(InputEvent::RequestSearchBuffer) => {
                    let (chars, _tags) = emulator.internal.handler.data_and_format_data_for_gui(0);
                    let mut combined = chars.scrollback;
                    combined.extend(chars.visible);
                    let total_rows = emulator.internal.handler.buffer().get_rows().len();
                    let _ = search_buffer_tx.send((total_rows, combined));
                }
                #[cfg(feature = "playback")]
                Ok(InputEvent::PlaybackControl(_)) => {
                    // Playback commands are handled by the dedicated playback
                    // consumer thread, not the normal PTY consumer.  Ignore.
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
                        emulator.handle_incoming_data(
                            &read.buf[0..read.read_amount],
                        );
                    } else {
                        info!("PTY read channel closed; signaling tab death");
                        post_event(&mut emulator, &window_cmd_tx, &arc_swap, &egui_ctx_pty);
                        let _ = pty_dead_tx.send(());
                        if let Some(ctx) = egui_ctx_pty.get() {
                            ctx.request_repaint();
                        }
                        return;
                    }
                }
                recv(input_rx) -> msg => {
                    if !handle_input(&mut emulator, msg, &clipboard_tx, &search_buffer_tx) {
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
                    post_event(&mut emulator, &window_cmd_tx, &arc_swap, &egui_ctx_pty);
                    let _ = pty_dead_tx.send(());
                    if let Some(ctx) = egui_ctx_pty.get() {
                        ctx.request_repaint();
                    }
                    return;
                }
            }

            post_event(&mut emulator, &window_cmd_tx, &arc_swap, &egui_ctx_pty);
        }
    });
}
