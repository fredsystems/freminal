// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use crate::ansi::{ParserOutcome, parse_param_as};
use crate::error::ParserFailures;
use freminal_common::buffer_states::terminal_output::TerminalOutput;

/// ICH — Insert Character (`CSI Ps @`)
///
/// Insert Ps blank characters at the cursor position, shifting existing
/// characters to the right (default = 1).
pub fn ansi_parser_inner_csi_finished_ich(
    params: &[u8],
    output: &mut Vec<TerminalOutput>,
) -> ParserOutcome {
    let Ok(param) = parse_param_as::<usize>(params) else {
        warn!("Invalid ich command");
        output.push(TerminalOutput::Invalid);

        return ParserOutcome::InvalidParserFailure(ParserFailures::UnhandledICHCommand(format!(
            "{params:?}"
        )));
    };

    let param = match param {
        Some(0 | 1) | None => 1,
        Some(n) => n,
    };

    // ecma-48 8.3.64
    output.push(TerminalOutput::InsertSpaces(param));

    ParserOutcome::Finished
}
