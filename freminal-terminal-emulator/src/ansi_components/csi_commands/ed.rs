// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use crate::ansi::{ParserOutcome, parse_param_as};
use crate::error::ParserFailures;
use freminal_common::buffer_states::terminal_output::TerminalOutput;

/// ED — Erase in Display (`CSI Ps J`)
///
/// Erase part of the display:
/// - Ps = 0 → From cursor to end of display (default)
/// - Ps = 1 → From start of display to cursor
/// - Ps = 2 → Entire display
/// - Ps = 3 → Entire display including scrollback buffer
pub fn ansi_parser_inner_csi_finished_ed(
    params: &[u8],
    output: &mut Vec<TerminalOutput>,
) -> ParserOutcome {
    let Ok(param) = parse_param_as::<usize>(params) else {
        return ParserOutcome::InvalidParserFailure(ParserFailures::UnhandledEDCommand(format!(
            "{params:?}"
        )));
    };

    let ret = match param.unwrap_or(0) {
        0 => TerminalOutput::ClearDisplayfromCursortoEndofDisplay,
        1 => TerminalOutput::ClearDisplayfromStartofDisplaytoCursor,
        2 => TerminalOutput::ClearDisplay,
        3 => TerminalOutput::ClearScrollbackandDisplay,
        _ => TerminalOutput::Invalid,
    };
    output.push(ret);

    ParserOutcome::Finished
}
