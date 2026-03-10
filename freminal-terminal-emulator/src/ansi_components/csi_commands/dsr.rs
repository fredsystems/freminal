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
/// ESC [ Ps n
/// # Errors
/// Will return an error if the parameter is not a valid number
pub fn ansi_parser_inner_csi_finished_dsr(
    params: &[u8],
    output: &mut Vec<TerminalOutput>,
) -> ParserOutcome {
    let Ok(param) = parse_param_as::<usize>(params) else {
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
        _ => {
            warn!("Unhandled DSR Ps value: {param:?}");
            output.push(TerminalOutput::Invalid);
        }
    }

    ParserOutcome::Finished
}
