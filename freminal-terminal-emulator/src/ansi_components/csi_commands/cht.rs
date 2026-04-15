// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use freminal_common::buffer_states::terminal_output::TerminalOutput;

use crate::ansi::{ParserOutcome, parse_param_as};
use crate::error::ParserFailures;

/// Cursor Forward Tabulation (CHT)
///
/// CSI Ps I
///
/// Move cursor forward Ps tab stops (default = 1)
pub fn ansi_parser_inner_csi_finished_cht(
    params: &[u8],
    output: &mut Vec<TerminalOutput>,
) -> ParserOutcome {
    let Ok(param) = parse_param_as::<usize>(params) else {
        return ParserOutcome::InvalidParserFailure(ParserFailures::UnhandledCHTCommand(
            String::from_utf8_lossy(params).to_string(),
        ));
    };

    let count = match param {
        Some(0) | None => 1,
        Some(n) => n,
    };

    output.push(TerminalOutput::CursorForwardTab(count));

    ParserOutcome::Finished
}

#[cfg(test)]
mod tests {
    use super::*;
    use freminal_common::buffer_states::terminal_output::TerminalOutput;

    #[test]
    fn cht_non_numeric_is_invalid() {
        let mut output = Vec::new();
        let result = ansi_parser_inner_csi_finished_cht(b"abc", &mut output);
        assert!(matches!(result, ParserOutcome::InvalidParserFailure(_)));
        assert!(output.is_empty());
    }

    #[test]
    fn cht_empty_defaults_to_1() {
        let mut output = Vec::new();
        let result = ansi_parser_inner_csi_finished_cht(b"", &mut output);
        assert_eq!(result, ParserOutcome::Finished);
        assert_eq!(output, vec![TerminalOutput::CursorForwardTab(1)]);
    }

    #[test]
    fn cht_explicit_count() {
        let mut output = Vec::new();
        let result = ansi_parser_inner_csi_finished_cht(b"4", &mut output);
        assert_eq!(result, ParserOutcome::Finished);
        assert_eq!(output, vec![TerminalOutput::CursorForwardTab(4)]);
    }
}
