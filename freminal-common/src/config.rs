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
    pub shader: ShaderConfig,
    pub tabs: TabsConfig,
    pub tab_title: TabTitleConfig,
    pub bell: BellConfig,
    pub security: SecurityConfig,
    pub shell_integration: ShellIntegrationConfig,
    pub command_blocks: CommandBlocksConfig,
    pub notifications: NotificationsConfig,
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

    /// Startup and layout configuration.
    #[serde(default)]
    pub startup: StartupConfig,

    /// First-run onboarding state.
    #[serde(default)]
    pub onboarding: OnboardingConfig,
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
            shader: ShaderConfig::default(),
            tabs: TabsConfig::default(),
            tab_title: TabTitleConfig::default(),
            bell: BellConfig::default(),
            security: SecurityConfig::default(),
            shell_integration: ShellIntegrationConfig::default(),
            command_blocks: CommandBlocksConfig::default(),
            notifications: NotificationsConfig::default(),
            keybindings: KeybindingsConfig::default(),
            managed_by: None,
            startup: StartupConfig::default(),
            onboarding: OnboardingConfig::default(),
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
/// Controls how the terminal selects between a dark and light theme.
///
/// Serialized as a kebab-case string in TOML: `"dark"`, `"light"`, `"auto"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ThemeMode {
    /// Always use the dark theme (`dark_name`). Default.
    #[default]
    Dark,
    /// Always use the light theme (`light_name`).
    Light,
    /// Automatically follow the OS light/dark preference.
    /// Uses `dark_name` when the OS is in dark mode, `light_name` in light mode.
    Auto,
}

/// Theme configuration: which palette(s) to use and how to select between them.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemeConfig {
    /// Theme to use when `mode = "dark"` or as the dark variant when `mode = "auto"`.
    pub dark_name: String,

    /// Theme to use when `mode = "light"` or as the light variant when `mode = "auto"`.
    pub light_name: String,

    /// How to choose between `dark_name` and `light_name`.
    /// `"dark"` (default), `"light"`, or `"auto"` (follow OS preference).
    pub mode: ThemeMode,

    /// **Deprecated.** Legacy alias for `dark_name`.
    /// Supported for backward compatibility: a config file that only sets `name`
    /// will continue to work, with `name` used as the dark theme name.
    ///
    /// With the current deserialization model, explicit field presence is not
    /// tracked separately from default values. As a result, `name` is only
    /// ignored when `dark_name` differs from its default value
    /// (`"catppuccin-mocha"`). If `dark_name` is omitted or explicitly set to
    /// that default value, `name` will still take effect.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self {
            dark_name: "catppuccin-mocha".to_string(),
            light_name: "catppuccin-latte".to_string(),
            mode: ThemeMode::Dark,
            name: None,
        }
    }
}

impl ThemeConfig {
    /// Return the effective dark theme slug, accounting for the deprecated `name` alias.
    ///
    /// If the deprecated `name` field is set and `dark_name` is still its default value,
    /// the `name` field takes precedence (backward-compat for old config files).
    #[must_use]
    pub fn effective_dark_name(&self) -> &str {
        const DEFAULT_DARK: &str = "catppuccin-mocha";
        if let Some(ref legacy) = self.name
            && self.dark_name == DEFAULT_DARK
        {
            return legacy.as_str();
        }
        &self.dark_name
    }

    /// Return the active theme slug based on `mode` and the OS preference.
    ///
    /// `os_is_dark` reflects the current OS dark/light state; it is only
    /// consulted when `mode = "auto"`.
    #[must_use]
    pub fn active_slug(&self, os_is_dark: bool) -> &str {
        match self.mode {
            ThemeMode::Dark => self.effective_dark_name(),
            ThemeMode::Light => &self.light_name,
            ThemeMode::Auto => {
                if os_is_dark {
                    self.effective_dark_name()
                } else {
                    &self.light_name
                }
            }
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

// ── UI ────────────────────────────────────────────────────────────────────────

/// How to fit the background image within the terminal viewport.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum BackgroundImageMode {
    /// Stretch the image to fill the entire viewport, ignoring aspect ratio.
    Fill,
    /// Scale the image to fit within the viewport while preserving aspect ratio
    /// (letterboxed — empty areas show through to the terminal background).
    Fit,
    /// Scale the image to cover the entire viewport while preserving aspect
    /// ratio (excess is cropped). Default.
    #[default]
    Cover,
    /// Repeat the image in both dimensions (tile pattern).
    Tile,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    /// Hide the menu bar at the top of the window. Default: `false`.
    pub hide_menu_bar: bool,
    /// Background opacity (0.0 = fully transparent, 1.0 = fully opaque).
    /// Only affects the terminal and menu bar backgrounds; text and content
    /// remain fully opaque.
    pub background_opacity: f32,
    /// Path to a background image displayed behind the terminal grid.
    /// Supports PNG, JPEG, and WebP. `None` disables the background image.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background_image: Option<PathBuf>,
    /// How to fit the background image within the terminal viewport.
    /// `"fill"`, `"fit"`, `"cover"` (default), or `"tile"`.
    pub background_image_mode: BackgroundImageMode,
    /// Opacity of the background image (0.0–1.0). Applied on top of the
    /// image itself; `background_opacity` then layers over that. Default: `0.5`.
    pub background_image_opacity: f32,
    /// Automatically detect plain URLs (http/https/file/ftp/mailto) in
    /// terminal output and make them clickable, in addition to OSC 8
    /// hyperlinks. OSC 8 links always take precedence when they overlap
    /// with an auto-detected URL. Default: `true`.
    pub auto_detect_urls: bool,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            hide_menu_bar: false,
            background_opacity: 1.0,
            background_image: None,
            background_image_mode: BackgroundImageMode::Cover,
            background_image_opacity: 0.5,
            auto_detect_urls: true,
        }
    }
}

// ------------------------------------------------------------------------------------------------
//  Shader
// ------------------------------------------------------------------------------------------------

/// Configuration for user-supplied post-processing GLSL fragment shaders.
///
/// When `path` is set, the terminal is rendered to an offscreen framebuffer
/// and the user's fragment shader is applied as a fullscreen post-processing
/// pass.  When `path` is `None`, the terminal renders directly to the screen
/// with no overhead.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ShaderConfig {
    /// Path to a custom GLSL fragment shader for post-processing.
    ///
    /// The shader receives these uniforms:
    /// - `uniform sampler2D u_terminal` — the terminal framebuffer texture
    /// - `uniform vec2 u_resolution` — viewport size in pixels
    /// - `uniform float u_time` — elapsed time in seconds
    ///
    /// `None` disables the post-processing pass (default — no FBO overhead).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,

    /// When `true`, reload and recompile the shader automatically when the
    /// file on disk changes.  Default: `true`.
    pub hot_reload: bool,
}

impl Default for ShaderConfig {
    fn default() -> Self {
        Self {
            path: None,
            hot_reload: true,
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
//  Tab Title
// ------------------------------------------------------------------------------------------------

/// Precedence policy that decides what is shown in a tab label (and the
/// window title) when a tab has both a user-assigned custom name and a
/// shell-asserted OSC 0/1/2 title.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TabTitlePolicy {
    /// Show `"{custom}{separator}{osc}"` when both are present.  Default.
    #[default]
    Prefix,
    /// Show `"{osc}{separator}{custom}"` when both are present.
    Suffix,
    /// Show the custom name only; ignore the OSC title for display.
    CustomWins,
    /// Show the OSC title only; OSC events clear `custom_name`.
    OscWins,
}

/// Configuration for tab/window title precedence.
///
/// ```toml
/// [tab_title]
/// policy = "prefix"
/// separator = ": "
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TabTitleConfig {
    /// Precedence policy when a tab has both a custom name and an OSC title.
    ///
    /// - `"prefix"` — show `"{custom}: {osc}"` (default)
    /// - `"suffix"` — show `"{osc}: {custom}"`
    /// - `"custom_wins"` — show custom only
    /// - `"osc_wins"` — show osc only; OSC events clear `custom_name`
    pub policy: TabTitlePolicy,

    /// Separator used in the `prefix` and `suffix` policies.  Default: `": "`.
    pub separator: String,
}

impl Default for TabTitleConfig {
    fn default() -> Self {
        Self {
            policy: TabTitlePolicy::default(),
            separator: String::from(": "),
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
    /// Best-effort system beep via the native platform API (`\x07` to stderr
    /// on Linux, `NSBeep` on macOS, `MessageBeep` on Windows).  See
    /// `gui::platform::system_beep` for details.
    Audio,
    /// Both the visual flash and the system beep.
    Both,
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
//  Shell Integration
// ------------------------------------------------------------------------------------------------

/// Configuration for OSC 133 (FinalTerm/FTCS) shell integration.
///
/// Freminal sets `TERM_PROGRAM=freminal` and `TERM_PROGRAM_VERSION=<crate version>`
/// in the PTY environment so shell scripts can detect us.  When enabled,
/// freminal also injects the necessary env at PTY spawn time so the bundled
/// bash/zsh/fish integration scripts auto-load — no manual sourcing required.
/// The scripts emit OSC 133 A/B/C/D markers so the terminal can render
/// command blocks, gutters (Task 73), and notifications (Task 76).
///
/// ```toml
/// [shell_integration]
/// set_term_program = true
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ShellIntegrationConfig {
    /// When `true`, freminal sets `TERM_PROGRAM=freminal` and
    /// `TERM_PROGRAM_VERSION` in the PTY environment, and injects the env
    /// needed for spawn-time auto-loading of the bundled shell-integration
    /// scripts.
    ///
    /// Default: `true`.  Disable only if you have an external workflow that
    /// conflicts with either the inherited `TERM_PROGRAM` or with our shell
    /// hooks.
    pub set_term_program: bool,
}

impl Default for ShellIntegrationConfig {
    fn default() -> Self {
        Self {
            set_term_program: true,
        }
    }
}

// ------------------------------------------------------------------------------------------------
//  Command Blocks
// ------------------------------------------------------------------------------------------------

/// Width, in physical pixels, of the colored command-block gutter strip
/// itself when it is enabled (`GutterPosition::Left`).
///
/// This is only the painted bar.  The cell grid is shifted further right by
/// an additional [`COMMAND_BLOCK_GUTTER_PADDING_PX`] so glyphs do not sit
/// flush against the strip; see [`GutterPosition::total_inset_px`].
pub const COMMAND_BLOCK_GUTTER_WIDTH_PX: f32 = 4.0;

/// Padding, in physical pixels, between the colored gutter strip and the
/// first glyph column.  Gives the status bar visual breathing room without
/// widening the painted strip.
pub const COMMAND_BLOCK_GUTTER_PADDING_PX: f32 = 4.0;

/// Where the command-block status gutter is drawn relative to the terminal
/// cell grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum GutterPosition {
    /// Reserve a thin strip on the left edge of the terminal area and draw a
    /// per-command status color bar in it.  Default.
    #[default]
    Left,
    /// No gutter.  The full terminal width is used for cell content and no
    /// status bar is drawn.
    Off,
}

impl GutterPosition {
    /// The painted gutter-strip width in physical pixels for this position.
    ///
    /// [`GutterPosition::Off`] reserves zero width.
    #[must_use]
    pub const fn width_px(self) -> f32 {
        match self {
            Self::Left => COMMAND_BLOCK_GUTTER_WIDTH_PX,
            Self::Off => 0.0,
        }
    }

    /// The total left inset in physical pixels: the painted strip width plus
    /// the padding gap before the first glyph column.
    ///
    /// This is the single source of truth shared by the column-count
    /// computation (which subtracts it from the available width before
    /// dividing by the cell width) and the renderer (which shifts the cell
    /// grid right by the same amount).  Keeping both consumers tied to this
    /// one value is what guarantees the column count reported to the PTY
    /// always matches the rendered cell-grid width.
    ///
    /// [`GutterPosition::Off`] reserves zero inset so both consumers fall
    /// back to the full terminal width with no shift.
    #[must_use]
    pub const fn total_inset_px(self) -> f32 {
        match self {
            Self::Left => COMMAND_BLOCK_GUTTER_WIDTH_PX + COMMAND_BLOCK_GUTTER_PADDING_PX,
            Self::Off => 0.0,
        }
    }
}

/// Configuration for OSC 133 command-block visualization.
///
/// Command blocks group each shell command's prompt, input, and output into
/// a selectable unit.  This config controls whether blocks are populated in
/// the snapshot and surfaced to the GUI, and whether the per-command
/// duration overlay is shown.
///
/// ```toml
/// [command_blocks]
/// enabled = true
/// show_duration = true
/// duration_threshold_secs = 2.0
/// gutter = "left"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CommandBlocksConfig {
    /// Master switch.  When `false`, OSC 133 markers are still parsed
    /// (because `FtcsState` matters for other features) but `command_blocks`
    /// is left empty in snapshots and the GUI shows no command-aware
    /// affordances.  Default: `true`.
    pub enabled: bool,

    /// Display the duration of long-running commands next to the gutter
    /// (e.g. `"3s"`, `"2m15s"`, `"1h5m"`).  Sub-second commands always
    /// show as `"1s"`; durations are truncated to whole seconds —
    /// fractional and millisecond labels are never emitted.  Default:
    /// `true`.
    pub show_duration: bool,

    /// Minimum command duration (in seconds) before a duration label is
    /// rendered.  Below this threshold the label is suppressed to avoid
    /// flicker on fast commands.  Default: `2.0`.
    pub duration_threshold_secs: f32,

    /// Where the per-command status gutter is drawn.  `"left"` reserves a
    /// thin strip on the left edge of the terminal area; `"off"` disables
    /// the gutter entirely.  Default: `"left"`.
    pub gutter: GutterPosition,
}

impl Default for CommandBlocksConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            show_duration: true,
            duration_threshold_secs: 2.0,
            gutter: GutterPosition::Left,
        }
    }
}

// ------------------------------------------------------------------------------------------------
//  Notifications
// ------------------------------------------------------------------------------------------------

/// Where a notification of a given category is delivered.
///
/// Notifications can surface as an in-app toast, a desktop notification via
/// the system notification daemon, both, or — for the command-finished
/// category — a desktop notification only when freminal is unfocused
/// (falling back to a toast when focused).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum NotificationRouting {
    /// In-app toast only.
    Toast,
    /// Desktop notification only.
    System,
    /// Both an in-app toast and a desktop notification.
    Both,
    /// Desktop notification when freminal is unfocused; an in-app toast when
    /// it is focused.  Default for command-finished events.
    #[default]
    SystemWhenUnfocused,
}

impl NotificationRouting {
    /// Whether this routing dispatches an in-app toast given the current
    /// focus state.
    ///
    /// [`Self::SystemWhenUnfocused`] produces a toast only when freminal is
    /// focused (the desktop notification is reserved for the unfocused case).
    #[must_use]
    pub const fn wants_toast(self, focused: bool) -> bool {
        match self {
            Self::Toast | Self::Both => true,
            Self::SystemWhenUnfocused => focused,
            Self::System => false,
        }
    }

    /// Whether this routing dispatches a desktop notification given the
    /// current focus state.
    ///
    /// [`Self::SystemWhenUnfocused`] produces a desktop notification only when
    /// freminal is unfocused.
    #[must_use]
    pub const fn wants_system(self, focused: bool) -> bool {
        match self {
            Self::System | Self::Both => true,
            Self::SystemWhenUnfocused => !focused,
            Self::Toast => false,
        }
    }
}

/// Configuration for the notification system (Task 76).
///
/// Notifications are produced from three sources: OSC 9 (iTerm2/WezTerm)
/// text payloads, OSC 777 (`notify;TITLE;BODY`, urxvt) payloads, and OSC 133
/// `D` command-finished events.  Each category routes independently to an
/// in-app toast and/or a desktop notification per [`NotificationRouting`].
///
/// The system is opt-in: [`Self::enabled`] defaults to `false`.
///
/// ```toml
/// [notifications]
/// enabled = false
/// osc_9 = true
/// osc_777 = true
/// on_command_finished = true
/// command_finished_threshold_secs = 10.0
/// routing_error = "both"
/// routing_info = "toast"
/// routing_command_finished = "system_when_unfocused"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
// Four independent TOML config toggles (master switch + per-source enables).
// Each maps directly to a documented `[notifications]` key; collapsing them
// into an enum would distort the config schema and add noise.
#[allow(clippy::struct_excessive_bools)]
pub struct NotificationsConfig {
    /// Master switch for the notification system.  When `false`, no
    /// notifications of any kind are produced.  Default: `false` (opt-in).
    pub enabled: bool,

    /// When `true`, OSC 9 (iTerm2) text payloads create notifications.
    /// Default: `true`.
    pub osc_9: bool,

    /// When `true`, OSC 777 (`notify;TITLE;BODY`) payloads create
    /// notifications.  Default: `true`.
    pub osc_777: bool,

    /// When `true`, an OSC 133 `D` (command finished) event fires a
    /// notification.  Default: `true`.
    pub on_command_finished: bool,

    /// Minimum command duration (in seconds) before a command-finished
    /// notification fires.  Avoids spamming notifications for fast commands.
    /// Default: `10.0`.
    pub command_finished_threshold_secs: f32,

    /// Routing for error-category notifications.  Default:
    /// [`NotificationRouting::Both`].
    pub routing_error: NotificationRouting,

    /// Routing for informational notifications.  Default:
    /// [`NotificationRouting::Toast`].
    pub routing_info: NotificationRouting,

    /// Routing for command-finished notifications.  Default:
    /// [`NotificationRouting::SystemWhenUnfocused`].
    pub routing_command_finished: NotificationRouting,
}

impl Default for NotificationsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            osc_9: true,
            osc_777: true,
            on_command_finished: true,
            command_finished_threshold_secs: 10.0,
            routing_error: NotificationRouting::Both,
            routing_info: NotificationRouting::Toast,
            routing_command_finished: NotificationRouting::SystemWhenUnfocused,
        }
    }
}

// ------------------------------------------------------------------------------------------------
//  Startup / Layout
// ------------------------------------------------------------------------------------------------

/// Configuration for startup behaviour and the layout system.
///
/// ```toml
/// [startup]
/// # Load a named layout from ~/.config/freminal/layouts/<name>.toml on
/// # startup.  Can also be a full path.
/// layout = "dev"
///
/// # When true, save the current layout on exit and restore it on next launch.
/// # The layout is saved to ~/.config/freminal/layouts/last_session.toml.
/// # Defaults to true.
/// restore_last_session = true
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct StartupConfig {
    /// Layout name or path to load on startup.
    ///
    /// If this is a bare name (no path separators, no `.toml` extension), the
    /// layout file `~/.config/freminal/layouts/<name>.toml` is used.
    /// Otherwise it is treated as a file path.
    ///
    /// Overridden by the `--layout` CLI flag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<String>,

    /// When `true`, save the current layout topology on exit and restore it
    /// on next launch (unless `--layout` is given on the command line).
    ///
    /// The saved layout is written to
    /// `~/.config/freminal/layouts/last_session.toml`.
    ///
    /// Defaults to `true` — session restore is the expected behaviour for a
    /// daily-driver terminal. Users who prefer a clean session each launch
    /// can opt out by setting this to `false` in `config.toml`.
    #[serde(default = "default_restore_last_session")]
    pub restore_last_session: bool,
}

impl Default for StartupConfig {
    fn default() -> Self {
        Self {
            layout: None,
            restore_last_session: default_restore_last_session(),
        }
    }
}

/// Default value for [`StartupConfig::restore_last_session`]. Kept as a free
/// function so `#[serde(default = "...")]` can reference it.
const fn default_restore_last_session() -> bool {
    true
}

/// **Deprecated** first-run onboarding state (kept for backward
/// compatibility with older `config.toml` files).
///
/// Historically Freminal stored the "user has dismissed the welcome
/// overlay" flag here.  This was moved out of `config.toml` because
/// managed installs (NixOS home-manager, system-wide configs, dotfile
/// managers locking permissions) make `config.toml` read-only, and the
/// program could not record the dismissal — the overlay reappeared on
/// every launch.
///
/// The flag now lives in `state.toml` under
/// `$XDG_STATE_HOME/freminal/` (Linux) — see
/// [`crate::app_state::AppState`].  This struct is kept so old
/// `config.toml` files still parse; on first launch with a new binary,
/// a `[onboarding] first_run_complete = true` value is migrated into
/// the new state file and then ignored.
///
/// Users can re-open the overlay at any time via Help → Show Welcome.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct OnboardingConfig {
    /// **Deprecated** — see struct docs.  `true` once the user has seen
    /// and dismissed the first-run welcome overlay.  Defaults to `false`.
    /// New code reads/writes `AppState::first_run_complete` instead;
    /// this field is migrated forward on first launch.
    pub first_run_complete: bool,
}

/// Returns the platform-canonical layout library directory.
///
/// | Platform  | Path                                        |
/// |-----------|---------------------------------------------|
/// | Linux/BSD | `$XDG_CONFIG_HOME/freminal/layouts/`        |
/// | macOS     | `~/Library/Application Support/Freminal/layouts/` |
/// | Windows   | `%APPDATA%\Freminal\layouts\`               |
///
/// Returns the platform-appropriate directory where FREC v2 recordings
/// are stored by default.
///
/// - macOS: `~/Library/Application Support/Freminal/recordings`
/// - Windows: `%APPDATA%\Freminal\recordings`
/// - Linux/BSD: `$XDG_CONFIG_HOME/freminal/recordings` (typically
///   `~/.config/freminal/recordings`)
///
/// The directory is created if it does not yet exist. Returns `None` if
/// the base directories cannot be determined.
#[must_use]
pub fn recording_library_dir() -> Option<PathBuf> {
    let base = BaseDirs::new()?;

    #[cfg(target_os = "macos")]
    {
        let p = base.data_dir().join("Freminal").join("recordings");
        create_dir_if_missing(&p);
        return Some(p);
    }

    #[cfg(target_os = "windows")]
    {
        let p = base.data_dir().join("Freminal").join("recordings");
        create_dir_if_missing(&p);
        return Some(p);
    }

    // Linux / BSD
    #[cfg(any(
        target_os = "linux",
        target_os = "freebsd",
        target_os = "dragonfly",
        target_os = "openbsd",
        target_os = "netbsd"
    ))]
    {
        let p = base.config_dir().join("freminal").join("recordings");
        create_dir_if_missing(&p);
        return Some(p);
    }

    #[allow(unreachable_code)]
    None
}

/// Returns `None` if the base directories cannot be determined.
#[must_use]
pub fn layout_library_dir() -> Option<PathBuf> {
    let base = BaseDirs::new()?;

    #[cfg(target_os = "macos")]
    {
        let p = base.data_dir().join("Freminal").join("layouts");
        create_dir_if_missing(&p);
        return Some(p);
    }

    #[cfg(target_os = "windows")]
    {
        let p = base.data_dir().join("Freminal").join("layouts");
        create_dir_if_missing(&p);
        return Some(p);
    }

    // Linux / BSD
    #[cfg(any(
        target_os = "linux",
        target_os = "freebsd",
        target_os = "dragonfly",
        target_os = "openbsd",
        target_os = "netbsd"
    ))]
    {
        let p = base.config_dir().join("freminal").join("layouts");
        create_dir_if_missing(&p);
        return Some(p);
    }

    #[allow(unreachable_code)]
    None
}

/// Resolved shell-integration script directory, tagged with whether
/// Freminal owns the directory or whether it was provided read-only by
/// the packager.
///
/// Callers that want to copy the bundled scripts onto disk (i.e.
/// [`shell_integration::sync_to_disk`]) must only do so on the
/// [`UserWritable`](Self::UserWritable) variant — writing into a
/// packaging-provided path (e.g. `/usr/share/freminal/shell-integration/`)
/// would either fail or, worse, succeed and silently mutate files owned
/// by the system package manager.
///
/// Both variants borrow their path uniformly via [`Self::path`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellIntegrationDir {
    /// Per-user data directory.  Freminal owns it and may freely
    /// extract / overwrite the bundled scripts on every launch.
    UserWritable(PathBuf),

    /// Packaging-provided directory — resolved either from
    /// `$FREMINAL_RESOURCES_DIR` (treated as the resources root, with
    /// `shell-integration/` appended) or from a hit under
    /// `$XDG_DATA_DIRS` (e.g. `/usr/share/freminal/shell-integration/`).
    /// Freminal must not write to this directory; the packager is
    /// responsible for keeping the scripts in sync.
    PackagingProvided(PathBuf),
}

impl ShellIntegrationDir {
    /// Borrow the underlying directory path regardless of variant.
    #[must_use]
    pub fn path(&self) -> &Path {
        match self {
            Self::UserWritable(p) | Self::PackagingProvided(p) => p.as_path(),
        }
    }
}

/// Directory where Freminal looks up shell-integration scripts.
///
/// Resolution order:
///
/// 1. `$FREMINAL_RESOURCES_DIR` if set — used by packaging / Nix setups.
///    The env var names the resources *root*; `shell-integration/` is
///    appended automatically so the variable matches its name.  Returned
///    as [`ShellIntegrationDir::PackagingProvided`]; never created.
/// 2. Any directory in `$XDG_DATA_DIRS` that already contains a
///    `freminal/shell-integration/` subtree — supports system-wide
///    installs (e.g. `/usr/share`, `/usr/local/share`).  The first match
///    wins.  Returned as [`ShellIntegrationDir::PackagingProvided`].
/// 3. Platform-default per-user data directory:
///    - Linux/BSD:  `~/.config/freminal/shell-integration/`
///    - macOS:      `~/Library/Application Support/Freminal/shell-integration/`
///    - Windows:    `%APPDATA%\Freminal\shell-integration\`
///
///    Created on first call (via `create_dir_if_missing`) and returned
///    as [`ShellIntegrationDir::UserWritable`] so callers can extract
///    the bundled scripts into it.
///
/// Returns `None` only if base directories cannot be determined AND
/// none of the override paths above resolved.
#[must_use]
pub fn shell_integration_dir() -> Option<ShellIntegrationDir> {
    // 1. Explicit override via $FREMINAL_RESOURCES_DIR.
    //    The env var names the resources root, so we always append
    //    `shell-integration/`.  The packager owns this tree; we never
    //    create it or write into it.
    if let Ok(custom) = std::env::var("FREMINAL_RESOURCES_DIR")
        && !custom.is_empty()
    {
        let p = PathBuf::from(custom).join("shell-integration");
        return Some(ShellIntegrationDir::PackagingProvided(p));
    }

    // 2. System-wide install via $XDG_DATA_DIRS.  We only return a hit
    //    if the directory already exists — otherwise an unrelated XDG
    //    entry would shadow the per-user default.  These paths
    //    (typically `/usr/share/...`) are owned by the packager.
    if let Ok(xdg_data_dirs) = std::env::var("XDG_DATA_DIRS")
        && !xdg_data_dirs.is_empty()
    {
        for entry in xdg_data_dirs.split(':') {
            if entry.is_empty() {
                continue;
            }
            let candidate = PathBuf::from(entry)
                .join("freminal")
                .join("shell-integration");
            if candidate.is_dir() {
                return Some(ShellIntegrationDir::PackagingProvided(candidate));
            }
        }
    }

    // 3. Platform-default per-user data directory.  Freminal owns this
    //    location and re-extracts the bundled scripts on every launch.
    let base = BaseDirs::new()?;

    #[cfg(target_os = "macos")]
    {
        let p = base.data_dir().join("Freminal").join("shell-integration");
        create_dir_if_missing(&p);
        return Some(ShellIntegrationDir::UserWritable(p));
    }

    #[cfg(target_os = "windows")]
    {
        let p = base.data_dir().join("Freminal").join("shell-integration");
        create_dir_if_missing(&p);
        return Some(ShellIntegrationDir::UserWritable(p));
    }

    // Linux / BSD
    #[cfg(any(
        target_os = "linux",
        target_os = "freebsd",
        target_os = "dragonfly",
        target_os = "openbsd",
        target_os = "netbsd"
    ))]
    {
        let p = base.config_dir().join("freminal").join("shell-integration");
        create_dir_if_missing(&p);
        return Some(ShellIntegrationDir::UserWritable(p));
    }

    #[allow(unreachable_code)]
    None
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
    pub shader: Option<ShaderConfig>,
    pub tabs: Option<TabsConfig>,
    pub bell: Option<BellConfig>,
    pub security: Option<SecurityConfig>,
    pub keybindings: Option<KeybindingsConfig>,
    pub managed_by: Option<String>,
    pub startup: Option<StartupConfig>,
    pub onboarding: Option<OnboardingConfig>,
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
        if let Some(shader) = partial.shader {
            self.shader = shader;
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
        if let Some(startup) = partial.startup {
            self.startup = startup;
        }
        if let Some(onboarding) = partial.onboarding {
            self.onboarding = onboarding;
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

        if !(0.0..=1.0).contains(&self.ui.background_image_opacity) {
            return Err(ConfigError::Validation(format!(
                "ui.background_image_opacity={} out of allowed range (0.0–1.0)",
                self.ui.background_image_opacity
            )));
        }

        // Validate the effective dark theme slug (also covers the deprecated `name` alias).
        let dark_slug = self.theme.effective_dark_name();
        if themes::by_slug(dark_slug).is_none() {
            return Err(ConfigError::Validation(format!(
                "theme.dark_name (or legacy theme.name)=\"{dark_slug}\" is not a recognized theme slug"
            )));
        }

        if themes::by_slug(&self.theme.light_name).is_none() {
            return Err(ConfigError::Validation(format!(
                "theme.light_name=\"{}\" is not a recognized theme slug",
                self.theme.light_name
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

/// Serialize `config` to TOML for diff / dirty-check purposes.
///
/// Used by callers that need a canonical string representation of a
/// `Config` for equality comparison (e.g. the Settings Modal's
/// unsaved-changes guard).  Uses the compact `toml::to_string` form rather
/// than `to_string_pretty` because only byte-equality matters.  Returns an
/// empty string on serialization failure; callers treat an empty baseline
/// as "always dirty", which conservatively triggers the confirm prompt
/// rather than silently dropping edits.
#[must_use]
pub fn serialize_config_for_diff(config: &Config) -> String {
    toml::to_string(config).unwrap_or_default()
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

    /// Minimal wrapper used to assert the TOML key/value form of
    /// [`GutterPosition`] independent of the full [`Config`] document.
    #[derive(Serialize)]
    struct GutterToml {
        gutter: GutterPosition,
    }

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
    fn shell_integration_and_command_blocks_round_trip_through_toml() {
        let mut cfg = Config::default();
        cfg.shell_integration.set_term_program = false;
        cfg.command_blocks.enabled = false;
        cfg.command_blocks.show_duration = false;
        cfg.command_blocks.duration_threshold_secs = 5.5;
        cfg.command_blocks.gutter = GutterPosition::Off;

        let toml = toml::to_string_pretty(&cfg).expect("serialise default config");
        let parsed: Config = toml::from_str(&toml).expect("re-parse");

        assert!(!parsed.shell_integration.set_term_program);
        assert!(!parsed.command_blocks.enabled);
        assert!(!parsed.command_blocks.show_duration);
        assert!((parsed.command_blocks.duration_threshold_secs - 5.5_f32).abs() < f32::EPSILON);
        assert_eq!(parsed.command_blocks.gutter, GutterPosition::Off);
    }

    #[test]
    fn notifications_default_is_opt_in() {
        let cfg = NotificationsConfig::default();
        assert!(!cfg.enabled);
        assert!(cfg.osc_9);
        assert!(cfg.osc_777);
        assert!(cfg.on_command_finished);
        assert!((cfg.command_finished_threshold_secs - 10.0_f32).abs() < f32::EPSILON);
        assert_eq!(cfg.routing_error, NotificationRouting::Both);
        assert_eq!(cfg.routing_info, NotificationRouting::Toast);
        assert_eq!(
            cfg.routing_command_finished,
            NotificationRouting::SystemWhenUnfocused
        );
    }

    #[test]
    fn notifications_round_trip_through_toml() {
        let mut cfg = Config::default();
        cfg.notifications.enabled = true;
        cfg.notifications.osc_9 = false;
        cfg.notifications.osc_777 = false;
        cfg.notifications.on_command_finished = false;
        cfg.notifications.command_finished_threshold_secs = 3.5;
        cfg.notifications.routing_error = NotificationRouting::System;
        cfg.notifications.routing_info = NotificationRouting::Both;
        cfg.notifications.routing_command_finished = NotificationRouting::Toast;

        let toml = toml::to_string_pretty(&cfg).expect("serialise config");
        let parsed: Config = toml::from_str(&toml).expect("re-parse");

        assert!(parsed.notifications.enabled);
        assert!(!parsed.notifications.osc_9);
        assert!(!parsed.notifications.osc_777);
        assert!(!parsed.notifications.on_command_finished);
        assert!(
            (parsed.notifications.command_finished_threshold_secs - 3.5_f32).abs() < f32::EPSILON
        );
        assert_eq!(
            parsed.notifications.routing_error,
            NotificationRouting::System
        );
        assert_eq!(parsed.notifications.routing_info, NotificationRouting::Both);
        assert_eq!(
            parsed.notifications.routing_command_finished,
            NotificationRouting::Toast
        );
    }

    #[test]
    fn notification_routing_serializes_as_snake_case() {
        #[derive(Serialize)]
        struct RoutingToml {
            routing: NotificationRouting,
        }
        let cases = [
            (NotificationRouting::Toast, "routing = \"toast\""),
            (NotificationRouting::System, "routing = \"system\""),
            (NotificationRouting::Both, "routing = \"both\""),
            (
                NotificationRouting::SystemWhenUnfocused,
                "routing = \"system_when_unfocused\"",
            ),
        ];
        for (routing, expected) in cases {
            let toml = toml::to_string(&RoutingToml { routing }).expect("serialise");
            assert!(toml.contains(expected), "got: {toml}");
        }
    }

    #[test]
    fn notification_routing_dispatch_decisions() {
        // Toast: always toast, never system.
        assert!(NotificationRouting::Toast.wants_toast(true));
        assert!(NotificationRouting::Toast.wants_toast(false));
        assert!(!NotificationRouting::Toast.wants_system(true));
        assert!(!NotificationRouting::Toast.wants_system(false));

        // System: never toast, always system.
        assert!(!NotificationRouting::System.wants_toast(true));
        assert!(!NotificationRouting::System.wants_toast(false));
        assert!(NotificationRouting::System.wants_system(true));
        assert!(NotificationRouting::System.wants_system(false));

        // Both: always toast, always system.
        assert!(NotificationRouting::Both.wants_toast(true));
        assert!(NotificationRouting::Both.wants_system(false));

        // SystemWhenUnfocused: toast when focused, system when unfocused.
        assert!(NotificationRouting::SystemWhenUnfocused.wants_toast(true));
        assert!(!NotificationRouting::SystemWhenUnfocused.wants_toast(false));
        assert!(!NotificationRouting::SystemWhenUnfocused.wants_system(true));
        assert!(NotificationRouting::SystemWhenUnfocused.wants_system(false));
    }

    #[test]
    fn tab_title_defaults_to_prefix_with_colon_space_separator() {
        let cfg = TabTitleConfig::default();
        assert_eq!(cfg.policy, TabTitlePolicy::Prefix);
        assert_eq!(cfg.separator, ": ");
    }

    #[test]
    fn tab_title_round_trips_through_toml() {
        for policy in [
            TabTitlePolicy::Prefix,
            TabTitlePolicy::Suffix,
            TabTitlePolicy::CustomWins,
            TabTitlePolicy::OscWins,
        ] {
            let mut cfg = Config::default();
            cfg.tab_title.policy = policy;
            cfg.tab_title.separator = String::from(" | ");

            let toml = toml::to_string_pretty(&cfg).expect("serialise config");
            let parsed: Config = toml::from_str(&toml).expect("re-parse");

            assert_eq!(parsed.tab_title.policy, policy);
            assert_eq!(parsed.tab_title.separator, " | ");
        }
    }

    #[test]
    fn tab_title_policy_serializes_as_snake_case() {
        #[derive(Serialize)]
        struct PolicyToml {
            policy: TabTitlePolicy,
        }
        let cases = [
            (TabTitlePolicy::Prefix, "policy = \"prefix\""),
            (TabTitlePolicy::Suffix, "policy = \"suffix\""),
            (TabTitlePolicy::CustomWins, "policy = \"custom_wins\""),
            (TabTitlePolicy::OscWins, "policy = \"osc_wins\""),
        ];
        for (policy, expected) in cases {
            let toml = toml::to_string(&PolicyToml { policy }).expect("serialise");
            assert!(toml.contains(expected), "got: {toml}");
        }
    }

    #[test]
    fn gutter_position_defaults_to_left() {
        assert_eq!(CommandBlocksConfig::default().gutter, GutterPosition::Left);
    }

    #[test]
    fn gutter_position_serializes_as_kebab_case() {
        // "left"/"off" lowercase in TOML, per the documented config key.
        let toml = toml::to_string(&GutterToml {
            gutter: GutterPosition::Left,
        })
        .expect("serialise");
        assert!(toml.contains("gutter = \"left\""), "got: {toml}");
        let toml_off = toml::to_string(&GutterToml {
            gutter: GutterPosition::Off,
        })
        .expect("serialise");
        assert!(toml_off.contains("gutter = \"off\""), "got: {toml_off}");
    }

    #[test]
    fn gutter_position_width_px_matches_constant() {
        assert!(
            (GutterPosition::Left.width_px() - COMMAND_BLOCK_GUTTER_WIDTH_PX).abs() < f32::EPSILON
        );
        assert!(GutterPosition::Off.width_px().abs() < f32::EPSILON);
    }

    #[test]
    fn gutter_position_total_inset_includes_padding() {
        // The cell grid is shifted by strip width + padding; the painted
        // strip is just the strip width.
        let expected = COMMAND_BLOCK_GUTTER_WIDTH_PX + COMMAND_BLOCK_GUTTER_PADDING_PX;
        assert!((GutterPosition::Left.total_inset_px() - expected).abs() < f32::EPSILON);
        assert!(
            GutterPosition::Left.total_inset_px() > GutterPosition::Left.width_px(),
            "total inset must exceed the painted strip width (padding gap)"
        );
        assert!(GutterPosition::Off.total_inset_px().abs() < f32::EPSILON);
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
    fn validate_rejects_unknown_dark_theme_slug() {
        let mut cfg = Config::default();
        cfg.theme.dark_name = "nonexistent-theme".to_string();
        let err = cfg.validate().unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("nonexistent-theme"),
            "error should mention the bad slug: {msg}"
        );
    }

    #[test]
    fn validate_rejects_unknown_light_theme_slug() {
        let mut cfg = Config::default();
        cfg.theme.light_name = "nonexistent-theme".to_string();
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
            cfg.theme.dark_name = theme.slug.to_string();
            cfg.validate().unwrap_or_else(|e| {
                panic!("theme '{}' should be valid as dark_name: {e}", theme.slug)
            });
        }
        for theme in themes::all_themes() {
            let mut cfg = Config::default();
            cfg.theme.light_name = theme.slug.to_string();
            cfg.validate().unwrap_or_else(|e| {
                panic!("theme '{}' should be valid as light_name: {e}", theme.slug)
            });
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

    #[test]
    fn bell_config_deserialize_audio() {
        let toml_str = r#"
[bell]
mode = "audio"
"#;
        let partial: ConfigPartial = toml::from_str(toml_str).expect("valid TOML should parse");
        let bell = partial.bell.expect("bell section should be present");
        assert_eq!(bell.mode, BellMode::Audio);
    }

    #[test]
    fn bell_config_deserialize_both() {
        let toml_str = r#"
[bell]
mode = "both"
"#;
        let partial: ConfigPartial = toml::from_str(toml_str).expect("valid TOML should parse");
        let bell = partial.bell.expect("bell section should be present");
        assert_eq!(bell.mode, BellMode::Both);
    }

    #[test]
    fn bell_config_roundtrip_audio() {
        let mut cfg = Config::default();
        cfg.bell.mode = BellMode::Audio;

        let toml_str = toml::to_string_pretty(&cfg).expect("Config should serialize");
        let deserialized: Config =
            toml::from_str(&toml_str).expect("serialized TOML should round-trip");
        assert_eq!(deserialized.bell.mode, BellMode::Audio);
    }

    #[test]
    fn bell_config_roundtrip_both() {
        let mut cfg = Config::default();
        cfg.bell.mode = BellMode::Both;

        let toml_str = toml::to_string_pretty(&cfg).expect("Config should serialize");
        let deserialized: Config =
            toml::from_str(&toml_str).expect("serialized TOML should round-trip");
        assert_eq!(deserialized.bell.mode, BellMode::Both);
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

    // =================================================================
    //  Task 52 — Adaptive Light/Dark Theming (ThemeMode + ThemeConfig)
    // =================================================================

    // ── ThemeMode TOML serialization ─────────────────────────────────

    #[test]
    fn theme_mode_dark_serializes_to_kebab_case() {
        #[derive(Serialize, Deserialize)]
        struct Wrapper {
            mode: ThemeMode,
        }
        let w = Wrapper {
            mode: ThemeMode::Dark,
        };
        let s = toml::to_string(&w).expect("serialize");
        assert!(s.contains("mode = \"dark\""), "got: {s}");
    }

    #[test]
    fn theme_mode_light_serializes_to_kebab_case() {
        #[derive(Serialize, Deserialize)]
        struct Wrapper {
            mode: ThemeMode,
        }
        let w = Wrapper {
            mode: ThemeMode::Light,
        };
        let s = toml::to_string(&w).expect("serialize");
        assert!(s.contains("mode = \"light\""), "got: {s}");
    }

    #[test]
    fn theme_mode_auto_serializes_to_kebab_case() {
        #[derive(Serialize, Deserialize)]
        struct Wrapper {
            mode: ThemeMode,
        }
        let w = Wrapper {
            mode: ThemeMode::Auto,
        };
        let s = toml::to_string(&w).expect("serialize");
        assert!(s.contains("mode = \"auto\""), "got: {s}");
    }

    #[test]
    fn theme_mode_round_trips_all_variants() {
        for (input, expected) in [
            (r#"mode = "dark""#, ThemeMode::Dark),
            (r#"mode = "light""#, ThemeMode::Light),
            (r#"mode = "auto""#, ThemeMode::Auto),
        ] {
            #[derive(Serialize, Deserialize)]
            struct Wrapper {
                mode: ThemeMode,
            }
            let w: Wrapper = toml::from_str(input).expect("deserialize");
            assert_eq!(w.mode, expected, "for input: {input}");
        }
    }

    // ── ThemeConfig::active_slug ──────────────────────────────────────

    #[test]
    fn active_slug_dark_mode_always_returns_dark_name() {
        let cfg = ThemeConfig {
            dark_name: "solarized-dark".to_string(),
            light_name: "solarized-light".to_string(),
            mode: ThemeMode::Dark,
            name: None,
        };
        assert_eq!(cfg.active_slug(true), "solarized-dark");
        assert_eq!(cfg.active_slug(false), "solarized-dark");
    }

    #[test]
    fn active_slug_light_mode_always_returns_light_name() {
        let cfg = ThemeConfig {
            dark_name: "solarized-dark".to_string(),
            light_name: "solarized-light".to_string(),
            mode: ThemeMode::Light,
            name: None,
        };
        assert_eq!(cfg.active_slug(true), "solarized-light");
        assert_eq!(cfg.active_slug(false), "solarized-light");
    }

    #[test]
    fn active_slug_auto_mode_follows_os_preference() {
        let cfg = ThemeConfig {
            dark_name: "catppuccin-mocha".to_string(),
            light_name: "catppuccin-latte".to_string(),
            mode: ThemeMode::Auto,
            name: None,
        };
        assert_eq!(
            cfg.active_slug(true),
            "catppuccin-mocha",
            "os dark => dark_name"
        );
        assert_eq!(
            cfg.active_slug(false),
            "catppuccin-latte",
            "os light => light_name"
        );
    }

    // ── ThemeConfig::effective_dark_name (backward compat) ──────────

    #[test]
    fn effective_dark_name_returns_dark_name_when_no_legacy_name() {
        let cfg = ThemeConfig {
            dark_name: "dracula".to_string(),
            light_name: "catppuccin-latte".to_string(),
            mode: ThemeMode::Dark,
            name: None,
        };
        assert_eq!(cfg.effective_dark_name(), "dracula");
    }

    #[test]
    fn effective_dark_name_prefers_legacy_name_when_dark_name_is_default() {
        // Old config file: only set `name = "gruvbox-dark"`, not `dark_name`.
        let cfg = ThemeConfig {
            dark_name: "catppuccin-mocha".to_string(), // still the default
            light_name: "catppuccin-latte".to_string(),
            mode: ThemeMode::Dark,
            name: Some("gruvbox-dark".to_string()),
        };
        assert_eq!(
            cfg.effective_dark_name(),
            "gruvbox-dark",
            "legacy name field should take priority when dark_name is still default"
        );
    }

    #[test]
    fn effective_dark_name_prefers_dark_name_when_explicitly_overridden() {
        // Config with both `name` and `dark_name` set explicitly.
        let cfg = ThemeConfig {
            dark_name: "nord".to_string(), // explicitly overridden
            light_name: "catppuccin-latte".to_string(),
            mode: ThemeMode::Dark,
            name: Some("gruvbox-dark".to_string()),
        };
        assert_eq!(
            cfg.effective_dark_name(),
            "nord",
            "explicit dark_name should win over legacy name when dark_name differs from default"
        );
    }

    // ── active_slug + effective_dark_name backward compat ─────────────

    #[test]
    fn active_slug_dark_mode_with_legacy_name() {
        // Old config: name = "gruvbox-dark", no dark_name set.
        let cfg = ThemeConfig {
            dark_name: "catppuccin-mocha".to_string(), // default
            light_name: "catppuccin-latte".to_string(),
            mode: ThemeMode::Dark,
            name: Some("gruvbox-dark".to_string()),
        };
        assert_eq!(cfg.active_slug(true), "gruvbox-dark");
        assert_eq!(cfg.active_slug(false), "gruvbox-dark");
    }

    // ── ThemeConfig validation ─────────────────────────────────────────

    #[test]
    fn validate_accepts_valid_dark_and_light_names() {
        let cfg = {
            let mut c = Config::default();
            c.theme.dark_name = "catppuccin-mocha".to_string();
            c.theme.light_name = "catppuccin-latte".to_string();
            c.theme.mode = ThemeMode::Auto;
            c
        };
        cfg.validate()
            .expect("valid dark+light theme config should pass validation");
    }

    #[test]
    fn validate_rejects_invalid_light_name() {
        let cfg = {
            let mut c = Config::default();
            c.theme.light_name = "not-a-real-theme".to_string();
            c
        };
        let err = cfg
            .validate()
            .expect_err("invalid light name should fail validation");
        let msg = err.to_string();
        assert!(
            msg.contains("light_name"),
            "error message should mention light_name, got: {msg}"
        );
    }

    // ── ThemeMode default ─────────────────────────────────────────────

    #[test]
    fn theme_mode_default_is_dark() {
        assert_eq!(ThemeMode::default(), ThemeMode::Dark);
    }

    #[test]
    fn theme_config_default_has_dark_mode() {
        let cfg = ThemeConfig::default();
        assert_eq!(cfg.mode, ThemeMode::Dark);
        assert_eq!(cfg.dark_name, "catppuccin-mocha");
        assert_eq!(cfg.light_name, "catppuccin-latte");
        assert!(cfg.name.is_none());
    }

    // ── BackgroundImageMode ────────────────────────────────────────────

    #[test]
    fn background_image_mode_default_is_cover() {
        let cfg = UiConfig::default();
        assert_eq!(cfg.background_image_mode, BackgroundImageMode::Cover);
    }

    #[test]
    fn background_image_opacity_default_is_0_5() {
        let cfg = UiConfig::default();
        assert!((cfg.background_image_opacity - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn auto_detect_urls_default_is_true() {
        let cfg = UiConfig::default();
        assert!(cfg.auto_detect_urls);
    }

    #[test]
    fn auto_detect_urls_deserialize_false() {
        let toml = "auto_detect_urls = false";
        let cfg: UiConfig = toml::from_str(toml).expect("parse");
        assert!(!cfg.auto_detect_urls);
    }

    #[test]
    fn background_image_mode_deserialize_all_variants() {
        for (toml_val, expected) in [
            ("\"fill\"", BackgroundImageMode::Fill),
            ("\"fit\"", BackgroundImageMode::Fit),
            ("\"cover\"", BackgroundImageMode::Cover),
            ("\"tile\"", BackgroundImageMode::Tile),
        ] {
            let toml_str = format!("[ui]\nbackground_image_mode = {toml_val}\n");
            let partial: ConfigPartial = toml::from_str(&toml_str).expect("valid TOML");
            let ui = partial.ui.expect("ui section present");
            assert_eq!(
                ui.background_image_mode, expected,
                "mode {toml_val} should parse to {expected:?}"
            );
        }
    }

    #[test]
    fn background_image_opacity_out_of_range_fails_validation() {
        let mut cfg = Config::default();
        cfg.ui.background_image_opacity = 1.5;
        let result = cfg.validate();
        assert!(
            result.is_err(),
            "opacity 1.5 should fail validation, got: {result:?}"
        );
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("background_image_opacity"),
            "error should mention background_image_opacity, got: {msg}"
        );
    }

    #[test]
    fn background_image_opacity_at_boundaries_passes_validation() {
        for opacity in [0.0_f32, 1.0_f32] {
            let mut cfg = Config::default();
            cfg.ui.background_image_opacity = opacity;
            assert!(
                cfg.validate().is_ok(),
                "opacity {opacity} should pass validation"
            );
        }
    }

    // ── ShaderConfig ───────────────────────────────────────────────────

    #[test]
    fn shader_config_default_has_no_path_and_hot_reload_enabled() {
        let cfg = ShaderConfig::default();
        assert!(cfg.path.is_none(), "default shader path should be None");
        assert!(cfg.hot_reload, "hot_reload should default to true");
    }

    #[test]
    fn shader_config_deserialize_path_and_hot_reload() {
        let toml_str = r#"
[shader]
path = "/tmp/my.frag"
hot_reload = false
"#;
        let partial: ConfigPartial = toml::from_str(toml_str).expect("valid TOML");
        let shader = partial.shader.expect("shader section present");
        assert_eq!(
            shader.path.as_deref(),
            Some(std::path::Path::new("/tmp/my.frag"))
        );
        assert!(!shader.hot_reload);
    }

    #[test]
    fn shader_config_missing_hot_reload_defaults_true() {
        let toml_str = r#"
[shader]
path = "/tmp/my.frag"
"#;
        let partial: ConfigPartial = toml::from_str(toml_str).expect("valid TOML");
        let shader = partial.shader.expect("shader section present");
        assert!(
            shader.hot_reload,
            "missing hot_reload should default to true"
        );
    }

    #[test]
    fn config_default_shader_is_disabled() {
        let cfg = Config::default();
        assert!(
            cfg.shader.path.is_none(),
            "default config should have no shader"
        );
    }

    // --- apply_cli_overrides ---

    #[test]
    fn apply_cli_overrides_hide_menu_bar() {
        // Exercises line 589: `self.ui.hide_menu_bar = true` when hide_menu_bar=true.
        let mut cfg = Config::default();
        assert!(!cfg.ui.hide_menu_bar);
        cfg.apply_cli_overrides(None, None, true);
        assert!(cfg.ui.hide_menu_bar);
    }

    #[test]
    fn apply_cli_overrides_hide_menu_bar_false_does_not_change() {
        // hide_menu_bar=false must NOT override an already-set value.
        let mut cfg = Config::default();
        cfg.ui.hide_menu_bar = true; // already true
        cfg.apply_cli_overrides(None, None, false);
        assert!(
            cfg.ui.hide_menu_bar,
            "false should not clear a previously-set value"
        );
    }

    // --- file_log_level ---

    #[test]
    fn file_log_level_defaults_to_info_when_not_set() {
        // Exercises line 598: `unwrap_or("info")` when level is None.
        let cfg = Config::default();
        assert!(
            cfg.logging.level.is_none(),
            "default logging level should be None"
        );
        assert_eq!(cfg.file_log_level(), "info");
    }

    #[test]
    fn file_log_level_returns_configured_value() {
        let mut cfg = Config::default();
        cfg.logging.level = Some("debug".to_string());
        assert_eq!(cfg.file_log_level(), "debug");
    }

    // --- save_config / load_config with explicit path ---

    #[test]
    fn save_config_writes_to_explicit_path() {
        // Exercises lines 822-824: save writes TOML to a temp file.
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let path = dir.path().join("test_config.toml");
        let cfg = Config::default();
        save_config(&cfg, Some(&path)).expect("save_config should succeed");
        assert!(path.exists(), "config file should be created");
        let contents = std::fs::read_to_string(&path).expect("read");
        assert!(
            contents.contains("version"),
            "written TOML should contain version key"
        );
    }

    #[test]
    fn load_config_from_explicit_path() {
        // Exercises line 771 equivalent (explicit path loading via load_config).
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let path = dir.path().join("test_config.toml");
        let mut cfg_to_save = Config::default();
        cfg_to_save.font.size = 20.0;
        save_config(&cfg_to_save, Some(&path)).expect("save_config should succeed");

        let loaded = load_config(Some(&path)).expect("load_config should succeed");
        assert!((loaded.font.size - 20.0).abs() < f32::EPSILON);
    }

    /// RAII guard that sets an env var on creation and restores the previous
    /// value (or removes it) on drop — even if the test panics.
    struct EnvVarGuard {
        key: &'static str,
        prev: Option<std::ffi::OsString>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let prev = std::env::var_os(key);
            // SAFETY: test code — concurrent mutation of this env var
            // across tests is serialized via `ENV_LOCK`.
            unsafe {
                std::env::set_var(key, value);
            }
            Self { key, prev }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            unsafe {
                if let Some(ref v) = self.prev {
                    std::env::set_var(self.key, v);
                } else {
                    std::env::remove_var(self.key);
                }
            }
        }
    }

    /// Serializes tests that mutate process-wide env vars consulted by
    /// [`shell_integration_dir`] (`FREMINAL_RESOURCES_DIR`,
    /// `XDG_DATA_DIRS`).  Without this lock, two such tests running on
    /// concurrent harness threads would observe each other's mutations
    /// and flake non-deterministically.  `PoisonError` is intentionally
    /// ignored: a panicking test poisons the lock but the underlying
    /// guards still restore the env vars on unwind, so subsequent tests
    /// can safely proceed.
    static SHELL_INTEGRATION_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn load_config_via_freminal_config_env_var() {
        // Exercises lines 774-779: FREMINAL_CONFIG env var path.
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let path = dir.path().join("env_config.toml");
        let mut cfg_to_save = Config::default();
        cfg_to_save.font.size = 18.0;
        save_config(&cfg_to_save, Some(&path)).expect("save_config should succeed");

        // Guard restores the env var even if the test panics.
        let _guard = EnvVarGuard::set("FREMINAL_CONFIG", path.to_str().unwrap());
        let result = load_config(None);

        let loaded = result.expect("load_config with FREMINAL_CONFIG should succeed");
        assert!((loaded.font.size - 18.0).abs() < f32::EPSILON);
    }

    #[test]
    fn shell_integration_dir_is_some_on_supported_platforms() {
        // Serialize against the other env-var-touching tests below so
        // none of us observes a sibling test's transient mutations.
        let _lock = SHELL_INTEGRATION_ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        // Clear both packaging-related overrides so the resolver is
        // guaranteed to fall through to the per-user data dir (variant 3).
        // The empty-string guards still restore whatever the CI host had
        // set before the test ran.
        let _g_res = EnvVarGuard::set("FREMINAL_RESOURCES_DIR", "");
        let _g_xdg = EnvVarGuard::set("XDG_DATA_DIRS", "");

        let dir = shell_integration_dir();
        // On all CI-reachable platforms (Linux, macOS, Windows) a home
        // directory is always available, so `None` here would indicate a
        // broken environment rather than incorrect code.
        assert!(
            dir.is_some(),
            "shell_integration_dir() returned None; \
             check that a home directory is available in the test environment"
        );
        let dir = dir.unwrap();
        let path = match dir {
            ShellIntegrationDir::UserWritable(ref p) => p.clone(),
            ShellIntegrationDir::PackagingProvided(ref p) => {
                panic!(
                    "expected UserWritable when packaging env vars are cleared, \
                     got PackagingProvided({})",
                    p.display()
                );
            }
        };
        let mut components: Vec<_> = path.components().collect();

        // Last component must be "shell-integration".
        let last = components
            .pop()
            .expect("path must have at least 2 components");
        let last_str = last.as_os_str().to_string_lossy();
        assert_eq!(
            last_str, "shell-integration",
            "last path component should be 'shell-integration', got '{last_str}'"
        );

        // Second-to-last must be "freminal" (Linux/BSD) or "Freminal" (macOS/Windows).
        let second_last = components
            .pop()
            .expect("path must have at least 2 components");
        let second_last_str = second_last.as_os_str().to_string_lossy();
        #[cfg(any(
            target_os = "linux",
            target_os = "freebsd",
            target_os = "dragonfly",
            target_os = "openbsd",
            target_os = "netbsd"
        ))]
        assert_eq!(
            second_last_str, "freminal",
            "second-to-last path component should be 'freminal' on Linux/BSD, \
             got '{second_last_str}'"
        );
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        assert_eq!(
            second_last_str, "Freminal",
            "second-to-last path component should be 'Freminal' on macOS/Windows, \
             got '{second_last_str}'"
        );
    }

    #[test]
    fn shell_integration_dir_with_freminal_resources_dir_env_returns_packaging_provided() {
        // Regression for the Copilot PR-333 review finding that
        // `FREMINAL_RESOURCES_DIR` should be the resources *root* (so
        // `shell-integration/` is appended) and that the resulting
        // directory must be tagged as packaging-provided so the binary
        // never calls `sync_to_disk` against it.
        let _lock = SHELL_INTEGRATION_ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        let tmp = tempfile::tempdir().expect("tempdir");
        let _g_res = EnvVarGuard::set(
            "FREMINAL_RESOURCES_DIR",
            tmp.path().to_str().expect("tempdir path is utf-8"),
        );
        let _g_xdg = EnvVarGuard::set("XDG_DATA_DIRS", "");

        let resolved = shell_integration_dir().expect("expected Some when override is set");
        let expected = tmp.path().join("shell-integration");
        match resolved {
            ShellIntegrationDir::PackagingProvided(ref p) => {
                assert_eq!(
                    p, &expected,
                    "FREMINAL_RESOURCES_DIR should be treated as the resources root \
                     with 'shell-integration/' appended"
                );
            }
            ShellIntegrationDir::UserWritable(ref p) => {
                panic!(
                    "FREMINAL_RESOURCES_DIR should resolve to PackagingProvided, \
                     got UserWritable({})",
                    p.display()
                );
            }
        }

        // The packaging path must NOT be auto-created.  The packager owns
        // the layout; mkdir-ing into it would be writing to a directory
        // we don't own.
        assert!(
            !expected.exists(),
            "FREMINAL_RESOURCES_DIR override must not auto-create the \
             shell-integration subdir (it exists at {})",
            expected.display()
        );
    }

    #[test]
    fn shell_integration_dir_with_xdg_data_dirs_hit_returns_packaging_provided() {
        // Regression for the Copilot PR-333 review finding that an
        // XDG_DATA_DIRS hit (typically a system path like
        // `/usr/share/freminal/shell-integration/`) must be tagged
        // packaging-provided so the binary never tries to `sync_to_disk`
        // into a system-owned directory.
        let _lock = SHELL_INTEGRATION_ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        let tmp = tempfile::tempdir().expect("tempdir");
        let expected = tmp.path().join("freminal").join("shell-integration");
        std::fs::create_dir_all(&expected).expect("create xdg hit directory");

        let _g_res = EnvVarGuard::set("FREMINAL_RESOURCES_DIR", "");
        let _g_xdg = EnvVarGuard::set(
            "XDG_DATA_DIRS",
            tmp.path().to_str().expect("tempdir path is utf-8"),
        );

        let resolved = shell_integration_dir().expect("expected Some when XDG hit exists");
        match resolved {
            ShellIntegrationDir::PackagingProvided(ref p) => {
                assert_eq!(
                    p, &expected,
                    "XDG_DATA_DIRS hit should resolve to the discovered candidate path"
                );
            }
            ShellIntegrationDir::UserWritable(ref p) => {
                panic!(
                    "XDG_DATA_DIRS hit should resolve to PackagingProvided, \
                     got UserWritable({})",
                    p.display()
                );
            }
        }
    }
}
