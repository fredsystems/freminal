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

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use conv2::{ConvUtil, ValueFrom};
use fontdb::Database;
use freminal_common::buffer_states::fonts::{FontDecorationFlags, FontDecorations, FontWeight};
use freminal_common::config::Config;

// ---------------------------------------------------------------------------
//  Bundled font data (compiled into the binary via include_bytes!)
// ---------------------------------------------------------------------------

/// The family name of the bundled default font.
///
/// Single source of truth for the bundled-font family string: the settings
/// font picker, default-label construction, and the "is this installed font the
/// same as our bundled one?" check all reference this constant rather than
/// duplicating the literal.
pub const BUNDLED_FONT_FAMILY: &str = "CaskaydiaCove Nerd Font";

static CASKAYDIA_REGULAR: &[u8] = include_bytes!("../../../res/CaskaydiaCoveNerdFont-Regular.ttf");
static CASKAYDIA_BOLD: &[u8] = include_bytes!("../../../res/CaskaydiaCoveNerdFont-Bold.ttf");
static CASKAYDIA_ITALIC: &[u8] = include_bytes!("../../../res/CaskaydiaCoveNerdFont-Italic.ttf");
static CASKAYDIA_BOLD_ITALIC: &[u8] =
    include_bytes!("../../../res/CaskaydiaCoveNerdFont-BoldItalic.ttf");

/// Bundled color emoji font (Noto Color Emoji, OFL-1.1). Guarantees emoji
/// rendering even on a system with no emoji font installed (Task #402).
static NOTO_COLOR_EMOJI: &[u8] = include_bytes!("../../../res/NotoColorEmoji.ttf");

/// The Unicode blocks that make up the emoji repertoire, as inclusive
/// `(start, end)` codepoint ranges.
///
/// A candidate emoji face is scored by counting how many codepoints across
/// these ranges its `cmap` actually maps ([`LoadedFace::emoji_coverage`]) —
/// a real coverage measurement over the font's own character map, not a
/// hand-picked sample. Ranges are the emoji-bearing blocks per the Unicode
/// Standard (the pictographic/emoji blocks; not every codepoint in these
/// blocks is an emoji, but the count is a faithful relative measure of how
/// much of the repertoire a font carries).
const EMOJI_BLOCKS: &[(u32, u32)] = &[
    (0x2600, 0x26FF),   // Miscellaneous Symbols
    (0x2700, 0x27BF),   // Dingbats
    (0x1F300, 0x1F5FF), // Miscellaneous Symbols and Pictographs
    (0x1F600, 0x1F64F), // Emoticons
    (0x1F680, 0x1F6FF), // Transport and Map Symbols
    (0x1F900, 0x1F9FF), // Supplemental Symbols and Pictographs
    (0x1FA70, 0x1FAFF), // Symbols and Pictographs Extended-A
];

/// Raw bytes of the bundled default font (`CaskaydiaCove` Nerd Font, Regular).
///
/// Exposed for callers that need to inspect the bundled face directly rather
/// than going through the egui font registry — notably the chrome-icon
/// regression test, which verifies every [`crate::gui::icons::ChromeIcon`]
/// codepoint resolves to a glyph in this exact face.
#[cfg(test)]
#[must_use]
pub(crate) const fn bundled_regular_font_bytes() -> &'static [u8] {
    CASKAYDIA_REGULAR
}

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
    /// Primary face (user font if configured, else bundled `CaskaydiaCove`).
    PrimaryRegular,
    /// Primary bold face.
    PrimaryBold,
    /// Primary italic face.
    PrimaryItalic,
    /// Primary bold-italic face.
    PrimaryBoldItalic,
    /// Bundled `CaskaydiaCove` — only present as fallback when a user font is primary.
    BundledRegular,
    /// Bundled bold face (fallback only).
    BundledBold,
    /// Bundled italic face (fallback only).
    BundledItalic,
    /// Bundled bold-italic face (fallback only).
    BundledBoldItalic,
    /// System emoji face.
    Emoji,
    /// Lazily-discovered system fallback face (index into `system_faces`).
    System(usize),
}

/// Errors that can occur when constructing or rebuilding a [`FontManager`].
///
/// All variants represent build-time or environment invariant violations that
/// would have previously triggered a panic.  They are surfaced as typed errors
/// so the binary can log a diagnostic and exit cleanly rather than aborting.
#[derive(Debug, thiserror::Error)]
pub enum FontManagerError {
    /// A bundled font embedded via `include_bytes!` failed to parse with swash.
    /// This indicates a packaging or build-time corruption error.
    #[error("bundled font '{face}' is corrupt and cannot be parsed by swash")]
    BundledFontCorrupt {
        /// Human-readable name of the bundled face that failed.
        face: &'static str,
    },

    /// Re-parsing font data that was just successfully parsed failed.  This
    /// should only occur if the backing `Vec<u8>` was mutated between the
    /// successful parse and the re-parse, which is not possible in the current
    /// code paths.
    #[error("failed to re-parse previously-validated font data for {variant} variant")]
    ReparseFailed {
        /// Style variant being re-parsed (e.g., "bold", "italic").
        variant: &'static str,
    },

    /// Obtaining a `swash::FontRef` from a previously-loaded face failed.  The
    /// face was successfully parsed on load, so this indicates memory corruption
    /// or a bug in swash.
    #[error("swash FontRef unavailable for primary regular face")]
    FontRefUnavailable,
}

/// Style selector for glyph resolution, derived from format tags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GlyphStyle {
    /// Whether the glyph should use the bold face.
    pub bold: bool,
    /// Whether the glyph should use the italic face.
    pub italic: bool,
}

impl GlyphStyle {
    /// Create a new `GlyphStyle` from explicit bold and italic flags.
    #[must_use]
    pub const fn new(bold: bool, italic: bool) -> Self {
        Self { bold, italic }
    }

    /// Construct from the buffer-layer font types.
    #[must_use]
    pub fn from_format(weight: &FontWeight, decorations: FontDecorationFlags) -> Self {
        Self {
            bold: *weight == FontWeight::Bold,
            italic: decorations.contains(FontDecorations::Italic),
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

/// Font data source.
///
/// The bytes are always held behind a single `Arc<[u8]>` — bundled
/// (`&'static`) fonts are copied into an `Arc` exactly once at load time
/// (`from_static`), heap-loaded fonts move their `Vec<u8>` into the `Arc`
/// (`from_owned`). Holding a single owner type means [`LoadedFace::arc_bytes`]
/// is *always* a cheap `Arc::clone` (refcount bump), so constructing a cached,
/// self-referential `rustybuzz::Face` ([`CachedFace`]) on a cache miss never
/// copies the font bytes — not even for bundled fonts (Task #430).
#[derive(Debug)]
struct FontData(Arc<[u8]>);

impl FontData {
    fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl LoadedFace {
    /// Load from static (bundled) font data.
    ///
    /// The `&'static` bytes are copied into an `Arc<[u8]>` exactly once here,
    /// at load time, so that later face-cache misses reuse the same allocation
    /// via a cheap `Arc::clone` rather than re-copying the (multi-MB) bundled
    /// font on every miss (Task #430).
    fn from_static(data: &'static [u8]) -> Option<Self> {
        let data: Arc<[u8]> = Arc::from(data);
        let font_ref = swash::FontRef::from_index(&data, 0)?;
        let key = font_ref.key;
        Some(Self {
            data: FontData(data),
            index: 0,
            cache_key: key,
        })
    }

    /// Load from owned font data.
    fn from_owned(data: Vec<u8>, index: usize) -> Option<Self> {
        let data: Arc<[u8]> = Arc::from(data);
        let font_ref = swash::FontRef::from_index(&data, index)?;
        let key = font_ref.key;
        Some(Self {
            data: FontData(data),
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

    /// Return this face's bytes as a stable, cheaply-cloneable `Arc<[u8]>`
    /// owner, suitable for constructing a cached self-referential
    /// `rustybuzz::Face` ([`CachedFace`]) (Task #430).
    ///
    /// Always a cheap `Arc::clone` (refcount bump) — the one-time copy of any
    /// `&'static` bundled bytes into the `Arc` happened at load time in
    /// [`Self::from_static`], so no font-byte copy occurs here on a cache miss.
    fn arc_bytes(&self) -> Arc<[u8]> {
        Arc::clone(&self.data.0)
    }

    /// Check if this face's charmap contains the given codepoint.
    fn has_glyph(&self, c: char) -> bool {
        self.as_font_ref().is_some_and(|f| f.charmap().map(c) != 0)
    }

    /// Map a codepoint to a glyph ID, returning 0 (`.notdef`) if unmapped.
    fn map_char(&self, c: char) -> u16 {
        self.as_font_ref().map_or(0, |f| f.charmap().map(c))
    }

    /// Whether this face carries color glyph tables (`COLR`/`CPAL` vector
    /// palettes or `CBDT`/`CBLC`/`sbix` bitmap strikes) — i.e. is a genuine
    /// color emoji font rather than a text font that happens to map an emoji
    /// codepoint to a monochrome outline.
    fn has_color_glyphs(&self) -> bool {
        self.as_font_ref().is_some_and(|f| {
            f.color_palettes().next().is_some() || f.color_strikes().next().is_some()
        })
    }

    /// Count how many codepoints across the [`EMOJI_BLOCKS`] this face's `cmap`
    /// actually maps to a glyph — a real coverage measurement over the font's
    /// character map. Used to rank candidate emoji faces by how much of the
    /// emoji repertoire they carry.
    fn emoji_coverage(&self) -> u32 {
        self.as_font_ref().map_or(0, |f| {
            let charmap = f.charmap();
            let mut count = 0u32;
            for &(start, end) in EMOJI_BLOCKS {
                for cp in start..=end {
                    if let Some(c) = char::from_u32(cp)
                        && charmap.map(c) != 0
                    {
                        count += 1;
                    }
                }
            }
            count
        })
    }
}

/// Type alias so `self_cell!` can reference `rustybuzz::Face` by a bare
/// identifier — the macro requires an `ident`, not a path (`rustybuzz::Face`
/// itself does not work as the `dependent` type).
type RustybuzzFace<'a> = rustybuzz::Face<'a>;

self_cell::self_cell!(
    /// A parsed `rustybuzz::Face` bundled together with the `Arc<[u8]>` byte
    /// buffer it borrows from (Task #430).
    ///
    /// `rustybuzz::Face<'a>` borrows the font bytes, so it cannot be stored
    /// directly as a field on [`FontManager`] alongside the owning
    /// [`LoadedFace`] — that would be self-referential. `self_cell` ties the
    /// owner (a cheaply-cloneable, stable `Arc<[u8]>`; see
    /// [`LoadedFace::arc_bytes`]) and the dependent (the parsed
    /// `rustybuzz::Face`) together in one heap-allocated, movable struct.
    /// This lets [`FontManager`] cache the parsed face per [`FaceId`]
    /// instead of re-parsing the font tables on every shape call.
    struct CachedFace {
        owner: Arc<[u8]>,

        #[covariant]
        dependent: RustybuzzFace,
    }
);

/// Cache key for a compiled `rustybuzz::ShapePlan` (Task #430).
///
/// Distinguishes plans by face, the ligature-feature-set identity
/// (`shaping_features(ligatures)` only ever varies on this bool), and the
/// buffer's guessed script/direction. `rustybuzz::Script` and
/// `rustybuzz::Direction` already implement `Copy + Eq + Hash`, so no
/// synthetic hashable representation is needed.
type PlanKey = (FaceId, bool, rustybuzz::Script, rustybuzz::Direction);

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

/// A fallback face's own cell metrics, used to normalise its glyphs into the
/// primary cell (Task #411).
///
/// All three are measured at the current rasterisation ppem. Cached per
/// [`FaceId`] because recomputing them (`compute_cell_metrics`, which parses the
/// OS/2 table and probes for Powerline glyphs) for every fallback glyph on every
/// frame is expensive.
#[derive(Debug, Clone, Copy)]
struct FallbackCellMetrics {
    /// The fallback face's own cell height in pixels.
    cell_h: f32,
    /// The fallback face's own baseline (cell-top to baseline) in pixels.
    baseline: f32,
    /// The fallback face's own cell width in pixels.
    cell_w: f32,
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
    /// If a user font is configured, it becomes primary; bundled `CaskaydiaCove`
    /// becomes the first fallback tier.
    primary: PrimaryFaces,

    /// Bundled `CaskaydiaCove` faces — present as a fallback tier only when a user
    /// font is active as primary. `None` when `CaskaydiaCove` is already primary.
    bundled_fallback: Option<PrimaryFaces>,

    /// System emoji face (Noto Color Emoji, Apple Color Emoji, etc.).
    emoji_face: Option<LoadedFace>,

    /// Lazily-discovered system fallback faces, keyed by codepoint.
    system_fallback_cache: HashMap<char, Option<usize>>,

    /// Heap of system fallback faces discovered so far.
    system_faces: Vec<LoadedFace>,

    /// Resolved glyph cache: (codepoint, style) -> (`face_id`, `glyph_id`).
    glyph_cache: HashMap<(char, GlyphStyle), (FaceId, u16)>,

    /// Per-face cache of fallback cell metrics (Task #411). Populated lazily by
    /// [`Self::fallback_cell_metrics`] and cleared on rebuild alongside
    /// `glyph_cache`. `None` means the face has no measurable metrics (or is a
    /// primary face). Interior mutability so the renderer can read metrics
    /// through a shared `&FontManager` while still caching.
    fallback_metrics_cache: RefCell<HashMap<FaceId, Option<FallbackCellMetrics>>>,

    /// Per-face cache of parsed `rustybuzz::Face` instances (Task #430).
    /// Populated lazily by [`Self::build_cached_face`] (invoked from
    /// [`Self::shape_cached`]) and cleared whenever the underlying font
    /// bytes may have changed (`rebuild`) or for consistency on a
    /// size/DPI change (`set_font_size`, `update_pixels_per_point`), even
    /// though the bytes themselves are unaffected by those. `None` means the
    /// face is not loaded or failed to parse as a rustybuzz `Face`.
    face_cache: RefCell<HashMap<FaceId, Option<CachedFace>>>,

    /// Per-`(face, ligatures, script, direction)` cache of compiled
    /// `rustybuzz::ShapePlan`s (Task #430). Plan compilation is the
    /// second-most expensive part of cold shaping after face parsing;
    /// caching it avoids recompiling an identical plan on every shape call.
    /// Cleared alongside `face_cache`.
    plan_cache: RefCell<HashMap<PlanKey, Arc<rustybuzz::ShapePlan>>>,

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

    /// Line-height multiplier (`config.font.line_height`) used as the deliberate
    /// row-pitch factor over the font's tight `ascent + descent` ink box. See
    /// [`compute_cell_layout`].
    line_height: f32,

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
    /// Loads the primary faces (user font or bundled `CaskaydiaCove`), discovers a
    /// system emoji font, and computes the authoritative cell size.
    ///
    /// `pixels_per_point` is the display scale factor from
    /// `egui::Context::pixels_per_point()`. It is used together with the
    /// configured font size (in typographic points) to compute the correct
    /// ppem value for swash metric queries.
    ///
    /// # Errors
    ///
    /// Returns [`FontManagerError`] if the bundled fonts fail to parse, if
    /// re-parsing validated font data fails, or if a swash `FontRef` cannot be
    /// obtained from a loaded face.  All such errors indicate build-time or
    /// memory-corruption invariant violations and should be treated as fatal
    /// by the binary.
    pub fn new(config: &Config, pixels_per_point: f32) -> Result<Self, FontManagerError> {
        let mut font_db = Database::new();
        font_db.load_system_fonts();

        let bundled = load_bundled_faces()?;
        let font_size_pt = config.font.size;
        let line_height = config.font.line_height;

        let (primary, bundled_fallback, current_family) =
            if let Some(family) = config.font.family.as_deref() {
                if let Some(user_primary) = load_user_faces(family, &font_db)? {
                    info!("Loaded user font '{}' as primary", family);
                    (user_primary, Some(bundled), Some(family.to_owned()))
                } else {
                    let suggestions = suggest_similar_families(family, &font_db);
                    if suggestions.is_empty() {
                        warn!(
                            "Failed to load user font '{}'; falling back to bundled CaskaydiaCove",
                            family
                        );
                    } else {
                        warn!(
                            "Failed to load user font '{}'; falling back to bundled CaskaydiaCove. \
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

        // Always resolves to at least the bundled Noto Color Emoji face.
        let emoji_face = discover_emoji_face(&font_db);

        let font_size_ppem = pt_to_ppem(font_size_pt, pixels_per_point);
        let CellMetrics {
            cell_width,
            cell_height,
            ascent,
            descent,
            underline_offset,
            strikeout_offset,
            stroke_size,
        } = compute_cell_metrics(&primary.regular, font_size_ppem, line_height)?;

        Ok(Self {
            primary,
            bundled_fallback,
            emoji_face,
            system_fallback_cache: HashMap::new(),
            system_faces: Vec::new(),
            glyph_cache: HashMap::new(),
            fallback_metrics_cache: RefCell::new(HashMap::new()),
            face_cache: RefCell::new(HashMap::new()),
            plan_cache: RefCell::new(HashMap::new()),
            cell_width,
            cell_height,
            ascent,
            descent,
            underline_offset,
            strikeout_offset,
            stroke_size,
            font_size_pt,
            line_height,
            pixels_per_point,
            current_family,
            font_db,
        })
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

    /// The pixels-per-em size glyphs must be **rasterized** at.
    ///
    /// This is the same ppem the cell metrics (ascent, descent, cell width)
    /// are computed at — `font_size_pt × 96/72 × pixels_per_point`. Glyphs
    /// MUST be rasterized at this size so their ink lines up with the baseline
    /// and cell geometry. Rasterizing at any other size (e.g. the cell
    /// *height*, which for Nerd Fonts is inflated by the OS/2 win-metrics
    /// floor) scales every glyph by the wrong factor, making text too large
    /// and pushing it toward the top of the cell.
    #[must_use]
    pub fn rasterization_ppem(&self) -> f32 {
        pt_to_ppem(self.font_size_pt, self.pixels_per_point)
    }

    // -----------------------------------------------------------------------
    //  Glyph resolution
    // -----------------------------------------------------------------------

    /// Resolve a codepoint + style to a `(FaceId, glyph_id)` pair.
    ///
    /// Tries each fallback tier in order:
    /// 1. Primary face (user font or bundled `CaskaydiaCove`)
    /// 2. Bundled fallback (`CaskaydiaCove`, only when a user font is primary)
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
        self.loaded_face(face_id).map(LoadedFace::as_bytes)
    }

    /// Get the font collection index for a given `FaceId`.
    ///
    /// Returns `None` if the face is not loaded.
    #[must_use]
    pub fn face_index(&self, face_id: FaceId) -> Option<usize> {
        self.loaded_face(face_id).map(|f| f.index)
    }

    /// Shape `buffer` for `face_id` using a cached parsed `rustybuzz::Face`
    /// and a cached compiled `rustybuzz::ShapePlan` (Task #430), instead of
    /// re-parsing the font tables and recompiling the plan on every call.
    ///
    /// The caller must have already called
    /// `buffer.guess_segment_properties()` so `buffer.script()` /
    /// `buffer.direction()` reflect the buffer's actual (guessed)
    /// properties — the plan is built to match exactly, mirroring what
    /// `rustybuzz::shape()` does internally (it also calls
    /// `guess_segment_properties()` and builds its one-shot plan from the
    /// resulting `direction`/`script`/`language`), except both the face
    /// parse and the plan compilation are cached here instead of repeated.
    ///
    /// Returns `None` if the face is not loaded or fails to parse as a
    /// rustybuzz `Face` — callers must fall back to tofu glyphs in that
    /// case, matching the previous `rustybuzz_face`-based behavior.
    #[must_use]
    pub(crate) fn shape_cached(
        &self,
        face_id: FaceId,
        ligatures: bool,
        features: &[rustybuzz::Feature],
        buffer: rustybuzz::UnicodeBuffer,
    ) -> Option<rustybuzz::GlyphBuffer> {
        if !self.face_cache.borrow().contains_key(&face_id) {
            let built = self.build_cached_face(face_id);
            self.face_cache.borrow_mut().insert(face_id, built);
        }

        let face_cache = self.face_cache.borrow();
        let cached = face_cache.get(&face_id)?.as_ref()?;
        let face = cached.borrow_dependent();

        let script = buffer.script();
        let direction = buffer.direction();
        let plan_key: PlanKey = (face_id, ligatures, script, direction);

        // Clone the cached `Arc` (if any) out from under the immutable borrow
        // first, so the borrow is released before a miss needs `borrow_mut()`
        // — holding the `Ref` across that call would panic at runtime.
        let cached_plan = self.plan_cache.borrow().get(&plan_key).cloned();
        let plan = cached_plan.unwrap_or_else(|| {
            // We pass `Some(script)` where the internal `rustybuzz::shape()`
            // passes the buffer's raw `Option<Script>` (which is `None` for a
            // run whose every char is Common/Inherited/Unknown, e.g. pure ASCII
            // punctuation). We cannot reach that raw `Option` through
            // rustybuzz's public API — `UnicodeBuffer::script()` collapses
            // `None` to `script::UNKNOWN` — and reimplementing the guesser to
            // recover it would risk drifting from rustybuzz. This collapse is
            // provably safe: `shape_with_plan` matches a plan to a buffer by
            // comparing `buffer.script.unwrap_or(UNKNOWN)` against
            // `plan.script.unwrap_or(UNKNOWN)` (see its `debug_assert_eq!`), so
            // rustybuzz itself treats `None` and `Some(UNKNOWN)` as equivalent
            // for plan selection, and no real font registers a `zzzz` (UNKNOWN)
            // OpenType script tag, so the compiled plan is identical either
            // way. The output-identity test shapes pure-Common-script runs
            // (`->`, `!=`) against the old `shape()` path to guard this.
            let plan = Arc::new(rustybuzz::ShapePlan::new(
                face,
                direction,
                Some(script),
                None,
                features,
            ));
            self.plan_cache
                .borrow_mut()
                .insert(plan_key, Arc::clone(&plan));
            plan
        });

        Some(rustybuzz::shape_with_plan(face, &plan, buffer))
    }

    /// Build (but do not cache) a [`CachedFace`] for `face_id`. Called from
    /// [`Self::shape_cached`] on a face-cache miss.
    fn build_cached_face(&self, face_id: FaceId) -> Option<CachedFace> {
        let loaded = self.loaded_face(face_id)?;
        let index = u32::value_from(loaded.index).unwrap_or(0);
        let owner = loaded.arc_bytes();
        CachedFace::try_new(owner, |bytes| {
            rustybuzz::Face::from_slice(bytes, index).ok_or(())
        })
        .ok()
    }

    /// Number of faces currently cached in [`Self::face_cache`].
    ///
    /// Test-only: exposes cache population without leaking internals into
    /// the production API.
    #[cfg(test)]
    pub(crate) fn face_cache_len(&self) -> usize {
        self.face_cache.borrow().len()
    }

    /// Number of shape plans currently cached in [`Self::plan_cache`].
    ///
    /// Test-only: exposes cache population without leaking internals into
    /// the production API.
    #[cfg(test)]
    pub(crate) fn plan_cache_len(&self) -> usize {
        self.plan_cache.borrow().len()
    }

    /// Create a `swash::FontRef` for the given `FaceId`.
    ///
    /// Returns `None` if the face is not loaded.
    #[must_use]
    pub fn swash_font_ref(&self, face_id: FaceId) -> Option<swash::FontRef<'_>> {
        self.loaded_face(face_id)?.as_font_ref()
    }

    /// Get the swash `CacheKey` for a given `FaceId`.
    ///
    /// Returns `None` if the face is not loaded.
    #[must_use]
    pub fn face_cache_key(&self, face_id: FaceId) -> Option<swash::CacheKey> {
        self.loaded_face(face_id).map(LoadedFace::cache_key)
    }

    /// The own cell height, baseline, and cell width of a **fallback** face, at
    /// the current rasterisation ppem (Task #411).
    ///
    /// A glyph resolved from a fallback face (bundled or system) was designed
    /// against *that* font's cell, not the primary font's. To place it into the
    /// primary cell without clipping or mis-centring, the renderer needs the
    /// fallback face's own `(cell_height, baseline, cell_width)` so it can scale
    /// the glyph independently on each axis into the primary cell.
    ///
    /// Returns `(cell_height, baseline, cell_width)`, or `None` for the primary
    /// faces (they drive the grid directly and need no normalisation) and for
    /// any face that cannot be measured.
    ///
    /// The result is cached per [`FaceId`]: this is called for every
    /// fallback glyph on every frame, and `compute_cell_metrics` (OS/2 parsing +
    /// Powerline probe) is far too costly to repeat per glyph.
    #[must_use]
    pub fn fallback_cell_metrics(&self, face_id: FaceId) -> Option<(f32, f32, f32)> {
        if let Some(cached) = self.fallback_metrics_cache.borrow().get(&face_id) {
            return cached.map(|m| (m.cell_h, m.baseline, m.cell_w));
        }

        let computed = self.compute_fallback_cell_metrics(face_id);
        self.fallback_metrics_cache
            .borrow_mut()
            .insert(face_id, computed);
        computed.map(|m| (m.cell_h, m.baseline, m.cell_w))
    }

    /// Uncached computation backing [`Self::fallback_cell_metrics`].
    fn compute_fallback_cell_metrics(&self, face_id: FaceId) -> Option<FallbackCellMetrics> {
        match face_id {
            FaceId::PrimaryRegular
            | FaceId::PrimaryBold
            | FaceId::PrimaryItalic
            | FaceId::PrimaryBoldItalic => None,
            _ => {
                let face = self.loaded_face(face_id)?;
                let ppem = pt_to_ppem(self.font_size_pt, self.pixels_per_point);
                let cm = compute_cell_metrics(face, ppem, self.line_height).ok()?;
                let cell_h: f32 = cm.cell_height.value_as::<f32>().ok()?;
                let cell_w: f32 = cm.cell_width.value_as::<f32>().ok()?;
                Some(FallbackCellMetrics {
                    cell_h,
                    baseline: cm.ascent,
                    cell_w,
                })
            }
        }
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
    ///
    /// # Errors
    ///
    /// Returns [`FontManagerError`] if the bundled fonts or user font data
    /// cannot be (re-)loaded.  Such errors represent build-time or memory
    /// corruption invariant violations and should be treated as fatal.
    pub fn rebuild(
        &mut self,
        config: &Config,
        pixels_per_point: f32,
    ) -> Result<RebuildResult, FontManagerError> {
        let new_family = config.font.family.as_deref();
        let new_size = config.font.size;
        let new_line_height = config.font.line_height;

        let requested_family_differs = new_family != self.current_family.as_deref();
        let size_changed = (new_size - self.font_size_pt).abs() > f32::EPSILON;
        let ppp_changed = (pixels_per_point - self.pixels_per_point).abs() > f32::EPSILON;
        let line_height_changed = (new_line_height - self.line_height).abs() > f32::EPSILON;

        if !requested_family_differs && !size_changed && !ppp_changed && !line_height_changed {
            return Ok(RebuildResult::NoChange);
        }

        // Track whether the effective font family actually changed (as opposed
        // to the config requesting a font that fails to load and falls back to
        // the same bundled CaskaydiaCove that was already active).
        let old_effective = self.current_family.clone();
        let mut effective_family_changed = false;

        if requested_family_differs {
            let bundled = load_bundled_faces()?;

            let (primary, bundled_fallback, current_family) = if let Some(family) = new_family {
                if let Some(user_primary) = load_user_faces(family, &self.font_db)? {
                    info!("Reloaded user font '{}' as primary", family);
                    (user_primary, Some(bundled), Some(family.to_owned()))
                } else {
                    warn!(
                        "Failed to reload user font '{}'; using bundled CaskaydiaCove",
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
        self.line_height = new_line_height;
        self.pixels_per_point = pixels_per_point;
        let font_size_ppem = pt_to_ppem(self.font_size_pt, self.pixels_per_point);
        let metrics =
            compute_cell_metrics(&self.primary.regular, font_size_ppem, self.line_height)?;
        self.apply_cell_metrics(metrics);

        // Clear caches — glyph IDs and system face mappings may differ.
        self.glyph_cache.clear();
        self.system_fallback_cache.clear();
        self.system_faces.clear();
        self.fallback_metrics_cache.borrow_mut().clear();
        // The font family may have changed, so a cached rustybuzz `Face`
        // (which borrows a specific `FaceId`'s byte buffer) or `ShapePlan`
        // (compiled against a specific `Face`) may now reference the wrong
        // font entirely — this clear is required for correctness, not just
        // consistency (Task #430).
        self.face_cache.borrow_mut().clear();
        self.plan_cache.borrow_mut().clear();

        if effective_family_changed {
            Ok(RebuildResult::FamilyChanged)
        } else if size_changed || ppp_changed || line_height_changed {
            Ok(RebuildResult::SizeChanged)
        } else {
            // The config requested a different family, but after attempting to
            // load it, the effective font is the same (e.g. both old and new
            // fell back to bundled CaskaydiaCove).  No observable change.
            Ok(RebuildResult::NoChange)
        }
    }

    /// Change the font size without altering the font family.
    ///
    /// Used by font zoom (Ctrl+Plus/Minus/0) to apply a per-tab effective
    /// font size that differs from the config's base size.  Returns `Ok(true)`
    /// if the size actually changed and caches were invalidated.
    ///
    /// # Errors
    ///
    /// Returns [`FontManagerError::FontRefUnavailable`] if re-computing cell
    /// metrics fails due to swash being unable to produce a `FontRef` from the
    /// previously-validated primary face.
    pub fn set_font_size(&mut self, size_pt: f32) -> Result<bool, FontManagerError> {
        if (size_pt - self.font_size_pt).abs() <= f32::EPSILON {
            return Ok(false);
        }

        self.font_size_pt = size_pt;
        let font_size_ppem = pt_to_ppem(self.font_size_pt, self.pixels_per_point);
        let metrics =
            compute_cell_metrics(&self.primary.regular, font_size_ppem, self.line_height)?;
        self.apply_cell_metrics(metrics);

        // Clear caches — glyph sizes differ at the new ppem.
        self.glyph_cache.clear();
        self.system_fallback_cache.clear();
        self.system_faces.clear();
        self.fallback_metrics_cache.borrow_mut().clear();
        // The underlying font bytes are unchanged by a size-only change, so
        // this is not strictly required for correctness, but it is cleared
        // for consistency with the other per-face caches above (Task #430).
        self.face_cache.borrow_mut().clear();
        self.plan_cache.borrow_mut().clear();

        Ok(true)
    }

    /// Check whether `pixels_per_point` has changed and recompute cell metrics
    /// if so.  Returns `Ok(true)` when metrics were recomputed (callers should
    /// invalidate the glyph atlas and shaping cache).
    ///
    /// This is a lightweight per-frame check intended to handle monitor DPI
    /// changes (e.g. dragging the window to a `HiDPI` display) without requiring
    /// a full config reload.
    /// Returns the currently-stored display scale factor.
    #[must_use]
    pub const fn pixels_per_point(&self) -> f32 {
        self.pixels_per_point
    }

    /// Recompute cell metrics if the `pixels_per_point` has changed.
    ///
    /// # Errors
    ///
    /// Returns [`FontManagerError::FontRefUnavailable`] if re-computing cell
    /// metrics fails.
    pub fn update_pixels_per_point(
        &mut self,
        pixels_per_point: f32,
    ) -> Result<bool, FontManagerError> {
        if (pixels_per_point - self.pixels_per_point).abs() <= f32::EPSILON {
            return Ok(false);
        }

        self.pixels_per_point = pixels_per_point;
        let font_size_ppem = pt_to_ppem(self.font_size_pt, self.pixels_per_point);
        let metrics =
            compute_cell_metrics(&self.primary.regular, font_size_ppem, self.line_height)?;
        self.apply_cell_metrics(metrics);

        // Clear caches — glyph sizes differ at new ppem.
        self.glyph_cache.clear();
        self.system_fallback_cache.clear();
        self.system_faces.clear();
        self.fallback_metrics_cache.borrow_mut().clear();
        // The underlying font bytes are unchanged by a DPI-only change, so
        // this is not strictly required for correctness, but it is cleared
        // for consistency with the other per-face caches above (Task #430).
        self.face_cache.borrow_mut().clear();
        self.plan_cache.borrow_mut().clear();

        Ok(true)
    }

    /// Return a sorted, deduplicated list of all monospaced font family names
    /// installed on the system.
    ///
    /// The list is computed fresh from the `fontdb` database each time it is
    /// called. For the settings modal this is only invoked once when the modal
    /// opens, so the cost is negligible.
    #[must_use]
    pub fn enumerate_monospace_families(&self) -> Vec<String> {
        let mut seen = std::collections::HashSet::new();
        let mut families: Vec<String> = self
            .font_db
            .faces()
            .filter(|f| f.monospaced)
            .flat_map(|f| f.families.iter().map(|(name, _)| name.clone()))
            .filter(|name| seen.insert(name.clone()))
            .collect();
        families.sort_unstable();
        families
    }

    /// Load the raw font file bytes for a given system font family name.
    ///
    /// Returns `None` if the family is not found in the `fontdb` database.
    /// Looks for a regular-weight, normal-style face first.
    #[must_use]
    pub fn load_font_bytes_for_family(&self, family: &str) -> Option<Vec<u8>> {
        find_system_font_data(
            &self.font_db,
            family,
            fontdb::Weight::NORMAL,
            fontdb::Style::Normal,
        )
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
    fn loaded_face(&self, face_id: FaceId) -> Option<&LoadedFace> {
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

    /// Copy freshly-computed cell metrics into the manager's cached fields.
    const fn apply_cell_metrics(&mut self, metrics: CellMetrics) {
        self.cell_width = metrics.cell_width;
        self.cell_height = metrics.cell_height;
        self.ascent = metrics.ascent;
        self.descent = metrics.descent;
        self.underline_offset = metrics.underline_offset;
        self.strikeout_offset = metrics.strikeout_offset;
        self.stroke_size = metrics.stroke_size;
    }
}

// ---------------------------------------------------------------------------
//  Free functions
// ---------------------------------------------------------------------------

/// Load all four bundled `CaskaydiaCove` Nerd Font faces.
///
/// # Errors
///
/// Returns [`FontManagerError::BundledFontCorrupt`] if any bundled font data
/// fails to parse by swash.  This is a build-time invariant — the font files
/// are embedded via `include_bytes!` and should always be valid; a failure here
/// indicates a packaging error.
fn load_bundled_faces() -> Result<PrimaryFaces, FontManagerError> {
    let regular =
        LoadedFace::from_static(CASKAYDIA_REGULAR).ok_or(FontManagerError::BundledFontCorrupt {
            face: "CaskaydiaCove-Regular.ttf",
        })?;
    let bold =
        LoadedFace::from_static(CASKAYDIA_BOLD).ok_or(FontManagerError::BundledFontCorrupt {
            face: "CaskaydiaCove-Bold.ttf",
        })?;
    let italic =
        LoadedFace::from_static(CASKAYDIA_ITALIC).ok_or(FontManagerError::BundledFontCorrupt {
            face: "CaskaydiaCove-Italic.ttf",
        })?;
    let bold_italic = LoadedFace::from_static(CASKAYDIA_BOLD_ITALIC).ok_or(
        FontManagerError::BundledFontCorrupt {
            face: "CaskaydiaCove-BoldItalic.ttf",
        },
    )?;

    Ok(PrimaryFaces {
        regular,
        bold,
        italic,
        bold_italic,
    })
}

/// Attempt to load a user font by file path or system font name.
///
/// If the string is an existing file, loads it directly. Otherwise, searches
/// `fontdb` for a matching family name.
///
/// # Errors
///
/// Returns [`FontManagerError::ReparseFailed`] if a previously-validated font
/// buffer fails to re-parse.  `Ok(None)` means the font was not found and the
/// caller should fall back to the bundled default.
fn load_user_faces(
    path_or_name: &str,
    font_db: &Database,
) -> Result<Option<PrimaryFaces>, FontManagerError> {
    // Try file path first.
    let path = Path::new(path_or_name);
    if path.exists()
        && path.is_file()
        && let Ok(data) = std::fs::read(path)
    {
        return Ok(build_user_primary_from_data(data, path_or_name, font_db));
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
///
/// # Errors
///
/// Returns [`FontManagerError::ReparseFailed`] if re-parsing the already-validated
/// regular face data fails when building a style fallback.  `Ok(None)` means the
/// family was not found in the system font database.
fn load_user_faces_by_name(
    name: &str,
    font_db: &Database,
) -> Result<Option<PrimaryFaces>, FontManagerError> {
    // Find the family in fontdb (case-insensitive substring match).
    let Some(regular_data) =
        find_system_font_data(font_db, name, fontdb::Weight::NORMAL, fontdb::Style::Normal)
    else {
        return Ok(None);
    };
    let Some(regular) = LoadedFace::from_owned(regular_data, 0) else {
        return Ok(None);
    };

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
    let bold = match bold {
        Some(face) => face,
        None => LoadedFace::from_owned(regular.data.as_bytes().to_vec(), 0)
            .ok_or(FontManagerError::ReparseFailed { variant: "bold" })?,
    };
    let italic = match italic {
        Some(face) => face,
        None => LoadedFace::from_owned(regular.data.as_bytes().to_vec(), 0)
            .ok_or(FontManagerError::ReparseFailed { variant: "italic" })?,
    };
    let bold_italic = match bold_italic {
        Some(face) => face,
        None => LoadedFace::from_owned(regular.data.as_bytes().to_vec(), 0).ok_or(
            FontManagerError::ReparseFailed {
                variant: "bold-italic",
            },
        )?,
    };

    Ok(Some(PrimaryFaces {
        regular,
        bold,
        italic,
        bold_italic,
    }))
}

/// Remove all whitespace from a string (for fuzzy font name matching).
fn strip_whitespace(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).collect()
}

/// Return `true` if an installed font `family` name refers to the same font as
/// the bundled default ([`BUNDLED_FONT_FAMILY`]).
///
/// Uses the same whitespace-stripped, case-insensitive comparison the font
/// lookup uses for naming variations (e.g. "Caskaydia Cove Nerd Font" vs
/// "`CaskaydiaCove` Nerd Font"), so a system copy of the bundled font is
/// recognised regardless of how fontconfig spells the family.
#[must_use]
pub fn family_matches_bundled(family: &str) -> bool {
    strip_whitespace(&family.to_lowercase())
        .eq_ignore_ascii_case(&strip_whitespace(&BUNDLED_FONT_FAMILY.to_lowercase()))
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

/// Choose the emoji face: the best capable color-emoji font installed on the
/// system, falling back to the bundled Noto Color Emoji so emoji always render
/// (Task #402).
///
/// System faces are ranked by capability, not merely by name:
///
/// 1. A candidate must actually be a **color** font ([`LoadedFace::has_color_glyphs`])
///    — this is what stops a plain text font that maps an emoji codepoint to a
///    monochrome outline from being chosen.
/// 2. Among color faces, a **known-good family name** (in [`EMOJI_CANDIDATES`]
///    order) is a strong prior, and **emoji codepoint coverage**
///    ([`LoadedFace::emoji_coverage`]) breaks ties and rescues well-covered
///    fonts with non-standard names (e.g. `JoyPixels`, a distro-renamed Noto).
///
/// This fixes the "user has an emoji font installed but we didn't find it
/// because it isn't named exactly `Noto Color Emoji`" tofu bug, while keeping
/// the previously-preferred fonts preferred. If no capable system face is
/// found, the bundled Noto face is used.
fn discover_emoji_face(font_db: &Database) -> Option<LoadedFace> {
    load_emoji_face_from_source(&resolve_emoji_source(font_db))
}

/// Load the [`LoadedFace`] for a resolved [`EmojiSource`], falling back to the
/// bundled floor if a system source can no longer be read.
///
/// Split out from [`discover_emoji_face`] so it can be exercised without the
/// process-global [`EMOJI_SOURCE_CACHE`] (which would otherwise couple tests to
/// execution order and the host's installed fonts).
fn load_emoji_face_from_source(source: &EmojiSource) -> Option<LoadedFace> {
    match source {
        EmojiSource::SystemFile { path, index } => {
            if let Ok(bytes) = std::fs::read(path)
                && let Some(loaded) = LoadedFace::from_owned(bytes, *index)
            {
                info!("Using system emoji font: {}", path.display());
                return Some(loaded);
            }
            // The cached source vanished (font uninstalled between windows) —
            // fall through to the bundled floor.
            warn!(
                "Cached system emoji font no longer loadable ({}); using bundled",
                path.display()
            );
            load_bundled_emoji_floor()
        }
        EmojiSource::Bundled => load_bundled_emoji_floor(),
    }
}

/// Load the bundled Noto Color Emoji floor face.
fn load_bundled_emoji_floor() -> Option<LoadedFace> {
    let bundled = LoadedFace::from_static(NOTO_COLOR_EMOJI);
    if bundled.is_some() {
        info!("Using bundled Noto Color Emoji (no suitable system emoji font)");
    } else {
        warn!("Bundled Noto Color Emoji failed to load");
    }
    bundled
}

/// A resolved emoji-font source: either a concrete system font file, or the
/// bundled floor. Cheap to clone and store in the process-global cache.
#[derive(Clone, Debug, PartialEq, Eq)]
enum EmojiSource {
    /// A system font file at `path`, face `index`.
    SystemFile {
        path: std::path::PathBuf,
        index: usize,
    },
    /// The bundled Noto Color Emoji floor.
    Bundled,
}

/// Process-global cache of the resolved emoji source.
///
/// Emoji discovery is host-configuration-invariant within a process run, but
/// `FontManager::new` runs **once per window**. Without this cache, opening a
/// window on a system with no known-named emoji font would re-run the full
/// capability scan — reading and parsing *every* installed font file from disk
/// — every single time. We resolve the source once and reuse it; only the
/// (single) winning font file is re-read per window.
static EMOJI_SOURCE_CACHE: std::sync::OnceLock<EmojiSource> = std::sync::OnceLock::new();

/// Resolve (and memoize process-wide) which emoji source to use.
fn resolve_emoji_source(font_db: &Database) -> EmojiSource {
    EMOJI_SOURCE_CACHE
        .get_or_init(|| best_system_emoji_source(font_db).unwrap_or(EmojiSource::Bundled))
        .clone()
}

/// Find the best color emoji face installed on the system, or `None`.
///
/// Two passes, so the common case stays cheap:
///
/// 1. **Fast path** — try the known emoji family names ([`EMOJI_CANDIDATES`])
///    in priority order, filtering by `fontdb`'s in-memory family metadata
///    (no disk I/O) before loading a candidate. The first one that is a real
///    color font wins. This is what runs on virtually every desktop.
/// 2. **Capability scan** — only if no known-named emoji font is installed do
///    we fall back to scanning *all* faces, loading each, gating on the color
///    tables, and ranking by real emoji-block coverage. This rescues fonts
///    with non-standard names (e.g. `JoyPixels`, a distro-renamed Noto) at the
///    cost of a fuller scan, which only happens when the fast path found
///    nothing.
///
/// Returns the resolved [`EmojiSource`] (path + index) rather than a loaded
/// face, so the result can be cheaply cached process-wide (see
/// [`resolve_emoji_source`]).
fn best_system_emoji_source(font_db: &Database) -> Option<EmojiSource> {
    // Fast path: known names, metadata-filtered, stop at the first color face.
    // Name match is case-insensitive so e.g. "noto color emoji" also matches.
    for candidate in EMOJI_CANDIDATES {
        let candidate_lower = candidate.to_lowercase();
        for face in font_db.faces() {
            if !face
                .families
                .iter()
                .any(|(fam, _)| fam.to_lowercase().contains(&candidate_lower))
            {
                continue;
            }
            let fontdb::Source::File(path) = &face.source else {
                continue;
            };
            let Ok(bytes) = std::fs::read(path) else {
                continue;
            };
            let index = usize::value_from(face.index).unwrap_or(0);
            if let Some(loaded) = LoadedFace::from_owned(bytes, index)
                && loaded.has_color_glyphs()
            {
                return Some(EmojiSource::SystemFile {
                    path: path.clone(),
                    index,
                });
            }
        }
    }

    // Capability scan: no known emoji font installed — rank every color face by
    // real coverage.
    let mut best: Option<(u32, EmojiSource)> = None;
    for face in font_db.faces() {
        let fontdb::Source::File(path) = &face.source else {
            continue;
        };
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let index = usize::value_from(face.index).unwrap_or(0);
        let Some(loaded) = LoadedFace::from_owned(bytes, index) else {
            continue;
        };
        if !loaded.has_color_glyphs() {
            continue;
        }
        let coverage = loaded.emoji_coverage();
        if coverage > 0
            && best
                .as_ref()
                .is_none_or(|(best_cov, _)| coverage > *best_cov)
        {
            best = Some((
                coverage,
                EmojiSource::SystemFile {
                    path: path.clone(),
                    index,
                },
            ));
        }
    }
    best.map(|(_, source)| source)
}

/// Search `fontdb` for any font containing the given codepoint.
fn find_system_face_for_char(font_db: &Database, c: char) -> Option<LoadedFace> {
    for face in font_db.faces() {
        if let fontdb::Source::File(path) = &face.source
            && let Ok(bytes) = std::fs::read(path)
            && let Some(loaded) =
                LoadedFace::from_owned(bytes, usize::value_from(face.index).unwrap_or(0))
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

/// Compute the pixel dimensions of a single terminal cell from a loaded font face.
///
/// `font_size_ppem` is in pixels-per-em — the value passed directly to
/// swash's `Metrics::scale()`. Callers must convert from typographic points
/// using [`pt_to_ppem`] before calling this function.
///
/// ## Cell width
///
/// The advance width of the ASCII character `'0'` at `font_size_ppem` is used
/// as the canonical cell width.  This avoids the inflated `max_width` that
/// Nerd Font variants carry (their icon/symbol glyphs are far wider than the
/// monospace text glyphs).  Fallback chain:
/// 1. `gm.advance_width(glyph_for_'0')`
/// 2. `metrics.average_width` (if `'0'` has no glyph or zero advance)
/// 3. `metrics.max_width` (last resort)
///
/// ## Cell height and baseline
///
/// Cell height is a *deliberate* line height (the tight `ascent + |descent|`
/// ink box scaled by the configured `font.line_height` factor, floored by the
/// font's own line-gap and — for Nerd Fonts only — the OS/2 `usWinAscent + usWinDescent`
/// box). The extra leading over the tight box is then split evenly above the
/// ascender line and below the descender line, so the space above the tallest
/// glyphs equals the space below the lowest ones. See [`compute_cell_layout`]
/// for the exact, independently unit-tested formula and the rationale for
/// abandoning the previous "cell height = ascent + descent" behaviour (which
/// had no leading to split, leaving ascenders flush against the top edge and
/// the glyphless descent band as empty space below — text biased upward).
fn compute_cell_metrics(
    face: &LoadedFace,
    font_size_ppem: f32,
    line_height: f32,
) -> Result<CellMetrics, FontManagerError> {
    use swash::{TableProvider, tag_from_bytes};

    let font_ref = face
        .as_font_ref()
        .ok_or(FontManagerError::FontRefUnavailable)?;

    let metrics = font_ref.metrics(&[]).scale(font_size_ppem);

    // Determine cell width from the advance width of a representative ASCII
    // glyph ('0').  For a true monospace font every glyph has the same advance,
    // but Nerd Font variants include wide icon/symbol glyphs that inflate
    // `metrics.max_width` far beyond the regular character advance.  Measuring
    // a concrete glyph gives us the correct monospace cell width.
    let glyph_id = font_ref.charmap().map('0');
    let cell_width = if glyph_id != 0 {
        let gm = font_ref.glyph_metrics(&[]).scale(font_size_ppem);
        let advance = gm.advance_width(glyph_id);
        if advance > 0.0 {
            advance.ceil().approx_as::<u32>().unwrap_or(1)
        } else {
            // Fallback: average_width > max_width
            let aw = metrics.average_width.ceil().approx_as::<u32>().unwrap_or(0);
            if aw > 0 {
                aw
            } else {
                metrics.max_width.ceil().approx_as::<u32>().unwrap_or(1)
            }
        }
    } else {
        // '0' not in font — use average_width as a reasonable default.
        let aw = metrics.average_width.ceil().approx_as::<u32>().unwrap_or(0);
        if aw > 0 {
            aw
        } else {
            metrics.max_width.ceil().approx_as::<u32>().unwrap_or(1)
        }
    };

    // --- Determine cell height and baseline placement ---
    //
    // See [`compute_cell_layout`] for the full geometry model. In short: a
    // deliberate line height (factor-scaled ink box, floored by the font's
    // line-gap), with the baseline placed so the *visible ink* (cap-height
    // above, descent below) is centred in the cell — anchored on real ink
    // extent rather than the arbitrary per-font padded ascent.
    let ascent = metrics.ascent;
    let descent = metrics.descent.abs();
    let leading = metrics.leading.max(0.0);

    // Win height from the OS/2 table (font design units → pixels).  This is
    // only actually applied as a height floor when `is_nerd_font` is true
    // (see `compute_cell_layout`), but it is cheap to compute unconditionally.
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

    let is_nerd_font = has_powerline_glyphs(face);
    let layout = compute_cell_layout(
        ascent,
        descent,
        leading,
        win_height,
        is_nerd_font,
        line_height,
    );

    // Ensure non-zero dimensions.
    let cell_width = cell_width.max(1);
    let cell_height = layout.cell_height.max(1);

    Ok(CellMetrics {
        cell_width,
        cell_height,
        ascent: layout.baseline_offset,
        descent,
        underline_offset: metrics.underline_offset,
        strikeout_offset: metrics.strikeout_offset,
        stroke_size: metrics.stroke_size,
    })
}

/// Powerline core separator glyphs (right/left solid and angled triangles),
/// `U+E0B0`–`U+E0B3`.
///
/// This is the original Powerline patcher's Private-Use-Area range; Nerd
/// Fonts preserve these exact codepoints for backward compatibility with
/// Powerline-style shell prompts, so their presence is the single most
/// reliable signal that a font is Nerd-Font/Powerline-patched. Ordinary
/// (non-patched) fonts essentially never ship glyphs in this PUA range.
///
/// A wider probe set was considered — e.g. also requiring one of the
/// Nerd-Font-only icon glyphs such as `U+F001` (Font Awesome) or `U+E5FA`
/// (devicons) — but the core Powerline block alone is deliberately used as
/// the *sole* gate: it is stable across every Nerd Font patch level and
/// every partial "just the powerline glyphs" patch, whereas the icon PUA
/// ranges vary between Nerd Font versions and are absent from some
/// minimal powerline-only patches. Requiring icon coverage in addition to
/// the core block would produce false negatives on those minimal patches;
/// checking icons *instead of* the core block would be a weaker signal
/// (broader unrelated PUA squatting is more common in icon-only ranges).
const POWERLINE_CORE_GLYPHS: [char; 4] = ['\u{E0B0}', '\u{E0B1}', '\u{E0B2}', '\u{E0B3}'];

/// Returns `true` if `face` appears to be a Nerd Font / Powerline-patched
/// font, based on whether it contains **any** of the core Powerline separator
/// glyphs ([`POWERLINE_CORE_GLYPHS`]).
///
/// This gates whether the OS/2 `usWinAscent`/`usWinDescent` height floor is
/// applied in [`compute_cell_layout`]: that floor exists to give
/// Nerd Font box-drawing/icon glyphs enough vertical room, and applying it
/// to an ordinary font (which has no such glyphs, but may still carry
/// generous, unrelated win metrics) would inflate that font's row height
/// for no reason.
///
/// The gate is `any`, not `all`: some minimal Powerline patches ship only a
/// subset of the four core separators (e.g. just the two solid triangles,
/// omitting the angled variants, or vice-versa). Requiring *all four* would
/// misclassify such a font as non-Nerd and strip the win-metrics headroom
/// from the icon glyphs it *does* carry — clipping them. Any single core PUA
/// separator is already a decisive Nerd-Font signal (ordinary fonts do not
/// ship glyphs in this PUA range), so presence of any one is sufficient.
///
/// Note: the separators themselves (`U+E0B0`–`U+E0BF`) are now rendered
/// procedurally (see `crate::gui::box_drawing`), so this probe is used purely
/// as a *font-type signal* for the headroom decision — not because we depend
/// on the font's own separator glyphs.
fn has_powerline_glyphs(face: &LoadedFace) -> bool {
    POWERLINE_CORE_GLYPHS.iter().any(|&c| face.has_glyph(c))
}

/// Output of [`compute_cell_layout`]: the final integer cell height and the
/// baseline offset (distance in pixels from the top of the cell down to the
/// text baseline).
#[derive(Debug, Clone, Copy, PartialEq)]
struct CellLayout {
    cell_height: u32,
    baseline_offset: f32,
}

/// Decide the final cell height and the baseline offset from font metrics.
///
/// All inputs are pixel-scaled swash metrics (already absolute-valued where
/// noted). `leading` is the font's own line-gap. `win_height` is the OS/2
/// `usWinAscent + usWinDescent` sum; `is_nerd_font` gates whether it acts as a
/// height floor (see [`has_powerline_glyphs`]).
///
/// ## The problem this fixes
///
/// The previous behaviour set `cell_height = ceil(ascent + |descent|)` and
/// `baseline = ascent` (plus only the sub-pixel rounding slack, split evenly).
/// That split *was* symmetric on the ascent/descent box — but there was
/// essentially no leading to split, so the ascender line sat ~0.2px from the
/// top edge of the cell. Lines without descenders (e.g. a row of `l`s) then
/// looked cramped at the top with the whole (glyphless) descent band as empty
/// space below: text visibly biased upward and the rows uncomfortably tight.
///
/// ## The model
///
/// 1. **Line height.** The row pitch is the tight ink box `ascent + |descent|`
///    scaled by `line_height_factor` (the configured `font.line_height`,
///    clamped to at least 1.0), floored by the font's own `leading` and (for
///    Nerd Fonts only) the OS/2 `win_height`. This deliberate leading is the
///    breathing room the tight box lacked.
/// 2. **Baseline.** The extra leading (`cell_height - (ascent + |descent|)`) is
///    split evenly above the ascender line and below the descender line:
///    `baseline = (cell_height + ascent - |descent|) / 2`. The gap above the
///    tallest glyphs then equals the gap below the lowest ones, for any font.
/// 3. **Clip guards.** The baseline is clamped so the full ascent always fits
///    above it and the full descent below it (both hold with margin for the
///    symmetric split; the guards only bite on pathological metrics).
///
/// Block-drawing / box glyphs are unaffected by the taller cell: they are
/// rasterised *procedurally at the exact current cell size* (see
/// `emit_procedural_glyph` / `crate::gui::box_drawing`) and span the cell
/// rectangle exactly, so they tile seamlessly at any cell height. The old
/// "cell height must equal the font's block-glyph ink box" constraint no
/// longer applies now that block glyphs are procedural.
fn compute_cell_layout(
    ascent: f32,
    descent: f32,
    leading: f32,
    win_height: f32,
    is_nerd_font: bool,
    line_height_factor: f32,
) -> CellLayout {
    // Tight ink box (baseline-relative extent of the padded ascent/descent).
    let tight_box = ascent + descent;

    // Deliberate line height: factor-scaled tight box, floored by the font's
    // own line-gap and (Nerd-Font-only) the OS/2 win-metrics box.
    let factor_height = tight_box * line_height_factor.max(1.0);
    let gap_height = tight_box + leading.max(0.0);
    let mut target_height = factor_height.max(gap_height);
    if is_nerd_font {
        target_height = target_height.max(win_height);
    }

    let cell_height_f = target_height.ceil();
    let cell_height = cell_height_f.approx_as::<u32>().unwrap_or(1).max(1);

    // Centre the ascent/descent box within the (now taller) cell: the extra
    // leading is split evenly above the ascender line and below the descender
    // line, so the gap above the tallest glyphs equals the gap below the
    // lowest ones.
    //
    //   baseline = top_leading + ascent,  where top_leading = (H - (asc+desc))/2
    //            = (H + ascent - descent) / 2
    //
    // The previous behaviour computed the *same* even split, but the cell
    // height was pinned to `ceil(ascent + descent)` — so there was essentially
    // no leading to split, leaving ascenders ~0.2px from the top edge and the
    // (glyphless) descent band looking like empty space below. Adding the
    // deliberate line height above and splitting it evenly is what gives the
    // top and bottom breathing room the same size.
    let mut baseline_offset = (cell_height_f + ascent - descent) * 0.5;

    // Clamp so the full ascent always fits above the baseline (no top clip)
    // and the full descent fits below it. Both hold with room to spare for the
    // symmetric split above; the guards only matter for pathological metrics.
    if baseline_offset < ascent {
        baseline_offset = ascent;
    }
    if baseline_offset + descent > cell_height_f {
        baseline_offset = cell_height_f - descent;
    }

    CellLayout {
        cell_height,
        baseline_offset,
    }
}

/// Cell-geometry output of [`compute_cell_metrics`].
#[derive(Debug, Clone, Copy)]
struct CellMetrics {
    cell_width: u32,
    cell_height: u32,
    ascent: f32,
    descent: f32,
    underline_offset: f32,
    strikeout_offset: f32,
    stroke_size: f32,
}

// ---------------------------------------------------------------------------
//  Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    /// Reference line-height factor for the layout tests: mirrors the runtime
    /// default `FontConfig::default().line_height` (1.05). The real value is
    /// user-configurable via `config.font.line_height` and threaded into
    /// `compute_cell_layout`; these pure-function tests pin it explicitly.
    const LINE_HEIGHT_FACTOR: f32 = 1.05;

    /// Helper to create a default `FontManager` with bundled fonts.
    ///
    /// Uses `pixels_per_point = 1.0` (standard non-HiDPI).
    fn default_manager() -> FontManager {
        FontManager::new(&Config::default(), 1.0).unwrap()
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

    // --- Test 2: Cell size computation matches expected range for CaskaydiaCove ---

    #[test]
    fn cell_size_reasonable_for_caskaydia() {
        let fm = default_manager();
        // At 12pt, CaskaydiaCove Nerd Font produces a 10x19 px cell on the
        // reference machine. The asserted range stays deliberately wide
        // (5-20 wide, 10-30 tall) to tolerate per-platform metric/rounding
        // variation while still catching a grossly wrong cell size.
        assert!(
            fm.cell_width >= 5 && fm.cell_width <= 20,
            "cell_width {} out of expected range for 12pt CaskaydiaCove",
            fm.cell_width
        );
        assert!(
            fm.cell_height >= 10 && fm.cell_height <= 30,
            "cell_height {} out of expected range for 12pt CaskaydiaCove",
            fm.cell_height
        );
    }

    // --- Test 2b: Cell height accounts for OS/2 win metrics (powerline) ---
    //
    // CaskaydiaCove IS a Nerd Font (it has the core Powerline separator
    // glyphs — see `caskaydia_is_detected_as_nerd_font` below), so the
    // win-metrics height floor legitimately applies to it and this test
    // must still pass after the fix for issue 403: gating that floor on
    // `has_powerline_glyphs` does not regress the one bundled font that
    // actually needs the extra headroom.

    #[test]
    #[allow(clippy::unwrap_used)]
    fn cell_height_includes_win_metrics() {
        use swash::{FontRef, TableProvider, tag_from_bytes};

        // Load the bundled CaskaydiaCove font and compute the win-metric height
        // at the default font size.  The cell height from `FontManager` must
        // be at least as tall as this value.
        let font_ref = FontRef::from_index(CASKAYDIA_REGULAR, 0);
        assert!(font_ref.is_some(), "bundled CaskaydiaCove must parse");
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
        assert!(os2_data.is_some(), "CaskaydiaCove must have an OS/2 table");
        let os2_data = os2_data.unwrap();
        let win_metrics = read_os2_win_metrics(os2_data);
        assert!(win_metrics.is_some(), "OS/2 table must have win metrics");
        let (wa, wd) = win_metrics.unwrap();
        assert!(
            wa > 0 || wd > 0,
            "CaskaydiaCove must have non-zero win metrics"
        );

        let win_height_px = f32::from(wa).mul_add(scale_fdu, f32::from(wd) * scale_fdu);

        let fm = default_manager();
        #[allow(clippy::cast_precision_loss)]
        let cell_h_f = fm.cell_height as f32;
        assert!(
            cell_h_f >= win_height_px.floor(),
            "cell_height ({cell_h_f}) must be >= win_height ({win_height_px})"
        );
    }

    // --- Test 2c: CaskaydiaCove is detected as a Nerd/Powerline font ---

    #[test]
    fn caskaydia_is_detected_as_nerd_font() {
        let face = LoadedFace::from_static(CASKAYDIA_REGULAR)
            .expect("bundled CaskaydiaCove must load as a LoadedFace");
        assert!(
            has_powerline_glyphs(&face),
            "CaskaydiaCove Nerd Font must be detected via its core Powerline \
             separator glyphs (U+E0B0-U+E0B3)"
        );
    }

    // --- Task 402: bundled Noto Color Emoji + capability-based ranking ---

    #[test]
    fn bundled_noto_is_a_color_emoji_face() {
        let noto = LoadedFace::from_static(NOTO_COLOR_EMOJI)
            .expect("bundled Noto Color Emoji must load as a LoadedFace");
        assert!(
            noto.has_color_glyphs(),
            "Noto Color Emoji must be detected as a color font (CBDT/CBLC)"
        );
        // A real color-emoji font covers a large chunk of the emoji blocks.
        assert!(
            noto.emoji_coverage() > 500,
            "Noto emoji coverage unexpectedly low: {}",
            noto.emoji_coverage()
        );
    }

    #[test]
    fn caskaydia_is_rejected_by_the_color_gate() {
        // A text/Nerd font must be rejected as an emoji face by the color gate
        // even though it may map a handful of dingbats/symbols as monochrome
        // outlines (its emoji-block coverage is trivial).
        let cask = LoadedFace::from_static(CASKAYDIA_REGULAR).expect("bundled CaskaydiaCove loads");
        assert!(
            !cask.has_color_glyphs(),
            "a monochrome text font must not be treated as a color emoji font"
        );
        assert!(
            cask.emoji_coverage() < noto_coverage_floor(),
            "a text font's emoji-block coverage must be far below a real emoji font's"
        );
    }

    /// A conservative lower bound on the bundled Noto face's emoji coverage,
    /// used to contrast against non-emoji fonts.
    fn noto_coverage_floor() -> u32 {
        500
    }

    #[test]
    fn emoji_face_falls_back_to_bundled_noto() {
        // An empty font database has no system emoji font, so source resolution
        // must yield the bundled floor. Exercised via the uncached path
        // (`best_system_emoji_source` + `load_emoji_face_from_source`) so this
        // test does not depend on / mutate the process-global source cache.
        let empty_db = Database::new();
        let source = best_system_emoji_source(&empty_db).unwrap_or(EmojiSource::Bundled);
        assert_eq!(
            source,
            EmojiSource::Bundled,
            "an empty db must resolve to the bundled floor"
        );
        let face = load_emoji_face_from_source(&source).expect("bundled floor must load");
        assert!(
            face.has_color_glyphs(),
            "the bundled fallback must be a color emoji face"
        );
    }

    #[test]
    fn default_manager_always_has_an_emoji_face() {
        // Regardless of the host, the manager must have a usable emoji face
        // (bundled floor) so emoji never resolve to tofu.
        let fm = default_manager();
        assert!(
            fm.emoji_face.is_some(),
            "FontManager must always have an emoji face (bundled Noto floor)"
        );
    }

    // --- Test 2d: `has_powerline_glyphs` accepts a *partial* core set ---
    //
    // Regression guard for the gating logic: a face carrying **any** of the
    // four core Powerline separators must be classified as a Nerd Font, so a
    // minimal/partial Powerline patch (e.g. only the two solid triangles) is
    // not stripped of its win-metrics headroom. We can't easily construct a
    // synthetic swash face with an arbitrary partial charmap in a unit test,
    // so the downstream height decision is exercised through
    // `compute_cell_layout`'s `is_nerd_font` parameter in the synthetic tests
    // below, which is exactly the boolean `has_powerline_glyphs` feeds into.

    // --- Test 2e: cell layout — synthetic, pure-function tests ---
    //
    // `compute_cell_layout` is the pure decision function factored out of
    // `compute_cell_metrics` so the height/baseline formula can be tested with
    // synthetic metric combinations, independent of any real font file.
    //
    // The invariants that matter most:
    //   (1) The ascent/descent box is centred in the (taller) cell: the gap
    //       above the ascender line equals the gap below the descender line.
    //   (2) A deliberate line height is applied (the tight box gets leading).
    //   (3) The full ascent/descent never clips.

    /// Cell height as `f32` for a computed layout.
    fn cell_h_f(layout: CellLayout) -> f32 {
        f32::from(u16::try_from(layout.cell_height).unwrap_or(u16::MAX))
    }

    /// Gap above the ascender line for a computed layout.
    fn gap_above_ascent(layout: CellLayout, ascent: f32) -> f32 {
        layout.baseline_offset - ascent
    }

    /// Gap below the descender line for a computed layout.
    fn gap_below_descent(layout: CellLayout, descent: f32) -> f32 {
        cell_h_f(layout) - layout.baseline_offset - descent
    }

    #[test]
    fn layout_centres_ascent_descent_box() {
        // Balanced-metric font (approximating CaskaydiaCove's typo metrics at
        // 14pt). The leading above the ascender must equal the leading below
        // the descender.
        let ascent = 17.32;
        let descent = 4.38;
        let layout = compute_cell_layout(ascent, descent, 0.0, 21.69, true, LINE_HEIGHT_FACTOR);
        let above = gap_above_ascent(layout, ascent);
        let below = gap_below_descent(layout, descent);
        assert!(
            above >= -0.001,
            "full ascent must fit (no top clip): {above}"
        );
        assert!(
            below >= -0.001,
            "full descent must fit (no bottom clip): {below}"
        );
        // Balanced within one pixel of integer-rounding slack.
        assert!(
            (above - below).abs() <= 1.0,
            "leading should be split evenly: above {above} vs below {below}"
        );
    }

    #[test]
    fn layout_centres_descent_heavy_font() {
        // Descent-heavy font (approximating Liberation Mono's hhea metrics at
        // 14pt: ascent 15.5, descent 5.6). Under the OLD behaviour the cell was
        // pinned to `ceil(ascent + descent)` with ~0.2px of leading, leaving
        // ascenders flush against the top. The new layout must give equal
        // breathing room above and below.
        let ascent = 15.54;
        let descent = 5.61;
        let layout = compute_cell_layout(ascent, descent, 0.0, 21.15, false, LINE_HEIGHT_FACTOR);
        let above = gap_above_ascent(layout, ascent);
        let below = gap_below_descent(layout, descent);
        assert!(
            above > 0.5,
            "there must be real breathing room above the ascender, got {above}"
        );
        assert!(
            (above - below).abs() <= 1.0,
            "leading should be split evenly: above {above} vs below {below}"
        );
    }

    #[test]
    fn layout_applies_line_height_factor() {
        // Cell height must exceed the tight `ascent + |descent|` ink box by
        // roughly the LINE_HEIGHT_FACTOR (the deliberate breathing room), not
        // sit exactly on the tight box as the previous behaviour did.
        let ascent = 12.0;
        let descent = 4.0;
        let tight = ascent + descent; // 16.0
        let layout = compute_cell_layout(ascent, descent, 0.0, 0.0, false, LINE_HEIGHT_FACTOR);
        assert!(
            cell_h_f(layout) >= (tight * LINE_HEIGHT_FACTOR).floor(),
            "cell height {} should reflect the line-height factor over tight box {tight}",
            cell_h_f(layout)
        );
    }

    #[test]
    fn layout_honours_font_leading_as_a_floor() {
        // A font asking for a large line-gap gets at least that much spacing,
        // even when it exceeds LINE_HEIGHT_FACTOR.
        let ascent = 12.0;
        let descent = 4.0;
        let big_leading = 20.0;
        let layout =
            compute_cell_layout(ascent, descent, big_leading, 0.0, false, LINE_HEIGHT_FACTOR);
        assert!(
            cell_h_f(layout) >= ascent + descent + big_leading - 1.0,
            "cell height {} should honour the font's own leading floor",
            cell_h_f(layout)
        );
    }

    #[test]
    fn layout_win_floor_only_applies_when_nerd_font() {
        // A large win_height acts as a height floor only for Nerd Fonts.
        let ascent = 6.0;
        let descent = 4.0;
        let win = 40.0;
        let non_nerd = compute_cell_layout(ascent, descent, 0.0, win, false, LINE_HEIGHT_FACTOR);
        let nerd = compute_cell_layout(ascent, descent, 0.0, win, true, LINE_HEIGHT_FACTOR);
        assert!(
            cell_h_f(non_nerd) < win,
            "non-Nerd faces must not be inflated by win_height"
        );
        assert!(
            cell_h_f(nerd) >= win,
            "Nerd faces must be floored to at least win_height"
        );
    }

    #[test]
    fn layout_never_produces_zero_height() {
        // Degenerate all-zero inputs must still floor to a 1px cell height.
        let layout = compute_cell_layout(0.0, 0.0, 0.0, 0.0, false, LINE_HEIGHT_FACTOR);
        assert_eq!(layout.cell_height, 1);
    }

    #[test]
    fn layout_larger_factor_adds_more_leading() {
        // A larger configured line-height factor must produce a taller cell
        // (more leading), and a factor below 1.0 is clamped to 1.0 (never
        // shrinks the cell below the tight ink box).
        let ascent = 12.0;
        let descent = 4.0;
        let tight = compute_cell_layout(ascent, descent, 0.0, 0.0, false, 1.0);
        let loose = compute_cell_layout(ascent, descent, 0.0, 0.0, false, 1.5);
        assert!(
            cell_h_f(loose) > cell_h_f(tight),
            "factor 1.5 (cell {}) must be taller than factor 1.0 (cell {})",
            cell_h_f(loose),
            cell_h_f(tight)
        );

        let clamped = compute_cell_layout(ascent, descent, 0.0, 0.0, false, 0.5);
        assert_eq!(
            clamped.cell_height, tight.cell_height,
            "a factor below 1.0 must be clamped to 1.0 (no sub-tight-box cell)"
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
        let fm = FontManager::new(&config, 1.0).unwrap();

        // Should have fallen back to bundled CaskaydiaCove as primary.
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
        let mut fm = FontManager::new(&config, 1.0).unwrap();
        let result = fm.rebuild(&config, 1.0).unwrap();
        assert_eq!(result, RebuildResult::NoChange);
    }

    // --- Test 11: rebuild() with size change ---

    #[test]
    fn rebuild_size_change() {
        let config = Config::default();
        let mut fm = FontManager::new(&config, 1.0).unwrap();

        // Pre-populate the glyph cache.
        let style = GlyphStyle::new(false, false);
        let _ = fm.resolve_glyph('A', style);
        assert!(!fm.glyph_cache.is_empty(), "cache should have an entry");

        let old_width = fm.cell_width;
        let old_height = fm.cell_height;

        let mut new_config = config;
        new_config.font.size = 24.0;
        let result = fm.rebuild(&new_config, 1.0).unwrap();

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
        let mut fm = FontManager::new(&config, 1.0).unwrap();

        let mut new_config = config;
        new_config.font.family = Some("/nonexistent/font.ttf".to_owned());
        let result = fm.rebuild(&new_config, 1.0).unwrap();

        // The requested font fails to load, so the effective family stays as
        // bundled CaskaydiaCove (None → None).  No observable change.
        assert_eq!(result, RebuildResult::NoChange);
        // Should have gracefully fallen back to bundled.
        assert!(fm.cell_width > 0);
    }

    // --- Test 13: GlyphStyle::from_format ---

    #[test]
    fn glyph_style_from_format() {
        let mut flags = FontDecorationFlags::empty();
        flags.insert(FontDecorations::Italic);
        let style = GlyphStyle::from_format(&FontWeight::Bold, flags);
        assert!(style.bold);
        assert!(style.italic);

        let style = GlyphStyle::from_format(&FontWeight::Normal, FontDecorationFlags::empty());
        assert!(!style.bold);
        assert!(!style.italic);
    }

    // --- Test 14: cached rustybuzz Face construction (Task #430) ---

    #[test]
    fn build_cached_face_creation() {
        let fm = default_manager();
        let cached = fm.build_cached_face(FaceId::PrimaryRegular);
        assert!(
            cached.is_some(),
            "Should be able to build a cached rustybuzz Face from primary regular"
        );
    }

    // --- Task #430: shape_cached reuses the Face + ShapePlan caches ---

    #[test]
    fn shape_cached_reuses_face_and_plan_across_calls() {
        let fm = default_manager();
        let features = [rustybuzz::Feature::new(
            rustybuzz::ttf_parser::Tag::from_bytes(b"kern"),
            1,
            ..,
        )];

        let mut buffer = rustybuzz::UnicodeBuffer::new();
        buffer.push_str("AB");
        buffer.guess_segment_properties();
        let first = fm.shape_cached(FaceId::PrimaryRegular, false, &features, buffer);
        assert!(first.is_some(), "first shape_cached call should succeed");
        assert_eq!(
            fm.face_cache_len(),
            1,
            "face cache should have one entry after the first call"
        );
        assert_eq!(
            fm.plan_cache_len(),
            1,
            "plan cache should have one entry after the first call"
        );

        let mut buffer2 = rustybuzz::UnicodeBuffer::new();
        buffer2.push_str("CD");
        buffer2.guess_segment_properties();
        let second = fm.shape_cached(FaceId::PrimaryRegular, false, &features, buffer2);
        assert!(second.is_some(), "second shape_cached call should succeed");
        assert_eq!(
            fm.face_cache_len(),
            1,
            "face cache must not grow on a cache hit for the same face"
        );
        assert_eq!(
            fm.plan_cache_len(),
            1,
            "plan cache must not grow on a cache hit for the same \
             (face, ligatures, script, direction) combination"
        );
    }

    #[test]
    fn shape_cached_misses_on_face_change() {
        let fm = default_manager();
        let features = [rustybuzz::Feature::new(
            rustybuzz::ttf_parser::Tag::from_bytes(b"kern"),
            1,
            ..,
        )];

        let mut buffer = rustybuzz::UnicodeBuffer::new();
        buffer.push_str("A");
        buffer.guess_segment_properties();
        let _ = fm.shape_cached(FaceId::PrimaryRegular, false, &features, buffer);

        let mut buffer2 = rustybuzz::UnicodeBuffer::new();
        buffer2.push_str("B");
        buffer2.guess_segment_properties();
        let _ = fm.shape_cached(FaceId::PrimaryBold, false, &features, buffer2);

        assert_eq!(
            fm.face_cache_len(),
            2,
            "a different FaceId must populate a separate face-cache entry"
        );
        assert_eq!(
            fm.plan_cache_len(),
            2,
            "a different FaceId must populate a separate plan-cache entry"
        );
    }

    #[test]
    fn rebuild_clears_face_and_plan_caches() {
        let config = Config::default();
        let mut fm = FontManager::new(&config, 1.0).unwrap();
        let features = [rustybuzz::Feature::new(
            rustybuzz::ttf_parser::Tag::from_bytes(b"kern"),
            1,
            ..,
        )];

        let mut buffer = rustybuzz::UnicodeBuffer::new();
        buffer.push_str("A");
        buffer.guess_segment_properties();
        let _ = fm.shape_cached(FaceId::PrimaryRegular, false, &features, buffer);
        assert_eq!(fm.face_cache_len(), 1);
        assert_eq!(fm.plan_cache_len(), 1);

        let mut new_config = config;
        new_config.font.size = 24.0;
        let _ = fm.rebuild(&new_config, 1.0).unwrap();

        assert_eq!(
            fm.face_cache_len(),
            0,
            "face cache must be cleared by rebuild()"
        );
        assert_eq!(
            fm.plan_cache_len(),
            0,
            "plan cache must be cleared by rebuild()"
        );
    }

    #[test]
    fn set_font_size_clears_face_and_plan_caches() {
        let mut fm = default_manager();
        let features = [rustybuzz::Feature::new(
            rustybuzz::ttf_parser::Tag::from_bytes(b"kern"),
            1,
            ..,
        )];

        let mut buffer = rustybuzz::UnicodeBuffer::new();
        buffer.push_str("A");
        buffer.guess_segment_properties();
        let _ = fm.shape_cached(FaceId::PrimaryRegular, false, &features, buffer);
        assert_eq!(fm.face_cache_len(), 1);
        assert_eq!(fm.plan_cache_len(), 1);

        let _ = fm.set_font_size(fm.font_size_pt() + 4.0).unwrap();

        assert_eq!(
            fm.face_cache_len(),
            0,
            "face cache must be cleared by set_font_size()"
        );
        assert_eq!(
            fm.plan_cache_len(),
            0,
            "plan cache must be cleared by set_font_size()"
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
        let fm_small = FontManager::new(&config_small, 1.0).unwrap();

        let mut config_large = Config::default();
        config_large.font.size = 32.0;
        let fm_large = FontManager::new(&config_large, 1.0).unwrap();

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

    #[test]
    fn family_matches_bundled_recognises_naming_variants() {
        // Exact and spacing/case variants of the bundled family all match.
        assert!(family_matches_bundled(BUNDLED_FONT_FAMILY));
        assert!(family_matches_bundled("CaskaydiaCove Nerd Font"));
        assert!(family_matches_bundled("Caskaydia Cove Nerd Font"));
        assert!(family_matches_bundled("caskaydiacove nerd font"));
        assert!(family_matches_bundled("CaskaydiaCoveNerdFont"));

        // Unrelated fonts do not match.
        assert!(!family_matches_bundled("Fira Code"));
        assert!(!family_matches_bundled("Cascadia Code"));
        assert!(!family_matches_bundled("Caskaydia Mono Nerd Font"));
        assert!(!family_matches_bundled(""));
    }

    // --- Test: update_pixels_per_point returns false when unchanged ---

    #[test]
    fn update_ppp_unchanged_returns_false() {
        let mut fm = default_manager();
        // The manager was created with ppp = 1.0; updating with the same
        // value must be a no-op.
        assert!(
            !fm.update_pixels_per_point(1.0).unwrap(),
            "Same pixels_per_point should return false"
        );
    }

    // --- Test: update_pixels_per_point returns true and updates metrics ---

    #[test]
    fn update_ppp_changed_updates_cell_size() {
        let mut fm = default_manager();
        let old_w = fm.cell_width;
        let old_h = fm.cell_height;

        // Switching to 2.0 (simulating a HiDPI monitor) must return true
        // and produce different cell metrics.
        assert!(
            fm.update_pixels_per_point(2.0).unwrap(),
            "Different pixels_per_point should return true"
        );
        assert_ne!(
            (fm.cell_width, fm.cell_height),
            (old_w, old_h),
            "Cell size must change after DPI scale change"
        );
        // At 2x DPI the ppem doubles, so cell size should roughly double.
        assert!(
            fm.cell_width > old_w,
            "Cell width should increase at higher DPI"
        );
        assert!(
            fm.cell_height > old_h,
            "Cell height should increase at higher DPI"
        );
    }

    // --- Test: update_pixels_per_point clears glyph cache ---

    #[test]
    fn update_ppp_changed_clears_caches() {
        let mut fm = default_manager();

        // Populate the glyph cache.
        let style = GlyphStyle::new(false, false);
        let _ = fm.resolve_glyph('A', style);
        assert!(!fm.glyph_cache.is_empty(), "cache should be populated");

        let _ = fm.update_pixels_per_point(2.0).unwrap();
        assert!(
            fm.glyph_cache.is_empty(),
            "glyph cache must be cleared after DPI change"
        );
    }

    // --- Test: enumerate_monospace_families returns sorted, deduplicated list ---

    #[test]
    fn enumerate_monospace_families_returns_sorted_deduplicated_list() {
        let fm = default_manager();
        let families = fm.enumerate_monospace_families();

        // The list may be empty on minimal CI/Docker environments where no
        // system fonts are installed — the bundled CaskaydiaCove is loaded via
        // swash, not registered in fontdb.  We only assert structural
        // properties (sorted, deduplicated) here.

        // Verify sorted order.
        for pair in families.windows(2) {
            assert!(
                pair[0] <= pair[1],
                "Families must be sorted, but found {:?} before {:?}",
                pair[0],
                pair[1]
            );
        }

        // Verify no duplicates.
        let mut deduped = families.clone();
        deduped.dedup();
        assert_eq!(
            families.len(),
            deduped.len(),
            "enumerate_monospace_families must not contain duplicates"
        );
    }

    // --- Tests: set_font_size ---

    #[test]
    fn set_font_size_changes_metrics() {
        let mut fm = default_manager();
        let old_w = fm.cell_width;
        let old_h = fm.cell_height;
        let old_pt = fm.font_size_pt();

        // Increase by 8pt — should produce larger cells.
        let changed = fm.set_font_size(old_pt + 8.0).unwrap();
        assert!(
            changed,
            "set_font_size should return true when size differs"
        );
        assert!(
            fm.cell_width > old_w,
            "cell width should increase with larger font"
        );
        assert!(
            fm.cell_height > old_h,
            "cell height should increase with larger font"
        );
        assert!(
            (fm.font_size_pt() - (old_pt + 8.0)).abs() < f32::EPSILON,
            "font_size_pt should reflect the new size"
        );
    }

    #[test]
    fn set_font_size_same_size_returns_false() {
        let mut fm = default_manager();
        let current = fm.font_size_pt();
        let changed = fm.set_font_size(current).unwrap();
        assert!(
            !changed,
            "set_font_size should return false when size is unchanged"
        );
    }

    #[test]
    fn set_font_size_clears_glyph_cache() {
        let mut fm = default_manager();
        let style = GlyphStyle::new(false, false);
        let _ = fm.resolve_glyph('A', style);
        assert!(!fm.glyph_cache.is_empty(), "cache should be populated");

        let _ = fm.set_font_size(fm.font_size_pt() + 4.0).unwrap();
        assert!(
            fm.glyph_cache.is_empty(),
            "glyph cache must be cleared after font size change"
        );
    }
}
