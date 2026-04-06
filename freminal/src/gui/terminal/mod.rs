// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Terminal widget module — split into focused sub-modules.
//!
//! The public surface of this module is unchanged from the original
//! `terminal.rs`: only [`FreminalTerminalWidget`] is re-exported.

pub(super) mod coords;
pub(super) mod input;
pub(super) mod widget;

pub use widget::FreminalTerminalWidget;
