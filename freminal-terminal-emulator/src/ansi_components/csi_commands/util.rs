// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Shared helpers for CSI command handlers.

/// Extract a parameter value at `idx` from the parsed CSI parameter list,
/// or return `default` when the index is out of bounds or the parameter
/// was omitted (i.e. the slot is `None`).
///
/// This is the standard xterm/DEC convention: "missing" and "empty" CSI
/// parameters both collapse to the caller-supplied default.
#[inline]
pub(super) fn param_or(params: &[Option<usize>], idx: usize, default: usize) -> usize {
    params.get(idx).and_then(|opt| *opt).unwrap_or(default)
}
