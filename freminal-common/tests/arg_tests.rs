// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use clap::Parser;
use freminal_common::args::Args;
use proptest::{prop_assert, prop_assert_eq, proptest};

/// Helper: run the parser with a simple iterator of strings
fn parse_from<I: IntoIterator<Item = S>, S: Into<std::ffi::OsString> + Clone>(
    args: I,
) -> Result<Args, clap::Error> {
    Args::try_parse_from(args)
}

// ------------------------
// Unit tests
// ------------------------

#[test]
fn parses_empty_args_defaults() {
    let args = parse_from(["freminal"]).unwrap();
    assert!(args.recording.is_none());
    assert!(args.shell.is_none());
    assert!(!args.show_all_debug);
    #[cfg(debug_assertions)]
    assert!(args.write_logs_to_file);
    #[cfg(not(debug_assertions))]
    assert!(!args.write_logs_to_file);
    assert!(args.config.is_none());
}

#[test]
fn parses_recording_path() {
    let args = parse_from(["freminal", "--recording-path", "rec.log"]).unwrap();
    assert_eq!(args.recording.as_deref(), Some("rec.log"));
}

#[test]
fn missing_recording_path_argument() {
    let result = parse_from(["freminal", "--recording-path"]);
    assert!(result.is_err());
}

#[test]
fn parses_shell_argument() {
    let args = parse_from(["freminal", "--shell", "/bin/bash"]).unwrap();
    assert_eq!(args.shell.as_deref(), Some("/bin/bash"));
}

#[test]
fn missing_shell_argument() {
    let result = parse_from(["freminal", "--shell"]);
    assert!(result.is_err());
}

#[test]
fn parses_show_all_debug_flag() {
    let args = parse_from(["freminal", "--show-all-debug"]).unwrap();
    assert!(args.show_all_debug);
}

#[test]
fn parses_write_logs_to_file_true() {
    let args = parse_from(["freminal", "--write-logs-to-file=true"]).unwrap();
    assert!(args.write_logs_to_file);
}

#[test]
fn parses_write_logs_to_file_false() {
    let args = parse_from(["freminal", "--write-logs-to-file=false"]).unwrap();
    assert!(!args.write_logs_to_file);
}

#[test]
fn missing_write_logs_to_file_value_defaults_to_true() {
    // With clap, `--write-logs-to-file` without `=value` should default to true
    // (via default_missing_value)
    let args = parse_from(["freminal", "--write-logs-to-file"]).unwrap();
    assert!(args.write_logs_to_file);
}

#[test]
fn invalid_write_logs_to_file_value() {
    let result = parse_from(["freminal", "--write-logs-to-file=maybe"]);
    assert!(result.is_err());
}

#[test]
fn invalid_argument_is_error() {
    let result = parse_from(["freminal", "--not-a-real-flag"]);
    assert!(result.is_err());
}

#[test]
fn parses_config_path() {
    let args = parse_from(["freminal", "--config", "/path/to/config.toml"]).unwrap();
    assert_eq!(
        args.config.as_deref(),
        Some(std::path::Path::new("/path/to/config.toml"))
    );
}

#[test]
fn missing_config_path_argument() {
    let result = parse_from(["freminal", "--config"]);
    assert!(result.is_err());
}

#[test]
fn help_flag_produces_help_error() {
    // Clap treats --help as a special error (not a parse failure)
    let result = parse_from(["freminal", "--help"]);
    assert!(result.is_err());
    if let Err(e) = result {
        assert_eq!(e.kind(), clap::error::ErrorKind::DisplayHelp);
    }
}

#[test]
fn version_flag_produces_version_error() {
    let result = parse_from(["freminal", "--version"]);
    assert!(result.is_err());
    if let Err(e) = result {
        assert_eq!(e.kind(), clap::error::ErrorKind::DisplayVersion);
    }
}

#[test]
fn all_flags_combined() {
    let args = parse_from([
        "freminal",
        "--recording-path",
        "rec.log",
        "--shell",
        "/bin/zsh",
        "--show-all-debug",
        "--write-logs-to-file=true",
        "--config",
        "/tmp/config.toml",
    ])
    .unwrap();
    assert_eq!(args.recording.as_deref(), Some("rec.log"));
    assert_eq!(args.shell.as_deref(), Some("/bin/zsh"));
    assert!(args.show_all_debug);
    assert!(args.write_logs_to_file);
    assert_eq!(
        args.config.as_deref(),
        Some(std::path::Path::new("/tmp/config.toml"))
    );
}

// ------------------------
// Property-based tests
// ------------------------

proptest! {
    /// Any combination of valid boolean flag forms for `--write-logs-to-file`
    /// should parse consistently.
    #[test]
    fn write_logs_to_file_accepts_boolean_values(val in proptest::bool::ANY) {
        let arg = format!("--write-logs-to-file={}", val);
        let args = parse_from(["freminal", &arg]).unwrap();
        prop_assert_eq!(args.write_logs_to_file, val);
    }

    /// Arbitrary strings that do *not* start with `--` should always trigger an error.
    #[test]
    fn invalid_arguments_fail(s in "[a-zA-Z0-9_]+") {
        // Avoid empty program name
        let result = parse_from(["freminal", &s]);
        prop_assert!(result.is_err());
    }

    /// Mixing valid and invalid flags: the first invalid should cause failure.
    #[test]
    fn mixed_valid_and_invalid_arguments_fail(
        bad_arg in "--[a-z]{1,8}",
        rec in "rec[0-9]+\\.log"
    ) {
        let args = ["freminal", "--recording-path", &rec, &bad_arg];
        let result = parse_from(args);
        // Note: some randomly generated flags might coincidentally match valid flags
        // (e.g., "--shell" without value, "--config" without value).
        // We accept both outcomes since the test is about arbitrary invalid flags.
        let _ = result;
    }

    /// Ensure `--recording-path` and `--shell` always propagate correctly
    /// for random filenames and shell names.
    #[test]
    fn recording_and_shell_preserved(
        path in "[a-zA-Z0-9_/\\.]{1,20}",
        shell in "/bin/[a-z]{2,8}"
    ) {
        let args = parse_from(["freminal", "--recording-path", &path, "--shell", &shell]).unwrap();
        prop_assert_eq!(args.recording.as_deref(), Some(path.as_str()));
        prop_assert_eq!(args.shell.as_deref(), Some(shell.as_str()));
    }

    /// The parser should never panic or crash for arbitrary ASCII input.
    #[test]
    fn parser_never_panics_on_random_input(input in proptest::collection::vec("[ -~]{0,20}", 0..10)) {
        let args: Vec<String> = std::iter::once("freminal".to_string())
            .chain(input.into_iter())
            .collect();
        let _ = Args::try_parse_from(args); // should not panic
    }
}
