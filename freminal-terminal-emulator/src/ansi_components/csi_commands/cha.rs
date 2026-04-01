// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use freminal_common::buffer_states::terminal_output::TerminalOutput;

use crate::ansi::{ParserOutcome, parse_param_as};
use crate::error::ParserFailures;

/// CHA — Cursor Horizontal Absolute (`CSI Ps G`)
///
/// Move the cursor to column Ps in the current row (default = 1).
/// Also handles HPA (`CSI Ps backtick`) which is functionally identical.
pub fn ansi_parser_inner_csi_finished_cha(
    params: &[u8],
    output: &mut Vec<TerminalOutput>,
) -> ParserOutcome {
    let Ok(param) = parse_param_as::<usize>(params) else {
        return ParserOutcome::InvalidParserFailure(ParserFailures::UnhandledCHACommand(
            String::from_utf8_lossy(params).to_string(),
        ));
    };

    let x_pos = match param {
        Some(0 | 1) | None => 1,
        Some(n) => n,
    };

    output.push(TerminalOutput::SetCursorPos {
        x: Some(x_pos),
        y: None,
    });

    ParserOutcome::Finished
}
