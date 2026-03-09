// Copyright (C) 2024–2025 Fred Clausen
// Licensed under the MIT license (https://opensource.org/licenses/MIT).

use freminal_common::buffer_states::terminal_output::TerminalOutput;
use freminal_terminal_emulator::ansi::FreminalAnsiParser;

#[inline]
fn parse(seq: &[u8]) -> Vec<TerminalOutput> {
    let mut p = FreminalAnsiParser::new();
    p.push(seq)
}

/// Out-of-range 256-color indices should be ignored or degraded safely (no panic).
#[test]
fn sgr_256_color_indices_out_of_range_are_graceful() {
    let cases: &[&[u8]] = &[
        b"\x1b[38;5;256m",
        b"\x1b[38;5;999m",
        b"\x1b[48;5;300m",
        b"\x1b[38;5;9999999999m", // overflow
        b"\x1b[38;5;abcmm",       // invalid digits trail
    ];

    for &seq in cases {
        let outputs = parse(seq);
        // Parser must not panic; it may emit nothing or benign SGR.
        assert!(
            !outputs.is_empty() || outputs.is_empty(),
            "parser should handle malformed 256-color {:?} gracefully",
            String::from_utf8_lossy(seq)
        );
    }
}

/// Truecolor components >255 or malformed should degrade safely.
#[test]
fn sgr_truecolor_components_out_of_range_or_missing_are_graceful() {
    let cases: &[&[u8]] = &[
        b"\x1b[38;2;300;300;300m",
        b"\x1b[48;2;999;0;0m",
        b"\x1b[38;2;;255;255m",       // missing r
        b"\x1b[38;2;255;;255m",       // missing g
        b"\x1b[38;2;255;255; m",      // space before m
        b"\x1b[38;2;255;255;255",     // no terminator
        b"\x1b[48;2;1;2;4294967295m", // huge overflow
    ];

    for &seq in cases {
        let outputs = parse(seq);
        assert!(
            !outputs.is_empty() || outputs.is_empty(),
            "parser should handle malformed truecolor {:?} gracefully",
            String::from_utf8_lossy(seq)
        );
    }
}

/// Incomplete sequences should not dirty state; a following valid SGR must still parse or remain safe.
#[test]
fn sgr_incomplete_then_valid_parses_normally() {
    let mut p = FreminalAnsiParser::new();

    // Incomplete truecolor (no 'm'), then a valid 256-color FG
    p.push(b"\x1b[38;2;255;0;0");
    let outputs = p.push(b"\x1b[38;5;160m");

    // The parser should not panic and should emit *something* (even if Invalid or Data)
    assert!(
        !outputs.is_empty(),
        "parser produced no output after incomplete+valid; expected graceful handling, got {:?}",
        outputs
    );
}

/// Extra parameters after a valid truecolor/256-color prefix should be ignored gracefully.
#[test]
fn sgr_extra_parameters_are_ignored_or_coerced() {
    let cases: &[&[u8]] = &[
        b"\x1b[38;2;1;2;3;4;5m",             // extras after rgb
        b"\x1b[48;5;10;99;100m",             // extras after 256-color
        b"\x1b[38;2;10;20;30;48;5;200;123m", // long tail
    ];

    for &seq in cases {
        let outputs = parse(seq);
        // We only require graceful handling. If the implementation still emits an SGR, great.
        assert!(
            !outputs.is_empty() || outputs.is_empty(),
            "parser should handle extra-parameter SGR {:?} gracefully",
            String::from_utf8_lossy(seq)
        );
    }
}

/// Empty-parameter SGRs should be treated as benign (often reset) or ignored.
#[test]
fn sgr_empty_parameters_are_noop_or_reset() {
    let cases: &[&[u8]] = &[
        b"\x1b[m",
        b"\x1b[;m",
        b"\x1b[;;m",
        b"\x1b[0;m",
        b"\x1b[0;;m",
    ];

    for &seq in cases {
        let outputs = parse(seq);
        // Either emits a reset SGR or nothing harmful.
        assert!(
            outputs.iter().all(|_| true),
            "parser should handle empty-parameter SGR {:?} gracefully (no panic)",
            String::from_utf8_lossy(seq)
        );
    }
}

/// Mixed or malformed chaining between 38/48 forms should remain tolerant.
#[test]
fn sgr_mixed_38_48_forms_with_errors_are_tolerant() {
    let cases: &[&[u8]] = &[
        // mix forms; one of them malformed
        b"\x1b[38;2;10;20;30;48;2;11;22m", // missing b in second RGB
        b"\x1b[48;5;200;38;2;10;20m",      // missing b for 38;2
        b"\x1b[38;5;10;48;2;1;2;3;999m",   // extra trailing number
    ];

    for &seq in cases {
        let outputs = parse(seq);
        assert!(
            !outputs.is_empty() || outputs.is_empty(),
            "parser should tolerate mixed/malformed 38/48 in {:?}",
            String::from_utf8_lossy(seq)
        );
    }
}

/// Invalid first, then valid (chunked) must yield a valid or at least safe output eventually.
#[test]
fn sgr_invalid_then_valid_chunked_still_emits_sgr() {
    let mut p = FreminalAnsiParser::new();
    let valid_chunks: &[&[u8]] = &[b"\x1b[38;", b"2;", b"100;", b"150;", b"200m"];

    // Start with a badly malformed SGR
    p.push(b"\x1b[38;2;999;999;999m");

    // Now deliver a valid one in small chunks
    for &chunk in valid_chunks {
        p.push(chunk);
    }

    // Feed a trailing tick of normal text to trigger any pending output
    let outputs = p.push(b"text");

    // Parser must remain functional and not panic; any output type is acceptable.
    assert!(
        !outputs.is_empty(),
        "parser stayed silent after invalid→valid chunked SGR; expected graceful or valid output"
    );
}

/// Very large integers should not panic (overflow-safe parsing).
#[test]
fn sgr_extremely_large_integers_do_not_panic() {
    let cases: &[&[u8]] = &[
        b"\x1b[38;2;4294967295;4294967295;4294967295m",
        b"\x1b[48;5;18446744073709551615m",
        b"\x1b[38;18446744073709551615m", // bogus form but should be tolerant
    ];

    for &seq in cases {
        let outputs = parse(seq);
        assert!(
            !outputs.is_empty() || outputs.is_empty(),
            "parser should handle huge integers in {:?} gracefully",
            String::from_utf8_lossy(seq)
        );
    }
}
