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
/// - Ps = 0 (or omitted) → The configured cursor style
/// - Ps = 1 → Blinking block
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

    output.push(match param.unwrap_or_default() {
        // `0` restores the configured style (kitty, Ghostty and WezTerm); it is
        // not an alias for `1`, which is an explicit blinking block.
        0 => TerminalOutput::CursorVisualStyleDefault,
        other => TerminalOutput::CursorVisualStyle(other.into()),
    });

    ParserOutcome::Finished
}

#[cfg(test)]
mod tests {
    use super::*;
    use freminal_common::cursor::CursorVisualStyle;

    fn parse(params: &[u8]) -> Vec<TerminalOutput> {
        let mut output = Vec::new();
        assert_eq!(
            ansi_parser_inner_csi_finished_decscusr(params, &mut output),
            ParserOutcome::Finished
        );
        output
    }

    #[test]
    fn zero_and_omitted_select_the_configured_style() {
        assert_eq!(parse(b"0"), [TerminalOutput::CursorVisualStyleDefault]);
        assert_eq!(parse(b""), [TerminalOutput::CursorVisualStyleDefault]);
    }

    #[test]
    fn one_is_an_explicit_blinking_block() {
        assert_eq!(
            parse(b"1"),
            [TerminalOutput::CursorVisualStyle(
                CursorVisualStyle::BlockCursorBlink
            )]
        );
    }

    #[test]
    fn shapes_two_through_six_map_to_their_styles() {
        for (ps, style) in [
            (b"2", CursorVisualStyle::BlockCursorSteady),
            (b"3", CursorVisualStyle::UnderlineCursorBlink),
            (b"4", CursorVisualStyle::UnderlineCursorSteady),
            (b"5", CursorVisualStyle::VerticalLineCursorBlink),
            (b"6", CursorVisualStyle::VerticalLineCursorSteady),
        ] {
            assert_eq!(parse(ps), [TerminalOutput::CursorVisualStyle(style)]);
        }
    }

    #[test]
    fn a_malformed_parameter_is_rejected() {
        let mut output = Vec::new();
        assert!(matches!(
            ansi_parser_inner_csi_finished_decscusr(b"x", &mut output),
            ParserOutcome::InvalidParserFailure(_)
        ));
        assert_eq!(output, []);
    }
}
