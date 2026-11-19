// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! The `TERM` value freminal advertises to the programs it spawns.
//!
//! [`TermPlatform`] selects the platform default and [`validate_term_name`] checks a
//! user-configured value (`[shell] term`).  Both are pure so every platform's behaviour is
//! testable on every host.  `ShellConfig::effective_term` (in [`crate::config`]) combines them
//! into the single resolved value.

use thiserror::Error;

/// The default `TERM`: freminal's own terminfo entry.
pub const DEFAULT_TERM: &str = "xterm-freminal";

/// The default `TERM` on Windows, where there is no terminfo distribution path and MSYS2/Cygwin
/// programs read their own databases, which do not carry [`DEFAULT_TERM`].
pub const WINDOWS_DEFAULT_TERM: &str = "xterm-256color";

/// The longest accepted `TERM` name in bytes. The terminfo format caps names at 128 bytes.
pub const MAX_TERM_NAME_LEN: usize = 128;

/// Which platform's default `TERM` applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TermPlatform {
    /// Windows (ConPTY): defaults to [`WINDOWS_DEFAULT_TERM`].
    Windows,
    /// Every other platform: defaults to [`DEFAULT_TERM`].
    Other,
}

impl TermPlatform {
    /// The platform this binary was compiled for.
    #[must_use]
    pub const fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else {
            Self::Other
        }
    }

    /// The `TERM` used on this platform when none is configured.
    #[must_use]
    pub const fn default_term(self) -> &'static str {
        match self {
            Self::Windows => WINDOWS_DEFAULT_TERM,
            Self::Other => DEFAULT_TERM,
        }
    }
}

/// Why a configured `TERM` value was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum TermNameError {
    /// The value is the empty string.
    #[error("must not be empty")]
    Empty,
    /// The value is longer than [`MAX_TERM_NAME_LEN`] bytes.
    #[error("must be at most {MAX_TERM_NAME_LEN} bytes")]
    TooLong,
    /// The value contains a control character.
    #[error("must not contain control characters")]
    ControlCharacter,
    /// The value contains whitespace.
    #[error("must not contain whitespace")]
    Whitespace,
    /// The value contains `/`.
    #[error("must not contain '/'")]
    Slash,
}

/// Check that `name` is usable as a `TERM` value.
///
/// A name must be non-empty, at most [`MAX_TERM_NAME_LEN`] bytes, and contain no control
/// characters, whitespace or `/`.  Whether a terminfo entry of that name exists is deliberately
/// not checked.
///
/// # Errors
/// Returns the first [`TermNameError`] the value violates.
pub fn validate_term_name(name: &str) -> Result<(), TermNameError> {
    if name.is_empty() {
        return Err(TermNameError::Empty);
    }
    if name.len() > MAX_TERM_NAME_LEN {
        return Err(TermNameError::TooLong);
    }
    for c in name.chars() {
        if c.is_control() {
            return Err(TermNameError::ControlCharacter);
        }
        if c.is_whitespace() {
            return Err(TermNameError::Whitespace);
        }
        if c == '/' {
            return Err(TermNameError::Slash);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_defaults() {
        assert_eq!(TermPlatform::Other.default_term(), "xterm-freminal");
        assert_eq!(TermPlatform::Windows.default_term(), "xterm-256color");
    }

    #[test]
    fn current_platform_matches_cfg() {
        let expected = if cfg!(windows) {
            TermPlatform::Windows
        } else {
            TermPlatform::Other
        };
        assert_eq!(TermPlatform::current(), expected);
    }

    #[test]
    fn accepts_ordinary_names() {
        for name in ["xterm-freminal", "xterm-256color", "screen.xterm+256", "a"] {
            assert_eq!(validate_term_name(name), Ok(()), "{name}");
        }
    }

    #[test]
    fn rejects_each_invalid_class() {
        assert_eq!(validate_term_name(""), Err(TermNameError::Empty));
        assert_eq!(validate_term_name("a b"), Err(TermNameError::Whitespace));
        assert_eq!(
            validate_term_name("a\u{a0}b"),
            Err(TermNameError::Whitespace)
        );
        assert_eq!(
            validate_term_name("a\tb"),
            Err(TermNameError::ControlCharacter)
        );
        assert_eq!(
            validate_term_name("a\nb"),
            Err(TermNameError::ControlCharacter)
        );
        assert_eq!(
            validate_term_name("a\u{1b}b"),
            Err(TermNameError::ControlCharacter)
        );
        assert_eq!(
            validate_term_name("a\u{7f}b"),
            Err(TermNameError::ControlCharacter)
        );
        assert_eq!(validate_term_name("x/y"), Err(TermNameError::Slash));
        assert_eq!(validate_term_name("../x"), Err(TermNameError::Slash));
    }

    #[test]
    fn length_limit_is_inclusive() {
        assert_eq!(validate_term_name(&"a".repeat(MAX_TERM_NAME_LEN)), Ok(()));
        assert_eq!(
            validate_term_name(&"a".repeat(MAX_TERM_NAME_LEN + 1)),
            Err(TermNameError::TooLong)
        );
    }
}
