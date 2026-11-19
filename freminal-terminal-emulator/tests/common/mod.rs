// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Helpers shared by the terminfo integration tests.
//!
//! Both users are Unix only (Windows has no terminfo database and no `tic`), so
//! the module is declared under `#[cfg(unix)]`.

/// Whether the `CI` variable marks this as a CI run: set and non-empty.
fn is_ci(value: Option<std::ffi::OsString>) -> bool {
    value.is_some_and(|value| !value.is_empty())
}

/// Report that a terminfo tool or entry is missing.  Locally that skips the test;
/// on CI (`CI` set and non-empty) every Unix runner has the tools and the system
/// `xterm-256color` entry, so a missing one fails instead of letting the test pass
/// without checking anything.
pub fn skip_unless_ci(reason: &str) {
    assert!(
        !is_ci(std::env::var_os("CI")),
        "CI is set, so a missing terminfo tool is a failure, not a skip: {reason}"
    );
    eprintln!("SKIPPED: {reason}");
}

#[cfg(test)]
mod tests {
    use super::is_ci;

    #[test]
    fn ci_is_a_set_and_non_empty_variable() {
        assert!(!is_ci(None));
        assert!(!is_ci(Some(std::ffi::OsString::new())));
        assert!(is_ci(Some(std::ffi::OsString::from("true"))));
        assert!(is_ci(Some(std::ffi::OsString::from("1"))));
    }
}
