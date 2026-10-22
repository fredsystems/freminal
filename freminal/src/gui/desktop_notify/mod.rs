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

mod standard_icon;

pub use standard_icon::StandardIcon;
