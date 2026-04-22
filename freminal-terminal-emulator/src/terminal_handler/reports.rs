// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Terminal report and query response methods for [`TerminalHandler`].
//!
//! Contains DA1/DA2/DA3, DECREQTPARM, DSR, color theme report, and
//! device name/version responses.

use freminal_common::buffer_states::modes::decanm::Decanm;

use super::TerminalHandler;

impl TerminalHandler {
    /// Handle DA2 — Secondary Device Attributes.
    /// Responds with `ESC [ > 65 ; 0 ; 0 c` (VT525, firmware 0, ROM 0).
    pub fn handle_secondary_device_attributes(&mut self) {
        tracing::debug!("DA2 query received");
        self.write_csi_response(">65;0;0c");
    }

    /// Handle DA3 — Tertiary Device Attributes.
    /// Responds with `DCS ! | 00000000 ST`.
    /// This identifies Freminal with a fixed 8-digit hexadecimal unit ID.
    pub fn handle_tertiary_device_attributes(&mut self) {
        self.write_dcs_response("!|00000000");
    }

    /// Handle DECREQTPARM — Request Terminal Parameters.
    ///
    /// Sends `CSI <code> ; 1 ; 1 ; 120 ; 120 ; 1 ; 0 x` where `<code>` is
    /// `2` for Ps=0 and `3` for Ps=1. Values chosen to represent:
    /// - Parity: 1 (NONE)
    /// - Bits: 1 (8-bit)
    /// - Transmit speed: 120 (38400 baud)
    /// - Receive speed: 120 (38400 baud)
    /// - Clock multiplier: 1
    /// - Flags: 0
    pub fn handle_request_terminal_parameters(&mut self, ps: u8) {
        // DECREQTPARM only defines Ps=0 and Ps=1.  The parser should have
        // already validated this, but we defend against unexpected values.
        let code = match ps {
            0 => 2u8,
            1 => 3u8,
            _ => return,
        };
        self.write_csi_response(&format!("{code};1;1;120;120;1;0x"));
    }

    /// Handle `RequestDeviceNameAndVersion` — respond with Freminal's name and version.
    ///
    /// Responds with `DCS >|XTerm(Freminal <version>) ST` (7-bit) or the 8-bit
    /// equivalent when S8C1T is active.
    ///
    /// The `XTerm(` prefix is intentional: tmux's XDA handler
    /// (`tty_keys_extended_device_attributes` in `tty-keys.c`) matches the
    /// payload against a small set of known prefixes to decide which terminal
    /// feature sets to enable.  Without a recognised prefix tmux skips
    /// `extkeys`, which means `modifyOtherKeys` (`\033[>4;2m`) is never sent
    /// to Freminal and extended key sequences are not forwarded to programs
    /// running inside tmux.  Prefixing with `XTerm(` causes tmux to apply the
    /// `XTerm` feature set (which includes `extkeys`), fixing the issue.
    pub fn handle_device_name_and_version(&mut self) {
        let version = env!("CARGO_PKG_VERSION");
        self.write_dcs_response(&format!(">|XTerm(Freminal {version})"));
    }

    /// Handle DSR — Device Status Report (Ps=5).
    /// Responds with `CSI 0 n` (device OK).
    pub fn handle_device_status_report(&mut self) {
        self.write_csi_response("0n");
    }

    /// Handle DSR ?996 — Color Theme Report.
    /// Responds with `CSI ? 997 ; Ps n` where Ps = 1 (light) or 2 (dark).
    /// Freminal's default background is dark (#45475a), so we report dark (2).
    pub fn handle_color_theme_report(&mut self) {
        // 1 = light, 2 = dark
        self.write_csi_response("?997;2n");
    }

    /// Handle DA1 — Primary Device Attributes.
    /// Responds with the capability string used by the old buffer (iTerm2 DA set).
    pub fn handle_request_device_attributes(&mut self) {
        tracing::debug!("DA1 query received");
        if self.vt52_mode == Decanm::Vt52 {
            // VT52 identify response: ESC / Z — not affected by S8C1T
            self.write_to_pty("\x1b/Z");
        } else {
            self.write_csi_response("?65;1;2;4;6;17;18;22c");
        }
    }
}
