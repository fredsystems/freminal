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
