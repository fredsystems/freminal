// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Cross-crate logging helpers.
//!
//! The workspace has 38+ repeated blocks of the form:
//!
//! ```text
//! if let Err(e) = sender.send(payload) {
//!     tracing::error!("Failed to …: {e}");
//! }
//! ```
//!
//! These follow identical "send to a channel and log on failure" semantics:
//! the channel receiver has gone away, so the send failed, but the sender has
//! no recovery path — it can only log and carry on. The `send_or_log!` macro
//! below captures that shape with zero runtime overhead (it expands to the
//! same `if let` the hand-written code uses) while preserving the
//! `tracing` span context of the call site.

/// Send a value through a channel and log at `error!` level on failure.
///
/// Expands to:
///
/// ```ignore
/// if let Err(e) = $sender.send($payload) {
///     ::tracing::error!(concat!($msg, ": {}"), e);
/// }
/// ```
///
/// The log message is formatted as `"<msg>: <error>"`. Use the three-argument
/// form with a `target:` component when the call site needs to override the
/// `tracing` target.
///
/// # Examples
///
/// ```ignore
/// use freminal_common::send_or_log;
///
/// send_or_log!(input_tx, InputEvent::ScrollOffset(0), "Failed to send scroll offset");
/// ```
#[macro_export]
macro_rules! send_or_log {
    ($sender:expr, $payload:expr, $msg:literal $(,)?) => {
        if let ::core::result::Result::Err(e) = $sender.send($payload) {
            ::tracing::error!(concat!($msg, ": {}"), e);
        }
    };
    (target: $target:literal, $sender:expr, $payload:expr, $msg:literal $(,)?) => {
        if let ::core::result::Result::Err(e) = $sender.send($payload) {
            ::tracing::error!(target: $target, concat!($msg, ": {}"), e);
        }
    };
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    #[test]
    fn send_or_log_forwards_to_open_channel() {
        let (tx, rx) = mpsc::channel::<u32>();
        send_or_log!(tx, 42, "Failed to send test value");
        assert_eq!(rx.recv(), Ok(42));
    }

    #[test]
    fn send_or_log_is_noop_on_closed_channel() {
        let (tx, rx) = mpsc::channel::<u32>();
        drop(rx);
        // Must not panic — closed channel, just logs and continues.
        send_or_log!(tx, 42, "Failed to send test value");
    }
}
