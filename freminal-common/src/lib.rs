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
//! - [`fonts`] — font decoration and weight types
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
    clippy::expect_used
)]
#![allow(clippy::multiple_crate_versions)] // Allow multiple versions from transitive dependencies
#![allow(clippy::cargo_common_metadata)] // Metadata is inherited from workspace

pub mod args;
pub mod base64;
pub mod buffer_states;
pub mod colors;
pub mod config;
pub mod cursor;
pub mod pty_write;
pub mod sgr;
pub mod terminal_size;
pub mod terminfo;
pub mod themes;

#[macro_use]
extern crate tracing;
