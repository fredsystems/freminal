// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use anyhow::Result;

#[allow(clippy::module_name_repetitions)]
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum WindowManipulation {
    DeIconifyWindow,
    MinimizeWindow,
    MoveWindow(usize, usize),
    ResizeWindow(usize, usize),
    RaiseWindowToTopOfStackingOrder,
    LowerWindowToBottomOfStackingOrder,
    RefreshWindow,
    ResizeWindowToLinesAndColumns(usize, usize),
    MaximizeWindow,
    RestoreNonMaximizedWindow,
    NotFullScreen,
    FullScreen,
    ToggleFullScreen,
    ReportWindowState,
    ReportWindowPositionWholeWindow,
    ReportWindowPositionTextArea,
    ReportWindowSizeInPixels,
    ReportWindowTextAreaSizeInPixels,
    ReportRootWindowSizeInPixels,
    ReportCharacterSizeInPixels,
    ReportTerminalSizeInCharacters,
    ReportRootWindowSizeInCharacters,
    ReportIconLabel,
    ReportTitle,
    SetTitleBarText(String),
    SaveWindowTitleToStack,
    RestoreWindowTitleFromStack,
}

impl TryFrom<(usize, usize, usize)> for WindowManipulation {
    type Error = anyhow::Error;

    fn try_from((command, param_ps2, param_ps3): (usize, usize, usize)) -> Result<Self> {
        match (command, param_ps2, param_ps3) {
            (1, _, _) => Ok(Self::DeIconifyWindow),
            (2, _, _) => Ok(Self::MinimizeWindow),
            (3, x, y) => Ok(Self::MoveWindow(x, y)),
            (4, x, y) => Ok(Self::ResizeWindow(x, y)),
            (5, _, _) => Ok(Self::RaiseWindowToTopOfStackingOrder),
            (6, _, _) => Ok(Self::LowerWindowToBottomOfStackingOrder),
            (7, _, _) => Ok(Self::RefreshWindow),
            (8, x, y) => Ok(Self::ResizeWindowToLinesAndColumns(x, y)),
            (9, 1, _) => Ok(Self::MaximizeWindow),
            (9, 0, _) => Ok(Self::RestoreNonMaximizedWindow),
            (10, 0, _) => Ok(Self::NotFullScreen),
            (10, 1, _) => Ok(Self::FullScreen),
            (10, 2, _) => Ok(Self::ToggleFullScreen),
            (11, _, _) => Ok(Self::ReportWindowState),
            (13, 0 | 1, _) => Ok(Self::ReportWindowPositionWholeWindow),
            (13, 2, 0) => Ok(Self::ReportWindowPositionTextArea),
            (14, 0 | 1, _) => Ok(Self::ReportWindowSizeInPixels),
            (14, 2, _) => Ok(Self::ReportWindowTextAreaSizeInPixels),
            (15, _, _) => Ok(Self::ReportRootWindowSizeInPixels),
            (16, _, _) => Ok(Self::ReportCharacterSizeInPixels),
            (18, _, _) => Ok(Self::ReportTerminalSizeInCharacters),
            (19, _, _) => Ok(Self::ReportRootWindowSizeInCharacters),
            (20, _, _) => Ok(Self::ReportIconLabel),
            (21, _, _) => Ok(Self::ReportTitle),
            (22, 0..=2, _) => Ok(Self::SaveWindowTitleToStack),
            (23, 0..=2, _) => Ok(Self::RestoreWindowTitleFromStack),
            (24, 0..=2, _) => Ok(Self::SetTitleBarText(String::new())),
            _ => Err(anyhow::anyhow!("Invalid WindowManipulation")),
        }
    }
}
