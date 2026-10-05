// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Guards the Task 125 `sustained-output-marked` measurement workload.
//!
//! That workload (`assets/profiling/task125/workloads.sh`) wraps every output
//! burst in OSC 133 marks so each terminal records prompts and command blocks.
//! Freminal silently ignores any OSC 133 `A`/`B`/`C`/`D` marker that lacks
//! `freminal=1;fid=<id>`, so an untagged workload would measure plain
//! `sustained-output` while being labelled as the command-block case -- a
//! failure that produces plausible-looking numbers.
//!
//! These tests extract the workload's own printf formats from the shell script
//! (the single source of truth), expand them exactly as `printf` does, replay
//! the resulting bytes through a headless emulator, and assert that prompt rows
//! and finished command blocks are really recorded.

use freminal_terminal_emulator::interface::TerminalEmulator;

const WORKLOADS_SH: &str = include_str!("../../assets/profiling/task125/workloads.sh");

/// Read the single-quoted value of `readonly <name>='...'` from the script.
fn script_format(name: &str) -> String {
    let prefix = format!("readonly {name}='");
    let line = WORKLOADS_SH
        .lines()
        .find(|l| l.starts_with(&prefix))
        .unwrap_or_else(|| panic!("workloads.sh no longer defines {name}"));
    let value = &line[prefix.len()..];
    value
        .strip_suffix('\'')
        .unwrap_or_else(|| panic!("{name} is not a single-quoted assignment"))
        .to_owned()
}

/// Expand a printf format the way bash's `printf` does for the subset the
/// workload uses: `\NNN` octal escapes and a single `%d` per occurrence.
fn expand_printf(format: &str, id: u64) -> Vec<u8> {
    let bytes = format.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => {
                let digits = &format[i + 1..];
                let octal: String = digits.chars().take(3).collect();
                assert!(
                    octal.len() == 3 && octal.chars().all(|c| ('0'..='7').contains(&c)),
                    "unsupported printf escape in {format:?}"
                );
                out.push(u8::from_str_radix(&octal, 8).expect("octal escape"));
                i += 4;
            }
            b'%' => {
                assert_eq!(bytes.get(i + 1), Some(&b'd'), "only %d is supported");
                out.extend_from_slice(id.to_string().as_bytes());
                i += 2;
            }
            other => {
                out.push(other);
                i += 1;
            }
        }
    }
    out
}

/// One burst exactly as the PTY delivers it: the marks, then `seq 1 200` with
/// the line discipline's `\n` -> `\r\n` translation.
fn burst(id: u64) -> Vec<u8> {
    let mut bytes = expand_printf(&script_format("TASK125_MARKED_PROMPT_FORMAT"), id);
    for n in 1..=200 {
        bytes.extend_from_slice(format!("{n}\r\n").as_bytes());
    }
    bytes.extend(expand_printf(
        &script_format("TASK125_MARKED_FINISH_FORMAT"),
        id,
    ));
    bytes
}

#[test]
fn expanded_formats_are_the_tagged_osc_133_sequences() {
    assert_eq!(
        expand_printf(&script_format("TASK125_MARKED_PROMPT_FORMAT"), 7),
        b"\x1b]133;A;freminal=1;fid=7\x07Task125 prompt>\
          \x1b]133;B;freminal=1;fid=7\x07\
          \x1b]133;C;freminal=1;fid=7\x07"
            .as_slice()
    );
    assert_eq!(
        expand_printf(&script_format("TASK125_MARKED_FINISH_FORMAT"), 7),
        b"\x1b]133;D;0;freminal=1;fid=7\x07".as_slice()
    );
}

#[test]
fn marked_workload_records_prompt_rows_and_finished_command_blocks() {
    const BURSTS: u64 = 30;
    let (mut emu, _rx) = TerminalEmulator::new_headless(None);
    for id in 1..=BURSTS {
        emu.handle_incoming_data(&burst(id));
    }
    let snap = emu.build_snapshot();
    let expected = usize::try_from(BURSTS).expect("burst count fits usize");

    assert_eq!(
        snap.prompt_rows.len(),
        expected,
        "every burst's OSC 133 A must record a prompt row"
    );
    assert_eq!(
        snap.command_blocks.len(),
        expected,
        "every burst must open and close its own command block"
    );
    for (index, block) in snap.command_blocks.iter().enumerate() {
        assert_eq!(block.fid, (index + 1).to_string());
        assert!(block.command_start_row.is_some(), "B must be recorded");
        assert!(block.output_start_row.is_some(), "C must be recorded");
        assert!(block.end_row.is_some(), "D must finish the block");
        assert_eq!(block.exit_code, Some(0), "D;0 must record exit code 0");
    }
}

#[test]
fn bare_osc_133_marks_would_record_nothing() {
    // The pre-fix workload bytes. Pinning this documents *why* the tag is
    // required, so nobody "simplifies" the script back to bare marks.
    let (mut emu, _rx) = TerminalEmulator::new_headless(None);
    emu.handle_incoming_data(b"\x1b]133;A\x07Task125 prompt>\x1b]133;B\x07\x1b]133;C\x07");
    emu.handle_incoming_data(b"1\r\n2\r\n\x1b]133;D;0\x07");
    let snap = emu.build_snapshot();
    assert!(snap.prompt_rows.is_empty());
    assert!(snap.command_blocks.is_empty());
}
