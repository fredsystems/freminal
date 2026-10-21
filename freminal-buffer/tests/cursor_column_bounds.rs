// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! The cursor column never leaves `0..=width` (Task 148.3,
//! GHSA-8f32-mj37-fw6v).
//!
//! `x == width` is the legitimate pending-wrap position; anything greater is a
//! stranded cursor that a later CBT used to slice the tab-stop table with,
//! panicking in release builds. Each test here pins one *producer* of such a
//! column, found by fuzzing: it fails on the code before the fix.

use freminal_buffer::buffer::Buffer;
use freminal_common::buffer_states::modes::{decawm::Decawm, declrmm::Declrmm};
use freminal_common::buffer_states::tchar::TChar;

fn text(len: usize) -> Vec<TChar> {
    vec![TChar::Ascii(b'x'); len]
}

fn assert_cursor_in_bounds(buf: &Buffer, context: &str) {
    let x = buf.cursor().pos.x;
    assert!(
        x <= buf.terminal_width(),
        "{context}: cursor.x {x} past width {}",
        buf.terminal_width()
    );
}

/// Write one character at a time so every intermediate cursor is checked.
fn write_and_check(buf: &mut Buffer, count: usize, context: &str) {
    for i in 0..count {
        buf.insert_text(&text(1));
        assert_cursor_in_bounds(buf, &format!("{context}, char {i}"));
    }
}

// ── Producer: set_size left DECSLRM margins at the old right edge ─────────

#[test]
fn narrowing_resize_with_declrmm_keeps_text_cursor_in_bounds_no_autowrap() {
    // The fuzz reproducer's shape: a wide buffer narrowed while DECLRMM is
    // on, DECAWM off, then more text than a row holds.
    let mut buf = Buffer::new(100, 5);
    buf.set_declrmm(Declrmm::Enabled);
    buf.set_wrap(Decawm::NoAutoWrap);
    buf.set_size(28, 5, 0);
    write_and_check(&mut buf, 40, "no-autowrap");
}

#[test]
fn narrowing_resize_with_declrmm_keeps_text_cursor_in_bounds_autowrap() {
    let mut buf = Buffer::new(100, 5);
    buf.set_declrmm(Declrmm::Enabled);
    buf.set_size(28, 5, 0);
    write_and_check(&mut buf, 100, "autowrap");
}

#[test]
fn full_width_margins_follow_a_narrowing_resize() {
    let mut buf = Buffer::new(100, 5);
    buf.set_declrmm(Declrmm::Enabled);
    assert_eq!(buf.left_right_margins(), (0, 99));
    buf.set_size(28, 5, 0);
    assert_eq!(buf.left_right_margins(), (0, 27));
}

#[test]
fn full_width_margins_follow_a_widening_resize() {
    let mut buf = Buffer::new(28, 5);
    buf.set_declrmm(Declrmm::Enabled);
    buf.set_size(100, 5, 0);
    assert_eq!(buf.left_right_margins(), (0, 99));
}

#[test]
fn partial_margins_that_still_fit_survive_a_resize() {
    let mut buf = Buffer::new(40, 5);
    buf.set_declrmm(Declrmm::Enabled);
    buf.set_left_right_margins(3, 10); // 1-based -> (2, 9)
    buf.set_size(20, 5, 0);
    assert_eq!(buf.left_right_margins(), (2, 9));
}

#[test]
fn partial_margins_that_no_longer_fit_reset_to_full_width() {
    let mut buf = Buffer::new(40, 5);
    buf.set_declrmm(Declrmm::Enabled);
    buf.set_left_right_margins(21, 30); // -> (20, 29)
    buf.set_size(12, 5, 0);
    assert_eq!(buf.left_right_margins(), (0, 11));
    write_and_check(&mut buf, 40, "reset margins");
}

#[test]
fn height_only_resize_leaves_margins_alone() {
    let mut buf = Buffer::new(40, 5);
    buf.set_declrmm(Declrmm::Enabled);
    buf.set_left_right_margins(3, 10);
    buf.set_size(40, 9, 0);
    assert_eq!(buf.left_right_margins(), (2, 9));
}
