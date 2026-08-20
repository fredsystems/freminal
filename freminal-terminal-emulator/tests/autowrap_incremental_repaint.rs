// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Regression tests for issue #491: autowrap must not erase the row it wraps
//! onto.
//!
//! Autowrap is defined as cursor movement. A TUI is entitled to repaint a
//! soft-wrapped line by touching only the cells that changed and using
//! cursor-forward / absolute positioning to skip the rest, relying on autowrap
//! alone to step from one screen row to the next. Clearing the destination row
//! on wrap destroyed every cell the repaint did not explicitly write.
//!
//! The concrete symptom was a long soft-wrapped line in neovim rendering as a
//! column of single characters down the right-hand edge of the screen: neovim
//! emits `<space>`, `CSI 122 C`, `<final char>` per row and lets autowrap
//! advance, so each row kept only those two cells.

mod vttest_common;

use vttest_common::VtTestHelper;

/// The minimal shape of the bug: fill two rows, then repaint the first one
/// using `CUF` to skip its middle, and let autowrap carry the cursor onto the
/// second row.
#[test]
fn autowrap_preserves_destination_row_contents() {
    let mut h = VtTestHelper::new(10, 4);

    h.feed_str("\x1b[HAAAAAAAAAA");
    h.feed_str("BBBBBBBBBB");
    assert_eq!(h.screen_text()[0], "AAAAAAAAAA");
    assert_eq!(h.screen_text()[1], "BBBBBBBBBB");

    // Repaint row 0's first and last cell only; CUF skips the middle.
    h.feed_str("\x1b[H");
    h.feed_str("X");
    h.feed_str("\x1b[8C");
    h.feed_str("Y");
    assert_eq!(
        h.screen_text()[0],
        "XAAAAAAAAY",
        "CUF must skip cells rather than erase them"
    );

    // Cursor is now in the pending-wrap state. The next printable character
    // wraps onto row 1 and writes exactly one cell there.
    h.feed_str("Z");
    assert_eq!(
        h.screen_text()[1],
        "ZBBBBBBBBB",
        "autowrap must not clear the row it wraps onto"
    );
}

/// The full neovim idiom, repeated down the screen: `<space>`, `CUF width-2`,
/// `<char>`, relying on autowrap to advance. Every row must retain the body
/// text that was painted earlier.
#[test]
fn neovim_style_incremental_repaint_of_wrapped_line() {
    const WIDTH: usize = 10;
    const ROWS: usize = 4;
    let mut h = VtTestHelper::new(WIDTH, ROWS);

    // Initial full paint: four rows of distinct filler.
    h.feed_str("\x1b[H");
    for r in 0..ROWS {
        let fill: String = std::iter::repeat_n(
            char::from(b'a' + u8::try_from(r).expect("row index fits in u8")),
            WIDTH,
        )
        .collect();
        h.feed_str(&fill);
    }
    for r in 0..ROWS {
        let expected: String = std::iter::repeat_n(
            char::from(b'a' + u8::try_from(r).expect("row index fits in u8")),
            WIDTH,
        )
        .collect();
        assert_eq!(h.screen_text()[r], expected, "row {r} initial paint");
    }

    // Incremental repaint: rewrite only column 0 and the final column of every
    // row, stepping between rows via autowrap alone.
    h.feed_str("\x1b[H");
    for r in 0..ROWS {
        h.feed_str(" ");
        h.feed_str(&format!("\x1b[{}C", WIDTH - 2));
        h.feed_str(&r.to_string());
    }

    for r in 0..ROWS {
        let body: String = std::iter::repeat_n(
            char::from(b'a' + u8::try_from(r).expect("row index fits in u8")),
            WIDTH - 2,
        )
        .collect();
        let expected = format!(" {body}{r}");
        assert_eq!(
            h.screen_text()[r],
            expected,
            "row {r} must keep the body text the repaint skipped over"
        );
    }
}

/// Guard against over-correcting: wrapped text that genuinely covers cells
/// must still overwrite them.
#[test]
fn autowrap_still_overwrites_what_it_writes() {
    let mut h = VtTestHelper::new(10, 4);

    h.feed_str("\x1b[HAAAAAAAAAA");
    h.feed_str("BBBBBBBBBB");

    // 13 characters from row 0 col 0: 10 fill row 0, 3 wrap onto row 1.
    h.feed_str("\x1b[H");
    h.feed_str("CCCCCCCCCCCCC");

    assert_eq!(h.screen_text()[0], "CCCCCCCCCC");
    assert_eq!(
        h.screen_text()[1],
        "CCCBBBBBBB",
        "wrapped text overwrites the cells it covers, and only those"
    );
}
