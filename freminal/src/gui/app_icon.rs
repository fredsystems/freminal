// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use crate::gui::desktop_notify::NotifyAssets;
use crate::gui::font_manager::bundled_regular_font_bytes;

/// freminal's application icon, the one PNG embedded in the binary. It is the
/// OS window icon (`run`) and, on Linux/BSD, the picture of every plain
/// desktop notification (`osc99::dbus_service`).
pub(in crate::gui) static FREMINAL_ICON_PNG: &[u8] = include_bytes!("../../../assets/icon.png");

/// The bytes the desktop-notification backends draw with: the application icon
/// and the bundled face the macOS and Windows standard icons are drawn from.
#[must_use]
pub(in crate::gui) fn notify_assets() -> NotifyAssets {
    NotifyAssets {
        app_icon_png: FREMINAL_ICON_PNG,
        icon_font: bundled_regular_font_bytes(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::desktop_notify::test_support::test_assets;

    #[test]
    fn the_backend_test_assets_are_the_files_the_gui_embeds() {
        let test = test_assets();
        assert_eq!(test.app_icon_png, FREMINAL_ICON_PNG);
        assert_eq!(test.icon_font, bundled_regular_font_bytes());
    }

    #[test]
    fn the_gui_hands_the_backends_its_own_files() {
        let assets = notify_assets();
        assert_eq!(assets.app_icon_png, FREMINAL_ICON_PNG);
        assert_eq!(assets.icon_font, bundled_regular_font_bytes());
    }
}
