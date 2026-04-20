// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use std::path::PathBuf;

use clap::Parser;

/// Freminal — a modern terminal emulator written in Rust
#[derive(Parser, Debug, Clone)]
#[command(name = "freminal", version, about)]
pub struct Args {
    /// Shell to run (overrides config file and default shell)
    #[arg(long)]
    pub shell: Option<String>,

    /// Show all debug output (disables default log filtering)
    #[arg(long = "show-all-debug")]
    pub show_all_debug: bool,

    /// Write logs to a file in the current directory.
    ///
    /// Accepts `--write-logs-to-file`, `--write-logs-to-file=true`, or
    /// `--write-logs-to-file=false`. When the flag is present without a value,
    /// it defaults to true. When the flag is absent, the config file value is
    /// used (falling back to false if not configured).
    #[arg(
        long = "write-logs-to-file",
        value_name = "BOOL",
        num_args = 0..=1,
        require_equals = true,
        default_missing_value = "true",
    )]
    pub write_logs_to_file: Option<bool>,

    /// Path to a TOML configuration file (overrides default config locations)
    #[arg(long = "config")]
    pub config: Option<PathBuf>,

    /// Hide the menu bar at the top of the window (overrides config file)
    #[arg(long = "hide-menu-bar")]
    pub hide_menu_bar: bool,

    /// Path to write a FREC v2 recording file.
    ///
    /// When specified, all PTY I/O, keyboard/mouse input, and topology events
    /// are recorded to the given file. The file is finalized on clean exit.
    #[arg(long = "recording-path")]
    pub recording_path: Option<PathBuf>,

    /// Layout to load on startup.
    ///
    /// Can be a bare name (e.g. `dev`) to load
    /// `~/.config/freminal/layouts/dev.toml`, or a full path to a `.toml`
    /// file.  Overrides `startup.layout` in the config file.
    ///
    /// Positional arguments after `--layout` (before `--`) are passed to the
    /// layout as `$1`, `$2`, etc.  Use `--var` to pass named variables.
    ///
    /// Example:
    ///   freminal --layout dev ~/projects/myapp
    #[arg(long = "layout")]
    pub layout: Option<String>,

    /// Override a named variable in the layout.
    ///
    /// Format: `NAME=VALUE`.  Can be repeated for multiple variables.
    ///
    /// Example:
    ///   `freminal --layout dev --var project_dir=~/myapp --var branch=main`
    #[arg(long = "var", value_name = "NAME=VALUE", action = clap::ArgAction::Append)]
    pub layout_vars: Vec<String>,

    /// Program to run instead of the default shell.
    ///
    /// Everything after `--` (or the first non-option argument) is treated as
    /// a command and its arguments. When specified, freminal launches this
    /// program and exits when it terminates.
    ///
    /// Examples:
    ///   freminal yazi
    ///   freminal -- nvim -u NONE file.txt
    ///   freminal htop
    #[arg(trailing_var_arg = true)]
    pub command: Vec<String>,
}

impl Args {
    /// Parse the `--var NAME=VALUE` overrides into a `HashMap`.
    ///
    /// Values that do not contain `=` are silently ignored.
    #[must_use]
    pub fn layout_var_map(&self) -> std::collections::HashMap<String, String> {
        self.layout_vars
            .iter()
            .filter_map(|s| {
                let mut parts = s.splitn(2, '=');
                let key = parts.next()?.to_string();
                let val = parts.next()?.to_string();
                Some((key, val))
            })
            .collect()
    }
}
