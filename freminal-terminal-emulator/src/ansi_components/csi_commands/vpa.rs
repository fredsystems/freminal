// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use freminal_common::buffer_states::terminal_output::TerminalOutput;

use crate::ansi::{ParserOutcome, parse_param_as};
use crate::error::ParserFailures;

/// Vertical Position Absolute
///
/// VPA moves the cursor to the specified row in the current column. If no
/// parameter is given the cursor moves to the first row.
///
/// ESC [ Pn d
///
/// # Errors
/// Will return an error if the parameter is not a valid number
pub fn ansi_parser_inner_csi_finished_vpa(
    params: &[u8],
    output: &mut Vec<TerminalOutput>,
) -> ParserOutcome {
    let Ok(param) = parse_param_as::<usize>(params) else {
        return ParserOutcome::InvalidParserFailure(ParserFailures::UnhandledVPACommand(
            String::from_utf8_lossy(params).to_string(),
        ));
    };

    // VPA row numbers are 1-based; default (omitted/0) means row 1.
    let y_pos = match param {
        Some(0) | None => 1,
        Some(n) => n,
    };

    output.push(TerminalOutput::SetCursorPos {
        x: None,
        y: Some(y_pos),
    });

    ParserOutcome::Finished
}

#[cfg(test)]
mod tests {
    use super::*;
    use freminal_common::buffer_states::terminal_output::TerminalOutput;

    #[test]
    fn vpa_no_param_defaults_to_row_1() {
        let mut output = Vec::new();
        let result = ansi_parser_inner_csi_finished_vpa(b"", &mut output);
        assert_eq!(result, ParserOutcome::Finished);
        assert_eq!(
            output,
            vec![TerminalOutput::SetCursorPos {
                x: None,
                y: Some(1),
            }]
        );
    }

    #[test]
    fn vpa_zero_param_defaults_to_row_1() {
        let mut output = Vec::new();
        let result = ansi_parser_inner_csi_finished_vpa(b"0", &mut output);
        assert_eq!(result, ParserOutcome::Finished);
        assert_eq!(
            output,
            vec![TerminalOutput::SetCursorPos {
                x: None,
                y: Some(1),
            }]
        );
    }

    #[test]
    fn vpa_explicit_row() {
        let mut output = Vec::new();
        let result = ansi_parser_inner_csi_finished_vpa(b"42", &mut output);
        assert_eq!(result, ParserOutcome::Finished);
        assert_eq!(
            output,
            vec![TerminalOutput::SetCursorPos {
                x: None,
                y: Some(42),
            }]
        );
    }

    #[test]
    fn vpa_row_1_explicit() {
        let mut output = Vec::new();
        let result = ansi_parser_inner_csi_finished_vpa(b"1", &mut output);
        assert_eq!(result, ParserOutcome::Finished);
        assert_eq!(
            output,
            vec![TerminalOutput::SetCursorPos {
                x: None,
                y: Some(1),
            }]
        );
    }

    #[test]
    fn vpa_does_not_set_x() {
        let mut output = Vec::new();
        ansi_parser_inner_csi_finished_vpa(b"10", &mut output);
        match &output[0] {
            TerminalOutput::SetCursorPos { x, y } => {
                assert_eq!(*x, None, "VPA must not set x");
                assert_eq!(*y, Some(10));
            }
            other => panic!("unexpected output: {other:?}"),
        }
    }
}
