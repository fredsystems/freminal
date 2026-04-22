// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use crate::ansi::{ParserOutcome, parse_param_as};
use crate::error::ParserFailures;
use freminal_common::buffer_states::terminal_output::TerminalOutput;

/// Erase-in-Display mode (`CSI Ps J`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EraseDisplayMode {
    /// Ps=0 — from cursor to end of display (default).
    CursorToEnd,
    /// Ps=1 — from start of display to cursor.
    StartToCursor,
    /// Ps=2 — entire display.
    All,
    /// Ps=3 — entire display plus scrollback.
    AllWithScrollback,
}

/// Error returned when a `CSI Ps J` param is not one of 0, 1, 2, or 3.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnknownEraseDisplayMode(pub usize);

impl TryFrom<usize> for EraseDisplayMode {
    type Error = UnknownEraseDisplayMode;
    fn try_from(v: usize) -> Result<Self, Self::Error> {
        match v {
            0 => Ok(Self::CursorToEnd),
            1 => Ok(Self::StartToCursor),
            2 => Ok(Self::All),
            3 => Ok(Self::AllWithScrollback),
            other => Err(UnknownEraseDisplayMode(other)),
        }
    }
}

/// ED — Erase in Display (`CSI Ps J`)
///
/// Erase part of the display:
/// - Ps = 0 → From cursor to end of display (default)
/// - Ps = 1 → From start of display to cursor
/// - Ps = 2 → Entire display
/// - Ps = 3 → Entire display including scrollback buffer
pub fn ansi_parser_inner_csi_finished_ed(
    params: &[u8],
    output: &mut Vec<TerminalOutput>,
) -> ParserOutcome {
    let Ok(param) = parse_param_as::<usize>(params) else {
        return ParserOutcome::InvalidParserFailure(ParserFailures::UnhandledEDCommand(format!(
            "{params:?}"
        )));
    };

    let ret = match param.unwrap_or(0) {
        0 => TerminalOutput::ClearDisplayfromCursortoEndofDisplay,
        1 => TerminalOutput::ClearDisplayfromStartofDisplaytoCursor,
        2 => TerminalOutput::ClearDisplay,
        3 => TerminalOutput::ClearScrollbackandDisplay,
        _ => TerminalOutput::Invalid,
    };
    output.push(ret);

    ParserOutcome::Finished
}

#[cfg(test)]
mod tests {
    use super::*;
    use freminal_common::buffer_states::terminal_output::TerminalOutput;

    #[test]
    fn ed_non_numeric_is_invalid() {
        let mut output = Vec::new();
        let result = ansi_parser_inner_csi_finished_ed(b"abc", &mut output);
        assert!(matches!(result, ParserOutcome::InvalidParserFailure(_)));
        assert!(output.is_empty());
    }

    #[test]
    fn ed_empty_clears_from_cursor_to_end() {
        let mut output = Vec::new();
        let result = ansi_parser_inner_csi_finished_ed(b"", &mut output);
        assert_eq!(result, ParserOutcome::Finished);
        assert_eq!(
            output,
            vec![TerminalOutput::ClearDisplayfromCursortoEndofDisplay]
        );
    }

    #[test]
    fn ed_2_clears_display() {
        let mut output = Vec::new();
        let result = ansi_parser_inner_csi_finished_ed(b"2", &mut output);
        assert_eq!(result, ParserOutcome::Finished);
        assert_eq!(output, vec![TerminalOutput::ClearDisplay]);
    }

    #[test]
    fn ed_3_clears_scrollback_and_display() {
        let mut output = Vec::new();
        let result = ansi_parser_inner_csi_finished_ed(b"3", &mut output);
        assert_eq!(result, ParserOutcome::Finished);
        assert_eq!(output, vec![TerminalOutput::ClearScrollbackandDisplay]);
    }
}
