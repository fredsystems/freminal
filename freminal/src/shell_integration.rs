// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Sync of Freminal's bundled shell-integration scripts to the on-disk
//! resources directory.
//!
//! On every launch (when `config.shell_integration.set_term_program ==
//! true`), Freminal synchronises the bundled
//! `shell-integration/{bash,zsh,fish}/...` tree to the platform-specific
//! shell-integration directory.  Files whose on-disk bytes already match
//! the embedded copy are left untouched (no rewrite, no mtime bump).
//! Files that differ are overwritten.
//!
//! The scripts are NOT meant to be sourced by users — they are loaded
//! automatically when Freminal spawns a child shell via shell-specific
//! injection (bash: `--posix` + `ENV`; zsh: `ZDOTDIR`; fish:
//! `XDG_DATA_DIRS`).  See
//! `freminal-terminal-emulator/src/io/pty.rs::run_terminal` and
//! `Documents/DESIGN_DECISIONS.md` for the rationale.
//!
//! All scripts are embedded at compile time via [`include_str!`] so the
//! binary is self-contained.

use std::path::Path;

/// Version of the shell-integration script set.  Every shipped script
/// begins with a version marker comment that must match this value; the
/// [`every_shipped_script_marker_version_matches_constant`] test enforces
/// the invariant.
///
/// Bump this when making incompatible changes to the script protocol
/// (e.g. payload format, marker semantics) so downstream tooling can
/// detect mismatched on-disk copies.
#[cfg(test)]
const FREMINAL_SHELL_INTEGRATION_VERSION: u32 = 4;

/// The bundled bash init script (loaded via `ENV=`).
pub const FREMINAL_BASH_INIT: &str =
    include_str!("../../shell-integration/bash/freminal-init.bash");
/// The bundled zsh `.zshenv` (loaded via `ZDOTDIR=`).
pub const FREMINAL_ZSH_ZSHENV: &str = include_str!("../../shell-integration/zsh/.zshenv");
/// The bundled zsh integration body (sourced by our `.zshenv`).
pub const FREMINAL_ZSH_INTEGRATION: &str =
    include_str!("../../shell-integration/zsh/freminal-integration");
/// The bundled fish vendor-confd integration (autoloaded via `XDG_DATA_DIRS`).
pub const FREMINAL_FISH_VENDOR_CONF: &str =
    include_str!("../../shell-integration/fish/vendor_conf.d/freminal.fish");
/// The bundled README.
pub const FREMINAL_README: &str = include_str!("../../shell-integration/README.md");

/// Relative-path → content table.  Used by [`sync_to_disk`] so the file
/// set is defined in one place.  Paths use forward slashes; they are
/// translated to platform-native separators by `Path::join`.
const SCRIPTS: &[(&str, &str)] = &[
    ("bash/freminal-init.bash", FREMINAL_BASH_INIT),
    ("zsh/.zshenv", FREMINAL_ZSH_ZSHENV),
    ("zsh/freminal-integration", FREMINAL_ZSH_INTEGRATION),
    (
        "fish/vendor_conf.d/freminal.fish",
        FREMINAL_FISH_VENDOR_CONF,
    ),
    ("README.md", FREMINAL_README),
];

/// Result of [`sync_to_disk`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InstallResult {
    /// Files that were written this call (either created or rewritten
    /// because the on-disk bytes differed from the embedded copy).
    pub written: Vec<String>,
    /// Files that already existed with the exact embedded content and
    /// were not rewritten.
    pub skipped: Vec<String>,
    /// File names whose write failed, paired with the IO error message.
    pub errors: Vec<(String, String)>,
}

impl InstallResult {
    /// `true` if at least one error occurred.
    #[must_use]
    // Vec::is_empty() is not stable as const fn; clippy's suggestion is a false positive.
    #[allow(clippy::missing_const_for_fn)]
    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }
}

/// Synchronise the bundled scripts to `dir`.
///
/// Files whose on-disk bytes already match the embedded copy are left
/// untouched; files that differ or do not yet exist are written.  Creates
/// `dir` and any required subdirectories (`bash/`, `zsh/`,
/// `fish/vendor_conf.d/`) as needed.
///
/// Called on every launch from `main.rs` (gated on
/// `config.shell_integration.set_term_program`).  User edits to these
/// files are intentionally NOT preserved — the scripts are part of the
/// freminal install, not user configuration.
#[must_use]
pub fn sync_to_disk(dir: &Path) -> InstallResult {
    let mut result = InstallResult::default();

    if let Err(e) = std::fs::create_dir_all(dir) {
        // If we can't even create the root directory, all scripts fail.
        result
            .errors
            .push((dir.display().to_string(), e.to_string()));
        return result;
    }

    for (relative_path, content) in SCRIPTS {
        let path = dir.join(relative_path);
        // Ensure the parent directory exists (e.g. `bash/`,
        // `fish/vendor_conf.d/`).
        if let Some(parent) = path.parent()
            && let Err(e) = std::fs::create_dir_all(parent)
        {
            result
                .errors
                .push(((*relative_path).to_owned(), e.to_string()));
            continue;
        }

        // Fast path: bytes match → no write, no mtime bump.
        if let Ok(existing) = std::fs::read(&path)
            && existing == content.as_bytes()
        {
            result.skipped.push((*relative_path).to_owned());
            continue;
        }

        match std::fs::write(&path, content) {
            Ok(()) => result.written.push((*relative_path).to_owned()),
            Err(e) => result
                .errors
                .push(((*relative_path).to_owned(), e.to_string())),
        }
    }

    result
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn sync_to_disk_writes_all_when_dir_empty() {
        let tmp = TempDir::new().expect("create tempdir");
        let result = sync_to_disk(tmp.path());
        assert_eq!(result.written.len(), SCRIPTS.len());
        assert!(result.skipped.is_empty());
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
        for (relative_path, _) in SCRIPTS {
            assert!(
                tmp.path().join(relative_path).exists(),
                "missing: {relative_path}"
            );
        }
    }

    #[test]
    fn sync_to_disk_handles_nested_dirs() {
        // Fresh tempdir — the subdirectories `bash/`, `zsh/`, and
        // `fish/vendor_conf.d/` must be created automatically.
        let tmp = TempDir::new().expect("create tempdir");
        let result = sync_to_disk(tmp.path());
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
        assert!(tmp.path().join("bash").is_dir());
        assert!(tmp.path().join("zsh").is_dir());
        assert!(tmp.path().join("fish").join("vendor_conf.d").is_dir());
    }

    #[test]
    fn sync_to_disk_skips_when_bytes_match() {
        let tmp = TempDir::new().expect("create tempdir");
        let first = sync_to_disk(tmp.path());
        assert_eq!(first.written.len(), SCRIPTS.len());

        let second = sync_to_disk(tmp.path());
        assert!(second.written.is_empty(), "written: {:?}", second.written);
        assert_eq!(second.skipped.len(), SCRIPTS.len());
        assert!(second.errors.is_empty());
    }

    #[test]
    fn sync_to_disk_writes_when_bytes_differ() {
        // Counterpart to `_skips_when_bytes_match`: pre-populate a file
        // with content that differs from the embedded copy and verify it
        // gets rewritten.
        let tmp = TempDir::new().expect("create tempdir");
        std::fs::create_dir_all(tmp.path().join("bash")).expect("mkdir bash");
        let bash_path = tmp.path().join("bash").join("freminal-init.bash");
        std::fs::write(&bash_path, "# user-customised content").expect("write");

        let result = sync_to_disk(tmp.path());
        assert!(
            result
                .written
                .iter()
                .any(|p| p == "bash/freminal-init.bash"),
            "expected bash/freminal-init.bash to be rewritten; written: {:?}",
            result.written
        );
        let after = std::fs::read_to_string(&bash_path).expect("read");
        assert_eq!(after, FREMINAL_BASH_INIT);
    }

    #[test]
    fn install_result_has_errors_reflects_state() {
        let result = InstallResult {
            written: vec!["a".to_owned()],
            skipped: Vec::new(),
            errors: Vec::new(),
        };
        assert!(!result.has_errors());

        let result = InstallResult {
            written: Vec::new(),
            skipped: Vec::new(),
            errors: vec![("a".to_owned(), "io error".to_owned())],
        };
        assert!(result.has_errors());
    }

    /// Every shipped script's `v<N>` version marker must agree with the
    /// Rust-side [`FREMINAL_SHELL_INTEGRATION_VERSION`] constant.  This
    /// invariant lets downstream tooling rely on a single source of
    /// truth when reasoning about protocol versions.
    #[test]
    fn every_shipped_script_marker_version_matches_constant() {
        let expected = format!("v{FREMINAL_SHELL_INTEGRATION_VERSION}");
        let needle = format!("freminal-shell-integration {expected}");
        for (path, content) in SCRIPTS {
            assert!(
                content.contains(&needle),
                "script `{path}` is missing version marker `{needle}`. \
                 First 200 bytes: {}",
                &content[..content.len().min(200)]
            );
        }
    }
}
