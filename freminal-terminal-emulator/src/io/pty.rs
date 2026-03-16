// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use std::{io::Write, path::Path, path::PathBuf, time::Instant};

use super::{PtyRead, PtyWrite};
use crate::recording;
use anyhow::Result;
use crossbeam_channel::{Receiver, Sender};
use freminal_common::{
    terminal_size::{DEFAULT_HEIGHT, DEFAULT_WIDTH},
    terminfo::TERMINFO,
};
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use sys_locale::get_locale;
use tempfile::TempDir;
use thiserror::Error;

pub struct FreminalPtyInputOutput {
    _termcaps: TempDir,
}

/// Return a safe temp directory path, bypassing `TMPDIR` which may be poisoned
/// by Nix devshell sandbox environments (pointing to e.g. `/build` which does
/// not exist at runtime).
///
/// Resolution order:
/// 1. `XDG_RUNTIME_DIR` — per-user volatile dir guaranteed to exist on systemd
///    systems (typically `/run/user/<uid>`).
/// 2. `/tmp` — universal fallback.
fn safe_temp_dir() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_RUNTIME_DIR") {
        let path = PathBuf::from(&xdg);
        if path.is_dir() {
            return path;
        }
    }
    PathBuf::from("/tmp")
}

fn extract_terminfo() -> Result<TempDir, ExtractTerminfoError> {
    let mut terminfo_tarball = tar::Archive::new(TERMINFO);
    let base = safe_temp_dir();
    let temp_dir = TempDir::new_in(&base).map_err(|e| ExtractTerminfoError::CreateTempDir {
        source: e,
        path: base,
    })?;
    terminfo_tarball
        .unpack(temp_dir.path())
        .map_err(ExtractTerminfoError::Extraction)?;

    Ok(temp_dir)
}

#[derive(Error, Debug)]
enum ExtractTerminfoError {
    #[error("failed to extract")]
    Extraction(#[source] std::io::Error),
    #[error("failed to create temp dir in {path}")]
    CreateTempDir {
        #[source]
        source: std::io::Error,
        path: PathBuf,
    },
}

#[allow(clippy::too_many_lines)]
pub fn run_terminal(
    write_rx: Receiver<PtyWrite>,
    send_tx: Sender<PtyRead>,
    recording_path: Option<String>,
    shell: Option<String>,
    termcaps: &Path,
) -> Result<()> {
    let pty_system = NativePtySystem::default();

    let pair = pty_system
        .openpty(PtySize {
            rows: DEFAULT_HEIGHT,
            cols: DEFAULT_WIDTH,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| {
            error!("Failed to open pty: {e}");
            e
        })?;

    let mut cmd = shell.map_or_else(CommandBuilder::new_default_prog, CommandBuilder::new);

    cmd.env("TERMINFO", termcaps);
    // TERM Strategy: We set TERM=xterm-256color rather than a custom value like
    // "xterm-freminal" for maximum compatibility. Programs like neovim, tmux, and
    // many TUI apps have hardcoded behavior based on TERM — setting it to an
    // unrecognized value causes them to fall back to minimal capabilities. This is
    // the same approach used by WezTerm, Alacritty, and most modern terminals.
    //
    // For capabilities beyond what xterm-256color declares (e.g., true color, styled
    // underlines, clipboard via OSC 52), we rely on XTGETTCAP (DCS +q) responses.
    // Modern programs query these at startup to discover extra features. See
    // `TerminalHandler::lookup_termcap` in freminal-buffer for the full list.
    //
    // The custom freminal.ti entry in res/ exists as a reference but is not used by
    // child processes. The TERMINFO env var points to the extracted tarball so that
    // programs that check for a valid TERMINFO directory find one.
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");

    // get the version of freminal
    let version = format!(
        "{}-{}",
        env!("CARGO_PKG_VERSION"),
        env!("VERGEN_BUILD_TIMESTAMP")
    );
    cmd.env("TERM_PROGRAM", "freminal");
    cmd.env("TERM_PROGRAM_VERSION", version);
    cmd.env("__CFBundleIdentifier", "io.github.fredclausen.freminal");

    // FIXME: I don't know if this works for all locales
    // the problem here is some programs (like ohmyposh and zsh)
    // want the LANG env variable set, otherwise it fucks up.
    // at least on my system, LANG isn't set by default.
    // I'm assuming this is the case for others, that `.utf-8` is
    // correct and that `-` in the locale should be replaced with `_`.

    if cmd.get_env("LANG").is_none() || cmd.get_env("LANG") == Some(std::ffi::OsStr::new("")) {
        let locale = format!(
            "{}.UTF-8",
            get_locale()
                .unwrap_or_else(|| String::from("en_US"))
                .replace('-', "_")
        );
        info!("No LANG detected in the environment. Detected locale: {locale}. Setting LANG");

        cmd.env("LANG", locale);
    }

    // Remove parent-terminal environment variables so the child shell does
    // not inherit stale references to the parent terminal emulator.
    cmd.env_remove("WEZTERM_CONFIG_DIR");
    cmd.env_remove("WEZTERM_CONFIG_FILE");
    cmd.env_remove("WEZTERM_EXECUTABLE");
    cmd.env_remove("WEZTERM_EXECUTABLE_DIR");
    cmd.env_remove("WEZTERM_PANE");
    cmd.env_remove("WEZTERM_UNIX_SOCKET");

    // Nix devshell / sandbox hygiene: when Freminal is launched from a
    // shell that has `direnv` + Nix devshell active, several variables
    // may point to non-existent sandbox paths (e.g. TMPDIR=/build).
    // Removing them lets the child shell fall back to system defaults.
    cmd.env_remove("TMPDIR");
    cmd.env_remove("TEMP");
    cmd.env_remove("TMP");
    cmd.env_remove("TEMPDIR");
    cmd.env_remove("NIX_BUILD_TOP");
    cmd.env_remove("IN_NIX_SHELL");
    cmd.env_remove("TERMINFO_DIRS");

    let _child = pair.slave.spawn_command(cmd)?;

    // Release any handles owned by the slave: we don't need it now
    // that we've spawned the child.
    drop(pair.slave);

    // Read the output in another thread.
    // This is important because it is easy to encounter a situation
    // where read/write buffers fill and block either your process
    // or the spawned process.
    let mut reader = pair.master.try_clone_reader()?;

    std::thread::spawn(move || {
        let buf = &mut [0u8; 4096];
        let mut recording = None;
        let mut recording_start = None;

        // if recording path is some, open a file for writing
        if let Some(path) = &recording_path {
            match std::fs::File::create(path) {
                Ok(mut file) => {
                    if let Err(e) = recording::write_header(&mut file) {
                        error!("Failed to write recording header: {e}");
                    } else {
                        recording_start = Some(Instant::now());
                        recording = Some(file);
                    }
                }
                Err(e) => {
                    error!("Failed to create recording file: {e}");
                }
            }
        }

        // Consume the output from the child.
        //
        // When the PTY is closed (amount_read == 0, i.e. the shell exited),
        // we simply `return` instead of calling `process::exit()`.  Returning
        // drops `send_tx`, which closes the `pty_read_rx` channel in the PTY
        // consumer thread — that thread then signals the GUI to close cleanly.
        //
        // Calling `process::exit()` from a worker thread is dangerous: it
        // runs C exit handlers (`__eglFini`, etc.) while the GUI main thread
        // may be mid-`eglSwapBuffers`, causing heap corruption → SIGSEGV.
        while let Ok(amount_read) = reader.read(buf) {
            if amount_read == 0 {
                info!("PTY closed (read returned 0 bytes); reader thread exiting");
                return;
            }
            let data = buf[..amount_read].to_vec();

            // Write framed data to recording file
            if let Some(file) = &mut recording {
                let elapsed = recording_start.map_or(0, |start| {
                    u64::try_from(start.elapsed().as_micros()).unwrap_or(u64::MAX)
                });
                if let Err(e) = recording::write_frame(file, elapsed, &data) {
                    error!("Failed to write to recording file: {e}");
                    return;
                }
            }

            if let Err(e) = send_tx.send(PtyRead {
                buf: data,
                read_amount: amount_read,
            }) {
                error!("Failed to send data to terminal: {e}");
                return;
            }
        }
    });

    {
        std::thread::spawn(move || {
            if cfg!(target_os = "macos") {
                // macOS quirk: the child and reader must be started and
                // allowed a brief grace period to run before we allow
                // the writer to drop. Otherwise, the data we send to
                // the kernel to trigger EOF is interleaved with the
                // data read by the reader! WTF!?
                // This appears to be a race condition for very short
                // lived processes on macOS.
                // I'd love to find a more deterministic solution to
                // this than sleeping.
                std::thread::sleep(std::time::Duration::from_millis(20));
            }

            let mut writer = match pair.master.take_writer() {
                Ok(writer) => writer,
                Err(e) => {
                    error!("Failed to take writer: {e}");
                    return;
                }
            };

            while let Ok(stuff_to_write) = write_rx.recv() {
                match stuff_to_write {
                    PtyWrite::Write(data) => match writer.write_all(&data) {
                        Ok(()) => {}
                        Err(e) => {
                            error!("Failed to write to pty: {e}");
                        }
                    },
                    PtyWrite::Resize(size) => {
                        let size: PtySize = match PtySize::try_from(size) {
                            Ok(size) => size,
                            Err(e) => {
                                error!("failed to convert size {e}");
                                continue;
                            }
                        };

                        debug!("resizing pty to {size:?}");

                        match pair.master.resize(size) {
                            Ok(()) => {}
                            Err(e) => {
                                error!("Failed to resize pty: {e}");
                            }
                        }
                    }
                }
            }
        });
    }

    Ok(())
}

impl FreminalPtyInputOutput {
    /// Create a new `FreminalPtyInputOutput` instance.
    ///
    /// # Errors
    /// Will return an error if the terminal cannot be created.
    pub fn new(
        write_rx: Receiver<PtyWrite>,
        send_tx: Sender<PtyRead>,
        recording: Option<String>,
        shell: Option<String>,
    ) -> Result<Self> {
        let termcaps = extract_terminfo().map_err(|e| {
            error!("Failed to extract terminfo: {e}");
            e
        })?;

        run_terminal(write_rx, send_tx, recording, shell, termcaps.path())?;
        Ok(Self {
            _termcaps: termcaps,
        })
    }
}
