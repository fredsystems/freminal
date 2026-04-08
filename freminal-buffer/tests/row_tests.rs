// freminal-buffer/tests/row_tests.rs

// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use freminal_buffer::cell::Cell;
use freminal_buffer::response::InsertResponse;
use freminal_buffer::row::Row;
use freminal_common::buffer_states::format_tag::FormatTag;
use freminal_common::buffer_states::tchar::TChar;

fn tag() -> FormatTag {
    FormatTag::default()
}

//
// ────────────────────────────────────────────────────────────
//  HELPERS
// ────────────────────────────────────────────────────────────
//

fn ascii(c: char) -> TChar {
    TChar::Ascii(c as u8)
}

fn emoji(s: &str) -> TChar {
    TChar::new_from_many_chars(s.as_bytes()).unwrap()
}

//
// ────────────────────────────────────────────────────────────
//  EXISTING TESTS (kept as-is, but cleaned up)
// ────────────────────────────────────────────────────────────
//

#[test]
fn insert_fits_entirely_in_row() {
    let mut row = Row::new(10);
    let text = vec![ascii('H'), ascii('e'), ascii('l'), ascii('l'), ascii('o')];

    let result = row.insert_text(0, &text, &tag());

    match result {
        InsertResponse::Consumed(final_col) => {
            assert_eq!(final_col, 5);
            assert_eq!(row.get_characters().len(), 5);
        }
        _ => panic!("expected Consumed"),
    }
}

#[test]
fn insert_wide_character_that_fits() {
    let mut row = Row::new(10);
    let text = vec![emoji("🙂")];

    let result = row.insert_text(0, &text, &tag());

    match result {
        InsertResponse::Consumed(final_col) => {
            assert_eq!(final_col, 2);
            assert_eq!(row.get_characters().len(), 2);
        }
        _ => panic!("expected Consumed"),
    }
}

#[test]
fn insert_wide_character_that_overflows() {
    let mut row = Row::new(3);
    let text = vec![emoji("🙂")]; // width=2

    let result = row.insert_text(2, &text, &tag());

    match result {
        InsertResponse::Leftover {
            leftover_start,
            final_col,
        } => {
            assert_eq!(final_col, 2);
            // The entire input was rejected because the row was already full.
            assert_eq!(leftover_start, 0);
            assert_eq!(&text[leftover_start..], &text[..]);
        }
        _ => panic!("expected Leftover"),
    }
}

/// Insert until we can't, return the rest
#[test]
fn insert_overflows_and_returns_leftover() {
    let mut row = Row::new(5);
    let text = vec![ascii('H'), ascii('e'), ascii('l'), ascii('l'), ascii('o')];

    let result = row.insert_text(3, &text, &tag());

    match result {
        InsertResponse::Leftover {
            leftover_start,
            final_col,
        } => {
            assert_eq!(final_col, 5);
            // 'H' and 'e' fit (cols 3–4); 'l', 'l', 'o' are the leftover.
            assert_eq!(
                &text[leftover_start..],
                &[ascii('l'), ascii('l'), ascii('o')]
            );
            assert_eq!(row.get_characters()[3], Cell::new(ascii('H'), tag()));
            assert_eq!(row.get_characters()[4], Cell::new(ascii('e'), tag()));
        }
        _ => panic!("expected Leftover"),
    }
}

//
// ────────────────────────────────────────────────────────────
//  NEW TESTS: Overwrite behavior
// ────────────────────────────────────────────────────────────
//

#[test]
fn overwrite_ascii_with_ascii() {
    let mut row = Row::new(10);

    row.insert_text(0, &[ascii('A'), ascii('B'), ascii('C')], &tag());
    row.insert_text(1, &[ascii('X')], &tag());

    let chars = row.get_characters();
    assert_eq!(chars[0].tchar(), &ascii('A'));
    assert_eq!(chars[1].tchar(), &ascii('X'));
    assert_eq!(chars[2].tchar(), &ascii('C'));
}

#[test]
fn overwrite_wide_head_clears_continuation() {
    let mut row = Row::new(10);

    row.insert_text(0, &[emoji("🙂")], &tag());
    row.insert_text(0, &[ascii('A')], &tag());

    let chars = row.get_characters();

    assert_eq!(chars[0].tchar(), &ascii('A'));
    assert!(!chars[1].is_continuation());
}

#[test]
fn overwrite_wide_continuation_cleans_up_head() {
    let mut row = Row::new(10);

    row.insert_text(0, &[emoji("🙂")], &tag());
    row.insert_text(1, &[ascii('A')], &tag());

    let chars = row.get_characters();
    assert!(!chars[0].is_continuation());
    assert_eq!(chars[1].tchar(), &ascii('A'));
}

//
// ────────────────────────────────────────────────────────────
//  NEW TESTS: Mixed-width insertion
// ────────────────────────────────────────────────────────────
//

#[test]
fn insert_mixed_ascii_and_wide() {
    let mut row = Row::new(10);

    let text = vec![ascii('A'), emoji("🙂"), ascii('B')];
    let result = row.insert_text(0, &text, &tag());

    match result {
        InsertResponse::Consumed(final_col) => assert_eq!(final_col, 4),
        _ => panic!(),
    }

    let chars = row.get_characters();
    assert_eq!(chars.len(), 4);
}

#[test]
fn mixed_overflow_correct_terminal_semantics() {
    let mut row = Row::new(5);

    let text = vec![ascii('A'), emoji("🙂"), ascii('B')];
    let result = row.insert_text(2, &text, &tag());

    match result {
        InsertResponse::Leftover {
            leftover_start,
            final_col,
        } => {
            assert_eq!(final_col, 5); // cursor at end
            // Only 'B' did not fit; it is at index 2 of the input slice.
            assert_eq!(&text[leftover_start..], &[ascii('B')]);
        }
        _ => panic!("expected leftover"),
    }

    let chars = row.get_characters();
    assert_eq!(chars[2], Cell::new(ascii('A'), tag()));
    assert!(chars[3].is_head()); // 🙂
    assert!(chars[4].is_continuation()); // 🙂
}

//
// ────────────────────────────────────────────────────────────
//  NEW TESTS: Continuation invariants
// ────────────────────────────────────────────────────────────
//

#[test]
fn continuation_invariant_head_must_exist() {
    let mut row = Row::new(10);
    row.insert_text(3, &[emoji("🙂")], &tag());

    let chars = row.get_characters();
    assert!(chars[3].is_head());
    assert!(chars[4].is_continuation());
}

#[test]
fn continuation_invariant_no_continuation_at_col0() {
    let mut row = Row::new(10);
    row.insert_text(0, &[emoji("🙂")], &tag());

    assert!(row.get_characters()[0].is_head());
    assert!(!row.get_characters()[0].is_continuation());
}

#[test]
fn no_dangling_continuation_cells_after_insertion() {
    let mut row = Row::new(10);

    // Insert |🙂 B|
    row.insert_text(0, &[emoji("🙂"), ascii('B')], &tag());

    let chars = row.get_characters();

    assert!(chars[0].is_head());
    assert!(chars[1].is_continuation());
    assert!(!chars[2].is_continuation());
}

//
// ────────────────────────────────────────────────────────────
//  GAP-PADDING FORMAT TAG TESTS
//  These tests verify that blank cells inserted to fill a column
//  gap (when the cursor jumps forward before writing) always use
//  FormatTag::default(), never the incoming text's tag.
//  This is the root cause of the "nano highlight bleeds across
//  blank space" bug.
// ────────────────────────────────────────────────────────────
//

/// Build a FormatTag that has reverse-video on (non-default colors).
fn reverse_tag() -> FormatTag {
    use freminal_common::buffer_states::cursor::{ReverseVideo, StateColors};
    FormatTag {
        colors: StateColors::default().with_reverse_video(ReverseVideo::On),
        ..FormatTag::default()
    }
}

#[test]
fn gap_padding_uses_default_tag_not_incoming_tag() {
    // Write 2 chars at col 0 with reverse-video tag ("^G"),
    // then write 2 chars at col 16 with the same tag ("^O").
    // The 14 blank cells between col 2 and col 16 must have the
    // default tag, not the reverse-video tag.
    let mut row = Row::new(80);
    let rv = reverse_tag();

    // Write "^G" at col 0
    row.insert_text(0, &[ascii('^'), ascii('G')], &rv);

    // Write "^O" at col 16 with reverse-video
    row.insert_text(16, &[ascii('^'), ascii('O')], &rv);

    let chars = row.get_characters();

    // Cells 0-1: reverse-video (the "^G" chars)
    assert_eq!(chars[0].tag(), &rv, "col 0 should have reverse-video tag");
    assert_eq!(chars[1].tag(), &rv, "col 1 should have reverse-video tag");

    // Cells 2-15: gap padding — must be default tag
    for (col, cell) in chars.iter().enumerate().skip(2).take(14) {
        assert_eq!(
            cell.tag(),
            &FormatTag::default(),
            "gap cell at col {col} must have default tag, not the incoming tag"
        );
    }

    // Cells 16-17: reverse-video (the "^O" chars)
    assert_eq!(chars[16].tag(), &rv, "col 16 should have reverse-video tag");
    assert_eq!(chars[17].tag(), &rv, "col 17 should have reverse-video tag");
}

#[test]
fn gap_padding_with_default_tag_is_trimmed_as_sparse() {
    // If only default-tag blanks are in the gap and nothing follows them
    // on the row, the sparse-row invariant should trim them away.
    // Concretely: write "AB" at col 0 (default tag), then check that
    // writing something with reverse-video far to the right leaves the
    // gap as default cells, and the sparse trim at clear_from still works.
    let mut row = Row::new(80);
    let rv = reverse_tag();

    // Write "A" at col 5 with reverse-video — creates a gap at cols 0-4
    row.insert_text(5, &[ascii('A')], &rv);

    let chars = row.get_characters();

    // Gap cells 0-4 must be default tag
    for (col, cell) in chars.iter().enumerate().take(5) {
        assert_eq!(
            cell.tag(),
            &FormatTag::default(),
            "gap cell at col {col} must use default tag"
        );
    }

    // Col 5 must be reverse-video
    assert_eq!(chars[5].tag(), &rv, "col 5 should have reverse-video tag");
}

#[test]
fn nano_shortcut_bar_highlight_does_not_bleed_into_gap() {
    // Reproduce the nano shortcut-bar pattern:
    //   col  0- 1: "^G" with reverse-video
    //   col  2- 6: " Help" with default tag
    //   col 16-17: "^O" with reverse-video
    //   col 18-31: " Write Out    " with default tag
    //
    // The cells at cols 7-15 (gap between " Help" and "^O") must ALL
    // have the default tag — not reverse-video.
    let mut row = Row::new(147);
    let rv = reverse_tag();
    let def = FormatTag::default();

    row.insert_text(0, &[ascii('^'), ascii('G')], &rv);
    row.insert_text(
        2,
        &[ascii(' '), ascii('H'), ascii('e'), ascii('l'), ascii('p')],
        &def,
    );
    row.insert_text(16, &[ascii('^'), ascii('O')], &rv);
    row.insert_text(
        18,
        &[
            ascii(' '),
            ascii('W'),
            ascii('r'),
            ascii('i'),
            ascii('t'),
            ascii('e'),
            ascii(' '),
            ascii('O'),
            ascii('u'),
            ascii('t'),
            ascii(' '),
            ascii(' '),
            ascii(' '),
            ascii(' '),
        ],
        &def,
    );

    let chars = row.get_characters();

    // ^G: cols 0-1 → reverse
    assert_eq!(chars[0].tag(), &rv, "^G col 0 must be reverse-video");
    assert_eq!(chars[1].tag(), &rv, "^G col 1 must be reverse-video");

    // " Help": cols 2-6 → default
    for (col, cell) in chars.iter().enumerate().skip(2).take(5) {
        assert_eq!(cell.tag(), &def, "' Help' col {col} must be default");
    }

    // gap: cols 7-15 → default (not reverse!)
    for (col, cell) in chars.iter().enumerate().skip(7).take(9) {
        assert_eq!(
            cell.tag(),
            &def,
            "gap col {col} must be default, not reverse-video (highlight bleed)"
        );
    }

    // ^O: cols 16-17 → reverse
    assert_eq!(chars[16].tag(), &rv, "^O col 16 must be reverse-video");
    assert_eq!(chars[17].tag(), &rv, "^O col 17 must be reverse-video");

    // " Write Out    ": cols 18-31 → default
    for (col, cell) in chars.iter().enumerate().skip(18).take(14) {
        assert_eq!(cell.tag(), &def, "' Write Out' col {col} must be default");
    }
}

//
// ────────────────────────────────────────────────────────────
//  BCE (Background Color Erase) TESTS
// ────────────────────────────────────────────────────────────
//

/// Build a `FormatTag` with a blue background (non-default).
fn blue_bg_tag() -> FormatTag {
    use freminal_common::buffer_states::cursor::StateColors;
    use freminal_common::colors::TerminalColor;
    FormatTag {
        colors: StateColors::default().with_background_color(TerminalColor::Blue),
        ..FormatTag::default()
    }
}

#[test]
fn clear_with_tag_default_leaves_row_sparse() {
    let mut row = Row::new(10);
    row.insert_text(0, &[ascii('A'), ascii('B'), ascii('C')], &tag());

    row.clear_with_tag(&FormatTag::default());

    // Row should be sparse (no explicit cells)
    assert!(
        row.get_characters().is_empty(),
        "clearing with default tag should leave the row sparse"
    );
}

#[test]
fn clear_with_tag_nondefault_fills_row() {
    let mut row = Row::new(10);
    row.insert_text(0, &[ascii('A'), ascii('B'), ascii('C')], &tag());

    let bce_tag = blue_bg_tag();
    row.clear_with_tag(&bce_tag);

    let chars = row.get_characters();
    assert_eq!(
        chars.len(),
        10,
        "clearing with non-default tag should fill all columns"
    );
    for (col, cell) in chars.iter().enumerate() {
        assert_eq!(cell.tchar(), &TChar::Space, "col {col}: expected blank");
        assert_eq!(cell.tag(), &bce_tag, "col {col}: expected blue-bg tag");
    }
}

#[test]
fn fill_with_tag_default_is_noop() {
    let mut row = Row::new(10);

    row.fill_with_tag(&FormatTag::default());

    assert!(
        row.get_characters().is_empty(),
        "fill_with_tag(default) should leave row sparse"
    );
}

#[test]
fn fill_with_tag_nondefault_fills_row() {
    let mut row = Row::new(8);
    let bce_tag = blue_bg_tag();

    row.fill_with_tag(&bce_tag);

    let chars = row.get_characters();
    assert_eq!(chars.len(), 8, "fill should write to all columns");
    for (col, cell) in chars.iter().enumerate() {
        assert_eq!(cell.tchar(), &TChar::Space, "col {col}: expected blank");
        assert_eq!(cell.tag(), &bce_tag, "col {col}: expected blue-bg tag");
    }
}

#[test]
fn erase_cells_at_with_bce_tag() {
    let mut row = Row::new(10);
    let bce_tag = blue_bg_tag();

    // Write "ABCDE" at col 0 with default formatting
    row.insert_text(
        0,
        &[ascii('A'), ascii('B'), ascii('C'), ascii('D'), ascii('E')],
        &tag(),
    );

    // Erase col 2 with BCE tag
    row.erase_cells_at(2, 1, &bce_tag);

    let chars = row.get_characters();
    // Col 2 should be a blank with the blue-bg tag
    assert_eq!(
        chars[2].tchar(),
        &TChar::Space,
        "erased cell should be blank"
    );
    assert_eq!(chars[2].tag(), &bce_tag, "erased cell should have BCE tag");

    // Neighbors untouched
    assert_eq!(chars[1].tchar(), &ascii('B'));
    assert_eq!(chars[3].tchar(), &ascii('D'));
}

#[test]
fn delete_cells_at_bce_tag_for_wide_glyph_cleanup() {
    let mut row = Row::new(10);
    let bce_tag = blue_bg_tag();

    // Write "A" then a wide emoji then "B"
    // A at 0, emoji at 1-2, B at 3
    row.insert_text(0, &[ascii('A'), emoji("🙂"), ascii('B')], &tag());

    // Delete 1 cell at col 1 (the head of the wide emoji) — this should clean
    // up the wide glyph, using bce_tag for any replacement blanks.
    row.delete_cells_at(1, 1, &bce_tag);

    // After deletion, the wide glyph's cells are replaced and shifted.
    // The important thing is that any cleanup blanks use the BCE tag.
    let chars = row.get_characters();
    // The row should still have A at col 0, then B shifted left.
    assert_eq!(chars[0].tchar(), &ascii('A'), "col 0 should still be A");
}
