// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::keybindings::{BindingMap, KeyAction, KeyCombo};
use crate::themes;
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
    pub ui: UiConfig,
    pub tabs: TabsConfig,
    pub bell: BellConfig,
    pub security: SecurityConfig,
    #[serde(default, skip_serializing_if = "KeybindingsConfig::is_empty")]
    pub keybindings: KeybindingsConfig,

    /// Indicates which external tool manages this config file.
    ///
    /// When set (e.g. `"home-manager"`), the settings modal opens in read-only
    /// mode with a message explaining that changes must be made in the managing
    /// tool.  This field is injected automatically by the Nix home-manager
    /// module and should **not** be set manually by end users.
    ///
    /// Omitted from serialized output when `None` so that user-written configs
    /// are not cluttered with it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub managed_by: Option<String>,
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
            ui: UiConfig::default(),
            tabs: TabsConfig::default(),
            bell: BellConfig::default(),
            security: SecurityConfig::default(),
            keybindings: KeybindingsConfig::default(),
            managed_by: None,
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
    /// Enable OpenType ligatures (`liga`, `clig`).  Default: `true`.
    pub ligatures: bool,
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            family: None,
            size: 12.0,
            ligatures: true,
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
    /// Enable smooth cursor trail animation (cursor glides to new position).
    pub trail: bool,
    /// Duration of the cursor trail animation in milliseconds.
    pub trail_duration_ms: u32,
}

impl Default for CursorConfig {
    fn default() -> Self {
        Self {
            shape: CursorShapeConfig::Block,
            blink: true,
            trail: false,
            trail_duration_ms: 150,
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
    /// Default: `"info"`.
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
///  UI
/// ---------------------------------------------------------------------------------------------
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    /// Hide the menu bar at the top of the window. Default: `false`.
    pub hide_menu_bar: bool,
    /// Background opacity (0.0 = fully transparent, 1.0 = fully opaque).
    /// Only affects the terminal and menu bar backgrounds; text and content
    /// remain fully opaque.
    pub background_opacity: f32,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            hide_menu_bar: false,
            background_opacity: 1.0,
        }
    }
}

// ----------------------------------------------------------------------------------------------
//  Tabs
// ----------------------------------------------------------------------------------------------

/// Position of the tab bar relative to the terminal area.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum TabBarPosition {
    /// Tab bar above the terminal (default).
    #[default]
    Top,
    /// Tab bar below the terminal.
    Bottom,
}

/// Configuration for tab behaviour and appearance.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TabsConfig {
    /// Whether to show the tab bar when only one tab is open.
    /// Default: `false` (tab bar only appears with multiple tabs).
    pub show_single_tab: bool,
    /// Position of the tab bar: `"top"` or `"bottom"`.
    pub position: TabBarPosition,
}

impl Default for TabsConfig {
    fn default() -> Self {
        Self {
            show_single_tab: false,
            position: TabBarPosition::Top,
        }
    }
}

// ------------------------------------------------------------------------------------------------
//  Bell
// ------------------------------------------------------------------------------------------------

/// How the terminal should respond to a bell character (`\x07`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum BellMode {
    /// Flash the terminal area briefly (visual bell).
    #[default]
    Visual,
    /// Do nothing.
    None,
}

/// Configuration for the terminal bell.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BellConfig {
    /// Bell mode: `"visual"` (default) or `"none"`.
    pub mode: BellMode,
}

impl Default for BellConfig {
    fn default() -> Self {
        Self {
            mode: BellMode::Visual,
        }
    }
}

// ------------------------------------------------------------------------------------------------
//  Security
// ------------------------------------------------------------------------------------------------

/// Security-related configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SecurityConfig {
    /// Allow applications to read the system clipboard via OSC 52 query.
    ///
    /// Default: `false` (clipboard reads return an empty response).
    /// When `true`, OSC 52 queries return the current clipboard contents
    /// base64-encoded.  This is a potential security risk if untrusted
    /// programs run inside the terminal.
    pub allow_clipboard_read: bool,

    /// Show a lock icon in the tab bar when the foreground process disables
    /// terminal echo (e.g. password prompts from `sudo`, `ssh`, `passwd`).
    ///
    /// Default: `true`.
    pub password_indicator: bool,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            allow_clipboard_read: false,
            password_indicator: true,
        }
    }
}

// ------------------------------------------------------------------------------------------------
//  Keybindings
// ------------------------------------------------------------------------------------------------

/// User-specified key binding overrides.
///
/// Each entry maps an action name (matching [`KeyAction`] `snake_case` names)
/// to a key combo string (e.g. `"Ctrl+Shift+C"`). A value of `"none"` or `""`
/// disables the binding for that action.
///
/// Only overridden actions need to be listed — omitted actions keep their
/// default bindings from [`BindingMap::default()`].
///
/// ## TOML example
///
/// ```toml
/// [keybindings]
/// copy = "Ctrl+C"
/// paste = "Ctrl+V"
/// new_tab = "none"
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct KeybindingsConfig {
    /// Override map: action name → key combo string.
    ///
    /// Stored as flat key-value pairs so that the TOML representation is a
    /// simple table of `action = "combo"` entries.
    #[serde(flatten)]
    pub overrides: HashMap<String, String>,
}

impl KeybindingsConfig {
    /// Returns `true` if no overrides are specified.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.overrides.is_empty()
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
    pub ui: Option<UiConfig>,
    pub tabs: Option<TabsConfig>,
    pub bell: Option<BellConfig>,
    pub security: Option<SecurityConfig>,
    pub keybindings: Option<KeybindingsConfig>,
    pub managed_by: Option<String>,
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
        if let Some(ui) = partial.ui {
            self.ui = ui;
        }
        if let Some(tabs) = partial.tabs {
            self.tabs = tabs;
        }
        if let Some(bell) = partial.bell {
            self.bell = bell;
        }
        if let Some(security) = partial.security {
            self.security = security;
        }
        if let Some(keybindings) = partial.keybindings {
            // Merge override maps: later layers add to / overwrite earlier ones.
            for (action, combo) in keybindings.overrides {
                self.keybindings.overrides.insert(action, combo);
            }
        }
        if partial.managed_by.is_some() {
            self.managed_by = partial.managed_by;
        }
    }

    /// Apply CLI argument overrides on top of the loaded configuration.
    ///
    /// For options that exist in both CLI and TOML, CLI takes precedence:
    ///   CLI > TOML > env var > system config > defaults
    ///
    /// Only `Some` values override; `None` means the CLI flag was not specified
    /// and the TOML value (or default) is kept.
    pub fn apply_cli_overrides(
        &mut self,
        shell: Option<&str>,
        _write_logs_to_file: Option<bool>,
        hide_menu_bar: bool,
    ) {
        if let Some(shell_path) = shell {
            self.shell.path = Some(shell_path.to_owned());
        }
        // `write_logs_to_file` is intentionally ignored — file logging is always on.
        // The CLI flag is retained only for backwards compatibility (deprecation notice
        // is printed by the caller).

        if hide_menu_bar {
            self.ui.hide_menu_bar = true;
        }
    }

    /// Returns the effective file log level as a string.
    ///
    /// Falls back to `"info"` when the config does not specify a level.
    #[must_use]
    pub fn file_log_level(&self) -> &str {
        self.logging.level.as_deref().unwrap_or("info")
    }

    /// Returns the effective shell path, if configured.
    ///
    /// Returns `None` when neither CLI nor TOML specified a shell path,
    /// in which case the system default should be used.
    #[must_use]
    pub fn shell_path(&self) -> Option<&str> {
        self.shell.path.as_deref()
    }

    /// Returns `true` when the config is managed by an external tool
    /// (e.g. Nix home-manager).
    #[must_use]
    pub const fn is_managed(&self) -> bool {
        self.managed_by.is_some()
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

        if !(0.0..=1.0).contains(&self.ui.background_opacity) {
            return Err(ConfigError::Validation(format!(
                "ui.background_opacity={} out of allowed range (0.0–1.0)",
                self.ui.background_opacity
            )));
        }

        if themes::by_slug(&self.theme.name).is_none() {
            return Err(ConfigError::Validation(format!(
                "theme.name=\"{}\" is not a recognized theme slug",
                self.theme.name
            )));
        }

        // Validate keybinding overrides: every action name must be recognized,
        // and every combo string must parse (or be "none" / empty to disable).
        for (action_str, combo_str) in &self.keybindings.overrides {
            KeyAction::from_str(action_str).map_err(|e| {
                ConfigError::Validation(format!(
                    "keybindings: invalid action \"{action_str}\": {e}"
                ))
            })?;

            let trimmed = combo_str.trim();
            if !trimmed.is_empty() && !trimmed.eq_ignore_ascii_case("none") {
                KeyCombo::from_str(trimmed).map_err(|e| {
                    ConfigError::Validation(format!(
                        "keybindings.{action_str}: invalid combo \"{combo_str}\": {e}"
                    ))
                })?;
            }
        }

        Ok(())
    }

    /// Build a [`BindingMap`] from the default bindings plus any user overrides
    /// specified in `[keybindings]`.
    ///
    /// # Errors
    ///
    /// Returns `ConfigError::Validation` if any override action name or combo
    /// string is invalid. (This should not happen if `validate()` has already
    /// been called, but the method is defensive.)
    pub fn build_binding_map(&self) -> Result<BindingMap, ConfigError> {
        let mut map = BindingMap::default();
        map.apply_overrides(&self.keybindings.overrides)
            .map_err(|e| ConfigError::Validation(format!("keybindings: {e}")))?;
        Ok(map)
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

    if let Some(path) = explicit_path {
        // Explicit --config: use ONLY this file on top of defaults.
        // Skip system, user, and env-var layers so the file is fully
        // isolated (no contamination from e.g. a home-manager managed config).
        let partial = load_partial(path)?;
        cfg.apply_partial(partial);
    } else {
        // Normal layered loading: system → user → env var.

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

/// Check whether the effective config file is writable.
///
/// If `explicit_path` is `Some`, that path is tested. Otherwise the
/// platform-specific user config path is resolved and tested.
///
/// Returns `true` when the file either does not yet exist (it can be created)
/// or exists and is writable.  Returns `false` when the file exists but
/// cannot be opened for writing, or when the config directory cannot be
/// determined.
#[must_use]
pub fn config_is_writable(explicit_path: Option<&Path>) -> bool {
    let path = match explicit_path {
        Some(p) => p.to_path_buf(),
        None => match user_config_path() {
            Some(p) => p,
            None => return false, // Can't determine config path — treat as non-writable.
        },
    };

    if !path.exists() {
        // File doesn't exist yet.  Check whether the parent directory is
        // writable (i.e. we could create the file).
        return path
            .parent()
            .is_some_and(|parent| !parent.exists() || is_dir_writable(parent));
    }

    // File exists — try opening it for writing (append mode so we don't
    // truncate).
    fs::OpenOptions::new().append(true).open(&path).is_ok()
}

/// Returns `true` when `dir` exists and we can write to it.
fn is_dir_writable(dir: &Path) -> bool {
    // Try creating and immediately removing a probe file.
    let probe = dir.join(".freminal_write_probe");
    if fs::write(&probe, b"").is_ok() {
        let _ = fs::remove_file(&probe);
        true
    } else {
        false
    }
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
// `missing_const_for_fn`: `PathBuf::from()` is not `const`, so the Linux branch prevents
// making this `const` even though the non-Linux branch would qualify.
// `unnecessary_wraps`: the `Option` is necessary for the shared return type across cfg branches;
// the non-Linux arm returns `None` by design, not by accident.
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
#[must_use]
pub(crate) fn user_config_path() -> Option<PathBuf> {
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn font_config_default_ligatures_true() {
        let cfg = FontConfig::default();
        assert!(cfg.ligatures, "ligatures should default to true");
    }

    #[test]
    fn font_config_deserialize_ligatures_true() {
        let toml_str = r"
[font]
size = 14.0
ligatures = true
";
        let partial: ConfigPartial = toml::from_str(toml_str).expect("valid TOML should parse");
        let font = partial.font.expect("font section should be present");
        assert!(font.ligatures);
    }

    #[test]
    fn font_config_deserialize_ligatures_false() {
        let toml_str = r"
[font]
size = 14.0
ligatures = false
";
        let partial: ConfigPartial = toml::from_str(toml_str).expect("valid TOML should parse");
        let font = partial.font.expect("font section should be present");
        assert!(!font.ligatures);
    }

    #[test]
    fn font_config_missing_ligatures_defaults_true() {
        // Backward compatibility: old config files without `ligatures` field
        // should default to true.
        let toml_str = r"
[font]
size = 14.0
";
        let partial: ConfigPartial = toml::from_str(toml_str).expect("valid TOML should parse");
        let font = partial.font.expect("font section should be present");
        assert!(
            font.ligatures,
            "missing ligatures field should default to true"
        );
    }

    #[test]
    fn full_config_default_has_ligatures_true() {
        let cfg = Config::default();
        assert!(cfg.font.ligatures);
    }

    #[test]
    fn config_roundtrip_preserves_ligatures() {
        let mut cfg = Config::default();
        cfg.font.ligatures = false;

        let toml_str = toml::to_string_pretty(&cfg).expect("Config should serialize to TOML");
        let deserialized: Config =
            toml::from_str(&toml_str).expect("serialized TOML should round-trip");
        assert!(!deserialized.font.ligatures);
    }

    #[test]
    fn validate_rejects_unknown_theme_slug() {
        let mut cfg = Config::default();
        cfg.theme.name = "nonexistent-theme".to_string();
        let err = cfg.validate().unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("nonexistent-theme"),
            "error should mention the bad slug: {msg}"
        );
    }

    #[test]
    fn validate_accepts_all_builtin_themes() {
        for theme in themes::all_themes() {
            let mut cfg = Config::default();
            cfg.theme.name = theme.slug.to_string();
            cfg.validate()
                .unwrap_or_else(|e| panic!("theme '{}' should be valid: {e}", theme.slug));
        }
    }

    // -----------------------------------------------------------------
    //  managed_by
    // -----------------------------------------------------------------

    #[test]
    fn default_config_managed_by_is_none() {
        let cfg = Config::default();
        assert!(cfg.managed_by.is_none());
        assert!(!cfg.is_managed());
    }

    #[test]
    fn managed_by_deserializes_from_toml() {
        let toml_str = r#"
version = 1
managed_by = "home-manager"
"#;
        let partial: ConfigPartial = toml::from_str(toml_str).expect("valid TOML should parse");
        assert_eq!(partial.managed_by.as_deref(), Some("home-manager"));
    }

    #[test]
    fn managed_by_absent_in_toml_defaults_to_none() {
        let toml_str = "version = 1\n";
        let partial: ConfigPartial = toml::from_str(toml_str).expect("valid TOML should parse");
        assert!(partial.managed_by.is_none());
    }

    #[test]
    fn managed_by_applied_via_partial() {
        let mut cfg = Config::default();
        assert!(!cfg.is_managed());

        let toml_str = r#"managed_by = "nix""#;
        let partial: ConfigPartial = toml::from_str(toml_str).expect("valid TOML");
        cfg.apply_partial(partial);
        assert!(cfg.is_managed());
        assert_eq!(cfg.managed_by.as_deref(), Some("nix"));
    }

    #[test]
    fn managed_by_not_serialized_when_none() {
        let cfg = Config::default();
        let toml_str = toml::to_string_pretty(&cfg).expect("should serialize");
        assert!(
            !toml_str.contains("managed_by"),
            "managed_by should be omitted when None: {toml_str}"
        );
    }

    #[test]
    fn managed_by_round_trips_when_set() {
        let cfg = Config {
            managed_by: Some("home-manager".to_string()),
            ..Config::default()
        };

        let toml_str = toml::to_string_pretty(&cfg).expect("should serialize");
        assert!(toml_str.contains("managed_by"));

        let deserialized: Config = toml::from_str(&toml_str).expect("should deserialize");
        assert_eq!(deserialized.managed_by.as_deref(), Some("home-manager"));
    }

    // -----------------------------------------------------------------
    //  config_is_writable
    // -----------------------------------------------------------------

    #[test]
    fn config_is_writable_for_nonexistent_file_in_writable_dir() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let path = dir.path().join("nonexistent.toml");
        assert!(config_is_writable(Some(&path)));
    }

    #[test]
    fn config_is_writable_for_existing_writable_file() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let path = dir.path().join("writable.toml");
        std::fs::write(&path, "version = 1\n").expect("write");
        assert!(config_is_writable(Some(&path)));
    }

    #[cfg(unix)]
    #[test]
    fn config_is_not_writable_for_readonly_file() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let path = dir.path().join("readonly.toml");
        std::fs::write(&path, "version = 1\n").expect("write");

        // Make the file read-only.
        let perms = std::fs::Permissions::from_mode(0o444);
        std::fs::set_permissions(&path, perms).expect("set permissions");

        assert!(!config_is_writable(Some(&path)));
    }

    // -----------------------------------------------------------------
    //  background_opacity
    // -----------------------------------------------------------------

    #[test]
    fn default_config_has_opacity_one() {
        let cfg = Config::default();
        assert!(
            (cfg.ui.background_opacity - 1.0).abs() < f32::EPSILON,
            "default background_opacity should be 1.0"
        );
    }

    #[test]
    fn validate_accepts_opacity_zero() {
        let mut cfg = Config::default();
        cfg.ui.background_opacity = 0.0;
        cfg.validate().expect("opacity 0.0 should be valid");
    }

    #[test]
    fn validate_accepts_opacity_half() {
        let mut cfg = Config::default();
        cfg.ui.background_opacity = 0.5;
        cfg.validate().expect("opacity 0.5 should be valid");
    }

    #[test]
    fn validate_accepts_opacity_one() {
        let mut cfg = Config::default();
        cfg.ui.background_opacity = 1.0;
        cfg.validate().expect("opacity 1.0 should be valid");
    }

    #[test]
    fn validate_rejects_opacity_negative() {
        let mut cfg = Config::default();
        cfg.ui.background_opacity = -0.1;
        let err = cfg.validate().unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("background_opacity"),
            "error should mention background_opacity: {msg}"
        );
    }

    #[test]
    fn validate_rejects_opacity_above_one() {
        let mut cfg = Config::default();
        cfg.ui.background_opacity = 1.1;
        let err = cfg.validate().unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("background_opacity"),
            "error should mention background_opacity: {msg}"
        );
    }

    #[test]
    fn validate_rejects_opacity_two() {
        let mut cfg = Config::default();
        cfg.ui.background_opacity = 2.0;
        let err = cfg.validate().unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("background_opacity"),
            "error should mention background_opacity: {msg}"
        );
    }

    #[test]
    fn opacity_roundtrip_preserves_value() {
        let mut cfg = Config::default();
        cfg.ui.background_opacity = 0.7;

        let toml_str = toml::to_string_pretty(&cfg).expect("Config should serialize");
        let deserialized: Config =
            toml::from_str(&toml_str).expect("serialized TOML should round-trip");
        assert!(
            (deserialized.ui.background_opacity - 0.7).abs() < f32::EPSILON,
            "background_opacity should round-trip: got {}",
            deserialized.ui.background_opacity
        );
    }

    #[test]
    fn missing_opacity_in_toml_defaults_to_one() {
        // Backward compatibility: old config files without background_opacity
        // should default to 1.0 (fully opaque).
        let toml_str = r"
[ui]
hide_menu_bar = false
";
        let partial: ConfigPartial = toml::from_str(toml_str).expect("valid TOML should parse");
        let ui = partial.ui.expect("ui section should be present");
        assert!(
            (ui.background_opacity - 1.0).abs() < f32::EPSILON,
            "missing background_opacity should default to 1.0"
        );
    }

    // -----------------------------------------------------------------
    //  keybindings
    // -----------------------------------------------------------------

    #[test]
    fn default_config_keybindings_is_empty() {
        let cfg = Config::default();
        assert!(cfg.keybindings.is_empty());
    }

    #[test]
    fn keybindings_deserialize_from_toml() {
        let toml_str = r#"
[keybindings]
copy = "Ctrl+C"
paste = "Ctrl+V"
"#;
        let partial: ConfigPartial = toml::from_str(toml_str).expect("valid TOML should parse");
        let kb = partial
            .keybindings
            .expect("keybindings section should be present");
        assert_eq!(kb.overrides.get("copy").unwrap(), "Ctrl+C");
        assert_eq!(kb.overrides.get("paste").unwrap(), "Ctrl+V");
    }

    #[test]
    fn keybindings_absent_in_toml_defaults_to_empty() {
        let toml_str = "version = 1\n";
        let partial: ConfigPartial = toml::from_str(toml_str).expect("valid TOML should parse");
        // When the section is absent, serde default gives us None.
        assert!(
            partial.keybindings.is_none()
                || partial
                    .keybindings
                    .as_ref()
                    .is_some_and(KeybindingsConfig::is_empty),
            "absent keybindings section should be None or empty"
        );
    }

    #[test]
    fn keybindings_applied_via_partial() {
        let mut cfg = Config::default();
        assert!(cfg.keybindings.is_empty());

        let toml_str = r#"
[keybindings]
copy = "Ctrl+C"
"#;
        let partial: ConfigPartial = toml::from_str(toml_str).expect("valid TOML");
        cfg.apply_partial(partial);
        assert_eq!(cfg.keybindings.overrides.get("copy").unwrap(), "Ctrl+C");
    }

    #[test]
    fn keybindings_partial_merge_is_additive() {
        let mut cfg = Config::default();

        let toml1 = r#"
[keybindings]
copy = "Ctrl+C"
"#;
        let partial1: ConfigPartial = toml::from_str(toml1).expect("valid TOML");
        cfg.apply_partial(partial1);

        let toml2 = r#"
[keybindings]
paste = "Ctrl+V"
"#;
        let partial2: ConfigPartial = toml::from_str(toml2).expect("valid TOML");
        cfg.apply_partial(partial2);

        assert_eq!(cfg.keybindings.overrides.len(), 2);
        assert_eq!(cfg.keybindings.overrides.get("copy").unwrap(), "Ctrl+C");
        assert_eq!(cfg.keybindings.overrides.get("paste").unwrap(), "Ctrl+V");
    }

    #[test]
    fn keybindings_partial_merge_overwrites() {
        let mut cfg = Config::default();

        let toml1 = r#"
[keybindings]
copy = "Ctrl+C"
"#;
        let partial1: ConfigPartial = toml::from_str(toml1).expect("valid TOML");
        cfg.apply_partial(partial1);

        let toml2 = r#"
[keybindings]
copy = "Ctrl+Shift+C"
"#;
        let partial2: ConfigPartial = toml::from_str(toml2).expect("valid TOML");
        cfg.apply_partial(partial2);

        assert_eq!(
            cfg.keybindings.overrides.get("copy").unwrap(),
            "Ctrl+Shift+C"
        );
    }

    #[test]
    fn keybindings_not_serialized_when_empty() {
        let cfg = Config::default();
        let toml_str = toml::to_string_pretty(&cfg).expect("should serialize");
        assert!(
            !toml_str.contains("keybindings"),
            "keybindings should be omitted when empty: {toml_str}"
        );
    }

    #[test]
    fn keybindings_roundtrip_when_set() {
        let mut cfg = Config::default();
        cfg.keybindings
            .overrides
            .insert("copy".to_string(), "Ctrl+C".to_string());
        cfg.keybindings
            .overrides
            .insert("new_tab".to_string(), "none".to_string());

        let toml_str = toml::to_string_pretty(&cfg).expect("should serialize");
        assert!(
            toml_str.contains("keybindings"),
            "keybindings should appear when non-empty"
        );

        let deserialized: Config = toml::from_str(&toml_str).expect("should deserialize");
        assert_eq!(
            deserialized.keybindings.overrides.get("copy").unwrap(),
            "Ctrl+C"
        );
        assert_eq!(
            deserialized.keybindings.overrides.get("new_tab").unwrap(),
            "none"
        );
    }

    #[test]
    fn validate_accepts_valid_keybinding_overrides() {
        let mut cfg = Config::default();
        cfg.keybindings
            .overrides
            .insert("copy".to_string(), "Ctrl+C".to_string());
        cfg.keybindings
            .overrides
            .insert("paste".to_string(), "none".to_string());
        cfg.keybindings
            .overrides
            .insert("new_tab".to_string(), String::new());
        cfg.validate().expect("valid overrides should pass");
    }

    #[test]
    fn validate_rejects_unknown_action_name() {
        let mut cfg = Config::default();
        cfg.keybindings
            .overrides
            .insert("launch_rockets".to_string(), "Ctrl+R".to_string());
        let err = cfg.validate().unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("launch_rockets"),
            "error should mention the bad action: {msg}"
        );
    }

    #[test]
    fn validate_rejects_invalid_combo_string() {
        let mut cfg = Config::default();
        cfg.keybindings
            .overrides
            .insert("copy".to_string(), "Super+C".to_string());
        let err = cfg.validate().unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("Super"),
            "error should mention the bad modifier: {msg}"
        );
    }

    #[test]
    fn build_binding_map_default_when_no_overrides() {
        use crate::keybindings::{BindingKey, BindingModifiers, KeyAction, KeyCombo};

        let cfg = Config::default();
        let map = cfg.build_binding_map().expect("should build default map");

        // Verify a default binding is present.
        let combo = KeyCombo::new(BindingKey::C, BindingModifiers::CTRL_SHIFT);
        assert_eq!(map.lookup(&combo), Some(KeyAction::Copy));
    }

    #[test]
    fn build_binding_map_with_override() {
        use crate::keybindings::{BindingKey, BindingModifiers, KeyAction, KeyCombo};

        let mut cfg = Config::default();
        cfg.keybindings
            .overrides
            .insert("copy".to_string(), "Ctrl+C".to_string());
        let map = cfg
            .build_binding_map()
            .expect("should build map with override");

        // The override Ctrl+C should now trigger Copy.
        let new_combo = KeyCombo::new(BindingKey::C, BindingModifiers::CTRL);
        assert_eq!(map.lookup(&new_combo), Some(KeyAction::Copy));
    }

    #[test]
    fn build_binding_map_with_none_unbinds() {
        use crate::keybindings::KeyAction;

        let mut cfg = Config::default();
        cfg.keybindings
            .overrides
            .insert("copy".to_string(), "none".to_string());
        let map = cfg.build_binding_map().expect("should build map");

        // Copy should have no bindings.
        assert!(map.all_combos_for(KeyAction::Copy).is_empty());
    }

    // -----------------------------------------------------------------
    //  tabs config
    // -----------------------------------------------------------------

    #[test]
    fn default_tabs_config() {
        let cfg = Config::default();
        assert!(!cfg.tabs.show_single_tab);
        assert_eq!(cfg.tabs.position, TabBarPosition::Top);
    }

    #[test]
    fn tabs_config_deserialize_from_toml() {
        let toml_str = r#"
[tabs]
show_single_tab = true
position = "bottom"
"#;
        let partial: ConfigPartial = toml::from_str(toml_str).expect("valid TOML should parse");
        let tabs = partial.tabs.expect("tabs section should be present");
        assert!(tabs.show_single_tab);
        assert_eq!(tabs.position, TabBarPosition::Bottom);
    }

    #[test]
    fn tabs_config_missing_defaults_correctly() {
        let toml_str = "version = 1\n";
        let partial: ConfigPartial = toml::from_str(toml_str).expect("valid TOML should parse");
        // When the section is absent, serde default gives us None.
        assert!(partial.tabs.is_none());
    }

    #[test]
    fn tabs_config_applied_via_partial() {
        let mut cfg = Config::default();
        assert!(!cfg.tabs.show_single_tab);

        let toml_str = r#"
[tabs]
show_single_tab = true
position = "bottom"
"#;
        let partial: ConfigPartial = toml::from_str(toml_str).expect("valid TOML");
        cfg.apply_partial(partial);
        assert!(cfg.tabs.show_single_tab);
        assert_eq!(cfg.tabs.position, TabBarPosition::Bottom);
    }

    #[test]
    fn tabs_config_roundtrip() {
        let mut cfg = Config::default();
        cfg.tabs.show_single_tab = true;
        cfg.tabs.position = TabBarPosition::Bottom;

        let toml_str = toml::to_string_pretty(&cfg).expect("Config should serialize");
        let deserialized: Config =
            toml::from_str(&toml_str).expect("serialized TOML should round-trip");
        assert!(deserialized.tabs.show_single_tab);
        assert_eq!(deserialized.tabs.position, TabBarPosition::Bottom);
    }

    #[test]
    fn tabs_config_partial_fields_default_when_missing() {
        // Backward compatibility: old config files without tabs section
        // should default to show_single_tab=false, position=top.
        let toml_str = r"
[tabs]
show_single_tab = true
";
        let partial: ConfigPartial = toml::from_str(toml_str).expect("valid TOML should parse");
        let tabs = partial.tabs.expect("tabs section should be present");
        assert!(tabs.show_single_tab);
        assert_eq!(
            tabs.position,
            TabBarPosition::Top,
            "missing position should default to Top"
        );
    }

    // ── Bell config tests ────────────────────────────────────────────

    #[test]
    fn bell_config_defaults_to_visual() {
        let cfg = BellConfig::default();
        assert_eq!(
            cfg.mode,
            BellMode::Visual,
            "bell mode should default to Visual"
        );
    }

    #[test]
    fn bell_config_deserialize_none() {
        let toml_str = r#"
[bell]
mode = "none"
"#;
        let partial: ConfigPartial = toml::from_str(toml_str).expect("valid TOML should parse");
        let bell = partial.bell.expect("bell section should be present");
        assert_eq!(bell.mode, BellMode::None);
    }

    #[test]
    fn bell_config_apply_partial() {
        let mut cfg = Config::default();
        assert_eq!(cfg.bell.mode, BellMode::Visual);

        let partial: ConfigPartial = toml::from_str(
            r#"
[bell]
mode = "none"
"#,
        )
        .expect("valid TOML");
        cfg.apply_partial(partial);
        assert_eq!(cfg.bell.mode, BellMode::None);
    }

    #[test]
    fn bell_config_roundtrip() {
        let mut cfg = Config::default();
        cfg.bell.mode = BellMode::None;

        let toml_str = toml::to_string_pretty(&cfg).expect("Config should serialize");
        let deserialized: Config =
            toml::from_str(&toml_str).expect("serialized TOML should round-trip");
        assert_eq!(deserialized.bell.mode, BellMode::None);
    }

    // ── Security config tests ────────────────────────────────────────

    #[test]
    fn security_config_defaults_to_deny_clipboard_read() {
        let cfg = SecurityConfig::default();
        assert!(
            !cfg.allow_clipboard_read,
            "clipboard read should default to false for security"
        );
    }

    #[test]
    fn security_config_password_indicator_defaults_to_true() {
        let cfg = SecurityConfig::default();
        assert!(
            cfg.password_indicator,
            "password_indicator should default to true"
        );
    }

    #[test]
    fn security_config_deserialize_allow() {
        let toml_str = r"
[security]
allow_clipboard_read = true
";
        let partial: ConfigPartial = toml::from_str(toml_str).expect("valid TOML should parse");
        let security = partial
            .security
            .expect("security section should be present");
        assert!(security.allow_clipboard_read);
        // password_indicator should still be its default (true) when omitted.
        assert!(security.password_indicator);
    }

    #[test]
    fn security_config_deserialize_password_indicator_disabled() {
        let toml_str = r"
[security]
password_indicator = false
";
        let partial: ConfigPartial = toml::from_str(toml_str).expect("valid TOML should parse");
        let security = partial
            .security
            .expect("security section should be present");
        assert!(!security.password_indicator);
        // allow_clipboard_read should still be its default (false) when omitted.
        assert!(!security.allow_clipboard_read);
    }

    #[test]
    fn security_config_apply_partial() {
        let mut cfg = Config::default();
        assert!(!cfg.security.allow_clipboard_read);

        let partial: ConfigPartial = toml::from_str(
            r"
[security]
allow_clipboard_read = true
",
        )
        .expect("valid TOML");
        cfg.apply_partial(partial);
        assert!(cfg.security.allow_clipboard_read);
    }

    #[test]
    fn security_config_apply_partial_password_indicator() {
        let mut cfg = Config::default();
        assert!(cfg.security.password_indicator);

        let partial: ConfigPartial = toml::from_str(
            r"
[security]
password_indicator = false
",
        )
        .expect("valid TOML");
        cfg.apply_partial(partial);
        assert!(!cfg.security.password_indicator);
    }

    #[test]
    fn security_config_roundtrip() {
        let mut cfg = Config::default();
        cfg.security.allow_clipboard_read = true;

        let toml_str = toml::to_string_pretty(&cfg).expect("Config should serialize");
        let deserialized: Config =
            toml::from_str(&toml_str).expect("serialized TOML should round-trip");
        assert!(deserialized.security.allow_clipboard_read);
    }

    #[test]
    fn security_config_roundtrip_password_indicator() {
        let mut cfg = Config::default();
        cfg.security.password_indicator = false;

        let toml_str = toml::to_string_pretty(&cfg).expect("Config should serialize");
        let deserialized: Config =
            toml::from_str(&toml_str).expect("serialized TOML should round-trip");
        assert!(!deserialized.security.password_indicator);
    }

    // -----------------------------------------------------------------
    //  cursor trail config
    // -----------------------------------------------------------------

    #[test]
    fn cursor_config_defaults() {
        let cfg = CursorConfig::default();
        assert!(!cfg.trail, "trail should default to false");
        assert_eq!(
            cfg.trail_duration_ms, 150,
            "trail_duration_ms should default to 150"
        );
    }

    #[test]
    fn cursor_trail_deserialize_enabled() {
        let toml_str = r"
[cursor]
trail = true
trail_duration_ms = 200
";
        let partial: ConfigPartial = toml::from_str(toml_str).expect("valid TOML should parse");
        let cursor = partial.cursor.expect("cursor section should be present");
        assert!(cursor.trail);
        assert_eq!(cursor.trail_duration_ms, 200);
    }

    #[test]
    fn cursor_trail_missing_defaults_correctly() {
        // Backward compatibility: old config files without trail fields
        // should default to trail=false, trail_duration_ms=150.
        let toml_str = r#"
[cursor]
shape = "block"
blink = true
"#;
        let partial: ConfigPartial = toml::from_str(toml_str).expect("valid TOML should parse");
        let cursor = partial.cursor.expect("cursor section should be present");
        assert!(!cursor.trail, "missing trail field should default to false");
        assert_eq!(
            cursor.trail_duration_ms, 150,
            "missing trail_duration_ms should default to 150"
        );
    }

    #[test]
    fn cursor_trail_applied_via_partial() {
        let mut cfg = Config::default();
        assert!(!cfg.cursor.trail);

        let toml_str = r"
[cursor]
trail = true
trail_duration_ms = 250
";
        let partial: ConfigPartial = toml::from_str(toml_str).expect("valid TOML");
        cfg.apply_partial(partial);
        assert!(cfg.cursor.trail);
        assert_eq!(cfg.cursor.trail_duration_ms, 250);
    }

    #[test]
    fn cursor_trail_roundtrip() {
        let mut cfg = Config::default();
        cfg.cursor.trail = true;
        cfg.cursor.trail_duration_ms = 300;

        let toml_str = toml::to_string_pretty(&cfg).expect("Config should serialize");
        let deserialized: Config =
            toml::from_str(&toml_str).expect("serialized TOML should round-trip");
        assert!(deserialized.cursor.trail);
        assert_eq!(deserialized.cursor.trail_duration_ms, 300);
    }
}
