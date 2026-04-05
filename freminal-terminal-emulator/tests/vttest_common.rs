// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Shared test helpers for the vttest integration test suite.
//!
//! Provides [`VtTestHelper`], a wrapper around [`TerminalState`] that:
//! - Constructs an 80x24 terminal (the standard VT100 size used by vttest).
//! - Feeds raw escape-sequence byte slices through the full parse pipeline.
//! - Extracts the visible screen as a `Vec<String>` (one per row, trailing
//!   whitespace trimmed).
//! - Compares screen content against golden reference files in `tests/golden/`.
//! - Supports `UPDATE_GOLDEN=1` to regenerate golden files.
//! - Provides cursor position and format tag assertions.
//!
//! # Golden File Format
//!
//! Golden files are plain UTF-8 text. Each line corresponds to one terminal row
//! with trailing whitespace trimmed. The file has exactly as many lines as the
//! terminal height (24 by default), with empty rows represented as empty lines.
//! A trailing newline terminates the last row.
//!
//! A header comment on the first line starting with `# ` records the cursor
//! position at the time the golden file was generated:
//!
//! ```text
//! # cursor: (col, row)
//! line 1 content
//! line 2 content
//! ...
//! ```

// This module is included via `mod vttest_common;` from multiple test binaries.
// Each binary uses a different subset of helpers, so unused items are expected.
#![allow(dead_code)]

use freminal_common::{buffer_states::cursor::CursorPos, pty_write::PtyWrite};
use freminal_terminal_emulator::{input::TerminalInput, state::internal::TerminalState};
use std::{
    fmt::Write as _,
    path::{Path, PathBuf},
};

/// Default terminal width for vttest (columns).
const DEFAULT_WIDTH: usize = 80;
/// Default terminal height for vttest (rows).
const DEFAULT_HEIGHT: usize = 24;

/// A test helper that wraps a [`TerminalState`] configured for vttest-style
/// integration tests.
pub struct VtTestHelper {
    pub state: TerminalState,
    pub width: usize,
    pub height: usize,
    write_rx: crossbeam_channel::Receiver<PtyWrite>,
}

impl VtTestHelper {
    /// Create a new test helper with the given terminal dimensions.
    ///
    /// The terminal starts in the default state: primary screen, cursor at
    /// (0, 0), no scroll region, DECAWM on, DECOM off.
    #[must_use]
    pub fn new(width: usize, height: usize) -> Self {
        let (write_tx, write_rx) = crossbeam_channel::unbounded::<PtyWrite>();
        let mut state = TerminalState::new(write_tx, None);
        // The default TerminalState uses DEFAULT_WIDTH x DEFAULT_HEIGHT (100x100).
        // Resize to the requested dimensions.
        #[allow(clippy::cast_possible_truncation)]
        state.set_win_size(width, height, 8, 16);
        Self {
            state,
            width,
            height,
            write_rx,
        }
    }

    /// Create a new test helper with the standard vttest dimensions (80x24).
    #[must_use]
    pub fn new_default() -> Self {
        Self::new(DEFAULT_WIDTH, DEFAULT_HEIGHT)
    }

    /// Feed raw bytes (including escape sequences) through the full terminal
    /// parse pipeline.
    pub fn feed(&mut self, data: &[u8]) {
        self.state.handle_incoming_data(data);
    }

    /// Feed a string as bytes through the terminal.
    pub fn feed_str(&mut self, s: &str) {
        self.feed(s.as_bytes());
    }

    /// Extract the visible screen as a `Vec<String>`, one entry per row.
    ///
    /// Trailing whitespace on each row is trimmed (terminal rows are
    /// space-padded to the full width). The returned vector always has
    /// exactly `self.height` entries.
    #[must_use]
    pub fn screen_text(&self) -> Vec<String> {
        let buffer = self.state.handler.buffer();
        let visible = buffer.visible_rows(0);
        let mut lines = Vec::with_capacity(self.height);

        for row_index in 0..self.height {
            let mut line = String::new();

            if let Some(row) = visible.get(row_index) {
                for col in 0..self.width {
                    let cell = row.resolve_cell(col);
                    if cell.is_continuation() {
                        continue;
                    }
                    let _ = write!(line, "{}", cell.tchar());
                }
            }

            // Trim trailing whitespace (spaces that pad the row).
            let trimmed = line.trim_end().to_owned();
            lines.push(trimmed);
        }

        // Pad with empty strings if visible_rows returned fewer than height.
        while lines.len() < self.height {
            lines.push(String::new());
        }

        lines
    }

    /// Get the cursor position in screen coordinates (0-indexed).
    #[must_use]
    pub fn cursor_pos(&self) -> CursorPos {
        self.state.handler.buffer().get_cursor_screen_pos()
    }

    /// Assert the cursor is at the given screen position (0-indexed).
    ///
    /// # Panics
    ///
    /// Panics with a descriptive message if the cursor position does not match.
    pub fn assert_cursor_pos(&self, expected_col: usize, expected_row: usize) {
        let actual = self.cursor_pos();
        assert_eq!(
            (actual.x, actual.y),
            (expected_col, expected_row),
            "cursor position mismatch: expected (col={expected_col}, row={expected_row}), \
             got (col={}, row={})",
            actual.x,
            actual.y,
        );
    }

    /// Assert that the given row (0-indexed) contains the expected text.
    ///
    /// Trailing whitespace is trimmed before comparison.
    ///
    /// # Panics
    ///
    /// Panics if the row content does not match.
    pub fn assert_row(&self, row: usize, expected: &str) {
        let screen = self.screen_text();
        assert!(
            row < screen.len(),
            "row {row} out of bounds (screen has {} rows)",
            screen.len(),
        );
        assert_eq!(
            screen[row], expected,
            "row {row} content mismatch:\n  expected: {expected:?}\n  actual:   {:?}",
            screen[row],
        );
    }

    /// Compare the current screen against a golden reference file.
    ///
    /// The golden file is stored at `tests/golden/{test_name}.txt` relative to
    /// the `freminal-terminal-emulator` crate root.
    ///
    /// If the environment variable `UPDATE_GOLDEN` is set to `1`, the actual
    /// output is written as the new golden file instead of comparing.
    ///
    /// # Panics
    ///
    /// Panics with a readable row-by-row diff if the screen does not match the
    /// golden reference.
    pub fn assert_screen(&self, test_name: &str) {
        let golden_path = golden_file_path(test_name);
        let actual_lines = self.screen_text();
        let cursor = self.cursor_pos();

        // UPDATE_GOLDEN mode: write actual output as the new golden file.
        if std::env::var("UPDATE_GOLDEN").as_deref() == Ok("1") {
            write_golden_file(&golden_path, &actual_lines, &cursor);
            return;
        }

        // Read the golden file.
        let golden_content = match std::fs::read_to_string(&golden_path) {
            Ok(content) => content,
            Err(e) => {
                panic!(
                    "golden file not found: {}\n\
                     Error: {e}\n\
                     Run with UPDATE_GOLDEN=1 to create it:\n  \
                     UPDATE_GOLDEN=1 cargo test --all",
                    golden_path.display(),
                );
            }
        };

        let (golden_cursor, golden_lines) = parse_golden_file(&golden_content);

        // Compare cursor position if the golden file has a cursor header.
        if let Some(expected_cursor) = golden_cursor {
            assert_eq!(
                (cursor.x, cursor.y),
                (expected_cursor.x, expected_cursor.y),
                "cursor position mismatch vs golden file {test_name}.txt:\n\
                 expected: (col={}, row={})\n\
                 actual:   (col={}, row={})",
                expected_cursor.x,
                expected_cursor.y,
                cursor.x,
                cursor.y,
            );
        }

        // Compare screen content row by row.
        let max_rows = actual_lines.len().max(golden_lines.len());
        let mut mismatches = Vec::new();

        for i in 0..max_rows {
            let actual = actual_lines.get(i).map_or("", String::as_str);
            let golden = golden_lines.get(i).map_or("", String::as_str);
            if actual != golden {
                mismatches.push(format!(
                    "  row {i:2}:\n    expected: {golden:?}\n    actual:   {actual:?}"
                ));
            }
        }

        if !mismatches.is_empty() {
            let diff = mismatches.join("\n");
            panic!(
                "screen mismatch vs golden file {test_name}.txt ({} row(s) differ):\n{diff}\n\n\
                 Run with UPDATE_GOLDEN=1 to update:\n  \
                 UPDATE_GOLDEN=1 cargo test --all",
                mismatches.len(),
            );
        }
    }

    /// Drain all pending PTY write-back messages and return them as raw byte
    /// vectors.
    ///
    /// This is useful for asserting on device report responses (DA, DSR, CPR,
    /// etc.) that the terminal sends back to the PTY.
    #[must_use]
    pub fn drain_pty_writes(&self) -> Vec<Vec<u8>> {
        let mut messages = Vec::new();
        while let Ok(msg) = self.write_rx.try_recv() {
            match msg {
                PtyWrite::Write(bytes) => messages.push(bytes),
                PtyWrite::Resize(_) => {} // Ignore resize messages.
            }
        }
        messages
    }

    /// Drain PTY writes and return them concatenated into a single byte vector.
    #[must_use]
    pub fn drain_pty_writes_concatenated(&self) -> Vec<u8> {
        self.drain_pty_writes().into_iter().flatten().collect()
    }

    /// Simulate a terminal input event (e.g. a keypress) through the full
    /// `TerminalState::write()` pipeline.
    ///
    /// This calls `to_payload()` using the terminal's current mode flags
    /// (DECCKM, LNM, etc.) and sends the resulting bytes to the PTY channel.
    /// The bytes can then be read back with [`drain_pty_writes`].
    pub fn write_terminal_input(&self, input: &TerminalInput) {
        self.state
            .write(input)
            .expect("write_terminal_input: send to PTY channel failed");
    }
}

// ─── Golden File I/O ────────────────────────────────────────────────────────

/// Compute the path to a golden reference file.
fn golden_file_path(test_name: &str) -> PathBuf {
    // The test binary runs from the workspace root. The golden files live
    // relative to the freminal-terminal-emulator crate.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("golden")
        .join(format!("{test_name}.txt"))
}

/// Write a golden reference file.
fn write_golden_file(path: &Path, lines: &[String], cursor: &CursorPos) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("failed to create golden directory");
    }

    let mut content = String::new();
    // Header with cursor position.
    let _ = writeln!(content, "# cursor: ({}, {})", cursor.x, cursor.y);
    for line in lines {
        let _ = writeln!(content, "{line}");
    }
    std::fs::write(path, &content).unwrap_or_else(|e| {
        panic!("failed to write golden file {}: {e}", path.display());
    });
}

/// Parse a golden reference file into an optional cursor position and screen
/// lines.
fn parse_golden_file(content: &str) -> (Option<CursorPos>, Vec<String>) {
    let mut cursor = None;
    let mut lines = Vec::new();

    for line in content.lines() {
        if line.starts_with("# cursor: ") {
            // Parse "# cursor: (col, row)"
            let coords = line
                .strip_prefix("# cursor: (")
                .and_then(|s| s.strip_suffix(')'))
                .and_then(|s| s.split_once(", "));
            if let Some((col_str, row_str)) = coords
                && let (Ok(col), Ok(row)) = (col_str.parse::<usize>(), row_str.parse::<usize>())
            {
                cursor = Some(CursorPos { x: col, y: row });
            }
        } else {
            // Trim trailing whitespace to normalize.
            lines.push(line.trim_end().to_owned());
        }
    }

    // Remove the trailing empty line that `writeln!` in `write_golden_file`
    // produces (the last `writeln!` adds a newline after the last row, which
    // `lines()` turns into an empty trailing element only if the file ends
    // with a double-newline — but `lines()` actually drops trailing empty
    // strings, so this is a no-op in practice).

    (cursor, lines)
}
