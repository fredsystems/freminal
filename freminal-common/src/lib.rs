// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Shared types and utilities for the Freminal terminal emulator workspace.
//!
//! This crate contains the data structures, enums, and helpers that are shared
//! across the other crates in the workspace (`freminal-buffer`,
//! `freminal-terminal-emulator`, and `freminal`). It has no terminal semantics
//! and no platform-specific dependencies beyond what is needed for type
//! definitions.
//!
//! Key modules:
//! - [`buffer_states`] — terminal cell format, cursor, colors, SGR, modes, and
//!   output types
//! - [`colors`] — terminal color representation and 256-color palette
//! - [`cursor`] — cursor position and visual style types
//! - [`themes`] — embedded color theme palettes
//! - [`buffer_states::fonts`] — font decoration and weight types
//! - [`pty_write`] — PTY write command types shared with the emulator

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

/// CLI argument types.
pub mod args;
/// Base-64 encoding/decoding utilities.
pub mod base64;
/// Terminal cell state types: format, cursor, colors, SGR, modes, and output.
pub mod buffer_states;
/// Terminal color representation and the 256-color xterm palette.
pub mod colors;
/// Application configuration loaded from TOML and CLI arguments.
pub mod config;
/// Cursor position and visual style types.
pub mod cursor;
/// Configurable key bindings: actions, key combos, and the binding map.
pub mod keybindings;
/// Layout file format types, parser, and resolver.
pub mod layout;
/// PTY write command types shared between the emulator and the OS PTY writer.
pub mod pty_write;
/// SGR (Select Graphic Rendition) parameter types.
pub mod sgr;
/// Terminal window size type.
pub mod terminal_size;
/// Embedded terminfo database blob.
pub mod terminfo;
/// Embedded color theme palettes.
pub mod themes;
/// Persisted ephemeral UI window geometry (e.g. Settings window).
pub mod window_state;

#[macro_use]
extern crate tracing;
