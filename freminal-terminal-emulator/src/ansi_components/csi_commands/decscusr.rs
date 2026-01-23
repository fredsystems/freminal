// Copyright (C) 2024-2025 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use crate::ansi::{ParserOutcome, TerminalOutput, parse_param_as};
use crate::error::ParserFailures;

/// DECSCUSR—Set Cursor Style
///
/// Select the style of the cursor on the screen.
/// 0, 1, or none: Blink Block (default)
/// 2: Steady Block
/// 3: Blink Underline
/// 4: Steady Underline
/// 5: Vertical line cursor / Blink
/// 6: Vertical line cursor / Steady
///
/// ESC [ Pn SP q
/// # Errors
/// Will return an error if the parameter is not a valid number
pub fn ansi_parser_inner_csi_finished_set_position_q(
    params: &[u8],
    output: &mut Vec<TerminalOutput>,
) -> ParserOutcome {
    let Ok(param) = parse_param_as::<usize>(params) else {
        return ParserOutcome::InvalidParserFailure(ParserFailures::UnhandledDECSCUSRCommand(
            format!("{params:?}"),
        ));
    };

    output.push(TerminalOutput::CursorVisualStyle(
        param.unwrap_or_default().into(),
    ));

    ParserOutcome::Finished
}
