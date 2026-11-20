// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Whether a pane's child is told it is running inside freminal.

/// Whether a pane's child is told it is running inside freminal.
///
/// One switch with two effects, both on the spawn path:
///
/// - the child gets `TERM_PROGRAM=freminal` and `TERM_PROGRAM_VERSION`;
/// - a bare bash, zsh or fish shell gets freminal's shell-integration scripts
///   injected (OSC 133 markers, the optional `sudo` wrapper).
///
/// Converted from the `[shell_integration] set_term_program` TOML toggle at the
/// GUI boundary, so no `bool` crosses into the spawn path.
///
/// There is deliberately no `Default` impl: the configured default is
/// [`TermProgramEnv::Advertise`], and a spawn site must say which it means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TermProgramEnv {
    /// Set `TERM_PROGRAM` / `TERM_PROGRAM_VERSION` and inject the shell
    /// integration into a recognised shell.
    Advertise,
    /// Set neither `TERM_PROGRAM` nor `TERM_PROGRAM_VERSION` and inject no shell
    /// integration (an inherited `TERM_PROGRAM` is not touched).
    ///
    /// This is not a pristine environment: the spawn path still sets `TERM`,
    /// `TERMINFO` and `FREMINAL_TERMINFO` (and the other variables every pane
    /// gets), and still removes an inherited `FREMINAL_SHELL_FEATURES`, so a
    /// parent pane's feature list never reaches an un-integrated child.
    Omit,
}
