// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use crate::ansi::{ParserOutcome, parse_param_as};
use crate::ansi_components::tracer::{SequenceTraceable, SequenceTracer};
use anyhow::Result;
use freminal_common::buffer_states::ftcs::parse_ftcs_params;
use freminal_common::buffer_states::osc::{
    AnsiOscInternalType, AnsiOscToken, AnsiOscType, OscTarget, UrlResponse,
};
use freminal_common::buffer_states::terminal_output::TerminalOutput;

use super::osc_clipboard::handle_osc_clipboard;
use super::osc_iterm2::handle_osc_iterm2;
use super::osc_palette::{handle_osc_palette_color, handle_osc_reset_palette};

#[derive(Eq, PartialEq, Debug)]
pub(crate) enum AnsiOscParserState {
    Params,
    Finished,
    Invalid,
    InvalidFinished,
}

#[derive(Eq, PartialEq, Debug)]
pub struct AnsiOscParser {
    pub(crate) state: AnsiOscParserState,
    pub(crate) params: Vec<u8>,
    pub(crate) intermediates: Vec<u8>,
    pub(crate) seq_trace: SequenceTracer,
}

impl SequenceTraceable for AnsiOscParser {
    #[inline]
    fn seq_tracer(&mut self) -> &mut SequenceTracer {
        &mut self.seq_trace
    }
    #[inline]
    fn seq_tracer_ref(&self) -> &SequenceTracer {
        &self.seq_trace
    }
}

// OSC Sequence looks like this:
// 1b]11;?1b\

impl Default for AnsiOscParser {
    fn default() -> Self {
        Self::new()
    }
}

impl AnsiOscParser {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            state: AnsiOscParserState::Params,
            params: Vec::new(),
            intermediates: Vec::new(),
            seq_trace: SequenceTracer::new(),
        }
    }

    /// Expose current sequence trace for testing and diagnostics.
    #[must_use]
    pub fn trace_str(&self) -> String {
        trace!("current buffer trace: {}", self.seq_trace.as_str());
        self.seq_trace.as_str()
    }

    /// Push a byte into the parser
    ///
    /// # Errors
    /// Will return an error if the parser is in the `Finished` or `InvalidFinished` state
    #[tracing::instrument(level = "trace", skip_all)]
    pub fn push(&mut self, b: u8) -> ParserOutcome {
        self.append_trace(b);
        if let AnsiOscParserState::Finished | AnsiOscParserState::InvalidFinished = &self.state {
            return ParserOutcome::Invalid("Parsed Pushed To Once Finished".to_string());
        }

        match self.state {
            AnsiOscParserState::Params => {
                if is_valid_osc_param(b) {
                    self.params.push(b);
                } else {
                    debug!("Invalid OSC param: {:x}", b);
                    {
                        self.state = AnsiOscParserState::Invalid;

                        self.params.clear();
                        self.intermediates.clear();

                        return ParserOutcome::Invalid("Invalid OSC param encountered".to_string());
                    };
                }

                if is_osc_terminator(&self.params) {
                    self.state = AnsiOscParserState::Finished;

                    self.seq_trace.trim_control_tail();

                    if !self.params.is_empty() {
                        while let Some(&last) = self.params.last() {
                            if is_final_character_osc_terminator(last) {
                                self.params.pop();
                            } else {
                                break;
                            }
                        }
                    }

                    return ParserOutcome::Finished;
                }

                ParserOutcome::Continue
            }
            AnsiOscParserState::Finished | AnsiOscParserState::InvalidFinished => {
                unreachable!()
            }
            AnsiOscParserState::Invalid => {
                if is_osc_terminator(&self.params) {
                    self.state = AnsiOscParserState::InvalidFinished;
                }

                ParserOutcome::Invalid("Invalid OSC sequence terminated".to_string())
            }
        }
    }

    /// Parse the OSC sequence
    ///
    /// # Errors
    /// Will return an error if the parser is in the `Finished` or `InvalidFinished` state
    #[tracing::instrument(level = "trace", skip_all)]
    pub fn ansiparser_inner_osc(
        &mut self,
        b: u8,
        output: &mut Vec<TerminalOutput>,
    ) -> ParserOutcome {
        let push_result = self.push(b);

        // if we failed the push result with ParserOutcome::Invalid, return push_result
        if let ParserOutcome::Invalid(_) = push_result {
            return push_result;
        }

        match self.state {
            AnsiOscParserState::Finished => {
                if let Ok(params) = split_params_into_semicolon_delimited_tokens(&self.params) {
                    let Some(type_number) = extract_param(0, &params) else {
                        output.push(TerminalOutput::Invalid);
                        return ParserOutcome::Invalid(format!(
                            "Invalid OSC params: recent='{}'",
                            self.seq_trace.as_str()
                        ));
                    };

                    // Only clone what's actually reused later.
                    let osc_target = OscTarget::from(&type_number);
                    let osc_internal_type = AnsiOscInternalType::from(&params);

                    dispatch_osc_target(
                        &osc_target,
                        osc_internal_type,
                        params,
                        &self.params,
                        &self.seq_trace,
                        output,
                    );
                } else {
                    output.push(TerminalOutput::Invalid);

                    return ParserOutcome::Invalid(format!(
                        "Invalid OSC params: recent='{}'",
                        self.seq_trace.as_str()
                    ));
                }

                ParserOutcome::Finished
            }
            AnsiOscParserState::Invalid => ParserOutcome::Invalid("Invalid OSC State".to_string()),
            _ => ParserOutcome::Continue,
        }
    }
}

fn dispatch_osc_target(
    osc_target: &OscTarget,
    osc_internal_type: AnsiOscInternalType,
    params: Vec<Option<AnsiOscToken>>,
    raw_params: &[u8],
    seq_trace: &SequenceTracer,
    output: &mut Vec<TerminalOutput>,
) {
    match *osc_target {
        OscTarget::Background => {
            output.push(TerminalOutput::OscResponse(
                AnsiOscType::RequestColorQueryBackground(osc_internal_type),
            ));
        }
        OscTarget::Foreground => {
            output.push(TerminalOutput::OscResponse(
                AnsiOscType::RequestColorQueryForeground(osc_internal_type),
            ));
        }
        OscTarget::CursorColor => {
            output.push(TerminalOutput::OscResponse(
                AnsiOscType::RequestColorQueryCursor(osc_internal_type),
            ));
        }
        OscTarget::TitleBar | OscTarget::IconName => {
            output.push(TerminalOutput::OscResponse(AnsiOscType::SetTitleBar(
                osc_internal_type.to_string(),
            )));
        }
        OscTarget::Ftcs => {
            // Extract the string tokens after "133" and pass
            // them to the FTCS parser.  E.g. for
            // `OSC 133 ; D ; 0 ST` → params_strs = ["D", "0"]
            let ftcs_strs: Vec<&str> = params
                .iter()
                .skip(1) // skip the "133" token
                .filter_map(|t| match t {
                    Some(AnsiOscToken::String(s)) => Some(s.as_str()),
                    _ => None,
                })
                .collect();

            if let Some(marker) = parse_ftcs_params(&ftcs_strs) {
                output.push(TerminalOutput::OscResponse(AnsiOscType::Ftcs(marker)));
            } else {
                tracing::warn!(
                    "OSC 133: unrecognised FTCS params: recent='{}'",
                    seq_trace.as_str()
                );
            }
        }
        OscTarget::Clipboard => {
            handle_osc_clipboard(&params, seq_trace, output);
        }
        OscTarget::PaletteColor => {
            handle_osc_palette_color(&params, seq_trace, output);
        }
        OscTarget::ResetPaletteColor => {
            handle_osc_reset_palette(&params, output);
        }
        OscTarget::RemoteHost => {
            output.push(TerminalOutput::OscResponse(AnsiOscType::RemoteHost(
                osc_internal_type.to_string(),
            )));
        }
        OscTarget::Url => {
            let url_response = UrlResponse::from(params);
            output.push(TerminalOutput::OscResponse(AnsiOscType::Url(url_response)));
        }
        OscTarget::ResetCursorColor => {
            output.push(TerminalOutput::OscResponse(AnsiOscType::ResetCursorColor));
        }
        OscTarget::ResetForeground => {
            output.push(TerminalOutput::OscResponse(
                AnsiOscType::ResetForegroundColor,
            ));
        }
        OscTarget::ResetBackground => {
            output.push(TerminalOutput::OscResponse(
                AnsiOscType::ResetBackgroundColor,
            ));
        }
        OscTarget::ITerm2 => {
            handle_osc_iterm2(raw_params, seq_trace, output);
        }
        OscTarget::Unknown => {
            // Unknown OSC sequences are silently consumed (like
            // xterm/VTE).  Downgraded from error!/Invalid to warn!
            // so they don't spam logs during normal usage.
            tracing::warn!(
                "Unknown OSC Target (silently consumed): type_number={osc_internal_type:?}, recent='{}'",
                seq_trace.as_str()
            );
        }
    }
}

const fn is_osc_terminator(b: &[u8]) -> bool {
    matches!(b, [.., 0x07] | [.., 0x1b, 0x5c])
}

// Strips individual trailing terminator bytes from the accumulated OSC parameter buffer.
// Works in tandem with `is_osc_terminator` (which detects the full ST sequence on the buffer)
// to clean up after termination is detected.
const fn is_final_character_osc_terminator(b: u8) -> bool {
    b == 0x5c || b == 0x07 || b == 0x1b
}

fn is_valid_osc_param(b: u8) -> bool {
    // if the character is a printable character, or is 0x1b or 0x5c then it is valid
    (0x20..=0x7E).contains(&b) || (0x80..=0xff).contains(&b) || b == 0x1b || b == 0x07
}

/// # Errors
/// Will return an error if a parameter segment cannot be parsed as an `AnsiOscToken`.
fn split_params_into_semicolon_delimited_tokens(
    params: &[u8],
) -> Result<Vec<Option<AnsiOscToken>>> {
    params
        .split(|b| *b == b';')
        .map(parse_param_as::<AnsiOscToken>)
        .collect::<Result<Vec<Option<AnsiOscToken>>>>()
}

fn extract_param(idx: usize, params: &[Option<AnsiOscToken>]) -> Option<AnsiOscToken> {
    params.get(idx).and_then(std::clone::Clone::clone)
}
