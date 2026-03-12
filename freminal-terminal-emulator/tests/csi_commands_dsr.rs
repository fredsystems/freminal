// Copyright (C) 2024-2026 Fred Clausen
// MIT license, see LICENSE file.

use freminal_common::buffer_states::terminal_output::TerminalOutput;
use freminal_terminal_emulator::ansi::FreminalAnsiParser;

fn push_seq(seq: &str) -> Vec<TerminalOutput> {
    let mut parser = FreminalAnsiParser::default();
    parser.push(seq.as_bytes())
}

// ── Standard DSR (ESC[Psn) ───────────────────────────────────────────

#[test]
fn dsr_ps5_produces_device_status_report() {
    let outputs = push_seq("\x1b[5n");
    assert!(
        outputs
            .iter()
            .any(|o| matches!(o, TerminalOutput::DeviceStatusReport)),
        "ESC[5n must produce DeviceStatusReport, got: {outputs:?}"
    );
}

#[test]
fn dsr_ps6_produces_cursor_report() {
    let outputs = push_seq("\x1b[6n");
    assert!(
        outputs
            .iter()
            .any(|o| matches!(o, TerminalOutput::CursorReport)),
        "ESC[6n must produce CursorReport, got: {outputs:?}"
    );
}

// ── DEC private DSR (ESC[?Psn) ───────────────────────────────────────

#[test]
fn dec_private_dsr_ps5_produces_device_status_report() {
    let outputs = push_seq("\x1b[?5n");
    assert!(
        outputs
            .iter()
            .any(|o| matches!(o, TerminalOutput::DeviceStatusReport)),
        "ESC[?5n must produce DeviceStatusReport, got: {outputs:?}"
    );
}

#[test]
fn dec_private_dsr_ps6_produces_cursor_report() {
    let outputs = push_seq("\x1b[?6n");
    assert!(
        outputs
            .iter()
            .any(|o| matches!(o, TerminalOutput::CursorReport)),
        "ESC[?6n must produce CursorReport, got: {outputs:?}"
    );
}

// ── Default parameter (no Ps digit) ─────────────────────────────────

#[test]
fn dsr_default_produces_device_status_report() {
    // ESC[n with no parameter defaults to Ps=5 (device status)
    let outputs = push_seq("\x1b[n");
    assert!(
        outputs
            .iter()
            .any(|o| matches!(o, TerminalOutput::DeviceStatusReport)),
        "ESC[n (default) must produce DeviceStatusReport, got: {outputs:?}"
    );
}

// ── Unknown Ps value ────────────────────────────────────────────────

#[test]
fn dsr_unknown_ps_produces_invalid() {
    let outputs = push_seq("\x1b[99n");
    assert!(
        outputs.iter().any(|o| matches!(o, TerminalOutput::Invalid)),
        "ESC[99n must produce Invalid, got: {outputs:?}"
    );
}

#[test]
fn dec_private_dsr_unknown_ps_produces_invalid() {
    let outputs = push_seq("\x1b[?99n");
    assert!(
        outputs.iter().any(|o| matches!(o, TerminalOutput::Invalid)),
        "ESC[?99n must produce Invalid, got: {outputs:?}"
    );
}

// ── DSR ?996 (Color Theme Query) ────────────────────────────────────

#[test]
fn dec_private_dsr_ps996_produces_color_theme_report() {
    let outputs = push_seq("\x1b[?996n");
    assert!(
        outputs
            .iter()
            .any(|o| matches!(o, TerminalOutput::ColorThemeReport)),
        "ESC[?996n must produce ColorThemeReport, got: {outputs:?}"
    );
}

#[test]
fn standard_dsr_ps996_does_not_produce_color_theme_report() {
    // DSR 996 without the ? prefix is not a valid color theme query.
    // It should produce Invalid, not ColorThemeReport.
    let outputs = push_seq("\x1b[996n");
    assert!(
        !outputs
            .iter()
            .any(|o| matches!(o, TerminalOutput::ColorThemeReport)),
        "ESC[996n (no ? prefix) must NOT produce ColorThemeReport, got: {outputs:?}"
    );
    assert!(
        outputs.iter().any(|o| matches!(o, TerminalOutput::Invalid)),
        "ESC[996n (no ? prefix) must produce Invalid, got: {outputs:?}"
    );
}
