// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Typed errors for the GPU renderer.
//!
//! Replaces the previous `Result<(), String>` pattern across the renderer
//! surface with structured, matchable error variants.  Each variant carries
//! enough context (resource label, info log, underlying glow/image error) to
//! reproduce the string-based messages the renderer used to emit.
//!
//! ## Hierarchy
//!
//! - [`BufferAllocError`] — GL object creation (VAO, VBO, FBO, shader/program
//!   objects).
//! - [`ShaderCompileError`] — GLSL compilation and program link failures.
//! - [`TextureUploadError`] — texture object creation and image decode.
//! - [`GpuInitError`] — top-level error returned from public renderer entry
//!   points; flattens the three sub-errors via `#[from]`.
//!
//! External callers already log errors via `error!("... {e}")` — the `Display`
//! impls preserve the original message shape so log output is unchanged.

use std::path::PathBuf;

use thiserror::Error;

/// GL object creation failure (VAO, VBO, FBO, shader or program handle).
///
/// Corresponds to the `gl.create_*() -> Result<_, String>` surface in
/// [`glow`](https://docs.rs/glow/).  The `resource` label is a short,
/// static identifier (e.g. `"bg_inst VAO"`, `"foreground instance VBO 0"`)
/// that pinpoints the allocation site for debugging.
#[derive(Debug, Error)]
#[error("create {resource}: {message}")]
pub struct BufferAllocError {
    /// Static resource label describing which GL object failed to allocate.
    pub resource: &'static str,
    /// Driver-supplied error message from `glow`.
    pub message: String,
}

impl BufferAllocError {
    /// Build a new error with a static resource label and a driver message.
    #[must_use]
    pub fn new(resource: &'static str, message: impl Into<String>) -> Self {
        Self {
            resource,
            message: message.into(),
        }
    }
}

/// GLSL compilation or program-link failure.
///
/// The info log from the driver is captured verbatim so it survives the
/// transition to typed errors and can still be surfaced to the user.
#[derive(Debug, Error)]
pub enum ShaderCompileError {
    /// `gl.create_shader` failed before source upload.
    #[error("create {label} shader: {message}")]
    CreateShader {
        /// Shader label (e.g. `"wpr_user"`, `"bg_inst"`).
        label: &'static str,
        /// Driver-supplied error message.
        message: String,
    },

    /// `gl.compile_shader` returned a non-success status.  `info` holds the
    /// GLSL compiler log.
    #[error("compile {label} shader: {info}")]
    Compile {
        /// Shader label identifying which stage failed.
        label: &'static str,
        /// Driver-supplied compile info log.
        info: String,
    },

    /// `gl.create_program` failed.
    #[error("create {label} program: {message}")]
    CreateProgram {
        /// Program label (e.g. `"wpr_user"`).
        label: &'static str,
        /// Driver-supplied error message.
        message: String,
    },

    /// `gl.link_program` returned a non-success status.  `info` holds the
    /// linker log.
    #[error("link {label} program: {info}")]
    Link {
        /// Program label identifying which program failed to link.
        label: &'static str,
        /// Driver-supplied link info log.
        info: String,
    },
}

/// Texture or image-asset load failure.
///
/// Covers both GL texture-object allocation and decoding of user-supplied
/// image files used for backgrounds.
#[derive(Debug, Error)]
pub enum TextureUploadError {
    /// `gl.create_texture` failed.
    #[error("create {label} texture: {message}")]
    CreateTexture {
        /// Texture label (e.g. `"atlas"`, `"bg_img"`).
        label: &'static str,
        /// Driver-supplied error message.
        message: String,
    },

    /// Failed to decode an image file from disk (e.g. background image).
    #[error("load background image '{}': {source}", path.display())]
    ImageDecode {
        /// Path of the image that failed to decode.
        path: PathBuf,
        /// Underlying image-crate error.
        #[source]
        source: image::ImageError,
    },
}

/// Top-level renderer initialization error.
///
/// Returned from every public GPU entry point that used to return
/// `Result<(), String>`.  Wraps the three narrower error types via
/// `#[from]` so internal `?` conversions are ergonomic.
#[derive(Debug, Error)]
pub enum GpuInitError {
    /// A VAO, VBO, FBO, or shader/program handle failed to allocate.
    #[error(transparent)]
    BufferAlloc(#[from] BufferAllocError),

    /// A GLSL shader failed to compile or a program failed to link.
    #[error(transparent)]
    ShaderCompile(#[from] ShaderCompileError),

    /// Texture creation or image decoding failed.
    #[error(transparent)]
    TextureUpload(#[from] TextureUploadError),
}
