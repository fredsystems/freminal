// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Cross-platform lock-key (Caps Lock / Num Lock / Scroll Lock) state query.
//!
//! The kitty keyboard protocol reports `caps_lock`/`num_lock` as ambient
//! modifier bits alongside other key reports. Those bits must reflect the
//! true OS/kernel lock state, not just what freminal has observed from key
//! events, because a lock can be toggled while the terminal is unfocused (or
//! before it is even launched). This module provides [`query_lock_state`] as
//! the single cross-platform entry point.
//!
//! On Linux (both X11 and Wayland), the query reads kernel LED state via
//! `evdev`, sidestepping the display server entirely -- see the durable
//! decision in `Documents/PLAN_VERSION_110.md` ("114 Durable decision:
//! per-platform lock-state query resolved") for the full rationale. Windows
//! and macOS real queries land in subtasks 114.2/114.3; until then those
//! platforms use a stub that always reports all locks as inactive.

/// The current state of the three standard lock keys.
///
/// This is an ambient snapshot of OS/kernel-level lock-key state, not a
/// freminal-tracked "have we seen a toggle" flag. Callers should re-query
/// at appropriate points (cold start, focus-gain) rather than caching this
/// indefinitely.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LockState {
    /// Whether Caps Lock is currently active.
    pub caps: bool,
    /// Whether Num Lock is currently active.
    pub num: bool,
    /// Whether Scroll Lock is currently active.
    pub scroll: bool,
}

/// Queries the current OS-level lock-key state.
///
/// This performs synchronous, non-blocking queries only (on Linux: a single
/// `EVIOCGLED` ioctl per candidate device via `evdev`). It does not read or
/// wait on input events, and is safe to call at startup and on focus-gain,
/// but should not be called in a per-keystroke hot path.
#[must_use]
pub fn query_lock_state() -> LockState {
    imp::query_lock_state()
}

#[cfg(target_os = "linux")]
mod imp {
    use super::LockState;
    use evdev::LedCode;

    /// Linux implementation: OR the LED state across every LED-capable
    /// input device found by `evdev::enumerate()`.
    ///
    /// Re-enumerates devices on every call (rather than caching the device
    /// list) so that hotplugged keyboards are picked up. Devices that fail
    /// to report LED state (e.g. a permissions error, or a race with
    /// unplugging) are skipped rather than failing the whole query.
    pub(super) fn query_lock_state() -> LockState {
        let mut state = LockState::default();

        for (path, device) in evdev::enumerate() {
            let Some(supported) = device.supported_leds() else {
                continue;
            };

            let has_caps = supported.contains(LedCode::LED_CAPSL);
            let has_num = supported.contains(LedCode::LED_NUML);
            let has_scroll = supported.contains(LedCode::LED_SCROLLL);

            if !has_caps && !has_num && !has_scroll {
                continue;
            }

            match device.get_led_state() {
                Ok(led_state) => {
                    state.caps |= has_caps && led_state.contains(LedCode::LED_CAPSL);
                    state.num |= has_num && led_state.contains(LedCode::LED_NUML);
                    state.scroll |= has_scroll && led_state.contains(LedCode::LED_SCROLLL);
                }
                Err(error) => {
                    tracing::trace!(
                        path = %path.display(),
                        %error,
                        "failed to read LED state from evdev device"
                    );
                }
            }
        }

        state
    }
}

#[cfg(not(target_os = "linux"))]
mod imp {
    use super::LockState;

    /// Non-Linux stub. Windows (`GetKeyState`) and macOS
    /// (`CGEventSourceFlagsState`) real queries are implemented in
    /// subtasks 114.2 and 114.3 respectively; until then, every lock is
    /// reported as inactive so the crate compiles and runs on every
    /// platform.
    pub(super) fn query_lock_state() -> LockState {
        LockState::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_state_default_is_all_false() {
        let state = LockState::default();
        assert!(!state.caps);
        assert!(!state.num);
        assert!(!state.scroll);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn query_lock_state_completes_without_panicking() {
        // The actual bool values are environment-dependent (they reflect
        // whatever the real hardware/kernel state is on the CI/dev box), so
        // this only asserts the call completes and returns a `LockState`.
        let _state: LockState = query_lock_state();
    }
}
