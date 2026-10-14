// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! `TerminalSnapshot::resolved_colors`: the content colours reach the snapshot,
//! the `Arc` is shared until a content colour changes, and the colour
//! sequences and resets drive it as specified.

use std::sync::Arc;

use freminal_common::{
    colors::TerminalColor,
    pty_write::PtyWrite,
    themes::{CATPPUCCIN_MOCHA, DRACULA},
};
use freminal_terminal_emulator::{interface::TerminalEmulator, snapshot::TerminalSnapshot};

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
fn ris_clears_the_selection_overrides() {
    let (mut emu, _rx) = make_emulator();
    emu.handle_incoming_data(b"\x1b]17;rgb:11/22/33\x1b\\");
    emu.handle_incoming_data(b"\x1b]19;rgb:44/55/66\x1b\\");
    let set = emu.build_snapshot();
    assert_eq!(set.resolved_colors.selection_background, (0x11, 0x22, 0x33));
    assert_eq!(set.resolved_colors.selection_foreground, (0x44, 0x55, 0x66));
    assert_ne!(
        set.resolved_colors.selection_background,
        CATPPUCCIN_MOCHA.selection_bg
    );
    assert_ne!(
        set.resolved_colors.selection_foreground,
        CATPPUCCIN_MOCHA.selection_fg
    );

    emu.handle_incoming_data(RIS);
    let reset = emu.build_snapshot();
    assert_eq!(
        reset.resolved_colors.selection_background,
        CATPPUCCIN_MOCHA.selection_bg
    );
    assert_eq!(
        reset.resolved_colors.selection_foreground,
        CATPPUCCIN_MOCHA.selection_fg
    );
}

#[test]
fn decstr_keeps_the_selection_overrides() {
    let (mut emu, _rx) = make_emulator();
    emu.handle_incoming_data(b"\x1b]17;rgb:11/22/33\x1b\\");
    emu.handle_incoming_data(b"\x1b]19;rgb:44/55/66\x1b\\");

    emu.handle_incoming_data(DECSTR);
    let after = emu.build_snapshot();
    assert_eq!(
        after.resolved_colors.selection_background,
        (0x11, 0x22, 0x33)
    );
    assert_eq!(
        after.resolved_colors.selection_foreground,
        (0x44, 0x55, 0x66)
    );
}

#[test]
fn three_value_osc10_chain_sets_foreground_background_and_cursor() {
    let (mut emu, _rx) = make_emulator();
    emu.handle_incoming_data(b"\x1b]10;#010203;#040506;#070809\x1b\\");
    let snap = emu.build_snapshot();
    assert_eq!(snap.resolved_colors.foreground, (1, 2, 3));
    assert_eq!(snap.resolved_colors.background, (4, 5, 6));
    assert_eq!(snap.cursor_color_override, Some((7, 8, 9)));
}

#[test]
fn four_value_osc10_chain_ignores_the_value_that_lands_on_code_13() {
    let (mut emu, _rx) = make_emulator();
    emu.handle_incoming_data(b"\x1b]10;#010203;#040506;#070809;#0a0b0c\x1b\\");
    let snap = emu.build_snapshot();
    // The fourth value targets OSC 13 (pointer foreground), which is ignored.
    assert_eq!(snap.resolved_colors.foreground, (1, 2, 3));
    assert_eq!(snap.resolved_colors.background, (4, 5, 6));
    assert_eq!(snap.cursor_color_override, Some((7, 8, 9)));
    assert_eq!(
        snap.resolved_colors.selection_background,
        CATPPUCCIN_MOCHA.selection_bg
    );
    assert_eq!(
        snap.resolved_colors.selection_foreground,
        CATPPUCCIN_MOCHA.selection_fg
    );
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

// --- Palette-index cells keep the index; the renderer resolves it -----------

/// The foreground colour stored for the first `X` in the snapshot.
// Test helper; not a #[test] fn, so clippy.toml's allow-*-in-tests does not cover it.
#[allow(clippy::expect_used)]
fn fg_of_first_x(snap: &TerminalSnapshot) -> TerminalColor {
    let pos = snap
        .visible_chars
        .iter()
        .position(|c| *c == freminal_common::buffer_states::tchar::TChar::Ascii(b'X'))
        .expect("snapshot must contain the written `X`");
    snap.visible_tags
        .iter()
        .find(|t| t.start <= pos && pos < t.end)
        .expect("a format tag must cover the written `X`")
        .colors
        .color
}

/// Drain every PTY reply the emulator wrote, concatenated and lossily decoded.
fn drain_replies(rx: &crossbeam_channel::Receiver<PtyWrite>) -> String {
    let mut out = Vec::new();
    while let Ok(msg) = rx.try_recv() {
        if let PtyWrite::Write(bytes) = msg {
            out.extend_from_slice(&bytes);
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[test]
fn osc4_recolours_an_existing_palette_index_cell_without_rewriting_it() {
    let (mut emu, _rx) = make_emulator();
    emu.handle_incoming_data(b"\x1b[38;5;1mX");
    let before = emu.build_snapshot();

    let cell_fg = fg_of_first_x(&before);
    assert_eq!(cell_fg, TerminalColor::PaletteIndex(1));
    assert_eq!(
        before.resolved_colors.rgb(cell_fg),
        CATPPUCCIN_MOCHA.ansi[1]
    );

    emu.handle_incoming_data(b"\x1b]4;1;rgb:ff/00/00\x1b\\");
    let after = emu.build_snapshot();

    // The very same stored cell value now resolves to the new colour ...
    assert_eq!(fg_of_first_x(&after), TerminalColor::PaletteIndex(1));
    assert_eq!(after.resolved_colors.rgb(cell_fg), (255, 0, 0));
    // ... and no row was rewritten: it is a render-time lookup.
    assert_eq!(before.row_epochs, after.row_epochs);
}

#[test]
fn theme_change_recolours_an_existing_palette_index_cell() {
    let (mut emu, _rx) = make_emulator();
    emu.handle_incoming_data(b"\x1b[38;5;3mX");
    let before = emu.build_snapshot();
    let cell_fg = fg_of_first_x(&before);
    assert_eq!(cell_fg, TerminalColor::PaletteIndex(3));
    assert_eq!(
        before.resolved_colors.rgb(cell_fg),
        CATPPUCCIN_MOCHA.ansi[3]
    );
    assert_ne!(CATPPUCCIN_MOCHA.ansi[3], DRACULA.ansi[3]);

    emu.internal.handler.set_theme(&DRACULA);
    let after = emu.build_snapshot();

    assert_eq!(fg_of_first_x(&after), TerminalColor::PaletteIndex(3));
    assert_eq!(after.resolved_colors.rgb(cell_fg), DRACULA.ansi[3]);
    assert_eq!(before.row_epochs, after.row_epochs);
}

#[test]
fn decrqss_sgr_reports_palette_index_foreground() {
    let (mut emu, rx) = make_emulator();
    emu.handle_incoming_data(b"\x1b[38;5;196m");
    emu.handle_incoming_data(b"\x1bP$qm\x1b\\");
    let reply = drain_replies(&rx);
    assert!(reply.contains("38;5;196"), "reply: {reply:?}");
}

#[test]
fn decrqss_sgr_reports_palette_index_background() {
    let (mut emu, rx) = make_emulator();
    emu.handle_incoming_data(b"\x1b[48;5;21m");
    emu.handle_incoming_data(b"\x1bP$qm\x1b\\");
    let reply = drain_replies(&rx);
    assert!(reply.contains("48;5;21"), "reply: {reply:?}");
}

#[test]
fn decrqss_sgr_reports_palette_index_underline_colour() {
    let (mut emu, rx) = make_emulator();
    emu.handle_incoming_data(b"\x1b[58;5;7m");
    emu.handle_incoming_data(b"\x1bP$qm\x1b\\");
    let reply = drain_replies(&rx);
    assert!(reply.contains("58;5;7"), "reply: {reply:?}");
}

#[test]
fn one_osc4_sequence_updates_several_palette_entries() {
    let (mut emu, _rx) = make_emulator();
    emu.handle_incoming_data(b"\x1b]4;1;red;2;#00ff00;3;blue\x1b\\");
    let snap = emu.build_snapshot();
    assert_eq!(snap.resolved_colors.palette[1], (255, 0, 0));
    assert_eq!(snap.resolved_colors.palette[2], (0, 255, 0));
    assert_eq!(snap.resolved_colors.palette[3], (0, 0, 255));
}

#[test]
fn osc104_with_indices_resets_only_those_entries() {
    let (mut emu, _rx) = make_emulator();
    emu.handle_incoming_data(b"\x1b]4;1;red;2;#00ff00;3;blue\x1b\\");
    emu.handle_incoming_data(b"\x1b]104;1;3\x1b\\");
    let snap = emu.build_snapshot();
    assert_eq!(snap.resolved_colors.palette[1], CATPPUCCIN_MOCHA.ansi[1]);
    assert_eq!(snap.resolved_colors.palette[2], (0, 255, 0));
    assert_eq!(snap.resolved_colors.palette[3], CATPPUCCIN_MOCHA.ansi[3]);
}

#[test]
fn one_osc10_sequence_sets_foreground_and_background() {
    let (mut emu, _rx) = make_emulator();
    emu.handle_incoming_data(b"\x1b]10;red;blue\x1b\\");
    let snap = emu.build_snapshot();
    assert_eq!(snap.resolved_colors.foreground, (255, 0, 0));
    assert_eq!(snap.resolved_colors.background, (0, 0, 255));
}

#[test]
fn osc110_and_osc111_reset_the_foreground_and_background() {
    let (mut emu, _rx) = make_emulator();
    emu.handle_incoming_data(b"\x1b]10;red;blue\x1b\\");
    emu.handle_incoming_data(b"\x1b]110\x1b\\\x1b]111\x1b\\");
    let snap = emu.build_snapshot();
    assert_eq!(snap.resolved_colors.foreground, CATPPUCCIN_MOCHA.foreground);
    assert_eq!(snap.resolved_colors.background, CATPPUCCIN_MOCHA.background);
}
