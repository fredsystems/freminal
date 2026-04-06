// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! SGR (Select Graphic Rendition) mapping for [`TerminalHandler`].
//!
//! This module contains all functions responsible for translating SGR escape
//! sequence parameters into [`FormatTag`] field mutations:
//!
//! - [`apply_sgr`] — pure function: maps a single [`SelectGraphicRendition`]
//!   variant onto a [`FormatTag`] in-place.
//! - [`TerminalHandler::handle_sgr`] — resolves palette indices then delegates
//!   to [`apply_sgr`].
//! - [`TerminalHandler::build_sgr_response`] — serialises the current
//!   [`FormatTag`] state back to an SGR parameter string (used by DECRQSS).
//! - [`append_color_sgr`] / [`append_underline_color_sgr`] — helper free
//!   functions used by [`TerminalHandler::build_sgr_response`].

use freminal_common::{
    buffer_states::{
        cursor::ReverseVideo,
        fonts::{BlinkState, FontDecorations, FontWeight},
        format_tag::FormatTag,
    },
    colors::TerminalColor,
    sgr::SelectGraphicRendition,
};

use super::TerminalHandler;

impl TerminalHandler {
    /// Handle SGR (Select Graphic Rendition) — update `current_format` and propagate to buffer.
    pub fn handle_sgr(&mut self, sgr: &SelectGraphicRendition) {
        // Resolve PaletteIndex colors against the mutable palette before applying.
        let resolved = match sgr {
            SelectGraphicRendition::Foreground(TerminalColor::PaletteIndex(idx)) => {
                SelectGraphicRendition::Foreground(
                    self.palette.lookup(usize::from(*idx), self.theme),
                )
            }
            SelectGraphicRendition::Background(TerminalColor::PaletteIndex(idx)) => {
                SelectGraphicRendition::Background(
                    self.palette.lookup(usize::from(*idx), self.theme),
                )
            }
            SelectGraphicRendition::UnderlineColor(TerminalColor::PaletteIndex(idx)) => {
                SelectGraphicRendition::UnderlineColor(
                    self.palette.lookup(usize::from(*idx), self.theme),
                )
            }
            _ => *sgr,
        };
        apply_sgr(&mut self.current_format, &resolved);
        self.buffer.set_format(self.current_format.clone());
    }

    /// Build the SGR parameter string for the current format state.
    ///
    /// Returns a string like `0;1;4;38;2;255;0;0` representing the active SGR
    /// attributes.  The leading `0` (reset) is always included; individual
    /// attributes are appended only when they differ from the default.
    pub(super) fn build_sgr_response(&self) -> String {
        let fmt = self.current_format();
        let mut parts: Vec<String> = vec!["0".to_string()];

        // Font weight
        if fmt.font_weight == FontWeight::Bold {
            parts.push("1".to_string());
        }

        // Font decorations
        for dec in fmt.font_decorations.iter() {
            match dec {
                FontDecorations::Faint => parts.push("2".to_string()),
                FontDecorations::Italic => parts.push("3".to_string()),
                FontDecorations::Underline => parts.push("4".to_string()),
                FontDecorations::Strikethrough => parts.push("9".to_string()),
            }
        }

        // Reverse video
        if fmt.colors.reverse_video == ReverseVideo::On {
            parts.push("7".to_string());
        }

        // Foreground color
        Self::append_color_sgr(&mut parts, fmt.colors.color, true);

        // Background color
        Self::append_color_sgr(&mut parts, fmt.colors.background_color, false);

        // Underline color (SGR 58)
        if fmt.colors.underline_color != TerminalColor::DefaultUnderlineColor {
            Self::append_underline_color_sgr(&mut parts, fmt.colors.underline_color);
        }

        parts.join(";")
    }

    /// Append SGR parameters for a foreground (`is_fg = true`) or background color.
    fn append_color_sgr(parts: &mut Vec<String>, color: TerminalColor, is_fg: bool) {
        let (base, idx_code, rgb_code) = if is_fg { (30, 38, 38) } else { (40, 48, 48) };

        match color {
            TerminalColor::Black => parts.push(format!("{base}")),
            TerminalColor::Red => parts.push(format!("{}", base + 1)),
            TerminalColor::Green => parts.push(format!("{}", base + 2)),
            TerminalColor::Yellow => parts.push(format!("{}", base + 3)),
            TerminalColor::Blue => parts.push(format!("{}", base + 4)),
            TerminalColor::Magenta => parts.push(format!("{}", base + 5)),
            TerminalColor::Cyan => parts.push(format!("{}", base + 6)),
            TerminalColor::White => parts.push(format!("{}", base + 7)),
            TerminalColor::BrightBlack => parts.push(format!("{}", base + 60)),
            TerminalColor::BrightRed => parts.push(format!("{}", base + 61)),
            TerminalColor::BrightGreen => parts.push(format!("{}", base + 62)),
            TerminalColor::BrightYellow => parts.push(format!("{}", base + 63)),
            TerminalColor::BrightBlue => parts.push(format!("{}", base + 64)),
            TerminalColor::BrightMagenta => parts.push(format!("{}", base + 65)),
            TerminalColor::BrightCyan => parts.push(format!("{}", base + 66)),
            TerminalColor::BrightWhite => parts.push(format!("{}", base + 67)),
            TerminalColor::PaletteIndex(idx) => {
                parts.push(format!("{idx_code};5;{idx}"));
            }
            TerminalColor::Custom(r, g, b) => {
                parts.push(format!("{rgb_code};2;{r};{g};{b}"));
            }
            // Default, DefaultBackground, DefaultUnderlineColor, DefaultCursorColor — no SGR needed
            _ => {}
        }
    }

    /// Append SGR 58 (underline color) parameters.
    fn append_underline_color_sgr(parts: &mut Vec<String>, color: TerminalColor) {
        match color {
            TerminalColor::PaletteIndex(idx) => {
                parts.push(format!("58;5;{idx}"));
            }
            TerminalColor::Custom(r, g, b) => {
                parts.push(format!("58;2;{r};{g};{b}"));
            }
            // Named colors as underline color: encode as palette index 0-15
            TerminalColor::Black => parts.push("58;5;0".to_string()),
            TerminalColor::Red => parts.push("58;5;1".to_string()),
            TerminalColor::Green => parts.push("58;5;2".to_string()),
            TerminalColor::Yellow => parts.push("58;5;3".to_string()),
            TerminalColor::Blue => parts.push("58;5;4".to_string()),
            TerminalColor::Magenta => parts.push("58;5;5".to_string()),
            TerminalColor::Cyan => parts.push("58;5;6".to_string()),
            TerminalColor::White => parts.push("58;5;7".to_string()),
            TerminalColor::BrightBlack => parts.push("58;5;8".to_string()),
            TerminalColor::BrightRed => parts.push("58;5;9".to_string()),
            TerminalColor::BrightGreen => parts.push("58;5;10".to_string()),
            TerminalColor::BrightYellow => parts.push("58;5;11".to_string()),
            TerminalColor::BrightBlue => parts.push("58;5;12".to_string()),
            TerminalColor::BrightMagenta => parts.push("58;5;13".to_string()),
            TerminalColor::BrightCyan => parts.push("58;5;14".to_string()),
            TerminalColor::BrightWhite => parts.push("58;5;15".to_string()),
            _ => {}
        }
    }
}

/// Apply a single `SelectGraphicRendition` value to a `FormatTag`, mutating it in-place.
///
/// This is the central mapping between the parser's SGR enum and the buffer's format
/// representation.  It is a pure function — it has no side effects beyond mutating `tag`.
// Inherently large: exhaustive match over all SGR variants mapping to `FormatTag` fields.
// Splitting would scatter a single coherent mapping.
#[allow(clippy::too_many_lines)]
pub(super) fn apply_sgr(tag: &mut FormatTag, sgr: &SelectGraphicRendition) {
    match sgr {
        // Reset: restore every field to its default value
        SelectGraphicRendition::Reset => {
            *tag = FormatTag::default();
        }

        // Font weight
        SelectGraphicRendition::Bold => {
            tag.font_weight = FontWeight::Bold;
        }
        SelectGraphicRendition::ResetBold => {
            tag.font_weight = FontWeight::Normal;
        }
        // NormalIntensity resets both bold AND faint
        SelectGraphicRendition::NormalIntensity => {
            tag.font_weight = FontWeight::Normal;
            tag.font_decorations.remove(FontDecorations::Faint);
        }

        // Italic
        SelectGraphicRendition::Italic => {
            tag.font_decorations.insert(FontDecorations::Italic);
        }
        SelectGraphicRendition::NotItalic => {
            tag.font_decorations.remove(FontDecorations::Italic);
        }

        // Faint
        SelectGraphicRendition::Faint => {
            tag.font_decorations.insert(FontDecorations::Faint);
        }

        // Underline
        SelectGraphicRendition::Underline => {
            tag.font_decorations.insert(FontDecorations::Underline);
        }
        SelectGraphicRendition::NotUnderlined => {
            tag.font_decorations.remove(FontDecorations::Underline);
        }

        // Strikethrough
        SelectGraphicRendition::Strikethrough => {
            tag.font_decorations.insert(FontDecorations::Strikethrough);
        }
        SelectGraphicRendition::NotStrikethrough => {
            tag.font_decorations.remove(FontDecorations::Strikethrough);
        }

        // Reverse video
        SelectGraphicRendition::ReverseVideo => {
            tag.colors.set_reverse_video(ReverseVideo::On);
        }
        SelectGraphicRendition::ResetReverseVideo => {
            tag.colors.set_reverse_video(ReverseVideo::Off);
        }

        // Colors
        SelectGraphicRendition::Foreground(color) => {
            tag.colors.set_color(*color);
        }
        SelectGraphicRendition::Background(color) => {
            tag.colors.set_background_color(*color);
        }
        SelectGraphicRendition::UnderlineColor(color) => {
            tag.colors.set_underline_color(*color);
        }

        // Blink
        SelectGraphicRendition::SlowBlink => {
            tag.blink = BlinkState::Slow;
        }
        SelectGraphicRendition::FastBlink => {
            tag.blink = BlinkState::Fast;
        }
        SelectGraphicRendition::NotBlinking => {
            tag.blink = BlinkState::None;
        }

        // Intentionally ignored attributes and unknown codes — these have no FormatTag
        // equivalent.  Silently ignore for forward compatibility.
        SelectGraphicRendition::NoOp
        | SelectGraphicRendition::Conceal
        | SelectGraphicRendition::Revealed
        | SelectGraphicRendition::PrimaryFont
        | SelectGraphicRendition::AlternativeFont1
        | SelectGraphicRendition::AlternativeFont2
        | SelectGraphicRendition::AlternativeFont3
        | SelectGraphicRendition::AlternativeFont4
        | SelectGraphicRendition::AlternativeFont5
        | SelectGraphicRendition::AlternativeFont6
        | SelectGraphicRendition::AlternativeFont7
        | SelectGraphicRendition::AlternativeFont8
        | SelectGraphicRendition::AlternativeFont9
        | SelectGraphicRendition::FontFranktur
        | SelectGraphicRendition::ProportionalSpacing
        | SelectGraphicRendition::DisableProportionalSpacing
        | SelectGraphicRendition::Framed
        | SelectGraphicRendition::Encircled
        | SelectGraphicRendition::Overlined
        | SelectGraphicRendition::NotOverlined
        | SelectGraphicRendition::NotFramedOrEncircled
        | SelectGraphicRendition::IdeogramUnderline
        | SelectGraphicRendition::IdeogramDoubleUnderline
        | SelectGraphicRendition::IdeogramOverline
        | SelectGraphicRendition::IdeogramDoubleOverline
        | SelectGraphicRendition::IdeogramStress
        | SelectGraphicRendition::IdeogramAttributes
        | SelectGraphicRendition::Superscript
        | SelectGraphicRendition::Subscript
        | SelectGraphicRendition::NeitherSuperscriptNorSubscript
        | SelectGraphicRendition::Unknown(_) => {}
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use freminal_common::{
        buffer_states::{
            cursor::ReverseVideo,
            fonts::{BlinkState, FontDecorations, FontWeight},
            format_tag::FormatTag,
            osc::AnsiOscType,
        },
        colors::TerminalColor,
        sgr::SelectGraphicRendition,
    };

    use super::*;

    // ------------------------------------------------------------------
    // apply_sgr unit tests (pure function, no buffer involved)
    // ------------------------------------------------------------------

    #[test]
    fn sgr_bold_sets_font_weight() {
        let mut tag = FormatTag::default();
        apply_sgr(&mut tag, &SelectGraphicRendition::Bold);
        assert_eq!(tag.font_weight, FontWeight::Bold);
    }

    #[test]
    fn sgr_reset_bold_clears_font_weight() {
        let mut tag = FormatTag::default();
        apply_sgr(&mut tag, &SelectGraphicRendition::Bold);
        apply_sgr(&mut tag, &SelectGraphicRendition::ResetBold);
        assert_eq!(tag.font_weight, FontWeight::Normal);
    }

    #[test]
    fn sgr_normal_intensity_clears_bold_and_faint() {
        let mut tag = FormatTag::default();
        apply_sgr(&mut tag, &SelectGraphicRendition::Bold);
        apply_sgr(&mut tag, &SelectGraphicRendition::Faint);
        apply_sgr(&mut tag, &SelectGraphicRendition::NormalIntensity);
        assert_eq!(tag.font_weight, FontWeight::Normal);
        assert!(!tag.font_decorations.contains(FontDecorations::Faint));
    }

    #[test]
    fn sgr_italic_toggle() {
        let mut tag = FormatTag::default();
        apply_sgr(&mut tag, &SelectGraphicRendition::Italic);
        assert!(tag.font_decorations.contains(FontDecorations::Italic));
        apply_sgr(&mut tag, &SelectGraphicRendition::NotItalic);
        assert!(!tag.font_decorations.contains(FontDecorations::Italic));
    }

    #[test]
    fn sgr_italic_not_duplicated() {
        let mut tag = FormatTag::default();
        apply_sgr(&mut tag, &SelectGraphicRendition::Italic);
        apply_sgr(&mut tag, &SelectGraphicRendition::Italic);
        assert_eq!(
            tag.font_decorations
                .iter()
                .filter(|d| *d == FontDecorations::Italic)
                .count(),
            1
        );
    }

    #[test]
    fn sgr_underline_toggle() {
        let mut tag = FormatTag::default();
        apply_sgr(&mut tag, &SelectGraphicRendition::Underline);
        assert!(tag.font_decorations.contains(FontDecorations::Underline));
        apply_sgr(&mut tag, &SelectGraphicRendition::NotUnderlined);
        assert!(!tag.font_decorations.contains(FontDecorations::Underline));
    }

    #[test]
    fn sgr_strikethrough_toggle() {
        let mut tag = FormatTag::default();
        apply_sgr(&mut tag, &SelectGraphicRendition::Strikethrough);
        assert!(
            tag.font_decorations
                .contains(FontDecorations::Strikethrough)
        );
        apply_sgr(&mut tag, &SelectGraphicRendition::NotStrikethrough);
        assert!(
            !tag.font_decorations
                .contains(FontDecorations::Strikethrough)
        );
    }

    #[test]
    fn sgr_faint_adds_decoration() {
        let mut tag = FormatTag::default();
        apply_sgr(&mut tag, &SelectGraphicRendition::Faint);
        assert!(tag.font_decorations.contains(FontDecorations::Faint));
    }

    #[test]
    fn sgr_fg_color() {
        let mut tag = FormatTag::default();
        apply_sgr(
            &mut tag,
            &SelectGraphicRendition::Foreground(TerminalColor::Red),
        );
        assert_eq!(tag.colors.color, TerminalColor::Red);
    }

    #[test]
    fn sgr_bg_color() {
        let mut tag = FormatTag::default();
        apply_sgr(
            &mut tag,
            &SelectGraphicRendition::Background(TerminalColor::Blue),
        );
        assert_eq!(tag.colors.background_color, TerminalColor::Blue);
    }

    #[test]
    fn sgr_custom_rgb_fg() {
        let mut tag = FormatTag::default();
        apply_sgr(
            &mut tag,
            &SelectGraphicRendition::Foreground(TerminalColor::Custom(255, 128, 0)),
        );
        assert_eq!(tag.colors.color, TerminalColor::Custom(255, 128, 0));
    }

    #[test]
    fn sgr_underline_color() {
        let mut tag = FormatTag::default();
        apply_sgr(
            &mut tag,
            &SelectGraphicRendition::UnderlineColor(TerminalColor::Green),
        );
        assert_eq!(tag.colors.underline_color, TerminalColor::Green);
    }

    #[test]
    fn sgr_reverse_video_on_off() {
        let mut tag = FormatTag::default();
        apply_sgr(&mut tag, &SelectGraphicRendition::ReverseVideo);
        assert_eq!(tag.colors.reverse_video, ReverseVideo::On);
        apply_sgr(&mut tag, &SelectGraphicRendition::ResetReverseVideo);
        assert_eq!(tag.colors.reverse_video, ReverseVideo::Off);
    }

    #[test]
    fn sgr_reset_clears_all() {
        let mut tag = FormatTag::default();
        apply_sgr(&mut tag, &SelectGraphicRendition::Bold);
        apply_sgr(
            &mut tag,
            &SelectGraphicRendition::Foreground(TerminalColor::Red),
        );
        apply_sgr(&mut tag, &SelectGraphicRendition::Italic);
        apply_sgr(&mut tag, &SelectGraphicRendition::Reset);
        assert_eq!(tag, FormatTag::default());
    }

    #[test]
    fn sgr_multiple_accumulate() {
        let mut tag = FormatTag::default();
        apply_sgr(&mut tag, &SelectGraphicRendition::Bold);
        apply_sgr(&mut tag, &SelectGraphicRendition::Underline);
        apply_sgr(
            &mut tag,
            &SelectGraphicRendition::Foreground(TerminalColor::Red),
        );
        assert_eq!(tag.font_weight, FontWeight::Bold);
        assert!(tag.font_decorations.contains(FontDecorations::Underline));
        assert_eq!(tag.colors.color, TerminalColor::Red);
    }

    #[test]
    fn sgr_noop_does_nothing() {
        let mut tag = FormatTag::default();
        apply_sgr(&mut tag, &SelectGraphicRendition::NoOp);
        assert_eq!(tag, FormatTag::default());
    }

    #[test]
    fn sgr_slow_blink_sets_blink_state() {
        let mut tag = FormatTag::default();
        apply_sgr(&mut tag, &SelectGraphicRendition::SlowBlink);
        assert_eq!(tag.blink, BlinkState::Slow);
    }

    #[test]
    fn sgr_fast_blink_sets_blink_state() {
        let mut tag = FormatTag::default();
        apply_sgr(&mut tag, &SelectGraphicRendition::FastBlink);
        assert_eq!(tag.blink, BlinkState::Fast);
    }

    #[test]
    fn sgr_not_blinking_clears_blink_state() {
        let mut tag = FormatTag::default();
        apply_sgr(&mut tag, &SelectGraphicRendition::SlowBlink);
        assert_eq!(tag.blink, BlinkState::Slow);
        apply_sgr(&mut tag, &SelectGraphicRendition::NotBlinking);
        assert_eq!(tag.blink, BlinkState::None);
    }

    #[test]
    fn sgr_reset_clears_blink_state() {
        let mut tag = FormatTag::default();
        apply_sgr(&mut tag, &SelectGraphicRendition::FastBlink);
        assert_eq!(tag.blink, BlinkState::Fast);
        apply_sgr(&mut tag, &SelectGraphicRendition::Reset);
        assert_eq!(tag.blink, BlinkState::None);
    }

    #[test]
    fn sgr_bold_and_blink_accumulate() {
        let mut tag = FormatTag::default();
        apply_sgr(&mut tag, &SelectGraphicRendition::Bold);
        apply_sgr(&mut tag, &SelectGraphicRendition::SlowBlink);
        assert_eq!(tag.font_weight, FontWeight::Bold);
        assert_eq!(tag.blink, BlinkState::Slow);
    }

    // ------------------------------------------------------------------
    // handle_sgr integration tests (via TerminalHandler)
    // ------------------------------------------------------------------

    #[test]
    fn handle_sgr_bold_propagates_to_buffer_format() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_sgr(&SelectGraphicRendition::Bold);
        assert_eq!(handler.current_format.font_weight, FontWeight::Bold);
    }

    #[test]
    fn handle_sgr_reset_propagates_to_buffer_format() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_sgr(&SelectGraphicRendition::Bold);
        handler.handle_sgr(&SelectGraphicRendition::Reset);
        assert_eq!(handler.current_format, FormatTag::default());
    }

    #[test]
    fn handle_sgr_slow_blink_propagates_to_current_format() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_sgr(&SelectGraphicRendition::SlowBlink);
        assert_eq!(handler.current_format.blink, BlinkState::Slow);
    }

    #[test]
    fn handle_sgr_fast_blink_propagates_to_current_format() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_sgr(&SelectGraphicRendition::FastBlink);
        assert_eq!(handler.current_format.blink, BlinkState::Fast);
    }

    #[test]
    fn handle_sgr_palette_index_resolves_against_palette() {
        let mut handler = TerminalHandler::new(80, 24);

        // Set index 42 to a custom colour.
        handler.handle_osc(&AnsiOscType::SetPaletteColor(42, 0xDE, 0xAD, 0x00));

        // Apply SGR foreground with PaletteIndex(42).
        handler.handle_sgr(&SelectGraphicRendition::Foreground(
            TerminalColor::PaletteIndex(42),
        ));

        // The resolved colour should be Custom(0xDE, 0xAD, 0x00), not PaletteIndex(42).
        let fmt = handler.current_format();
        assert_eq!(
            fmt.colors.color,
            TerminalColor::Custom(0xDE, 0xAD, 0x00),
            "PaletteIndex should be resolved to Custom via palette lookup"
        );
    }

    #[test]
    fn handle_sgr_palette_index_background_and_underline() {
        let mut handler = TerminalHandler::new(80, 24);

        handler.handle_osc(&AnsiOscType::SetPaletteColor(200, 0xAA, 0xBB, 0xCC));

        // Background
        handler.handle_sgr(&SelectGraphicRendition::Background(
            TerminalColor::PaletteIndex(200),
        ));
        assert_eq!(
            handler.current_format().colors.background_color,
            TerminalColor::Custom(0xAA, 0xBB, 0xCC),
        );

        // Underline colour
        handler.handle_sgr(&SelectGraphicRendition::UnderlineColor(
            TerminalColor::PaletteIndex(200),
        ));
        assert_eq!(
            handler.current_format().colors.underline_color,
            TerminalColor::Custom(0xAA, 0xBB, 0xCC),
        );
    }

    #[test]
    fn handle_sgr_palette_index_uses_default_when_no_override() {
        let mut handler = TerminalHandler::new(80, 24);

        // PaletteIndex(1) with no override → should resolve to the default for index 1.
        handler.handle_sgr(&SelectGraphicRendition::Foreground(
            TerminalColor::PaletteIndex(1),
        ));

        let expected = handler.palette().lookup(1, handler.theme());
        assert_eq!(
            handler.current_format().colors.color,
            expected,
            "PaletteIndex without override should resolve to default colour"
        );
    }
}
