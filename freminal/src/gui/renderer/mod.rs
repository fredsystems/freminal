// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Terminal rendering pipeline split into focused sub-modules.
//!
//! - [`gpu`] — [`TerminalRenderer`] struct, GL init/draw/destroy, shader compilation,
//!   VAO/VBO setup, and GL upload helpers.
//! - [`shaders`] — GLSL source string constants for the four shader passes
//!   (decoration, background, foreground, image).
//! - [`vertex`] — CPU-side vertex/instance builders, `FgRenderOptions`, and helpers.
//!   Contains the full test suite for vertex generation logic.

pub mod errors;
pub mod gpu;
pub(super) mod shaders;
pub mod vertex;

pub use gpu::{TerminalRenderer, WindowPostRenderer};
pub use vertex::{
    BackgroundFrame, CURSOR_QUAD_FLOATS, FgRenderOptions, MatchHighlight,
    build_background_instances, build_cursor_verts_only, build_foreground_instances,
    build_image_verts,
};
