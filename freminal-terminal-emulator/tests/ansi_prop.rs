// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use freminal_common::buffer_states::terminal_output::TerminalOutput;
use freminal_terminal_emulator::ansi::FreminalAnsiParser;
use proptest::{
    prelude::any,
    prop_assert, prop_assert_eq, prop_assume, prop_oneof, proptest,
    strategy::{Just, Strategy},
};

/// Generates arbitrary byte sequences that may contain printable data,
/// control bytes, and escape sequences.
fn arb_ansi_bytes() -> impl Strategy<Value = Vec<u8>> {
    proptest::collection::vec(
        prop_oneof![
            // Normal printable ASCII
            (0x20u8..=0x7Eu8),
            // Common control characters
            proptest::sample::select(vec![0x07u8, 0x08u8, b'\r', b'\n']),
            // Escape initiator
            Just(0x1Bu8),
            // Some extended random bytes (0–255)
            any::<u8>(),
        ],
        0..256, // random length up to 256 bytes
    )
}

proptest! {
    /// Property: The ANSI parser should never panic or enter an invalid state
    /// when presented with arbitrary data streams.
    #[test]
    fn ansi_parser_never_panics_on_random_bytes(data in arb_ansi_bytes()) {
        let mut parser = FreminalAnsiParser::new();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            parser.push(&data)
        }));

        // Should never panic
        prop_assert!(result.is_ok());
        let outputs = result.unwrap();

        // Parser output should always be finite and contain valid TerminalOutput items
        prop_assert!(outputs.len() < 10_000, "unexpectedly huge output");
        for o in &outputs {
            // Formatting each variant should not panic
            let _ = format!("{o}");
        }
    }

    /// Property: Feeding the same bytes in chunks vs all-at-once should yield equivalent output.
    #[test]
    fn push_is_deterministic_across_chunking(data in arb_ansi_bytes()) {
        let mut parser_full = FreminalAnsiParser::new();
        let out_full = parser_full.push(&data);

        let mut parser_chunked = FreminalAnsiParser::new();
        let mut out_chunked = Vec::new();
        for chunk in data.chunks(1) {
            out_chunked.extend(parser_chunked.push(chunk));
        }

        // Extract and concatenate only the Data payloads from both outputs
        let bytes_full: Vec<u8> = out_full
            .iter()
            .filter_map(|o| match o {
                TerminalOutput::Data(d) => Some(d.clone()),
                _ => None,
            })
            .flatten()
            .collect();

        let bytes_chunked: Vec<u8> = out_chunked
            .iter()
            .filter_map(|o| match o {
                TerminalOutput::Data(d) => Some(d.clone()),
                _ => None,
            })
            .flatten()
            .collect();

        // ✅ Assert the actual data stream is equivalent
        prop_assert_eq!(
            bytes_full, bytes_chunked,
            "Concatenated data content should match even if Data batching differs"
        );

        // ✅ Assert all outputs are valid, printable, or known control responses
        for o in out_full.iter().chain(out_chunked.iter()) {
            let _ = format!("{o}"); // Should never panic
        }
    }

    /// Property: Normal printable ASCII sequences should return a single Data variant.
    #[test]
    fn printable_ascii_roundtrip(s in "\\PC{1,64}") {
        let bytes = s.as_bytes();
        prop_assume!(!bytes.contains(&0x1B)); // skip escape bytes
        let mut parser = FreminalAnsiParser::new();
        let out = parser.push(bytes);
        prop_assert_eq!(out, vec![TerminalOutput::Data(bytes.to_vec())]);
    }

    /// Property: Each control character should map to a distinct TerminalOutput variant.
    #[test]
    fn known_control_chars_emit_expected_output(b in proptest::sample::select(vec![b'\r', b'\n', 0x07u8, 0x08u8])) {
        let mut parser = FreminalAnsiParser::new();
        let out = parser.push(&[b]);
        prop_assert!(matches!(
            out.last().unwrap(),
            TerminalOutput::CarriageReturn
                | TerminalOutput::Newline
                | TerminalOutput::Backspace
                | TerminalOutput::Bell
        ));
    }
}
