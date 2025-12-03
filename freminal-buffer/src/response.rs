// Copyright (C) 2024-2025 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use std::io::Cursor;

use freminal_common::buffer_states::{cursor::CursorPos, tchar::TChar};

pub enum InsertResponse {
    Consumed(usize), // final column
    Leftover { data: Vec<TChar>, final_col: usize },
}
