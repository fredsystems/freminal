// Copyright (C) 2024-2025 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use crate::ansi::{ParserOutcome, TerminalOutput, split_params_into_semicolon_delimited_usize};
use crate::error::ParserFailures;
use freminal_common::window_manipulation::WindowManipulation;

/// DECSLLP - Window Manipulation
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
/// # Errors
/// Will return an error if the parameter is not a valid number
#[inline]
fn param_or(params: &[Option<usize>], idx: usize, default: usize) -> usize {
    params.get(idx).and_then(|opt| *opt).unwrap_or(default)
}

/// # Errors
/// Will return an error if the parameter is not a valid number
pub fn ansi_parser_inner_csi_finished_set_position_t(
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
