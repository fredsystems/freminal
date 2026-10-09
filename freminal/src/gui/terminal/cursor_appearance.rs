// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Pure resolution of how a pane's cursor should be drawn (Task 127.2).
//!
//! The cursor's appearance depends on four independent facts: whether the
//! terminal application wants it shown (`DECTCEM`), whether the pane is in
//! echo-off (password prompt) mode, whether the pane and window currently have
//! focus, and the user's [`UnfocusedCursorStyle`] preference. This module
//! folds those into a single [`CursorAppearance`] so the renderer can decide
//! with one `match` instead of scattered focus checks.
//!
//! The module is deliberately pure: it depends on no egui, renderer, or
//! snapshot type beyond [`CursorVisualStyle`]. Blink gating for
//! [`CursorAppearance::Solid`] stays in the renderer's
//! `cursor_blink_is_visible`; this module only decides *what* to draw, not
//! *when* it is in the visible half of a blink cycle.

use freminal_common::config::UnfocusedCursorStyle;
use freminal_common::cursor::CursorVisualStyle;

use super::input::PaneFocus;
use crate::gui::frame_drain::WindowFocus;

/// The combined focus state that determines whether a cursor is drawn as
/// focused or unfocused.
#[cfg_attr(not(test), expect(dead_code))] // TODO(127.5): wired into the pane renderer
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorFocus {
    /// The window is focused and this is the active pane.
    Focused,
    /// The window is focused but this pane is not the active one.
    InactivePane,
    /// The window does not have input focus (regardless of which pane is
    /// active within it).
    UnfocusedWindow,
}

/// How the cursor should be drawn for one pane in one frame.
#[cfg_attr(not(test), expect(dead_code))] // TODO(127.5): wired into the pane renderer
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CursorAppearance {
    /// No cursor is drawn.
    Hidden,
    /// A solid cursor in the given DECSCUSR style. Whether a blinking style
    /// is currently in the visible half of its cycle is decided by the
    /// renderer, not here.
    Solid(CursorVisualStyle),
    /// A steady hollow block.
    Hollow,
}

/// Whether the terminal application has asked for the cursor to be shown
/// (derived from `snap.show_cursor`, i.e. `DECTCEM`).
#[cfg_attr(not(test), expect(dead_code))] // TODO(127.5): wired into the pane renderer
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorVisibility {
    /// The application wants the cursor shown.
    Shown,
    /// The application has hidden the cursor.
    Hidden,
}

/// Whether the pane's terminal is in echo-off mode (e.g. a password prompt),
/// in which the cursor is suppressed.
#[cfg_attr(not(test), expect(dead_code))] // TODO(127.5): wired into the pane renderer
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EchoState {
    /// Normal echo; the cursor is not suppressed on this account.
    Normal,
    /// Echo is off; the cursor is hidden.
    EchoOff,
}

/// Everything [`resolve_cursor_appearance`] needs to decide.
#[cfg_attr(not(test), expect(dead_code))] // TODO(127.5): wired into the pane renderer
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorAppearanceInputs {
    /// Whether the application wants the cursor shown.
    pub snapshot_visible: CursorVisibility,
    /// Whether the pane is in echo-off mode.
    pub echo: EchoState,
    /// The combined pane/window focus state.
    pub focus: CursorFocus,
    /// The application's DECSCUSR style.
    pub style: CursorVisualStyle,
    /// The user's preference for unfocused cursors.
    pub unfocused_style: UnfocusedCursorStyle,
}

/// Combine pane focus and window focus into a [`CursorFocus`].
///
/// An unfocused window wins over an inactive pane: when the window itself
/// lacks focus, the pane's own active/inactive status is irrelevant.
#[cfg_attr(not(test), expect(dead_code))] // TODO(127.5): wired into the pane renderer
pub(super) const fn cursor_focus(pane: PaneFocus, window: WindowFocus) -> CursorFocus {
    match (window, pane) {
        (WindowFocus::Unfocused, _) => CursorFocus::UnfocusedWindow,
        (WindowFocus::Focused, PaneFocus::Inactive) => CursorFocus::InactivePane,
        (WindowFocus::Focused, PaneFocus::Active) => CursorFocus::Focused,
    }
}

/// The steady counterpart of a DECSCUSR style. Steady styles map to
/// themselves.
#[cfg_attr(not(test), expect(dead_code))] // TODO(127.5): wired into the pane renderer
const fn steady_variant(style: &CursorVisualStyle) -> CursorVisualStyle {
    match style {
        CursorVisualStyle::BlockCursorBlink | CursorVisualStyle::BlockCursorSteady => {
            CursorVisualStyle::BlockCursorSteady
        }
        CursorVisualStyle::UnderlineCursorBlink | CursorVisualStyle::UnderlineCursorSteady => {
            CursorVisualStyle::UnderlineCursorSteady
        }
        CursorVisualStyle::VerticalLineCursorBlink
        | CursorVisualStyle::VerticalLineCursorSteady => {
            CursorVisualStyle::VerticalLineCursorSteady
        }
    }
}

/// Decide how the cursor is drawn.
///
/// - A cursor the application hid, or one in echo-off mode, is
///   [`CursorAppearance::Hidden`] in every focus state.
/// - When focused, the application's style is kept exactly (blink included).
/// - When not focused, the user's [`UnfocusedCursorStyle`] applies: `Hollow`
///   draws a hollow block, `Unchanged` keeps the shape but forces it steady,
///   and `Hidden` draws nothing.
#[cfg_attr(not(test), expect(dead_code))] // TODO(127.5): wired into the pane renderer
pub fn resolve_cursor_appearance(inputs: &CursorAppearanceInputs) -> CursorAppearance {
    if inputs.snapshot_visible == CursorVisibility::Hidden || inputs.echo == EchoState::EchoOff {
        return CursorAppearance::Hidden;
    }

    match inputs.focus {
        CursorFocus::Focused => CursorAppearance::Solid(inputs.style.clone()),
        CursorFocus::InactivePane | CursorFocus::UnfocusedWindow => match inputs.unfocused_style {
            UnfocusedCursorStyle::Hollow => CursorAppearance::Hollow,
            UnfocusedCursorStyle::Unchanged => {
                CursorAppearance::Solid(steady_variant(&inputs.style))
            }
            UnfocusedCursorStyle::Hidden => CursorAppearance::Hidden,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_STYLES: [CursorVisualStyle; 6] = [
        CursorVisualStyle::BlockCursorBlink,
        CursorVisualStyle::BlockCursorSteady,
        CursorVisualStyle::UnderlineCursorBlink,
        CursorVisualStyle::UnderlineCursorSteady,
        CursorVisualStyle::VerticalLineCursorBlink,
        CursorVisualStyle::VerticalLineCursorSteady,
    ];

    const ALL_VISIBILITY: [CursorVisibility; 2] =
        [CursorVisibility::Shown, CursorVisibility::Hidden];

    const ALL_ECHO: [EchoState; 2] = [EchoState::Normal, EchoState::EchoOff];

    const ALL_FOCUS: [CursorFocus; 3] = [
        CursorFocus::Focused,
        CursorFocus::InactivePane,
        CursorFocus::UnfocusedWindow,
    ];

    const ALL_UNFOCUSED: [UnfocusedCursorStyle; 3] = [
        UnfocusedCursorStyle::Hollow,
        UnfocusedCursorStyle::Unchanged,
        UnfocusedCursorStyle::Hidden,
    ];

    /// The expected steady style for each entry of `ALL_STYLES`, written out
    /// independently of `steady_variant`.
    const EXPECTED_STEADY: [CursorVisualStyle; 6] = [
        CursorVisualStyle::BlockCursorSteady,
        CursorVisualStyle::BlockCursorSteady,
        CursorVisualStyle::UnderlineCursorSteady,
        CursorVisualStyle::UnderlineCursorSteady,
        CursorVisualStyle::VerticalLineCursorSteady,
        CursorVisualStyle::VerticalLineCursorSteady,
    ];

    #[test]
    fn resolve_exhaustive_table() {
        let mut cases = 0_usize;
        for visibility in ALL_VISIBILITY {
            for echo in ALL_ECHO {
                for focus in ALL_FOCUS {
                    for (style, steady) in ALL_STYLES.iter().zip(EXPECTED_STEADY.iter()) {
                        for unfocused_style in ALL_UNFOCUSED {
                            let inputs = CursorAppearanceInputs {
                                snapshot_visible: visibility,
                                echo,
                                focus,
                                style: style.clone(),
                                unfocused_style,
                            };
                            let got = resolve_cursor_appearance(&inputs);
                            cases += 1;

                            let suppressed = visibility == CursorVisibility::Hidden
                                || echo == EchoState::EchoOff;
                            let expected = if suppressed {
                                CursorAppearance::Hidden
                            } else if focus == CursorFocus::Focused {
                                CursorAppearance::Solid(style.clone())
                            } else {
                                match unfocused_style {
                                    UnfocusedCursorStyle::Hollow => CursorAppearance::Hollow,
                                    UnfocusedCursorStyle::Unchanged => {
                                        CursorAppearance::Solid(steady.clone())
                                    }
                                    UnfocusedCursorStyle::Hidden => CursorAppearance::Hidden,
                                }
                            };
                            assert_eq!(got, expected, "inputs: {inputs:?}");
                        }
                    }
                }
            }
        }
        assert_eq!(cases, 2 * 2 * 3 * 6 * 3);
    }

    #[test]
    fn focused_preserves_blink_variant() {
        let inputs = CursorAppearanceInputs {
            snapshot_visible: CursorVisibility::Shown,
            echo: EchoState::Normal,
            focus: CursorFocus::Focused,
            style: CursorVisualStyle::BlockCursorBlink,
            unfocused_style: UnfocusedCursorStyle::Unchanged,
        };
        assert_eq!(
            resolve_cursor_appearance(&inputs),
            CursorAppearance::Solid(CursorVisualStyle::BlockCursorBlink)
        );
    }

    #[test]
    fn unfocused_unchanged_makes_blink_steady() {
        let inputs = CursorAppearanceInputs {
            snapshot_visible: CursorVisibility::Shown,
            echo: EchoState::Normal,
            focus: CursorFocus::InactivePane,
            style: CursorVisualStyle::VerticalLineCursorBlink,
            unfocused_style: UnfocusedCursorStyle::Unchanged,
        };
        assert_eq!(
            resolve_cursor_appearance(&inputs),
            CursorAppearance::Solid(CursorVisualStyle::VerticalLineCursorSteady)
        );
    }

    #[test]
    fn cursor_focus_active_pane_focused_window() {
        assert_eq!(
            cursor_focus(PaneFocus::Active, WindowFocus::Focused),
            CursorFocus::Focused
        );
    }

    #[test]
    fn cursor_focus_inactive_pane_focused_window() {
        assert_eq!(
            cursor_focus(PaneFocus::Inactive, WindowFocus::Focused),
            CursorFocus::InactivePane
        );
    }

    #[test]
    fn cursor_focus_active_pane_unfocused_window() {
        assert_eq!(
            cursor_focus(PaneFocus::Active, WindowFocus::Unfocused),
            CursorFocus::UnfocusedWindow
        );
    }

    #[test]
    fn cursor_focus_inactive_pane_unfocused_window() {
        assert_eq!(
            cursor_focus(PaneFocus::Inactive, WindowFocus::Unfocused),
            CursorFocus::UnfocusedWindow
        );
    }
}
