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
