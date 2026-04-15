// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use freminal_common::buffer_states::terminal_output::TerminalOutput;

use crate::ansi::ParserOutcome;
use crate::ansi_components::tracer::{SequenceTraceable, SequenceTracer};

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
#[derive(Eq, PartialEq, Debug)]
pub struct DcsParser {
    /// Accumulated sequence bytes, starting with `P`.
    pub sequence: Vec<u8>,

    // Internal trace of recent bytes for diagnostics.
    seq_trace: SequenceTracer,
}

impl Default for DcsParser {
    fn default() -> Self {
        Self::new()
    }
}

impl SequenceTraceable for DcsParser {
    #[inline]
    fn seq_tracer(&mut self) -> &mut SequenceTracer {
        &mut self.seq_trace
    }
    #[inline]
    fn seq_tracer_ref(&self) -> &SequenceTracer {
        &self.seq_trace
    }
}

impl DcsParser {
    #[must_use]
    pub fn new() -> Self {
        Self {
            sequence: vec![b'P'],
            seq_trace: SequenceTracer::new(),
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

    /// Returns `true` when this parser is accumulating a tmux DCS
    /// passthrough sequence (the sequence buffer starts with `Ptmux;`).
    #[must_use]
    fn is_tmux_passthrough(&self) -> bool {
        self.sequence.starts_with(b"Ptmux;")
    }

    /// Expose current sequence trace for testing and diagnostics.
    #[must_use]
    pub fn trace_str(&self) -> String {
        self.seq_trace.as_str()
    }

    /// Push a byte into the DCS parser and return the parser outcome.
    ///
    /// Accumulates bytes until a String Terminator is detected, at which
    /// point it emits `TerminalOutput::DeviceControlString` and returns
    /// `ParserOutcome::Finished`.
    pub fn dcs_parser_inner(&mut self, b: u8, output: &mut Vec<TerminalOutput>) -> ParserOutcome {
        self.append_trace(b);
        self.sequence.push(b);

        if self.contains_string_terminator() {
            self.seq_trace.trim_control_tail();
            output.push(TerminalOutput::DeviceControlString(std::mem::take(
                &mut self.sequence,
            )));
            return ParserOutcome::Finished;
        }

        ParserOutcome::Continue
    }
}

#[cfg(test)]
mod tests {
    use super::DcsParser;
    use crate::ansi::ParserOutcome;
    use crate::ansi_components::tracer::SequenceTraceable;
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
    fn seq_tracer_returns_mutable_reference() {
        let mut parser = DcsParser::new();
        // Calling seq_tracer() should give a mutable reference to the internal tracer
        let tracer = parser.seq_tracer();
        tracer.push(b'A');
    }

    #[test]
    fn seq_tracer_ref_returns_immutable_reference() {
        let parser = DcsParser::new();
        // seq_tracer_ref() should return a reference to the internal tracer
        let tracer = parser.seq_tracer_ref();
        // The tracer starts empty
        assert_eq!(tracer.as_str(), "");
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
        assert!(output.is_empty());
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
        assert!(output.is_empty());
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
}
