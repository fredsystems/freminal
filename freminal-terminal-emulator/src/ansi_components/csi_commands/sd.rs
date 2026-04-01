// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use crate::ansi::{ParserOutcome, parse_param_as};
use crate::error::ParserFailures;
use freminal_common::buffer_states::terminal_output::TerminalOutput;

/// SD — Scroll Down (`CSI Ps T`)
///
/// Scroll the content within the scroll region down by Ps lines (default = 1).
/// Content moves down; blank lines appear at the top.
pub fn ansi_parser_inner_csi_finished_sd(
    params: &[u8],
    output: &mut Vec<TerminalOutput>,
) -> ParserOutcome {
    let Ok(param) = parse_param_as::<usize>(params) else {
        warn!("Invalid SD command");
        output.push(TerminalOutput::Invalid);

        return ParserOutcome::InvalidParserFailure(ParserFailures::UnhandledSDCommand(format!(
            "{params:?}"
        )));
    };

    let param = match param {
        Some(0 | 1) | None => 1,
        Some(n) => n,
    };

    output.push(TerminalOutput::ScrollDown(param));

    ParserOutcome::Finished
}
