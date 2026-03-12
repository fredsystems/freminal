// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;

use directories::BaseDirs;

/// ---------------------------------------------------------------------------------------------
///  Top-level Config Structure
/// ---------------------------------------------------------------------------------------------
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub version: u32,
    pub font: FontConfig,
    pub cursor: CursorConfig,
    pub theme: ThemeConfig,
    pub shell: ShellConfig,
    pub logging: LoggingConfig,
    pub scrollback: ScrollbackConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: 1,
            font: FontConfig::default(),
            cursor: CursorConfig::default(),
            theme: ThemeConfig::default(),
            shell: ShellConfig::default(),
            logging: LoggingConfig::default(),
            scrollback: ScrollbackConfig::default(),
        }
    }
}

/// ---------------------------------------------------------------------------------------------
///  Font
/// ---------------------------------------------------------------------------------------------
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FontConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub family: Option<String>,
    pub size: f32,
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            family: None,
            size: 12.0,
        }
    }
}

/// ---------------------------------------------------------------------------------------------
///  Cursor
/// ---------------------------------------------------------------------------------------------
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CursorConfig {
    pub shape: CursorShapeConfig,
    pub blink: bool,
}

impl Default for CursorConfig {
    fn default() -> Self {
        Self {
            shape: CursorShapeConfig::Block,
            blink: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum CursorShapeConfig {
    #[default]
    Block,
    Underline,
    Bar,
}

/// ---------------------------------------------------------------------------------------------
///  Theme
/// ---------------------------------------------------------------------------------------------
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemeConfig {
    pub name: String,
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self {
            name: "catppuccin-mocha".to_string(),
        }
    }
}

/// ---------------------------------------------------------------------------------------------
///  Shell
/// ---------------------------------------------------------------------------------------------
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ShellConfig {
    /// Default shell path. When `None`, the system default shell is used.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// ---------------------------------------------------------------------------------------------
///  Logging
/// ---------------------------------------------------------------------------------------------
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LoggingConfig {
    /// **Deprecated.** File logging is now always on. This field is retained for
    /// backwards-compatible deserialisation but is ignored at runtime.
    #[serde(default)]
    pub write_to_file: bool,

    /// Log level for the file appender. Accepts standard tracing level strings:
    /// `"trace"`, `"debug"`, `"info"`, `"warn"`, `"error"`.
    /// Default: `"debug"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<String>,
}

/// ---------------------------------------------------------------------------------------------
///  Scrollback
/// ---------------------------------------------------------------------------------------------
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ScrollbackConfig {
    /// Maximum number of scrollback lines. Must be in the range `1..=100_000`.
    pub limit: usize,
}

impl Default for ScrollbackConfig {
    fn default() -> Self {
        Self { limit: 4000 }
    }
}

/// ---------------------------------------------------------------------------------------------
///  Partial config (for layered merging)
/// ---------------------------------------------------------------------------------------------
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ConfigPartial {
    pub version: Option<u32>,
    pub font: Option<FontConfig>,
    pub cursor: Option<CursorConfig>,
    pub theme: Option<ThemeConfig>,
    pub shell: Option<ShellConfig>,
    pub logging: Option<LoggingConfig>,
    pub scrollback: Option<ScrollbackConfig>,
}

impl Config {
    fn apply_partial(&mut self, partial: ConfigPartial) {
        if let Some(v) = partial.version {
            self.version = v;
        }
        if let Some(font) = partial.font {
            self.font = font;
        }
        if let Some(cursor) = partial.cursor {
            self.cursor = cursor;
        }
        if let Some(theme) = partial.theme {
            self.theme = theme;
        }
        if let Some(shell) = partial.shell {
            self.shell = shell;
        }
        if let Some(logging) = partial.logging {
            self.logging = logging;
        }
        if let Some(scrollback) = partial.scrollback {
            self.scrollback = scrollback;
        }
    }

    /// Apply CLI argument overrides on top of the loaded configuration.
    ///
    /// For options that exist in both CLI and TOML, CLI takes precedence:
    ///   CLI > TOML > env var > system config > defaults
    ///
    /// Only `Some` values override; `None` means the CLI flag was not specified
    /// and the TOML value (or default) is kept.
    pub fn apply_cli_overrides(&mut self, shell: Option<&str>, _write_logs_to_file: Option<bool>) {
        if let Some(shell_path) = shell {
            self.shell.path = Some(shell_path.to_owned());
        }
        // `write_logs_to_file` is intentionally ignored — file logging is always on.
        // The CLI flag is retained only for backwards compatibility (deprecation notice
        // is printed by the caller).
    }

    /// Returns the effective file log level as a string.
    ///
    /// Falls back to `"debug"` when the config does not specify a level.
    #[must_use]
    pub fn file_log_level(&self) -> &str {
        self.logging.level.as_deref().unwrap_or("debug")
    }

    /// Returns the effective shell path, if configured.
    ///
    /// Returns `None` when neither CLI nor TOML specified a shell path,
    /// in which case the system default should be used.
    #[must_use]
    pub fn shell_path(&self) -> Option<&str> {
        self.shell.path.as_deref()
    }

    fn validate(&self) -> Result<(), ConfigError> {
        if !(4.0..=96.0).contains(&self.font.size) {
            return Err(ConfigError::Validation(format!(
                "font.size={} out of allowed range (4.0–96.0)",
                self.font.size
            )));
        }

        if self.version == 0 {
            return Err(ConfigError::Validation("version must be >= 1".to_string()));
        }

        if self.scrollback.limit == 0 || self.scrollback.limit > 100_000 {
            return Err(ConfigError::Validation(format!(
                "scrollback.limit={} out of allowed range (1–100000)",
                self.scrollback.limit
            )));
        }

        Ok(())
    }
}

/// ---------------------------------------------------------------------------------------------
///  Errors
/// ---------------------------------------------------------------------------------------------
#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("I/O error reading {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("TOML parse error in {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },

    #[error("Invalid configuration: {0}")]
    Validation(String),

    #[error("failed to serialize config: {0}")]
    Serialize(String),

    #[error("I/O error writing {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// ---------------------------------------------------------------------------------------------
///  Public loader
/// ---------------------------------------------------------------------------------------------
/// Loads the configuration by applying layers in the following order (later layers override
/// earlier ones):
///
/// # Errors
/// Returns `ConfigError` if any config file cannot be read or parsed, or if the final config
/// is invalid.
pub fn load_config(explicit_path: Option<&Path>) -> Result<Config, ConfigError> {
    let mut cfg = Config::default();

    // 1. System config (Linux only)
    if let Some(system_path) = system_config_path()
        && system_path.is_file()
    {
        let partial = load_partial(&system_path)?;
        cfg.apply_partial(partial);
    }

    // 2. Platform-specific user config
    if let Some(user_path) = user_config_path()
        && user_path.is_file()
    {
        let partial = load_partial(&user_path)?;
        cfg.apply_partial(partial);
    }

    // 3. FREMINAL_CONFIG= override
    if let Ok(env_path) = env::var("FREMINAL_CONFIG") {
        let path = PathBuf::from(env_path);
        if path.is_file() {
            let partial = load_partial(&path)?;
            cfg.apply_partial(partial);
        }
    }

    // 4. Explicit CLI override — if the user specified --config, the file MUST exist.
    if let Some(path) = explicit_path {
        let partial = load_partial(path)?;
        cfg.apply_partial(partial);
    }

    cfg.validate()?;
    Ok(cfg)
}

/// Saves the configuration to a TOML file.
///
/// If `path` is `Some`, the config is written to that exact location.
/// If `path` is `None`, the config is written to the platform-specific
/// user config path (e.g. `$XDG_CONFIG_HOME/freminal/config.toml`).
///
/// The config is validated before writing so that invalid values are
/// never persisted to disk.
///
/// # Errors
///
/// Returns `ConfigError` if validation fails, serialization fails,
/// the target directory cannot be determined (no home directory), or
/// the file cannot be written.
pub fn save_config(config: &Config, path: Option<&Path>) -> Result<(), ConfigError> {
    config.validate()?;

    let target = match path {
        Some(p) => p.to_path_buf(),
        None => user_config_path().ok_or_else(|| {
            ConfigError::Validation(
                "cannot determine user config directory (no home directory?)".to_string(),
            )
        })?,
    };

    // Ensure the parent directory exists.
    if let Some(parent) = target.parent() {
        create_dir_if_missing(parent);
    }

    let toml_str =
        toml::to_string_pretty(config).map_err(|e| ConfigError::Serialize(e.to_string()))?;

    fs::write(&target, toml_str).map_err(|source| ConfigError::Write {
        path: target,
        source,
    })
}

/// ---------------------------------------------------------------------------------------------
///  Helpers
/// ---------------------------------------------------------------------------------------------
fn load_partial(path: &Path) -> Result<ConfigPartial, ConfigError> {
    let contents = fs::read_to_string(path).map_err(|source| ConfigError::Io {
        path: path.to_path_buf(),
        source,
    })?;

    toml::from_str(&contents).map_err(|source| ConfigError::Parse {
        path: path.to_path_buf(),
        source,
    })
}

/// ---------------------------------------------------------------------------------------------
///  Platform-specific config paths
/// ---------------------------------------------------------------------------------------------
#[allow(clippy::missing_const_for_fn, clippy::unnecessary_wraps)]
fn system_config_path() -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        Some(PathBuf::from("/etc/freminal/config.toml"))
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

/// User config paths:
///
/// Linux:   `$XDG_CONFIG_HOME/freminal/config.toml`
/// macOS:   ~/Library/Application Support/Freminal/config.toml
/// Windows: %APPDATA%\Freminal\config.toml
#[allow(unreachable_code)]
fn user_config_path() -> Option<PathBuf> {
    let base = BaseDirs::new()?;

    #[cfg(target_os = "macos")]
    {
        let mut p = base.data_dir().join("Freminal");
        create_dir_if_missing(&p);
        p.push("config.toml");
        return Some(p);
    }

    #[cfg(target_os = "windows")]
    {
        let mut p = base.data_dir().join("Freminal");
        create_dir_if_missing(&p);
        p.push("config.toml");
        return Some(p);
    }

    // Linux / BSD / everything else Unix-y
    #[cfg(any(
        target_os = "linux",
        target_os = "freebsd",
        target_os = "dragonfly",
        target_os = "openbsd",
        target_os = "netbsd"
    ))]
    {
        let mut p = base.config_dir().join("freminal");
        create_dir_if_missing(&p);
        p.push("config.toml");
        return Some(p);
    }

    None
}

fn create_dir_if_missing(path: &Path) {
    if !path.exists() {
        let _ = fs::create_dir_all(path);
    }
}

/// Returns the platform-canonical log directory for Freminal.
///
/// | Platform  | Path                            |
/// |-----------|---------------------------------|
/// | Linux/BSD | `$XDG_STATE_HOME/freminal/`     |
/// | macOS     | `~/Library/Logs/Freminal/`      |
/// | Windows   | `%LOCALAPPDATA%\Freminal\logs\` |
///
/// The directory is created if it does not already exist.
/// Returns `None` only if the platform's base directories cannot be determined
/// (e.g. no home directory).
#[allow(unreachable_code)]
#[must_use]
pub fn log_dir() -> Option<PathBuf> {
    let base = BaseDirs::new()?;

    #[cfg(target_os = "macos")]
    {
        let p = base.home_dir().join("Library/Logs/Freminal");
        create_dir_if_missing(&p);
        return Some(p);
    }

    #[cfg(target_os = "windows")]
    {
        let p = base.data_local_dir().join("Freminal").join("logs");
        create_dir_if_missing(&p);
        return Some(p);
    }

    // Linux / BSD / everything else Unix-y — use XDG state dir.
    #[cfg(any(
        target_os = "linux",
        target_os = "freebsd",
        target_os = "dragonfly",
        target_os = "openbsd",
        target_os = "netbsd"
    ))]
    {
        // `state_dir()` returns `$XDG_STATE_HOME` (typically `~/.local/state`).
        let p = base.state_dir()?.join("freminal");
        create_dir_if_missing(&p);
        return Some(p);
    }

    None
}
