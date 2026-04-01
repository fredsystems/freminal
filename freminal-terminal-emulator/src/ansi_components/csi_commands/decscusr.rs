// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use crate::ansi::{ParserOutcome, parse_param_as};
use crate::error::ParserFailures;
use freminal_common::buffer_states::terminal_output::TerminalOutput;

/// DECSCUSR — Set Cursor Style (`CSI Ps SP q`)
///
/// Select the cursor style:
/// - Ps = 0, 1 → Blinking block (default)
/// - Ps = 2 → Steady block
/// - Ps = 3 → Blinking underline
/// - Ps = 4 → Steady underline
/// - Ps = 5 → Blinking bar
/// - Ps = 6 → Steady bar
pub fn ansi_parser_inner_csi_finished_decscusr(
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
