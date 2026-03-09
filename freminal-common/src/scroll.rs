// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

#[allow(clippy::module_name_repetitions)]
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum ScrollDirection {
    Up(usize),
    Down(usize),
}

impl Default for ScrollDirection {
    fn default() -> Self {
        Self::Up(1)
    }
}
