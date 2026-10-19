// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Small platform-specific helpers that don't fit anywhere else.
//!
//! * [`system_beep`] — produces a short audible alert using the native
//!   "system beep" API on each supported platform.
//! * [`read_cwd`] — resolves the current working directory of a running
//!   process by PID, used when serialising layouts and recording snapshots.
//! * [`resolve_spawn_cwd`] — picks the directory a new tab, split or saved
//!   layout pane should use, trusting the shell-reported OSC 7 path only when
//!   it names an existing directory.
//!
//! * [`check_spawn_directory`] — validates a directory taken from a saved
//!   layout before a pane is spawned in it.
//!
//! The filesystem-touching helpers run on a short-lived helper thread bounded
//! by [`SPAWN_CWD_DEADLINE`] (see [`with_deadline`]) so a hung network mount
//! cannot stall the GUI thread.
//!
//! The beep and cwd functions are best-effort: if the platform has no supported path,
//! they silently no-op (beep) or return `None` (cwd) and emit a trace log.
//! See subtasks 71.14 (bell) and 71.16 (CWD) in
//! `Documents/PLAN_VERSION_080.md`.

use std::sync::mpsc;
use std::time::Duration;

/// Best-effort audible alert using the platform's native system-beep API.
///
/// * **Linux** — writes the BEL byte (`0x07`) to stderr.  Works when freminal
///   is launched from a terminal (the kernel / parent terminal translates it
///   into a short beep).  Silently no-ops when stderr is not a TTY (for
///   example when freminal is launched from a .desktop file under Wayland).
/// * **macOS** — calls `AppKit`'s `NSBeep()`.
/// * **Windows** — calls `user32!MessageBeep(MB_OK)`.
/// * Other platforms — no-op with a trace log.
///
/// Any I/O error (for example a closed stderr) is swallowed and traced; a
/// failure to beep must never take down the terminal.
pub fn system_beep() {
    #[cfg(target_os = "linux")]
    {
        use std::io::Write as _;
        // A write failure here is harmless — we just couldn't produce the
        // beep.  Trace for debuggability but never surface to the user.
        if let Err(e) = std::io::stderr().write_all(b"\x07") {
            tracing::trace!("system_beep: stderr write failed: {e}");
        }
    }

    #[cfg(target_os = "macos")]
    {
        // `NSBeep` is a zero-argument C function exported by AppKit.
        // Linking `AppKit` is already done transitively by winit on macOS,
        // so this adds no new build-time dependency.
        unsafe extern "C" {
            fn NSBeep();
        }
        // SAFETY: `NSBeep` is a well-documented AppKit API that takes no
        // arguments, returns no value, and has no thread-affinity
        // constraints for this usage.  The extern declaration matches its
        // real signature.
        unsafe {
            NSBeep();
        }
    }

    #[cfg(target_os = "windows")]
    {
        // `MessageBeep(MB_OK)` plays the default system sound.  `MB_OK` is
        // `0x00000000`, so we just pass `0` to avoid pulling in winapi
        // constants.  The return value (BOOL) indicates success; we ignore
        // it because a failure to beep must not disrupt the terminal.
        unsafe extern "system" {
            fn MessageBeep(uType: u32) -> i32;
        }
        // SAFETY: `MessageBeep` is a thread-safe user32 API that accepts any
        // `u32` value (unknown values simply fall back to the default
        // sound).  The extern declaration matches the documented signature.
        unsafe {
            let _ = MessageBeep(0);
        }
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        tracing::trace!("system_beep: no implementation for this platform");
    }
}

/// Resolve the current working directory of the process with the given PID.
///
/// Returns the CWD as a UTF-8 string, or `None` if the platform is unsupported,
/// the process has exited, permission is denied, or the path is not valid
/// UTF-8.  All errors are swallowed and traced — a failure here must not break
/// layout save, recording snapshots, or any other caller.
///
/// * **Linux** — reads the `/proc/<pid>/cwd` symlink via `std::fs::read_link`.
///   Fast, zero-allocation beyond the path, no external crate needed.
/// * **macOS / Windows** — delegates to the `sysinfo` crate, which wraps
///   `proc_pidinfo(PROC_PIDVNODEPATHINFO)` on macOS and
///   `NtQueryInformationProcess` (reading the target's PEB
///   `RTL_USER_PROCESS_PARAMETERS.CurrentDirectory`) on Windows.  Both are
///   standard, documented techniques with no unsafe code in our tree.
/// * Other platforms — returns `None` with a trace log.
pub fn read_cwd(pid: u32) -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        let link = format!("/proc/{pid}/cwd");
        match std::fs::read_link(&link) {
            Ok(p) => p.into_os_string().into_string().ok(),
            Err(e) => {
                tracing::trace!("read_cwd: readlink({link}) failed: {e}");
                None
            }
        }
    }

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System};

        // Build a minimally-populated System and refresh only the single PID
        // we care about.  `ProcessRefreshKind::nothing().with_cwd(...)` keeps
        // the syscall count to the minimum needed to fetch the CWD field.
        let mut sys = System::new_with_specifics(RefreshKind::nothing());
        let sys_pid = Pid::from_u32(pid);
        sys.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[sys_pid]),
            true,
            ProcessRefreshKind::nothing().with_cwd(sysinfo::UpdateKind::Always),
        );
        sys.process(sys_pid).map_or_else(
            || {
                tracing::trace!("read_cwd: sysinfo has no process for pid {pid}");
                None
            },
            |proc| proc.cwd().map(|p| p.to_string_lossy().into_owned()),
        )
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        let _ = pid;
        tracing::trace!("read_cwd: no implementation for this platform");
        None
    }
}

/// Longest the GUI thread waits for filesystem / process inspection when
/// choosing a spawn directory.
///
/// OSC 7 paths are attacker-influenced: one naming a hung network mount or an
/// autofs trigger would otherwise block `is_dir` (and the GUI) indefinitely.
pub const SPAWN_CWD_DEADLINE: Duration = Duration::from_millis(250);

/// Outcome of [`with_deadline`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeadlineResult<T> {
    /// The closure finished within the deadline.
    Done(T),
    /// The closure did not finish in time. Its thread keeps running.
    TimedOut,
    /// The helper thread could not be started, or it died (panicked) before
    /// producing a result.
    Unavailable,
}

/// Run `f` on a short-lived helper thread and wait at most `deadline` for it.
///
/// On [`DeadlineResult::TimedOut`] the helper thread is **not** cancelled
/// (std threads cannot be): it finishes on its own whenever the blocked
/// syscall returns, and its result is discarded. A thread stuck on a hung
/// mount is therefore leaked until that syscall returns, which is acceptable
/// for the user-triggered actions that call this. If the thread cannot be
/// spawned the closure is deliberately *not* run inline, since that would
/// reintroduce the stall; the caller gets [`DeadlineResult::Unavailable`].
pub fn with_deadline<T: Send + 'static>(
    deadline: Duration,
    f: impl FnOnce() -> T + Send + 'static,
) -> DeadlineResult<T> {
    let (tx, rx) = mpsc::sync_channel(1);
    let spawned = std::thread::Builder::new()
        .name("spawn-cwd-probe".to_owned())
        .spawn(move || {
            // The receiver is gone if the caller already timed out; the
            // result is intentionally discarded then.
            let _ = tx.send(f());
        });
    if let Err(e) = spawned {
        tracing::warn!("with_deadline: could not start helper thread: {e}");
        return DeadlineResult::Unavailable;
    }
    match rx.recv_timeout(deadline) {
        Ok(v) => DeadlineResult::Done(v),
        Err(mpsc::RecvTimeoutError::Timeout) => DeadlineResult::TimedOut,
        Err(mpsc::RecvTimeoutError::Disconnected) => DeadlineResult::Unavailable,
    }
}

/// Resolve the working directory for a new tab / split / saved layout pane.
///
/// `osc7_cwd` is the directory the shell reported via OSC 7. OSC 7 is
/// unauthenticated — any program writing to the terminal can emit it — so it
/// is used only when [`std::path::Path::is_dir`] confirms it names an existing
/// directory. Otherwise (absent, nonexistent, or not a directory) the result
/// falls back to [`read_cwd`] on `child_pid`, the OS's own answer for the
/// shell process. Returns `None` when neither source yields a directory.
///
/// The probing runs under [`SPAWN_CWD_DEADLINE`]; if it does not finish in
/// time the result is `None` (the new pane starts in the default directory)
/// and a warning without the path is logged.
///
/// Call it only in response to user actions (new tab, split, layout save),
/// never per frame.
pub fn resolve_spawn_cwd(osc7_cwd: Option<&str>, child_pid: Option<u32>) -> Option<String> {
    let osc7 = osc7_cwd.map(str::to_owned);
    match with_deadline(SPAWN_CWD_DEADLINE, move || {
        resolve_spawn_cwd_blocking(osc7.as_deref(), child_pid)
    }) {
        DeadlineResult::Done(cwd) => cwd,
        DeadlineResult::TimedOut => {
            tracing::warn!(
                "resolve_spawn_cwd: directory check exceeded {} ms; using the default directory",
                SPAWN_CWD_DEADLINE.as_millis()
            );
            None
        }
        DeadlineResult::Unavailable => {
            tracing::warn!("resolve_spawn_cwd: directory check unavailable; using the default");
            None
        }
    }
}

/// The unbounded body of [`resolve_spawn_cwd`]; may block on the filesystem.
fn resolve_spawn_cwd_blocking(osc7_cwd: Option<&str>, child_pid: Option<u32>) -> Option<String> {
    if let Some(cwd) = osc7_cwd {
        if std::path::Path::new(cwd).is_dir() {
            return Some(cwd.to_owned());
        }
        tracing::debug!("resolve_spawn_cwd: OSC 7 cwd is not an existing directory; ignoring");
    }
    read_cwd(child_pid?)
}

/// Why [`check_spawn_directory`] refused a directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SpawnDirectoryRejection {
    /// The path contains a control character.
    #[error("directory contains control characters")]
    ControlCharacters,
    /// The path does not name an existing directory.
    #[error("directory does not exist or is not a directory")]
    NotADirectory,
    /// The existence check did not finish within [`SPAWN_CWD_DEADLINE`].
    #[error("directory could not be checked in time")]
    Unchecked,
}

/// Validate a directory taken from a saved layout before spawning a pane in it.
///
/// The directory must contain no control characters and must be an existing
/// directory, confirmed within [`SPAWN_CWD_DEADLINE`]. Layouts (including
/// `last_session.toml`, which older builds could write from an unvalidated
/// OSC 7 path) are otherwise trusted input; this only guards against paths
/// that cannot work or could stall the GUI.
///
/// # Errors
///
/// Returns the [`SpawnDirectoryRejection`] describing why the directory must
/// not be used.
pub fn check_spawn_directory(dir: &str) -> Result<(), SpawnDirectoryRejection> {
    if dir.chars().any(char::is_control) {
        return Err(SpawnDirectoryRejection::ControlCharacters);
    }
    let owned = dir.to_owned();
    match with_deadline(SPAWN_CWD_DEADLINE, move || {
        std::path::Path::new(&owned).is_dir()
    }) {
        DeadlineResult::Done(true) => Ok(()),
        DeadlineResult::Done(false) => Err(SpawnDirectoryRejection::NotADirectory),
        DeadlineResult::TimedOut | DeadlineResult::Unavailable => {
            Err(SpawnDirectoryRejection::Unchecked)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn existing_osc7_dir_is_used() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_str().unwrap().to_owned();
        assert_eq!(
            resolve_spawn_cwd(Some(&path), None).as_deref(),
            Some(path.as_str())
        );
    }

    #[test]
    fn existing_osc7_dir_wins_over_process_cwd() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_str().unwrap().to_owned();
        assert_eq!(
            resolve_spawn_cwd(Some(&path), Some(std::process::id())).as_deref(),
            Some(path.as_str())
        );
    }

    #[test]
    fn none_and_none_is_none() {
        assert_eq!(resolve_spawn_cwd(None, None), None);
    }

    #[test]
    fn nonexistent_osc7_without_pid_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("does-not-exist");
        assert_eq!(resolve_spawn_cwd(missing.to_str(), None), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn nonexistent_osc7_falls_back_to_process_cwd() {
        let pid = std::process::id();
        let expected = read_cwd(pid);
        assert!(expected.is_some());
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("does-not-exist");
        assert_eq!(resolve_spawn_cwd(missing.to_str(), Some(pid)), expected);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn file_osc7_path_falls_back_to_process_cwd() {
        let pid = std::process::id();
        let expected = read_cwd(pid);
        assert!(expected.is_some());
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a-file");
        std::fs::write(&file, b"x").unwrap();
        assert_eq!(resolve_spawn_cwd(file.to_str(), Some(pid)), expected);
    }

    #[test]
    fn with_deadline_fast_closure_is_done() {
        assert_eq!(
            with_deadline(Duration::from_secs(30), || 7_u32),
            DeadlineResult::Done(7)
        );
    }

    #[test]
    fn with_deadline_blocked_closure_times_out_then_thread_exits() {
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let (exited_tx, exited_rx) = mpsc::channel::<()>();
        let result = with_deadline(Duration::from_millis(20), move || {
            // Blocks until the test releases it, so it cannot finish in time.
            let _ = release_rx.recv();
            let _ = exited_tx.send(());
        });
        assert_eq!(result, DeadlineResult::TimedOut);
        // Release the helper thread and wait for it to finish: nothing leaks.
        release_tx.send(()).unwrap();
        exited_rx.recv().unwrap();
    }

    #[test]
    fn with_deadline_panicking_closure_is_unavailable() {
        let result: DeadlineResult<()> =
            with_deadline(Duration::from_secs(30), || panic!("intentional test panic"));
        assert_eq!(result, DeadlineResult::Unavailable);
    }

    #[test]
    fn check_spawn_directory_accepts_existing_dir() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(check_spawn_directory(dir.path().to_str().unwrap()), Ok(()));
    }

    #[test]
    fn check_spawn_directory_rejects_missing_and_file() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope");
        assert_eq!(
            check_spawn_directory(missing.to_str().unwrap()),
            Err(SpawnDirectoryRejection::NotADirectory)
        );
        let file = dir.path().join("f");
        std::fs::write(&file, b"x").unwrap();
        assert_eq!(
            check_spawn_directory(file.to_str().unwrap()),
            Err(SpawnDirectoryRejection::NotADirectory)
        );
    }

    #[test]
    fn check_spawn_directory_rejects_control_characters() {
        assert_eq!(
            check_spawn_directory("/tmp/a\u{1b}[31mb"),
            Err(SpawnDirectoryRejection::ControlCharacters)
        );
        assert_eq!(
            check_spawn_directory("/tmp/a\nb"),
            Err(SpawnDirectoryRejection::ControlCharacters)
        );
    }
}
