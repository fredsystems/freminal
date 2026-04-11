// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Terminal widget module — split into focused sub-modules.
//!
//! The public surface of this module exports the main [`FreminalTerminalWidget`]
//! as well as pane-specific rendering types (`PaneRenderCache`, `RenderState`,
//! and `new_render_state`) required by the multi-pane GUI architecture.

pub(crate) mod coords;
pub(crate) mod input;
pub(crate) mod widget;

pub use widget::FreminalTerminalWidget;
pub use widget::{PaneRenderCache, RenderState, new_render_state};
