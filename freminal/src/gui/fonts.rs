// Copyright (C) 2024-2025 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use eframe::egui::{self, FontData, FontDefinitions, FontFamily, FontId};
use freminal_common::buffer_states::fonts::{FontDecorations, FontWeight};

// -------------------------------------------------------------------------------------------------
//  Freminal Terminal – Unified Font Loader
//
//  Fallback order:
//
//      1. Bundled Primary Monospace Font (MesloLGS Nerd Mono)
//      2. Bundled Nerd Symbols Fallback (same bundle or separate)
//      3. User-Selected Primary Font (from config; optional)
//      4. Emoji Fallback Font (best available from system)
//      5. LAST RESORT: minimal system fallback (optional, rare)
//
//  We DO NOT choose system monospace fonts as primary.
//  We DO NOT depend on OS defaults.
// -------------------------------------------------------------------------------------------------

use std::path::Path;

// Primary bundled font names
const PRIMARY_REGULAR: &str = "freminal-primary-regular";
const PRIMARY_BOLD: &str = "freminal-primary-bold";
const PRIMARY_ITALIC: &str = "freminal-primary-italic";
const PRIMARY_BOLD_ITALIC: &str = "freminal-primary-bold-italic";

// Bundled Nerd symbols fallback (optional separate file)
//const NERD_SYMBOLS: &str = "freminal-nerd-symbols";

/// Example use:
///
/// ```rust
/// use freminal::gui::fonts::FontConfig;
///
/// let cfg = FontConfig {
///     user_font: Some("JetBrains Mono".to_string()),
///     size: 15.0,
///     enable_emoji_fallback: true,
///     enable_system_last_resort: false,
/// };
///
/// assert_eq!(cfg.size, 15.0);
/// ```

#[derive(Clone, Debug)]
pub struct FontConfig {
    pub user_font: Option<String>,
    pub size: f32,
    pub enable_emoji_fallback: bool,
    pub enable_system_last_resort: bool,
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            user_font: None,
            size: 12.0,
            enable_emoji_fallback: true,
            enable_system_last_resort: true,
        }
    }
}

// -------------------------------------------------------------------------------------------------
//  Public entry point: set up all fonts
// -------------------------------------------------------------------------------------------------

pub fn setup_font_files(ctx: &egui::Context, cfg: &FontConfig) {
    let mut defs = FontDefinitions::default();

    // 1. Load bundled primary font family (Meslo Nerd)
    load_bundled_primary_fonts(&mut defs);

    // 2. Load bundled Nerd symbols fallback
    load_bundled_nerd_symbols(&mut defs);

    // 3. User-selected primary font override (optional)
    if let Some(path_or_name) = &cfg.user_font {
        try_load_user_primary_font(path_or_name, &mut defs);
    }

    // 4. Emoji fallback (system, prioritized)
    if cfg.enable_emoji_fallback {
        emoji_fonts::add_emoji_fallback(&mut defs);
    }

    // 5. Last resort system fallback (disabled by default)
    if cfg.enable_system_last_resort {
        system_fallback::add_last_resort_system_fonts(&mut defs);
    }

    ctx.set_fonts(defs);
}

// -------------------------------------------------------------------------------------------------
// 1. Bundled primary font family
// -------------------------------------------------------------------------------------------------

fn load_bundled_primary_fonts(defs: &mut FontDefinitions) {
    defs.font_data.insert(
        PRIMARY_REGULAR.to_owned(),
        FontData::from_static(include_bytes!(
            "../../../res/MesloLGSNerdFontMono-Regular.ttf"
        ))
        .into(),
    );

    defs.font_data.insert(
        PRIMARY_BOLD.to_owned(),
        FontData::from_static(include_bytes!("../../../res/MesloLGSNerdFontMono-Bold.ttf")).into(),
    );

    defs.font_data.insert(
        PRIMARY_ITALIC.to_owned(),
        FontData::from_static(include_bytes!(
            "../../../res/MesloLGSNerdFontMono-Italic.ttf"
        ))
        .into(),
    );

    defs.font_data.insert(
        PRIMARY_BOLD_ITALIC.to_owned(),
        FontData::from_static(include_bytes!(
            "../../../res/MesloLGSNerdFontMono-BoldItalic.ttf"
        ))
        .into(),
    );

    // Add to terminal families
    defs.families.insert(
        FontFamily::Name(PRIMARY_REGULAR.into()),
        vec![PRIMARY_REGULAR.into()],
    );
    defs.families.insert(
        FontFamily::Name(PRIMARY_BOLD.into()),
        vec![PRIMARY_BOLD.into()],
    );
    defs.families.insert(
        FontFamily::Name(PRIMARY_ITALIC.into()),
        vec![PRIMARY_ITALIC.into()],
    );
    defs.families.insert(
        FontFamily::Name(PRIMARY_BOLD_ITALIC.into()),
        vec![PRIMARY_BOLD_ITALIC.into()],
    );

    // Monospace = primary regular
    let mono = defs.families.entry(FontFamily::Monospace).or_default();
    mono.insert(0, PRIMARY_REGULAR.to_owned());

    info!("Loaded bundled Freminal primary monospace font");
}

// -------------------------------------------------------------------------------------------------
// 2. Bundled Nerd Symbols (fallback)
// -------------------------------------------------------------------------------------------------
// FIXME: for now, we're just going to ignore bundled emoji fonts, but we probably should bundle here
const fn load_bundled_nerd_symbols(_defs: &mut FontDefinitions) {
    // If you have a separate Nerd symbols file, load it here.
    // If MesloLGS Nerd Mono already includes full symbols, you can comment this out.

    // Example (uncomment if you ship a standalone symbols font):

    //defs.font_data.insert(
    //     NERD_SYMBOLS.to_owned(),
    //     FontData::from_static(include_bytes!("../../../res/NerdSymbols-Regular.ttf")).into(),
    //);

    // Add fallback for all terminal families
    // add_fallback_to_terminal_families(defs, NERD_SYMBOLS);

    //info!("Added bundled Nerd symbols fallback");
}

// -------------------------------------------------------------------------------------------------
// 3. User-selected primary font
// -------------------------------------------------------------------------------------------------

fn try_load_user_primary_font(path_or_name: &str, defs: &mut FontDefinitions) {
    // CASE 1: File path
    let path = Path::new(path_or_name);
    if path.exists() && path.is_file() {
        match std::fs::read(path) {
            Ok(bytes) => {
                defs.font_data
                    .insert("user-primary".into(), FontData::from_owned(bytes).into());
                insert_user_primary_into_families(defs, "user-primary");
                info!(
                    "Loaded user-selected primary font from path: {}",
                    path.display()
                );
                return;
            }
            Err(err) => warn!("Failed to read user font '{}': {}", path_or_name, err),
        }
    }

    // CASE 2: System font family name
    if let Some(bytes) = find_system_font_by_name(path_or_name) {
        defs.font_data
            .insert("user-primary".into(), FontData::from_owned(bytes).into());
        insert_user_primary_into_families(defs, "user-primary");
        info!(
            "Loaded user-selected primary font by name '{}'",
            path_or_name
        );
    }
}

fn find_system_font_by_name(name: &str) -> Option<Vec<u8>> {
    let mut db = fontdb::Database::new();
    db.load_system_fonts();

    // Match family name case-insensitively
    for face in db.faces() {
        // Check family names
        let matches = face.families.iter().any(|fam| {
            fam.0.eq_ignore_ascii_case(name) || fam.0.to_lowercase().contains(&name.to_lowercase())
        });

        if !matches {
            continue;
        }

        // Load from file
        if let fontdb::Source::File(path) = &face.source {
            if let Ok(bytes) = std::fs::read(path) {
                return Some(bytes);
            }
        }
    }

    None
}

fn insert_user_primary_into_families(defs: &mut FontDefinitions, name: &str) {
    let families = [
        FontFamily::Monospace,
        FontFamily::Name(PRIMARY_REGULAR.into()),
        FontFamily::Name(PRIMARY_BOLD.into()),
        FontFamily::Name(PRIMARY_ITALIC.into()),
        FontFamily::Name(PRIMARY_BOLD_ITALIC.into()),
    ];

    for fam in families {
        if let Some(list) = defs.families.get_mut(&fam) {
            list.insert(0, name.to_owned());
        }
    }

    info!("Inserted user primary font into terminal families");
}

// -------------------------------------------------------------------------------------------------
// Shared fallback insertion
// -------------------------------------------------------------------------------------------------

fn add_fallback_to_terminal_families(defs: &mut FontDefinitions, name: &str) {
    let families = [
        FontFamily::Monospace,
        FontFamily::Name(PRIMARY_REGULAR.into()),
        FontFamily::Name(PRIMARY_BOLD.into()),
        FontFamily::Name(PRIMARY_ITALIC.into()),
        FontFamily::Name(PRIMARY_BOLD_ITALIC.into()),
    ];

    for fam in families {
        if let Some(list) = defs.families.get_mut(&fam) {
            if !list.contains(&name.to_owned()) {
                list.push(name.to_owned());
            }
        }
    }
}

// -------------------------------------------------------------------------------------------------
// 4. Emoji fallback (prioritized system fonts)
// -------------------------------------------------------------------------------------------------

mod emoji_fonts {
    use super::{FontData, FontDefinitions};
    use fontdb::{Database, Source};

    const CANDIDATES: &[&str] = &[
        "Apple Color Emoji",
        "Noto Color Emoji",
        "Segoe UI Emoji",
        "Twemoji",
        "Emoji One",
        "OpenMoji",
        "Emoji",
        "Symbola",
    ];

    pub fn add_emoji_fallback(defs: &mut FontDefinitions) {
        let mut db = Database::new();
        db.load_system_fonts();

        for candidate in CANDIDATES {
            if let Some((name, bytes)) = find_candidate(&db, candidate) {
                defs.font_data
                    .insert(name.clone(), FontData::from_owned(bytes).into());
                super::add_fallback_to_terminal_families(defs, &name);
                info!("Emoji fallback: using {}", name);
                return;
            }
        }

        warn!("Emoji fallback: no suitable emoji font found");
    }

    fn find_candidate(db: &Database, target: &str) -> Option<(String, Vec<u8>)> {
        for face in db.faces() {
            let matches = face.families.iter().any(|fam| fam.0.contains(target));

            if !matches {
                continue;
            }

            if let Source::File(path) = &face.source {
                if path.exists() {
                    if let Ok(bytes) = std::fs::read(path) {
                        let key = format!("emoji-{}", target.replace(' ', "_"));
                        return Some((key, bytes));
                    }
                }
            }
        }

        None
    }
}

// -------------------------------------------------------------------------------------------------
// 5. Minimal last resort fallback (system fonts)
// -------------------------------------------------------------------------------------------------

mod system_fallback {
    use super::{FontData, FontDefinitions};

    pub fn add_last_resort_system_fonts(defs: &mut FontDefinitions) {
        let safe_fonts = [
            "/System/Library/Fonts/Apple Color Emoji.ttc", // macOS
            "/usr/share/fonts/noto/NotoColorEmoji.ttf",    // Linux
            "/usr/share/fonts/truetype/noto/NotoColorEmoji.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf", // generic fallback
        ];

        for path in safe_fonts
            .iter()
            .filter(|p| std::path::Path::new(p).exists())
        {
            match std::fs::read(path) {
                Ok(bytes) => {
                    let key = format!("fallback-{}", path.replace('/', "_"));
                    defs.font_data
                        .insert(key.clone(), FontData::from_owned(bytes).into());
                    super::add_fallback_to_terminal_families(defs, &key);
                }
                Err(err) => {
                    tracing::warn!("Failed to load fallback font {}: {}", path, err);
                }
            }
        }

        tracing::info!("Added safe fallback fonts");
    }
}

// -------------------------------------------------------------------------------------------------
// TerminalFont wrapper (unchanged)
// -------------------------------------------------------------------------------------------------
#[derive(Clone, Debug)]
pub struct TerminalFont {
    regular: FontFamily,
    bold: FontFamily,
    italic: FontFamily,
    bold_italic: FontFamily,
    pub size: f32,
}

impl TerminalFont {
    #[must_use]
    pub fn new(size: f32) -> Self {
        Self {
            regular: FontFamily::Name(PRIMARY_REGULAR.into()),
            bold: FontFamily::Name(PRIMARY_BOLD.into()),
            italic: FontFamily::Name(PRIMARY_ITALIC.into()),
            bold_italic: FontFamily::Name(PRIMARY_BOLD_ITALIC.into()),
            size,
        }
    }

    #[must_use]
    pub fn get_family(&self, decs: &[FontDecorations], weight: &FontWeight) -> FontFamily {
        let italic = decs.contains(&FontDecorations::Italic);

        match (weight, italic) {
            (FontWeight::Bold, false) => self.bold.clone(),
            (FontWeight::Normal, false) => self.regular.clone(),
            (FontWeight::Normal, true) => self.italic.clone(),
            (FontWeight::Bold, true) => self.bold_italic.clone(),
        }
    }
}

// -------------------------------------------------------------------------------------------------
// Char size helper
// -------------------------------------------------------------------------------------------------

#[must_use]
pub fn get_char_size(ctx: &egui::Context, font: &TerminalFont) -> (f32, f32) {
    let id = FontId {
        size: font.size,
        family: font.regular.clone(),
    };

    let width = ctx.fonts_mut(|f| f.glyph_width(&id, ' '));
    let height = ctx.fonts_mut(|f| f.row_height(&id));

    (width, height)
}
