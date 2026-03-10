// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use freminal_common::colors::{
    ColorPalette, TerminalColor, cube_component, default_index_to_rgb, lookup_256_color_by_index,
};
use proptest::{prop_assert, prop_assert_eq, proptest};
use std::fmt::Write;
use std::str::FromStr;

//
// ---------- Deterministic Unit Tests ----------
//

#[test]
fn lookup_standard_colors_complete() {
    // Standard ANSI 0–15 mappings
    assert_eq!(lookup_256_color_by_index(0), TerminalColor::Black);
    assert_eq!(lookup_256_color_by_index(1), TerminalColor::Red);
    assert_eq!(lookup_256_color_by_index(2), TerminalColor::Green);
    assert_eq!(lookup_256_color_by_index(3), TerminalColor::Yellow);
    assert_eq!(lookup_256_color_by_index(4), TerminalColor::Blue);
    assert_eq!(lookup_256_color_by_index(5), TerminalColor::Magenta);
    assert_eq!(lookup_256_color_by_index(6), TerminalColor::Cyan);
    assert_eq!(lookup_256_color_by_index(7), TerminalColor::White);
    assert_eq!(lookup_256_color_by_index(8), TerminalColor::BrightBlack);
    assert_eq!(lookup_256_color_by_index(9), TerminalColor::BrightRed);
    assert_eq!(lookup_256_color_by_index(10), TerminalColor::BrightGreen);
    assert_eq!(lookup_256_color_by_index(11), TerminalColor::BrightYellow);
    assert_eq!(lookup_256_color_by_index(12), TerminalColor::BrightBlue);
    assert_eq!(lookup_256_color_by_index(13), TerminalColor::BrightMagenta);
    assert_eq!(lookup_256_color_by_index(14), TerminalColor::BrightCyan);
    assert_eq!(lookup_256_color_by_index(15), TerminalColor::BrightWhite);
}

#[test]
fn lookup_grayscale_range() {
    // 232 → rgb(8,8,8)
    assert_eq!(
        lookup_256_color_by_index(232),
        TerminalColor::Custom(8, 8, 8)
    );

    // 255 → rgb(238,238,238)
    assert_eq!(
        lookup_256_color_by_index(255),
        TerminalColor::Custom(238, 238, 238)
    );
}

#[test]
fn lookup_out_of_range_defaults_to_custom_or_default() {
    // Out of range values map to Custom or Default
    let c = lookup_256_color_by_index(300);
    // Should be a Custom or Default (depending on your policy)
    match c {
        TerminalColor::Custom(_, _, _) | TerminalColor::Default => {}
        _ => panic!("unexpected color variant for out-of-range index: {c:?}"),
    }
}

#[test]
fn lookup_color_cube_sample() {
    // Example index in color cube range
    let idx = 40;
    let result = lookup_256_color_by_index(idx);
    if let TerminalColor::Custom(r, g, b) = result {
        let expected_r = cube_component(idx, 36);
        let expected_g = cube_component(idx, 6);
        let expected_b = cube_component(idx, 1);

        assert_eq!(r as usize, expected_r);
        assert_eq!(g as usize, expected_g);
        assert_eq!(b as usize, expected_b);
    } else {
        panic!("Expected Custom color, got {result:?}");
    }
}

#[test]
fn cube_component_values_basic() {
    assert_eq!(cube_component(16, 36), 0);
    assert_eq!(cube_component(52, 36), ((14135 + 10280) / 256));
    assert_eq!(cube_component(88, 36), ((14135 + 10280 * 2) / 256));
}

#[test]
fn default_colors_to_regular() {
    assert_eq!(
        TerminalColor::Default.default_to_regular(),
        TerminalColor::White
    );
    assert_eq!(
        TerminalColor::DefaultUnderlineColor.default_to_regular(),
        TerminalColor::White
    );
    assert_eq!(
        TerminalColor::DefaultCursorColor.default_to_regular(),
        TerminalColor::White
    );
    assert_eq!(
        TerminalColor::DefaultBackground.default_to_regular(),
        TerminalColor::Black
    );
    assert_eq!(TerminalColor::Red.default_to_regular(), TerminalColor::Red);
}

#[test]
fn display_predefined_colors_full() {
    // Standard
    assert_eq!(TerminalColor::Default.to_string(), "default");
    assert_eq!(TerminalColor::Black.to_string(), "black");
    assert_eq!(TerminalColor::Red.to_string(), "red");
    assert_eq!(TerminalColor::Green.to_string(), "green");
    assert_eq!(TerminalColor::Yellow.to_string(), "yellow");
    assert_eq!(TerminalColor::Blue.to_string(), "blue");
    assert_eq!(TerminalColor::Magenta.to_string(), "magenta");
    assert_eq!(TerminalColor::Cyan.to_string(), "cyan");
    assert_eq!(TerminalColor::White.to_string(), "white");

    // Bright
    assert_eq!(TerminalColor::BrightYellow.to_string(), "bright yellow");
    assert_eq!(TerminalColor::BrightBlack.to_string(), "bright black");
    assert_eq!(TerminalColor::BrightRed.to_string(), "bright red");
    assert_eq!(TerminalColor::BrightGreen.to_string(), "bright green");
    assert_eq!(TerminalColor::BrightBlue.to_string(), "bright blue");
    assert_eq!(TerminalColor::BrightMagenta.to_string(), "bright magenta");
    assert_eq!(TerminalColor::BrightCyan.to_string(), "bright cyan");
    assert_eq!(TerminalColor::BrightWhite.to_string(), "bright white");

    // Defaults
    assert_eq!(
        TerminalColor::DefaultUnderlineColor.to_string(),
        "default underline color"
    );
    assert_eq!(
        TerminalColor::DefaultBackground.to_string(),
        "default background"
    );
    assert_eq!(
        TerminalColor::DefaultCursorColor.to_string(),
        "default cursor color"
    );

    // Custom
    assert_eq!(
        TerminalColor::Custom(12, 34, 56).to_string(),
        "rgb(12, 34, 56)"
    );
}

#[test]
fn parse_all_valid_and_invalid_colors() {
    // All valid mappings
    let pairs = [
        ("default", TerminalColor::Default),
        ("default_background", TerminalColor::DefaultBackground),
        (
            "default_underline_color",
            TerminalColor::DefaultUnderlineColor,
        ),
        ("default_cursor_color", TerminalColor::DefaultCursorColor),
        ("black", TerminalColor::Black),
        ("red", TerminalColor::Red),
        ("green", TerminalColor::Green),
        ("yellow", TerminalColor::Yellow),
        ("blue", TerminalColor::Blue),
        ("magenta", TerminalColor::Magenta),
        ("cyan", TerminalColor::Cyan),
        ("white", TerminalColor::White),
        ("bright yellow", TerminalColor::BrightYellow),
        ("bright black", TerminalColor::BrightBlack),
        ("bright red", TerminalColor::BrightRed),
        ("bright green", TerminalColor::BrightGreen),
        ("bright blue", TerminalColor::BrightBlue),
        ("bright magenta", TerminalColor::BrightMagenta),
        ("bright cyan", TerminalColor::BrightCyan),
        ("bright white", TerminalColor::BrightWhite),
    ];

    for (name, expected) in pairs {
        assert_eq!(TerminalColor::from_str(name).unwrap(), expected);
    }

    // Invalid input hits Err branch
    let err = TerminalColor::from_str("unknown_color").unwrap_err();
    assert!(err.to_string().contains("Invalid color string"));
}

#[test]
fn manual_display_write_covers_all_paths() {
    let mut buf = String::new();

    // Normal write_str path
    write!(&mut buf, "{}", TerminalColor::Yellow).unwrap();
    assert_eq!(buf, "yellow");
    buf.clear();

    // Return write!() path (Custom)
    write!(&mut buf, "{}", TerminalColor::Custom(200, 150, 100)).unwrap();
    assert_eq!(buf, "rgb(200, 150, 100)");
    buf.clear();

    // PaletteIndex display path
    write!(&mut buf, "{}", TerminalColor::PaletteIndex(42)).unwrap();
    assert_eq!(buf, "palette(42)");
    buf.clear();

    // Default variant again
    write!(&mut buf, "{}", TerminalColor::Default).unwrap();
    assert_eq!(buf, "default");
}

// ------------------------------------------------------------------
// ColorPalette tests
// ------------------------------------------------------------------

#[test]
fn palette_default_is_all_none_overrides() {
    let palette = ColorPalette::default();
    // Default palette should produce standard colors for all indices.
    assert_eq!(palette.lookup(0), TerminalColor::Black);
    assert_eq!(palette.lookup(1), TerminalColor::Red);
    assert_eq!(palette.lookup(15), TerminalColor::BrightWhite);
    // Color cube index
    let c = palette.lookup(196);
    assert!(matches!(c, TerminalColor::Custom(..)));
}

#[test]
fn palette_set_overrides_default() {
    let mut palette = ColorPalette::default();
    // Override index 0 (normally Black) with a custom color
    palette.set(0, 0x12, 0x34, 0x56);
    assert_eq!(palette.lookup(0), TerminalColor::Custom(0x12, 0x34, 0x56));
    // Other indices are unaffected
    assert_eq!(palette.lookup(1), TerminalColor::Red);
}

#[test]
fn palette_reset_restores_default() {
    let mut palette = ColorPalette::default();
    palette.set(5, 0xAA, 0xBB, 0xCC);
    assert_eq!(palette.lookup(5), TerminalColor::Custom(0xAA, 0xBB, 0xCC));
    palette.reset(5);
    assert_eq!(palette.lookup(5), TerminalColor::Magenta);
}

#[test]
fn palette_reset_all_clears_all_overrides() {
    let mut palette = ColorPalette::default();
    palette.set(0, 1, 2, 3);
    palette.set(100, 4, 5, 6);
    palette.set(255, 7, 8, 9);
    palette.reset_all();
    assert_eq!(palette.lookup(0), TerminalColor::Black);
    assert_eq!(palette.lookup(100), lookup_256_color_by_index(100));
    assert_eq!(palette.lookup(255), lookup_256_color_by_index(255));
}

#[test]
fn palette_get_rgb_returns_override_when_set() {
    let mut palette = ColorPalette::default();
    palette.set(42, 0xFF, 0x00, 0x80);
    assert_eq!(palette.get_rgb(42), (0xFF, 0x00, 0x80));
}

#[test]
fn palette_get_rgb_returns_default_when_not_set() {
    let palette = ColorPalette::default();
    // Index 0 should return the Catppuccin Mocha Black RGB
    assert_eq!(palette.get_rgb(0), (0x45, 0x47, 0x5a));
}

#[test]
fn palette_equality() {
    let p1 = ColorPalette::default();
    let p2 = ColorPalette::default();
    assert_eq!(p1, p2);

    let mut p3 = ColorPalette::default();
    p3.set(10, 1, 2, 3);
    assert_ne!(p1, p3);
}

// ------------------------------------------------------------------
// PaletteIndex variant tests
// ------------------------------------------------------------------

#[test]
fn palette_index_display() {
    assert_eq!(TerminalColor::PaletteIndex(0).to_string(), "palette(0)");
    assert_eq!(TerminalColor::PaletteIndex(255).to_string(), "palette(255)");
}

#[test]
fn palette_index_resolve_default() {
    // PaletteIndex(1) should resolve to Red (the default for index 1)
    assert_eq!(
        TerminalColor::PaletteIndex(1).resolve_palette_default(),
        TerminalColor::Red
    );
    // PaletteIndex for a color-cube index should resolve to Custom(...)
    let resolved = TerminalColor::PaletteIndex(196).resolve_palette_default();
    assert!(matches!(resolved, TerminalColor::Custom(..)));
}

#[test]
fn palette_index_resolve_non_palette_is_identity() {
    assert_eq!(
        TerminalColor::Red.resolve_palette_default(),
        TerminalColor::Red
    );
    assert_eq!(
        TerminalColor::Custom(1, 2, 3).resolve_palette_default(),
        TerminalColor::Custom(1, 2, 3)
    );
}

#[test]
fn palette_index_default_to_regular_identity() {
    // PaletteIndex is not a "default" variant, so default_to_regular returns self
    assert_eq!(
        TerminalColor::PaletteIndex(5).default_to_regular(),
        TerminalColor::PaletteIndex(5)
    );
}

// ------------------------------------------------------------------
// default_index_to_rgb tests
// ------------------------------------------------------------------

#[test]
fn default_index_to_rgb_named_colors() {
    // Catppuccin Mocha Black
    assert_eq!(default_index_to_rgb(0), (0x45, 0x47, 0x5a));
    // Catppuccin Mocha Red
    assert_eq!(default_index_to_rgb(1), (0xf3, 0x8b, 0xa8));
    // Catppuccin Mocha BrightWhite
    assert_eq!(default_index_to_rgb(15), (0xba, 0xc2, 0xde));
}

#[test]
fn default_index_to_rgb_greyscale_ramp() {
    // Index 232 is the start of the greyscale ramp
    let (r, g, b) = default_index_to_rgb(232);
    assert_eq!(r, g);
    assert_eq!(g, b);

    // Index 255 is the end of the greyscale ramp
    let (r2, g2, b2) = default_index_to_rgb(255);
    assert_eq!(r2, g2);
    assert_eq!(g2, b2);
    assert!(r2 > r); // brighter
}

#[test]
fn default_index_to_rgb_color_cube() {
    // Index 16 is rgb(0,0,0) in the color cube
    assert_eq!(default_index_to_rgb(16), (0, 0, 0));
    // Index 196 is a red in the color cube
    let (r, _g, _b) = default_index_to_rgb(196);
    assert!(r > 0);
}

//
// ---------- Property-Based Tests ----------
//

proptest! {
    #[test]
    fn grayscale_monotonic(index in 232usize..=254usize) {
        let c1 = lookup_256_color_by_index(index);
        let c2 = lookup_256_color_by_index(index + 1);

        match (c1, c2) {
            (TerminalColor::Custom(r1,g1,b1), TerminalColor::Custom(r2,_,_)) => {
                prop_assert_eq!(r1, g1);
                prop_assert_eq!(g1, b1);
                prop_assert!(r2 >= r1);
            }
            _ => prop_assert!(false, "Expected Custom colors"),
        }
    }

    #[test]
    fn cube_component_cycles(value in 16usize..=230usize, modifier in proptest::sample::select(vec![36usize, 6usize, 1usize])) {
        let c = cube_component(value, modifier);
        prop_assert!(c <= 255);

        let wrap = cube_component(value + modifier * 6, modifier);
        prop_assert_eq!(wrap, c);
    }

    #[test]
    fn custom_color_display_roundtrip(r in 0u8..=255, g in 0u8..=255, b in 0u8..=255) {
        let color = TerminalColor::Custom(r, g, b);
        let text = color.to_string();
        prop_assert!(text.starts_with("rgb(") && text.ends_with(")"));

        let parts: Vec<u8> = text.trim_start_matches("rgb(")
            .trim_end_matches(")")
            .split(',')
            .map(|p| p.trim().parse::<u8>().unwrap())
            .collect();

        prop_assert_eq!(parts, vec![r, g, b]);
    }
}
