// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use std::fmt;

/// An OSC 8 hyperlink URL, optionally identified by an opaque string ID.
///
/// The OSC 8 spec (`\e]8;params;uri\e\\`) allows a `id=...` key/value pair in
/// the parameter field. Freminal stores only the `id` value (if present) and the
/// URI string.  Adjacent cells that share the same `id` and `url` are treated as
/// the same hyperlink region.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Url {
    /// Optional opaque link identifier from the `id=` parameter.
    ///
    /// Per the spec, matching IDs across separate OSC 8 sequences marks the
    /// same logical hyperlink even if the URI differs.
    pub id: Option<String>,
    /// The URI string for this hyperlink.
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
