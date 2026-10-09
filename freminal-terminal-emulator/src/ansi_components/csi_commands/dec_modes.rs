// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use freminal_common::buffer_states::mode::{Mode, SetMode};
use freminal_common::buffer_states::terminal_output::TerminalOutput;

/// Split CSI mode parameters on `;` and emit one `TerminalOutput::Mode` per sub-parameter.
///
/// When the parameter string starts with `?` (DEC private indicator), the `?` prefix is
/// re-applied to each sub-parameter so that `terminal_mode_from_params` matches correctly.
///
/// For example, `?1049;2004` is split into `?1049` and `?2004`, each producing its own
/// `TerminalOutput::Mode`.
pub(crate) fn push_split_mode_params(
    params: &[u8],
    mode: SetMode,
    output: &mut Vec<TerminalOutput>,
) {
    let (is_dec_private, param_body) = if params.first() == Some(&b'?') {
        (true, &params[1..])
    } else {
        (false, params)
    };

    // Fast path: no semicolons means a single parameter — avoid allocation.
    if !param_body.contains(&b';') {
        output.push(TerminalOutput::Mode(Mode::terminal_mode_from_params(
            params, mode,
        )));
        return;
    }

    for sub_param in param_body.split(|&b| b == b';') {
        if sub_param.is_empty() {
            continue;
        }
        if is_dec_private {
            let mut prefixed = Vec::with_capacity(1 + sub_param.len());
            prefixed.push(b'?');
            prefixed.extend_from_slice(sub_param);
            output.push(TerminalOutput::Mode(Mode::terminal_mode_from_params(
                &prefixed, mode,
            )));
        } else {
            output.push(TerminalOutput::Mode(Mode::terminal_mode_from_params(
                sub_param, mode,
            )));
        }
    }
}
