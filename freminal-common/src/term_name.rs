// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! The `TERM` value freminal advertises to the programs it spawns.
//!
//! [`TermPlatform`] selects the platform default and [`validate_term_name`] checks a
//! user-configured value (`[shell] term`).  Both are pure so every platform's behaviour is
//! testable on every host.  `ShellConfig::effective_term` (in [`crate::config`]) combines them
//! into the configured value, and [`TermName::resolve`] adds the per-pane `env` override to give
//! the single value computed once per pane at spawn.

use std::collections::HashMap;
use std::fmt;

use thiserror::Error;

use crate::config::ShellConfig;

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

/// The environment variable that carries the terminal name.
const TERM_ENV_KEY: &str = "TERM";

/// The `TERM` a pane advertises: the one value used for the child's environment, XTGETTCAP `TN`
/// and the FREC `term` metadata.
///
/// Computed once per pane at spawn by [`TermName::resolve`].  A `TERM` in the pane's (or the
/// layout's) `env` map wins over `[shell] term`, which wins over the platform default.  The `env`
/// value is taken verbatim: it is an explicit instruction for that pane, and honouring it exactly
/// is what keeps the child, `TN` and the recording in agreement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TermName(String);

impl TermName {
    /// Resolve the `TERM` for a pane spawned on `platform` with the given `[shell]` config and
    /// per-pane `env` map.
    #[must_use]
    pub fn resolve(
        shell: &ShellConfig,
        platform: TermPlatform,
        pane_env: Option<&HashMap<String, String>>,
    ) -> Self {
        let term = pane_env
            .and_then(Self::env_override)
            .unwrap_or_else(|| shell.effective_term(platform));
        Self(term.to_owned())
    }

    /// The `TERM` set by a per-pane `env` map, if it has one.
    #[must_use]
    pub fn env_override(env: &HashMap<String, String>) -> Option<&str> {
        env.get(TERM_ENV_KEY).map(String::as_str)
    }

    /// Whether `key` names the `TERM` variable in a per-pane `env` map.
    ///
    /// The spawn path skips this key when it applies the map, because the value is already
    /// folded into the [`TermName`] it sets.
    #[must_use]
    pub fn is_env_key(key: &str) -> bool {
        key == TERM_ENV_KEY
    }

    /// The default `TERM` on `platform`, for panes with no configuration to read.
    #[must_use]
    pub fn platform_default(platform: TermPlatform) -> Self {
        Self(platform.default_term().to_owned())
    }

    /// The name as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for TermName {
    /// The default `TERM` on the platform this binary was compiled for.
    fn default() -> Self {
        Self::platform_default(TermPlatform::current())
    }
}

impl fmt::Display for TermName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
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

    fn shell_with_term(term: Option<&str>) -> ShellConfig {
        ShellConfig {
            term: term.map(str::to_owned),
            ..ShellConfig::default()
        }
    }

    fn env_with_term(term: &str) -> HashMap<String, String> {
        HashMap::from([("TERM".to_owned(), term.to_owned())])
    }

    #[test]
    fn resolve_uses_the_platform_default_without_config_or_env() {
        let shell = shell_with_term(None);
        for platform in [TermPlatform::Other, TermPlatform::Windows] {
            assert_eq!(
                TermName::resolve(&shell, platform, None).as_str(),
                platform.default_term()
            );
            assert_eq!(
                TermName::resolve(&shell, platform, Some(&HashMap::new())).as_str(),
                platform.default_term()
            );
        }
        assert_eq!(
            TermName::resolve(&shell, TermPlatform::Other, None).as_str(),
            "xterm-freminal"
        );
    }

    #[test]
    fn resolve_prefers_the_configured_term_over_the_default() {
        let shell = shell_with_term(Some("xterm-256color"));
        assert_eq!(
            TermName::resolve(&shell, TermPlatform::Other, None).as_str(),
            "xterm-256color"
        );
    }

    #[test]
    fn resolve_ignores_an_invalid_configured_term() {
        let shell = shell_with_term(Some("a b"));
        assert_eq!(
            TermName::resolve(&shell, TermPlatform::Other, None).as_str(),
            "xterm-freminal"
        );
    }

    #[test]
    fn resolve_prefers_the_pane_env_term_over_config() {
        let shell = shell_with_term(Some("xterm-256color"));
        let env = env_with_term("screen-256color");
        assert_eq!(
            TermName::resolve(&shell, TermPlatform::Other, Some(&env)).as_str(),
            "screen-256color"
        );
        assert_eq!(
            TermName::resolve(&shell_with_term(None), TermPlatform::Windows, Some(&env)).as_str(),
            "screen-256color"
        );
    }

    #[test]
    fn resolve_ignores_unrelated_env_entries() {
        let shell = shell_with_term(None);
        let env = HashMap::from([("TERM_PROGRAM".to_owned(), "other".to_owned())]);
        assert_eq!(
            TermName::resolve(&shell, TermPlatform::Other, Some(&env)).as_str(),
            "xterm-freminal"
        );
    }

    #[test]
    fn env_key_is_exactly_term() {
        assert!(TermName::is_env_key("TERM"));
        assert!(!TermName::is_env_key("TERM_PROGRAM"));
        assert!(!TermName::is_env_key("term"));
    }

    #[test]
    fn default_is_the_current_platform_default() {
        assert_eq!(
            TermName::default().as_str(),
            TermPlatform::current().default_term()
        );
        assert_eq!(
            TermName::default().to_string(),
            TermName::default().as_str()
        );
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
