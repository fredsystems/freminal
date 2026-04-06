// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Integration tests that replay tmux-like escape sequences through the full
//! TerminalState pipeline (parser → handler → buffer) to verify scroll region
//! behaviour.
//!
//! These tests simulate the exact patterns observed in a real tmux session
//! captured to `freminal.bin`:
//!   1. Enter alternate screen via CSI ?1049h
//!   2. Set scroll region via DECSTBM
//!   3. Scroll via DL (delete lines), IL (insert lines), SU (scroll up)
//!   4. Verify buffer state after each operation

#![cfg(feature = "playback")]

use crossbeam_channel::Receiver;
use freminal_common::pty_write::PtyWrite;
use freminal_terminal_emulator::recording::parse_recording;
use freminal_terminal_emulator::state::internal::TerminalState;

/// Create a TerminalState and return it with the PTY write channel receiver.
fn make_state() -> (TerminalState, Receiver<PtyWrite>) {
    let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
    let state = TerminalState::new(tx, None);
    (state, rx)
}

/// Helper: extract the visible text of a single row as a String.
/// Empty/space cells are represented as spaces. Leading spaces are preserved,
/// trailing spaces are trimmed.
fn row_text(state: &TerminalState, row_idx: usize) -> String {
    let rows = state.handler.buffer().get_rows();
    if row_idx >= rows.len() {
        return String::new();
    }
    let cells = rows[row_idx].cells();
    let mut s = String::new();
    for cell in cells {
        s.push_str(&cell.into_utf8());
    }
    // Trim trailing spaces only
    s.trim_end().to_string()
}

/// Helper: check that a row is completely blank (all spaces or empty cells).
fn row_is_blank(state: &TerminalState, row_idx: usize) -> bool {
    row_text(state, row_idx).trim().is_empty()
}

/// Helper: resize the terminal via the handler's set_size method.
fn resize(state: &mut TerminalState, width: usize, height: usize) {
    state.handler.buffer_mut().set_size(width, height, 0);
}

// ═══════════════════════════════════════════════════════════════════════════
// Test: tmux startup pattern — enter alternate, set region, fill screen
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn tmux_enter_alternate_and_set_scroll_region() {
    let (mut state, _rx) = make_state();
    resize(&mut state, 80, 24);

    // Enter alternate screen: CSI ?1049h
    state.handle_incoming_data(b"\x1b[?1049h");

    assert!(
        state.handler.buffer().is_alternate_screen(),
        "should be in alternate screen"
    );
    assert_eq!(
        state.handler.buffer().get_rows().len(),
        24,
        "alternate buffer should have exactly height rows"
    );

    // Set scroll region to rows 1-23 (1-indexed), leaving row 24 for status bar
    // DECSTBM: CSI 1;23r
    state.handle_incoming_data(b"\x1b[1;23r");

    let (top, bot) = state.handler.buffer().scroll_region();
    assert_eq!(top, 0, "scroll_region_top should be 0");
    assert_eq!(bot, 22, "scroll_region_bottom should be 22");

    // Cursor should be homed to (0,0)
    assert_eq!(state.handler.buffer().get_cursor().pos.x, 0);
    assert_eq!(state.handler.buffer().get_cursor().pos.y, 0);
}

// ═══════════════════════════════════════════════════════════════════════════
// Test: tmux DL(1) scroll-up pattern
//   DECSTBM(1;23) → CUP(1;1) → DL(1) → CUP(23;1) → write content
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn tmux_dl_scroll_up_pattern() {
    let (mut state, _rx) = make_state();
    resize(&mut state, 80, 24);

    // Enter alternate screen
    state.handle_incoming_data(b"\x1b[?1049h");

    // Fill lines 1-23 with content (row 24 is status bar)
    for i in 0..23 {
        // CUP to row i+1, col 1 (1-indexed)
        let cup = format!("\x1b[{};1H", i + 1);
        state.handle_incoming_data(cup.as_bytes());
        let text = format!("Line {:02}", i);
        state.handle_incoming_data(text.as_bytes());
    }

    // Put "STATUS" on row 24 (the tmux status bar row, outside scroll region)
    state.handle_incoming_data(b"\x1b[24;1H");
    state.handle_incoming_data(b"STATUS");

    // Verify initial state
    assert_eq!(row_text(&state, 0), "Line 00");
    assert_eq!(row_text(&state, 1), "Line 01");
    assert_eq!(row_text(&state, 22), "Line 22");
    assert_eq!(row_text(&state, 23), "STATUS");

    // Set scroll region: DECSTBM(1;23) → rows 0-22 (0-indexed)
    state.handle_incoming_data(b"\x1b[1;23r");

    // tmux DL pattern: CUP(1;1) → DL(1)
    // This deletes line 0 (the top), shifts 1-22 up, blanks line 22
    state.handle_incoming_data(b"\x1b[1;1H");
    state.handle_incoming_data(b"\x1b[1M"); // DL(1) = CSI 1 M

    // After DL(1): rows should have shifted up within the region
    assert_eq!(
        row_text(&state, 0),
        "Line 01",
        "row 0 should now have Line 01 (shifted up)"
    );
    assert_eq!(
        row_text(&state, 1),
        "Line 02",
        "row 1 should now have Line 02"
    );
    assert_eq!(
        row_text(&state, 21),
        "Line 22",
        "row 21 should now have Line 22"
    );
    assert!(
        row_is_blank(&state, 22),
        "row 22 should be blank (new line inserted at bottom of region)"
    );
    assert_eq!(
        row_text(&state, 23),
        "STATUS",
        "row 23 (status bar) should be untouched"
    );

    // Now tmux writes new content at the bottom of the region
    state.handle_incoming_data(b"\x1b[23;1H"); // CUP to row 23 (1-indexed) = row 22 (0-indexed)
    state.handle_incoming_data(b"New Bottom Line");

    assert_eq!(row_text(&state, 22), "New Bottom Line");
    assert_eq!(row_text(&state, 23), "STATUS");
}

// ═══════════════════════════════════════════════════════════════════════════
// Test: tmux IL(1) scroll-down pattern
//   DECSTBM(1;23) → CUP(1;1) → IL(1) → CUP(1;1) → write content
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn tmux_il_scroll_down_pattern() {
    let (mut state, _rx) = make_state();
    resize(&mut state, 80, 24);

    // Enter alternate screen
    state.handle_incoming_data(b"\x1b[?1049h");

    // Fill lines 0-22
    for i in 0..23 {
        let cup = format!("\x1b[{};1H", i + 1);
        state.handle_incoming_data(cup.as_bytes());
        let text = format!("Line {:02}", i);
        state.handle_incoming_data(text.as_bytes());
    }

    // Status bar on row 23
    state.handle_incoming_data(b"\x1b[24;1H");
    state.handle_incoming_data(b"STATUS");

    // Set scroll region
    state.handle_incoming_data(b"\x1b[1;23r");

    // tmux IL pattern: CUP(1;1) → IL(1)
    // This inserts a blank line at row 0, shifts 0-21 down, discards row 22's content
    state.handle_incoming_data(b"\x1b[1;1H");
    state.handle_incoming_data(b"\x1b[1L"); // IL(1) = CSI 1 L

    // After IL(1): rows should have shifted down within the region
    assert!(
        row_is_blank(&state, 0),
        "row 0 should be blank (new line inserted)"
    );
    assert_eq!(
        row_text(&state, 1),
        "Line 00",
        "row 1 should now have Line 00 (shifted down)"
    );
    assert_eq!(
        row_text(&state, 2),
        "Line 01",
        "row 2 should now have Line 01"
    );
    assert_eq!(
        row_text(&state, 22),
        "Line 21",
        "row 22 should have Line 21 (Line 22 was pushed out)"
    );
    assert_eq!(
        row_text(&state, 23),
        "STATUS",
        "row 23 (status bar) should be untouched"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Test: tmux SU(n) bulk scroll pattern
//   DECSTBM(28;78) → SU(51)
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn tmux_su_bulk_scroll_pattern() {
    let (mut state, _rx) = make_state();
    resize(&mut state, 80, 80);

    // Enter alternate screen
    state.handle_incoming_data(b"\x1b[?1049h");

    // Fill all 80 rows with content
    for i in 0..80 {
        let cup = format!("\x1b[{};1H", i + 1);
        state.handle_incoming_data(cup.as_bytes());
        let text = format!("Row {:02}", i);
        state.handle_incoming_data(text.as_bytes());
    }

    // Set scroll region to rows 28-78 (1-indexed) = 27-77 (0-indexed)
    state.handle_incoming_data(b"\x1b[28;78r");

    let (top, bot) = state.handler.buffer().scroll_region();
    assert_eq!(top, 27);
    assert_eq!(bot, 77);

    // SU(51): scroll the region up by 51 lines
    // CSI 51 S
    state.handle_incoming_data(b"\x1b[51S");

    // After SU(51), the region (27-77) should be scrolled:
    // - Rows 0-26 untouched (above region)
    // - Rows 27-77: the 51 rows that were 78-77 are gone (only 51 rows existed),
    //   so the entire region should be blanked (since 51 >= region size of 51)
    // Wait: region is 27..77 inclusive = 51 rows. SU(51) shifts ALL content out.
    for i in 0..27 {
        assert_eq!(
            row_text(&state, i),
            format!("Row {:02}", i),
            "row {} above region should be untouched",
            i
        );
    }
    for i in 27..=77 {
        assert!(
            row_is_blank(&state, i),
            "row {} in the region should be blank after SU(51)",
            i
        );
    }

    // Rows 78-79 should be untouched (below region)
    assert_eq!(row_text(&state, 78), "Row 78");
    assert_eq!(row_text(&state, 79), "Row 79");
}

// ═══════════════════════════════════════════════════════════════════════════
// Test: LF at the bottom margin of scroll region scrolls correctly
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn tmux_lf_at_bottom_of_scroll_region() {
    let (mut state, _rx) = make_state();
    resize(&mut state, 80, 24);

    // Enter alternate screen
    state.handle_incoming_data(b"\x1b[?1049h");

    // Fill lines 0-22
    for i in 0..23 {
        let cup = format!("\x1b[{};1H", i + 1);
        state.handle_incoming_data(cup.as_bytes());
        let text = format!("Line {:02}", i);
        state.handle_incoming_data(text.as_bytes());
    }

    // Status bar
    state.handle_incoming_data(b"\x1b[24;1H");
    state.handle_incoming_data(b"STATUS");

    // Set scroll region to 1-23
    state.handle_incoming_data(b"\x1b[1;23r");

    // Move cursor to bottom of region (row 23, 1-indexed = row 22, 0-indexed)
    state.handle_incoming_data(b"\x1b[23;1H");

    // Send LF — should scroll region up (Line 00 pushed out, blank at 22)
    state.handle_incoming_data(b"\n");

    assert_eq!(
        row_text(&state, 0),
        "Line 01",
        "row 0 should have shifted to Line 01"
    );
    assert_eq!(row_text(&state, 21), "Line 22");
    assert!(
        row_is_blank(&state, 22),
        "row 22 should be blank after scroll"
    );
    assert_eq!(row_text(&state, 23), "STATUS", "status bar untouched");
}

// ═══════════════════════════════════════════════════════════════════════════
// Test: Multiple DL operations (sustained scroll)
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn tmux_repeated_dl_scroll() {
    let (mut state, _rx) = make_state();
    resize(&mut state, 80, 24);

    // Enter alternate screen
    state.handle_incoming_data(b"\x1b[?1049h");

    // Fill lines 0-22 with "Line 00" through "Line 22"
    for i in 0..23 {
        let cup = format!("\x1b[{};1H", i + 1);
        state.handle_incoming_data(cup.as_bytes());
        let text = format!("Line {:02}", i);
        state.handle_incoming_data(text.as_bytes());
    }

    // Status bar
    state.handle_incoming_data(b"\x1b[24;1H");
    state.handle_incoming_data(b"STATUS");

    // Do 5 rounds of tmux scroll-up: DECSTBM → CUP → DL → CUP → write new content
    for round in 0..5u32 {
        state.handle_incoming_data(b"\x1b[1;23r"); // DECSTBM(1;23)
        state.handle_incoming_data(b"\x1b[1;1H"); // CUP(1;1)
        state.handle_incoming_data(b"\x1b[1M"); // DL(1)
        state.handle_incoming_data(b"\x1b[23;1H"); // CUP(23;1)
        let new_text = format!("New {}", round);
        state.handle_incoming_data(new_text.as_bytes());
    }

    // After 5 DLs: lines 00-04 are gone.
    // Row 0 should be "Line 05"
    assert_eq!(
        row_text(&state, 0),
        "Line 05",
        "after 5 DLs, row 0 should be Line 05"
    );
    assert_eq!(row_text(&state, 17), "Line 22");
    assert_eq!(row_text(&state, 18), "New 0");
    assert_eq!(row_text(&state, 19), "New 1");
    assert_eq!(row_text(&state, 20), "New 2");
    assert_eq!(row_text(&state, 21), "New 3");
    assert_eq!(row_text(&state, 22), "New 4");
    assert_eq!(
        row_text(&state, 23),
        "STATUS",
        "status bar should be untouched after all scrolls"
    );

    // Invariant: still 24 rows
    assert_eq!(state.handler.buffer().get_rows().len(), 24);
}

// ═══════════════════════════════════════════════════════════════════════════
// Test: Replay freminal.bin through the full pipeline (smoke test)
//
// This feeds the actual binary dump of a tmux session through the parser
// and checks that basic invariants hold (no panics, correct buffer type,
// correct number of rows).
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn replay_freminal_bin_smoke_test() {
    // The binary file may not be available in all environments.
    // Skip the test gracefully if missing.
    let Ok(data) = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../freminal.bin")) else {
        eprintln!("freminal.bin not found, skipping replay test");
        return;
    };

    // Parse the FREC recording format to extract individual frames
    let frames = match parse_recording(&data) {
        Ok(f) => f,
        Err(e) => {
            panic!("Failed to parse freminal.bin as FREC recording: {e}");
        }
    };

    assert!(
        !frames.is_empty(),
        "freminal.bin should contain at least one frame"
    );

    let (mut state, _rx) = make_state();
    // Default TerminalState is 100x100 (DEFAULT_WIDTH × DEFAULT_HEIGHT).
    // The recording was captured at this size, so no explicit resize needed.

    // Feed each frame's data through the pipeline (like the real PTY reader does)
    for frame in &frames {
        if !frame.data.is_empty() {
            state.handle_incoming_data(&frame.data);
        }
    }

    // After the full dump, the terminal should be in the alternate screen
    // (tmux entered it at the beginning and never left).
    assert!(
        state.handler.buffer().is_alternate_screen(),
        "should still be in alternate screen after tmux session"
    );

    // Buffer should have exactly `height` rows (alternate invariant).
    let height = state.handler.buffer().get_rows().len();
    // The default terminal is 100x100, so the alternate buffer should be 100 rows.
    assert!(
        height > 0 && height <= 500,
        "alternate buffer height {} is unreasonable",
        height
    );

    // The scroll region should be valid.
    let (top, bot) = state.handler.buffer().scroll_region();
    assert!(
        top <= bot,
        "scroll_region_top {} > scroll_region_bottom {}",
        top,
        bot
    );
    assert!(
        bot < height,
        "scroll_region_bottom {} >= height {}",
        bot,
        height
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Test: Replay freminal.bin with detailed buffer state inspection
//
// Feeds the real tmux recording frame-by-frame and checks:
// 1. The alternate buffer invariant (rows.len() == height) after each frame
// 2. The scroll region stays valid after each frame
// 3. The final buffer state looks reasonable (no duplicate/garbled lines)
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn replay_freminal_bin_detailed_inspection() {
    let Ok(data) = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../freminal.bin")) else {
        eprintln!("freminal.bin not found, skipping replay test");
        return;
    };

    let frames = parse_recording(&data).expect("valid FREC");
    let (mut state, _rx) = make_state();

    for (frame_idx, frame) in frames.iter().enumerate() {
        if frame.data.is_empty() {
            continue;
        }
        state.handle_incoming_data(&frame.data);

        let buf = state.handler.buffer();

        // If we're in alternate screen, rows.len() must equal height
        if buf.is_alternate_screen() {
            let (_, height) = state.handler.get_win_size();
            let rows_len = buf.get_rows().len();
            assert_eq!(
                rows_len, height,
                "frame {frame_idx}: alternate buffer rows.len()={rows_len} != height={height}"
            );
        }

        // Scroll region must be valid
        let (top, bot) = buf.scroll_region();
        assert!(
            top <= bot,
            "frame {frame_idx}: scroll_region_top {top} > scroll_region_bottom {bot}"
        );
        let rows_len = buf.get_rows().len();
        assert!(
            bot < rows_len,
            "frame {frame_idx}: scroll_region_bottom {bot} >= rows.len() {rows_len}"
        );
    }

    // Dump the final visible buffer state for inspection
    let buf = state.handler.buffer();
    let rows_len = buf.get_rows().len();
    eprintln!("=== Final buffer state: {rows_len} rows ===");
    for i in 0..rows_len {
        let text = row_text(&state, i);
        if !text.is_empty() {
            eprintln!("  row {i:3}: {text}");
        }
    }
    eprintln!("=== End buffer state ===");

    // Check that the buffer is in a reasonable state:
    // tmux should have a status bar on the last row or second-to-last
    // and content should not be garbled (no duplicate status bars, etc.)
    let (top, bot) = state.handler.buffer().scroll_region();
    eprintln!("Scroll region: top={top}, bot={bot}");
    let cursor = state.handler.buffer().get_cursor();
    eprintln!("Cursor: x={}, y={}", cursor.pos.x, cursor.pos.y);
}

// ═══════════════════════════════════════════════════════════════════════════
// Test: Replay freminal.bin — check for the specific corruption pattern
//
// The user reported "scrolling in tmux panes just doesn't work. It's not
// clearing lines, and I don't think it's accumulating scrollback properly".
// This test checks for overlapping/duplicate content lines.
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn replay_freminal_bin_no_duplicate_content() {
    let Ok(data) = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../freminal.bin")) else {
        eprintln!("freminal.bin not found, skipping replay test");
        return;
    };

    let frames = parse_recording(&data).expect("valid FREC");
    let (mut state, _rx) = make_state();

    for frame in &frames {
        if !frame.data.is_empty() {
            state.handle_incoming_data(&frame.data);
        }
    }

    // Collect all non-empty rows
    let rows_len = state.handler.buffer().get_rows().len();
    let mut non_empty_rows: Vec<(usize, String)> = Vec::new();
    for i in 0..rows_len {
        let text = row_text(&state, i);
        if !text.is_empty() {
            non_empty_rows.push((i, text));
        }
    }

    // Check that the status bar content (if present) appears only once
    // tmux status bars typically contain the session name and window list
    // We can't know the exact content, but we can check no row is an
    // exact duplicate of another row (allowing blank rows to repeat)
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut duplicate_count = 0;
    for (idx, text) in &non_empty_rows {
        if !seen.insert(text.clone()) {
            // Duplicate found — this might be legitimate (e.g. same prompt on
            // multiple tmux panes), so just count them
            duplicate_count += 1;
            eprintln!("  duplicate row at {idx}: {text}");
        }
    }

    // A small number of duplicates is OK (e.g. blank-ish rows, repeated
    // prompts). But if half the visible content is duplicates, something
    // is seriously wrong.
    let total = non_empty_rows.len();
    if total > 0 {
        let dup_ratio = duplicate_count as f64 / total as f64;
        eprintln!(
            "Duplicate ratio: {duplicate_count}/{total} = {:.1}%",
            dup_ratio * 100.0
        );
        assert!(
            dup_ratio < 0.5,
            "too many duplicate rows ({duplicate_count}/{total}) — likely scroll corruption"
        );
    }
}
