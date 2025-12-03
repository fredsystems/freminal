// Copyright (C) 2024-2025 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use thiserror::Error;

#[derive(Debug, Error, Eq, PartialEq, Clone)]
#[error(transparent)]
pub enum TCharError {
    #[error("Invalid TChar: {0:?}")]
    InvalidTChar(Vec<u8>),
}
