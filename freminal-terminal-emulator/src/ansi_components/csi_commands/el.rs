// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use crate::ansi::{ParserOutcome, parse_param_as};
use crate::error::ParserFailures;
use freminal_common::buffer_states::terminal_output::TerminalOutput;

/// EL — Erase in Line (`CSI Ps K`)
///
/// Erase part of the current line:
/// - Ps = 0 → From cursor to end of line (default)
/// - Ps = 1 → From start of line to cursor
/// - Ps = 2 → Entire line
pub fn ansi_parser_inner_csi_finished_el(
    params: &[u8],
    output: &mut Vec<TerminalOutput>,
) -> ParserOutcome {
    let Ok(param) = parse_param_as::<usize>(params) else {
        return ParserOutcome::InvalidParserFailure(ParserFailures::UnhandledELCommand(format!(
            "{params:?}"
        )));
    };

    // ECMA-48 8.3.39
    match param.unwrap_or(0) {
        0 => output.push(TerminalOutput::ClearLineForwards),
        1 => output.push(TerminalOutput::ClearLineBackwards),
        2 => output.push(TerminalOutput::ClearLine),
        v => {
            warn!("Unsupported erase in line command ({v})");
            output.push(TerminalOutput::Invalid);
        }
    }

    ParserOutcome::Finished
}
