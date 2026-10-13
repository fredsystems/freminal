//! Fuzz the whole emulator pipeline end to end, headlessly.
//!
//! Builds a PTY-less `TerminalEmulator` with `TerminalEmulator::new_headless`
//! (which wraps a `TerminalState` constructed with a live write channel),
//! resizes it, then feeds it the input in chunks through
//! `handle_incoming_data`: parser, handler, buffer, image store and reply
//! generation all run. Snapshots are built periodically and at the end, which
//! exercises the visible-window flattening and its change-tracking cache.
//!
//! Input layout: byte 0 is the column count seed, byte 1 the row count seed,
//! byte 2 seeds the chunk lengths; the remainder is the terminal byte stream.
//! Columns are clamped to `1..=300` and rows to `1..=120`, with an 8x16 pixel
//! cell. Inputs shorter than three bytes are ignored.
//!
//! After every chunk the reply channel (DA, CPR, ...) and the pending window
//! command queue are drained, so they cannot grow without bound across a long
//! input.
//!
//! WARNING: this target can drive kitty graphics `t=f` / `t=t` / `t=s`
//! transmission, which reads arbitrary paths and deletes kitty-marked temporary
//! files and shared-memory segments. Run it only inside a sandbox (private
//! `/tmp`, `/dev/shm`, no network); no input filtering is done here.
//!
//! Invariant asserted: none beyond "does not panic, hang or run out of memory".

#![no_main]

use freminal_fuzz::split_chunks;
use freminal_terminal_emulator::interface::TerminalEmulator;
use libfuzzer_sys::fuzz_target;

/// Cell size used for every run, in pixels.
const CELL_WIDTH_PX: usize = 8;
const CELL_HEIGHT_PX: usize = 16;

/// Build a snapshot after every this-many chunks (and always after the last).
const SNAPSHOT_EVERY: usize = 4;

fuzz_target!(|data: &[u8]| {
    let [cols_byte, rows_byte, seed, stream @ ..] = data else {
        return;
    };
    let cols = 1 + usize::from(*cols_byte) % 300;
    let rows = 1 + usize::from(*rows_byte) % 120;

    let (mut emulator, write_rx) = TerminalEmulator::new_headless(None);
    if emulator
        .set_win_size(cols, rows, CELL_WIDTH_PX, CELL_HEIGHT_PX)
        .is_err()
    {
        return;
    }

    let chunks = split_chunks(*seed, 4, stream);
    let last = chunks.len().saturating_sub(1);
    for (index, chunk) in chunks.into_iter().enumerate() {
        emulator.handle_incoming_data(chunk);

        emulator.internal.window_commands.clear();
        write_rx.try_iter().for_each(drop);

        if index % SNAPSHOT_EVERY == SNAPSHOT_EVERY - 1 || index == last {
            drop(emulator.build_snapshot());
        }
    }
});
