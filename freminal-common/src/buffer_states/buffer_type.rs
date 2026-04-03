// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

/// Which of the two terminal buffers (primary or alternate) is currently active.
///
/// Terminals maintain a primary screen and an alternate screen. Programs like
/// `vim` and `htop` switch to the alternate screen on entry and restore the
/// primary screen on exit, leaving the user's scrollback history undisturbed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BufferType {
    /// The normal primary screen (with scrollback history).
    #[default]
    Primary,
    /// The alternate screen used by full-screen applications.
    Alternate,
}
