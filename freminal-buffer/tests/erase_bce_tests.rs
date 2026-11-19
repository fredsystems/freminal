// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Task 140.2: erased cells take only the current background colour.
//!
//! Every erase operation (ED, EL, ECH, ICH, DCH and the DECLRMM column-confined
//! scroll fills) blanks cells with a tag that carries the stored background
//! colour and nothing else: no foreground, weight, decoration, reverse flag,
//! blink or OSC 8 hyperlink. SGR 7 does not swap the stored background.

use std::sync::Arc;

use freminal_buffer::buffer::Buffer;
use freminal_common::{
    buffer_states::{
        cursor::{ReverseVideo, StateColors},
        fonts::{BlinkState, FontDecorationFlags, FontDecorations, FontWeight, UnderlineStyle},
        format_tag::FormatTag,
        modes::declrmm::Declrmm,
        tchar::TChar,
        url::Url,
    },
    colors::TerminalColor,
};

const WIDTH: usize = 10;
const HEIGHT: usize = 4;

/// Foreground, bold, underline, reverse, blink, an OSC 8 link and a blue
/// background: everything an erase must *not* copy except the background.
fn loud_tag() -> FormatTag {
    let mut decorations = FontDecorationFlags::empty();
    decorations.insert(FontDecorations::Underline);
    decorations.set_underline_style(UnderlineStyle::Curly);
    FormatTag {
        colors: StateColors::default()
            .with_color(TerminalColor::Red)
            .with_background_color(TerminalColor::Blue)
            .with_reverse_video(ReverseVideo::On),
        font_weight: FontWeight::Bold,
        font_decorations: decorations,
        url: Some(Arc::new(Url {
            id: None,
            url: "https://example.com".to_owned(),
        })),
        blink: BlinkState::Slow,
        ..FormatTag::default()
    }
}

fn background_only(background: TerminalColor) -> FormatTag {
    FormatTag {
        colors: StateColors::default().with_background_color(background),
        ..FormatTag::default()
    }
}

/// A `WIDTH` x `HEIGHT` buffer filled with `X` in the default format, then
/// given `tag` as the active format. The cursor is left at (0, 0).
fn filled(tag: FormatTag) -> Buffer {
    let mut buf = Buffer::new(WIDTH, HEIGHT);
    let xs: Vec<TChar> = (0..WIDTH * HEIGHT).map(|_| TChar::Ascii(b'X')).collect();
    buf.insert_text(&xs);
    buf.set_format(tag);
    buf.set_cursor_pos(Some(0), Some(0));
    buf
}

fn assert_erased(buf: &Buffer, row: usize, cols: std::ops::Range<usize>) {
    let expected = background_only(TerminalColor::Blue);
    for col in cols {
        let cell = buf.rows()[row].resolve_cell(col);
        assert_eq!(
            cell.tag(),
            &expected,
            "row {row} col {col} must carry only the background"
        );
        assert_eq!(cell.tchar(), &TChar::Space, "row {row} col {col} is blank");
    }
}

fn assert_untouched(buf: &Buffer, row: usize, cols: std::ops::Range<usize>) {
    for col in cols {
        let cell = buf.rows()[row].resolve_cell(col);
        assert_eq!(
            cell.tchar(),
            &TChar::Ascii(b'X'),
            "row {row} col {col} keeps its content"
        );
        assert_eq!(
            cell.tag(),
            &FormatTag::default(),
            "row {row} col {col} keeps its format"
        );
    }
}

#[test]
fn ed0_erases_with_the_background_only() {
    let mut buf = filled(loud_tag());
    buf.set_cursor_pos(Some(3), Some(1));
    buf.erase_to_end_of_display();

    assert_untouched(&buf, 0, 0..WIDTH);
    assert_untouched(&buf, 1, 0..3);
    assert_erased(&buf, 1, 3..WIDTH);
    assert_erased(&buf, 2, 0..WIDTH);
    assert_erased(&buf, 3, 0..WIDTH);
}

#[test]
fn ed1_erases_with_the_background_only() {
    let mut buf = filled(loud_tag());
    buf.set_cursor_pos(Some(3), Some(2));
    buf.erase_to_beginning_of_display();

    assert_erased(&buf, 0, 0..WIDTH);
    assert_erased(&buf, 1, 0..WIDTH);
    assert_erased(&buf, 2, 0..4);
    assert_untouched(&buf, 2, 4..WIDTH);
    assert_untouched(&buf, 3, 0..WIDTH);
}

#[test]
fn ed2_erases_with_the_background_only() {
    let mut buf = filled(loud_tag());
    buf.erase_display();

    for row in 0..HEIGHT {
        assert_erased(&buf, row, 0..WIDTH);
    }
}

#[test]
fn el0_erases_with_the_background_only() {
    let mut buf = filled(loud_tag());
    buf.set_cursor_pos(Some(4), Some(1));
    buf.erase_line_to_end();

    assert_untouched(&buf, 1, 0..4);
    assert_erased(&buf, 1, 4..WIDTH);
    assert_untouched(&buf, 0, 0..WIDTH);
    assert_untouched(&buf, 2, 0..WIDTH);
}

#[test]
fn el1_erases_with_the_background_only() {
    let mut buf = filled(loud_tag());
    buf.set_cursor_pos(Some(4), Some(1));
    buf.erase_line_to_beginning();

    assert_erased(&buf, 1, 0..5);
    assert_untouched(&buf, 1, 5..WIDTH);
    assert_untouched(&buf, 0, 0..WIDTH);
    assert_untouched(&buf, 2, 0..WIDTH);
}

#[test]
fn el2_erases_with_the_background_only() {
    let mut buf = filled(loud_tag());
    buf.set_cursor_pos(Some(4), Some(1));
    buf.erase_line();

    assert_erased(&buf, 1, 0..WIDTH);
    assert_untouched(&buf, 0, 0..WIDTH);
    assert_untouched(&buf, 2, 0..WIDTH);
}

#[test]
fn ech_erases_with_the_background_only() {
    let mut buf = filled(loud_tag());
    buf.set_cursor_pos(Some(2), Some(1));
    buf.erase_chars(3);

    assert_untouched(&buf, 1, 0..2);
    assert_erased(&buf, 1, 2..5);
    assert_untouched(&buf, 1, 5..WIDTH);
}

#[test]
fn ich_blanks_with_the_background_only() {
    let mut buf = filled(loud_tag());
    buf.set_cursor_pos(Some(2), Some(1));
    buf.insert_spaces(3);

    assert_untouched(&buf, 1, 0..2);
    assert_erased(&buf, 1, 2..5);
    assert_untouched(&buf, 1, 5..WIDTH);
}

#[test]
fn ich_under_declrmm_blanks_with_the_background_only() {
    let mut buf = filled(loud_tag());
    buf.set_declrmm(Declrmm::Enabled);
    buf.set_left_right_margins(3, 7); // 0-based columns [2, 6]
    buf.set_cursor_pos(Some(2), Some(1));
    buf.insert_spaces(2);

    assert_untouched(&buf, 1, 0..2);
    assert_erased(&buf, 1, 2..4);
    assert_untouched(&buf, 1, 4..WIDTH);
}

// Test helper; not a #[test] fn, so clippy.toml's allow-*-in-tests does not cover it.
#[allow(clippy::unwrap_used)]
fn emoji(s: &str) -> TChar {
    TChar::new_from_many_chars(s.as_bytes()).unwrap()
}

#[test]
fn dch_blanks_a_split_wide_glyph_with_the_background_only() {
    // The cells vacated at the right edge take the erase tag, and so does the
    // remnant of a wide glyph whose continuation cell ends the deleted range.
    let mut buf = Buffer::new(WIDTH, HEIGHT);
    buf.insert_text(&[TChar::Ascii(b'A'), TChar::Ascii(b'B'), emoji("🙂")]);
    buf.insert_text(&[TChar::Ascii(b'C')]);
    buf.set_format(loud_tag());
    buf.set_cursor_pos(Some(1), Some(0));
    buf.delete_chars(2);

    assert_eq!(buf.rows()[0].resolve_cell(0).tchar(), &TChar::Ascii(b'A'));
    assert_erased(&buf, 0, 1..2);
    assert_eq!(buf.rows()[0].resolve_cell(2).tchar(), &TChar::Ascii(b'C'));
    assert_erased(&buf, 0, WIDTH - 2..WIDTH);
    assert_eq!(buf.rows()[0].cells().len(), WIDTH);
}

#[test]
fn dch_of_a_wide_head_fills_every_vacated_column() {
    // Deleting one column that holds a wide head removes both of its columns,
    // so two columns are vacated at the right edge.
    let mut buf = Buffer::new(WIDTH, HEIGHT);
    buf.insert_text(&[TChar::Ascii(b'A'), emoji("🙂"), TChar::Ascii(b'C')]);
    buf.set_format(loud_tag());
    buf.set_cursor_pos(Some(1), Some(0));
    buf.delete_chars(1);

    assert_eq!(buf.rows()[0].resolve_cell(0).tchar(), &TChar::Ascii(b'A'));
    assert_eq!(buf.rows()[0].resolve_cell(1).tchar(), &TChar::Ascii(b'C'));
    for col in 2..WIDTH - 2 {
        assert_eq!(
            buf.rows()[0].resolve_cell(col).tag(),
            &FormatTag::default(),
            "col {col} is outside the vacated columns"
        );
    }
    assert_erased(&buf, 0, WIDTH - 2..WIDTH);
}

#[test]
fn dch_fills_the_vacated_right_edge_with_the_background_only() {
    let mut buf = filled(loud_tag());
    buf.set_cursor_pos(Some(2), Some(1));
    buf.delete_chars(3);

    assert_untouched(&buf, 1, 0..2);
    assert_untouched(&buf, 1, 2..WIDTH - 3);
    assert_erased(&buf, 1, WIDTH - 3..WIDTH);
    assert_untouched(&buf, 0, 0..WIDTH);
    assert_untouched(&buf, 2, 0..WIDTH);
}

#[test]
fn dch_larger_than_the_remaining_columns_fills_to_the_right_edge() {
    let mut buf = filled(loud_tag());
    buf.set_cursor_pos(Some(7), Some(1));
    buf.delete_chars(100);

    assert_untouched(&buf, 1, 0..7);
    assert_erased(&buf, 1, 7..WIDTH);
    assert_eq!(buf.rows()[1].cells().len(), WIDTH);
}

#[test]
fn dch_past_the_stored_content_still_fills_the_right_edge() {
    let mut buf = Buffer::new(WIDTH, HEIGHT);
    buf.insert_text(&[TChar::Ascii(b'A'), TChar::Ascii(b'B')]);
    buf.set_format(loud_tag());
    buf.set_cursor_pos(Some(5), Some(0));
    buf.delete_chars(3);

    assert_eq!(buf.rows()[0].resolve_cell(0).tchar(), &TChar::Ascii(b'A'));
    assert_eq!(buf.rows()[0].resolve_cell(1).tchar(), &TChar::Ascii(b'B'));
    assert_erased(&buf, 0, WIDTH - 3..WIDTH);
}

#[test]
fn dch_with_a_default_background_keeps_the_row_sparse() {
    let reverse_only = FormatTag {
        colors: StateColors::default()
            .with_color(TerminalColor::Red)
            .with_reverse_video(ReverseVideo::On),
        ..FormatTag::default()
    };
    let mut buf = filled(reverse_only);
    buf.set_cursor_pos(Some(2), Some(1));
    buf.delete_chars(3);

    assert_eq!(
        buf.rows()[1].cells().len(),
        WIDTH - 3,
        "no explicit cells at the right edge"
    );

    let mut buf = Buffer::new(WIDTH, HEIGHT);
    buf.insert_text(&[TChar::Ascii(b'A'), TChar::Ascii(b'B')]);
    buf.set_cursor_pos(Some(5), Some(0));
    buf.delete_chars(3);
    assert_eq!(buf.rows()[0].cells().len(), 2);
}

#[test]
fn dch_under_declrmm_fills_the_gap_with_the_background_only() {
    let mut buf = filled(loud_tag());
    buf.set_declrmm(Declrmm::Enabled);
    buf.set_left_right_margins(3, 7); // 0-based columns [2, 6]
    buf.set_cursor_pos(Some(2), Some(1));
    buf.delete_chars(2);

    assert_untouched(&buf, 1, 0..5);
    assert_erased(&buf, 1, 5..7);
    assert_untouched(&buf, 1, 7..WIDTH);
}

#[test]
fn declrmm_delete_lines_fills_the_columns_with_the_background_only() {
    let mut buf = filled(loud_tag());
    buf.set_declrmm(Declrmm::Enabled);
    buf.set_left_right_margins(3, 7); // 0-based columns [2, 6]
    buf.set_cursor_pos(Some(2), Some(1));
    buf.delete_lines(1);

    assert_untouched(&buf, 3, 0..2);
    assert_erased(&buf, 3, 2..7);
    assert_untouched(&buf, 3, 7..WIDTH);
}

#[test]
fn declrmm_insert_lines_fills_the_columns_with_the_background_only() {
    let mut buf = filled(loud_tag());
    buf.set_declrmm(Declrmm::Enabled);
    buf.set_left_right_margins(3, 7); // 0-based columns [2, 6]
    buf.set_cursor_pos(Some(2), Some(1));
    buf.insert_lines(1);

    assert_untouched(&buf, 1, 0..2);
    assert_erased(&buf, 1, 2..7);
    assert_untouched(&buf, 1, 7..WIDTH);
}

#[test]
fn whole_row_il_and_dl_keep_default_blank_rows() {
    let mut buf = filled(loud_tag());
    buf.set_cursor_pos(Some(0), Some(1));
    buf.insert_lines(1);
    assert!(
        buf.rows()[1].cells().is_empty(),
        "IL's new row is default and sparse (decision 4)"
    );

    buf.delete_lines(1);
    assert!(
        buf.rows()[HEIGHT - 1].cells().is_empty(),
        "DL's new bottom row is default and sparse (decision 4)"
    );
}

#[test]
fn reverse_video_with_a_default_background_erases_to_default_sparse_cells() {
    let reverse_only = FormatTag {
        colors: StateColors::default()
            .with_color(TerminalColor::Red)
            .with_reverse_video(ReverseVideo::On),
        ..FormatTag::default()
    };
    let mut buf = filled(reverse_only);

    buf.erase_display();
    for (index, row) in buf.rows().iter().enumerate() {
        assert!(row.cells().is_empty(), "row {index} stays sparse");
    }

    let mut buf = filled(FormatTag {
        colors: StateColors::default().with_reverse_video(ReverseVideo::On),
        ..FormatTag::default()
    });
    buf.set_cursor_pos(Some(2), Some(1));
    buf.erase_line_to_end();
    for col in 2..WIDTH {
        assert_eq!(
            buf.rows()[1].resolve_cell(col).tag(),
            &FormatTag::default(),
            "col {col} is default"
        );
    }
}

#[test]
fn reverse_video_does_not_swap_the_stored_background() {
    let mut buf = filled(loud_tag());
    buf.erase_line();

    for col in 0..WIDTH {
        let colors = buf.rows()[0].resolve_cell(col).tag().colors;
        assert_eq!(colors.background_color, TerminalColor::Blue);
        assert_eq!(colors.color, TerminalColor::Default);
        assert_eq!(colors.reverse_video, ReverseVideo::Off);
    }
}

#[test]
fn a_default_format_keeps_erased_rows_sparse() {
    let mut buf = filled(FormatTag::default());
    buf.erase_display();
    for (index, row) in buf.rows().iter().enumerate() {
        assert!(row.cells().is_empty(), "row {index} stays sparse");
    }
}

fn assert_default_blank(buf: &Buffer, row: usize, cols: std::ops::Range<usize>) {
    for col in cols {
        let cell = buf.rows()[row].resolve_cell(col);
        assert_eq!(
            cell.tag(),
            &FormatTag::default(),
            "row {row} col {col} stays a default blank"
        );
        assert_eq!(cell.tchar(), &TChar::Space, "row {row} col {col} is blank");
    }
}

fn ascii(s: &[u8]) -> Vec<TChar> {
    s.iter().map(|&b| TChar::Ascii(b)).collect()
}

#[test]
fn dch_on_a_short_row_fills_the_right_edge_not_the_content_edge() {
    let mut buf = Buffer::new(WIDTH, HEIGHT);
    buf.insert_text(&ascii(b"ABCDE"));
    buf.set_format(loud_tag());
    buf.set_cursor_pos(Some(1), Some(0));
    buf.delete_chars(2);

    for (col, ch) in [(0, b'A'), (1, b'D'), (2, b'E')] {
        assert_eq!(buf.rows()[0].resolve_cell(col).tchar(), &TChar::Ascii(ch));
        assert_eq!(buf.rows()[0].resolve_cell(col).tag(), &FormatTag::default());
    }
    assert_default_blank(&buf, 0, 3..WIDTH - 2);
    assert_erased(&buf, 0, WIDTH - 2..WIDTH);
}

#[test]
fn dch_at_the_last_column_erases_just_that_column() {
    let mut buf = filled(loud_tag());
    buf.set_cursor_pos(Some(WIDTH - 1), Some(1));
    buf.delete_chars(1);

    assert_untouched(&buf, 1, 0..WIDTH - 1);
    assert_erased(&buf, 1, WIDTH - 1..WIDTH);
}

#[test]
fn dch_of_zero_with_a_loud_tag_changes_nothing() {
    let mut buf = filled(loud_tag());
    buf.set_cursor_pos(Some(2), Some(1));
    buf.delete_chars(0);

    for row in 0..HEIGHT {
        assert_untouched(&buf, row, 0..WIDTH);
    }
}

#[test]
fn dch_with_a_huge_count_does_not_overflow() {
    let mut buf = filled(loud_tag());
    buf.set_cursor_pos(Some(2), Some(1));
    buf.delete_chars(usize::MAX);

    assert_untouched(&buf, 1, 0..2);
    assert_erased(&buf, 1, 2..WIDTH);
    assert_eq!(buf.rows()[1].cells().len(), WIDTH);
    assert_untouched(&buf, 0, 0..WIDTH);
}

#[test]
fn dch_with_a_huge_count_and_a_default_tag_truncates_the_row() {
    let mut buf = filled(FormatTag::default());
    buf.set_cursor_pos(Some(2), Some(1));
    buf.delete_chars(usize::MAX);

    assert_untouched(&buf, 1, 0..2);
    assert_default_blank(&buf, 1, 2..WIDTH);
    assert_eq!(buf.rows()[1].cells().len(), 2);
}

#[test]
fn dch_with_a_huge_count_under_declrmm_does_not_overflow() {
    let mut buf = filled(loud_tag());
    buf.set_declrmm(Declrmm::Enabled);
    buf.set_left_right_margins(3, 7); // 0-based columns [2, 6]
    buf.set_cursor_pos(Some(2), Some(1));
    buf.delete_chars(usize::MAX);

    assert_untouched(&buf, 1, 0..2);
    assert_erased(&buf, 1, 2..7);
    assert_untouched(&buf, 1, 7..WIDTH);
}

#[test]
fn ech_with_a_huge_count_does_not_overflow() {
    let mut buf = filled(loud_tag());
    buf.set_cursor_pos(Some(2), Some(1));
    buf.erase_chars(usize::MAX);

    assert_untouched(&buf, 1, 0..2);
    assert_erased(&buf, 1, 2..WIDTH);
}

#[test]
fn ich_with_a_huge_count_does_not_overflow() {
    let mut buf = filled(loud_tag());
    buf.set_cursor_pos(Some(2), Some(1));
    buf.insert_spaces(usize::MAX);

    assert_untouched(&buf, 1, 0..2);
    assert_erased(&buf, 1, 2..WIDTH);
}

/// Stamp Kitty image cells across the whole of row 1 so the image-accounting
/// paths in DCH and ECH run with a huge count.
fn with_kitty_images_on_row_1(tag: FormatTag) -> Buffer {
    use freminal_buffer::image_store::ImageProtocol;
    let mut buf = filled(tag);
    buf.place_image_at(
        7,
        1,
        0,
        WIDTH,
        1,
        ImageProtocol::Kitty,
        None,
        None,
        0,
        None,
        1,
        None,
    );
    buf
}

#[test]
fn dch_with_a_huge_count_and_image_cells_does_not_overflow() {
    let mut buf = with_kitty_images_on_row_1(loud_tag());
    buf.set_cursor_pos(Some(3), Some(1));
    buf.delete_chars(usize::MAX);
    assert_erased(&buf, 1, 3..WIDTH);

    let mut buf = with_kitty_images_on_row_1(loud_tag());
    buf.set_declrmm(Declrmm::Enabled);
    buf.set_left_right_margins(3, 7);
    buf.set_cursor_pos(Some(3), Some(1));
    buf.delete_chars(usize::MAX);
    assert_erased(&buf, 1, 3..7);
}

#[test]
fn ech_with_a_huge_count_and_image_cells_does_not_overflow() {
    let mut buf = with_kitty_images_on_row_1(loud_tag());
    buf.set_cursor_pos(Some(3), Some(1));
    buf.erase_chars(usize::MAX);
    assert_erased(&buf, 1, 3..WIDTH);
}
