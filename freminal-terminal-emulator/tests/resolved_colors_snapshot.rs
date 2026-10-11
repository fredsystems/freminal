// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! `TerminalSnapshot::resolved_colors`: the content colours reach the snapshot,
//! the `Arc` is shared until a content colour changes, and the colour
//! sequences and resets drive it as specified.

use std::sync::Arc;

use freminal_common::{pty_write::PtyWrite, themes::CATPPUCCIN_MOCHA};
use freminal_terminal_emulator::interface::TerminalEmulator;

/// Headless emulator; keep the receiver alive so OSC replies do not error.
fn make_emulator() -> (TerminalEmulator, crossbeam_channel::Receiver<PtyWrite>) {
    TerminalEmulator::new_headless(None)
}

const DECSTR: &[u8] = b"\x1b[!p";
const RIS: &[u8] = b"\x1bc";

#[test]
fn consecutive_snapshots_with_no_input_share_the_arc() {
    let (mut emu, _rx) = make_emulator();
    let a = emu.build_snapshot();
    let b = emu.build_snapshot();
    assert!(Arc::ptr_eq(&a.resolved_colors, &b.resolved_colors));
}

#[test]
fn ordinary_output_does_not_replace_the_arc() {
    let (mut emu, _rx) = make_emulator();
    let a = emu.build_snapshot();
    emu.handle_incoming_data(b"hello\r\n");
    let b = emu.build_snapshot();
    assert!(Arc::ptr_eq(&a.resolved_colors, &b.resolved_colors));
}

#[test]
fn default_snapshot_carries_theme_colors() {
    let (mut emu, _rx) = make_emulator();
    let snap = emu.build_snapshot();
    assert_eq!(snap.resolved_colors.foreground, CATPPUCCIN_MOCHA.foreground);
    assert_eq!(snap.resolved_colors.background, CATPPUCCIN_MOCHA.background);
    assert_eq!(snap.resolved_colors.palette[1], CATPPUCCIN_MOCHA.ansi[1]);
}

#[test]
fn osc4_set_changes_palette_and_replaces_the_arc() {
    let (mut emu, _rx) = make_emulator();
    let before = emu.build_snapshot();
    emu.handle_incoming_data(b"\x1b]4;1;rgb:ff/00/00\x1b\\");
    let after = emu.build_snapshot();
    assert_eq!(after.resolved_colors.palette[1], (255, 0, 0));
    assert!(!Arc::ptr_eq(
        &before.resolved_colors,
        &after.resolved_colors
    ));
}

#[test]
fn osc4_set_to_the_same_value_keeps_the_arc() {
    let (mut emu, _rx) = make_emulator();
    emu.handle_incoming_data(b"\x1b]4;1;rgb:ff/00/00\x1b\\");
    let a = emu.build_snapshot();
    emu.handle_incoming_data(b"\x1b]4;1;rgb:ff/00/00\x1b\\");
    let b = emu.build_snapshot();
    assert!(Arc::ptr_eq(&a.resolved_colors, &b.resolved_colors));
}

#[test]
fn osc104_resets_the_palette_entry() {
    let (mut emu, _rx) = make_emulator();
    emu.handle_incoming_data(b"\x1b]4;1;rgb:ff/00/00\x1b\\");
    let set = emu.build_snapshot();
    emu.handle_incoming_data(b"\x1b]104;1\x1b\\");
    let reset = emu.build_snapshot();
    assert_eq!(set.resolved_colors.palette[1], (255, 0, 0));
    assert_eq!(reset.resolved_colors.palette[1], CATPPUCCIN_MOCHA.ansi[1]);
    assert!(!Arc::ptr_eq(&set.resolved_colors, &reset.resolved_colors));
}

#[test]
fn osc10_set_changes_foreground_and_osc110_resets_it() {
    let (mut emu, _rx) = make_emulator();
    let before = emu.build_snapshot();
    emu.handle_incoming_data(b"\x1b]10;rgb:12/34/56\x1b\\");
    let set = emu.build_snapshot();
    assert_eq!(set.resolved_colors.foreground, (0x12, 0x34, 0x56));
    assert!(!Arc::ptr_eq(&before.resolved_colors, &set.resolved_colors));

    emu.handle_incoming_data(b"\x1b]110\x1b\\");
    let reset = emu.build_snapshot();
    assert_eq!(
        reset.resolved_colors.foreground,
        CATPPUCCIN_MOCHA.foreground
    );
    assert!(!Arc::ptr_eq(&set.resolved_colors, &reset.resolved_colors));
}

#[test]
fn osc11_set_changes_background() {
    let (mut emu, _rx) = make_emulator();
    emu.handle_incoming_data(b"\x1b]11;rgb:01/02/03\x1b\\");
    let snap = emu.build_snapshot();
    assert_eq!(snap.resolved_colors.background, (1, 2, 3));
}

#[test]
fn ris_clears_palette_and_dynamic_overrides() {
    let (mut emu, _rx) = make_emulator();
    emu.handle_incoming_data(b"\x1b]4;1;rgb:ff/00/00\x1b\\");
    emu.handle_incoming_data(b"\x1b]10;rgb:12/34/56\x1b\\");
    emu.handle_incoming_data(b"\x1b]12;rgb:aa/bb/cc\x1b\\");
    let set = emu.build_snapshot();
    assert_eq!(set.resolved_colors.palette[1], (255, 0, 0));
    assert_eq!(set.resolved_colors.foreground, (0x12, 0x34, 0x56));
    assert_eq!(set.cursor_color_override, Some((0xaa, 0xbb, 0xcc)));

    emu.handle_incoming_data(RIS);
    let reset = emu.build_snapshot();
    assert_eq!(reset.resolved_colors.palette[1], CATPPUCCIN_MOCHA.ansi[1]);
    assert_eq!(
        reset.resolved_colors.foreground,
        CATPPUCCIN_MOCHA.foreground
    );
    assert_eq!(reset.cursor_color_override, None);
}

#[test]
fn decstr_resets_palette_but_keeps_the_osc10_override() {
    let (mut emu, _rx) = make_emulator();
    emu.handle_incoming_data(b"\x1b]4;1;rgb:ff/00/00\x1b\\");
    emu.handle_incoming_data(b"\x1b]10;rgb:12/34/56\x1b\\");
    let set = emu.build_snapshot();
    assert_eq!(set.resolved_colors.palette[1], (255, 0, 0));

    emu.handle_incoming_data(DECSTR);
    let after = emu.build_snapshot();
    assert_eq!(after.resolved_colors.palette[1], CATPPUCCIN_MOCHA.ansi[1]);
    assert_eq!(after.resolved_colors.foreground, (0x12, 0x34, 0x56));
}

#[test]
fn osc12_changes_cursor_override_but_not_the_resolved_arc() {
    let (mut emu, _rx) = make_emulator();
    let before = emu.build_snapshot();
    assert_eq!(before.cursor_color_override, None);

    emu.handle_incoming_data(b"\x1b]12;rgb:aa/bb/cc\x1b\\");
    let after = emu.build_snapshot();
    assert_eq!(after.cursor_color_override, Some((0xaa, 0xbb, 0xcc)));
    assert!(Arc::ptr_eq(&before.resolved_colors, &after.resolved_colors));
}
