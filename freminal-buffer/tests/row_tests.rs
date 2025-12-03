// freminal-buffer/tests/row_tests.rs

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
    TChar::Utf8(s.as_bytes().to_vec())
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
        InsertResponse::Leftover { data, final_col } => {
            assert_eq!(final_col, 2);
            assert_eq!(data, text);
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
        InsertResponse::Leftover { data, final_col } => {
            assert_eq!(final_col, 5);
            assert_eq!(data, vec![ascii('l'), ascii('l'), ascii('o')]);
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
        InsertResponse::Leftover { final_col, data } => {
            assert_eq!(final_col, 5); // cursor at end
            assert_eq!(data, vec![ascii('B')]); // only B does not fit
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
