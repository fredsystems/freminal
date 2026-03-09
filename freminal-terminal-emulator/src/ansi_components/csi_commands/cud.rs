// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use crate::ansi::{ParserOutcome, parse_param_as};
use crate::error::ParserFailures;
use freminal_common::buffer_states::terminal_output::TerminalOutput;

/// Cursor Down
///
/// CUD moves the cursor down by a specified number of lines without changing columns.
///
/// ESC [ Pn B
/// # Errors
/// Will return an error if the parameter is not a valid number
pub fn ansi_parser_inner_csi_finished_move_down(
    params: &[u8],
    output: &mut Vec<TerminalOutput>,
) -> ParserOutcome {
    let Ok(param) = parse_param_as::<i32>(params) else {
        return ParserOutcome::InvalidParserFailure(ParserFailures::UnhandledCUDCommand(
            String::from_utf8_lossy(params).to_string(),
        ));
    };

    let param = match param {
        Some(0 | 1) | None => 1,
        Some(n) => n,
    };

    output.push(TerminalOutput::SetCursorPosRel {
        x: None,
        y: Some(param),
    });

    ParserOutcome::Finished
}
