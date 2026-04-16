// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Playback consumer thread: replays a recorded terminal session.
//!
//! Instead of reading from a PTY, this thread owns a `TerminalEmulator` and a
//! pre-parsed `Vec<PlaybackFrame>`.  It listens for `InputEvent` messages from
//! the GUI — notably `InputEvent::PlaybackControl` — and drives playback
//! through a state machine.
//!
//! Playback modes:
//!
//! - **Instant**: process all remaining frames immediately on Play.
//! - **`RealTime`**: replay with original inter-frame timing (play/pause).
//! - **`FrameStepping`**: advance one frame per `NextFrame` command.

use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use arc_swap::ArcSwap;
use crossbeam_channel::Sender;
use freminal_common::buffer_states::tchar::TChar;
use freminal_common::buffer_states::window_manipulation::WindowManipulation;
use freminal_terminal_emulator::interface::TerminalEmulator;
use freminal_terminal_emulator::io::{InputEvent, PlaybackCommand, PlaybackMode, WindowCommand};
use freminal_terminal_emulator::recording::PlaybackFrame;
use freminal_terminal_emulator::snapshot::{PlaybackInfo, TerminalSnapshot};

/// Internal playback state machine.
enum PlaybackState {
    /// No mode selected yet — waiting for `SetMode`.
    WaitingForMode,
    /// Mode selected, waiting for `Play`.
    WaitingForPlay,
    /// Real-time playback actively running.
    RealTimePlaying {
        play_start: Instant,
        base_timestamp_us: u64,
    },
    /// Real-time playback paused.
    RealTimePaused { elapsed_at_pause: Duration },
    /// Frame-stepping mode: waiting for `NextFrame`.
    FrameStepWaiting,
    /// All frames have been processed.
    Complete,
}

/// Run the playback consumer thread.
///
/// This function never returns until the GUI closes the input channel.
/// It owns the `TerminalEmulator` exclusively — no locks involved.
///
/// Arguments are taken by value intentionally: this is a thread entry point
/// that keeps everything alive for the thread's entire lifetime.
#[allow(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    clippy::needless_pass_by_value
)]
pub fn run_playback_thread(
    mut emulator: TerminalEmulator,
    frames: Vec<PlaybackFrame>,
    input_rx: crossbeam_channel::Receiver<InputEvent>,
    window_cmd_tx: Sender<WindowCommand>,
    arc_swap: Arc<ArcSwap<TerminalSnapshot>>,
    egui_ctx: Arc<OnceLock<eframe::egui::Context>>,
    clipboard_tx: Sender<String>,
    search_buffer_tx: Sender<(usize, Vec<TChar>)>,
) {
    let total_frames = frames.len();
    let mut current_frame: usize = 0;
    let mut mode = PlaybackMode::Instant;
    let mut state = PlaybackState::WaitingForMode;

    // Publish initial snapshot so the GUI has something to render.
    publish_snapshot(
        &mut emulator,
        &window_cmd_tx,
        &arc_swap,
        &egui_ctx,
        playback_info(current_frame, total_frames, mode, &state),
    );

    loop {
        match state {
            PlaybackState::WaitingForMode
            | PlaybackState::WaitingForPlay
            | PlaybackState::FrameStepWaiting
            | PlaybackState::Complete => {
                // Block until the next GUI event.
                match input_rx.recv() {
                    Ok(ref event) => {
                        if !handle_event(
                            event,
                            &mut emulator,
                            &mut state,
                            &mut mode,
                            &mut current_frame,
                            &frames,
                            &clipboard_tx,
                            &search_buffer_tx,
                        ) {
                            return;
                        }
                    }
                    Err(_) => return, // GUI closed
                }
                publish_snapshot(
                    &mut emulator,
                    &window_cmd_tx,
                    &arc_swap,
                    &egui_ctx,
                    playback_info(current_frame, total_frames, mode, &state),
                );
            }

            PlaybackState::RealTimePlaying {
                play_start,
                base_timestamp_us,
            } => {
                if current_frame >= total_frames {
                    state = PlaybackState::Complete;
                    publish_snapshot(
                        &mut emulator,
                        &window_cmd_tx,
                        &arc_swap,
                        &egui_ctx,
                        playback_info(current_frame, total_frames, mode, &state),
                    );
                    continue;
                }

                // Calculate how long to wait for the next frame.
                let frame_ts = frames[current_frame].timestamp_us;
                let target_elapsed =
                    Duration::from_micros(frame_ts.saturating_sub(base_timestamp_us));
                let actual_elapsed = play_start.elapsed();

                if actual_elapsed >= target_elapsed {
                    // Frame is due — process it.
                    emulator.handle_incoming_data(&frames[current_frame].data);
                    current_frame += 1;
                    publish_snapshot(
                        &mut emulator,
                        &window_cmd_tx,
                        &arc_swap,
                        &egui_ctx,
                        playback_info(current_frame, total_frames, mode, &state),
                    );
                } else {
                    // Wait for the frame to become due, but also listen for
                    // GUI events so we can pause or resize mid-playback.
                    let wait = target_elapsed.saturating_sub(actual_elapsed);
                    match input_rx.recv_timeout(wait) {
                        Ok(ref event) => {
                            if !handle_event(
                                event,
                                &mut emulator,
                                &mut state,
                                &mut mode,
                                &mut current_frame,
                                &frames,
                                &clipboard_tx,
                                &search_buffer_tx,
                            ) {
                                return;
                            }
                            publish_snapshot(
                                &mut emulator,
                                &window_cmd_tx,
                                &arc_swap,
                                &egui_ctx,
                                playback_info(current_frame, total_frames, mode, &state),
                            );
                        }
                        Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                            // Timer expired — process the frame on next iteration.
                        }
                        Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                            return; // GUI closed
                        }
                    }
                }
            }

            PlaybackState::RealTimePaused { .. } => {
                // Paused: block until the GUI sends a command.
                match input_rx.recv() {
                    Ok(ref event) => {
                        if !handle_event(
                            event,
                            &mut emulator,
                            &mut state,
                            &mut mode,
                            &mut current_frame,
                            &frames,
                            &clipboard_tx,
                            &search_buffer_tx,
                        ) {
                            return;
                        }
                    }
                    Err(_) => return,
                }
                publish_snapshot(
                    &mut emulator,
                    &window_cmd_tx,
                    &arc_swap,
                    &egui_ctx,
                    playback_info(current_frame, total_frames, mode, &state),
                );
            }
        }
    }
}

/// Handle a single `InputEvent`.  Returns `false` if the thread should exit.
// All parameters are required context for playback event dispatch: shared state, channels,
// timing, and frame control. Grouping would obscure the data flow.
#[allow(clippy::too_many_arguments)]
fn handle_event(
    event: &InputEvent,
    emulator: &mut TerminalEmulator,
    state: &mut PlaybackState,
    mode: &mut PlaybackMode,
    current_frame: &mut usize,
    frames: &[PlaybackFrame],
    clipboard_tx: &Sender<String>,
    search_buffer_tx: &Sender<(usize, Vec<TChar>)>,
) -> bool {
    match *event {
        InputEvent::PlaybackControl(cmd) => {
            handle_playback_command(cmd, emulator, state, mode, current_frame, frames);
        }
        InputEvent::Resize(w, h, pw, ph) => {
            emulator.handle_resize_event(w, h, pw, ph);
        }
        InputEvent::ScrollOffset(offset) => {
            emulator.set_gui_scroll_offset(offset);
        }
        InputEvent::ThemeChange(theme) => {
            emulator.internal.handler.set_theme(theme);
        }
        InputEvent::ThemeModeUpdate(theme_mode, _os_is_dark) => {
            // In playback mode, just update the theme_mode so DECRPM responses
            // are correct. The OS preference is not meaningful in playback.
            emulator.internal.modes.theme_mode = theme_mode;
        }
        InputEvent::ExtractSelection {
            start_row,
            start_col,
            end_row,
            end_col,
            is_block,
        } => {
            let text =
                emulator.extract_selection_text(start_row, start_col, end_row, end_col, is_block);
            let _ = clipboard_tx.send(text);
        }
        InputEvent::RequestSearchBuffer => {
            let (chars, _tags) = emulator.internal.handler.data_and_format_data_for_gui(0);
            let mut combined = chars.scrollback;
            combined.extend(chars.visible);
            let total_rows = emulator.internal.handler.buffer().get_rows().len();
            let _ = search_buffer_tx.send((total_rows, combined));
        }
        InputEvent::FocusChange(_) | InputEvent::Key(_) => {
            // In playback mode, keyboard input and focus changes are ignored
            // (there is no PTY to write to).
        }
    }
    true
}

/// Handle a `PlaybackCommand`, transitioning the state machine.
fn handle_playback_command(
    cmd: PlaybackCommand,
    emulator: &mut TerminalEmulator,
    state: &mut PlaybackState,
    mode: &mut PlaybackMode,
    current_frame: &mut usize,
    frames: &[PlaybackFrame],
) {
    match cmd {
        PlaybackCommand::SetMode(new_mode) => {
            *mode = new_mode;
            // SetMode always pauses; user must press Play.
            *state = PlaybackState::WaitingForPlay;
        }
        PlaybackCommand::Play => {
            if *current_frame >= frames.len() {
                *state = PlaybackState::Complete;
                return;
            }

            match mode {
                PlaybackMode::Instant => {
                    // Process all remaining frames at once.
                    for frame in &frames[*current_frame..] {
                        emulator.handle_incoming_data(&frame.data);
                    }
                    *current_frame = frames.len();
                    *state = PlaybackState::Complete;
                }
                PlaybackMode::RealTime => {
                    let base_timestamp_us = frames[*current_frame].timestamp_us;
                    *state = match state {
                        PlaybackState::RealTimePaused { elapsed_at_pause } => {
                            // Resume: adjust play_start so elapsed time is
                            // correct.  Use saturating_sub to avoid panic on
                            // clock weirdness.
                            let resumed_start = Instant::now()
                                .checked_sub(*elapsed_at_pause)
                                .unwrap_or_else(Instant::now);
                            PlaybackState::RealTimePlaying {
                                play_start: resumed_start,
                                base_timestamp_us,
                            }
                        }
                        _ => PlaybackState::RealTimePlaying {
                            play_start: Instant::now(),
                            base_timestamp_us,
                        },
                    };
                }
                PlaybackMode::FrameStepping => {
                    // In frame-stepping mode, Play processes the first frame
                    // and then waits for NextFrame commands.
                    emulator.handle_incoming_data(&frames[*current_frame].data);
                    *current_frame += 1;
                    *state = if *current_frame >= frames.len() {
                        PlaybackState::Complete
                    } else {
                        PlaybackState::FrameStepWaiting
                    };
                }
            }
        }
        PlaybackCommand::Pause if let PlaybackState::RealTimePlaying { play_start, .. } = state => {
            *state = PlaybackState::RealTimePaused {
                elapsed_at_pause: play_start.elapsed(),
            };
        }
        PlaybackCommand::NextFrame
            if matches!(
                state,
                PlaybackState::FrameStepWaiting | PlaybackState::WaitingForPlay
            ) && *mode == PlaybackMode::FrameStepping
                && *current_frame < frames.len() =>
        {
            emulator.handle_incoming_data(&frames[*current_frame].data);
            *current_frame += 1;
            *state = if *current_frame >= frames.len() {
                PlaybackState::Complete
            } else {
                PlaybackState::FrameStepWaiting
            };
        }
        // Pause in other states is a no-op. NextFrame in non-steppable states is a no-op.
        PlaybackCommand::Pause | PlaybackCommand::NextFrame => {}
    }
}

/// Build a `PlaybackInfo` from the current playback state.
const fn playback_info(
    current_frame: usize,
    total_frames: usize,
    mode: PlaybackMode,
    state: &PlaybackState,
) -> PlaybackInfo {
    let playing = matches!(state, PlaybackState::RealTimePlaying { .. });
    PlaybackInfo {
        current_frame,
        total_frames,
        mode,
        playing,
    }
}

/// Drain window commands, build a snapshot with playback info, publish, and
/// request a repaint.
fn publish_snapshot(
    emulator: &mut TerminalEmulator,
    window_cmd_tx: &Sender<WindowCommand>,
    arc_swap: &ArcSwap<TerminalSnapshot>,
    egui_ctx: &OnceLock<eframe::egui::Context>,
    info: PlaybackInfo,
) {
    // Drain any window manipulation commands produced by escape sequences.
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

    let mut snap = emulator.build_snapshot();
    snap.playback_info = Some(info);
    arc_swap.store(Arc::new(snap));

    if let Some(ctx) = egui_ctx.get() {
        ctx.request_repaint_after(Duration::from_millis(8));
    }
}
