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

pub mod ansi;
pub mod ansi_components;
pub mod error;

pub mod input;
pub mod interface;
pub mod io;
#[cfg(test)]
pub(crate) mod log_capture;
pub mod recording;
pub mod snapshot;
pub mod state;
pub mod terminal_handler;

#[macro_use]
extern crate tracing;

// Re-export image types so the `freminal` binary crate can use them without
// taking a direct dependency on `freminal-buffer`.
pub use freminal_buffer::image_store::{
    AnimationControl, AnimationRunMode, ImageFrame, ImagePlacement, ImageProtocol, ImageSizeMode,
    InlineImage, SourceCrop, SubCellOffset, next_placement_instance_id,
};

// Re-export `LineWidth` for the renderer to apply DECDWL / DECDHL scaling.
pub use freminal_buffer::row::LineWidth;

/// Git describe output for the current build.
///
/// Typical values: `v0.7.0-3-gabc1234` (commits past a tag) or `v0.7.0` (on
/// a tag), or `unknown` when `git describe` failed at build time (e.g. a
/// shallow clone with no tags).  Emitted by this crate's `build.rs` so the
/// binary crate can display it in the About dialog without needing its own
/// build script.
pub const GIT_DESCRIBE: &str = env!("VERGEN_GIT_DESCRIBE");
