// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use crate::ansi::{ParserOutcome, parse_param_as};
use crate::error::ParserFailures;
use freminal_common::buffer_states::terminal_output::TerminalOutput;

/// ECH — Erase Character (`CSI Ps X`)
///
/// Erase Ps characters starting at the cursor position by replacing them
/// with blanks (default = 1). Does not shift remaining characters.
pub fn ansi_parser_inner_csi_finished_ech(
    params: &[u8],
    output: &mut Vec<TerminalOutput>,
) -> ParserOutcome {
    let Ok(param) = parse_param_as::<usize>(params) else {
        return ParserOutcome::InvalidParserFailure(ParserFailures::UnhandledECHCommand(format!(
            "{params:?}"
        )));
    };

    let param = match param {
        Some(0 | 1) | None => 1,
        Some(n) => n,
    };

    output.push(TerminalOutput::Erase(param));

    ParserOutcome::Finished
}
