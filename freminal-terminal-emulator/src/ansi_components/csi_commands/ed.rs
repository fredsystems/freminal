// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use crate::ansi::{ParserOutcome, parse_param_as};
use crate::error::ParserFailures;
use freminal_common::buffer_states::terminal_output::TerminalOutput;

/// Erase in Display
///
/// ED clears part of the screen.
///
/// Values for param:
/// 0 - Erase from the cursor to the end of the screen (default)
/// 1 - Erase from the beginning of the screen to the cursor
/// 2 - Erase the entire screen
/// 3 - Erase the entire screen including the scrollback buffer
///
/// ESC [ Pn J
/// # Errors
/// Will return an error if the parameter is not a valid number
pub fn ansi_parser_inner_csi_finished_set_position_j(
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
