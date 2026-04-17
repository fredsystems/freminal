// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! GUI application library for the Freminal terminal emulator.
//!
//! This crate implements the egui-based graphical front-end using the
//! `freminal-windowing` crate for window management. The render loop in
//! `update()` is a pure read of `TerminalSnapshot` — it performs no
//! terminal state mutation. All user input is routed through a
//! `Sender<InputEvent>` to the PTY processing thread.
//!
//! Key modules:
//! - [`gui`] — top-level GUI types, `FreminalGui`, and the `App` impl
//! - [`gui::terminal`] — terminal widget, input handling, and rendering
//! - [`gui::view_state`] — `ViewState` (scroll offset, mouse, focus) owned
//!   entirely by the GUI and never shared with the PTY thread

#![deny(
    clippy::pedantic,
    clippy::cargo,
    clippy::nursery,
    clippy::style,
    clippy::correctness,
    clippy::all,
    clippy::suspicious,
    clippy::complexity,
    clippy::perf,
    clippy::unwrap_used,
    clippy::expect_used
)]
#![allow(clippy::multiple_crate_versions)] // Allow multiple versions from transitive dependencies
#![allow(clippy::cargo_common_metadata)] // Metadata is inherited from workspace

#[macro_use]
extern crate tracing;

pub mod gui;
#[cfg(feature = "playback")]
pub mod playback;
