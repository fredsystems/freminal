// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use freminal_common::buffer_states::terminal_output::TerminalOutput;

use crate::ansi::{ParserOutcome, parse_param_as};
use crate::error::ParserFailures;

/// Cursor Backward Tabulation (CBT)
///
/// CSI Ps Z
///
/// Move cursor backward Ps tab stops (default = 1)
pub fn ansi_parser_inner_csi_finished_cbt(
    params: &[u8],
    output: &mut Vec<TerminalOutput>,
) -> ParserOutcome {
    let Ok(param) = parse_param_as::<usize>(params) else {
        return ParserOutcome::InvalidParserFailure(ParserFailures::UnhandledCBTCommand(
            String::from_utf8_lossy(params).to_string(),
        ));
    };

    let count = match param {
        Some(0) | None => 1,
        Some(n) => n,
    };

    output.push(TerminalOutput::CursorBackwardTab(count));

    ParserOutcome::Finished
}

#[cfg(test)]
mod tests {
    use super::*;
    use freminal_common::buffer_states::terminal_output::TerminalOutput;

    #[test]
    fn cbt_non_numeric_is_invalid() {
        let mut output = Vec::new();
        let result = ansi_parser_inner_csi_finished_cbt(b"abc", &mut output);
        assert!(matches!(result, ParserOutcome::InvalidParserFailure(_)));
        assert!(output.is_empty());
    }

    #[test]
    fn cbt_empty_defaults_to_1() {
        let mut output = Vec::new();
        let result = ansi_parser_inner_csi_finished_cbt(b"", &mut output);
        assert_eq!(result, ParserOutcome::Finished);
        assert_eq!(output, vec![TerminalOutput::CursorBackwardTab(1)]);
    }

    #[test]
    fn cbt_explicit_count() {
        let mut output = Vec::new();
        let result = ansi_parser_inner_csi_finished_cbt(b"3", &mut output);
        assert_eq!(result, ParserOutcome::Finished);
        assert_eq!(output, vec![TerminalOutput::CursorBackwardTab(3)]);
    }
}
