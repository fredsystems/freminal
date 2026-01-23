// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use crate::ansi::{ParserOutcome, parse_param_as};
use crate::error::ParserFailures;
use freminal_common::buffer_states::terminal_output::TerminalOutput;

/// Request device name and version
///
/// ESC [ > Ps q
///
/// # Errors
/// Will return an error if the parameter is not a valid number
pub fn ansi_parser_inner_csi_finished_report_version_q(
    params: &[u8],
    output: &mut Vec<TerminalOutput>,
) -> ParserOutcome {
    let Ok(param) = parse_param_as::<usize>(&[*params.get(1).unwrap_or(&b'0')]) else {
        return ParserOutcome::InvalidParserFailure(ParserFailures::UnhandledXTVERSIONCommand(
            String::from_utf8_lossy(params).to_string(),
        ));
    };

    let request = param.unwrap_or(0);

    if request == 0 {
        output.push(TerminalOutput::RequestDeviceNameAndVersion);
    } else {
        return ParserOutcome::InvalidParserFailure(ParserFailures::UnhandledXTVERSIONCommand(
            String::from_utf8_lossy(params).to_string(),
        ));
    }

    ParserOutcome::Finished
}
