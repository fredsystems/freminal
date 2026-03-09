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
    /// Accepts `--write-logs-to-file=true` or `--write-logs-to-file=false`.
    /// Defaults to true in debug builds and false in release builds.
    #[arg(
        long = "write-logs-to-file",
        value_name = "BOOL",
        default_value_t = cfg!(debug_assertions),
        num_args = 0..=1,
        require_equals = true,
        default_missing_value = "true",
    )]
    pub write_logs_to_file: bool,

    /// Path to a TOML configuration file (overrides default config locations)
    #[arg(long = "config")]
    pub config: Option<PathBuf>,
}
