// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use crate::config::CursorShapeConfig;

/// The shape and blink behaviour of the terminal cursor.
///
/// Values map to the DECSCUSR parameter (`CSI Ps SP q`):
///
/// | Variant                  | Ps |
/// | ------------------------ | -- |
/// | `BlockCursorBlink`       | 0 or 1 |
/// | `BlockCursorSteady`      | 2 |
/// | `UnderlineCursorBlink`   | 3 |
/// | `UnderlineCursorSteady`  | 4 |
/// | `VerticalLineCursorBlink`| 5 |
/// | `VerticalLineCursorSteady`| 6 |
#[allow(clippy::module_name_repetitions)]
#[derive(Default, Debug, Eq, PartialEq, Clone)]
pub enum CursorVisualStyle {
    /// Blinking block cursor (DECSCUSR 0 or 1).
    BlockCursorBlink,
    /// Steady block cursor — the default (DECSCUSR 2).
    #[default]
    BlockCursorSteady,
    /// Blinking underline cursor (DECSCUSR 3).
    UnderlineCursorBlink,
    /// Steady underline cursor (DECSCUSR 4).
    UnderlineCursorSteady,
    /// Blinking vertical-bar cursor (DECSCUSR 5).
    VerticalLineCursorBlink,
    /// Steady vertical-bar cursor (DECSCUSR 6).
    VerticalLineCursorSteady,
}

impl From<usize> for CursorVisualStyle {
    fn from(value: usize) -> Self {
        match value {
            2 => Self::BlockCursorSteady,
            3 => Self::UnderlineCursorBlink,
            4 => Self::UnderlineCursorSteady,
            5 => Self::VerticalLineCursorBlink,
            6 => Self::VerticalLineCursorSteady,
            _ => Self::BlockCursorBlink,
        }
    }
}

impl CursorVisualStyle {
    /// Build the initial/default cursor style from the user's `[cursor]`
    /// config (issue #406: `cursor.shape`/`cursor.blink` were previously
    /// only read by the Settings UI form itself, never applied to the
    /// terminal's actual `CursorVisualStyle`).
    ///
    /// This is a one-time seed, not a live constraint: once a running
    /// program sets a style via DECSCUSR or toggles `XTCBlink`, that
    /// program's request takes over, exactly as on a real terminal — the
    /// config only supplies the value in effect before any program has
    /// asked for something else.
    #[must_use]
    pub const fn from_config(shape: &CursorShapeConfig, blink: bool) -> Self {
        match (shape, blink) {
            (CursorShapeConfig::Block, true) => Self::BlockCursorBlink,
            (CursorShapeConfig::Block, false) => Self::BlockCursorSteady,
            (CursorShapeConfig::Underline, true) => Self::UnderlineCursorBlink,
            (CursorShapeConfig::Underline, false) => Self::UnderlineCursorSteady,
            (CursorShapeConfig::Bar, true) => Self::VerticalLineCursorBlink,
            (CursorShapeConfig::Bar, false) => Self::VerticalLineCursorSteady,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_config_maps_every_shape_and_blink_combination() {
        assert_eq!(
            CursorVisualStyle::from_config(&CursorShapeConfig::Block, true),
            CursorVisualStyle::BlockCursorBlink
        );
        assert_eq!(
            CursorVisualStyle::from_config(&CursorShapeConfig::Block, false),
            CursorVisualStyle::BlockCursorSteady
        );
        assert_eq!(
            CursorVisualStyle::from_config(&CursorShapeConfig::Underline, true),
            CursorVisualStyle::UnderlineCursorBlink
        );
        assert_eq!(
            CursorVisualStyle::from_config(&CursorShapeConfig::Underline, false),
            CursorVisualStyle::UnderlineCursorSteady
        );
        assert_eq!(
            CursorVisualStyle::from_config(&CursorShapeConfig::Bar, true),
            CursorVisualStyle::VerticalLineCursorBlink
        );
        assert_eq!(
            CursorVisualStyle::from_config(&CursorShapeConfig::Bar, false),
            CursorVisualStyle::VerticalLineCursorSteady
        );
    }

    #[test]
    fn from_config_matches_default_config_default_style() {
        // `CursorConfig::default()` is `Block` + `blink: true`, which should
        // agree with `CursorVisualStyle::default()`'s intent... except the
        // *type* default is steady block (matching xterm's power-on
        // default), while the *config* default enables blink. This
        // deliberately documents that the config default and the bare-type
        // default are NOT required to match — `from_config` must honor
        // whatever the config says, not silently fall back to the type
        // default.
        //
        // Reads `blink` from a real `CursorConfig::default()` (rather than
        // hardcoding `true`) so this test actually fails if the config
        // default ever changes, instead of silently drifting out of sync.
        let default_cursor_config = crate::config::CursorConfig::default();
        assert_eq!(
            CursorVisualStyle::from_config(
                &default_cursor_config.shape,
                default_cursor_config.blink
            ),
            CursorVisualStyle::BlockCursorBlink
        );
    }
}
