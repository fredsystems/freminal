// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Regression tests for issue #463: the scrollback search corpus must place
//! exactly one row separator at the scrollback/visible seam.
//!
//! `visible_as_tchars_and_tags` and `scrollback_as_tchars_and_tags` each
//! flatten their own slice of rows and deliberately emit no trailing
//! separator after their own last row. Concatenating the two therefore fuses
//! the last scrollback row with the first visible row unless a separator is
//! inserted between them, which made every match from the visible window
//! onward report a row index one too low.

use freminal_common::buffer_states::tchar::TChar;
use freminal_terminal_emulator::interface::TerminalEmulator;

/// Split a corpus into rows the same way `run_search` walks it.
fn corpus_rows(corpus: &[TChar]) -> Vec<String> {
    corpus
        .split(|c| matches!(c, TChar::NewLine))
        .map(|r| {
            r.iter()
                .map(std::string::ToString::to_string)
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .collect()
}

/// With scrollback present, corpus row `n` must be buffer row `n`.
#[test]
fn corpus_row_indices_match_buffer_rows_across_the_seam() {
    let (mut emu, _rx) = TerminalEmulator::new_headless(None);
    let _ = emu.set_win_size(10, 3, 8, 16);

    // Six distinct lines on a three-row screen leaves scrollback behind.
    for i in 0..6 {
        emu.handle_incoming_data(format!("row{i}\r\n").as_bytes());
    }

    let corpus = emu.internal.handler.search_corpus(0);
    let rows = corpus_rows(&corpus.chars);
    let total_rows = emu.internal.handler.buffer().rows().len();

    assert_eq!(
        rows.len(),
        total_rows,
        "corpus row count must equal the buffer's row count; got {rows:?}"
    );
    for i in 0..6 {
        assert_eq!(
            rows[i],
            format!("row{i}"),
            "corpus row {i} must be buffer row {i}; got {rows:?}"
        );
    }
}

/// No two rows may be fused: the seam row must not contain the text of the
/// row that follows it.
#[test]
fn seam_does_not_fuse_last_scrollback_row_with_first_visible_row() {
    let (mut emu, _rx) = TerminalEmulator::new_headless(None);
    let _ = emu.set_win_size(10, 3, 8, 16);
    for i in 0..6 {
        emu.handle_incoming_data(format!("row{i}\r\n").as_bytes());
    }

    let rows = corpus_rows(&emu.internal.handler.search_corpus(0).chars);
    assert!(
        !rows.iter().any(|r| r == "row3row4"),
        "the scrollback/visible seam fused two rows: {rows:?}"
    );
}

/// A completely blank scrollback row still occupies a row index. It flattens
/// to zero characters, so the separator decision must be driven by the row
/// count rather than by whether the scrollback produced any text.
#[test]
fn blank_scrollback_row_still_gets_a_separator() {
    let (mut emu, _rx) = TerminalEmulator::new_headless(None);
    let _ = emu.set_win_size(10, 3, 8, 16);

    // One empty line, then enough content to push it into scrollback.
    emu.handle_incoming_data(b"\r\n");
    for i in 0..3 {
        emu.handle_incoming_data(format!("row{i}\r\n").as_bytes());
    }

    let rows = corpus_rows(&emu.internal.handler.search_corpus(0).chars);
    let total_rows = emu.internal.handler.buffer().rows().len();
    assert_eq!(
        rows.len(),
        total_rows,
        "a blank scrollback row must still be counted; got {rows:?}"
    );
    assert_eq!(rows[0], "", "row 0 is the blank scrollback line");
    assert_eq!(rows[1], "row0", "row 1 must not have been fused with row 0");
}

/// With no scrollback at all, no leading separator may be introduced --
/// that would shift every row the other way.
#[test]
fn no_scrollback_means_no_leading_separator() {
    let (mut emu, _rx) = TerminalEmulator::new_headless(None);
    let _ = emu.set_win_size(10, 5, 8, 16);
    emu.handle_incoming_data(b"alpha\r\nbeta");

    let rows = corpus_rows(&emu.internal.handler.search_corpus(0).chars);
    assert_eq!(rows[0], "alpha", "row 0 must be the first visible row");
    assert_eq!(rows[1], "beta");
}

/// Task 125.17: the corpus is tagged with the rows it was cut from, and that
/// tag keeps pace with the buffer at scrollback capacity, where the row
/// count freezes but the base advances.
#[test]
fn corpus_extent_matches_the_snapshot_and_advances_at_capacity() {
    let (mut emu, _rx) = TerminalEmulator::new_headless(Some(10));
    let _ = emu.set_win_size(10, 3, 8, 16);
    for i in 0..4 {
        emu.handle_incoming_data(format!("row{i}\r\n").as_bytes());
    }

    let corpus = emu.internal.handler.search_corpus(0);
    let snap = emu.build_snapshot();
    assert_eq!(corpus.extent, snap.extent(), "corpus and snapshot agree");
    assert_eq!(
        corpus_rows(&corpus.chars).len(),
        corpus.extent.total_rows,
        "corpus row count is the extent's row count"
    );

    // Fill to capacity, then keep going: total_rows freezes, row_base moves.
    for i in 0..200 {
        emu.handle_incoming_data(format!("fill{i}\r\n").as_bytes());
    }
    let at_capacity = emu.internal.handler.search_corpus(0);
    assert!(
        at_capacity.extent.row_base > corpus.extent.row_base,
        "the corpus base must advance with eviction"
    );
    let frozen_total = at_capacity.extent.total_rows;
    emu.handle_incoming_data(b"one more\r\n");
    let next = emu.internal.handler.search_corpus(0);
    assert_eq!(next.extent.total_rows, frozen_total, "total_rows is frozen");
    assert_ne!(
        next.extent, at_capacity.extent,
        "yet the extent still differs, so a staleness check can see it"
    );

    // Corpus row i is buffer row `row_base + i`: the last row holds the cursor
    // line, and the one above it the final written line.
    let rows = corpus_rows(&next.chars);
    assert_eq!(rows[rows.len() - 2], "one more");
}
