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
use freminal_terminal_emulator::io::{InputEvent, PtyRead, WindowCommand};
use freminal_terminal_emulator::recording::{EventPayload, RecordingSwap};
use freminal_terminal_emulator::snapshot::TerminalSnapshot;
use freminal_terminal_emulator::terminal_handler::TerminalHandler;
use freminal_windowing::{RepaintProxy, WindowId};

/// Ask the system allocator to release free heap pages back to the OS.
///
/// After idle scrollback compaction (Task 118) frees a large number of small
/// allocations (`Vec<Cell>` / `RowCacheEntry`), the glibc allocator keeps
/// those pages in its per-arena free lists rather than `munmap`-ing them, so
/// process RSS stays high even though the freed memory is no longer live. A
/// one-shot `malloc_trim(0)` at the point the compaction backlog has fully
/// drained returns the freed pages to the OS (empirically ~820 MB → ~380 MB
/// after catting a 100k-line file into a pane). On non-glibc platforms this is
/// a no-op: other allocators (macOS libmalloc, Windows) manage return-to-OS
/// themselves and expose no equivalent portable call.
// On non-glibc targets the body is empty, so clippy sees a trivially-const fn
// and fires `missing_const_for_fn`; on glibc it makes a non-const FFI call and
// cannot be const. The signature must be uniform across targets, so suppress
// the conditional lint rather than split the definition.
#[allow(clippy::missing_const_for_fn)]
fn release_freed_heap() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        // SAFETY: `malloc_trim` is a glibc entry point with no preconditions;
        // it only consolidates and releases already-free heap and returns
        // non-zero if any memory was released. It cannot affect live
        // allocations or Rust's memory safety.
        unsafe extern "C" {
            fn malloc_trim(pad: usize) -> core::ffi::c_int;
        }
        unsafe {
            let _ = malloc_trim(0);
        }
    }
}

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

/// Upper bound on how many *additional* queued `PtyRead` messages a single
/// [`drain_pty_reads`] call folds into one batch before yielding back to the
/// consumer thread's `select!` loop.
///
/// The blocking-recv'd message is always processed; this caps only the
/// non-blocking `try_recv` drain that follows. It exists so a sustained
/// high-throughput producer that outpaces parsing cannot keep the consumer
/// inside the drain loop indefinitely and starve `input_rx` / `child_exit`.
/// At the reader thread's 4096-byte read buffer, 64 chunks is ~256 KiB of
/// output per batch — comfortably more than any single interactive redraw,
/// while bounding worst-case input-handling latency to one batch of parses.
const MAX_PTY_READ_BATCH: usize = 64;

/// Feed a just-received `PtyRead` and every `PtyRead` already queued behind it
/// (up to [`MAX_PTY_READ_BATCH`]) into `sink`, in arrival order, in a single
/// batch (issue #439).
///
/// The PTY consumer thread's `select!` loop calls `post_event`
/// (`build_snapshot` then `arc_swap.store` then a repaint request) exactly once
/// per loop iteration. Before this batching, a single visual redraw from a
/// full-screen TUI (btop, htop, vim, less) exceeded the reader thread's
/// 4096-byte buffer and so arrived as N separate `PtyRead` messages, each
/// triggering its own iteration and thus its own full vertex rebuild plus
/// repaint: a ~20-40x over-draw for a screen that visually changes ~2x/sec.
///
/// This helper feeds `first` (the message the blocking `recv` already took)
/// and then non-blockingly drains up to [`MAX_PTY_READ_BATCH`] further queued
/// messages via `try_recv`, so a burst of currently-available output is folded
/// into the emulator before the single trailing `post_event`. It mirrors the
/// write-side drain idiom in `freminal-terminal-emulator/src/io/pty.rs`.
///
/// The batch is **capped** rather than draining to exhaustion: under a
/// sustained high-throughput producer that outpaces parsing (`yes`,
/// `cat bigfile`, a log flood), an unbounded drain could keep the consumer
/// thread inside this loop indefinitely, starving the sibling `select!` arms
/// (`input_rx` keystrokes/resize, `child_exit`). Capping at
/// [`MAX_PTY_READ_BATCH`] bounds worst-case input latency to one batch of
/// parses while still coalescing any realistic single visual redraw (btop's is
/// ~5-6 chunks) into one snapshot. Anything beyond the cap simply stays queued
/// and is drained by the next `select!` iteration, which also re-services the
/// other arms first-come-first-served.
///
/// It is architecturally clean: the consumer thread owns the emulator
/// exclusively, and the terminal handler's `handle_incoming_data` never itself
/// builds a snapshot or requests a repaint — it only mutates emulator state and
/// sets per-row dirty flags that the single trailing `build_snapshot` consults
/// once. Ordering is preserved (crossbeam channels are FIFO), which is required
/// for both correct escape-sequence parsing and byte-accurate recording.
///
/// Extracted as a free function so the "process the first message, then drain
/// up to the cap in order, exactly once each" contract is unit-testable without
/// spinning up a real shell.
fn drain_pty_reads<F>(first: PtyRead, rx: &Receiver<PtyRead>, mut sink: F)
where
    F: FnMut(PtyRead),
{
    sink(first);
    for _ in 0..MAX_PTY_READ_BATCH {
        match rx.try_recv() {
            Ok(read) => sink(read),
            Err(_) => break,
        }
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

/// Already-resolved config values applied once, immediately after a new
/// pane's `TerminalHandler` is constructed.
///
/// This seeds the pane's starting state from what the user has configured,
/// rather than always the hardcoded type defaults. Grouped into one struct
/// (rather than more `spawn_pty_tab` parameters) so
/// adding a future per-pane seed value doesn't push the function over
/// clippy's argument-count limit. All fields here mirror an existing live
/// `InputEvent` used to re-apply the same value to an already-running pane
/// when the user changes it in Settings.
pub struct PtyTabInitialState {
    /// Active color theme (`InputEvent::ThemeChange` is the live-apply
    /// equivalent).
    pub theme: &'static freminal_common::themes::ThemePalette,
    /// Auto-detect plain URLs in terminal output
    /// (`InputEvent::AutoDetectUrls` is the live-apply equivalent).
    pub auto_detect_urls: bool,
    /// Cursor shape/blink style, resolved from `config.cursor` via
    /// [`CursorVisualStyle::from_config`](freminal_common::cursor::CursorVisualStyle::from_config)
    /// (`InputEvent::CursorConfigChange` is the live-apply equivalent;
    /// issue #406).
    pub cursor_style: freminal_common::cursor::CursorVisualStyle,
}

/// Apply `initial_state` to a freshly constructed pane's handler.
///
/// Extracted out of `spawn_pty_tab` so this seeding logic — as opposed to
/// the PTY spawn machinery around it, which needs a real child process — is
/// unit-testable on its own with a bare `TerminalHandler`.
fn apply_initial_state(handler: &mut TerminalHandler, initial_state: PtyTabInitialState) {
    // Apply the configured theme so all snapshots carry the correct palette.
    handler.set_theme(initial_state.theme);

    // Apply the auto URL detection flag so the buffer's flatten cache
    // surfaces auto-detected URLs in `FormatTag.url` entries.
    handler
        .buffer_mut()
        .set_auto_detect_urls(initial_state.auto_detect_urls);

    // Seed the cursor's initial shape/blink from `config.cursor` (issue
    // #406). Like `theme`, this is only the *starting* state: a running
    // program's own DECSCUSR / XTCBlink request still overrides it
    // normally, exactly as on a real terminal.
    handler.set_cursor_visual_style(initial_state.cursor_style);
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
/// Creates a `TerminalEmulator`, sets the given theme and initial cursor
/// style, wires all channels, and spawns the PTY consumer thread.  Returns
/// the GUI-side channel endpoints as a [`TabChannels`].
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
    initial_state: PtyTabInitialState,
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

    apply_initial_state(&mut terminal.internal.handler, initial_state);

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
            // Idle-driven scrollback compaction (Task 118.9b) and compression
            // (Task 119.5): the only place `compact_idle_scrollback` and
            // `compress_idle_scrollback` are ever called from. Fires ~100ms
            // after the last PTY read or GUI input event, compacts a bounded
            // budget of not-yet-compacted scrollback rows, then — once
            // compaction reports nothing left to do this tick — compresses a
            // bounded budget of now-cold compact rows into LZ4 blocks.
            // Re-arms itself while either has work remaining and disarms
            // (via `never()`) once the terminal is fully caught up on both,
            // so a quiescent pane is not woken forever.
            //
            // The interval serves double duty: it is both the idle-detection
            // delay (time since last activity before the first slice runs) and
            // the inter-slice delay (gap between successive bounded slices
            // while a backlog drains). At 1024 compaction rows / 100ms that is
            // ~10k rows/sec, so a freshly-dumped very large scrollback (e.g.
            // 100k lines) fully compacts-then-compresses in ~25s of background
            // idle time (see the compaction-then-compression sequencing in the
            // idle arm below) — correct and never blocking; a middle ground
            // between the original 512-row (~40s) trickle and an aggressive
            // 4096-row budget (~2.5s but a much larger per-tick burst).
            //
            // TODO(Task 118 follow-up): compaction pacing could be made DYNAMIC
            // (larger budget / shorter interval while a large backlog exists,
            // decaying to a gentle cadence once caught up). That is the right
            // way to get fast catch-up without a large fixed per-tick burst on
            // modest hardware; the current fixed budgets are the
            // deliberately-simple stopgap.
            const IDLE_COMPACTION_INTERVAL: std::time::Duration =
                std::time::Duration::from_millis(100);
            // Compaction is an in-place RLE Live->Compact conversion. Measured
            // via `bench_idle_compaction_tick`
            // (`freminal-buffer/benches/buffer_row_bench.rs`, which runs at this
            // exact 1024-row budget) at ~1.6ms per tick on the dev machine.
            // That is a ~1.6ms slice per 100ms tick here during a catch-up
            // burst; even on a CPU ~5x slower (~8ms) it stays far under the
            // 100ms interval and leaves the core idle most of the time, so a
            // full-scrollback catch-up never stalls the PTY tick loop or pegs a
            // core. Kept modest (1024, up from Task 118's 512)
            // rather than aggressive (4096) precisely so the per-tick burst
            // stays small on weak hardware; the price is a slower background
            // catch-up on a pathological all-at-once dump, invisible in normal
            // incremental use.
            const IDLE_COMPACTION_BUDGET: usize = 1024;
            // Compression is a separate idle pass (LZ4 over BLOCK_SIZE-row
            // blocks of already-compacted rows), run only once compaction has
            // caught up for the tick. One full budget's worth (4096 rows = 16
            // blocks of 256) measures ~4.5ms end-to-end on the dev machine
            // (`bench_idle_compression_tick`) — the real
            // `compress_idle_scrollback` path (run-scan + per-block LZ4 +
            // eviction bookkeeping), larger than the ~73µs/block isolated
            // `bench_compressed_block_round_trip` figure but still well under
            // the 100ms interval (and under one 16.6ms frame); ~5x slower is
            // ~22ms, still within one interval. Left at 4096 (vs compaction's
            // 1024) because compression's per-row cost is lower and it only
            // runs after compaction settles, so the larger budget drains the
            // compress backlog without a large burst.
            const IDLE_COMPRESSION_BUDGET: usize = 4096;

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
                        // 16ms == the 60fps frame budget and the same floor
                        // enforced everywhere else (issue #439). The previous
                        // 8ms value bypassed that floor via the unclamped
                        // `RequestRepaintAfter` proxy path, letting a bursty
                        // PTY stream drive the GUI toward ~120fps. The event
                        // loop now also clamps this path to 16ms, so this is
                        // belt-and-braces: request the correct delay AND rely
                        // on the floor as a backstop.
                        proxy.request_repaint_after(*wid, std::time::Duration::from_millis(16));
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
                        Ok(InputEvent::CursorConfigChange(style)) => {
                            emulator.internal.handler.set_cursor_visual_style(style);
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

            let mut idle_deadline: crossbeam_channel::Receiver<std::time::Instant> =
                crossbeam_channel::after(IDLE_COMPACTION_INTERVAL);

            // Tracks whether any scrollback rows were compacted or compressed
            // since the last time we released freed heap back to the OS.
            // Compaction frees a large amount of small allocations
            // (`Vec<Cell>` / `RowCacheEntry`); compression additionally frees
            // a `CompactRow`'s bytes into an LZ4 block. Either way, the
            // system allocator (glibc) retains those pages in its arenas
            // rather than returning them, so RSS stays high until we
            // explicitly trim. We trim only once, when both are fully
            // drained (`compacted == 0 && compressed == 0`) AND real work
            // happened since the last trim — never mid-drain (the pages
            // would just be re-faulted by the next slice) and never on a
            // trivial re-arm that did nothing.
            let mut work_since_trim = false;

            // Primary loop: service PTY reads, GUI input events, child-exit
            // signals, and the idle scrollback-compaction tick.
            loop {
                crossbeam_channel::select! {
                    recv(pty_read_rx) -> msg => {
                        if let Ok(read) = msg {
                            // Batch-drain (issue #439): a single visual redraw
                            // from a full-screen TUI (btop, htop, vim, less)
                            // exceeds the 4096-byte reader buffer, so one
                            // redraw arrives as N `PtyRead` chunks. Feed the
                            // blocking-recv'd chunk AND every chunk already
                            // queued behind it into the emulator, then fall
                            // through to a SINGLE `post_event` (build_snapshot
                            // + arc_swap.store + repaint) at the bottom of the
                            // loop. Without this drain, N chunks became N full
                            // vertex rebuilds + N repaint requests — a ~20-40x
                            // over-draw for a screen that changes ~2x/sec.
                            //
                            // This mirrors the write-side drain idiom in
                            // `io/pty.rs` (`while let Ok(..) = try_recv()`) and
                            // the `child_exit` drain arm below. It is
                            // architecturally clean: this consumer thread owns
                            // the emulator exclusively, and `handle_incoming_data`
                            // never itself builds a snapshot or requests a
                            // repaint — it only mutates emulator state and sets
                            // per-row dirty flags that the single trailing
                            // `build_snapshot` consults once.
                            //
                            // Recording fidelity is preserved: every chunk is
                            // still emitted to the recorder individually, in
                            // arrival order, exactly as before.
                            drain_pty_reads(read, &pty_read_rx, |read: PtyRead| {
                                let data = &read.buf[0..read.read_amount];
                                if let Some(rec) = recording_swap.load_full() {
                                    rec.emit(EventPayload::PtyOutput {
                                        pane_id: recording_pane_id,
                                        data: data.to_vec(),
                                    });
                                }
                                emulator.handle_incoming_data(data);
                            });
                            idle_deadline = crossbeam_channel::after(IDLE_COMPACTION_INTERVAL);
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
                        idle_deadline = crossbeam_channel::after(IDLE_COMPACTION_INTERVAL);
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
                    recv(idle_deadline) -> _ => {
                        // Snapshot content is byte-identical after compaction
                        // or compression (both only change the internal
                        // storage representation), so this arm must never
                        // call `post_event` — doing so would be a spurious
                        // GUI wake and defeat the idle/battery goal.
                        // `continue` skips the trailing `post_event` call
                        // below.
                        //
                        // Compact first, compress second: a row must be
                        // Task-118-compacted before it is a Task-119
                        // compression candidate, so only spend the
                        // compression budget once this tick's compaction
                        // pass reports nothing left to compact — otherwise
                        // compression would scan a scrollback full of `Live`
                        // rows and correctly find nothing, wasting the tick.
                        let buffer = emulator.internal.handler.buffer_mut();
                        let compacted = buffer.compact_idle_scrollback(IDLE_COMPACTION_BUDGET);
                        let compressed = if compacted == 0 {
                            buffer.compress_idle_scrollback(IDLE_COMPRESSION_BUDGET)
                        } else {
                            0
                        };
                        if compacted > 0 || compressed > 0 {
                            // More may remain — keep draining on the next tick.
                            work_since_trim = true;
                            idle_deadline = crossbeam_channel::after(IDLE_COMPACTION_INTERVAL);
                        } else {
                            // Both backlogs fully drained. If we actually did
                            // work since the last trim, release the freed
                            // pages back to the OS now: the transient
                            // allocation churn is over, so the freed heap is
                            // stable and worth returning. Doing this once per
                            // settle (rather than per slice) avoids repeatedly
                            // munmap-ing pages the allocator would re-fault
                            // during an ongoing drain. See `release_freed_heap`.
                            if work_since_trim {
                                release_freed_heap();
                                work_since_trim = false;
                            }
                            idle_deadline = crossbeam_channel::never();
                        }
                        continue;
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
    fn drain_pty_reads_processes_first_then_queue_in_order_exactly_once() {
        // Issue #439: the consumer must fold the blocking-recv'd chunk plus
        // every already-queued chunk into ONE batch, in FIFO order, each
        // exactly once, before the single trailing `post_event`. Collect the
        // read_amounts to prove order + exactly-once without touching a parser.
        let (tx, rx) = crossbeam_channel::unbounded::<PtyRead>();
        tx.send(PtyRead {
            buf: b"CD".to_vec(),
            read_amount: 2,
        })
        .unwrap();
        tx.send(PtyRead {
            buf: b"EF".to_vec(),
            read_amount: 3,
        })
        .unwrap();

        let first = PtyRead {
            buf: b"AB".to_vec(),
            read_amount: 1,
        };
        let mut seen: Vec<usize> = Vec::new();
        drain_pty_reads(first, &rx, |read| seen.push(read.read_amount));

        assert_eq!(
            seen,
            vec![1, 2, 3],
            "first message must be processed first, then the queue in FIFO order, each once"
        );
        assert!(rx.try_recv().is_err(), "the queue must be fully drained");
    }

    #[test]
    fn drain_pty_reads_single_message_processes_only_that_one() {
        // The common steady-state case: nothing queued behind the recv'd
        // message. Exactly one sink call, queue left empty.
        let (_tx, rx) = crossbeam_channel::unbounded::<PtyRead>();
        let first = PtyRead {
            buf: b"x".to_vec(),
            read_amount: 1,
        };
        let mut count = 0u32;
        drain_pty_reads(first, &rx, |_read| count += 1);
        assert_eq!(count, 1, "a lone message must be processed exactly once");
    }

    #[test]
    fn drain_pty_reads_caps_batch_and_leaves_remainder_queued() {
        // Responsiveness guard: a sustained producer must not keep the consumer
        // inside the drain forever. `drain_pty_reads` processes `first` plus at
        // most MAX_PTY_READ_BATCH queued messages, then returns so the caller's
        // `select!` loop can re-service input_rx / child_exit. Anything beyond
        // the cap stays queued for the next iteration.
        let (tx, rx) = crossbeam_channel::unbounded::<PtyRead>();
        // Queue far more than the cap behind the first message.
        let queued = MAX_PTY_READ_BATCH * 3;
        for _ in 0..queued {
            tx.send(PtyRead {
                buf: b"y".to_vec(),
                read_amount: 1,
            })
            .unwrap();
        }
        let first = PtyRead {
            buf: b"y".to_vec(),
            read_amount: 1,
        };
        let mut count = 0usize;
        drain_pty_reads(first, &rx, |_read| count += 1);

        // Exactly first (1) + MAX_PTY_READ_BATCH processed this call.
        assert_eq!(
            count,
            1 + MAX_PTY_READ_BATCH,
            "must process the first message plus at most MAX_PTY_READ_BATCH queued"
        );
        // The rest remain queued for the next select! iteration.
        assert_eq!(
            rx.len(),
            queued - MAX_PTY_READ_BATCH,
            "over-cap messages must stay queued, not be dropped"
        );
    }

    #[test]
    fn drain_pty_reads_feeds_emulator_bytes_in_order() {
        // End-to-end against a real headless emulator: three chunks whose
        // concatenation spells "ABCDEF" must be parsed as "ABCDEF" — an
        // out-of-order or duplicated feed would produce a different string.
        use freminal_terminal_emulator::interface::TerminalEmulator;

        let (mut emu, _write_rx) = TerminalEmulator::new_headless(None);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyRead>();
        tx.send(PtyRead {
            buf: b"CD".to_vec(),
            read_amount: 2,
        })
        .unwrap();
        tx.send(PtyRead {
            buf: b"EF".to_vec(),
            read_amount: 2,
        })
        .unwrap();

        let first = PtyRead {
            buf: b"AB".to_vec(),
            read_amount: 2,
        };
        drain_pty_reads(first, &rx, |read| {
            emu.handle_incoming_data(&read.buf[0..read.read_amount]);
        });

        let text = emu.extract_selection_text(0, 0, 0, 5, false);
        assert!(
            text.contains("ABCDEF"),
            "batched chunks must be parsed in arrival order; got: {text:?}"
        );
        assert!(rx.try_recv().is_err(), "the queue must be fully drained");
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

    #[test]
    fn apply_initial_state_seeds_theme_auto_detect_urls_and_cursor_style() {
        // Regression test for issue #406 (and the pre-existing theme /
        // auto_detect_urls seeding this PR's cursor_style change follows the
        // shape of): `spawn_pty_tab` cannot be unit-tested directly (it
        // spawns a real child process), but the seeding logic it delegates
        // to can be, against a bare `TerminalHandler`.
        use freminal_common::cursor::CursorVisualStyle;
        use freminal_common::themes::{CATPPUCCIN_MOCHA, DRACULA};

        let mut handler = TerminalHandler::new(80, 24);
        assert_eq!(
            handler.theme(),
            &CATPPUCCIN_MOCHA,
            "sanity: TerminalHandler::new defaults to Catppuccin Mocha"
        );
        assert_eq!(handler.cursor_visual_style(), CursorVisualStyle::default());
        // Seed the opposite of whatever the fresh handler's default is, so
        // this test proves `apply_initial_state` actually changed the value
        // rather than happening to match a pre-existing default.
        let seeded_auto_detect_urls = !handler.buffer_mut().auto_detect_urls();

        apply_initial_state(
            &mut handler,
            PtyTabInitialState {
                theme: &DRACULA,
                auto_detect_urls: seeded_auto_detect_urls,
                cursor_style: CursorVisualStyle::VerticalLineCursorBlink,
            },
        );

        assert_eq!(handler.theme(), &DRACULA);
        assert_eq!(
            handler.buffer_mut().auto_detect_urls(),
            seeded_auto_detect_urls
        );
        assert_eq!(
            handler.cursor_visual_style(),
            CursorVisualStyle::VerticalLineCursorBlink
        );
    }
}
