// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use crate::ansi::{ParserOutcome, split_params_into_semicolon_delimited_usize};
use crate::error::ParserFailures;
use freminal_common::buffer_states::terminal_output::TerminalOutput;

/// Extract a parameter value or return a default.
#[inline]
fn param_or(params: &[Option<usize>], idx: usize, default: usize) -> usize {
    params.get(idx).and_then(|opt| *opt).unwrap_or(default)
}

/// DECSTBM — Set Top and Bottom Margins (`CSI Ps ; Ps r`)
///
/// Set the scrolling region:
/// - Ps1 = top margin row (1-based, default = 1)
/// - Ps2 = bottom margin row (1-based, default = last row)
///
/// A value of 0 or omission uses the default. The parameter `usize::MAX`
/// is used internally as a sentinel meaning "use terminal height".
pub fn ansi_parser_inner_csi_finished_decstbm(
    params: &[u8],
    output: &mut Vec<TerminalOutput>,
) -> ParserOutcome {
    if params.is_empty() {
        output.push(TerminalOutput::SetTopAndBottomMargins {
            top_margin: 1,
            bottom_margin: usize::MAX,
        });

        return ParserOutcome::Finished;
    }

    let params = split_params_into_semicolon_delimited_usize(params);

    let Ok(params) = params else {
        return ParserOutcome::InvalidParserFailure(ParserFailures::UnhandledDECSTBMCommand(
            format!("Failed to parse in to {params:?}"),
        ));
    };

    if params.is_empty() || params.len() > 2 {
        return ParserOutcome::InvalidParserFailure(ParserFailures::UnhandledDECSTBMCommand(
            format!("{params:?}"),
        ));
    }

    let pt = match params.first() {
        Some(Some(0 | 1) | None) | None => 1,
        Some(Some(n)) => *n,
    };

    let pb = param_or(&params, 1, usize::MAX);

    if pt >= pb || pt == 0 || pb == 0 {
        return ParserOutcome::InvalidParserFailure(ParserFailures::UnhandledDECSTBMCommand(
            format!("{params:?}"),
        ));
    }

    output.push(TerminalOutput::SetTopAndBottomMargins {
        top_margin: pt,
        bottom_margin: pb,
    });

    ParserOutcome::Finished
}
