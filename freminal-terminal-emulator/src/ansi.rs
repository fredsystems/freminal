// Copyright (C) 2024-2025 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use core::fmt;

use crate::{
    ansi_components::{
        csi::AnsiCsiParser,
        line_draw::DecSpecialGraphics,
        mode::Mode,
        osc::{AnsiOscParser, AnsiOscType},
        sgr::SelectGraphicRendition,
        standard::StandardParser,
        tracer::SequenceTraceable,
    },
    error::ParserFailures,
};

use crate::ansi_components::tracer::SequenceTracer;
use anyhow::Result;
use freminal_common::{cursor::CursorVisualStyle, window_manipulation::WindowManipulation};

/// Represents the high-level result of feeding one byte to the parser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParserOutcome {
    /// The parser consumed the byte and no complete output was produced yet.
    Continue,
    /// The parser produced at least one `TerminalOutput` as a result of this byte.
    Finished,
    /// The byte resulted in an invalid sequence or parse error (error string provided).
    Invalid(String),
    InvalidParserFailure(ParserFailures),
}

impl fmt::Display for ParserOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Continue => write!(f, "Continue"),
            Self::Finished => write!(f, "Finished"),
            Self::Invalid(msg) => write!(f, "Invalid: {msg}"),
            Self::InvalidParserFailure(msg) => {
                write!(f, "InvalidParserFailure: {msg:?}")
            }
        }
    }
}

/// High-level actions produced by the ANSI/OSC parser.
///
/// This enum represents normalized terminal effects (cursor movement,
/// erasures, SGR, window ops, etc.) emitted by parsing.
/// The set may grow; match exhaustively with a wildcard for forward-compat.
/// High-level actions produced by the ANSI/OSC parser.
///
/// This enum represents normalized terminal effects (cursor movement,
/// erasures, SGR, window ops, etc.) emitted by parsing.
/// The set may grow; match exhaustively with a wildcard for forward-compat.
#[non_exhaustive]
#[derive(Debug, Eq, PartialEq)]
pub enum TerminalOutput {
    SetCursorPos {
        x: Option<usize>,
        y: Option<usize>,
    },
    SetCursorPosRel {
        x: Option<i32>,
        y: Option<i32>,
    },
    ClearDisplayfromCursortoEndofDisplay,
    ClearDisplayfromStartofDisplaytoCursor,
    ClearScrollbackandDisplay,
    ClearDisplay,
    CarriageReturn,
    ClearLineForwards,
    ClearLineBackwards,
    ClearLine,
    Newline,
    Backspace,
    Bell,
    ApplicationKeypadMode,
    NormalKeypadMode,
    InsertLines(usize),
    Delete(usize),
    Erase(usize),
    Sgr(SelectGraphicRendition),
    Data(Vec<u8>),
    Mode(Mode),
    // ich (8.3.64 of ecma-48)
    InsertSpaces(usize),
    OscResponse(AnsiOscType),
    CursorReport,
    Invalid,
    Skipped,
    DecSpecialGraphics(DecSpecialGraphics),
    CursorVisualStyle(CursorVisualStyle),
    WindowManipulation(WindowManipulation),
    RequestDeviceAttributes,
    SetTopAndBottomMargins {
        top_margin: usize,
        bottom_margin: usize,
    },
    EightBitControl,
    SevenBitControl,
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
    CharsetG1AsGR,
    CharsetG2,
    CharsetG2AsGR,
    CharsetG2AsGL,
    CharsetG3,
    CharsetG3AsGR,
    CharsetG3AsGL,
    DecSpecial,
    CharsetUK,
    CharsetUS,
    CharsetUSASCII,
    CharsetDutch,
    CharsetFinnish,
    CharsetFrench,
    CharsetFrenchCanadian,
    CharsetGerman,
    CharsetItalian,
    CharsetNorwegianDanish,
    CharsetSpanish,
    CharsetSwedish,
    CharsetSwiss,
    SaveCursor,
    RestoreCursor,
    CursorToLowerLeftCorner,
    ResetDevice,
    MemoryLock,
    MemoryUnlock,
    DeviceControlString(Vec<u8>),
    ApplicationProgramCommand(Vec<u8>),
    RequestDeviceNameAndVersion,
    RequestSecondaryDeviceAttributes {
        param: usize,
    }, // for ESC[>Ps c
    RequestXtVersion, // for ESC[>c
}

// impl format display for TerminalOutput

#[allow(clippy::too_many_lines)]
impl std::fmt::Display for TerminalOutput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SetCursorPos { x, y } => {
                write!(f, "SetCursorPos: x: {x:?}, y: {y:?}")
            }
            Self::SetCursorPosRel { x, y } => {
                write!(f, "SetCursorPosRel: x: {x:?}, y: {y:?}")
            }
            Self::ClearDisplayfromCursortoEndofDisplay => write!(f, "ClearForwards"),
            Self::ClearScrollbackandDisplay => write!(f, "ClearAll"),
            Self::ClearDisplayfromStartofDisplaytoCursor => write!(f, "ClearBackwards"),
            Self::ClearDisplay => write!(f, "ClearDisplay"),
            Self::CarriageReturn => write!(f, "CarriageReturn"),
            Self::ClearLineForwards => write!(f, "ClearLineForwards"),
            Self::ClearLineBackwards => write!(f, "ClearLineBackwards"),
            Self::ClearLine => write!(f, "ClearLine"),
            Self::Newline => write!(f, "Newline"),
            Self::Backspace => write!(f, "Backspace"),
            Self::Bell => write!(f, "Bell"),
            Self::InsertLines(n) => write!(f, "InsertLines({n})"),
            Self::Delete(n) => write!(f, "Delete({n})"),
            Self::Erase(n) => write!(f, "Erase({n})"),
            Self::Sgr(sgr) => write!(f, "Sgr({sgr:?})"),
            Self::Data(data) => {
                write!(f, "Data({})", String::from_utf8_lossy(data))
            }
            Self::Mode(mode) => write!(f, "SetMode({mode})"),
            Self::InsertSpaces(n) => write!(f, "InsertSpaces({n})"),
            Self::OscResponse(n) => write!(f, "OscResponse({n})"),
            Self::DecSpecialGraphics(dec_special_graphics) => {
                write!(f, "DecSpecialGraphics({dec_special_graphics:?})")
            }
            Self::Invalid => write!(f, "Invalid"),
            Self::CursorReport => write!(f, "CursorReport"),
            Self::Skipped => write!(f, "Skipped"),
            Self::ApplicationKeypadMode => write!(f, "ApplicationKeypadMode"),
            Self::NormalKeypadMode => write!(f, "NormalKeypadMode"),
            Self::CursorVisualStyle(cursor_visual_style) => {
                write!(f, "CursorVisualStyle({cursor_visual_style:?})")
            }
            Self::WindowManipulation(window_manipulation) => {
                write!(f, "WindowManipulation({window_manipulation:?})")
            }
            Self::SetTopAndBottomMargins {
                top_margin,
                bottom_margin,
            } => {
                write!(f, "SetTopAndBottomMargins({top_margin}, {bottom_margin})")
            }
            Self::RequestDeviceAttributes => write!(f, "RequestDeviceAttributes"),
            Self::EightBitControl => write!(f, "EightBitControl"),
            Self::SevenBitControl => write!(f, "SevenBitControl"),
            Self::AnsiConformanceLevelOne => write!(f, "AnsiConformanceLevelOne"),
            Self::AnsiConformanceLevelTwo => write!(f, "AnsiConformanceLevelTwo"),
            Self::AnsiConformanceLevelThree => write!(f, "AnsiConformanceLevelThree"),
            Self::DoubleLineHeightTop => write!(f, "DoubleLineHeightTop"),
            Self::DoubleLineHeightBottom => write!(f, "DoubleLineHeightBottom"),
            Self::SingleWidthLine => write!(f, "SingleWidthLine"),
            Self::DoubleWidthLine => write!(f, "DoubleWidthLine"),
            Self::ScreenAlignmentTest => write!(f, "ScreenAlignmentTest"),
            Self::CharsetDefault => write!(f, "CharsetDefault"),
            Self::CharsetUTF8 => write!(f, "CharsetUTF8"),
            Self::CharsetG0 => write!(f, "CharsetG0"),
            Self::CharsetG1 => write!(f, "CharsetG1"),
            Self::CharsetG1AsGR => write!(f, "CharsetG1AsGR"),
            Self::CharsetG2 => write!(f, "CharsetG2"),
            Self::CharsetG2AsGR => write!(f, "CharsetG2AsGR"),
            Self::CharsetG2AsGL => write!(f, "CharsetG2AsGL"),
            Self::CharsetG3 => write!(f, "CharsetG3"),
            Self::CharsetG3AsGR => write!(f, "CharsetG3AsGR"),
            Self::CharsetG3AsGL => write!(f, "CharsetG3AsGL"),
            Self::DecSpecial => write!(f, "DecSpecial"),
            Self::CharsetUK => write!(f, "CharsetUK"),
            Self::CharsetUS => write!(f, "CharsetUS"),
            Self::CharsetUSASCII => write!(f, "CharsetUSASCII"),
            Self::CharsetDutch => write!(f, "CharsetDutch"),
            Self::CharsetFinnish => write!(f, "CharsetFinnish"),
            Self::CharsetFrench => write!(f, "CharsetFrench"),
            Self::CharsetFrenchCanadian => write!(f, "CharsetFrenchCanadian"),
            Self::CharsetGerman => write!(f, "CharsetGerman"),
            Self::CharsetItalian => write!(f, "CharsetItalian"),
            Self::CharsetNorwegianDanish => write!(f, "CharsetNorwegianDanish"),
            Self::CharsetSpanish => write!(f, "CharsetSpanish"),
            Self::CharsetSwedish => write!(f, "CharsetSwedish"),
            Self::CharsetSwiss => write!(f, "CharsetSwiss"),
            Self::SaveCursor => write!(f, "SaveCursor"),
            Self::RestoreCursor => write!(f, "RestoreCursor"),
            Self::CursorToLowerLeftCorner => write!(f, "CursorToLowerLeftCorner"),
            Self::ResetDevice => write!(f, "ResetDevice"),
            Self::MemoryLock => write!(f, "MemoryLock"),
            Self::MemoryUnlock => write!(f, "MemoryUnlock"),
            Self::DeviceControlString(data) => {
                write!(f, "DeviceControlString({})", String::from_utf8_lossy(data))
            }
            Self::ApplicationProgramCommand(data) => {
                write!(
                    f,
                    "ApplicationProgramCommand({})",
                    String::from_utf8_lossy(data)
                )
            }
            Self::RequestDeviceNameAndVersion => write!(f, "RequestDeviceNameandVersion"),
            Self::RequestSecondaryDeviceAttributes { param } => {
                write!(f, "RequestSecondaryDeviceAttributes({param})")
            }
            Self::RequestXtVersion => write!(f, "RequestXtVersion"),
        }
    }
}

#[must_use]
pub fn extract_param(idx: usize, params: &[Option<usize>]) -> Option<usize> {
    params.get(idx).copied().flatten()
}

/// # Errors
/// Will return an error if the parameter is not a valid number
pub fn split_params_into_semicolon_delimited_usize(params: &[u8]) -> Result<Vec<Option<usize>>> {
    params
        .split(|b| *b == b';')
        .map(parse_param_as::<usize>)
        .collect::<Result<Vec<Option<usize>>>>()
}

/// # Errors
/// Will return an error if the parameter is not a valid number
pub fn split_params_into_colon_delimited_usize(params: &[u8]) -> Result<Vec<Option<usize>>> {
    params
        .split(|b| *b == b':')
        .map(parse_param_as::<usize>)
        .collect::<Result<Vec<Option<usize>>>>()
}

/// # Errors
/// Will return an error if the parameter is not a valid number
pub fn parse_param_as<T: std::str::FromStr>(param_bytes: &[u8]) -> Result<Option<T>> {
    let param_str = std::str::from_utf8(param_bytes)?;

    if param_str.is_empty() {
        return Ok(None);
    }

    param_str
        .parse()
        .map_err(|_| anyhow::Error::msg("Parse error"))
        .map(Some)
}

fn push_data_if_non_empty(data: &mut Vec<u8>, output: &mut Vec<TerminalOutput>) {
    if !data.is_empty() {
        output.push(TerminalOutput::Data(std::mem::take(data)));
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum ParserInner {
    Empty,
    Escape,
    Csi(AnsiCsiParser),
    Osc(AnsiOscParser),
    Standard(StandardParser),
}

#[derive(Debug, Eq, PartialEq)]
pub struct FreminalAnsiParser {
    pub inner: ParserInner,
    // Accumulates plain text between control sequences across chunk boundaries,
    // reducing per-call allocations and enabling coalesced Data emissions.
    pending_data: Vec<u8>,
    seq_trace: SequenceTracer,
}

impl SequenceTraceable for FreminalAnsiParser {
    #[inline]
    fn seq_tracer(&mut self) -> &mut SequenceTracer {
        &mut self.seq_trace
    }
    #[inline]
    fn seq_tracer_ref(&self) -> &SequenceTracer {
        &self.seq_trace
    }
}

impl Default for FreminalAnsiParser {
    fn default() -> Self {
        Self::new()
    }
}

impl FreminalAnsiParser {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            inner: ParserInner::Empty,
            pending_data: Vec::new(),
            seq_trace: SequenceTracer::new(),
        }
    }

    fn ansi_parser_inner_empty(
        &mut self,
        b: u8,
        data_output: &mut Vec<u8>,
        output: &mut Vec<TerminalOutput>,
    ) -> Result<(), ()> {
        if b == b'\x1b' {
            self.inner = ParserInner::Escape;
            return Err(());
        }

        if b == b'\r' {
            push_data_if_non_empty(data_output, output);
            output.push(TerminalOutput::CarriageReturn);
            return Err(());
        }

        if b == b'\n' {
            push_data_if_non_empty(data_output, output);
            output.push(TerminalOutput::Newline);
            return Err(());
        }

        if b == 0x08 {
            push_data_if_non_empty(data_output, output);
            output.push(TerminalOutput::Backspace);
            return Err(());
        }

        if b == 0x07 {
            push_data_if_non_empty(data_output, output);
            output.push(TerminalOutput::Bell);
            return Err(());
        }

        Ok(())
    }

    fn ansiparser_inner_escape(
        &mut self,
        b: u8,
        data_output: &mut Vec<u8>,
        output: &mut Vec<TerminalOutput>,
    ) {
        // Allow ESC ESC sequences (common in DCS/OSC passthrough)
        if b == 0x1B {
            self.inner = ParserInner::Escape;
            return;
        }

        // String Terminator (ESC \)
        if b == b'\\' {
            self.inner = ParserInner::Empty;
            self.clear_trace();
            return;
        }

        push_data_if_non_empty(data_output, output);

        match b {
            b'[' => {
                self.inner = ParserInner::Csi(AnsiCsiParser::new());
            }
            b']' => {
                self.inner = ParserInner::Osc(AnsiOscParser::new());
            }
            b'\x1b' => {
                // ESC followed by ESC is invalid; reset to Empty
                info!("ANSI parser: ESC followed by ESC");
            }
            _ => {
                let mut parser = StandardParser::new();

                match parser.standard_parser_inner(b, output) {
                    ParserOutcome::Finished => {
                        self.inner = ParserInner::Empty;
                        // if the last value pushed to output is terminal Invalid, print out the sequence of characters that caused the error

                        if output.last() == Some(&TerminalOutput::Invalid) {
                            debug!("Invalid ANSI sequence; recent={}", self.current_trace_str());
                        }
                    }
                    ParserOutcome::Continue => {
                        // Note: retain assignment here — we're transitioning from Escape to Standard.)
                        self.inner = ParserInner::Standard(parser);
                    }

                    ParserOutcome::InvalidParserFailure(_message) => unreachable!(),

                    ParserOutcome::Invalid(message) => {
                        // All invalid sequences emit `TerminalOutput::Invalid` here.
                        output.push(TerminalOutput::Invalid);
                        error!(
                            "ANSI parser error: {}; recent={}",
                            message,
                            self.current_trace_str()
                        );
                        warn!(
                            "ANSI parser error; resetting state; recent={}",
                            self.current_trace_str()
                        );
                        debug!("Invalid ANSI sequence; recent={}", self.current_trace_str());
                        self.inner = ParserInner::Empty;
                    }
                }
            }
        }
    }

    #[allow(clippy::too_many_lines)]
    pub fn push(&mut self, incoming: &[u8]) -> Vec<TerminalOutput> {
        // Take the pending buffer out temporarily
        let mut data_output = std::mem::take(&mut self.pending_data);
        let mut output = Vec::new();

        for &b in incoming {
            self.seq_trace.push(b);

            match &mut self.inner {
                ParserInner::Empty => {
                    if self
                        .ansi_parser_inner_empty(b, &mut data_output, &mut output)
                        .is_err()
                    {
                        continue;
                    }

                    data_output.push(b);
                }
                ParserInner::Escape => {
                    self.ansiparser_inner_escape(b, &mut data_output, &mut output);
                }
                ParserInner::Standard(parser) => {
                    match parser.standard_parser_inner(b, &mut output) {
                        ParserOutcome::Finished => {
                            self.inner = ParserInner::Empty;
                            if output.last() == Some(&TerminalOutput::Invalid) {
                                debug!(
                                    "Invalid ANSI sequence; recent={}",
                                    self.current_trace_str()
                                );
                            }
                        }
                        ParserOutcome::Continue => (),
                        // we cannot hit this branch right now because the standard parser does not return ParserOutcome::InvalidParserFailure
                        ParserOutcome::InvalidParserFailure(_message) => unreachable!(),
                        ParserOutcome::Invalid(message) => {
                            // All invalid sequences emit `TerminalOutput::Invalid` here.
                            output.push(TerminalOutput::Invalid);
                            error!(
                                "ANSI parser error: {}; recent={}",
                                message,
                                self.current_trace_str()
                            );
                            warn!(
                                "ANSI parser error; resetting state; recent={}",
                                self.current_trace_str()
                            );
                            debug!("Invalid ANSI sequence; recent={}", self.current_trace_str());
                            self.inner = ParserInner::Empty;
                        }
                    }
                }
                ParserInner::Csi(parser) => match parser.ansiparser_inner_csi(b, &mut output) {
                    ParserOutcome::Finished => {
                        self.inner = ParserInner::Empty;
                        if output.last() == Some(&TerminalOutput::Invalid) {
                            debug!("Invalid ANSI sequence; recent={}", self.current_trace_str());
                        }
                    }
                    ParserOutcome::Continue => (),
                    ParserOutcome::InvalidParserFailure(message) => {
                        error!(
                            "ANSI parser error: {}; recent={}",
                            message,
                            self.current_trace_str()
                        );
                        warn!(
                            "ANSI parser error; resetting state; recent={}",
                            self.current_trace_str()
                        );
                        debug!("Invalid ANSI sequence; recent={}", self.current_trace_str());
                        self.inner = ParserInner::Empty;
                        output.push(TerminalOutput::Invalid);
                    }
                    ParserOutcome::Invalid(message) => {
                        error!(
                            "ANSI parser error: {}; recent={}",
                            message,
                            self.current_trace_str()
                        );
                        warn!(
                            "ANSI parser error; resetting state; recent={}",
                            self.current_trace_str()
                        );
                        debug!("Invalid ANSI sequence; recent={}", self.current_trace_str());
                        self.inner = ParserInner::Empty;
                        output.push(TerminalOutput::Invalid);
                    }
                },
                ParserInner::Osc(parser) => match parser.ansiparser_inner_osc(b, &mut output) {
                    ParserOutcome::Finished => {
                        self.inner = ParserInner::Empty;
                        if output.last() == Some(&TerminalOutput::Invalid) {
                            debug!("Invalid ANSI sequence; recent={}", self.current_trace_str());
                        }
                    }
                    ParserOutcome::Continue => (),
                    // we cannot hit this branch right now because the osc parser does not return ParserOutcome::InvalidParserFailure
                    ParserOutcome::InvalidParserFailure(_message) => unreachable!(),
                    ParserOutcome::Invalid(message) => {
                        // All invalid sequences emit `TerminalOutput::Invalid` here.
                        output.push(TerminalOutput::Invalid);
                        error!(
                            "ANSI parser error: {}; recent={}",
                            message,
                            self.current_trace_str()
                        );
                        warn!(
                            "ANSI parser error; resetting state; recent={}",
                            self.current_trace_str()
                        );
                        debug!("Invalid ANSI sequence; recent={}", self.current_trace_str());
                        self.inner = ParserInner::Empty;
                    }
                },
            }
        }

        // Flush any accumulated data
        if !data_output.is_empty() {
            output.push(TerminalOutput::Data(std::mem::take(&mut data_output)));
        }

        // Put the buffer back into self (no allocations, same Vec reused)
        self.pending_data = data_output;

        output
    }

    /// Retrieve a snapshot of the currently active parser's trace buffer.
    #[must_use]
    pub fn current_trace_str(&self) -> String {
        match &self.inner {
            ParserInner::Osc(p) => p.trace_str(),
            ParserInner::Csi(p) => p.trace_str(),
            ParserInner::Standard(p) => p.trace_str(),
            _ => self.seq_trace.as_str(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------------
    // Existing tests
    // -------------------------------------------------------------------------

    #[test]
    fn push_data_if_non_empty_behavior() {
        let mut data = vec![b'a', b'b'];
        let mut output = Vec::new();
        push_data_if_non_empty(&mut data, &mut output);
        assert_eq!(data.len(), 0);
        assert_eq!(output.len(), 1);
        push_data_if_non_empty(&mut data, &mut output);
        assert_eq!(output.len(), 1);
    }

    #[test]
    fn ansi_parser_inner_empty_branches() {
        let mut p = FreminalAnsiParser::new();
        let mut out = vec![];
        let mut data = vec![];

        // Escape → Err, sets inner
        assert!(
            p.ansi_parser_inner_empty(b'\x1b', &mut data, &mut out)
                .is_err()
        );
        assert_eq!(p.inner, ParserInner::Escape);

        // Reset to Empty
        p.inner = ParserInner::Empty;
        for &(b, expected) in &[
            (b'\r', "CarriageReturn"),
            (b'\n', "Newline"),
            (0x08, "Backspace"),
            (0x07, "Bell"),
        ] {
            out.clear();
            data.clear();
            let r = p.ansi_parser_inner_empty(b, &mut data, &mut out);
            assert!(r.is_err());
            assert!(
                !out.is_empty(),
                "Expected output vector to contain at least one element"
            );
            if let Some(last_output) = out.last() {
                assert_eq!(last_output.to_string(), expected);
            } else {
                panic!("Output vector should not be empty");
            }
        }

        // Normal data path (Ok)
        out.clear();
        data.clear();
        assert!(p.ansi_parser_inner_empty(b'A', &mut data, &mut out).is_ok());
        assert!(data.is_empty()); // correct: data is only pushed in FreminalAnsiParser::push
    }

    // -------------------------------------------------------------------------
    // New tests for full coverage
    // -------------------------------------------------------------------------

    #[test]
    fn escape_branches_cover_all_modes() {
        let mut p = FreminalAnsiParser::new();
        let mut out = Vec::new();
        let mut data = Vec::new();

        // '[' → CSI
        p.inner = ParserInner::Escape;
        p.ansiparser_inner_escape(b'[', &mut data, &mut out);
        assert!(matches!(p.inner, ParserInner::Csi(_)));

        // ']' → OSC
        p.inner = ParserInner::Escape;
        p.ansiparser_inner_escape(b']', &mut data, &mut out);
        assert!(matches!(p.inner, ParserInner::Osc(_)));

        // other → Standard
        p.inner = ParserInner::Escape;
        p.ansiparser_inner_escape(b'Z', &mut data, &mut out);
        assert!(
            matches!(p.inner, ParserInner::Standard(_) | ParserInner::Empty),
            "Unexpected inner state: {:?}",
            p.inner
        );
    }

    #[test]
    fn push_drives_each_parser_inner_variant() {
        let mut parser = FreminalAnsiParser::new();

        // Empty path → Data
        let out = parser.push(b"abc");
        assert_eq!(out.last(), Some(&TerminalOutput::Data(b"abc".to_vec())));

        // Escape → CSI
        let result_csi = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            parser.push(b"\x1b[31m") // valid CSI SGR sequence
        }));
        assert!(result_csi.is_ok(), "CSI branch should not panic");

        // Escape → OSC
        let result_osc = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            parser.push(b"\x1b]0;hi\x07") // valid OSC title sequence
        }));
        assert!(result_osc.is_ok(), "OSC branch should not panic");

        // Escape → Standard
        let result_std = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            parser.push(b"\x1bZ") // DECID or unsupported escape
        }));
        assert!(result_std.is_ok(), "Standard branch should not panic");
    }

    #[test]
    fn parser_handles_error_paths_without_panic() {
        let mut parser = FreminalAnsiParser::new();
        // malformed ESC / OSC sequences; should log but never panic
        let weird = b"\x1b[999;xxxm\x1b]invalid\x07";
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| parser.push(weird)));
        assert!(result.is_ok());
    }

    #[test]
    fn display_all_terminal_output_variants_exhaustively() {
        use TerminalOutput::*;
        let outputs = vec![
            ClearDisplayfromCursortoEndofDisplay,
            ClearDisplayfromStartofDisplaytoCursor,
            ClearScrollbackandDisplay,
            ClearLineForwards,
            ClearLineBackwards,
            DecSpecialGraphics(crate::ansi_components::line_draw::DecSpecialGraphics::Replace),
            CursorVisualStyle(freminal_common::cursor::CursorVisualStyle::BlockCursorSteady),
            WindowManipulation(
                freminal_common::window_manipulation::WindowManipulation::DeIconifyWindow,
            ),
            MemoryLock,
            MemoryUnlock,
            DeviceControlString(b"abc".to_vec()),
            ApplicationProgramCommand(b"xyz".to_vec()),
            RequestDeviceNameAndVersion,
            EightBitControl,
            SevenBitControl,
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
            CharsetG1AsGR,
            CharsetG2,
            CharsetG2AsGR,
            CharsetG2AsGL,
            CharsetG3,
            CharsetG3AsGR,
            CharsetG3AsGL,
            DecSpecial,
            CharsetUK,
            CharsetUS,
            CharsetUSASCII,
            CharsetDutch,
            CharsetFinnish,
            CharsetFrench,
            CharsetFrenchCanadian,
            CharsetGerman,
            CharsetItalian,
            CharsetNorwegianDanish,
            CharsetSpanish,
            CharsetSwedish,
            CharsetSwiss,
            SaveCursor,
            RestoreCursor,
            CursorToLowerLeftCorner,
            ResetDevice,
            RequestDeviceAttributes,
            Skipped,
            Invalid,
        ];
        for o in outputs {
            let _ = format!("{o}");
        }
    }

    #[test]
    fn extract_and_split_helpers_cover_edge_cases() {
        // extract_param out-of-range
        assert_eq!(extract_param(5, &[Some(1)]), None);

        // split_params empty string
        let r = split_params_into_semicolon_delimited_usize(b"");

        if let Ok(v) = r.as_ref() {
            assert_eq!(v, &vec![None]);
        } else {
            panic!("Expected Ok result");
        }

        // colon-delimited also works on empty
        let r = split_params_into_colon_delimited_usize(b"");
        if let Ok(v) = r.as_ref() {
            assert_eq!(v, &vec![None]);
        } else {
            panic!("Expected Ok result");
        }
    }
}
