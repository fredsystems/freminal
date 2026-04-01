// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use crate::ansi::{ParserOutcome, parse_param_as};
use crate::error::ParserFailures;
use freminal_common::buffer_states::terminal_output::TerminalOutput;

/// SU — Scroll Up (`CSI Ps S`)
///
/// Scroll the content within the scroll region up by Ps lines (default = 1).
/// Content moves up; blank lines appear at the bottom.
pub fn ansi_parser_inner_csi_finished_su(
    params: &[u8],
    output: &mut Vec<TerminalOutput>,
) -> ParserOutcome {
    let Ok(param) = parse_param_as::<usize>(params) else {
        warn!("Invalid SU command");
        output.push(TerminalOutput::Invalid);

        return ParserOutcome::InvalidParserFailure(ParserFailures::UnhandledSUCommand(format!(
            "{params:?}"
        )));
    };

    let param = match param {
        Some(0 | 1) | None => 1,
        Some(n) => n,
    };

    output.push(TerminalOutput::ScrollUp(param));

    ParserOutcome::Finished
}
