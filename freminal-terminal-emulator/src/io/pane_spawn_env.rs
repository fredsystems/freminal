// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! How one pane's child process is spawned.

use std::collections::HashMap;
use std::path::Path;

use freminal_common::term_name::TermName;

use super::shell_features::SudoTerminfo;
use super::term_program_env::TermProgramEnv;

/// The per-pane values that shape the environment a pane's child is spawned in.
///
/// Everything here comes from the pane's own configuration (its layout leaf, the
/// `[shell]` and `[shell_integration]` sections) rather than from the command
/// line, which `TerminalEmulator::new` reads separately.  It is turned into the
/// spawn configuration handed to the PTY (`PtySpawnConfig`) once the command-line
/// shell and positional command have been folded in.
#[derive(Clone, Copy)]
pub struct PaneSpawnEnv<'a> {
    /// Working directory for the child process.
    pub cwd: Option<&'a Path>,
    /// Extra environment variables to set on the child process.
    pub extra_env: Option<&'a HashMap<String, String>>,
    /// Shell executable override (a layout-level shell); takes precedence over
    /// `--shell` and is ignored when a positional command is given.
    pub shell_override: Option<&'a str>,
    /// Whether to set `TERM_PROGRAM` / `TERM_PROGRAM_VERSION` on the child and
    /// inject the shell-integration scripts.  Driven by
    /// `config.shell_integration.set_term_program`.
    pub term_program: TermProgramEnv,
    /// Whether the shell-integration `sudo` wrapper that preserves `TERMINFO` is
    /// on; only takes effect when shell integration is injected.
    pub sudo_terminfo: SudoTerminfo,
    /// The pane's resolved `TERM` (see [`TermName::resolve`]): set on the child
    /// and reported by XTGETTCAP `TN`, so the two cannot disagree.
    pub term: &'a TermName,
}
