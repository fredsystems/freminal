// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Small platform-specific helpers that don't fit anywhere else.
//!
//! Currently only [`system_beep`], which produces a short audible alert using
//! the native "system beep" API on each supported platform.  The function is
//! best-effort: if the platform has no equivalent (notably headless Linux or
//! Wayland sessions without a controlling TTY) it silently does nothing and
//! emits a trace log.  See subtask 71.14 in `Documents/PLAN_VERSION_080.md`.

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
