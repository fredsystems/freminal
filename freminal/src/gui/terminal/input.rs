// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Keyboard and mouse input translation from egui events to terminal bytes.

use crate::gui::{
    mouse::{
        FreminalMousePosition, PreviousMouseState, handle_pointer_button, handle_pointer_moved,
        handle_pointer_scroll,
    },
    view_state::{CellCoord, PendingPaste, ViewState},
};

use conv2::ConvUtil;
use crossbeam_channel::Sender;
use egui::{Event, InputState, Key, Modifiers, PointerButton, Rect};
use freminal_common::buffer_states::command_block::{CommandBlock, CommandBlockId};
use freminal_common::buffer_states::modes::{
    application_escape_key::ApplicationEscapeKey, decarm::Decarm, decbkm::Decbkm, decckm::Decckm,
    keypad::KeypadMode, lnm::Lnm, mouse::MouseTrack,
};
use freminal_common::keybindings::{BindingKey, BindingMap, BindingModifiers, KeyAction, KeyCombo};
use freminal_common::send_or_log;
use freminal_terminal_emulator::{
    input::{
        KKP_KP_0_CODEPOINT, KKP_KP_1_CODEPOINT, KKP_KP_2_CODEPOINT, KKP_KP_3_CODEPOINT,
        KKP_KP_4_CODEPOINT, KKP_KP_5_CODEPOINT, KKP_KP_6_CODEPOINT, KKP_KP_7_CODEPOINT,
        KKP_KP_8_CODEPOINT, KKP_KP_9_CODEPOINT, KKP_KP_ADD_CODEPOINT, KKP_KP_DECIMAL_CODEPOINT,
        KKP_KP_DIVIDE_CODEPOINT, KKP_KP_ENTER_CODEPOINT, KKP_KP_EQUAL_CODEPOINT,
        KKP_KP_MULTIPLY_CODEPOINT, KKP_KP_SEPARATOR_CODEPOINT, KKP_KP_SUBTRACT_CODEPOINT,
        KKP_LOWER_VOLUME_CODEPOINT, KKP_MEDIA_PLAY_PAUSE_CODEPOINT, KKP_MEDIA_STOP_CODEPOINT,
        KKP_MEDIA_TRACK_NEXT_CODEPOINT, KKP_MEDIA_TRACK_PREVIOUS_CODEPOINT, KKP_MENU_CODEPOINT,
        KKP_MUTE_VOLUME_CODEPOINT, KKP_PAUSE_CODEPOINT, KKP_PRINT_SCREEN_CODEPOINT,
        KKP_RAISE_VOLUME_CODEPOINT, KeyEventMeta, KeyEventType, KeyModifiers, TerminalInput,
        TerminalInputPayload, collect_text,
    },
    io::InputEvent,
    recording::{EventPayload, RecordingContext},
    snapshot::TerminalSnapshot,
};
use std::borrow::Cow;

use super::coords::{
    encode_egui_mouse_pos_as_usize, visible_window_start, visible_window_start_for,
};
use super::widget::hit_test_placeholder;
use crate::gui::folding::{compute_extra_rows, compute_fold_ranges};

/// Build an [`InputEvent::ScrollOffset`] for a target raw scroll offset,
/// computing the fold-aware `extra_rows` the renderer needs at that offset.
///
/// `extra_rows` is the number of rows that must be flattened above the normal
/// visible window so the screen stays full after collapsing every fold that
/// overlaps the window (see [`compute_extra_rows`]). When no fold is in view
/// it is `0` and this is equivalent to the old `ScrollOffset(offset)`.
pub fn scroll_event(
    snap: &TerminalSnapshot,
    folded_blocks: &std::collections::HashSet<CommandBlockId>,
    offset: usize,
) -> InputEvent {
    let extra_rows = if folded_blocks.is_empty() {
        0
    } else {
        let ranges = compute_fold_ranges(&snap.command_blocks, folded_blocks);
        let win_start = visible_window_start_for(snap, offset);
        compute_extra_rows(&ranges, win_start, snap.term_height)
    };
    InputEvent::ScrollOffset { offset, extra_rows }
}

/// Map an on-screen row (0 = top of the painted terminal area) to a
/// buffer-absolute row, accounting for command-block folds and the
/// bottom-anchored extra-row window.
///
/// Mirrors the renderer's `FoldLayout`: screen → rendered → snapshot →
/// buffer. When the screen row lands on a fold placeholder, the folded block's
/// first row is returned (so a click/selection on the placeholder anchors at
/// the block). When no fold is in view this is exactly
/// `visible_window_start(snap) + screen_row`.
pub(super) fn screen_row_to_buffer_row(
    snap: &TerminalSnapshot,
    folded_blocks: &std::collections::HashSet<CommandBlockId>,
    screen_row: usize,
) -> usize {
    if folded_blocks.is_empty() && snap.window_extra_rows == 0 {
        return visible_window_start(snap) + screen_row;
    }
    let ranges = compute_fold_ranges(&snap.command_blocks, folded_blocks);
    let flat_window_start = visible_window_start(snap).saturating_sub(snap.window_extra_rows);
    let snap_rows = snap.term_height.saturating_add(snap.window_extra_rows);
    let translated = crate::gui::folding::translate_ranges_to_snapshot(&ranges, flat_window_start);
    let row_map = crate::gui::folding::RowMap::new(snap_rows, &translated);
    let render_skip = row_map
        .rendered_row_count()
        .saturating_sub(snap.term_height);
    let rendered = screen_row.saturating_add(render_skip);
    match row_map.rendered_to_snapshot(rendered) {
        Some(crate::gui::folding::RenderedRow::Snapshot(snap_row)) => flat_window_start + snap_row,
        Some(crate::gui::folding::RenderedRow::Placeholder(range)) => {
            flat_window_start + range.start_row
        }
        // Below the last rendered row → clamp to the live bottom.
        None => visible_window_start(snap) + snap.term_height.saturating_sub(1),
    }
}

/// Compute a new raw scroll offset for a scroll request expressed in
/// **rendered rows** (visible lines), skipping over rows hidden inside
/// collapsed folds so each step moves the view by exactly one visible line.
///
/// Delegates to [`crate::gui::folding::apply_rendered_scroll`]. When no fold is
/// in view this is exactly `current ± steps` (clamped), matching the unfolded
/// behaviour.
pub(super) fn scrolled_offset(
    snap: &TerminalSnapshot,
    folded_blocks: &std::collections::HashSet<CommandBlockId>,
    current: usize,
    dir: crate::gui::folding::ScrollDir,
    steps: usize,
) -> usize {
    if folded_blocks.is_empty() {
        return match dir {
            crate::gui::folding::ScrollDir::Up => {
                current.saturating_add(steps).min(snap.max_scroll_offset)
            }
            crate::gui::folding::ScrollDir::Down => current.saturating_sub(steps),
        };
    }
    let ranges = compute_fold_ranges(&snap.command_blocks, folded_blocks);
    crate::gui::folding::apply_rendered_scroll(
        &ranges,
        snap.total_rows,
        snap.term_height,
        snap.max_scroll_offset,
        current,
        dir,
        steps,
    )
}

/// Re-send the current scroll window to the PTY so the snapshot's
/// `window_extra_rows` is recomputed after a fold/unfold action.
///
/// Folding does not change `view_state.scroll_offset`, but it does change how
/// many extra rows the renderer needs above the window. Without this resend
/// the new extra-row count would not take effect until the user's next scroll
/// event. Sends at the *current* offset.
pub(super) fn resend_scroll_window(
    snap: &TerminalSnapshot,
    view_state: &ViewState,
    input_tx: &Sender<InputEvent>,
) {
    send_or_log!(
        input_tx,
        scroll_event(snap, &view_state.folded_blocks, view_state.scroll_offset),
        "Failed to send scroll window to PTY consumer"
    );
}

/// Convert egui [`Modifiers`] to the terminal-emulator's [`KeyModifiers`].
///
/// This is used for special keys (arrows, function keys, Home/End, etc.)
/// where the xterm modifier encoding (`ESC[1;Nm…`) applies. It must NOT
/// be used for regular ASCII keys where Ctrl already produces a C0 control
/// code — that path is handled by `control_key()` / `TerminalInput::Ctrl`.
///
/// `super_held` is the caller-tracked physical Super/Windows key hold-state
/// (egui exposes no `Modifiers` bit for it on Linux/Windows — see
/// [`write_input_to_terminal`]'s `SuperLeft`/`SuperRight` tracking). On
/// macOS, `m.mac_cmd` is true whenever the physical ⌘ key is held, so it is
/// `OR`ed in directly. `m.command` is deliberately NOT folded into `ctrl`
/// here: on Linux/Windows `m.command == m.ctrl` (so dropping the OR is a
/// no-op), and on macOS `m.command == m.mac_cmd`, which must map to
/// `super_key`, not `ctrl`.
///
/// `hyper` and `meta` remain `false`: egui has no producer for them at all,
/// so they stay an unsourced permanent gap (tracked under Task 114).
/// `caps_lock`/`num_lock` are likewise hardcoded `false` — ambient OS
/// lock-key state was reverted (see freminal#380 / winit#1426, egui#3653);
/// it cannot be sourced correctly and uniformly across platforms.
pub(super) const fn egui_mods_to_key_modifiers(m: Modifiers, super_held: bool) -> KeyModifiers {
    KeyModifiers {
        shift: m.shift,
        ctrl: m.ctrl,
        alt: m.alt,
        super_key: super_held || m.mac_cmd,
        // hyper, meta: egui has no producer for these at all — remain a
        // permanent gap (Task 114).
        hyper: false,
        meta: false,
        // caps_lock/num_lock: not sourced — see freminal#380
        // (winit#1426, egui#3653).
        caps_lock: false,
        num_lock: false,
    }
}

/// Map a blocked physical `winit` [`KeyCode`](winit::keyboard::KeyCode) (Task
/// 114.5/114.7's intercepted set) to its Kitty Keyboard Protocol codepoint.
///
/// Returns `None` for any key outside the blocked set (defensive — the
/// `on_raw_key_event` intercept in `freminal-windowing` already filters to
/// exactly this set, but this function stays total rather than assuming
/// that invariant holds forever).
///
/// `NumpadStar` maps to the same `KKP_KP_MULTIPLY_CODEPOINT` as
/// `NumpadMultiply` — the kitty spec has one "keypad multiply" codepoint and
/// winit's `NumpadStar`/`NumpadMultiply` distinction (layout-dependent
/// physical labeling) collapses to it.
///
/// `ISO_Level3_Shift`/`ISO_Level5_Shift` have no `winit::keyboard::KeyCode`
/// variant at all (114.5 finding) and are therefore not representable here —
/// they remain a documented permanent gap (Task 114.9), same tier as
/// hyper/meta.
#[must_use]
pub const fn kitty_keycode_to_codepoint(key: winit::keyboard::KeyCode) -> Option<u32> {
    use winit::keyboard::KeyCode;
    match key {
        // System keys.
        KeyCode::PrintScreen => Some(KKP_PRINT_SCREEN_CODEPOINT),
        KeyCode::Pause => Some(KKP_PAUSE_CODEPOINT),
        KeyCode::ContextMenu => Some(KKP_MENU_CODEPOINT),
        // Keypad operators.
        KeyCode::NumpadDivide => Some(KKP_KP_DIVIDE_CODEPOINT),
        KeyCode::NumpadMultiply | KeyCode::NumpadStar => Some(KKP_KP_MULTIPLY_CODEPOINT),
        KeyCode::NumpadSubtract => Some(KKP_KP_SUBTRACT_CODEPOINT),
        KeyCode::NumpadAdd => Some(KKP_KP_ADD_CODEPOINT),
        KeyCode::NumpadEnter => Some(KKP_KP_ENTER_CODEPOINT),
        KeyCode::NumpadEqual => Some(KKP_KP_EQUAL_CODEPOINT),
        KeyCode::NumpadComma => Some(KKP_KP_SEPARATOR_CODEPOINT),
        KeyCode::NumpadDecimal => Some(KKP_KP_DECIMAL_CODEPOINT),
        // Keypad digits.
        KeyCode::Numpad0 => Some(KKP_KP_0_CODEPOINT),
        KeyCode::Numpad1 => Some(KKP_KP_1_CODEPOINT),
        KeyCode::Numpad2 => Some(KKP_KP_2_CODEPOINT),
        KeyCode::Numpad3 => Some(KKP_KP_3_CODEPOINT),
        KeyCode::Numpad4 => Some(KKP_KP_4_CODEPOINT),
        KeyCode::Numpad5 => Some(KKP_KP_5_CODEPOINT),
        KeyCode::Numpad6 => Some(KKP_KP_6_CODEPOINT),
        KeyCode::Numpad7 => Some(KKP_KP_7_CODEPOINT),
        KeyCode::Numpad8 => Some(KKP_KP_8_CODEPOINT),
        KeyCode::Numpad9 => Some(KKP_KP_9_CODEPOINT),
        // Media keys.
        KeyCode::MediaPlayPause => Some(KKP_MEDIA_PLAY_PAUSE_CODEPOINT),
        KeyCode::MediaStop => Some(KKP_MEDIA_STOP_CODEPOINT),
        KeyCode::MediaTrackNext => Some(KKP_MEDIA_TRACK_NEXT_CODEPOINT),
        KeyCode::MediaTrackPrevious => Some(KKP_MEDIA_TRACK_PREVIOUS_CODEPOINT),
        KeyCode::AudioVolumeUp => Some(KKP_RAISE_VOLUME_CODEPOINT),
        KeyCode::AudioVolumeDown => Some(KKP_LOWER_VOLUME_CODEPOINT),
        KeyCode::AudioVolumeMute => Some(KKP_MUTE_VOLUME_CODEPOINT),
        _ => None,
    }
}

/// Convert a queued [`RawKeyMods`](freminal_windowing::RawKeyMods) into
/// [`KeyModifiers`] for encoding a raw (egui-blocked) key event (Task 114.7).
///
/// Mirrors [`egui_mods_to_key_modifiers`], but for the raw-key path: `shift`,
/// `ctrl`, and `alt` come from the queued `RawKeyMods` (reliable — sourced
/// from `state.egui.modifiers()` at intercept time). `super_key` deliberately
/// does NOT come from `RawKeyMods.super_key` — that field is
/// `egui::Modifiers::command`, which equals `ctrl` on Linux/Windows and so
/// over-reports super (114.7 binding decision). Instead it comes from the
/// caller-supplied `super_pressed`, the active pane's real physical
/// Super/Command hold-state (Task 101.2 tracking), read fresh on the render
/// path where this is called. `hyper`/`meta` remain `false` (permanent gap,
/// same as `egui_mods_to_key_modifiers`); `caps_lock`/`num_lock` are
/// likewise hardcoded `false` — not sourced, see freminal#380
/// (winit#1426, egui#3653).
#[must_use]
pub const fn raw_mods_to_key_modifiers(
    mods: freminal_windowing::RawKeyMods,
    super_pressed: bool,
) -> KeyModifiers {
    KeyModifiers {
        shift: mods.shift,
        ctrl: mods.ctrl,
        alt: mods.alt,
        super_key: super_pressed,
        hyper: false,
        meta: false,
        caps_lock: false,
        num_lock: false,
    }
}

/// Drain queued raw key events (Task 114.7) for the active pane, encoding
/// each into a [`TerminalInput::KittyFunctional`] and sending it through the
/// existing [`send_terminal_inputs`] funnel.
///
/// ## Why this runs on the render path, not inside `on_raw_key_event`
///
/// `App::on_raw_key_event` fires at winit-event time, before the active
/// pane's `super_pressed` state has been refreshed for the current frame
/// (that only happens inside [`super::widget::FreminalTerminalWidget::show`]).
/// Encoding immediately would risk reading a stale `super_pressed` for a
/// Super+blocked-key chord. Instead, the raw events are queued on
/// `PerWindowState::pending_raw_keys` and this function is called once per
/// frame, after the active pane's `show()` has returned — so `super_pressed`
/// and `snap` both reflect the current frame.
///
/// ## KKP gating
///
/// No explicit KKP-flag check is needed here: when no relevant
/// `kitty_keyboard_flags` bit is set, `TerminalInput::KittyFunctional`'s
/// `to_payload` returns an empty payload (these keys have no legacy
/// encoding), so [`send_terminal_inputs`] sends nothing.
///
/// ## Broadcast (Task 74)
///
/// Mirrors the encoded bytes to `key_broadcast_targets` when broadcast input
/// is active, matching how other genuine keyboard input is fanned out
/// elsewhere in this module.
pub fn drain_pending_raw_keys(
    pending: &mut Vec<(
        freminal_windowing::RawKeyEvent,
        freminal_windowing::RawKeyMods,
    )>,
    input_tx: &Sender<InputEvent>,
    snap: &TerminalSnapshot,
    super_pressed: bool,
    key_broadcast_targets: &[Sender<InputEvent>],
) {
    if pending.is_empty() {
        return;
    }
    let modes = InputModes::from_snapshot(snap);
    for (event, mods) in pending.drain(..) {
        let Some(codepoint) = kitty_keycode_to_codepoint(event.key_code) else {
            continue;
        };

        let key_mods = raw_mods_to_key_modifiers(mods, super_pressed);
        let meta = if !event.pressed {
            KeyEventMeta {
                event_type: KeyEventType::Release,
                associated_text: None,
            }
        } else if event.repeat {
            KeyEventMeta {
                event_type: KeyEventType::Repeat,
                associated_text: None,
            }
        } else {
            KeyEventMeta::PRESS
        };

        let input = TerminalInput::KittyFunctional {
            codepoint,
            mods: key_mods,
        };
        send_terminal_inputs(std::slice::from_ref(&input), input_tx, &modes, &meta);

        if !key_broadcast_targets.is_empty() {
            let bytes = encode_terminal_inputs(std::slice::from_ref(&input), &modes, &meta);
            broadcast_key_bytes(key_broadcast_targets, &bytes);
        }
    }
}

/// The physical held-key tracking state (currently just physical Super) that
/// should carry across a focus change (Task 114.8).
///
/// On focus-loss we clear held-key tracking: the terminal may miss the
/// release of a key held at the moment focus left (the user releases it in
/// another window), so retaining `true` would report a phantom-held key
/// afterward. Per the transition-only binding decision no synthetic release is
/// emitted — the next real key press rebuilds tracking honestly. On focus-gain
/// the current value is preserved (real modifier state arrives via the
/// compositor's `ModifiersChanged`).
const fn held_keys_after_focus_change(focused: bool, current_super_pressed: bool) -> bool {
    if focused {
        current_super_pressed
    } else {
        false
    }
}

/// Encode egui [`Modifiers`] as a packed `u8` for FREC v2 recording.
///
/// Bit layout: 0=shift, 1=ctrl, 2=alt, 3=super/command.
const fn modifiers_to_recording_u8(m: Modifiers) -> u8 {
    let mut bits = 0u8;
    if m.shift {
        bits |= 1;
    }
    if m.ctrl {
        bits |= 2;
    }
    if m.alt {
        bits |= 4;
    }
    if m.command {
        bits |= 8;
    }
    bits
}

/// Convert egui [`PointerButton`] to a recording button number
/// (0=left, 1=middle, 2=right, 3+=extra).
const fn pointer_button_to_u8(b: PointerButton) -> u8 {
    match b {
        PointerButton::Primary => 0,
        PointerButton::Secondary => 2,
        PointerButton::Middle => 1,
        PointerButton::Extra1 => 3,
        PointerButton::Extra2 => 4,
    }
}

/// Convert an egui [`Key`] to the keybinding layer's [`BindingKey`].
///
/// Returns `None` for keys that have no `BindingKey` equivalent (numpad
/// variants, media keys, etc.) — those fall through to the normal PTY
/// dispatch path unchanged.
pub(in crate::gui) const fn egui_key_to_binding_key(key: Key) -> Option<BindingKey> {
    match key {
        Key::A => Some(BindingKey::A),
        Key::B => Some(BindingKey::B),
        Key::C => Some(BindingKey::C),
        Key::D => Some(BindingKey::D),
        Key::E => Some(BindingKey::E),
        Key::F => Some(BindingKey::F),
        Key::G => Some(BindingKey::G),
        Key::H => Some(BindingKey::H),
        Key::I => Some(BindingKey::I),
        Key::J => Some(BindingKey::J),
        Key::K => Some(BindingKey::K),
        Key::L => Some(BindingKey::L),
        Key::M => Some(BindingKey::M),
        Key::N => Some(BindingKey::N),
        Key::O => Some(BindingKey::O),
        Key::P => Some(BindingKey::P),
        Key::Q => Some(BindingKey::Q),
        Key::R => Some(BindingKey::R),
        Key::S => Some(BindingKey::S),
        Key::T => Some(BindingKey::T),
        Key::U => Some(BindingKey::U),
        Key::V => Some(BindingKey::V),
        Key::W => Some(BindingKey::W),
        Key::X => Some(BindingKey::X),
        Key::Y => Some(BindingKey::Y),
        Key::Z => Some(BindingKey::Z),
        Key::Num0 => Some(BindingKey::Num0),
        Key::Num1 => Some(BindingKey::Num1),
        Key::Num2 => Some(BindingKey::Num2),
        Key::Num3 => Some(BindingKey::Num3),
        Key::Num4 => Some(BindingKey::Num4),
        Key::Num5 => Some(BindingKey::Num5),
        Key::Num6 => Some(BindingKey::Num6),
        Key::Num7 => Some(BindingKey::Num7),
        Key::Num8 => Some(BindingKey::Num8),
        Key::Num9 => Some(BindingKey::Num9),
        Key::F1 => Some(BindingKey::F1),
        Key::F2 => Some(BindingKey::F2),
        Key::F3 => Some(BindingKey::F3),
        Key::F4 => Some(BindingKey::F4),
        Key::F5 => Some(BindingKey::F5),
        Key::F6 => Some(BindingKey::F6),
        Key::F7 => Some(BindingKey::F7),
        Key::F8 => Some(BindingKey::F8),
        Key::F9 => Some(BindingKey::F9),
        Key::F10 => Some(BindingKey::F10),
        Key::F11 => Some(BindingKey::F11),
        Key::F12 => Some(BindingKey::F12),
        Key::ArrowUp => Some(BindingKey::ArrowUp),
        Key::ArrowDown => Some(BindingKey::ArrowDown),
        Key::ArrowLeft => Some(BindingKey::ArrowLeft),
        Key::ArrowRight => Some(BindingKey::ArrowRight),
        Key::Home => Some(BindingKey::Home),
        Key::End => Some(BindingKey::End),
        Key::PageUp => Some(BindingKey::PageUp),
        Key::PageDown => Some(BindingKey::PageDown),
        Key::Insert => Some(BindingKey::Insert),
        Key::Delete => Some(BindingKey::Delete),
        Key::Backspace => Some(BindingKey::Backspace),
        Key::Tab => Some(BindingKey::Tab),
        Key::Enter => Some(BindingKey::Enter),
        Key::Space => Some(BindingKey::Space),
        Key::Escape => Some(BindingKey::Escape),
        Key::Plus => Some(BindingKey::Plus),
        Key::Minus => Some(BindingKey::Minus),
        Key::Equals => Some(BindingKey::Equals),
        Key::Comma => Some(BindingKey::Comma),
        Key::Period => Some(BindingKey::Period),
        Key::Semicolon => Some(BindingKey::Semicolon),
        Key::Colon => Some(BindingKey::Colon),
        Key::Slash => Some(BindingKey::Slash),
        Key::Backslash => Some(BindingKey::Backslash),
        Key::OpenBracket => Some(BindingKey::OpenBracket),
        Key::CloseBracket => Some(BindingKey::CloseBracket),
        Key::Backtick => Some(BindingKey::Backtick),
        Key::Quote => Some(BindingKey::Quote),
        Key::Pipe => Some(BindingKey::Pipe),
        _ => None,
    }
}

/// Convert egui [`Modifiers`] to the keybinding layer's [`BindingModifiers`].
///
/// `m.command` (macOS Cmd) is mapped to `ctrl = true` — matching the
/// behaviour of [`egui_mods_to_key_modifiers`].
pub(in crate::gui) const fn egui_mods_to_binding_mods(m: Modifiers) -> BindingModifiers {
    BindingModifiers {
        ctrl: m.ctrl || m.command,
        shift: m.shift,
        alt: m.alt,
    }
}

/// Resolve which command block (if any) the `FoldPreviousCommand` action
/// should operate on.
///
/// Selection order:
///
/// 1. If the PTY cursor row falls inside a completed block's
///    `[command_start_row, end_row]` range, that block is chosen.  This is
///    the future-facing path for when scrollback-cursor navigation (Task
///    72.x) or pane gutter clicks (Task 73) let the user point at a
///    specific historical block.
/// 2. Otherwise, the most recently completed block is chosen — i.e. the
///    last block in `command_blocks` whose `end_row` is set.  In normal
///    interactive use the PTY cursor lives on the active prompt line,
///    which is always *after* every completed block, so case 1 never
///    matches and this fallback is the path users actually exercise.
///
/// Running blocks (`end_row.is_none()`) and blocks that never saw an OSC
/// 133 `C` marker (`command_start_row.is_none()`) are not foldable and
/// are excluded from both passes.
///
/// Returns `None` only if no completed block exists in the snapshot at
/// all, in which case the keybinding silently no-ops.
fn find_fold_target(snap: &TerminalSnapshot) -> Option<CommandBlockId> {
    let cursor_row = snap.cursor_pos.y;
    let is_completed = |b: &&CommandBlock| b.command_start_row.is_some() && b.end_row.is_some();

    // Pass 1: cursor inside a completed block's body.
    if let Some(block) = snap.command_blocks.iter().filter(is_completed).find(|b| {
        match (b.command_start_row, b.end_row) {
            (Some(start), Some(end)) => cursor_row >= start && cursor_row <= end,
            _ => false,
        }
    }) {
        return Some(block.id);
    }

    // Pass 2: most recent completed block (VecDeque pushes back, so newest
    // is last → iterate in reverse).
    snap.command_blocks
        .iter()
        .rev()
        .find(is_completed)
        .map(|b| b.id)
}

/// Return the output row range `[output_start_row, end_row]` for a
/// command block, if both bounds are present.
///
/// Blocks without an OSC 133 `C` marker (`output_start_row == None`) or
/// still-running blocks (`end_row == None`) cannot have their output
/// copied and return `None`.
const fn block_output_range(block: &CommandBlock) -> Option<(usize, usize)> {
    match (block.output_start_row, block.end_row) {
        (Some(start), Some(end)) if start <= end => Some((start, end)),
        _ => None,
    }
}

/// Find the most recently completed command block (newest first that has
/// a finished `end_row` *and* a captured `output_start_row`).
///
/// "Completed" here is stricter than [`find_fold_target`]'s notion: copy
/// actions require the C marker to have fired so we know where the
/// output region begins. Blocks missing `output_start_row` are skipped
/// even if `end_row` is set.
pub(super) fn find_last_copyable_block(snap: &TerminalSnapshot) -> Option<&CommandBlock> {
    snap.command_blocks
        .iter()
        .rev()
        .find(|b| block_output_range(b).is_some())
}

/// Find the command block whose `[command_start_row, end_row]` row range
/// contains `row`.
///
/// Used by the right-click "Copy Command Output" menu entry and by
/// `CopyCommandOutputAtCursor` to map a visible row back to a block.
/// Running blocks (no `end_row`) and blocks missing
/// `command_start_row` are skipped. If multiple blocks cover the same
/// row (which should not happen for well-formed OSC 133 streams), the
/// first match in insertion order wins.
pub(super) fn find_block_containing_row(
    snap: &TerminalSnapshot,
    row: usize,
) -> Option<&CommandBlock> {
    snap.command_blocks
        .iter()
        .find(|b| match (b.command_start_row, b.end_row) {
            (Some(start), Some(end)) => row >= start && row <= end,
            _ => false,
        })
}

/// Send an `ExtractSelection` event covering the full-width rows
/// `[start_row, end_row]` to the PTY consumer.
///
/// `end_col` is set to `term_width.saturating_sub(1)` so each row is
/// captured edge-to-edge; the buffer's `extract_text` impl already
/// clamps per-row to actual cell length, so trailing empty columns
/// don't produce spurious whitespace.
///
/// Returns `true` if the send succeeded. Failure is logged at error
/// level and the caller should leave `clipboard_pending` untouched.
fn send_extract_output_range(
    input_tx: &Sender<InputEvent>,
    snap: &TerminalSnapshot,
    start_row: usize,
    end_row: usize,
) -> bool {
    let end_col = snap.term_width.saturating_sub(1);
    match input_tx.send(InputEvent::ExtractSelection {
        start_row,
        start_col: 0,
        end_row,
        end_col,
        is_block: false,
    }) {
        Ok(()) => true,
        Err(e) => {
            error!("Failed to send ExtractSelection (command output): {e}");
            false
        }
    }
}

/// Dispatch a [`KeyAction`] that was resolved from the binding map.
///
/// Handles the subset of actions that can be executed inside
/// `write_input_to_terminal`: clipboard copy and all scrollback actions.
/// All other actions (zoom, open settings, tab management, etc.) cannot be
/// handled here — they require access to GUI state that is not available at
/// this call site.  Those actions are collected in `deferred_actions` so the
/// caller can propagate them to the GUI layer.
///
/// **All bound keys are consumed** — they are never forwarded to the PTY,
/// regardless of whether the action is handled here or deferred.
// Clippy `too_many_lines` fires at 106 lines after Task 70.N expanded the
// in-body `if let Err` blocks to `send_or_log!` macro calls (which rustfmt
// formats on multiple lines). The function is a flat `match` over
// `KeyAction` variants — splitting it would just move the match into helper
// functions that each forward one variant, which adds indirection without
// clarity. Suppressing for this specific function.
#[allow(clippy::too_many_lines)]
pub(super) fn dispatch_binding_action(
    action: KeyAction,
    view_state: &mut ViewState,
    input_tx: &Sender<InputEvent>,
    snap: &TerminalSnapshot,
    clipboard_pending: &mut bool,
    deferred_actions: &mut Vec<KeyAction>,
) {
    match action {
        // `KeyAction::Paste` is intentionally NOT handled here. It is deferred
        // to `dispatch_deferred_action` (via the `other` arm below) so the
        // smart paste guard (Task 77) can analyze the clipboard with access to
        // the live config and the per-window confirm dialog, which are not
        // reachable at this input-layer call site.
        KeyAction::Copy if let Some((start, end)) = view_state.selection.normalised() => {
            if let Err(e) = input_tx.send(InputEvent::ExtractSelection {
                start_row: start.row,
                start_col: start.col,
                end_row: end.row,
                end_col: end.col,
                is_block: view_state.selection.is_block,
            }) {
                error!("Failed to send ExtractSelection to PTY consumer: {e}");
            } else {
                *clipboard_pending = true;
            }
        }
        KeyAction::ScrollPageUp => {
            let new_offset = scrolled_offset(
                snap,
                &view_state.folded_blocks,
                view_state.scroll_offset,
                crate::gui::folding::ScrollDir::Up,
                snap.term_height,
            );
            if new_offset != view_state.scroll_offset {
                view_state.scroll_offset = new_offset;
                send_or_log!(
                    input_tx,
                    scroll_event(snap, &view_state.folded_blocks, new_offset),
                    "Failed to send scroll offset to PTY consumer"
                );
            }
        }
        KeyAction::ScrollPageDown => {
            let new_offset = scrolled_offset(
                snap,
                &view_state.folded_blocks,
                view_state.scroll_offset,
                crate::gui::folding::ScrollDir::Down,
                snap.term_height,
            );
            if new_offset != view_state.scroll_offset {
                view_state.scroll_offset = new_offset;
                send_or_log!(
                    input_tx,
                    scroll_event(snap, &view_state.folded_blocks, new_offset),
                    "Failed to send scroll offset to PTY consumer"
                );
            }
        }
        KeyAction::ScrollToTop => {
            let new_offset = snap.max_scroll_offset;
            if new_offset != view_state.scroll_offset {
                view_state.scroll_offset = new_offset;
                send_or_log!(
                    input_tx,
                    scroll_event(snap, &view_state.folded_blocks, new_offset),
                    "Failed to send scroll offset to PTY consumer"
                );
            }
        }
        KeyAction::ScrollToBottom if view_state.scroll_offset != 0 => {
            view_state.scroll_offset = 0;
            send_or_log!(
                input_tx,
                scroll_event(snap, &view_state.folded_blocks, 0),
                "Failed to send scroll offset to PTY consumer"
            );
        }
        KeyAction::ScrollLineUp => {
            let new_offset = scrolled_offset(
                snap,
                &view_state.folded_blocks,
                view_state.scroll_offset,
                crate::gui::folding::ScrollDir::Up,
                1,
            );
            if new_offset != view_state.scroll_offset {
                view_state.scroll_offset = new_offset;
                send_or_log!(
                    input_tx,
                    scroll_event(snap, &view_state.folded_blocks, new_offset),
                    "Failed to send scroll offset to PTY consumer"
                );
            }
        }
        KeyAction::ScrollLineDown => {
            let new_offset = scrolled_offset(
                snap,
                &view_state.folded_blocks,
                view_state.scroll_offset,
                crate::gui::folding::ScrollDir::Down,
                1,
            );
            if new_offset != view_state.scroll_offset {
                view_state.scroll_offset = new_offset;
                send_or_log!(
                    input_tx,
                    scroll_event(snap, &view_state.folded_blocks, new_offset),
                    "Failed to send scroll offset to PTY consumer"
                );
            }
        }
        KeyAction::ClearScrollback => {
            // Reset the local scroll offset so the next render pulls from the
            // live view — the PTY side will also drop its gui_scroll_offset
            // when it processes the ClearScrollback event. Doing it here
            // avoids one frame of stale rendering if the user was scrolled
            // back at the moment they pressed the key.
            view_state.scroll_offset = 0;
            send_or_log!(
                input_tx,
                InputEvent::ClearScrollback,
                "Failed to send ClearScrollback to PTY consumer"
            );
        }
        KeyAction::FoldPreviousCommand => {
            if let Some(id) = find_fold_target(snap) {
                view_state.toggle_fold(id);
                resend_scroll_window(snap, view_state, input_tx);
            }
        }
        KeyAction::FoldAll => {
            view_state.fold_all(snap.command_blocks.iter());
            resend_scroll_window(snap, view_state, input_tx);
        }
        KeyAction::UnfoldAll => {
            view_state.unfold_all();
            resend_scroll_window(snap, view_state, input_tx);
        }
        KeyAction::CopyLastCommandOutput => {
            if let Some(block) = find_last_copyable_block(snap)
                && let Some((start_row, end_row)) = block_output_range(block)
                && send_extract_output_range(input_tx, snap, start_row, end_row)
            {
                *clipboard_pending = true;
            }
        }
        KeyAction::CopyCommandOutputAtCursor => {
            let cursor_row = snap.cursor_pos.y;
            if let Some(block) = find_block_containing_row(snap, cursor_row)
                && let Some((start_row, end_row)) = block_output_range(block)
                && send_extract_output_range(input_tx, snap, start_row, end_row)
            {
                *clipboard_pending = true;
            }
        }
        // All other actions (zoom, settings, tabs, etc.) require GUI state
        // not available here.  Defer them to the GUI layer.
        other => deferred_actions.push(other),
    }
}

pub(super) fn control_key(key: Key) -> Option<Cow<'static, [TerminalInput]>> {
    if key >= Key::A && key <= Key::Z {
        let name = key.name();
        assert!(name.len() == 1);
        let name_c = name.as_bytes()[0];
        return Some(vec![TerminalInput::Ctrl(name_c)].into());
    }

    // https://catern.com/posts/terminal_quirks.html
    // https://en.wikipedia.org/wiki/C0_and_C1_control_codes
    //
    // Ctrl + special/punctuation keys follow the rule: code = ascii & 0x1F
    // For uppercase letters 0x40–0x5F, this maps cleanly to 0x00–0x1F.
    // For other characters we apply the same mask, but note that punctuation
    // codes only make sense for characters whose ASCII value has bit 5 or 6
    // set.  The well-known mappings used by terminals (and nano) are listed
    // below with their resulting control byte.
    match key {
        // These three follow the same 0x40-range rule as letters
        Key::OpenBracket => Some([TerminalInput::Ctrl(b'[')].as_ref().into()), // 0x1B ESC
        Key::CloseBracket => Some([TerminalInput::Ctrl(b']')].as_ref().into()), // 0x1D GS
        Key::Backslash => Some([TerminalInput::Ctrl(b'\\')].as_ref().into()),  // 0x1C FS

        // Ctrl+Space => 0x00 NUL  (0x20 & 0x1F = 0x00)
        Key::Space => Some([TerminalInput::Ctrl(b' ')].as_ref().into()),

        // Ctrl+- => 0x1F US  (nano "Undo")
        // Ctrl+/ => 0x1F US  (nano "Go to Line" / same byte as Ctrl+_)
        // Ctrl+7 => 0x1F US  (same as Ctrl+_ / Ctrl+- / Ctrl+/)
        Key::Minus | Key::Slash | Key::Num7 => Some([TerminalInput::Ascii(0x1F)].as_ref().into()),

        // Digit row: Ctrl+2..8 produce the C0 bytes that letters cannot reach
        // Ctrl+2 => 0x00 NUL  (same as Ctrl+Space / Ctrl+@)
        Key::Num2 => Some([TerminalInput::Ascii(0x00)].as_ref().into()),
        // Ctrl+3 => 0x1B ESC  (same as Ctrl+[)
        Key::Num3 => Some([TerminalInput::Ascii(0x1B)].as_ref().into()),
        // Ctrl+4 => 0x1C FS   (same as Ctrl+\)
        Key::Num4 => Some([TerminalInput::Ascii(0x1C)].as_ref().into()),
        // Ctrl+5 => 0x1D GS   (same as Ctrl+])
        Key::Num5 => Some([TerminalInput::Ascii(0x1D)].as_ref().into()),
        // Ctrl+6 => 0x1E RS   (same as Ctrl+^)
        Key::Num6 => Some([TerminalInput::Ascii(0x1E)].as_ref().into()),

        // Ctrl+8 => 0x7F DEL
        Key::Num8 => Some([TerminalInput::Ascii(0x7F)].as_ref().into()),

        _ => None,
    }
}

/// Convert an egui [`Key`] + [`Modifiers`] to a [`TerminalInput`] variant.
///
/// Used for KKP flag 2 release-event forwarding: when a key is released we
/// need to reconstruct the same `TerminalInput` that the press would have
/// generated so the release can be encoded with the matching codepoint.
///
/// `super_held` is threaded through to [`egui_mods_to_key_modifiers`] — see
/// its doc comment for why it cannot be derived from `mods` alone on
/// Linux/Windows.
fn egui_key_to_terminal_input(
    key: Key,
    mods: Modifiers,
    super_held: bool,
) -> Option<TerminalInput> {
    let km = egui_mods_to_key_modifiers(mods, super_held);
    match key {
        Key::Enter => Some(TerminalInput::Enter),
        Key::Backspace => Some(TerminalInput::Backspace),
        Key::Tab => Some(TerminalInput::Tab),
        Key::Escape => Some(TerminalInput::Escape),
        Key::ArrowUp => Some(TerminalInput::ArrowUp(km)),
        Key::ArrowDown => Some(TerminalInput::ArrowDown(km)),
        Key::ArrowLeft => Some(TerminalInput::ArrowLeft(km)),
        Key::ArrowRight => Some(TerminalInput::ArrowRight(km)),
        Key::Home => Some(TerminalInput::Home(km)),
        Key::End => Some(TerminalInput::End(km)),
        Key::Delete => Some(TerminalInput::Delete(km)),
        Key::Insert => Some(TerminalInput::Insert(km)),
        Key::PageUp => Some(TerminalInput::PageUp(km)),
        Key::PageDown => Some(TerminalInput::PageDown(km)),
        Key::F1 => Some(TerminalInput::FunctionKey(1, km)),
        Key::F2 => Some(TerminalInput::FunctionKey(2, km)),
        Key::F3 => Some(TerminalInput::FunctionKey(3, km)),
        Key::F4 => Some(TerminalInput::FunctionKey(4, km)),
        Key::F5 => Some(TerminalInput::FunctionKey(5, km)),
        Key::F6 => Some(TerminalInput::FunctionKey(6, km)),
        Key::F7 => Some(TerminalInput::FunctionKey(7, km)),
        Key::F8 => Some(TerminalInput::FunctionKey(8, km)),
        Key::F9 => Some(TerminalInput::FunctionKey(9, km)),
        Key::F10 => Some(TerminalInput::FunctionKey(10, km)),
        Key::F11 => Some(TerminalInput::FunctionKey(11, km)),
        Key::F12 => Some(TerminalInput::FunctionKey(12, km)),
        // Modifier keys as keys (KKP flag 8) — used so a release of a bare
        // modifier-key press can be reconstructed and forwarded (see the
        // KKP flag 2 release-forwarding call site above).
        Key::ShiftLeft => Some(TerminalInput::ShiftLeft(km)),
        Key::ShiftRight => Some(TerminalInput::ShiftRight(km)),
        Key::ControlLeft => Some(TerminalInput::ControlLeft(km)),
        Key::ControlRight => Some(TerminalInput::ControlRight(km)),
        Key::AltLeft => Some(TerminalInput::AltLeft(km)),
        Key::AltRight => Some(TerminalInput::AltRight(km)),
        Key::SuperLeft => Some(TerminalInput::SuperLeft(km)),
        Key::SuperRight => Some(TerminalInput::SuperRight(km)),
        Key::Space => Some(TerminalInput::Ascii(b' ')),
        k if k >= Key::A && k <= Key::Z => {
            let name = k.name();
            let byte = name.as_bytes()[0];
            if mods.ctrl || mods.command {
                Some(TerminalInput::Ctrl(byte))
            } else if mods.shift {
                Some(TerminalInput::Ascii(byte))
            } else {
                Some(TerminalInput::Ascii(byte.to_ascii_lowercase()))
            }
        }
        _ => {
            let name = key.name();
            let bytes = name.as_bytes();
            if bytes.len() == 1 && bytes[0].is_ascii() {
                Some(TerminalInput::Ascii(bytes[0]))
            } else {
                None
            }
        }
    }
}

/// Bundle of terminal mode flags that affect how keyboard input is encoded.
///
/// Extracted from `TerminalSnapshot` once per frame and passed to
/// [`send_terminal_inputs`] to keep the argument count manageable.
pub(super) struct InputModes {
    cursor_key_app_mode: Decckm,
    keypad_app_mode: KeypadMode,
    modify_other_keys: u8,
    application_escape_key: ApplicationEscapeKey,
    backarrow_sends_bs: Decbkm,
    line_feed_mode: Lnm,
    kitty_keyboard_flags: u32,
}

impl InputModes {
    /// Extract all input-encoding mode fields from a snapshot.
    pub(super) const fn from_snapshot(snap: &TerminalSnapshot) -> Self {
        Self {
            cursor_key_app_mode: snap.cursor_key_app_mode,
            keypad_app_mode: snap.keypad_app_mode,
            modify_other_keys: snap.modify_other_keys,
            application_escape_key: snap.application_escape_key,
            backarrow_sends_bs: snap.backarrow_sends_bs,
            line_feed_mode: snap.line_feed_mode,
            kitty_keyboard_flags: snap.kitty_keyboard_flags,
        }
    }
}

/// Collect all bytes from a slice of `TerminalInput` values into a single
/// `Vec<u8>` and send them as one atomic `InputEvent::Key` to the PTY
/// consumer thread.
///
/// Sending all bytes in a single message is critical for multi-byte
/// sequences such as mouse reports (`\x1b[<0;5;3M`) — if each byte were
/// sent as a separate `InputEvent` the PTY application would receive them
/// as individual characters rather than as literal typed text, causing
/// them to be interpreted as individual typed characters.
///
/// Encode terminal inputs into raw bytes using the given mode flags.
///
/// This is the single source of truth for converting [`TerminalInput`] slices
/// into the byte sequence that gets sent to the PTY.
fn encode_terminal_inputs(
    inputs: &[TerminalInput],
    modes: &InputModes,
    meta: &KeyEventMeta,
) -> Vec<u8> {
    inputs
        .iter()
        .flat_map(|input| {
            match input.to_payload(
                modes.cursor_key_app_mode,
                modes.keypad_app_mode,
                modes.modify_other_keys,
                modes.application_escape_key,
                modes.backarrow_sends_bs,
                modes.line_feed_mode,
                modes.kitty_keyboard_flags,
                meta,
            ) {
                TerminalInputPayload::Single(b) => vec![b],
                TerminalInputPayload::Many(bs) => bs.to_vec(),
                TerminalInputPayload::Owned(bs) => bs,
            }
        })
        .collect()
}

/// Encode and send terminal inputs to the PTY consumer thread.
///
/// ## Mode parameters
///
/// The `modes` parameter bundles all terminal mode flags that affect input
/// encoding. See [`InputModes`] for the individual fields.
pub(super) fn send_terminal_inputs(
    inputs: &[TerminalInput],
    input_tx: &Sender<InputEvent>,
    modes: &InputModes,
    meta: &KeyEventMeta,
) {
    let bytes = encode_terminal_inputs(inputs, modes, meta);
    if bytes.is_empty() {
        return;
    }
    send_or_log!(
        input_tx,
        InputEvent::Key(bytes),
        "Failed to send key input to PTY consumer"
    );
}

/// Mirror an already-encoded keyboard byte sequence to every broadcast target.
///
/// Used by Task 74 broadcast-input mode: the active pane's encoded
/// `InputEvent::Key` payload is replayed to every other leaf pane in the tab.
/// Each pane independently applies its own bracketed-paste / cursor-key mode,
/// so we replay the raw encoded bytes rather than re-encoding per pane.
///
/// A failed send to one target is logged and skipped — it never aborts the
/// fan-out to the remaining targets.
fn broadcast_key_bytes(targets: &[Sender<InputEvent>], bytes: &[u8]) {
    if bytes.is_empty() {
        return;
    }
    for target in targets {
        if let Err(e) = target.send(InputEvent::Key(bytes.to_vec())) {
            error!("Failed to broadcast key input to a pane: {e}");
        }
    }
}

/// Handle mouse scroll when mouse tracking is off.
///
/// On the **alternate screen** (less, vim, htop, ...) scroll events are
/// converted to `ArrowUp`/`ArrowDown` key presses sent to the PTY — this
/// matches the behaviour of every major terminal emulator (kitty, Alacritty,
/// `WezTerm`, GNOME Terminal, etc.).
///
/// The conversion happens unconditionally on the alternate screen, regardless
/// of the `?1007` (Alternate Scroll) DEC private mode.  Most terminal
/// emulators translate scroll to arrow keys on the alternate screen by
/// default, and most applications (less, vim, htop) do not explicitly set
/// `?1007`.  Gating on `AlternateScroll::Enabled` would break scroll in
/// nearly all alternate-screen applications.
///
/// On the **primary screen** scroll events adjust the scroll offset and send
/// it to the PTY thread via `InputEvent::ScrollOffset`.  The PTY thread
/// clamps the value to `max_scroll_offset()` when building the next snapshot.
pub(super) fn handle_scroll_fallback(
    scroll_amount_to_do: f32,
    character_size_y: f32,
    snap: &TerminalSnapshot,
    input_tx: &Sender<InputEvent>,
    view_state: &mut ViewState,
) {
    let lines = (scroll_amount_to_do / character_size_y).round();
    let abs_lines = lines.abs();

    if snap.is_alternate_screen {
        // Convert scroll delta to arrow key presses unconditionally.
        // This matches kitty, Alacritty, WezTerm, and GNOME Terminal
        // behaviour: scroll on the alternate screen always sends arrow
        // keys to the PTY, regardless of the ?1007 mode flag.
        let count = abs_lines.approx_as::<usize>().unwrap_or(0).max(1);
        let key = if lines > 0.0 {
            TerminalInput::ArrowUp(KeyModifiers::NONE)
        } else {
            TerminalInput::ArrowDown(KeyModifiers::NONE)
        };
        for _ in 0..count {
            send_terminal_inputs(
                std::slice::from_ref(&key),
                input_tx,
                &InputModes::from_snapshot(snap),
                &KeyEventMeta::PRESS,
            );
        }
    } else {
        // Primary screen: adjust scroll offset and send to PTY thread.
        // Multiply by 3 so each wheel tick scrolls 3 lines — matching the
        // default behavior of most terminal emulators (iTerm2, Alacritty,
        // kitty, GNOME Terminal, etc.).
        const SCROLL_MULTIPLIER: usize = 3;
        let n = abs_lines.approx_as::<usize>().unwrap_or(0).max(1) * SCROLL_MULTIPLIER;

        // Scroll by rendered (visible) rows so a tick over a collapsed fold
        // moves one visible line, not one hidden buffer row.
        let dir = if lines > 0.0 {
            crate::gui::folding::ScrollDir::Up
        } else {
            crate::gui::folding::ScrollDir::Down
        };
        let new_offset = scrolled_offset(
            snap,
            &view_state.folded_blocks,
            view_state.scroll_offset,
            dir,
            n,
        );

        if new_offset != view_state.scroll_offset {
            view_state.scroll_offset = new_offset;
            send_or_log!(
                input_tx,
                scroll_event(snap, &view_state.folded_blocks, new_offset),
                "Failed to send scroll offset to PTY consumer"
            );
        }
    }
}

/// Compute the end column for a mouse-release event, respecting the current
/// multi-click mode.
///
/// For triple-click (`click_count >= 3`) the end snaps to line boundaries.
/// For double-click (`click_count == 2`) it snaps to word boundaries.
/// For single-click it returns the raw column `x`.
fn release_end_col(
    view_state: &ViewState,
    snap: &TerminalSnapshot,
    x: usize,
    abs_row: usize,
) -> usize {
    // Snapshot-row index for `visible_chars` lookups, derived from the
    // buffer-absolute `abs_row` so it accounts for the extra-row fold window.
    // `y` (the raw screen row) is NOT a valid `visible_chars` index when folds
    // shift the window.
    let snap_y =
        abs_row.saturating_sub(visible_window_start(snap).saturating_sub(snap.window_extra_rows));
    if view_state.click_count >= 3 {
        let anchor_row = view_state.selection.anchor.map_or(abs_row, |a| a.row);
        let (line_start, line_end) =
            crate::gui::view_state::line_boundaries(&snap.visible_chars, snap_y);
        if abs_row >= anchor_row {
            line_end
        } else {
            line_start
        }
    } else if view_state.click_count == 2 {
        let anchor_row = view_state.selection.anchor.map_or(abs_row, |a| a.row);
        let anchor_col = view_state.selection.anchor.map_or(x, |a| a.col);
        let (word_start, word_end) =
            crate::gui::view_state::word_boundaries(&snap.visible_chars, snap_y, x);
        if abs_row > anchor_row || (abs_row == anchor_row && word_end >= anchor_col) {
            word_end
        } else {
            word_start
        }
    } else {
        x
    }
}

/// Return type of [`write_input_to_terminal`] — see its "Return value" doc
/// section for the meaning of each element. Factored into a named alias
/// (rather than an inline tuple) to satisfy `clippy::type_complexity`.
type WriteInputResult = (
    bool,
    Option<PreviousMouseState>,
    Option<Key>,
    f32,
    bool,
    Vec<KeyAction>,
    bool,
);

#[allow(
    clippy::cognitive_complexity,
    clippy::too_many_lines,
    clippy::too_many_arguments
)]
/// Translate egui input events into terminal input and send them to the PTY.
///
/// ## Input routing
///
/// Each egui `Event` is classified and converted into one or more
/// [`TerminalInput`] values which are then serialised to bytes and sent as a
/// single `InputEvent::Key(Vec<u8>)` via `input_tx`.  Sending all bytes in a
/// single message is critical for multi-byte sequences (mouse reports,
/// modifier-encoded arrows) — splitting across multiple sends would let the
/// PTY application see them as individual typed characters.
///
/// | egui event           | Routing                                               |
/// |----------------------|-------------------------------------------------------|
/// | `Text(s)`            | UTF-8 bytes, possibly wrapped in bracketed-paste markers. |
/// | `Key` (printable)    | `Ctrl+letter` → C0 control byte; `Ctrl+punctuation` → low-ASCII byte via `control_key()`. |
/// | `Key` (special)      | Arrows, Home/End, Delete, Insert, PgUp/Dn, F1–F12 → xterm escape sequences via `to_payload()`. |
/// | `PointerButton`      | Translated to X10/X11/SGR mouse report bytes when mouse tracking is active. |
/// | `PointerMoved`       | Mouse-move report when button-motion or any-event tracking is active; updates text selection when tracking is off. |
/// | `Scroll`             | Alternate screen: unconditionally converted to arrow-key bytes (matching kitty/Alacritty/WezTerm). Primary screen: updates `ViewState::scroll_offset` and sends `InputEvent::ScrollOffset`. |
/// | `WindowFocused`      | Sends `InputEvent::FocusChange`; clears mouse position on unfocus. |
/// | `Paste`              | Bracketed-paste wrapped if `RlBracket::Bracketed` is set. |
/// | `Copy`               | Selection text placed on system clipboard. |
///
/// ## Mouse tracking suppression
///
/// When `view_state.scroll_offset > 0` (user scrolled into history), mouse
/// tracking is suppressed — `effective_mouse_tracking` is overridden to
/// `NoTracking`.  This matches the behavior of xterm/kitty/WezTerm: the
/// visible content is historical, not the live terminal the PTY application
/// expects mouse coordinates to reference.
///
/// ## Return value
///
/// Returns `(left_mouse_pressed, last_reported_mouse_pos, previous_key, scroll_amount, clipboard_pending, deferred_actions, super_pressed)`:
/// - `left_mouse_pressed` — true if a primary left-click was pressed inside this pane's rect this frame (used for click-to-focus by the caller).
/// - `last_reported_mouse_pos` — updated mouse tracking state for the next call.
/// - `previous_key` — last pressed key (used for key-repeat deduplication).
/// - `scroll_amount` — accumulated fractional scroll pixels not yet converted to full line units.
/// - `clipboard_pending` — true if a selection-copy was queued; the caller reads the clipboard channel.
/// - `super_pressed` — updated physical Super/Command hold-state (see `super_pressed` parameter) for the next call.
///
/// ## Active-pane gating
///
/// When `is_active_pane` is `false`, only primary left-click presses are detected (to support
/// click-to-focus in the caller). All keyboard, text, paste, copy, scroll, and mouse-tracking
/// events are suppressed — they are never forwarded to the inactive pane's PTY.
///
/// ## Broadcast input (Task 74)
///
/// `key_broadcast_targets` lists the input senders of the *other* panes in
/// the tab when broadcast mode is active on the active pane. Genuine keyboard
/// input (text, control keys, KKP press/release, synthesized control
/// sequences, and paste payloads) is mirrored to every target. Mouse,
/// scroll-derived, resize, focus, and selection events are never mirrored.
/// The slice is empty when broadcast is off or this is not the active pane.
pub(super) fn write_input_to_terminal(
    input: &InputState,
    snap: &TerminalSnapshot,
    input_tx: &Sender<InputEvent>,
    view_state: &mut ViewState,
    character_size_x: f32,
    character_size_y: f32,
    terminal_rect: Rect,
    last_reported_mouse_pos: Option<PreviousMouseState>,
    repeat_characters: Decarm,
    previous_key: Option<Key>,
    scroll_amount: f32,
    binding_map: &BindingMap,
    is_active_pane: bool,
    recording_ctx: Option<&RecordingContext<'_>>,
    placeholder_rects: &[(Rect, CommandBlockId)],
    key_broadcast_targets: &[Sender<InputEvent>],
    super_pressed: bool,
) -> WriteInputResult {
    if input.raw.events.is_empty() {
        return (
            false,
            last_reported_mouse_pos,
            previous_key,
            scroll_amount,
            false,
            Vec::new(),
            super_pressed,
        );
    }

    let mut previous_key = previous_key;
    let mut state_changed = false;
    let mut last_reported_mouse_pos = last_reported_mouse_pos;
    let mut left_mouse_button_pressed = false;
    let mut scroll_amount = scroll_amount;
    let mut clipboard_pending = false;
    let mut deferred_actions: Vec<KeyAction> = Vec::new();
    // Physical Super/Command key hold-state. On macOS this is redundant with
    // `Modifiers::mac_cmd` (handled directly in `egui_mods_to_key_modifiers`);
    // on Linux/Windows, egui exposes no `Modifiers` bit for the physical
    // Super/Windows key, so it must be tracked across frames via the
    // `Key::SuperLeft`/`Key::SuperRight` press/release events observed below.
    let mut super_pressed = super_pressed;

    // Derive the terminal origin from the rect.  Pointer events whose
    // position falls outside `terminal_rect` are ignored — they belong to
    // other UI panels (e.g. the tab bar).
    let terminal_origin = terminal_rect.min;

    // When the user is scrolled back into history, suppress mouse forwarding
    // to the PTY — the visible content is historical, not the live terminal
    // output the PTY application expects mouse coordinates to refer to.
    // Standard terminal emulator behavior (xterm, kitty, WezTerm, etc.).
    let effective_mouse_tracking = if view_state.scroll_offset > 0 {
        &MouseTrack::NoTracking
    } else {
        &snap.mouse_tracking
    };

    let mouse_encoding = &snap.mouse_encoding;

    for event in &input.raw.events {
        info!("event: {:?}", event);

        // Non-active panes: only detect primary left-click press so the caller
        // can implement click-to-focus.  All other events (keyboard, scroll,
        // paste, mouse tracking) are suppressed — they belong to the active pane.
        if !is_active_pane {
            if let Event::PointerButton {
                button: PointerButton::Primary,
                pressed: true,
                pos,
                ..
            } = event
                && terminal_rect.contains(*pos)
            {
                left_mouse_button_pressed = true;
            }
            continue;
        }

        // ── Physical Super/Command hold-state (Task 101.2) ───────────────
        // egui delivers the physical Super/Windows key only as discrete
        // `SuperLeft`/`SuperRight` press/release events — there is no
        // `Modifiers` bit for it on Linux/Windows. Track the hold-state
        // across frames so `egui_mods_to_key_modifiers` can set `super_key`
        // for keys pressed while Super is held. This is purely observational
        // — it does not consume the event, so any future encoding of the
        // Super key itself (Task 101.3) still sees it.
        if let Event::Key {
            key: Key::SuperLeft | Key::SuperRight,
            pressed,
            ..
        } = event
        {
            super_pressed = *pressed;
        }

        if let Event::Key { pressed: false, .. } = event {
            previous_key = None;
        }

        // ── KKP flag 2: forward key release events ───────────────────────
        // When KKP report-event-types (flag 2) is active together with
        // DISAMBIGUATE (1) or REPORT_ALL (8), key releases must be
        // forwarded to the PTY so applications can track key-up.
        if let Event::Key {
            key,
            pressed: false,
            modifiers,
            ..
        } = event
        {
            let kkp = snap.kitty_keyboard_flags;
            if kkp & 2 != 0
                && kkp & (1 | 8) != 0
                && let Some(ti) = egui_key_to_terminal_input(*key, *modifiers, super_pressed)
            {
                let release_meta = KeyEventMeta {
                    event_type: KeyEventType::Release,
                    associated_text: None,
                };
                let modes = InputModes::from_snapshot(snap);
                send_terminal_inputs(std::slice::from_ref(&ti), input_tx, &modes, &release_meta);
                // Broadcast (Task 74): mirror the KKP release sequence to the
                // other panes. Each pane shares the same KKP mode here, so
                // re-encoding with the active pane's modes is correct.
                if !key_broadcast_targets.is_empty() {
                    let bytes =
                        encode_terminal_inputs(std::slice::from_ref(&ti), &modes, &release_meta);
                    broadcast_key_bytes(key_broadcast_targets, &bytes);
                }
                state_changed = true;
                continue;
            }
        }

        // ── Binding map pre-check ─────────────────────────────────────────
        // Before routing the event to the PTY, check whether the key combo
        // is bound to a terminal action in the user's BindingMap.  Bound
        // combos are consumed here and never forwarded to the PTY.
        //
        // Event::Key carries the key and modifiers directly, so we can
        // build a KeyCombo and look it up.  Event::Copy is a synthetic
        // egui-winit event (Ctrl+C / Ctrl+Shift+C never arrive as
        // Event::Key), so it is checked separately below.
        if let Event::Key {
            key,
            modifiers,
            pressed: true,
            ..
        } = event
            && let Some(binding_key) = egui_key_to_binding_key(*key)
        {
            let combo = KeyCombo::new(binding_key, egui_mods_to_binding_mods(*modifiers));
            if let Some(action) = binding_map.lookup(&combo) {
                dispatch_binding_action(
                    action,
                    view_state,
                    input_tx,
                    snap,
                    &mut clipboard_pending,
                    &mut deferred_actions,
                );
                state_changed = true;
                continue;
            }
        }
        // Event::Copy is the synthetic event fired by egui-winit for
        // Ctrl+C (and Ctrl+Shift+C).  Reconstruct the key combo so the
        // binding map can intercept Ctrl+Shift+C → Copy before it falls
        // through to the Ctrl+C → \x03 arm below.
        if matches!(event, Event::Copy) {
            let combo = KeyCombo::new(
                BindingKey::C,
                BindingModifiers {
                    ctrl: true,
                    shift: input.modifiers.shift,
                    alt: false,
                },
            );
            if let Some(action) = binding_map.lookup(&combo) {
                dispatch_binding_action(
                    action,
                    view_state,
                    input_tx,
                    snap,
                    &mut clipboard_pending,
                    &mut deferred_actions,
                );
                state_changed = true;
                continue;
            }
        }

        // ── Construct KKP event metadata ─────────────────────────────────
        // Extract event type (press/repeat/release) from egui Key events.
        // Non-key events default to Press.  For Event::Text the associated
        // text is the text string itself (used by KKP flag 16).
        let event_meta = match event {
            Event::Key {
                pressed: true,
                repeat: true,
                ..
            } => KeyEventMeta {
                event_type: KeyEventType::Repeat,
                associated_text: None,
            },
            Event::Key { pressed: false, .. } => KeyEventMeta {
                event_type: KeyEventType::Release,
                associated_text: None,
            },
            Event::Text(text) => KeyEventMeta {
                event_type: KeyEventType::Press,
                associated_text: if text.is_empty() {
                    None
                } else {
                    Some(text.clone())
                },
            },
            _ => KeyEventMeta::PRESS,
        };

        let inputs: Cow<'static, [TerminalInput]> = match event {
            // LIMITATION (egui#3653): egui unifies numpad and main-row keys.
            // Application keypad mode cannot distinguish them until egui exposes
            // separate key variants.
            Event::Text(text)
                if repeat_characters == Decarm::RepeatKey || previous_key.is_none() =>
            {
                collect_text(text)
            }
            Event::Text(_) => continue,
            Event::Key {
                key: Key::Enter,
                pressed: true,
                modifiers,
                ..
            } if modifiers.is_none() => [TerminalInput::Enter].as_ref().into(),
            #[allow(clippy::match_same_arms)] // Semantically distinct from Event::Text(_) above
            Event::Key {
                key: Key::Enter,
                pressed: true,
                ..
            } => continue,
            // https://github.com/emilk/egui/issues/3653
            // egui-winit intercepts Ctrl+C and Ctrl+X at the platform layer and converts them
            // to Event::Copy and Event::Cut respectively, before they can reach us as
            // Event::Key { key: Key::C/X, ctrl: true }.  We must handle both synthetic events
            // here so that terminal apps (e.g. nano ^C interrupt, ^X exit) receive the correct
            // C0 control bytes.
            //
            // Ctrl+Shift+C → Copy is now intercepted by the binding-map pre-check above
            // (which calls dispatch_binding_action and continues).  Any Event::Copy that
            // reaches this arm is therefore an unbound Ctrl+C: send \x03 (SIGINT).
            // Same logic for Cut: Ctrl+X → \x18, Ctrl+Shift+X → no-op (can't cut from terminal).
            Event::Copy => [TerminalInput::Ctrl(b'c')].as_ref().into(),
            Event::Cut => {
                if input.modifiers.shift {
                    continue;
                }
                [TerminalInput::Ctrl(b'x')].as_ref().into()
            }
            Event::Key {
                key: Key::J,
                pressed: true,
                modifiers: Modifiers { ctrl: true, .. },
                ..
            } => [TerminalInput::LineFeed].as_ref().into(),
            Event::Key {
                key: Key::Backspace,
                pressed: true,
                ..
            } => [TerminalInput::Backspace].as_ref().into(),
            Event::Key {
                key: Key::ArrowUp,
                pressed: true,
                modifiers,
                ..
            } => vec![TerminalInput::ArrowUp(egui_mods_to_key_modifiers(
                *modifiers,
                super_pressed,
            ))]
            .into(),
            Event::Key {
                key: Key::ArrowDown,
                pressed: true,
                modifiers,
                ..
            } => vec![TerminalInput::ArrowDown(egui_mods_to_key_modifiers(
                *modifiers,
                super_pressed,
            ))]
            .into(),
            Event::Key {
                key: Key::ArrowLeft,
                pressed: true,
                modifiers,
                ..
            } => vec![TerminalInput::ArrowLeft(egui_mods_to_key_modifiers(
                *modifiers,
                super_pressed,
            ))]
            .into(),
            Event::Key {
                key: Key::ArrowRight,
                pressed: true,
                modifiers,
                ..
            } => vec![TerminalInput::ArrowRight(egui_mods_to_key_modifiers(
                *modifiers,
                super_pressed,
            ))]
            .into(),
            Event::Key {
                key: Key::Home,
                pressed: true,
                modifiers,
                ..
            } => vec![TerminalInput::Home(egui_mods_to_key_modifiers(
                *modifiers,
                super_pressed,
            ))]
            .into(),
            Event::Key {
                key: Key::End,
                pressed: true,
                modifiers,
                ..
            } => vec![TerminalInput::End(egui_mods_to_key_modifiers(
                *modifiers,
                super_pressed,
            ))]
            .into(),
            Event::Key {
                key: Key::Delete,
                pressed: true,
                modifiers,
                ..
            } => vec![TerminalInput::Delete(egui_mods_to_key_modifiers(
                *modifiers,
                super_pressed,
            ))]
            .into(),
            Event::Key {
                key: Key::Insert,
                pressed: true,
                modifiers,
                ..
            } => vec![TerminalInput::Insert(egui_mods_to_key_modifiers(
                *modifiers,
                super_pressed,
            ))]
            .into(),
            Event::Key {
                key: Key::PageUp,
                pressed: true,
                modifiers,
                ..
            } => vec![TerminalInput::PageUp(egui_mods_to_key_modifiers(
                *modifiers,
                super_pressed,
            ))]
            .into(),
            Event::Key {
                key: Key::PageDown,
                pressed: true,
                modifiers,
                ..
            } => vec![TerminalInput::PageDown(egui_mods_to_key_modifiers(
                *modifiers,
                super_pressed,
            ))]
            .into(),
            Event::Key {
                key: Key::Tab,
                pressed: true,
                ..
            } => [TerminalInput::Tab].as_ref().into(),

            Event::Key {
                key: Key::F1,
                pressed: true,
                modifiers,
                ..
            } => vec![TerminalInput::FunctionKey(
                1,
                egui_mods_to_key_modifiers(*modifiers, super_pressed),
            )]
            .into(),
            Event::Key {
                key: Key::F2,
                pressed: true,
                modifiers,
                ..
            } => vec![TerminalInput::FunctionKey(
                2,
                egui_mods_to_key_modifiers(*modifiers, super_pressed),
            )]
            .into(),
            Event::Key {
                key: Key::F3,
                pressed: true,
                modifiers,
                ..
            } => vec![TerminalInput::FunctionKey(
                3,
                egui_mods_to_key_modifiers(*modifiers, super_pressed),
            )]
            .into(),
            Event::Key {
                key: Key::F4,
                pressed: true,
                modifiers,
                ..
            } => vec![TerminalInput::FunctionKey(
                4,
                egui_mods_to_key_modifiers(*modifiers, super_pressed),
            )]
            .into(),
            Event::Key {
                key: Key::F5,
                pressed: true,
                modifiers,
                ..
            } => vec![TerminalInput::FunctionKey(
                5,
                egui_mods_to_key_modifiers(*modifiers, super_pressed),
            )]
            .into(),
            Event::Key {
                key: Key::F6,
                pressed: true,
                modifiers,
                ..
            } => vec![TerminalInput::FunctionKey(
                6,
                egui_mods_to_key_modifiers(*modifiers, super_pressed),
            )]
            .into(),
            Event::Key {
                key: Key::F7,
                pressed: true,
                modifiers,
                ..
            } => vec![TerminalInput::FunctionKey(
                7,
                egui_mods_to_key_modifiers(*modifiers, super_pressed),
            )]
            .into(),
            Event::Key {
                key: Key::F8,
                pressed: true,
                modifiers,
                ..
            } => vec![TerminalInput::FunctionKey(
                8,
                egui_mods_to_key_modifiers(*modifiers, super_pressed),
            )]
            .into(),
            Event::Key {
                key: Key::F9,
                pressed: true,
                modifiers,
                ..
            } => vec![TerminalInput::FunctionKey(
                9,
                egui_mods_to_key_modifiers(*modifiers, super_pressed),
            )]
            .into(),
            Event::Key {
                key: Key::F10,
                pressed: true,
                modifiers,
                ..
            } => vec![TerminalInput::FunctionKey(
                10,
                egui_mods_to_key_modifiers(*modifiers, super_pressed),
            )]
            .into(),
            Event::Key {
                key: Key::F11,
                pressed: true,
                modifiers,
                ..
            } => vec![TerminalInput::FunctionKey(
                11,
                egui_mods_to_key_modifiers(*modifiers, super_pressed),
            )]
            .into(),
            Event::Key {
                key: Key::F12,
                pressed: true,
                modifiers,
                ..
            } => vec![TerminalInput::FunctionKey(
                12,
                egui_mods_to_key_modifiers(*modifiers, super_pressed),
            )]
            .into(),

            // ── KKP "modifier keys as keys" (flag 8 only) ─────────────────
            //
            // A bare press of a modifier key (no other key held) is reported
            // as its own CSI u event when REPORT_ALL is active; encoding
            // (via `TerminalInput::to_payload`) suppresses these entirely
            // outside flag 8. Must be BEFORE the wildcard Ctrl+<letter> arm
            // below, since egui reports these presses with the
            // corresponding `Modifiers::ctrl`/`alt`/`shift` bit already set,
            // which would otherwise be swallowed by that wildcard.
            //
            // `super_pressed` is updated by the observational block above
            // before this match runs, so a Super press sees its own
            // `super_key` bit set in the resulting modifier report.
            Event::Key {
                key: Key::ShiftLeft,
                pressed: true,
                modifiers,
                ..
            } => vec![TerminalInput::ShiftLeft(egui_mods_to_key_modifiers(
                *modifiers,
                super_pressed,
            ))]
            .into(),
            Event::Key {
                key: Key::ShiftRight,
                pressed: true,
                modifiers,
                ..
            } => vec![TerminalInput::ShiftRight(egui_mods_to_key_modifiers(
                *modifiers,
                super_pressed,
            ))]
            .into(),
            Event::Key {
                key: Key::ControlLeft,
                pressed: true,
                modifiers,
                ..
            } => vec![TerminalInput::ControlLeft(egui_mods_to_key_modifiers(
                *modifiers,
                super_pressed,
            ))]
            .into(),
            Event::Key {
                key: Key::ControlRight,
                pressed: true,
                modifiers,
                ..
            } => vec![TerminalInput::ControlRight(egui_mods_to_key_modifiers(
                *modifiers,
                super_pressed,
            ))]
            .into(),
            Event::Key {
                key: Key::AltLeft,
                pressed: true,
                modifiers,
                ..
            } => vec![TerminalInput::AltLeft(egui_mods_to_key_modifiers(
                *modifiers,
                super_pressed,
            ))]
            .into(),
            Event::Key {
                key: Key::AltRight,
                pressed: true,
                modifiers,
                ..
            } => vec![TerminalInput::AltRight(egui_mods_to_key_modifiers(
                *modifiers,
                super_pressed,
            ))]
            .into(),
            Event::Key {
                key: Key::SuperLeft,
                pressed: true,
                modifiers,
                ..
            } => vec![TerminalInput::SuperLeft(egui_mods_to_key_modifiers(
                *modifiers,
                super_pressed,
            ))]
            .into(),
            Event::Key {
                key: Key::SuperRight,
                pressed: true,
                modifiers,
                ..
            } => vec![TerminalInput::SuperRight(egui_mods_to_key_modifiers(
                *modifiers,
                super_pressed,
            ))]
            .into(),

            // Wildcard Ctrl+<letter> arm — must be AFTER all specific key arms
            // (arrows, navigation, F-keys, etc.) so those aren't swallowed.
            Event::Key {
                key,
                pressed: true,
                modifiers: Modifiers { ctrl: true, .. },
                ..
            } => {
                if let Some(inputs) = control_key(*key) {
                    inputs
                } else {
                    debug!("Ignoring ctrl key with no C0 mapping: {}", key.name());
                    continue;
                }
            }
            // log any Event::Key that we don't handle
            // Event::Key { key, pressed: true, .. } => {
            //     warn!("Unhandled key event: {:?}", key);
            //     continue;
            // }
            Event::Key {
                key: Key::Escape,
                pressed: true,
                ..
            } => [TerminalInput::Escape].as_ref().into(),
            Event::Key {
                key,
                pressed: true,
                repeat: true,
                ..
            } => {
                previous_key = Some(*key);
                continue;
            }
            Event::Paste(text) => {
                // The windowing layer already read the clipboard (via the
                // reliable egui-winit path) and injected this event. Stash the
                // text for handling in `update()` with access to the config and
                // confirm dialog. Do NOT re-read the clipboard via arboard here
                // — that path is unreliable on Wayland and would discard this
                // known-good text.
                //
                // The windowing paste interceptor fires for any `command + v`,
                // including the `PasteUnsafe` combo (Ctrl+Shift+Alt+V), so the
                // BindingMap entry never sees it on that path. Detect the
                // bypass intent here by the Alt modifier: Alt held ==
                // PasteUnsafe (skip the guard).
                view_state.pending_paste = Some(PendingPaste {
                    text: text.clone(),
                    bypass_guard: input.modifiers.alt,
                });
                continue;
            }
            Event::PointerGone => {
                view_state.mouse_position = None;
                last_reported_mouse_pos = None;
                continue;
            }
            Event::WindowFocused(focused) => {
                view_state.window_focused = *focused;
                // Forward focus change to the PTY consumer thread so it can
                // send the focus-reporting escape sequence if enabled.
                send_or_log!(
                    input_tx,
                    InputEvent::FocusChange(*focused),
                    "Failed to send focus change event"
                );

                if !*focused {
                    view_state.mouse_position = None;
                    last_reported_mouse_pos = None;
                }
                // Task 114.8: clear the held-key tracking (physical Super) on
                // focus-loss so stale "held" state cannot leak into a later
                // key report. Per the transition-only binding decision we emit
                // NO synthetic release for it — the next real key rebuilds
                // tracking honestly on focus-gain. If the user released Super
                // in another window while we were unfocused, we simply never
                // saw it; resetting here avoids reporting a phantom-held Super
                // afterward. Factored into `held_keys_after_focus_change` so
                // the invariant is unit-testable without an egui harness.
                super_pressed = held_keys_after_focus_change(*focused, super_pressed);

                continue;
            }
            Event::PointerMoved(pos) => {
                view_state.mouse_position = Some(*pos);

                // Ignore pointer moves outside the terminal area (e.g. over
                // the tab bar) so they do not pollute mouse-tracking state or
                // start spurious text selections.
                if !terminal_rect.contains(*pos) {
                    continue;
                }

                let (x, y) = encode_egui_mouse_pos_as_usize(
                    *pos,
                    (character_size_x, character_size_y),
                    terminal_origin,
                );

                let position = FreminalMousePosition::new(x, y);
                let (previous, current) =
                    if let Some(last_mouse_position) = &mut last_reported_mouse_pos {
                        (
                            last_mouse_position.clone(),
                            last_mouse_position.new_from_previous_mouse_state(position),
                        )
                    } else {
                        (
                            PreviousMouseState::default(),
                            PreviousMouseState::new(
                                PointerButton::Primary,
                                false,
                                position,
                                Modifiers::default(),
                            ),
                        )
                    };

                let res = handle_pointer_moved(
                    &current,
                    &previous,
                    effective_mouse_tracking,
                    mouse_encoding,
                );

                last_reported_mouse_pos = Some(current);

                // Record mouse move event (every move — debouncing is a future optimization).
                if let Some(ctx) = recording_ctx {
                    // Saturating `usize -> u32` for recording coords: any
                    // realistic window size fits in u32; clamp on overflow.
                    ctx.handle.emit(EventPayload::MouseMove {
                        window_id: ctx.window_id,
                        pane_id: ctx.pane_id,
                        x: u32::try_from(x).unwrap_or(u32::MAX),
                        y: u32::try_from(y).unwrap_or(u32::MAX),
                        coalesced_count: 1,
                    });
                }

                if let Some(res) = res {
                    res
                } else {
                    // Mouse tracking is off — update text selection if a drag
                    // is in progress.
                    if view_state.selection.is_selecting {
                        let abs_row = screen_row_to_buffer_row(snap, &view_state.folded_blocks, y);
                        let end_col = if view_state.click_count >= 3 {
                            // Triple-click drag — snap end to line boundaries.
                            let anchor_row = view_state.selection.anchor.map_or(abs_row, |a| a.row);
                            let snap_y = abs_row.saturating_sub(
                                visible_window_start(snap).saturating_sub(snap.window_extra_rows),
                            );
                            let (line_start, line_end) = crate::gui::view_state::line_boundaries(
                                &snap.visible_chars,
                                snap_y,
                            );
                            if abs_row >= anchor_row {
                                line_end
                            } else {
                                line_start
                            }
                        } else if view_state.click_count == 2 {
                            // Double-click drag — snap end to word boundaries.
                            let anchor_row = view_state.selection.anchor.map_or(abs_row, |a| a.row);
                            let anchor_col = view_state.selection.anchor.map_or(x, |a| a.col);
                            let (word_start, word_end) =
                                crate::gui::view_state::word_boundaries(&snap.visible_chars, y, x);
                            if abs_row > anchor_row
                                || (abs_row == anchor_row && word_end >= anchor_col)
                            {
                                word_end
                            } else {
                                word_start
                            }
                        } else {
                            // Single-click drag — track exact cell.
                            x
                        };
                        view_state.selection.end = Some(CellCoord {
                            col: end_col,
                            row: abs_row,
                        });
                        // Keep block mode in sync with the current Alt state so
                        // releasing or pressing Alt mid-drag switches mode live.
                        if view_state.click_count <= 1 {
                            view_state.selection.is_block = input.modifiers.alt;
                        }
                        state_changed = true;
                    }
                    continue;
                }
            }
            Event::PointerButton {
                button,
                pressed,
                modifiers,
                pos,
            } => {
                // Ignore clicks outside the terminal area (e.g. tab bar
                // buttons) so they do not start text selections or generate
                // spurious mouse reports at row 0.
                if !terminal_rect.contains(*pos) {
                    continue;
                }

                state_changed = true;

                let (x, y) = encode_egui_mouse_pos_as_usize(
                    *pos,
                    (character_size_x, character_size_y),
                    terminal_origin,
                );
                let mouse_pos = FreminalMousePosition::new(x, y);
                let new_mouse_position =
                    PreviousMouseState::new(*button, *pressed, mouse_pos.clone(), *modifiers);

                // Shift+right-click escape hatch: when mouse tracking is
                // active, holding Shift overrides PTY forwarding so the user
                // can access the terminal emulator's context menu even inside
                // mouse-aware applications (tmux, vim, etc.).
                let shift_right_click = *button == PointerButton::Secondary
                    && *pressed
                    && modifiers.shift
                    && *effective_mouse_tracking != MouseTrack::NoTracking;

                let response = if shift_right_click {
                    None
                } else {
                    handle_pointer_button(
                        *button,
                        &new_mouse_position,
                        effective_mouse_tracking,
                        mouse_encoding,
                    )
                };

                last_reported_mouse_pos = Some(new_mouse_position.clone());

                // Record mouse button event.
                if let Some(ctx) = recording_ctx {
                    // Saturating `usize -> u32` for recording coords.
                    ctx.handle.emit(EventPayload::MouseButton {
                        window_id: ctx.window_id,
                        pane_id: ctx.pane_id,
                        button: pointer_button_to_u8(*button),
                        pressed: *pressed,
                        x: u32::try_from(x).unwrap_or(u32::MAX),
                        y: u32::try_from(y).unwrap_or(u32::MAX),
                    });
                }

                if *button == PointerButton::Primary && *pressed {
                    left_mouse_button_pressed = true;
                }

                // Fold placeholder click: if this primary press landed on a
                // fold placeholder row, unfold it and consume the event so
                // it does not start a text selection or get reported to the
                // PTY via mouse tracking. Active-pane gating is respected by
                // the surrounding event loop (inactive panes only reach this
                // point for click-to-focus, but unfolding on focus-click is
                // acceptable and matches user intent).
                if *button == PointerButton::Primary
                    && *pressed
                    && let Some(block_id) = hit_test_placeholder(placeholder_rects, *pos)
                {
                    view_state.unfold(block_id);
                    resend_scroll_window(snap, view_state, input_tx);
                    continue;
                }

                if let Some(response) = response {
                    response
                } else {
                    // Mouse tracking is off (or overridden by Shift) — handle
                    // text selection and right-click context menu.
                    if *button == PointerButton::Secondary && *pressed {
                        // Record the right-clicked cell so the widget layer
                        // can open the context menu and detect URLs.
                        let abs_row = screen_row_to_buffer_row(snap, &view_state.folded_blocks, y);
                        view_state.context_menu_cell = Some(CellCoord {
                            col: x,
                            row: abs_row,
                        });
                        view_state.context_menu_pos = Some(*pos);
                    } else if *button == PointerButton::Primary {
                        if *pressed {
                            // If there is an active selection, this click
                            // should ONLY clear it — not start a new one.
                            // The user must click a second time to begin
                            // selecting again. This matches the behaviour
                            // of most terminal emulators (iTerm2, kitty,
                            // Alacritty, GNOME Terminal).
                            if view_state.selection.has_selection() {
                                view_state.selection.clear();
                                view_state.click_count = 0;
                                continue;
                            }

                            // Start a new selection at this cell.
                            // Use buffer-absolute row so the selection
                            // survives scroll offset changes.
                            let abs_row =
                                screen_row_to_buffer_row(snap, &view_state.folded_blocks, y);
                            // Snapshot-row index for `visible_chars` lookups
                            // (word/line boundaries), accounting for the
                            // extra-row fold window.
                            let snap_y = abs_row.saturating_sub(
                                visible_window_start(snap).saturating_sub(snap.window_extra_rows),
                            );
                            let coord = CellCoord {
                                col: x,
                                row: abs_row,
                            };
                            let click_count =
                                view_state.register_click(coord, std::time::Instant::now());

                            if click_count >= 3 {
                                // Triple-click — select the entire visual line.
                                let (start_col, end_col) = crate::gui::view_state::line_boundaries(
                                    &snap.visible_chars,
                                    snap_y,
                                );
                                view_state.selection.anchor = Some(CellCoord {
                                    col: start_col,
                                    row: abs_row,
                                });
                                view_state.selection.end = Some(CellCoord {
                                    col: end_col,
                                    row: abs_row,
                                });
                            } else if click_count == 2 {
                                // Double-click — select the word under the cursor.
                                let (start_col, end_col) = crate::gui::view_state::word_boundaries(
                                    &snap.visible_chars,
                                    snap_y,
                                    x,
                                );
                                view_state.selection.anchor = Some(CellCoord {
                                    col: start_col,
                                    row: abs_row,
                                });
                                view_state.selection.end = Some(CellCoord {
                                    col: end_col,
                                    row: abs_row,
                                });
                            } else {
                                // Single click — start point selection.
                                // Alt+drag activates rectangular block selection.
                                view_state.selection.anchor = Some(coord);
                                view_state.selection.end = Some(coord);
                                view_state.selection.is_block = modifiers.alt;
                            }
                            view_state.selection.is_selecting = true;
                        } else if view_state.selection.is_selecting {
                            // Mouse released — finalize the selection.
                            // Respect click_count so double/triple-click
                            // selections are not collapsed to the raw
                            // mouse position on release.
                            let abs_row =
                                screen_row_to_buffer_row(snap, &view_state.folded_blocks, y);
                            let end_col = release_end_col(view_state, snap, x, abs_row);
                            let end_coord = CellCoord {
                                col: end_col,
                                row: abs_row,
                            };
                            view_state.selection.end = Some(end_coord);
                            view_state.selection.is_selecting = false;

                            // Record selection event if a real selection exists.
                            if let Some(ctx) = recording_ctx
                                && let Some(anchor) = view_state.selection.anchor
                                && anchor != end_coord
                            {
                                // Saturating `usize -> u32` for recording
                                // row/col — any realistic terminal fits in u32.
                                ctx.handle.emit(EventPayload::SelectionEvent {
                                    pane_id: ctx.pane_id,
                                    start_row: u32::try_from(anchor.row).unwrap_or(u32::MAX),
                                    start_col: u32::try_from(anchor.col).unwrap_or(u32::MAX),
                                    end_row: u32::try_from(end_coord.row).unwrap_or(u32::MAX),
                                    end_col: u32::try_from(end_coord.col).unwrap_or(u32::MAX),
                                    is_block: view_state.selection.is_block,
                                });
                            }

                            // If anchor == end the user clicked without
                            // dragging — there is no real selection.
                            // Clear it so the next click starts fresh
                            // rather than hitting the "clear existing
                            // selection" path.
                            if view_state.selection.anchor == Some(end_coord) {
                                view_state.selection.clear();
                            }
                        }
                    }
                    continue;
                }
            }
            Event::MouseWheel {
                delta,
                modifiers,
                unit,
                ..
            } => {
                match unit {
                    egui::MouseWheelUnit::Point => {
                        scroll_amount += delta.y;
                    }
                    egui::MouseWheelUnit::Line => {
                        scroll_amount = delta.y.mul_add(character_size_y, scroll_amount);
                    }
                    egui::MouseWheelUnit::Page => {
                        error!("Unhandled MouseWheelUnit: {:?}", unit);
                        continue;
                    }
                }
                // Horizontal (x-axis) scroll is intentionally ignored — the terminal
                // mouse protocol has no horizontal wheel events (see mouse.rs).

                if scroll_amount.abs() < character_size_y {
                    continue;
                }

                // The amount scrolled should be in increments of the character size.
                // The remainder is carried to the next scroll event.
                //
                // `trunc()` rounds toward zero so the remainder preserves its
                // sign.  `floor()` would overshoot for negative values (e.g.
                // floor(-20.3) = -21), leaving a positive remainder that delays
                // the next downward event and makes trackpad scrolling feel
                // uneven.
                let scroll_amount_to_do = scroll_amount.trunc();
                scroll_amount -= scroll_amount_to_do;

                state_changed = true;

                // Record scroll event.
                if let Some(ctx) = recording_ctx {
                    ctx.handle.emit(EventPayload::MouseScroll {
                        window_id: ctx.window_id,
                        pane_id: ctx.pane_id,
                        delta_x: delta.x,
                        delta_y: delta.y,
                    });
                }

                // Resolve the mouse position for scroll reporting.  Prefer
                // `last_reported_mouse_pos` (set by PointerMoved), but fall
                // back to egui's `latest_pos()` when the tracked position was
                // cleared (e.g. by PointerGone or window unfocus).  Without
                // this fallback, scrolling after re-focusing the window would
                // bypass mouse tracking and send arrow keys instead.
                if last_reported_mouse_pos.is_none()
                    && let Some(hover) = input.pointer.latest_pos()
                    && terminal_rect.contains(hover)
                {
                    let (x, y) = encode_egui_mouse_pos_as_usize(
                        hover,
                        (character_size_x, character_size_y),
                        terminal_origin,
                    );
                    let position = FreminalMousePosition::new(x, y);
                    last_reported_mouse_pos = Some(PreviousMouseState::new(
                        PointerButton::Primary,
                        false,
                        position,
                        *modifiers,
                    ));
                }

                if let Some(last_mouse_position) = &mut last_reported_mouse_pos {
                    // update the modifiers if necessary
                    if last_mouse_position.modifiers != *modifiers {
                        last_mouse_position.modifiers = *modifiers;
                        *last_mouse_position = last_mouse_position.clone();
                    }

                    // Compute how many discrete scroll lines this delta
                    // represents and the unit direction (+1.0 up, -1.0 down).
                    let lines_f = scroll_amount_to_do / character_size_y;
                    let line_count = lines_f
                        .abs()
                        .round()
                        .max(1.0)
                        .approx_as::<usize>()
                        .unwrap_or(1);
                    let direction = lines_f.signum(); // +1.0 or -1.0
                    let unit_delta = egui::Vec2::new(0.0, direction);

                    // Probe once to see if the active mouse-tracking mode
                    // handles scroll events.
                    if handle_pointer_scroll(
                        unit_delta,
                        last_mouse_position,
                        effective_mouse_tracking,
                        mouse_encoding,
                    )
                    .is_some()
                    {
                        // Mouse tracking is active — send one escape sequence
                        // per line, matching xterm / kitty / WezTerm behavior.
                        for _ in 0..line_count {
                            if let Some(response) = handle_pointer_scroll(
                                unit_delta,
                                last_mouse_position,
                                effective_mouse_tracking,
                                mouse_encoding,
                            ) {
                                send_terminal_inputs(
                                    response.as_ref(),
                                    input_tx,
                                    &InputModes::from_snapshot(snap),
                                    &KeyEventMeta::PRESS,
                                );
                            }
                        }
                        continue;
                    }
                }

                // Mouse tracking is off or no mouse position tracked — handle
                // scroll ourselves.
                handle_scroll_fallback(
                    scroll_amount_to_do,
                    character_size_y,
                    snap,
                    input_tx,
                    view_state,
                );
                continue;
            }
            _ => {
                continue;
            }
        };

        if !inputs.is_empty() {
            state_changed = true;

            // Capture encoded bytes for recording before sending.
            let modes = InputModes::from_snapshot(snap);
            let encoded = encode_terminal_inputs(&inputs, &modes, &event_meta);

            if !encoded.is_empty()
                && let Err(e) = input_tx.send(InputEvent::Key(encoded.clone()))
            {
                error!("Failed to send key input to PTY consumer: {e}");
            }

            // Broadcast (Task 74): mirror this keyboard/paste payload to every
            // other pane in the tab. The encoded bytes already carry the
            // active pane's bracketed-paste / cursor-key wrapping.
            broadcast_key_bytes(key_broadcast_targets, &encoded);

            // Emit recording event for keyboard input.
            if let Some(ctx) = recording_ctx {
                // For paste events, emit ClipboardPaste instead of KeyboardInput.
                if let Event::Paste(text) = event {
                    ctx.handle.emit(EventPayload::ClipboardPaste {
                        pane_id: ctx.pane_id,
                        data: text.as_bytes().to_vec(),
                    });
                } else {
                    let (key_name, modifier_bits) = match event {
                        Event::Text(text) => (text.clone(), 0u8),
                        Event::Key { key, modifiers, .. } => (
                            key.name().to_string(),
                            modifiers_to_recording_u8(*modifiers),
                        ),
                        Event::Copy => ("Ctrl+C".to_string(), 2u8),
                        Event::Cut => ("Ctrl+X".to_string(), 2u8),
                        _ => (String::new(), 0u8),
                    };
                    ctx.handle.emit(EventPayload::KeyboardInput {
                        window_id: ctx.window_id,
                        pane_id: ctx.pane_id,
                        key_name,
                        modifiers: modifier_bits,
                        encoded,
                    });
                }
            }
        }
    }

    if state_changed {
        debug!("Inputs detected, forwarding to PTY consumer thread");
    }

    (
        left_mouse_button_pressed,
        last_reported_mouse_pos,
        previous_key,
        scroll_amount,
        clipboard_pending,
        deferred_actions,
        super_pressed,
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod fold_target_tests {
    //! Tests for [`find_fold_target`].
    //!
    //! These cover the selection logic in isolation from the dispatcher and
    //! from PTY/clipboard side effects.  The dispatcher itself requires a
    //! live `Sender<InputEvent>` and is exercised by integration tests; the
    //! interesting behaviour here is purely "given a snapshot, which block
    //! should we fold?".

    use super::*;
    use freminal_common::buffer_states::command_block::{CommandBlock, CommandBlockId};
    use freminal_common::buffer_states::cursor::CursorPos;
    use std::sync::Arc;
    use std::time::SystemTime;

    /// Build a completed command block occupying rows `[prompt..=end]`.
    fn completed(id: u64, prompt: usize, end: usize) -> CommandBlock {
        CommandBlock {
            id: CommandBlockId(id),
            fid: format!("test-{id}"),
            prompt_start_row: prompt,
            command_start_row: Some(prompt),
            output_start_row: Some(prompt + 1),
            end_row: Some(end),
            exit_code: Some(0),
            cwd: None,
            started_at: SystemTime::UNIX_EPOCH,
            executed_at: Some(SystemTime::UNIX_EPOCH),
            finished_at: Some(SystemTime::UNIX_EPOCH),
        }
    }

    /// Build a running (still-open) block — no `end_row`.
    fn running(id: u64, prompt: usize) -> CommandBlock {
        CommandBlock {
            id: CommandBlockId(id),
            fid: format!("test-{id}"),
            prompt_start_row: prompt,
            command_start_row: Some(prompt),
            output_start_row: Some(prompt + 1),
            end_row: None,
            exit_code: None,
            cwd: None,
            started_at: SystemTime::UNIX_EPOCH,
            executed_at: None,
            finished_at: None,
        }
    }

    fn snap_with(blocks: Vec<CommandBlock>, cursor_row: usize) -> TerminalSnapshot {
        let mut s = TerminalSnapshot::empty();
        s.command_blocks = Arc::from(blocks);
        s.cursor_pos = CursorPos {
            x: 0,
            y: cursor_row,
        };
        s
    }

    #[test]
    fn cursor_inside_completed_block_selects_that_block() {
        // Cursor lives at row 7, which is inside block 1's [5..=10].  The
        // most-recent fallback would pick block 2 (rows 15..=20) — but the
        // cursor-inside rule wins.
        let snap = snap_with(vec![completed(1, 5, 10), completed(2, 15, 20)], 7);
        assert_eq!(find_fold_target(&snap), Some(CommandBlockId(1)));
    }

    #[test]
    fn cursor_outside_all_blocks_falls_back_to_most_recent_completed() {
        // The realistic case: cursor sits on the active prompt line (row
        // 25), which is past every completed block.  We expect the
        // newest-appended completed block (id 2).
        let snap = snap_with(vec![completed(1, 5, 10), completed(2, 15, 20)], 25);
        assert_eq!(find_fold_target(&snap), Some(CommandBlockId(2)));
    }

    #[test]
    fn running_block_is_never_selected() {
        // Cursor is inside the running block's rows, but running blocks
        // are not foldable.  The fallback picks the completed block.
        let snap = snap_with(vec![completed(1, 5, 10), running(2, 15)], 17);
        assert_eq!(find_fold_target(&snap), Some(CommandBlockId(1)));
    }

    #[test]
    fn running_block_does_not_shadow_completed_in_recency_fallback() {
        // Even when the most recently *appended* block is still running,
        // the recency fallback must skip it and return the most recent
        // *completed* block.
        let snap = snap_with(vec![completed(1, 5, 10), running(2, 15)], 25);
        assert_eq!(find_fold_target(&snap), Some(CommandBlockId(1)));
    }

    #[test]
    fn no_completed_blocks_returns_none() {
        // A snapshot with only running blocks (or none at all) yields
        // no fold target — the keybinding silently no-ops.
        let snap = snap_with(vec![running(1, 5)], 7);
        assert_eq!(find_fold_target(&snap), None);

        let snap = snap_with(vec![], 0);
        assert_eq!(find_fold_target(&snap), None);
    }

    #[test]
    fn block_missing_command_start_row_is_skipped() {
        // A block that saw `A` (prompt_start_row) but never `C`
        // (command_start_row) has no foldable body and must be excluded
        // from both passes.  The next completed block wins.
        let mut partial = completed(2, 15, 20);
        partial.command_start_row = None;
        let snap = snap_with(vec![completed(1, 5, 10), partial], 25);
        assert_eq!(find_fold_target(&snap), Some(CommandBlockId(1)));
    }

    // ---- find_last_copyable_block ----

    #[test]
    fn last_copyable_picks_newest_completed_block() {
        // Two completed blocks; the most recently appended (id 2) wins.
        let snap = snap_with(vec![completed(1, 5, 10), completed(2, 15, 20)], 0);
        assert_eq!(
            find_last_copyable_block(&snap).map(|b| b.id),
            Some(CommandBlockId(2))
        );
    }

    #[test]
    fn last_copyable_skips_running_blocks() {
        // The newest block is still running (no end_row); we must fall
        // back to the previous completed block.
        let snap = snap_with(vec![completed(1, 5, 10), running(2, 15)], 0);
        assert_eq!(
            find_last_copyable_block(&snap).map(|b| b.id),
            Some(CommandBlockId(1))
        );
    }

    #[test]
    fn last_copyable_skips_block_missing_output_start_row() {
        // A completed block that never saw the `C` marker has no
        // copyable output range and must be skipped.
        let mut partial = completed(2, 15, 20);
        partial.output_start_row = None;
        let snap = snap_with(vec![completed(1, 5, 10), partial], 0);
        assert_eq!(
            find_last_copyable_block(&snap).map(|b| b.id),
            Some(CommandBlockId(1))
        );
    }

    #[test]
    fn last_copyable_empty_snapshot_returns_none() {
        let snap = snap_with(vec![], 0);
        assert!(find_last_copyable_block(&snap).is_none());

        let snap = snap_with(vec![running(1, 5)], 0);
        assert!(find_last_copyable_block(&snap).is_none());
    }

    // ---- find_block_containing_row ----

    #[test]
    fn containing_row_inside_block_returns_that_block() {
        let snap = snap_with(vec![completed(1, 5, 10), completed(2, 15, 20)], 0);
        assert_eq!(
            find_block_containing_row(&snap, 7).map(|b| b.id),
            Some(CommandBlockId(1))
        );
        assert_eq!(
            find_block_containing_row(&snap, 18).map(|b| b.id),
            Some(CommandBlockId(2))
        );
    }

    #[test]
    fn containing_row_on_boundary_is_inclusive() {
        // Both the command_start_row and end_row endpoints belong to
        // the block.
        let snap = snap_with(vec![completed(1, 5, 10)], 0);
        assert_eq!(
            find_block_containing_row(&snap, 5).map(|b| b.id),
            Some(CommandBlockId(1))
        );
        assert_eq!(
            find_block_containing_row(&snap, 10).map(|b| b.id),
            Some(CommandBlockId(1))
        );
    }

    #[test]
    fn containing_row_outside_all_blocks_returns_none() {
        let snap = snap_with(vec![completed(1, 5, 10), completed(2, 15, 20)], 0);
        assert!(find_block_containing_row(&snap, 12).is_none());
        assert!(find_block_containing_row(&snap, 25).is_none());
    }

    #[test]
    fn containing_row_skips_running_block() {
        // The cursor row falls within the running block's prompt..end
        // span, but running blocks have no end_row and must be excluded.
        let snap = snap_with(vec![running(1, 5)], 0);
        assert!(find_block_containing_row(&snap, 7).is_none());
    }

    #[test]
    fn containing_row_skips_block_missing_command_start_row() {
        // Blocks without a `C` marker have no usable start row.
        let mut partial = completed(1, 5, 10);
        partial.command_start_row = None;
        let snap = snap_with(vec![partial], 0);
        assert!(find_block_containing_row(&snap, 7).is_none());
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod scroll_window_tests {
    //! Tests for the fold-aware scroll helpers [`scroll_event`] and
    //! [`screen_row_to_buffer_row`].

    use super::*;
    use freminal_common::buffer_states::command_block::{CommandBlock, CommandBlockId};
    use std::collections::HashSet;
    use std::sync::Arc;
    use std::time::SystemTime;

    fn completed(id: u64, prompt: usize, end: usize) -> CommandBlock {
        CommandBlock {
            id: CommandBlockId(id),
            fid: format!("test-{id}"),
            prompt_start_row: prompt,
            command_start_row: Some(prompt),
            output_start_row: Some(prompt + 1),
            end_row: Some(end),
            exit_code: Some(0),
            cwd: None,
            started_at: SystemTime::UNIX_EPOCH,
            executed_at: Some(SystemTime::UNIX_EPOCH),
            finished_at: Some(SystemTime::UNIX_EPOCH),
        }
    }

    /// Snapshot with `total_rows` buffer rows, `term_height` visible rows, the
    /// given scroll offset, and the supplied command blocks.
    fn snap_geo(
        total_rows: usize,
        term_height: usize,
        scroll_offset: usize,
        blocks: Vec<CommandBlock>,
    ) -> TerminalSnapshot {
        let mut s = TerminalSnapshot::empty();
        s.total_rows = total_rows;
        s.term_height = term_height;
        s.scroll_offset = scroll_offset;
        s.command_blocks = Arc::from(blocks);
        s
    }

    fn folded(ids: &[u64]) -> HashSet<CommandBlockId> {
        ids.iter().copied().map(CommandBlockId).collect()
    }

    #[test]
    fn scroll_event_no_folds_has_zero_extra() {
        // total 100, height 24, at live bottom. No folds → extra 0.
        let snap = snap_geo(100, 24, 0, vec![]);
        match scroll_event(&snap, &HashSet::new(), 0) {
            InputEvent::ScrollOffset { offset, extra_rows } => {
                assert_eq!(offset, 0);
                assert_eq!(extra_rows, 0);
            }
            other => panic!("expected ScrollOffset, got {other:?}"),
        }
    }

    #[test]
    fn scroll_event_fold_in_window_requests_extra() {
        // total 100, height 24, live bottom → window [76, 100).
        // Block 80..=89: the prompt/command line (row 80) stays visible and
        // only the OUTPUT rows 81..=89 (9 rows) are folded; collapsing them to
        // one placeholder frees 8 rows of screen.
        let snap = snap_geo(100, 24, 0, vec![completed(1, 80, 89)]);
        match scroll_event(&snap, &folded(&[1]), 0) {
            InputEvent::ScrollOffset { offset, extra_rows } => {
                assert_eq!(offset, 0);
                assert_eq!(extra_rows, 8, "9 folded output rows free 8 rows of screen");
            }
            other => panic!("expected ScrollOffset, got {other:?}"),
        }
    }

    #[test]
    fn scroll_event_unfolded_block_no_extra() {
        // The block exists but is not in the folded set → no extra rows.
        let snap = snap_geo(100, 24, 0, vec![completed(1, 80, 89)]);
        match scroll_event(&snap, &HashSet::new(), 0) {
            InputEvent::ScrollOffset { extra_rows, .. } => assert_eq!(extra_rows, 0),
            other => panic!("expected ScrollOffset, got {other:?}"),
        }
    }

    #[test]
    fn scroll_event_uses_target_offset_window() {
        // At a scrolled-back offset the window moves; the fold must be
        // evaluated against the *target* window, not the snapshot's current
        // one. Offset 20 → window [56, 80). Block 80..=89 is below the window
        // (its end is at the live bottom edge) — only its overlap counts.
        let snap = snap_geo(100, 24, 0, vec![completed(1, 60, 69)]);
        // Target offset 20 → window [56, 80). Block 60..=69: output rows
        // 61..=69 (9 rows) folded → frees 8.
        match scroll_event(&snap, &folded(&[1]), 20) {
            InputEvent::ScrollOffset { offset, extra_rows } => {
                assert_eq!(offset, 20);
                assert_eq!(extra_rows, 8);
            }
            other => panic!("expected ScrollOffset, got {other:?}"),
        }
    }

    #[test]
    fn screen_row_to_buffer_no_folds_is_window_start_plus_row() {
        // No folds → screen row maps directly: win_start (76) + screen.
        let snap = snap_geo(100, 24, 0, vec![]);
        assert_eq!(screen_row_to_buffer_row(&snap, &HashSet::new(), 0), 76);
        assert_eq!(screen_row_to_buffer_row(&snap, &HashSet::new(), 5), 81);
    }

    #[test]
    fn screen_row_to_buffer_with_fold_skips_collapsed_rows() {
        // Window [76, 100); fold block 80..=89 (output 81..=89 folded, 9 rows)
        // collapses to a placeholder. Screen rows above the fold map 1:1;
        // rows below the placeholder jump past the hidden rows.
        let snap = snap_geo(100, 24, 0, vec![completed(1, 80, 89)]);
        let f = folded(&[1]);
        // Row 0 is buffer row 76 (well above the fold which starts at output
        // row 81).
        assert_eq!(screen_row_to_buffer_row(&snap, &f, 0), 76);
        // A row well past the placeholder must map beyond the folded span
        // (>= 90), proving the hidden rows are skipped.
        let late = screen_row_to_buffer_row(&snap, &f, 20);
        assert!(
            late >= 90,
            "rows after the fold placeholder must skip the hidden span, got {late}"
        );
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod broadcast_tests {
    //! Tests for [`broadcast_key_bytes`] — the Task 74 keyboard-input
    //! fan-out helper. These verify the bytes are mirrored verbatim to
    //! every target channel and that a closed/disconnected target does
    //! not abort the fan-out to the remaining targets.

    use super::*;

    #[test]
    fn broadcasts_bytes_to_every_target() {
        let (tx1, rx1) = crossbeam_channel::unbounded();
        let (tx2, rx2) = crossbeam_channel::unbounded();
        let (tx3, rx3) = crossbeam_channel::unbounded();

        broadcast_key_bytes(&[tx1, tx2, tx3], b"hello");

        for rx in [&rx1, &rx2, &rx3] {
            match rx.try_recv() {
                Ok(InputEvent::Key(bytes)) => assert_eq!(bytes, b"hello"),
                other => panic!("expected InputEvent::Key(b\"hello\"), got {other:?}"),
            }
        }
    }

    #[test]
    fn empty_bytes_sends_nothing() {
        let (tx, rx) = crossbeam_channel::unbounded();
        broadcast_key_bytes(std::slice::from_ref(&tx), b"");
        assert!(rx.try_recv().is_err(), "empty payload must not be sent");
    }

    #[test]
    fn empty_targets_is_a_noop() {
        // Must not panic with no targets (the common broadcast-off case).
        broadcast_key_bytes(&[], b"data");
    }

    #[test]
    fn disconnected_target_does_not_abort_remaining_fanout() {
        let (tx_dead, rx_dead) = crossbeam_channel::unbounded::<InputEvent>();
        drop(rx_dead); // Make tx_dead's send fail.
        let (tx_live, rx_live) = crossbeam_channel::unbounded();

        // Dead target first, live target second: the live target must still
        // receive the bytes even though the first send errored.
        broadcast_key_bytes(&[tx_dead, tx_live], b"x");

        match rx_live.try_recv() {
            Ok(InputEvent::Key(bytes)) => assert_eq!(bytes, b"x"),
            other => panic!("expected InputEvent::Key(b\"x\"), got {other:?}"),
        }
    }
}

#[cfg(test)]
mod key_modifiers_tests {
    //! Tests for [`egui_mods_to_key_modifiers`] — the Task 101.2 `super_key`
    //! wiring. Covers the Linux/Windows path (tracked `super_held` hold-state)
    //! and the macOS path (`Modifiers::mac_cmd`), and confirms `Modifiers::command`
    //! is no longer folded into `ctrl`.

    use super::*;

    #[test]
    fn super_held_true_sets_super_key() {
        // Linux/Windows path: no egui `Modifiers` bit for the physical Super
        // key, so the caller-tracked `super_held` flag alone must set it.
        let km = egui_mods_to_key_modifiers(Modifiers::default(), true);
        assert!(km.super_key);
        assert!(!km.ctrl);
    }

    #[test]
    fn focus_loss_clears_held_super() {
        // Task 114.8: on focus-loss, held-key tracking (physical Super) is
        // cleared so a phantom-held Super cannot leak into a later report.
        assert!(!held_keys_after_focus_change(false, true));
        assert!(!held_keys_after_focus_change(false, false));
    }

    #[test]
    fn focus_gain_preserves_held_super() {
        // Task 114.8: focus-gain does not fabricate or drop held state — the
        // current value carries through (real modifiers arrive via the
        // compositor's ModifiersChanged).
        assert!(held_keys_after_focus_change(true, true));
        assert!(!held_keys_after_focus_change(true, false));
    }

    #[test]
    fn mac_cmd_sets_super_key_not_ctrl() {
        // macOS path: physical ⌘ sets `mac_cmd` (and mirrors into `command`),
        // which must route to `super_key`, not `ctrl`.
        let mods = Modifiers {
            mac_cmd: true,
            command: true,
            ..Modifiers::default()
        };
        let km = egui_mods_to_key_modifiers(mods, false);
        assert!(km.super_key);
        assert!(!km.ctrl);
    }

    #[test]
    fn plain_ctrl_sets_ctrl_not_super_key() {
        let mods = Modifiers {
            ctrl: true,
            ..Modifiers::default()
        };
        let km = egui_mods_to_key_modifiers(mods, false);
        assert!(km.ctrl);
        assert!(!km.super_key);
    }

    #[test]
    fn no_modifiers_no_super_held_is_empty() {
        let km = egui_mods_to_key_modifiers(Modifiers::default(), false);
        assert!(km.is_empty());
    }
}

#[cfg(test)]
mod modifier_keys_as_keys_tests {
    //! Tests for the Task 101.3 KKP "modifier keys as keys" GUI-side wiring:
    //! [`egui_key_to_terminal_input`] must map the 8 physical modifier-key
    //! `egui::Key` variants to their corresponding `TerminalInput` variants
    //! so KKP flag 2 release-forwarding (see `write_input_to_terminal`) can
    //! reconstruct the same event a press would have generated.

    use super::*;

    #[test]
    fn shift_left_maps_to_terminal_input_variant() {
        let ti = egui_key_to_terminal_input(Key::ShiftLeft, Modifiers::default(), false);
        assert!(matches!(ti, Some(TerminalInput::ShiftLeft(_))));
    }

    #[test]
    fn control_right_maps_to_terminal_input_variant() {
        let ti = egui_key_to_terminal_input(Key::ControlRight, Modifiers::default(), false);
        assert!(matches!(ti, Some(TerminalInput::ControlRight(_))));
    }

    #[test]
    fn super_left_maps_to_terminal_input_variant_with_super_key_set() {
        // super_held = true (as observed via the SuperLeft press itself)
        // must set `super_key` in the resulting modifier report.
        let ti = egui_key_to_terminal_input(Key::SuperLeft, Modifiers::default(), true);
        match ti {
            Some(TerminalInput::SuperLeft(km)) => assert!(km.super_key),
            other => panic!("expected Some(TerminalInput::SuperLeft(_)), got {other:?}"),
        }
    }

    #[test]
    fn alt_right_maps_to_terminal_input_variant() {
        let ti = egui_key_to_terminal_input(Key::AltRight, Modifiers::default(), false);
        assert!(matches!(ti, Some(TerminalInput::AltRight(_))));
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod raw_key_tests {
    //! Tests for the Task 114.7 egui-blocked raw-key delivery path:
    //! [`kitty_keycode_to_codepoint`]'s codepoint table and
    //! [`drain_pending_raw_keys`]'s end-to-end encoding through
    //! `send_terminal_inputs`.

    use super::*;
    use freminal_windowing::{RawKeyEvent, RawKeyMods};
    use winit::keyboard::KeyCode;

    #[test]
    fn maps_numpad_enter_to_its_codepoint() {
        assert_eq!(
            kitty_keycode_to_codepoint(KeyCode::NumpadEnter),
            Some(KKP_KP_ENTER_CODEPOINT)
        );
    }

    #[test]
    fn maps_numpad_5_to_its_codepoint() {
        assert_eq!(
            kitty_keycode_to_codepoint(KeyCode::Numpad5),
            Some(KKP_KP_5_CODEPOINT)
        );
    }

    #[test]
    fn maps_audio_volume_mute_to_its_codepoint() {
        assert_eq!(
            kitty_keycode_to_codepoint(KeyCode::AudioVolumeMute),
            Some(KKP_MUTE_VOLUME_CODEPOINT)
        );
    }

    #[test]
    fn unblocked_key_maps_to_none() {
        assert_eq!(kitty_keycode_to_codepoint(KeyCode::KeyA), None);
    }

    fn snap_with_kkp_flags(flags: u32) -> TerminalSnapshot {
        let mut s = TerminalSnapshot::empty();
        s.kitty_keyboard_flags = flags;
        s
    }

    #[test]
    fn drain_produces_no_bytes_when_kkp_is_off() {
        // Flags 0: `KittyFunctional` has no legacy encoding, so nothing is
        // sent to the PTY -- matches how all other KKP-only keys behave.
        let snap = snap_with_kkp_flags(0);
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut pending = vec![(
            RawKeyEvent {
                key_code: KeyCode::MediaPlayPause,
                pressed: true,
                repeat: false,
            },
            RawKeyMods::default(),
        )];

        drain_pending_raw_keys(&mut pending, &tx, &snap, false, &[]);

        assert!(
            rx.try_recv().is_err(),
            "no bytes should be sent when KKP is off"
        );
    }

    #[test]
    fn drain_ignores_unmapped_key() {
        // A blocked-set-adjacent key with no codepoint mapping (defensive
        // path) must be skipped without panicking or sending bytes.
        let snap = snap_with_kkp_flags(8);
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut pending = vec![(
            RawKeyEvent {
                key_code: KeyCode::KeyA,
                pressed: true,
                repeat: false,
            },
            RawKeyMods::default(),
        )];

        drain_pending_raw_keys(&mut pending, &tx, &snap, false, &[]);

        assert!(pending.is_empty());
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn empty_queue_is_a_noop() {
        let snap = snap_with_kkp_flags(8);
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut pending: Vec<(RawKeyEvent, RawKeyMods)> = Vec::new();

        drain_pending_raw_keys(&mut pending, &tx, &snap, false, &[]);

        assert!(rx.try_recv().is_err());
    }
}
