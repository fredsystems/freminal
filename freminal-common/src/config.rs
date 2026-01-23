// Copyright (C) 2024-2025 Fred Clausen
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
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: 1,
            font: FontConfig::default(),
            cursor: CursorConfig::default(),
            theme: ThemeConfig::default(),
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

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
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
///  Partial config (for layered merging)
/// ---------------------------------------------------------------------------------------------
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ConfigPartial {
    pub version: Option<u32>,
    pub font: Option<FontConfig>,
    pub cursor: Option<CursorConfig>,
    pub theme: Option<ThemeConfig>,
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

    // 4. Explicit CLI override
    if let Some(path) = explicit_path
        && path.is_file()
    {
        let partial = load_partial(path)?;
        cfg.apply_partial(partial);
    }

    cfg.validate()?;
    Ok(cfg)
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
