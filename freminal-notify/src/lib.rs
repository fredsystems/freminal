// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Showing notifications on the operating system.
//!
//! This crate is the single owner of "show this notification on the OS": the
//! Linux/BSD D-Bus service, the macOS and Windows delivery workers, the icon
//! pipeline, markup escaping and the daemon capability types. OSC 99 protocol
//! logic, routing and capability policy are callers of this crate and live
//! in the `freminal` binary.
//!
//! Outside code uses only the items re-exported from this file, and nothing
//! reaches into a submodule. The crate may depend only on `freminal_common`
//! and external crates: nothing from the GUI, no egui, no `freminal_windowing`
//! and no `freminal_terminal_emulator`.
//!
//! The `test-utils` feature enables the `test_support` module: the recording
//! backends, `PlainSink::recording`, the bundled test assets and, on
//! Linux/BSD, the in-process fake notification daemon. A production build
//! never enables it.

mod assets;
mod backend;
mod capabilities;
mod icons;
#[cfg(all(unix, not(target_os = "macos")))]
mod linux;
#[cfg(all(unix, not(target_os = "macos")))]
mod markup;
mod notifier;
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
pub use backend::{
    NoTrackedBackend, NotificationInstanceId, SYSTEM_LEG, SystemDispatch, SystemLeg, TrackedBackend,
};
pub use capabilities::{DaemonCapabilities, DaemonSupport, LinuxDaemon, NotifyPlatform};
#[cfg(all(unix, not(target_os = "macos")))]
pub use linux::{SystemAction, SystemNotifyEvent};
pub use notifier::{DesktopNotifier, ServiceDemand};
pub use plain_sink::{PlainNotification, PlainSink};
pub use standard_icon::StandardIcon;
pub use untracked::{NoUntrackedBackend, UntrackedBackend};

/// Test doubles and fixtures for the callers' tests.
#[cfg(any(test, feature = "test-utils"))]
pub mod test_support {
    pub use super::assets::test_assets;
    pub use super::backend::{RecordedShow, RecordedShowAnswer, RecordingTrackedBackend};
    #[cfg(all(unix, not(target_os = "macos")))]
    pub use super::linux::test_rig::{TestResult, assert_no_queued_events, started};
    pub use super::plain_sink::PlainRecorder;
    pub use super::untracked::RecordingUntrackedBackend;
}
