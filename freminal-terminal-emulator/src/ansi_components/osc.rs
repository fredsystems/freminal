// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//use eframe::egui::Color32;

use crate::ansi::ParserOutcome;
use crate::ansi_components::tracer::{SequenceTraceable, SequenceTracer};
use anyhow::Result;
use freminal_common::buffer_states::osc::{
    AnsiOscInternalType, AnsiOscToken, AnsiOscType, OscTarget, UrlResponse,
};
use freminal_common::buffer_states::terminal_output::TerminalOutput;

#[derive(Eq, PartialEq, Debug)]
pub enum AnsiOscParserState {
    Params,
    //Intermediates,
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
        info!("current buffer trace: {}", self.seq_trace.as_str());
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
                    warn!("Invalid OSC param: {:x}", b);
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
            // OscParserState::Intermediates => {
            //     panic!("OscParser should not be in intermediates state");
            // }
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
                if let Ok(params) = split_params_into_semicolon_delimited_usize(&self.params) {
                    let Some(type_number) = extract_param(0, &params) else {
                        output.push(TerminalOutput::Invalid);
                        return ParserOutcome::Invalid(format!(
                            "Invalid OSC params: recent='{}'",
                            self.seq_trace.as_str()
                        ));
                    };

                    // Only clone what’s actually reused later.
                    let osc_target = OscTarget::from(&type_number);
                    let osc_internal_type = AnsiOscInternalType::from(&params);

                    match osc_target {
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
                        OscTarget::TitleBar | OscTarget::IconName => {
                            output.push(TerminalOutput::OscResponse(AnsiOscType::SetTitleBar(
                                osc_internal_type.to_string(),
                            )));
                        }
                        OscTarget::Ftcs => {
                            output.push(TerminalOutput::OscResponse(AnsiOscType::Ftcs(
                                osc_internal_type.to_string(),
                            )));
                        }
                        OscTarget::RemoteHost => {
                            output.push(TerminalOutput::OscResponse(AnsiOscType::RemoteHost(
                                osc_internal_type.to_string(),
                            )));
                        }
                        OscTarget::Url => {
                            // `params` is reused here → must keep the clone above
                            let url_response = UrlResponse::from(params);
                            output
                                .push(TerminalOutput::OscResponse(AnsiOscType::Url(url_response)));
                        }
                        OscTarget::ResetCursorColor => {
                            output.push(TerminalOutput::OscResponse(AnsiOscType::ResetCursorColor));
                        }
                        OscTarget::ITerm2 => {
                            output.push(TerminalOutput::OscResponse(AnsiOscType::ITerm2));
                        }
                        OscTarget::Unknown => {
                            // `type_number` reused here → must keep the clone above
                            output.push(TerminalOutput::Invalid);
                            return ParserOutcome::Invalid(format!(
                                "Unknown OSC Target: type_number={type_number:?}, recent='{}'",
                                self.seq_trace.as_str()
                            ));
                        }
                    }
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

// the terminator of the OSC sequence is a ST (0x5C) or BEL (0x07)
const fn is_osc_terminator(b: &[u8]) -> bool {
    matches!(b, [.., 0x07] | [.., 0x1b, 0x5c])
}

// FIXME: Support ST (0x1b)\ as a terminator
const fn is_final_character_osc_terminator(b: u8) -> bool {
    b == 0x5c || b == 0x07 || b == 0x1b
}

fn is_valid_osc_param(b: u8) -> bool {
    // if the character is a printable character, or is 0x1b or 0x5c then it is valid
    (0x20..=0x7E).contains(&b) || (0x80..=0xff).contains(&b) || b == 0x1b || b == 0x07
}

/// # Errors
/// Will return an error if the parameter is not a valid number
pub fn split_params_into_semicolon_delimited_usize(
    params: &[u8],
) -> Result<Vec<Option<AnsiOscToken>>> {
    params
        .split(|b| *b == b';')
        .map(parse_param_as::<AnsiOscToken>)
        .collect::<Result<Vec<Option<AnsiOscToken>>>>()
}

/// # Errors
///
/// Will return an error if the parameter is not a valid number
pub fn parse_param_as<T: std::str::FromStr>(param_bytes: &[u8]) -> Result<Option<T>> {
    let param_str = std::str::from_utf8(param_bytes)?;
    if param_str.is_empty() {
        return Ok(None);
    }
    param_str.parse().map_err(|_| ()).map_or_else(
        |()| {
            warn!(
                "Failed to parse parameter ({:?}) as {:?}",
                param_bytes,
                std::any::type_name::<T>()
            );
            Err(anyhow::anyhow!("Failed to parse parameter"))
        },
        |value| Ok(Some(value)),
    )
}

pub fn extract_param(idx: usize, params: &[Option<AnsiOscToken>]) -> Option<AnsiOscToken> {
    // get the parameter at the index
    params.get(idx).and_then(std::clone::Clone::clone)
}
