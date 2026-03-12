// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use crate::gui::{
    fonts::{FontConfig, setup_font_files},
    mouse::{
        FreminalMousePosition, PreviousMouseState, handle_pointer_button, handle_pointer_moved,
        handle_pointer_scroll,
    },
    view_state::{CellCoord, ViewState},
};

use crossbeam_channel::Sender;
use freminal_common::{
    buffer_states::{modes::mouse::MouseTrack, modes::rl_bracket::RlBracket, tchar::TChar},
    config::Config,
    pty_write::PtyWrite,
};
use freminal_terminal_emulator::{
    interface::{KeyModifiers, TerminalInput, TerminalInputPayload, collect_text},
    io::InputEvent,
    snapshot::TerminalSnapshot,
};

use eframe::egui::{
    self, Color32, Context, CursorIcon, Event, InputState, Key, Modifiers, PointerButton, Pos2,
    Rect, Ui,
};

use super::{
    atlas::GlyphAtlas,
    font_manager::FontManager,
    renderer::{
        CURSOR_QUAD_FLOATS, TerminalRenderer, build_background_verts, build_cursor_verts_only,
        build_foreground_verts,
    },
    shaping::ShapingCache,
};

use conv2::{ApproxFrom, ConvUtil, RoundToZero};
use eframe::egui_glow::CallbackFn;
use std::borrow::Cow;
use std::sync::{Arc, Mutex};

/// Convert egui [`Modifiers`] to the terminal-emulator's [`KeyModifiers`].
///
/// This is used for special keys (arrows, function keys, Home/End, etc.)
/// where the xterm modifier encoding (`ESC[1;Nm…`) applies. It must NOT
/// be used for regular ASCII keys where Ctrl already produces a C0 control
/// code — that path is handled by `control_key()` / `TerminalInput::Ctrl`.
const fn egui_mods_to_key_modifiers(m: Modifiers) -> KeyModifiers {
    KeyModifiers {
        shift: m.shift,
        ctrl: m.ctrl || m.command,
        alt: m.alt,
    }
}

fn control_key(key: Key) -> Option<Cow<'static, [TerminalInput]>> {
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

/// Handle mouse scroll when mouse tracking is off.
///
/// On the **alternate screen** (less, vim, htop, ...) scroll events are
/// converted to `ArrowUp`/`ArrowDown` key presses sent to the PTY — this
/// matches the behaviour of every major terminal emulator.
///
/// On the **primary screen** scroll events adjust the scroll offset and send
/// it to the PTY thread via `InputEvent::ScrollOffset`.  The PTY thread
/// clamps the value to `max_scroll_offset()` when building the next snapshot.
fn handle_scroll_fallback(
    scroll_amount_to_do: f32,
    character_size_y: f32,
    snap: &TerminalSnapshot,
    input_tx: &Sender<InputEvent>,
    view_state: &mut ViewState,
) {
    let lines = (scroll_amount_to_do / character_size_y).round();
    let abs_lines = lines.abs();

    if snap.is_alternate_screen {
        // Convert scroll delta to arrow key presses.
        // Safety: abs_lines >= 0, and we clamp to 1 below.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let count = (abs_lines as usize).max(1);
        let key = if lines > 0.0 {
            TerminalInput::ArrowUp(KeyModifiers::NONE)
        } else {
            TerminalInput::ArrowDown(KeyModifiers::NONE)
        };
        for _ in 0..count {
            send_terminal_input(&key, input_tx, snap.cursor_key_app_mode);
        }
    } else {
        // Primary screen: adjust scroll offset and send to PTY thread.
        // Multiply by 3 so each wheel tick scrolls 3 lines — matching the
        // default behavior of most terminal emulators (iTerm2, Alacritty,
        // kitty, GNOME Terminal, etc.).
        const SCROLL_MULTIPLIER: usize = 3;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let n = ((abs_lines as usize).max(1)) * SCROLL_MULTIPLIER;

        let new_offset = if lines > 0.0 {
            // Scroll up (into history) — increase offset.
            // The PTY thread will clamp to max_scroll_offset().
            view_state.scroll_offset.saturating_add(n)
        } else {
            // Scroll down (toward live bottom) — decrease offset.
            view_state.scroll_offset.saturating_sub(n)
        };

        if new_offset != view_state.scroll_offset {
            view_state.scroll_offset = new_offset;
            if let Err(e) = input_tx.send(InputEvent::ScrollOffset(new_offset)) {
                error!("Failed to send scroll offset to PTY consumer: {e}");
            }
        }
    }
}

/// Convert a `TerminalInput` value to raw bytes and send them to the PTY
/// consumer thread via `InputEvent::Key`.
///
/// The `cursor_key_app_mode` flag from the snapshot drives `DECCKM`-sensitive
/// key encoding (arrow keys, home, end).
fn send_terminal_input(
    input: &TerminalInput,
    input_tx: &Sender<InputEvent>,
    cursor_key_app_mode: bool,
) {
    let bytes = match input.to_payload(cursor_key_app_mode, cursor_key_app_mode) {
        TerminalInputPayload::Single(b) => vec![b],
        TerminalInputPayload::Many(bs) => bs.to_vec(),
        TerminalInputPayload::Owned(bs) => bs,
    };
    if bytes.is_empty() {
        return;
    }
    if let Err(e) = input_tx.send(InputEvent::Key(bytes)) {
        error!("Failed to send key input to PTY consumer: {e}");
    }
}

#[allow(
    clippy::cognitive_complexity,
    clippy::too_many_lines,
    clippy::too_many_arguments
)]
fn write_input_to_terminal(
    input: &InputState,
    snap: &TerminalSnapshot,
    input_tx: &Sender<InputEvent>,
    view_state: &mut ViewState,
    character_size_x: f32,
    character_size_y: f32,
    terminal_origin: Pos2,
    last_reported_mouse_pos: Option<PreviousMouseState>,
    repeat_characters: bool,
    previous_key: Option<Key>,
    scroll_amount: f32,
) -> (
    bool,
    Option<PreviousMouseState>,
    Option<Key>,
    f32,
    Option<String>,
) {
    if input.raw.events.is_empty() {
        return (
            false,
            last_reported_mouse_pos,
            previous_key,
            scroll_amount,
            None,
        );
    }

    let mut previous_key = previous_key;
    let mut state_changed = false;
    let mut last_reported_mouse_pos = last_reported_mouse_pos;
    let mut left_mouse_button_pressed = false;
    let mut scroll_amount = scroll_amount;
    let mut clipboard_text: Option<String> = None;

    // When the user is scrolled back into history, suppress mouse forwarding
    // to the PTY — the visible content is historical, not the live terminal
    // output the PTY application expects mouse coordinates to refer to.
    // Standard terminal emulator behavior (xterm, kitty, WezTerm, etc.).
    let effective_mouse_tracking = if view_state.scroll_offset > 0 {
        &MouseTrack::NoTracking
    } else {
        &snap.mouse_tracking
    };

    for event in &input.raw.events {
        debug!("event: {:?}", event);
        if let Event::Key { pressed: false, .. } = event {
            previous_key = None;
        }

        let inputs: Cow<'static, [TerminalInput]> = match event {
            // FIXME: We don't support separating out numpad vs regular keys
            // This is an egui issue. See: https://github.com/emilk/egui/issues/3653
            Event::Text(text) => {
                if repeat_characters || previous_key.is_none() {
                    collect_text(text)
                } else {
                    continue;
                }
            }
            Event::Key {
                key: Key::Enter,
                pressed: true,
                modifiers,
                ..
            } => {
                if modifiers.is_none() {
                    [TerminalInput::Enter].as_ref().into()
                } else {
                    continue;
                }
            }
            // https://github.com/emilk/egui/issues/3653
            // egui-winit intercepts Ctrl+C and Ctrl+X at the platform layer and converts them
            // to Event::Copy and Event::Cut respectively, before they can reach us as
            // Event::Key { key: Key::C/X, ctrl: true }.  We must handle both synthetic events
            // here so that terminal apps (e.g. nano ^C interrupt, ^X exit) receive the correct
            // C0 control bytes.
            //
            // Ctrl+Shift+C is the standard "copy selection" shortcut in terminal emulators.
            // egui-winit also converts it to Event::Copy (the shift is an extra modifier),
            // so we check input.modifiers.shift to distinguish:
            //   - Ctrl+C       (no shift) → send \x03 (SIGINT)
            //   - Ctrl+Shift+C (shift)    → copy selection to clipboard (Phase 3)
            // Same logic for Cut: Ctrl+X → \x18, Ctrl+Shift+X → no-op (can't cut from terminal).
            Event::Copy => {
                if input.modifiers.shift {
                    // Ctrl+Shift+C: copy selection text to clipboard.
                    // The actual copy_text() call is deferred until after the
                    // ui.input() closure returns, because copy_text() needs a
                    // write lock on the Context and we are inside a read lock.
                    if view_state.selection.has_selection() {
                        let text = extract_selected_text(
                            &snap.visible_chars,
                            snap.term_width,
                            &view_state.selection,
                        );
                        if !text.is_empty() {
                            clipboard_text = Some(text);
                        }
                    }
                    continue;
                }
                [TerminalInput::Ctrl(b'c')].as_ref().into()
            }
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
            ))]
            .into(),
            Event::Key {
                key: Key::ArrowDown,
                pressed: true,
                modifiers,
                ..
            } => vec![TerminalInput::ArrowDown(egui_mods_to_key_modifiers(
                *modifiers,
            ))]
            .into(),
            Event::Key {
                key: Key::ArrowLeft,
                pressed: true,
                modifiers,
                ..
            } => vec![TerminalInput::ArrowLeft(egui_mods_to_key_modifiers(
                *modifiers,
            ))]
            .into(),
            Event::Key {
                key: Key::ArrowRight,
                pressed: true,
                modifiers,
                ..
            } => vec![TerminalInput::ArrowRight(egui_mods_to_key_modifiers(
                *modifiers,
            ))]
            .into(),
            Event::Key {
                key: Key::Home,
                pressed: true,
                modifiers,
                ..
            } => vec![TerminalInput::Home(egui_mods_to_key_modifiers(*modifiers))].into(),
            Event::Key {
                key: Key::End,
                pressed: true,
                modifiers,
                ..
            } => vec![TerminalInput::End(egui_mods_to_key_modifiers(*modifiers))].into(),
            Event::Key {
                key: Key::Delete,
                pressed: true,
                modifiers,
                ..
            } => vec![TerminalInput::Delete(egui_mods_to_key_modifiers(
                *modifiers,
            ))]
            .into(),
            Event::Key {
                key: Key::Insert,
                pressed: true,
                modifiers,
                ..
            } => vec![TerminalInput::Insert(egui_mods_to_key_modifiers(
                *modifiers,
            ))]
            .into(),
            Event::Key {
                key: Key::PageUp,
                pressed: true,
                modifiers,
                ..
            } => vec![TerminalInput::PageUp(egui_mods_to_key_modifiers(
                *modifiers,
            ))]
            .into(),
            Event::Key {
                key: Key::PageDown,
                pressed: true,
                modifiers,
                ..
            } => vec![TerminalInput::PageDown(egui_mods_to_key_modifiers(
                *modifiers,
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
                egui_mods_to_key_modifiers(*modifiers),
            )]
            .into(),
            Event::Key {
                key: Key::F2,
                pressed: true,
                modifiers,
                ..
            } => vec![TerminalInput::FunctionKey(
                2,
                egui_mods_to_key_modifiers(*modifiers),
            )]
            .into(),
            Event::Key {
                key: Key::F3,
                pressed: true,
                modifiers,
                ..
            } => vec![TerminalInput::FunctionKey(
                3,
                egui_mods_to_key_modifiers(*modifiers),
            )]
            .into(),
            Event::Key {
                key: Key::F4,
                pressed: true,
                modifiers,
                ..
            } => vec![TerminalInput::FunctionKey(
                4,
                egui_mods_to_key_modifiers(*modifiers),
            )]
            .into(),
            Event::Key {
                key: Key::F5,
                pressed: true,
                modifiers,
                ..
            } => vec![TerminalInput::FunctionKey(
                5,
                egui_mods_to_key_modifiers(*modifiers),
            )]
            .into(),
            Event::Key {
                key: Key::F6,
                pressed: true,
                modifiers,
                ..
            } => vec![TerminalInput::FunctionKey(
                6,
                egui_mods_to_key_modifiers(*modifiers),
            )]
            .into(),
            Event::Key {
                key: Key::F7,
                pressed: true,
                modifiers,
                ..
            } => vec![TerminalInput::FunctionKey(
                7,
                egui_mods_to_key_modifiers(*modifiers),
            )]
            .into(),
            Event::Key {
                key: Key::F8,
                pressed: true,
                modifiers,
                ..
            } => vec![TerminalInput::FunctionKey(
                8,
                egui_mods_to_key_modifiers(*modifiers),
            )]
            .into(),
            Event::Key {
                key: Key::F9,
                pressed: true,
                modifiers,
                ..
            } => vec![TerminalInput::FunctionKey(
                9,
                egui_mods_to_key_modifiers(*modifiers),
            )]
            .into(),
            Event::Key {
                key: Key::F10,
                pressed: true,
                modifiers,
                ..
            } => vec![TerminalInput::FunctionKey(
                10,
                egui_mods_to_key_modifiers(*modifiers),
            )]
            .into(),
            Event::Key {
                key: Key::F11,
                pressed: true,
                modifiers,
                ..
            } => vec![TerminalInput::FunctionKey(
                11,
                egui_mods_to_key_modifiers(*modifiers),
            )]
            .into(),
            Event::Key {
                key: Key::F12,
                pressed: true,
                modifiers,
                ..
            } => vec![TerminalInput::FunctionKey(
                12,
                egui_mods_to_key_modifiers(*modifiers),
            )]
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
                    error!("Unexpected ctrl key: {}", key.name());
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
                if snap.bracketed_paste == RlBracket::Enabled {
                    // ESC [ 200 ~, followed by the pasted text, followed by ESC [ 201 ~.
                    collect_text(&format!("\x1b[200~{}{}", text, "\x1b[201~"))
                } else {
                    collect_text(text)
                }
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
                if let Err(e) = input_tx.send(InputEvent::FocusChange(*focused)) {
                    error!("Failed to send focus change event: {e}");
                }

                if !*focused {
                    view_state.mouse_position = None;
                    last_reported_mouse_pos = None;
                }

                continue;
            }
            Event::PointerMoved(pos) => {
                view_state.mouse_position = Some(*pos);
                let (x, y) = encode_egui_mouse_pos_as_usize(
                    *pos,
                    (character_size_x, character_size_y),
                    terminal_origin,
                );

                let position = FreminalMousePosition::new(x, y, pos.x, pos.y);
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

                let res = handle_pointer_moved(&current, &previous, effective_mouse_tracking);

                last_reported_mouse_pos = Some(current);

                if let Some(res) = res {
                    res
                } else {
                    // Mouse tracking is off — update text selection if a drag
                    // is in progress.
                    if view_state.selection.is_selecting {
                        view_state.selection.end = Some(CellCoord { col: x, row: y });
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
                state_changed = true;

                let (x, y) = encode_egui_mouse_pos_as_usize(
                    *pos,
                    (character_size_x, character_size_y),
                    terminal_origin,
                );
                let mouse_pos = FreminalMousePosition::new(x, y, pos.x, pos.y);
                let new_mouse_position =
                    PreviousMouseState::new(*button, *pressed, mouse_pos.clone(), *modifiers);
                let response =
                    handle_pointer_button(*button, &new_mouse_position, effective_mouse_tracking);

                last_reported_mouse_pos = Some(new_mouse_position.clone());

                if *button == PointerButton::Primary && *pressed {
                    left_mouse_button_pressed = true;
                }

                if let Some(response) = response {
                    response
                } else {
                    // Mouse tracking is off — handle text selection.
                    if *button == PointerButton::Primary {
                        if *pressed {
                            // Start a new selection at this cell.
                            let coord = CellCoord { col: x, row: y };
                            view_state.selection.anchor = Some(coord);
                            view_state.selection.end = Some(coord);
                            view_state.selection.is_selecting = true;
                        } else if view_state.selection.is_selecting {
                            // Mouse released — finalize the selection.
                            view_state.selection.end = Some(CellCoord { col: x, row: y });
                            view_state.selection.is_selecting = false;
                        }
                    }
                    continue;
                }
            }
            Event::MouseWheel {
                delta,
                modifiers,
                unit,
            } => {
                match unit {
                    egui::MouseWheelUnit::Point => {
                        scroll_amount += delta.y;
                    }
                    egui::MouseWheelUnit::Line => {
                        scroll_amount += delta.y * character_size_y;
                    }
                    egui::MouseWheelUnit::Page => {
                        error!("Unhandled MouseWheelUnit: {:?}", unit);
                        continue;
                    }
                }
                // TODO: should we care if we scrolled in the x axis?

                if scroll_amount.abs() < character_size_y {
                    continue;
                }

                // the amount scrolled should be in increments of the character size
                // the remaineder should be added to the next scroll event

                let scroll_amount_to_do = scroll_amount.floor();
                scroll_amount -= scroll_amount_to_do;

                state_changed = true;

                if let Some(last_mouse_position) = &mut last_reported_mouse_pos {
                    // update the modifiers if necessary
                    if last_mouse_position.modifiers != *modifiers {
                        last_mouse_position.modifiers = *modifiers;
                        *last_mouse_position = last_mouse_position.clone();
                    }
                    let response = handle_pointer_scroll(
                        egui::Vec2::new(0.0, scroll_amount_to_do / character_size_y),
                        last_mouse_position,
                        effective_mouse_tracking,
                    );

                    if let Some(response) = response {
                        response
                    } else {
                        // Mouse tracking is off — handle scroll ourselves.
                        handle_scroll_fallback(
                            scroll_amount_to_do,
                            character_size_y,
                            snap,
                            input_tx,
                            view_state,
                        );
                        continue;
                    }
                } else {
                    // No mouse position tracked — same fallback as above.
                    handle_scroll_fallback(
                        scroll_amount_to_do,
                        character_size_y,
                        snap,
                        input_tx,
                        view_state,
                    );
                    continue;
                }
            }
            _ => {
                continue;
            }
        };

        for input in inputs.as_ref() {
            state_changed = true;
            send_terminal_input(input, input_tx, snap.cursor_key_app_mode);
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
        clipboard_text,
    )
}

fn encode_egui_mouse_pos_as_usize(
    pos: Pos2,
    character_size: (f32, f32),
    origin: Pos2,
) -> (usize, usize) {
    // Subtract the terminal area origin so that coordinates are relative to
    // the top-left of the terminal grid, not the top-left of the window.
    let rel_x = (pos.x - origin.x).max(0.0);
    let rel_y = (pos.y - origin.y).max(0.0);

    let x = ((rel_x / character_size.0).floor())
        .approx_as::<usize>()
        .unwrap_or_else(|_| {
            if rel_x > 0.0 {
                debug!("Mouse x ({}) out of range, clamping to 255", rel_x);
                255
            } else {
                debug!("Mouse x ({}) out of range, clamping to 0", rel_x);
                0
            }
        });
    let y = ((rel_y / character_size.1).floor())
        .approx_as::<usize>()
        .unwrap_or_else(|_| {
            if rel_y > 0.0 {
                debug!("Mouse y ({}) out of range, clamping to 255", rel_y);
                255
            } else {
                debug!("Mouse y ({}) out of range, clamping to 0", rel_y);
                0
            }
        });

    (x, y)
}

/// Extract the text covered by the current selection from `visible_chars`.
///
/// `visible_chars` is a flat `Vec<TChar>` where rows are separated by
/// `TChar::NewLine`.  `term_width` is the terminal width in columns.
///
/// The selection is defined by the normalised `(start, end)` `CellCoord`s from
/// `SelectionState`.  For multi-line selections, trailing whitespace on each
/// line is trimmed and a newline is inserted between rows.
fn extract_selected_text(
    visible_chars: &[TChar],
    _term_width: usize,
    selection: &super::view_state::SelectionState,
) -> String {
    use std::fmt::Write as _;

    let Some((start, end)) = selection.normalised() else {
        return String::new();
    };

    // Split the flat visible_chars on NewLine boundaries to get per-row slices.
    // Each row in the flattened buffer has a *variable* number of TChars
    // (empty rows have 0, rows with wide chars have fewer than term_width, etc.),
    // so a fixed-stride approach does not work.
    let lines = split_visible_into_lines(visible_chars);

    let mut result = String::new();

    for row in start.row..=end.row {
        let line = if row < lines.len() {
            lines[row]
        } else {
            &[] // row beyond available data — treat as empty
        };

        // Column range for this row within the selection.
        let col_begin = if row == start.row { start.col } else { 0 };
        let col_end = if row == end.row {
            end.col
        } else {
            // Select to end of line content (not a fixed width).
            line.len().saturating_sub(1)
        };

        // Collect characters for this row's selected range.
        let mut row_text = String::new();
        for col in col_begin..=col_end {
            if col >= line.len() {
                break;
            }
            match &line[col] {
                TChar::NewLine => break,
                tc => {
                    write!(&mut row_text, "{tc}").unwrap_or_default();
                }
            }
        }

        // Trim trailing whitespace on each line (standard terminal behavior —
        // empty cells at the end of a line are spaces, not meaningful content).
        let trimmed = row_text.trim_end();
        result.push_str(trimmed);

        // Add newline between rows (but not after the last row).
        if row < end.row {
            result.push('\n');
        }
    }

    result
}

/// Split a flat `TChar` slice into per-line segments at `TChar::NewLine` boundaries.
///
/// The `NewLine` characters themselves are NOT included in the returned slices.
/// This mirrors `shaping::split_into_lines` but is kept local to avoid a
/// cross-module dependency for a trivial helper.
fn split_visible_into_lines(chars: &[TChar]) -> Vec<&[TChar]> {
    let mut lines = Vec::new();
    let mut start = 0;

    for (i, ch) in chars.iter().enumerate() {
        if matches!(ch, TChar::NewLine) {
            lines.push(&chars[start..i]);
            start = i + 1;
        }
    }

    // Trailing content after the last NewLine (or the entire array if no NewLine).
    if start <= chars.len() {
        lines.push(&chars[start..]);
    }

    lines
}
///
/// The scrollbar is only shown when the user is actively scrolled back
/// (`scroll_offset > 0`).  It disappears at the live bottom.
///
/// The indicator is purely visual — it does not handle drag input.
fn paint_scrollbar(scroll_offset: usize, max_scroll_offset: usize, ui: &Ui) {
    const SCROLLBAR_WIDTH: f32 = 6.0;
    const SCROLLBAR_MARGIN: f32 = 2.0;
    const MIN_THUMB_HEIGHT: f32 = 12.0;

    // Only show when scrolled back into history.
    if scroll_offset == 0 || max_scroll_offset == 0 {
        return;
    }

    let painter = ui.painter();

    // ── Dimensions ───────────────────────────────────────────────────────
    // Anchor to the full viewport rect, not the text content rect, so the
    // scrollbar stays pinned to the right edge regardless of content width.
    let viewport = ui.max_rect();
    let track_top = viewport.top();
    let track_bottom = viewport.bottom();
    let track_height = track_bottom - track_top;
    if track_height <= 0.0 {
        return;
    }

    let track_right = viewport.right() - SCROLLBAR_MARGIN;
    let track_left = track_right - SCROLLBAR_WIDTH;

    // ── Thumb geometry ───────────────────────────────────────────────────
    // The visible window covers `term_height` rows out of a total of
    // `max_scroll_offset + term_height`.  We don't have `term_height` here
    // but it cancels out: the thumb fraction in pixels equals
    //   track_height / (max_scroll_offset + term_height)  * term_height
    // which simplifies when we use the pixel track_height as the visible
    // proxy (they are proportional).
    //
    // Precision loss is acceptable — these are pixel coordinates.
    #[allow(clippy::cast_precision_loss)]
    let max_f = max_scroll_offset as f32;
    let total = max_f + track_height;
    let thumb_fraction = (track_height / total).clamp(0.05, 1.0);
    let thumb_height = (track_height * thumb_fraction)
        .max(MIN_THUMB_HEIGHT)
        .min(track_height);

    // Position: scroll_offset 0 = bottom, max = top.
    let scrollable_track = track_height - thumb_height;
    #[allow(clippy::cast_precision_loss)]
    let position_fraction = scroll_offset as f32 / max_f;
    let thumb_top = track_top + scrollable_track * (1.0 - position_fraction);

    let thumb_rect = Rect::from_min_max(
        Pos2::new(track_left, thumb_top),
        Pos2::new(track_right, thumb_top + thumb_height),
    );

    // ── Appearance ───────────────────────────────────────────────────────
    let color = Color32::from_rgba_premultiplied(200, 200, 200, 180);
    let rounding = SCROLLBAR_WIDTH / 2.0; // pill shape

    painter.rect_filled(thumb_rect, rounding, color);
}

/// GPU resources shared between the main thread (vertex building) and the
/// egui `PaintCallback` closure (draw calls).
///
/// Wrapped in `Arc<Mutex<…>>` so that the pre-built vertex data can be
/// written on the main thread and consumed inside the `PaintCallback`,
/// which requires `Send + Sync + 'static` captures.
struct RenderState {
    renderer: TerminalRenderer,
    atlas: GlyphAtlas,
    bg_verts: Vec<f32>,
    fg_verts: Vec<f32>,
    /// Float offset (not byte offset) into `bg_verts` where the cursor quad
    /// data begins.  Set after every full vertex rebuild so cursor-only frames
    /// can patch just this region.
    cursor_vert_float_offset: usize,
}

pub struct FreminalTerminalWidget {
    font_manager: FontManager,
    shaping_cache: ShapingCache,
    render_state: Arc<Mutex<RenderState>>,
    previous_mouse_state: Option<PreviousMouseState>,
    previous_key: Option<Key>,
    previous_scroll_amount: f32,
    /// Cursor blink state from the most recently rendered frame.
    previous_cursor_blink_on: bool,
    /// Cursor position from the most recently rendered frame.
    previous_cursor_pos: freminal_common::buffer_states::cursor::CursorPos,
    /// Whether the cursor was shown in the most recently rendered frame.
    previous_show_cursor: bool,
    /// The `visible_chars` arc from the last full vertex rebuild.
    ///
    /// Used to detect content changes via `Arc::ptr_eq` — immune to the race
    /// where a later snapshot overwrites `content_changed` before the GUI wakes.
    last_rendered_visible: Option<Arc<Vec<TChar>>>,
    /// The normalised selection from the last full vertex rebuild, used to
    /// detect selection changes that require a full rebuild.
    previous_selection: Option<(CellCoord, CellCoord)>,
}

impl FreminalTerminalWidget {
    #[must_use]
    pub fn new(ctx: &Context, config: &Config) -> Self {
        let font_config = FontConfig {
            size: config.font.size,
            user_font: config.font.family.clone(),
            ..FontConfig::default()
        };
        setup_font_files(ctx, &font_config);

        Self {
            font_manager: FontManager::new(config),
            shaping_cache: ShapingCache::new(),
            render_state: Arc::new(Mutex::new(RenderState {
                renderer: TerminalRenderer::new(),
                atlas: GlyphAtlas::default(),
                bg_verts: Vec::new(),
                fg_verts: Vec::new(),
                cursor_vert_float_offset: 0,
            })),
            previous_mouse_state: None,
            previous_key: None,
            previous_scroll_amount: 0.0,
            previous_cursor_blink_on: true,
            previous_cursor_pos: freminal_common::buffer_states::cursor::CursorPos::default(),
            previous_show_cursor: false,
            last_rendered_visible: None,
            previous_selection: None,
        }
    }

    /// Returns the authoritative cell size in integer pixels `(width, height)`.
    ///
    /// Computed once from swash font metrics and updated on font change.
    #[must_use]
    pub const fn cell_size(&self) -> (u32, u32) {
        self.font_manager.cell_size()
    }

    #[allow(clippy::too_many_lines)]
    pub fn show(
        &mut self,
        ui: &mut Ui,
        snap: &TerminalSnapshot,
        view_state: &mut ViewState,
        input_tx: &Sender<InputEvent>,
        _pty_write_tx: &Sender<PtyWrite>,
        modal_is_open: bool,
    ) {
        const BLINK_TICK_SECONDS: f64 = 0.50;
        let (cell_w, cell_h) = self.font_manager.cell_size();
        #[allow(clippy::cast_precision_loss)]
        let cell_w_f = cell_w as f32;
        #[allow(clippy::cast_precision_loss)]
        let row_h_f = cell_h as f32;

        // Claim the full available space.
        let available = ui.available_size();
        ui.set_min_size(available);

        // Compute the terminal area origin BEFORE processing input events.
        // Pointer events from `input.raw.events` are in window coordinates,
        // so `encode_egui_mouse_pos_as_usize` must subtract this origin to
        // get terminal-grid-relative coordinates.
        let terminal_origin = ui.available_rect_before_wrap().min;

        // When a modal dialog (e.g. the settings window) is open, do NOT
        // forward keyboard/mouse events to the PTY — they belong to the
        // modal's egui widgets instead.
        if !modal_is_open {
            let repeat_characters = snap.repeat_keys;
            let ctx = ui.ctx().clone();
            let (
                _left_mouse_button_pressed,
                new_mouse_pos,
                previous_key,
                scroll_amount,
                clipboard_text,
            ) = ui.input(|input_state| {
                write_input_to_terminal(
                    input_state,
                    snap,
                    input_tx,
                    view_state,
                    cell_w_f,
                    row_h_f,
                    terminal_origin,
                    self.previous_mouse_state.clone(),
                    repeat_characters,
                    self.previous_key,
                    self.previous_scroll_amount,
                )
            });
            self.previous_mouse_state = new_mouse_pos;
            self.previous_key = previous_key;
            self.previous_scroll_amount = scroll_amount;

            // Perform the clipboard copy OUTSIDE the ui.input() closure.
            // copy_text() calls ctx.output_mut() which needs a write lock on
            // the Context, but ui.input() holds a read lock — calling
            // copy_text() inside the closure would deadlock.
            if let Some(text) = clipboard_text {
                ctx.copy_text(text);
                // Clear the selection highlight now that the text has been
                // copied to the clipboard.
                view_state.selection.clear();
            }
        }

        // Blink state must be computed here — cannot call `ui.input` inside
        // the `Arc<CallbackFn>` closure (it must be `Send + Sync`).
        let time = ui.input(|i| i.time);
        let cursor_blink_on = match <i64 as ApproxFrom<f64, RoundToZero>>::approx_from(
            (time / BLINK_TICK_SECONDS).floor(),
        ) {
            Ok(ticks) => ticks % 2 == 0,
            Err(e) => {
                error!("Failed to convert blink ticks to i64: {e}");
                true
            }
        };

        if !snap.skip_draw {
            // Detect content changes via `Arc::ptr_eq` — this is immune to the
            // race where the PTY thread overwrites a "changed" snapshot with a
            // "clean" one before the GUI wakes up.  If the `visible_chars` arc
            // is a different allocation from the one we last rendered, the
            // content has changed regardless of the `content_changed` flag.
            let content_changed = snap.content_changed
                || self
                    .last_rendered_visible
                    .as_ref()
                    .is_none_or(|prev| !Arc::ptr_eq(prev, &snap.visible_chars));

            // Clear the selection when actual terminal text content changes so
            // stale highlights don't linger over shifted text.  We use
            // `snap.content_changed` here (NOT the `Arc::ptr_eq`-augmented
            // `content_changed`) because the PTY thread may re-flatten and
            // allocate a new Arc for cursor-blink dirty rows even when the
            // visible text is byte-identical.  Using the broader check would
            // clear the selection within ~500 ms of mouse release (on every
            // cursor blink), making copy impossible.
            if snap.content_changed && !view_state.selection.is_selecting {
                view_state.selection.clear();
            }

            // Check whether the selection has changed since the last frame.
            let current_selection = view_state.selection.normalised();
            let selection_changed = current_selection != self.previous_selection;

            // Determine whether we can take the cursor-only fast path.
            //
            // Cursor-only: content has not changed, the selection has not
            // changed, but the cursor blink state or position has changed
            // since the last frame.  We only need to patch the cursor quad
            // in the background VBO — no re-shaping and no full vertex
            // rebuild required.
            let cursor_state_changed = cursor_blink_on != self.previous_cursor_blink_on
                || snap.cursor_pos != self.previous_cursor_pos
                || snap.show_cursor != self.previous_show_cursor;

            let cursor_only = !content_changed
                && !selection_changed
                && cursor_state_changed
                && !self
                    .render_state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .bg_verts
                    .is_empty();

            if cursor_only {
                // Fast path: build just the cursor quad and stash it.
                let cursor_verts = build_cursor_verts_only(
                    cell_w,
                    cell_h,
                    snap.show_cursor,
                    cursor_blink_on,
                    snap.cursor_pos,
                    &snap.cursor_visual_style,
                );
                let mut rs = self
                    .render_state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                // Patch the cursor region in bg_verts so the PaintCallback can
                // detect the cursor-only mode via a separate flag.
                // We overwrite the cursor quad data in the CPU copy so that if
                // a full rebuild happens next frame it starts from correct state.
                let cfo = rs.cursor_vert_float_offset;
                if cursor_verts.is_empty() {
                    // Hide cursor: zero out the region.
                    if cfo + CURSOR_QUAD_FLOATS <= rs.bg_verts.len() {
                        for f in &mut rs.bg_verts[cfo..cfo + CURSOR_QUAD_FLOATS] {
                            *f = 0.0;
                        }
                    }
                } else if cfo + CURSOR_QUAD_FLOATS <= rs.bg_verts.len()
                    && cursor_verts.len() == CURSOR_QUAD_FLOATS
                {
                    rs.bg_verts[cfo..cfo + CURSOR_QUAD_FLOATS].copy_from_slice(&cursor_verts);
                }
            } else if content_changed
                || selection_changed
                || self
                    .render_state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .bg_verts
                    .is_empty()
            {
                // Full rebuild path.
                let shaped_lines = self.shaping_cache.shape_visible(
                    &snap.visible_chars,
                    &snap.visible_tags,
                    snap.term_width,
                    &mut self.font_manager,
                    cell_w_f,
                );

                let bg_verts = build_background_verts(
                    &shaped_lines,
                    cell_w,
                    cell_h,
                    self.font_manager.underline_offset(),
                    self.font_manager.strikeout_offset(),
                    self.font_manager.stroke_size(),
                    snap.show_cursor,
                    cursor_blink_on,
                    snap.cursor_pos,
                    &snap.cursor_visual_style,
                    current_selection.map(|(s, e)| (s.col, s.row, e.col, e.row)),
                );

                // Record where the cursor quad starts in the background VBO.
                // The cursor is always appended at the END of bg_verts, and is
                // exactly CURSOR_QUAD_FLOATS floats (or absent when hidden).
                let cursor_vert_float_offset = if snap.show_cursor {
                    bg_verts.len().saturating_sub(CURSOR_QUAD_FLOATS)
                } else {
                    bg_verts.len()
                };

                // `build_foreground_verts` needs mutable access to the atlas for
                // rasterisation, so acquire the lock before calling it.
                let mut rs = self
                    .render_state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let fg_verts = build_foreground_verts(
                    &shaped_lines,
                    &mut rs.atlas,
                    &self.font_manager,
                    cell_h,
                    self.font_manager.ascent(),
                    current_selection.map(|(s, e)| (s.col, s.row, e.col, e.row)),
                );
                rs.bg_verts = bg_verts;
                rs.fg_verts = fg_verts;
                rs.cursor_vert_float_offset = cursor_vert_float_offset;
                drop(rs);

                // Remember which `visible_chars` allocation we rendered, so
                // the next frame can detect changes via `Arc::ptr_eq`.
                self.last_rendered_visible = Some(Arc::clone(&snap.visible_chars));
                self.previous_selection = current_selection;
            }
            // If neither path applies (content unchanged, cursor unchanged,
            // selection unchanged, buffers not empty) we simply re-draw the
            // existing VBO data — no CPU work at all.
        }

        // Update per-frame cursor state for the next frame's comparison.
        self.previous_cursor_blink_on = cursor_blink_on;
        self.previous_cursor_pos = snap.cursor_pos;
        self.previous_show_cursor = snap.show_cursor;

        // Allocate the exact terminal rect.
        #[allow(clippy::cast_precision_loss)]
        let desired_size = egui::Vec2::new(
            snap.term_width as f32 * cell_w_f,
            snap.height as f32 * row_h_f,
        );
        let (rect, _response) = ui.allocate_exact_size(desired_size, egui::Sense::hover());

        // Hand off the draw call to egui's paint phase via PaintCallback.
        // The closure must be `Send + Sync + 'static`, so only `Arc<Mutex<…>>`
        // data (not `FontManager`) may be captured here.
        let render_state = Arc::clone(&self.render_state);
        // The MutexGuard inside the callback intentionally lives through
        // `draw_with_verts` because the renderer and atlas are refs into it.
        #[allow(clippy::significant_drop_tightening)]
        ui.painter().add(egui::PaintCallback {
            rect,
            callback: Arc::new(CallbackFn::new(move |info, painter| {
                let gl = painter.gl();
                let vp = info.viewport_in_pixels();
                let mut rs = render_state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if !rs.renderer.initialized()
                    && let Err(e) = rs.renderer.init(gl)
                {
                    error!("GL init failed: {e}");
                    return;
                }
                // Clone pre-built verts to avoid conflicting borrows of `rs`.
                let bg = rs.bg_verts.clone();
                let fg = rs.fg_verts.clone();
                // Split borrow: get &mut RenderState so the borrow checker sees
                // renderer and atlas as disjoint fields.
                let rs_ref: &mut RenderState = &mut rs;
                let renderer = &mut rs_ref.renderer;
                let atlas = &mut rs_ref.atlas;
                renderer.draw_with_verts(
                    gl,
                    atlas,
                    &bg,
                    &fg,
                    vp.width_px,
                    vp.height_px,
                    painter.intermediate_fbo(),
                );
            })),
        });

        paint_scrollbar(snap.scroll_offset, snap.max_scroll_offset, ui);

        // URL hover detection is not yet ported to snapshots.
        // TODO(task-9): implement URL hover via snapshot data.
        if let Some(mouse_position) = view_state.mouse_position {
            let _ = mouse_position; // suppress unused-variable lint
            debug!("No URL hover detection in snapshot mode yet");
            ui.ctx().output_mut(|output| {
                output.cursor_icon = CursorIcon::Default;
            });
        } else {
            debug!("No mouse position");
            ui.ctx().output_mut(|output| {
                output.cursor_icon = CursorIcon::Default;
            });
        }
    }

    /// Apply config changes that can be hot-reloaded at runtime.
    ///
    /// Called when the user clicks "Apply" in the settings modal. Compares the
    /// old and new configs and updates font/cursor/theme state as needed.
    pub fn apply_config_changes(
        &mut self,
        ctx: &egui::Context,
        old_config: &Config,
        new_config: &Config,
        input_tx: &Sender<InputEvent>,
    ) {
        let rebuild_result = self.font_manager.rebuild(new_config);
        if rebuild_result.font_changed() {
            let mut rs = self
                .render_state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            rs.atlas.clear();
            drop(rs);
            self.shaping_cache.clear();
        }

        // Keep egui font infrastructure updated for chrome (menu bar, settings
        // modal).  This is retained from the old pipeline; it will be cleaned
        // up in subtask 1.9 once chrome fonts are fully migrated.
        let font_changed = old_config.font.family != new_config.font.family
            || (old_config.font.size - new_config.font.size).abs() > f32::EPSILON;
        if font_changed {
            let new_font_config = FontConfig {
                size: new_config.font.size,
                user_font: new_config.font.family.clone(),
                ..FontConfig::default()
            };
            setup_font_files(ctx, &new_font_config);
        }

        // When the font changes, cell size may change too.  If the cell size
        // differs from the last-known value, send a Resize event so the PTY
        // thread can reflow the buffer to the new column/row count.
        //
        // We pass zero pixel dimensions here because the exact pixel size will
        // be re-computed from the new cell metrics on the next frame.  The PTY
        // thread ignores pixel dimensions when the char dimensions are non-zero.
        if rebuild_result.font_changed() {
            // Cell size has been updated by `font_manager.rebuild()`; read the
            // freshly computed dimensions.
            let (new_cell_w, new_cell_h) = self.font_manager.cell_size();
            #[allow(clippy::cast_possible_truncation)]
            let _ = input_tx.send(freminal_terminal_emulator::io::InputEvent::Resize(
                0,
                0,
                new_cell_w as usize,
                new_cell_h as usize,
            ));
        }
    }
}

#[cfg(test)]
mod subtask_1_7_tests {
    use super::*;

    /// Verify that an empty `RenderState` has empty vertex buffers.
    ///
    /// This confirms that `skip_draw` leaves the existing (initially empty)
    /// vertex buffers untouched rather than calling the vertex-build path.
    #[test]
    fn skip_draw_leaves_verts_empty() {
        let rs = RenderState {
            renderer: TerminalRenderer::new(),
            atlas: GlyphAtlas::default(),
            bg_verts: Vec::new(),
            fg_verts: Vec::new(),
            cursor_vert_float_offset: 0,
        };
        assert!(rs.bg_verts.is_empty(), "bg_verts should be empty");
        assert!(rs.fg_verts.is_empty(), "fg_verts should be empty");
    }

    /// Verify that `FontManager::cell_size()` returns non-zero dimensions for
    /// the default config (bundled `MesloLGS` Nerd Font Mono).
    #[test]
    fn cell_size_from_font_manager_is_nonzero() {
        let config = freminal_common::config::Config::default();
        let fm = FontManager::new(&config);
        let (w, h) = fm.cell_size();
        assert!(w > 0, "cell_width must be non-zero, got {w}");
        assert!(h > 0, "cell_height must be non-zero, got {h}");
    }
}
