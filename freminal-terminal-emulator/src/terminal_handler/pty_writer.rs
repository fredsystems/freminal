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

use crate::io::{GuiReply, Osc99CloseTracking, WindowStateReport};

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

    /// Serialise a GUI-originated reply and write it to the PTY.
    ///
    /// Builds the reply body and frames it through [`Self::write_csi_response`]
    /// or [`Self::write_osc_response`], so the introducer and terminator follow
    /// the current S8C1T mode. Absent or empty OSC 99 ids default to `0`.
    ///
    /// Text an application chose and the terminal merely stores (the title,
    /// the icon label, the OSC 52 selection parameter) is sanitised before it
    /// is framed, so a reply can never smuggle a control sequence back into
    /// the application's own input.
    pub fn write_gui_reply(&self, reply: &GuiReply) {
        match reply {
            GuiReply::WindowState(state) => {
                let body = match state {
                    WindowStateReport::Normal => "1t",
                    WindowStateReport::Iconified => "2t",
                };
                self.write_csi_response(body);
            }
            GuiReply::WindowPosition { x, y } => {
                self.write_csi_response(&format!("3;{x};{y}t"));
            }
            GuiReply::WindowSizePixels { height, width } => {
                self.write_csi_response(&format!("4;{height};{width}t"));
            }
            GuiReply::ScreenSizePixels { height, width } => {
                self.write_csi_response(&format!("5;{height};{width}t"));
            }
            GuiReply::IconLabel(label) => {
                self.write_osc_response(&format!("L{}", sanitize_reported_text(label)));
            }
            GuiReply::WindowTitle(title) => {
                self.write_osc_response(&format!("l{}", sanitize_reported_text(title)));
            }
            GuiReply::Clipboard {
                selection,
                base64_payload,
            } => self.write_osc_response(&format!(
                "52;{};{base64_payload}",
                sanitize_selection(selection)
            )),
            GuiReply::Osc99Activation { id, button } => {
                let id = osc99_reply_id(id.as_deref());
                let button = sanitize_osc99_identifier(button.as_deref().unwrap_or(""));
                self.write_osc_response(&format!("99;i={id};{button}"));
            }
            GuiReply::Osc99Closed { id, tracking } => {
                let id = osc99_reply_id(id.as_deref());
                let payload = match tracking {
                    Osc99CloseTracking::Tracked => "",
                    Osc99CloseTracking::Untracked => "untracked",
                };
                self.write_osc_response(&format!("99;i={id}:p=close;{payload}"));
            }
            GuiReply::Osc99Alive {
                request_id,
                live_ids,
            } => {
                let id = osc99_reply_id(request_id.as_deref());
                // An id that sanitises to nothing is not a usable identifier;
                // drop it rather than emit an empty list entry.
                let list = live_ids
                    .iter()
                    .map(|live| sanitize_osc99_identifier(live))
                    .filter(|live| !live.is_empty())
                    .collect::<Vec<_>>()
                    .join(",");
                self.write_osc_response(&format!("99;i={id}:p=alive;{list}"));
            }
        }
    }
}

/// Strip every control character from application-chosen text before it is
/// echoed back in a title or icon-label report.
///
/// An application can store a title containing ESC, and a report that echoes
/// it verbatim lets the application inject arbitrary input into itself (the
/// title-report injection class, CVE-2003-0063). [`char::is_control`] is true
/// for the Unicode general category `Cc`: C0 (U+0000–U+001F), DEL (U+007F) and
/// C1 (U+0080–U+009F), which is exactly the set that must not be echoed.
fn sanitize_reported_text(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).collect()
}

/// Keep only the characters the kitty notification protocol allows in an
/// identifier (`[A-Za-z0-9_+.-]`) when echoing an OSC 99 id, button number or
/// live-id list back to the application.
///
/// The spec requires this: "Terminals must sanitize ids received from client
/// programs before sending them back". The ids are validated when parsed, but
/// that is in another crate; sanitising at the serialiser means a reply cannot
/// carry a control byte whatever a future caller passes in.
pub(in crate::terminal_handler) fn sanitize_osc99_identifier(id: &str) -> String {
    id.chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '+' | '.' | '-'))
        .collect()
}

/// The `i=` value for an OSC 99 reply: the sanitised id, or `0` when the id is
/// absent or sanitises to nothing.
pub(in crate::terminal_handler) fn osc99_reply_id(id: Option<&str>) -> String {
    let sanitized = id.map(sanitize_osc99_identifier).unwrap_or_default();
    if sanitized.is_empty() {
        "0".to_owned()
    } else {
        sanitized
    }
}

/// Keep only the characters xterm defines for the OSC 52 `Pc` parameter
/// (`c`, `p`, `q`, `s` and `0`–`7`, see xterm's ctlseqs) when echoing the
/// selection parameter of a clipboard report.
///
/// The parameter is application-supplied; echoing it verbatim would allow the
/// same injection as an unsanitised title (CVE-2003-0063). An empty result is
/// reported as empty: xterm's "empty means `s0`" rule concerns the request,
/// not the reply.
fn sanitize_selection(selection: &str) -> String {
    selection
        .chars()
        .filter(|c| "cpqs01234567".contains(*c))
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use freminal_common::{buffer_states::modes::s8c1t::S8c1t, pty_write::PtyWrite};

    use crate::io::{GuiReply, Osc99CloseTracking, WindowStateReport};
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

    /// One row of the `GuiReply` serialisation table: the reply, its exact
    /// 7-bit bytes (copied from the GUI's former hand-formatted strings), and
    /// its exact 8-bit bytes.
    struct ReplyCase {
        reply: GuiReply,
        seven_bit: &'static [u8],
        eight_bit: &'static [u8],
    }

    fn s(text: &str) -> String {
        text.to_owned()
    }

    fn reply_cases() -> Vec<ReplyCase> {
        let mut cases = window_reply_cases();
        cases.extend(osc99_reply_cases());
        cases
    }

    /// CSI window reports, title/label reports and OSC 52 answers.
    fn window_reply_cases() -> Vec<ReplyCase> {
        vec![
            ReplyCase {
                reply: GuiReply::WindowState(WindowStateReport::Normal),
                seven_bit: b"\x1b[1t",
                eight_bit: b"\x9b1t",
            },
            ReplyCase {
                reply: GuiReply::WindowState(WindowStateReport::Iconified),
                seven_bit: b"\x1b[2t",
                eight_bit: b"\x9b2t",
            },
            ReplyCase {
                reply: GuiReply::WindowPosition { x: 10, y: 20 },
                seven_bit: b"\x1b[3;10;20t",
                eight_bit: b"\x9b3;10;20t",
            },
            ReplyCase {
                reply: GuiReply::WindowSizePixels {
                    height: 600,
                    width: 800,
                },
                seven_bit: b"\x1b[4;600;800t",
                eight_bit: b"\x9b4;600;800t",
            },
            ReplyCase {
                reply: GuiReply::ScreenSizePixels {
                    height: 1080,
                    width: 1920,
                },
                seven_bit: b"\x1b[5;1080;1920t",
                eight_bit: b"\x9b5;1080;1920t",
            },
            ReplyCase {
                reply: GuiReply::IconLabel(s("Label")),
                seven_bit: b"\x1b]LLabel\x1b\\",
                eight_bit: b"\x9dLLabel\x9c",
            },
            ReplyCase {
                reply: GuiReply::WindowTitle(s("Title")),
                seven_bit: b"\x1b]lTitle\x1b\\",
                eight_bit: b"\x9dlTitle\x9c",
            },
            ReplyCase {
                reply: GuiReply::Clipboard {
                    selection: s("c"),
                    base64_payload: s("aGk="),
                },
                seven_bit: b"\x1b]52;c;aGk=\x1b\\",
                eight_bit: b"\x9d52;c;aGk=\x9c",
            },
            ReplyCase {
                reply: GuiReply::Clipboard {
                    selection: s("c"),
                    base64_payload: String::new(),
                },
                seven_bit: b"\x1b]52;c;\x1b\\",
                eight_bit: b"\x9d52;c;\x9c",
            },
        ]
    }

    /// OSC 99 activation, close and alive reports.
    fn osc99_reply_cases() -> Vec<ReplyCase> {
        vec![
            ReplyCase {
                reply: GuiReply::Osc99Activation {
                    id: Some(s("abc")),
                    button: None,
                },
                seven_bit: b"\x1b]99;i=abc;\x1b\\",
                eight_bit: b"\x9d99;i=abc;\x9c",
            },
            ReplyCase {
                reply: GuiReply::Osc99Activation {
                    id: Some(s("abc")),
                    button: Some(s("2")),
                },
                seven_bit: b"\x1b]99;i=abc;2\x1b\\",
                eight_bit: b"\x9d99;i=abc;2\x9c",
            },
            ReplyCase {
                reply: GuiReply::Osc99Activation {
                    id: None,
                    button: None,
                },
                seven_bit: b"\x1b]99;i=0;\x1b\\",
                eight_bit: b"\x9d99;i=0;\x9c",
            },
            ReplyCase {
                reply: GuiReply::Osc99Closed {
                    id: Some(s("abc")),
                    tracking: Osc99CloseTracking::Tracked,
                },
                seven_bit: b"\x1b]99;i=abc:p=close;\x1b\\",
                eight_bit: b"\x9d99;i=abc:p=close;\x9c",
            },
            ReplyCase {
                reply: GuiReply::Osc99Closed {
                    id: Some(s("abc")),
                    tracking: Osc99CloseTracking::Untracked,
                },
                seven_bit: b"\x1b]99;i=abc:p=close;untracked\x1b\\",
                eight_bit: b"\x9d99;i=abc:p=close;untracked\x9c",
            },
            ReplyCase {
                reply: GuiReply::Osc99Closed {
                    id: None,
                    tracking: Osc99CloseTracking::Untracked,
                },
                seven_bit: b"\x1b]99;i=0:p=close;untracked\x1b\\",
                eight_bit: b"\x9d99;i=0:p=close;untracked\x9c",
            },
            ReplyCase {
                reply: GuiReply::Osc99Alive {
                    request_id: Some(s("q1")),
                    live_ids: vec![s("a"), s("b")],
                },
                seven_bit: b"\x1b]99;i=q1:p=alive;a,b\x1b\\",
                eight_bit: b"\x9d99;i=q1:p=alive;a,b\x9c",
            },
            ReplyCase {
                reply: GuiReply::Osc99Alive {
                    request_id: Some(s("q1")),
                    live_ids: vec![],
                },
                seven_bit: b"\x1b]99;i=q1:p=alive;\x1b\\",
                eight_bit: b"\x9d99;i=q1:p=alive;\x9c",
            },
            ReplyCase {
                reply: GuiReply::Osc99Alive {
                    request_id: None,
                    live_ids: vec![s("x")],
                },
                seven_bit: b"\x1b]99;i=0:p=alive;x\x1b\\",
                eight_bit: b"\x9d99;i=0:p=alive;x\x9c",
            },
        ]
    }

    #[test]
    fn write_gui_reply_seven_bit_bytes_for_every_variant() {
        for case in reply_cases() {
            let (handler, rx) = handler_with_rx(S8c1t::SevenBit);
            handler.write_gui_reply(&case.reply);
            assert_eq!(
                recv_bytes(&rx),
                case.seven_bit.to_vec(),
                "7-bit bytes for {:?}",
                case.reply
            );
            assert!(
                rx.try_recv().is_err(),
                "exactly one write for {:?}",
                case.reply
            );
        }
    }

    #[test]
    fn write_gui_reply_eight_bit_bytes_for_every_variant() {
        for case in reply_cases() {
            let (handler, rx) = handler_with_rx(S8c1t::EightBit);
            handler.write_gui_reply(&case.reply);
            assert_eq!(
                recv_bytes(&rx),
                case.eight_bit.to_vec(),
                "8-bit bytes for {:?}",
                case.reply
            );
            assert!(
                rx.try_recv().is_err(),
                "exactly one write for {:?}",
                case.reply
            );
        }
    }

    /// Title / label carrying ESC, BEL, U+009B (C1 CSI) and DEL around ordinary text.
    const HOSTILE_TEXT: &str = "A\u{1b}[6nB\u{7}\u{9b}\u{7f}Z";

    #[test]
    fn window_title_reply_drops_control_characters() {
        let (handler, rx) = handler_with_rx(S8c1t::SevenBit);
        handler.write_gui_reply(&GuiReply::WindowTitle(HOSTILE_TEXT.to_string()));
        assert_eq!(recv_bytes(&rx), b"\x1b]lA[6nBZ\x1b\\".to_vec());

        let (handler, rx) = handler_with_rx(S8c1t::EightBit);
        handler.write_gui_reply(&GuiReply::WindowTitle(HOSTILE_TEXT.to_string()));
        assert_eq!(recv_bytes(&rx), b"\x9dlA[6nBZ\x9c".to_vec());
    }

    #[test]
    fn icon_label_reply_drops_control_characters() {
        let (handler, rx) = handler_with_rx(S8c1t::SevenBit);
        handler.write_gui_reply(&GuiReply::IconLabel(HOSTILE_TEXT.to_string()));
        assert_eq!(recv_bytes(&rx), b"\x1b]LA[6nBZ\x1b\\".to_vec());

        let (handler, rx) = handler_with_rx(S8c1t::EightBit);
        handler.write_gui_reply(&GuiReply::IconLabel(HOSTILE_TEXT.to_string()));
        assert_eq!(recv_bytes(&rx), b"\x9dLA[6nBZ\x9c".to_vec());
    }

    #[test]
    fn plain_title_reply_is_unchanged() {
        let (handler, rx) = handler_with_rx(S8c1t::SevenBit);
        handler.write_gui_reply(&GuiReply::WindowTitle("vim: main.rs".to_string()));
        assert_eq!(recv_bytes(&rx), b"\x1b]lvim: main.rs\x1b\\".to_vec());
    }

    #[test]
    fn clipboard_reply_keeps_only_selection_characters() {
        let (handler, rx) = handler_with_rx(S8c1t::SevenBit);
        handler.write_gui_reply(&GuiReply::Clipboard {
            selection: "c\u{1b}[6n".to_string(),
            base64_payload: "aGk=".to_string(),
        });
        // `6` is a valid `Pc` digit, so it survives; ESC, `[` and `n` do not.
        assert_eq!(recv_bytes(&rx), b"\x1b]52;c6;aGk=\x1b\\".to_vec());

        let (handler, rx) = handler_with_rx(S8c1t::EightBit);
        handler.write_gui_reply(&GuiReply::Clipboard {
            selection: "c\u{1b}[Hn\u{9b}".to_string(),
            base64_payload: "aGk=".to_string(),
        });
        assert_eq!(recv_bytes(&rx), b"\x9d52;c;aGk=\x9c".to_vec());
    }

    #[test]
    fn clipboard_reply_valid_selection_is_unchanged() {
        let (handler, rx) = handler_with_rx(S8c1t::SevenBit);
        handler.write_gui_reply(&GuiReply::Clipboard {
            selection: "cps07".to_string(),
            base64_payload: String::new(),
        });
        assert_eq!(recv_bytes(&rx), b"\x1b]52;cps07;\x1b\\".to_vec());
    }

    #[test]
    fn clipboard_reply_empty_selection_stays_empty() {
        let (handler, rx) = handler_with_rx(S8c1t::SevenBit);
        handler.write_gui_reply(&GuiReply::Clipboard {
            selection: String::new(),
            base64_payload: "aGk=".to_string(),
        });
        assert_eq!(recv_bytes(&rx), b"\x1b]52;;aGk=\x1b\\".to_vec());
    }

    /// An id carrying ESC, BEL, a C1 CSI, spaces and the separators `:`/`;`/`,`
    /// around a valid identifier.
    const HOSTILE_ID: &str = "a\u{1b}[6nb\u{7}\u{9b}:;, c_+.-9";
    /// What [`HOSTILE_ID`] reduces to under `[A-Za-z0-9_+.-]`.
    const CLEAN_ID: &str = "a6nbc_+.-9";

    #[test]
    fn osc99_activation_reply_sanitises_id_and_button() {
        let (handler, rx) = handler_with_rx(S8c1t::SevenBit);
        handler.write_gui_reply(&GuiReply::Osc99Activation {
            id: Some(HOSTILE_ID.to_string()),
            button: Some("2\u{1b}[6n".to_string()),
        });
        assert_eq!(
            recv_bytes(&rx),
            format!("\x1b]99;i={CLEAN_ID};26n\x1b\\").into_bytes()
        );
    }

    #[test]
    fn osc99_closed_reply_sanitises_id() {
        let (handler, rx) = handler_with_rx(S8c1t::EightBit);
        handler.write_gui_reply(&GuiReply::Osc99Closed {
            id: Some(HOSTILE_ID.to_string()),
            tracking: Osc99CloseTracking::Tracked,
        });
        let mut expected = vec![0x9d];
        expected.extend_from_slice(format!("99;i={CLEAN_ID}:p=close;").as_bytes());
        expected.push(0x9c);
        assert_eq!(recv_bytes(&rx), expected);
    }

    #[test]
    fn osc99_alive_reply_sanitises_request_id_and_live_ids() {
        let (handler, rx) = handler_with_rx(S8c1t::SevenBit);
        handler.write_gui_reply(&GuiReply::Osc99Alive {
            request_id: Some(HOSTILE_ID.to_string()),
            live_ids: vec![
                HOSTILE_ID.to_string(),
                "\u{1b}\u{7}".to_string(),
                "ok".to_string(),
            ],
        });
        // The id that sanitises to nothing is dropped, not sent as an empty entry.
        assert_eq!(
            recv_bytes(&rx),
            format!("\x1b]99;i={CLEAN_ID}:p=alive;{CLEAN_ID},ok\x1b\\").into_bytes()
        );
    }

    #[test]
    fn osc99_empty_or_all_hostile_id_is_sent_as_zero() {
        for id in [
            None,
            Some(String::new()),
            Some("\u{1b}\u{7} :;".to_string()),
        ] {
            let (handler, rx) = handler_with_rx(S8c1t::SevenBit);
            handler.write_gui_reply(&GuiReply::Osc99Activation {
                id: id.clone(),
                button: None,
            });
            assert_eq!(recv_bytes(&rx), b"\x1b]99;i=0;\x1b\\".to_vec(), "{id:?}");

            handler.write_gui_reply(&GuiReply::Osc99Closed {
                id: id.clone(),
                tracking: Osc99CloseTracking::Tracked,
            });
            assert_eq!(
                recv_bytes(&rx),
                b"\x1b]99;i=0:p=close;\x1b\\".to_vec(),
                "{id:?}"
            );

            handler.write_gui_reply(&GuiReply::Osc99Alive {
                request_id: id.clone(),
                live_ids: vec![],
            });
            assert_eq!(
                recv_bytes(&rx),
                b"\x1b]99;i=0:p=alive;\x1b\\".to_vec(),
                "{id:?}"
            );
        }
    }

    #[test]
    fn sanitize_osc99_identifier_keeps_exactly_the_identifier_set() {
        assert_eq!(super::sanitize_osc99_identifier(HOSTILE_ID), CLEAN_ID);
        assert_eq!(super::sanitize_osc99_identifier("Az09_+.-"), "Az09_+.-");
        assert_eq!(super::sanitize_osc99_identifier(""), "");
        assert_eq!(super::osc99_reply_id(Some("\u{1b}")), "0");
        assert_eq!(super::osc99_reply_id(Some("x1")), "x1");
        assert_eq!(super::osc99_reply_id(None), "0");
    }

    #[test]
    fn write_gui_reply_without_channel_is_silent() {
        let handler = TerminalHandler::new(80, 24);
        handler.write_gui_reply(&GuiReply::WindowState(WindowStateReport::Normal));
    }
}
