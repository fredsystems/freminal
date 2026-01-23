// freminal-buffer/tests/buffer_tests.rs

use freminal_buffer::buffer::Buffer;
use freminal_common::buffer_states::tchar::TChar;

fn ascii(c: char) -> TChar {
    TChar::Ascii(c as u8)
}

fn emoji(s: &str) -> TChar {
    TChar::Utf8(s.as_bytes().to_vec())
}

#[test]
fn insert_simple_text_in_buffer() {
    let mut buf = Buffer::new(10, 10);

    buf.insert_text(&[ascii('H'), ascii('e'), ascii('l'), ascii('l'), ascii('o')]);

    assert_eq!(buf.get_cursor().pos.x, 5);
    assert_eq!(buf.get_cursor().pos.y, 0);
}

#[test]
fn insert_wraps_into_next_row() {
    let mut buf = Buffer::new(5, 10);

    buf.insert_text(&[ascii('H'), ascii('e'), ascii('l'), ascii('l'), ascii('o')]); // col=5 -> wrap
    buf.insert_text(&[ascii('!')]);

    assert_eq!(buf.get_cursor().pos.y, 1);
    assert_eq!(buf.get_cursor().pos.x, 1);
}

#[test]
fn insert_wide_char_wrap() {
    let mut buf = Buffer::new(4, 10);

    buf.insert_text(&[ascii('A'), emoji("🙂")]); // A takes 1, 🙂 takes 2 → 3 total

    assert_eq!(buf.get_cursor().pos.x, 3);

    buf.insert_text(&[emoji("🙂")]); // does NOT fit at col 3 → wraps

    assert_eq!(buf.get_cursor().pos.y, 1);
    assert_eq!(buf.get_cursor().pos.x, 2);
}

#[test]
fn insert_multiple_wraps() {
    let mut buf = Buffer::new(3, 10);

    buf.insert_text(&[ascii('A'), ascii('B'), ascii('C'), ascii('D'), ascii('E')]);

    assert_eq!(buf.get_cursor().pos.y, 1);
    assert_eq!(buf.get_cursor().pos.x, 2);
}

#[test]
fn multi_row_mixed_width_insertion() {
    let mut buf = Buffer::new(4, 10);

    buf.insert_text(&[ascii('A'), emoji("🙂"), ascii('B'), emoji("🙂")]);
    // Expected:
    // Row 0: A 🙂 B → col=4 (wrap)
    // Row 1: 🙂     → col=2

    assert_eq!(buf.get_cursor().pos.y, 1);
    assert_eq!(buf.get_cursor().pos.x, 2);
}
