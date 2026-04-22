// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! ANSI/VT terminal emulator library for the Freminal terminal emulator.
//!
//! This crate owns the ANSI parser (`FreminalAnsiParser`), terminal state
//! machine (`TerminalState`), and handler (`TerminalHandler`) that together
//! drive buffer mutations. It produces [`snapshot::TerminalSnapshot`] values for the GUI
//! via [`interface::TerminalEmulator::build_snapshot`].
//!
//! The crate does **not** render, interact with egui, or hold GUI state.
//! All terminal input events arrive through a `crossbeam_channel` and all
//! PTY write-backs go through a `Sender<PtyWrite>`.
//!
//! Key types:
//! - [`interface::TerminalEmulator`] — top-level owner; wraps `TerminalState` and manages
//!   snapshot publishing
//! - [`snapshot::TerminalSnapshot`] — immutable view of terminal state shared
//!   lock-free with the GUI via `ArcSwap`
//! - [`io::InputEvent`] — keyboard, resize, and focus events sent from the GUI
//! - [`io::WindowCommand`] — viewport and report commands sent to the GUI

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
    clippy::expect_used,
    // Task 70.H tripwires: explicit deny for the three cast lints that guard
    // against silent truncation/sign-loss/wrap in numeric conversions. These
    // are already part of `clippy::pedantic` above, but naming them directly
    // documents the contract and survives any future reorganization of the
    // pedantic group. All remaining `as` casts must be covered by a local
    // `#[allow(...)]` with a justification comment per agents.md.
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap
)]
#![allow(clippy::multiple_crate_versions)] // Allow multiple versions from transitive dependencies
#![allow(clippy::cargo_common_metadata)] // Metadata is inherited from workspace
#![allow(clippy::range_plus_one)]

pub mod ansi;
pub mod ansi_components;
pub mod error;

pub mod input;
pub mod interface;
pub mod io;
pub mod recording;
pub mod snapshot;
pub mod state;
pub mod terminal_handler;

#[macro_use]
extern crate tracing;

// Re-export image types so the `freminal` binary crate can use them without
// taking a direct dependency on `freminal-buffer`.
pub use freminal_buffer::image_store::{ImagePlacement, InlineImage};

// Re-export `LineWidth` for the renderer to apply DECDWL / DECDHL scaling.
pub use freminal_buffer::row::LineWidth;
