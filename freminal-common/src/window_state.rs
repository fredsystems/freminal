// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Persisted ephemeral window geometry for Freminal UI windows.
//!
//! This module tracks the last-known size and position of Freminal's UI
//! windows — both the Settings window and each main terminal window —
//! across sessions.  It is deliberately separate from both:
//!
//! - `config.toml` — user-authored preferences that are intentionally edited
//! - saved layouts — user-authored or auto-saved descriptions of terminal
//!   workspaces (tabs, panes, CWDs, commands)
//!
//! The rationale for a separate file is that window geometry is purely
//! ephemeral UI state: it is written automatically, never edited by the
//! user, and should not round-trip through the Settings Modal.
//!
//! The file is stored at `~/.config/freminal/window_state.toml` (Linux/BSD),
//! `~/Library/Application Support/Freminal/window_state.toml` (macOS), or
//! `%APPDATA%\Freminal\window_state.toml` (Windows).
//!
//! All fields are optional.  A missing file, malformed TOML, or missing
//! field is not an error — the caller simply falls back to defaults.

use std::path::{Path, PathBuf};

use directories::BaseDirs;
use serde::{Deserialize, Serialize};

/// Rectangular window geometry.
///
/// `size` is the inner (client-area) size in *logical* pixels (DPI-independent
/// units).  `position` is the outer (frame) position in logical pixels in the
/// display's coordinate space.  Either may be absent.  On platforms like
/// Wayland, `position` cannot be reliably reported by the compositor and will
/// typically be `None`.
///
/// Logical pixels are used (rather than physical pixels) because both the
/// winit `LogicalSize`/`LogicalPosition` APIs and Freminal's
/// `freminal_windowing::WindowConfig` consume geometry in logical units, so
/// storing logical values avoids a round-trip conversion on save and load.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowGeometry {
    /// Inner size in logical pixels: `[width, height]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<[u32; 2]>,

    /// Outer position in logical pixels: `[x, y]`.
    ///
    /// `None` on platforms where the compositor does not expose window
    /// position (e.g. Wayland).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<[i32; 2]>,
}

impl WindowGeometry {
    /// Construct a geometry with the given size and position.  Either may
    /// be `None`.
    #[must_use]
    pub const fn new(size: Option<[u32; 2]>, position: Option<[i32; 2]>) -> Self {
        Self { size, position }
    }

    /// Returns `true` if both size and position are `None`.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.size.is_none() && self.position.is_none()
    }
}

/// Persisted ephemeral state for Freminal UI windows.
///
/// All fields are optional and new fields can be added without breaking
/// older state files.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowState {
    /// Last known geometry of the Settings window.
    #[serde(skip_serializing_if = "WindowGeometry::is_empty")]
    pub settings: WindowGeometry,

    /// Last known geometry of each main terminal window, one entry per
    /// window that was open when the state was last persisted.
    ///
    /// On startup the first entry is used to seed the primary window's
    /// creation `WindowConfig`.  Additional entries (for multi-window
    /// sessions) are applied to subsequently-spawned windows.
    ///
    /// Applied at window creation time rather than via a post-creation
    /// viewport command so the compositor sees the requested size in the
    /// initial surface configure on Wayland.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub main_windows: Vec<WindowGeometry>,
}

impl WindowState {
    /// Load the persisted window state from `path`.
    ///
    /// Returns [`WindowState::default`] if the file does not exist, cannot
    /// be read, or contains malformed TOML.  An unreadable/invalid state
    /// file is never fatal — the caller falls back to defaults.
    #[must_use]
    pub fn load_or_default(path: &Path) -> Self {
        let Ok(content) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        toml::from_str(&content).unwrap_or_else(|e| {
            tracing::warn!(
                "window_state: failed to parse {}: {e}; using defaults",
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
    /// be unreachable for a well-formed `WindowState` — all fields are
    /// plain integers wrapped in `Option`.
    pub fn to_toml_string(&self) -> Result<String, toml::ser::Error> {
        toml::to_string_pretty(self)
    }

    /// Atomically persist the state to `path`.
    ///
    /// The parent directory is created if it does not exist.  Writes are
    /// performed via a temporary file rename so a crash mid-write cannot
    /// leave a truncated file.
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
/// On Unix, [`std::fs::rename`] already replaces the destination atomically.
/// On Windows, [`std::fs::rename`] fails with `ERROR_ALREADY_EXISTS` when the
/// destination exists, so we must remove the destination first.  The window
/// between `remove_file` and `rename` is tolerable here: if the process is
/// killed in that window the next save simply writes a fresh file, and
/// readers fall back to defaults on any missing/malformed file.
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

/// Returns the platform-canonical path to `window_state.toml`.
///
/// | Platform  | Path                                                     |
/// |-----------|----------------------------------------------------------|
/// | Linux/BSD | `$XDG_CONFIG_HOME/freminal/window_state.toml`            |
/// | macOS     | `~/Library/Application Support/Freminal/window_state.toml` |
/// | Windows   | `%APPDATA%\Freminal\window_state.toml`                   |
///
/// Returns `None` if the base directories cannot be determined.
#[must_use]
pub fn window_state_path() -> Option<PathBuf> {
    let base = BaseDirs::new()?;

    #[cfg(target_os = "macos")]
    {
        return Some(base.data_dir().join("Freminal").join("window_state.toml"));
    }

    #[cfg(target_os = "windows")]
    {
        return Some(base.data_dir().join("Freminal").join("window_state.toml"));
    }

    #[cfg(any(
        target_os = "linux",
        target_os = "freebsd",
        target_os = "dragonfly",
        target_os = "openbsd",
        target_os = "netbsd"
    ))]
    {
        return Some(base.config_dir().join("freminal").join("window_state.toml"));
    }

    #[allow(unreachable_code)]
    None
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn default_state_serializes_to_empty_table() {
        let s = WindowState::default();
        let toml_str = s.to_toml_string().expect("serialize");
        // All fields are skipped when empty, so the output is an empty document.
        assert!(toml_str.trim().is_empty() || toml_str.trim() == "");
    }

    #[test]
    fn roundtrip_preserves_geometry() {
        let s = WindowState {
            settings: WindowGeometry {
                size: Some([640, 480]),
                position: Some([100, 200]),
            },
            main_windows: Vec::new(),
        };
        let toml_str = s.to_toml_string().expect("serialize");
        let parsed: WindowState = toml::from_str(&toml_str).expect("parse");
        assert_eq!(parsed, s);
    }

    #[test]
    fn roundtrip_with_partial_geometry() {
        // Size only (position unavailable, e.g. Wayland).
        let s = WindowState {
            settings: WindowGeometry {
                size: Some([800, 600]),
                position: None,
            },
            main_windows: Vec::new(),
        };
        let toml_str = s.to_toml_string().expect("serialize");
        let parsed: WindowState = toml::from_str(&toml_str).expect("parse");
        assert_eq!(parsed, s);
    }

    #[test]
    fn roundtrip_preserves_main_windows() {
        let s = WindowState {
            settings: WindowGeometry::default(),
            main_windows: vec![
                WindowGeometry {
                    size: Some([1280, 800]),
                    position: Some([0, 0]),
                },
                WindowGeometry {
                    size: Some([1024, 768]),
                    position: None,
                },
            ],
        };
        let toml_str = s.to_toml_string().expect("serialize");
        let parsed: WindowState = toml::from_str(&toml_str).expect("parse");
        assert_eq!(parsed, s);
    }

    #[test]
    fn empty_main_windows_is_skipped() {
        let s = WindowState {
            settings: WindowGeometry {
                size: Some([400, 300]),
                position: None,
            },
            main_windows: Vec::new(),
        };
        let toml_str = s.to_toml_string().expect("serialize");
        // main_windows = [] should be omitted from the output.
        assert!(!toml_str.contains("main_windows"));
    }

    #[test]
    fn parses_file_missing_optional_fields() {
        // Minimal / empty file should parse to default.
        let parsed: WindowState = toml::from_str("").expect("parse empty");
        assert_eq!(parsed, WindowState::default());

        // File with only the table header and no fields.
        let parsed: WindowState = toml::from_str("[settings]\n").expect("parse header");
        assert_eq!(parsed, WindowState::default());
    }

    #[test]
    fn malformed_toml_falls_back_to_default() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("window_state.toml");
        std::fs::write(&path, "this is not valid toml = = =").expect("write");
        let loaded = WindowState::load_or_default(&path);
        assert_eq!(loaded, WindowState::default());
    }

    #[test]
    fn missing_file_returns_default() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("does_not_exist.toml");
        let loaded = WindowState::load_or_default(&path);
        assert_eq!(loaded, WindowState::default());
    }

    #[test]
    fn save_then_load_roundtrip() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("subdir").join("window_state.toml");
        let s = WindowState {
            settings: WindowGeometry {
                size: Some([1024, 768]),
                position: Some([50, 75]),
            },
            main_windows: vec![WindowGeometry {
                size: Some([1920, 1080]),
                position: Some([10, 20]),
            }],
        };
        s.save(&path).expect("save");
        let loaded = WindowState::load_or_default(&path);
        assert_eq!(loaded, s);
    }

    #[test]
    fn save_overwrites_existing_file_when_called_twice() {
        // Regression: on Windows `std::fs::rename` fails if the destination
        // exists, so the second save would error unless we explicitly
        // replace the destination.  Exercise the replace-file path by
        // saving twice to the same location.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("window_state.toml");

        let first = WindowState {
            settings: WindowGeometry {
                size: Some([800, 600]),
                position: None,
            },
            main_windows: Vec::new(),
        };
        first.save(&path).expect("first save");

        let second = WindowState {
            settings: WindowGeometry {
                size: Some([1280, 720]),
                position: Some([40, 60]),
            },
            main_windows: vec![WindowGeometry {
                size: Some([640, 480]),
                position: None,
            }],
        };
        second.save(&path).expect("second save must overwrite");

        let loaded = WindowState::load_or_default(&path);
        assert_eq!(loaded, second);
    }

    #[test]
    fn is_empty_reflects_state() {
        assert!(WindowGeometry::default().is_empty());
        assert!(!WindowGeometry::new(Some([1, 1]), None).is_empty());
        assert!(!WindowGeometry::new(None, Some([0, 0])).is_empty());
    }
}
