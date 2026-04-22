// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use std::str::Utf8Error;
use thiserror::Error;

use super::tchar::TCHAR_MAX_UTF8_LEN;

#[derive(Debug, Error)]
pub enum TCharError {
    #[error("Empty byte sequence cannot be a TChar")]
    EmptyTChar,
    #[error("Byte sequence of length {0} exceeds TChar maximum of {TCHAR_MAX_UTF8_LEN} bytes")]
    TooLong(usize),
    #[error("byte slice was not valid UTF-8")]
    InvalidUtf8(#[from] Utf8Error),
}
