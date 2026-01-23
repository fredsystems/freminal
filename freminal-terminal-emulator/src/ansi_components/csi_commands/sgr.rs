// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use std::vec::IntoIter;

use crate::ansi::split_params_into_semicolon_delimited_usize;
use crate::ansi::{ParserOutcome, split_params_into_colon_delimited_usize};
use crate::error::ParserFailures;
use freminal_common::buffer_states::terminal_output::TerminalOutput;
use freminal_common::colors::{TerminalColor, lookup_256_color_by_index};
use freminal_common::sgr::SelectGraphicRendition;

#[inline]
fn opt(params: &[Option<usize>], idx: usize) -> Option<usize> {
    params.get(idx).copied().flatten()
}

/// Select Graphic Rendition
///
/// SGR sets the text attributes for the following characters. Several attributes can be combined by separating them with a semicolon.
///
/// Values for param are defined in the `SelectGraphicRendition` enum
///
/// ESC [ params m
/// # Errors
/// Will return an error if the parameter is not a valid number
pub fn ansi_parser_inner_csi_finished_sgr_ansi(
    params: &[u8],
    output: &mut Vec<TerminalOutput>,
) -> ParserOutcome {
    if params.first() == Some(&b'>') {
        output.push(TerminalOutput::Skipped);
        return ParserOutcome::Finished;
    }

    let (params, split_by_colon) = if params.contains(&b':') {
        (split_params_into_colon_delimited_usize(params), true)
    } else {
        (split_params_into_semicolon_delimited_usize(params), false)
    };

    let Ok(mut params) = params else {
        return ParserOutcome::InvalidParserFailure(ParserFailures::UnhandledSGRCommand(format!(
            "{params:?}"
        )));
    };

    // Empty SGR params means reset (0)
    if params.is_empty() {
        params.push(Some(0));
    }

    // If exactly one param and it's None, treat as 0 (reset)
    if params.len() == 1 && opt(&params, 0).is_none() {
        params[0] = Some(0);
    }

    let mut param_iter: IntoIter<Option<usize>> = params.into_iter();
    loop {
        let Some(param) = param_iter.next().flatten() else {
            break;
        };

        if param == 38 || param == 48 || param == 58 {
            handle_custom_color(output, &mut param_iter, param, split_by_colon);
            continue;
        }

        output.push(TerminalOutput::Sgr(SelectGraphicRendition::from_usize(
            param,
        )));
    }

    ParserOutcome::Finished
}

fn default_color(output: &mut Vec<TerminalOutput>, custom_color_control_code: usize) {
    // FIXME: we'll treat '\x1b[38m' or '\x1b[48m' as a color reset.
    // I can't find documentation for this, but it seems that other terminals handle it this way

    output.push(match custom_color_control_code {
        38 => TerminalOutput::Sgr(SelectGraphicRendition::Foreground(TerminalColor::Default)),
        48 => TerminalOutput::Sgr(SelectGraphicRendition::Background(
            TerminalColor::DefaultBackground,
        )),
        // instead of matching directly on 58, we'll match on a wildcard. This helps with codecov because it thought
        // we were testing `_` in the match statement when it's impossible to end up here with a value other than 58
        _ => TerminalOutput::Sgr(SelectGraphicRendition::UnderlineColor(
            TerminalColor::DefaultUnderlineColor,
        )),
    });
}

pub fn handle_custom_color(
    output: &mut Vec<TerminalOutput>,
    param_iter: &mut IntoIter<Option<usize>>,
    param: usize,
    split_by_colon: bool,
) {
    // if control code is 38, 48 or 58 we need to read the next param
    // otherwise, store the param as is
    let custom_color_control_code = param;

    let next = param_iter.next();
    let Some(param) = next.flatten() else {
        // No mode parameter after 38/48/58 → treat as a reset for that channel
        default_color(output, custom_color_control_code);
        return;
    };

    match param {
        // Truecolor: 2;r;g;b  or  2:r:g:b
        2 => {
            // Some colon-splitters may leave an extra token after the 2; skip it if present.
            if param_iter.len() > 3 && split_by_colon {
                let _ = param_iter.next();
            }

            let r = param_iter.next().flatten().unwrap_or(0);
            let g = param_iter.next().flatten().unwrap_or(0);
            let b = param_iter.next().flatten().unwrap_or(0);

            match SelectGraphicRendition::from_usize_color(custom_color_control_code, r, g, b) {
                Ok(sgr) => output.push(TerminalOutput::Sgr(sgr)),
                Err(e) => {
                    warn!("Invalid RGB SGR sequence: {}", e);
                    output.push(TerminalOutput::Invalid);
                }
            }
        }

        // 256-color: 5;idx
        5 => {
            let lookup = param_iter.next().flatten().unwrap_or(0);

            match custom_color_control_code {
                38 => output.push(TerminalOutput::Sgr(SelectGraphicRendition::Foreground(
                    lookup_256_color_by_index(lookup),
                ))),
                48 => output.push(TerminalOutput::Sgr(SelectGraphicRendition::Background(
                    lookup_256_color_by_index(lookup),
                ))),
                58 => output.push(TerminalOutput::Sgr(SelectGraphicRendition::UnderlineColor(
                    lookup_256_color_by_index(lookup),
                ))),
                _ => output.push(TerminalOutput::Invalid),
            }
        }

        _ => {
            warn!("Invalid SGR sequence: {}", param);
            output.push(TerminalOutput::Invalid);
        }
    }
}
