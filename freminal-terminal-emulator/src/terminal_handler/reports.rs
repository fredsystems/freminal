// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Terminal report and query response methods for [`TerminalHandler`].
//!
//! Contains DA1/DA2/DA3, DECREQTPARM, DSR, color theme report, and
//! device name/version responses.

use conv2::ValueFrom;
use freminal_common::buffer_states::modes::decanm::Decanm;
use freminal_common::buffer_states::sixel::{MAX_PALETTE, MAX_SIXEL_DIMENSION, MAX_SIXEL_PIXELS};
use freminal_common::buffer_states::xtsmgraphics::{
    GraphicsAttributeAction, GraphicsAttributeItem,
};

use super::TerminalHandler;

/// XTSMGRAPHICS status: success.
const XTSMGRAPHICS_OK: u8 = 0;
/// XTSMGRAPHICS status: error in `Pi`.
const XTSMGRAPHICS_BAD_ITEM: u8 = 1;
/// XTSMGRAPHICS status: error in `Pa`.
const XTSMGRAPHICS_BAD_ACTION: u8 = 2;
/// XTSMGRAPHICS status: failure.
const XTSMGRAPHICS_FAILURE: u8 = 3;

/// The largest sixel geometry, per axis, that is safe to advertise.
///
/// The decoder caps each axis at [`MAX_SIXEL_DIMENSION`] and the total at
/// [`MAX_SIXEL_PIXELS`], so a client that sizes an image to the reported
/// maximum on both axes must not exceed either. The largest square that
/// satisfies both is the answer.
const MAX_SIXEL_GEOMETRY: usize = {
    let side = MAX_SIXEL_PIXELS.isqrt();
    if side < MAX_SIXEL_DIMENSION {
        side
    } else {
        MAX_SIXEL_DIMENSION
    }
};

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

    /// Handle XTSMGRAPHICS (`CSI ? Pi ; Pa ; Pv S`).
    ///
    /// Replies `CSI ? Pi ; Ps [; Pv ...] S` following xterm built with sixel
    /// but without `ReGIS` (`charproc.c`, `CASE_GRAPHICS_ATTRIBUTES`; xterm
    /// `ctlseqs.txt`). The status `Ps` is 0 (success), 1 (error in `Pi`), 2
    /// (error in `Pa`) or 3 (failure); the value fields are present only on
    /// success.
    ///
    /// - `Pi=1` (colour registers): read, reset and read-maximum answer
    ///   [`MAX_PALETTE`], because the register count is fixed; set answers 3.
    /// - `Pi=2` (sixel geometry): read answers the text-area pixel size
    ///   (grid size times cell pixel size) clamped to the decoder maximum;
    ///   read-maximum answers that maximum; reset and set answer 3.
    /// - `Pi=3` (`ReGIS`) answers 1 (error in `Pi`), as xterm does when built
    ///   without `ReGIS`: the item does not exist (`OPT_REGIS_GRAPHICS` compiled
    ///   out falls to the `default:` arm).
    pub fn handle_graphics_attribute_request(
        &mut self,
        item: GraphicsAttributeItem,
        action: GraphicsAttributeAction,
    ) {
        use GraphicsAttributeAction as Action;

        let pi = item.code();
        let reply = match item {
            GraphicsAttributeItem::ColorRegisters => match action {
                Action::Read | Action::Reset | Action::ReadMaximum => {
                    format!("?{pi};{XTSMGRAPHICS_OK};{MAX_PALETTE}S")
                }
                Action::Set => format!("?{pi};{XTSMGRAPHICS_FAILURE}S"),
                Action::Unknown(_) => format!("?{pi};{XTSMGRAPHICS_BAD_ACTION}S"),
            },
            GraphicsAttributeItem::SixelGeometry => match action {
                Action::Read => {
                    let (cols, rows) = self.win_size();
                    let cell_w = usize::value_from(self.cell_pixel_width).unwrap_or(usize::MAX);
                    let cell_h = usize::value_from(self.cell_pixel_height).unwrap_or(usize::MAX);
                    let width = cols.saturating_mul(cell_w).min(MAX_SIXEL_GEOMETRY);
                    let height = rows.saturating_mul(cell_h).min(MAX_SIXEL_GEOMETRY);
                    format!("?{pi};{XTSMGRAPHICS_OK};{width};{height}S")
                }
                Action::ReadMaximum => {
                    format!("?{pi};{XTSMGRAPHICS_OK};{MAX_SIXEL_GEOMETRY};{MAX_SIXEL_GEOMETRY}S")
                }
                Action::Reset | Action::Set => format!("?{pi};{XTSMGRAPHICS_FAILURE}S"),
                Action::Unknown(_) => format!("?{pi};{XTSMGRAPHICS_BAD_ACTION}S"),
            },
            GraphicsAttributeItem::ReGisGeometry | GraphicsAttributeItem::Unknown(_) => {
                format!("?{pi};{XTSMGRAPHICS_BAD_ITEM}S")
            }
        };
        self.write_csi_response(&reply);
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

#[cfg(test)]
mod tests {
    use crate::terminal_handler::TerminalHandler;
    use freminal_common::pty_write::PtyWrite;

    fn recv_pty_response(rx: &crossbeam_channel::Receiver<PtyWrite>) -> String {
        let Ok(PtyWrite::Write(bytes)) = rx.try_recv() else {
            panic!("expected PtyWrite::Write response");
        };
        String::from_utf8(bytes).expect("response should be valid UTF-8")
    }

    /// Regression guard: the XTVERSION / XTGETTCAP device-name
    /// response must keep the `>|XTerm(Freminal ...)` shape. The `XTerm(`
    /// prefix is load-bearing — tmux only enables `extkeys`
    /// (`modifyOtherKeys`) when it recognises the prefix
    /// (`tty_keys_extended_device_attributes`). Dropping it regresses
    /// extended-key forwarding inside tmux. The `Freminal` token is the
    /// freminal-identifying part of the payload.
    #[test]
    fn device_name_and_version_keeps_xterm_prefix_and_freminal_token() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        handler.handle_device_name_and_version();

        let response = recv_pty_response(&rx);
        let version = env!("CARGO_PKG_VERSION");
        // 7-bit DCS framing: ESC P {body} ESC \
        assert_eq!(
            response,
            format!("\x1bP>|XTerm(Freminal {version})\x1b\\"),
            "XTVERSION response must keep the tmux-compatible XTerm( prefix"
        );
    }
}
