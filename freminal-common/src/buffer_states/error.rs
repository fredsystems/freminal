// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use thiserror::Error;

use super::tchar::TCHAR_MAX_UTF8_LEN;

#[derive(Debug, Error, Eq, PartialEq, Clone)]
pub enum TCharError {
    #[error("Empty byte sequence cannot be a TChar")]
    EmptyTChar,
    #[error("Byte sequence of length {0} exceeds TChar maximum of {TCHAR_MAX_UTF8_LEN} bytes")]
    TooLong(usize),
}
