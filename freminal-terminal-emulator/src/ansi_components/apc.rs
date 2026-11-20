// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use freminal_common::buffer_states::terminal_output::TerminalOutput;

use crate::ansi::{ParserOutcome, PrevByte};
use crate::ansi_components::tracer::lossy_sequence_for_log_bounded;

/// Maximum size of an APC sequence, in bytes.
///
/// The cap applies to the stored sequence **before its terminator**: the `_`
/// introducer plus the body. A sequence whose stored bytes, excluding the
/// closing `ESC \`, number `MAX_APC_BYTES` or fewer is dispatched; one more
/// byte and it is dropped. (A trailing ESC is not counted until the byte after
/// it shows it was not the start of the terminator.)
///
/// kitty's own cap is 256 KiB (`MAX_ESCAPE_CODE_LENGTH` in `vt-parser.c`).
/// Freminal is deliberately more generous at 1 MiB, because a single kitty
/// graphics chunk is at most 4 KiB but other APC users are not bounded by that
/// protocol.
pub const MAX_APC_BYTES: usize = 1024 * 1024;

/// Whether an APC sequence is still being accumulated or has been dropped.
#[derive(Eq, PartialEq, Debug)]
enum ApcState {
    /// Bytes are accumulated in [`ApcParser::sequence`].
    Accumulating,
    /// The sequence went over [`MAX_APC_BYTES`]; its buffer was released and
    /// only what terminator detection needs is kept.
    Overflow { total: usize, prev: PrevByte },
}

/// Parser for APC (Application Program Command) sequences.
///
/// An APC sequence is introduced by `ESC _` and terminated by ST (`ESC \`).
/// The parser accumulates all bytes between the introducer and the terminator,
/// including the `_` prefix and the trailing `ESC \`. APC content is opaque —
/// no interpretation of the inner bytes is performed.
///
/// A sequence larger than [`MAX_APC_BYTES`] is dropped: its buffer is freed,
/// it is consumed through its terminator, it produces no output, and one
/// warning (introducer, cap and total length, never the payload) is logged.
#[derive(Eq, PartialEq, Debug)]
pub struct ApcParser {
    /// Accumulated sequence bytes, starting with `_`.
    pub sequence: Vec<u8>,
    state: ApcState,
}

impl Default for ApcParser {
    fn default() -> Self {
        Self::new()
    }
}

impl ApcParser {
    #[must_use]
    pub fn new() -> Self {
        Self {
            sequence: vec![b'_'],
            state: ApcState::Accumulating,
        }
    }

    /// Returns `true` when the accumulated sequence ends with ST (`ESC \`).
    #[must_use]
    pub fn contains_string_terminator(&self) -> bool {
        self.sequence.ends_with(b"\x1b\\")
    }

    /// Expose the current sequence for testing and diagnostics.
    ///
    /// Rendered from [`Self::sequence`] without its leading `_` introducer,
    /// lossily decoded and bounded to
    /// [`crate::ansi_components::tracer::LOG_SEQUENCE_MAX_BYTES`].
    #[must_use]
    pub fn trace_str(&self) -> String {
        lossy_sequence_for_log_bounded(self.sequence.get(1..).unwrap_or_default())
    }

    /// Push a byte into the APC parser and return the parser outcome.
    ///
    /// Accumulates bytes until a String Terminator is detected, at which
    /// point it emits `TerminalOutput::ApplicationProgramCommand` and
    /// returns `ParserOutcome::Finished`. A sequence over [`MAX_APC_BYTES`]
    /// is consumed through its terminator and returns `Finished` without
    /// emitting anything.
    ///
    /// An ESC followed by anything other than `\` (a bare ESC) also ends the
    /// sequence. It is dispatched (or dropped, if over the cap) as if ST had
    /// arrived, and the parser returns `ParserOutcome::FinishedAtEsc` without
    /// consuming `b`: the top-level parser parses it as the second byte of the
    /// new escape sequence that ESC starts.
    ///
    /// CAN and SUB cancel the sequence, as in xterm's `sos_table` (`CASE_CAN` /
    /// `CASE_SUB`) and the DEC VT500 state machine: nothing is dispatched, and
    /// the parser returns `ParserOutcome::Finished` so the top-level parser
    /// returns to ground.
    pub fn apc_parser_inner(&mut self, b: u8, output: &mut Vec<TerminalOutput>) -> ParserOutcome {
        // The range test keeps the common (printable) byte to one comparison.
        if b < 0x1b && matches!(b, 0x18 | 0x1a) {
            return self.cancel(b);
        }

        if let ApcState::Overflow { total, prev } = &mut self.state {
            let after_esc = *prev == PrevByte::Esc;
            let bare_esc = after_esc && b != b'\\';
            if !bare_esc {
                *total = total.saturating_add(1);
            }
            let terminated = after_esc && b == b'\\';
            *prev = PrevByte::of(b);
            if terminated || bare_esc {
                tracing::warn!(
                    "APC sequence over the {MAX_APC_BYTES}-byte cap dropped: total length {total} bytes"
                );
                return if bare_esc {
                    ParserOutcome::FinishedAtEsc
                } else {
                    ParserOutcome::Finished
                };
            }
            return ParserOutcome::Continue;
        }

        // A stored ESC is always the last stored byte: the next byte either
        // completes `ESC \` or, being anything else, shows the ESC was bare.
        // The sequence is then dispatched exactly as if it ended in ST.
        if b != b'\\' && self.sequence.last() == Some(&0x1b) {
            self.sequence.push(b'\\');
            output.push(TerminalOutput::ApplicationProgramCommand(std::mem::take(
                &mut self.sequence,
            )));
            return ParserOutcome::FinishedAtEsc;
        }

        self.sequence.push(b);

        if self.contains_string_terminator() {
            output.push(TerminalOutput::ApplicationProgramCommand(std::mem::take(
                &mut self.sequence,
            )));
            return ParserOutcome::Finished;
        }

        if self.sequence.len() > MAX_APC_BYTES {
            self.overflow_if_over_cap();
        }

        ParserOutcome::Continue
    }

    /// Cancel the sequence on CAN or SUB: no output, buffer released.
    fn cancel(&mut self, b: u8) -> ParserOutcome {
        debug!(
            "APC cancelled by 0x{b:02x} after {} stored bytes",
            self.sequence.len()
        );
        self.sequence = Vec::new();
        ParserOutcome::Finished
    }

    /// End the sequence at an 8-bit C1 ST (`0x9C`): the same as `ESC \`. The
    /// top-level parser calls this only when 8-bit C1 controls are in force
    /// (S8C1T); otherwise `0x9C` is an ordinary byte of the body.
    ///
    /// A stored trailing ESC is dropped, and the sequence is dispatched with
    /// the canonical `ESC \` terminator appended. An overflowed sequence is
    /// dropped, as at any other terminator.
    pub fn terminate_c1_st(&mut self, output: &mut Vec<TerminalOutput>) -> ParserOutcome {
        if let ApcState::Overflow { total, .. } = &self.state {
            tracing::warn!(
                "APC sequence over the {MAX_APC_BYTES}-byte cap dropped: total length {total} bytes"
            );
            return ParserOutcome::Finished;
        }

        if self.sequence.last() == Some(&0x1b) {
            self.sequence.pop();
        }
        self.sequence.extend_from_slice(b"\x1b\\");
        output.push(TerminalOutput::ApplicationProgramCommand(std::mem::take(
            &mut self.sequence,
        )));
        ParserOutcome::Finished
    }

    /// Called only once the stored sequence is longer than the cap and is not
    /// terminated. A trailing ESC may still be the start of the terminator, so
    /// it is not counted; if the rest is still over the cap, drop the buffer.
    fn overflow_if_over_cap(&mut self) {
        let last = self.sequence.last().copied().unwrap_or_default();
        let uncounted = usize::from(last == 0x1b);
        if self.sequence.len().saturating_sub(uncounted) > MAX_APC_BYTES {
            self.state = ApcState::Overflow {
                total: self.sequence.len(),
                prev: PrevByte::of(last),
            };
            self.sequence = Vec::new();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ApcParser, MAX_APC_BYTES};
    use crate::ansi::{FreminalAnsiParser, ParserOutcome};
    use freminal_common::buffer_states::terminal_output::TerminalOutput;

    #[test]
    fn default_creates_valid_parser() {
        let parser = ApcParser::default();
        assert_eq!(parser.sequence, vec![b'_']);
        assert!(!parser.contains_string_terminator());
    }

    #[test]
    fn new_creates_valid_parser() {
        let parser = ApcParser::new();
        assert_eq!(parser.sequence, vec![b'_']);
        assert!(!parser.contains_string_terminator());
    }

    #[test]
    fn apc_parser_accumulates_bytes_until_st() {
        let mut parser = ApcParser::new();
        let mut output = Vec::new();
        // Feed data bytes
        for &b in b"hello" {
            let result = parser.apc_parser_inner(b, &mut output);
            assert!(matches!(result, ParserOutcome::Continue));
        }
        assert_eq!(output, []);
        // Feed ST: ESC \
        parser.apc_parser_inner(0x1b, &mut output);
        let result = parser.apc_parser_inner(b'\\', &mut output);
        assert!(matches!(result, ParserOutcome::Finished));
        assert_eq!(output.len(), 1);
        assert!(matches!(
            &output[0],
            TerminalOutput::ApplicationProgramCommand(_)
        ));
    }

    #[test]
    fn apc_parser_no_terminator_keeps_continuing() {
        let mut parser = ApcParser::new();
        let mut output = Vec::new();
        for &b in b"data without terminator" {
            let result = parser.apc_parser_inner(b, &mut output);
            assert!(matches!(result, ParserOutcome::Continue));
        }
        assert_eq!(output, []);
    }

    #[test]
    fn trace_str_returns_string() {
        let mut parser = ApcParser::new();
        let mut output = Vec::new();
        parser.apc_parser_inner(b'A', &mut output);
        let trace = parser.trace_str();
        assert!(trace.contains('A'));
    }

    #[test]
    fn contains_string_terminator_false_without_st() {
        let parser = ApcParser::new();
        assert!(!parser.contains_string_terminator());
    }

    #[test]
    fn contains_string_terminator_true_after_st() {
        let mut parser = ApcParser::new();
        let mut output = Vec::new();
        parser.apc_parser_inner(0x1b, &mut output);
        parser.apc_parser_inner(b'\\', &mut output);
        assert!(
            parser.sequence.is_empty() || parser.contains_string_terminator() || output.len() == 1
        );
    }

    // ------------------------------------------------------------------
    // Byte cap (Task 129.9)
    // ------------------------------------------------------------------

    /// Push every byte, asserting that none of them completes the sequence.
    /// The sub-parser starts with its `_` introducer already stored, so `n`
    /// body bytes bring the stored length to `1 + n`.
    fn push_all(parser: &mut ApcParser, bytes: &[u8], output: &mut Vec<TerminalOutput>) {
        for &b in bytes {
            let outcome = parser.apc_parser_inner(b, output);
            assert!(matches!(outcome, ParserOutcome::Continue));
        }
    }

    #[test]
    fn exactly_at_cap_is_dispatched() {
        // Stored bytes before the terminator: `_` + (MAX - 1) body bytes.
        let mut parser = ApcParser::new();
        let mut output = Vec::new();
        push_all(&mut parser, &vec![b'x'; MAX_APC_BYTES - 1], &mut output);
        assert_eq!(parser.sequence.len(), MAX_APC_BYTES);
        // The terminator is not counted: the ESC is accepted at the cap...
        parser.apc_parser_inner(0x1b, &mut output);
        // ...and `\` completes it.
        let outcome = parser.apc_parser_inner(b'\\', &mut output);
        assert!(matches!(outcome, ParserOutcome::Finished));
        assert_eq!(output.len(), 1);
        let TerminalOutput::ApplicationProgramCommand(seq) = &output[0] else {
            panic!("expected an APC output, got {:?}", output[0]);
        };
        assert_eq!(seq.len(), MAX_APC_BYTES + 2);
    }

    #[test]
    fn one_byte_over_cap_is_dropped() {
        let mut parser = ApcParser::new();
        let mut output = Vec::new();
        push_all(&mut parser, &vec![b'x'; MAX_APC_BYTES], &mut output);
        // Over the cap: buffer released, nothing emitted.
        assert_eq!(parser.sequence.capacity(), 0);
        parser.apc_parser_inner(0x1b, &mut output);
        let outcome = parser.apc_parser_inner(b'\\', &mut output);
        assert!(matches!(outcome, ParserOutcome::Finished));
        assert_eq!(output, []);
    }

    #[test]
    fn overflow_warns_with_introducer_and_length_but_not_payload() {
        let mut parser = ApcParser::new();
        let mut output = Vec::new();
        let events = crate::log_capture::capture(|| {
            push_all(&mut parser, b"SECRETMARK", &mut output);
            push_all(
                &mut parser,
                &vec![b'x'; MAX_APC_BYTES - b"SECRETMARK".len()],
                &mut output,
            );
            // The stored length is now MAX + 1 (introducer included): over.
            parser.apc_parser_inner(0x1b, &mut output);
            let outcome = parser.apc_parser_inner(b'\\', &mut output);
            assert!(matches!(outcome, ParserOutcome::Finished));
        });
        assert_eq!(output, []);
        let warns = crate::log_capture::warnings(&events);
        assert_eq!(warns.len(), 1, "{events:?}");
        let total = MAX_APC_BYTES + 3;
        assert!(warns[0].1.contains("APC"), "{warns:?}");
        assert!(
            warns[0].1.contains(&format!("total length {total} bytes")),
            "{warns:?}"
        );
        assert!(!warns[0].1.contains("SECRETMARK"), "{warns:?}");
    }

    #[test]
    fn overflow_ends_only_at_a_real_st_or_a_bare_esc() {
        let overflowed = || {
            let mut parser = ApcParser::new();
            let mut output = Vec::new();
            push_all(&mut parser, &vec![b'x'; MAX_APC_BYTES], &mut output);
            parser
        };
        let mut output = Vec::new();

        // A lone backslash does not end it; ESC `\` does.
        let mut parser = overflowed();
        push_all(&mut parser, b"\\\\", &mut output);
        push_all(&mut parser, b"\x1b", &mut output);
        let outcome = parser.apc_parser_inner(b'\\', &mut output);
        assert!(matches!(outcome, ParserOutcome::Finished));
        assert_eq!(output, []);

        // An ESC followed by anything else is a bare ESC: it ends the
        // sequence (dropped), leaving the following byte unconsumed.
        for next in [b'x', b'[', 0x1b] {
            let mut parser = overflowed();
            push_all(&mut parser, b"\x1b", &mut output);
            let outcome = parser.apc_parser_inner(next, &mut output);
            assert!(matches!(outcome, ParserOutcome::FinishedAtEsc), "{next:#x}");
            assert_eq!(output, []);
        }
    }

    #[test]
    fn bare_esc_dispatches_the_body_as_if_st_had_arrived() {
        let mut parser = ApcParser::new();
        let mut output = Vec::new();
        push_all(&mut parser, b"Ga=q,i=1;\x1b", &mut output);
        assert_eq!(output, []);
        let outcome = parser.apc_parser_inner(b'[', &mut output);
        assert!(matches!(outcome, ParserOutcome::FinishedAtEsc));
        // Same bytes as the `ESC \`-terminated form.
        assert_eq!(
            output,
            vec![TerminalOutput::ApplicationProgramCommand(
                b"_Ga=q,i=1;\x1b\\".to_vec()
            )]
        );
    }

    #[test]
    fn esc_followed_by_backslash_still_terminates() {
        let mut parser = ApcParser::new();
        let mut output = Vec::new();
        push_all(&mut parser, b"Gx\x1b", &mut output);
        let outcome = parser.apc_parser_inner(b'\\', &mut output);
        assert!(matches!(outcome, ParserOutcome::Finished));
        assert_eq!(
            output,
            vec![TerminalOutput::ApplicationProgramCommand(
                b"_Gx\x1b\\".to_vec()
            )]
        );
    }

    #[test]
    fn bare_esc_over_the_cap_drops_the_sequence_and_the_next_byte_starts_a_new_one() {
        let mut bytes = b"\x1b_G".to_vec();
        bytes.extend(std::iter::repeat_n(b'A', MAX_APC_BYTES + 10));
        bytes.extend_from_slice(b"\x1b[31mhello");

        let mut parser = FreminalAnsiParser::new();
        let output = parser.push(&bytes);
        assert_eq!(output.len(), 2, "{output:?}");
        assert!(matches!(&output[0], TerminalOutput::Sgr(_)), "{output:?}");
        assert_eq!(output[1], TerminalOutput::Data(b"hello".to_vec()));
    }

    #[test]
    fn overflowed_sequence_is_followed_by_normal_text() {
        let mut bytes = b"\x1b_G".to_vec();
        bytes.extend(std::iter::repeat_n(b'A', MAX_APC_BYTES + 10));
        bytes.extend_from_slice(b"\x1b\\hello");

        let mut parser = FreminalAnsiParser::new();
        let output = parser.push(&bytes);
        assert_eq!(output, vec![TerminalOutput::Data(b"hello".to_vec())]);
    }

    #[test]
    fn at_cap_sequence_through_the_full_parser_is_dispatched() {
        // `ESC _` is consumed by the top-level parser; `G` + body brings the
        // stored length to MAX_APC_BYTES.
        let mut bytes = b"\x1b_".to_vec();
        bytes.extend(std::iter::repeat_n(b'A', MAX_APC_BYTES - 1));
        bytes.extend_from_slice(b"\x1b\\hello");

        let mut parser = FreminalAnsiParser::new();
        let output = parser.push(&bytes);
        assert_eq!(output.len(), 2);
        assert!(matches!(
            &output[0],
            TerminalOutput::ApplicationProgramCommand(_)
        ));
        assert_eq!(output[1], TerminalOutput::Data(b"hello".to_vec()));
    }

    // ------------------------------------------------------------------
    // CAN / SUB and C1 ST (Task 147 adversarial review)
    // ------------------------------------------------------------------

    #[test]
    fn can_and_sub_cancel_the_sequence_with_no_output() {
        for cancel in [0x18_u8, 0x1a] {
            let mut parser = ApcParser::new();
            let mut output = Vec::new();
            push_all(&mut parser, b"abc", &mut output);
            let outcome = parser.apc_parser_inner(cancel, &mut output);
            assert!(matches!(outcome, ParserOutcome::Finished), "{outcome:?}");
            assert_eq!(output, []);
        }
    }

    #[test]
    fn can_cancels_an_overflowed_sequence() {
        let mut parser = ApcParser::new();
        let mut output = Vec::new();
        push_all(&mut parser, &vec![b'x'; MAX_APC_BYTES], &mut output);
        let outcome = parser.apc_parser_inner(0x18, &mut output);
        assert!(matches!(outcome, ParserOutcome::Finished), "{outcome:?}");
        assert_eq!(output, []);
    }

    #[test]
    fn c1_st_dispatches_the_sequence_with_the_canonical_terminator() {
        let mut parser = ApcParser::new();
        let mut output = Vec::new();
        push_all(&mut parser, b"abc", &mut output);
        let outcome = parser.terminate_c1_st(&mut output);
        assert!(matches!(outcome, ParserOutcome::Finished), "{outcome:?}");
        assert_eq!(
            output,
            [TerminalOutput::ApplicationProgramCommand(
                b"_abc\x1b\\".to_vec()
            )]
        );
    }

    #[test]
    fn c1_st_drops_a_stored_trailing_esc() {
        let mut parser = ApcParser::new();
        let mut output = Vec::new();
        push_all(&mut parser, b"abc\x1b", &mut output);
        let outcome = parser.terminate_c1_st(&mut output);
        assert!(matches!(outcome, ParserOutcome::Finished), "{outcome:?}");
        assert_eq!(
            output,
            [TerminalOutput::ApplicationProgramCommand(
                b"_abc\x1b\\".to_vec()
            )]
        );
    }

    #[test]
    fn c1_st_ends_an_overflowed_sequence_with_no_output() {
        let mut parser = ApcParser::new();
        let mut output = Vec::new();
        push_all(&mut parser, &vec![b'x'; MAX_APC_BYTES], &mut output);
        let outcome = parser.terminate_c1_st(&mut output);
        assert!(matches!(outcome, ParserOutcome::Finished), "{outcome:?}");
        assert_eq!(output, []);
    }
}
