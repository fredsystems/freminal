// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Cell-based terminal buffer model for the Freminal terminal emulator.
//!
//! This crate implements the pure data model for terminal content. It is
//! responsible for cells, rows, cursor tracking, soft-wrapping, and producing
//! explicit mutation results. It does **not** parse escape sequences, implement
//! terminal semantics, perform rendering, interact with UI frameworks, or
//! access OS/platform APIs.
//!
//! Key types:
//! - [`buffer::Buffer`] — the primary terminal buffer, owning all rows and cursor state
//! - [`row::Row`] — a single row of terminal cells with wrapping metadata
//! - [`cell::Cell`] — the smallest addressable unit; always valid (empty cells are
//!   explicit)

pub mod buffer;
pub mod cell;
pub mod compact_row;
pub mod compressed_block;
pub mod image_store;
pub mod response;
pub mod row;
pub mod url_detect;
