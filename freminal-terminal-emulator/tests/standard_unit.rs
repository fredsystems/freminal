// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use freminal_common::buffer_states::terminal_output::TerminalOutput;
use freminal_terminal_emulator::ansi::FreminalAnsiParser;
use freminal_terminal_emulator::ansi_components::dcs::DcsParser;
use freminal_terminal_emulator::ansi_components::standard::StandardParser;
use freminal_terminal_emulator::ansi_components::tracer::SequenceTraceable;

#[test]
fn standard_plain_text_does_not_panic() {
    let mut p = StandardParser::default();
    for &b in b"hello world" {
        let _ = p.push(b);
    }
    // no panic, parser accepted bytes
}

#[test]
fn standard_esc_starts_control_sequence() {
    let mut p = StandardParser::default();
    let _ = p.push(0x1b);
    // Ensure internal trace has ESC recorded
    assert!(p.current_trace_str().contains("\x1b") || !p.current_trace_str().is_empty());
}

// ── tmux-aware ST detection ──────────────────────────────────────────

/// Helper: build a `DcsParser` with the given sequence bytes already
/// accumulated (simulating what the parser would accumulate after the main
/// `FreminalAnsiParser` consumed the leading ESC and the DcsParser was
/// created with the `P` prefix byte).
fn build_dcs_parser(sequence_bytes: &[u8]) -> DcsParser {
    let mut p = DcsParser::new();
    // DcsParser::new() already has `P` in sequence.
    // Push the remaining bytes directly into the sequence.
    for &b in sequence_bytes {
        p.sequence.push(b);
    }
    p
}

#[test]
fn st_detection_normal_dcs_real_st() {
    // Non-tmux DCS: sequence ends with ESC \  → real ST
    let p = build_dcs_parser(b"somedata\x1b\\");
    assert!(
        p.contains_string_terminator(),
        "Normal DCS ending with ESC \\ should be detected as ST"
    );
}

#[test]
fn st_detection_normal_dcs_no_st() {
    // Non-tmux DCS: sequence does NOT end with ESC \
    let p = build_dcs_parser(b"somedata");
    assert!(
        !p.contains_string_terminator(),
        "Normal DCS without trailing ESC \\ should not be detected as ST"
    );
}

#[test]
fn st_detection_tmux_doubled_esc_backslash_is_not_st() {
    // tmux passthrough: inner ESC ESC \ should NOT be detected as ST
    // (even count of ESCs before \)
    let p = build_dcs_parser(b"tmux;\x1b\x1b\\");
    assert!(
        !p.contains_string_terminator(),
        "tmux passthrough: ESC ESC \\ (2 ESCs = even) should NOT be ST"
    );
}

#[test]
fn st_detection_tmux_single_esc_backslash_is_st() {
    // tmux passthrough: real outer ST is a single ESC \ at the end
    // (odd count of ESCs before \)
    let p = build_dcs_parser(b"tmux;inner_content\x1b\\");
    assert!(
        p.contains_string_terminator(),
        "tmux passthrough: single ESC \\ (1 ESC = odd) should be ST"
    );
}

#[test]
fn st_detection_tmux_full_kitty_payload() {
    // Simulate the full sequence for a tmux-wrapped Kitty graphics query:
    //   Ptmux; ESC ESC _ G a=q,i=1; ESC ESC \ ESC \
    // The parser's sequence would be:
    //   Ptmux;\x1b\x1b_Ga=q,i=1;\x1b\x1b\x5c\x1b\x5c
    //
    // Test that the inner ESC ESC \ does NOT trigger ST...
    let inner_only = build_dcs_parser(b"tmux;\x1b\x1b_Ga=q,i=1;\x1b\x1b\\");
    assert!(
        !inner_only.contains_string_terminator(),
        "tmux: inner doubled ESC \\ must not trigger ST"
    );

    // ...but with the real outer ESC \ appended, it does.
    let with_outer_st = build_dcs_parser(b"tmux;\x1b\x1b_Ga=q,i=1;\x1b\x1b\\\x1b\\");
    assert!(
        with_outer_st.contains_string_terminator(),
        "tmux: outer ESC \\ after doubled content must trigger ST"
    );
}

#[test]
fn st_detection_tmux_three_escs_before_backslash() {
    // Three consecutive ESCs before \: odd → real ST
    // This represents: one doubled pair + one real ESC for the ST
    let p = build_dcs_parser(b"tmux;data\x1b\x1b\x1b\\");
    assert!(
        p.contains_string_terminator(),
        "tmux: 3 ESCs before \\ (odd) should be ST"
    );
}

#[test]
fn st_detection_tmux_four_escs_before_backslash() {
    // Four consecutive ESCs before \: even → NOT ST
    // This represents: two doubled pairs, and the \ is inner content
    let p = build_dcs_parser(b"tmux;data\x1b\x1b\x1b\x1b\\");
    assert!(
        !p.contains_string_terminator(),
        "tmux: 4 ESCs before \\ (even) should NOT be ST"
    );
}

// ── Full parser integration: tmux DCS delivered complete ─────────────

#[test]
fn full_parser_tmux_dcs_not_terminated_early() {
    // Feed a complete tmux DCS passthrough containing a Kitty graphics
    // query through the full FreminalAnsiParser.  Verify that:
    // 1. The parser delivers exactly one DeviceControlString output.
    // 2. The delivered payload contains the COMPLETE inner content
    //    (including the inner doubled ESC \), not a truncated version.
    //
    // Wire format:
    //   ESC P tmux; ESC ESC _ G q=2,t=f,i=1; ESC ESC \ ESC \
    //   ^^^^                                             ^^^^
    //   outer DCS                                        outer ST
    let mut wire: Vec<u8> = Vec::new();
    wire.push(0x1b); // ESC
    wire.push(b'P'); // DCS introducer
    wire.extend_from_slice(b"tmux;");
    wire.extend_from_slice(b"\x1b\x1b_Gq=2,t=f,i=1;"); // inner APC (doubled ESC)
    wire.extend_from_slice(b"\x1b\x1b\\"); // inner ST (doubled ESC + \)
    wire.extend_from_slice(b"\x1b\\"); // outer ST

    let mut parser = FreminalAnsiParser::new();
    let outputs = parser.push(&wire);

    // Find the DCS output
    let dcs_outputs: Vec<_> = outputs
        .iter()
        .filter(|o| matches!(o, TerminalOutput::DeviceControlString(_)))
        .collect();

    assert_eq!(
        dcs_outputs.len(),
        1,
        "Expected exactly one DCS output, got {}: {dcs_outputs:?}",
        dcs_outputs.len()
    );

    let TerminalOutput::DeviceControlString(seq) = &dcs_outputs[0] else {
        unreachable!();
    };

    // The sequence should start with "Ptmux;"
    assert!(
        seq.starts_with(b"Ptmux;"),
        "DCS sequence should start with Ptmux;, got: {}",
        String::from_utf8_lossy(seq)
    );

    // The sequence should contain the complete inner doubled payload.
    // Specifically, it must contain the inner ESC ESC \ (the doubled
    // ST that was previously being incorrectly detected as the real ST).
    //
    // The full sequence (without the leading ESC consumed by the main
    // parser, but including the P and trailing ESC \) is:
    //   Ptmux;\x1b\x1b_Gq=2,t=f,i=1;\x1b\x1b\x5c\x1b\x5c
    //
    // The trailing \x1b\x5c is the real ST that the parser strips or
    // includes depending on implementation.  The critical assertion is
    // that the inner \x1b\x1b\x5c is present.
    let inner_doubled_st = b"\x1b\x1b\\";
    assert!(
        seq.windows(inner_doubled_st.len())
            .any(|w| w == inner_doubled_st),
        "DCS payload must contain the complete inner doubled ESC \\. Got: {:02x?}",
        seq
    );
}

#[test]
fn full_parser_tmux_dcs_csi_inner_delivered_complete() {
    // tmux-wrapped CSI: ESC P tmux; ESC ESC [ > q ESC \
    // The inner CSI [>q (XTVERSION) should be delivered as part of the
    // complete DCS, not terminated early.
    let mut wire: Vec<u8> = Vec::new();
    wire.push(0x1b);
    wire.push(b'P');
    wire.extend_from_slice(b"tmux;");
    wire.extend_from_slice(b"\x1b\x1b[>q"); // inner doubled ESC + CSI
    wire.extend_from_slice(b"\x1b\\"); // outer ST

    let mut parser = FreminalAnsiParser::new();
    let outputs = parser.push(&wire);

    let dcs_outputs: Vec<_> = outputs
        .iter()
        .filter(|o| matches!(o, TerminalOutput::DeviceControlString(_)))
        .collect();

    assert_eq!(
        dcs_outputs.len(),
        1,
        "Expected exactly one DCS output, got {}: {dcs_outputs:?}",
        dcs_outputs.len()
    );

    let TerminalOutput::DeviceControlString(seq) = &dcs_outputs[0] else {
        unreachable!();
    };

    // Must contain the inner ESC ESC [ (doubled CSI introducer)
    assert!(
        seq.windows(3).any(|w| w == b"\x1b\x1b["),
        "DCS payload must contain inner doubled ESC [. Got: {:02x?}",
        seq
    );
}

#[test]
fn full_parser_normal_dcs_still_terminates_correctly() {
    // Non-tmux DCS should still terminate on the first ESC \.
    // ESC P $ q m ESC \
    let wire = b"\x1bP$qm\x1b\\";

    let mut parser = FreminalAnsiParser::new();
    let outputs = parser.push(wire);

    let dcs_outputs: Vec<_> = outputs
        .iter()
        .filter(|o| matches!(o, TerminalOutput::DeviceControlString(_)))
        .collect();

    assert_eq!(
        dcs_outputs.len(),
        1,
        "Expected exactly one DCS output for normal DCS"
    );

    let TerminalOutput::DeviceControlString(seq) = &dcs_outputs[0] else {
        unreachable!();
    };

    // Should be "P$qm\x1b\"
    assert!(
        seq.starts_with(b"P$qm"),
        "Normal DCS should contain $qm. Got: {:02x?}",
        seq
    );
}

#[test]
fn full_parser_tmux_dcs_back_to_back_doubled_esc() {
    // Test with content that has multiple doubled ESC sequences:
    //   ESC P tmux; ESC ESC [ 1 ; 4 2 H ESC ESC _ G ... ESC ESC \ ESC \
    // This simulates Frame 132 from test.bin: CSI cursor move + Kitty APC
    let mut wire: Vec<u8> = Vec::new();
    wire.push(0x1b);
    wire.push(b'P');
    wire.extend_from_slice(b"tmux;");
    wire.extend_from_slice(b"\x1b\x1b[1;42H"); // doubled ESC + CSI
    wire.extend_from_slice(b"\x1b\x1b_Gf=100;AAAA"); // doubled ESC + APC
    wire.extend_from_slice(b"\x1b\x1b\\"); // inner ST (doubled)
    wire.extend_from_slice(b"\x1b\\"); // outer ST

    let mut parser = FreminalAnsiParser::new();
    let outputs = parser.push(&wire);

    let dcs_outputs: Vec<_> = outputs
        .iter()
        .filter(|o| matches!(o, TerminalOutput::DeviceControlString(_)))
        .collect();

    assert_eq!(
        dcs_outputs.len(),
        1,
        "Expected exactly one DCS output for multi-inner tmux. Got {}: {dcs_outputs:?}",
        dcs_outputs.len()
    );

    let TerminalOutput::DeviceControlString(seq) = &dcs_outputs[0] else {
        unreachable!();
    };

    // Must contain both inner doubled ESC sequences
    assert!(
        seq.windows(3).any(|w| w == b"\x1b\x1b["),
        "Must contain inner doubled CSI. Got: {:02x?}",
        seq
    );
    assert!(
        seq.windows(3).any(|w| w == b"\x1b\x1b_"),
        "Must contain inner doubled APC. Got: {:02x?}",
        seq
    );
}

// ── APC parsing through FreminalAnsiParser ──────────────────────────

#[test]
fn full_parser_apc_basic() {
    // ESC _ hello ESC \  →  ApplicationProgramCommand
    let wire = b"\x1b_hello\x1b\\";
    let mut parser = FreminalAnsiParser::new();
    let outputs = parser.push(wire);

    let apc_outputs: Vec<_> = outputs
        .iter()
        .filter(|o| matches!(o, TerminalOutput::ApplicationProgramCommand(_)))
        .collect();

    assert_eq!(
        apc_outputs.len(),
        1,
        "Expected exactly one APC output, got {}: {apc_outputs:?}",
        apc_outputs.len()
    );

    let TerminalOutput::ApplicationProgramCommand(seq) = &apc_outputs[0] else {
        unreachable!();
    };

    // Sequence starts with `_` (the APC introducer byte) and includes
    // the content plus the trailing ST.
    assert!(
        seq.starts_with(b"_hello"),
        "APC sequence should start with _hello. Got: {:02x?}",
        seq
    );
}

#[test]
fn full_parser_apc_split_across_chunks() {
    // Feed the APC in two chunks to verify cross-chunk accumulation.
    let mut parser = FreminalAnsiParser::new();
    let result1 = parser.push(b"\x1b_hel");
    assert!(
        result1.is_empty(),
        "First chunk should not produce output: {result1:?}"
    );

    let result2 = parser.push(b"lo\x1b\\");
    let apc_outputs: Vec<_> = result2
        .iter()
        .filter(|o| matches!(o, TerminalOutput::ApplicationProgramCommand(_)))
        .collect();

    assert_eq!(
        apc_outputs.len(),
        1,
        "Expected exactly one APC output after second chunk"
    );
}
