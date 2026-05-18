// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Auto-install of Freminal's shell-integration scripts.
//!
//! On first launch (when `config.shell_integration.auto_install == true`),
//! Freminal copies the bundled `freminal.{bash,zsh,fish}` scripts and the
//! companion `README.md` to the platform-specific shell-integration
//! directory.  Existing files are NOT overwritten so user customisations
//! are preserved.
//!
//! The "Re-install Scripts" button in the Settings modal calls
//! [`reinstall_scripts`] which DOES overwrite existing files —
//! semantically "reset the user's local copies to the freminal-shipped
//! versions".
//!
//! All scripts are embedded at compile time via [`include_str!`] so the
//! binary is self-contained.

use std::path::Path;

/// The bundled bash script.
pub const FREMINAL_BASH: &str = include_str!("../../shell-integration/freminal.bash");
/// The bundled zsh script.
pub const FREMINAL_ZSH: &str = include_str!("../../shell-integration/freminal.zsh");
/// The bundled fish script.
pub const FREMINAL_FISH: &str = include_str!("../../shell-integration/freminal.fish");
/// The bundled README.
pub const FREMINAL_README: &str = include_str!("../../shell-integration/README.md");

/// File-name → content table.  Used by [`install_if_missing`] and
/// [`reinstall_scripts`] so the file set is defined in one place.
const SCRIPTS: &[(&str, &str)] = &[
    ("freminal.bash", FREMINAL_BASH),
    ("freminal.zsh", FREMINAL_ZSH),
    ("freminal.fish", FREMINAL_FISH),
    ("README.md", FREMINAL_README),
];

/// Result of [`install_if_missing`] or [`reinstall_scripts`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallResult {
    /// Files that were written successfully.
    pub written: Vec<String>,
    /// Files that already existed and were not overwritten (only meaningful
    /// for `install_if_missing`).
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

/// Install the bundled scripts into `dir`, skipping any file that already
/// exists.  Creates `dir` if it does not exist.
///
/// Used by the startup auto-install path so existing user-edited scripts
/// are preserved across upgrades.
#[must_use]
pub fn install_if_missing(dir: &Path) -> InstallResult {
    install_with_policy(dir, /* overwrite = */ false)
}

/// Re-install all bundled scripts into `dir`, overwriting any existing
/// files.  Creates `dir` if it does not exist.
///
/// Used by the "Re-install Scripts" button in the Settings modal —
/// semantically "reset to ship defaults".
#[must_use]
pub fn reinstall_scripts(dir: &Path) -> InstallResult {
    install_with_policy(dir, /* overwrite = */ true)
}

fn install_with_policy(dir: &Path, overwrite: bool) -> InstallResult {
    let mut result = InstallResult {
        written: Vec::new(),
        skipped: Vec::new(),
        errors: Vec::new(),
    };

    if let Err(e) = std::fs::create_dir_all(dir) {
        // If we can't even create the directory, all scripts fail.  Record
        // a single combined error so the caller can surface one message.
        result
            .errors
            .push((dir.display().to_string(), e.to_string()));
        return result;
    }

    for (name, content) in SCRIPTS {
        let path = dir.join(name);
        if !overwrite && path.exists() {
            result.skipped.push((*name).to_owned());
            continue;
        }
        match std::fs::write(&path, content) {
            Ok(()) => result.written.push((*name).to_owned()),
            Err(e) => result.errors.push(((*name).to_owned(), e.to_string())),
        }
    }

    result
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Create a unique temporary directory for the duration of a test.
    /// The caller is responsible for calling `cleanup_tmp_dir` when done.
    fn make_tmp_dir(suffix: &str) -> PathBuf {
        let base = std::env::temp_dir();
        let dir = base.join(format!("freminal_shell_integ_test_{suffix}"));
        std::fs::create_dir_all(&dir).expect("create test temp dir");
        dir
    }

    fn cleanup_tmp_dir(dir: &PathBuf) {
        // Best-effort cleanup; ignore errors.
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn install_if_missing_writes_all_when_dir_empty() {
        let tmp = make_tmp_dir("writes_all");
        let result = install_if_missing(&tmp);
        assert_eq!(result.written.len(), SCRIPTS.len());
        assert!(result.skipped.is_empty());
        assert!(result.errors.is_empty());
        // Verify each file is present.
        for (name, _) in SCRIPTS {
            assert!(tmp.join(name).exists());
        }
        cleanup_tmp_dir(&tmp);
    }

    #[test]
    fn install_if_missing_skips_existing_files() {
        let tmp = make_tmp_dir("skips_existing");
        // Pre-populate one file with custom content.
        let bash_path = tmp.join("freminal.bash");
        std::fs::write(&bash_path, "# user-customised content").expect("write");
        let original = std::fs::read_to_string(&bash_path).expect("read");

        let result = install_if_missing(&tmp);
        assert_eq!(result.skipped, vec!["freminal.bash".to_owned()]);
        assert_eq!(result.written.len(), SCRIPTS.len() - 1);
        // User content must NOT be overwritten.
        let after = std::fs::read_to_string(&bash_path).expect("read");
        assert_eq!(after, original);
        cleanup_tmp_dir(&tmp);
    }

    #[test]
    fn reinstall_scripts_overwrites_existing_files() {
        let tmp = make_tmp_dir("overwrites");
        let bash_path = tmp.join("freminal.bash");
        std::fs::write(&bash_path, "# user-customised content").expect("write");

        let result = reinstall_scripts(&tmp);
        assert_eq!(result.written.len(), SCRIPTS.len());
        assert!(result.skipped.is_empty());
        assert!(result.errors.is_empty());
        // User content MUST be overwritten with the ship version.
        let after = std::fs::read_to_string(&bash_path).expect("read");
        assert_eq!(after, FREMINAL_BASH);
        cleanup_tmp_dir(&tmp);
    }

    #[test]
    fn install_if_missing_idempotent_on_second_call() {
        let tmp = make_tmp_dir("idempotent");
        let first = install_if_missing(&tmp);
        assert_eq!(first.written.len(), SCRIPTS.len());

        let second = install_if_missing(&tmp);
        assert_eq!(second.written.len(), 0);
        assert_eq!(second.skipped.len(), SCRIPTS.len());
        assert!(second.errors.is_empty());
        cleanup_tmp_dir(&tmp);
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
}
