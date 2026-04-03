// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use crate::ansi::{ParserOutcome, parse_param_as};
use crate::error::ParserFailures;
use freminal_common::buffer_states::terminal_output::TerminalOutput;

/// CNL — Cursor Next Line (`CSI Ps E`)
///
/// Move the cursor to the first column of the Ps-th line down (default = 1).
pub fn ansi_parser_inner_csi_finished_cnl(
    params: &[u8],
    output: &mut Vec<TerminalOutput>,
) -> ParserOutcome {
    let Ok(param) = parse_param_as::<i32>(params) else {
        return ParserOutcome::InvalidParserFailure(ParserFailures::UnhandledCNLCommand(
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

    output.push(TerminalOutput::SetCursorPos {
        x: Some(1),
        y: None,
    });

    ParserOutcome::Finished
}
