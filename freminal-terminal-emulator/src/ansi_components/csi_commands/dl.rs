// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use crate::ansi::{ParserOutcome, parse_param_as};
use crate::error::ParserFailures;
use freminal_common::buffer_states::terminal_output::TerminalOutput;

/// DL — Delete Line (`CSI Ps M`)
///
/// Delete Ps lines starting at the cursor row, scrolling lines below
/// up within the scroll region (default = 1).
pub fn ansi_parser_inner_csi_finished_dl(
    params: &[u8],
    output: &mut Vec<TerminalOutput>,
) -> ParserOutcome {
    let Ok(param) = parse_param_as::<usize>(params) else {
        warn!("Invalid dl command");
        output.push(TerminalOutput::Invalid);

        return ParserOutcome::InvalidParserFailure(ParserFailures::UnhandledDLCommand(format!(
            "{params:?}"
        )));
    };

    let param = match param {
        Some(0 | 1) | None => 1,
        Some(n) => n,
    };

    output.push(TerminalOutput::DeleteLines(param));

    ParserOutcome::Finished
}
