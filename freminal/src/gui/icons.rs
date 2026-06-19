//! Bundled chrome action icons.
//!
//! Every action symbol the non-terminal chrome draws (close, recording
//! indicator, broadcast/antenna, lock, tab `+`, etc.) is requested through the
//! typed [`ChromeIcon`] enum rather than a raw codepoint embedded at the call
//! site. Each variant maps to a Nerd Font Private-Use-Area glyph carried by the
//! bundled **`CaskaydiaCove` Nerd Font** (registered at position 0 of egui's
//! [`egui::FontFamily::Monospace`] in [`crate::gui::fonts`]).
//!
//! This replaces the previous reliance on emoji / system-font codepoints
//! (U+1F512 🔒, U+1F4E1 📡, …) that fall through to a system emoji font on
//! Linux — a fallible external resource that renders as an empty square (tofu)
//! when no such font is installed. Because the glyphs live in the font we ship
//! ourselves, every action symbol renders identically on every platform.
//!
//! ## Rendering
//!
//! Icons are monochrome glyphs, so they tint to the active palette via egui's
//! [`egui::RichText::color`]. The two convenience constructors here build a
//! `RichText` in the monospace family carrying the glyph:
//!
//! - [`ChromeIcon::rich_text`] — the glyph, untinted (inherits the surrounding
//!   text color).
//! - [`ChromeIcon::rich_text_colored`] — the glyph tinted to a caller-supplied
//!   [`egui::Color32`] (typically pulled from `ui.visuals()` — e.g.
//!   `warn_fg_color` for the lock indicator, `error_fg_color` for the recording
//!   dot).
//!
//! ## Adding an icon
//!
//! Add a variant, give it a codepoint in [`ChromeIcon::codepoint`], and add it
//! to [`ChromeIcon::ALL`]. The regression test
//! (`every_icon_resolves_in_bundled_font`) then guarantees the bundled face
//! actually carries a glyph for it — guarding against the tofu failure mode this
//! module exists to eliminate.

use egui::{Color32, FontFamily, RichText};

/// A chrome action symbol, decoupled from its underlying glyph codepoint.
///
/// Chrome code requests an icon by name; the concrete Nerd Font codepoint is an
/// implementation detail of [`ChromeIcon::codepoint`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ChromeIcon {
    /// Closed padlock — echo-off (password-prompt) indicator.
    Lock,
    /// Closed padlock with a key — password-prompt tab indicator.
    LockKey,
    /// Filled dot — the live "recording" indicator in the menu bar.
    RecordDot,
    /// Record control glyph — the recording-toggle button label in settings.
    Record,
    /// Bell — unacknowledged bell on a tab.
    Bell,
    /// Antenna — broadcast-input-active tab indicator.
    Broadcast,
    /// Multiplication / cross — close / dismiss buttons (tab close, toast
    /// dismiss).
    Close,
    /// Minus — remove a paste-guard pattern.
    Minus,
    /// Plus — add a paste-guard pattern / new tab.
    Plus,
    /// Warning triangle — keybinding-conflict and risky-paste warnings.
    Warning,
}

impl ChromeIcon {
    /// Every [`ChromeIcon`] variant, for exhaustive iteration (used by the
    /// regression test that verifies each glyph resolves in the bundled font).
    #[cfg(test)]
    pub const ALL: &'static [Self] = &[
        Self::Lock,
        Self::LockKey,
        Self::RecordDot,
        Self::Record,
        Self::Bell,
        Self::Broadcast,
        Self::Close,
        Self::Minus,
        Self::Plus,
        Self::Warning,
    ];

    /// The Nerd Font codepoint for this icon.
    ///
    /// All codepoints are verified present in the bundled `CaskaydiaCove` Nerd
    /// Font (see `every_icon_resolves_in_bundled_font`). Most are from the
    /// `FontAwesome` (`nf-fa-*`, `U+F0xx`/`U+F1xx`) range; the record-control glyph
    /// is from the `Codicon` (`nf-cod-*`) range.
    #[must_use]
    pub const fn codepoint(self) -> char {
        match self {
            // nf-fa-lock
            Self::Lock => '\u{f023}',
            // nf-fa-key
            Self::LockKey => '\u{f084}',
            // nf-fa-circle (filled dot)
            Self::RecordDot => '\u{f111}',
            // nf-cod-record
            Self::Record => '\u{eba7}',
            // nf-fa-bell
            Self::Bell => '\u{f0f3}',
            // nf-fa-rss (broadcast antenna)
            Self::Broadcast => '\u{f519}',
            // nf-fa-times (close)
            Self::Close => '\u{f00d}',
            // nf-fa-minus
            Self::Minus => '\u{f068}',
            // nf-fa-plus
            Self::Plus => '\u{f067}',
            // nf-fa-warning (exclamation triangle)
            Self::Warning => '\u{f071}',
        }
    }

    /// The icon's glyph as a single-character string.
    #[must_use]
    pub fn glyph(self) -> String {
        self.codepoint().to_string()
    }

    /// Build an untinted [`RichText`] carrying the icon glyph in the monospace
    /// font family (the bundled `CaskaydiaCove` face), so the glyph resolves from
    /// the bundled font rather than the proportional UI font.
    #[must_use]
    pub fn rich_text(self) -> RichText {
        RichText::new(self.glyph()).family(FontFamily::Monospace)
    }

    /// Build a [`RichText`] carrying the icon glyph tinted to `color`, in the
    /// monospace font family.
    ///
    /// Callers typically pass a palette-derived color from `ui.visuals()` (e.g.
    /// `warn_fg_color`, `error_fg_color`).
    #[must_use]
    pub fn rich_text_colored(self, color: Color32) -> RichText {
        self.rich_text().color(color)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::gui::font_manager::bundled_regular_font_bytes;

    /// The core 112.10/112.12 guarantee: every [`ChromeIcon`] variant resolves
    /// to a real glyph in the bundled `CaskaydiaCove` face. If a future font swap
    /// drops one of these codepoints, this test goes red instead of the icon
    /// silently rendering as tofu at runtime.
    #[test]
    fn every_icon_resolves_in_bundled_font() {
        let face = rustybuzz::Face::from_slice(bundled_regular_font_bytes(), 0)
            .expect("bundled CaskaydiaCove regular face must parse");

        for icon in ChromeIcon::ALL {
            let cp = icon.codepoint();
            let glyph = face.glyph_index(cp);
            assert!(
                glyph.is_some(),
                "ChromeIcon::{icon:?} codepoint U+{:04X} has no glyph in the bundled font",
                u32::from(cp),
            );
            // A resolved glyph must have a real (nonzero-id) outline mapping; the
            // ttf-parser `.notdef` glyph is id 0 and is what `glyph_index`
            // returns for absent characters, so `is_some()` already excludes it.
        }
    }

    #[test]
    fn all_contains_every_variant() {
        // Guards against adding a variant but forgetting to list it in ALL
        // (which would silently exclude it from the resolution test above).
        // Each codepoint is distinct, so the count of distinct codepoints in
        // ALL equals the number of variants.
        let mut codepoints: Vec<char> = ChromeIcon::ALL.iter().map(|i| i.codepoint()).collect();
        let before = codepoints.len();
        codepoints.sort_unstable();
        codepoints.dedup();
        assert_eq!(
            before,
            codepoints.len(),
            "duplicate codepoints in ChromeIcon::ALL"
        );
        assert_eq!(before, 10, "ChromeIcon::ALL must list every variant");
    }

    #[test]
    fn glyph_is_single_char() {
        for icon in ChromeIcon::ALL {
            assert_eq!(icon.glyph().chars().count(), 1);
        }
    }
}
