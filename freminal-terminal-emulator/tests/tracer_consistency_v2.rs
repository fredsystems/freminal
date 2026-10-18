// Copyright (C) 2024–2025 Fred Clausen
// Licensed under the MIT license (https://opensource.org/licenses/MIT).

use freminal_terminal_emulator::{
    ansi::FreminalAnsiParser, ansi_components::tracer::SequenceTraceable,
};

/// Ensures sequence tracer buffer appends deterministically and resets only when appropriate.
#[test]
fn tracer_buffer_appends_and_clears_in_expected_pattern() {
    let mut parser = FreminalAnsiParser::new();

    // Feed partial sequence, then complete it.
    parser.push(b"\x1b[38;2;255");
    let mid_trace = parser.seq_tracer().as_str();
    assert!(
        mid_trace.contains("38;2;255"),
        "trace should contain partial data before completion"
    );

    // Now complete sequence
    parser.push(b";0;0m");
    let final_trace = parser.seq_tracer().as_str();

    // The tracer is designed to retain the last completed sequence for debugging/logging.
    // It must NOT be cleared prematurely, but it should still contain the completed sequence.
    assert!(
        final_trace.contains("38;2;255;0;0m"),
        "trace should retain the last completed sequence, got: {final_trace:?}"
    );
}

/// Ensures identical data chunked differently yields identical tracer states.
#[test]
fn tracer_deterministic_across_chunking_patterns() {
    let mut parser1 = FreminalAnsiParser::new();
    let mut parser2 = FreminalAnsiParser::new();

    // Feed in one go
    parser1.push(b"\x1b[38;2;255;0;0m");

    // Feed in small chunks — use slice-of-slices so lengths can differ.
    let steps: &[&[u8]] = &[b"\x1b[38;", b"2;255;", b"0;", b"0m"];

    for &chunk in steps {
        parser2.push(chunk);
    }

    assert_eq!(
        parser1.seq_tracer().as_str(),
        parser2.seq_tracer().as_str(),
        "sequence tracer content should be identical across chunking patterns"
    );
}
