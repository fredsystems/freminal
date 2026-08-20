// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Integration tests for selected [`TerminalEmulator`] methods.
//!
//! Covered:
//! - `write_raw_bytes` — bytes land on the PTY write channel as `PtyWrite::Write`
//! - `clone_write_tx` — cloned sender delivers messages to the same receiver
//! - `set_requested_scroll_offset` / `reset_scroll_offset` — offsets are reflected in
//!   subsequent snapshots, and are clamped to `max_scroll_offset`
//! - `build_snapshot` — `scroll_offset` / `max_scroll_offset` semantics under
//!   normal use, clamping, reset, and alternate-screen force-zero
//! - `extract_selection_text` — text written to the buffer is extractable by
//!   buffer-absolute coordinates; empty regions return empty/whitespace strings

#![allow(clippy::unwrap_used)]

use freminal_common::pty_write::PtyWrite;
use freminal_terminal_emulator::interface::TerminalEmulator;

// ─── helpers ────────────────────────────────────────────────────────────────

/// Build a headless emulator that exposes the PTY write receiver.
///
/// Keep `_rx` alive for the duration of each test so the channel is never
/// disconnected (a disconnected receiver would cause the handler to log errors
/// on escape-sequence write-backs such as DA or CPR responses).
fn make_emulator() -> (TerminalEmulator, crossbeam_channel::Receiver<PtyWrite>) {
    TerminalEmulator::new_headless(None)
}

/// Drain and discard all currently-pending messages on `rx`.
///
/// Called before assertions that inspect `rx` contents when prior
/// `handle_incoming_data` calls may have queued escape-sequence responses
/// (e.g. device-attribute replies) that would otherwise pollute the channel.
fn drain(rx: &crossbeam_channel::Receiver<PtyWrite>) {
    while rx.try_recv().is_ok() {}
}

/// Unwrap a `PtyWrite::Write` variant and return its inner byte vector.
///
/// Panics with a descriptive message if the message is `PtyWrite::Resize`.
fn unwrap_write(msg: PtyWrite) -> Vec<u8> {
    match msg {
        PtyWrite::Write(bytes) => bytes,
        PtyWrite::Resize(_) => panic!("expected PtyWrite::Write, got PtyWrite::Resize"),
    }
}

/// Write `n` numbered lines into `emu` so that scrollback history is created.
///
/// The default terminal size is 100 × 100.  Writing more than 100 lines pushes
/// the oldest lines into scrollback, making `max_scroll_offset > 0`.
/// Use `n >= 150` to guarantee a non-zero scrollback with default dimensions.
fn fill_scrollback(emu: &mut TerminalEmulator, n: u32) {
    for i in 0..n {
        let line = format!("line {i:04}\r\n");
        emu.handle_incoming_data(line.as_bytes());
    }
}

// ─── write_raw_bytes ─────────────────────────────────────────────────────────

/// Sending a non-empty ASCII payload produces exactly one `PtyWrite::Write`
/// on the channel with the same bytes.
#[test]
fn test_write_raw_bytes_simple() {
    let (emu, rx) = make_emulator();
    drain(&rx);

    emu.write_raw_bytes(b"hello").unwrap();

    let msg = rx.try_recv().expect("channel must contain a message");
    assert_eq!(
        unwrap_write(msg),
        b"hello".to_vec(),
        "write_raw_bytes must deliver the exact bytes as PtyWrite::Write"
    );
    // No further messages should be queued.
    assert!(
        rx.try_recv().is_err(),
        "exactly one message must be on the channel after write_raw_bytes"
    );
}

/// Sending an empty slice produces `PtyWrite::Write(vec![])`.
#[test]
fn test_write_raw_bytes_empty() {
    let (emu, rx) = make_emulator();
    drain(&rx);

    emu.write_raw_bytes(b"").unwrap();

    let msg = rx
        .try_recv()
        .expect("channel must contain a message after write_raw_bytes(b\"\")");
    assert_eq!(
        unwrap_write(msg),
        Vec::<u8>::new(),
        "write_raw_bytes(b\"\") must produce PtyWrite::Write(vec![])"
    );
}

/// Sending arbitrary binary bytes (ESC [ A) is passed through unchanged.
#[test]
fn test_write_raw_bytes_binary() {
    let (emu, rx) = make_emulator();
    drain(&rx);

    // ESC [ A  — cursor-up sequence
    let payload = &[0x1b_u8, 0x5b, 0x41];
    emu.write_raw_bytes(payload).unwrap();

    let msg = rx.try_recv().expect("channel must contain a message");
    assert_eq!(
        unwrap_write(msg),
        payload.to_vec(),
        "write_raw_bytes must forward binary escape bytes unmodified"
    );
}

// ─── clone_write_tx ──────────────────────────────────────────────────────────

/// A sender obtained via `clone_write_tx` delivers messages to the same
/// receiver as the emulator's internal sender.
#[test]
fn test_clone_write_tx_works() {
    let (emu, rx) = make_emulator();
    drain(&rx);

    let cloned_tx = emu.clone_write_tx();
    cloned_tx
        .send(PtyWrite::Write(b"from_clone".to_vec()))
        .unwrap();

    let msg = rx
        .try_recv()
        .expect("cloned sender must deliver to the shared receiver");
    assert_eq!(
        unwrap_write(msg),
        b"from_clone".to_vec(),
        "message sent through cloned_tx must arrive on the emulator's write receiver"
    );
}

// ─── set_requested_scroll_offset ───────────────────────────────────────────────────

/// After creating sufficient scrollback and setting a scroll offset, the next
/// snapshot carries that exact offset in `snap.scroll_offset`.
#[test]
fn test_set_requested_scroll_offset_and_snapshot() {
    let (mut emu, _rx) = make_emulator();

    fill_scrollback(&mut emu, 150);
    let live = emu.build_snapshot();
    assert!(
        live.max_scroll_offset > 0,
        "expected scrollback after 150 lines; max_scroll_offset = {}",
        live.max_scroll_offset
    );

    emu.set_requested_scroll_offset(10);
    let snap = emu.build_snapshot();
    assert_eq!(
        snap.scroll_offset, 10,
        "build_snapshot must reflect the requested scroll offset"
    );
}

/// An offset larger than `max_scroll_offset` is silently clamped to the
/// maximum valid value.
#[test]
fn test_set_requested_scroll_offset_clamped() {
    let (mut emu, _rx) = make_emulator();

    fill_scrollback(&mut emu, 150);
    let live = emu.build_snapshot();
    let max = live.max_scroll_offset;
    assert!(max > 0, "expected scrollback; max_scroll_offset = {max}");

    emu.set_requested_scroll_offset(999_999);
    let snap = emu.build_snapshot();
    assert_eq!(
        snap.scroll_offset, max,
        "scroll_offset must be clamped to max_scroll_offset ({}), got {}",
        max, snap.scroll_offset
    );
    assert_eq!(
        snap.scroll_offset, snap.max_scroll_offset,
        "clamped scroll_offset must equal max_scroll_offset"
    );
}

// ─── reset_scroll_offset ─────────────────────────────────────────────────────

/// After setting a non-zero offset and then calling `reset_scroll_offset`,
/// the next snapshot reports `scroll_offset == 0`.
#[test]
fn test_reset_scroll_offset() {
    let (mut emu, _rx) = make_emulator();

    fill_scrollback(&mut emu, 150);
    let live = emu.build_snapshot();
    assert!(live.max_scroll_offset > 0, "expected scrollback");

    emu.set_requested_scroll_offset(10);
    let before_reset = emu.build_snapshot();
    assert_eq!(
        before_reset.scroll_offset, 10,
        "offset must be 10 before reset"
    );

    emu.reset_scroll_offset();
    let after_reset = emu.build_snapshot();
    assert_eq!(
        after_reset.scroll_offset, 0,
        "reset_scroll_offset must set scroll_offset to 0 in the next snapshot"
    );
}

// ─── handle_incoming_data auto-reset ─────────────────────────────────────────

/// When the user is scrolled back and new PTY data arrives,
/// `handle_incoming_data` resets `requested_scroll_offset` to 0; the subsequent
/// snapshot reports `scroll_offset == 0`.
#[test]
fn test_handle_incoming_data_resets_scroll() {
    let (mut emu, _rx) = make_emulator();

    fill_scrollback(&mut emu, 150);
    let live = emu.build_snapshot();
    assert!(live.max_scroll_offset > 0, "expected scrollback");

    emu.set_requested_scroll_offset(5);
    let scrolled = emu.build_snapshot();
    assert_eq!(
        scrolled.scroll_offset, 5,
        "offset must be 5 before new data"
    );

    // New data arriving while scrolled back must auto-scroll to bottom.
    emu.handle_incoming_data(b"fresh output\r\n");
    let after = emu.build_snapshot();
    assert_eq!(
        after.scroll_offset, 0,
        "new PTY data must reset scroll_offset to 0 (auto-scroll to bottom)"
    );
}

// ─── extract_selection_text ──────────────────────────────────────────────────

/// Text written to the terminal is extractable from the buffer via
/// buffer-absolute row and column coordinates.
#[test]
fn test_extract_selection_text() {
    let (mut emu, _rx) = make_emulator();

    // Write a known string followed by a carriage-return so it lands at row 0.
    emu.handle_incoming_data(b"Hello World");

    // The buffer's first row (index 0) starts at the top of the terminal,
    // which corresponds to the first visible row after a fresh emulator.
    // Default size is 100×100, so row 0 of the *buffer* is the first row.
    let snap = emu.build_snapshot();
    let total_rows = snap.total_rows;
    // The visible window occupies the last `term_height` rows of the buffer.
    // With the default 100-row terminal and no scrollback, the first visible
    // row is buffer row 0.
    let first_visible_row = total_rows.saturating_sub(snap.term_height);

    // Extract columns 0–4 of the first visible row (should include "Hello").
    let text = emu.extract_selection_text(first_visible_row, 0, first_visible_row, 4, false);
    assert!(
        text.starts_with("Hello"),
        "extracted text from col 0-4 must start with \"Hello\"; got: {text:?}"
    );
}

/// Extracting from a region with no written content returns an empty string or
/// only whitespace (the buffer fills empty cells with spaces).
#[test]
fn test_extract_selection_empty() {
    let (mut emu, _rx) = make_emulator();

    // No data written; the first row of the buffer is all empty cells.
    let snap = emu.build_snapshot();
    let first_row = snap.total_rows.saturating_sub(snap.term_height);

    let text = emu.extract_selection_text(first_row, 0, first_row, 9, false);
    assert!(
        text.trim().is_empty(),
        "extracting from an empty buffer region must yield empty/whitespace; got: {text:?}"
    );
}

// ─── alternate screen forces scroll_offset to zero ───────────────────────────

/// While in the alternate screen buffer, `build_snapshot` must always report
/// `scroll_offset == 0`, regardless of what `set_requested_scroll_offset` was called
/// with.  Alternate screens have no scrollback history.
#[test]
fn test_scroll_offset_zero_in_alternate() {
    let (mut emu, _rx) = make_emulator();

    // Enter alternate screen (DECSC / DECRC + alt buffer).
    emu.handle_incoming_data(b"\x1b[?1049h");

    // Attempt to set a non-zero offset while on the alternate screen.
    emu.set_requested_scroll_offset(5);
    let snap = emu.build_snapshot();

    assert!(
        snap.is_alternate_screen,
        "terminal must be on the alternate screen after \\x1b[?1049h"
    );
    assert_eq!(
        snap.scroll_offset, 0,
        "alternate screen must force scroll_offset = 0 regardless of set_requested_scroll_offset"
    );
    assert_eq!(
        snap.max_scroll_offset, 0,
        "alternate screen must always report max_scroll_offset = 0"
    );
}

// ─── synchronized updates (?2026) timeout ────────────────────────────────────

/// After the program sets DontDraw (?2026h) and then immediately resets it
/// (?2026l), the snapshot must carry `skip_draw = false`.
#[test]
fn test_sync_updates_normal_reset() {
    let (mut emu, _rx) = make_emulator();

    // Set DontDraw.
    emu.handle_incoming_data(b"\x1b[?2026h");
    let snap = emu.build_snapshot();
    assert!(
        snap.skip_draw,
        "skip_draw must be true immediately after ?2026h"
    );

    // Reset DontDraw — program finished its frame.
    emu.handle_incoming_data(b"\x1b[?2026l");
    let snap = emu.build_snapshot();
    assert!(
        !snap.skip_draw,
        "skip_draw must be false after ?2026l (normal program reset)"
    );
}

/// When a program sets DontDraw but never resets it, `build_snapshot` must
/// automatically override `skip_draw` to `false` once 200 ms have elapsed.
///
/// This test sleeps briefly past the 200 ms boundary.  It is the only test
/// that touches wall-clock time; keep it in a dedicated function so CI can
/// quarantine it if flakiness arises.
#[test]
fn test_sync_updates_timeout_auto_resume() {
    use std::time::Duration;

    let (mut emu, _rx) = make_emulator();

    // Activate DontDraw — simulates a program that sets ?2026 then crashes.
    emu.handle_incoming_data(b"\x1b[?2026h");

    // First snapshot must honour DontDraw.
    let snap = emu.build_snapshot();
    assert!(
        snap.skip_draw,
        "skip_draw must be true immediately after ?2026h"
    );

    // Sleep past the 200 ms timeout boundary (add a small margin).
    std::thread::sleep(Duration::from_millis(250));

    // After the timeout, build_snapshot must auto-reset and return skip_draw = false.
    let snap = emu.build_snapshot();
    assert!(
        !snap.skip_draw,
        "skip_draw must be false after the 200 ms timeout has elapsed without a ?2026l reset"
    );
}

/// When DontDraw is active and then reset by the program *before* the timeout,
/// a subsequent `build_snapshot` call after the 200 ms window must still report
/// `skip_draw = false` (not re-fire the timeout on stale timer state).
#[test]
fn test_sync_updates_timer_cleared_on_reset() {
    use std::time::Duration;

    let (mut emu, _rx) = make_emulator();

    // Set DontDraw, take a snapshot to start the timer.
    emu.handle_incoming_data(b"\x1b[?2026h");
    let _ = emu.build_snapshot();

    // Reset DontDraw immediately — program behaved correctly.
    emu.handle_incoming_data(b"\x1b[?2026l");
    let snap = emu.build_snapshot();
    assert!(!snap.skip_draw, "skip_draw must be false after ?2026l");

    // Sleep past the timeout period.  The timer must have been cleared by the
    // reset so this must NOT cause skip_draw to flip.
    std::thread::sleep(Duration::from_millis(250));
    let snap = emu.build_snapshot();
    assert!(
        !snap.skip_draw,
        "skip_draw must remain false after timeout period when DontDraw was already reset"
    );
}

// ─── synchronized updates (?2026): change signals across skipped frames ──────
//
// Regression coverage for issue #490. `content_changed` / `scroll_changed` are
// edge-triggered against the previous snapshot, but the GUI does not render
// snapshots carrying `skip_draw = true`. Without deferral the edge reported by
// a skipped frame is lost forever and the screen stays stale.

/// The canonical #490 repro: a paint arrives inside a synchronized-output
/// block and the closing `?2026l` lands in a *later* PTY read.
///
/// The frame carrying the new content is skipped, and the content is then
/// byte-identical on the following (renderable) frame — so a naive diff
/// reports no change. The renderable frame must still report
/// `content_changed = true`, or the GUI never rebuilds its vertex buffers.
#[test]
fn test_sync_updates_content_change_survives_skipped_frame() {
    let (mut emu, _rx) = make_emulator();

    // Settle: consume the initial "everything is new" snapshot.
    let _ = emu.build_snapshot();

    // Read 1: BSU plus the entire paint. The ESU has not arrived yet.
    emu.handle_incoming_data(b"\x1b[?2026h\x1b[HHELLO WORLD");
    let skipped = emu.build_snapshot();
    assert!(
        skipped.skip_draw,
        "precondition: the frame carrying the paint must be skipped by the GUI"
    );

    // Read 2: just the ESU. The visible content is unchanged since the frame
    // the GUI was told to throw away.
    emu.handle_incoming_data(b"\x1b[?2026l");
    let rendered = emu.build_snapshot();
    assert!(
        !rendered.skip_draw,
        "precondition: the frame after ?2026l must be renderable"
    );
    assert!(
        rendered.content_changed,
        "the first renderable frame after a skipped one must replay the skipped \
         frame's content_changed edge, otherwise the GUI keeps showing stale pixels"
    );
}

/// The deferred edge must survive an arbitrarily long run of skipped frames,
/// not just one, and must be reported exactly once.
#[test]
fn test_sync_updates_content_change_survives_many_skipped_frames() {
    let (mut emu, _rx) = make_emulator();
    let _ = emu.build_snapshot();

    emu.handle_incoming_data(b"\x1b[?2026h\x1b[Hthe paint");
    for _ in 0..5 {
        let snap = emu.build_snapshot();
        assert!(
            snap.skip_draw,
            "frames inside the ?2026 block must be skipped"
        );
    }

    emu.handle_incoming_data(b"\x1b[?2026l");
    let rendered = emu.build_snapshot();
    assert!(
        rendered.content_changed,
        "a content edge must survive any number of skipped frames"
    );

    // Debt paid: a subsequent idle frame must not keep re-reporting it, or the
    // GUI would rebuild its vertex buffers on every frame forever.
    let idle = emu.build_snapshot();
    assert!(
        !idle.content_changed,
        "the deferred edge must be cleared once it has been delivered"
    );
}

/// `scroll_changed` is deferred by the same mechanism as `content_changed`
/// and needs its own coverage.
///
/// The block is left via the 200 ms auto-resume rather than `?2026l`, because
/// `?2026l` is itself PTY output and new output auto-resets the scroll offset
/// to the live bottom -- which manufactures a fresh, genuine scroll edge and
/// would let this pass with the deferral removed.
#[test]
fn test_sync_updates_scroll_change_survives_skipped_frame() {
    use std::time::Duration;

    let (mut emu, _rx) = make_emulator();
    // Enough output for a non-zero scroll offset to be reachable.
    for i in 0..200 {
        emu.handle_incoming_data(format!("line {i}\r\n").as_bytes());
    }
    let _ = emu.build_snapshot();

    // Enter the synchronized block, then scroll while frames are skipped.
    emu.handle_incoming_data(b"\x1b[?2026h");
    let _ = emu.build_snapshot();
    emu.set_requested_scroll_offset(5);
    let skipped = emu.build_snapshot();
    assert!(skipped.skip_draw, "precondition: the frame is skipped");
    assert!(
        skipped.scroll_changed,
        "precondition: the scroll edge occurs on the skipped frame"
    );

    // Release rendering without feeding any bytes.
    std::thread::sleep(Duration::from_millis(250));

    let rendered = emu.build_snapshot();
    assert!(
        !rendered.skip_draw,
        "precondition: the timeout released rendering"
    );
    assert!(
        rendered.scroll_changed,
        "the scroll edge from the skipped frame must be replayed; without \
         deferral the offset now matches the previous one and this reports false"
    );

    // Delivered exactly once, like the content flag.
    let idle = emu.build_snapshot();
    assert!(!idle.scroll_changed);
}

/// The two deferred flags are tracked independently: a content-only change
/// must not manufacture a scroll that never happened. This matters because
/// the GUI's selection auto-clear keys off exactly that distinction -- a pure
/// scroll leaves a selection alone, a content change does not.
#[test]
fn test_sync_updates_content_change_does_not_imply_scroll_change() {
    let (mut emu, _rx) = make_emulator();
    let _ = emu.build_snapshot();

    emu.handle_incoming_data(b"\x1b[?2026h\x1b[Hcontent only");
    let skipped = emu.build_snapshot();
    assert!(skipped.skip_draw, "precondition: the frame is skipped");

    emu.handle_incoming_data(b"\x1b[?2026l");
    let rendered = emu.build_snapshot();
    assert!(rendered.content_changed, "the content edge is replayed");
    assert!(
        !rendered.scroll_changed,
        "a content change must not be reported as a scroll"
    );
}

/// A skipped frame with genuinely no change must not manufacture one.
#[test]
fn test_sync_updates_skipped_frame_without_change_reports_none() {
    let (mut emu, _rx) = make_emulator();
    emu.handle_incoming_data(b"\x1b[Hsettled");
    let _ = emu.build_snapshot();

    // Enter and leave the synchronized block without painting anything.
    emu.handle_incoming_data(b"\x1b[?2026h");
    let skipped = emu.build_snapshot();
    assert!(skipped.skip_draw, "precondition: frame is skipped");

    emu.handle_incoming_data(b"\x1b[?2026l");
    let rendered = emu.build_snapshot();
    assert!(
        !rendered.content_changed,
        "deferral must replay real edges only, never invent one"
    );
}

/// The 200 ms auto-resume path is also a transition from skipped to rendered,
/// so a content edge observed while `DontDraw` was stuck must be replayed on
/// the frame the timeout releases.
#[test]
fn test_sync_updates_content_change_survives_timeout_resume() {
    use std::time::Duration;

    let (mut emu, _rx) = make_emulator();
    let _ = emu.build_snapshot();

    // A program that paints, sets DontDraw, and then crashes without resetting.
    emu.handle_incoming_data(b"\x1b[?2026h\x1b[Horphaned paint");
    let skipped = emu.build_snapshot();
    assert!(skipped.skip_draw, "precondition: frame is skipped");

    std::thread::sleep(Duration::from_millis(250));

    let rendered = emu.build_snapshot();
    assert!(
        !rendered.skip_draw,
        "precondition: the 200 ms timeout must have released rendering"
    );
    assert!(
        rendered.content_changed,
        "the timeout-released frame must replay the skipped frame's content edge"
    );
}
