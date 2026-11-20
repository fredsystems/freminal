// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! The shell-integration feature list freminal hands to its bundled scripts.
//!
//! Like Ghostty's `GHOSTTY_SHELL_FEATURES`, [`SHELL_FEATURES_ENV`] holds a
//! comma-separated list of the optional integration features that are on.  The
//! bundled bash, zsh and fish scripts test it with a substring match, so a
//! feature is on exactly when its name is in the list.  The variable is absent
//! when no feature is on.
//!
//! Today the only feature is `sudo`: wrap `sudo` so it keeps `TERMINFO`
//! (`[shell_integration] sudo_preserve_terminfo`, on by default).

use portable_pty::CommandBuilder;

/// Name of the environment variable carrying the comma-separated feature list.
pub const SHELL_FEATURES_ENV: &str = "FREMINAL_SHELL_FEATURES";

/// Whether the shell-integration `sudo` wrapper that preserves `TERMINFO` is
/// on for a pane.
///
/// Converted from the `[shell_integration] sudo_preserve_terminfo` TOML toggle
/// at the GUI boundary, so no `bool` crosses into the spawn path.
///
/// There is deliberately no `Default` impl: the configured default is
/// [`SudoTerminfo::Preserve`], and a spawn site must say which it means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SudoTerminfo {
    /// Wrap `sudo` as `sudo --preserve-env=TERMINFO ...` (except `sudo -e`).
    Preserve,
    /// Leave `sudo` alone.
    Unchanged,
}

/// The value for [`SHELL_FEATURES_ENV`], or `None` when no feature is on.
#[must_use]
pub fn shell_features_value(sudo: SudoTerminfo) -> Option<String> {
    let mut features: Vec<&str> = Vec::new();
    if sudo == SudoTerminfo::Preserve {
        features.push("sudo");
    }
    if features.is_empty() {
        None
    } else {
        Some(features.join(","))
    }
}

/// Set [`SHELL_FEATURES_ENV`] on `cmd` when a feature is on, and remove any
/// inherited value otherwise.
///
/// The removal matters when freminal is started from a shell that is itself
/// inside a freminal pane: without it a stale list would turn a feature on
/// that this pane's configuration has off.
pub fn apply_shell_features_env(cmd: &mut CommandBuilder, sudo: SudoTerminfo) {
    match shell_features_value(sudo) {
        Some(value) => cmd.env(SHELL_FEATURES_ENV, value),
        None => cmd.env_remove(SHELL_FEATURES_ENV),
    }
}

#[cfg(test)]
mod tests {
    use super::{SHELL_FEATURES_ENV, SudoTerminfo, apply_shell_features_env, shell_features_value};
    use portable_pty::CommandBuilder;
    use std::ffi::OsStr;

    #[test]
    fn feature_value_lists_sudo_only_when_preserving() {
        assert_eq!(
            shell_features_value(SudoTerminfo::Preserve).as_deref(),
            Some("sudo")
        );
        assert_eq!(shell_features_value(SudoTerminfo::Unchanged), None);
    }

    #[test]
    fn env_is_set_iff_preserving() {
        let mut on = CommandBuilder::new_default_prog();
        apply_shell_features_env(&mut on, SudoTerminfo::Preserve);
        assert_eq!(on.get_env(SHELL_FEATURES_ENV), Some(OsStr::new("sudo")));

        let mut off = CommandBuilder::new_default_prog();
        apply_shell_features_env(&mut off, SudoTerminfo::Unchanged);
        assert_eq!(off.get_env(SHELL_FEATURES_ENV), None);
    }

    #[test]
    fn default_removes_an_inherited_feature_list() {
        let mut cmd = CommandBuilder::new_default_prog();
        cmd.env(SHELL_FEATURES_ENV, "sudo");
        apply_shell_features_env(&mut cmd, SudoTerminfo::Unchanged);
        assert_eq!(
            cmd.get_env(SHELL_FEATURES_ENV),
            None,
            "an inherited feature list must not survive when the feature is off"
        );
    }
}
