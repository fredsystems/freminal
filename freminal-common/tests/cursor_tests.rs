// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

#![allow(clippy::unwrap_used)]

use freminal_common::buffer_states::cursor::{CursorPos, CursorState, ReverseVideo, StateColors};
use freminal_common::buffer_states::fonts::{FontDecorations, FontWeight};
use freminal_common::buffer_states::line_wrap::LineWrap;
use freminal_common::colors::TerminalColor;

// ---------------------------------------------------------------------------
// ReverseVideo
// ---------------------------------------------------------------------------

#[test]
fn test_reverse_video_default_is_off() {
    assert_eq!(ReverseVideo::default(), ReverseVideo::Off);
}

// ---------------------------------------------------------------------------
// StateColors — construction and field defaults
// ---------------------------------------------------------------------------

#[test]
fn test_default_state_colors() {
    let sc = StateColors::default();
    assert_eq!(sc.color, TerminalColor::Default);
    assert_eq!(sc.background_color, TerminalColor::DefaultBackground);
    assert_eq!(sc.underline_color, TerminalColor::DefaultUnderlineColor);
    assert_eq!(sc.reverse_video, ReverseVideo::Off);
}

#[test]
fn test_new_equals_default() {
    assert_eq!(StateColors::new(), StateColors::default());
}

// ---------------------------------------------------------------------------
// StateColors::get_color
// ---------------------------------------------------------------------------

#[test]
fn test_get_color_reverse_off() {
    // ReverseVideo::Off → returns the raw `color` field.
    let sc = StateColors::default(); // color = Default, reverse = Off
    assert_eq!(sc.get_color(), TerminalColor::Default);
}

#[test]
fn test_get_color_reverse_on_default() {
    // ReverseVideo::On → returns background_color.default_to_regular().
    // DefaultBackground.default_to_regular() == Black.
    let sc = StateColors::default().with_reverse_video(ReverseVideo::On);
    assert_eq!(sc.get_color(), TerminalColor::Black);
}

#[test]
fn test_get_color_reverse_on_custom_colors() {
    // Custom background_color passes through default_to_regular unchanged.
    let sc = StateColors::default()
        .with_color(TerminalColor::Red)
        .with_background_color(TerminalColor::Blue)
        .with_reverse_video(ReverseVideo::On);
    // get_color() returns background_color.default_to_regular() = Blue (unchanged)
    assert_eq!(sc.get_color(), TerminalColor::Blue);
}

// ---------------------------------------------------------------------------
// StateColors::get_background_color
// ---------------------------------------------------------------------------

#[test]
fn test_get_background_color_reverse_off() {
    let sc = StateColors::default(); // reverse = Off
    assert_eq!(sc.get_background_color(), TerminalColor::DefaultBackground);
}

#[test]
fn test_get_background_color_reverse_on_default() {
    // ReverseVideo::On → returns color.default_to_regular().
    // Default.default_to_regular() == White.
    let sc = StateColors::default().with_reverse_video(ReverseVideo::On);
    assert_eq!(sc.get_background_color(), TerminalColor::White);
}

#[test]
fn test_get_background_color_reverse_on_custom_colors() {
    // Custom color passes through default_to_regular unchanged.
    let sc = StateColors::default()
        .with_color(TerminalColor::Red)
        .with_background_color(TerminalColor::Blue)
        .with_reverse_video(ReverseVideo::On);
    // get_background_color() returns color.default_to_regular() = Red (unchanged)
    assert_eq!(sc.get_background_color(), TerminalColor::Red);
}

// ---------------------------------------------------------------------------
// StateColors::get_underline_color
// ---------------------------------------------------------------------------

#[test]
fn test_get_underline_color_reverse_off() {
    let sc = StateColors::default(); // reverse = Off
    assert_eq!(
        sc.get_underline_color(),
        TerminalColor::DefaultUnderlineColor
    );
}

#[test]
fn test_get_underline_color_reverse_on() {
    // ReverseVideo::On → returns background_color.default_to_regular().
    // DefaultBackground.default_to_regular() == Black.
    let sc = StateColors::default().with_reverse_video(ReverseVideo::On);
    assert_eq!(sc.get_underline_color(), TerminalColor::Black);
}

#[test]
fn test_get_underline_color_reverse_on_custom_ul() {
    // An explicitly-set underline colour is independent of fg/bg inversion.
    // Even under reverse video, the explicit green should be returned unchanged.
    let sc = StateColors::default()
        .with_underline_color(TerminalColor::Green)
        .with_background_color(TerminalColor::Cyan)
        .with_reverse_video(ReverseVideo::On);
    assert_eq!(sc.get_underline_color(), TerminalColor::Green);
}

// ---------------------------------------------------------------------------
// StateColors::flip_reverse_video
// ---------------------------------------------------------------------------

#[test]
fn test_flip_reverse_video_off_to_on() {
    let mut sc = StateColors::default(); // Off
    sc.flip_reverse_video();
    assert_eq!(sc.reverse_video, ReverseVideo::On);
}

#[test]
fn test_flip_reverse_video_on_to_off() {
    let mut sc = StateColors::default().with_reverse_video(ReverseVideo::On);
    sc.flip_reverse_video();
    assert_eq!(sc.reverse_video, ReverseVideo::Off);
}

#[test]
fn test_flip_reverse_video_double() {
    let mut sc = StateColors::default(); // Off
    sc.flip_reverse_video(); // → On
    sc.flip_reverse_video(); // → Off
    assert_eq!(sc.reverse_video, ReverseVideo::Off);
}

// ---------------------------------------------------------------------------
// StateColors::set_default
// ---------------------------------------------------------------------------

#[test]
fn test_set_default_resets_all() {
    let mut sc = StateColors::default()
        .with_color(TerminalColor::Red)
        .with_background_color(TerminalColor::Blue)
        .with_underline_color(TerminalColor::Green)
        .with_reverse_video(ReverseVideo::On);

    sc.set_default();

    assert_eq!(sc.color, TerminalColor::Default);
    assert_eq!(sc.background_color, TerminalColor::DefaultBackground);
    assert_eq!(sc.underline_color, TerminalColor::DefaultUnderlineColor);
    assert_eq!(sc.reverse_video, ReverseVideo::Off);
}

// ---------------------------------------------------------------------------
// StateColors builder methods
// ---------------------------------------------------------------------------

#[test]
fn test_builder_methods() {
    let sc = StateColors::default()
        .with_color(TerminalColor::Magenta)
        .with_background_color(TerminalColor::Yellow)
        .with_underline_color(TerminalColor::Cyan)
        .with_reverse_video(ReverseVideo::On);

    assert_eq!(sc.color, TerminalColor::Magenta);
    assert_eq!(sc.background_color, TerminalColor::Yellow);
    assert_eq!(sc.underline_color, TerminalColor::Cyan);
    assert_eq!(sc.reverse_video, ReverseVideo::On);
}

// ---------------------------------------------------------------------------
// StateColors setter methods
// ---------------------------------------------------------------------------

#[test]
fn test_setter_methods() {
    let mut sc = StateColors::default();

    sc.set_color(TerminalColor::BrightRed);
    sc.set_background_color(TerminalColor::BrightBlue);
    sc.set_underline_color(TerminalColor::BrightGreen);
    sc.set_reverse_video(ReverseVideo::On);

    assert_eq!(sc.color, TerminalColor::BrightRed);
    assert_eq!(sc.background_color, TerminalColor::BrightBlue);
    assert_eq!(sc.underline_color, TerminalColor::BrightGreen);
    assert_eq!(sc.reverse_video, ReverseVideo::On);
}

// ---------------------------------------------------------------------------
// CursorPos
// ---------------------------------------------------------------------------

#[test]
fn test_cursor_pos_default() {
    let pos = CursorPos::default();
    assert_eq!(pos.x, 0);
    assert_eq!(pos.y, 0);
}

#[test]
fn test_cursor_pos_display() {
    let pos = CursorPos { x: 5, y: 10 };
    assert_eq!(pos.to_string(), "CursorPos { x: 5, y: 10 }");
}

#[test]
fn test_cursor_pos_display_zeros() {
    let pos = CursorPos { x: 0, y: 0 };
    assert_eq!(pos.to_string(), "CursorPos { x: 0, y: 0 }");
}

#[test]
fn test_cursor_pos_equality() {
    let a = CursorPos { x: 3, y: 7 };
    let b = CursorPos { x: 3, y: 7 };
    let c = CursorPos { x: 3, y: 8 };
    assert_eq!(a, b);
    assert_ne!(a, c);
}

#[test]
fn test_cursor_pos_copy() {
    // CursorPos derives Copy — assignment must not move
    let a = CursorPos { x: 1, y: 2 };
    let b = a;
    assert_eq!(a, b); // both still usable
}

// ---------------------------------------------------------------------------
// CursorState — construction and defaults
// ---------------------------------------------------------------------------

#[test]
fn test_cursor_state_default() {
    let cs = CursorState::default();
    assert_eq!(cs.pos, CursorPos { x: 0, y: 0 });
    assert_eq!(cs.font_weight, FontWeight::Normal);
    assert!(cs.font_decorations.is_empty());
    assert_eq!(cs.colors, StateColors::default());
    assert_eq!(cs.line_wrap_mode, LineWrap::Wrap);
    assert!(cs.url.is_none());
}

#[test]
fn test_cursor_state_new_equals_default() {
    assert_eq!(CursorState::new(), CursorState::default());
}

// ---------------------------------------------------------------------------
// CursorState builder methods
// ---------------------------------------------------------------------------

#[test]
fn test_cursor_state_builders() {
    let cs = CursorState::new()
        .with_color(TerminalColor::Green)
        .with_background_color(TerminalColor::BrightBlack)
        .with_pos(CursorPos { x: 4, y: 9 })
        .with_font_weight(FontWeight::Bold)
        .with_font_decorations(vec![FontDecorations::Underline, FontDecorations::Italic]);

    assert_eq!(cs.colors.color, TerminalColor::Green);
    assert_eq!(cs.colors.background_color, TerminalColor::BrightBlack);
    assert_eq!(cs.pos, CursorPos { x: 4, y: 9 });
    assert_eq!(cs.font_weight, FontWeight::Bold);
    assert_eq!(
        cs.font_decorations,
        vec![FontDecorations::Underline, FontDecorations::Italic]
    );
}

#[test]
fn test_cursor_state_builder_pos_zero() {
    // with_pos at (0, 0) is explicitly set, not just default
    let cs = CursorState::new().with_pos(CursorPos { x: 0, y: 0 });
    assert_eq!(cs.pos, CursorPos { x: 0, y: 0 });
}

#[test]
fn test_cursor_state_builder_font_decorations_empty() {
    let cs = CursorState::new().with_font_decorations(vec![]);
    assert!(cs.font_decorations.is_empty());
}

#[test]
fn test_cursor_state_builder_all_font_decorations() {
    let all_decorations = vec![
        FontDecorations::Italic,
        FontDecorations::Underline,
        FontDecorations::Faint,
        FontDecorations::Strikethrough,
    ];
    let cs = CursorState::new().with_font_decorations(all_decorations.clone());
    assert_eq!(cs.font_decorations, all_decorations);
}

#[test]
fn test_cursor_state_builder_does_not_affect_unset_fields() {
    // Setting only color leaves everything else at default.
    let cs = CursorState::new().with_color(TerminalColor::Red);
    assert_eq!(cs.pos, CursorPos { x: 0, y: 0 });
    assert_eq!(cs.font_weight, FontWeight::Normal);
    assert!(cs.font_decorations.is_empty());
    assert_eq!(cs.colors.background_color, TerminalColor::DefaultBackground);
    assert_eq!(
        cs.colors.underline_color,
        TerminalColor::DefaultUnderlineColor
    );
    assert_eq!(cs.colors.reverse_video, ReverseVideo::Off);
    assert_eq!(cs.line_wrap_mode, LineWrap::Wrap);
    assert!(cs.url.is_none());
}
