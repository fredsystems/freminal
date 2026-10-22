//! Showing notifications on the operating system.
//!
//! This module is the single owner of "show this notification on the OS": the
//! Linux/BSD D-Bus service, the macOS and Windows delivery workers, the icon
//! pipeline, markup escaping and the daemon capability types. OSC 99 protocol
//! logic, routing and capability policy are callers of this module and live
//! elsewhere in `gui`.
//!
//! The module is designed as if it were a crate. Outside code uses only the
//! items re-exported from this file, and nothing reaches into a submodule. It
//! is planned to become the `freminal-notify` crate, so it may depend only on
//! `freminal_common` and external crates: nothing from the rest of `gui`, no egui, no
//! `freminal_windowing` and no `freminal_terminal_emulator`.

mod assets;
mod backend;
mod capabilities;
mod icons;
#[cfg(all(unix, not(target_os = "macos")))]
mod linux;
#[cfg(all(unix, not(target_os = "macos")))]
mod markup;
mod plain_sink;
#[cfg(any(test, not(all(unix, not(target_os = "macos")))))]
mod plain_worker;
#[cfg(all(unix, not(target_os = "macos")))]
mod presentation;
#[cfg(any(test, not(all(unix, not(target_os = "macos")))))]
mod queue;
mod standard_icon;
mod untracked;

pub use assets::NotifyAssets;
pub use backend::{NoTrackedBackend, NotificationInstanceId, SystemDispatch, TrackedBackend};
#[cfg(any(all(unix, not(target_os = "macos")), test))]
pub use capabilities::DaemonCapabilities;
pub use capabilities::{DaemonSupport, LinuxDaemon, NotifyPlatform};
#[cfg(all(unix, not(target_os = "macos")))]
pub use linux::{LinuxNotifyHandle, LinuxNotifyService, SystemAction, SystemNotifyEvent};
pub use plain_sink::{PlainNotification, PlainSink};
pub use standard_icon::StandardIcon;
pub use untracked::{NoUntrackedBackend, UntrackedBackend, platform_untracked_backend};

/// Test doubles and fixtures for the callers' tests.
#[cfg(test)]
pub mod test_support {
    pub use super::assets::test_assets;
    pub use super::backend::{RecordedShow, RecordingTrackedBackend};
    #[cfg(all(unix, not(target_os = "macos")))]
    pub use super::linux::test_rig::{TestResult, assert_no_queued_events, started};
    #[cfg(all(unix, not(target_os = "macos")))]
    pub use super::linux::{DetachedQueue, detached_handle};
    pub use super::untracked::RecordingUntrackedBackend;
}
