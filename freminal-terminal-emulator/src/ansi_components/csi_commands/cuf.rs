// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use crate::ansi::{ParserOutcome, parse_param_as};
use crate::error::ParserFailures;
use freminal_common::buffer_states::terminal_output::TerminalOutput;

/// CUF — Cursor Forward (`CSI Ps C`)
///
/// Move the cursor right by Ps columns without changing the row (default = 1).
pub fn ansi_parser_inner_csi_finished_cuf(
    params: &[u8],
    output: &mut Vec<TerminalOutput>,
) -> ParserOutcome {
    let Ok(param) = parse_param_as::<i32>(params) else {
        return ParserOutcome::InvalidParserFailure(ParserFailures::UnhandledCUFCommand(
            String::from_utf8_lossy(params).to_string(),
        ));
    };

    let param = match param {
        Some(0 | 1) | None => 1,
        Some(n) => n,
    };

    output.push(TerminalOutput::SetCursorPosRel {
        x: Some(param),
        y: None,
    });

    ParserOutcome::Finished
}

#[cfg(test)]
mod tests {
    use super::*;
    use freminal_common::buffer_states::terminal_output::TerminalOutput;

    #[test]
    fn cuf_non_numeric_is_invalid() {
        let mut output = Vec::new();
        let result = ansi_parser_inner_csi_finished_cuf(b"abc", &mut output);
        assert!(matches!(result, ParserOutcome::InvalidParserFailure(_)));
        assert!(output.is_empty());
    }

    #[test]
    fn cuf_empty_defaults_to_1() {
        let mut output = Vec::new();
        let result = ansi_parser_inner_csi_finished_cuf(b"", &mut output);
        assert_eq!(result, ParserOutcome::Finished);
        assert_eq!(
            output,
            vec![TerminalOutput::SetCursorPosRel {
                x: Some(1),
                y: None
            }]
        );
    }
}
