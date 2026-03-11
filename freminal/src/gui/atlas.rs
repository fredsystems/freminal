// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Glyph atlas and rasterisation.
//!
//! Manages a single RGBA texture atlas that stores rasterised glyphs for the
//! terminal renderer. Monochrome glyphs are stored as white+alpha (tinted by
//! the foreground color in the shader). Color emoji are stored as full RGBA.
//! Uses shelf-based bin packing with LRU eviction.

/// A rasterised glyph's location and metadata within the atlas texture.
///
/// This struct will be fleshed out in subtask 1.4.
pub struct AtlasEntry {
    /// UV coordinates in the atlas texture: `(u_min, v_min, u_max, v_max)`.
    pub uv_rect: [f32; 4],
    /// Horizontal bearing offset from the cell origin (pixels).
    pub bearing_x: i16,
    /// Vertical bearing offset from the cell origin (pixels).
    pub bearing_y: i16,
    /// Width of the rasterised glyph image in pixels.
    pub width: u16,
    /// Height of the rasterised glyph image in pixels.
    pub height: u16,
    /// Whether this glyph is a color emoji (RGBA) vs monochrome (alpha-only).
    pub is_color: bool,
}

/// The glyph atlas texture and its lookup table.
///
/// This struct will be fleshed out in subtask 1.4.
pub struct GlyphAtlas {
    /// Atlas texture dimensions (always square, power of 2).
    size: u32,
}

impl GlyphAtlas {
    /// Create a new empty atlas with the given initial size.
    #[must_use]
    pub const fn new(initial_size: u32) -> Self {
        Self { size: initial_size }
    }

    /// Return the current atlas texture size in pixels.
    #[must_use]
    pub const fn size(&self) -> u32 {
        self.size
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atlas_constructs() {
        let atlas = GlyphAtlas::new(1024);
        assert_eq!(atlas.size(), 1024);
    }

    #[test]
    fn atlas_entry_fields() {
        let entry = AtlasEntry {
            uv_rect: [0.0, 0.0, 0.1, 0.1],
            bearing_x: 0,
            bearing_y: 10,
            width: 8,
            height: 16,
            is_color: false,
        };
        assert!(!entry.is_color);
        assert_eq!(entry.width, 8);
    }
}
