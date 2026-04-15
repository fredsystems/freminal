// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use crate::ansi_components::tracer::SequenceTracer;
use freminal_common::buffer_states::osc::{AnsiOscToken, AnsiOscType};
use freminal_common::buffer_states::terminal_output::TerminalOutput;

/// Handle OSC 52 clipboard set/query.
///
/// `params[0]` = `OscValue(52)`, `params[1]` = selection string, `params[2]` = base64 or `?`.
pub(super) fn handle_osc_clipboard(
    params: &[Option<AnsiOscToken>],
    seq_trace: &SequenceTracer,
    output: &mut Vec<TerminalOutput>,
) {
    let selection = match params.get(1) {
        Some(Some(AnsiOscToken::String(s))) => s.clone(),
        _ => "c".to_string(), // default to clipboard
    };

    match params.get(2) {
        Some(Some(AnsiOscToken::String(data))) if data == "?" => {
            output.push(TerminalOutput::OscResponse(AnsiOscType::QueryClipboard(
                selection,
            )));
        }
        Some(Some(AnsiOscToken::String(data))) => match freminal_common::base64::decode(data) {
            Ok(decoded_bytes) => {
                let content = String::from_utf8_lossy(&decoded_bytes).into_owned();
                output.push(TerminalOutput::OscResponse(AnsiOscType::SetClipboard(
                    selection, content,
                )));
            }
            Err(e) => {
                tracing::warn!("OSC 52: invalid base64 payload: {e}");
            }
        },
        _ => {
            tracing::warn!(
                "OSC 52: missing or invalid payload: recent='{}'",
                seq_trace.as_str()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::osc::AnsiOscParser;
    use freminal_common::buffer_states::osc::AnsiOscType;
    use freminal_common::buffer_states::terminal_output::TerminalOutput;

    fn feed_osc(payload: &[u8]) -> Vec<TerminalOutput> {
        let mut parser = AnsiOscParser::new();
        let mut output = Vec::new();
        for &b in payload {
            parser.ansiparser_inner_osc(b, &mut output);
        }
        output
    }

    #[test]
    fn osc52_query_default_clipboard_selection() {
        // OSC 52 ; c ; ? BEL — query the clipboard
        let payload = b"52;c;?\x07";
        let output = feed_osc(payload);
        assert_eq!(output.len(), 1);
        match &output[0] {
            TerminalOutput::OscResponse(AnsiOscType::QueryClipboard(sel)) => {
                assert_eq!(sel, "c");
            }
            other => panic!("Expected QueryClipboard, got: {other:?}"),
        }
    }

    #[test]
    fn osc52_set_clipboard_valid_base64() {
        // OSC 52 ; c ; <base64("hello")> BEL
        let b64 = freminal_common::base64::encode(b"hello");
        let mut payload = format!("52;c;{b64}").into_bytes();
        payload.push(0x07);
        let output = feed_osc(&payload);
        assert_eq!(output.len(), 1);
        match &output[0] {
            TerminalOutput::OscResponse(AnsiOscType::SetClipboard(sel, content)) => {
                assert_eq!(sel, "c");
                assert_eq!(content, "hello");
            }
            other => panic!("Expected SetClipboard, got: {other:?}"),
        }
    }

    #[test]
    fn osc52_set_clipboard_custom_selection() {
        // OSC 52 ; p ; <base64("world")> BEL — selection "p" (primary)
        let b64 = freminal_common::base64::encode(b"world");
        let mut payload = format!("52;p;{b64}").into_bytes();
        payload.push(0x07);
        let output = feed_osc(&payload);
        assert_eq!(output.len(), 1);
        match &output[0] {
            TerminalOutput::OscResponse(AnsiOscType::SetClipboard(sel, content)) => {
                assert_eq!(sel, "p");
                assert_eq!(content, "world");
            }
            other => panic!("Expected SetClipboard, got: {other:?}"),
        }
    }

    #[test]
    fn osc52_invalid_base64_no_output() {
        // OSC 52 ; c ; !!!invalid!!! BEL — invalid base64 → warn, no output
        let payload = b"52;c;!!!invalid!!!\x07";
        let output = feed_osc(payload);
        assert!(output.is_empty());
    }

    #[test]
    fn osc52_missing_payload_no_output() {
        // OSC 52 BEL — only one param, no payload
        let payload = b"52\x07";
        let output = feed_osc(payload);
        assert!(output.is_empty());
    }

    #[test]
    fn osc52_query_with_st_terminator() {
        // OSC 52 ; c ; ? ST (ESC \)
        let payload = b"52;c;?\x1b\\";
        let output = feed_osc(payload);
        assert_eq!(output.len(), 1);
        assert!(matches!(
            &output[0],
            TerminalOutput::OscResponse(AnsiOscType::QueryClipboard(_))
        ));
    }

    #[test]
    fn osc52_default_selection_when_missing() {
        // OSC 52 ; ; ? BEL — empty selection → default "c"
        let payload = b"52;;?\x07";
        let output = feed_osc(payload);
        assert_eq!(output.len(), 1);
        match &output[0] {
            TerminalOutput::OscResponse(AnsiOscType::QueryClipboard(sel)) => {
                assert_eq!(sel, "c");
            }
            other => panic!("Expected QueryClipboard, got: {other:?}"),
        }
    }
}
