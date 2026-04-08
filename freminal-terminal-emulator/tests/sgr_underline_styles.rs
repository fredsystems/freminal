// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Tests for SGR underline style extensions (subtask 47.6).
//!
//! Covers:
//! - Colon-form underline styles (`4:0` through `4:5`)
//! - Mixed semicolon/colon delimiter handling
//! - Underline color via colon form (`58:2:R:G:B`, `58:5:IDX`)
//! - `UnderlineWithStyle` SGR variant propagation

use freminal_common::buffer_states::fonts::UnderlineStyle;
use freminal_common::buffer_states::terminal_output::TerminalOutput;
use freminal_common::colors::TerminalColor;
use freminal_common::sgr::SelectGraphicRendition;
use freminal_terminal_emulator::ansi::FreminalAnsiParser;

/// Push a string through the parser and return all outputs.
fn parse_seq(seq: &str) -> Vec<TerminalOutput> {
    let mut parser = FreminalAnsiParser::new();
    parser.push(seq.as_bytes())
}

/// Extract only the SGR variants from parser output.
fn extract_sgrs(seq: &str) -> Vec<SelectGraphicRendition> {
    parse_seq(seq)
        .into_iter()
        .filter_map(|o| match o {
            TerminalOutput::Sgr(s) => Some(s),
            _ => None,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Colon-form underline styles: 4:N
// ---------------------------------------------------------------------------

#[test]
fn sgr_colon_underline_style_none() {
    let sgrs = extract_sgrs("\x1b[4:0m");
    assert_eq!(sgrs, vec![SelectGraphicRendition::NotUnderlined]);
}

#[test]
fn sgr_colon_underline_style_single() {
    let sgrs = extract_sgrs("\x1b[4:1m");
    assert_eq!(
        sgrs,
        vec![SelectGraphicRendition::UnderlineWithStyle(
            UnderlineStyle::Single
        )]
    );
}

#[test]
fn sgr_colon_underline_style_double() {
    let sgrs = extract_sgrs("\x1b[4:2m");
    assert_eq!(
        sgrs,
        vec![SelectGraphicRendition::UnderlineWithStyle(
            UnderlineStyle::Double
        )]
    );
}

#[test]
fn sgr_colon_underline_style_curly() {
    let sgrs = extract_sgrs("\x1b[4:3m");
    assert_eq!(
        sgrs,
        vec![SelectGraphicRendition::UnderlineWithStyle(
            UnderlineStyle::Curly
        )]
    );
}

#[test]
fn sgr_colon_underline_style_dotted() {
    let sgrs = extract_sgrs("\x1b[4:4m");
    assert_eq!(
        sgrs,
        vec![SelectGraphicRendition::UnderlineWithStyle(
            UnderlineStyle::Dotted
        )]
    );
}

#[test]
fn sgr_colon_underline_style_dashed() {
    let sgrs = extract_sgrs("\x1b[4:5m");
    assert_eq!(
        sgrs,
        vec![SelectGraphicRendition::UnderlineWithStyle(
            UnderlineStyle::Dashed
        )]
    );
}

#[test]
fn sgr_colon_underline_style_unknown_value_clears() {
    // 4:6 is not a defined style — should produce NotUnderlined (same as 4:0)
    let sgrs = extract_sgrs("\x1b[4:6m");
    assert_eq!(sgrs, vec![SelectGraphicRendition::NotUnderlined]);
}

// ---------------------------------------------------------------------------
// Mixed semicolon/colon forms
// ---------------------------------------------------------------------------

#[test]
fn sgr_mixed_bold_curly_underline_truecolor_fg() {
    // Bold (1), curly underline (4:3), truecolor FG (38:2::255:0:0)
    let sgrs = extract_sgrs("\x1b[1;4:3;38:2::255:0:0m");
    assert_eq!(
        sgrs,
        vec![
            SelectGraphicRendition::Bold,
            SelectGraphicRendition::UnderlineWithStyle(UnderlineStyle::Curly),
            SelectGraphicRendition::Foreground(TerminalColor::Custom(255, 0, 0)),
        ]
    );
}

#[test]
fn sgr_semicolon_truecolor_with_colon_underline() {
    // Curly underline via colon, truecolor FG via semicolons
    let sgrs = extract_sgrs("\x1b[4:3;38;2;128;64;255m");
    assert_eq!(
        sgrs,
        vec![
            SelectGraphicRendition::UnderlineWithStyle(UnderlineStyle::Curly),
            SelectGraphicRendition::Foreground(TerminalColor::Custom(128, 64, 255)),
        ]
    );
}

#[test]
fn sgr_colon_truecolor_bg_with_semicolon_bold() {
    // Bold via semicolon, truecolor BG via colon
    let sgrs = extract_sgrs("\x1b[1;48:2::30:30:46m");
    assert_eq!(
        sgrs,
        vec![
            SelectGraphicRendition::Bold,
            SelectGraphicRendition::Background(TerminalColor::Custom(30, 30, 46)),
        ]
    );
}

// ---------------------------------------------------------------------------
// Underline color (SGR 58)
// ---------------------------------------------------------------------------

#[test]
fn sgr_colon_underline_color_truecolor() {
    let sgrs = extract_sgrs("\x1b[58:2::128:64:255m");
    assert_eq!(
        sgrs,
        vec![SelectGraphicRendition::UnderlineColor(
            TerminalColor::Custom(128, 64, 255)
        )]
    );
}

#[test]
fn sgr_colon_underline_color_palette() {
    let sgrs = extract_sgrs("\x1b[58:5:196m");
    assert_eq!(
        sgrs,
        vec![SelectGraphicRendition::UnderlineColor(
            TerminalColor::PaletteIndex(196)
        )]
    );
}

#[test]
fn sgr_semicolon_underline_color_truecolor() {
    let sgrs = extract_sgrs("\x1b[58;2;100;200;50m");
    assert_eq!(
        sgrs,
        vec![SelectGraphicRendition::UnderlineColor(
            TerminalColor::Custom(100, 200, 50)
        )]
    );
}

// ---------------------------------------------------------------------------
// Plain SGR 4 (no colon) still produces single underline
// ---------------------------------------------------------------------------

#[test]
fn sgr_plain_underline_produces_single() {
    let sgrs = extract_sgrs("\x1b[4m");
    assert_eq!(sgrs, vec![SelectGraphicRendition::Underline]);
}

#[test]
fn sgr_plain_not_underlined() {
    let sgrs = extract_sgrs("\x1b[24m");
    assert_eq!(sgrs, vec![SelectGraphicRendition::NotUnderlined]);
}

// ---------------------------------------------------------------------------
// Complex real-world sequences
// ---------------------------------------------------------------------------

#[test]
fn sgr_kitty_curly_underline_with_underline_color() {
    // Common kitty/neovim pattern: curly underline + red underline color
    let sgrs = extract_sgrs("\x1b[4:3;58:2::255:0:0m");
    assert_eq!(
        sgrs,
        vec![
            SelectGraphicRendition::UnderlineWithStyle(UnderlineStyle::Curly),
            SelectGraphicRendition::UnderlineColor(TerminalColor::Custom(255, 0, 0)),
        ]
    );
}

#[test]
fn sgr_reset_then_styled_underline() {
    let sgrs = extract_sgrs("\x1b[0;4:2m");
    assert_eq!(
        sgrs,
        vec![
            SelectGraphicRendition::Reset,
            SelectGraphicRendition::UnderlineWithStyle(UnderlineStyle::Double),
        ]
    );
}

#[test]
fn sgr_multiple_colon_segments() {
    // Multiple colon-form segments in one sequence
    let sgrs = extract_sgrs("\x1b[4:3;38:2::255:128:0;48:5:17m");
    assert_eq!(
        sgrs,
        vec![
            SelectGraphicRendition::UnderlineWithStyle(UnderlineStyle::Curly),
            SelectGraphicRendition::Foreground(TerminalColor::Custom(255, 128, 0)),
            SelectGraphicRendition::Background(TerminalColor::PaletteIndex(17)),
        ]
    );
}
