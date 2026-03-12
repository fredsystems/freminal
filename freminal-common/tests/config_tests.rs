// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use freminal_common::config::{
    Config, ConfigError, LoggingConfig, ScrollbackConfig, ShellConfig, load_config, save_config,
};
use std::io::Write;
use std::path::Path;
use tempfile::NamedTempFile;

/// Helper: write TOML content to a temp file and load it via `load_config`.
fn load_from_toml(toml: &str) -> Result<Config, freminal_common::config::ConfigError> {
    let mut file = NamedTempFile::new().expect("failed to create temp file");
    file.write_all(toml.as_bytes())
        .expect("failed to write temp file");
    load_config(Some(file.path()))
}

// ─────────────────────────────────────────────────────────────────────────────
//  Defaults
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn default_config_has_expected_values() {
    let cfg = Config::default();
    assert_eq!(cfg.version, 1);
    assert!((cfg.font.size - 12.0).abs() < f32::EPSILON);
    assert!(cfg.font.family.is_none());
    assert!(cfg.cursor.blink);
    assert_eq!(cfg.theme.name, "catppuccin-mocha");
    assert!(cfg.shell.path.is_none());
    assert!(!cfg.logging.write_to_file);
    assert_eq!(cfg.scrollback.limit, 4000);
}

// ─────────────────────────────────────────────────────────────────────────────
//  Backward compatibility — old configs without new sections
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn v1_config_without_new_sections_loads_with_defaults() {
    let toml = r#"
        version = 1

        [font]
        family = "Fira Code"
        size = 14.0

        [cursor]
        shape = "bar"
        blink = false

        [theme]
        name = "gruvbox"
    "#;

    let cfg = load_from_toml(toml).expect("should parse v1 config");
    assert_eq!(cfg.version, 1);
    assert_eq!(cfg.font.family.as_deref(), Some("Fira Code"));
    assert!((cfg.font.size - 14.0).abs() < f32::EPSILON);
    assert!(!cfg.cursor.blink);
    assert_eq!(cfg.theme.name, "gruvbox");

    // New sections should fall back to defaults
    assert!(cfg.shell.path.is_none());
    assert!(!cfg.logging.write_to_file);
    assert_eq!(cfg.scrollback.limit, 4000);
}

#[test]
fn empty_config_file_loads_all_defaults() {
    // An empty file should parse successfully and use all defaults.
    let cfg = load_from_toml("").expect("empty config should parse");
    // version defaults to 1 (valid)
    assert_eq!(cfg.version, 1);
    assert_eq!(cfg.scrollback.limit, 4000);
    assert!(!cfg.logging.write_to_file);
}

// ─────────────────────────────────────────────────────────────────────────────
//  Full config with all sections
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn full_config_with_all_sections_parses() {
    let toml = r#"
        version = 1

        [font]
        family = "JetBrains Mono"
        size = 16.0

        [cursor]
        shape = "underline"
        blink = true

        [theme]
        name = "dracula"

        [shell]
        path = "/bin/zsh"

        [logging]
        write_to_file = true

        [scrollback]
        limit = 10000
    "#;

    let cfg = load_from_toml(toml).expect("should parse full config");
    assert_eq!(cfg.font.family.as_deref(), Some("JetBrains Mono"));
    assert!((cfg.font.size - 16.0).abs() < f32::EPSILON);
    assert!(cfg.cursor.blink);
    assert_eq!(cfg.theme.name, "dracula");
    assert_eq!(cfg.shell.path.as_deref(), Some("/bin/zsh"));
    assert!(cfg.logging.write_to_file);
    assert_eq!(cfg.scrollback.limit, 10000);
}

// ─────────────────────────────────────────────────────────────────────────────
//  Individual new sections
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn shell_section_parses_with_path() {
    let toml = r#"
        version = 1
        [shell]
        path = "/usr/local/bin/fish"
    "#;
    let cfg = load_from_toml(toml).expect("should parse shell section");
    assert_eq!(cfg.shell.path.as_deref(), Some("/usr/local/bin/fish"));
}

#[test]
fn shell_section_without_path_defaults_to_none() {
    let toml = r#"
        version = 1
        [shell]
    "#;
    let cfg = load_from_toml(toml).expect("should parse empty shell section");
    assert!(cfg.shell.path.is_none());
}

#[test]
fn logging_section_write_to_file_true() {
    let toml = r#"
        version = 1
        [logging]
        write_to_file = true
    "#;
    let cfg = load_from_toml(toml).expect("should parse logging section");
    assert!(cfg.logging.write_to_file);
}

#[test]
fn logging_section_write_to_file_false() {
    let toml = r#"
        version = 1
        [logging]
        write_to_file = false
    "#;
    let cfg = load_from_toml(toml).expect("should parse logging section");
    assert!(!cfg.logging.write_to_file);
}

#[test]
fn scrollback_section_custom_limit() {
    let toml = r#"
        version = 1
        [scrollback]
        limit = 50000
    "#;
    let cfg = load_from_toml(toml).expect("should parse scrollback section");
    assert_eq!(cfg.scrollback.limit, 50000);
}

#[test]
fn scrollback_section_minimum_valid_limit() {
    let toml = r#"
        version = 1
        [scrollback]
        limit = 1
    "#;
    let cfg = load_from_toml(toml).expect("should accept scrollback.limit=1");
    assert_eq!(cfg.scrollback.limit, 1);
}

#[test]
fn scrollback_section_maximum_valid_limit() {
    let toml = r#"
        version = 1
        [scrollback]
        limit = 100000
    "#;
    let cfg = load_from_toml(toml).expect("should accept scrollback.limit=100000");
    assert_eq!(cfg.scrollback.limit, 100_000);
}

// ─────────────────────────────────────────────────────────────────────────────
//  Validation — scrollback
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn scrollback_limit_zero_is_rejected() {
    let toml = r#"
        version = 1
        [scrollback]
        limit = 0
    "#;
    let err = load_from_toml(toml).expect_err("limit=0 should fail validation");
    let msg = err.to_string();
    assert!(
        msg.contains("scrollback.limit=0"),
        "error should mention the bad value: {msg}"
    );
}

#[test]
fn scrollback_limit_too_large_is_rejected() {
    let toml = r#"
        version = 1
        [scrollback]
        limit = 100001
    "#;
    let err = load_from_toml(toml).expect_err("limit=100001 should fail validation");
    let msg = err.to_string();
    assert!(
        msg.contains("scrollback.limit=100001"),
        "error should mention the bad value: {msg}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
//  Validation — existing checks still work
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn font_size_too_small_is_rejected() {
    let toml = r#"
        version = 1
        [font]
        size = 2.0
    "#;
    let err = load_from_toml(toml).expect_err("font.size=2.0 should fail validation");
    let msg = err.to_string();
    assert!(msg.contains("font.size=2"), "error: {msg}");
}

#[test]
fn font_size_too_large_is_rejected() {
    let toml = r#"
        version = 1
        [font]
        size = 100.0
    "#;
    let err = load_from_toml(toml).expect_err("font.size=100.0 should fail validation");
    let msg = err.to_string();
    assert!(msg.contains("font.size=100"), "error: {msg}");
}

#[test]
fn version_zero_is_rejected() {
    let toml = r#"
        version = 0
    "#;
    let err = load_from_toml(toml).expect_err("version=0 should fail validation");
    let msg = err.to_string();
    assert!(msg.contains("version must be >= 1"), "error: {msg}");
}

// ─────────────────────────────────────────────────────────────────────────────
//  Serialization round-trip (for future save_config support)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn config_serializes_to_valid_toml() {
    let cfg = Config::default();
    let toml_str = toml::to_string_pretty(&cfg).expect("should serialize to TOML");

    // Deserialize it back
    let deserialized: Config = toml::from_str(&toml_str).expect("serialized TOML should parse");
    assert_eq!(deserialized.version, cfg.version);
    assert!((deserialized.font.size - cfg.font.size).abs() < f32::EPSILON);
    assert_eq!(deserialized.theme.name, cfg.theme.name);
    assert_eq!(deserialized.scrollback.limit, cfg.scrollback.limit);
    assert_eq!(
        deserialized.logging.write_to_file,
        cfg.logging.write_to_file
    );
    assert_eq!(deserialized.shell.path, cfg.shell.path);
}

#[test]
fn config_with_custom_values_round_trips() {
    let mut cfg = Config::default();
    cfg.shell.path = Some("/bin/fish".to_string());
    cfg.logging.write_to_file = true;
    cfg.scrollback.limit = 8000;
    cfg.font.family = Some("Hack".to_string());

    let toml_str = toml::to_string_pretty(&cfg).expect("should serialize");
    let deserialized: Config = toml::from_str(&toml_str).expect("should deserialize");

    assert_eq!(deserialized.shell.path.as_deref(), Some("/bin/fish"));
    assert!(deserialized.logging.write_to_file);
    assert_eq!(deserialized.scrollback.limit, 8000);
    assert_eq!(deserialized.font.family.as_deref(), Some("Hack"));
}

// ─────────────────────────────────────────────────────────────────────────────
//  Default impls for section structs
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn shell_config_default() {
    let s = ShellConfig::default();
    assert!(s.path.is_none());
}

#[test]
fn logging_config_default() {
    let l = LoggingConfig::default();
    assert!(!l.write_to_file);
}

#[test]
fn scrollback_config_default() {
    let s = ScrollbackConfig::default();
    assert_eq!(s.limit, 4000);
}

// ─────────────────────────────────────────────────────────────────────────────
//  Partial configs — only some new sections present
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn config_with_only_shell_section_uses_defaults_for_rest() {
    let toml = r#"
        version = 1
        [shell]
        path = "/bin/bash"
    "#;
    let cfg = load_from_toml(toml).expect("should parse");
    assert_eq!(cfg.shell.path.as_deref(), Some("/bin/bash"));
    assert!(!cfg.logging.write_to_file);
    assert_eq!(cfg.scrollback.limit, 4000);
}

#[test]
fn config_with_only_logging_section_uses_defaults_for_rest() {
    let toml = r#"
        version = 1
        [logging]
        write_to_file = true
    "#;
    let cfg = load_from_toml(toml).expect("should parse");
    assert!(cfg.logging.write_to_file);
    assert!(cfg.shell.path.is_none());
    assert_eq!(cfg.scrollback.limit, 4000);
}

#[test]
fn config_with_only_scrollback_section_uses_defaults_for_rest() {
    let toml = r#"
        version = 1
        [scrollback]
        limit = 500
    "#;
    let cfg = load_from_toml(toml).expect("should parse");
    assert_eq!(cfg.scrollback.limit, 500);
    assert!(cfg.shell.path.is_none());
    assert!(!cfg.logging.write_to_file);
}

// ─────────────────────────────────────────────────────────────────────────────
//  TOML parse errors for invalid types in new sections
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn scrollback_limit_string_is_parse_error() {
    let toml = r#"
        version = 1
        [scrollback]
        limit = "lots"
    "#;
    let err = load_from_toml(toml).expect_err("string limit should fail");
    let msg = err.to_string();
    assert!(msg.contains("TOML parse error"), "error: {msg}");
}

#[test]
fn logging_write_to_file_string_is_parse_error() {
    let toml = r#"
        version = 1
        [logging]
        write_to_file = "yes"
    "#;
    let err = load_from_toml(toml).expect_err("string bool should fail");
    let msg = err.to_string();
    assert!(msg.contains("TOML parse error"), "error: {msg}");
}

// ─────────────────────────────────────────────────────────────────────────────
//  Explicit --config path behavior
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn explicit_config_path_that_does_not_exist_returns_io_error() {
    let path = Path::new("/tmp/freminal_nonexistent_test_config_12345.toml");
    assert!(!path.exists(), "test precondition: file should not exist");

    let err = load_config(Some(path)).expect_err("missing explicit path should fail");
    assert!(
        matches!(err, ConfigError::Io { .. }),
        "expected ConfigError::Io, got: {err}"
    );
    let msg = err.to_string();
    assert!(
        msg.contains("freminal_nonexistent_test_config_12345.toml"),
        "error should mention the path: {msg}"
    );
}

#[test]
fn explicit_config_path_that_exists_loads_successfully() {
    let toml = r#"
        version = 1
        [scrollback]
        limit = 7777
    "#;
    let mut file = NamedTempFile::new().expect("failed to create temp file");
    file.write_all(toml.as_bytes())
        .expect("failed to write temp file");

    let cfg = load_config(Some(file.path())).expect("valid explicit path should succeed");
    assert_eq!(cfg.scrollback.limit, 7777);
}

#[test]
fn no_explicit_config_path_uses_layered_defaults() {
    // When no explicit path is given, load_config should succeed with defaults
    // (assuming no user/system config files override anything in the test environment).
    // We can't fully control what's on disk, but at minimum it shouldn't error.
    let result = load_config(None);
    assert!(
        result.is_ok(),
        "load_config(None) should succeed: {:?}",
        result.err()
    );
}

// ─────────────────────────────────────────────────────────────────────────────
//  CLI + TOML precedence merge (apply_cli_overrides)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn cli_shell_overrides_toml_shell() {
    let toml = r#"
        version = 1
        [shell]
        path = "/bin/zsh"
    "#;
    let mut cfg = load_from_toml(toml).expect("should parse");
    assert_eq!(cfg.shell_path(), Some("/bin/zsh"));

    cfg.apply_cli_overrides(Some("/bin/bash"), None);
    assert_eq!(cfg.shell_path(), Some("/bin/bash"));
}

#[test]
fn cli_shell_none_preserves_toml_shell() {
    let toml = r#"
        version = 1
        [shell]
        path = "/bin/zsh"
    "#;
    let mut cfg = load_from_toml(toml).expect("should parse");
    cfg.apply_cli_overrides(None, None);
    assert_eq!(cfg.shell_path(), Some("/bin/zsh"));
}

#[test]
fn cli_write_logs_override_is_ignored_deprecated() {
    let toml = r#"
        version = 1
        [logging]
        write_to_file = false
    "#;
    let mut cfg = load_from_toml(toml).expect("should parse");
    assert!(!cfg.logging.write_to_file);

    // apply_cli_overrides intentionally ignores write_logs_to_file (deprecated)
    cfg.apply_cli_overrides(None, Some(true));
    assert!(!cfg.logging.write_to_file);
}

#[test]
fn cli_write_logs_none_preserves_toml_logging() {
    let toml = r#"
        version = 1
        [logging]
        write_to_file = true
    "#;
    let mut cfg = load_from_toml(toml).expect("should parse");
    assert!(cfg.logging.write_to_file);

    cfg.apply_cli_overrides(None, None);
    assert!(cfg.logging.write_to_file);
}

#[test]
fn cli_overrides_shell_but_not_logging() {
    let toml = r#"
        version = 1
        [shell]
        path = "/bin/zsh"
        [logging]
        write_to_file = false
    "#;
    let mut cfg = load_from_toml(toml).expect("should parse");

    cfg.apply_cli_overrides(Some("/usr/local/bin/fish"), Some(true));
    assert_eq!(cfg.shell_path(), Some("/usr/local/bin/fish"));
    // write_logs_to_file is deprecated and ignored by apply_cli_overrides
    assert!(!cfg.logging.write_to_file);
}

#[test]
fn default_config_with_no_cli_overrides() {
    let mut cfg = Config::default();
    assert!(cfg.shell_path().is_none());
    assert!(!cfg.logging.write_to_file);

    cfg.apply_cli_overrides(None, None);
    assert!(cfg.shell_path().is_none());
    assert!(!cfg.logging.write_to_file);
}

#[test]
fn cli_shell_overrides_when_toml_has_no_shell() {
    let mut cfg = Config::default();
    assert!(cfg.shell_path().is_none());

    cfg.apply_cli_overrides(Some("/bin/fish"), None);
    assert_eq!(cfg.shell_path(), Some("/bin/fish"));
}

#[test]
fn cli_write_logs_false_is_ignored_deprecated() {
    let toml = r#"
        version = 1
        [logging]
        write_to_file = true
    "#;
    let mut cfg = load_from_toml(toml).expect("should parse");
    assert!(cfg.logging.write_to_file);

    // apply_cli_overrides intentionally ignores write_logs_to_file (deprecated)
    cfg.apply_cli_overrides(None, Some(false));
    assert!(cfg.logging.write_to_file);
}

// ─────────────────────────────────────────────────────────────────────────────
//  file_log_level()
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn file_log_level_defaults_to_debug() {
    let cfg = Config::default();
    assert_eq!(cfg.file_log_level(), "debug");
}

#[test]
fn file_log_level_reads_from_toml() {
    let toml = r#"
        version = 1
        [logging]
        level = "trace"
    "#;
    let cfg = load_from_toml(toml).expect("should parse");
    assert_eq!(cfg.file_log_level(), "trace");
}

#[test]
fn file_log_level_absent_in_toml_defaults_to_debug() {
    let toml = r#"
        version = 1
        [logging]
    "#;
    let cfg = load_from_toml(toml).expect("should parse");
    assert_eq!(cfg.file_log_level(), "debug");
}

// ─────────────────────────────────────────────────────────────────────────────
//  save_config — serialization / write-back
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn save_default_config_to_explicit_path_creates_valid_file() {
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let path = dir.path().join("config.toml");

    let cfg = Config::default();
    save_config(&cfg, Some(&path)).expect("save should succeed");

    // The file should exist and be valid TOML that we can load back.
    assert!(path.exists(), "config file should be created");
    let loaded = load_config(Some(&path)).expect("saved config should reload");
    assert_eq!(loaded.version, cfg.version);
    assert!((loaded.font.size - cfg.font.size).abs() < f32::EPSILON);
    assert_eq!(loaded.scrollback.limit, cfg.scrollback.limit);
}

#[test]
fn save_config_round_trips_custom_values() {
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let path = dir.path().join("custom.toml");

    let mut cfg = Config::default();
    cfg.shell.path = Some("/bin/zsh".to_string());
    cfg.logging.write_to_file = true;
    cfg.scrollback.limit = 10_000;
    cfg.font.size = 16.0;
    cfg.font.family = Some("JetBrains Mono".to_string());
    cfg.theme.name = "solarized-dark".to_string();

    save_config(&cfg, Some(&path)).expect("save should succeed");

    let loaded = load_config(Some(&path)).expect("reloaded config should be valid");
    assert_eq!(loaded.shell.path.as_deref(), Some("/bin/zsh"));
    assert!(loaded.logging.write_to_file);
    assert_eq!(loaded.scrollback.limit, 10_000);
    assert!((loaded.font.size - 16.0).abs() < f32::EPSILON);
    assert_eq!(loaded.font.family.as_deref(), Some("JetBrains Mono"));
    assert_eq!(loaded.theme.name, "solarized-dark");
}

#[test]
fn save_config_creates_parent_directories() {
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let path = dir.path().join("nested").join("deep").join("config.toml");

    let cfg = Config::default();
    save_config(&cfg, Some(&path)).expect("save should create parent dirs");
    assert!(path.exists());
}

#[test]
fn save_config_rejects_invalid_config() {
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let path = dir.path().join("bad.toml");

    let mut cfg = Config::default();
    cfg.scrollback.limit = 0; // Invalid: must be >= 1

    let err = save_config(&cfg, Some(&path));
    assert!(err.is_err(), "should reject invalid scrollback limit");
    assert!(!path.exists(), "invalid config should not be written");
}

#[test]
fn save_config_output_is_valid_toml() {
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let path = dir.path().join("check.toml");

    let cfg = Config::default();
    save_config(&cfg, Some(&path)).expect("save should succeed");

    let contents = std::fs::read_to_string(&path).expect("should read file");
    // Verify it parses as TOML
    let _: toml::Value = toml::from_str(&contents).expect("output should be valid TOML");
}

#[test]
fn save_then_modify_then_save_overwrites() {
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let path = dir.path().join("overwrite.toml");

    let cfg = Config::default();
    save_config(&cfg, Some(&path)).expect("first save");

    let mut cfg2 = Config::default();
    cfg2.scrollback.limit = 8000;
    save_config(&cfg2, Some(&path)).expect("second save");

    let loaded = load_config(Some(&path)).expect("reload after overwrite");
    assert_eq!(loaded.scrollback.limit, 8000);
}

#[test]
fn save_config_preserves_none_shell_path() {
    // When shell.path is None, it should not appear in the TOML output
    // (thanks to skip_serializing_if = "Option::is_none").
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let path = dir.path().join("no_shell.toml");

    let cfg = Config::default();
    assert!(cfg.shell.path.is_none());
    save_config(&cfg, Some(&path)).expect("save should succeed");

    let contents = std::fs::read_to_string(&path).expect("should read file");
    // The [shell] section should exist but not contain "path =".
    assert!(
        !contents.contains("path ="),
        "None shell.path should be omitted from TOML output"
    );

    // Reloading should still produce None for shell.path.
    let loaded = load_config(Some(&path)).expect("reload should succeed");
    assert!(loaded.shell.path.is_none());
}

// ─────────────────────────────────────────────────────────────────────────────
//  log_dir()
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn log_dir_returns_some_on_current_platform() {
    let dir = freminal_common::config::log_dir();
    assert!(
        dir.is_some(),
        "log_dir() should return Some on this platform"
    );
}

#[test]
fn log_dir_ends_with_expected_directory_name() {
    let dir = freminal_common::config::log_dir().expect("log_dir() returned None");
    let dir_name = dir
        .file_name()
        .expect("log dir should have a final component")
        .to_string_lossy();

    // Linux/BSD: "freminal", macOS: "Freminal", Windows: "logs" (parent is "Freminal")
    #[cfg(any(
        target_os = "linux",
        target_os = "freebsd",
        target_os = "dragonfly",
        target_os = "openbsd",
        target_os = "netbsd"
    ))]
    assert_eq!(dir_name, "freminal");

    #[cfg(target_os = "macos")]
    assert_eq!(dir_name, "Freminal");

    #[cfg(target_os = "windows")]
    {
        assert_eq!(dir_name, "logs");
        let parent_name = dir
            .parent()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        assert_eq!(parent_name, "Freminal");
    }
}
