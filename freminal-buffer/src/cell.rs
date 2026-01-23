// Copyright (C) 2024-2025 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use freminal_common::buffer_states::{format_tag::FormatTag, tchar::TChar};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    value: TChar,
    format: FormatTag,
    is_wide_head: bool,
    is_wide_continuation: bool,
}

impl Cell {
    #[must_use]
    pub fn new(value: TChar, format: FormatTag) -> Self {
        let width = value.display_width();

        Self {
            value,
            format,
            is_wide_head: width > 1,
            is_wide_continuation: false,
        }
    }

    #[must_use]
    pub const fn blank_with_tag(format: FormatTag) -> Self {
        Self {
            value: TChar::Space,
            format,
            is_wide_head: false,
            is_wide_continuation: false,
        }
    }

    #[must_use]
    pub fn wide_continuation() -> Self {
        Self {
            value: TChar::Space, // filler glyph
            format: FormatTag::default(),
            is_wide_continuation: true,
            is_wide_head: false,
        }
    }

    #[must_use]
    pub const fn is_head(&self) -> bool {
        self.is_wide_head
    }

    #[must_use]
    pub const fn tchar(&self) -> &TChar {
        &self.value
    }

    #[must_use]
    pub const fn tag(&self) -> &FormatTag {
        &self.format
    }

    #[must_use]
    pub fn display_width(&self) -> usize {
        self.value.display_width()
    }

    #[must_use]
    pub fn into_utf8(&self) -> String {
        match self.value {
            TChar::Ascii(c) => (c as char).to_string(),
            TChar::Utf8(ref bytes) => String::from_utf8_lossy(bytes).to_string(),
            TChar::Space => " ".to_string(),
            TChar::NewLine => "\n".to_string(),
        }
    }

    #[must_use]
    pub const fn is_continuation(&self) -> bool {
        self.is_wide_continuation
    }
}

#[cfg(test)]
mod cell_tests {
    use super::*;
    use freminal_common::buffer_states::tchar::TChar;

    #[test]
    fn test_cell_creation() {
        let cell = Cell::new(TChar::Ascii(b'A'), FormatTag::default());
        assert_eq!(cell.tchar(), &TChar::Ascii(b'A'));
        assert!(!cell.is_head());
        assert!(!cell.is_continuation());

        let wide_char = TChar::Utf8("あ".as_bytes().to_vec());
        let wide_cell = Cell::new(wide_char.clone(), FormatTag::default());
        assert_eq!(wide_cell.tchar(), &wide_char);
        assert!(wide_cell.is_head());
        assert!(!wide_cell.is_continuation());

        let continuation_cell = Cell::wide_continuation();
        assert!(continuation_cell.is_continuation());
        assert!(!continuation_cell.is_head());
    }

    #[test]
    fn test_cell_display_width() {
        let ascii_cell = Cell::new(TChar::Ascii(b'A'), FormatTag::default());
        assert_eq!(ascii_cell.display_width(), 1);

        let wide_cell = Cell::new(TChar::Utf8("あ".as_bytes().to_vec()), FormatTag::default());
        assert_eq!(wide_cell.display_width(), 2);

        let space_cell = Cell::new(TChar::Space, FormatTag::default());
        assert_eq!(space_cell.display_width(), 1);

        let newline_cell = Cell::new(TChar::NewLine, FormatTag::default());
        assert_eq!(newline_cell.display_width(), 0);
    }

    #[test]
    fn test_cell_into_utf8() {
        let ascii_cell = Cell::new(TChar::Ascii(b'A'), FormatTag::default());
        assert_eq!(ascii_cell.into_utf8(), "A");

        let wide_cell = Cell::new(TChar::Utf8("あ".as_bytes().to_vec()), FormatTag::default());
        assert_eq!(wide_cell.into_utf8(), "あ");

        let space_cell = Cell::new(TChar::Space, FormatTag::default());
        assert_eq!(space_cell.into_utf8(), " ");

        let newline_cell = Cell::new(TChar::NewLine, FormatTag::default());
        assert_eq!(newline_cell.into_utf8(), "\n");
    }
}
