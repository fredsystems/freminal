// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use thiserror::Error;

#[derive(Debug, Error, Eq, PartialEq, Clone)]
#[error(transparent)]
pub(crate) enum TCharError {
    #[error("Empty byte sequence cannot be a TChar")]
    EmptyTChar,
    #[error("Byte sequence of length {0} exceeds TChar maximum of 16 bytes")]
    TooLong(usize),
}
