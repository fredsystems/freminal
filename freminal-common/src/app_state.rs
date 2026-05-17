// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Persisted application state for Freminal.
//!
//! This module holds *mutable runtime state* that the program writes on the
//! user's behalf — things the user does not author and that should never
//! round-trip through `config.toml` or the Settings Modal.  Today this is
//! just the first-run onboarding bit; in future it will also be the natural
//! home for things like "dismissed update prompts", "dismissed tips", and
//! similar per-install flags.
//!
//! ## Why a separate file?
//!
//! `config.toml` is *declarative user intent*.  On managed installs (NixOS
//! home-manager, GNU Stow with locked permissions, /etc/freminal/config.toml,
//! enterprise rollouts) the file may be a read-only symlink into a derivation
//! or a system path.  Trying to mutate it from inside the program fails with
//! `EROFS`/`EACCES` and pesters the user with error toasts on every launch.
//!
//! `AppState` lives in a separate, always-user-writable location:
//!
//! | Platform  | Path                                                        |
//! |-----------|-------------------------------------------------------------|
//! | Linux/BSD | `$XDG_STATE_HOME/freminal/state.toml`                       |
//! |           | (typically `~/.local/state/freminal/state.toml`)            |
//! | macOS     | `~/Library/Application Support/Freminal/state.toml`         |
//! | Windows   | `%APPDATA%\Freminal\state.toml`                             |
//!
//! On Linux this deliberately uses `$XDG_STATE_HOME`, *not* the config dir,
//! so home-manager users whose entire `~/.config/freminal/` directory is a
//! read-only Nix symlink still get a writable state location.
//!
//! ## Format
//!
//! TOML.  All fields are optional and new fields can be added without
//! breaking older state files.  A missing file, malformed TOML, or missing
//! field is not an error — the caller falls back to defaults.

use std::path::{Path, PathBuf};

use directories::BaseDirs;
use serde::{Deserialize, Serialize};

/// Persisted application state.
///
/// All fields are optional.  Adding new fields is forward- and
/// backward-compatible: older binaries silently ignore unknown fields and
/// newer binaries fill in defaults for fields missing from older state
/// files.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppState {
    /// `true` once the user has seen and dismissed the first-run welcome
    /// overlay.  Defaults to `false` so a fresh install triggers the
    /// overlay on first launch.
    ///
    /// Previously this lived at `config.onboarding.first_run_complete` in
    /// `config.toml`.  It was moved here so read-only/managed configs (NixOS
    /// home-manager, Stow with locked permissions, system-wide installs)
    /// can still record the dismissal without trying to mutate a read-only
    /// file.
    pub first_run_complete: bool,
}

impl AppState {
    /// Load the persisted state from `path`.
    ///
    /// Returns [`AppState::default`] if the file does not exist, cannot
    /// be read, or contains malformed TOML.  An unreadable/invalid state
    /// file is never fatal — the caller falls back to defaults.
    #[must_use]
    pub fn load_or_default(path: &Path) -> Self {
        let Ok(content) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        toml::from_str(&content).unwrap_or_else(|e| {
            tracing::warn!(
                "app_state: failed to parse {}: {e}; using defaults",
                path.display()
            );
            Self::default()
        })
    }

    /// Serialize to a pretty-printed TOML string.
    ///
    /// # Errors
    ///
    /// Returns a TOML serialization error if encoding fails.  This should
    /// be unreachable for a well-formed [`AppState`] — all fields are
    /// plain primitives.
    pub fn to_toml_string(&self) -> Result<String, toml::ser::Error> {
        toml::to_string_pretty(self)
    }

    /// Atomically persist the state to `path`.
    ///
    /// The parent directory is created if it does not exist.  Writes are
    /// performed via a temporary file plus rename so a crash mid-write
    /// cannot leave a truncated file.
    ///
    /// # Errors
    ///
    /// Returns an `io::Error` if the parent directory cannot be created,
    /// the temp file cannot be written, or the rename fails.  Returns a
    /// serialization error wrapped in `io::Error` if TOML encoding fails.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let toml_str = self.to_toml_string().map_err(std::io::Error::other)?;

        // Atomic write: temp file + rename.  Keeps the file readable even
        // if the process is killed mid-write.
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, toml_str)?;
        replace_file(&tmp, path)?;
        Ok(())
    }
}

/// Rename `src` onto `dst`, replacing `dst` if it already exists.
///
/// On Unix, [`std::fs::rename`] already replaces the destination
/// atomically.  On Windows, [`std::fs::rename`] fails with
/// `ERROR_ALREADY_EXISTS` when the destination exists, so we remove
/// it first.  The window between `remove_file` and `rename` is tolerable:
/// if the process is killed in that window the next save simply writes a
/// fresh file, and readers fall back to defaults on any missing/malformed
/// file.
fn replace_file(src: &Path, dst: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        match std::fs::remove_file(dst) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    std::fs::rename(src, dst)
}

/// Returns the platform-canonical path to `state.toml`.
///
/// | Platform  | Path                                                        |
/// |-----------|-------------------------------------------------------------|
/// | Linux/BSD | `$XDG_STATE_HOME/freminal/state.toml`                       |
/// | macOS     | `~/Library/Application Support/Freminal/state.toml`         |
/// | Windows   | `%APPDATA%\Freminal\state.toml`                             |
///
/// On Linux/BSD, `$XDG_STATE_HOME` is used deliberately (not the config
/// dir) so home-manager users whose `~/.config/freminal/` is a read-only
/// Nix symlink still get a writable location.
///
/// Returns `None` if the base directories cannot be determined (e.g. no
/// home directory).
#[allow(unreachable_code)]
#[must_use]
pub fn app_state_path() -> Option<PathBuf> {
    let base = BaseDirs::new()?;

    #[cfg(target_os = "macos")]
    {
        return Some(base.data_dir().join("Freminal").join("state.toml"));
    }

    #[cfg(target_os = "windows")]
    {
        return Some(base.data_dir().join("Freminal").join("state.toml"));
    }

    #[cfg(any(
        target_os = "linux",
        target_os = "freebsd",
        target_os = "dragonfly",
        target_os = "openbsd",
        target_os = "netbsd"
    ))]
    {
        // `state_dir()` returns `$XDG_STATE_HOME` (typically
        // `~/.local/state`).  This is the correct XDG location for
        // mutable program state and — crucially for NixOS home-manager
        // users — is not part of the read-only `~/.config/freminal/`
        // symlink tree.
        return Some(base.state_dir()?.join("freminal").join("state.toml"));
    }

    None
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn default_state_has_first_run_incomplete() {
        let s = AppState::default();
        assert!(!s.first_run_complete);
    }

    #[test]
    fn default_state_serializes_to_explicit_field() {
        // first_run_complete is a plain bool (not Option), so it is
        // serialized explicitly even at default.  This is intentional —
        // it documents the on-disk schema for users inspecting the file.
        let s = AppState::default();
        let toml_str = s.to_toml_string().expect("serialize");
        assert!(toml_str.contains("first_run_complete"));
    }

    #[test]
    fn roundtrip_preserves_first_run_complete() {
        let s = AppState {
            first_run_complete: true,
        };
        let toml_str = s.to_toml_string().expect("serialize");
        let parsed: AppState = toml::from_str(&toml_str).expect("parse");
        assert_eq!(parsed, s);
    }

    #[test]
    fn parses_empty_file_as_default() {
        let parsed: AppState = toml::from_str("").expect("parse empty");
        assert_eq!(parsed, AppState::default());
    }

    #[test]
    fn parses_file_with_unknown_fields() {
        // Forward compatibility: an older binary reading a newer state
        // file with extra fields must not error.
        let parsed: AppState =
            toml::from_str("first_run_complete = true\nfuture_field = 42\n").expect("parse");
        assert!(parsed.first_run_complete);
    }

    #[test]
    fn malformed_toml_falls_back_to_default() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("state.toml");
        std::fs::write(&path, "this is not valid toml = = =").expect("write");
        let loaded = AppState::load_or_default(&path);
        assert_eq!(loaded, AppState::default());
    }

    #[test]
    fn missing_file_returns_default() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("does_not_exist.toml");
        let loaded = AppState::load_or_default(&path);
        assert_eq!(loaded, AppState::default());
    }

    #[test]
    fn save_then_load_roundtrip() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("subdir").join("state.toml");
        let s = AppState {
            first_run_complete: true,
        };
        s.save(&path).expect("save");
        let loaded = AppState::load_or_default(&path);
        assert_eq!(loaded, s);
    }

    #[test]
    fn save_creates_missing_parent_directories() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("a").join("b").join("c").join("state.toml");
        let s = AppState {
            first_run_complete: true,
        };
        s.save(&path).expect("save");
        assert!(path.exists());
    }

    #[test]
    fn save_overwrites_existing_file_when_called_twice() {
        // Regression: on Windows `std::fs::rename` fails if the
        // destination exists, so the second save would error unless we
        // explicitly replace the destination.  Exercise the
        // replace-file path by saving twice.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("state.toml");

        let first = AppState {
            first_run_complete: false,
        };
        first.save(&path).expect("first save");

        let second = AppState {
            first_run_complete: true,
        };
        second.save(&path).expect("second save must overwrite");

        let loaded = AppState::load_or_default(&path);
        assert_eq!(loaded, second);
    }

    #[test]
    fn load_or_default_on_unreadable_file_returns_default() {
        // A file we cannot read (e.g. wrong permissions, or in our case
        // a directory standing in where a file is expected) must not
        // panic and must return defaults.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().to_path_buf(); // a directory, not a file
        let loaded = AppState::load_or_default(&path);
        assert_eq!(loaded, AppState::default());
    }
}
