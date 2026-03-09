// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use eframe::egui::Color32;
use freminal_common::colors::TerminalColor;

/// Catppuccin Mocha (Lavender-adjusted) palette mapping
#[must_use]
pub fn internal_color_to_egui(color: TerminalColor, make_faint: bool) -> Color32 {
    let color_before_faint = match color {
        TerminalColor::Default
        | TerminalColor::DefaultUnderlineColor
        | TerminalColor::DefaultCursorColor => Color32::from_hex("#cdd6f4").unwrap_or_default(),

        TerminalColor::DefaultBackground => Color32::from_hex("#1e1e2e").unwrap_or_default(),

        // Base palette 0–7
        TerminalColor::Black => Color32::from_hex("#45475a").unwrap_or_default(), // 0
        TerminalColor::Red => Color32::from_hex("#f38ba8").unwrap_or_default(),   // 1
        TerminalColor::Green => Color32::from_hex("#a6e3a1").unwrap_or_default(), // 2
        TerminalColor::Yellow => Color32::from_hex("#f9e2af").unwrap_or_default(), // 3
        TerminalColor::Blue => Color32::from_hex("#89b4fa").unwrap_or_default(),  // 4
        TerminalColor::Magenta => Color32::from_hex("#f5c2e7").unwrap_or_default(), // 5
        TerminalColor::Cyan => Color32::from_hex("#94e2d5").unwrap_or_default(),  // 6
        TerminalColor::White => Color32::from_hex("#a6adc8").unwrap_or_default(), // 7

        // Bright palette 8–15
        TerminalColor::BrightBlack => Color32::from_hex("#585b70").unwrap_or_default(), // 8
        TerminalColor::BrightRed => Color32::from_hex("#f37799").unwrap_or_default(),   // 9
        TerminalColor::BrightGreen => Color32::from_hex("#89d88b").unwrap_or_default(), // 10
        TerminalColor::BrightYellow => Color32::from_hex("#ebd391").unwrap_or_default(), // 11
        TerminalColor::BrightBlue => Color32::from_hex("#74a8fc").unwrap_or_default(),  // 12
        TerminalColor::BrightMagenta => Color32::from_hex("#f2aede").unwrap_or_default(), // 13
        TerminalColor::BrightCyan => Color32::from_hex("#6bd7ca").unwrap_or_default(),  // 14
        TerminalColor::BrightWhite => Color32::from_hex("#bac2de").unwrap_or_default(), // 15

        TerminalColor::Custom(r, g, b) => Color32::from_rgb(r, g, b),
    };

    if make_faint {
        color_before_faint.gamma_multiply(0.5)
    } else {
        color_before_faint
    }
}

// Theme reference
// background = "#1e1e2e"
// foreground = "#cdd6f4"
// selection-background = "#353748"
// selection-foreground = "#cdd6f4"
// cursor-color = "#f5e0dc"
// cursor-text = "#11111b"
