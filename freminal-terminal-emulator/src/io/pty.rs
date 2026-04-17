// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use std::{io::Write, path::Path, path::PathBuf};

#[cfg(feature = "playback")]
use std::time::Instant;

use super::{PtyRead, PtyWrite};
#[cfg(feature = "playback")]
use crate::recording;
use anyhow::Result;
use conv2::ValueFrom;
use crossbeam_channel::{Receiver, Sender};
use freminal_common::{
    pty_write::FreminalTerminalSize,
    terminal_size::{DEFAULT_HEIGHT, DEFAULT_WIDTH},
    terminfo::TERMINFO,
};
use portable_pty::{CommandBuilder, MasterPty, NativePtySystem, PtySize, PtySystem};

use sys_locale::get_locale;
use tempfile::TempDir;
use thiserror::Error;

/// Convert [`FreminalTerminalSize`] to [`PtySize`].
///
/// This conversion lives here (rather than as a `TryFrom` impl on
/// `freminal-common`) so that `freminal-common` does not depend on
/// `portable-pty` — keeping it free of platform-specific dependencies.
///
/// # Errors
/// Returns an error if any dimension value exceeds `u16::MAX`.
fn pty_size_from_terminal_size(value: &FreminalTerminalSize) -> Result<PtySize> {
    Ok(PtySize {
        rows: u16::value_from(value.height)?,
        cols: u16::value_from(value.width)?,
        pixel_width: u16::value_from(value.pixel_width)?,
        pixel_height: u16::value_from(value.pixel_height)?,
    })
}

pub struct FreminalPtyInputOutput {
    /// Holds the extracted terminfo directory alive for the lifetime of the PTY.
    /// `None` on Windows where terminfo is not used.
    _termcaps: Option<TempDir>,
    /// Receives a signal when the child process exits.
    ///
    /// On Unix the PTY reader thread detects child exit via `read() == 0` and
    /// drops the `PtyRead` sender, which is sufficient.  On Windows the `ConPTY`
    /// keeps the read pipe open after the child exits, so the reader thread
    /// blocks indefinitely.  A dedicated watcher thread calls `child.wait()`
    /// and sends `()` here when the child exits, giving the consumer thread a
    /// reliable cross-platform shutdown signal.
    pub child_exit_rx: Receiver<()>,
    /// Shared atomic flag reflecting whether the PTY slave currently has
    /// `ECHO` disabled (i.e. a password prompt is active).
    ///
    /// Updated by the writer thread every 100 ms (via `recv_timeout`) using
    /// `MasterPty::get_termios()`.  Compiled only on Unix; on Windows the
    /// `#[cfg(unix)]` termios block is omitted entirely, so the atomic stays
    /// at its default `false`.  Read by the GUI via a cheap `Relaxed` load
    /// without any locking overhead.
    pub echo_off: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

/// Return a safe temp directory path, bypassing `TMPDIR` which may be poisoned
/// by Nix devshell sandbox environments (pointing to e.g. `/build` which does
/// not exist at runtime).
///
/// Resolution order:
/// 1. `XDG_RUNTIME_DIR` — per-user volatile dir guaranteed to exist on systemd
///    systems (typically `/run/user/<uid>`).
/// 2. `std::env::temp_dir()` — platform-native fallback (`%TEMP%` on Windows,
///    `/tmp` on Linux/macOS).  Validated with `.is_dir()` to catch poisoned
///    environment variables (e.g. `TMPDIR=/build` in Nix sandboxes).
/// 3. On Unix only: hardcoded `/tmp` as a last resort.
fn safe_temp_dir() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_RUNTIME_DIR") {
        let path = PathBuf::from(&xdg);
        if path.is_dir() {
            return path;
        }
    }
    let system_temp = std::env::temp_dir();
    if system_temp.is_dir() {
        return system_temp;
    }
    // On Unix, /tmp is virtually always available even when env vars are
    // poisoned (e.g. inside Nix build sandboxes).
    #[cfg(unix)]
    {
        let fallback = PathBuf::from("/tmp");
        if fallback.is_dir() {
            return fallback;
        }
    }
    // All validation failed — return the system default anyway and let the
    // caller surface the error when it tries to create a temp directory.
    system_temp
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

/// The result of [`run_terminal`], bundling all values that the caller
/// needs after the PTY threads are launched.
pub struct RunTerminalResult {
    /// Receives `()` when the child process exits.
    pub child_exit_rx: Receiver<()>,
    /// Shared atomic flag reflecting whether the PTY slave currently has
    /// `ECHO` disabled.
    ///
    /// The writer thread polls `pair.master.get_termios()` every 250 ms
    /// (or immediately after each write/resize).  The PTY consumer thread
    /// reads it atomically when building snapshots.
    /// `Arc` lets both threads share ownership without a lock.
    pub echo_off: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

/// Process a single `PtyWrite` message: either write data or resize the PTY.
fn process_pty_write(
    writer: &mut Box<dyn std::io::Write + Send>,
    master: &dyn MasterPty,
    msg: &PtyWrite,
) {
    match msg {
        PtyWrite::Write(data) if let Err(e) = writer.write_all(data) => {
            error!("Failed to write to pty: {e}");
        }
        PtyWrite::Write(_) => {}
        PtyWrite::Resize(size) => {
            let size: PtySize = match pty_size_from_terminal_size(size) {
                Ok(size) => size,
                Err(e) => {
                    error!("failed to convert size {e}");
                    return;
                }
            };

            debug!("resizing pty to {size:?}");

            if let Err(e) = master.resize(size) {
                error!("Failed to resize pty: {e}");
            }
        }
    }
}

// Inherently large: the PTY thread event loop integrating the PTY reader, input channel, and
// window-command dispatch. Splitting would produce artificial sub-functions with no clear
// independent responsibility.
#[allow(clippy::too_many_lines)]
#[cfg_attr(not(feature = "playback"), allow(clippy::needless_pass_by_value))]
pub fn run_terminal(
    write_rx: Receiver<PtyWrite>,
    send_tx: Sender<PtyRead>,
    recording_path: Option<String>,
    command: Option<(String, Vec<String>)>,
    shell: Option<String>,
    termcaps: Option<&Path>,
    initial_size: &FreminalTerminalSize,
) -> Result<RunTerminalResult> {
    let pty_system = NativePtySystem::default();

    let pair = pty_system
        .openpty(
            pty_size_from_terminal_size(initial_size).unwrap_or(PtySize {
                rows: DEFAULT_HEIGHT,
                cols: DEFAULT_WIDTH,
                pixel_width: 0,
                pixel_height: 0,
            }),
        )
        .map_err(|e| {
            error!("Failed to open pty: {e}");
            e
        })?;

    let mut cmd = if let Some((prog, args)) = command {
        let mut c = CommandBuilder::new(prog);
        c.args(args)?;
        c
    } else {
        shell.map_or_else(CommandBuilder::new_default_prog, CommandBuilder::new)
    };

    // Use TERMINFO_DIRS (colon-separated search path) instead of TERMINFO
    // (single exclusive directory).  TERMINFO is exclusive: if set, ncurses
    // ONLY looks there — and our tarball only contains `freminal` /
    // `xterm-freminal`, not `xterm-256color`.  With TERMINFO_DIRS, ncurses
    // checks our directory first but falls back to the system terminfo so
    // readline and other programs can find xterm-256color capabilities.
    if let Some(termcaps) = termcaps {
        let termcaps_str = termcaps.display();
        cmd.env("TERMINFO_DIRS", format!("{termcaps_str}:"));
        cmd.env_remove("TERMINFO");
    }
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
    // child processes. The TERMINFO_DIRS env var includes the extracted tarball so
    // that programs that check for a valid TERMINFO directory find one.
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

    // NOTE: Some programs (e.g. ohmyposh, zsh) require LANG to be set.  On
    // many Linux installs LANG is unset by default, so we synthesise it here.
    //
    // If the detected locale already contains a codeset separator (`.`), we
    // use it verbatim — the system already knows its codeset.  Otherwise we
    // apply POSIX `_` normalisation only to the language/region portion and
    // append `.UTF-8`.
    //
    // NOTE: This assumes the system's native codeset is UTF-8 when none is
    // declared.  Non-UTF-8 locales (e.g. EUC-JP) that rely on LANG being
    // absent are an edge case that freminal does not handle today — the
    // correct fix would require a `locale(1)` query, which adds platform
    // complexity.  Filed for future improvement.

    if cmd.get_env("LANG").is_none() || cmd.get_env("LANG") == Some(std::ffi::OsStr::new("")) {
        let raw = get_locale().unwrap_or_else(|| String::from("en_US"));
        let locale = normalize_locale(&raw);
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
    //
    // On Windows, TEMP and TMP are essential system variables that must
    // not be removed — many programs (and the OS itself) rely on them.
    // TMPDIR and TEMPDIR are Unix conventions that don't exist on Windows.
    if cfg!(not(target_os = "windows")) {
        cmd.env_remove("TMPDIR");
        cmd.env_remove("TEMP");
        cmd.env_remove("TMP");
        cmd.env_remove("TEMPDIR");
    }
    cmd.env_remove("NIX_BUILD_TOP");
    cmd.env_remove("IN_NIX_SHELL");

    let mut child = pair.slave.spawn_command(cmd)?;

    // Release any handles owned by the slave: we don't need it now
    // that we've spawned the child.
    drop(pair.slave);

    // Spawn a child-watcher thread.  On Unix the PTY reader thread
    // usually detects child exit via `read() == 0`, but on Windows the
    // ConPTY keeps the read pipe open after the child exits so the
    // reader blocks indefinitely.  This watcher provides a reliable
    // cross-platform "child exited" signal.
    let (child_exit_tx, child_exit_rx) = crossbeam_channel::bounded::<()>(1);
    std::thread::spawn(move || {
        // `child.wait()` blocks until the child process exits.
        match child.wait() {
            Ok(status) => {
                info!("Child process exited with status: {status:?}");
            }
            Err(e) => {
                error!("Failed to wait on child process: {e}");
            }
        }
        let _ = child_exit_tx.send(());
    });

    // Read the output in another thread.
    // This is important because it is easy to encounter a situation
    // where read/write buffers fill and block either your process
    // or the spawned process.
    let mut reader = pair.master.try_clone_reader()?;

    std::thread::spawn(move || {
        let buf = &mut [0u8; 4096];
        #[cfg(feature = "playback")]
        let mut recording = None;
        #[cfg(feature = "playback")]
        let mut recording_start = None;
        #[cfg(not(feature = "playback"))]
        let _ = recording_path;

        // if recording path is some, open a file for writing
        #[cfg(feature = "playback")]
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
            #[cfg(feature = "playback")]
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

    // Shared atomic flag updated by the writer thread after each PTY event.
    // The PTY consumer thread reads this via `FreminalPtyInputOutput::is_echo_off()`
    // when building snapshots.  Using `Arc<AtomicBool>` avoids any lock on the hot
    // snapshot path while keeping ownership safely shared between threads.
    let echo_off = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    #[cfg(unix)]
    let echo_off_writer = std::sync::Arc::clone(&echo_off);

    {
        std::thread::spawn(move || {
            // Poll interval for checking slave termios echo state.
            // Used with `recv_timeout` so we detect password prompts
            // even when no keystrokes are arriving.
            const ECHO_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);

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

            // Use `recv_timeout` so we also poll the slave termios
            // periodically even when no keystrokes arrive.  This lets the
            // lock icon appear as soon as a password prompt disables ECHO,
            // without waiting for user input.
            //
            // Termios is polled only on timeout (no pending writes), not
            // after every keystroke — this avoids ioctl overhead during
            // rapid typing.

            loop {
                match write_rx.recv_timeout(ECHO_POLL_INTERVAL) {
                    Ok(stuff_to_write) => {
                        process_pty_write(&mut writer, &*pair.master, &stuff_to_write);

                        // Drain any queued writes before polling termios.
                        while let Ok(more) = write_rx.try_recv() {
                            process_pty_write(&mut writer, &*pair.master, &more);
                        }
                    }
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                        // No input — fall through to poll termios below.
                    }
                    Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
                }

                // Poll the slave termios after draining all pending writes
                // or on timeout.  The writer thread is the natural place for
                // this because it already owns `pair.master`.  When `ECHO` is
                // absent and `ICANON` is present, the foreground process
                // (e.g. `sudo`) has set up a canonical-mode password prompt.
                // Shell line editors (zsh ZLE, bash readline) also disable
                // `ECHO` but additionally disable `ICANON`, so checking both
                // flags avoids false positives during normal interactive
                // editing.
                //
                // This block is compiled only on Unix; on Windows (ConPTY)
                // there is no termios API, so the atomic stays at its
                // default `false`.
                #[cfg(unix)]
                {
                    use nix::sys::termios::LocalFlags;
                    // Password prompts (sudo, ssh, getpass) disable ECHO but
                    // keep ICANON (canonical/line-buffered mode).  Shell line
                    // editors (zsh ZLE, bash readline) also disable ECHO but
                    // additionally disable ICANON (raw/char-at-a-time mode)
                    // because they handle input character by character.
                    //
                    // Checking `!ECHO && ICANON` filters out the shell's
                    // normal interactive editing and only triggers for genuine
                    // password prompts.
                    let is_off = pair.master.get_termios().is_some_and(|t| {
                        !t.local_flags.contains(LocalFlags::ECHO)
                            && t.local_flags.contains(LocalFlags::ICANON)
                    });
                    let old = echo_off_writer.swap(is_off, std::sync::atomic::Ordering::Relaxed);
                    if old != is_off {
                        debug!("echo_off changed: {old} -> {is_off}");
                    }
                }
            }
        });
    }

    Ok(RunTerminalResult {
        child_exit_rx,
        echo_off,
    })
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
        command: Option<(String, Vec<String>)>,
        shell: Option<String>,
        initial_size: &FreminalTerminalSize,
    ) -> Result<Self> {
        // don't use it.  Skip extraction entirely on Windows to avoid issues
        // with symlinks in the tarball requiring elevated privileges.
        let termcaps = if cfg!(target_os = "windows") {
            None
        } else {
            Some(extract_terminfo().map_err(|e| {
                error!("Failed to extract terminfo: {e}");
                e
            })?)
        };

        #[cfg(not(feature = "playback"))]
        let _ = recording;

        let result = run_terminal(
            write_rx,
            send_tx,
            recording,
            command,
            shell,
            termcaps.as_ref().map(TempDir::path),
            initial_size,
        )?;
        Ok(Self {
            _termcaps: termcaps,
            child_exit_rx: result.child_exit_rx,
            echo_off: result.echo_off,
        })
    }

    /// Return `true` when the PTY slave currently has the `ECHO` flag disabled.
    ///
    /// This is the standard signal that a password prompt is active — the slave
    /// process (e.g. `sudo`, `ssh`) called `tcsetattr()` to turn off echoing so
    /// the typed password does not appear on screen.
    ///
    /// On Unix the writer thread polls `MasterPty::get_termios()` every 250 ms
    /// (and after each write/resize) and updates the shared atomic; this method
    /// simply reads that atomic.
    ///
    /// Always returns `false` on Windows where `ConPTY` does not support termios.
    #[must_use]
    pub fn is_echo_off(&self) -> bool {
        self.echo_off.load(std::sync::atomic::Ordering::Relaxed)
    }
}

/// Normalise a raw locale string into a `LANG`-compatible value.
///
/// POSIX locale format: `language[_territory][.codeset][@modifier]`
///
/// If `raw` already contains a codeset separator (`.`), it is returned
/// verbatim — the system already declared its codeset, so we must not
/// clobber it.  Otherwise, `.UTF-8` is inserted before any `@modifier`
/// suffix, and any `-` characters in the language/region portion are
/// replaced with `_` (POSIX convention).
///
/// Examples:
/// - `"en-US"` → `"en_US.UTF-8"`
/// - `"en_US"` → `"en_US.UTF-8"`
/// - `"en_US.UTF-8"` → `"en_US.UTF-8"` (unchanged)
/// - `"ja_JP.EUC-JP"` → `"ja_JP.EUC-JP"` (unchanged)
/// - `"en_US@euro"` → `"en_US.UTF-8@euro"`
/// - `"en-GB@euro"` → `"en_GB.UTF-8@euro"`
fn normalize_locale(raw: &str) -> String {
    if raw.contains('.') {
        // Codeset already declared — use as-is.
        raw.to_string()
    } else {
        // No codeset — normalise separators and insert `.UTF-8` before
        // any `@modifier` suffix.
        let (lang_region, modifier) = match raw.split_once('@') {
            Some((lr, m)) => (lr, Some(m)),
            None => (raw, None),
        };
        let normalised = lang_region.replace('-', "_");
        modifier.map_or_else(
            || format!("{normalised}.UTF-8"),
            |m| format!("{normalised}.UTF-8@{m}"),
        )
    }
}

#[cfg(test)]
mod locale_tests {
    use super::normalize_locale;

    #[test]
    fn locale_with_codeset_is_returned_unchanged() {
        assert_eq!(normalize_locale("en_US.UTF-8"), "en_US.UTF-8");
    }

    #[test]
    fn locale_with_non_utf8_codeset_is_returned_unchanged() {
        assert_eq!(normalize_locale("ja_JP.EUC-JP"), "ja_JP.EUC-JP");
    }

    #[test]
    fn locale_without_codeset_gets_utf8_appended() {
        assert_eq!(normalize_locale("en_US"), "en_US.UTF-8");
    }

    #[test]
    fn locale_with_dash_separator_is_normalised_to_underscore() {
        assert_eq!(normalize_locale("en-US"), "en_US.UTF-8");
    }

    #[test]
    fn locale_with_already_underscore_separator_unchanged_apart_from_codeset() {
        assert_eq!(normalize_locale("fr_FR"), "fr_FR.UTF-8");
    }

    #[test]
    fn empty_locale_gets_utf8_appended() {
        assert_eq!(normalize_locale(""), ".UTF-8");
    }

    #[test]
    fn locale_with_modifier_inserts_codeset_before_modifier() {
        assert_eq!(normalize_locale("en_US@euro"), "en_US.UTF-8@euro");
    }

    #[test]
    fn locale_with_dash_and_modifier_normalises_and_inserts_codeset() {
        assert_eq!(normalize_locale("en-GB@euro"), "en_GB.UTF-8@euro");
    }

    #[test]
    fn locale_with_codeset_and_modifier_returned_unchanged() {
        assert_eq!(normalize_locale("en_US.UTF-8@euro"), "en_US.UTF-8@euro");
    }
}
