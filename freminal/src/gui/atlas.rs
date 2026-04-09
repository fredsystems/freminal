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

use conv2::{ApproxFrom, ValueFrom};
use rustc_hash::FxHashMap;

use swash::scale::image::Content;
use swash::scale::{Render, ScaleContext, Source, StrikeWith};
use swash::zeno::Format;

use super::font_manager::{FaceId, FontManager};

// ---------------------------------------------------------------------------
//  Public types
// ---------------------------------------------------------------------------

/// Uniquely identifies a rasterised glyph in the atlas.
///
/// The key includes the glyph ID, the face it belongs to, and the pixel size
/// so that different sizes of the same glyph are stored separately.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GlyphKey {
    /// Glyph ID in the font.
    pub glyph_id: u16,
    /// Which font face this glyph comes from.
    pub face_id: FaceId,
    /// Font size in pixels (rounded to nearest integer for cache key).
    pub size_px: u16,
}

/// A rasterised glyph's location and metadata within the atlas texture.
#[derive(Debug, Clone)]
pub struct AtlasEntry {
    /// UV coordinates in the atlas texture: `(u_min, v_min, u_max, v_max)`.
    pub uv_rect: [f32; 4],
    /// Horizontal bearing offset from the pen position (pixels).
    pub bearing_x: i16,
    /// Vertical bearing offset from the baseline (pixels, positive = up).
    pub bearing_y: i16,
    /// Width of the rasterised glyph image in pixels.
    pub width: u16,
    /// Height of the rasterised glyph image in pixels.
    pub height: u16,
    /// Whether this glyph is a color emoji (RGBA) vs monochrome (alpha-only).
    pub is_color: bool,
    /// Which shelf this entry lives in (for LRU tracking).
    shelf_idx: usize,
}

/// CPU-side rasterised glyph image before atlas insertion.
struct RasterizedGlyph {
    /// RGBA pixel data (always 4 bytes per pixel).
    data: Vec<u8>,
    /// Width in pixels.
    width: u32,
    /// Height in pixels.
    height: u32,
    /// Horizontal bearing from pen position.
    bearing_x: i32,
    /// Vertical bearing from baseline (positive = up).
    bearing_y: i32,
    /// Whether this is a color glyph (emoji).
    is_color: bool,
}

// ---------------------------------------------------------------------------
//  Shelf-based bin packing
// ---------------------------------------------------------------------------

/// A horizontal shelf (row) in the atlas for packing glyphs.
struct Shelf {
    /// Y coordinate of the top of this shelf in the atlas.
    y_origin: u32,
    /// Height of this shelf (tallest glyph placed so far).
    height: u32,
    /// Next available X position for placing a glyph.
    next_x: u32,
    /// LRU generation counter — updated on every access.
    last_used: u64,
    /// Number of glyphs stored in this shelf.
    glyph_count: u32,
}

/// Tracks a rectangular region that was modified and needs GPU upload.
#[derive(Debug, Clone, Copy)]
pub struct DirtyRect {
    /// X offset in the atlas texture.
    pub x: u32,
    /// Y offset in the atlas texture.
    pub y: u32,
    /// Width of the dirty region.
    pub width: u32,
    /// Height of the dirty region.
    pub height: u32,
}

// ---------------------------------------------------------------------------
//  GlyphAtlas
// ---------------------------------------------------------------------------

/// The glyph atlas texture and its lookup table.
///
/// Manages CPU-side RGBA pixel data, shelf-based bin packing, LRU eviction,
/// and delta-upload tracking.  GPU texture operations are handled externally
/// by the renderer using the dirty rects reported by [`Self::take_dirty_rects`].
pub struct GlyphAtlas {
    /// Atlas texture width and height in pixels (always square).
    size: u32,
    /// Maximum atlas size in pixels before refusing to grow further.
    max_size: u32,
    /// CPU-side RGBA pixel buffer (`size * size * 4` bytes).
    pixels: Vec<u8>,
    /// Glyph key → atlas entry lookup (`FxHash` for speed — keys are small,
    /// non-adversarial glyph IDs).
    entries: FxHashMap<GlyphKey, AtlasEntry>,
    /// Shelf list, ordered by Y position.
    shelves: Vec<Shelf>,
    /// Global LRU generation counter, incremented on each lookup/insert.
    generation: u64,
    /// Swash scale context for rasterisation (one per atlas instance).
    scale_ctx: ScaleContext,
    /// Regions modified since the last call to [`Self::take_dirty_rects`].
    dirty_rects: Vec<DirtyRect>,
    /// Whether the entire atlas was reallocated (grow) and needs a full upload.
    full_reupload: bool,
}

impl Default for GlyphAtlas {
    fn default() -> Self {
        Self::new(1024, 4096)
    }
}

impl GlyphAtlas {
    /// Create a new empty atlas with the given initial and maximum sizes.
    ///
    /// Both sizes should be powers of 2.
    #[must_use]
    pub fn new(initial_size: u32, max_size: u32) -> Self {
        let pixel_count = (initial_size as usize) * (initial_size as usize) * 4;
        Self {
            size: initial_size,
            max_size,
            pixels: vec![0u8; pixel_count],
            entries: FxHashMap::default(),
            shelves: Vec::new(),
            generation: 0,
            scale_ctx: ScaleContext::new(),
            dirty_rects: Vec::new(),
            full_reupload: true, // First frame needs a full upload.
        }
    }

    /// Return the current atlas texture size in pixels (square).
    #[must_use]
    pub const fn size(&self) -> u32 {
        self.size
    }

    /// Return the CPU-side RGBA pixel buffer.
    #[must_use]
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// Return the number of cached glyph entries.
    #[must_use]
    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }

    /// Look up a glyph in the cache.  Returns `None` on cache miss.
    ///
    /// Updates the LRU generation on cache hit.
    ///
    /// Note: this requires two `FxHashMap` lookups because reading
    /// `shelf_idx` from the entry borrows `self.entries`, and then
    /// `self.shelves.get_mut()` needs `&mut self` which invalidates that
    /// borrow.  The second lookup is required for the return reference.
    /// `FxHash` makes each lookup cheap enough that this is acceptable.
    pub fn get(&mut self, key: &GlyphKey) -> Option<&AtlasEntry> {
        // Lookup 1: read shelf_idx (borrow released before shelves mutation).
        let shelf_idx = self.entries.get(key)?.shelf_idx;

        // Only bump generation on actual hits.
        self.generation += 1;
        let current_gen = self.generation;
        if let Some(shelf) = self.shelves.get_mut(shelf_idx) {
            shelf.last_used = current_gen;
        }
        // Lookup 2: return the entry reference.
        self.entries.get(key)
    }

    /// Rasterise a glyph and insert it into the atlas.
    ///
    /// Returns the `AtlasEntry` for the newly inserted glyph, or `None` if
    /// rasterisation failed (missing glyph, unsupported format, etc.).
    ///
    /// If the atlas is full, LRU eviction is attempted.  If the atlas is at
    /// max size and eviction cannot free enough space, returns `None`.
    ///
    /// Callers must check the cache first via [`Self::get`] or
    /// [`Self::get_or_insert`] — this method assumes a cache miss.
    pub fn rasterize_and_insert(
        &mut self,
        key: GlyphKey,
        font_manager: &FontManager,
    ) -> Option<&AtlasEntry> {
        // Rasterise the glyph.
        let rasterized = self.rasterize_glyph(key, font_manager)?;

        // Find or create space in the atlas.
        self.insert_rasterized(key, &rasterized)
    }

    /// Get or insert a glyph: returns the cached entry on hit, rasterises on
    /// miss.
    ///
    /// This is the primary entry point for the renderer.  On cache hit this
    /// performs two `FxHashMap` lookups plus an LRU shelf bump (three lookups
    /// total — same as [`Self::get`]).  On miss it rasterises and inserts the
    /// glyph.
    pub fn get_or_insert(
        &mut self,
        key: GlyphKey,
        font_manager: &FontManager,
    ) -> Option<&AtlasEntry> {
        // Fast path: entry exists → update LRU and return.
        // We cannot use `if let Some(e) = self.get(&key)` because the
        // returned borrow would prevent the fallthrough to
        // `rasterize_and_insert`.  `contains_key` releases the borrow.
        if self.entries.contains_key(&key) {
            return self.get(&key);
        }
        // Cache miss — rasterise and insert.
        self.rasterize_and_insert(key, font_manager)
    }

    /// Take the list of dirty rects accumulated since the last call.
    ///
    /// The renderer calls this once per frame to determine which atlas regions
    /// need GPU upload via `gl.tex_sub_image_2d()`.
    pub fn take_dirty_rects(&mut self) -> Vec<DirtyRect> {
        std::mem::take(&mut self.dirty_rects)
    }

    /// Whether the entire atlas texture needs to be re-uploaded (after a grow).
    pub fn needs_full_reupload(&mut self) -> bool {
        std::mem::take(&mut self.full_reupload)
    }

    /// Invalidate the entire atlas (e.g. on font change).
    pub fn clear(&mut self) {
        self.entries.clear();
        self.shelves.clear();
        self.pixels.fill(0);
        self.dirty_rects.clear();
        self.full_reupload = true;
    }

    // -----------------------------------------------------------------------
    //  Rasterisation
    // -----------------------------------------------------------------------

    /// Rasterise a single glyph using swash.
    fn rasterize_glyph(
        &mut self,
        key: GlyphKey,
        font_manager: &FontManager,
    ) -> Option<RasterizedGlyph> {
        let font_ref = font_manager.swash_font_ref(key.face_id)?;

        let mut scaler = self
            .scale_ctx
            .builder(font_ref)
            .size(f32::from(key.size_px))
            .hint(true)
            .build();

        // Try color sources first, then fall back to outline.
        let sources = [
            Source::ColorOutline(0),
            Source::ColorBitmap(StrikeWith::BestFit),
            Source::Outline,
        ];

        let image = Render::new(&sources)
            .format(Format::Alpha)
            .render(&mut scaler, key.glyph_id)?;

        let is_color = image.content == Content::Color;
        let placement = image.placement;

        // Convert to RGBA.
        let rgba_data = match image.content {
            Content::Mask => {
                // Alpha-only → white + alpha for shader tinting.
                let mut rgba = Vec::with_capacity(image.data.len() * 4);
                for &alpha in &image.data {
                    rgba.extend_from_slice(&[255, 255, 255, alpha]);
                }
                rgba
            }
            Content::Color => {
                // Already RGBA.
                image.data
            }
            Content::SubpixelMask => {
                // Subpixel → treat as RGBA (per-channel coverage).
                // For terminal use we just use it as-is.
                image.data
            }
        };

        Some(RasterizedGlyph {
            data: rgba_data,
            width: placement.width,
            height: placement.height,
            bearing_x: placement.left,
            bearing_y: placement.top,
            is_color,
        })
    }

    // -----------------------------------------------------------------------
    //  Bin packing and insertion
    // -----------------------------------------------------------------------

    /// Insert a rasterised glyph into the atlas.
    fn insert_rasterized(&mut self, key: GlyphKey, glyph: &RasterizedGlyph) -> Option<&AtlasEntry> {
        let gw = glyph.width;
        let gh = glyph.height;

        // Zero-size glyphs (e.g. space) get a degenerate entry.
        if gw == 0 || gh == 0 {
            return Some(self.insert_zero_size_entry(key, glyph));
        }

        // 1-pixel padding between glyphs to avoid texture filtering bleed.
        let padded_w = gw + 1;
        let padded_h = gh + 1;

        // Try to find an existing shelf with enough room.
        let shelf_idx = self.find_shelf(padded_w, padded_h);

        let shelf_idx = shelf_idx.or_else(|| {
            // No shelf fits — try to create a new one.
            self.create_shelf(padded_h)
        });

        let shelf_idx = match shelf_idx {
            Some(idx) => idx,
            None => {
                // Atlas is full — try eviction then grow.
                if self.try_evict_and_retry(padded_w, padded_h) {
                    // Retry after eviction.
                    self.find_shelf(padded_w, padded_h)
                        .or_else(|| self.create_shelf(padded_h))?
                } else if self.try_grow() {
                    // Retry after growing.
                    self.find_shelf(padded_w, padded_h)
                        .or_else(|| self.create_shelf(padded_h))?
                } else {
                    // Cannot fit this glyph.
                    return None;
                }
            }
        };

        // Place the glyph in the shelf.
        Some(self.place_in_shelf(shelf_idx, key, glyph))
    }

    /// Insert a zero-size entry (for space characters, etc.).
    fn insert_zero_size_entry(&mut self, key: GlyphKey, glyph: &RasterizedGlyph) -> &AtlasEntry {
        let entry = AtlasEntry {
            uv_rect: [0.0, 0.0, 0.0, 0.0],
            bearing_x: i16::value_from(glyph.bearing_x).unwrap_or(0),
            bearing_y: i16::value_from(glyph.bearing_y).unwrap_or(0),
            width: 0,
            height: 0,
            is_color: glyph.is_color,
            shelf_idx: 0,
        };
        // Use entry API to insert and return a reference in one lookup.
        self.entries.entry(key).or_insert(entry)
    }

    /// Find an existing shelf that can fit a glyph of the given padded size.
    ///
    /// Prefers shelves whose height closely matches the glyph height to
    /// minimise wasted space.
    fn find_shelf(&self, padded_w: u32, padded_h: u32) -> Option<usize> {
        let mut best: Option<(usize, u32)> = None; // (index, waste)

        for (idx, shelf) in self.shelves.iter().enumerate() {
            // Check width: enough horizontal room?
            if shelf.next_x + padded_w > self.size {
                continue;
            }
            // Check height: glyph must fit in the shelf's allocated height.
            if padded_h > shelf.height {
                continue;
            }
            let waste = shelf.height - padded_h;
            if best.as_ref().is_none_or(|&(_, bw)| waste < bw) {
                best = Some((idx, waste));
            }
        }

        best.map(|(idx, _)| idx)
    }

    /// Create a new shelf below the existing ones.
    ///
    /// Returns `None` if there isn't enough vertical space.
    fn create_shelf(&mut self, padded_h: u32) -> Option<usize> {
        let y_after_last = self.shelves.last().map_or(0, |s| s.y_origin + s.height);

        if y_after_last + padded_h > self.size {
            return None; // Not enough vertical space.
        }

        let idx = self.shelves.len();
        self.shelves.push(Shelf {
            y_origin: y_after_last,
            height: padded_h,
            next_x: 0,
            last_used: self.generation,
            glyph_count: 0,
        });
        Some(idx)
    }

    /// Place a glyph into a specific shelf and blit its pixels.
    fn place_in_shelf(
        &mut self,
        shelf_idx: usize,
        key: GlyphKey,
        glyph: &RasterizedGlyph,
    ) -> &AtlasEntry {
        let gw = glyph.width;
        let gh = glyph.height;

        let current_shelf = &mut self.shelves[shelf_idx];
        let place_x = current_shelf.next_x;
        let place_y = current_shelf.y_origin;
        current_shelf.next_x += gw + 1; // +1 for padding
        current_shelf.last_used = self.generation;
        current_shelf.glyph_count += 1;

        // Blit glyph pixels into the atlas pixel buffer.
        self.blit_glyph(place_x, place_y, gw, gh, &glyph.data);

        // Record dirty rect for delta upload.
        self.dirty_rects.push(DirtyRect {
            x: place_x,
            y: place_y,
            width: gw,
            height: gh,
        });

        // Compute UV coordinates.
        let atlas_size_f = f32::approx_from(self.size).unwrap_or(1.0);
        let u_min = f32::approx_from(place_x).unwrap_or(0.0) / atlas_size_f;
        let v_min = f32::approx_from(place_y).unwrap_or(0.0) / atlas_size_f;
        let u_max = f32::approx_from(place_x + gw).unwrap_or(0.0) / atlas_size_f;
        let v_max = f32::approx_from(place_y + gh).unwrap_or(0.0) / atlas_size_f;

        let entry = AtlasEntry {
            uv_rect: [u_min, v_min, u_max, v_max],
            bearing_x: i16::value_from(glyph.bearing_x).unwrap_or(0),
            bearing_y: i16::value_from(glyph.bearing_y).unwrap_or(0),
            width: u16::value_from(gw).unwrap_or(0),
            height: u16::value_from(gh).unwrap_or(0),
            is_color: glyph.is_color,
            shelf_idx,
        };

        // Use entry API to insert and return a reference in one lookup.
        self.entries.entry(key).or_insert(entry)
    }

    /// Blit glyph RGBA data into the atlas pixel buffer.
    fn blit_glyph(&mut self, dst_x: u32, dst_y: u32, width: u32, height: u32, data: &[u8]) {
        let atlas_stride = (self.size as usize) * 4;

        for row in 0..height {
            let src_offset = (row as usize) * (width as usize) * 4;
            let dst_offset = ((dst_y + row) as usize) * atlas_stride + (dst_x as usize) * 4;
            let row_bytes = (width as usize) * 4;

            if src_offset + row_bytes <= data.len() && dst_offset + row_bytes <= self.pixels.len() {
                self.pixels[dst_offset..dst_offset + row_bytes]
                    .copy_from_slice(&data[src_offset..src_offset + row_bytes]);
            }
        }
    }

    // -----------------------------------------------------------------------
    //  LRU eviction
    // -----------------------------------------------------------------------

    /// Try to evict the least-recently-used shelf and return whether any
    /// space was freed.
    fn try_evict_and_retry(&mut self, _needed_w: u32, _needed_h: u32) -> bool {
        if self.shelves.is_empty() {
            return false;
        }

        // Find the LRU shelf.
        let lru_idx = self
            .shelves
            .iter()
            .enumerate()
            .min_by_key(|(_, s)| s.last_used)
            .map(|(idx, _)| idx);

        let Some(lru_idx) = lru_idx else {
            return false;
        };

        self.evict_shelf(lru_idx);
        true
    }

    /// Evict all glyphs from a shelf, freeing its horizontal space.
    fn evict_shelf(&mut self, shelf_idx: usize) {
        // Remove all entries that belong to this shelf.
        self.entries.retain(|_, entry| entry.shelf_idx != shelf_idx);

        // Reset the shelf's horizontal cursor so it can be reused.
        if let Some(shelf) = self.shelves.get_mut(shelf_idx) {
            shelf.next_x = 0;
            shelf.glyph_count = 0;
            shelf.last_used = self.generation;

            // Zero out the shelf's pixel region.
            let atlas_stride = (self.size as usize) * 4;
            for row in 0..shelf.height {
                let y = (shelf.y_origin + row) as usize;
                let offset = y * atlas_stride;
                let end = offset + atlas_stride;
                if end <= self.pixels.len() {
                    self.pixels[offset..end].fill(0);
                }
            }

            // Mark the entire shelf region as dirty.
            self.dirty_rects.push(DirtyRect {
                x: 0,
                y: shelf.y_origin,
                width: self.size,
                height: shelf.height,
            });
        }
    }

    // -----------------------------------------------------------------------
    //  Atlas growth
    // -----------------------------------------------------------------------

    /// Try to double the atlas size.  Returns `true` if growth succeeded.
    fn try_grow(&mut self) -> bool {
        let new_size = self.size * 2;
        if new_size > self.max_size {
            return false;
        }

        let new_pixel_count = (new_size as usize) * (new_size as usize) * 4;
        let mut new_pixels = vec![0u8; new_pixel_count];

        // Copy existing pixel data row by row.
        let old_stride = (self.size as usize) * 4;
        let new_stride = (new_size as usize) * 4;
        for row in 0..self.size {
            let old_offset = (row as usize) * old_stride;
            let new_offset = (row as usize) * new_stride;
            new_pixels[new_offset..new_offset + old_stride]
                .copy_from_slice(&self.pixels[old_offset..old_offset + old_stride]);
        }

        self.pixels = new_pixels;

        // Recompute UV rects for all existing entries.
        let new_size_f = f32::approx_from(new_size).unwrap_or(1.0);
        let old_size_f = f32::approx_from(self.size).unwrap_or(1.0);
        let scale = old_size_f / new_size_f;

        for entry in self.entries.values_mut() {
            entry.uv_rect[0] *= scale;
            entry.uv_rect[1] *= scale;
            entry.uv_rect[2] *= scale;
            entry.uv_rect[3] *= scale;
        }

        self.size = new_size;
        self.full_reupload = true;
        true
    }
}

// ---------------------------------------------------------------------------
//  Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use freminal_common::config::Config;

    /// Helper to get a `FontManager` for tests.
    fn test_font_manager() -> FontManager {
        FontManager::new(&Config::default(), 1.0)
    }

    /// Helper to create a standard glyph key.
    const fn test_key(glyph_id: u16) -> GlyphKey {
        GlyphKey {
            glyph_id,
            face_id: FaceId::PrimaryRegular,
            size_px: 16,
        }
    }

    #[test]
    fn atlas_default_size() {
        let atlas = GlyphAtlas::default();
        assert_eq!(atlas.size(), 1024);
        assert_eq!(atlas.pixels().len(), 1024 * 1024 * 4);
    }

    #[test]
    fn atlas_custom_size() {
        let atlas = GlyphAtlas::new(512, 2048);
        assert_eq!(atlas.size(), 512);
        assert_eq!(atlas.pixels().len(), 512 * 512 * 4);
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
            shelf_idx: 0,
        };
        assert!(!entry.is_color);
        assert_eq!(entry.width, 8);
    }

    #[test]
    fn rasterize_ascii_glyph() {
        let fm = test_font_manager();
        let mut atlas = GlyphAtlas::new(256, 1024);

        // Resolve glyph ID for 'A'.
        let mut fm_mut = fm;
        let style = super::super::font_manager::GlyphStyle::new(false, false);
        let (face_id, glyph_id) = fm_mut.resolve_glyph('A', style);

        let key = GlyphKey {
            glyph_id,
            face_id,
            size_px: 16,
        };

        let entry = atlas.rasterize_and_insert(key, &fm_mut);
        assert!(entry.is_some(), "should rasterise 'A' successfully");

        let entry = entry.unwrap();
        assert!(!entry.is_color, "'A' should be monochrome");
        assert!(entry.width > 0, "glyph width should be non-zero");
        assert!(entry.height > 0, "glyph height should be non-zero");
    }

    #[test]
    fn rasterize_produces_non_zero_alpha() {
        let fm = test_font_manager();
        let mut atlas = GlyphAtlas::new(256, 1024);

        let mut fm_mut = fm;
        let style = super::super::font_manager::GlyphStyle::new(false, false);
        let (face_id, glyph_id) = fm_mut.resolve_glyph('A', style);

        let key = GlyphKey {
            glyph_id,
            face_id,
            size_px: 16,
        };

        atlas.rasterize_and_insert(key, &fm_mut);

        // Check that the atlas pixels contain non-zero alpha in the glyph region.
        let has_nonzero_alpha = atlas.pixels().chunks_exact(4).any(|px| px[3] > 0);
        assert!(
            has_nonzero_alpha,
            "atlas should contain non-zero alpha pixels"
        );
    }

    #[test]
    fn cache_hit_returns_same_entry() {
        let fm = test_font_manager();
        let mut atlas = GlyphAtlas::new(256, 1024);

        let mut fm_mut = fm;
        let style = super::super::font_manager::GlyphStyle::new(false, false);
        let (face_id, glyph_id) = fm_mut.resolve_glyph('B', style);

        let key = GlyphKey {
            glyph_id,
            face_id,
            size_px: 16,
        };

        // First insertion.
        let entry1 = atlas.rasterize_and_insert(key, &fm_mut).unwrap().clone();

        // Second lookup should be a cache hit with the same UV rect.
        let entry2 = atlas.get(&key).unwrap();

        assert!(
            entry1
                .uv_rect
                .iter()
                .zip(entry2.uv_rect.iter())
                .all(|(a, b)| (a - b).abs() < f32::EPSILON),
            "UV rects should match between insert and cache hit"
        );
        assert_eq!(entry1.width, entry2.width);
        assert_eq!(entry1.height, entry2.height);
    }

    #[test]
    fn glyphs_do_not_overlap() {
        let fm = test_font_manager();
        let mut atlas = GlyphAtlas::new(256, 1024);

        let mut fm_mut = fm;
        let style = super::super::font_manager::GlyphStyle::new(false, false);

        // Insert several different glyphs.
        let mut entries = Vec::new();
        for ch in ['A', 'B', 'C', 'D', 'E'] {
            let (face_id, glyph_id) = fm_mut.resolve_glyph(ch, style);
            let key = GlyphKey {
                glyph_id,
                face_id,
                size_px: 16,
            };
            let entry = atlas.rasterize_and_insert(key, &fm_mut).unwrap().clone();
            entries.push(entry);
        }

        // Verify no overlapping UV rects.
        for (idx, entry_a) in entries.iter().enumerate() {
            if entry_a.width == 0 || entry_a.height == 0 {
                continue;
            }
            for entry_b in entries.iter().skip(idx + 1) {
                if entry_b.width == 0 || entry_b.height == 0 {
                    continue;
                }
                // Check AABB overlap.
                let overlap_x = entry_a.uv_rect[0] < entry_b.uv_rect[2]
                    && entry_a.uv_rect[2] > entry_b.uv_rect[0];
                let overlap_y = entry_a.uv_rect[1] < entry_b.uv_rect[3]
                    && entry_a.uv_rect[3] > entry_b.uv_rect[1];
                assert!(
                    !(overlap_x && overlap_y),
                    "glyphs should not overlap in the atlas"
                );
            }
        }
    }

    #[test]
    fn atlas_growth_preserves_entries() {
        // Use a tiny atlas that will need to grow.
        let fm = test_font_manager();
        let mut atlas = GlyphAtlas::new(32, 256);

        let mut fm_mut = fm;
        let style = super::super::font_manager::GlyphStyle::new(false, false);

        // Insert glyphs until the atlas grows.
        let initial_size = atlas.size();
        let mut inserted = Vec::new();

        for ch in 'A'..='Z' {
            let (face_id, glyph_id) = fm_mut.resolve_glyph(ch, style);
            let key = GlyphKey {
                glyph_id,
                face_id,
                size_px: 16,
            };
            if atlas.rasterize_and_insert(key, &fm_mut).is_some() {
                inserted.push(key);
            }
        }

        // If the atlas grew, verify old entries are still valid.
        if atlas.size() > initial_size {
            for key in &inserted {
                let entry = atlas.get(key);
                assert!(entry.is_some(), "entry should still exist after growth");
                let entry = entry.unwrap();
                // UV values should be within [0, 1].
                assert!(entry.uv_rect[0] >= 0.0 && entry.uv_rect[0] <= 1.0);
                assert!(entry.uv_rect[1] >= 0.0 && entry.uv_rect[1] <= 1.0);
                assert!(entry.uv_rect[2] >= 0.0 && entry.uv_rect[2] <= 1.0);
                assert!(entry.uv_rect[3] >= 0.0 && entry.uv_rect[3] <= 1.0);
            }
        }
    }

    #[test]
    fn eviction_frees_space() {
        // Use a very small atlas to force eviction.
        let fm = test_font_manager();
        let mut atlas = GlyphAtlas::new(32, 32); // Cannot grow.

        let mut fm_mut = fm;
        let style = super::super::font_manager::GlyphStyle::new(false, false);

        // Fill the atlas.
        let mut last_success = None;
        for ch in 'A'..='Z' {
            let (face_id, glyph_id) = fm_mut.resolve_glyph(ch, style);
            let key = GlyphKey {
                glyph_id,
                face_id,
                size_px: 16,
            };
            if atlas.rasterize_and_insert(key, &fm_mut).is_some() {
                last_success = Some(ch);
            }
        }

        // At least some glyphs should have been inserted (the atlas is tiny,
        // so we can't fit all 26, but eviction should allow progress).
        assert!(last_success.is_some(), "should insert at least one glyph");
    }

    #[test]
    fn dirty_rects_track_insertions() {
        let fm = test_font_manager();
        let mut atlas = GlyphAtlas::new(256, 1024);

        // Consume the initial full-reupload flag.
        let _ = atlas.needs_full_reupload();

        let mut fm_mut = fm;
        let style = super::super::font_manager::GlyphStyle::new(false, false);
        let (face_id, glyph_id) = fm_mut.resolve_glyph('X', style);
        let key = GlyphKey {
            glyph_id,
            face_id,
            size_px: 16,
        };

        atlas.rasterize_and_insert(key, &fm_mut);

        let rects = atlas.take_dirty_rects();
        assert!(!rects.is_empty(), "should have dirty rects after insertion");

        // Subsequent call should be empty (consumed).
        let rects2 = atlas.take_dirty_rects();
        assert!(rects2.is_empty(), "dirty rects should be consumed");
    }

    #[test]
    fn clear_resets_atlas() {
        let fm = test_font_manager();
        let mut atlas = GlyphAtlas::new(256, 1024);

        let mut fm_mut = fm;
        let style = super::super::font_manager::GlyphStyle::new(false, false);
        let (face_id, glyph_id) = fm_mut.resolve_glyph('A', style);
        let key = GlyphKey {
            glyph_id,
            face_id,
            size_px: 16,
        };

        atlas.rasterize_and_insert(key, &fm_mut);
        assert_eq!(atlas.entry_count(), 1);

        atlas.clear();
        assert_eq!(atlas.entry_count(), 0);
        assert!(atlas.get(&key).is_none());
    }

    #[test]
    fn get_or_insert_works() {
        let fm = test_font_manager();
        let mut atlas = GlyphAtlas::new(256, 1024);

        let mut fm_mut = fm;
        let style = super::super::font_manager::GlyphStyle::new(false, false);
        let (face_id, glyph_id) = fm_mut.resolve_glyph('Z', style);
        let key = GlyphKey {
            glyph_id,
            face_id,
            size_px: 16,
        };

        // First call — miss, rasterises.
        let entry1 = atlas.get_or_insert(key, &fm_mut).unwrap().clone();

        // Second call — hit, cached.
        let entry2 = atlas.get_or_insert(key, &fm_mut).unwrap();

        assert!(
            entry1
                .uv_rect
                .iter()
                .zip(entry2.uv_rect.iter())
                .all(|(a, b)| (a - b).abs() < f32::EPSILON),
            "UV rects should match between first and second get_or_insert"
        );
    }

    #[test]
    fn zero_glyph_key_is_usable() {
        // glyph_id 0 is the .notdef glyph — should still work.
        let fm = test_font_manager();
        let mut atlas = GlyphAtlas::new(256, 1024);

        let key = test_key(0);
        // May or may not rasterise (depends on font), but shouldn't panic.
        let _ = atlas.rasterize_and_insert(key, &fm);
    }

    #[test]
    fn shelf_based_packing_fills_horizontally() {
        let fm = test_font_manager();
        let mut atlas = GlyphAtlas::new(256, 1024);

        let mut fm_mut = fm;
        let style = super::super::font_manager::GlyphStyle::new(false, false);

        // Insert two glyphs; they should land in the same shelf.
        let (face1, gid1) = fm_mut.resolve_glyph('A', style);
        let (face2, gid2) = fm_mut.resolve_glyph('B', style);

        let key1 = GlyphKey {
            glyph_id: gid1,
            face_id: face1,
            size_px: 16,
        };
        let key2 = GlyphKey {
            glyph_id: gid2,
            face_id: face2,
            size_px: 16,
        };

        atlas.rasterize_and_insert(key1, &fm_mut);
        atlas.rasterize_and_insert(key2, &fm_mut);

        // Both should be in the same shelf (same shelf_idx).
        let e1 = atlas.entries.get(&key1).unwrap();
        let e2 = atlas.entries.get(&key2).unwrap();
        assert_eq!(
            e1.shelf_idx, e2.shelf_idx,
            "same-height glyphs should share a shelf"
        );

        // Second glyph should be to the right of the first.
        assert!(
            e2.uv_rect[0] > e1.uv_rect[0],
            "second glyph should be to the right"
        );
    }

    #[test]
    fn growth_doubles_size() {
        let mut atlas = GlyphAtlas::new(64, 256);
        assert_eq!(atlas.size(), 64);

        let grew = atlas.try_grow();
        assert!(grew, "should be able to grow");
        assert_eq!(atlas.size(), 128);

        let grew = atlas.try_grow();
        assert!(grew);
        assert_eq!(atlas.size(), 256);

        // At max size, should not grow further.
        let grew = atlas.try_grow();
        assert!(!grew, "should not grow beyond max");
        assert_eq!(atlas.size(), 256);
    }

    #[test]
    fn full_reupload_flag_after_growth() {
        let mut atlas = GlyphAtlas::new(64, 256);

        // Initial state: full reupload needed.
        assert!(atlas.needs_full_reupload());

        // After consuming, should be false.
        assert!(!atlas.needs_full_reupload());

        // After growth, should be true again.
        atlas.try_grow();
        assert!(atlas.needs_full_reupload());
    }
}
