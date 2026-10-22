// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! The bytes the backends draw with.

/// The bytes the backends draw with. The GUI owns the files
/// (`include_bytes!`) and passes them in.
#[derive(Debug, Clone, Copy)]
pub struct NotifyAssets {
    /// freminal's application icon, a PNG: the icon of plain (OSC 9 / 777 /
    /// 133 D) notifications on Linux/BSD.
    pub app_icon_png: &'static [u8],
    /// The bundled `CaskaydiaCove` regular face: the OSC 99 standard icons
    /// are drawn from it on macOS and Windows.
    pub icon_font: &'static [u8],
}

/// The assets the backends' tests use: the same repository files the GUI
/// embeds.
#[cfg(any(test, feature = "test-utils"))]
#[must_use]
pub const fn test_assets() -> NotifyAssets {
    NotifyAssets {
        app_icon_png: include_bytes!("../../assets/icon.png"),
        icon_font: include_bytes!("../../res/CaskaydiaCoveNerdFont-Regular.ttf"),
    }
}
