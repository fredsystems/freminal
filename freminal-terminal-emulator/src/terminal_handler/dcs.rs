// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! DCS (Device Control String) sub-protocol dispatch for [`TerminalHandler`].
//!
//! This module contains all functions responsible for handling DCS sequences:
//!
//! - [`TerminalHandler::handle_device_control_string`] — main entry point
//! - DECRQSS (`$ q`) — Request Selection or Setting
//! - XTGETTCAP (`+ q`) — xterm termcap/terminfo capability query
//! - tmux DCS passthrough (`tmux;`) — un-doubles ESC bytes and queues the
//!   whole inner payload for `TerminalState`, which runs it through a fresh
//!   instance of the real parser (see `state/internal.rs`)

use freminal_common::{buffer_states::modes::s8c1t::S8c1t, cursor::CursorVisualStyle};

use super::TerminalHandler;
use crate::ansi_components::tracer::{
    escape_sequence_for_log_bounded, lossy_sequence_for_log_bounded,
};

impl TerminalHandler {
    /// Handle a DCS (Device Control String) sequence.
    ///
    /// The raw `dcs` payload includes the leading `P` byte and the trailing `ESC \`
    /// string terminator.  We strip those to get the inner content, then dispatch on
    /// known DCS sub-commands:
    ///
    /// - **DECRQSS** (`$ q <Pt> ST`): Request Selection or Setting.
    /// - **XTGETTCAP** (`+ q <hex> ST`): xterm termcap/terminfo query.
    /// - **tmux passthrough** (`tmux; <inner> ST`): un-doubles ESC bytes and
    ///   queues the whole inner payload on `tmux_passthrough_queue`.
    ///   `TerminalState` drains the queue right after this output, parsing
    ///   the payload with a fresh parser and processing the result in order.
    ///
    /// A DCS whose body starts with `@kitty-` (kitty's `@kitty-cmd`,
    /// `@kitty-print` and friends) is consumed with a single debug line
    /// carrying its length only. Other unknown or unsupported DCS
    /// sub-commands are logged at warn level, without any payload bytes.
    pub fn handle_device_control_string(&mut self, dcs: &[u8]) {
        tracing::debug!("DCS received: \"{}\"", escape_sequence_for_log_bounded(dcs));
        // Strip leading 'P' and trailing ESC '\' to get inner content.
        let inner = Self::strip_dcs_envelope(dcs);

        if let Some(pt) = inner.strip_prefix(b"$q") {
            self.handle_decrqss(pt);
        } else if let Some(hex_payload) = inner.strip_prefix(b"+q") {
            self.handle_xtgettcap(hex_payload);
        } else if Self::is_sixel_sequence(inner) {
            self.handle_sixel(inner);
        } else if let Some(payload) = inner.strip_prefix(b"tmux;") {
            self.handle_tmux_passthrough(payload);
        } else if inner.starts_with(b"@kitty-") {
            // kitty's own remote-control / print DCS family. Freminal does not
            // implement it; consume it quietly, logging the length only.
            tracing::debug!("DCS @kitty- sequence ignored ({} bytes)", inner.len());
        } else {
            tracing::warn!("DCS sub-command not recognized ({} bytes)", inner.len());
        }
    }

    /// Strip the DCS envelope: leading `P` byte and trailing `ESC \` (if present).
    pub(super) fn strip_dcs_envelope(dcs: &[u8]) -> &[u8] {
        let start = usize::from(dcs.first() == Some(&b'P'));
        let end = if dcs.len() >= 2 && dcs[dcs.len() - 2] == 0x1b && dcs[dcs.len() - 1] == b'\\' {
            dcs.len() - 2
        } else {
            dcs.len()
        };
        if start <= end { &dcs[start..end] } else { &[] }
    }

    /// Un-double ESC bytes in a tmux passthrough payload.
    ///
    /// tmux DCS passthrough encodes the inner escape sequence with every `ESC`
    /// (`0x1b`) byte doubled to `ESC ESC`.  This function reverses that
    /// encoding: consecutive pairs of `0x1b` are collapsed to a single `0x1b`.
    pub(super) fn undouble_esc(data: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(data.len());
        let mut i = 0;
        while i < data.len() {
            if data[i] == 0x1b && i + 1 < data.len() && data[i + 1] == 0x1b {
                out.push(0x1b);
                i += 2;
            } else {
                out.push(data[i]);
                i += 1;
            }
        }
        out
    }

    /// Handle a tmux DCS passthrough payload.
    ///
    /// The `payload` is the content after the `tmux;` prefix, with ESC bytes
    /// still doubled.  This method un-doubles the ESC bytes and queues the
    /// **whole** inner payload (starting with its `ESC` introducer) on
    /// `tmux_passthrough_queue`, whatever kind of sequence it is.  It
    /// dispatches nothing itself: the ANSI parser lives in `TerminalState`,
    /// which takes the queue immediately after the output that produced it,
    /// parses each payload with a fresh [`FreminalAnsiParser`] and processes
    /// the result in event order.  A tmux payload is therefore handled by
    /// exactly the code that handles the same bytes sent directly.
    ///
    /// A payload that is empty, shorter than two bytes, or does not start
    /// with `ESC` after un-doubling is logged (lengths only) and dropped.
    ///
    /// [`FreminalAnsiParser`]: crate::ansi::FreminalAnsiParser
    pub(super) fn handle_tmux_passthrough(&mut self, payload: &[u8]) {
        if payload.is_empty() {
            tracing::warn!("DCS tmux passthrough: empty payload");
            return;
        }

        let inner = Self::undouble_esc(payload);

        if inner.len() < 2 || inner[0] != 0x1b {
            tracing::warn!(
                "DCS tmux passthrough: inner sequence does not start with ESC ({} bytes)",
                inner.len()
            );
            tracing::debug!(
                "DCS tmux passthrough: inner sequence: \"{}\"",
                escape_sequence_for_log_bounded(&inner)
            );
            return;
        }

        tracing::debug!(
            "DCS tmux passthrough: queuing inner sequence ({} bytes)",
            inner.len()
        );
        self.tmux_passthrough_queue.push(inner);
    }

    /// Handle DECRQSS — Request Selection or Setting.
    ///
    /// `pt` is the setting identifier after stripping the `$q` prefix:
    /// - `m`     → current SGR attributes
    /// - `r`     → current scroll region (DECSTBM)
    /// - `SP q`  → current cursor style (DECSCUSR)  (note: space + q)
    ///
    /// Response format: `DCS Ps $ r Pt ST`
    /// - `Ps = 1` for valid request, `Ps = 0` for invalid.
    fn handle_decrqss(&self, pt: &[u8]) {
        match pt {
            b"m" => {
                let sgr = self.build_sgr_response();
                self.write_dcs_response(&format!("1$r{sgr}m"));
            }
            b"r" => {
                let (top, bottom) = self.buffer.scroll_region();
                // Respond with 1-based row numbers.
                let top_1 = top + 1;
                let bottom_1 = bottom + 1;
                self.write_dcs_response(&format!("1$r{top_1};{bottom_1}r"));
            }
            // SP q = space (0x20) followed by 'q' (0x71)
            b" q" => {
                let style_num = match self.cursor_visual_style() {
                    CursorVisualStyle::BlockCursorBlink => 1,
                    CursorVisualStyle::BlockCursorSteady => 2,
                    CursorVisualStyle::UnderlineCursorBlink => 3,
                    CursorVisualStyle::UnderlineCursorSteady => 4,
                    CursorVisualStyle::VerticalLineCursorBlink => 5,
                    CursorVisualStyle::VerticalLineCursorSteady => 6,
                };
                self.write_dcs_response(&format!("1$r{style_num} q"));
            }
            // "p = DECSCL (Set Conformance Level) query.
            //
            // Response format: DCS 1 $ r Ps1 ; Ps2 " p ST
            //   Ps1 = 6x where x is the conformance level (1–5)
            //   Ps2 = C1 control mode (0 or 2 = 8-bit, 1 = 7-bit)
            //
            // Freminal advertises VT525 (DA1 first param = 65) and uses 7-bit
            // controls by default; when S8C1T is active, report 8-bit.
            b"\"p" => {
                let c1_mode = match self.s8c1t_mode {
                    S8c1t::EightBit => 0,
                    S8c1t::SevenBit => 1,
                };
                self.write_dcs_response(&format!("1$r65;{c1_mode}\"p"));
            }
            _ => {
                // Invalid / unrecognized query → DCS 0 $ r ST
                self.write_dcs_response("0$r");
                tracing::warn!("DECRQSS: unrecognized setting query ({} bytes)", pt.len());
                tracing::debug!(
                    "DECRQSS: unrecognized setting query: \"{}\"",
                    escape_sequence_for_log_bounded(pt)
                );
            }
        }
    }

    /// Handle XTGETTCAP — xterm termcap/terminfo capability query.
    ///
    /// `hex_payload` is the hex-encoded capability name(s) after stripping the `+q`
    /// prefix.  Multiple capability names may be separated by `;` in the hex payload.
    ///
    /// Response: `DCS 1 + r <hex-name> = <hex-value> ST` for known capabilities,
    ///           `DCS 0 + r <hex-name> ST` for unknown ones.
    fn handle_xtgettcap(&self, hex_payload: &[u8]) {
        tracing::debug!(
            "XTGETTCAP query: \"{}\"",
            escape_sequence_for_log_bounded(hex_payload)
        );
        let payload_str = String::from_utf8_lossy(hex_payload);

        // Split on ';' to support multiple capability queries in a single DCS.
        for hex_name in payload_str.split(';') {
            if hex_name.is_empty() {
                continue;
            }

            let Some(cap_name) = Self::hex_decode(hex_name) else {
                tracing::warn!("XTGETTCAP: invalid hex encoding in capability name");
                tracing::debug!(
                    "XTGETTCAP: invalid hex encoding: {}",
                    lossy_sequence_for_log_bounded(hex_name.as_bytes())
                );
                self.write_dcs_response(&format!("0+r{hex_name}"));
                continue;
            };

            // "u" — Kitty keyboard protocol flags.  This is instance state
            // (not a static value), so handle it before the static lookup.
            if cap_name == "u" {
                let flags = self.kitty_keyboard_flags();
                let hex_value = Self::hex_encode(&flags.to_string());
                self.write_dcs_response(&format!("1+r{hex_name}={hex_value}"));
                continue;
            }

            if let Some(value) = Self::lookup_termcap(&cap_name) {
                let hex_value = Self::hex_encode(value);
                self.write_dcs_response(&format!("1+r{hex_name}={hex_value}"));
            } else {
                tracing::debug!(
                    "XTGETTCAP: unknown capability: {}",
                    lossy_sequence_for_log_bounded(cap_name.as_bytes())
                );
                self.write_dcs_response(&format!("0+r{hex_name}"));
            }
        }
    }

    /// Decode a hex-encoded ASCII string (e.g., "524742" → "RGB").
    pub(super) fn hex_decode(hex: &str) -> Option<String> {
        let bytes = hex.as_bytes();
        if !bytes.len().is_multiple_of(2) {
            return None;
        }
        let mut result = Vec::with_capacity(bytes.len() / 2);
        let mut i = 0;
        while i < bytes.len() {
            let hi = Self::hex_nibble(bytes[i])?;
            let lo = Self::hex_nibble(bytes[i + 1])?;
            result.push((hi << 4) | lo);
            i += 2;
        }
        String::from_utf8(result).ok()
    }

    /// Encode an ASCII string as hex (e.g., "1" → "31").
    pub(super) fn hex_encode(s: &str) -> String {
        let mut result = String::with_capacity(s.len() * 2);
        for b in s.bytes() {
            result.push(Self::nibble_to_hex(b >> 4));
            result.push(Self::nibble_to_hex(b & 0x0F));
        }
        result
    }

    /// Convert a single ASCII hex character to its numeric value.
    const fn hex_nibble(b: u8) -> Option<u8> {
        match b {
            b'0'..=b'9' => Some(b - b'0'),
            b'a'..=b'f' => Some(b - b'a' + 10),
            b'A'..=b'F' => Some(b - b'A' + 10),
            _ => None,
        }
    }

    /// Convert a 4-bit nibble to an uppercase hex character.
    const fn nibble_to_hex(n: u8) -> char {
        match n {
            0..=9 => (b'0' + n) as char,
            _ => (b'A' + n - 10) as char,
        }
    }

    /// Look up a termcap/terminfo capability by decoded name.
    ///
    /// Returns `Some(value_str)` for known capabilities, `None` for unknown ones.
    /// The returned string is the raw value (not yet hex-encoded).
    fn lookup_termcap(name: &str) -> Option<&'static str> {
        match name {
            // RGB — terminal supports direct-color (24-bit) via SGR 38/48;2;R;G;B
            "RGB" => Some("8/8/8"),
            // Tc — tmux extension: true color support
            // ut — terminal uses background color erase (BCE)
            // Su — boolean: terminal supports styled (extended) underlines.
            // Advertised by kitty, WezTerm, foot. nvim checks this to enable
            // underline color support.
            "Tc" | "ut" | "Su" => Some(""),
            // setrgbf — SGR sequence to set RGB foreground
            "setrgbf" => Some("\x1b[38;2;%p1%d;%p2%d;%p3%dm"),
            // setrgbb — SGR sequence to set RGB background
            "setrgbb" => Some("\x1b[48;2;%p1%d;%p2%d;%p3%dm"),
            // colors — number of colors supported
            "colors" | "Co" => Some("256"),
            // TN — terminal name.  This MUST match the runtime `TERM` value
            // (`xterm-256color`, set in `io::pty::run_terminal`), since TN is
            // defined as "the value of $TERM".  Freminal deliberately presents
            // as `xterm-256color` for maximum compatibility (the Task-12
            // strategy mirroring WezTerm/Alacritty); the freminal identity is
            // advertised separately via `TERM_PROGRAM=freminal` (Task 72.6)
            // and the XTVERSION `>|XTerm(Freminal ...)` payload.  Do not change
            // TN to `freminal` — it would desync from TERM and break terminfo
            // lookups.
            "TN" => Some("xterm-256color"),
            // Ms — set selection (clipboard) via OSC 52
            "Ms" => Some("\x1b]52;%p1%s;%p2%s\x1b\\"),
            // Se — reset cursor to default style (DECSCUSR 0)
            "Se" => Some("\x1b[2 q"),
            // Ss — set cursor style (DECSCUSR)
            "Ss" => Some("\x1b[%p1%d q"),
            // Smulx — extended underline (SGR 4:N for curly, dotted, etc.)
            "Smulx" => Some("\x1b[4:%p1%dm"),
            // Setulc — set underline color (colon sub-parameter syntax per
            // ITU T.416, single packed-integer RGB like kitty/WezTerm).
            "Setulc" => Some("\x1b[58:2::%p1%{65536}%/%d:%p1%{256}%/%{255}%&%d:%p1%{255}%&%d%;m"),
            // khome — Home key
            "khome" => Some("\x1bOH"),
            // kend — End key
            "kend" => Some("\x1bOF"),
            // kHOM — Shift+Home
            "kHOM" => Some("\x1b[1;2H"),
            // kEND — Shift+End
            "kEND" => Some("\x1b[1;2F"),
            // smkx — enter keypad transmit (application) mode
            "smkx" => Some("\x1b[?1h\x1b="),
            // rmkx — exit keypad transmit mode (back to numeric)
            "rmkx" => Some("\x1b[?1l\x1b>"),
            _ => None,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use freminal_common::{
        buffer_states::{modes::s8c1t::S8c1t, terminal_output::TerminalOutput},
        colors::TerminalColor,
        cursor::CursorVisualStyle,
        pty_write::PtyWrite,
        sgr::SelectGraphicRendition,
    };

    use super::TerminalHandler;
    use crate::log_capture::{Captured, capture, warnings};
    use crate::state::internal::TerminalState;
    use tracing::Level;

    // ------------------------------------------------------------------
    // DECRQSS tests (DCS $ q ... ST)
    // ------------------------------------------------------------------

    /// Helper: build a raw DCS payload as the standard parser would produce.
    /// Format: `P` + content + `ESC \`
    fn build_dcs_payload(content: &[u8]) -> Vec<u8> {
        let mut v = vec![b'P'];
        v.extend_from_slice(content);
        v.extend_from_slice(b"\x1b\\");
        v
    }

    /// Helper: receive the PTY write-back response from a DECRQSS query.
    fn recv_pty_response(rx: &crossbeam_channel::Receiver<PtyWrite>) -> String {
        let Ok(PtyWrite::Write(bytes)) = rx.try_recv() else {
            panic!("expected PtyWrite::Write response from DCS query");
        };
        let Ok(s) = String::from_utf8(bytes) else {
            panic!("DCS response should be valid UTF-8");
        };
        s
    }

    #[test]
    fn decrqss_sgr_default_attributes() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        let dcs = build_dcs_payload(b"$qm");
        handler.handle_device_control_string(&dcs);

        let response = recv_pty_response(&rx);
        // Default state: just "0" (reset)
        assert_eq!(response, "\x1bP1$r0m\x1b\\");
    }

    #[test]
    fn decrqss_sgr_bold_and_italic() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // Apply bold + italic
        handler.process_outputs(&[TerminalOutput::Sgr(SelectGraphicRendition::Bold)]);
        handler.process_outputs(&[TerminalOutput::Sgr(SelectGraphicRendition::Italic)]);

        let dcs = build_dcs_payload(b"$qm");
        handler.handle_device_control_string(&dcs);

        let response = recv_pty_response(&rx);
        assert_eq!(response, "\x1bP1$r0;1;3m\x1b\\");
    }

    #[test]
    fn decrqss_sgr_with_fg_color() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        handler.process_outputs(&[TerminalOutput::Sgr(SelectGraphicRendition::Foreground(
            TerminalColor::Red,
        ))]);

        let dcs = build_dcs_payload(b"$qm");
        handler.handle_device_control_string(&dcs);

        let response = recv_pty_response(&rx);
        assert_eq!(response, "\x1bP1$r0;31m\x1b\\");
    }

    #[test]
    fn decrqss_sgr_with_truecolor() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        handler.process_outputs(&[TerminalOutput::Sgr(SelectGraphicRendition::Foreground(
            TerminalColor::Custom(255, 128, 0),
        ))]);

        let dcs = build_dcs_payload(b"$qm");
        handler.handle_device_control_string(&dcs);

        let response = recv_pty_response(&rx);
        assert_eq!(response, "\x1bP1$r0;38;2;255;128;0m\x1b\\");
    }

    #[test]
    fn decrqss_sgr_reverse_video() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        handler.process_outputs(&[TerminalOutput::Sgr(SelectGraphicRendition::ReverseVideo)]);

        let dcs = build_dcs_payload(b"$qm");
        handler.handle_device_control_string(&dcs);

        let response = recv_pty_response(&rx);
        assert_eq!(response, "\x1bP1$r0;7m\x1b\\");
    }

    #[test]
    fn decrqss_decstbm_default_scroll_region() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        let dcs = build_dcs_payload(b"$qr");
        handler.handle_device_control_string(&dcs);

        let response = recv_pty_response(&rx);
        // Default scroll region: full screen [0, 23] → 1-based [1, 24]
        assert_eq!(response, "\x1bP1$r1;24r\x1b\\");
    }

    #[test]
    fn decrqss_decstbm_custom_scroll_region() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // Set scroll region to 1-based rows 5-20
        handler.handle_set_scroll_region(5, 20);

        let dcs = build_dcs_payload(b"$qr");
        handler.handle_device_control_string(&dcs);

        let response = recv_pty_response(&rx);
        assert_eq!(response, "\x1bP1$r5;20r\x1b\\");
    }

    #[test]
    fn decrqss_decscusr_default_cursor_style() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // Note: space + q = DECSCUSR query
        let dcs = build_dcs_payload(b"$q q");
        handler.handle_device_control_string(&dcs);

        let response = recv_pty_response(&rx);
        // Default is BlockCursorSteady = 2
        assert_eq!(response, "\x1bP1$r2 q\x1b\\");
    }

    #[test]
    fn decrqss_decscusr_after_style_change() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        handler.process_outputs(&[TerminalOutput::CursorVisualStyle(
            CursorVisualStyle::UnderlineCursorBlink,
        )]);

        let dcs = build_dcs_payload(b"$q q");
        handler.handle_device_control_string(&dcs);

        let response = recv_pty_response(&rx);
        assert_eq!(response, "\x1bP1$r3 q\x1b\\");
    }

    #[test]
    fn decrqss_decscl_conformance_level() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // "p = DECSCL (Set Conformance Level) query
        let dcs = build_dcs_payload(b"$q\"p");
        handler.handle_device_control_string(&dcs);

        let response = recv_pty_response(&rx);
        // Freminal claims VT525 (level 5) with 7-bit C1 controls (Ps2=1)
        // Response format: DCS 1 $ r 65 ; 1 " p ST
        assert_eq!(response, "\x1bP1$r65;1\"p\x1b\\");
    }

    #[test]
    fn decrqss_invalid_query() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        let dcs = build_dcs_payload(b"$qZ");
        handler.handle_device_control_string(&dcs);

        let response = recv_pty_response(&rx);
        // Invalid query → DCS 0 $ r ST
        assert_eq!(response, "\x1bP0$r\x1b\\");
    }

    #[test]
    fn dcs_unknown_subcommand_does_not_panic() {
        let mut handler = TerminalHandler::new(80, 24);

        // No write_tx set — should not panic even on unknown DCS
        let dcs = build_dcs_payload(b"!zsome_data");
        handler.handle_device_control_string(&dcs);
        // Success = no panic
    }

    #[test]
    fn strip_dcs_envelope_handles_minimal_payload() {
        // Just "P" + ESC '\' — inner content is empty
        let dcs = b"P\x1b\\";
        let inner = TerminalHandler::strip_dcs_envelope(dcs);
        assert_eq!(inner, []);
    }

    #[test]
    fn strip_dcs_envelope_preserves_content() {
        let dcs = b"P$qm\x1b\\";
        let inner = TerminalHandler::strip_dcs_envelope(dcs);
        assert_eq!(inner, b"$qm");
    }

    // ── undouble_esc tests ────────────────────────────────────────────────

    #[test]
    fn undouble_esc_no_esc_bytes() {
        let data = b"hello world";
        let result = TerminalHandler::undouble_esc(data);
        assert_eq!(result, b"hello world");
    }

    #[test]
    fn undouble_esc_single_pair() {
        // ESC ESC → ESC
        let data = b"\x1b\x1b";
        let result = TerminalHandler::undouble_esc(data);
        assert_eq!(result, b"\x1b");
    }

    #[test]
    fn undouble_esc_multiple_pairs() {
        // Two doubled pairs with content between
        let data = b"\x1b\x1b_G\x1b\x1b\\";
        let result = TerminalHandler::undouble_esc(data);
        assert_eq!(result, b"\x1b_G\x1b\\");
    }

    #[test]
    fn undouble_esc_lone_esc_at_end() {
        // A single ESC at the end (not doubled) stays as-is
        let data = b"abc\x1b";
        let result = TerminalHandler::undouble_esc(data);
        assert_eq!(result, b"abc\x1b");
    }

    #[test]
    fn undouble_esc_empty() {
        let result = TerminalHandler::undouble_esc(b"");
        assert_eq!(result, []);
    }

    #[test]
    fn undouble_esc_triple_esc() {
        // Three consecutive ESC bytes: first two form a pair → ESC, the third
        // remains as a lone ESC.
        let data = b"\x1b\x1b\x1b";
        let result = TerminalHandler::undouble_esc(data);
        assert_eq!(result, b"\x1b\x1b");
    }

    // ── tmux passthrough: handler side queues the whole inner payload ─────

    /// Wrap `inner` as the DCS body the handler sees: `P tmux; <doubled> ESC \`.
    fn tmux_dcs(inner: &[u8]) -> Vec<u8> {
        let mut dcs = b"Ptmux;".to_vec();
        for &b in inner {
            if b == 0x1b {
                dcs.push(0x1b);
            }
            dcs.push(b);
        }
        dcs.extend_from_slice(b"\x1b\\");
        dcs
    }

    #[test]
    fn tmux_passthrough_empty_payload_pushes_nothing() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_tmux_passthrough(b"");
        assert_eq!(handler.take_tmux_passthrough_queue(), Vec::<Vec<u8>>::new());
    }

    #[test]
    fn tmux_passthrough_no_esc_prefix_pushes_nothing() {
        let mut handler = TerminalHandler::new(80, 24);
        // Payload that does not start with ESC after un-doubling.
        handler.handle_tmux_passthrough(b"junk data");
        assert_eq!(handler.take_tmux_passthrough_queue(), Vec::<Vec<u8>>::new());
    }

    #[test]
    fn tmux_passthrough_too_short_pushes_nothing() {
        let mut handler = TerminalHandler::new(80, 24);
        // A doubled ESC with no type byte: un-doubled it is one byte.
        handler.handle_tmux_passthrough(b"\x1b\x1b");
        assert_eq!(handler.take_tmux_passthrough_queue(), Vec::<Vec<u8>>::new());
        // A lone, undoubled ESC is one byte as well.
        handler.handle_tmux_passthrough(b"\x1b");
        assert_eq!(handler.take_tmux_passthrough_queue(), Vec::<Vec<u8>>::new());
    }

    #[test]
    fn tmux_passthrough_queues_apc_payload_whole() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_tmux_passthrough(b"\x1b\x1b_Ga=q,i=1;\x1b\x1b\\");
        assert_eq!(
            handler.take_tmux_passthrough_queue(),
            vec![b"\x1b_Ga=q,i=1;\x1b\\".to_vec()]
        );
    }

    #[test]
    fn tmux_passthrough_queues_dcs_payload_whole() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_tmux_passthrough(b"\x1b\x1bP$qm\x1b\x1b\\");
        assert_eq!(
            handler.take_tmux_passthrough_queue(),
            vec![b"\x1bP$qm\x1b\\".to_vec()]
        );
    }

    #[test]
    fn tmux_passthrough_queues_osc_payload_whole() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_tmux_passthrough(b"\x1b\x1b]0;title\x1b\x1b\\");
        assert_eq!(
            handler.take_tmux_passthrough_queue(),
            vec![b"\x1b]0;title\x1b\\".to_vec()]
        );
    }

    #[test]
    fn tmux_passthrough_queues_csi_payload_whole_and_does_not_run_it() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_tmux_passthrough(b"\x1b\x1b[5;10H");
        assert_eq!(
            handler.take_tmux_passthrough_queue(),
            vec![b"\x1b[5;10H".to_vec()]
        );
        // The handler itself no longer executes inner CSI.
        let cursor = handler.buffer.cursor().pos;
        assert_eq!((cursor.x, cursor.y), (0, 0));
    }

    #[test]
    fn tmux_passthrough_queues_any_introducer() {
        // The introducer no longer matters to the handler: the real parser
        // decides what `ESC Z` is.
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_tmux_passthrough(b"\x1b\x1bZ");
        assert_eq!(
            handler.take_tmux_passthrough_queue(),
            vec![b"\x1bZ".to_vec()]
        );
    }

    #[test]
    fn tmux_passthrough_queue_preserves_arrival_order_and_is_taken_once() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_device_control_string(&tmux_dcs(b"\x1b[1m"));
        handler.handle_device_control_string(&tmux_dcs(b"\x1b[2m"));
        assert_eq!(
            handler.take_tmux_passthrough_queue(),
            vec![b"\x1b[1m".to_vec(), b"\x1b[2m".to_vec()]
        );
        assert_eq!(handler.take_tmux_passthrough_queue(), Vec::<Vec<u8>>::new());
    }

    #[test]
    fn tmux_passthrough_via_full_dcs_handler_queues_without_replying() {
        // The normal entry point: the handler queues the payload and writes
        // nothing; replies come from the real parser's processing of it.
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        handler.handle_device_control_string(&tmux_dcs(b"\x1b_Ga=q,i=1;\x1b\\"));

        assert!(rx.try_recv().is_err(), "the handler must not reply itself");
        assert_eq!(
            handler.take_tmux_passthrough_queue(),
            vec![b"\x1b_Ga=q,i=1;\x1b\\".to_vec()]
        );
    }

    #[test]
    fn tmux_passthrough_early_return_warns_without_payload() {
        let events = capture(|| {
            let mut handler = TerminalHandler::new(80, 24);
            handler.handle_tmux_passthrough(b"SECRETPAYLOAD");
        });
        let warns = warnings(&events);
        assert!(!warns.is_empty(), "expected a warn, got: {events:?}");
        for (level, text) in warns {
            assert!(
                !text.contains("SECRETPAYLOAD"),
                "{level} line leaked payload: {text}"
            );
        }
    }

    // ── XTGETTCAP tests ──────────────────────────────────────────────────

    #[test]
    fn xtgettcap_hex_decode_rgb() {
        // "RGB" = 0x52 0x47 0x42 → "524742"
        let decoded = TerminalHandler::hex_decode("524742");
        assert_eq!(decoded.as_deref(), Some("RGB"));
    }

    #[test]
    fn xtgettcap_hex_decode_lowercase() {
        // "Ms" = 0x4D 0x73 → uppercase hex "4D73", lowercase "4d73"
        // 'd' is a hex letter that differs between cases — a good test for
        // case-insensitive parsing.
        let decoded_upper = TerminalHandler::hex_decode("4D73");
        assert_eq!(decoded_upper.as_deref(), Some("Ms"));

        let decoded_lower = TerminalHandler::hex_decode("4d73");
        assert_eq!(decoded_lower.as_deref(), Some("Ms"));
    }

    #[test]
    fn xtgettcap_hex_decode_odd_length_fails() {
        // Odd-length hex string is invalid
        assert!(TerminalHandler::hex_decode("52474").is_none());
    }

    #[test]
    fn xtgettcap_hex_encode_roundtrip() {
        let original = "RGB";
        let encoded = TerminalHandler::hex_encode(original);
        assert_eq!(encoded, "524742");
        let decoded = TerminalHandler::hex_decode(&encoded);
        assert_eq!(decoded.as_deref(), Some(original));
    }

    #[test]
    fn xtgettcap_known_capability_rgb() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // Query "RGB" → hex "524742"
        let dcs = build_dcs_payload(b"+q524742");
        handler.handle_device_control_string(&dcs);

        let response = recv_pty_response(&rx);
        // "8/8/8" → hex "382F382F38"
        assert_eq!(response, "\x1bP1+r524742=382F382F38\x1b\\");
    }

    #[test]
    fn xtgettcap_known_capability_colors() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // Query "colors" → hex "636F6C6F7273"
        let dcs = build_dcs_payload(b"+q636F6C6F7273");
        handler.handle_device_control_string(&dcs);

        let response = recv_pty_response(&rx);
        // "256" → hex "323536"
        assert_eq!(response, "\x1bP1+r636F6C6F7273=323536\x1b\\");
    }

    #[test]
    fn xtgettcap_known_capability_tn() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // Query "TN" → hex "544E"
        let dcs = build_dcs_payload(b"+q544E");
        handler.handle_device_control_string(&dcs);

        let response = recv_pty_response(&rx);
        // Regression guard (Task 76.6): TN must report `xterm-256color`, the
        // same value as the runtime `TERM` env var.  TN is "the value of
        // $TERM"; reporting `freminal` would desync from TERM and break
        // terminfo lookups.  Freminal advertises its identity via
        // TERM_PROGRAM and XTVERSION instead.
        let expected_hex = TerminalHandler::hex_encode("xterm-256color");
        assert_eq!(response, format!("\x1bP1+r544E={expected_hex}\x1b\\"));
    }

    #[test]
    fn xtgettcap_unknown_capability() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // Query "UNKN" → hex "554E4B4E"
        let dcs = build_dcs_payload(b"+q554E4B4E");
        handler.handle_device_control_string(&dcs);

        let response = recv_pty_response(&rx);
        assert_eq!(response, "\x1bP0+r554E4B4E\x1b\\");
    }

    #[test]
    fn xtgettcap_multiple_capabilities() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // Query "RGB" and "TN" separated by ';'
        // "RGB" = 524742, "TN" = 544E
        let dcs = build_dcs_payload(b"+q524742;544E");
        handler.handle_device_control_string(&dcs);

        // Should get two separate responses
        let response1 = recv_pty_response(&rx);
        assert_eq!(response1, "\x1bP1+r524742=382F382F38\x1b\\");

        let response2 = recv_pty_response(&rx);
        let tn_hex = TerminalHandler::hex_encode("xterm-256color");
        assert_eq!(response2, format!("\x1bP1+r544E={tn_hex}\x1b\\"));
    }

    #[test]
    fn xtgettcap_known_capability_tc() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // Query "Tc" → hex "5463"
        let dcs = build_dcs_payload(b"+q5463");
        handler.handle_device_control_string(&dcs);

        let response = recv_pty_response(&rx);
        // "Tc" has empty value, so hex-encoded value is ""
        assert_eq!(response, "\x1bP1+r5463=\x1b\\");
    }

    #[test]
    fn xtgettcap_known_capability_se() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // Query "Se" → hex "5365"
        let dcs = build_dcs_payload(b"+q5365");
        handler.handle_device_control_string(&dcs);

        let response = recv_pty_response(&rx);
        // "\x1b[2 q" → hex "1B5B322071"
        let expected_hex = TerminalHandler::hex_encode("\x1b[2 q");
        assert_eq!(response, format!("\x1bP1+r5365={expected_hex}\x1b\\"));
    }

    #[test]
    fn xtgettcap_known_capability_setrgbf() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // Query "setrgbf" → hex encode each byte
        let hex_name = TerminalHandler::hex_encode("setrgbf");
        let mut payload = Vec::new();
        payload.extend_from_slice(b"+q");
        payload.extend_from_slice(hex_name.as_bytes());
        let dcs = build_dcs_payload(&payload);
        handler.handle_device_control_string(&dcs);

        let response = recv_pty_response(&rx);
        let expected_val_hex = TerminalHandler::hex_encode("\x1b[38;2;%p1%d;%p2%d;%p3%dm");
        assert_eq!(
            response,
            format!("\x1bP1+r{hex_name}={expected_val_hex}\x1b\\")
        );
    }

    #[test]
    fn xtgettcap_known_capability_setrgbb() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        let hex_name = TerminalHandler::hex_encode("setrgbb");
        let mut payload = Vec::new();
        payload.extend_from_slice(b"+q");
        payload.extend_from_slice(hex_name.as_bytes());
        let dcs = build_dcs_payload(&payload);
        handler.handle_device_control_string(&dcs);

        let response = recv_pty_response(&rx);
        let expected_val_hex = TerminalHandler::hex_encode("\x1b[48;2;%p1%d;%p2%d;%p3%dm");
        assert_eq!(
            response,
            format!("\x1bP1+r{hex_name}={expected_val_hex}\x1b\\")
        );
    }

    #[test]
    fn xtgettcap_known_capability_co_alias() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // "Co" is an alias for "colors"; both should return "256"
        // "Co" = 0x43 0x6F → hex "436F"
        let dcs = build_dcs_payload(b"+q436F");
        handler.handle_device_control_string(&dcs);

        let response = recv_pty_response(&rx);
        // "256" → hex "323536"
        assert_eq!(response, "\x1bP1+r436F=323536\x1b\\");
    }

    #[test]
    fn xtgettcap_known_capability_ms() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        let hex_name = TerminalHandler::hex_encode("Ms");
        let mut payload = Vec::new();
        payload.extend_from_slice(b"+q");
        payload.extend_from_slice(hex_name.as_bytes());
        let dcs = build_dcs_payload(&payload);
        handler.handle_device_control_string(&dcs);

        let response = recv_pty_response(&rx);
        let expected_val_hex = TerminalHandler::hex_encode("\x1b]52;%p1%s;%p2%s\x1b\\");
        assert_eq!(
            response,
            format!("\x1bP1+r{hex_name}={expected_val_hex}\x1b\\")
        );
    }

    #[test]
    fn xtgettcap_known_capability_ss() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        let hex_name = TerminalHandler::hex_encode("Ss");
        let mut payload = Vec::new();
        payload.extend_from_slice(b"+q");
        payload.extend_from_slice(hex_name.as_bytes());
        let dcs = build_dcs_payload(&payload);
        handler.handle_device_control_string(&dcs);

        let response = recv_pty_response(&rx);
        let expected_val_hex = TerminalHandler::hex_encode("\x1b[%p1%d q");
        assert_eq!(
            response,
            format!("\x1bP1+r{hex_name}={expected_val_hex}\x1b\\")
        );
    }

    #[test]
    fn xtgettcap_known_capability_smulx() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        let hex_name = TerminalHandler::hex_encode("Smulx");
        let mut payload = Vec::new();
        payload.extend_from_slice(b"+q");
        payload.extend_from_slice(hex_name.as_bytes());
        let dcs = build_dcs_payload(&payload);
        handler.handle_device_control_string(&dcs);

        let response = recv_pty_response(&rx);
        let expected_val_hex = TerminalHandler::hex_encode("\x1b[4:%p1%dm");
        assert_eq!(
            response,
            format!("\x1bP1+r{hex_name}={expected_val_hex}\x1b\\")
        );
    }

    #[test]
    fn xtgettcap_known_capability_setulc() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        let hex_name = TerminalHandler::hex_encode("Setulc");
        let mut payload = Vec::new();
        payload.extend_from_slice(b"+q");
        payload.extend_from_slice(hex_name.as_bytes());
        let dcs = build_dcs_payload(&payload);
        handler.handle_device_control_string(&dcs);

        let response = recv_pty_response(&rx);
        let expected_val_hex = TerminalHandler::hex_encode(
            "\x1b[58:2::%p1%{65536}%/%d:%p1%{256}%/%{255}%&%d:%p1%{255}%&%d%;m",
        );
        assert_eq!(
            response,
            format!("\x1bP1+r{hex_name}={expected_val_hex}\x1b\\")
        );
    }

    #[test]
    fn xtgettcap_known_capability_su() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        let hex_name = TerminalHandler::hex_encode("Su");
        let mut payload = Vec::new();
        payload.extend_from_slice(b"+q");
        payload.extend_from_slice(hex_name.as_bytes());
        let dcs = build_dcs_payload(&payload);
        handler.handle_device_control_string(&dcs);

        let response = recv_pty_response(&rx);
        // Su is a boolean capability — empty value string.
        let expected_val_hex = TerminalHandler::hex_encode("");
        assert_eq!(
            response,
            format!("\x1bP1+r{hex_name}={expected_val_hex}\x1b\\")
        );
    }

    #[test]
    fn xtgettcap_known_capability_khome() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // "khome" → hex encode name, expect CSI H response (\x1b[H)
        let hex_name = TerminalHandler::hex_encode("khome");
        let mut payload = Vec::new();
        payload.extend_from_slice(b"+q");
        payload.extend_from_slice(hex_name.as_bytes());
        let dcs = build_dcs_payload(&payload);
        handler.handle_device_control_string(&dcs);

        let response = recv_pty_response(&rx);
        // SS3 H = \x1bOH — the sequence Freminal sends for Home in DECCKM Application mode
        let expected_val_hex = TerminalHandler::hex_encode("\x1bOH");
        assert_eq!(
            response,
            format!("\x1bP1+r{hex_name}={expected_val_hex}\x1b\\")
        );
    }

    #[test]
    fn xtgettcap_known_capability_kend() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // "kend" → hex encode name, expect SS3 F response (\x1bOF)
        let hex_name = TerminalHandler::hex_encode("kend");
        let mut payload = Vec::new();
        payload.extend_from_slice(b"+q");
        payload.extend_from_slice(hex_name.as_bytes());
        let dcs = build_dcs_payload(&payload);
        handler.handle_device_control_string(&dcs);

        let response = recv_pty_response(&rx);
        // SS3 F = \x1bOF — the sequence Freminal sends for End in DECCKM Application mode
        let expected_val_hex = TerminalHandler::hex_encode("\x1bOF");
        assert_eq!(
            response,
            format!("\x1bP1+r{hex_name}={expected_val_hex}\x1b\\")
        );
    }

    #[test]
    fn xtgettcap_known_capability_khom_shift() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // "kHOM" (Shift+Home) → expect \x1b[1;2H
        let hex_name = TerminalHandler::hex_encode("kHOM");
        let mut payload = Vec::new();
        payload.extend_from_slice(b"+q");
        payload.extend_from_slice(hex_name.as_bytes());
        let dcs = build_dcs_payload(&payload);
        handler.handle_device_control_string(&dcs);

        let response = recv_pty_response(&rx);
        let expected_val_hex = TerminalHandler::hex_encode("\x1b[1;2H");
        assert_eq!(
            response,
            format!("\x1bP1+r{hex_name}={expected_val_hex}\x1b\\")
        );
    }

    #[test]
    fn xtgettcap_known_capability_kend_shift() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // "kEND" (Shift+End) → expect \x1b[1;2F
        let hex_name = TerminalHandler::hex_encode("kEND");
        let mut payload = Vec::new();
        payload.extend_from_slice(b"+q");
        payload.extend_from_slice(hex_name.as_bytes());
        let dcs = build_dcs_payload(&payload);
        handler.handle_device_control_string(&dcs);

        let response = recv_pty_response(&rx);
        let expected_val_hex = TerminalHandler::hex_encode("\x1b[1;2F");
        assert_eq!(
            response,
            format!("\x1bP1+r{hex_name}={expected_val_hex}\x1b\\")
        );
    }

    // ------------------------------------------------------------------
    // DECRQSS — cursor style and DECSCL queries
    // ------------------------------------------------------------------

    #[test]
    fn decrqss_cursor_style() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // Query cursor style: DCS $q SP q ST
        let dcs = build_dcs_payload(b"$q q");
        handler.handle_device_control_string(&dcs);

        let response = recv_pty_response(&rx);
        // Default cursor is BlockCursorSteady = 2
        assert_eq!(response, "\x1bP1$r2 q\x1b\\");
    }

    #[test]
    fn decrqss_cursor_style_underline() {
        use freminal_common::cursor::CursorVisualStyle;
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        handler.cursor_visual_style = CursorVisualStyle::UnderlineCursorSteady;

        let dcs = build_dcs_payload(b"$q q");
        handler.handle_device_control_string(&dcs);

        let response = recv_pty_response(&rx);
        assert_eq!(response, "\x1bP1$r4 q\x1b\\");
    }

    #[test]
    fn decrqss_cursor_style_vertical_line() {
        use freminal_common::cursor::CursorVisualStyle;
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        handler.cursor_visual_style = CursorVisualStyle::VerticalLineCursorBlink;

        let dcs = build_dcs_payload(b"$q q");
        handler.handle_device_control_string(&dcs);

        let response = recv_pty_response(&rx);
        assert_eq!(response, "\x1bP1$r5 q\x1b\\");
    }

    #[test]
    fn decrqss_decscl() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // Default is 7-bit → c1_mode = 1
        let dcs = build_dcs_payload(b"$q\"p");
        handler.handle_device_control_string(&dcs);

        let response = recv_pty_response(&rx);
        assert_eq!(response, "\x1bP1$r65;1\"p\x1b\\");
    }

    #[test]
    fn decrqss_decscl_eight_bit() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        handler.set_s8c1t_mode(S8c1t::EightBit);

        let dcs = build_dcs_payload(b"$q\"p");
        handler.handle_device_control_string(&dcs);

        let Ok(PtyWrite::Write(bytes)) = rx.try_recv() else {
            panic!("expected PtyWrite::Write response");
        };
        // 8-bit mode: DCS = 0x90, ST = 0x9C
        // Body: "1$r65;0\"p"
        let mut expected = Vec::new();
        expected.push(0x90);
        expected.extend_from_slice(b"1$r65;0\"p");
        expected.push(0x9C);
        assert_eq!(bytes, expected);
    }

    #[test]
    fn decrqss_invalid_query_unknown_setting() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // Query something unknown ("z" is not a recognized setting)
        let dcs = build_dcs_payload(b"$qz");
        handler.handle_device_control_string(&dcs);

        let response = recv_pty_response(&rx);
        assert_eq!(response, "\x1bP0$r\x1b\\");
    }

    // ------------------------------------------------------------------
    // XTGETTCAP — remaining capabilities
    // ------------------------------------------------------------------

    #[test]
    fn xtgettcap_known_capability_smkx() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        let hex_name = TerminalHandler::hex_encode("smkx");
        let mut payload = Vec::new();
        payload.extend_from_slice(b"+q");
        payload.extend_from_slice(hex_name.as_bytes());
        let dcs = build_dcs_payload(&payload);
        handler.handle_device_control_string(&dcs);

        let response = recv_pty_response(&rx);
        let expected_val_hex = TerminalHandler::hex_encode("\x1b[?1h\x1b=");
        assert_eq!(
            response,
            format!("\x1bP1+r{hex_name}={expected_val_hex}\x1b\\")
        );
    }

    #[test]
    fn xtgettcap_known_capability_rmkx() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        let hex_name = TerminalHandler::hex_encode("rmkx");
        let mut payload = Vec::new();
        payload.extend_from_slice(b"+q");
        payload.extend_from_slice(hex_name.as_bytes());
        let dcs = build_dcs_payload(&payload);
        handler.handle_device_control_string(&dcs);

        let response = recv_pty_response(&rx);
        let expected_val_hex = TerminalHandler::hex_encode("\x1b[?1l\x1b>");
        assert_eq!(
            response,
            format!("\x1bP1+r{hex_name}={expected_val_hex}\x1b\\")
        );
    }

    #[test]
    fn xtgettcap_known_capability_ut() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        let hex_name = TerminalHandler::hex_encode("ut");
        let mut payload = Vec::new();
        payload.extend_from_slice(b"+q");
        payload.extend_from_slice(hex_name.as_bytes());
        let dcs = build_dcs_payload(&payload);
        handler.handle_device_control_string(&dcs);

        let response = recv_pty_response(&rx);
        let expected_val_hex = TerminalHandler::hex_encode("");
        assert_eq!(
            response,
            format!("\x1bP1+r{hex_name}={expected_val_hex}\x1b\\")
        );
    }

    #[test]
    fn xtgettcap_invalid_hex_encoding() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // "ZZ" is valid hex (but 'Z' is an invalid hex nibble)
        let dcs = build_dcs_payload(b"+qZZ");
        handler.handle_device_control_string(&dcs);

        let response = recv_pty_response(&rx);
        assert_eq!(response, "\x1bP0+rZZ\x1b\\");
    }

    #[test]
    fn xtgettcap_empty_segment_skipped() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // ";524742" — first segment is empty (skipped), second is "RGB"
        let dcs = build_dcs_payload(b"+q;524742");
        handler.handle_device_control_string(&dcs);

        let response = recv_pty_response(&rx);
        // Only one response (for RGB), empty segment skipped
        assert_eq!(response, "\x1bP1+r524742=382F382F38\x1b\\");
    }

    #[test]
    fn xtgettcap_kitty_keyboard_u_capability() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // "u" → hex "75"
        let dcs = build_dcs_payload(b"+q75");
        handler.handle_device_control_string(&dcs);

        let response = recv_pty_response(&rx);
        // Default kitty keyboard flags = 0 → "0" → hex "30"
        assert_eq!(response, "\x1bP1+r75=30\x1b\\");
    }

    // ------------------------------------------------------------------
    // hex_nibble edge case
    // ------------------------------------------------------------------

    #[test]
    fn hex_nibble_invalid_returns_none() {
        assert!(TerminalHandler::hex_nibble(b'G').is_none());
        assert!(TerminalHandler::hex_nibble(b'z').is_none());
        assert!(TerminalHandler::hex_nibble(b' ').is_none());
    }

    // ------------------------------------------------------------------
    // strip_dcs_envelope edge cases
    // ------------------------------------------------------------------

    #[test]
    fn strip_dcs_envelope_no_p_prefix() {
        // Input without 'P' prefix: just "hello\x1b\\"
        let input = b"hello\x1b\\";
        let stripped = TerminalHandler::strip_dcs_envelope(input);
        assert_eq!(stripped, b"hello");
    }

    #[test]
    fn strip_dcs_envelope_no_st_suffix() {
        // Input with 'P' prefix but no ST suffix
        let input = b"Phello";
        let stripped = TerminalHandler::strip_dcs_envelope(input);
        assert_eq!(stripped, b"hello");
    }

    // ------------------------------------------------------------------
    // DCS dispatch — unrecognized sub-command
    // ------------------------------------------------------------------

    #[test]
    fn dcs_unrecognized_subcommand() {
        let mut handler = TerminalHandler::new(80, 24);
        // DCS with unknown prefix (not $q, +q, sixel, or tmux;)
        let dcs = build_dcs_payload(b"UNKNOWN");
        handler.handle_device_control_string(&dcs);
        // Should just log a warning and not panic
    }

    // ------------------------------------------------------------------
    // Payload-free warn logging (129.14)
    // ------------------------------------------------------------------

    /// Feed `bytes` through the whole parser + handler pipeline and return
    /// every log event emitted on this thread while doing so.
    fn feed_and_capture(bytes: &[u8]) -> Vec<Captured> {
        capture(|| {
            let mut state = TerminalState::default();
            state.handle_incoming_data(bytes);
        })
    }

    #[test]
    fn unknown_dcs_warns_without_payload() {
        let events = feed_and_capture(b"\x1bPzzSECRETPAYLOAD\x1b\\");
        let warns = warnings(&events);
        assert!(!warns.is_empty(), "expected a warn, got: {events:?}");
        for (level, text) in warns {
            assert!(
                !text.contains("SECRETPAYLOAD"),
                "{level} line leaked payload: {text}"
            );
        }
    }

    #[test]
    fn kitty_print_dcs_produces_no_warn() {
        let events = feed_and_capture(b"\x1bP@kitty-print|SECRETPAYLOAD\x1b\\");
        assert!(
            warnings(&events).is_empty(),
            "@kitty- DCS must not warn: {events:?}"
        );
        assert!(
            events.iter().any(|(level, text)| *level == Level::DEBUG
                && text.contains("DCS @kitty- sequence ignored")),
            "expected the dedicated @kitty- debug line: {events:?}"
        );
        assert!(
            events.iter().all(|(level, text)| {
                // Only the generic, bounded `DCS received` debug may show the body.
                *level != Level::DEBUG
                    || !text.contains("SECRETPAYLOAD")
                    || text.contains("DCS received")
            }),
            "the @kitty- consumption line must carry a length only: {events:?}"
        );
    }

    #[test]
    fn decrqss_unrecognised_query_warn_omits_payload() {
        let events = feed_and_capture(b"\x1bP$qSECRETPAYLOAD\x1b\\");
        let warns = warnings(&events);
        assert!(!warns.is_empty(), "expected a warn, got: {events:?}");
        for (level, text) in warns {
            assert!(
                !text.contains("SECRETPAYLOAD"),
                "{level} line leaked payload: {text}"
            );
        }
    }

    #[test]
    fn xtgettcap_invalid_hex_warn_omits_payload() {
        let events = feed_and_capture(b"\x1bP+qSECRETPAYLOAD\x1b\\");
        let warns = warnings(&events);
        assert!(!warns.is_empty(), "expected a warn, got: {events:?}");
        for (level, text) in warns {
            assert!(
                !text.contains("SECRETPAYLOAD"),
                "{level} line leaked payload: {text}"
            );
        }
    }

    #[test]
    fn dcs_received_debug_is_bounded() {
        let mut dcs = b"\x1bPzz".to_vec();
        dcs.extend(std::iter::repeat_n(b'A', 100_000));
        dcs.extend_from_slice(b"\x1b\\");
        let events = feed_and_capture(&dcs);
        // Every level, TRACE included: the per-output trace in
        // `state/internal.rs` is bounded too (129.C6).
        for (_, text) in &events {
            assert!(
                text.len() < 4096,
                "a log line carried an unbounded payload ({} bytes): {}",
                text.len(),
                &text[..text.len().min(120)]
            );
        }
    }
}
