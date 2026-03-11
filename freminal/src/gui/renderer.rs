// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! GL shader programs, vertex buffers, and draw calls for the terminal renderer.
//!
//! `TerminalRenderer` owns all GPU state for the custom terminal rendering pipeline:
//! background shader (solid-color quads), foreground shader (textured glyph quads
//! from the atlas), vertex buffers, and the atlas texture handle. Rendering is
//! triggered via egui's `PaintCallback` mechanism using `glow`.

/// Holds all GPU resources for the terminal renderer.
///
/// This struct will be fleshed out in subtask 1.5.
pub struct TerminalRenderer {
    /// Whether GPU resources have been created.
    initialized: bool,
}

impl TerminalRenderer {
    /// Create a new (uninitialized) renderer.
    ///
    /// Actual GPU resource creation requires a `glow::Context` and happens
    /// in `init()`, called on first use within the `PaintCallback`.
    #[must_use]
    pub const fn new() -> Self {
        Self { initialized: false }
    }

    /// Return whether GPU resources have been created.
    #[must_use]
    pub const fn initialized(&self) -> bool {
        self.initialized
    }
}

impl Default for TerminalRenderer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renderer_constructs() {
        let r = TerminalRenderer::new();
        assert!(!r.initialized());
    }
}
