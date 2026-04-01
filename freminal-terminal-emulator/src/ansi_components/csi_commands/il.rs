// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use crate::ansi::{ParserOutcome, parse_param_as};
use crate::error::ParserFailures;
use freminal_common::buffer_states::terminal_output::TerminalOutput;
/// IL — Insert Line (`CSI Ps L`)
///
/// Insert Ps blank lines at the cursor row, scrolling existing lines
/// down within the scroll region (default = 1).
pub fn ansi_parser_inner_csi_finished_il(
    params: &[u8],
    output: &mut Vec<TerminalOutput>,
) -> ParserOutcome {
    let Ok(param) = parse_param_as::<usize>(params) else {
        warn!("Invalid il command");
        output.push(TerminalOutput::Invalid);

        return ParserOutcome::InvalidParserFailure(ParserFailures::UnhandledILCommand(format!(
            "{params:?}"
        )));
    };

    let param = match param {
        Some(0 | 1) | None => 1,
        Some(n) => n,
    };

    output.push(TerminalOutput::InsertLines(param));

    ParserOutcome::Finished
}
