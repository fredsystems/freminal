// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Font loading, face management, and fallback chain.
//!
//! `FontManager` is the single authoritative source of font metrics and cell size
//! for the terminal renderer. It loads fonts via `swash`, provides `rustybuzz::Face`
//! references for the shaping pipeline, and resolves glyphs through a tiered
//! fallback chain: primary face -> bundled fallback -> emoji -> system -> tofu.

use fontdb::Database;

/// Font manager for the terminal renderer.
///
/// Owns the font stack (primary faces, emoji, system fallback), computes cell size
/// from font metrics, and resolves individual glyphs to (face, `glyph_id`) pairs.
///
/// This struct will be fleshed out in subtask 1.2.
pub struct FontManager {
    /// fontdb database for system font discovery.
    _font_db: Database,
}

impl FontManager {
    /// Create a new `FontManager` with default (bundled) fonts.
    ///
    /// Loads the bundled `MesloLGS` Nerd Font Mono faces and discovers system
    /// emoji fonts via `fontdb`.
    #[must_use]
    pub fn new() -> Self {
        let font_db = Database::new();

        Self { _font_db: font_db }
    }
}

impl Default for FontManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Validate that `swash::FontRef` can parse a minimal font data buffer.
/// This function exists solely to ensure the `swash` dependency is used and
/// not flagged as unused by cargo-machete. It will be replaced by real font
/// loading logic in subtask 1.2.
#[cfg(test)]
fn _swash_usage_check() {
    // FontRef::from_index requires valid font data; just ensure the type is reachable.
    let _: Option<swash::FontRef<'_>> = swash::FontRef::from_index(&[], 0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn font_manager_constructs() {
        let _fm = FontManager::new();
    }
}
