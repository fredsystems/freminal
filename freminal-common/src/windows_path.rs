// Copyright (C) 2026 Fred Clausen
//
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Classification of a path *string* by Windows path syntax.
//!
//! The classifier is pure string inspection and therefore platform
//! independent: it behaves identically on every host, so its table of cases
//! is exercised by Linux CI even though the callers that act on it are
//! Windows-only.
//!
//! Its purpose is to refuse paths that would make Windows contact a remote
//! host before any file is opened. On Windows, resolving a UNC path
//! (`\\server\share\x`) -- even just canonicalising it -- opens an SMB
//! connection and sends an `NTLMv2` challenge-response to `server`, which an
//! attacker who controls the path (for example through terminal output) can
//! harvest (GHSA-3g79-vfp3-q57g). Callers must classify the string and
//! reject everything that is not [`WindowsPathKind::LocalDrive`] **before**
//! making any filesystem call.
//!
//! # Residual risk
//!
//! A drive letter can be mapped to a network share (`net use Z: \\host\s`),
//! in which case `Z:\x` still contacts the host. That cannot be detected
//! from the string, so [`WindowsPathKind::LocalDrive`] means "drive-letter
//! absolute", not "provably on a local disk".

/// The syntactic form of a path string, as Windows would interpret it.
///
/// Both `\` and `/` are treated as separators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowsPathKind {
    /// A drive-letter absolute path: an ASCII letter, `:`, then a separator
    /// (`C:\x`, `c:/x`).
    LocalDrive,
    /// A UNC path: two leading separators followed by a server name
    /// (`\\server\share`, `//server/share`). Resolving it contacts the
    /// server.
    Unc,
    /// A verbatim or device path: two leading separators, then `?` or `.`,
    /// then a separator (`\\?\C:\x`, `\\?\UNC\h\s`, `\\.\pipe\x`). These
    /// bypass normalisation and can name remote hosts or devices.
    DeviceOrVerbatim,
    /// A drive-relative path: a drive letter and `:` not followed by a
    /// separator (`C:`, `C:foo`). Relative to that drive's current
    /// directory, so not absolute.
    DriveRelative,
    /// Anything else, including plain relative paths and a single leading
    /// separator (`\foo`, which is the root of the *current* drive and is
    /// not drive-letter absolute).
    Relative,
}

impl WindowsPathKind {
    /// Whether this is a drive-letter absolute path, the only form callers
    /// are allowed to pass on to the filesystem.
    #[must_use]
    pub const fn is_local_absolute(self) -> bool {
        matches!(self, Self::LocalDrive)
    }
}

/// Whether `b` is a Windows path separator.
const fn is_sep(b: u8) -> bool {
    b == b'\\' || b == b'/'
}

/// Classify `path` by Windows path syntax. See the module docs for why.
///
/// Pure string inspection: no filesystem access, identical on every
/// platform.
#[must_use]
pub fn classify_windows_path(path: &str) -> WindowsPathKind {
    let b = path.as_bytes();

    if b.len() >= 2 && is_sep(b[0]) && is_sep(b[1]) {
        let device_prefix = b.len() >= 4 && matches!(b[2], b'?' | b'.') && is_sep(b[3]);
        return if device_prefix {
            WindowsPathKind::DeviceOrVerbatim
        } else {
            WindowsPathKind::Unc
        };
    }

    if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
        return if b.get(2).copied().is_some_and(is_sep) {
            WindowsPathKind::LocalDrive
        } else {
            WindowsPathKind::DriveRelative
        };
    }

    WindowsPathKind::Relative
}

#[cfg(test)]
mod tests {
    use super::WindowsPathKind::{DeviceOrVerbatim, DriveRelative, LocalDrive, Relative, Unc};
    use super::{WindowsPathKind, classify_windows_path};

    #[test]
    fn classification_table() {
        let cases: &[(&str, WindowsPathKind)] = &[
            // Local drive, both separator styles and letter cases.
            (r"C:\x", LocalDrive),
            (r"c:\x", LocalDrive),
            ("C:/x", LocalDrive),
            ("z:/x/y", LocalDrive),
            (r"C:\", LocalDrive),
            ("C:/", LocalDrive),
            // UNC, every separator mix.
            (r"\\host\share\x.png", Unc),
            ("//h/s", Unc),
            (r"\/h\s", Unc),
            (r"/\h/s", Unc),
            (r"\\host", Unc),
            (r"\\", Unc),
            ("//", Unc),
            // `?`/`.` without a following separator is just a server name.
            (r"\\?", Unc),
            (r"\\.", Unc),
            (r"\\?x\y", Unc),
            // Verbatim and device.
            (r"\\?\C:\x", DeviceOrVerbatim),
            (r"\\?\UNC\h\s", DeviceOrVerbatim),
            (r"\\.\pipe\x", DeviceOrVerbatim),
            (r"\\.\COM1", DeviceOrVerbatim),
            ("//?/C:/x", DeviceOrVerbatim),
            (r"\/?/UNC/h/s", DeviceOrVerbatim),
            ("//./pipe/x", DeviceOrVerbatim),
            // Drive-relative.
            ("C:", DriveRelative),
            ("c:", DriveRelative),
            ("C:foo", DriveRelative),
            (r"C:foo\bar", DriveRelative),
            ("C:.", DriveRelative),
            // Relative / root-of-current-drive / degenerate.
            (r"\x", Relative),
            ("/x", Relative),
            ("x", Relative),
            (r"dir\x", Relative),
            ("./x", Relative),
            ("", Relative),
            ("C", Relative),
            ("1:\\x", Relative),
            // A non-ASCII "drive letter" is not a drive.
            ("é:\\", Relative),
            ("é:", Relative),
            // A UNC-looking tail after a drive is still a local-drive path.
            (r"C:\\server\share", LocalDrive),
        ];
        for &(path, expected) in cases {
            assert_eq!(classify_windows_path(path), expected, "path {path:?}");
        }
    }

    #[test]
    fn only_local_drive_is_local_absolute() {
        assert!(LocalDrive.is_local_absolute());
        for kind in [Unc, DeviceOrVerbatim, DriveRelative, Relative] {
            assert!(!kind.is_local_absolute(), "{kind:?}");
        }
    }
}
