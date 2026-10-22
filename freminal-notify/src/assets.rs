// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! The bytes the backends draw with.

use std::fmt;

/// The bytes the backends draw with. The GUI owns the files
/// (`include_bytes!`) and passes them in.
///
/// `Debug` prints the length of each file, not its bytes: the font alone is
/// about 2 MB.
#[derive(Clone, Copy)]
pub struct NotifyAssets {
    /// freminal's application icon, a PNG: the icon of plain (OSC 9 / 777 /
    /// 133 D) notifications on Linux/BSD.
    pub app_icon_png: &'static [u8],
    /// The bundled `CaskaydiaCove` regular face: the OSC 99 standard icons
    /// are drawn from it on macOS and Windows.
    pub icon_font: &'static [u8],
}

impl fmt::Debug for NotifyAssets {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NotifyAssets")
            .field("app_icon_png", &ByteLen(self.app_icon_png))
            .field("icon_font", &ByteLen(self.icon_font))
            .finish()
    }
}

/// A byte slice that `Debug`-prints as its length (`<N bytes>`), for the
/// fields of the types above and below that hold an embedded file.
pub struct ByteLen<'a>(pub &'a [u8]);

impl fmt::Debug for ByteLen<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<{} bytes>", self.0.len())
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_prints_the_byte_lengths_not_the_bytes() {
        let assets = test_assets();
        let printed = format!("{assets:?}");

        assert!(printed.len() < 200, "{} chars: {printed}", printed.len());
        assert!(
            printed.contains(&format!(
                "app_icon_png: <{} bytes>",
                assets.app_icon_png.len()
            )),
            "{printed}"
        );
        assert!(
            printed.contains(&format!("icon_font: <{} bytes>", assets.icon_font.len())),
            "{printed}"
        );
    }
}
