// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Behavioural tests for `TerminalEmulator::build_snapshot`.
//!
//! Each test targets a specific invariant of the three-way branch logic in
//! `build_snapshot`:
//!
//! 1. **First-ever snapshot** — nothing dirty yet, no cache → flatten once,
//!    `content_changed = true`.
//! 2. **Clean path / cache hit** — no rows dirty after a snapshot with no new
//!    data → return Arc clones, `content_changed = false`, zero allocation.
//! 3. **Dirty path** — rows dirtied by `handle_incoming_data` → re-flatten,
//!    compare to previous, set `content_changed` correctly.
//!
//! Additional invariants covered:
//! - Alt-screen enter/leave invalidates the snapshot cache.
//! - Alt-screen always reports `scroll_offset = 0` and `max_scroll_offset = 0`.
//! - `show_cursor = false` when `scroll_offset > 0`.
//! - `scroll_changed` is `true` only on the first snapshot after the offset
//!   changed, then `false` on the subsequent one.
//! - New data while scrolled back auto-resets `scroll_offset` to 0.
//! - Arc pointer identity on the clean path (no extra allocation).
//! - Cursor-only move does not set `content_changed = true`.

use std::sync::Arc;

use freminal_terminal_emulator::interface::TerminalEmulator;

// ─── helpers ────────────────────────────────────────────────────────────────

/// Construct a headless emulator with no PTY.
///
/// Returns `(emulator, _write_rx)` — keep `_write_rx` alive so the channel
/// is never disconnected (that would cause the handler to log errors on
/// write-back attempts).
fn make_emulator() -> (
    TerminalEmulator,
    crossbeam_channel::Receiver<freminal_common::pty_write::PtyWrite>,
) {
    TerminalEmulator::new_headless(None)
}

/// Write `n` numbered lines into `emu` to create scrollback history.
///
/// The default terminal size is 100 × 100.  Writing more than 100 lines causes
/// the oldest lines to be pushed into scrollback so that `max_scroll_offset > 0`.
/// Use `n >= 150` to guarantee scrollback with the default terminal dimensions.
fn fill_scrollback(emu: &mut TerminalEmulator, n: u32) {
    for i in 0..n {
        let line = format!("line {i:04}\r\n");
        emu.handle_incoming_data(line.as_bytes());
    }
}

// ─── 1. First-ever snapshot ──────────────────────────────────────────────────

#[test]
fn first_snapshot_reports_content_changed_true() {
    let (mut emu, _rx) = make_emulator();
    let snap = emu.build_snapshot();
    assert!(
        snap.content_changed,
        "first snapshot must report content_changed = true (no prior cache)"
    );
}

#[test]
fn first_snapshot_has_correct_dimensions() {
    let (mut emu, _rx) = make_emulator();
    let snap = emu.build_snapshot();
    assert!(snap.term_width > 0, "term_width must be > 0");
    assert!(snap.term_height > 0, "term_height must be > 0");
    assert_eq!(
        snap.height, snap.term_height,
        "height must equal term_height"
    );
}

#[test]
fn first_snapshot_is_not_alternate_screen() {
    let (mut emu, _rx) = make_emulator();
    let snap = emu.build_snapshot();
    assert!(
        !snap.is_alternate_screen,
        "fresh emulator must start on primary screen"
    );
}

// ─── 2. Clean path / cache hit ───────────────────────────────────────────────

#[test]
fn second_snapshot_with_no_new_data_reports_content_changed_false() {
    let (mut emu, _rx) = make_emulator();
    let _first = emu.build_snapshot();
    let second = emu.build_snapshot();
    assert!(
        !second.content_changed,
        "second snapshot with no new data must report content_changed = false"
    );
}

#[test]
fn clean_path_reuses_same_arc_allocation() {
    let (mut emu, _rx) = make_emulator();
    // Seed real content so the Arc is non-trivially populated.
    emu.handle_incoming_data(b"hello world");
    let first = emu.build_snapshot();
    // No new data — second snapshot must reuse the same Arc.
    let second = emu.build_snapshot();
    assert!(
        Arc::ptr_eq(&first.visible_chars, &second.visible_chars),
        "clean-path snapshots must share the same visible_chars Arc (no allocation)"
    );
    assert!(
        Arc::ptr_eq(&first.visible_tags, &second.visible_tags),
        "clean-path snapshots must share the same visible_tags Arc (no allocation)"
    );
}

// ─── 3. Dirty path ───────────────────────────────────────────────────────────

#[test]
fn new_data_after_snapshot_causes_content_changed_true() {
    let (mut emu, _rx) = make_emulator();
    let _first = emu.build_snapshot(); // populate cache
    emu.handle_incoming_data(b"new output");
    let second = emu.build_snapshot();
    assert!(
        second.content_changed,
        "snapshot after new PTY data must report content_changed = true"
    );
}

#[test]
fn dirty_path_produces_new_arc_allocation() {
    let (mut emu, _rx) = make_emulator();
    emu.handle_incoming_data(b"first");
    let first = emu.build_snapshot();
    emu.handle_incoming_data(b"second");
    let second = emu.build_snapshot();
    assert!(
        !Arc::ptr_eq(&first.visible_chars, &second.visible_chars),
        "dirty-path snapshot must produce a new Arc allocation"
    );
}

#[test]
fn multiple_snapshots_after_successive_writes() {
    let (mut emu, _rx) = make_emulator();
    for i in 0..5u8 {
        let line = format!("line {i}\r\n");
        emu.handle_incoming_data(line.as_bytes());
        let snap = emu.build_snapshot();
        assert!(
            snap.content_changed,
            "snapshot immediately after write #{i} must report content_changed = true"
        );
        // A second snapshot without new data must be clean.
        let clean = emu.build_snapshot();
        assert!(
            !clean.content_changed,
            "snapshot with no new data after write #{i} must report content_changed = false"
        );
    }
}

// ─── 4. Cursor movement only ─────────────────────────────────────────────────

#[test]
fn cursor_only_move_does_not_set_content_changed() {
    let (mut emu, _rx) = make_emulator();
    // Write some initial text so the visible window is non-empty.
    emu.handle_incoming_data(b"hello");
    let _first = emu.build_snapshot(); // cache the content

    // Move cursor via CUP without writing any text cells.
    // ESC [ 1 ; 1 H  → CUP row=1, col=1
    emu.handle_incoming_data(b"\x1b[1;1H");
    let after_move = emu.build_snapshot();
    assert!(
        !after_move.content_changed,
        "cursor-only CUP must not set content_changed (no cells mutated)"
    );
}

// ─── 5. Alternate screen ─────────────────────────────────────────────────────

#[test]
fn enter_alternate_screen_is_reflected_in_snapshot() {
    let (mut emu, _rx) = make_emulator();
    let _before = emu.build_snapshot();
    // ESC [ ? 1049 h  → enter alternate screen
    emu.handle_incoming_data(b"\x1b[?1049h");
    let snap = emu.build_snapshot();
    assert!(
        snap.is_alternate_screen,
        "snapshot must reflect is_alternate_screen = true after \\x1b[?1049h"
    );
}

#[test]
fn leave_alternate_screen_is_reflected_in_snapshot() {
    let (mut emu, _rx) = make_emulator();
    emu.handle_incoming_data(b"\x1b[?1049h"); // enter
    let _in_alt = emu.build_snapshot();
    emu.handle_incoming_data(b"\x1b[?1049l"); // leave
    let snap = emu.build_snapshot();
    assert!(
        !snap.is_alternate_screen,
        "snapshot must reflect is_alternate_screen = false after \\x1b[?1049l"
    );
}

#[test]
fn alternate_screen_always_has_zero_scroll_offsets() {
    let (mut emu, _rx) = make_emulator();
    emu.handle_incoming_data(b"\x1b[?1049h");
    // Even if we attempt to set a non-zero scroll offset, alternate screen
    // must clamp both values to 0.
    emu.set_gui_scroll_offset(999);
    let snap = emu.build_snapshot();
    assert_eq!(
        snap.scroll_offset, 0,
        "alternate screen must always report scroll_offset = 0"
    );
    assert_eq!(
        snap.max_scroll_offset, 0,
        "alternate screen must always report max_scroll_offset = 0"
    );
}

#[test]
fn alt_screen_enter_invalidates_cache() {
    let (mut emu, _rx) = make_emulator();
    emu.handle_incoming_data(b"primary text");
    let _primary = emu.build_snapshot(); // populate cache with primary content

    // Switch to alternate screen — cache must be invalidated.
    emu.handle_incoming_data(b"\x1b[?1049h");
    let alt = emu.build_snapshot();
    // content_changed must be true on the first snapshot in the new buffer
    // (cache was invalidated, so we always report changed).
    assert!(
        alt.content_changed,
        "first snapshot after entering alternate screen must report content_changed = true"
    );
}

#[test]
fn alt_screen_leave_invalidates_cache() {
    let (mut emu, _rx) = make_emulator();
    emu.handle_incoming_data(b"\x1b[?1049h"); // enter alt
    emu.handle_incoming_data(b"alt content");
    let _alt = emu.build_snapshot(); // populate cache with alt content

    // Leave alternate screen — cache must be invalidated.
    emu.handle_incoming_data(b"\x1b[?1049l");
    let primary = emu.build_snapshot();
    assert!(
        primary.content_changed,
        "first snapshot after leaving alternate screen must report content_changed = true"
    );
}

// ─── 6. show_cursor suppression when scrolled back ───────────────────────────

#[test]
fn show_cursor_is_false_when_scrolled_back() {
    let (mut emu, _rx) = make_emulator();
    // Default terminal is 100×100; write 150 lines to guarantee scrollback.
    fill_scrollback(&mut emu, 150);

    let live = emu.build_snapshot();
    assert!(
        live.max_scroll_offset > 0,
        "expected scrollback after 150 lines; max_scroll_offset = {}",
        live.max_scroll_offset
    );
    // Cursor should be visible at the live view (scroll_offset = 0).
    assert!(
        live.show_cursor,
        "cursor must be visible at live view (scroll_offset = 0)"
    );

    // Scroll back by one row.
    emu.set_gui_scroll_offset(1);
    let scrolled = emu.build_snapshot();
    assert!(
        !scrolled.show_cursor,
        "cursor must be hidden when scrolled back (scroll_offset > 0)"
    );
}

// ─── 7. scroll_changed flag ──────────────────────────────────────────────────

#[test]
fn scroll_changed_is_false_on_first_snapshot_at_zero_offset() {
    let (mut emu, _rx) = make_emulator();
    let snap = emu.build_snapshot();
    assert!(
        !snap.scroll_changed,
        "scroll_changed must be false when offset was never changed"
    );
}

#[test]
fn scroll_changed_true_on_first_snapshot_after_offset_set() {
    let (mut emu, _rx) = make_emulator();
    // Default terminal is 100×100; write 150 lines to guarantee scrollback.
    fill_scrollback(&mut emu, 150);

    let live = emu.build_snapshot();
    assert!(
        live.max_scroll_offset > 0,
        "expected scrollback after 150 lines; max_scroll_offset = {}",
        live.max_scroll_offset
    );

    emu.set_gui_scroll_offset(5);
    let scrolled = emu.build_snapshot();
    assert!(
        scrolled.scroll_changed,
        "scroll_changed must be true on the first snapshot after offset changed"
    );
}

#[test]
fn scroll_changed_false_on_second_snapshot_at_same_offset() {
    let (mut emu, _rx) = make_emulator();
    fill_scrollback(&mut emu, 150);

    let live = emu.build_snapshot();
    assert!(
        live.max_scroll_offset > 0,
        "expected scrollback after 150 lines; max_scroll_offset = {}",
        live.max_scroll_offset
    );

    emu.set_gui_scroll_offset(5);
    let _first_scrolled = emu.build_snapshot(); // scroll_changed = true here
    // Same offset, no movement → scroll_changed must be false.
    let second_scrolled = emu.build_snapshot();
    assert!(
        !second_scrolled.scroll_changed,
        "scroll_changed must be false on repeated snapshots at the same offset"
    );
}

// ─── 8. Auto-scroll on new data while scrolled back ──────────────────────────

#[test]
fn new_data_while_scrolled_back_resets_scroll_offset_to_zero() {
    let (mut emu, _rx) = make_emulator();
    fill_scrollback(&mut emu, 150);

    let live = emu.build_snapshot();
    assert!(
        live.max_scroll_offset > 0,
        "expected scrollback after 150 lines; max_scroll_offset = {}",
        live.max_scroll_offset
    );

    emu.set_gui_scroll_offset(10);
    let scrolled = emu.build_snapshot();
    assert_eq!(
        scrolled.scroll_offset, 10,
        "scroll_offset must reflect the requested value"
    );

    // New PTY data arrives — auto-scroll should reset offset to 0.
    emu.handle_incoming_data(b"new output\r\n");
    let after_new_data = emu.build_snapshot();
    assert_eq!(
        after_new_data.scroll_offset, 0,
        "new PTY data must auto-reset scroll_offset to 0"
    );
}

// ─── 9. scroll_offset clamped to max_scroll_offset ───────────────────────────

#[test]
fn scroll_offset_is_clamped_to_max() {
    let (mut emu, _rx) = make_emulator();
    fill_scrollback(&mut emu, 150);

    let live = emu.build_snapshot();
    let max = live.max_scroll_offset;
    assert!(
        max > 0,
        "expected scrollback after 150 lines; max_scroll_offset = {max}"
    );

    // Request an offset way beyond the max.
    emu.set_gui_scroll_offset(max + 9999);
    let clamped = emu.build_snapshot();
    assert_eq!(
        clamped.scroll_offset, max,
        "scroll_offset must be clamped to max_scroll_offset"
    );
}

// ─── 10. Snapshot dimensions reflect terminal size ────────────────────────────

#[test]
fn snapshot_dimensions_reflect_terminal_size() {
    let (mut emu, _rx) = make_emulator();
    let snap = emu.build_snapshot();
    let (w, h) = emu.get_win_size();
    assert_eq!(
        snap.term_width, w,
        "snap.term_width must match get_win_size().0"
    );
    assert_eq!(
        snap.term_height, h,
        "snap.term_height must match get_win_size().1"
    );
}

// ─── 11. total_rows tracking ──────────────────────────────────────────────────

#[test]
fn total_rows_grows_as_output_fills_buffer() {
    let (mut emu, _rx) = make_emulator();
    let initial = emu.build_snapshot();

    // Write enough lines to grow the buffer beyond the initial visible height.
    for i in 0..100u32 {
        let line = format!("line {i:04}\r\n");
        emu.handle_incoming_data(line.as_bytes());
    }
    let after = emu.build_snapshot();
    assert!(
        after.total_rows >= initial.total_rows,
        "total_rows must not shrink after writing more output"
    );
}

// ─── 12. is_normal_display flag ───────────────────────────────────────────────

#[test]
fn is_normal_display_true_by_default() {
    let (mut emu, _rx) = make_emulator();
    let snap = emu.build_snapshot();
    assert!(
        snap.is_normal_display,
        "display must be in normal (non-inverted) mode by default"
    );
}

#[test]
fn is_normal_display_false_after_decscnm_set() {
    let (mut emu, _rx) = make_emulator();
    // ESC [ ? 5 h  → DECSCNM (reverse video / inverted display) on
    emu.handle_incoming_data(b"\x1b[?5h");
    let snap = emu.build_snapshot();
    assert!(
        !snap.is_normal_display,
        "is_normal_display must be false after DECSCNM is set"
    );
}

#[test]
fn is_normal_display_restored_after_decscnm_reset() {
    let (mut emu, _rx) = make_emulator();
    emu.handle_incoming_data(b"\x1b[?5h"); // enable reverse video
    emu.handle_incoming_data(b"\x1b[?5l"); // disable it again
    let snap = emu.build_snapshot();
    assert!(
        snap.is_normal_display,
        "is_normal_display must be restored to true after DECSCNM is unset"
    );
}

// ─── 13. Visible content matches what was written ────────────────────────────

#[test]
fn visible_chars_contains_written_text() {
    use freminal_common::buffer_states::tchar::TChar;

    let (mut emu, _rx) = make_emulator();
    emu.handle_incoming_data(b"ABC");
    let snap = emu.build_snapshot();

    // visible_chars must contain TChar representations of 'A', 'B', 'C'.
    let chars: Vec<u8> = snap
        .visible_chars
        .iter()
        .filter_map(|tc| match tc {
            TChar::Ascii(b) => Some(*b),
            _ => None,
        })
        .collect();

    assert!(
        chars.windows(3).any(|w| w == b"ABC"),
        "visible_chars must contain the bytes 'ABC' that were written; got: {chars:?}"
    );
}

// ─── 14. Round-trip: clear screen does not persist stale content ─────────────

#[test]
fn erase_display_sets_content_changed_true() {
    let (mut emu, _rx) = make_emulator();
    emu.handle_incoming_data(b"some content");
    let _first = emu.build_snapshot();

    // ESC [ 2 J → erase entire display
    emu.handle_incoming_data(b"\x1b[2J");
    let after_erase = emu.build_snapshot();
    assert!(
        after_erase.content_changed,
        "erase-display must mark visible rows dirty → content_changed = true"
    );
}

// ─── 15. has_blinking_text flag ──────────────────────────────────────────────

#[test]
fn has_blinking_text_false_for_plain_text() {
    let (mut emu, _rx) = make_emulator();
    emu.handle_incoming_data(b"no blink here");
    let snap = emu.build_snapshot();
    assert!(
        !snap.has_blinking_text,
        "has_blinking_text must be false when no blink SGR was used"
    );
}

#[test]
fn has_blinking_text_true_after_sgr_5() {
    let (mut emu, _rx) = make_emulator();
    // ESC[5m → slow blink, then write text
    emu.handle_incoming_data(b"\x1b[5mBlinky");
    let snap = emu.build_snapshot();
    assert!(
        snap.has_blinking_text,
        "has_blinking_text must be true when visible text has SGR 5 (slow blink)"
    );
}

#[test]
fn has_blinking_text_true_after_sgr_6() {
    let (mut emu, _rx) = make_emulator();
    // ESC[6m → fast blink, then write text
    emu.handle_incoming_data(b"\x1b[6mRapidBlink");
    let snap = emu.build_snapshot();
    assert!(
        snap.has_blinking_text,
        "has_blinking_text must be true when visible text has SGR 6 (fast blink)"
    );
}

#[test]
fn has_blinking_text_false_after_blink_cleared() {
    let (mut emu, _rx) = make_emulator();
    // Write blinking text, then reset and overwrite with plain text
    emu.handle_incoming_data(b"\x1b[5mBlink\x1b[0m");
    // Move cursor back to start and overwrite with non-blinking text
    emu.handle_incoming_data(b"\x1b[H\x1b[0mPlain text here!!");
    let snap = emu.build_snapshot();
    // The blinking text was overwritten; whether has_blinking_text is true
    // depends on whether any visible tag still carries blink.
    // The "Blink" text at cols 0-4 was overwritten by "Plain text here!!"
    // (17 chars), so all original blinking cells are gone.
    assert!(
        !snap.has_blinking_text,
        "has_blinking_text must be false after all blinking cells are overwritten"
    );
}
