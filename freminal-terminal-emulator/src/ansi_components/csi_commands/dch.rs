// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use crate::ansi::{ParserOutcome, parse_param_as};
use crate::error::ParserFailures;
use freminal_common::buffer_states::terminal_output::TerminalOutput;

/// DCH — Delete Character (`CSI Ps P`)
///
/// Delete Ps characters starting at the cursor position, shifting remaining
/// characters to the left (default = 1).
pub fn ansi_parser_inner_csi_finished_dch(
    params: &[u8],
    output: &mut Vec<TerminalOutput>,
) -> ParserOutcome {
    let Ok(param) = parse_param_as::<usize>(params) else {
        return ParserOutcome::InvalidParserFailure(ParserFailures::UnhandledDCHCommand(format!(
            "{params:?}"
        )));
    };

    let param = match param {
        Some(0 | 1) | None => 1,
        Some(n) => n,
    };

    output.push(TerminalOutput::Delete(param));

    ParserOutcome::Finished
}

#[cfg(test)]
mod tests {
    use super::*;
    use freminal_common::buffer_states::terminal_output::TerminalOutput;

    #[test]
    fn dch_non_numeric_is_invalid() {
        let mut output = Vec::new();
        let result = ansi_parser_inner_csi_finished_dch(b"abc", &mut output);
        assert!(matches!(result, ParserOutcome::InvalidParserFailure(_)));
        assert!(output.is_empty());
    }

    #[test]
    fn dch_empty_defaults_to_1() {
        let mut output = Vec::new();
        let result = ansi_parser_inner_csi_finished_dch(b"", &mut output);
        assert_eq!(result, ParserOutcome::Finished);
        assert_eq!(output, vec![TerminalOutput::Delete(1)]);
    }

    #[test]
    fn dch_explicit_count() {
        let mut output = Vec::new();
        let result = ansi_parser_inner_csi_finished_dch(b"7", &mut output);
        assert_eq!(result, ParserOutcome::Finished);
        assert_eq!(output, vec![TerminalOutput::Delete(7)]);
    }
}
