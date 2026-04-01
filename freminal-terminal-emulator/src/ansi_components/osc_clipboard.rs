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
