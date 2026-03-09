// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use anyhow::Result;

pub struct Args {
    pub recording: Option<String>,
    pub shell: Option<String>,
    pub show_all_debug: bool,
    pub write_logs_to_file: bool,
}

impl Args {
    /// Parse the arguments
    ///
    /// # Errors
    /// Will return an error if the arguments are invalid
    pub fn parse<It: Iterator<Item = String>>(mut it: It) -> Result<Self> {
        trace!("Parsing args");

        let program_name = it.next();
        let mut recording_path = None;
        let mut shell = None;
        let mut error = false;
        let mut show_all_debug = false;
        #[cfg(debug_assertions)]
        let mut write_logs_to_file = true;
        #[cfg(not(debug_assertions))]
        let mut write_logs_to_file = false;

        while let Some(arg) = it.next() {
            match arg {
                arg if arg.as_str() == "--recording-path" => {
                    recording_path = it.next().map_or_else(
                        || {
                            println!("Missing argument for --recording-path");
                            Self::help(program_name.as_deref());
                            error = true;
                            None
                        },
                        Some,
                    );
                }
                arg if arg.as_str() == "--shell" => {
                    shell = it.next().map_or_else(
                        || {
                            println!("Missing argument for --shell");
                            Self::help(program_name.as_deref());
                            error = true;
                            None
                        },
                        Some,
                    );
                }
                arg if arg.as_str() == "--help" => Self::help(program_name.as_deref()),
                arg if arg.as_str() == "--show-all-debug" => show_all_debug = true,
                arg if arg.as_str().contains("--write-logs-to-file") => {
                    let mut internal_error = false;
                    write_logs_to_file = arg.split('=').nth(1).map_or_else(
                        || {
                            println!("Missing argument for --write-logs-to-file");
                            Self::help(program_name.as_deref());
                            internal_error = true;
                            false
                        },
                        |val| {
                            val.parse().unwrap_or_else(|_| {
                                println!("Invalid argument for --write-logs-to-file");
                                Self::help(program_name.as_deref());
                                error = true;
                                false
                            })
                        },
                    );

                    if internal_error {
                        error = true;
                    }
                }
                _ => {
                    println!("Invalid argument {arg}");
                    Self::help(program_name.as_deref());
                    error = true;
                }
            }
        }

        if error {
            return Err(anyhow::anyhow!("Invalid arguments"));
        }

        Ok(Self {
            recording: recording_path,
            shell,
            show_all_debug,
            write_logs_to_file,
        })
    }

    fn help(program_name: Option<&str>) {
        trace!("Showing help");

        let program_name = program_name.unwrap_or("freminal");
        println!(
            "\
                 Usage:\n\
                 {program_name} [ARGS]\n\
                 \n\
                 Args:\n\
                    --recording-path: Optional, where to output recordings to\n--shell: Optional, shell to run\n--help: Show this help message\n--write-logs-to-file=[true/false]\
                 "
        );
    }
}
