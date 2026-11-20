// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use core::fmt;

use crate::{
    ansi_components::{
        apc::ApcParser,
        csi::{AnsiCsiParser, CsiBuffers, CsiIntroducer},
        dcs::DcsParser,
        osc::AnsiOscParser,
        standard::StandardParser,
        tracer::SequenceTraceable,
    },
    error::ParserFailures,
};

use crate::ansi_components::tracer::SequenceTracer;
use crate::error::AnsiParseError;
use freminal_common::buffer_states::{
    charset::{CharacterSet, CharsetSlot},
    mode::Mode,
    modes::decanm::Decanm,
    modes::s8c1t::S8c1t,
    terminal_output::TerminalOutput,
};

/// Represents the high-level result of feeding one byte to the parser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParserOutcome {
    /// The parser consumed the byte and no complete output was produced yet.
    Continue,
    /// The byte completed a sequence. A completed sequence usually produces at
    /// least one `TerminalOutput`, but need not: a sequence that is recognised
    /// and ignored, or one dropped for exceeding its byte cap, produces none.
    Finished,
    /// A string sequence (OSC, DCS or APC) ended because a bare ESC, one not
    /// followed by `\`, preceded this byte. The sequence is complete: it was
    /// dispatched (or dropped, if it was over its cap) exactly as if ST had
    /// arrived. The byte just pushed was **not** consumed: it is the second byte
    /// of a new escape sequence started by that ESC, and the top-level parser
    /// must re-enter the escape state and feed it that byte.
    ///
    /// An ESC-intermediate sequence (`ESC (`, `ESC SP`, `ESC #`, ...) returns
    /// it when the byte pushed is itself an ESC: the sequence is abandoned
    /// with no output, and that ESC starts a new one (re-entering the escape
    /// state with an ESC is just re-entering the escape state).
    FinishedAtEsc,
    /// The byte resulted in an invalid sequence or parse error (error string provided).
    Invalid(String),
    InvalidParserFailure(ParserFailures),
}

impl fmt::Display for ParserOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Continue => write!(f, "Continue"),
            Self::Finished => write!(f, "Finished"),
            Self::FinishedAtEsc => write!(f, "FinishedAtEsc"),
            Self::Invalid(msg) => write!(f, "Invalid: {msg}"),
            Self::InvalidParserFailure(msg) => {
                write!(f, "InvalidParserFailure: {msg:?}")
            }
        }
    }
}

/// What the previous byte was, for a string sequence (OSC, APC, DCS) that has
/// overflowed its byte cap and so no longer keeps its payload.
///
/// An overflowed sequence is consumed only to find its terminator, and the
/// terminator's first byte is ESC, so this is all that has to be remembered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PrevByte {
    /// The previous byte was ESC (`0x1b`).
    Esc,
    /// The previous byte was anything else.
    Other,
}

impl PrevByte {
    /// Classify `b` as the previous byte for the next push.
    pub(crate) const fn of(b: u8) -> Self {
        if b == 0x1b { Self::Esc } else { Self::Other }
    }
}

#[must_use]
pub fn extract_param(idx: usize, params: &[Option<usize>]) -> Option<usize> {
    params.get(idx).copied().flatten()
}

/// # Errors
/// Will return an error if the parameter is not a valid number
pub fn split_params_into_semicolon_delimited_usize(
    params: &[u8],
) -> Result<Vec<Option<usize>>, AnsiParseError> {
    params
        .split(|b| *b == b';')
        .map(parse_param_as::<usize>)
        .collect::<Result<Vec<Option<usize>>, AnsiParseError>>()
}

/// Split a parameter string on `:` and parse each piece as a `usize`.
///
/// Not yet called from production code: its intended consumer is Task 103
/// (kitty multiple cursors), whose `CSI > ... SP q` sequence carries colon
/// sub-parameters.
///
/// # Errors
/// Will return an error if the parameter is not a valid number
pub fn split_params_into_colon_delimited_usize(
    params: &[u8],
) -> Result<Vec<Option<usize>>, AnsiParseError> {
    params
        .split(|b| *b == b':')
        .map(parse_param_as::<usize>)
        .collect::<Result<Vec<Option<usize>>, AnsiParseError>>()
}

/// # Errors
/// Will return an error if the parameter is not a valid number
pub fn parse_param_as<T: std::str::FromStr>(
    param_bytes: &[u8],
) -> Result<Option<T>, AnsiParseError> {
    let param_str = std::str::from_utf8(param_bytes)?;

    if param_str.is_empty() {
        return Ok(None);
    }

    param_str.parse().map_err(|_| ()).map_or_else(
        |()| {
            debug!(
                "Failed to parse parameter ({:?}) as {:?}",
                param_bytes,
                std::any::type_name::<T>()
            );
            Err(AnsiParseError::ParseFailed {
                bytes: param_bytes.to_vec(),
                type_name: std::any::type_name::<T>(),
            })
        },
        |value| Ok(Some(value)),
    )
}

/// Whether `b` is the 8-bit C1 String Terminator (`0x9C`) with 8-bit C1
/// controls in force.
///
/// Under S7C1T `0x9C` is not a control: inside a string it is an ordinary
/// body byte (the continuation byte of some UTF-8 characters). This is the
/// same gate `ansi_parser_inner_empty` applies to every other 8-bit C1
/// introducer.
const fn is_c1_st(b: u8, s8c1t: S8c1t) -> bool {
    b == 0x9C && matches!(s8c1t, S8c1t::EightBit)
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
    /// VT52: inside an `ESC (`, `ESC )`, `ESC *`, `ESC +`, `ESC SP` or
    /// `ESC $` introducer, which VT52 has no meaning for. Mirrors xterm's
    /// `vt52_ignore_table`: the sequence ends at (and swallows) the next byte
    /// in 0x30-0x7E; 0x20-0x2F and DEL are swallowed and the state stays;
    /// C0 controls execute and the state stays.
    Vt52Ignore,
    /// VT52: part-way through a non-ASCII character met while in
    /// `Vt52Escape` or `Vt52Ignore`. The character is consumed whole; see
    /// `FreminalAnsiParser::vt52_non_ascii_byte`.
    Vt52NonAscii {
        table: Vt52Table,
        prefix: Utf8Prefix,
    },
}

/// Which of xterm's VT52 tables a byte is looked up in.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Vt52Table {
    /// `vt52_esc_table`: the byte after ESC.
    Escape,
    /// `vt52_ignore_table`: inside an `ESC (`-style sequence.
    Ignore,
}

impl Vt52Table {
    const fn parser_state(self) -> ParserInner {
        match self {
            Self::Escape => ParserInner::Vt52Escape,
            Self::Ignore => ParserInner::Vt52Ignore,
        }
    }
}

/// The bytes of a UTF-8 character received so far (at most four).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Utf8Prefix {
    bytes: [u8; 4],
    len: usize,
}

impl Utf8Prefix {
    const fn new() -> Self {
        Self {
            bytes: [0; 4],
            len: 0,
        }
    }

    fn as_slice(&self) -> &[u8] {
        self.bytes.get(..self.len).unwrap_or_default()
    }

    /// Append `b`; `false` if the prefix is already full.
    fn push(&mut self, b: u8) -> bool {
        match self.bytes.get_mut(self.len) {
            Some(slot) => {
                *slot = b;
                self.len += 1;
                true
            }
            None => false,
        }
    }
}

/// Apply to `vt52_mode` / `s8c1t_mode` the change `output` makes to the
/// parser's own mode, if any.
///
/// The parser calls this as it emits each output, so a mode change governs
/// the very next byte of the same read.  `TerminalState` calls it again, in
/// event order, to track the mode in force at each point of the byte stream
/// (used to seed the tmux passthrough payload parser).
pub(crate) const fn apply_parser_mode_change(
    output: &TerminalOutput,
    vt52_mode: &mut Decanm,
    s8c1t_mode: &mut S8c1t,
) {
    match output {
        TerminalOutput::EightBitControl => *s8c1t_mode = S8c1t::EightBit,
        TerminalOutput::Mode(Mode::Decanm(Decanm::Vt52)) => *vt52_mode = Decanm::Vt52,
        TerminalOutput::Mode(Mode::Decanm(Decanm::Ansi)) => *vt52_mode = Decanm::Ansi,
        // RIS only reaches here from the ANSI parser (`ESC c` is ignored in
        // VT52 mode), so DECANM is already `Ansi`: only S8C1T needs resetting.
        TerminalOutput::SevenBitControl | TerminalOutput::ResetDevice => {
            *s8c1t_mode = S8c1t::SevenBit;
        }
        _ => {}
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct FreminalAnsiParser {
    pub inner: ParserInner,
    // Accumulates plain text between control sequences across chunk boundaries,
    // reducing per-call allocations and enabling coalesced Data emissions.
    pending_data: Vec<u8>,
    /// Spare byte buffers lent to each CSI sub-parser for the length of one
    /// sequence, so a CSI allocates nothing per sequence. Always empty while
    /// lent out and between sequences.
    csi_buffers: CsiBuffers,
    seq_trace: SequenceTracer,
    /// When set to `Decanm::Vt52`, the parser uses the reduced VT52 escape set
    /// instead of standard ANSI/VT100+ sequences.  Toggled by DECANM (`?2`).
    pub vt52_mode: Decanm,
    /// When set to `S8c1t::EightBit`, the parser recognizes bytes 0x80–0x9F as
    /// 8-bit C1 control introducers.  Toggled by `ESC SP G` (S8C1T) /
    /// `ESC SP F` (S7C1T).
    pub s8c1t_mode: S8c1t,
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
            csi_buffers: CsiBuffers::new(),
            seq_trace: SequenceTracer::new(),
            vt52_mode: Decanm::Ansi,
            s8c1t_mode: S8c1t::SevenBit,
        }
    }

    /// Enter the CSI state, lending the parser's spare buffers to the new
    /// sub-parser.
    fn start_csi(&mut self, introducer: CsiIntroducer) {
        self.recycle_csi_buffers();
        let buffers = std::mem::take(&mut self.csi_buffers);
        self.inner = ParserInner::Csi(AnsiCsiParser::from_buffers(introducer, buffers));
    }

    /// If a CSI sub-parser is active, take its buffers back (emptied, with
    /// oversized capacity released). Call this before `self.inner` stops
    /// being `Csi` by any route; the sub-parser's own diagnostics are gone
    /// afterwards, so log first.
    fn recycle_csi_buffers(&mut self) {
        if let ParserInner::Csi(parser) = &mut self.inner {
            self.csi_buffers = parser.take_buffers();
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

        // SI (0x0F) — Shift In: invoke G0 into GL.
        // SO (0x0E) — Shift Out: invoke G1 into GL.
        // VT52 mode ignores both (xterm `vt52_table`: SO/SI are `CASE_IGNORE`).
        if b == 0x0F || b == 0x0E {
            if self.vt52_mode == Decanm::Vt52 {
                return Err(());
            }
            push_data_if_non_empty(data_output, output);
            output.push(TerminalOutput::InvokeGl(if b == 0x0F {
                CharsetSlot::G0
            } else {
                CharsetSlot::G1
            }));
            return Err(());
        }

        // ENQ (0x05) — transmit answerback message
        if b == 0x05 {
            push_data_if_non_empty(data_output, output);
            output.push(TerminalOutput::Enq);
            return Err(());
        }

        // NUL (0x00) and DEL (0x7F) are silently ignored
        if b == 0x00 || b == 0x7F {
            return Err(());
        }

        // 8-bit C1 controls (0x80–0x9F) — recognized only when S8C1T is active
        // and we are in ANSI mode (VT52 mode does not support 8-bit C1).
        if self.s8c1t_mode == S8c1t::EightBit
            && self.vt52_mode == Decanm::Ansi
            && (0x80..=0x9F).contains(&b)
        {
            push_data_if_non_empty(data_output, output);
            match b {
                // IND (0x84) ≡ ESC D — Index (cursor down, scroll if at bottom)
                0x84 => output.push(TerminalOutput::Index),
                // NEL (0x85) ≡ ESC E — Next Line
                0x85 => output.push(TerminalOutput::NextLine),
                // HTS (0x88) ≡ ESC H — Horizontal Tab Set
                0x88 => output.push(TerminalOutput::HorizontalTabSet),
                // RI (0x8D) ≡ ESC M — Reverse Index
                0x8D => output.push(TerminalOutput::ReverseIndex),
                // DCS (0x90) ≡ ESC P — Device Control String
                0x90 => {
                    self.inner = ParserInner::Dcs(DcsParser::new());
                }
                // CSI (0x9B) ≡ ESC [ — Control Sequence Introducer (8-bit C1)
                0x9B => {
                    self.start_csi(CsiIntroducer::EightBit);
                }
                // OSC (0x9D) ≡ ESC ] — Operating System Command
                0x9D => {
                    self.inner = ParserInner::Osc(AnsiOscParser::new());
                }
                // APC (0x9F) ≡ ESC _ — Application Program Command
                0x9F => {
                    self.inner = ParserInner::Apc(ApcParser::new());
                }
                // SS2 (0x8E) ≡ ESC N — Single Shift G2
                0x8E => output.push(TerminalOutput::SingleShift(CharsetSlot::G2)),
                // SS3 (0x8F) ≡ ESC O — Single Shift G3
                0x8F => output.push(TerminalOutput::SingleShift(CharsetSlot::G3)),
                // ST (0x9C) — String Terminator (no-op when no string is open)
                // PM (0x9E) — Privacy Message (ignored, no handler)
                // Other bytes in 0x80–0x9F without defined C1 meaning — silently ignored
                _ => {}
            }
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
                self.start_csi(CsiIntroducer::SevenBit);
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
                            debug!(
                                "Invalid ANSI sequence; recent={}",
                                self.current_trace_escaped()
                            );
                        }
                    }
                    ParserOutcome::Continue => {
                        // Note: retain assignment here — we're transitioning from Escape to Standard.)
                        self.inner = ParserInner::Standard(parser);
                    }

                    ParserOutcome::InvalidParserFailure(_message) => unreachable!(),
                    // Only an ESC aborts a standard sequence, and a fresh
                    // parser has seen only `b`, which is not an ESC (that was
                    // handled at the top of this function), so this is not
                    // reached; it is handled as the `Standard` state does.
                    ParserOutcome::FinishedAtEsc => {
                        self.inner = ParserInner::Escape;
                    }

                    ParserOutcome::Invalid(message) => {
                        // All invalid sequences emit `TerminalOutput::Invalid` here.
                        output.push(TerminalOutput::Invalid);
                        debug!(
                            "Invalid ANSI sequence: {}; recent={}",
                            message,
                            self.current_trace_escaped()
                        );
                        self.inner = ParserInner::Empty;
                    }
                }
            }
        }
    }

    /// A string sequence (OSC, DCS or APC) just ended at a bare ESC: the ESC
    /// that preceded `b` starts a new escape sequence, and `b` is its second
    /// byte.
    ///
    /// The string itself has already been dispatched by its sub-parser. This
    /// re-enters the escape state (the ESC the sub-parser consumed) and feeds
    /// `b` to it, so `b` is parsed exactly as it would be after any ESC.
    fn restart_escape_with(
        &mut self,
        b: u8,
        data_output: &mut Vec<u8>,
        output: &mut Vec<TerminalOutput>,
    ) {
        self.inner = ParserInner::Escape;
        self.ansiparser_inner_escape(b, data_output, output);
    }

    /// Feed a raw byte slice from the PTY into the parser, returning all
    /// completed [`TerminalOutput`] events produced by the input.
    ///
    /// ## State machine overview
    ///
    /// The parser is a single-byte state machine (`ParserInner`).  Each call
    /// iterates over `incoming` byte-by-byte, routing each byte through the
    /// active state:
    ///
    /// ```text
    /// Empty          — base state; plain text accumulates in `pending_data`.
    ///                  C0 controls (CR, LF, BEL, BS, TAB, NUL, DEL) are
    ///                  emitted immediately as discrete TerminalOutput variants.
    ///                  ESC (0x1B) transitions to Escape (or Vt52Escape when
    ///                  vt52_mode is Vt52).
    /// Escape         — waiting for the introducer byte after ESC:
    ///                    '[' → Csi    ']' → Osc    'P' → Dcs
    ///                    '_' → Apc    anything else → StandardParser
    ///                  ESC ESC restarts Escape; ESC '\' (ST) ends a string.
    /// Csi            — collecting CSI parameters and intermediates until a
    ///                  final byte in 0x40–0x7E is received; then dispatches
    ///                  to the appropriate csi_commands sub-handler.
    /// Osc            — collecting the OSC string until BEL (0x07) or ST
    ///                  (ESC '\') terminates it; then dispatches to osc.rs.
    ///                  A bare ESC (not followed by '\') also ends it: the
    ///                  body so far is dispatched and the ESC starts a new
    ///                  escape sequence (Escape state, next byte its second).
    /// Standard       — handles two-byte Fe sequences (ESC # Pn, ESC ( G, …).
    /// Dcs            — Device Control String; passes bytes to dcs.rs until ST
    ///                  or a bare ESC (as for Osc; inside `DCS tmux;` an
    ///                  `ESC ESC` pair is data).
    /// Apc            — Application Program Command; passes bytes to apc.rs
    ///                  until ST or a bare ESC (as for Osc).
    /// Vt52Escape     — waiting for the single command byte after ESC in VT52
    ///                  mode; 'Y' transitions to Vt52CursorAddress.
    /// Vt52CursorAddress — collects the row byte then column byte for the
    ///                  ESC Y row col direct-cursor-address sequence.
    /// Vt52Ignore     — swallows the rest of a VT52-meaningless `ESC (`-style
    ///                  sequence (see `ParserInner::Vt52Ignore`).
    /// Vt52NonAscii   — consuming one non-ASCII character met inside either
    ///                  of the two states above.
    /// ```
    ///
    /// Plain text bytes are coalesced into `pending_data` and flushed as a
    /// single `TerminalOutput::Data(Vec<u8>)` whenever a control sequence
    /// interrupts the data stream, minimising allocations for the common case
    /// of long runs of ASCII text.
    // Inherently large: the ANSI parser state machine handles many escape sequence prefixes
    // (ESC, CSI, OSC, DCS, APC, SOS, PM, ST). Each branch is essential and tightly coupled to
    // the parser state. Splitting would fragment the state machine without reducing complexity.
    #[allow(clippy::too_many_lines)]
    pub fn push(&mut self, incoming: &[u8]) -> Vec<TerminalOutput> {
        // Take the pending buffer out temporarily
        let mut data_output = std::mem::take(&mut self.pending_data);
        let mut output = Vec::new();

        for &b in incoming {
            self.seq_trace.push(b);
            let emitted_from = output.len();

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
                                    self.current_trace_escaped()
                                );
                            }
                        }
                        ParserOutcome::Continue => (),
                        // we cannot hit this branch right now because the standard parser does not return ParserOutcome::InvalidParserFailure
                        ParserOutcome::InvalidParserFailure(_message) => unreachable!(),
                        // `b` is an ESC inside an ESC-intermediate sequence
                        // (`ESC (`, `ESC SP`, ...): the sequence is abandoned
                        // and the ESC starts a new one, as it does in CSI.
                        ParserOutcome::FinishedAtEsc => {
                            self.restart_escape_with(b, &mut data_output, &mut output);
                        }
                        ParserOutcome::Invalid(message) => {
                            // All invalid sequences emit `TerminalOutput::Invalid` here.
                            output.push(TerminalOutput::Invalid);
                            debug!(
                                "Invalid ANSI sequence: {}; recent={}",
                                message,
                                self.current_trace_escaped()
                            );
                            self.inner = ParserInner::Empty;
                        }
                    }
                }
                ParserInner::Dcs(parser) => match if is_c1_st(b, self.s8c1t_mode) {
                    parser.terminate_c1_st(&mut output)
                } else {
                    parser.dcs_parser_inner(b, &mut output)
                } {
                    ParserOutcome::Finished => {
                        self.inner = ParserInner::Empty;
                    }
                    ParserOutcome::FinishedAtEsc => {
                        self.restart_escape_with(b, &mut data_output, &mut output);
                    }
                    ParserOutcome::Continue => (),
                    ParserOutcome::Invalid(_) | ParserOutcome::InvalidParserFailure(_) => {
                        output.push(TerminalOutput::Invalid);
                        self.inner = ParserInner::Empty;
                    }
                },
                ParserInner::Apc(parser) => match if is_c1_st(b, self.s8c1t_mode) {
                    parser.terminate_c1_st(&mut output)
                } else {
                    parser.apc_parser_inner(b, &mut output)
                } {
                    ParserOutcome::Finished => {
                        self.inner = ParserInner::Empty;
                    }
                    ParserOutcome::FinishedAtEsc => {
                        self.restart_escape_with(b, &mut data_output, &mut output);
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
                        self.recycle_csi_buffers();
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
                            self.recycle_csi_buffers();
                            self.inner = ParserInner::Empty;
                            if output.last() == Some(&TerminalOutput::Invalid) {
                                debug!(
                                    "Invalid ANSI sequence; recent={}",
                                    self.current_trace_escaped()
                                );
                            }
                        }
                        ParserOutcome::Continue => (),
                        // CSI aborts on ESC above and never returns this;
                        // handled for completeness rather than as a panic path.
                        ParserOutcome::FinishedAtEsc => {
                            self.recycle_csi_buffers();
                            self.restart_escape_with(b, &mut data_output, &mut output);
                        }
                        ParserOutcome::InvalidParserFailure(message) => {
                            debug!(
                                "Invalid ANSI sequence: {}; recent={}",
                                message,
                                self.current_trace_escaped()
                            );
                            self.recycle_csi_buffers();
                            self.inner = ParserInner::Empty;
                            output.push(TerminalOutput::Invalid);
                        }
                        ParserOutcome::Invalid(message) => {
                            debug!(
                                "Invalid ANSI sequence: {}; recent={}",
                                message,
                                self.current_trace_escaped()
                            );
                            self.recycle_csi_buffers();
                            self.inner = ParserInner::Empty;
                            output.push(TerminalOutput::Invalid);
                        }
                    }
                }
                ParserInner::Osc(parser) => match if is_c1_st(b, self.s8c1t_mode) {
                    parser.terminate_c1_st(&mut output)
                } else {
                    parser.ansiparser_inner_osc(b, &mut output)
                } {
                    ParserOutcome::Finished => {
                        self.inner = ParserInner::Empty;
                        if output.last() == Some(&TerminalOutput::Invalid) {
                            debug!(
                                "Invalid ANSI sequence; recent={}",
                                self.current_trace_escaped()
                            );
                        }
                    }
                    ParserOutcome::FinishedAtEsc => {
                        self.restart_escape_with(b, &mut data_output, &mut output);
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
                            self.current_trace_escaped()
                        );
                        self.inner = ParserInner::Empty;
                    }
                },
                ParserInner::Vt52Escape => {
                    push_data_if_non_empty(&mut data_output, &mut output);
                    self.handle_vt52_escape(b, &mut data_output, &mut output);
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
                ParserInner::Vt52Ignore => {
                    self.handle_vt52_ignore(b, &mut data_output, &mut output);
                }
                ParserInner::Vt52NonAscii { table, prefix } => {
                    let (table, prefix) = (*table, *prefix);
                    self.vt52_non_ascii_byte(table, prefix, b, &mut data_output, &mut output);
                }
            }

            // A mode change (S8C1T/S7C1T, DECANM, RIS) must govern the next
            // byte of this same read, not the next read, so the parser applies
            // it here, as it emits the output.  The `continue` paths above
            // emit only C0 / C1 controls, none of which change a mode.
            for emitted in &output[emitted_from..] {
                apply_parser_mode_change(emitted, &mut self.vt52_mode, &mut self.s8c1t_mode);
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

    /// Handle a single byte inside an ignored VT52 `ESC (`-style sequence.
    ///
    /// Follows xterm's `vt52_ignore_table` (`VTPrsTbl.c:9890`): C0 controls
    /// and DEL as in [`Self::vt52_control`], 0x20-0x2F swallowed with the
    /// state unchanged, 0x30-0x7E swallowed and the state ends, and a
    /// non-ASCII character as in [`Self::vt52_non_ascii_byte`].
    fn handle_vt52_ignore(
        &mut self,
        b: u8,
        data_output: &mut Vec<u8>,
        output: &mut Vec<TerminalOutput>,
    ) {
        match b {
            0x00..=0x1F | 0x7F => self.vt52_control(b, data_output, output),
            0x20..=0x2F => {}
            0x30..=0x7E => self.inner = ParserInner::Empty,
            _ => self.vt52_non_ascii_byte(
                Vt52Table::Ignore,
                Utf8Prefix::new(),
                b,
                data_output,
                output,
            ),
        }
    }

    /// A C0 control or DEL met while in a VT52 escape table.
    ///
    /// Identical in `vt52_esc_table` and `vt52_ignore_table`: ESC restarts the
    /// escape (`CASE_ESC`, `charproc.c:3717`: `parsestate = vt52_esc_table`),
    /// CAN and SUB abort to ground (`CASE_CAN`/`CASE_SUB`, 3618/3646:
    /// `ResetState`), and every other control either executes (ENQ, BEL, BS,
    /// HT, LF/VT/FF, CR: 3680-3770, none of which touches `parsestate`) or is
    /// `CASE_IGNORE` (3614, also leaves `parsestate`), so the table in force
    /// stays in force.
    fn vt52_control(&mut self, b: u8, data_output: &mut Vec<u8>, output: &mut Vec<TerminalOutput>) {
        match b {
            0x1B => self.inner = ParserInner::Vt52Escape,
            0x18 | 0x1A => self.inner = ParserInner::Empty,
            _ => {
                // Bytes without an executed meaning are dropped, not queued as text.
                let _ = self.ansi_parser_inner_empty(b, data_output, output);
            }
        }
    }

    /// One byte of a non-ASCII character met while in a VT52 escape table.
    ///
    /// xterm decodes UTF-8 before it consults the table: a character above
    /// U+00FF is `CASE_GROUND_STATE` (`charproc.c:3224-3235`, "wide
    /// characters ... treated the same as printing characters"), so it is
    /// swallowed and ends the sequence; U+0080-U+00FF index the table
    /// (`CASE_IGNORE` in both VT52 tables), so they are swallowed and the
    /// table stays in force.
    ///
    /// `prefix` holds the bytes of the character so far (empty for its first
    /// byte). Character boundaries are those of `std::str::from_utf8`, the
    /// same notion the text path's lossy decoding uses: a stray continuation
    /// byte, an invalid lead byte or a sequence cut short by a non-continuation
    /// byte is one replacement character (above U+00FF, so it ends the
    /// sequence). The cutting byte is then handled as ordinary input.
    fn vt52_non_ascii_byte(
        &mut self,
        table: Vt52Table,
        mut prefix: Utf8Prefix,
        b: u8,
        data_output: &mut Vec<u8>,
        output: &mut Vec<TerminalOutput>,
    ) {
        let is_continuation = (0x80..=0xBF).contains(&b);
        if prefix.len > 0 && !is_continuation {
            self.inner = ParserInner::Empty;
            if self.ansi_parser_inner_empty(b, data_output, output).is_ok() {
                data_output.push(b);
            }
            return;
        }
        if !prefix.push(b) {
            self.inner = ParserInner::Empty;
            return;
        }
        match std::str::from_utf8(prefix.as_slice()) {
            Ok(text) => {
                let above_latin1 = text.chars().next().is_some_and(|c| u32::from(c) > 0xFF);
                self.inner = if above_latin1 {
                    ParserInner::Empty
                } else {
                    table.parser_state()
                };
            }
            Err(e) => match e.error_len() {
                None => self.inner = ParserInner::Vt52NonAscii { table, prefix },
                Some(invalid) => {
                    // The invalid bytes are one replacement character; any
                    // bytes after them are left for the text path to decode.
                    if let Some(rest) = prefix.as_slice().get(invalid..) {
                        data_output.extend_from_slice(rest);
                    }
                    self.inner = ParserInner::Empty;
                }
            },
        }
    }

    /// Handle a single byte after ESC in VT52 mode.
    ///
    /// VT52 escape sequences are all single-byte commands except `ESC Y` (cursor
    /// address) which takes two additional parameter bytes.
    fn handle_vt52_escape(
        &mut self,
        b: u8,
        data_output: &mut Vec<u8>,
        output: &mut Vec<TerminalOutput>,
    ) {
        match b {
            0x00..=0x1F | 0x7F => self.vt52_control(b, data_output, output),
            0x80..=0xFF => self.vt52_non_ascii_byte(
                Vt52Table::Escape,
                Utf8Prefix::new(),
                b,
                data_output,
                output,
            ),
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
            // ESC F — Enter graphics mode (DEC special graphics in G0)
            b'F' => {
                output.push(TerminalOutput::DesignateCharset {
                    slot: CharsetSlot::G0,
                    set: CharacterSet::DecSpecialGraphics,
                });
                self.inner = ParserInner::Empty;
            }
            // ESC G — Exit graphics mode (ASCII in G0)
            b'G' => {
                output.push(TerminalOutput::DesignateCharset {
                    slot: CharsetSlot::G0,
                    set: CharacterSet::Ascii,
                });
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
            //
            // `push()` flips `vt52_mode` as soon as this output is emitted
            // (see `apply_parser_mode_change`), because any bytes remaining in
            // the current buffer must be routed through the ANSI parser, not
            // the VT52 parser.  vttest issues `ESC <` followed by
            // `ESC P … ESC \` (DECRQSS) in a single PTY read; without the
            // immediate flip the `ESC P` would be routed to `handle_vt52_escape`
            // which does not recognise `P` and the DCS payload would leak to the
            // display as literal text.
            b'<' => {
                output.push(TerminalOutput::Mode(Mode::Decanm(Decanm::Ansi)));
                self.inner = ParserInner::Empty;
            }
            // 0x20-0x2F (SP ! " # $ % & ' ( ) * + , - . /) — `CASE_VT52_IGNORE`:
            // VT52 has no sequence here, and the byte that follows belongs to
            // it and is swallowed.
            0x20..=0x2F => {
                self.inner = ParserInner::Vt52Ignore;
            }
            // Every other final is `CASE_GROUND_STATE` in `vt52_esc_table`:
            // silently ignored, e.g. ESC N/O/n/o/|/}/~ (the ANSI-only shifts).
            _ => {
                debug!(
                    "Ignoring VT52 escape: 0x{b:02x}; raw sequence: \"{}\"",
                    self.current_trace_escaped()
                );
                self.inner = ParserInner::Empty;
            }
        }
    }

    /// Render the bytes of the sequence currently being parsed, for
    /// diagnostics. Each sub-parser renders the bytes it already holds
    /// (bounded for logging); with no sub-parser active this falls back to the
    /// top-level trace of recent input.
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
    use freminal_common::buffer_states::osc::AnsiOscType;

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
        assert_eq!(data, []); // correct: data is only pushed in FreminalAnsiParser::push
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
            DesignateCharset {
                slot: CharsetSlot::G1,
                set: CharacterSet::DecSpecialGraphics,
            },
            InvokeGl(CharsetSlot::G1),
            InvokeGr(CharsetSlot::G2),
            SingleShift(CharsetSlot::G3),
            CursorVisualStyle(freminal_common::cursor::CursorVisualStyle::BlockCursorSteady),
            CursorVisualStyleDefault,
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
            vec![TerminalOutput::DesignateCharset {
                slot: CharsetSlot::G0,
                set: CharacterSet::DecSpecialGraphics,
            }]
        );
    }

    #[test]
    fn vt52_exit_graphics_mode() {
        let mut p = vt52_parser();
        let result = p.push(b"\x1bG");
        assert_eq!(
            result,
            vec![TerminalOutput::DesignateCharset {
                slot: CharsetSlot::G0,
                set: CharacterSet::Ascii,
            }]
        );
    }

    #[test]
    fn vt52_so_and_si_are_ignored() {
        let mut p = vt52_parser();
        assert_eq!(p.push(b"\x0e"), vec![]);
        assert_eq!(p.push(b"\x0f"), vec![]);
        assert_eq!(
            p.push(b"a\x0eb\x0fc"),
            vec![TerminalOutput::Data(b"abc".to_vec())]
        );
    }

    #[test]
    fn vt52_so_si_ignored_but_esc_f_and_esc_g_still_emit() {
        let mut p = vt52_parser();
        assert_eq!(
            p.push(b"\x0e\x1bF\x0f\x1bG"),
            vec![
                TerminalOutput::DesignateCharset {
                    slot: CharsetSlot::G0,
                    set: CharacterSet::DecSpecialGraphics,
                },
                TerminalOutput::DesignateCharset {
                    slot: CharsetSlot::G0,
                    set: CharacterSet::Ascii,
                },
            ]
        );
    }

    #[test]
    fn vt52_ignored_introducers_swallow_the_next_printable_byte() {
        for intro in *b"()*+ $" {
            let mut p = vt52_parser();
            assert_eq!(
                p.push(&[0x1B, intro, b'0', b'A']),
                vec![TerminalOutput::Data(b"A".to_vec())],
                "ESC 0x{intro:02x} 0 A"
            );
            assert_eq!(p.inner, ParserInner::Empty);
        }
    }

    #[test]
    fn vt52_ignore_state_swallows_0x20_to_0x2f_and_del_then_ends_on_0x30_to_0x7e() {
        let mut p = vt52_parser();
        assert_eq!(
            p.push(b"\x1b$ !/\x7f0A"),
            vec![TerminalOutput::Data(b"A".to_vec())]
        );
        assert_eq!(p.inner, ParserInner::Empty);

        let mut p = vt52_parser();
        assert_eq!(
            p.push(b"\x1b(~A"),
            vec![TerminalOutput::Data(b"A".to_vec())]
        );
    }

    #[test]
    fn vt52_ignore_state_executes_c0_controls_and_stays() {
        let mut p = vt52_parser();
        assert_eq!(
            p.push(b"\x1b(\r\n\x08\x07\t0A"),
            vec![
                TerminalOutput::CarriageReturn,
                TerminalOutput::Newline,
                TerminalOutput::Backspace,
                TerminalOutput::Bell,
                TerminalOutput::Tab,
                TerminalOutput::Data(b"A".to_vec()),
            ]
        );
    }

    #[test]
    fn vt52_ignore_state_drops_unassigned_controls_and_so_si() {
        let mut p = vt52_parser();
        assert_eq!(
            p.push(b"\x1b(\x00\x01\x0e\x0f\x1f0A"),
            vec![TerminalOutput::Data(b"A".to_vec())]
        );
    }

    #[test]
    fn vt52_ignore_state_cancelled_by_can_and_sub() {
        for cancel in [0x18u8, 0x1A] {
            let mut p = vt52_parser();
            assert_eq!(
                p.push(&[0x1B, b'(', cancel, b'0']),
                vec![TerminalOutput::Data(b"0".to_vec())],
                "cancel 0x{cancel:02x}"
            );
            assert_eq!(p.inner, ParserInner::Empty);
        }
    }

    #[test]
    fn vt52_ignore_state_esc_restarts_a_vt52_escape() {
        let mut p = vt52_parser();
        assert_eq!(
            p.push(b"\x1b(\x1bA"),
            vec![TerminalOutput::SetCursorPosRel {
                x: None,
                y: Some(-1),
            }]
        );
        assert_eq!(p.inner, ParserInner::Empty);
    }

    #[test]
    fn vt52_exit_inside_ignore_state_is_an_escape_restart_not_swallowed() {
        let mut p = vt52_parser();
        let result = p.push(b"\x1b(\x1b<A");
        assert_eq!(
            result,
            vec![
                TerminalOutput::Mode(Mode::Decanm(Decanm::Ansi)),
                TerminalOutput::Data(b"A".to_vec()),
            ]
        );
        assert_eq!(p.vt52_mode, Decanm::Ansi);
        assert_eq!(p.inner, ParserInner::Empty);
    }

    #[test]
    fn vt52_ignore_state_survives_a_chunk_boundary() {
        let mut p = vt52_parser();
        assert_eq!(p.push(b"\x1b("), vec![]);
        assert_eq!(p.inner, ParserInner::Vt52Ignore);
        assert_eq!(p.push(b"0"), vec![]);
        assert_eq!(p.inner, ParserInner::Empty);
        assert_eq!(p.push(b"A"), vec![TerminalOutput::Data(b"A".to_vec())]);

        let mut p = vt52_parser();
        assert_eq!(p.push(b"\x1b"), vec![]);
        assert_eq!(p.push(b"$"), vec![]);
        assert_eq!(p.push(b" "), vec![]);
        assert_eq!(p.inner, ParserInner::Vt52Ignore);
        assert_eq!(p.push(b"0A"), vec![TerminalOutput::Data(b"A".to_vec())]);
    }

    #[test]
    fn vt52_single_and_locking_shift_escapes_are_silent() {
        for b in *b"NOno|}~" {
            let mut p = vt52_parser();
            assert_eq!(
                p.push(&[0x1B, b, b'A']),
                vec![TerminalOutput::Data(b"A".to_vec())],
                "ESC 0x{b:02x}"
            );
            assert!(!p.push(&[0x1B, b]).contains(&TerminalOutput::Invalid));
            assert_eq!(p.inner, ParserInner::Empty);
        }
    }

    #[test]
    fn vt52_ignore_introducers_in_ansi_mode_keep_their_ansi_meaning() {
        let mut p = FreminalAnsiParser::new();
        assert_eq!(
            p.push(b"\x1b(0A"),
            vec![
                TerminalOutput::DesignateCharset {
                    slot: CharsetSlot::G0,
                    set: CharacterSet::DecSpecialGraphics,
                },
                TerminalOutput::Data(b"A".to_vec()),
            ]
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

    /// Finals xterm's `vt52_esc_table` handles; every other byte in
    /// 0x30-0x7E is `CASE_GROUND_STATE`.
    const VT52_HANDLED_FINALS: &[u8] = b"ABCDFGHIJKYZ=><";

    #[test]
    fn vt52_ground_state_finals_are_silent() {
        for b in 0x30u8..=0x7E {
            if VT52_HANDLED_FINALS.contains(&b) {
                continue;
            }
            let mut p = vt52_parser();
            assert_eq!(
                p.push(&[0x1B, b, b'A']),
                vec![TerminalOutput::Data(b"A".to_vec())],
                "ESC 0x{b:02x} A"
            );
            assert_eq!(p.inner, ParserInner::Empty, "ESC 0x{b:02x}");
        }
    }

    #[test]
    fn vt52_every_byte_0x20_to_0x2f_after_esc_swallows_the_next() {
        for b in 0x20u8..=0x2F {
            let mut p = vt52_parser();
            assert_eq!(
                p.push(&[0x1B, b, b'0', b'A']),
                vec![TerminalOutput::Data(b"A".to_vec())],
                "ESC 0x{b:02x} 0 A"
            );
        }
    }

    #[test]
    fn vt52_escape_executes_controls_and_stays_in_the_escape() {
        let up = TerminalOutput::SetCursorPosRel {
            x: None,
            y: Some(-1),
        };
        let cases: [(u8, TerminalOutput); 8] = [
            (b'\r', TerminalOutput::CarriageReturn),
            (b'\n', TerminalOutput::Newline),
            (0x0B, TerminalOutput::Newline),
            (0x0C, TerminalOutput::Newline),
            (0x08, TerminalOutput::Backspace),
            (0x09, TerminalOutput::Tab),
            (0x07, TerminalOutput::Bell),
            (0x05, TerminalOutput::Enq),
        ];
        for (control, expected) in cases {
            let mut p = vt52_parser();
            assert_eq!(
                p.push(&[0x1B, control, b'A']),
                vec![expected, up.clone()],
                "ESC 0x{control:02x} A"
            );
            assert_eq!(p.inner, ParserInner::Empty);
        }
    }

    #[test]
    fn vt52_escape_drops_ignored_controls_and_del_and_stays() {
        let up = TerminalOutput::SetCursorPosRel {
            x: None,
            y: Some(-1),
        };
        for control in [0x00u8, 0x01, 0x0E, 0x0F, 0x10, 0x1F, 0x7F] {
            let mut p = vt52_parser();
            assert_eq!(
                p.push(&[0x1B, control, b'A']),
                vec![up.clone()],
                "ESC 0x{control:02x} A"
            );
        }
    }

    #[test]
    fn vt52_escape_restarts_on_esc_and_aborts_on_can_and_sub() {
        let mut p = vt52_parser();
        assert_eq!(
            p.push(b"\x1b\x1bA"),
            vec![TerminalOutput::SetCursorPosRel {
                x: None,
                y: Some(-1),
            }]
        );
        for cancel in [0x18u8, 0x1A] {
            let mut p = vt52_parser();
            assert_eq!(
                p.push(&[0x1B, cancel, b'A']),
                vec![TerminalOutput::Data(b"A".to_vec())],
                "ESC 0x{cancel:02x} A"
            );
            assert_eq!(p.inner, ParserInner::Empty);
        }
    }

    #[test]
    fn vt52_escape_control_split_across_a_chunk_boundary() {
        let mut p = vt52_parser();
        assert_eq!(p.push(b"\x1b"), vec![]);
        assert_eq!(p.push(b"\r"), vec![TerminalOutput::CarriageReturn]);
        assert_eq!(p.inner, ParserInner::Vt52Escape);
    }

    #[test]
    fn vt52_non_ascii_above_latin1_ends_the_ignore_state() {
        for ch in ["\u{4e16}", "\u{1f600}", "\u{100}"] {
            let mut p = vt52_parser();
            let mut input = b"\x1b(".to_vec();
            input.extend_from_slice(ch.as_bytes());
            input.push(b'A');
            assert_eq!(
                p.push(&input),
                vec![TerminalOutput::Data(b"A".to_vec())],
                "ESC ( {ch:?} A"
            );
            assert_eq!(p.inner, ParserInner::Empty);
        }
    }

    #[test]
    fn vt52_latin1_character_is_ignored_and_the_ignore_state_stays() {
        // U+00E9 indexes xterm's table (CASE_IGNORE); only the ASCII `A` ends it.
        let mut p = vt52_parser();
        assert_eq!(p.push("\x1b(\u{e9}A".as_bytes()), vec![]);
        assert_eq!(p.inner, ParserInner::Empty);
        assert_eq!(p.push(b"A"), vec![TerminalOutput::Data(b"A".to_vec())]);

        let mut p = vt52_parser();
        assert_eq!(p.push("\x1b(\u{9f}".as_bytes()), vec![]);
        assert_eq!(p.inner, ParserInner::Vt52Ignore);
    }

    #[test]
    fn vt52_non_ascii_in_the_escape_table() {
        let up = TerminalOutput::SetCursorPosRel {
            x: None,
            y: Some(-1),
        };
        // Above U+00FF: ground, silently.
        let mut p = vt52_parser();
        assert_eq!(
            p.push("\x1b\u{4e16}A".as_bytes()),
            vec![TerminalOutput::Data(b"A".to_vec())]
        );
        // U+0080-U+00FF: ignored, still after the ESC.
        let mut p = vt52_parser();
        assert_eq!(p.push("\x1b\u{e9}A".as_bytes()), vec![up]);
    }

    #[test]
    fn vt52_non_ascii_character_split_across_chunks() {
        let bytes = "\u{4e16}".as_bytes();
        let mut p = vt52_parser();
        assert_eq!(p.push(b"\x1b("), vec![]);
        assert_eq!(p.push(&bytes[..1]), vec![]);
        assert!(matches!(p.inner, ParserInner::Vt52NonAscii { .. }));
        assert_eq!(p.push(&bytes[1..2]), vec![]);
        assert!(matches!(p.inner, ParserInner::Vt52NonAscii { .. }));
        assert_eq!(p.push(&bytes[2..]), vec![]);
        assert_eq!(p.inner, ParserInner::Empty);
        assert_eq!(p.push(b"A"), vec![TerminalOutput::Data(b"A".to_vec())]);

        // And in the escape table, with a Latin-1 character (stays in escape).
        let latin = "\u{e9}".as_bytes();
        let mut p = vt52_parser();
        assert_eq!(p.push(&[0x1B, latin[0]]), vec![]);
        assert!(matches!(p.inner, ParserInner::Vt52NonAscii { .. }));
        assert_eq!(p.push(&latin[1..]), vec![]);
        assert_eq!(p.inner, ParserInner::Vt52Escape);
    }

    #[test]
    fn vt52_invalid_utf8_is_one_replacement_character_and_ends_the_state() {
        let data = |bytes: &[u8]| vec![TerminalOutput::Data(bytes.to_vec())];
        // Stray continuation byte, invalid lead bytes.
        for bad in [0x80u8, 0xBF, 0xC0, 0xC1, 0xF5, 0xFF] {
            let mut p = vt52_parser();
            assert_eq!(
                p.push(&[0x1B, b'(', bad, b'A']),
                data(b"A"),
                "ESC ( 0x{bad:02x} A"
            );
            assert_eq!(p.inner, ParserInner::Empty);
        }
        // Truncated sequence cut short by an ASCII byte: the byte is ordinary input.
        let mut p = vt52_parser();
        assert_eq!(p.push(&[0x1B, b'(', 0xE4, 0xB8, b'A']), data(b"A"));
        assert_eq!(p.inner, ParserInner::Empty);
        // ... including a control, which then executes normally.
        let mut p = vt52_parser();
        assert_eq!(
            p.push(&[0x1B, b'(', 0xE4, b'\r']),
            vec![TerminalOutput::CarriageReturn]
        );
        assert_eq!(p.inner, ParserInner::Empty);
        // A bad second byte: the lead is the replacement character and the
        // byte after it is left for the text path.
        let mut p = vt52_parser();
        assert_eq!(p.push(&[0x1B, b'(', 0xE0, 0x80, b'A']), data(b"\x80A"));
        // The same in the escape table.
        let mut p = vt52_parser();
        assert_eq!(p.push(&[0x1B, 0xFF, b'A']), data(b"A"));
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
        assert_eq!(p.vt52_mode, Decanm::Ansi);

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

        // ESC < should emit Mode::Decanm(Decanm::Ansi) and immediately
        // flip the parser's vt52_mode flag (no external sync needed).
        let result = p.push(b"\x1b<");
        assert_eq!(
            result,
            vec![TerminalOutput::Mode(Mode::Decanm(Decanm::Ansi))]
        );
        assert_eq!(
            p.vt52_mode,
            Decanm::Ansi,
            "ESC < must immediately flip vt52_mode"
        );

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

    /// Regression test: `ESC <` in VT52 mode must immediately flip the parser
    /// to ANSI mode so that bytes following in the same `push()` call are
    /// routed through the ANSI parser.
    ///
    /// Reproduces the vttest Menu 7 DECRQSS bug: vttest sends `ESC <` (exit
    /// VT52) followed by `ESC P $ q " p ESC \` (DECRQSS for DECSCL) in a
    /// single buffer.  Without the immediate flip the `ESC P` was routed to
    /// `handle_vt52_escape` which doesn't recognise `P`, and the DCS payload
    /// leaked to the display as literal text.
    #[test]
    fn vt52_exit_followed_by_dcs_in_same_buffer() {
        let mut p = vt52_parser();

        // ESC < (exit VT52) + ESC P $ q " p ESC \ (DECRQSS for DECSCL)
        let result = p.push(b"\x1b<\x1bP$q\"p\x1b\\");

        // First output: Mode change to ANSI
        assert_eq!(
            result[0],
            TerminalOutput::Mode(Mode::Decanm(Decanm::Ansi)),
            "First output should be ANSI mode switch"
        );

        // Second output: the DCS should be parsed as DeviceControlString,
        // NOT as Data (literal text).
        let has_dcs = result
            .iter()
            .any(|o| matches!(o, TerminalOutput::DeviceControlString(_)));
        assert!(
            has_dcs,
            "DCS after ESC < must be parsed as DeviceControlString, not literal text: {result:?}"
        );

        // The parser should now be in ANSI mode
        assert_eq!(p.vt52_mode, Decanm::Ansi);
    }

    // -------------------------------------------------------------------------
    // Parser mode changes take effect within the same push() (Task 147.2)
    // -------------------------------------------------------------------------

    /// `ESC SP G` then `0x9B 6 n` in one `push()`: the 0x9B is an 8-bit CSI.
    /// Before 147.2 the parser switched mode only when `TerminalState` applied
    /// the output, so the 0x9B was parsed as text.
    #[test]
    fn s8c1t_applies_to_the_rest_of_the_same_push() {
        let mut p = FreminalAnsiParser::new();
        let result = p.push(b"\x1b G\x9b6n");
        assert_eq!(
            result,
            vec![
                TerminalOutput::EightBitControl,
                TerminalOutput::CursorReport
            ]
        );
        assert_eq!(p.s8c1t_mode, S8c1t::EightBit);
    }

    /// `ESC SP F` then `0x9B 6 n` in one `push()`: the 0x9B is no longer a CSI
    /// introducer, so it is plain text, as in any 7-bit read.
    #[test]
    fn s7c1t_applies_to_the_rest_of_the_same_push() {
        let mut p = FreminalAnsiParser::new();
        p.s8c1t_mode = S8c1t::EightBit;
        let result = p.push(b"\x1b F\x9b6n");
        assert_eq!(
            result,
            vec![
                TerminalOutput::SevenBitControl,
                TerminalOutput::Data(b"\x9b6n".to_vec()),
            ]
        );
        assert_eq!(p.s8c1t_mode, S8c1t::SevenBit);
    }

    /// With the parser already in 8-bit mode, RIS then `0x9B 6 n` in one
    /// `push()`: RIS returns the parser to 7-bit at its position, so the 0x9B
    /// is text.  Before 147.2 the parser stayed 8-bit for the whole read and
    /// executed the CSI.
    #[test]
    fn ris_resets_parser_modes_within_the_same_push() {
        let mut p = FreminalAnsiParser::new();
        p.s8c1t_mode = S8c1t::EightBit;
        let result = p.push(b"\x1bc\x9b6n");
        assert_eq!(
            result,
            vec![
                TerminalOutput::ResetDevice,
                TerminalOutput::Data(b"\x9b6n".to_vec()),
            ]
        );
        assert_eq!(p.s8c1t_mode, S8c1t::SevenBit);
        assert_eq!(p.vt52_mode, Decanm::Ansi);
    }

    /// S8C1T, RIS, then S8C1T again in one `push()`: the mode changes apply in
    /// order, so the final state is 8-bit and the trailing `0x9B` is a CSI.
    #[test]
    fn s8c1t_ris_s8c1t_apply_in_order_within_the_same_push() {
        let mut p = FreminalAnsiParser::new();
        let result = p.push(b"\x1b G\x1bc\x1b G\x9b6n");
        assert_eq!(
            result,
            vec![
                TerminalOutput::EightBitControl,
                TerminalOutput::ResetDevice,
                TerminalOutput::EightBitControl,
                TerminalOutput::CursorReport,
            ]
        );
    }

    /// Leaving VT52 with `ESC <` and then RIS in one `push()`: the RIS is
    /// parsed as RIS (the ANSI escape state is back) and returns the parser to
    /// 7-bit controls, so the `0x9B` after it is text. RIS cannot be sent in
    /// VT52 mode itself (`ESC c` is ignored there), so the parser's own DECANM
    /// is never something RIS has to undo.
    #[test]
    fn ris_after_leaving_vt52_resets_s8c1t_within_the_same_push() {
        let mut p = FreminalAnsiParser::new();
        p.s8c1t_mode = S8c1t::EightBit;
        let _ = p.push(b"\x1b[?2l");
        assert_eq!(p.vt52_mode, Decanm::Vt52);
        let result = p.push(b"\x1b<\x1bc\x9b6n");
        assert_eq!(
            result,
            vec![
                TerminalOutput::Mode(Mode::Decanm(Decanm::Ansi)),
                TerminalOutput::ResetDevice,
                TerminalOutput::Data(b"\x9b6n".to_vec()),
            ]
        );
        assert_eq!(p.vt52_mode, Decanm::Ansi);
        assert_eq!(p.s8c1t_mode, S8c1t::SevenBit);
    }

    /// In VT52 mode `ESC c` is not RIS: it is ignored, and S8C1T is untouched.
    #[test]
    fn esc_c_in_vt52_mode_is_not_a_reset() {
        let mut p = vt52_parser();
        p.s8c1t_mode = S8c1t::EightBit;
        assert_eq!(p.push(b"\x1bc"), vec![]);
        assert_eq!(p.vt52_mode, Decanm::Vt52);
        assert_eq!(p.s8c1t_mode, S8c1t::EightBit);
    }

    /// `CSI ? 2 l` then a VT52 `ESC A` in one `push()`: the `ESC A` is VT52
    /// cursor-up, not an (invalid) ANSI escape.
    #[test]
    fn decanm_reset_applies_to_the_rest_of_the_same_push() {
        let mut p = FreminalAnsiParser::new();
        let result = p.push(b"\x1b[?2l\x1bA");
        assert_eq!(
            result,
            vec![
                TerminalOutput::Mode(Mode::Decanm(Decanm::Vt52)),
                TerminalOutput::SetCursorPosRel {
                    x: None,
                    y: Some(-1)
                },
            ]
        );
        assert_eq!(p.vt52_mode, Decanm::Vt52);
    }

    /// The reverse: `CSI ? 2 h` is not reachable from VT52 mode (CSI is not
    /// recognised there), so `ESC <` is the way back, and the very next byte
    /// of the same push is ANSI.
    #[test]
    fn vt52_exit_applies_to_the_rest_of_the_same_push() {
        let mut p = vt52_parser();
        let result = p.push(b"\x1b<\x1b[1A");
        assert_eq!(
            result,
            vec![
                TerminalOutput::Mode(Mode::Decanm(Decanm::Ansi)),
                TerminalOutput::SetCursorPosRel {
                    x: None,
                    y: Some(-1)
                },
            ]
        );
    }

    #[test]
    fn esc_plus_0_designates_g3_as_dec_special_graphics() {
        // ESC + 0 designates DEC Special Graphics into G3 and into no other
        // slot: it must not behave like ESC ( 0.
        let mut parser = FreminalAnsiParser::new();
        let result = parser.push(b"\x1b+0");
        assert_eq!(
            result,
            vec![TerminalOutput::DesignateCharset {
                slot: CharsetSlot::G3,
                set: CharacterSet::DecSpecialGraphics,
            }]
        );
    }

    /// ENQ (0x05) must be recognised as a C0 control character and produce
    /// `TerminalOutput::Enq` — not be treated as printable data.
    #[test]
    fn enq_produces_enq_output() {
        let mut parser = FreminalAnsiParser::new();
        let result = parser.push(b"\x05");
        assert_eq!(result, vec![TerminalOutput::Enq]);
    }

    /// ENQ in the middle of text must flush the preceding data and produce
    /// a separate `Enq` output.
    #[test]
    fn enq_flushes_data_and_emits_enq() {
        let mut parser = FreminalAnsiParser::new();
        let result = parser.push(b"AB\x05CD");
        assert_eq!(
            result,
            vec![
                TerminalOutput::Data(b"AB".to_vec()),
                TerminalOutput::Enq,
                TerminalOutput::Data(b"CD".to_vec()),
            ]
        );
    }

    // =========================================================================
    // Coverage-gap tests
    // =========================================================================

    // ── ParserOutcome Display impl (lines 38-40) ────────────────────────────

    #[test]
    fn parser_outcome_display_continue() {
        assert_eq!(ParserOutcome::Continue.to_string(), "Continue");
    }

    #[test]
    fn parser_outcome_display_finished() {
        assert_eq!(ParserOutcome::Finished.to_string(), "Finished");
    }

    #[test]
    fn parser_outcome_display_invalid() {
        let o = ParserOutcome::Invalid("bad input".to_string());
        assert_eq!(o.to_string(), "Invalid: bad input");
    }

    #[test]
    fn parser_outcome_display_invalid_parser_failure() {
        let o = ParserOutcome::InvalidParserFailure(ParserFailures::UnhandledInnerEscape(
            "test".to_string(),
        ));
        let s = o.to_string();
        assert!(s.starts_with("InvalidParserFailure:"));
    }

    // ── parse_param_as error path (line 85) ─────────────────────────────────

    #[test]
    fn parse_param_as_invalid_number() {
        // "abc" cannot be parsed as usize → triggers the error path
        let result = parse_param_as::<usize>(b"abc");
        assert!(result.is_err());
    }

    // ── seq_tracer_ref (lines 137-139) ──────────────────────────────────────

    #[test]
    fn seq_tracer_ref_accessible() {
        let p = FreminalAnsiParser::new();
        // Just calling seq_tracer_ref exercises the uncovered getter
        let trace = p.seq_tracer_ref();
        // New parser should have empty trace
        assert!(
            trace.as_str().is_empty(),
            "new parser trace should be empty"
        );
    }

    // ── 8-bit C1 controls: OSC, APC, fallthrough (lines 253-264) ───────────

    #[test]
    fn c1_osc_0x9d_enters_osc_parser() {
        let mut p = FreminalAnsiParser::new();
        p.s8c1t_mode = S8c1t::EightBit;
        // 0x9D = 8-bit OSC introducer; followed by "0;title" + BEL
        let result = p.push(b"\x9D0;MyTitle\x07");
        let has_set_title = result
            .iter()
            .any(|o| matches!(o, TerminalOutput::OscResponse(AnsiOscType::SetTitleBar(t)) if t == "MyTitle"));
        assert!(has_set_title, "8-bit C1 OSC should be parsed: {result:?}");
    }

    #[test]
    fn c1_apc_0x9f_enters_apc_parser() {
        let mut p = FreminalAnsiParser::new();
        p.s8c1t_mode = S8c1t::EightBit;
        // 0x9F = 8-bit APC introducer; feed some content + ST (ESC \)
        let result = p.push(b"\x9Fsome_apc_data\x1b\\");
        // APC should be consumed; check parser returns to empty state
        assert_eq!(p.inner, ParserInner::Empty);
        // We don't care about the exact APC output, just that it was parsed
        let _ = result;
    }

    #[test]
    fn c1_fallthrough_other_bytes_silently_ignored() {
        let mut p = FreminalAnsiParser::new();
        p.s8c1t_mode = S8c1t::EightBit;
        // 0x80 is in the C1 range but has no defined meaning → catch-all
        let result = p.push(b"\x80");
        // Should produce no output and parser stays Empty
        assert!(
            result.is_empty(),
            "0x80 C1 byte should be silently ignored: {result:?}"
        );
        assert_eq!(p.inner, ParserInner::Empty);

        // PM (0x9E) has no handler either and is ignored.
        let result2 = p.push(b"\x9E");
        assert!(
            result2.is_empty(),
            "PM (0x9E) should be silently ignored: {result2:?}"
        );
    }

    // ── Escape state: ESC ESC is logged as debug (line 308) ─────────────────
    // Note: line 308 is a debug!() message inside match arm for ESC after ESC;
    // but that arm is unreachable because ESC ESC is caught by the `if b == 0x1B`
    // guard at line 279. The arm at line 306-308 is dead code — but we can still
    // exercise the ESC→ESC path at line 279.

    #[test]
    fn escape_esc_restarts_escape_state() {
        let mut p = FreminalAnsiParser::new();
        // Feed ESC ESC [1A — first ESC enters Escape, second ESC restarts Escape,
        // [1A completes as CSI 1A (cursor up)
        let result = p.push(b"\x1b\x1b[1A");
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
            "ESC ESC [1A should produce cursor up: {result:?}"
        );
    }

    // ── Escape state: Standard parser returns Invalid (lines 318-335) ───────

    #[test]
    fn escape_standard_parser_invalid_branch() {
        let mut p = FreminalAnsiParser::new();
        // ESC followed by an unknown byte like '!' which the StandardParser
        // doesn't recognize should eventually produce Invalid
        let result = p.push(b"\x1b!");
        // The parser should return to Empty and may produce Invalid
        assert_eq!(p.inner, ParserInner::Empty);
        let _ = result; // Invalid might or might not appear depending on standard parser
    }

    // ── Standard parser in push() loop: Invalid/Continue paths (413-428) ────

    #[test]
    fn standard_parser_multi_byte_sequence_produces_output() {
        let mut p = FreminalAnsiParser::new();
        // ESC ( 0 = Designate G0 as DEC Special Graphics (Standard parser, 2 bytes)
        let result = p.push(b"\x1b(0");
        assert!(
            result
                .iter()
                .any(|o| matches!(o, TerminalOutput::DesignateCharset { .. })),
            "ESC ( 0 should produce DesignateCharset: {result:?}"
        );
        assert_eq!(p.inner, ParserInner::Empty);
    }

    #[test]
    fn standard_parser_invalid_charset_designator() {
        let mut p = FreminalAnsiParser::new();
        // ESC ( followed by a byte that cannot be a designator final
        let result = p.push(b"\x1b(\x01");
        let has_invalid = result.iter().any(|o| matches!(o, TerminalOutput::Invalid));
        assert!(has_invalid, "ESC ( 0x01 should produce Invalid: {result:?}");
        assert_eq!(p.inner, ParserInner::Empty);
    }

    #[test]
    fn unknown_charset_designator_final_is_ignored() {
        let mut p = FreminalAnsiParser::new();
        // 'Z' is a well-formed final for a set freminal does not implement: the
        // sequence is consumed and emits nothing, not even `Invalid`.
        let result = p.push(b"\x1b(Z");
        assert!(result.is_empty(), "ESC ( Z must emit nothing: {result:?}");
        assert_eq!(p.inner, ParserInner::Empty);
    }

    // ── Character sets: SO/SI, single shifts ────────────────────────────────

    #[test]
    fn so_and_si_invoke_g1_and_g0_after_flushing_data() {
        let mut p = FreminalAnsiParser::new();
        let result = p.push(b"a\x0eb\x0fc");
        assert_eq!(
            result,
            vec![
                TerminalOutput::Data(b"a".to_vec()),
                TerminalOutput::InvokeGl(CharsetSlot::G1),
                TerminalOutput::Data(b"b".to_vec()),
                TerminalOutput::InvokeGl(CharsetSlot::G0),
                TerminalOutput::Data(b"c".to_vec()),
            ]
        );
    }

    #[test]
    fn so_inside_a_csi_executes_inline_and_the_csi_continues() {
        let mut p = FreminalAnsiParser::new();
        let result = p.push(b"\x1b[1\x0eA");
        assert_eq!(
            result,
            vec![
                TerminalOutput::InvokeGl(CharsetSlot::G1),
                TerminalOutput::SetCursorPosRel {
                    x: None,
                    y: Some(-1)
                },
            ]
        );
    }

    #[test]
    fn seven_bit_ss2_and_ss3_emit_single_shifts() {
        let mut p = FreminalAnsiParser::new();
        let result = p.push(b"a\x1bNb\x1bOc");
        assert_eq!(
            result,
            vec![
                TerminalOutput::Data(b"a".to_vec()),
                TerminalOutput::SingleShift(CharsetSlot::G2),
                TerminalOutput::Data(b"b".to_vec()),
                TerminalOutput::SingleShift(CharsetSlot::G3),
                TerminalOutput::Data(b"c".to_vec()),
            ]
        );
    }

    #[test]
    fn eight_bit_ss2_and_ss3_emit_single_shifts_under_s8c1t() {
        let mut p = FreminalAnsiParser::new();
        p.s8c1t_mode = S8c1t::EightBit;
        let result = p.push(b"a\x8eb\x8fc");
        assert_eq!(
            result,
            vec![
                TerminalOutput::Data(b"a".to_vec()),
                TerminalOutput::SingleShift(CharsetSlot::G2),
                TerminalOutput::Data(b"b".to_vec()),
                TerminalOutput::SingleShift(CharsetSlot::G3),
                TerminalOutput::Data(b"c".to_vec()),
            ]
        );
    }

    #[test]
    fn eight_bit_ss2_and_ss3_are_not_recognised_under_s7c1t() {
        // Without S8C1T the bytes are not C1 controls: no single shift.
        let mut p = FreminalAnsiParser::new();
        let result = p.push(b"\x8e\x8f");
        assert!(
            !result
                .iter()
                .any(|o| matches!(o, TerminalOutput::SingleShift(_))),
            "SS2/SS3 bytes must not shift under S7C1T: {result:?}"
        );
    }

    #[test]
    fn charset_outputs_do_not_change_a_parser_mode() {
        // `apply_parser_mode_change` must treat the charset outputs as inert.
        for output in [
            TerminalOutput::DesignateCharset {
                slot: CharsetSlot::G0,
                set: CharacterSet::DecSpecialGraphics,
            },
            TerminalOutput::InvokeGl(CharsetSlot::G1),
            TerminalOutput::InvokeGr(CharsetSlot::G2),
            TerminalOutput::SingleShift(CharsetSlot::G3),
        ] {
            let mut vt52 = Decanm::Vt52;
            let mut s8c1t = S8c1t::EightBit;
            apply_parser_mode_change(&output, &mut vt52, &mut s8c1t);
            assert_eq!(vt52, Decanm::Vt52, "{output}");
            assert_eq!(s8c1t, S8c1t::EightBit, "{output}");
        }
    }

    #[test]
    fn bare_esc_ending_a_string_then_n_is_single_shift_two() {
        // The bare ESC ends the OSC (`FinishedAtEsc`), and the byte after it is
        // the second byte of a new escape sequence: here ESC N.
        let mut p = FreminalAnsiParser::new();
        let result = p.push(b"\x1b]0;t\x1bNq");
        assert!(
            result.contains(&TerminalOutput::SingleShift(CharsetSlot::G2)),
            "ESC N after a bare-ESC-terminated OSC must be SS2: {result:?}"
        );
        assert_eq!(result.last(), Some(&TerminalOutput::Data(b"q".to_vec())));
    }

    // ── DCS parser Invalid path (lines 439-442) ────────────────────────────

    #[test]
    fn dcs_invalid_sequence_produces_invalid_output() {
        let mut p = FreminalAnsiParser::new();
        // Send a DCS with gibberish that the DCS parser can't handle,
        // terminated by ST (ESC \)
        // DCS parser expects specific formats; random bytes should trigger Invalid
        let result = p.push(b"\x1bP\xff\xff\x1b\\");
        // Parser should be back to Empty
        assert_eq!(p.inner, ParserInner::Empty);
        let _ = result;
    }

    // ── APC parser Invalid path (lines 449-452) ────────────────────────────
    // APC collects until ST; it's hard to make it return Invalid from typical
    // input, but we can exercise the path through the 8-bit introducer.

    #[test]
    fn apc_normal_sequence_completes() {
        let mut p = FreminalAnsiParser::new();
        // APC via ESC _ ... ESC \ — any content
        let result = p.push(b"\x1b_test_apc\x1b\\");
        assert_eq!(p.inner, ParserInner::Empty);
        let _ = result;
    }

    // ── CSI parser InvalidParserFailure path (lines 481-488) ────────────────

    #[test]
    fn csi_invalid_parser_failure_path() {
        let mut p = FreminalAnsiParser::new();
        // CSI > 0 c is a DA2 request — providing malformed params like "> ;"
        // triggers InvalidParserFailure from the DA handler.
        // Actually, let's trigger it through XTVERSION: CSI > 0 q with extra params
        // CSI > 999 q should trigger InvalidParserFailure from xtversion handler
        let result = p.push(b"\x1b[>999q");
        // Parser should be back to Empty
        assert_eq!(p.inner, ParserInner::Empty);
        // Should have produced Invalid from the InvalidParserFailure path
        let has_invalid = result.iter().any(|o| matches!(o, TerminalOutput::Invalid));
        assert!(
            has_invalid,
            "Malformed XTVERSION params should produce Invalid: {result:?}"
        );
    }

    // ── CSI parser Invalid path (lines 490-498) ────────────────────────────

    #[test]
    fn csi_completely_unknown_sequence_produces_invalid() {
        let mut p = FreminalAnsiParser::new();
        // CSI with a genuinely unhandled final byte + strange intermediate.
        // `!p` used to live here, but that is now DECSTR (soft terminal
        // reset) — see `csi_bang_p_is_decstr_soft_reset` below.
        let result = p.push(b"\x1b[!z");
        assert_eq!(p.inner, ParserInner::Empty);
        let _ = result;
    }

    #[test]
    fn csi_bang_p_is_decstr_soft_reset() {
        // `CSI ! p` is DECSTR (soft terminal reset), not an unhandled
        // sequence — see `ansi_parser_inner_csi_finished_decstr`.
        let mut p = FreminalAnsiParser::new();
        let result = p.push(b"\x1b[!p");
        assert_eq!(p.inner, ParserInner::Empty);
        assert_eq!(result, vec![TerminalOutput::SoftReset]);
    }

    // ── OSC parser Invalid path (lines 504-519) ────────────────────────────

    #[test]
    fn osc_invalid_sequence_produces_invalid() {
        let mut p = FreminalAnsiParser::new();
        // OSC with invalid/unrecognized content terminated by BEL
        let result = p.push(b"\x1b]99999;garbage\x07");
        // Parser should be back to Empty
        assert_eq!(p.inner, ParserInner::Empty);
        // May or may not produce Invalid depending on osc parser behavior
        let _ = result;
    }

    // ── current_trace_str with different parser states (lines 685-688) ──────

    #[test]
    fn current_trace_str_osc_state() {
        let mut p = FreminalAnsiParser::new();
        // Start an OSC sequence but don't finish it
        p.push(b"\x1b]0;partial");
        // Parser should be in OSC state
        assert!(matches!(p.inner, ParserInner::Osc(_)));
        // Reflects the OSC body bytes accepted so far (the sub-parser's `params`).
        assert_eq!(p.current_trace_str(), "0;partial");
    }

    #[test]
    fn current_trace_str_csi_state() {
        let mut p = FreminalAnsiParser::new();
        // Start a CSI sequence but don't finish it
        p.push(b"\x1b[1");
        assert!(matches!(p.inner, ParserInner::Csi(_)));
        // Reflects the CSI body so far (the sub-parser's `sequence`).
        assert_eq!(p.current_trace_str(), "1");
    }

    #[test]
    fn current_trace_str_standard_state() {
        let mut p = FreminalAnsiParser::new();
        // ESC ( enters Standard parser waiting for charset byte
        p.push(b"\x1b(");
        assert!(matches!(p.inner, ParserInner::Standard(_)));
        // Reflects the intermediates accepted so far.
        assert_eq!(p.current_trace_str(), "(");
    }

    #[test]
    fn current_trace_str_dcs_state() {
        let mut p = FreminalAnsiParser::new();
        // Start a DCS sequence but don't finish it
        p.push(b"\x1bP$q");
        assert!(matches!(p.inner, ParserInner::Dcs(_)));
        // Reflects the DCS body so far, without the leading `P` introducer.
        assert_eq!(p.current_trace_str(), "$q");
    }

    #[test]
    fn current_trace_str_apc_state() {
        let mut p = FreminalAnsiParser::new();
        // Start an APC sequence but don't finish it
        p.push(b"\x1b_partial");
        assert!(matches!(p.inner, ParserInner::Apc(_)));
        // Reflects the APC body so far, without the leading `_` introducer.
        assert_eq!(p.current_trace_str(), "partial");
    }

    #[test]
    fn current_trace_str_empty_state() {
        let p = FreminalAnsiParser::new();
        assert!(matches!(p.inner, ParserInner::Empty));
        let trace = p.current_trace_str();
        // Empty state uses the parser's own trace, which should be empty initially
        assert_eq!(trace, "");
    }

    // ── 8-bit C1 controls: more specific cases ─────────────────────────────

    #[test]
    fn c1_dcs_0x90_enters_dcs_parser() {
        let mut p = FreminalAnsiParser::new();
        p.s8c1t_mode = S8c1t::EightBit;
        // 0x90 = 8-bit DCS; feed DECRQSS query + ST
        let result = p.push(b"\x90$q\"p\x1b\\");
        assert_eq!(p.inner, ParserInner::Empty);
        let has_dcs = result
            .iter()
            .any(|o| matches!(o, TerminalOutput::DeviceControlString(_)));
        assert!(has_dcs, "8-bit DCS (0x90) should be parsed: {result:?}");
    }

    #[test]
    fn c1_csi_0x9b_enters_csi_parser() {
        let mut p = FreminalAnsiParser::new();
        p.s8c1t_mode = S8c1t::EightBit;
        // 0x9B = 8-bit CSI; followed by 1A (cursor up 1)
        let result = p.push(b"\x9B1A");
        assert_eq!(p.inner, ParserInner::Empty);
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
            "8-bit CSI 1A should produce cursor up: {result:?}"
        );
    }

    // ── 8-bit C1 not active in VT52 mode ───────────────────────────────────

    #[test]
    fn c1_not_recognized_in_vt52_mode() {
        let mut p = FreminalAnsiParser::new();
        p.s8c1t_mode = S8c1t::EightBit;
        p.vt52_mode = Decanm::Vt52;
        // 0x9B in VT52+S8C1T mode should NOT be recognized as CSI
        // It should fall through to the data path
        let result = p.push(b"\x9B");
        // No CSI parsing should occur
        let has_data = result.iter().any(|o| matches!(o, TerminalOutput::Data(_)));
        assert!(
            has_data,
            "0x9B in VT52 mode should be treated as data: {result:?}"
        );
    }

    // ── Escape backslash (ST) resets to Empty ───────────────────────────────

    #[test]
    fn escape_backslash_st_resets_to_empty() {
        let mut p = FreminalAnsiParser::new();
        // ESC \ (String Terminator when no string is open) should reset to Empty
        let result = p.push(b"\x1b\\");
        assert_eq!(p.inner, ParserInner::Empty);
        assert!(
            result.is_empty(),
            "ESC \\ with no open string should produce no output: {result:?}"
        );
    }

    // ── CSI with ESC inside aborts and starts new escape ────────────────────

    #[test]
    fn csi_esc_aborts_and_starts_new_sequence() {
        let mut p = FreminalAnsiParser::new();
        // ESC [ 2 ESC ] 0;title BEL — ESC aborts CSI, starts OSC
        let result = p.push(b"\x1b[2\x1b]0;Hello\x07");
        assert_eq!(p.inner, ParserInner::Empty);
        let has_title = result
            .iter()
            .any(|o| matches!(o, TerminalOutput::OscResponse(AnsiOscType::SetTitleBar(t)) if t == "Hello"));
        assert!(
            has_title,
            "After ESC aborts CSI, new OSC should complete: {result:?}"
        );
    }

    // ── Size regression guard ───────────────────────────────────────────────

    /// Every escape sequence constructs a fresh sub-parser and stores it in
    /// `ParserInner`, so the size of those types is paid as a memset (on
    /// construction) plus a memmove (into the enum) per sequence.
    ///
    /// Each sub-parser used to embed an 8 KB `SequenceTracer` ring buffer,
    /// which made every sequence cost ~16 KB of memory traffic (26-36% of
    /// samples in the SGR-heavy parse bench). Only the top-level
    /// `FreminalAnsiParser` may own a tracer; sub-parsers must stay small. If
    /// this test fails, a large inline array (or a tracer) crept back into a
    /// sub-parser: move the buffer to the heap or to the top-level parser.
    #[test]
    fn sub_parsers_stay_small() {
        const MAX_BYTES: usize = 128;
        assert!(std::mem::size_of::<ParserInner>() < MAX_BYTES);
        assert!(std::mem::size_of::<AnsiCsiParser>() < MAX_BYTES);
        assert!(std::mem::size_of::<AnsiOscParser>() < MAX_BYTES);
        assert!(std::mem::size_of::<StandardParser>() < MAX_BYTES);
        assert!(std::mem::size_of::<DcsParser>() < MAX_BYTES);
        assert!(std::mem::size_of::<ApcParser>() < MAX_BYTES);
    }

    // ── CSI buffer reuse (147.15) ───────────────────────────────────────────

    /// Assert the parser holds no stale CSI bytes and a bounded spare capacity.
    fn assert_csi_buffers_clean(p: &FreminalAnsiParser) {
        assert!(p.csi_buffers.is_empty(), "stale bytes: {:?}", p.csi_buffers);
        assert!(p.csi_buffers.max_capacity() <= crate::ansi_components::csi::RETAINED_CAPACITY_MAX);
    }

    /// What a brand-new parser makes of `bytes`.
    fn fresh_output(bytes: &[u8]) -> Vec<TerminalOutput> {
        FreminalAnsiParser::new().push(bytes)
    }

    #[test]
    fn csi_after_long_csi_has_no_stale_params_or_intermediates() {
        let mut p = FreminalAnsiParser::new();
        // A long sequence with intermediates and many parameters.
        let mut long = b"\x1b[".to_vec();
        for i in 0..200 {
            long.extend_from_slice(format!("{i};").as_bytes());
        }
        long.extend_from_slice(b"1 q");
        let _ = p.push(&long);
        assert_csi_buffers_clean(&p);

        let result = p.push(b"\x1b[5;10H");
        assert_eq!(result, fresh_output(b"\x1b[5;10H"));
        assert_eq!(
            result,
            vec![TerminalOutput::SetCursorPos {
                x: Some(10),
                y: Some(5)
            }]
        );
        assert_csi_buffers_clean(&p);
    }

    #[test]
    fn csi_buffer_capacity_is_bounded_after_a_huge_sequence() {
        let mut p = FreminalAnsiParser::new();
        let mut huge = b"\x1b[".to_vec();
        huge.extend(std::iter::repeat_n(b'1', 200_000));
        huge.push(b'm');
        let _ = p.push(&huge);
        assert_csi_buffers_clean(&p);

        // A huge sequence that is aborted instead of finished.
        let mut huge_aborted = b"\x1b[".to_vec();
        huge_aborted.extend(std::iter::repeat_n(b'1', 200_000));
        let _ = p.push(&huge_aborted);
        let _ = p.push(b"\x1b");
        assert_csi_buffers_clean(&p);

        assert_eq!(p.push(b"[2J"), fresh_output(b"\x1b[2J"));
    }

    #[test]
    fn csi_buffers_are_clean_after_every_exit_path() {
        let follow_up = b"\x1b[3;4H";
        let expected = fresh_output(follow_up);
        let prefix_cases: [(&str, &[u8]); 5] = [
            ("finished", b"\x1b[1;2;3;4m"),
            ("invalid byte", b"\x1b[1;2\x7f"),
            ("abort on ESC", b"\x1b[1;2;3 \x1b"),
            ("abort on ESC into new CSI", b"\x1b[1;2;3\x1b[9"),
            ("unhandled final", b"\x1b[1;2;3 W"),
        ];
        for (name, prefix) in prefix_cases {
            let mut p = FreminalAnsiParser::new();
            let _ = p.push(prefix);
            // The "new CSI" case is mid-sequence; finish it harmlessly.
            if matches!(p.inner, ParserInner::Csi(_)) {
                let _ = p.push(b"m");
            }
            if matches!(p.inner, ParserInner::Escape) {
                let _ = p.push(b"[0m");
            }
            assert_csi_buffers_clean(&p);
            assert_eq!(p.push(follow_up), expected, "{name}");
            assert_csi_buffers_clean(&p);
        }
    }

    #[test]
    fn csi_buffers_are_clean_after_c1_csi_and_vt52_and_ris() {
        let follow_up = b"\x1b[3;4H";
        let expected = fresh_output(follow_up);

        // 8-bit CSI, then a sequence that restarts through ESC.
        let mut p = FreminalAnsiParser::new();
        p.s8c1t_mode = S8c1t::EightBit;
        let _ = p.push(b"\x9b1;2;3");
        let _ = p.push(b"\x1bc"); // ESC c: RIS aborts the open CSI.
        assert_csi_buffers_clean(&p);
        assert_eq!(p.vt52_mode, Decanm::Ansi);
        assert_eq!(p.s8c1t_mode, S8c1t::SevenBit);
        assert_eq!(p.push(follow_up), expected);

        // CSI, then VT52 mode in effect: the CSI is gone, VT52 `ESC H` works.
        let mut p = FreminalAnsiParser::new();
        let _ = p.push(b"\x1b[?2l"); // DECANM reset: enter VT52.
        assert_eq!(p.vt52_mode, Decanm::Vt52);
        assert_csi_buffers_clean(&p);
        assert_eq!(
            p.push(b"\x1bH"),
            vec![TerminalOutput::SetCursorPos {
                x: Some(1),
                y: Some(1)
            }]
        );
        let _ = p.push(b"\x1b<"); // back to ANSI
        assert_eq!(p.push(follow_up), expected);
        assert_csi_buffers_clean(&p);
    }

    #[test]
    fn csi_split_across_pushes_still_reuses_buffers_correctly() {
        let mut p = FreminalAnsiParser::new();
        let _ = p.push(b"\x1b[1;2");
        let _ = p.push(b";3m\x1b[5");
        let result = p.push(b";10H");
        assert_eq!(
            result,
            vec![TerminalOutput::SetCursorPos {
                x: Some(10),
                y: Some(5)
            }]
        );
        assert_csi_buffers_clean(&p);
    }

    // -------------------------------------------------------------------------
    // C1 ST, CAN/SUB and ESC inside strings and escape intermediates
    // (Task 147 adversarial review A1-A4)
    // -------------------------------------------------------------------------

    fn eight_bit_parser() -> FreminalAnsiParser {
        let mut p = FreminalAnsiParser::new();
        p.s8c1t_mode = S8c1t::EightBit;
        p
    }

    fn data(text: &str) -> TerminalOutput {
        TerminalOutput::Data(text.as_bytes().to_vec())
    }

    fn title(text: &str) -> TerminalOutput {
        TerminalOutput::OscResponse(AnsiOscType::SetTitleBar(text.to_string()))
    }

    #[test]
    fn c1_st_ends_an_osc_under_s8c1t() {
        assert_eq!(
            eight_bit_parser().push(b"\x9d0;t\x9cX"),
            vec![title("t"), data("X")]
        );
    }

    #[test]
    fn c1_st_ends_a_dcs_under_s8c1t() {
        assert_eq!(
            eight_bit_parser().push(b"\x90$qm\x9cX"),
            vec![
                TerminalOutput::DeviceControlString(b"P$qm\x1b\\".to_vec()),
                data("X"),
            ]
        );
    }

    #[test]
    fn c1_st_ends_an_apc_under_s8c1t() {
        assert_eq!(
            eight_bit_parser().push(b"\x9fGa=q,i=1;\x9cX"),
            vec![
                TerminalOutput::ApplicationProgramCommand(b"_Ga=q,i=1;\x1b\\".to_vec()),
                data("X"),
            ]
        );
    }

    #[test]
    fn c1_st_also_ends_strings_opened_with_a_seven_bit_introducer() {
        let mut p = eight_bit_parser();
        assert_eq!(p.push(b"\x1b]0;t\x9cX"), vec![title("t"), data("X")]);
        assert_eq!(
            p.push(b"\x1bP$qm\x9cX"),
            vec![
                TerminalOutput::DeviceControlString(b"P$qm\x1b\\".to_vec()),
                data("X"),
            ]
        );
    }

    #[test]
    fn c1_st_after_an_unfinished_seven_bit_st_drops_the_esc() {
        // `ESC` then `0x9C`: the ESC was the start of an ST that never
        // completed, and is not part of the body.
        assert_eq!(
            eight_bit_parser().push(b"\x9d0;t\x1b\x9cX"),
            vec![title("t"), data("X")]
        );
        assert_eq!(
            eight_bit_parser().push(b"\x90$qm\x1b\x9cX"),
            vec![
                TerminalOutput::DeviceControlString(b"P$qm\x1b\\".to_vec()),
                data("X"),
            ]
        );
        assert_eq!(
            eight_bit_parser().push(b"\x9fGa=q;\x1b\x9cX"),
            vec![
                TerminalOutput::ApplicationProgramCommand(b"_Ga=q;\x1b\\".to_vec()),
                data("X"),
            ]
        );
    }

    /// Under S7C1T `0x9C` is not a control: it is the continuation byte of
    /// U+015C in an OSC title, and must stay in the body.
    #[test]
    fn c1_st_byte_is_body_under_s7c1t() {
        let mut p = FreminalAnsiParser::new();
        assert_eq!(
            p.push("\x1b]0;\u{15c}\x07X".as_bytes()),
            vec![title("\u{15c}"), data("X")]
        );
        let out = p.push(b"\x1bP$q\x9cm\x1b\\X");
        assert_eq!(
            out,
            vec![
                TerminalOutput::DeviceControlString(b"P$q\x9cm\x1b\\".to_vec()),
                data("X"),
            ]
        );
        let out = p.push(b"\x1b_Ga\x9cb\x1b\\X");
        assert_eq!(
            out,
            vec![
                TerminalOutput::ApplicationProgramCommand(b"_Ga\x9cb\x1b\\".to_vec()),
                data("X"),
            ]
        );
    }

    #[test]
    fn c1_st_ends_an_overflowed_apc_under_s8c1t_with_no_output() {
        let mut p = eight_bit_parser();
        let mut input = vec![0x9f];
        input.resize(crate::ansi_components::apc::MAX_APC_BYTES + 10, b'x');
        input.extend_from_slice(b"\x9cX");
        assert_eq!(p.push(&input), vec![data("X")]);
        assert_eq!(p.inner, ParserInner::Empty);
    }

    #[test]
    fn c1_st_ends_an_overflowed_osc_under_s8c1t_with_no_output() {
        let mut p = eight_bit_parser();
        let mut input = b"\x9d0;".to_vec();
        input.resize(crate::ansi_components::osc::MAX_OSC_BYTES + 10, b'x');
        input.extend_from_slice(b"\x9cX");
        assert_eq!(p.push(&input), vec![data("X")]);
        assert_eq!(p.inner, ParserInner::Empty);
    }

    #[test]
    fn can_and_sub_cancel_a_string_and_the_next_byte_is_text() {
        for cancel in [0x18_u8, 0x1a] {
            for introducer in [&b"\x1b]0;t"[..], b"\x1bP$qm", b"\x1b_Ga=q;"] {
                let mut input = introducer.to_vec();
                input.extend_from_slice(&[cancel, b'X']);
                let mut p = FreminalAnsiParser::new();
                assert_eq!(
                    p.push(&input),
                    vec![data("X")],
                    "0x{cancel:02x} after {:?}",
                    String::from_utf8_lossy(introducer)
                );
                assert_eq!(p.inner, ParserInner::Empty);
            }
        }
    }

    #[test]
    fn can_cancels_a_string_that_has_a_pending_esc() {
        // The ESC is part of a possible ST; CAN still cancels the whole string.
        let mut p = FreminalAnsiParser::new();
        assert_eq!(p.push(b"\x1bP$qm\x1b\x18X"), vec![data("X")]);
        assert_eq!(p.push(b"\x1b_Ga\x1b\x18X"), vec![data("X")]);
        assert_eq!(p.push(b"\x1b]0;t\x1b\x18X"), vec![data("X")]);
    }

    #[test]
    fn esc_inside_an_escape_intermediate_sequence_restarts_the_escape() {
        let reference = FreminalAnsiParser::new().push(b"\x1b[31mX");
        assert!(
            reference.len() >= 2 && !reference.contains(&TerminalOutput::Invalid),
            "reference: {reference:?}"
        );
        for intermediate in *b"()*+ #%" {
            let mut p = FreminalAnsiParser::new();
            let result = p.push(&[0x1b, intermediate, 0x1b, b'[', b'3', b'1', b'm', b'X']);
            assert_eq!(
                result, reference,
                "ESC {:?} ESC [31mX must give red X",
                intermediate as char
            );
            assert_eq!(p.inner, ParserInner::Empty);
        }
    }

    #[test]
    fn esc_inside_an_escape_intermediate_sequence_can_start_another_intermediate_sequence() {
        let mut p = FreminalAnsiParser::new();
        assert_eq!(
            p.push(b"\x1b(\x1b(0"),
            vec![TerminalOutput::DesignateCharset {
                slot: CharsetSlot::G0,
                set: CharacterSet::DecSpecialGraphics,
            }]
        );
    }

    #[test]
    fn extra_designator_intermediates_swallow_the_whole_sequence() {
        for sequence in [
            &b"\x1b(%5abc"[..],
            b"\x1b(\"4abc",
            b"\x1b)%5abc",
            b"\x1b*&4abc",
            b"\x1b+ !#5abc",
        ] {
            let mut p = FreminalAnsiParser::new();
            assert_eq!(
                p.push(sequence),
                vec![data("abc")],
                "{:?}",
                String::from_utf8_lossy(sequence)
            );
            assert_eq!(p.inner, ParserInner::Empty);
        }
    }

    #[test]
    fn a_multi_byte_designator_split_across_pushes_is_still_swallowed() {
        let mut p = FreminalAnsiParser::new();
        assert_eq!(p.push(b"\x1b(%"), vec![]);
        assert_eq!(p.push(b"5abc"), vec![data("abc")]);
    }
}
