// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Task 148.5 (issue #558): `insert_text` terminates even when the margins
//! give a fresh row no room at all.
//!
//! Valid state never has such margins -- DECSLRM requires `left < right <
//! width`, and `set_size` keeps them in range -- so these tests corrupt the
//! private margin fields directly. They pin the termination guarantee as
//! independent of that invariant: before the fix each of them looped forever,
//! wrapping to a new row that offered no more room than the last.

use freminal_common::buffer_states::modes::declrmm::Declrmm;
use freminal_common::buffer_states::tchar::TChar;

use crate::buffer::Buffer;

/// A buffer whose wrap span is empty: the left margin sits at the first
/// column past the wrap column, so every row starts out "full".
fn buffer_with_empty_wrap_span() -> Buffer {
    let mut buf = Buffer::new(10, 3);
    buf.set_declrmm(Declrmm::Enabled);
    buf.scroll_region_left = 12;
    buf.scroll_region_right = 11;
    buf.cursor.pos.x = 12;
    buf
}

#[test]
fn text_is_discarded_when_a_fresh_row_has_no_room() {
    let mut buf = buffer_with_empty_wrap_span();
    let rows_before = buf.rows().len();

    buf.insert_text(&[TChar::Ascii(b'a'), TChar::Ascii(b'b')]);

    assert_eq!(buf.rows().len(), rows_before, "nothing may wrap or push");
    assert!(
        buf.rows()[0]
            .cells()
            .iter()
            .all(|c| c.tchar() == &TChar::Space),
        "nothing may be written"
    );
}

#[test]
fn empty_text_terminates_when_a_fresh_row_has_no_room() {
    // With no glyph to discard, only the wrap-progress guard stops this: the
    // cursor is already past the wrap column, wrapping lands past it again.
    let mut buf = buffer_with_empty_wrap_span();
    let rows_before = buf.rows().len();

    buf.insert_text(&[]);

    assert!(
        buf.rows().len() <= rows_before + 1,
        "at most the one wrap before the guard trips"
    );
}
