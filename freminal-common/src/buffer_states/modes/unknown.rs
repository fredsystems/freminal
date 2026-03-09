// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use std::fmt;

use crate::buffer_states::{mode::SetMode, modes::ReportMode};

#[derive(Debug, Eq, PartialEq, Clone)]
pub struct UnknownMode {
    pub params: String,
    pub mode: SetMode,
}

impl UnknownMode {
    #[must_use]
    pub fn new(params: &[u8], set_mode: SetMode) -> Self {
        let params_s = std::str::from_utf8(params).unwrap_or("Unknown");

        Self {
            params: params_s.to_string(),
            mode: set_mode,
        }
    }
}

impl ReportMode for UnknownMode {
    // FIXME: we may need to get specific about DEC vs ANSI here. For now....we'll just report DEC
    fn report(&self, _override_mode: Option<SetMode>) -> String {
        format!("\x1b[?{};0$y", self.params)
    }
}

impl fmt::Display for UnknownMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} Unknown Mode({})", self.mode, self.params)
    }
}
