// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

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
