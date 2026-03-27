// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Font loading, face management, and fallback chain.
//!
//! [`FontManager`] is the single authoritative source of font metrics and cell size
//! for the terminal renderer. It loads fonts via `swash`, provides `rustybuzz::Face`
//! references for the shaping pipeline, and resolves glyphs through a tiered
//! fallback chain: primary face -> bundled fallback -> emoji -> system -> tofu.

use std::collections::HashMap;
use std::path::Path;

use fontdb::Database;
use freminal_common::buffer_states::fonts::{FontDecorations, FontWeight};
use freminal_common::config::Config;

// ---------------------------------------------------------------------------
//  Bundled font data (compiled into the binary via include_bytes!)
// ---------------------------------------------------------------------------

static MESLO_REGULAR: &[u8] = include_bytes!("../../../res/MesloLGSNerdFontMono-Regular.ttf");
static MESLO_BOLD: &[u8] = include_bytes!("../../../res/MesloLGSNerdFontMono-Bold.ttf");
static MESLO_ITALIC: &[u8] = include_bytes!("../../../res/MesloLGSNerdFontMono-Italic.ttf");
static MESLO_BOLD_ITALIC: &[u8] =
    include_bytes!("../../../res/MesloLGSNerdFontMono-BoldItalic.ttf");

/// Emoji font family candidates, tried in order.
const EMOJI_CANDIDATES: &[&str] = &[
    "Apple Color Emoji",
    "Noto Color Emoji",
    "Segoe UI Emoji",
    "Twemoji",
    "Emoji One",
    "OpenMoji",
    "Emoji",
    "Symbola",
];

// ---------------------------------------------------------------------------
//  Supporting types
// ---------------------------------------------------------------------------

/// Identifies which face in the manager a glyph was resolved from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FaceId {
    /// Primary face (user font if configured, else bundled `MesloLGS`).
    PrimaryRegular,
    PrimaryBold,
    PrimaryItalic,
    PrimaryBoldItalic,
    /// Bundled `MesloLGS` — only present as fallback when a user font is primary.
    BundledRegular,
    BundledBold,
    BundledItalic,
    BundledBoldItalic,
    /// System emoji face.
    Emoji,
    /// Lazily-discovered system fallback face (index into `system_faces`).
    System(usize),
}

/// Style selector for glyph resolution, derived from format tags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GlyphStyle {
    pub bold: bool,
    pub italic: bool,
}

impl GlyphStyle {
    #[must_use]
    pub const fn new(bold: bool, italic: bool) -> Self {
        Self { bold, italic }
    }

    /// Construct from the buffer-layer font types.
    #[must_use]
    pub fn from_format(weight: &FontWeight, decorations: &[FontDecorations]) -> Self {
        Self {
            bold: *weight == FontWeight::Bold,
            italic: decorations.contains(&FontDecorations::Italic),
        }
    }
}

/// A loaded font face: owns the raw font data and caches the swash `CacheKey`.
///
/// The font data is stored as an enum to avoid copying bundled (`&'static`) data.
#[derive(Debug)]
struct LoadedFace {
    /// Raw font file bytes (owned or static).
    data: FontData,
    /// Index within the font collection (usually 0 for single-font files).
    index: usize,
    /// Swash cache key for this face.
    cache_key: swash::CacheKey,
}

/// Font data source — either compiled-in or heap-allocated.
#[derive(Debug)]
enum FontData {
    Static(&'static [u8]),
    Owned(Vec<u8>),
}

impl FontData {
    fn as_bytes(&self) -> &[u8] {
        match self {
            Self::Static(b) => b,
            Self::Owned(v) => v,
        }
    }
}

impl LoadedFace {
    /// Load from static (bundled) font data.
    fn from_static(data: &'static [u8]) -> Option<Self> {
        let font_ref = swash::FontRef::from_index(data, 0)?;
        Some(Self {
            data: FontData::Static(data),
            index: 0,
            cache_key: font_ref.key,
        })
    }

    /// Load from owned font data.
    fn from_owned(data: Vec<u8>, index: usize) -> Option<Self> {
        let font_ref = swash::FontRef::from_index(&data, index)?;
        let key = font_ref.key;
        Some(Self {
            data: FontData::Owned(data),
            index,
            cache_key: key,
        })
    }

    /// The swash cache key for this face.
    const fn cache_key(&self) -> swash::CacheKey {
        self.cache_key
    }

    /// Create a `swash::FontRef` that borrows this face's data.
    fn as_font_ref(&self) -> Option<swash::FontRef<'_>> {
        swash::FontRef::from_index(self.as_bytes(), self.index)
    }

    fn as_bytes(&self) -> &[u8] {
        self.data.as_bytes()
    }

    /// Check if this face's charmap contains the given codepoint.
    fn has_glyph(&self, c: char) -> bool {
        self.as_font_ref().is_some_and(|f| f.charmap().map(c) != 0)
    }

    /// Map a codepoint to a glyph ID, returning 0 (`.notdef`) if unmapped.
    fn map_char(&self, c: char) -> u16 {
        self.as_font_ref().map_or(0, |f| f.charmap().map(c))
    }
}

/// The four style variants of a single font family.
struct PrimaryFaces {
    regular: LoadedFace,
    bold: LoadedFace,
    italic: LoadedFace,
    bold_italic: LoadedFace,
}

impl PrimaryFaces {
    /// Get the face matching the requested style.
    const fn get(&self, style: GlyphStyle) -> &LoadedFace {
        match (style.bold, style.italic) {
            (false, false) => &self.regular,
            (true, false) => &self.bold,
            (false, true) => &self.italic,
            (true, true) => &self.bold_italic,
        }
    }

    /// Get the face ID matching the requested style.
    const fn face_id(style: GlyphStyle, is_bundled_tier: bool) -> FaceId {
        if is_bundled_tier {
            match (style.bold, style.italic) {
                (false, false) => FaceId::BundledRegular,
                (true, false) => FaceId::BundledBold,
                (false, true) => FaceId::BundledItalic,
                (true, true) => FaceId::BundledBoldItalic,
            }
        } else {
            match (style.bold, style.italic) {
                (false, false) => FaceId::PrimaryRegular,
                (true, false) => FaceId::PrimaryBold,
                (false, true) => FaceId::PrimaryItalic,
                (true, true) => FaceId::PrimaryBoldItalic,
            }
        }
    }
}

/// Result of a [`FontManager::rebuild`] call, indicating what changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RebuildResult {
    /// Nothing changed — no work needed.
    NoChange,
    /// Font family changed — faces reloaded, cell size recomputed.
    FamilyChanged,
    /// Only font size changed — cell size recomputed, faces kept.
    SizeChanged,
}

impl RebuildResult {
    /// Returns `true` if any font-related state was invalidated.
    #[must_use]
    pub const fn font_changed(self) -> bool {
        matches!(self, Self::FamilyChanged | Self::SizeChanged)
    }
}

// ---------------------------------------------------------------------------
//  FontManager
// ---------------------------------------------------------------------------

/// Font manager for the terminal renderer.
///
/// Owns the font stack (primary faces, emoji, system fallback), computes cell size
/// from font metrics, and resolves individual glyphs to (face, `glyph_id`) pairs.
pub struct FontManager {
    /// Primary face stack (regular, bold, italic, bold-italic).
    /// If a user font is configured, it becomes primary; bundled `MesloLGS`
    /// becomes the first fallback tier.
    primary: PrimaryFaces,

    /// Bundled `MesloLGS` faces — present as a fallback tier only when a user
    /// font is active as primary. `None` when `MesloLGS` is already primary.
    bundled_fallback: Option<PrimaryFaces>,

    /// System emoji face (Noto Color Emoji, Apple Color Emoji, etc.).
    emoji_face: Option<LoadedFace>,

    /// Lazily-discovered system fallback faces, keyed by codepoint.
    system_fallback_cache: HashMap<char, Option<usize>>,

    /// Heap of system fallback faces discovered so far.
    system_faces: Vec<LoadedFace>,

    /// Resolved glyph cache: (codepoint, style) -> (`face_id`, `glyph_id`).
    glyph_cache: HashMap<(char, GlyphStyle), (FaceId, u16)>,

    /// Authoritative cell width in integer pixels.
    cell_width: u32,

    /// Authoritative cell height in integer pixels.
    cell_height: u32,

    /// Baseline offset in pixels (ascent + top padding for headroom).
    ascent: f32,

    /// Scaled descent in pixels (for glyph baseline positioning).
    descent: f32,

    /// Scaled underline offset from baseline, in pixels.
    underline_offset: f32,

    /// Scaled strikethrough offset from baseline, in pixels.
    strikeout_offset: f32,

    /// Scaled stroke thickness in pixels.
    stroke_size: f32,

    /// Font size in points (as specified by the user in config).
    font_size_pt: f32,

    /// Display scale factor (`egui::Context::pixels_per_point()`).
    ///
    /// Used together with `font_size_pt` to compute the correct ppem value
    /// for swash metric queries: `ppem = font_size_pt × (96/72) × pixels_per_point`.
    pixels_per_point: f32,

    /// The currently-active font family name (or `None` for bundled default).
    current_family: Option<String>,

    /// fontdb database for system font discovery.
    font_db: Database,
}

impl FontManager {
    /// Create a new `FontManager` with the given configuration.
    ///
    /// Loads the primary faces (user font or bundled `MesloLGS`), discovers a
    /// system emoji font, and computes the authoritative cell size.
    ///
    /// `pixels_per_point` is the display scale factor from
    /// `egui::Context::pixels_per_point()`. It is used together with the
    /// configured font size (in typographic points) to compute the correct
    /// ppem value for swash metric queries.
    #[must_use]
    pub fn new(config: &Config, pixels_per_point: f32) -> Self {
        let mut font_db = Database::new();
        font_db.load_system_fonts();

        let bundled = load_bundled_faces();
        let font_size_pt = config.font.size;

        let (primary, bundled_fallback, current_family) =
            if let Some(family) = config.font.family.as_deref() {
                if let Some(user_primary) = load_user_faces(family, &font_db) {
                    info!("Loaded user font '{}' as primary", family);
                    (user_primary, Some(bundled), Some(family.to_owned()))
                } else {
                    let suggestions = suggest_similar_families(family, &font_db);
                    if suggestions.is_empty() {
                        warn!(
                            "Failed to load user font '{}'; falling back to bundled MesloLGS",
                            family
                        );
                    } else {
                        warn!(
                            "Failed to load user font '{}'; falling back to bundled MesloLGS. \
                             Similar families found: {}",
                            family,
                            suggestions.join(", ")
                        );
                    }
                    (bundled, None, None)
                }
            } else {
                (bundled, None, None)
            };

        let emoji_face = discover_emoji_face(&font_db);
        if emoji_face.is_some() {
            info!("Discovered system emoji font");
        } else {
            warn!("No system emoji font found");
        }

        let font_size_ppem = pt_to_ppem(font_size_pt, pixels_per_point);
        let (
            cell_width,
            cell_height,
            ascent,
            descent,
            underline_offset,
            strikeout_offset,
            stroke_size,
        ) = compute_cell_metrics(&primary.regular, font_size_ppem);

        Self {
            primary,
            bundled_fallback,
            emoji_face,
            system_fallback_cache: HashMap::new(),
            system_faces: Vec::new(),
            glyph_cache: HashMap::new(),
            cell_width,
            cell_height,
            ascent,
            descent,
            underline_offset,
            strikeout_offset,
            stroke_size,
            font_size_pt,
            pixels_per_point,
            current_family,
            font_db,
        }
    }

    // -----------------------------------------------------------------------
    //  Public accessors
    // -----------------------------------------------------------------------

    /// Authoritative cell width in integer pixels.
    #[must_use]
    pub const fn cell_width(&self) -> u32 {
        self.cell_width
    }

    /// Authoritative cell height in integer pixels.
    #[must_use]
    pub const fn cell_height(&self) -> u32 {
        self.cell_height
    }

    /// Cell size as `(width, height)` in integer pixels.
    #[must_use]
    pub const fn cell_size(&self) -> (u32, u32) {
        (self.cell_width, self.cell_height)
    }

    /// Baseline offset in pixels (ascent + top padding).
    ///
    /// The renderer uses this as the Y distance from the top of each cell row
    /// to the text baseline.  Includes a small top pad so glyphs are not flush
    /// against the top edge of the cell.
    #[must_use]
    pub const fn ascent(&self) -> f32 {
        self.ascent
    }

    /// Scaled font descent in pixels (distance from baseline to bottom, positive).
    #[must_use]
    pub const fn descent(&self) -> f32 {
        self.descent
    }

    /// Scaled underline offset from baseline in pixels.
    #[must_use]
    pub const fn underline_offset(&self) -> f32 {
        self.underline_offset
    }

    /// Scaled strikethrough offset from baseline in pixels.
    #[must_use]
    pub const fn strikeout_offset(&self) -> f32 {
        self.strikeout_offset
    }

    /// Scaled stroke thickness in pixels.
    #[must_use]
    pub const fn stroke_size(&self) -> f32 {
        self.stroke_size
    }

    /// Current font size in points.
    #[must_use]
    pub const fn font_size_pt(&self) -> f32 {
        self.font_size_pt
    }

    // -----------------------------------------------------------------------
    //  Glyph resolution
    // -----------------------------------------------------------------------

    /// Resolve a codepoint + style to a `(FaceId, glyph_id)` pair.
    ///
    /// Tries each fallback tier in order:
    /// 1. Primary face (user font or bundled `MesloLGS`)
    /// 2. Bundled fallback (`MesloLGS`, only when a user font is primary)
    /// 3. Emoji face
    /// 4. System fallback (fontdb discovery, cached per codepoint)
    /// 5. Tofu (`.notdef` glyph from the primary regular face)
    ///
    /// Results are cached per `(codepoint, style)` pair.
    pub fn resolve_glyph(&mut self, codepoint: char, style: GlyphStyle) -> (FaceId, u16) {
        if let Some(&cached) = self.glyph_cache.get(&(codepoint, style)) {
            return cached;
        }

        let result = self.resolve_glyph_uncached(codepoint, style);
        self.glyph_cache.insert((codepoint, style), result);
        result
    }

    /// Get the raw font data bytes for a given `FaceId`.
    ///
    /// Returns `None` if the face is not loaded (e.g. a bundled fallback
    /// when no user font is active).
    #[must_use]
    pub fn face_data(&self, face_id: FaceId) -> Option<&[u8]> {
        self.get_loaded_face(face_id).map(LoadedFace::as_bytes)
    }

    /// Get the font collection index for a given `FaceId`.
    ///
    /// Returns `None` if the face is not loaded.
    #[must_use]
    pub fn face_index(&self, face_id: FaceId) -> Option<usize> {
        self.get_loaded_face(face_id).map(|f| f.index)
    }

    /// Create a `rustybuzz::Face` for the given `FaceId`.
    ///
    /// The returned `Face` borrows the font data owned by this `FontManager`,
    /// so the caller must not outlive `&self`.
    ///
    /// Returns `None` if the face is not loaded or the data cannot be parsed.
    #[must_use]
    pub fn rustybuzz_face(&self, face_id: FaceId) -> Option<rustybuzz::Face<'_>> {
        let loaded = self.get_loaded_face(face_id)?;
        #[allow(clippy::cast_possible_truncation)]
        rustybuzz::Face::from_slice(loaded.as_bytes(), loaded.index as u32)
    }

    /// Create a `swash::FontRef` for the given `FaceId`.
    ///
    /// Returns `None` if the face is not loaded.
    #[must_use]
    pub fn swash_font_ref(&self, face_id: FaceId) -> Option<swash::FontRef<'_>> {
        self.get_loaded_face(face_id)?.as_font_ref()
    }

    /// Get the swash `CacheKey` for a given `FaceId`.
    ///
    /// Returns `None` if the face is not loaded.
    #[must_use]
    pub fn face_cache_key(&self, face_id: FaceId) -> Option<swash::CacheKey> {
        self.get_loaded_face(face_id).map(LoadedFace::cache_key)
    }

    // -----------------------------------------------------------------------
    //  Hot reload
    // -----------------------------------------------------------------------

    /// Compare the current configuration against the given `Config` and reload
    /// fonts / recompute cell size as needed.
    ///
    /// Returns a [`RebuildResult`] indicating what changed so the caller can
    /// invalidate the glyph atlas and shaping cache if necessary.
    ///
    /// `pixels_per_point` is the current display scale factor; it may have
    /// changed since the last rebuild (e.g. the window was dragged to a
    /// different monitor).
    pub fn rebuild(&mut self, config: &Config, pixels_per_point: f32) -> RebuildResult {
        let new_family = config.font.family.as_deref();
        let new_size = config.font.size;

        let requested_family_differs = new_family != self.current_family.as_deref();
        let size_changed = (new_size - self.font_size_pt).abs() > f32::EPSILON;
        let ppp_changed = (pixels_per_point - self.pixels_per_point).abs() > f32::EPSILON;

        if !requested_family_differs && !size_changed && !ppp_changed {
            return RebuildResult::NoChange;
        }

        // Track whether the effective font family actually changed (as opposed
        // to the config requesting a font that fails to load and falls back to
        // the same bundled MesloLGS that was already active).
        let old_effective = self.current_family.clone();
        let mut effective_family_changed = false;

        if requested_family_differs {
            let bundled = load_bundled_faces();

            let (primary, bundled_fallback, current_family) = if let Some(family) = new_family {
                if let Some(user_primary) = load_user_faces(family, &self.font_db) {
                    info!("Reloaded user font '{}' as primary", family);
                    (user_primary, Some(bundled), Some(family.to_owned()))
                } else {
                    warn!(
                        "Failed to reload user font '{}'; using bundled MesloLGS",
                        family
                    );
                    (bundled, None, None)
                }
            } else {
                (bundled, None, None)
            };

            effective_family_changed = current_family != old_effective;
            self.primary = primary;
            self.bundled_fallback = bundled_fallback;
            self.current_family = current_family;
        }

        self.font_size_pt = new_size;
        self.pixels_per_point = pixels_per_point;
        let font_size_ppem = pt_to_ppem(self.font_size_pt, self.pixels_per_point);
        let (cw, ch, ascent, descent, uo, so, ss) =
            compute_cell_metrics(&self.primary.regular, font_size_ppem);
        self.cell_width = cw;
        self.cell_height = ch;
        self.ascent = ascent;
        self.descent = descent;
        self.underline_offset = uo;
        self.strikeout_offset = so;
        self.stroke_size = ss;

        // Clear caches — glyph IDs and system face mappings may differ.
        self.glyph_cache.clear();
        self.system_fallback_cache.clear();
        self.system_faces.clear();

        if effective_family_changed {
            RebuildResult::FamilyChanged
        } else if size_changed || ppp_changed {
            RebuildResult::SizeChanged
        } else {
            // The config requested a different family, but after attempting to
            // load it, the effective font is the same (e.g. both old and new
            // fell back to bundled MesloLGS).  No observable change.
            RebuildResult::NoChange
        }
    }

    /// Check whether `pixels_per_point` has changed and recompute cell metrics
    /// if so.  Returns `true` when metrics were recomputed (callers should
    /// invalidate the glyph atlas and shaping cache).
    ///
    /// This is a lightweight per-frame check intended to handle monitor DPI
    /// changes (e.g. dragging the window to a `HiDPI` display) without requiring
    /// a full config reload.
    pub fn update_pixels_per_point(&mut self, pixels_per_point: f32) -> bool {
        if (pixels_per_point - self.pixels_per_point).abs() <= f32::EPSILON {
            return false;
        }

        self.pixels_per_point = pixels_per_point;
        let font_size_ppem = pt_to_ppem(self.font_size_pt, self.pixels_per_point);
        let (cw, ch, ascent, descent, uo, so, ss) =
            compute_cell_metrics(&self.primary.regular, font_size_ppem);
        self.cell_width = cw;
        self.cell_height = ch;
        self.ascent = ascent;
        self.descent = descent;
        self.underline_offset = uo;
        self.strikeout_offset = so;
        self.stroke_size = ss;

        // Clear caches — glyph sizes differ at new ppem.
        self.glyph_cache.clear();
        self.system_fallback_cache.clear();
        self.system_faces.clear();

        true
    }

    // -----------------------------------------------------------------------
    //  Internal helpers
    // -----------------------------------------------------------------------

    /// Resolve a glyph without consulting the cache.
    fn resolve_glyph_uncached(&mut self, codepoint: char, style: GlyphStyle) -> (FaceId, u16) {
        // 1. Primary face
        let primary_face = self.primary.get(style);
        let gid = primary_face.map_char(codepoint);
        if gid != 0 {
            return (PrimaryFaces::face_id(style, false), gid);
        }

        // 2. Bundled fallback (only when user font is primary)
        if let Some(bundled) = &self.bundled_fallback {
            let bundled_face = bundled.get(style);
            let gid = bundled_face.map_char(codepoint);
            if gid != 0 {
                return (PrimaryFaces::face_id(style, true), gid);
            }
        }

        // 3. Emoji face
        if let Some(emoji) = &self.emoji_face {
            let gid = emoji.map_char(codepoint);
            if gid != 0 {
                return (FaceId::Emoji, gid);
            }
        }

        // 4. System fallback (lazy discovery via fontdb)
        if let Some(result) = self.try_system_fallback(codepoint) {
            return result;
        }

        // 5. Tofu — return .notdef (glyph 0) from primary regular face
        (FaceId::PrimaryRegular, 0)
    }

    /// Try to find a system font containing the given codepoint.
    fn try_system_fallback(&mut self, codepoint: char) -> Option<(FaceId, u16)> {
        // Check if we've already looked up this codepoint.
        if let Some(cached_idx) = self.system_fallback_cache.get(&codepoint) {
            return cached_idx.map(|idx| {
                let gid = self.system_faces[idx].map_char(codepoint);
                (FaceId::System(idx), gid)
            });
        }

        // Check existing system faces first.
        for (idx, face) in self.system_faces.iter().enumerate() {
            if face.has_glyph(codepoint) {
                let gid = face.map_char(codepoint);
                self.system_fallback_cache.insert(codepoint, Some(idx));
                return Some((FaceId::System(idx), gid));
            }
        }

        // Search fontdb for a face covering this codepoint.
        let result = find_system_face_for_char(&self.font_db, codepoint);

        if let Some(loaded) = result {
            let idx = self.system_faces.len();
            let gid = loaded.map_char(codepoint);
            self.system_faces.push(loaded);
            self.system_fallback_cache.insert(codepoint, Some(idx));
            Some((FaceId::System(idx), gid))
        } else {
            self.system_fallback_cache.insert(codepoint, None);
            None
        }
    }

    /// Look up a `LoadedFace` by its `FaceId`.
    fn get_loaded_face(&self, face_id: FaceId) -> Option<&LoadedFace> {
        match face_id {
            FaceId::PrimaryRegular => Some(&self.primary.regular),
            FaceId::PrimaryBold => Some(&self.primary.bold),
            FaceId::PrimaryItalic => Some(&self.primary.italic),
            FaceId::PrimaryBoldItalic => Some(&self.primary.bold_italic),
            FaceId::BundledRegular => self.bundled_fallback.as_ref().map(|b| &b.regular),
            FaceId::BundledBold => self.bundled_fallback.as_ref().map(|b| &b.bold),
            FaceId::BundledItalic => self.bundled_fallback.as_ref().map(|b| &b.italic),
            FaceId::BundledBoldItalic => self.bundled_fallback.as_ref().map(|b| &b.bold_italic),
            FaceId::Emoji => self.emoji_face.as_ref(),
            FaceId::System(idx) => self.system_faces.get(idx),
        }
    }
}

impl Default for FontManager {
    fn default() -> Self {
        Self::new(&Config::default(), 1.0)
    }
}

// ---------------------------------------------------------------------------
//  Free functions
// ---------------------------------------------------------------------------

/// Load all four bundled `MesloLGS` Nerd Font Mono faces.
///
/// # Panics
///
/// Panics if the bundled font data is corrupt and cannot be parsed by swash.
/// This is a build-time invariant — the font files are embedded via
/// `include_bytes!` and should always be valid.
fn load_bundled_faces() -> PrimaryFaces {
    // Safety: bundled fonts are known-good TTF files. If they fail to parse,
    // it indicates a build/packaging error, not a runtime condition.
    let regular = LoadedFace::from_static(MESLO_REGULAR)
        .unwrap_or_else(|| unreachable!("bundled MesloLGS-Regular.ttf is corrupt"));
    let bold = LoadedFace::from_static(MESLO_BOLD)
        .unwrap_or_else(|| unreachable!("bundled MesloLGS-Bold.ttf is corrupt"));
    let italic = LoadedFace::from_static(MESLO_ITALIC)
        .unwrap_or_else(|| unreachable!("bundled MesloLGS-Italic.ttf is corrupt"));
    let bold_italic = LoadedFace::from_static(MESLO_BOLD_ITALIC)
        .unwrap_or_else(|| unreachable!("bundled MesloLGS-BoldItalic.ttf is corrupt"));

    PrimaryFaces {
        regular,
        bold,
        italic,
        bold_italic,
    }
}

/// Attempt to load a user font by file path or system font name.
///
/// If the string is an existing file, loads it directly. Otherwise, searches
/// `fontdb` for a matching family name. Returns `None` on failure.
fn load_user_faces(path_or_name: &str, font_db: &Database) -> Option<PrimaryFaces> {
    // Try file path first.
    let path = Path::new(path_or_name);
    if path.exists()
        && path.is_file()
        && let Ok(data) = std::fs::read(path)
    {
        return build_user_primary_from_data(data, path_or_name, font_db);
    }

    // Try system font name lookup.
    load_user_faces_by_name(path_or_name, font_db)
}

/// Build a `PrimaryFaces` from owned font data (the regular face).
///
/// Searches `fontdb` for bold, italic, and bold-italic variants of the same
/// family. Falls back to the regular face for missing variants.
fn build_user_primary_from_data(
    regular_data: Vec<u8>,
    _hint: &str,
    _font_db: &Database,
) -> Option<PrimaryFaces> {
    let regular = LoadedFace::from_owned(regular_data.clone(), 0)?;

    // For a file-path font, we only have one file. Use the same data for all
    // style variants. In the future this could search the same directory for
    // Bold/Italic/BoldItalic variants.
    let bold = LoadedFace::from_owned(regular_data.clone(), 0)?;
    let italic = LoadedFace::from_owned(regular_data.clone(), 0)?;
    let bold_italic = LoadedFace::from_owned(regular_data, 0)?;

    Some(PrimaryFaces {
        regular,
        bold,
        italic,
        bold_italic,
    })
}

/// Load a user font by family name from the system font database.
///
/// Searches for regular, bold, italic, and bold-italic variants. Falls back
/// to the regular face for missing variants.
fn load_user_faces_by_name(name: &str, font_db: &Database) -> Option<PrimaryFaces> {
    // Find the family in fontdb (case-insensitive substring match).
    let regular_data =
        find_system_font_data(font_db, name, fontdb::Weight::NORMAL, fontdb::Style::Normal)?;
    let regular = LoadedFace::from_owned(regular_data, 0)?;

    // Try to find bold variant.
    let bold = find_system_font_data(font_db, name, fontdb::Weight::BOLD, fontdb::Style::Normal)
        .and_then(|d| LoadedFace::from_owned(d, 0));

    // Try to find italic variant.
    let italic =
        find_system_font_data(font_db, name, fontdb::Weight::NORMAL, fontdb::Style::Italic)
            .and_then(|d| LoadedFace::from_owned(d, 0));

    // Try to find bold-italic variant.
    let bold_italic =
        find_system_font_data(font_db, name, fontdb::Weight::BOLD, fontdb::Style::Italic)
            .and_then(|d| LoadedFace::from_owned(d, 0));

    // Fall back to regular for missing variants (clone the data).
    let bold = bold.unwrap_or_else(|| {
        LoadedFace::from_owned(regular.data.as_bytes().to_vec(), 0)
            .unwrap_or_else(|| unreachable!("re-parsing known-good regular data"))
    });
    let italic = italic.unwrap_or_else(|| {
        LoadedFace::from_owned(regular.data.as_bytes().to_vec(), 0)
            .unwrap_or_else(|| unreachable!("re-parsing known-good regular data"))
    });
    let bold_italic = bold_italic.unwrap_or_else(|| {
        LoadedFace::from_owned(regular.data.as_bytes().to_vec(), 0)
            .unwrap_or_else(|| unreachable!("re-parsing known-good regular data"))
    });

    Some(PrimaryFaces {
        regular,
        bold,
        italic,
        bold_italic,
    })
}

/// Remove all whitespace from a string (for fuzzy font name matching).
fn strip_whitespace(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).collect()
}

/// Search the fontdb for a font matching a family name, weight, and style.
///
/// Matching strategy (in order):
/// 1. Exact case-insensitive match on any family name
/// 2. Case-insensitive substring match (font family contains the query)
/// 3. Whitespace-normalised match: strip all whitespace from both the query and
///    the font family name, then compare case-insensitively.  This handles
///    naming variations like "Caskaydia Cove Nerd Font" vs "`CaskaydiaCove` Nerd
///    Font" that fontconfig resolves via aliases but fontdb does not.
fn find_system_font_data(
    font_db: &Database,
    name: &str,
    weight: fontdb::Weight,
    style: fontdb::Style,
) -> Option<Vec<u8>> {
    let lower_name = name.to_lowercase();
    let stripped_name = strip_whitespace(&lower_name);

    for face in font_db.faces() {
        let family_match = face.families.iter().any(|fam| {
            // 1. Exact case-insensitive
            fam.0.eq_ignore_ascii_case(name)
                // 2. Substring (font family contains query)
                || fam.0.to_lowercase().contains(&lower_name)
                // 3. Whitespace-stripped case-insensitive
                || strip_whitespace(&fam.0).eq_ignore_ascii_case(&stripped_name)
        });

        if !family_match {
            continue;
        }

        // Check weight and style.
        let weight_match = (i32::from(face.weight.0) - i32::from(weight.0)).unsigned_abs() < 100;
        let style_match = face.style == style;

        if !weight_match || !style_match {
            continue;
        }

        if let fontdb::Source::File(path) = &face.source
            && let Ok(bytes) = std::fs::read(path)
        {
            return Some(bytes);
        }
    }

    None
}

/// Suggest system font family names similar to the given query.
///
/// Returns up to 5 family names that share at least one word with the query.
/// Useful for diagnostic warnings when font lookup fails.
fn suggest_similar_families(query: &str, font_db: &Database) -> Vec<String> {
    let query_words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();

    let mut seen = std::collections::HashSet::new();
    let mut suggestions = Vec::new();

    for face in font_db.faces() {
        for (family_name, _) in &face.families {
            if seen.contains(family_name) {
                continue;
            }

            let family_lower = family_name.to_lowercase();
            let shares_word = query_words
                .iter()
                .any(|qw| family_lower.contains(qw.as_str()));

            if shares_word {
                seen.insert(family_name.clone());
                suggestions.push(family_name.clone());
                if suggestions.len() >= 5 {
                    return suggestions;
                }
            }
        }
    }

    suggestions
}

/// Discover the best available system emoji font.
fn discover_emoji_face(font_db: &Database) -> Option<LoadedFace> {
    for candidate in EMOJI_CANDIDATES {
        for face in font_db.faces() {
            let matches = face.families.iter().any(|fam| fam.0.contains(candidate));
            if !matches {
                continue;
            }

            if let fontdb::Source::File(path) = &face.source
                && let Ok(bytes) = std::fs::read(path)
                && let Some(loaded) = LoadedFace::from_owned(bytes, face.index as usize)
            {
                return Some(loaded);
            }
        }
    }

    None
}

/// Search `fontdb` for any font containing the given codepoint.
fn find_system_face_for_char(font_db: &Database, c: char) -> Option<LoadedFace> {
    for face in font_db.faces() {
        if let fontdb::Source::File(path) = &face.source
            && let Ok(bytes) = std::fs::read(path)
            && let Some(loaded) = LoadedFace::from_owned(bytes, face.index as usize)
            && loaded.has_glyph(c)
        {
            return Some(loaded);
        }
    }

    None
}

/// Read `usWinAscent` (u16 at byte offset 74) and `usWinDescent` (u16 at byte
/// offset 76) from a raw OS/2 table.  Returns `None` if the table is too short.
fn read_os2_win_metrics(os2_data: &[u8]) -> Option<(u16, u16)> {
    // usWinAscent is at offset 74, usWinDescent at offset 76.
    // Each is a big-endian u16; the table must be at least 78 bytes.
    if os2_data.len() < 78 {
        return None;
    }
    let win_ascent = u16::from_be_bytes([os2_data[74], os2_data[75]]);
    let win_descent = u16::from_be_bytes([os2_data[76], os2_data[77]]);
    Some((win_ascent, win_descent))
}

/// Convert a font size in typographic points to pixels-per-em (ppem).
///
/// The standard conversion is `ppem = pt × DPI / 72`.  On Linux and Windows
/// the reference DPI is 96 (CSS reference pixel), so `96 / 72 = 4/3`.
/// `pixels_per_point` (from egui) converts from logical pixels to physical
/// pixels for `HiDPI` displays.
///
/// This matches how `WezTerm`, Ghostty, and other terminals interpret font
/// size: a configured size of 12 pt on a 96 DPI display yields 16 ppem.
fn pt_to_ppem(font_size_pt: f32, pixels_per_point: f32) -> f32 {
    font_size_pt * (96.0 / 72.0) * pixels_per_point
}

/// Compute cell metrics from the regular face at the given ppem size.
///
/// `font_size_ppem` is in pixels-per-em — the value passed directly to
/// swash's `Metrics::scale()`. Callers must convert from typographic points
/// using [`pt_to_ppem`] before calling this function.
///
/// Returns `(cell_width, cell_height, baseline_offset, descent, underline_offset,
/// strikeout_offset, stroke_size)`.
///
/// `baseline_offset` is the Y distance from the top of each cell row to the text
/// baseline.  The renderer uses this value directly.
///
/// Cell height is the maximum of two metric sets:
///
/// 1. **Typographic height** — `ascent + |descent| + leading` from the font's
///    primary metrics (either `sTypoAscender`/`sTypoDescender` when the
///    `USE_TYPO_METRICS` flag is set, or `hhea.ascender`/`hhea.descender`
///    otherwise).  This is what swash's `Metrics.ascent`/`.descent` reflect.
///
/// 2. **Win height** — `usWinAscent + usWinDescent` from the OS/2 table.  Nerd
///    Font / Powerline glyphs are designed to fill this region, which is
///    typically larger than the typographic height.
///
/// When the win height is larger, the extra vertical space is distributed evenly
/// above and below the typographic region so that standard Latin glyphs remain
/// vertically centred within the cell.
fn compute_cell_metrics(
    face: &LoadedFace,
    font_size_ppem: f32,
) -> (u32, u32, f32, f32, f32, f32, f32) {
    use swash::{TableProvider, tag_from_bytes};

    let font_ref = face
        .as_font_ref()
        .unwrap_or_else(|| unreachable!("primary regular face data is corrupt"));

    let metrics = font_ref.metrics(&[]).scale(font_size_ppem);

    // Determine cell width from the advance width of a representative ASCII
    // glyph ('0').  For a true monospace font every glyph has the same advance,
    // but Nerd Font variants include wide icon/symbol glyphs that inflate
    // `metrics.max_width` far beyond the regular character advance.  Measuring
    // a concrete glyph gives us the correct monospace cell width.
    let glyph_id = font_ref.charmap().map('0');
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let cell_width = if glyph_id != 0 {
        let gm = font_ref.glyph_metrics(&[]).scale(font_size_ppem);
        let advance = gm.advance_width(glyph_id);
        if advance > 0.0 {
            advance.ceil() as u32
        } else {
            // Fallback: average_width > max_width
            let aw = metrics.average_width.ceil() as u32;
            if aw > 0 {
                aw
            } else {
                metrics.max_width.ceil() as u32
            }
        }
    } else {
        // '0' not in font — use average_width as a reasonable default.
        let aw = metrics.average_width.ceil() as u32;
        if aw > 0 {
            aw
        } else {
            metrics.max_width.ceil() as u32
        }
    };

    // --- Determine cell height from the tallest metric set ---
    //
    // Typographic height (what swash gives us in `metrics`).
    let typo_height = metrics.ascent + metrics.descent.abs() + metrics.leading;

    // Win height from the OS/2 table (font design units → pixels).
    let unscaled = font_ref.metrics(&[]);
    let upem_f = if unscaled.units_per_em != 0 {
        f32::from(unscaled.units_per_em)
    } else {
        1.0
    };
    let scale_fdu = font_size_ppem / upem_f;

    let os2_tag = tag_from_bytes(b"OS/2");
    let win_height = font_ref
        .table_by_tag(os2_tag)
        .and_then(read_os2_win_metrics)
        .map_or(0.0, |(wa, wd)| {
            f32::from(wa).mul_add(scale_fdu, f32::from(wd) * scale_fdu)
        });

    let effective_height = typo_height.max(win_height);

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let cell_height = effective_height.ceil() as u32;

    // --- Baseline offset ---
    //
    // When the win height is larger than the typographic height the extra
    // vertical space is split evenly above and below so Latin glyphs stay
    // centred.  `extra_top` absorbs the top half of that extra space.
    let extra_top = if win_height > typo_height {
        (win_height - typo_height) * 0.5
    } else {
        0.0
    };

    // A minimum 1 px top pad ensures glyphs are never flush against the cell
    // edge (important for fonts like MesloLGS where leading == 0).
    let top_pad = metrics.leading.mul_add(0.5, extra_top).max(1.0);
    let baseline_offset = metrics.ascent + top_pad;

    // Ensure non-zero dimensions.
    let cell_width = cell_width.max(1);
    let cell_height = cell_height.max(1);

    (
        cell_width,
        cell_height,
        baseline_offset,
        metrics.descent.abs(),
        metrics.underline_offset,
        metrics.strikeout_offset,
        metrics.stroke_size,
    )
}

// ---------------------------------------------------------------------------
//  Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper to create a default `FontManager` with bundled fonts.
    ///
    /// Uses `pixels_per_point = 1.0` (standard non-HiDPI).
    fn default_manager() -> FontManager {
        FontManager::new(&Config::default(), 1.0)
    }

    // --- Test 1: Bundled font loading produces non-zero metrics ---

    #[test]
    fn bundled_fonts_produce_nonzero_metrics() {
        let fm = default_manager();
        assert!(fm.cell_width > 0, "cell_width must be > 0");
        assert!(fm.cell_height > 0, "cell_height must be > 0");
        assert!(fm.ascent > 0.0, "ascent must be > 0");
        assert!(fm.descent > 0.0, "descent must be > 0 (absolute value)");
    }

    // --- Test 2: Cell size computation matches expected range for MesloLGS ---

    #[test]
    fn cell_size_reasonable_for_meslo() {
        let fm = default_manager();
        // At 12pt, MesloLGS Nerd Font Mono should produce a cell roughly
        // 7-10 pixels wide and 14-22 pixels tall (varies by exact font metrics).
        assert!(
            fm.cell_width >= 5 && fm.cell_width <= 20,
            "cell_width {} out of expected range for 12pt MesloLGS",
            fm.cell_width
        );
        assert!(
            fm.cell_height >= 10 && fm.cell_height <= 30,
            "cell_height {} out of expected range for 12pt MesloLGS",
            fm.cell_height
        );
    }

    // --- Test 2b: Cell height accounts for OS/2 win metrics (powerline) ---

    #[test]
    #[allow(clippy::unwrap_used)]
    fn cell_height_includes_win_metrics() {
        use swash::{FontRef, TableProvider, tag_from_bytes};

        // Load the bundled MesloLGS font and compute the win-metric height
        // at the default font size.  The cell height from `FontManager` must
        // be at least as tall as this value.
        let font_ref = FontRef::from_index(MESLO_REGULAR, 0);
        assert!(font_ref.is_some(), "bundled MesloLGS must parse");
        let font_ref = font_ref.unwrap();

        let config = Config::default();
        let font_size_pt = config.font.size;
        let font_size_ppem = pt_to_ppem(font_size_pt, 1.0);

        let unscaled = font_ref.metrics(&[]);
        let upem_f = if unscaled.units_per_em != 0 {
            f32::from(unscaled.units_per_em)
        } else {
            1.0
        };
        let scale_fdu = font_size_ppem / upem_f;

        let os2_tag = tag_from_bytes(b"OS/2");
        let os2_data = font_ref.table_by_tag(os2_tag);
        assert!(os2_data.is_some(), "MesloLGS must have an OS/2 table");
        let os2_data = os2_data.unwrap();
        let win_metrics = read_os2_win_metrics(os2_data);
        assert!(win_metrics.is_some(), "OS/2 table must have win metrics");
        let (wa, wd) = win_metrics.unwrap();
        assert!(wa > 0 || wd > 0, "MesloLGS must have non-zero win metrics");

        let win_height_px = f32::from(wa).mul_add(scale_fdu, f32::from(wd) * scale_fdu);

        let fm = default_manager();
        #[allow(clippy::cast_precision_loss)]
        let cell_h_f = fm.cell_height as f32;
        assert!(
            cell_h_f >= win_height_px.floor(),
            "cell_height ({cell_h_f}) must be >= win_height ({win_height_px})"
        );
    }

    // --- Test 3: Fallback chain — ASCII resolves to primary ---

    #[test]
    fn ascii_resolves_to_primary() {
        let mut fm = default_manager();
        let style = GlyphStyle::new(false, false);
        let (face_id, gid) = fm.resolve_glyph('A', style);
        assert_eq!(face_id, FaceId::PrimaryRegular);
        assert_ne!(gid, 0, "ASCII 'A' should have a non-zero glyph ID");
    }

    // --- Test 4: Glyph cache — second call returns same result ---

    #[test]
    fn glyph_cache_returns_same_result() {
        let mut fm = default_manager();
        let style = GlyphStyle::new(false, false);
        let first = fm.resolve_glyph('X', style);
        let second = fm.resolve_glyph('X', style);
        assert_eq!(first, second, "Cached result must match initial resolution");
    }

    // --- Test 5: Bold style selects bold face ---

    #[test]
    fn bold_resolves_to_bold_face() {
        let mut fm = default_manager();
        let style = GlyphStyle::new(true, false);
        let (face_id, gid) = fm.resolve_glyph('B', style);
        assert_eq!(face_id, FaceId::PrimaryBold);
        assert_ne!(gid, 0);
    }

    // --- Test 6: Italic style selects italic face ---

    #[test]
    fn italic_resolves_to_italic_face() {
        let mut fm = default_manager();
        let style = GlyphStyle::new(false, true);
        let (face_id, gid) = fm.resolve_glyph('I', style);
        assert_eq!(face_id, FaceId::PrimaryItalic);
        assert_ne!(gid, 0);
    }

    // --- Test 7: Bold-italic style selects bold-italic face ---

    #[test]
    fn bold_italic_resolves_to_bold_italic_face() {
        let mut fm = default_manager();
        let style = GlyphStyle::new(true, true);
        let (face_id, gid) = fm.resolve_glyph('Z', style);
        assert_eq!(face_id, FaceId::PrimaryBoldItalic);
        assert_ne!(gid, 0);
    }

    // --- Test 8: Unknown codepoint falls to tofu or system fallback ---

    #[test]
    fn unknown_codepoint_resolves_without_panic() {
        let mut fm = default_manager();
        let style = GlyphStyle::new(false, false);
        // U+FFFFF is in a supplementary private use area — may or may not be
        // covered by an installed system font. The important thing is that the
        // fallback chain completes without panicking and returns *some* result.
        let (face_id, gid) = fm.resolve_glyph('\u{FFFFF}', style);

        // If no system font covers it, we expect tofu (PrimaryRegular, glyph 0).
        // If a system font does cover it, we accept that too.
        match face_id {
            FaceId::PrimaryRegular => {
                assert_eq!(gid, 0, "Tofu should be glyph 0 (.notdef)");
            }
            FaceId::System(_) => {
                // System font found a glyph — that's fine.
                assert_ne!(gid, 0, "System fallback should have a real glyph");
            }
            _ => {
                // Unexpected tier — still valid, just note it.
            }
        }
    }

    // --- Test 9: User font failure — graceful fallback to bundled ---

    #[test]
    fn user_font_failure_falls_back_to_bundled() {
        let mut config = Config::default();
        config.font.family = Some("/nonexistent/path/to/font.ttf".to_owned());
        let fm = FontManager::new(&config, 1.0);

        // Should have fallen back to bundled MesloLGS as primary.
        assert!(
            fm.bundled_fallback.is_none(),
            "No bundled fallback tier when bundled is primary"
        );
        assert!(fm.cell_width > 0);
        assert!(fm.cell_height > 0);
    }

    // --- Test 10: rebuild() with no changes returns NoChange ---

    #[test]
    fn rebuild_no_change() {
        let config = Config::default();
        let mut fm = FontManager::new(&config, 1.0);
        let result = fm.rebuild(&config, 1.0);
        assert_eq!(result, RebuildResult::NoChange);
    }

    // --- Test 11: rebuild() with size change ---

    #[test]
    fn rebuild_size_change() {
        let config = Config::default();
        let mut fm = FontManager::new(&config, 1.0);

        // Pre-populate the glyph cache.
        let style = GlyphStyle::new(false, false);
        let _ = fm.resolve_glyph('A', style);
        assert!(!fm.glyph_cache.is_empty(), "cache should have an entry");

        let old_width = fm.cell_width;
        let old_height = fm.cell_height;

        let mut new_config = config;
        new_config.font.size = 24.0;
        let result = fm.rebuild(&new_config, 1.0);

        assert_eq!(result, RebuildResult::SizeChanged);
        assert!(fm.glyph_cache.is_empty(), "cache should be cleared");

        // Cell size should differ at a very different font size.
        // (24pt vs 12pt should roughly double the cell dimensions.)
        assert_ne!(
            (fm.cell_width, fm.cell_height),
            (old_width, old_height),
            "cell size should change with font size"
        );
    }

    // --- Test 12: rebuild() with family change to invalid font ---

    #[test]
    fn rebuild_family_change_with_invalid_font() {
        let config = Config::default();
        let mut fm = FontManager::new(&config, 1.0);

        let mut new_config = config;
        new_config.font.family = Some("/nonexistent/font.ttf".to_owned());
        let result = fm.rebuild(&new_config, 1.0);

        // The requested font fails to load, so the effective family stays as
        // bundled MesloLGS (None → None).  No observable change.
        assert_eq!(result, RebuildResult::NoChange);
        // Should have gracefully fallen back to bundled.
        assert!(fm.cell_width > 0);
    }

    // --- Test 13: GlyphStyle::from_format ---

    #[test]
    fn glyph_style_from_format() {
        let style = GlyphStyle::from_format(&FontWeight::Bold, &[FontDecorations::Italic]);
        assert!(style.bold);
        assert!(style.italic);

        let style = GlyphStyle::from_format(&FontWeight::Normal, &[]);
        assert!(!style.bold);
        assert!(!style.italic);
    }

    // --- Test 14: rustybuzz Face creation ---

    #[test]
    fn rustybuzz_face_creation() {
        let fm = default_manager();
        let face = fm.rustybuzz_face(FaceId::PrimaryRegular);
        assert!(
            face.is_some(),
            "Should be able to create a rustybuzz Face from primary regular"
        );
    }

    // --- Test 15: swash FontRef creation ---

    #[test]
    fn swash_font_ref_creation() {
        let fm = default_manager();
        let font_ref = fm.swash_font_ref(FaceId::PrimaryRegular);
        assert!(
            font_ref.is_some(),
            "Should be able to create a swash FontRef from primary regular"
        );
    }

    // --- Test 16: rebuild() font_changed() predicate ---

    #[test]
    fn rebuild_result_font_changed_predicate() {
        assert!(!RebuildResult::NoChange.font_changed());
        assert!(RebuildResult::SizeChanged.font_changed());
        assert!(RebuildResult::FamilyChanged.font_changed());
    }

    // --- Test 17: Cell size scales with font size ---

    #[test]
    fn cell_size_scales_with_font_size() {
        let mut config_small = Config::default();
        config_small.font.size = 8.0;
        let fm_small = FontManager::new(&config_small, 1.0);

        let mut config_large = Config::default();
        config_large.font.size = 32.0;
        let fm_large = FontManager::new(&config_large, 1.0);

        assert!(
            fm_large.cell_width > fm_small.cell_width,
            "Larger font size should produce larger cell width"
        );
        assert!(
            fm_large.cell_height > fm_small.cell_height,
            "Larger font size should produce larger cell height"
        );
    }

    #[test]
    fn strip_whitespace_removes_all_whitespace() {
        assert_eq!(
            strip_whitespace("Caskaydia Cove Nerd Font"),
            "CaskaydiaCoveNerdFont"
        );
        assert_eq!(
            strip_whitespace("CaskaydiaCove Nerd Font"),
            "CaskaydiaCoveNerdFont"
        );
        assert_eq!(strip_whitespace("  A  B  "), "AB");
        assert_eq!(strip_whitespace("NoSpaces"), "NoSpaces");
        assert_eq!(strip_whitespace(""), "");
    }

    #[test]
    fn strip_whitespace_matching_detects_nerd_font_variants() {
        // Simulates the matching logic: user queries "Caskaydia Cove Nerd Font",
        // font family is "CaskaydiaCove Nerd Font". Whitespace-stripped comparison
        // should match.
        let query = "Caskaydia Cove Nerd Font";
        let family = "CaskaydiaCove Nerd Font";

        let stripped_query = strip_whitespace(&query.to_lowercase());
        let stripped_family = strip_whitespace(family);

        assert!(
            stripped_family.eq_ignore_ascii_case(&stripped_query),
            "Whitespace-stripped comparison should match: '{stripped_family}' vs '{stripped_query}'"
        );
    }
}
