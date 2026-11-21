// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use freminal_common::buffer_states::terminal_output::TerminalOutput;

use super::csi_dispatch::{dispatch_csi, warn_unhandled};
use super::csi_key::CsiKey;
use crate::ansi::ParserOutcome;
use crate::ansi_components::tracer::{escape_sequence_for_log, lossy_sequence_for_log_bounded};

#[derive(Eq, PartialEq, Debug, Default)]
pub(crate) enum AnsiCsiParserState {
    #[default]
    Params,
    Intermediates,
    Finished(u8),
    Invalid,
    InvalidFinished,
    /// The sequence went over [`MAX_CSI_BYTES`] while in the parameter bytes;
    /// its buffers were released and the rest is discarded through its final
    /// byte.
    OverflowParams,
    /// As [`Self::OverflowParams`], after the first intermediate byte.
    OverflowIntermediates,
}
/// Which Control Sequence Introducer began this CSI sequence.
///
/// The 7-bit `ESC [` form and the 8-bit `0x9B` (C1) form are equivalent
/// grammatically, but for lossless diagnostic logging they must be
/// reconstructed distinctly: the sub-parser never receives the introducer
/// bytes, so it records which one was used at construction time.
#[derive(Eq, PartialEq, Debug, Default, Clone, Copy)]
pub enum CsiIntroducer {
    /// 7-bit CSI: `ESC [` (`0x1B 0x5B`).
    #[default]
    SevenBit,
    /// 8-bit CSI: `0x9B` (C1).
    EightBit,
}

impl CsiIntroducer {
    /// The raw introducer bytes as they appear on the wire.
    const fn bytes(self) -> &'static [u8] {
        match self {
            Self::SevenBit => b"\x1b[",
            Self::EightBit => b"\x9b",
        }
    }
}

/// Maximum size of a CSI sequence's body, in bytes (parameters, intermediates
/// and final byte, excluding the introducer).
///
/// A CSI sequence is short: real ones are well under a kilobyte (xterm keeps
/// at most 30 parameters). The cap exists because an open sequence otherwise
/// grows its buffers for as long as the stream sends parameter bytes. One more
/// byte than this and the buffers are released; the sequence is then discarded
/// through its final byte (or an aborting ESC) without being dispatched.
pub const MAX_CSI_BYTES: usize = 64 * 1024;

/// Initial capacity of the parameter buffer.
const PARAMS_CAPACITY: usize = 8;
/// Initial capacity of the intermediate-byte buffer.
const INTERMEDIATES_CAPACITY: usize = 4;
/// Initial capacity of the whole-body buffer.
const SEQUENCE_CAPACITY: usize = 16;
/// Capacity a recycled buffer is shrunk back to when a sequence left it
/// larger than [`RETAINED_CAPACITY_MAX`]. A pathological sequence (a
/// megabyte of parameter bytes) must not pin that memory for the life of the
/// parser; ordinary CSI sequences are well under 64 bytes.
pub(crate) const RETAINED_CAPACITY_MAX: usize = 1024;

/// The three per-sequence byte buffers of a CSI parser.
///
/// The top-level parser owns one set and lends it to each CSI sub-parser as
/// the sequence starts, taking it back when the sequence ends, so a stream of
/// short sequences (an SGR-heavy TUI) allocates nothing per sequence. A
/// recycled set is always empty.
#[derive(Eq, PartialEq, Debug, Default)]
pub(crate) struct CsiBuffers {
    params: Vec<u8>,
    intermediates: Vec<u8>,
    sequence: Vec<u8>,
}

impl CsiBuffers {
    pub(crate) const fn new() -> Self {
        Self {
            params: Vec::new(),
            intermediates: Vec::new(),
            sequence: Vec::new(),
        }
    }

    /// True when no buffer holds a byte.
    #[cfg(test)]
    pub(crate) const fn is_empty(&self) -> bool {
        self.params.is_empty() && self.intermediates.is_empty() && self.sequence.is_empty()
    }

    /// The largest capacity of the three buffers.
    #[cfg(test)]
    pub(crate) fn max_capacity(&self) -> usize {
        self.params
            .capacity()
            .max(self.intermediates.capacity())
            .max(self.sequence.capacity())
    }

    /// Empty every buffer and release the excess if one grew past
    /// [`RETAINED_CAPACITY_MAX`].
    fn recycle(&mut self) {
        for (buf, initial) in [
            (&mut self.params, PARAMS_CAPACITY),
            (&mut self.intermediates, INTERMEDIATES_CAPACITY),
            (&mut self.sequence, SEQUENCE_CAPACITY),
        ] {
            buf.clear();
            if buf.capacity() > RETAINED_CAPACITY_MAX {
                buf.shrink_to(initial);
            }
        }
    }

    /// A set pre-sized for one ordinary sequence, for a parser built outside
    /// the top-level parser. The top-level parser's own spare set starts
    /// empty and simply keeps whatever capacity its first sequence grows.
    fn with_initial_capacity() -> Self {
        Self {
            params: Vec::with_capacity(PARAMS_CAPACITY),
            intermediates: Vec::with_capacity(INTERMEDIATES_CAPACITY),
            sequence: Vec::with_capacity(SEQUENCE_CAPACITY),
        }
    }
}

#[derive(Eq, PartialEq, Debug, Default)]
pub struct AnsiCsiParser {
    pub(crate) state: AnsiCsiParserState,
    pub params: Vec<u8>,
    pub intermediates: Vec<u8>,
    pub sequence: Vec<u8>,
    /// The introducer that began this sequence, for lossless diagnostics.
    introducer: CsiIntroducer,
}

impl AnsiCsiParser {
    /// Construct a parser for a 7-bit (`ESC [`) CSI sequence.
    #[must_use]
    pub fn new() -> Self {
        Self::with_introducer(CsiIntroducer::SevenBit, CsiBuffers::with_initial_capacity())
    }

    /// Construct a parser for an 8-bit (`0x9B`, C1) CSI sequence.
    #[must_use]
    pub fn new_c1() -> Self {
        Self::with_introducer(CsiIntroducer::EightBit, CsiBuffers::with_initial_capacity())
    }

    /// Construct a parser for a sequence started by `introducer`, reusing the
    /// recycled `buffers` handed back by [`Self::take_buffers`].
    #[must_use]
    pub(crate) fn from_buffers(introducer: CsiIntroducer, buffers: CsiBuffers) -> Self {
        Self::with_introducer(introducer, buffers)
    }

    #[must_use]
    fn with_introducer(introducer: CsiIntroducer, buffers: CsiBuffers) -> Self {
        Self {
            state: AnsiCsiParserState::Params,
            params: buffers.params,
            intermediates: buffers.intermediates,
            sequence: buffers.sequence,
            introducer,
        }
    }

    /// Take the byte buffers out of this parser, emptied and with any
    /// oversized capacity released, ready to start the next sequence.
    ///
    /// Every exit from the CSI state (finish, invalid, abort on ESC) hands
    /// the buffers back through here, so no byte of a previous sequence can
    /// reach the next one.
    pub(crate) fn take_buffers(&mut self) -> CsiBuffers {
        let mut buffers = CsiBuffers {
            params: std::mem::take(&mut self.params),
            intermediates: std::mem::take(&mut self.intermediates),
            sequence: std::mem::take(&mut self.sequence),
        };
        buffers.recycle();
        buffers
    }

    /// Expose the current sequence for testing and diagnostics.
    ///
    /// Rendered from [`Self::sequence`] (the CSI body bytes pushed so far,
    /// without the introducer), lossily decoded and bounded to
    /// [`crate::ansi_components::tracer::LOG_SEQUENCE_MAX_BYTES`].
    #[must_use]
    pub fn trace_str(&self) -> String {
        lossy_sequence_for_log_bounded(&self.sequence)
    }

    /// Render the full raw CSI sequence — including the reconstructed CSI
    /// introducer that the sub-parser never receives — as a
    /// reconstruction-faithful escaped string for diagnostics.
    ///
    /// The body is [`Self::sequence`], the accumulated CSI body bytes
    /// (parameters, intermediates, and the final byte). The actual introducer
    /// (`ESC [` for 7-bit, `0x9B` for 8-bit C1) is prepended so the logged form
    /// is the complete sequence a terminal would send.
    #[must_use]
    fn format_raw_csi(&self) -> String {
        let introducer = self.introducer.bytes();
        let mut full = Vec::with_capacity(self.sequence.len().saturating_add(introducer.len()));
        full.extend_from_slice(introducer);
        full.extend_from_slice(&self.sequence);
        escape_sequence_for_log(&full)
    }

    /// Push a byte into the parser
    ///
    /// # Errors
    /// Will return an error if the parser is in a finished state
    #[tracing::instrument(level = "trace", skip_all)]
    pub fn push(&mut self, b: u8) -> ParserOutcome {
        match self.state {
            AnsiCsiParserState::Finished(_) | AnsiCsiParserState::InvalidFinished => {
                return ParserOutcome::Invalid("Parser pushed to once finished".to_string());
            }
            AnsiCsiParserState::OverflowParams | AnsiCsiParserState::OverflowIntermediates => {
                return self.push_overflowed(b);
            }
            AnsiCsiParserState::Params
            | AnsiCsiParserState::Intermediates
            | AnsiCsiParserState::Invalid => {}
        }

        if self.sequence.len() >= MAX_CSI_BYTES {
            self.overflow();
            return self.push_overflowed(b);
        }

        self.sequence.push(b);

        match &mut self.state {
            AnsiCsiParserState::Params => {
                if is_csi_param(b) {
                    self.params.push(b);
                    return ParserOutcome::Continue;
                } else if is_csi_intermediate(b) {
                    self.intermediates.push(b);
                    self.state = AnsiCsiParserState::Intermediates;
                    return ParserOutcome::Continue;
                } else if is_csi_terminator(b) {
                    self.state = AnsiCsiParserState::Finished(b);
                    return ParserOutcome::Finished;
                }

                self.state = AnsiCsiParserState::Invalid;

                ParserOutcome::Invalid("Invalid CSI parameter".to_string())
            }
            AnsiCsiParserState::Intermediates => {
                if is_csi_param(b) {
                    self.state = AnsiCsiParserState::Invalid;

                    return ParserOutcome::Invalid("Invalid CSI intermediate".to_string());
                } else if is_csi_intermediate(b) {
                    self.intermediates.push(b);
                    return ParserOutcome::Continue;
                } else if is_csi_terminator(b) {
                    self.state = AnsiCsiParserState::Finished(b);
                    return ParserOutcome::Finished;
                }

                self.state = AnsiCsiParserState::Invalid;

                ParserOutcome::Invalid("Invalid CSI intermediate".to_string())
            }
            AnsiCsiParserState::Invalid => {
                if is_csi_terminator(b) {
                    self.state = AnsiCsiParserState::InvalidFinished;
                }

                ParserOutcome::Invalid("Invalid CSI sequence".to_string())
            }
            AnsiCsiParserState::Finished(_)
            | AnsiCsiParserState::InvalidFinished
            | AnsiCsiParserState::OverflowParams
            | AnsiCsiParserState::OverflowIntermediates => {
                // Guarded by the caller, but surface explicitly as an invalid
                // outcome rather than panicking if the invariant ever breaks.
                ParserOutcome::Invalid("CSI parser received byte after termination".to_string())
            }
        }
    }

    /// The sequence just went over [`MAX_CSI_BYTES`]: release its buffers and
    /// switch to discarding. One warning (the cap, never the payload) is
    /// logged.
    fn overflow(&mut self) {
        tracing::warn!("CSI sequence over the {MAX_CSI_BYTES}-byte cap dropped");
        self.params = Vec::new();
        self.intermediates = Vec::new();
        self.sequence = Vec::new();
        self.state = match self.state {
            AnsiCsiParserState::Intermediates => AnsiCsiParserState::OverflowIntermediates,
            _ => AnsiCsiParserState::OverflowParams,
        };
    }

    /// Consume one byte of an overflowed sequence, with the same grammar as
    /// [`Self::push`]: the final byte ends it (with nothing dispatched), and a
    /// byte that cannot be part of a CSI sequence is invalid.
    fn push_overflowed(&mut self, b: u8) -> ParserOutcome {
        let after_intermediate = self.state == AnsiCsiParserState::OverflowIntermediates;
        if is_csi_terminator(b) {
            self.state = AnsiCsiParserState::InvalidFinished;
            ParserOutcome::Finished
        } else if is_csi_intermediate(b) {
            self.state = AnsiCsiParserState::OverflowIntermediates;
            ParserOutcome::Continue
        } else if is_csi_param(b) && !after_intermediate {
            ParserOutcome::Continue
        } else {
            self.state = AnsiCsiParserState::Invalid;
            ParserOutcome::Invalid("Invalid CSI sequence".to_string())
        }
    }

    /// Push a byte into the parser and return the next state
    ///
    /// When the byte completes the sequence, the sequence is classified into a
    /// `CsiKey` and routed by `csi_dispatch::dispatch_csi`. A sequence whose private
    /// marker is misplaced cannot be classified and is treated like any other
    /// recognised-but-unhandled sequence: logged, no output.
    ///
    /// # Errors
    /// Will return an error if the parser encounters an invalid state
    #[tracing::instrument(level = "trace", skip_all)]
    pub fn ansiparser_inner_csi(
        &mut self,
        b: u8,
        output: &mut Vec<TerminalOutput>,
    ) -> ParserOutcome {
        let push_result = self.push(b);

        // Anything that is not a finished sequence (Continue, Invalid, ...)
        // is reported exactly as `push` reported it.
        let AnsiCsiParserState::Finished(final_byte) = self.state else {
            return push_result;
        };

        let raw = || self.format_raw_csi();

        let Ok(key) = CsiKey::classify(&self.params, &self.intermediates, final_byte) else {
            warn_unhandled(&raw);
            return ParserOutcome::Finished;
        };

        dispatch_csi(key, &self.params, &raw, output)
    }
}

fn is_csi_param(b: u8) -> bool {
    (0x30..=0x3f).contains(&b)
}

fn is_csi_terminator(b: u8) -> bool {
    (0x40..=0x7e).contains(&b)
}

fn is_csi_intermediate(b: u8) -> bool {
    (0x20..=0x2f).contains(&b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use freminal_common::buffer_states::mode::Mode;
    use freminal_common::buffer_states::modes::{
        decckm::Decckm,
        mouse::{MouseEncoding, MouseTrack},
        rl_bracket::RlBracket,
        xtextscrn::XtExtscrn,
    };

    /// Helper: feed a full CSI sequence (everything after ESC[) into the parser
    /// and return the collected `TerminalOutput` vec.
    fn parse_csi_sequence(bytes: &[u8]) -> Vec<TerminalOutput> {
        let mut parser = AnsiCsiParser::new();
        let mut output = Vec::new();
        for &b in bytes {
            parser.ansiparser_inner_csi(b, &mut output);
        }
        output
    }

    /// Extract `Mode` variants from a `Vec<TerminalOutput>`.
    fn extract_modes(outputs: &[TerminalOutput]) -> Vec<&Mode> {
        outputs
            .iter()
            .filter_map(|o| {
                if let TerminalOutput::Mode(m) = o {
                    Some(m)
                } else {
                    None
                }
            })
            .collect()
    }

    #[test]
    fn test_compound_mode_set_alternate_screen_and_bracketed_paste() {
        // ESC[?1049;2004h — set alternate screen AND bracketed paste
        let output = parse_csi_sequence(b"?1049;2004h");
        let modes = extract_modes(&output);
        assert_eq!(modes.len(), 2, "expected two modes, got {modes:?}");
        assert_eq!(
            *modes[0],
            Mode::XtExtscrn(XtExtscrn::Alternate),
            "first mode should be alternate screen"
        );
        assert_eq!(
            *modes[1],
            Mode::BracketedPaste(RlBracket::Enabled),
            "second mode should be bracketed paste enabled"
        );
    }

    #[test]
    fn test_compound_mode_set_alternate_screen_and_decckm() {
        // ESC[?1049;1h — set alternate screen AND DECCKM application mode
        let output = parse_csi_sequence(b"?1049;1h");
        let modes = extract_modes(&output);
        assert_eq!(modes.len(), 2, "expected two modes, got {modes:?}");
        assert_eq!(*modes[0], Mode::XtExtscrn(XtExtscrn::Alternate));
        assert_eq!(*modes[1], Mode::Decckm(Decckm::Application));
    }

    #[test]
    fn test_compound_mode_set_mouse_x11_and_sgr() {
        // ESC[?1000;1006h — set X11 mouse tracking AND SGR mouse encoding
        let output = parse_csi_sequence(b"?1000;1006h");
        let modes = extract_modes(&output);
        assert_eq!(modes.len(), 2, "expected two modes, got {modes:?}");
        assert_eq!(*modes[0], Mode::MouseMode(MouseTrack::XtMseX11));
        assert_eq!(*modes[1], Mode::MouseEncodingMode(MouseEncoding::Sgr));
    }

    #[test]
    fn test_compound_mode_reset() {
        // ESC[?1049;2004l — reset alternate screen AND bracketed paste
        let output = parse_csi_sequence(b"?1049;2004l");
        let modes = extract_modes(&output);
        assert_eq!(modes.len(), 2, "expected two modes, got {modes:?}");
        assert_eq!(*modes[0], Mode::XtExtscrn(XtExtscrn::Primary));
        assert_eq!(*modes[1], Mode::BracketedPaste(RlBracket::Disabled));
    }

    #[test]
    fn test_single_param_mode_set_unchanged() {
        // ESC[?1049h — single param, must still work
        let output = parse_csi_sequence(b"?1049h");
        let modes = extract_modes(&output);
        assert_eq!(modes.len(), 1);
        assert_eq!(*modes[0], Mode::XtExtscrn(XtExtscrn::Alternate));
    }

    #[test]
    fn test_single_param_mode_reset_unchanged() {
        // ESC[?2004l — single param reset
        let output = parse_csi_sequence(b"?2004l");
        let modes = extract_modes(&output);
        assert_eq!(modes.len(), 1);
        assert_eq!(*modes[0], Mode::BracketedPaste(RlBracket::Disabled));
    }

    #[test]
    fn test_non_dec_single_param_unchanged() {
        // ESC[20h — non-DEC single param (LNM)
        let output = parse_csi_sequence(b"20h");
        let modes = extract_modes(&output);
        assert_eq!(modes.len(), 1);
        assert_eq!(
            *modes[0],
            Mode::LineFeedMode(freminal_common::buffer_states::modes::lnm::Lnm::NewLine)
        );
    }

    #[test]
    fn test_three_params_compound() {
        // ESC[?1049;1;2004h — three params: alternate screen + DECCKM + bracketed paste
        let output = parse_csi_sequence(b"?1049;1;2004h");
        let modes = extract_modes(&output);
        assert_eq!(modes.len(), 3, "expected three modes, got {modes:?}");
        assert_eq!(*modes[0], Mode::XtExtscrn(XtExtscrn::Alternate));
        assert_eq!(*modes[1], Mode::Decckm(Decckm::Application));
        assert_eq!(*modes[2], Mode::BracketedPaste(RlBracket::Enabled));
    }

    // ── CSI s routing: SCOSC vs DECSLRM ────────────────────────────────

    #[test]
    fn csi_s_no_params_is_save_cursor() {
        // CSI s with no params → SCOSC (save cursor)
        let out = parse_csi_sequence(b"s");
        assert_eq!(out, vec![TerminalOutput::SaveCursor]);
    }

    #[test]
    fn csi_s_with_params_is_decslrm() {
        // CSI 5;10 s → DECSLRM
        let out = parse_csi_sequence(b"5;10s");
        assert_eq!(
            out,
            vec![TerminalOutput::SetLeftAndRightMargins {
                left_margin: 5,
                right_margin: 10,
            }]
        );
    }

    #[test]
    fn csi_s_with_single_param_is_decslrm() {
        // CSI 3 s → DECSLRM with right=MAX
        let out = parse_csi_sequence(b"3s");
        assert_eq!(
            out,
            vec![TerminalOutput::SetLeftAndRightMargins {
                left_margin: 3,
                right_margin: usize::MAX,
            }]
        );
    }

    #[test]
    fn test_empty_sub_params_skipped() {
        // ESC[?1049;;2004h — empty sub-param between semicolons should be skipped
        let output = parse_csi_sequence(b"?1049;;2004h");
        let modes = extract_modes(&output);
        assert_eq!(modes.len(), 2, "empty sub-params should be skipped");
        assert_eq!(*modes[0], Mode::XtExtscrn(XtExtscrn::Alternate));
        assert_eq!(*modes[1], Mode::BracketedPaste(RlBracket::Enabled));
    }

    // ── push() state-machine edge cases ─────────────────────────────────────

    #[test]
    fn csi_push_to_finished_parser_returns_invalid() {
        // Feed a complete sequence, then push another byte → should return Invalid.
        let mut parser = AnsiCsiParser::new();
        let mut output = Vec::new();
        // Complete with 'H' (CUP)
        parser.ansiparser_inner_csi(b'H', &mut output);
        // Now push again to a finished parser
        let result = parser.push(b'A');
        assert!(matches!(result, ParserOutcome::Invalid(_)));
    }

    #[test]
    fn csi_intermediate_then_param_byte_is_invalid() {
        // Transition: Params → Intermediates (on `!`) → then a param byte (digit `5`)
        // The second byte `5` is a CSI param byte (0x30–0x3f) arriving in Intermediates state
        // → should set state to Invalid.
        let mut parser = AnsiCsiParser::new();
        // Push an intermediate byte to enter Intermediates state
        let r1 = parser.push(b' '); // space (0x20) is a valid CSI intermediate
        assert_eq!(r1, ParserOutcome::Continue);
        assert_eq!(parser.state, AnsiCsiParserState::Intermediates);
        // Now push a param byte (digit) which is invalid in Intermediates state
        let r2 = parser.push(b'5');
        assert!(matches!(r2, ParserOutcome::Invalid(_)));
        assert_eq!(parser.state, AnsiCsiParserState::Invalid);
    }

    #[test]
    fn csi_invalid_state_terminator_sets_invalid_finished() {
        // Enter Invalid state then receive a terminator → InvalidFinished.
        let mut parser = AnsiCsiParser::new();
        // Push a non-param, non-intermediate, non-terminator byte (0x01) → Invalid
        let r1 = parser.push(0x01);
        assert!(matches!(r1, ParserOutcome::Invalid(_)));
        assert_eq!(parser.state, AnsiCsiParserState::Invalid);
        // Push a terminator → InvalidFinished
        let r2 = parser.push(b'H');
        assert!(matches!(r2, ParserOutcome::Invalid(_)));
        assert_eq!(parser.state, AnsiCsiParserState::InvalidFinished);
        // Push yet another byte to the finished-invalid parser → Invalid
        let r3 = parser.push(b'A');
        assert!(matches!(r3, ParserOutcome::Invalid(_)));
    }

    // ── Non-DEC multi-param mode split ──────────────────────────────────────

    #[test]
    fn csi_non_dec_multi_param_mode_set() {
        // ESC[20;4h — non-DEC private, multiple params (splits to two Mode outputs).
        // Each sub-param is dispatched without the `?` prefix.
        let output = parse_csi_sequence(b"20;4h");
        // Both should be Mode outputs (even if NoOp/Unknown)
        assert!(
            output.len() >= 2,
            "expected at least 2 outputs, got {output:?}"
        );
        assert!(output.iter().all(|o| matches!(o, TerminalOutput::Mode(_))));
    }

    // ── CSI > ... q: XTVERSION vs. `>`-prefixed sequences with intermediates ─

    #[test]
    fn xtversion_bare_gt_q_through_parser() {
        let output = parse_csi_sequence(b">q");
        assert_eq!(output, vec![TerminalOutput::RequestDeviceNameAndVersion]);
    }

    #[test]
    fn xtversion_gt0_q_through_parser() {
        let output = parse_csi_sequence(b">0q");
        assert_eq!(output, vec![TerminalOutput::RequestDeviceNameAndVersion]);
    }

    #[test]
    fn gt_prefix_with_intermediate_q_emits_nothing() {
        // `>`-prefixed + intermediate is not XTVERSION and not DECSCUSR; it is
        // recognised (kitty multiple-cursors) but unimplemented, so
        // the parser must emit no output at all.
        for seq in [&b">0;4 q"[..], b"> q", b">100 q", b">1;2:3:4 q", b">0;4$q"] {
            let mut parser = AnsiCsiParser::new();
            let mut output = Vec::new();
            let mut last = ParserOutcome::Continue;
            for &b in seq {
                last = parser.ansiparser_inner_csi(b, &mut output);
            }
            assert_eq!(last, ParserOutcome::Finished, "seq {seq:?}");
            assert_eq!(output, [], "seq {seq:?} must emit nothing");
        }
    }

    #[test]
    fn gt_prefix_without_intermediate_non_xtversion_params_is_rejected() {
        // `CSI > 0;4 q` (no space) is not XTVERSION; xtversion rejects it, so
        // no RequestDeviceNameAndVersion is emitted.
        let output = parse_csi_sequence(b">0;4q");
        assert!(
            !output.contains(&TerminalOutput::RequestDeviceNameAndVersion),
            "got {output:?}"
        );
    }

    #[test]
    fn decscusr_with_intermediate_still_dispatches() {
        // Unchanged path: no `>` prefix, `CSI 2 SP q` is DECSCUSR.
        let output = parse_csi_sequence(b"2 q");
        assert_eq!(output.len(), 1);
        assert!(matches!(output[0], TerminalOutput::CursorVisualStyle(_)));
    }

    // ── Unrecognized CSI final byte ──────────────────────────────────────────

    #[test]
    fn csi_unrecognized_final_byte_returns_push_result() {
        // A final byte that is not in the dispatch table (e.g. `w`) → falls through
        // to the `Finished(_esc) => push_result` arm and returns Finished.
        let mut parser = AnsiCsiParser::new();
        let mut output = Vec::new();
        // `w` (0x77) is a valid CSI terminator but not dispatched to any handler
        let result = parser.ansiparser_inner_csi(b'w', &mut output);
        assert_eq!(result, ParserOutcome::Finished);
        assert_eq!(output, []);
    }

    #[test]
    fn format_raw_csi_reconstructs_full_sequence() {
        // The CSI body accumulated by the parser excludes the `ESC [`
        // introducer; format_raw_csi must prepend it so the logged form is the
        // complete sequence a terminal would send.
        let mut parser = AnsiCsiParser::new();
        parser.sequence = b"38;5;9w".to_vec();
        assert_eq!(parser.format_raw_csi(), "\\x1b[38;5;9w");
        // Empty body still shows the introducer.
        let empty = AnsiCsiParser::new();
        assert_eq!(empty.format_raw_csi(), "\\x1b[");
    }

    #[test]
    fn format_raw_csi_preserves_c1_introducer() {
        // A sequence begun by the 8-bit C1 introducer (`0x9B`) must be logged
        // as `\x9b...`, not the 7-bit `\x1b[...` form, to stay lossless.
        let mut parser = AnsiCsiParser::new_c1();
        parser.sequence = b"38;5;9w".to_vec();
        assert_eq!(parser.format_raw_csi(), "\\x9b38;5;9w");
        // Empty body still shows the 8-bit introducer.
        let empty = AnsiCsiParser::new_c1();
        assert_eq!(empty.format_raw_csi(), "\\x9b");
    }

    #[test]
    fn unhandled_final_byte_accumulates_full_body_for_logging() {
        // Feed an unhandled-but-valid CSI (`ESC [ 1 ; 2 W`) one byte at a time
        // and confirm the parser's `sequence` field holds the complete body so
        // the diagnostic log can render it.
        let mut parser = AnsiCsiParser::new();
        let mut output = Vec::new();
        for &b in b"1;2" {
            let _ = parser.ansiparser_inner_csi(b, &mut output);
        }
        let result = parser.ansiparser_inner_csi(b'W', &mut output);
        assert_eq!(result, ParserOutcome::Finished);
        assert_eq!(output, []);
        assert_eq!(parser.sequence, b"1;2W");
        assert_eq!(parser.format_raw_csi(), "\\x1b[1;2W");
    }

    // ── Buffer reuse ────────────────────────────────────────────────────────

    #[test]
    fn take_buffers_returns_empty_buffers_and_leaves_parser_empty() {
        let mut parser = AnsiCsiParser::new();
        let mut output = Vec::new();
        for &b in b"1;2 " {
            let _ = parser.ansiparser_inner_csi(b, &mut output);
        }
        assert_ne!(parser.params.len(), 0);
        assert_ne!(parser.intermediates.len(), 0);
        assert_ne!(parser.sequence.len(), 0);

        let buffers = parser.take_buffers();
        assert!(buffers.is_empty());
        assert_eq!(parser.params.len(), 0);
        assert_eq!(parser.intermediates.len(), 0);
        assert_eq!(parser.sequence.len(), 0);
    }

    #[test]
    fn recycled_buffers_keep_modest_capacity() {
        let mut parser = AnsiCsiParser::new();
        let mut output = Vec::new();
        for &b in b"1;2;3m" {
            let _ = parser.ansiparser_inner_csi(b, &mut output);
        }
        let capacity = parser.sequence.capacity();
        let buffers = parser.take_buffers();
        assert_eq!(buffers.sequence.capacity(), capacity);
        assert!(buffers.max_capacity() <= RETAINED_CAPACITY_MAX);
    }

    #[test]
    fn oversized_buffers_are_shrunk_on_recycle() {
        let mut parser = AnsiCsiParser::new();
        let mut output = Vec::new();
        for _ in 0..50_000 {
            let _ = parser.ansiparser_inner_csi(b'1', &mut output);
        }
        assert!(parser.params.capacity() > RETAINED_CAPACITY_MAX);
        let buffers = parser.take_buffers();
        assert!(buffers.is_empty());
        assert!(
            buffers.max_capacity() <= RETAINED_CAPACITY_MAX,
            "capacity {} should have been released",
            buffers.max_capacity()
        );
    }

    #[test]
    fn parser_built_from_recycled_buffers_parses_like_a_fresh_one() {
        let mut first = AnsiCsiParser::new();
        let mut scratch = Vec::new();
        for &b in b"?1049;2004h" {
            let _ = first.ansiparser_inner_csi(b, &mut scratch);
        }
        let buffers = first.take_buffers();

        let mut reused = AnsiCsiParser::from_buffers(CsiIntroducer::SevenBit, buffers);
        let mut reused_out = Vec::new();
        for &b in b"5;10H" {
            let _ = reused.ansiparser_inner_csi(b, &mut reused_out);
        }
        assert_eq!(reused_out, parse_csi_sequence(b"5;10H"));
        assert_eq!(reused.sequence, b"5;10H");
        assert_eq!(reused.params, b"5;10");
        assert_eq!(reused.intermediates.len(), 0);
    }
}
