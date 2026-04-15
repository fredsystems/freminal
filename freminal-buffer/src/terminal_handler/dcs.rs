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
//! - tmux DCS passthrough (`tmux;`) — un-doubles ESC bytes and dispatches the
//!   inner escape sequence to the appropriate handler
//! - CSI direct dispatch for tmux passthrough ordering correctness

use conv2::ValueFrom;
use freminal_common::{buffer_states::modes::s8c1t::S8c1t, cursor::CursorVisualStyle};

use super::TerminalHandler;

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
    ///   dispatches the inner escape sequence to the appropriate handler.
    ///
    /// Unknown or unsupported DCS sub-commands are logged at warn level.
    pub fn handle_device_control_string(&mut self, dcs: &[u8]) {
        tracing::debug!("DCS received: {:?}", String::from_utf8_lossy(dcs));
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
        } else {
            tracing::warn!(
                "DCS sub-command not recognized: {}",
                String::from_utf8_lossy(dcs)
            );
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

    /// Double every ESC byte in `data`.
    ///
    /// This is the inverse of [`undouble_esc`]: each `0x1b` in the input
    /// becomes `0x1b 0x1b` in the output.  Used when wrapping a response
    /// in a DCS tmux passthrough envelope.
    fn double_esc(data: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(data.len() + data.len() / 4);
        for &b in data {
            if b == 0x1b {
                out.push(0x1b);
            }
            out.push(b);
        }
        out
    }

    /// Wrap raw response bytes in a DCS tmux passthrough envelope.
    ///
    /// Format: `ESC P tmux; <payload-with-doubled-ESCs> ESC \`
    pub(super) fn wrap_tmux_passthrough(data: &[u8]) -> Vec<u8> {
        let doubled = Self::double_esc(data);
        // \x1bPtmux; ... \x1b\\
        let mut out = Vec::with_capacity(8 + doubled.len() + 2);
        out.extend_from_slice(b"\x1bPtmux;");
        out.extend_from_slice(&doubled);
        out.extend_from_slice(b"\x1b\\");
        out
    }

    /// Handle a tmux DCS passthrough payload.
    ///
    /// The `payload` is the content after the `tmux;` prefix, with ESC bytes
    /// still doubled.  This method un-doubles the ESC bytes, identifies the
    /// inner escape sequence type from its introducer byte, and dispatches to
    /// the appropriate handler.
    ///
    /// Supported inner sequence types:
    /// - **APC** (`ESC _`): dispatched to [`Self::handle_application_program_command`]
    ///   (e.g. Kitty graphics protocol).
    /// - **DCS** (`ESC P`): dispatched to [`Self::handle_device_control_string`]
    ///   (recursive — the inner DCS is itself unwrapped).
    /// - **OSC** (`ESC ]`): not yet supported (logged at warn level).
    /// - **CSI** (`ESC [`): not yet supported (logged at warn level).
    ///
    /// Any other introducer byte is logged at warn level.
    pub(super) fn handle_tmux_passthrough(&mut self, payload: &[u8]) {
        if payload.is_empty() {
            tracing::warn!("DCS tmux passthrough: empty payload");
            return;
        }

        let inner = Self::undouble_esc(payload);

        if inner.len() < 2 || inner[0] != 0x1b {
            tracing::warn!(
                "DCS tmux passthrough: inner sequence does not start with ESC: {}",
                String::from_utf8_lossy(&inner)
            );
            return;
        }

        // Set the flag so write_to_pty wraps responses in DCS tmux passthrough.
        self.in_tmux_passthrough = true;

        // The byte after ESC determines the sequence type.
        match inner[1] {
            // APC: ESC _ <content> ESC \   →  pass `_<content>ESC \` to APC handler
            b'_' => {
                tracing::debug!(
                    "DCS tmux passthrough: dispatching APC ({} bytes)",
                    inner.len()
                );
                // The APC handler expects the raw sequence starting with `_`
                // (strip_apc_envelope will remove the `_` prefix and `ESC \` suffix).
                self.handle_application_program_command(&inner[1..]);
            }
            // DCS: ESC P <content> ESC \   →  pass `P<content>ESC \` to DCS handler
            b'P' => {
                tracing::debug!(
                    "DCS tmux passthrough: dispatching DCS ({} bytes)",
                    inner.len()
                );
                // The DCS handler expects the raw sequence starting with `P`
                // (strip_dcs_envelope will remove the `P` prefix and `ESC \` suffix).
                self.handle_device_control_string(&inner[1..]);
            }
            // OSC: ESC ] <content> ESC \   →  queue for re-parsing
            b']' => {
                tracing::debug!(
                    "DCS tmux passthrough: queuing OSC for re-parse ({} bytes)",
                    inner.len()
                );
                self.tmux_reparse_queue.push(inner);
            }
            // CSI: ESC [ <params> <terminator>
            //
            // We dispatch common CSI commands (cursor movement, erase, etc.)
            // directly to avoid ordering issues.  When a DCS-wrapped CUP and
            // a DCS-wrapped APC Kitty Put arrive in the same PTY frame, the
            // CUP must execute before the Put so the cursor is at the correct
            // position.  If the CUP were queued to the reparse queue it would
            // only run *after* all DCS items in the current batch, which is
            // too late.
            //
            // Mode-setting commands (CSI ? ... h/l) and SGR (CSI ... m) are
            // still queued because they need the full parser or
            // TerminalState-level sync.
            b'[' => {
                // inner[0] = ESC, inner[1] = '[', CSI body starts at [2].
                if !self.dispatch_tmux_csi(&inner[2..]) {
                    // Unhandled CSI — fall back to the reparse queue.
                    self.tmux_reparse_queue.push(inner);
                }
            }
            other => {
                tracing::warn!(
                    "DCS tmux passthrough: unknown inner sequence type 0x{other:02x}: {}",
                    String::from_utf8_lossy(&inner)
                );
            }
        }

        // Clear the flag after dispatch so subsequent direct writes are not wrapped.
        self.in_tmux_passthrough = false;
    }

    /// Directly dispatch a CSI sequence from inside a tmux DCS passthrough.
    ///
    /// `csi_body` is the bytes *after* `ESC [` — i.e. the parameter bytes and
    /// the terminator.  Returns `true` if the command was handled directly,
    /// `false` if the caller should fall back to the reparse queue.
    ///
    /// This handles the subset of CSI commands that are purely buffer-level
    /// (cursor movement, erase) so they execute immediately — critical for
    /// correct ordering when a CUP precedes a Kitty Put in the same frame.
    // Inherently large: tmux-passthrough CSI dispatch table. Each arm handles a distinct CSI
    // sequence. Splitting would scatter related escape-sequence handling.
    #[allow(clippy::too_many_lines)]
    pub(super) fn dispatch_tmux_csi(&mut self, csi_body: &[u8]) -> bool {
        if csi_body.is_empty() {
            return false;
        }

        // CSI parameters that start with '?' are DEC private modes (h/l).
        // These need TerminalState-level sync, so fall back to the reparse queue.
        if csi_body.first() == Some(&b'?') {
            tracing::debug!("DCS tmux passthrough: queuing DEC private CSI for re-parse");
            return false;
        }

        // Find the terminator: the last byte in 0x40..=0x7E range.
        let Some(&terminator) = csi_body.last() else {
            return false;
        };
        if !(0x40..=0x7e).contains(&terminator) {
            return false;
        }

        // Param bytes are everything before the terminator.
        let params = &csi_body[..csi_body.len() - 1];

        // Check for intermediate bytes (0x20..=0x2F) — these indicate
        // extended CSI commands that we don't handle directly.
        if params.iter().any(|&b| (0x20..=0x2f).contains(&b)) {
            tracing::debug!("DCS tmux passthrough: queuing CSI with intermediates for re-parse");
            return false;
        }

        // Parse semicolon-delimited numeric parameters.
        let numeric_params = Self::parse_csi_params(params);

        match terminator {
            // CUP — Cursor Position: ESC [ row ; col H  (or f)
            b'H' | b'f' => {
                let row = numeric_params
                    .first()
                    .copied()
                    .unwrap_or(Some(1))
                    .unwrap_or(1)
                    .max(1);
                let col = numeric_params
                    .get(1)
                    .copied()
                    .unwrap_or(Some(1))
                    .unwrap_or(1)
                    .max(1);
                tracing::debug!(
                    "DCS tmux passthrough: CSI CUP row={row} col={col} (direct dispatch)"
                );
                self.handle_cursor_pos(Some(col), Some(row));
                true
            }
            // CUU — Cursor Up: ESC [ n A
            b'A' => {
                let n = numeric_params
                    .first()
                    .copied()
                    .unwrap_or(Some(1))
                    .unwrap_or(1)
                    .max(1);
                self.handle_cursor_up(n);
                true
            }
            // CUD — Cursor Down: ESC [ n B
            b'B' => {
                let n = numeric_params
                    .first()
                    .copied()
                    .unwrap_or(Some(1))
                    .unwrap_or(1)
                    .max(1);
                self.handle_cursor_down(n);
                true
            }
            // CUF — Cursor Forward: ESC [ n C
            b'C' => {
                let n = numeric_params
                    .first()
                    .copied()
                    .unwrap_or(Some(1))
                    .unwrap_or(1)
                    .max(1);
                self.handle_cursor_forward(n);
                true
            }
            // CUB — Cursor Backward: ESC [ n D
            b'D' => {
                let n = numeric_params
                    .first()
                    .copied()
                    .unwrap_or(Some(1))
                    .unwrap_or(1)
                    .max(1);
                self.handle_cursor_backward(n);
                true
            }
            // CNL — Cursor Next Line: ESC [ n E
            b'E' => {
                let n = numeric_params
                    .first()
                    .copied()
                    .unwrap_or(Some(1))
                    .unwrap_or(1)
                    .max(1);
                let n_i32 = i32::value_from(n).unwrap_or(i32::MAX);
                self.handle_cursor_relative(0, n_i32);
                self.handle_cursor_pos(Some(1), None);
                true
            }
            // CPL — Cursor Previous Line: ESC [ n F
            b'F' => {
                let n = numeric_params
                    .first()
                    .copied()
                    .unwrap_or(Some(1))
                    .unwrap_or(1)
                    .max(1);
                let n_i32 = i32::value_from(n).unwrap_or(i32::MAX);
                self.handle_cursor_relative(0, -n_i32);
                self.handle_cursor_pos(Some(1), None);
                true
            }
            // CHA/HPA — Cursor Horizontal Absolute: ESC [ n G  (or `)
            b'G' | b'`' => {
                let col = numeric_params
                    .first()
                    .copied()
                    .unwrap_or(Some(1))
                    .unwrap_or(1)
                    .max(1);
                self.handle_cursor_pos(Some(col), None);
                true
            }
            // VPA — Vertical Position Absolute: ESC [ n d
            b'd' => {
                let row = numeric_params
                    .first()
                    .copied()
                    .unwrap_or(Some(1))
                    .unwrap_or(1)
                    .max(1);
                self.handle_cursor_pos(None, Some(row));
                true
            }
            // ED — Erase in Display: ESC [ n J
            b'J' => {
                let mode = numeric_params
                    .first()
                    .copied()
                    .unwrap_or(Some(0))
                    .unwrap_or(0);
                self.handle_erase_in_display(mode);
                true
            }
            // EL — Erase in Line: ESC [ n K
            b'K' => {
                let mode = numeric_params
                    .first()
                    .copied()
                    .unwrap_or(Some(0))
                    .unwrap_or(0);
                self.handle_erase_in_line(mode);
                true
            }
            // IL — Insert Lines: ESC [ n L
            b'L' => {
                let n = numeric_params
                    .first()
                    .copied()
                    .unwrap_or(Some(1))
                    .unwrap_or(1)
                    .max(1);
                self.handle_insert_lines(n);
                true
            }
            // DL — Delete Lines: ESC [ n M
            b'M' => {
                let n = numeric_params
                    .first()
                    .copied()
                    .unwrap_or(Some(1))
                    .unwrap_or(1)
                    .max(1);
                self.handle_delete_lines(n);
                true
            }
            // DCH — Delete Characters: ESC [ n P
            b'P' => {
                let n = numeric_params
                    .first()
                    .copied()
                    .unwrap_or(Some(1))
                    .unwrap_or(1)
                    .max(1);
                self.handle_delete_chars(n);
                true
            }
            // ECH — Erase Characters: ESC [ n X
            b'X' => {
                let n = numeric_params
                    .first()
                    .copied()
                    .unwrap_or(Some(1))
                    .unwrap_or(1)
                    .max(1);
                self.handle_erase_chars(n);
                true
            }
            // ICH — Insert Characters: ESC [ n @
            b'@' => {
                let n = numeric_params
                    .first()
                    .copied()
                    .unwrap_or(Some(1))
                    .unwrap_or(1)
                    .max(1);
                self.handle_insert_spaces(n);
                true
            }
            // SU — Scroll Up: ESC [ n S
            b'S' => {
                let n = numeric_params
                    .first()
                    .copied()
                    .unwrap_or(Some(1))
                    .unwrap_or(1)
                    .max(1);
                self.handle_scroll_up(n);
                true
            }
            // SD — Scroll Down: ESC [ n T
            b'T' => {
                let n = numeric_params
                    .first()
                    .copied()
                    .unwrap_or(Some(1))
                    .unwrap_or(1)
                    .max(1);
                self.handle_scroll_down(n);
                true
            }
            // DECSTBM — Set Top and Bottom Margins: ESC [ top ; bottom r
            b'r' => {
                let top = numeric_params
                    .first()
                    .copied()
                    .flatten()
                    .unwrap_or(1)
                    .max(1);
                let bottom = numeric_params
                    .get(1)
                    .copied()
                    .flatten()
                    .unwrap_or(usize::MAX);
                self.handle_set_scroll_region(top, bottom);
                true
            }
            // SCOSC — Save Cursor: ESC [ s
            b's' if params.is_empty() => {
                self.buffer.save_cursor();
                true
            }
            // SCORC — Restore Cursor: ESC [ u
            b'u' if params.is_empty() => {
                self.buffer.restore_cursor();
                true
            }
            // SGR and mode-setting (h/l) fall through to the reparse queue.
            // SGR (m) needs the full SGR parser; mode set/reset (h/l) needs
            // TerminalState-level sync.
            _ => {
                tracing::debug!(
                    "DCS tmux passthrough: queuing unhandled CSI '{}'(0x{terminator:02x}) for re-parse",
                    terminator as char,
                );
                false
            }
        }
    }

    /// Parse CSI parameter bytes into a list of `Option<usize>` values.
    ///
    /// Parameters are separated by `;`.  An empty field yields `None`.
    /// For example, `b"1;42"` → `[Some(1), Some(42)]`,
    /// `b""` → `[]`, `b";"` → `[None, None]`.
    pub(super) fn parse_csi_params(params: &[u8]) -> Vec<Option<usize>> {
        if params.is_empty() {
            return Vec::new();
        }

        let param_str = std::str::from_utf8(params).unwrap_or("");
        param_str
            .split(';')
            .map(|s| {
                if s.is_empty() {
                    None
                } else {
                    s.parse::<usize>().ok()
                }
            })
            .collect()
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
                tracing::warn!(
                    "DECRQSS: unrecognized setting query: {}",
                    String::from_utf8_lossy(pt)
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
            "XTGETTCAP query: {:?}",
            String::from_utf8_lossy(hex_payload)
        );
        let payload_str = String::from_utf8_lossy(hex_payload);

        // Split on ';' to support multiple capability queries in a single DCS.
        for hex_name in payload_str.split(';') {
            if hex_name.is_empty() {
                continue;
            }

            let Some(cap_name) = Self::hex_decode(hex_name) else {
                tracing::warn!("XTGETTCAP: invalid hex encoding: {hex_name}");
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
                tracing::warn!("XTGETTCAP: unknown capability: {cap_name}");
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
            // TN — terminal name
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
        handler.process_output(&TerminalOutput::Sgr(SelectGraphicRendition::Bold));
        handler.process_output(&TerminalOutput::Sgr(SelectGraphicRendition::Italic));

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

        handler.process_output(&TerminalOutput::Sgr(SelectGraphicRendition::Foreground(
            TerminalColor::Red,
        )));

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

        handler.process_output(&TerminalOutput::Sgr(SelectGraphicRendition::Foreground(
            TerminalColor::Custom(255, 128, 0),
        )));

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

        handler.process_output(&TerminalOutput::Sgr(SelectGraphicRendition::ReverseVideo));

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

        handler.process_output(&TerminalOutput::CursorVisualStyle(
            CursorVisualStyle::UnderlineCursorBlink,
        ));

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
        assert!(inner.is_empty());
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
        assert!(result.is_empty());
    }

    #[test]
    fn undouble_esc_triple_esc() {
        // Three consecutive ESC bytes: first two form a pair → ESC, the third
        // remains as a lone ESC.
        let data = b"\x1b\x1b\x1b";
        let result = TerminalHandler::undouble_esc(data);
        assert_eq!(result, b"\x1b\x1b");
    }

    // ── double_esc tests ──────────────────────────────────────────────────

    #[test]
    fn double_esc_no_esc_bytes() {
        let data = b"hello world";
        let result = TerminalHandler::double_esc(data);
        assert_eq!(result, b"hello world");
    }

    #[test]
    fn double_esc_single_esc() {
        let data = b"\x1b";
        let result = TerminalHandler::double_esc(data);
        assert_eq!(result, b"\x1b\x1b");
    }

    #[test]
    fn double_esc_apc_sequence() {
        // ESC _ G i=1;OK ESC \  → ESC ESC _ G i=1;OK ESC ESC backslash
        let data = b"\x1b_Gi=1;OK\x1b\\";
        let result = TerminalHandler::double_esc(data);
        assert_eq!(result, b"\x1b\x1b_Gi=1;OK\x1b\x1b\\");
    }

    #[test]
    fn double_esc_empty() {
        let result = TerminalHandler::double_esc(b"");
        assert!(result.is_empty());
    }

    #[test]
    fn double_esc_roundtrip() {
        // undouble(double(x)) == x for any input
        let original = b"\x1b_Ga=q,i=1;\x1b\\";
        let doubled = TerminalHandler::double_esc(original);
        let undoubled = TerminalHandler::undouble_esc(&doubled);
        assert_eq!(undoubled, original.to_vec());
    }

    // ── wrap_tmux_passthrough tests ───────────────────────────────────────

    #[test]
    fn wrap_tmux_passthrough_kitty_response() {
        // A Kitty OK response should be wrapped correctly
        let response = b"\x1b_Gi=1;OK\x1b\\";
        let wrapped = TerminalHandler::wrap_tmux_passthrough(response);
        // Expected: ESC P tmux; ESC ESC _ G i=1;OK ESC ESC \ ESC \
        let expected = b"\x1bPtmux;\x1b\x1b_Gi=1;OK\x1b\x1b\\\x1b\\";
        assert_eq!(wrapped, expected.to_vec());
    }

    #[test]
    fn wrap_tmux_passthrough_plain_text() {
        // Plain text (no ESC) should pass through with just the envelope
        let data = b"hello";
        let wrapped = TerminalHandler::wrap_tmux_passthrough(data);
        assert_eq!(wrapped, b"\x1bPtmux;hello\x1b\\".to_vec());
    }

    // ── tmux passthrough dispatch tests ───────────────────────────────────

    #[test]
    fn tmux_passthrough_empty_payload_does_not_panic() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_tmux_passthrough(b"");
        // Success = no panic
    }

    #[test]
    fn tmux_passthrough_no_esc_prefix_does_not_panic() {
        let mut handler = TerminalHandler::new(80, 24);
        // Payload that does not start with doubled ESC
        handler.handle_tmux_passthrough(b"junk data");
        // Success = no panic
    }

    #[test]
    fn tmux_passthrough_too_short_does_not_panic() {
        let mut handler = TerminalHandler::new(80, 24);
        // Payload is just a doubled ESC with no type byte
        handler.handle_tmux_passthrough(b"\x1b\x1b");
        // After un-doubling: [0x1b] — length < 2 → warn and return
    }

    #[test]
    fn tmux_passthrough_dispatches_apc_kitty_query() {
        // Build a tmux-wrapped Kitty graphics query:
        //   Inner (un-doubled): ESC _ G a=q,i=1; ESC \
        //   Doubled for tmux:   ESC ESC _ G a=q,i=1; ESC ESC \
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // The tmux payload (after "tmux;" prefix has been stripped):
        // doubled-ESC _ G a=q,i=1; doubled-ESC backslash
        let mut payload = Vec::new();
        payload.extend_from_slice(b"\x1b\x1b_Ga=q,i=1;\x1b\x1b\\");

        handler.handle_tmux_passthrough(&payload);

        // The Kitty query handler should respond with a tmux-wrapped APC response
        let response = rx.try_recv();
        assert!(
            response.is_ok(),
            "Expected a Kitty graphics query response via PTY write"
        );
        let PtyWrite::Write(bytes) = response.unwrap() else {
            panic!("expected PtyWrite::Write");
        };
        let resp_str = String::from_utf8_lossy(&bytes);
        // Response should be wrapped in DCS tmux passthrough
        assert!(
            resp_str.starts_with("\x1bPtmux;"),
            "Expected tmux-wrapped response, got: {resp_str}"
        );
        // The inner content (after un-doubling) should be a Kitty APC response
        let inner = resp_str
            .strip_prefix("\x1bPtmux;")
            .and_then(|s| s.strip_suffix("\x1b\\"))
            .expect("Expected DCS tmux envelope");
        let inner_bytes = TerminalHandler::undouble_esc(inner.as_bytes());
        let inner_str = String::from_utf8_lossy(&inner_bytes);
        assert!(
            inner_str.starts_with("\x1b_G"),
            "Expected inner Kitty APC response, got: {inner_str}"
        );

        // The passthrough flag should be cleared after dispatch
        assert!(
            !handler.in_tmux_passthrough,
            "in_tmux_passthrough should be false after dispatch"
        );
    }

    #[test]
    fn tmux_passthrough_dispatches_nested_dcs() {
        // Build a tmux-wrapped DCS DECRQSS query for SGR:
        //   Inner (un-doubled): ESC P $ q m ESC \
        //   Doubled for tmux:   ESC ESC P $ q m ESC ESC \
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        let mut payload = Vec::new();
        payload.extend_from_slice(b"\x1b\x1bP$qm\x1b\x1b\\");

        handler.handle_tmux_passthrough(&payload);

        // The DECRQSS handler should respond with a tmux-wrapped DCS response
        let response = rx.try_recv();
        assert!(
            response.is_ok(),
            "Expected a DECRQSS response via PTY write"
        );
        let PtyWrite::Write(bytes) = response.unwrap() else {
            panic!("expected PtyWrite::Write");
        };
        let resp_str = String::from_utf8_lossy(&bytes);
        // Response should be wrapped in DCS tmux passthrough
        assert!(
            resp_str.starts_with("\x1bPtmux;"),
            "Expected tmux-wrapped response, got: {resp_str}"
        );
        // The inner content should be a DECRQSS response
        let inner = resp_str
            .strip_prefix("\x1bPtmux;")
            .and_then(|s| s.strip_suffix("\x1b\\"))
            .expect("Expected DCS tmux envelope");
        let inner_bytes = TerminalHandler::undouble_esc(inner.as_bytes());
        let inner_str = String::from_utf8_lossy(&inner_bytes);
        assert!(
            inner_str.contains("$r"),
            "Expected DECRQSS response, got: {inner_str}"
        );
    }

    #[test]
    fn tmux_passthrough_unknown_type_does_not_panic() {
        let mut handler = TerminalHandler::new(80, 24);
        // Inner: ESC Z (unknown type)
        let payload = b"\x1b\x1bZ";
        handler.handle_tmux_passthrough(payload);
        // Success = no panic; flag should be cleared
        assert!(!handler.in_tmux_passthrough);
    }

    #[test]
    fn tmux_passthrough_via_full_dcs_handler() {
        // End-to-end: feed a complete DCS tmux passthrough through
        // handle_device_control_string (the normal entry point).
        //
        // Format: P tmux; <doubled-payload> ESC \
        // Payload: Kitty graphics query: ESC ESC _ G a=q,i=1; ESC ESC \
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        let mut dcs = vec![b'P'];
        dcs.extend_from_slice(b"tmux;");
        dcs.extend_from_slice(b"\x1b\x1b_Ga=q,i=1;\x1b\x1b\\");
        dcs.extend_from_slice(b"\x1b\\");

        handler.handle_device_control_string(&dcs);

        // Should have dispatched to the Kitty query handler with tmux wrapping
        let response = rx.try_recv();
        assert!(
            response.is_ok(),
            "Expected a Kitty graphics query response from full DCS tmux passthrough"
        );
        let PtyWrite::Write(bytes) = response.unwrap() else {
            panic!("expected PtyWrite::Write");
        };
        let resp_str = String::from_utf8_lossy(&bytes);
        // Response should be wrapped in DCS tmux passthrough
        assert!(
            resp_str.starts_with("\x1bPtmux;"),
            "Expected tmux-wrapped response, got: {resp_str}"
        );
    }

    #[test]
    fn tmux_passthrough_flag_cleared_after_early_return() {
        // Even when the payload is invalid and we return early,
        // the flag should not be left set.
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_tmux_passthrough(b"");
        assert!(!handler.in_tmux_passthrough);
        handler.handle_tmux_passthrough(b"junk");
        assert!(!handler.in_tmux_passthrough);
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
        // "xterm-256color" → hex
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
    // dispatch_tmux_csi unit tests
    // ------------------------------------------------------------------

    #[test]
    fn tmux_csi_cup_dispatches_directly() {
        let mut handler = TerminalHandler::new(80, 24);
        // CSI 5;10H → move cursor to row 5, col 10 (1-based)
        let dispatched = handler.dispatch_tmux_csi(b"5;10H");
        assert!(dispatched, "CUP should be handled directly");
        let cursor = handler.buffer.get_cursor().pos;
        assert_eq!(cursor.x, 9, "col should be 10 - 1 = 9 (0-based)");
        assert_eq!(cursor.y, 4, "row should be 5 - 1 = 4 (0-based)");
    }

    #[test]
    fn tmux_csi_cup_default_params() {
        let mut handler = TerminalHandler::new(80, 24);
        // CSI H → move cursor to row 1, col 1 (default)
        let dispatched = handler.dispatch_tmux_csi(b"H");
        assert!(dispatched);
        let cursor = handler.buffer.get_cursor().pos;
        assert_eq!(cursor.x, 0);
        assert_eq!(cursor.y, 0);
    }

    #[test]
    fn tmux_csi_cup_with_f_terminator() {
        let mut handler = TerminalHandler::new(80, 24);
        // CSI 3;7f → same as H
        let dispatched = handler.dispatch_tmux_csi(b"3;7f");
        assert!(dispatched);
        let cursor = handler.buffer.get_cursor().pos;
        assert_eq!(cursor.x, 6);
        assert_eq!(cursor.y, 2);
    }

    #[test]
    fn tmux_csi_cursor_up() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_cursor_pos(Some(5), Some(10));
        let dispatched = handler.dispatch_tmux_csi(b"3A");
        assert!(dispatched);
        let cursor = handler.buffer.get_cursor().pos;
        assert_eq!(cursor.y, 6, "should move up 3 from row 9 → row 6");
    }

    #[test]
    fn tmux_csi_cursor_down() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_cursor_pos(Some(1), Some(5));
        let dispatched = handler.dispatch_tmux_csi(b"2B");
        assert!(dispatched);
        let cursor = handler.buffer.get_cursor().pos;
        assert_eq!(cursor.y, 6, "should move down 2 from row 4 → row 6");
    }

    #[test]
    fn tmux_csi_cursor_forward() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_cursor_pos(Some(10), Some(1));
        let dispatched = handler.dispatch_tmux_csi(b"5C");
        assert!(dispatched);
        let cursor = handler.buffer.get_cursor().pos;
        assert_eq!(cursor.x, 14, "should move forward 5 from col 9 → col 14");
    }

    #[test]
    fn tmux_csi_cursor_backward() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_cursor_pos(Some(10), Some(1));
        let dispatched = handler.dispatch_tmux_csi(b"3D");
        assert!(dispatched);
        let cursor = handler.buffer.get_cursor().pos;
        assert_eq!(cursor.x, 6, "should move backward 3 from col 9 → col 6");
    }

    #[test]
    fn tmux_csi_cha() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_cursor_pos(Some(1), Some(5));
        let dispatched = handler.dispatch_tmux_csi(b"20G");
        assert!(dispatched);
        let cursor = handler.buffer.get_cursor().pos;
        assert_eq!(cursor.x, 19, "CHA should set col to 20 - 1 = 19");
        assert_eq!(cursor.y, 4, "CHA should not change row");
    }

    #[test]
    fn tmux_csi_vpa() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_cursor_pos(Some(10), Some(1));
        let dispatched = handler.dispatch_tmux_csi(b"15d");
        assert!(dispatched);
        let cursor = handler.buffer.get_cursor().pos;
        assert_eq!(cursor.y, 14, "VPA should set row to 15 - 1 = 14");
    }

    #[test]
    fn tmux_csi_dec_private_falls_through() {
        let mut handler = TerminalHandler::new(80, 24);
        // CSI ? 1049 h — DEC private mode, should not be handled directly
        let dispatched = handler.dispatch_tmux_csi(b"?1049h");
        assert!(
            !dispatched,
            "DEC private modes should fall through to reparse queue"
        );
    }

    #[test]
    fn tmux_csi_sgr_falls_through() {
        let mut handler = TerminalHandler::new(80, 24);
        // CSI 1;32 m — SGR bold green, should fall through
        let dispatched = handler.dispatch_tmux_csi(b"1;32m");
        assert!(!dispatched, "SGR should fall through to reparse queue");
    }

    #[test]
    fn tmux_csi_erase_in_display() {
        let mut handler = TerminalHandler::new(80, 24);
        // Write some text first
        handler.handle_data(b"Hello");
        // CSI 2 J — erase display
        let dispatched = handler.dispatch_tmux_csi(b"2J");
        assert!(dispatched, "ED should be handled directly");
    }

    #[test]
    fn tmux_csi_erase_in_line() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_data(b"Hello");
        let dispatched = handler.dispatch_tmux_csi(b"0K");
        assert!(dispatched, "EL should be handled directly");
    }

    #[test]
    fn tmux_csi_empty_body_returns_false() {
        let mut handler = TerminalHandler::new(80, 24);
        assert!(!handler.dispatch_tmux_csi(b""));
    }

    #[test]
    fn tmux_csi_intermediates_fall_through() {
        let mut handler = TerminalHandler::new(80, 24);
        // CSI with intermediate byte (space + p = DECRQM)
        assert!(!handler.dispatch_tmux_csi(b"?1049$p"));
    }

    #[test]
    fn parse_csi_params_basic() {
        assert_eq!(
            TerminalHandler::parse_csi_params(b"1;42"),
            vec![Some(1), Some(42)]
        );
    }

    #[test]
    fn parse_csi_params_empty() {
        assert_eq!(
            TerminalHandler::parse_csi_params(b""),
            Vec::<Option<usize>>::new()
        );
    }

    #[test]
    fn parse_csi_params_missing_field() {
        assert_eq!(
            TerminalHandler::parse_csi_params(b";42"),
            vec![None, Some(42)]
        );
    }

    #[test]
    fn parse_csi_params_single() {
        assert_eq!(TerminalHandler::parse_csi_params(b"5"), vec![Some(5)]);
    }

    // ------------------------------------------------------------------
    // dispatch_tmux_csi — remaining CSI commands
    // ------------------------------------------------------------------

    #[test]
    fn tmux_csi_cnl_cursor_next_line() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_cursor_pos(Some(10), Some(5));
        // CSI 2 E → cursor next line, move down 2 and to col 1
        let dispatched = handler.dispatch_tmux_csi(b"2E");
        assert!(dispatched);
        let cursor = handler.buffer.get_cursor().pos;
        assert_eq!(cursor.x, 0, "CNL should reset col to 0");
        assert_eq!(cursor.y, 6, "CNL should move down 2 from row 4 → row 6");
    }

    #[test]
    fn tmux_csi_cpl_cursor_previous_line() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_cursor_pos(Some(10), Some(10));
        // CSI 3 F → cursor previous line, move up 3 and to col 1
        let dispatched = handler.dispatch_tmux_csi(b"3F");
        assert!(dispatched);
        let cursor = handler.buffer.get_cursor().pos;
        assert_eq!(cursor.x, 0, "CPL should reset col to 0");
        assert_eq!(cursor.y, 6, "CPL should move up 3 from row 9 → row 6");
    }

    #[test]
    fn tmux_csi_il_insert_lines() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_cursor_pos(Some(1), Some(5));
        let dispatched = handler.dispatch_tmux_csi(b"2L");
        assert!(dispatched, "IL should be handled directly");
    }

    #[test]
    fn tmux_csi_dl_delete_lines() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_cursor_pos(Some(1), Some(5));
        let dispatched = handler.dispatch_tmux_csi(b"2M");
        assert!(dispatched, "DL should be handled directly");
    }

    #[test]
    fn tmux_csi_dch_delete_chars() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_data(b"Hello World");
        handler.handle_cursor_pos(Some(1), Some(1));
        let dispatched = handler.dispatch_tmux_csi(b"3P");
        assert!(dispatched, "DCH should be handled directly");
    }

    #[test]
    fn tmux_csi_ech_erase_chars() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_data(b"Hello World");
        handler.handle_cursor_pos(Some(1), Some(1));
        let dispatched = handler.dispatch_tmux_csi(b"5X");
        assert!(dispatched, "ECH should be handled directly");
    }

    #[test]
    fn tmux_csi_ich_insert_chars() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_data(b"Hello");
        handler.handle_cursor_pos(Some(1), Some(1));
        let dispatched = handler.dispatch_tmux_csi(b"2@");
        assert!(dispatched, "ICH should be handled directly");
    }

    #[test]
    fn tmux_csi_su_scroll_up() {
        let mut handler = TerminalHandler::new(80, 24);
        let dispatched = handler.dispatch_tmux_csi(b"3S");
        assert!(dispatched, "SU should be handled directly");
    }

    #[test]
    fn tmux_csi_sd_scroll_down() {
        let mut handler = TerminalHandler::new(80, 24);
        let dispatched = handler.dispatch_tmux_csi(b"2T");
        assert!(dispatched, "SD should be handled directly");
    }

    #[test]
    fn tmux_csi_decstbm_set_scroll_region() {
        let mut handler = TerminalHandler::new(80, 24);
        // CSI 5;20 r → set scroll region rows 5..20
        let dispatched = handler.dispatch_tmux_csi(b"5;20r");
        assert!(dispatched, "DECSTBM should be handled directly");
    }

    #[test]
    fn tmux_csi_scosc_save_cursor() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_cursor_pos(Some(10), Some(5));
        // CSI s (no params) → save cursor
        let dispatched = handler.dispatch_tmux_csi(b"s");
        assert!(dispatched, "SCOSC should be handled directly");
    }

    #[test]
    fn tmux_csi_scorc_restore_cursor() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_cursor_pos(Some(10), Some(5));
        handler.buffer.save_cursor();
        handler.handle_cursor_pos(Some(1), Some(1));
        // CSI u (no params) → restore cursor
        let dispatched = handler.dispatch_tmux_csi(b"u");
        assert!(dispatched, "SCORC should be handled directly");
        let cursor = handler.buffer.get_cursor().pos;
        assert_eq!(cursor.x, 9, "should restore col to 9");
        assert_eq!(cursor.y, 4, "should restore row to 4");
    }

    #[test]
    fn tmux_csi_unknown_terminator_falls_through() {
        let mut handler = TerminalHandler::new(80, 24);
        // CSI n — DSR, not in the direct dispatch table
        let dispatched = handler.dispatch_tmux_csi(b"6n");
        assert!(
            !dispatched,
            "Unknown CSI should fall through to reparse queue"
        );
    }

    #[test]
    fn tmux_csi_invalid_terminator_byte() {
        let mut handler = TerminalHandler::new(80, 24);
        // Terminator 0x3F ('?') is below 0x40 range
        assert!(!handler.dispatch_tmux_csi(b"1;2?"));
    }

    #[test]
    fn tmux_csi_hpa_backtick() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_cursor_pos(Some(1), Some(5));
        // CSI 30 ` → HPA, set col to 30
        let dispatched = handler.dispatch_tmux_csi(b"30`");
        assert!(dispatched, "HPA should be handled directly");
        let cursor = handler.buffer.get_cursor().pos;
        assert_eq!(cursor.x, 29, "HPA should set col to 30 - 1 = 29");
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
    // tmux passthrough — OSC queuing
    // ------------------------------------------------------------------

    #[test]
    fn tmux_passthrough_osc_queued_to_reparse() {
        let mut handler = TerminalHandler::new(80, 24);
        // Inner (un-doubled): ESC ] 0;title ESC \
        // Doubled for tmux: ESC ESC ] 0;title ESC ESC \
        let mut payload = Vec::new();
        payload.extend_from_slice(b"\x1b\x1b]0;title\x1b\x1b\\");

        handler.handle_tmux_passthrough(&payload);

        // The inner sequence should be in the reparse queue
        assert!(
            !handler.tmux_reparse_queue.is_empty(),
            "OSC should be queued for re-parse"
        );
    }

    #[test]
    fn tmux_passthrough_csi_handled_queued_to_reparse() {
        let mut handler = TerminalHandler::new(80, 24);
        // Inner (un-doubled): ESC [ 1 ; 32 m  (SGR — should NOT be direct-dispatched)
        // Doubled for tmux: ESC ESC [ 1 ; 32 m
        let mut payload = Vec::new();
        payload.extend_from_slice(b"\x1b\x1b[1;32m");

        handler.handle_tmux_passthrough(&payload);

        // SGR falls through dispatch_tmux_csi, so gets queued to reparse
        assert!(
            !handler.tmux_reparse_queue.is_empty(),
            "Unhandled CSI should be queued for re-parse"
        );
    }

    #[test]
    fn tmux_passthrough_csi_cup_direct_dispatch() {
        let mut handler = TerminalHandler::new(80, 24);
        // Inner (un-doubled): ESC [ 5 ; 10 H  (CUP — should be direct-dispatched)
        let mut payload = Vec::new();
        payload.extend_from_slice(b"\x1b\x1b[5;10H");

        handler.handle_tmux_passthrough(&payload);

        // CUP is directly dispatched, should NOT be in reparse queue
        assert!(
            handler.tmux_reparse_queue.is_empty(),
            "CUP should be directly dispatched, not queued"
        );
        let cursor = handler.buffer.get_cursor().pos;
        assert_eq!(cursor.x, 9);
        assert_eq!(cursor.y, 4);
    }

    #[test]
    fn tmux_passthrough_inner_no_esc_prefix() {
        let mut handler = TerminalHandler::new(80, 24);
        // Payload that un-doubles to "junk" (no ESC prefix)
        handler.handle_tmux_passthrough(b"junk");
        assert!(!handler.in_tmux_passthrough);
    }

    #[test]
    fn tmux_passthrough_inner_too_short() {
        let mut handler = TerminalHandler::new(80, 24);
        // Payload that un-doubles to just ESC (one byte, too short)
        handler.handle_tmux_passthrough(b"\x1b");
        assert!(!handler.in_tmux_passthrough);
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

    /// Integration test: simulate the exact nvim tmux passthrough scenario
    /// where CUP and Kitty Put arrive as separate DCS tmux items in the same
    /// `process_outputs()` batch.  With the direct CSI dispatch, the CUP
    /// should execute immediately so the Put reads the correct cursor position.
    #[test]
    fn tmux_passthrough_cup_then_kitty_put_ordering() {
        let mut handler = TerminalHandler::new(80, 24);

        // Start cursor at 0,0
        assert_eq!(handler.buffer.get_cursor().pos.x, 0);
        assert_eq!(handler.buffer.get_cursor().pos.y, 0);

        // Simulate: DCS tmux passthrough containing CSI 1;42H (CUP row 1, col 42)
        // The tmux DCS wrapper has already been stripped; the inner content is
        // ESC [ 1 ; 4 2 H with doubled ESC bytes.
        // undouble_esc would produce: ESC [ 1 ; 4 2 H
        // handle_tmux_passthrough would match inner[1] == '[' and call dispatch_tmux_csi
        // with "1;42H".
        let dispatched = handler.dispatch_tmux_csi(b"1;42H");
        assert!(dispatched);
        let cursor = handler.buffer.get_cursor().pos;
        assert_eq!(cursor.x, 41, "col should be 42 - 1 = 41 (0-based)");
        assert_eq!(cursor.y, 0, "row should be 1 - 1 = 0 (0-based)");

        // Now the APC Kitty Put would execute, reading cursor.pos correctly.
    }
}
