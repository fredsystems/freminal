// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Kitty Unicode placeholder support.
//!
//! The Kitty graphics protocol allows encoding image cell references using
//! the Unicode Private Use character U+10EEEE followed by combining
//! diacritics that encode the row, column, and optional MSB of the image ID.
//!
//! The image ID is encoded in the foreground color of the placeholder cell:
//! - 256-color mode: 8-bit image ID
//! - True color (RGB): 24-bit image ID (R << 16 | G << 8 | B)
//!
//! The placement ID is encoded in the underline color (same scheme).
//!
//! Reference: <https://sw.kovidgoyal.net/kitty/graphics-protocol/#unicode-placeholders>

use crate::colors::TerminalColor;

/// UTF-8 encoding of U+10EEEE: F4 8E BB AE (4 bytes).
pub(crate) const PLACEHOLDER_UTF8: [u8; 4] = [0xF4, 0x8E, 0xBB, 0xAE];

/// Diacritics used to encode row/column indices in Kitty Unicode placeholders.
///
/// Derived from Unicode 6.0 combining marks of class 230 (above), excluding
/// characters with decomposition mappings or that may fuse during
/// normalization. The index in this array is the row/column value.
///
/// Source: <https://github.com/nicm/tmux/blob/master/graphics.c> /
/// Kitty `gen/rowcolumn-diacritics.txt`.
const DIACRITICS: [u32; 297] = [
    0x0305, 0x030D, 0x030E, 0x0310, 0x0312, 0x033D, 0x033E, 0x033F, 0x0346, 0x034A, 0x034B, 0x034C,
    0x0350, 0x0351, 0x0352, 0x0357, 0x035B, 0x0363, 0x0364, 0x0365, 0x0366, 0x0367, 0x0368, 0x0369,
    0x036A, 0x036B, 0x036C, 0x036D, 0x036E, 0x036F, 0x0483, 0x0484, 0x0485, 0x0486, 0x0487, 0x0592,
    0x0593, 0x0594, 0x0595, 0x0597, 0x0598, 0x0599, 0x059C, 0x059D, 0x059E, 0x059F, 0x05A0, 0x05A1,
    0x05A8, 0x05A9, 0x05AB, 0x05AC, 0x05AF, 0x05C4, 0x0610, 0x0611, 0x0612, 0x0613, 0x0614, 0x0615,
    0x0616, 0x0617, 0x0657, 0x0658, 0x0659, 0x065A, 0x065B, 0x065D, 0x065E, 0x06D6, 0x06D7, 0x06D8,
    0x06D9, 0x06DA, 0x06DB, 0x06DC, 0x06DF, 0x06E0, 0x06E1, 0x06E2, 0x06E4, 0x06E7, 0x06E8, 0x06EB,
    0x06EC, 0x0730, 0x0732, 0x0733, 0x0735, 0x0736, 0x073A, 0x073D, 0x073F, 0x0740, 0x0741, 0x0743,
    0x0745, 0x0747, 0x0749, 0x074A, 0x07EB, 0x07EC, 0x07ED, 0x07EE, 0x07EF, 0x07F0, 0x07F1, 0x07F3,
    0x0816, 0x0817, 0x0818, 0x0819, 0x081B, 0x081C, 0x081D, 0x081E, 0x081F, 0x0820, 0x0821, 0x0822,
    0x0823, 0x0825, 0x0826, 0x0827, 0x0829, 0x082A, 0x082B, 0x082C, 0x082D, 0x0951, 0x0953, 0x0954,
    0x0F82, 0x0F83, 0x0F86, 0x0F87, 0x135D, 0x135E, 0x135F, 0x17DD, 0x193A, 0x1A17, 0x1A75, 0x1A76,
    0x1A77, 0x1A78, 0x1A79, 0x1A7A, 0x1A7B, 0x1A7C, 0x1B6B, 0x1B6D, 0x1B6E, 0x1B6F, 0x1B70, 0x1B71,
    0x1B72, 0x1B73, 0x1CD0, 0x1CD1, 0x1CD2, 0x1CDA, 0x1CDB, 0x1CE0, 0x1DC0, 0x1DC1, 0x1DC3, 0x1DC4,
    0x1DC5, 0x1DC6, 0x1DC7, 0x1DC8, 0x1DC9, 0x1DCB, 0x1DCC, 0x1DD1, 0x1DD2, 0x1DD3, 0x1DD4, 0x1DD5,
    0x1DD6, 0x1DD7, 0x1DD8, 0x1DD9, 0x1DDA, 0x1DDB, 0x1DDC, 0x1DDD, 0x1DDE, 0x1DDF, 0x1DE0, 0x1DE1,
    0x1DE2, 0x1DE3, 0x1DE4, 0x1DE5, 0x1DE6, 0x1DFE, 0x20D0, 0x20D1, 0x20D4, 0x20D5, 0x20D6, 0x20D7,
    0x20DB, 0x20DC, 0x20E1, 0x20E7, 0x20E9, 0x20F0, 0x2CEF, 0x2CF0, 0x2CF1, 0x2DE0, 0x2DE1, 0x2DE2,
    0x2DE3, 0x2DE4, 0x2DE5, 0x2DE6, 0x2DE7, 0x2DE8, 0x2DE9, 0x2DEA, 0x2DEB, 0x2DEC, 0x2DED, 0x2DEE,
    0x2DEF, 0x2DF0, 0x2DF1, 0x2DF2, 0x2DF3, 0x2DF4, 0x2DF5, 0x2DF6, 0x2DF7, 0x2DF8, 0x2DF9, 0x2DFA,
    0x2DFB, 0x2DFC, 0x2DFD, 0x2DFE, 0x2DFF, 0xA66F, 0xA67C, 0xA67D, 0xA6F0, 0xA6F1, 0xA8E0, 0xA8E1,
    0xA8E2, 0xA8E3, 0xA8E4, 0xA8E5, 0xA8E6, 0xA8E7, 0xA8E8, 0xA8E9, 0xA8EA, 0xA8EB, 0xA8EC, 0xA8ED,
    0xA8EE, 0xA8EF, 0xA8F0, 0xA8F1, 0xAAB0, 0xAAB2, 0xAAB3, 0xAAB7, 0xAAB8, 0xAABE, 0xAABF, 0xAAC1,
    0xFE20, 0xFE21, 0xFE22, 0xFE23, 0xFE24, 0xFE25, 0xFE26, 0x10A0F, 0x10A38, 0x1D185, 0x1D186,
    0x1D187, 0x1D188, 0x1D189, 0x1D1AA, 0x1D1AB, 0x1D1AC, 0x1D1AD, 0x1D242, 0x1D243, 0x1D244,
];

/// Map a combining diacritic codepoint to its row/column index (0-based).
///
/// Returns `None` if the codepoint is not in the Kitty placeholder
/// diacritics table.
#[must_use]
pub(crate) fn diacritic_to_index(codepoint: u32) -> Option<u16> {
    // Binary search — the table is sorted by codepoint value.
    DIACRITICS.binary_search(&codepoint).ok().map(|i| {
        // The table has at most 297 entries, which fits in u16.
        #[allow(clippy::cast_possible_truncation)]
        let idx = i as u16;
        idx
    })
}

/// Parsed data from a Kitty Unicode placeholder grapheme.
///
/// Extracted from a single grapheme cluster beginning with U+10EEEE followed
/// by 0–3 combining diacritics encoding row, column, and optional image ID
/// MSB.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlaceholderDiacritics {
    /// Row index within the image (from the 1st diacritic, or 0 if absent).
    pub row: u16,
    /// Column index within the image (from the 2nd diacritic, or 0 if absent).
    pub col: u16,
    /// Most-significant byte of the image ID (from the 3rd diacritic, or 0).
    pub id_msb: u16,
    /// How many diacritics were present (0, 1, 2, or 3).
    pub diacritic_count: u8,
}

/// Parse the combining diacritics that follow U+10EEEE in a grapheme cluster.
///
/// `bytes` is the full UTF-8 byte sequence of the grapheme cluster (including
/// the leading U+10EEEE bytes). Returns `None` if the bytes don't start with
/// the placeholder character.
#[must_use]
pub fn parse_placeholder_diacritics(bytes: &[u8]) -> Option<PlaceholderDiacritics> {
    // Must start with U+10EEEE (4 bytes: F4 8E BB AE).
    if bytes.len() < 4 || bytes[..4] != PLACEHOLDER_UTF8 {
        return None;
    }

    let remaining = &bytes[4..];
    // The remaining bytes are combining diacritics encoded as UTF-8 chars.
    let s = std::str::from_utf8(remaining).ok()?;

    let mut row: u16 = 0;
    let mut col: u16 = 0;
    let mut id_msb: u16 = 0;
    let mut count: u8 = 0;

    for ch in s.chars() {
        let idx = diacritic_to_index(u32::from(ch))?;
        match count {
            0 => row = idx,
            1 => col = idx,
            2 => id_msb = idx,
            _ => break, // Ignore any further diacritics.
        }
        count += 1;
    }

    Some(PlaceholderDiacritics {
        row,
        col,
        id_msb,
        diacritic_count: count,
    })
}

/// A virtual placement created by a Kitty `a=p,U=1` or `a=T,U=1` command.
///
/// Virtual placements are not rendered directly in the buffer; instead, the
/// terminal waits for U+10EEEE placeholder characters to appear in the text
/// stream and uses the stored virtual placement dimensions to map each cell
/// to the correct image tile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VirtualPlacement {
    /// The Kitty image ID this placement refers to.
    pub image_id: u64,
    /// The placement ID (0 = any virtual placement for this image).
    pub placement_id: u32,
    /// Display width in terminal columns.
    pub cols: u32,
    /// Display height in terminal rows.
    pub rows: u32,
}

/// Extract a 24-bit image ID from a `TerminalColor` foreground value.
///
/// - `Custom(r, g, b)` → `(r << 16) | (g << 8) | b`
/// - `PaletteIndex(n)` → `n` (8-bit ID)
/// - Named colors / defaults → 0 (no image)
#[must_use]
pub fn color_to_image_id(color: &TerminalColor) -> u32 {
    match color {
        TerminalColor::Custom(r, g, b) => {
            (u32::from(*r) << 16) | (u32::from(*g) << 8) | u32::from(*b)
        }
        TerminalColor::PaletteIndex(n) => u32::from(*n),
        _ => 0,
    }
}

/// Extract a placement ID from a `TerminalColor` underline color value.
///
/// Same encoding as `color_to_image_id` but for the underline color field.
#[must_use]
pub fn color_to_placement_id(color: &TerminalColor) -> u32 {
    match color {
        TerminalColor::Custom(r, g, b) => {
            (u32::from(*r) << 16) | (u32::from(*g) << 8) | u32::from(*b)
        }
        TerminalColor::PaletteIndex(n) => u32::from(*n),
        // DefaultUnderlineColor means placement_id = 0 (any virtual placement).
        _ => 0,
    }
}

/// Returns `true` if the given UTF-8 byte slice starts with the Kitty
/// placeholder character U+10EEEE.
#[must_use]
pub fn is_placeholder(bytes: &[u8]) -> bool {
    bytes.len() >= 4 && bytes[..4] == PLACEHOLDER_UTF8
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    /// The Unicode codepoint used as the Kitty image placeholder character.
    const PLACEHOLDER_CHAR: char = '\u{10EEEE}';

    #[test]
    fn test_diacritic_to_index_known_values() {
        // From the Kitty spec examples:
        assert_eq!(diacritic_to_index(0x0305), Some(0));
        assert_eq!(diacritic_to_index(0x030D), Some(1));
        assert_eq!(diacritic_to_index(0x030E), Some(2));
    }

    #[test]
    fn test_diacritic_to_index_last_entry() {
        assert_eq!(diacritic_to_index(0x1D244), Some(296));
    }

    #[test]
    fn test_diacritic_to_index_unknown() {
        assert_eq!(diacritic_to_index(0x0041), None); // 'A' is not a diacritic
        assert_eq!(diacritic_to_index(0x0300), None); // Excluded combining grave
        assert_eq!(diacritic_to_index(0xFFFF), None);
    }

    #[test]
    fn test_placeholder_utf8_encoding() {
        let mut buf = [0u8; 4];
        let s = PLACEHOLDER_CHAR.encode_utf8(&mut buf);
        assert_eq!(s.as_bytes(), &PLACEHOLDER_UTF8);
    }

    #[test]
    fn test_parse_placeholder_no_diacritics() {
        let result = parse_placeholder_diacritics(&PLACEHOLDER_UTF8);
        assert_eq!(
            result,
            Some(PlaceholderDiacritics {
                row: 0,
                col: 0,
                id_msb: 0,
                diacritic_count: 0,
            })
        );
    }

    #[test]
    fn test_parse_placeholder_one_diacritic() {
        // U+10EEEE followed by U+030E (index 2 = row 2)
        let mut bytes = PLACEHOLDER_UTF8.to_vec();
        bytes.extend_from_slice("\u{030E}".as_bytes());
        let result = parse_placeholder_diacritics(&bytes).unwrap();
        assert_eq!(result.row, 2);
        assert_eq!(result.col, 0);
        assert_eq!(result.id_msb, 0);
        assert_eq!(result.diacritic_count, 1);
    }

    #[test]
    fn test_parse_placeholder_two_diacritics() {
        // row=1 (U+030D), col=2 (U+030E)
        let mut bytes = PLACEHOLDER_UTF8.to_vec();
        bytes.extend_from_slice("\u{030D}\u{030E}".as_bytes());
        let result = parse_placeholder_diacritics(&bytes).unwrap();
        assert_eq!(result.row, 1);
        assert_eq!(result.col, 2);
        assert_eq!(result.id_msb, 0);
        assert_eq!(result.diacritic_count, 2);
    }

    #[test]
    fn test_parse_placeholder_three_diacritics() {
        // row=0 (U+0305), col=1 (U+030D), msb=2 (U+030E)
        let mut bytes = PLACEHOLDER_UTF8.to_vec();
        bytes.extend_from_slice("\u{0305}\u{030D}\u{030E}".as_bytes());
        let result = parse_placeholder_diacritics(&bytes).unwrap();
        assert_eq!(result.row, 0);
        assert_eq!(result.col, 1);
        assert_eq!(result.id_msb, 2);
        assert_eq!(result.diacritic_count, 3);
    }

    #[test]
    fn test_parse_placeholder_not_placeholder() {
        assert_eq!(parse_placeholder_diacritics(b"hello"), None);
        assert_eq!(parse_placeholder_diacritics(&[0xF4, 0x8E, 0xBB]), None);
    }

    #[test]
    fn test_is_placeholder() {
        assert!(is_placeholder(&PLACEHOLDER_UTF8));
        let mut with_diacritics = PLACEHOLDER_UTF8.to_vec();
        with_diacritics.extend_from_slice("\u{0305}".as_bytes());
        assert!(is_placeholder(&with_diacritics));
        assert!(!is_placeholder(b"hello"));
        assert!(!is_placeholder(&[0xF4, 0x8E]));
    }

    #[test]
    fn test_color_to_image_id_custom() {
        assert_eq!(
            color_to_image_id(&TerminalColor::Custom(0x12, 0x34, 0x56)),
            0x0012_3456
        );
    }

    #[test]
    fn test_color_to_image_id_palette() {
        assert_eq!(color_to_image_id(&TerminalColor::PaletteIndex(42)), 42);
    }

    #[test]
    fn test_color_to_image_id_default() {
        assert_eq!(color_to_image_id(&TerminalColor::Default), 0);
    }

    #[test]
    fn test_color_to_placement_id() {
        assert_eq!(color_to_placement_id(&TerminalColor::Custom(0, 0, 5)), 5);
        assert_eq!(
            color_to_placement_id(&TerminalColor::DefaultUnderlineColor),
            0
        );
    }

    #[test]
    fn test_diacritics_table_is_sorted() {
        for pair in DIACRITICS.windows(2) {
            assert!(
                pair[0] < pair[1],
                "Diacritics table not sorted: {:#06X} >= {:#06X}",
                pair[0],
                pair[1]
            );
        }
    }

    #[test]
    fn test_diacritics_table_length() {
        assert_eq!(DIACRITICS.len(), 297);
    }
}
