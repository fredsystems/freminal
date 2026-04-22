// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use freminal_common::buffer_states::terminal_output::{TabClearMode, TerminalOutput};

use crate::ansi::{ParserOutcome, parse_param_as};
use crate::error::ParserFailures;

/// Tab Clear (TBC) — ECMA-48 Section 8.3.155
///
/// CSI Ps g
///
/// Ps = 0 → Clear character tab stop at current column (default)
/// Ps = 1 → Clear line tab stop at current line — no-op (line tabulation not supported)
/// Ps = 2 → Clear line tab stop at current line — no-op (line tabulation not supported)
/// Ps = 3 → Clear all character tab stops
/// Ps = 4 → Clear all line tab stops — no-op (line tabulation not supported)
/// Ps = 5 → Clear all tab stops (character and line) — equivalent to Ps=3
///
/// The handler dispatches all six values. Ps=1 and Ps=4 are silently accepted
/// as no-ops because no modern terminal implements line tabulation (VTS/CVT/TSM).
pub fn ansi_parser_inner_csi_finished_tbc(
    params: &[u8],
    output: &mut Vec<TerminalOutput>,
) -> ParserOutcome {
    let Ok(param) = parse_param_as::<usize>(params) else {
        return ParserOutcome::InvalidParserFailure(ParserFailures::UnhandledTBCCommand(
            String::from_utf8_lossy(params).to_string(),
        ));
    };

    let ps = param.unwrap_or(0);

    if let Ok(mode) = TabClearMode::try_from(ps) {
        output.push(TerminalOutput::TabClear(mode));
    } else {
        tracing::warn!("TBC with unsupported Ps={ps} (ignored)");
    }

    ParserOutcome::Finished
}

#[cfg(test)]
mod tests {
    use super::*;
    use freminal_common::buffer_states::terminal_output::{TabClearMode, TerminalOutput};

    #[test]
    fn tbc_non_numeric_is_invalid() {
        let mut output = Vec::new();
        let result = ansi_parser_inner_csi_finished_tbc(b"abc", &mut output);
        assert!(matches!(result, ParserOutcome::InvalidParserFailure(_)));
        assert!(output.is_empty());
    }

    #[test]
    fn tbc_empty_defaults_to_0() {
        let mut output = Vec::new();
        let result = ansi_parser_inner_csi_finished_tbc(b"", &mut output);
        assert_eq!(result, ParserOutcome::Finished);
        assert_eq!(
            output,
            vec![TerminalOutput::TabClear(TabClearMode::CurrentColumn)]
        );
    }

    #[test]
    fn tbc_explicit_ps_3() {
        let mut output = Vec::new();
        let result = ansi_parser_inner_csi_finished_tbc(b"3", &mut output);
        assert_eq!(result, ParserOutcome::Finished);
        assert_eq!(
            output,
            vec![TerminalOutput::TabClear(TabClearMode::AllCharacter)]
        );
    }

    #[test]
    fn tbc_unknown_ps_is_ignored() {
        let mut output = Vec::new();
        let result = ansi_parser_inner_csi_finished_tbc(b"99", &mut output);
        assert_eq!(result, ParserOutcome::Finished);
        assert!(output.is_empty(), "Unknown Ps should produce no output");
    }
}
