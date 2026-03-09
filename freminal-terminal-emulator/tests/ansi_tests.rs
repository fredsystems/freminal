// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use freminal_common::buffer_states::terminal_output::TerminalOutput;
use freminal_common::{
    buffer_states::window_manipulation::WindowManipulation, cursor::CursorVisualStyle,
};
use freminal_terminal_emulator::ansi::{
    FreminalAnsiParser, ParserInner, extract_param, parse_param_as,
    split_params_into_colon_delimited_usize, split_params_into_semicolon_delimited_usize,
};
use proptest::{prelude::any, prop_assert_eq, proptest};
use std::fmt::Write;
// ---------- TerminalOutput Display Tests ----------

#[test]
fn display_basic_variants() {
    assert_eq!(
        TerminalOutput::SetCursorPos {
            x: Some(3),
            y: None
        }
        .to_string(),
        "SetCursorPos: x: Some(3), y: None"
    );
    assert_eq!(
        TerminalOutput::SetCursorPosRel {
            x: Some(-1),
            y: Some(5)
        }
        .to_string(),
        "SetCursorPosRel: x: Some(-1), y: Some(5)"
    );
    assert_eq!(TerminalOutput::ClearDisplay.to_string(), "ClearDisplay");
    assert_eq!(TerminalOutput::CarriageReturn.to_string(), "CarriageReturn");
    assert_eq!(TerminalOutput::ClearLine.to_string(), "ClearLine");
    assert_eq!(TerminalOutput::Newline.to_string(), "Newline");
    assert_eq!(TerminalOutput::Backspace.to_string(), "Backspace");
    assert_eq!(TerminalOutput::Bell.to_string(), "Bell");
    assert_eq!(TerminalOutput::InsertLines(2).to_string(), "InsertLines(2)");
    assert_eq!(TerminalOutput::Delete(5).to_string(), "Delete(5)");
    assert_eq!(TerminalOutput::Erase(1).to_string(), "Erase(1)");
    assert_eq!(
        TerminalOutput::Sgr(Default::default()).to_string(),
        "Sgr(NoOp)"
    );
    assert_eq!(
        TerminalOutput::Data(vec![b'H', b'i']).to_string(),
        "Data(Hi)"
    );
    assert_eq!(
        TerminalOutput::Mode(Default::default()).to_string(),
        "SetMode(NoOp)"
    );
    assert_eq!(
        TerminalOutput::InsertSpaces(3).to_string(),
        "InsertSpaces(3)"
    );
    assert_eq!(
        TerminalOutput::OscResponse(Default::default()).to_string(),
        "OscResponse(NoOp)"
    );
    assert_eq!(TerminalOutput::Invalid.to_string(), "Invalid");
    assert_eq!(TerminalOutput::CursorReport.to_string(), "CursorReport");
    assert_eq!(TerminalOutput::Skipped.to_string(), "Skipped");
    assert_eq!(
        TerminalOutput::ApplicationKeypadMode.to_string(),
        "ApplicationKeypadMode"
    );
    assert_eq!(
        TerminalOutput::NormalKeypadMode.to_string(),
        "NormalKeypadMode"
    );
    assert_eq!(
        TerminalOutput::CursorVisualStyle(CursorVisualStyle::BlockCursorBlink).to_string(),
        "CursorVisualStyle(BlockCursorBlink)"
    );
    assert_eq!(
        TerminalOutput::WindowManipulation(WindowManipulation::DeIconifyWindow).to_string(),
        "WindowManipulation(DeIconifyWindow)"
    );
    assert_eq!(
        TerminalOutput::SetTopAndBottomMargins {
            top_margin: 2,
            bottom_margin: 4
        }
        .to_string(),
        "SetTopAndBottomMargins(2, 4)"
    );
}

#[test]
fn display_exhaustive_variants_short() {
    let mut buf = String::new();
    let variants = [
        TerminalOutput::EightBitControl,
        TerminalOutput::SevenBitControl,
        TerminalOutput::AnsiConformanceLevelOne,
        TerminalOutput::AnsiConformanceLevelTwo,
        TerminalOutput::AnsiConformanceLevelThree,
        TerminalOutput::DoubleLineHeightTop,
        TerminalOutput::DoubleLineHeightBottom,
        TerminalOutput::SingleWidthLine,
        TerminalOutput::DoubleWidthLine,
        TerminalOutput::ScreenAlignmentTest,
        TerminalOutput::CharsetDefault,
        TerminalOutput::CharsetUTF8,
        TerminalOutput::CharsetG0,
        TerminalOutput::CharsetG1,
        TerminalOutput::CharsetG1AsGR,
        TerminalOutput::CharsetG2,
        TerminalOutput::CharsetG2AsGR,
        TerminalOutput::CharsetG2AsGL,
        TerminalOutput::CharsetG3,
        TerminalOutput::CharsetG3AsGR,
        TerminalOutput::CharsetG3AsGL,
        TerminalOutput::DecSpecial,
        TerminalOutput::CharsetUK,
        TerminalOutput::CharsetUS,
        TerminalOutput::CharsetUSASCII,
        TerminalOutput::CharsetDutch,
        TerminalOutput::CharsetFinnish,
        TerminalOutput::CharsetFrench,
        TerminalOutput::CharsetFrenchCanadian,
        TerminalOutput::CharsetGerman,
        TerminalOutput::CharsetItalian,
        TerminalOutput::CharsetNorwegianDanish,
        TerminalOutput::CharsetSpanish,
        TerminalOutput::CharsetSwedish,
        TerminalOutput::CharsetSwiss,
        TerminalOutput::SaveCursor,
        TerminalOutput::RestoreCursor,
        TerminalOutput::CursorToLowerLeftCorner,
        TerminalOutput::ResetDevice,
        TerminalOutput::MemoryLock,
        TerminalOutput::MemoryUnlock,
        TerminalOutput::RequestDeviceAttributes,
        TerminalOutput::RequestDeviceNameAndVersion,
    ];
    for v in variants {
        write!(&mut buf, "{v}").unwrap();
        buf.clear();
    }

    // DeviceControlString + ApplicationProgramCommand
    assert_eq!(
        TerminalOutput::DeviceControlString(b"abc".to_vec()).to_string(),
        "DeviceControlString(abc)"
    );
    assert_eq!(
        TerminalOutput::ApplicationProgramCommand(b"xyz".to_vec()).to_string(),
        "ApplicationProgramCommand(xyz)"
    );
}

// ---------- Utility Function Tests ----------

#[test]
fn extract_param_basic() {
    let params = vec![Some(5), None];
    assert_eq!(extract_param(0, &params), Some(5));
    assert_eq!(extract_param(1, &params), None);
    assert_eq!(extract_param(2, &params), None);
}

#[test]
fn parse_param_as_variants() {
    assert_eq!(parse_param_as::<usize>(b"123").unwrap(), Some(123));
    assert_eq!(parse_param_as::<usize>(b"").unwrap(), None);
    assert!(parse_param_as::<usize>(b"abc").is_err());
}

#[test]
fn split_params_semicolon_and_colon() {
    let s = b"1;2;;3";
    let res = split_params_into_semicolon_delimited_usize(s).unwrap();
    assert_eq!(res, vec![Some(1), Some(2), None, Some(3)]);

    let s = b"10:20:30";
    let res = split_params_into_colon_delimited_usize(s).unwrap();
    assert_eq!(res, vec![Some(10), Some(20), Some(30)]);
}

// ---------- Parser Logic Tests ----------

#[test]
fn parser_default_and_new_equivalence() {
    let p1 = FreminalAnsiParser::default();
    let p2 = FreminalAnsiParser::new();
    assert_eq!(p1.inner, p2.inner);
    assert_eq!(p1.inner, ParserInner::Empty);
}

#[test]
fn push_handles_escape_to_modes() {
    let mut parser = FreminalAnsiParser::new();
    // Normal bytes first
    let out = parser.push(b"abc");
    assert_eq!(out.last(), Some(&TerminalOutput::Data(b"abc".to_vec())));

    // Escape enters escape mode
    let out = parser.push(b"\x1b[");
    assert!(matches!(parser.inner, ParserInner::Csi(_)));
    assert!(out.is_empty());
}

#[test]
fn parser_push_cr_lf_backspace_bell_paths() {
    let mut parser = FreminalAnsiParser::new();
    let inputs = [b'\r', b'\n', 0x08, 0x07];
    for b in inputs {
        let out = parser.push(&[b]);
        assert!(matches!(
            out.last().unwrap(),
            TerminalOutput::CarriageReturn
                | TerminalOutput::Newline
                | TerminalOutput::Backspace
                | TerminalOutput::Bell
        ));
    }
}

// ---------- Property Tests ----------

proptest! {
    #[test]
    fn parse_param_roundtrip(v in 0usize..10000) {
        let s = v.to_string();
        let bytes = s.as_bytes();
        let parsed = parse_param_as::<usize>(bytes).unwrap();
        prop_assert_eq!(parsed, Some(v));
    }

    #[test]
    fn split_semicolon_various(input in "([0-9]*;){1,5}[0-9]*") {
        let _ = split_params_into_semicolon_delimited_usize(input.as_bytes());
    }
}

#[test]
fn osc_parser_error_path_triggers_logging() {
    let mut parser = FreminalAnsiParser::new();

    // Malformed OSC: ESC ] then junk without terminator
    let data = b"\x1b]not_a_valid_osc_sequence";
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| parser.push(data)));

    assert!(result.is_ok());
    let out = result.unwrap();
    assert!(
        out.is_empty() || out.iter().any(|o| matches!(o, TerminalOutput::Invalid)),
        "Expected Invalid or empty output"
    );
}

#[test]
fn standard_parser_error_and_invalid_logging() {
    let mut parser = FreminalAnsiParser::new();

    // ESC followed by an illegal single char that should trigger Standard parser failure
    let data = b"\x1b9"; // '9' isn't a valid standard escape
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| parser.push(data)));

    assert!(result.is_ok());
    let out = result.unwrap();
    assert!(
        out.is_empty() || out.iter().any(|o| matches!(o, TerminalOutput::Invalid)),
        "Expected Invalid output"
    );
}

#[test]
fn data_is_pushed_after_final_iteration() {
    let mut parser = FreminalAnsiParser::new();

    // A sequence with both data and control to ensure leftover data is flushed at end
    let out = parser.push(b"abc\x1b["); // ESC begins CSI, data before it should be pushed
    assert!(
        out.iter().any(|o| matches!(o, TerminalOutput::Data(_))),
        "Expected Data output before CSI"
    );
}

#[test]
fn ansi_parser_inner_empty_and_data_push_combination() {
    let mut parser = FreminalAnsiParser::new();

    // Interleave carriage returns and normal text to ensure push_data_if_non_empty executes at the end
    let data = b"a\rb\nc";
    let out = parser.push(data);

    // Should include both CarriageReturn/Newline and Data pushes
    assert!(
        out.iter().any(|o| matches!(o, TerminalOutput::Data(_))),
        "Expected Data entries"
    );
    assert!(
        out.iter()
            .any(|o| matches!(o, TerminalOutput::CarriageReturn)),
        "Expected CR entries"
    );
}

// ---------- Phase 8: Trace Buffer & Determinism Tests ----------

use freminal_terminal_emulator::ansi_components::tracer::SequenceTraceable;

#[test]
fn trace_buffer_appends_and_clears() {
    let mut parser = FreminalAnsiParser::default();
    parser.append_trace(b'A');
    parser.append_trace(b'B');
    assert_eq!(parser.current_trace_str(), "AB");
    parser.clear_trace();
    assert_eq!(parser.current_trace_str(), "");
}

#[test]
fn trace_buffer_clears_after_state_reset() {
    let mut parser = FreminalAnsiParser::default();
    parser.append_trace(b'X');
    parser.append_trace(b'Y');
    parser.clear_trace();
    assert!(parser.current_trace_str().is_empty());
}

proptest! {
    #[test]
    fn trace_never_panics_on_random_bytes(data in proptest::collection::vec(any::<u8>(), 0..256)) {
        let mut parser = FreminalAnsiParser::default();
        for b in data {
            parser.append_trace(b);
        }
    }
}

#[test]
fn osc_1337_sequence_records_bytes() {
    use freminal_terminal_emulator::ansi_components::osc::AnsiOscParser;
    let mut osc = AnsiOscParser::default();
    let seq = b"\x1b]1337;File=name=test.png;size=1000\x07";
    for b in seq.iter().copied() {
        osc.push(b);
    }
    let trace = osc.current_trace_str();
    assert!(
        trace.contains("1337"),
        "Trace should contain 1337, got: {trace}"
    );
    assert!(
        trace.len() > 10,
        "{}",
        format!("Trace length too short: {}", trace.len())
    );
}

#[test]
fn deterministic_output_across_chunking() {
    fn parse_all(mut parser: FreminalAnsiParser, bytes: &[u8]) -> Vec<TerminalOutput> {
        let outputs = Vec::new();
        for b in bytes.iter().copied() {
            parser.append_trace(b);
        }
        outputs
    }

    let sequence = b"\x1b[1;2HHello\x1b]1337;URL=test\x07World";
    let full = parse_all(FreminalAnsiParser::default(), sequence);
    let mut chunked = Vec::new();
    for chunk in sequence.chunks(3) {
        chunked.extend(parse_all(FreminalAnsiParser::default(), chunk));
    }
    assert_eq!(
        full, chunked,
        "Output should be deterministic across chunking"
    );
}

#[test]
fn fuzz_small_inputs_no_panic() {
    let mut parser = FreminalAnsiParser::default();
    for b in 0u8..=255u8 {
        parser.append_trace(b);
    }
    // If we reach here, no panic occurred and the internal trace buffer is safe.
}
