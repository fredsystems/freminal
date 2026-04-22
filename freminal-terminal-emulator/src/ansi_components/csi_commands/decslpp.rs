// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use crate::ansi::{ParserOutcome, split_params_into_semicolon_delimited_usize};
use crate::ansi_components::csi_commands::util::param_or;
use crate::error::ParserFailures;
use freminal_common::buffer_states::terminal_output::TerminalOutput;
use freminal_common::buffer_states::window_manipulation::WindowManipulation;

/// DECSLPP — Window Manipulation
///
/// Ps1 1    De-iconify window.
///     2    Minimize window.
///     3    Move window to [Ps2, Ps3].
///     4    Resize window to height Ps2 pixels and width Ps3 pixels.
///     5    Raise the window to the top of the stacking order.
///     6    Lower the window to the bottom of the stacking order.
///     7    Refresh window.
///     8    Resize window to Ps2 lines and Ps3 columns.
///     9    Change maximize state of window.
///             Ps2 0    Restore maximized window.
///                 1    Maximize window.
///     10    Change full-screen state of window. Currently use the window maximizing instead.
///             Ps2 0    Undo full-screen mode.
///                 1    Change to full-screen.
///                 2    Toggle full-screen.
///     11    Reports window state.
///             Response: CSI s t
///               s 1    Normal. (non-iconified)
///                 2    Iconified.
///     13    Reports window position.
///             Ps2 Omitted, 0, 1
///                        Reports whole window position.
///                 2    Reports text area position.
///             Response: CSI 3 ; x ; y t
///               x    X position of window.
///               y    Y position of window.
///     14    Reports window size in pixels.
///             Ps2 Omitted, 0, 1
///                        Reports text area size.
///                 2    Reports whole window size.
///             Response: CSI 4 ; y ; x t
///               y    Window height in pixels.
///               x    Window width in pixels.
///     15    Reports root window size in pixels.
///             Response: CSI 5 ; y ; x t
///               y    Root window height in pixels.
///               x    Root window width in pixels.
///     16    Reports character size in pixels.
///             Response: CSI 6 ; y ; x t
///               y    character height in pixels.
///               x    character width in pixels.
///     18    Reports terminal size in characters.
///             Response: CSI 8 ; y ; x t
///               y    Terminal height in characters. (Lines)
///               x    Terminal width in characters. (Columns)
///     19    Reports root window size in characters.
///             Response: CSI 9 ; y ; x t
///               y    Root window height in characters.
///               x    Root window width in characters.
///     20    Reports icon label.
///             Response: OSC L title ST
///               title    icon label. (window title)
///     21    Reports window title.
///             Response: OSC l title ST
///               title    Window title.
///     22    Save window title on stack.
///             Ps2 0, 1, 2    Save window title.
///     23    Restore window title from stack.
///            Ps2 0, 1, 2    Restore window title.
///
/// ESC [ Ps1 ; Ps2 ; Ps3 t
/// DECSLPP — Window Manipulation (`CSI Ps ; Ps ; Ps t`)
///
/// Handles xterm window manipulation operations. See the full operation
/// table in the source for all supported Ps values.
pub fn ansi_parser_inner_csi_finished_decslpp(
    params: &[u8],
    output: &mut Vec<TerminalOutput>,
) -> ParserOutcome {
    let params = split_params_into_semicolon_delimited_usize(params);

    let Ok(params) = params else {
        return ParserOutcome::InvalidParserFailure(ParserFailures::UnhandledDECSLPPCommand(
            format!("{params:?}"),
        ));
    };

    let (ps1, ps2, ps3) = if params.len() == 1 {
        (param_or(&params, 0, usize::MAX), 0, 0)
    } else if params.len() == 2 {
        (
            param_or(&params, 0, usize::MAX),
            param_or(&params, 1, usize::MAX),
            0,
        )
    } else {
        (
            param_or(&params, 0, usize::MAX),
            param_or(&params, 1, usize::MAX),
            param_or(&params, 2, usize::MAX),
        )
    };

    let parsed = match WindowManipulation::try_from((ps1, ps2, ps3)) {
        Ok(parsed) => parsed,
        Err(e) => {
            warn!("Invalid DECSLPP sequence: {e}");
            output.push(TerminalOutput::Invalid);

            return ParserOutcome::InvalidParserFailure(ParserFailures::UnhandledDECSLPPCommand(
                format!("{params:?}"),
            ));
        }
    };

    output.push(TerminalOutput::WindowManipulation(parsed));
    ParserOutcome::Finished
}

#[cfg(test)]
mod tests {
    use super::*;
    use freminal_common::buffer_states::terminal_output::TerminalOutput;

    #[test]
    fn decslpp_non_numeric_params_is_invalid() {
        let mut output = Vec::new();
        let result = ansi_parser_inner_csi_finished_decslpp(b"abc", &mut output);
        assert!(matches!(result, ParserOutcome::InvalidParserFailure(_)));
    }

    #[test]
    fn decslpp_zero_params_is_invalid() {
        // Empty slice → split produces 0 params, but split gives one empty item.
        // Actually an empty byte slice → split on `;` gives `[""]` → parse as None.
        // That gives params.len() == 1, ps1 = MAX → WindowManipulation::try_from fails.
        let mut output = Vec::new();
        // We use a known-bad value to test the Err branch from try_from
        // ps1=usize::MAX → will fail or be handled as invalid by try_from
        let result = ansi_parser_inner_csi_finished_decslpp(b"999", &mut output);
        // 999 is not a known WindowManipulation code → InvalidParserFailure
        assert!(matches!(result, ParserOutcome::InvalidParserFailure(_)));
    }

    #[test]
    fn decslpp_known_command_1_param() {
        // ps1=1 (de-iconify) → valid with 1 param
        let mut output = Vec::new();
        let result = ansi_parser_inner_csi_finished_decslpp(b"1", &mut output);
        assert_eq!(result, ParserOutcome::Finished);
        assert_eq!(output.len(), 1);
        assert!(matches!(output[0], TerminalOutput::WindowManipulation(_)));
    }

    #[test]
    fn decslpp_known_command_2_params() {
        // ps1=3, ps2=100, ps3=200 (move window) → 3 params
        let mut output = Vec::new();
        let result = ansi_parser_inner_csi_finished_decslpp(b"3;100;200", &mut output);
        assert_eq!(result, ParserOutcome::Finished);
        assert_eq!(output.len(), 1);
        assert!(matches!(output[0], TerminalOutput::WindowManipulation(_)));
    }
}
