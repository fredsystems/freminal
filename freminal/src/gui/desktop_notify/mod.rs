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
//! `freminal_common` and external crates: no `crate::gui::...`, no egui, no
//! `freminal_windowing` and no `freminal_terminal_emulator`.

mod assets;
mod capabilities;
mod icons;
#[cfg(all(unix, not(target_os = "macos")))]
mod markup;
#[cfg(all(unix, not(target_os = "macos")))]
mod presentation;
mod standard_icon;
mod untracked;

pub use assets::NotifyAssets;
#[cfg(any(all(unix, not(target_os = "macos")), test))]
pub use capabilities::Osc99DaemonCapabilities;
pub use capabilities::{Osc99DaemonSupport, Osc99LinuxDaemon, Osc99Platform};
pub use standard_icon::StandardIcon;
pub use untracked::{NoUntrackedBackend, Osc99UntrackedBackend, platform_untracked_backend};

// Used by osc99::dbus_service until it moves here (150.4b).
#[cfg(all(unix, not(target_os = "macos")))]
pub use icons::lookup::XdgIconLookup;
#[cfg(all(unix, not(target_os = "macos")))]
pub use presentation::{
    Osc99DecodedIconCache, Osc99IconChoice, Osc99IconImage, Osc99IconLookup, Osc99SoundHint,
    category_hint, choose_icon, decode_icon_image, escape_body_markup, sound_hint,
};

/// Test doubles and fixtures for the callers' tests.
#[cfg(test)]
pub mod test_support {
    pub use super::assets::test_assets;
    pub use super::untracked::RecordingUntrackedBackend;
}
