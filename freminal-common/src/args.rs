// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use std::path::PathBuf;

use clap::Parser;

/// Freminal — a modern terminal emulator written in Rust
#[derive(Parser, Debug)]
#[command(name = "freminal", version, about)]
pub struct Args {
    /// Path to write session recordings to
    #[arg(long = "recording-path")]
    pub recording: Option<String>,

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

    /// Replay a recorded session file instead of launching a PTY.
    ///
    /// The file must contain raw bytes as produced by `--recording-path`.
    /// No shell is spawned; the recording is fed directly into the terminal
    /// emulator.
    #[arg(long = "with-playback-file")]
    pub playback: Option<PathBuf>,

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
