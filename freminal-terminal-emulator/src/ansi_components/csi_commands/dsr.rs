// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use crate::ansi::{ParserOutcome, parse_param_as};
use crate::error::ParserFailures;
use freminal_common::buffer_states::terminal_output::TerminalOutput;

/// Device Status Report (DSR)
///
/// DSR requests status information from the terminal.
///
/// Values for Ps:
/// 5 - Device status report: respond with CSI 0 n (device OK)
/// 6 - Cursor position report: respond with CSI row ; col R
///
/// DEC private variants (with `?` prefix):
/// ?5 - DEC device status report: respond with CSI 0 n (device OK)
/// ?6 - DEC cursor position report (DECXCPR): respond with CSI row ; col R
/// ?996 - Color theme query: respond with CSI ? 997 ; Ps n
///        where Ps = 1 (light) or 2 (dark)
///
/// ESC [ Ps n        (standard DSR)
/// ESC [ ? Ps n      (DEC private DSR)
/// # Errors
/// Will return an error if the parameter is not a valid number
pub fn ansi_parser_inner_csi_finished_dsr(
    params: &[u8],
    output: &mut Vec<TerminalOutput>,
) -> ParserOutcome {
    // DEC private DSR: params start with '?' (e.g. "?6" for ESC[?6n)
    let is_private = params.first() == Some(&b'?');
    let actual_params = if is_private { &params[1..] } else { params };

    let Ok(param) = parse_param_as::<usize>(actual_params) else {
        warn!("Invalid DSR command");
        output.push(TerminalOutput::Invalid);

        return ParserOutcome::InvalidParserFailure(ParserFailures::UnhandledDSRCommand(format!(
            "{params:?}"
        )));
    };

    match param.unwrap_or(5) {
        5 => {
            output.push(TerminalOutput::DeviceStatusReport);
        }
        6 => {
            output.push(TerminalOutput::CursorReport);
        }
        996 if is_private => {
            output.push(TerminalOutput::ColorThemeReport);
        }
        _ => {
            warn!("Unhandled DSR Ps value: {param:?}");
            output.push(TerminalOutput::Invalid);
        }
    }

    ParserOutcome::Finished
}
