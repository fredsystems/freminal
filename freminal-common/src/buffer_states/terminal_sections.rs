// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

/// A pair of terminal data slices: content that has scrolled off the top of
/// the visible window (`scrollback`) and the currently visible content
/// (`visible`).
///
/// This type is generic so it can carry both `Vec<TChar>` and
/// `Vec<FormatTag>` without duplication.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct TerminalSections<T> {
    pub scrollback: T,
    pub visible: T,
}
