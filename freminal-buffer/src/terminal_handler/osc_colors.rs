// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! OSC color management for [`TerminalHandler`].
//!
//! This module contains all functions responsible for handling OSC 10/11/12
//! foreground, background, and cursor color queries and overrides, as well as
//! the corresponding OSC 110/111/112 reset sequences:
//!
//! - [`TerminalHandler::handle_osc_fg_bg_color`] — main entry point for
//!   OSC 10/11/12/110/111/112 color query, set, and reset sequences.

use freminal_common::{
    buffer_states::osc::{AnsiOscInternalType, AnsiOscType},
    colors::parse_color_spec,
};

use super::TerminalHandler;

impl TerminalHandler {
    /// Handle OSC 10/11/12 foreground/background/cursor color query, set,
    /// and reset (OSC 110/111/112).
    ///
    /// Extracted from `handle_osc` to keep that function within the 100-line clippy limit.
    ///
    /// - `RequestColorQuery*(Query)`: respond with the effective color
    ///   (override or theme default).
    /// - `RequestColorQuery*(String(spec))`: parse the X11 color spec and
    ///   store as an override.
    /// - `ResetForegroundColor` / `ResetBackgroundColor` / `ResetCursorColor`:
    ///   clear the corresponding override so subsequent queries return the
    ///   theme color.
    pub(super) fn handle_osc_fg_bg_color(&mut self, osc: &AnsiOscType) {
        match osc {
            // OSC 11 query: respond with the effective background color.
            AnsiOscType::RequestColorQueryBackground(AnsiOscInternalType::Query) => {
                let (r, g, b) = self.bg_color_override.unwrap_or(self.theme.background);
                self.write_osc_response(&format!("11;rgb:{r:02x}/{g:02x}/{b:02x}"));
            }
            // OSC 10 query: respond with the effective foreground color.
            AnsiOscType::RequestColorQueryForeground(AnsiOscInternalType::Query) => {
                let (r, g, b) = self.fg_color_override.unwrap_or(self.theme.foreground);
                self.write_osc_response(&format!("10;rgb:{r:02x}/{g:02x}/{b:02x}"));
            }
            // OSC 12 query: respond with the effective cursor color.
            AnsiOscType::RequestColorQueryCursor(AnsiOscInternalType::Query) => {
                let (r, g, b) = self.cursor_color_override.unwrap_or(self.theme.cursor);
                self.write_osc_response(&format!("12;rgb:{r:02x}/{g:02x}/{b:02x}"));
            }
            // OSC 11 set: store a dynamic background color override.
            AnsiOscType::RequestColorQueryBackground(AnsiOscInternalType::String(spec)) => {
                if let Some(rgb) = parse_color_spec(spec) {
                    self.bg_color_override = Some(rgb);
                } else {
                    tracing::warn!("OSC 11: unrecognised color spec: {spec:?}");
                }
            }
            // OSC 10 set: store a dynamic foreground color override.
            AnsiOscType::RequestColorQueryForeground(AnsiOscInternalType::String(spec)) => {
                if let Some(rgb) = parse_color_spec(spec) {
                    self.fg_color_override = Some(rgb);
                } else {
                    tracing::warn!("OSC 10: unrecognised color spec: {spec:?}");
                }
            }
            // OSC 12 set: store a dynamic cursor color override.
            AnsiOscType::RequestColorQueryCursor(AnsiOscInternalType::String(spec)) => {
                if let Some(rgb) = parse_color_spec(spec) {
                    self.cursor_color_override = Some(rgb);
                } else {
                    tracing::warn!("OSC 12: unrecognised color spec: {spec:?}");
                }
            }
            // OSC 110: reset dynamic foreground color override.
            AnsiOscType::ResetForegroundColor => {
                self.fg_color_override = None;
            }
            // OSC 111: reset dynamic background color override.
            AnsiOscType::ResetBackgroundColor => {
                self.bg_color_override = None;
            }
            // OSC 112: reset dynamic cursor color override.
            AnsiOscType::ResetCursorColor => {
                self.cursor_color_override = None;
            }
            // Unknown internal-type variants and unreachable arms — silently ignore.
            _ => {}
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use freminal_common::{
        buffer_states::osc::{AnsiOscInternalType, AnsiOscType},
        pty_write::PtyWrite,
    };

    use super::super::TerminalHandler;

    // ------------------------------------------------------------------
    // OSC 10/11/12 — foreground/background/cursor color query
    // ------------------------------------------------------------------

    #[test]
    fn osc_fg_query_returns_theme_default_when_no_override() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        handler.handle_osc_fg_bg_color(&AnsiOscType::RequestColorQueryForeground(
            AnsiOscInternalType::Query,
        ));

        let Ok(PtyWrite::Write(bytes)) = rx.try_recv() else {
            panic!("expected PtyWrite::Write response");
        };
        let response = String::from_utf8(bytes).expect("response must be valid UTF-8");
        // Default theme foreground for Catppuccin Mocha is (205, 214, 244).
        // 205*257=52685=0xcdd, scaled: 205 → cd_cd; but X11 uses full 16-bit (205*257=52685=0xcdcd)
        // Format: ESC ] 10 ; rgb:rr/gg/bb ESC \  (7-bit mode, 2-byte hex per channel)
        assert!(
            response.contains("10;rgb:"),
            "response should be an OSC 10 color reply: {response:?}"
        );
    }

    #[test]
    fn osc_bg_query_returns_theme_default_when_no_override() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        handler.handle_osc_fg_bg_color(&AnsiOscType::RequestColorQueryBackground(
            AnsiOscInternalType::Query,
        ));

        let Ok(PtyWrite::Write(bytes)) = rx.try_recv() else {
            panic!("expected PtyWrite::Write response");
        };
        let response = String::from_utf8(bytes).expect("response must be valid UTF-8");
        assert!(
            response.contains("11;rgb:"),
            "response should be an OSC 11 color reply: {response:?}"
        );
    }

    #[test]
    fn osc_cursor_query_returns_theme_default_when_no_override() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        handler.handle_osc_fg_bg_color(&AnsiOscType::RequestColorQueryCursor(
            AnsiOscInternalType::Query,
        ));

        let Ok(PtyWrite::Write(bytes)) = rx.try_recv() else {
            panic!("expected PtyWrite::Write response");
        };
        let response = String::from_utf8(bytes).expect("response must be valid UTF-8");
        assert!(
            response.contains("12;rgb:"),
            "response should be an OSC 12 color reply: {response:?}"
        );
    }

    #[test]
    fn osc_fg_set_stores_override_and_query_returns_it() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // Set foreground to red via rgb: spec.
        handler.handle_osc_fg_bg_color(&AnsiOscType::RequestColorQueryForeground(
            AnsiOscInternalType::String("rgb:ff/00/00".to_string()),
        ));

        // Query should return the override.
        handler.handle_osc_fg_bg_color(&AnsiOscType::RequestColorQueryForeground(
            AnsiOscInternalType::Query,
        ));

        let Ok(PtyWrite::Write(bytes)) = rx.try_recv() else {
            panic!("expected PtyWrite::Write response");
        };
        let response = String::from_utf8(bytes).expect("response must be valid UTF-8");
        assert!(
            response.contains("10;rgb:ff/00/00"),
            "response should reflect the set override: {response:?}"
        );
    }

    #[test]
    fn osc_bg_set_stores_override_and_query_returns_it() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        handler.handle_osc_fg_bg_color(&AnsiOscType::RequestColorQueryBackground(
            AnsiOscInternalType::String("rgb:00/ff/00".to_string()),
        ));

        handler.handle_osc_fg_bg_color(&AnsiOscType::RequestColorQueryBackground(
            AnsiOscInternalType::Query,
        ));

        let Ok(PtyWrite::Write(bytes)) = rx.try_recv() else {
            panic!("expected PtyWrite::Write response");
        };
        let response = String::from_utf8(bytes).expect("response must be valid UTF-8");
        assert!(
            response.contains("11;rgb:00/ff/00"),
            "response should reflect the set override: {response:?}"
        );
    }

    #[test]
    fn osc_cursor_set_stores_override_and_query_returns_it() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        handler.handle_osc_fg_bg_color(&AnsiOscType::RequestColorQueryCursor(
            AnsiOscInternalType::String("rgb:00/00/ff".to_string()),
        ));

        handler.handle_osc_fg_bg_color(&AnsiOscType::RequestColorQueryCursor(
            AnsiOscInternalType::Query,
        ));

        let Ok(PtyWrite::Write(bytes)) = rx.try_recv() else {
            panic!("expected PtyWrite::Write response");
        };
        let response = String::from_utf8(bytes).expect("response must be valid UTF-8");
        assert!(
            response.contains("12;rgb:00/00/ff"),
            "response should reflect the set override: {response:?}"
        );
    }

    #[test]
    fn osc_110_resets_fg_override() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // Set an override.
        handler.handle_osc_fg_bg_color(&AnsiOscType::RequestColorQueryForeground(
            AnsiOscInternalType::String("rgb:ff/00/00".to_string()),
        ));

        // Reset it (OSC 110).
        handler.handle_osc_fg_bg_color(&AnsiOscType::ResetForegroundColor);

        // Query should now return the theme default, not the override.
        handler.handle_osc_fg_bg_color(&AnsiOscType::RequestColorQueryForeground(
            AnsiOscInternalType::Query,
        ));

        let Ok(PtyWrite::Write(bytes)) = rx.try_recv() else {
            panic!("expected PtyWrite::Write response after reset");
        };
        let response = String::from_utf8(bytes).expect("response must be valid UTF-8");
        // The theme default is not rgb:ff/00/00.
        assert!(
            !response.contains("10;rgb:ff/00/00"),
            "after OSC 110 reset, response should not contain the old override: {response:?}"
        );
        assert!(
            response.contains("10;rgb:"),
            "response should still be an OSC 10 color reply: {response:?}"
        );
    }

    #[test]
    fn osc_111_resets_bg_override() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        handler.handle_osc_fg_bg_color(&AnsiOscType::RequestColorQueryBackground(
            AnsiOscInternalType::String("rgb:00/ff/00".to_string()),
        ));

        handler.handle_osc_fg_bg_color(&AnsiOscType::ResetBackgroundColor);

        handler.handle_osc_fg_bg_color(&AnsiOscType::RequestColorQueryBackground(
            AnsiOscInternalType::Query,
        ));

        let Ok(PtyWrite::Write(bytes)) = rx.try_recv() else {
            panic!("expected PtyWrite::Write response after reset");
        };
        let response = String::from_utf8(bytes).expect("response must be valid UTF-8");
        assert!(
            !response.contains("11;rgb:00/ff/00"),
            "after OSC 111 reset, response should not contain the old override: {response:?}"
        );
        assert!(
            response.contains("11;rgb:"),
            "response should still be an OSC 11 color reply: {response:?}"
        );
    }

    #[test]
    fn osc_112_resets_cursor_override() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        handler.handle_osc_fg_bg_color(&AnsiOscType::RequestColorQueryCursor(
            AnsiOscInternalType::String("rgb:00/00/ff".to_string()),
        ));

        handler.handle_osc_fg_bg_color(&AnsiOscType::ResetCursorColor);

        handler.handle_osc_fg_bg_color(&AnsiOscType::RequestColorQueryCursor(
            AnsiOscInternalType::Query,
        ));

        let Ok(PtyWrite::Write(bytes)) = rx.try_recv() else {
            panic!("expected PtyWrite::Write response after reset");
        };
        let response = String::from_utf8(bytes).expect("response must be valid UTF-8");
        assert!(
            !response.contains("12;rgb:00/00/ff"),
            "after OSC 112 reset, response should not contain the old override: {response:?}"
        );
        assert!(
            response.contains("12;rgb:"),
            "response should still be an OSC 12 color reply: {response:?}"
        );
    }

    #[test]
    fn osc_fg_set_invalid_spec_is_ignored() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // Set a valid override first.
        handler.handle_osc_fg_bg_color(&AnsiOscType::RequestColorQueryForeground(
            AnsiOscInternalType::String("rgb:ff/00/00".to_string()),
        ));

        // Attempt to set an invalid spec — override should remain unchanged.
        handler.handle_osc_fg_bg_color(&AnsiOscType::RequestColorQueryForeground(
            AnsiOscInternalType::String("not-a-color".to_string()),
        ));

        handler.handle_osc_fg_bg_color(&AnsiOscType::RequestColorQueryForeground(
            AnsiOscInternalType::Query,
        ));

        let Ok(PtyWrite::Write(bytes)) = rx.try_recv() else {
            panic!("expected PtyWrite::Write response");
        };
        let response = String::from_utf8(bytes).expect("response must be valid UTF-8");
        // The previous valid override (ff/00/00) should still be active.
        assert!(
            response.contains("10;rgb:ff/00/00"),
            "invalid spec should not overwrite a valid override: {response:?}"
        );
    }

    #[test]
    fn full_reset_clears_fg_bg_cursor_overrides() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // Set all three overrides.
        handler.handle_osc_fg_bg_color(&AnsiOscType::RequestColorQueryForeground(
            AnsiOscInternalType::String("rgb:ff/00/00".to_string()),
        ));
        handler.handle_osc_fg_bg_color(&AnsiOscType::RequestColorQueryBackground(
            AnsiOscInternalType::String("rgb:00/ff/00".to_string()),
        ));
        handler.handle_osc_fg_bg_color(&AnsiOscType::RequestColorQueryCursor(
            AnsiOscInternalType::String("rgb:00/00/ff".to_string()),
        ));

        // full_reset clears all overrides.
        handler.full_reset();

        // All queries should now return theme defaults, not the set overrides.
        handler.handle_osc_fg_bg_color(&AnsiOscType::RequestColorQueryForeground(
            AnsiOscInternalType::Query,
        ));
        let Ok(PtyWrite::Write(fg_bytes)) = rx.try_recv() else {
            panic!("expected fg PtyWrite::Write response after full_reset");
        };
        let fg_response = String::from_utf8(fg_bytes).expect("fg response must be valid UTF-8");
        assert!(
            !fg_response.contains("10;rgb:ff/00/00"),
            "full_reset must clear fg override: {fg_response:?}"
        );

        handler.handle_osc_fg_bg_color(&AnsiOscType::RequestColorQueryBackground(
            AnsiOscInternalType::Query,
        ));
        let Ok(PtyWrite::Write(bg_bytes)) = rx.try_recv() else {
            panic!("expected bg PtyWrite::Write response after full_reset");
        };
        let bg_response = String::from_utf8(bg_bytes).expect("bg response must be valid UTF-8");
        assert!(
            !bg_response.contains("11;rgb:00/ff/00"),
            "full_reset must clear bg override: {bg_response:?}"
        );

        handler.handle_osc_fg_bg_color(&AnsiOscType::RequestColorQueryCursor(
            AnsiOscInternalType::Query,
        ));
        let Ok(PtyWrite::Write(cur_bytes)) = rx.try_recv() else {
            panic!("expected cursor PtyWrite::Write response after full_reset");
        };
        let cur_response =
            String::from_utf8(cur_bytes).expect("cursor response must be valid UTF-8");
        assert!(
            !cur_response.contains("12;rgb:00/00/ff"),
            "full_reset must clear cursor override: {cur_response:?}"
        );
    }

    #[test]
    fn cursor_color_override_accessor_reflects_set_value() {
        let mut handler = TerminalHandler::new(80, 24);

        // Initially no override.
        assert_eq!(handler.cursor_color_override(), None);

        // Set a cursor color.
        handler.handle_osc_fg_bg_color(&AnsiOscType::RequestColorQueryCursor(
            AnsiOscInternalType::String("rgb:12/34/56".to_string()),
        ));
        assert_eq!(handler.cursor_color_override(), Some((0x12, 0x34, 0x56)));

        // Reset it.
        handler.handle_osc_fg_bg_color(&AnsiOscType::ResetCursorColor);
        assert_eq!(handler.cursor_color_override(), None);
    }
}
