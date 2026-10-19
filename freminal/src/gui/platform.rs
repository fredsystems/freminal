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
//! Both functions are best-effort: if the platform has no supported path,
//! they silently no-op (beep) or return `None` (cwd) and emit a trace log.
//! See subtasks 71.14 (bell) and 71.16 (CWD) in
//! `Documents/PLAN_VERSION_080.md`.

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

/// Resolve the working directory for a new tab / split / saved layout pane.
///
/// `osc7_cwd` is the directory the shell reported via OSC 7. OSC 7 is
/// unauthenticated — any program writing to the terminal can emit it — so it
/// is used only when [`std::path::Path::is_dir`] confirms it names an existing
/// directory. Otherwise (absent, nonexistent, or not a directory) the result
/// falls back to [`read_cwd`] on `child_pid`, the OS's own answer for the
/// shell process. Returns `None` when neither source yields a directory.
///
/// This performs filesystem / process-inspection syscalls, so call it only in
/// response to user actions (new tab, split, layout save), never per frame.
pub fn resolve_spawn_cwd(osc7_cwd: Option<&str>, child_pid: Option<u32>) -> Option<String> {
    if let Some(cwd) = osc7_cwd {
        if std::path::Path::new(cwd).is_dir() {
            return Some(cwd.to_owned());
        }
        tracing::debug!("resolve_spawn_cwd: OSC 7 cwd is not an existing directory; ignoring");
    }
    read_cwd(child_pid?)
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
}
