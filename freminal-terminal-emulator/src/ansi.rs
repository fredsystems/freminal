// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use core::fmt;

use crate::{
    ansi_components::{
        apc::ApcParser, csi::AnsiCsiParser, dcs::DcsParser, osc::AnsiOscParser,
        standard::StandardParser, tracer::SequenceTraceable,
    },
    error::ParserFailures,
};

use crate::ansi_components::tracer::SequenceTracer;
use anyhow::Result;
use freminal_common::buffer_states::{
    line_draw::DecSpecialGraphics, mode::Mode, modes::decanm::Decanm,
    terminal_output::TerminalOutput,
};

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
    Dcs(DcsParser),
    Apc(ApcParser),
    /// VT52: waiting for the command byte after ESC.
    Vt52Escape,
    /// VT52: `ESC Y` cursor address — waiting for row and column bytes.
    /// First `Option` is the row byte (if received); second call fills column.
    Vt52CursorAddress(Option<u8>),
}

#[derive(Debug, Eq, PartialEq)]
pub struct FreminalAnsiParser {
    pub inner: ParserInner,
    // Accumulates plain text between control sequences across chunk boundaries,
    // reducing per-call allocations and enabling coalesced Data emissions.
    pending_data: Vec<u8>,
    seq_trace: SequenceTracer,
    /// When set to `Decanm::Vt52`, the parser uses the reduced VT52 escape set
    /// instead of standard ANSI/VT100+ sequences.  Toggled by DECANM (`?2`).
    pub vt52_mode: Decanm,
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
            vt52_mode: Decanm::Ansi,
        }
    }

    fn ansi_parser_inner_empty(
        &mut self,
        b: u8,
        data_output: &mut Vec<u8>,
        output: &mut Vec<TerminalOutput>,
    ) -> Result<(), ()> {
        if b == b'\x1b' {
            if self.vt52_mode == Decanm::Vt52 {
                self.inner = ParserInner::Vt52Escape;
            } else {
                self.inner = ParserInner::Escape;
            }
            return Err(());
        }

        if b == b'\r' {
            push_data_if_non_empty(data_output, output);
            output.push(TerminalOutput::CarriageReturn);
            return Err(());
        }

        // LF (0x0A), VT (0x0B), FF (0x0C) all produce a newline
        if b == b'\n' || b == 0x0B || b == 0x0C {
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

        if b == 0x09 {
            push_data_if_non_empty(data_output, output);
            output.push(TerminalOutput::Tab);
            return Err(());
        }

        // NUL (0x00) and DEL (0x7F) are silently ignored
        if b == 0x00 || b == 0x7F {
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
            b'P' => {
                self.inner = ParserInner::Dcs(DcsParser::new());
            }
            b'_' => {
                self.inner = ParserInner::Apc(ApcParser::new());
            }
            b'\x1b' => {
                // ESC followed by ESC is invalid; reset to Empty
                debug!("ANSI parser: ESC followed by ESC");
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
                        debug!(
                            "Invalid ANSI sequence: {}; recent={}",
                            message,
                            self.current_trace_str()
                        );
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
                            debug!(
                                "Invalid ANSI sequence: {}; recent={}",
                                message,
                                self.current_trace_str()
                            );
                            self.inner = ParserInner::Empty;
                        }
                    }
                }
                ParserInner::Dcs(parser) => match parser.dcs_parser_inner(b, &mut output) {
                    ParserOutcome::Finished => {
                        self.inner = ParserInner::Empty;
                    }
                    ParserOutcome::Continue => (),
                    ParserOutcome::Invalid(_) | ParserOutcome::InvalidParserFailure(_) => {
                        output.push(TerminalOutput::Invalid);
                        self.inner = ParserInner::Empty;
                    }
                },
                ParserInner::Apc(parser) => match parser.apc_parser_inner(b, &mut output) {
                    ParserOutcome::Finished => {
                        self.inner = ParserInner::Empty;
                    }
                    ParserOutcome::Continue => (),
                    ParserOutcome::Invalid(_) | ParserOutcome::InvalidParserFailure(_) => {
                        output.push(TerminalOutput::Invalid);
                        self.inner = ParserInner::Empty;
                    }
                },
                ParserInner::Csi(parser) => {
                    // ECMA-48 §5.5: C0 control characters received during a
                    // control sequence are executed immediately; the control
                    // sequence continues parsing afterwards.
                    // ESC (0x1B) is special: it aborts the current CSI and
                    // starts a new escape sequence.
                    if b == 0x1B {
                        self.inner = ParserInner::Escape;
                        continue;
                    }
                    if b < 0x20 {
                        // Execute C0 control inline; CSI parsing continues.
                        let _ = self.ansi_parser_inner_empty(b, &mut data_output, &mut output);
                        continue;
                    }

                    match parser.ansiparser_inner_csi(b, &mut output) {
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
                        ParserOutcome::InvalidParserFailure(message) => {
                            debug!(
                                "Invalid ANSI sequence: {}; recent={}",
                                message,
                                self.current_trace_str()
                            );
                            self.inner = ParserInner::Empty;
                            output.push(TerminalOutput::Invalid);
                        }
                        ParserOutcome::Invalid(message) => {
                            debug!(
                                "Invalid ANSI sequence: {}; recent={}",
                                message,
                                self.current_trace_str()
                            );
                            self.inner = ParserInner::Empty;
                            output.push(TerminalOutput::Invalid);
                        }
                    }
                }
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
                        debug!(
                            "Invalid ANSI sequence: {}; recent={}",
                            message,
                            self.current_trace_str()
                        );
                        self.inner = ParserInner::Empty;
                    }
                },
                ParserInner::Vt52Escape => {
                    push_data_if_non_empty(&mut data_output, &mut output);
                    self.handle_vt52_escape(b, &mut output);
                }
                ParserInner::Vt52CursorAddress(row_opt) => {
                    push_data_if_non_empty(&mut data_output, &mut output);
                    match row_opt {
                        None => {
                            // First parameter byte: row
                            *row_opt = Some(b);
                        }
                        Some(row_byte) => {
                            // Second parameter byte: col — emit the cursor position.
                            // Both are offset by 0x20; 1-indexed for SetCursorPos.
                            let row = usize::from(row_byte.saturating_sub(0x1F));
                            let col = usize::from(b.saturating_sub(0x1F));
                            output.push(TerminalOutput::SetCursorPos {
                                x: Some(col),
                                y: Some(row),
                            });
                            self.inner = ParserInner::Empty;
                        }
                    }
                }
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

    /// Handle a single byte after ESC in VT52 mode.
    ///
    /// VT52 escape sequences are all single-byte commands except `ESC Y` (cursor
    /// address) which takes two additional parameter bytes.
    fn handle_vt52_escape(&mut self, b: u8, output: &mut Vec<TerminalOutput>) {
        match b {
            // ESC A — Cursor up
            b'A' => {
                output.push(TerminalOutput::SetCursorPosRel {
                    x: None,
                    y: Some(-1),
                });
                self.inner = ParserInner::Empty;
            }
            // ESC B — Cursor down
            b'B' => {
                output.push(TerminalOutput::SetCursorPosRel {
                    x: None,
                    y: Some(1),
                });
                self.inner = ParserInner::Empty;
            }
            // ESC C — Cursor right (forward)
            b'C' => {
                output.push(TerminalOutput::SetCursorPosRel {
                    x: Some(1),
                    y: None,
                });
                self.inner = ParserInner::Empty;
            }
            // ESC D — Cursor left (backward)
            b'D' => {
                output.push(TerminalOutput::SetCursorPosRel {
                    x: Some(-1),
                    y: None,
                });
                self.inner = ParserInner::Empty;
            }
            // ESC F — Enter graphics mode (DEC special graphics)
            b'F' => {
                output.push(TerminalOutput::DecSpecialGraphics(
                    DecSpecialGraphics::Replace,
                ));
                self.inner = ParserInner::Empty;
            }
            // ESC G — Exit graphics mode
            b'G' => {
                output.push(TerminalOutput::DecSpecialGraphics(
                    DecSpecialGraphics::DontReplace,
                ));
                self.inner = ParserInner::Empty;
            }
            // ESC H — Cursor home (row 1, col 1)
            b'H' => {
                output.push(TerminalOutput::SetCursorPos {
                    x: Some(1),
                    y: Some(1),
                });
                self.inner = ParserInner::Empty;
            }
            // ESC I — Reverse line feed (reverse index)
            b'I' => {
                output.push(TerminalOutput::ReverseIndex);
                self.inner = ParserInner::Empty;
            }
            // ESC J — Erase to end of screen
            b'J' => {
                output.push(TerminalOutput::ClearDisplayfromCursortoEndofDisplay);
                self.inner = ParserInner::Empty;
            }
            // ESC K — Erase to end of line
            b'K' => {
                output.push(TerminalOutput::ClearLineForwards);
                self.inner = ParserInner::Empty;
            }
            // ESC Y — Direct cursor address (two parameter bytes follow)
            b'Y' => {
                self.inner = ParserInner::Vt52CursorAddress(None);
            }
            // ESC Z — Identify (handler responds with ESC / Z when vt52_mode)
            b'Z' => {
                output.push(TerminalOutput::RequestDeviceAttributes);
                self.inner = ParserInner::Empty;
            }
            // ESC = — Enter alternate keypad mode
            b'=' => {
                output.push(TerminalOutput::ApplicationKeypadMode);
                self.inner = ParserInner::Empty;
            }
            // ESC > — Exit alternate keypad mode
            b'>' => {
                output.push(TerminalOutput::NormalKeypadMode);
                self.inner = ParserInner::Empty;
            }
            // ESC < — Exit VT52 mode, return to ANSI mode
            b'<' => {
                output.push(TerminalOutput::Mode(Mode::Decanm(Decanm::Ansi)));
                self.inner = ParserInner::Empty;
            }
            _ => {
                debug!(
                    "Unknown VT52 escape: 0x{b:02x}; recent={}",
                    self.current_trace_str()
                );
                output.push(TerminalOutput::Invalid);
                self.inner = ParserInner::Empty;
            }
        }
    }

    /// Retrieve a snapshot of the currently active parser's trace buffer.
    #[must_use]
    pub fn current_trace_str(&self) -> String {
        match &self.inner {
            ParserInner::Osc(p) => p.trace_str(),
            ParserInner::Csi(p) => p.trace_str(),
            ParserInner::Standard(p) => p.trace_str(),
            ParserInner::Dcs(p) => p.trace_str(),
            ParserInner::Apc(p) => p.trace_str(),
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

        // 'P' → DCS
        p.inner = ParserInner::Escape;
        p.ansiparser_inner_escape(b'P', &mut data, &mut out);
        assert!(matches!(p.inner, ParserInner::Dcs(_)));

        // '_' → APC
        p.inner = ParserInner::Escape;
        p.ansiparser_inner_escape(b'_', &mut data, &mut out);
        assert!(matches!(p.inner, ParserInner::Apc(_)));

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
            DecSpecialGraphics(
                freminal_common::buffer_states::line_draw::DecSpecialGraphics::Replace,
            ),
            CursorVisualStyle(freminal_common::cursor::CursorVisualStyle::BlockCursorSteady),
            WindowManipulation(
                freminal_common::buffer_states::window_manipulation::WindowManipulation::DeIconifyWindow,
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

    // -------------------------------------------------------------------------
    // Subtask 7.4: VT (0x0B) and FF (0x0C) treated as LF
    // -------------------------------------------------------------------------

    #[test]
    fn vt_produces_newline() {
        let mut p = FreminalAnsiParser::new();
        let mut out = Vec::new();
        let mut data = Vec::new();
        let r = p.ansi_parser_inner_empty(0x0B, &mut data, &mut out);
        assert!(r.is_err());
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].to_string(), "Newline");
    }

    #[test]
    fn ff_produces_newline() {
        let mut p = FreminalAnsiParser::new();
        let mut out = Vec::new();
        let mut data = Vec::new();
        let r = p.ansi_parser_inner_empty(0x0C, &mut data, &mut out);
        assert!(r.is_err());
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].to_string(), "Newline");
    }

    #[test]
    fn vt_and_ff_via_push() {
        let mut parser = FreminalAnsiParser::new();
        // Push data containing VT and FF interleaved with text
        let result = parser.push(b"A\x0BB\x0CC");
        // Should see: Data("A"), Newline, Data("B"), Newline, Data("C")
        let newline_count = result
            .iter()
            .filter(|o| matches!(o, TerminalOutput::Newline))
            .count();
        assert_eq!(newline_count, 2, "VT and FF should each produce a Newline");
    }

    // -------------------------------------------------------------------------
    // Subtask 7.5: NUL (0x00) and DEL (0x7F) silently ignored
    // -------------------------------------------------------------------------

    #[test]
    fn nul_silently_ignored() {
        let mut p = FreminalAnsiParser::new();
        let mut out = Vec::new();
        let mut data = Vec::new();
        let r = p.ansi_parser_inner_empty(0x00, &mut data, &mut out);
        assert!(r.is_err());
        assert!(out.is_empty(), "NUL should produce no output");
    }

    #[test]
    fn del_silently_ignored() {
        let mut p = FreminalAnsiParser::new();
        let mut out = Vec::new();
        let mut data = Vec::new();
        let r = p.ansi_parser_inner_empty(0x7F, &mut data, &mut out);
        assert!(r.is_err());
        assert!(out.is_empty(), "DEL should produce no output");
    }

    #[test]
    fn nul_and_del_stripped_from_push_output() {
        let mut parser = FreminalAnsiParser::new();
        let result = parser.push(b"\x00Hello\x7F");
        // NUL and DEL should not appear in output; we should just get Data("Hello")
        let combined: Vec<u8> = result
            .iter()
            .filter_map(|o| {
                if let TerminalOutput::Data(d) = o {
                    Some(d.clone())
                } else {
                    None
                }
            })
            .flatten()
            .collect();
        assert_eq!(combined, b"Hello");
    }

    // -------------------------------------------------------------------------
    // ECMA-48 §5.5: C0 controls inside CSI sequences
    // -------------------------------------------------------------------------

    #[test]
    fn c0_bs_inside_csi() {
        // ESC[2 BS C — BS is executed, then CSI 2C (CursorForward 2) completes
        let mut parser = FreminalAnsiParser::new();
        let result = parser.push(b"\x1b[2\x08C");
        let has_backspace = result
            .iter()
            .any(|o| matches!(o, TerminalOutput::Backspace));
        let has_cursor_forward = result.iter().any(|o| {
            matches!(
                o,
                TerminalOutput::SetCursorPosRel {
                    x: Some(2),
                    y: None
                }
            )
        });
        assert!(has_backspace, "BS should be executed inline: {result:?}");
        assert!(
            has_cursor_forward,
            "CSI 2C should complete after BS: {result:?}"
        );
    }

    #[test]
    fn c0_cr_inside_csi() {
        // ESC[ CR 2C — CR is executed, then CSI 2C completes
        let mut parser = FreminalAnsiParser::new();
        let result = parser.push(b"\x1b[\x0d2C");
        let has_cr = result
            .iter()
            .any(|o| matches!(o, TerminalOutput::CarriageReturn));
        let has_cursor_forward = result.iter().any(|o| {
            matches!(
                o,
                TerminalOutput::SetCursorPosRel {
                    x: Some(2),
                    y: None
                }
            )
        });
        assert!(has_cr, "CR should be executed inline: {result:?}");
        assert!(
            has_cursor_forward,
            "CSI 2C should complete after CR: {result:?}"
        );
    }

    #[test]
    fn c0_vt_inside_csi() {
        // ESC[1 VT A — VT (0x0B) is executed as Newline, then CSI 1A (CursorUp 1) completes
        let mut parser = FreminalAnsiParser::new();
        let result = parser.push(b"\x1b[1\x0bA");
        let has_newline = result.iter().any(|o| matches!(o, TerminalOutput::Newline));
        let has_cursor_up = result.iter().any(|o| {
            matches!(
                o,
                TerminalOutput::SetCursorPosRel {
                    x: None,
                    y: Some(-1)
                }
            )
        });
        assert!(has_newline, "VT should be executed as Newline: {result:?}");
        assert!(has_cursor_up, "CSI 1A should complete after VT: {result:?}");
    }

    #[test]
    fn c0_nul_inside_csi() {
        // ESC[1 NUL A — NUL is silently ignored, CSI 1A (CursorUp 1) completes
        let mut parser = FreminalAnsiParser::new();
        let result = parser.push(b"\x1b[1\x00A");
        let has_cursor_up = result.iter().any(|o| {
            matches!(
                o,
                TerminalOutput::SetCursorPosRel {
                    x: None,
                    y: Some(-1)
                }
            )
        });
        assert!(
            has_cursor_up,
            "CSI 1A should complete despite NUL: {result:?}"
        );
        // NUL should produce no extra output
        let non_cursor: Vec<_> = result
            .iter()
            .filter(|o| !matches!(o, TerminalOutput::SetCursorPosRel { .. }))
            .collect();
        assert!(
            non_cursor.is_empty(),
            "NUL should not produce output: {non_cursor:?}"
        );
    }

    #[test]
    fn c0_esc_inside_csi_aborts() {
        // ESC[2 ESC [1A — ESC aborts the first CSI, starts new CSI 1A (CursorUp 1)
        let mut parser = FreminalAnsiParser::new();
        let result = parser.push(b"\x1b[2\x1b[1A");
        let has_cursor_up = result.iter().any(|o| {
            matches!(
                o,
                TerminalOutput::SetCursorPosRel {
                    x: None,
                    y: Some(-1)
                }
            )
        });
        assert!(
            has_cursor_up,
            "New CSI 1A should complete after ESC abort: {result:?}"
        );
        // The original CSI 2 was aborted — should not produce CursorForward
        let has_cursor_forward = result.iter().any(|o| {
            matches!(
                o,
                TerminalOutput::SetCursorPosRel {
                    x: Some(_),
                    y: None
                }
            )
        });
        assert!(
            !has_cursor_forward,
            "Aborted CSI should not produce CursorForward: {result:?}"
        );
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

    // -------------------------------------------------------------------------
    // VT52 mode parser tests (subtask 20.8)
    // -------------------------------------------------------------------------

    /// Helper: create a parser already in VT52 mode.
    fn vt52_parser() -> FreminalAnsiParser {
        let mut p = FreminalAnsiParser::new();
        p.vt52_mode = Decanm::Vt52;
        p
    }

    #[test]
    fn vt52_esc_routes_to_vt52_escape_state() {
        let mut p = vt52_parser();
        let mut out = Vec::new();
        let mut data = Vec::new();
        let r = p.ansi_parser_inner_empty(b'\x1b', &mut data, &mut out);
        assert!(r.is_err());
        assert_eq!(p.inner, ParserInner::Vt52Escape);
    }

    #[test]
    fn vt52_cursor_up() {
        let mut p = vt52_parser();
        let result = p.push(b"\x1bA");
        assert_eq!(
            result,
            vec![TerminalOutput::SetCursorPosRel {
                x: None,
                y: Some(-1)
            }]
        );
    }

    #[test]
    fn vt52_cursor_down() {
        let mut p = vt52_parser();
        let result = p.push(b"\x1bB");
        assert_eq!(
            result,
            vec![TerminalOutput::SetCursorPosRel {
                x: None,
                y: Some(1)
            }]
        );
    }

    #[test]
    fn vt52_cursor_right() {
        let mut p = vt52_parser();
        let result = p.push(b"\x1bC");
        assert_eq!(
            result,
            vec![TerminalOutput::SetCursorPosRel {
                x: Some(1),
                y: None
            }]
        );
    }

    #[test]
    fn vt52_cursor_left() {
        let mut p = vt52_parser();
        let result = p.push(b"\x1bD");
        assert_eq!(
            result,
            vec![TerminalOutput::SetCursorPosRel {
                x: Some(-1),
                y: None
            }]
        );
    }

    #[test]
    fn vt52_enter_graphics_mode() {
        let mut p = vt52_parser();
        let result = p.push(b"\x1bF");
        assert_eq!(
            result,
            vec![TerminalOutput::DecSpecialGraphics(
                freminal_common::buffer_states::line_draw::DecSpecialGraphics::Replace
            )]
        );
    }

    #[test]
    fn vt52_exit_graphics_mode() {
        let mut p = vt52_parser();
        let result = p.push(b"\x1bG");
        assert_eq!(
            result,
            vec![TerminalOutput::DecSpecialGraphics(
                freminal_common::buffer_states::line_draw::DecSpecialGraphics::DontReplace
            )]
        );
    }

    #[test]
    fn vt52_cursor_home() {
        let mut p = vt52_parser();
        let result = p.push(b"\x1bH");
        assert_eq!(
            result,
            vec![TerminalOutput::SetCursorPos {
                x: Some(1),
                y: Some(1)
            }]
        );
    }

    #[test]
    fn vt52_reverse_line_feed() {
        let mut p = vt52_parser();
        let result = p.push(b"\x1bI");
        assert_eq!(result, vec![TerminalOutput::ReverseIndex]);
    }

    #[test]
    fn vt52_erase_to_end_of_screen() {
        let mut p = vt52_parser();
        let result = p.push(b"\x1bJ");
        assert_eq!(
            result,
            vec![TerminalOutput::ClearDisplayfromCursortoEndofDisplay]
        );
    }

    #[test]
    fn vt52_erase_to_end_of_line() {
        let mut p = vt52_parser();
        let result = p.push(b"\x1bK");
        assert_eq!(result, vec![TerminalOutput::ClearLineForwards]);
    }

    #[test]
    fn vt52_identify() {
        let mut p = vt52_parser();
        let result = p.push(b"\x1bZ");
        assert_eq!(result, vec![TerminalOutput::RequestDeviceAttributes]);
    }

    #[test]
    fn vt52_enter_alternate_keypad() {
        let mut p = vt52_parser();
        let result = p.push(b"\x1b=");
        assert_eq!(result, vec![TerminalOutput::ApplicationKeypadMode]);
    }

    #[test]
    fn vt52_exit_alternate_keypad() {
        let mut p = vt52_parser();
        let result = p.push(b"\x1b>");
        assert_eq!(result, vec![TerminalOutput::NormalKeypadMode]);
    }

    #[test]
    fn vt52_exit_vt52_mode() {
        let mut p = vt52_parser();
        let result = p.push(b"\x1b<");
        assert_eq!(
            result,
            vec![TerminalOutput::Mode(Mode::Decanm(Decanm::Ansi))]
        );
    }

    #[test]
    fn vt52_direct_cursor_address() {
        let mut p = vt52_parser();
        // ESC Y <row+0x20> <col+0x20>: row=5, col=10 → 0x20+5-1=0x24, 0x20+10-1=0x29
        // SetCursorPos uses 1-indexed: row = (0x24 - 0x1F) = 5, col = (0x29 - 0x1F) = 10
        let result = p.push(b"\x1bY\x24\x29");
        assert_eq!(
            result,
            vec![TerminalOutput::SetCursorPos {
                x: Some(10),
                y: Some(5)
            }]
        );
    }

    #[test]
    fn vt52_cursor_address_row1_col1() {
        let mut p = vt52_parser();
        // Row 1, Col 1: both = 0x20 → (0x20 - 0x1F) = 1
        let result = p.push(b"\x1bY\x20\x20");
        assert_eq!(
            result,
            vec![TerminalOutput::SetCursorPos {
                x: Some(1),
                y: Some(1)
            }]
        );
    }

    #[test]
    fn vt52_cursor_address_split_across_chunks() {
        let mut p = vt52_parser();
        // First chunk: ESC Y <row>
        let result1 = p.push(b"\x1bY\x24");
        assert!(result1.is_empty(), "Should have no output yet: {result1:?}");
        assert!(matches!(
            p.inner,
            ParserInner::Vt52CursorAddress(Some(0x24))
        ));
        // Second chunk: <col>
        let result2 = p.push(b"\x29");
        assert_eq!(
            result2,
            vec![TerminalOutput::SetCursorPos {
                x: Some(10),
                y: Some(5)
            }]
        );
    }

    #[test]
    fn vt52_unknown_escape_produces_invalid() {
        let mut p = vt52_parser();
        let result = p.push(b"\x1bX");
        assert_eq!(result, vec![TerminalOutput::Invalid]);
    }

    #[test]
    fn vt52_data_between_escapes() {
        let mut p = vt52_parser();
        let result = p.push(b"Hello\x1bAWorld");
        // Expect: Data("Hello"), CursorUp, Data("World")
        assert_eq!(result.len(), 3);
        assert_eq!(result[0], TerminalOutput::Data(b"Hello".to_vec()));
        assert_eq!(
            result[1],
            TerminalOutput::SetCursorPosRel {
                x: None,
                y: Some(-1)
            }
        );
        assert_eq!(result[2], TerminalOutput::Data(b"World".to_vec()));
    }

    #[test]
    fn vt52_ansi_roundtrip() {
        // Start in ANSI mode, switch to VT52 via CSI, then back via ESC <
        let mut p = FreminalAnsiParser::new();
        assert!(p.vt52_mode == Decanm::Ansi);

        // Enter VT52 mode via the mode change (parser flag set externally,
        // as sync_mode would do after processing Mode::Decanm(Decanm::Vt52))
        p.vt52_mode = Decanm::Vt52;
        let result = p.push(b"\x1bA");
        assert_eq!(
            result,
            vec![TerminalOutput::SetCursorPosRel {
                x: None,
                y: Some(-1)
            }]
        );

        // ESC < should emit Mode::Decanm(Decanm::Ansi)
        let result = p.push(b"\x1b<");
        assert_eq!(
            result,
            vec![TerminalOutput::Mode(Mode::Decanm(Decanm::Ansi))]
        );

        // After sync_mode processes it, parser.vt52_mode would be set to false.
        // Simulate that:
        p.vt52_mode = Decanm::Ansi;

        // Now ANSI escape sequences should work again
        let result = p.push(b"\x1b[1A"); // CSI 1A = cursor up 1
        let has_cursor_up = result.iter().any(|o| {
            matches!(
                o,
                TerminalOutput::SetCursorPosRel {
                    x: None,
                    y: Some(-1)
                }
            )
        });
        assert!(
            has_cursor_up,
            "ANSI CSI should work after ESC <: {result:?}"
        );
    }

    #[test]
    fn vt52_c0_controls_still_work() {
        // C0 controls (CR, LF, BS, BEL, TAB) should work in VT52 mode
        let mut p = vt52_parser();
        let result = p.push(b"\r\n\x08\x07\x09");
        assert_eq!(
            result,
            vec![
                TerminalOutput::CarriageReturn,
                TerminalOutput::Newline,
                TerminalOutput::Backspace,
                TerminalOutput::Bell,
                TerminalOutput::Tab,
            ]
        );
    }
}
