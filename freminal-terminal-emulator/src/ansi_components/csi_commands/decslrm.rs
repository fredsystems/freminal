// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use crate::ansi::{ParserOutcome, split_params_into_semicolon_delimited_usize};
use crate::error::ParserFailures;
use freminal_common::buffer_states::terminal_output::TerminalOutput;

/// Set Left and Right Margins (DECSLRM)
///
/// `CSI Pl ; Pr s` — only active when DECLRMM (`?69`) is set.
///
/// Pl - column number for the left margin (1-based)
/// Pr - column number for the right margin (1-based)
///
/// Internally we use `usize::MAX` to mean "use the default" (right edge of
/// the screen).  The buffer's `set_left_right_margins` method handles the
/// clamping and validation.
///
/// # Errors
/// Returns an error outcome if the params cannot be parsed.
pub fn ansi_parser_inner_csi_set_left_and_right_margins(
    params: &[u8],
    output: &mut Vec<TerminalOutput>,
) -> ParserOutcome {
    if params.is_empty() {
        // `CSI s` with no params and no DECLRMM context is SCOSC.
        // The caller (`csi.rs`) is responsible for routing: it only calls
        // this function when params are present.  But be defensive and emit
        // the full-screen reset variant.
        output.push(TerminalOutput::SetLeftAndRightMargins {
            left_margin: 1,
            right_margin: usize::MAX,
        });
        return ParserOutcome::Finished;
    }

    let params = split_params_into_semicolon_delimited_usize(params);

    let Ok(params) = params else {
        return ParserOutcome::InvalidParserFailure(ParserFailures::UnhandledDECSTBMCommand(
            format!("DECSLRM: failed to parse params: {params:?}"),
        ));
    };

    if params.is_empty() || params.len() > 2 {
        return ParserOutcome::InvalidParserFailure(ParserFailures::UnhandledDECSTBMCommand(
            format!("DECSLRM: unexpected param count: {params:?}"),
        ));
    }

    let pl = match params.first() {
        Some(Some(0) | None) | None => 1,
        Some(Some(n)) => *n,
    };

    let pr = match params.get(1) {
        Some(Some(n)) => *n,
        _ => usize::MAX,
    };

    output.push(TerminalOutput::SetLeftAndRightMargins {
        left_margin: pl,
        right_margin: pr,
    });

    ParserOutcome::Finished
}

#[cfg(test)]
mod tests {
    use super::*;
    use freminal_common::buffer_states::terminal_output::TerminalOutput;

    fn parse(params: &[u8]) -> Vec<TerminalOutput> {
        let mut out = Vec::new();
        ansi_parser_inner_csi_set_left_and_right_margins(params, &mut out);
        out
    }

    #[test]
    fn empty_params_resets_to_full_screen() {
        let out = parse(b"");
        assert_eq!(
            out,
            vec![TerminalOutput::SetLeftAndRightMargins {
                left_margin: 1,
                right_margin: usize::MAX,
            }]
        );
    }

    #[test]
    fn explicit_margins_parsed_correctly() {
        let out = parse(b"5;10");
        assert_eq!(
            out,
            vec![TerminalOutput::SetLeftAndRightMargins {
                left_margin: 5,
                right_margin: 10,
            }]
        );
    }

    #[test]
    fn left_only_defaults_right_to_max() {
        let out = parse(b"3");
        assert_eq!(
            out,
            vec![TerminalOutput::SetLeftAndRightMargins {
                left_margin: 3,
                right_margin: usize::MAX,
            }]
        );
    }

    #[test]
    fn zero_left_is_treated_as_one() {
        let out = parse(b"0;10");
        assert_eq!(
            out,
            vec![TerminalOutput::SetLeftAndRightMargins {
                left_margin: 1,
                right_margin: 10,
            }]
        );
    }
}
