// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use freminal_common::buffer_states::terminal_output::TerminalOutput;

use crate::ansi::{ParserOutcome, PrevByte};
use crate::ansi_components::tracer::lossy_sequence_for_log_bounded;

/// Maximum size of a DCS sequence, in bytes.
///
/// The cap applies to the stored sequence **before its terminator**: the `P`
/// introducer plus the body. A sequence whose stored bytes, excluding the
/// closing `ESC \`, number `MAX_DCS_BYTES` or fewer is dispatched; one more
/// byte and it is dropped. (A trailing ESC is not counted until the byte after
/// it shows it was not the start of the terminator.)
///
/// DCS carries sixel images and tmux passthrough, where a single large
/// sequence is legitimate, so unlike OSC and APC (1 MiB) the cap is 64 MiB.
/// kitty's own cap is 256 KiB (`MAX_ESCAPE_CODE_LENGTH` in `vt-parser.c`);
/// Freminal is deliberately more generous because it also implements sixel.
pub const MAX_DCS_BYTES: usize = 64 * 1024 * 1024;

/// How an overflowed DCS finds its terminator.
#[derive(Eq, PartialEq, Debug)]
enum DcsOverflowTerminator {
    /// Plain DCS: a real ST is any `ESC \`.
    Plain { prev: PrevByte },
    /// tmux passthrough (`Ptmux;`): every inner ESC is doubled, so `ESC \` is
    /// a real ST only when the run of consecutive ESC bytes before the `\` is
    /// odd. `esc_run` is the current run length.
    Tmux { esc_run: usize },
}

/// Whether a DCS sequence is still being accumulated or has been dropped.
#[derive(Eq, PartialEq, Debug)]
enum DcsState {
    /// Bytes are accumulated in [`DcsParser::sequence`].
    Accumulating,
    /// The sequence went over [`MAX_DCS_BYTES`]; its buffer was released and
    /// only what terminator detection needs is kept.
    Overflow {
        total: usize,
        terminator: DcsOverflowTerminator,
    },
}

/// Parser for DCS (Device Control String) sequences.
///
/// A DCS sequence is introduced by `ESC P` and terminated by ST (`ESC \`).
/// The parser accumulates all bytes between the introducer and the terminator,
/// including the `P` prefix and the trailing `ESC \`.
///
/// For tmux DCS passthrough sequences (`ESC P tmux; ... ESC \`), every ESC
/// in the inner payload is doubled. The parser correctly handles this by
/// counting consecutive ESC bytes before the trailing `\` to distinguish
/// real ST from doubled inner content.
///
/// A sequence larger than [`MAX_DCS_BYTES`] is dropped: its buffer is freed,
/// it is consumed through its (real) terminator, it produces no output, and
/// one warning (introducer, cap and total length, never the payload) is
/// logged.
#[derive(Eq, PartialEq, Debug)]
pub struct DcsParser {
    /// Accumulated sequence bytes, starting with `P`.
    pub sequence: Vec<u8>,
    state: DcsState,
}

impl Default for DcsParser {
    fn default() -> Self {
        Self::new()
    }
}

impl DcsParser {
    #[must_use]
    pub fn new() -> Self {
        Self {
            sequence: vec![b'P'],
            state: DcsState::Accumulating,
        }
    }

    /// Returns `true` when the accumulated sequence ends with a real
    /// String Terminator (`ESC \`).
    ///
    /// For **tmux DCS passthrough** (`\x1bPtmux;…\x1b\\`), every ESC in
    /// the inner payload is doubled.  A naïve `ends_with(b"\x1b\\")` would
    /// falsely detect `ESC ESC \` (a doubled-ESC followed by a literal
    /// backslash) as the real ST.
    ///
    /// The algorithm counts consecutive ESC bytes immediately before the
    /// trailing `\`:
    ///
    /// - **Odd count** (1, 3, …) → the final ESC is unpaired → real ST.
    /// - **Even count** (2, 4, …) → all ESCs are doubled pairs, the `\` is
    ///   inner content → **not** an ST.
    ///
    /// For non-tmux DCS sequences the simple suffix check is used
    /// (no doubling is expected there).
    #[must_use]
    pub fn contains_string_terminator(&self) -> bool {
        if !self.sequence.ends_with(b"\x1b\\") {
            return false;
        }

        // Non-tmux sequences: the simple suffix check is sufficient.
        if !self.is_tmux_passthrough() {
            return true;
        }

        // Tmux passthrough: count consecutive ESC bytes before the final `\`.
        // The `\` is at sequence[len - 1], so we walk backwards from
        // sequence[len - 2].
        let len = self.sequence.len();
        let mut esc_count: usize = 0;
        for &b in self.sequence[..len - 1].iter().rev() {
            if b == 0x1b {
                esc_count += 1;
            } else {
                break;
            }
        }

        // Odd count → real ST; even count → doubled inner content.
        esc_count % 2 == 1
    }

    /// Returns `true` when `b`, arriving next, shows that the stored trailing
    /// ESC is a bare ESC: one that starts neither `ESC \` nor (in tmux
    /// passthrough) a doubled `ESC ESC`. A bare ESC ends the string.
    ///
    /// A stored ESC is always the last stored byte, because the byte after it
    /// either completes the terminator or ends the string here.
    ///
    /// - **Plain DCS:** any byte but `\` after the ESC, `ESC` included.
    /// - **tmux passthrough:** an `ESC ESC` pair is data, so only an unpaired
    ///   ESC (an odd run of ESC bytes) followed by a byte that is neither `\`
    ///   (the real ST) nor ESC (which would pair it) is bare. Runs are counted
    ///   only when a non-ESC byte arrives, so a flood of ESC bytes costs
    ///   nothing per byte.
    fn ends_at_bare_esc(&self, b: u8) -> bool {
        if b == b'\\' || self.sequence.last() != Some(&0x1b) {
            return false;
        }
        if !self.is_tmux_passthrough() {
            return true;
        }
        if b == 0x1b {
            return false;
        }
        let run = self
            .sequence
            .iter()
            .rev()
            .take_while(|&&c| c == 0x1b)
            .count();
        run % 2 == 1
    }

    /// Returns `true` when the stored trailing ESC is not half of a doubled
    /// `ESC ESC` pair: always in plain DCS, and for an odd run of ESC bytes in
    /// tmux passthrough.
    fn trailing_esc_is_unpaired(&self) -> bool {
        if !self.is_tmux_passthrough() {
            return true;
        }
        let run = self
            .sequence
            .iter()
            .rev()
            .take_while(|&&c| c == 0x1b)
            .count();
        run % 2 == 1
    }

    /// Returns `true` when this parser is accumulating a tmux DCS
    /// passthrough sequence (the sequence buffer starts with `Ptmux;`).
    #[must_use]
    fn is_tmux_passthrough(&self) -> bool {
        self.sequence.starts_with(b"Ptmux;")
    }

    /// Expose the current sequence for testing and diagnostics.
    ///
    /// Rendered from [`Self::sequence`] without its leading `P` introducer,
    /// lossily decoded and bounded to
    /// [`crate::ansi_components::tracer::LOG_SEQUENCE_MAX_BYTES`].
    #[must_use]
    pub fn trace_str(&self) -> String {
        lossy_sequence_for_log_bounded(self.sequence.get(1..).unwrap_or_default())
    }

    /// Push a byte into the DCS parser and return the parser outcome.
    ///
    /// Accumulates bytes until a String Terminator is detected, at which
    /// point it emits `TerminalOutput::DeviceControlString` and returns
    /// `ParserOutcome::Finished`. A sequence over [`MAX_DCS_BYTES`] is
    /// consumed through its terminator and returns `Finished` without
    /// emitting anything.
    ///
    /// A bare ESC (one not followed by `\`) also ends the sequence: it is
    /// dispatched (or dropped, if over the cap) as if ST had arrived, and the
    /// parser returns `ParserOutcome::FinishedAtEsc` without consuming `b`. The
    /// top-level parser parses `b` as the second byte of the new escape
    /// sequence that ESC starts. In `DCS tmux;` passthrough a doubled
    /// `ESC ESC` is data and does not end the sequence.
    ///
    /// CAN and SUB cancel the sequence, as in xterm's `sos_table` (`CASE_CAN` /
    /// `CASE_SUB`) and the DEC VT500 state machine: nothing is dispatched, and
    /// the parser returns `ParserOutcome::Finished` so the top-level parser
    /// returns to ground. This holds in tmux passthrough too: tmux's own parser
    /// is cancelled by these bytes, so none can be inner payload.
    pub fn dcs_parser_inner(&mut self, b: u8, output: &mut Vec<TerminalOutput>) -> ParserOutcome {
        // The range test keeps the common (printable) byte to one comparison.
        if b < 0x1b && matches!(b, 0x18 | 0x1a) {
            return self.cancel(b);
        }

        if let DcsState::Overflow { total, terminator } = &mut self.state {
            let (terminated, bare_esc) = match terminator {
                DcsOverflowTerminator::Plain { prev } => {
                    let after_esc = *prev == PrevByte::Esc;
                    *prev = PrevByte::of(b);
                    (after_esc && b == b'\\', after_esc && b != b'\\')
                }
                DcsOverflowTerminator::Tmux { esc_run } => {
                    // An odd run leaves one unpaired ESC: `\` ends the string
                    // (real ST) and any byte but ESC shows the ESC was bare.
                    let unpaired = *esc_run % 2 == 1;
                    *esc_run = if b == 0x1b {
                        esc_run.saturating_add(1)
                    } else {
                        0
                    };
                    (unpaired && b == b'\\', unpaired && b != b'\\' && b != 0x1b)
                }
            };
            if !bare_esc {
                *total = total.saturating_add(1);
            }
            if terminated || bare_esc {
                tracing::warn!(
                    "DCS sequence over the {MAX_DCS_BYTES}-byte cap dropped: total length {total} bytes"
                );
                return if bare_esc {
                    ParserOutcome::FinishedAtEsc
                } else {
                    ParserOutcome::Finished
                };
            }
            return ParserOutcome::Continue;
        }

        if self.ends_at_bare_esc(b) {
            // Dispatch exactly as if the ESC had been the start of ST.
            self.sequence.push(b'\\');
            output.push(TerminalOutput::DeviceControlString(std::mem::take(
                &mut self.sequence,
            )));
            return ParserOutcome::FinishedAtEsc;
        }

        self.sequence.push(b);

        if self.contains_string_terminator() {
            output.push(TerminalOutput::DeviceControlString(std::mem::take(
                &mut self.sequence,
            )));
            return ParserOutcome::Finished;
        }

        if self.sequence.len() > MAX_DCS_BYTES {
            self.overflow_if_over_cap();
        }

        ParserOutcome::Continue
    }

    /// Cancel the sequence on CAN or SUB: no output, buffer released.
    fn cancel(&mut self, b: u8) -> ParserOutcome {
        debug!(
            "DCS cancelled by 0x{b:02x} after {} stored bytes",
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
        if let DcsState::Overflow { total, .. } = &self.state {
            tracing::warn!(
                "DCS sequence over the {MAX_DCS_BYTES}-byte cap dropped: total length {total} bytes"
            );
            return ParserOutcome::Finished;
        }

        // Only an unpaired trailing ESC is a bare ESC to strip. In tmux
        // passthrough an even run of ESC bytes is doubled inner data, and the
        // canonical terminator appended below must see it intact.
        if self.sequence.last() == Some(&0x1b) && self.trailing_esc_is_unpaired() {
            self.sequence.pop();
        }
        if self.sequence.len() > MAX_DCS_BYTES {
            // A trailing ESC was not counted while it might start the
            // terminator; as data it is, and the body is over the cap.
            tracing::warn!(
                "DCS sequence over the {MAX_DCS_BYTES}-byte cap dropped: total length {} bytes",
                self.sequence.len()
            );
            self.sequence = Vec::new();
            return ParserOutcome::Finished;
        }
        self.sequence.extend_from_slice(b"\x1b\\");
        output.push(TerminalOutput::DeviceControlString(std::mem::take(
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
        if self.sequence.len().saturating_sub(uncounted) <= MAX_DCS_BYTES {
            return;
        }

        let terminator = if self.is_tmux_passthrough() {
            let esc_run = self
                .sequence
                .iter()
                .rev()
                .take_while(|&&c| c == 0x1b)
                .count();
            DcsOverflowTerminator::Tmux { esc_run }
        } else {
            DcsOverflowTerminator::Plain {
                prev: PrevByte::of(last),
            }
        };
        self.state = DcsState::Overflow {
            total: self.sequence.len(),
            terminator,
        };
        self.sequence = Vec::new();
    }
}

#[cfg(test)]
mod tests {
    use super::{DcsParser, MAX_DCS_BYTES};
    use crate::ansi::{FreminalAnsiParser, ParserOutcome};
    use freminal_common::buffer_states::terminal_output::TerminalOutput;

    #[test]
    fn default_creates_valid_parser() {
        let parser = DcsParser::default();
        assert_eq!(parser.sequence, vec![b'P']);
        assert!(!parser.contains_string_terminator());
    }

    #[test]
    fn new_creates_valid_parser() {
        let parser = DcsParser::new();
        assert_eq!(parser.sequence, vec![b'P']);
        assert!(!parser.contains_string_terminator());
    }

    #[test]
    fn dcs_parser_accumulates_bytes_until_st() {
        let mut parser = DcsParser::new();
        let mut output = Vec::new();
        // Feed data bytes
        for &b in b"hello" {
            let result = parser.dcs_parser_inner(b, &mut output);
            assert!(matches!(result, ParserOutcome::Continue));
        }
        assert_eq!(output, []);
        // Feed ST: ESC \
        parser.dcs_parser_inner(0x1b, &mut output);
        let result = parser.dcs_parser_inner(b'\\', &mut output);
        assert!(matches!(result, ParserOutcome::Finished));
        assert_eq!(output.len(), 1);
        assert!(matches!(&output[0], TerminalOutput::DeviceControlString(_)));
    }

    #[test]
    fn dcs_parser_no_terminator_keeps_continuing() {
        let mut parser = DcsParser::new();
        let mut output = Vec::new();
        for &b in b"data without terminator" {
            let result = parser.dcs_parser_inner(b, &mut output);
            assert!(matches!(result, ParserOutcome::Continue));
        }
        assert_eq!(output, []);
    }

    #[test]
    fn trace_str_returns_string() {
        let mut parser = DcsParser::new();
        let mut output = Vec::new();
        parser.dcs_parser_inner(b'A', &mut output);
        let trace = parser.trace_str();
        assert!(trace.contains('A'));
    }

    #[test]
    fn contains_string_terminator_false_without_st() {
        let parser = DcsParser::new();
        assert!(!parser.contains_string_terminator());
    }

    #[test]
    fn tmux_passthrough_not_false_terminated_by_doubled_esc() {
        // A tmux passthrough that ends with \x1b\x1b\ should NOT be treated as ST.
        // Sequence: b"Ptmux;" + data + ESC ESC \
        let mut parser = DcsParser::new();
        // Set up a tmux passthrough sequence manually
        for &b in b"tmux;" {
            parser.sequence.push(b);
        }
        // Add inner ESC ESC \ (doubled ESC = even count → not real ST)
        parser.sequence.push(0x1b);
        parser.sequence.push(0x1b);
        parser.sequence.push(b'\\');
        assert!(!parser.contains_string_terminator());
    }

    #[test]
    fn tmux_passthrough_terminated_by_single_esc() {
        // A tmux passthrough ending with a single ESC \ is a real ST.
        let mut parser = DcsParser::new();
        for &b in b"tmux;" {
            parser.sequence.push(b);
        }
        // Single ESC \ (odd count → real ST)
        parser.sequence.push(0x1b);
        parser.sequence.push(b'\\');
        assert!(parser.contains_string_terminator());
    }

    // ------------------------------------------------------------------
    // Byte cap
    // ------------------------------------------------------------------

    /// A parser whose stored sequence is `prefix` padded with `x` to exactly
    /// `len` bytes. Avoids pushing 64 MiB a byte at a time in every test.
    fn parser_with_stored_len(prefix: &[u8], len: usize) -> DcsParser {
        let mut parser = DcsParser::new();
        parser.sequence = prefix.to_vec();
        parser.sequence.resize(len, b'x');
        parser
    }

    fn push_all(parser: &mut DcsParser, bytes: &[u8], output: &mut Vec<TerminalOutput>) {
        for &b in bytes {
            let outcome = parser.dcs_parser_inner(b, output);
            assert!(matches!(outcome, ParserOutcome::Continue));
        }
    }

    #[test]
    fn exactly_at_cap_is_dispatched() {
        // Stored bytes before the terminator: `P` + body == MAX_DCS_BYTES.
        let mut parser = parser_with_stored_len(b"P", MAX_DCS_BYTES);
        let mut output = Vec::new();
        // The terminator is not counted: the ESC is accepted at the cap...
        parser.dcs_parser_inner(0x1b, &mut output);
        // ...and `\` completes it.
        let outcome = parser.dcs_parser_inner(b'\\', &mut output);
        assert!(matches!(outcome, ParserOutcome::Finished));
        assert_eq!(output.len(), 1);
        let TerminalOutput::DeviceControlString(seq) = &output[0] else {
            panic!("expected a DCS output, got {:?}", output[0]);
        };
        assert_eq!(seq.len(), MAX_DCS_BYTES + 2);
    }

    #[test]
    fn tmux_exactly_at_cap_is_dispatched() {
        let mut parser = parser_with_stored_len(b"Ptmux;", MAX_DCS_BYTES);
        let mut output = Vec::new();
        parser.dcs_parser_inner(0x1b, &mut output);
        let outcome = parser.dcs_parser_inner(b'\\', &mut output);
        assert!(matches!(outcome, ParserOutcome::Finished));
        assert_eq!(output.len(), 1);
    }

    #[test]
    fn one_byte_over_cap_is_dropped() {
        let mut parser = parser_with_stored_len(b"P", MAX_DCS_BYTES);
        let mut output = Vec::new();
        push_all(&mut parser, b"x", &mut output);
        // Over the cap: buffer released, nothing emitted.
        assert_eq!(parser.sequence.capacity(), 0);
        parser.dcs_parser_inner(0x1b, &mut output);
        let outcome = parser.dcs_parser_inner(b'\\', &mut output);
        assert!(matches!(outcome, ParserOutcome::Finished));
        assert_eq!(output, []);
    }

    /// Drive `parser` (already at the cap) over it and through `ESC \`, and
    /// assert the single warn names `DCS` and the total length but not the
    /// marker.
    fn assert_overflow_warn_is_payload_free(mut parser: DcsParser) {
        let mut output = Vec::new();
        let events = crate::log_capture::capture(|| {
            push_all(&mut parser, b"x", &mut output);
            parser.dcs_parser_inner(0x1b, &mut output);
            let outcome = parser.dcs_parser_inner(b'\\', &mut output);
            assert!(matches!(outcome, ParserOutcome::Finished));
        });
        assert_eq!(output, []);
        let warns = crate::log_capture::warnings(&events);
        assert_eq!(warns.len(), 1, "{events:?}");
        let total = MAX_DCS_BYTES + 3;
        assert!(warns[0].1.contains("DCS"), "{warns:?}");
        assert!(
            warns[0].1.contains(&format!("total length {total} bytes")),
            "{warns:?}"
        );
        assert!(!warns[0].1.contains("SECRETMARK"), "{warns:?}");
    }

    #[test]
    fn overflow_warns_with_introducer_and_length_but_not_payload() {
        assert_overflow_warn_is_payload_free(parser_with_stored_len(b"PSECRETMARK", MAX_DCS_BYTES));
    }

    #[test]
    fn tmux_overflow_warns_with_introducer_and_length_but_not_payload() {
        assert_overflow_warn_is_payload_free(parser_with_stored_len(
            b"Ptmux;SECRETMARK",
            MAX_DCS_BYTES,
        ));
    }

    #[test]
    fn overflowed_plain_dcs_ends_only_at_a_real_st_or_a_bare_esc() {
        let mut parser = parser_with_stored_len(b"P", MAX_DCS_BYTES);
        let mut output = Vec::new();
        push_all(&mut parser, b"x", &mut output);
        // A lone backslash does not end it.
        push_all(&mut parser, b"\\", &mut output);
        // ESC `\` does.
        push_all(&mut parser, b"\x1b", &mut output);
        let outcome = parser.dcs_parser_inner(b'\\', &mut output);
        assert!(matches!(outcome, ParserOutcome::Finished));
        assert_eq!(output, []);

        // An ESC followed by anything else (ESC included) is a bare ESC: it
        // ends the sequence, dropped, leaving the next byte unconsumed.
        for next in [b'x', b'[', 0x1b] {
            let mut parser = parser_with_stored_len(b"P", MAX_DCS_BYTES);
            push_all(&mut parser, b"x\x1b", &mut output);
            let outcome = parser.dcs_parser_inner(next, &mut output);
            assert!(matches!(outcome, ParserOutcome::FinishedAtEsc), "{next:#x}");
            assert_eq!(output, []);
        }
    }

    #[test]
    fn overflowed_tmux_dcs_ends_at_an_unpaired_esc_but_not_a_doubled_one() {
        let overflowed = || {
            let mut parser = parser_with_stored_len(b"Ptmux;", MAX_DCS_BYTES);
            let mut output = Vec::new();
            push_all(&mut parser, b"x", &mut output);
            parser
        };
        let mut output = Vec::new();

        // A doubled ESC followed by a non-`\` byte is data.
        let mut parser = overflowed();
        push_all(&mut parser, b"\x1b\x1b[", &mut output);
        // An unpaired ESC (run of one, or of three) ends it...
        for run in [&b"\x1b"[..], b"\x1b\x1b\x1b"] {
            let mut parser = overflowed();
            push_all(&mut parser, run, &mut output);
            let outcome = parser.dcs_parser_inner(b'[', &mut output);
            assert!(matches!(outcome, ParserOutcome::FinishedAtEsc), "{run:?}");
        }
        // ...and the ST is still an unpaired ESC followed by `\`.
        push_all(&mut parser, b"\x1b", &mut output);
        let outcome = parser.dcs_parser_inner(b'\\', &mut output);
        assert!(matches!(outcome, ParserOutcome::Finished));
        assert_eq!(output, []);
    }

    // ------------------------------------------------------------------
    // A bare ESC ends the string
    // ------------------------------------------------------------------

    fn dcs_of(output: &[TerminalOutput]) -> &[u8] {
        let [TerminalOutput::DeviceControlString(seq)] = output else {
            panic!("expected a single DCS, got {output:?}");
        };
        seq
    }

    #[test]
    fn bare_esc_dispatches_the_body_as_if_st_had_arrived() {
        let mut parser = DcsParser::new();
        let mut output = Vec::new();
        push_all(&mut parser, b"$qm\x1b", &mut output);
        assert_eq!(output, []);
        let outcome = parser.dcs_parser_inner(b'[', &mut output);
        assert!(matches!(outcome, ParserOutcome::FinishedAtEsc));
        // The same bytes as the `ESC \`-terminated form.
        assert_eq!(dcs_of(&output), b"P$qm\x1b\\");
    }

    #[test]
    fn esc_followed_by_backslash_still_terminates() {
        let mut parser = DcsParser::new();
        let mut output = Vec::new();
        push_all(&mut parser, b"$qm\x1b", &mut output);
        let outcome = parser.dcs_parser_inner(b'\\', &mut output);
        assert!(matches!(outcome, ParserOutcome::Finished));
        assert_eq!(dcs_of(&output), b"P$qm\x1b\\");
    }

    #[test]
    fn esc_esc_in_a_plain_dcs_ends_it_at_the_second_esc() {
        let mut parser = DcsParser::new();
        let mut output = Vec::new();
        push_all(&mut parser, b"$qm\x1b", &mut output);
        let outcome = parser.dcs_parser_inner(0x1b, &mut output);
        assert!(matches!(outcome, ParserOutcome::FinishedAtEsc));
        assert_eq!(dcs_of(&output), b"P$qm\x1b\\");
    }

    #[test]
    fn tmux_doubled_esc_is_data_and_only_an_unpaired_esc_ends_the_string() {
        let mut parser = DcsParser::new();
        let mut output = Vec::new();
        // A doubled `ESC ESC [` is the wrapped CSI introducer: data.
        push_all(&mut parser, b"tmux;\x1b\x1b[31m\x1b\x1b\\", &mut output);
        assert_eq!(output, []);
        // The outer ST is an unpaired ESC followed by `\`.
        parser.dcs_parser_inner(0x1b, &mut output);
        let outcome = parser.dcs_parser_inner(b'\\', &mut output);
        assert!(matches!(outcome, ParserOutcome::Finished));
        assert_eq!(dcs_of(&output), b"Ptmux;\x1b\x1b[31m\x1b\x1b\\\x1b\\");
    }

    #[test]
    fn tmux_unpaired_esc_followed_by_another_byte_ends_the_string() {
        // `ESC ESC ESC [`: the first two are a pair (data), the third is bare.
        let mut parser = DcsParser::new();
        let mut output = Vec::new();
        push_all(&mut parser, b"tmux;a\x1b\x1b\x1b", &mut output);
        let outcome = parser.dcs_parser_inner(b'[', &mut output);
        assert!(matches!(outcome, ParserOutcome::FinishedAtEsc));
        assert_eq!(dcs_of(&output), b"Ptmux;a\x1b\x1b\x1b\\");
    }

    #[test]
    fn tmux_long_even_esc_run_is_data() {
        let mut parser = DcsParser::new();
        let mut output = Vec::new();
        push_all(&mut parser, b"tmux;", &mut output);
        let esc_run = 200_000;
        push_all(&mut parser, &vec![0x1b; esc_run], &mut output);
        let outcome = parser.dcs_parser_inner(b'[', &mut output);
        // An even run is all doubled pairs: data, no end.
        assert!(matches!(outcome, ParserOutcome::Continue));
        assert_eq!(output, []);
    }

    #[test]
    fn overflowed_tmux_dcs_with_doubled_escapes_ends_at_the_real_st() {
        let mut parser = parser_with_stored_len(b"Ptmux;", MAX_DCS_BYTES);
        let mut output = Vec::new();
        push_all(&mut parser, b"x", &mut output);
        assert_eq!(parser.sequence.capacity(), 0);

        // A doubled ESC followed by `\` is inner content, not the ST.
        push_all(&mut parser, b"\x1b\x1b\\", &mut output);
        // So is two doubled ESCs (run of four) followed by `\`.
        push_all(&mut parser, b"yy\x1b\x1b\x1b\x1b\\", &mut output);
        // An ESC, an inner doubled pair, then `\` makes a run of three: odd,
        // so this is a real ST.
        push_all(&mut parser, b"z\x1b\x1b", &mut output);
        parser.dcs_parser_inner(0x1b, &mut output);
        let outcome = parser.dcs_parser_inner(b'\\', &mut output);
        assert!(matches!(outcome, ParserOutcome::Finished));
        assert_eq!(output, []);
    }

    #[test]
    fn tmux_escape_run_is_carried_across_the_overflow_point() {
        // The ESC run that is still open when the cap is crossed must be
        // counted: here the buffer is MAX - 1 long, then three ESC bytes cross
        // the cap, so an odd run (3) is open and the next `\` is the real ST.
        let mut parser = parser_with_stored_len(b"Ptmux;", MAX_DCS_BYTES - 1);
        let mut output = Vec::new();
        push_all(&mut parser, b"\x1b\x1b\x1b", &mut output);
        assert_eq!(parser.sequence.capacity(), 0);
        let outcome = parser.dcs_parser_inner(b'\\', &mut output);
        assert!(matches!(outcome, ParserOutcome::Finished));
        assert_eq!(output, []);

        // Four ESC bytes cross the cap: an even run, so `\` is inner content
        // and only a following ESC `\` ends the sequence.
        let mut parser = parser_with_stored_len(b"Ptmux;", MAX_DCS_BYTES - 2);
        push_all(&mut parser, b"\x1b\x1b\x1b\x1b", &mut output);
        assert_eq!(parser.sequence.capacity(), 0);
        push_all(&mut parser, b"\\", &mut output);
        parser.dcs_parser_inner(0x1b, &mut output);
        let outcome = parser.dcs_parser_inner(b'\\', &mut output);
        assert!(matches!(outcome, ParserOutcome::Finished));
        assert_eq!(output, []);
    }

    #[test]
    fn overflowed_sequence_is_followed_by_normal_text() {
        let mut bytes = b"\x1bPq".to_vec();
        bytes.extend(std::iter::repeat_n(b'~', MAX_DCS_BYTES + 10));
        bytes.extend_from_slice(b"\x1b\\hello");

        let mut parser = FreminalAnsiParser::new();
        let output = parser.push(&bytes);
        assert_eq!(output, vec![TerminalOutput::Data(b"hello".to_vec())]);
    }

    // ------------------------------------------------------------------
    // CAN / SUB and C1 ST
    // ------------------------------------------------------------------

    #[test]
    fn can_and_sub_cancel_the_sequence_with_no_output() {
        for cancel in [0x18_u8, 0x1a] {
            let mut parser = DcsParser::new();
            let mut output = Vec::new();
            push_all(&mut parser, b"abc", &mut output);
            let outcome = parser.dcs_parser_inner(cancel, &mut output);
            assert!(matches!(outcome, ParserOutcome::Finished), "{outcome:?}");
            assert_eq!(output, []);
        }
    }

    #[test]
    fn can_cancels_an_overflowed_sequence() {
        let mut parser = parser_with_stored_len(b"P", MAX_DCS_BYTES);
        let mut output = Vec::new();
        // One more byte takes the stored sequence over the cap.
        assert!(matches!(
            parser.dcs_parser_inner(b'x', &mut output),
            ParserOutcome::Continue
        ));
        let outcome = parser.dcs_parser_inner(0x18, &mut output);
        assert!(matches!(outcome, ParserOutcome::Finished), "{outcome:?}");
        assert_eq!(output, []);
    }

    #[test]
    fn c1_st_dispatches_the_sequence_with_the_canonical_terminator() {
        let mut parser = DcsParser::new();
        let mut output = Vec::new();
        push_all(&mut parser, b"abc", &mut output);
        let outcome = parser.terminate_c1_st(&mut output);
        assert!(matches!(outcome, ParserOutcome::Finished), "{outcome:?}");
        assert_eq!(
            output,
            [TerminalOutput::DeviceControlString(b"Pabc\x1b\\".to_vec())]
        );
    }

    #[test]
    fn c1_st_drops_a_stored_trailing_esc() {
        let mut parser = DcsParser::new();
        let mut output = Vec::new();
        push_all(&mut parser, b"abc\x1b", &mut output);
        let outcome = parser.terminate_c1_st(&mut output);
        assert!(matches!(outcome, ParserOutcome::Finished), "{outcome:?}");
        assert_eq!(
            output,
            [TerminalOutput::DeviceControlString(b"Pabc\x1b\\".to_vec())]
        );
    }

    #[test]
    fn c1_st_keeps_a_doubled_esc_pair_at_the_end_of_a_tmux_body() {
        let mut parser = DcsParser::new();
        let mut output = Vec::new();
        push_all(&mut parser, b"tmux;a\x1b\x1b", &mut output);
        let outcome = parser.terminate_c1_st(&mut output);
        assert!(matches!(outcome, ParserOutcome::Finished), "{outcome:?}");
        // The pair stays whole, then the canonical terminator: a run of three.
        assert_eq!(dcs_of(&output), b"Ptmux;a\x1b\x1b\x1b\\");

        // An unpaired trailing ESC (a run of three) is still stripped.
        let mut parser = DcsParser::new();
        let mut output = Vec::new();
        push_all(&mut parser, b"tmux;a\x1b\x1b\x1b", &mut output);
        parser.terminate_c1_st(&mut output);
        assert_eq!(dcs_of(&output), b"Ptmux;a\x1b\x1b\x1b\\");
    }

    #[test]
    fn c1_st_rechecks_the_cap_once_a_doubled_esc_counts_as_data() {
        // `MAX - 1` stored bytes plus a doubled pair is `MAX + 1` of body: the
        // last ESC was uncounted only while it might have started the ST.
        let mut parser = parser_with_stored_len(b"Ptmux;", MAX_DCS_BYTES - 1);
        let mut output = Vec::new();
        push_all(&mut parser, b"\x1b\x1b", &mut output);
        let outcome = parser.terminate_c1_st(&mut output);
        assert!(matches!(outcome, ParserOutcome::Finished), "{outcome:?}");
        assert_eq!(output, []);

        // An unpaired trailing ESC at the same length leaves `MAX` of body.
        let mut parser = parser_with_stored_len(b"Ptmux;", MAX_DCS_BYTES);
        parser.dcs_parser_inner(0x1b, &mut output);
        parser.terminate_c1_st(&mut output);
        assert_eq!(dcs_of(&output).len(), MAX_DCS_BYTES + 2);
    }

    #[test]
    fn c1_st_ends_an_overflowed_sequence_with_no_output() {
        let mut parser = parser_with_stored_len(b"P", MAX_DCS_BYTES);
        let mut output = Vec::new();
        // One more byte takes the stored sequence over the cap.
        assert!(matches!(
            parser.dcs_parser_inner(b'x', &mut output),
            ParserOutcome::Continue
        ));
        let outcome = parser.terminate_c1_st(&mut output);
        assert!(matches!(outcome, ParserOutcome::Finished), "{outcome:?}");
        assert_eq!(output, []);
    }
}
