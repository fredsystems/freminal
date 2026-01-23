// Copyright (C) 2024-2025 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Url {
    // Ostensibly, the ID is a key/value pair that is used to identify the URL
    // However, the current spec (https://iterm2.com/documentation-escape-codes.html) only
    // defines the ID as the only valid parameter
    pub id: Option<String>,
    pub url: String,
}

impl fmt::Display for Url {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Url {{ id: {}, url: {} }}",
            self.id.as_deref().unwrap_or("None"),
            self.url
        )
    }
}
