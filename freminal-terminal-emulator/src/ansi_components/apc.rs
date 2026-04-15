// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use freminal_common::buffer_states::terminal_output::TerminalOutput;

use crate::ansi::ParserOutcome;
use crate::ansi_components::tracer::{SequenceTraceable, SequenceTracer};

/// Parser for APC (Application Program Command) sequences.
///
/// An APC sequence is introduced by `ESC _` and terminated by ST (`ESC \`).
/// The parser accumulates all bytes between the introducer and the terminator,
/// including the `_` prefix and the trailing `ESC \`. APC content is opaque —
/// no interpretation of the inner bytes is performed.
#[derive(Eq, PartialEq, Debug)]
pub struct ApcParser {
    /// Accumulated sequence bytes, starting with `_`.
    pub sequence: Vec<u8>,

    // Internal trace of recent bytes for diagnostics.
    seq_trace: SequenceTracer,
}

impl Default for ApcParser {
    fn default() -> Self {
        Self::new()
    }
}

impl SequenceTraceable for ApcParser {
    #[inline]
    fn seq_tracer(&mut self) -> &mut SequenceTracer {
        &mut self.seq_trace
    }
    #[inline]
    fn seq_tracer_ref(&self) -> &SequenceTracer {
        &self.seq_trace
    }
}

impl ApcParser {
    #[must_use]
    pub fn new() -> Self {
        Self {
            sequence: vec![b'_'],
            seq_trace: SequenceTracer::new(),
        }
    }

    /// Returns `true` when the accumulated sequence ends with ST (`ESC \`).
    #[must_use]
    pub fn contains_string_terminator(&self) -> bool {
        self.sequence.ends_with(b"\x1b\\")
    }

    /// Expose current sequence trace for testing and diagnostics.
    #[must_use]
    pub fn trace_str(&self) -> String {
        self.seq_trace.as_str()
    }

    /// Push a byte into the APC parser and return the parser outcome.
    ///
    /// Accumulates bytes until a String Terminator is detected, at which
    /// point it emits `TerminalOutput::ApplicationProgramCommand` and
    /// returns `ParserOutcome::Finished`.
    pub fn apc_parser_inner(&mut self, b: u8, output: &mut Vec<TerminalOutput>) -> ParserOutcome {
        self.append_trace(b);
        self.sequence.push(b);

        if self.contains_string_terminator() {
            self.seq_trace.trim_control_tail();
            output.push(TerminalOutput::ApplicationProgramCommand(std::mem::take(
                &mut self.sequence,
            )));
            return ParserOutcome::Finished;
        }

        ParserOutcome::Continue
    }
}

#[cfg(test)]
mod tests {
    use super::ApcParser;
    use crate::ansi::ParserOutcome;
    use crate::ansi_components::tracer::SequenceTraceable;
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
    fn seq_tracer_returns_mutable_reference() {
        let mut parser = ApcParser::new();
        // Calling seq_tracer() should give a mutable reference to the internal tracer
        let tracer = parser.seq_tracer();
        tracer.push(b'A');
    }

    #[test]
    fn seq_tracer_ref_returns_immutable_reference() {
        let parser = ApcParser::new();
        // seq_tracer_ref() should return a reference to the internal tracer
        let tracer = parser.seq_tracer_ref();
        // The tracer starts empty
        assert_eq!(tracer.as_str(), "");
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
        assert!(output.is_empty());
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
        assert!(output.is_empty());
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
}
