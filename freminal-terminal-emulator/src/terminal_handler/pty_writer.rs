// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! PTY response encoding for [`TerminalHandler`].
//!
//! This module contains all functions responsible for writing responses back to
//! the PTY file descriptor:
//!
//! - S8C1T / 7-bit C1 control introducer helpers (`csi_response`,
//!   `apc_response`, `dcs_response`, `osc_response`, `st_response`)
//! - Low-level byte-write primitives (`write_bytes_to_pty`, `write_to_pty`)
//! - High-level response writers (`write_apc_response`, `write_csi_response`,
//!   `write_dcs_response`, `write_osc_response`)

use freminal_common::{buffer_states::modes::s8c1t::S8c1t, pty_write::PtyWrite};

use super::TerminalHandler;

impl TerminalHandler {
    /// Return the APC introducer for PTY responses: `0x9F` in 8-bit mode,
    /// `ESC _` in 7-bit mode.
    pub(super) const fn apc_response(&self) -> &'static [u8] {
        match self.s8c1t_mode {
            S8c1t::EightBit => &[0x9F],
            S8c1t::SevenBit => b"\x1b_",
        }
    }

    /// Return the CSI introducer for PTY responses: `0x9B` in 8-bit mode,
    /// `ESC [` in 7-bit mode.
    pub(super) const fn csi_response(&self) -> &'static [u8] {
        match self.s8c1t_mode {
            S8c1t::EightBit => &[0x9B],
            S8c1t::SevenBit => b"\x1b[",
        }
    }

    /// Return the DCS introducer for PTY responses: `0x90` in 8-bit mode,
    /// `ESC P` in 7-bit mode.
    pub(super) const fn dcs_response(&self) -> &'static [u8] {
        match self.s8c1t_mode {
            S8c1t::EightBit => &[0x90],
            S8c1t::SevenBit => b"\x1bP",
        }
    }

    /// Return the OSC introducer for PTY responses: `0x9D` in 8-bit mode,
    /// `ESC ]` in 7-bit mode.
    pub(super) const fn osc_response(&self) -> &'static [u8] {
        match self.s8c1t_mode {
            S8c1t::EightBit => &[0x9D],
            S8c1t::SevenBit => b"\x1b]",
        }
    }

    /// Return the ST (String Terminator) for PTY responses: `0x9C` in 8-bit
    /// mode, `ESC \` in 7-bit mode.
    pub(super) const fn st_response(&self) -> &'static [u8] {
        match self.s8c1t_mode {
            S8c1t::EightBit => &[0x9C],
            S8c1t::SevenBit => b"\x1b\\",
        }
    }

    /// Send a raw string response to the PTY.  Silently drops if no channel is set.
    ///
    /// Replies are written verbatim.  They are never wrapped in a DCS tmux
    /// passthrough envelope, even when the query that provoked them arrived
    /// inside one: tmux does not unwrap application-bound passthrough.
    pub(super) fn write_to_pty(&self, text: &str) {
        self.write_bytes_to_pty(text.as_bytes());
    }

    /// Write raw bytes back to the PTY.  Silently drops if no channel is set.
    fn write_bytes_to_pty(&self, data: &[u8]) {
        if let Some(tx) = &self.write_tx
            && let Err(e) = tx.send(PtyWrite::Write(data.to_vec()))
        {
            tracing::error!("Failed to write to PTY: {e}");
        }
    }

    /// Write an APC response to the PTY using the correct C1 encoding.
    ///
    /// Sends `APC {body} ST` where APC and ST use 8-bit or 7-bit forms
    /// depending on the current S8C1T mode.  Goes through
    /// [`Self::write_bytes_to_pty`].
    pub(super) fn write_apc_response(&self, body: &str) {
        let mut buf = Vec::with_capacity(4 + body.len());
        buf.extend_from_slice(self.apc_response());
        buf.extend_from_slice(body.as_bytes());
        buf.extend_from_slice(self.st_response());
        self.write_bytes_to_pty(&buf);
    }

    /// Write a CSI response to the PTY using the correct C1 encoding.
    ///
    /// Sends `CSI {body}` where CSI is `0x9B` (8-bit) or `ESC [` (7-bit)
    /// depending on the current S8C1T mode.
    pub(crate) fn write_csi_response(&self, body: &str) {
        let mut buf = Vec::with_capacity(2 + body.len());
        buf.extend_from_slice(self.csi_response());
        buf.extend_from_slice(body.as_bytes());
        self.write_bytes_to_pty(&buf);
    }

    /// Write a DCS response to the PTY using the correct C1 encoding.
    ///
    /// Sends `DCS {body} ST` where DCS and ST use 8-bit or 7-bit forms
    /// depending on the current S8C1T mode.
    pub(super) fn write_dcs_response(&self, body: &str) {
        let mut buf = Vec::with_capacity(4 + body.len());
        buf.extend_from_slice(self.dcs_response());
        buf.extend_from_slice(body.as_bytes());
        buf.extend_from_slice(self.st_response());
        self.write_bytes_to_pty(&buf);
    }

    /// Write an OSC response to the PTY using the correct C1 encoding.
    ///
    /// Sends `OSC {body} ST` where OSC and ST use 8-bit or 7-bit forms
    /// depending on the current S8C1T mode.
    pub(super) fn write_osc_response(&self, body: &str) {
        let mut buf = Vec::with_capacity(4 + body.len());
        buf.extend_from_slice(self.osc_response());
        buf.extend_from_slice(body.as_bytes());
        buf.extend_from_slice(self.st_response());
        self.write_bytes_to_pty(&buf);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use freminal_common::{buffer_states::modes::s8c1t::S8c1t, pty_write::PtyWrite};

    use crate::terminal_handler::TerminalHandler;

    /// Build a handler wired to a channel and in the given S8C1T mode.
    fn handler_with_rx(mode: S8c1t) -> (TerminalHandler, crossbeam_channel::Receiver<PtyWrite>) {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);
        handler.set_s8c1t_mode(mode);
        (handler, rx)
    }

    /// Receive the single queued write as raw bytes.
    fn recv_bytes(rx: &crossbeam_channel::Receiver<PtyWrite>) -> Vec<u8> {
        let PtyWrite::Write(bytes) = rx.try_recv().unwrap() else {
            panic!("expected PtyWrite::Write");
        };
        bytes
    }

    #[test]
    fn apc_response_introducer_follows_s8c1t_mode() {
        let mut handler = TerminalHandler::new(80, 24);
        assert_eq!(handler.apc_response(), b"\x1b_");
        handler.set_s8c1t_mode(S8c1t::EightBit);
        assert_eq!(handler.apc_response(), &[0x9F]);
    }

    #[test]
    fn write_apc_response_seven_bit_framing() {
        let (handler, rx) = handler_with_rx(S8c1t::SevenBit);
        handler.write_apc_response("Gi=1;OK");
        assert_eq!(recv_bytes(&rx), b"\x1b_Gi=1;OK\x1b\\".to_vec());
    }

    #[test]
    fn write_apc_response_eight_bit_framing() {
        let (handler, rx) = handler_with_rx(S8c1t::EightBit);
        handler.write_apc_response("Gi=1;OK");
        assert_eq!(recv_bytes(&rx), b"\x9fGi=1;OK\x9c".to_vec());
    }

    #[test]
    fn write_osc_response_framing_seven_and_eight_bit() {
        let (handler, rx) = handler_with_rx(S8c1t::SevenBit);
        handler.write_osc_response("11;rgb:00/00/00");
        assert_eq!(recv_bytes(&rx), b"\x1b]11;rgb:00/00/00\x1b\\".to_vec());

        let (handler, rx) = handler_with_rx(S8c1t::EightBit);
        handler.write_osc_response("11;rgb:00/00/00");
        assert_eq!(recv_bytes(&rx), b"\x9d11;rgb:00/00/00\x9c".to_vec());
    }

    #[test]
    fn write_dcs_response_framing_seven_and_eight_bit() {
        let (handler, rx) = handler_with_rx(S8c1t::SevenBit);
        handler.write_dcs_response("0$r");
        assert_eq!(recv_bytes(&rx), b"\x1bP0$r\x1b\\".to_vec());

        let (handler, rx) = handler_with_rx(S8c1t::EightBit);
        handler.write_dcs_response("0$r");
        assert_eq!(recv_bytes(&rx), b"\x900$r\x9c".to_vec());
    }

    #[test]
    fn direct_write_not_wrapped() {
        // When a Kitty query arrives directly (not via tmux passthrough),
        // the response should be a bare APC, not wrapped.
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // Direct APC (no tmux wrapping)
        let apc = b"_Ga=q,i=1;\x1b\\";
        handler.handle_application_program_command(apc);

        let response = rx.try_recv();
        assert!(response.is_ok(), "Expected a Kitty response");
        let PtyWrite::Write(bytes) = response.unwrap() else {
            panic!("expected PtyWrite::Write");
        };
        let resp_str = String::from_utf8_lossy(&bytes);
        // Should be a bare APC, NOT wrapped in tmux passthrough
        assert!(
            resp_str.starts_with("\x1b_G"),
            "Expected bare APC response, got: {resp_str}"
        );
        assert!(
            !resp_str.starts_with("\x1bPtmux;"),
            "Direct query should NOT produce tmux-wrapped response"
        );
    }
}
