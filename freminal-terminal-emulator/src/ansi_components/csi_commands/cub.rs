// Copyright (C) 2024-2025 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use crate::ansi::{ParserOutcome, TerminalOutput, parse_param_as};
use crate::error::ParserFailures;

/// Cursor Backward
///
/// CUB moves the cursor backward by a specified number of lines without changing columns.
///
/// ESC [ Pn D
/// # Errors
/// Will return an error if the parameter is not a valid number
pub fn ansi_parser_inner_csi_finished_move_cursor_left(
    params: &[u8],
    output: &mut Vec<TerminalOutput>,
) -> ParserOutcome {
    let Ok(param) = parse_param_as::<i32>(params) else {
        return ParserOutcome::InvalidParserFailure(ParserFailures::UnhandledCUBCommand(
            String::from_utf8_lossy(params).to_string(),
        ));
    };

    let param = match param {
        Some(0 | 1) | None => 1,
        Some(n) => n,
    };

    output.push(TerminalOutput::SetCursorPosRel {
        x: Some(-param),
        y: None,
    });

    ParserOutcome::Finished
}
