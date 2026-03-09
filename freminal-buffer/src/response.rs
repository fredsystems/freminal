// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

/// Response returned by `Row::insert_text` and consumed by `Buffer::insert_text`.
///
/// Using a start-index for `Leftover` instead of an owned `Vec<TChar>` avoids
/// cloning the remaining portion of the input slice on every row overflow.
/// The caller (`Buffer::insert_text`) keeps a cursor into the original `text`
/// slice and only advances it when a `Leftover` is returned.
pub enum InsertResponse {
    /// All supplied text was inserted.  The value is the final cursor column.
    Consumed(usize),
    /// The row filled before all text was consumed.
    ///
    /// `leftover_start` is the index into the original `text` slice at which
    /// the un-inserted portion begins.  The caller should pass
    /// `&text[leftover_start..]` to the next row.
    ///
    /// `final_col` is the cursor column after the last character that was
    /// successfully written on this row.
    Leftover {
        leftover_start: usize,
        final_col: usize,
    },
}
