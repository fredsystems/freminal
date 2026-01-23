// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use freminal_common::buffer_states::line_draw::DecSpecialGraphics;
use freminal_common::buffer_states::terminal_output::TerminalOutput;

use crate::ansi::ParserOutcome;
use crate::ansi_components::tracer::{SequenceTraceable, SequenceTracer};

#[derive(Eq, PartialEq, Debug)]
pub enum StandardParserState {
    Params,
    Intermediates,
    Finished,
    Invalid,
    InvalidFinished,
}

#[derive(Eq, PartialEq, Debug)]
pub enum StandardOutput {
    SevenBitControl,
    EightBitControl,
    AnsiConformanceLevelOne,
    AnsiConformanceLevelTwo,
    AnsiConformanceLevelThree,
    DoubleLineHeightTop,
    DoubleLineHeightBottom,
    SingleWidthLine,
    DoubleWidthLine,
    ScreenAlignmentTest,
    CharsetDefault,
    CharsetUTF8,
    CharsetG0,
    CharsetG1,
    CharsetG2,
    CharsetG3,
}

#[derive(Eq, PartialEq, Debug)]
pub struct StandardParser {
    pub state: StandardParserState,
    pub params: Vec<u8>,
    pub intermediates: Vec<u8>,
    pub sequence: Vec<u8>,
    pub dcs: bool,
    pub apc: bool,

    // Internal trace of recent bytes for diagnostics.
    seq_trace: SequenceTracer,
}

impl Default for StandardParser {
    fn default() -> Self {
        Self::new()
    }
}

impl SequenceTraceable for StandardParser {
    #[inline]
    fn seq_tracer(&mut self) -> &mut SequenceTracer {
        &mut self.seq_trace
    }
    #[inline]
    fn seq_tracer_ref(&self) -> &SequenceTracer {
        &self.seq_trace
    }
}

impl StandardParser {
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: StandardParserState::Intermediates,
            params: Vec::with_capacity(8),
            intermediates: Vec::with_capacity(8),
            sequence: Vec::with_capacity(8),
            dcs: false,
            apc: false,
            seq_trace: SequenceTracer::new(),
        }
    }

    #[must_use]
    pub fn contains_string_terminator(&self) -> bool {
        self.sequence.ends_with(b"\x1b\\")
    }

    /// Expose current sequence trace for testing and diagnostics.
    #[must_use]
    pub fn trace_str(&self) -> String {
        self.seq_trace.as_str()
    }

    /// Push a byte into the parser
    ///
    /// # Errors
    /// Will return an error if the parser is in a finished state
    #[tracing::instrument(level = "trace", skip_all)]
    pub fn push(&mut self, b: u8) -> ParserOutcome {
        self.append_trace(b);

        if let StandardParserState::Finished | StandardParserState::InvalidFinished = &self.state {
            return ParserOutcome::Invalid("Parser pushed to after finish".to_string());
        }

        self.sequence.push(b);

        match self.state {
            StandardParserState::Intermediates => {
                if is_standard_intermediate_final(b) {
                    self.state = StandardParserState::Finished;

                    self.seq_trace.trim_control_tail();
                    self.intermediates.push(b);

                    return ParserOutcome::Finished;
                } else if is_standard_intermediate_continue(b) {
                    self.state = StandardParserState::Params;
                    self.intermediates.push(b);

                    if b == b'P' {
                        self.dcs = true;
                    } else if b == b'_' {
                        self.apc = true;
                    }

                    return ParserOutcome::Continue;
                }

                self.state = StandardParserState::Invalid;

                ParserOutcome::Invalid("Invalid intermediate byte".to_string())
            }
            StandardParserState::Params => {
                if self.dcs || self.apc {
                    self.params.push(b);

                    if self.contains_string_terminator() {
                        self.state = StandardParserState::Finished;

                        self.seq_trace.trim_control_tail();
                        return ParserOutcome::Finished;
                    }

                    return ParserOutcome::Continue;
                } else if is_standard_param(b) {
                    self.params.push(b);
                    self.state = StandardParserState::Finished;

                    self.seq_trace.trim_control_tail();

                    return ParserOutcome::Finished;
                }

                self.state = StandardParserState::Invalid;

                ParserOutcome::Invalid("Invalid parameter byte".to_string())
            }

            _ => ParserOutcome::Continue,
        }
    }

    /// Push a byte into the parser and return the next state
    ///
    /// # Errors
    /// Will return an error if the parser encounters an invalid state
    #[allow(clippy::too_many_lines)]
    pub fn standard_parser_inner(
        &mut self,
        b: u8,
        output: &mut Vec<TerminalOutput>,
    ) -> ParserOutcome {
        let return_state = self.push(b);

        if let ParserOutcome::Invalid(_) = return_state {
            return return_state;
        }

        if self.state == StandardParserState::Finished {
            if self.dcs {
                output.push(TerminalOutput::DeviceControlString(std::mem::take(
                    &mut self.sequence,
                )));
            } else if self.apc {
                output.push(TerminalOutput::ApplicationProgramCommand(std::mem::take(
                    &mut self.sequence,
                )));
            }

            return ParserOutcome::Finished;
        }

        match self.state {
            StandardParserState::Finished => match self.intermediates.first() {
                None => ParserOutcome::Invalid("No intermediates".to_string()),
                Some(b' ') => {
                    let value = self.params.first();

                    match value {
                        None => ParserOutcome::Invalid("No params".to_string()),
                        Some(value) => {
                            let value = *value as char;
                            match value {
                                'F' => output.push(TerminalOutput::SevenBitControl),
                                'G' => output.push(TerminalOutput::EightBitControl),
                                'L' => output.push(TerminalOutput::AnsiConformanceLevelOne),
                                'M' => output.push(TerminalOutput::AnsiConformanceLevelTwo),
                                'N' => output.push(TerminalOutput::AnsiConformanceLevelThree),
                                _ => {
                                    output.push(TerminalOutput::Invalid);
                                    return ParserOutcome::Invalid(
                                        "Invalid param value".to_string(),
                                    );
                                }
                            }

                            ParserOutcome::Finished
                        }
                    }
                }
                Some(b'#') => {
                    let value = self.params.first();

                    match value {
                        None => ParserOutcome::Invalid("No params".to_string()),
                        Some(value) => {
                            let value = *value as char;
                            match value {
                                '3' => output.push(TerminalOutput::DoubleLineHeightTop),
                                '4' => output.push(TerminalOutput::DoubleLineHeightBottom),
                                '5' => output.push(TerminalOutput::SingleWidthLine),
                                '6' => output.push(TerminalOutput::DoubleWidthLine),
                                '8' => output.push(TerminalOutput::ScreenAlignmentTest),
                                _ => {
                                    output.push(TerminalOutput::Invalid);
                                    return ParserOutcome::Invalid(
                                        "Invalid param value".to_string(),
                                    );
                                }
                            }

                            ParserOutcome::Finished
                        }
                    }
                }
                Some(b'%') => {
                    let value = self.params.first();

                    match value {
                        None => ParserOutcome::Invalid("No params".to_string()),
                        Some(value) => {
                            let value = *value as char;
                            match value {
                                '@' => output.push(TerminalOutput::CharsetDefault),
                                'G' => output.push(TerminalOutput::CharsetUTF8),
                                _ => {
                                    output.push(TerminalOutput::Invalid);
                                    return ParserOutcome::Invalid(
                                        "Invalid param value".to_string(),
                                    );
                                }
                            }

                            ParserOutcome::Finished
                        }
                    }
                }
                Some(b'(') => {
                    let value = self.params.first();

                    match value {
                        None => ParserOutcome::Invalid("No params".to_string()),
                        Some(value) => {
                            let value = *value as char;

                            match value {
                                '0' => output.push(TerminalOutput::DecSpecialGraphics(
                                    DecSpecialGraphics::Replace,
                                )),
                                'B' => output.push(TerminalOutput::DecSpecialGraphics(
                                    DecSpecialGraphics::DontReplace,
                                )),
                                'C' => output.push(TerminalOutput::CharsetG0),
                                _ => {
                                    output.push(TerminalOutput::Invalid);
                                    return ParserOutcome::Invalid(
                                        "Invalid param value".to_string(),
                                    );
                                }
                            }
                            ParserOutcome::Finished
                        }
                    }
                }
                Some(b')') => {
                    let value = self.params.first();

                    match value {
                        None => ParserOutcome::Invalid("No params".to_string()),
                        Some(value) => {
                            if *value == b'C' {
                                output.push(TerminalOutput::CharsetG1);
                            } else {
                                output.push(TerminalOutput::Invalid);
                                return ParserOutcome::Invalid("Invalid param value".to_string());
                            }

                            ParserOutcome::Finished
                        }
                    }
                }
                Some(b'*') => {
                    let value = self.params.first();

                    match value {
                        None => ParserOutcome::Invalid("No params".to_string()),
                        Some(value) => {
                            if *value == b'C' {
                                output.push(TerminalOutput::CharsetG2);
                            } else {
                                output.push(TerminalOutput::Invalid);
                                return ParserOutcome::Invalid("Invalid param value".to_string());
                            }
                            ParserOutcome::Finished
                        }
                    }
                }
                Some(b'+') => {
                    let value = self.params.first();

                    match value {
                        None => ParserOutcome::Invalid("No params".to_string()),
                        Some(value) => {
                            match value {
                                // FIXME: Should this be the same as DecSpecialGraphics::Replace?
                                b'0' => output.push(TerminalOutput::DecSpecial),
                                b'A' => output.push(TerminalOutput::CharsetUK),
                                b'B' => output.push(TerminalOutput::CharsetUSASCII),
                                b'4' => output.push(TerminalOutput::CharsetDutch),
                                b'5' | b'C' => output.push(TerminalOutput::CharsetFinnish),
                                b'R' => output.push(TerminalOutput::CharsetFrench),
                                b'Q' => output.push(TerminalOutput::CharsetFrenchCanadian),
                                b'K' => output.push(TerminalOutput::CharsetGerman),
                                b'Y' => output.push(TerminalOutput::CharsetItalian),
                                b'E' | b'6' => output.push(TerminalOutput::CharsetNorwegianDanish),
                                b'Z' => output.push(TerminalOutput::CharsetSpanish),
                                b'H' | b'7' => output.push(TerminalOutput::CharsetSwedish),
                                b'=' => output.push(TerminalOutput::CharsetSwiss),
                                _ => {
                                    output.push(TerminalOutput::Invalid);
                                    return ParserOutcome::Invalid(
                                        "Invalid param value".to_string(),
                                    );
                                }
                            }

                            ParserOutcome::Finished
                        }
                    }
                }
                Some(value) => {
                    let value = *value as char;
                    match value {
                        '7' => output.push(TerminalOutput::SaveCursor),
                        '8' => output.push(TerminalOutput::RestoreCursor),
                        '=' => output.push(TerminalOutput::ApplicationKeypadMode),
                        '>' => output.push(TerminalOutput::NormalKeypadMode),
                        'F' => output.push(TerminalOutput::CursorToLowerLeftCorner),
                        'c' => output.push(TerminalOutput::ResetDevice),
                        'l' => output.push(TerminalOutput::MemoryLock),
                        'm' => output.push(TerminalOutput::MemoryUnlock),
                        'n' => output.push(TerminalOutput::CharsetG2AsGL),
                        'o' => output.push(TerminalOutput::CharsetG3AsGL),
                        '|' => output.push(TerminalOutput::CharsetG3AsGR),
                        '}' => output.push(TerminalOutput::CharsetG2AsGR),
                        '~' => output.push(TerminalOutput::CharsetG1AsGR),
                        'M' => {
                            output.push(TerminalOutput::SetCursorPosRel {
                                x: None,
                                y: Some(-1),
                            });
                        }
                        'D' => {
                            output.push(TerminalOutput::SetCursorPosRel {
                                x: None,
                                y: Some(1),
                            });
                        }
                        'E' => {
                            output.push(TerminalOutput::SetCursorPosRel {
                                x: Some(1),
                                y: Some(1),
                            });
                        }
                        _ => {
                            output.push(TerminalOutput::Invalid);
                            return ParserOutcome::Invalid(
                                "Invalid intermediate value".to_string(),
                            );
                        }
                    }

                    ParserOutcome::Finished
                }
            },
            StandardParserState::Invalid => {
                ParserOutcome::Invalid("Invalid parser state".to_string())
            }
            _ => ParserOutcome::Continue,
        }
    }
}

#[must_use]
pub const fn is_standard_intermediate_final(b: u8) -> bool {
    // 7 8 = > F c l m n o | } ~ are final and we want to enter the finished state

    matches!(
        b,
        0x7 | 0x8
            | 0x3e
            | 0x46
            | 0x63
            | 0x6c
            | 0x6d
            | 0x6e
            | 0x6f
            | 0x7c
            | 0x7d
            | 0x7e
            | 0x3d
            | 0x37
            | 0x38
            | 0x4d
            | 0x44
            | 0x45
    )
}

#[must_use]
pub const fn is_standard_intermediate_continue(b: u8) -> bool {
    // space # % ( ) * + is a state where we want to continue and get a Params

    matches!(
        b,
        0x20 | 0x23 | 0x25 | 0x28 | 0x29 | 0x2a | 0x2b | 0x50 | 0x5f | 0x5c
    )
}

#[must_use]
pub const fn is_standard_param(b: u8) -> bool {
    // F G L M N 3 4 5 6 8 @ G C 0 A B 4 5 R Q K Y E Z H 7 = are valid params

    matches!(
        b,
        0x46 | 0x47
            | 0x4c
            | 0x4d
            | 0x4e
            | 0x33
            | 0x34
            | 0x35
            | 0x36
            | 0x38
            | 0x40
            | 0x43
            | 0x30
            | 0x41
            | 0x42
            | 0x52
            | 0x51
            | 0x4b
            | 0x59
            | 0x45
            | 0x5a
            | 0x48
            | 0x37
            | 0x3d
    )
}
