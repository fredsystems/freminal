// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use freminal_common::buffer_states::terminal_output::TerminalOutput;

use crate::ansi::{ParserOutcome, parse_param_as};
use crate::error::ParserFailures;

/// Tab Clear (TBC)
///
/// CSI Ps g
///
/// Ps = 0 → Clear tab stop at current column (default)
/// Ps = 3 → Clear all tab stops
pub fn ansi_parser_inner_csi_finished_tbc(
    params: &[u8],
    output: &mut Vec<TerminalOutput>,
) -> ParserOutcome {
    let Ok(param) = parse_param_as::<usize>(params) else {
        return ParserOutcome::InvalidParserFailure(ParserFailures::UnhandledTBCCommand(
            String::from_utf8_lossy(params).to_string(),
        ));
    };

    let ps = param.unwrap_or(0);

    output.push(TerminalOutput::TabClear(ps));

    ParserOutcome::Finished
}
