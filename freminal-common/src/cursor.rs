// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

#[allow(clippy::module_name_repetitions)]
#[derive(Default, Debug, Eq, PartialEq, Clone)]
pub enum CursorVisualStyle {
    BlockCursorBlink,
    #[default]
    BlockCursorSteady,
    UnderlineCursorBlink,
    UnderlineCursorSteady,
    VerticalLineCursorBlink,
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
