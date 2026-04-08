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
    view_state::{CellCoord, ViewState},
};

use conv2::ConvUtil;
use crossbeam_channel::Sender;
use eframe::egui::{Event, InputState, Key, Modifiers, PointerButton, Rect};
use freminal_common::buffer_states::modes::{
    application_escape_key::ApplicationEscapeKey, decarm::Decarm, decbkm::Decbkm, decckm::Decckm,
    keypad::KeypadMode, lnm::Lnm, mouse::MouseTrack, rl_bracket::RlBracket,
};
use freminal_common::keybindings::{BindingKey, BindingMap, BindingModifiers, KeyAction, KeyCombo};
use freminal_terminal_emulator::{
    input::{KeyModifiers, TerminalInput, TerminalInputPayload, collect_text},
    io::InputEvent,
    snapshot::TerminalSnapshot,
};
use std::borrow::Cow;

use super::coords::{encode_egui_mouse_pos_as_usize, visible_window_start};

/// Convert egui [`Modifiers`] to the terminal-emulator's [`KeyModifiers`].
///
/// This is used for special keys (arrows, function keys, Home/End, etc.)
/// where the xterm modifier encoding (`ESC[1;Nm…`) applies. It must NOT
/// be used for regular ASCII keys where Ctrl already produces a C0 control
/// code — that path is handled by `control_key()` / `TerminalInput::Ctrl`.
pub(super) const fn egui_mods_to_key_modifiers(m: Modifiers) -> KeyModifiers {
    KeyModifiers {
        shift: m.shift,
        ctrl: m.ctrl || m.command,
        alt: m.alt,
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
pub(super) fn dispatch_binding_action(
    action: KeyAction,
    view_state: &mut ViewState,
    input_tx: &Sender<InputEvent>,
    snap: &TerminalSnapshot,
    clipboard_pending: &mut bool,
    deferred_actions: &mut Vec<KeyAction>,
) {
    match action {
        KeyAction::Copy => {
            if let Some((start, end)) = view_state.selection.normalised() {
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
        }
        KeyAction::ScrollPageUp => {
            let new_offset = view_state
                .scroll_offset
                .saturating_add(snap.term_height)
                .min(snap.max_scroll_offset);
            if new_offset != view_state.scroll_offset {
                view_state.scroll_offset = new_offset;
                if let Err(e) = input_tx.send(InputEvent::ScrollOffset(new_offset)) {
                    error!("Failed to send scroll offset to PTY consumer: {e}");
                }
            }
        }
        KeyAction::ScrollPageDown => {
            let new_offset = view_state.scroll_offset.saturating_sub(snap.term_height);
            if new_offset != view_state.scroll_offset {
                view_state.scroll_offset = new_offset;
                if let Err(e) = input_tx.send(InputEvent::ScrollOffset(new_offset)) {
                    error!("Failed to send scroll offset to PTY consumer: {e}");
                }
            }
        }
        KeyAction::ScrollToTop => {
            let new_offset = snap.max_scroll_offset;
            if new_offset != view_state.scroll_offset {
                view_state.scroll_offset = new_offset;
                if let Err(e) = input_tx.send(InputEvent::ScrollOffset(new_offset)) {
                    error!("Failed to send scroll offset to PTY consumer: {e}");
                }
            }
        }
        KeyAction::ScrollToBottom => {
            if view_state.scroll_offset != 0 {
                view_state.scroll_offset = 0;
                if let Err(e) = input_tx.send(InputEvent::ScrollOffset(0)) {
                    error!("Failed to send scroll offset to PTY consumer: {e}");
                }
            }
        }
        KeyAction::ScrollLineUp => {
            let new_offset = view_state
                .scroll_offset
                .saturating_add(1)
                .min(snap.max_scroll_offset);
            if new_offset != view_state.scroll_offset {
                view_state.scroll_offset = new_offset;
                if let Err(e) = input_tx.send(InputEvent::ScrollOffset(new_offset)) {
                    error!("Failed to send scroll offset to PTY consumer: {e}");
                }
            }
        }
        KeyAction::ScrollLineDown => {
            let new_offset = view_state.scroll_offset.saturating_sub(1);
            if new_offset != view_state.scroll_offset {
                view_state.scroll_offset = new_offset;
                if let Err(e) = input_tx.send(InputEvent::ScrollOffset(new_offset)) {
                    error!("Failed to send scroll offset to PTY consumer: {e}");
                }
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
/// ## Mode parameters
///
/// The `modes` parameter bundles all terminal mode flags that affect input
/// encoding. See [`InputModes`] for the individual fields.
pub(super) fn send_terminal_inputs(
    inputs: &[TerminalInput],
    input_tx: &Sender<InputEvent>,
    modes: &InputModes,
) {
    let bytes: Vec<u8> = inputs
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
            ) {
                TerminalInputPayload::Single(b) => vec![b],
                TerminalInputPayload::Many(bs) => bs.to_vec(),
                TerminalInputPayload::Owned(bs) => bs,
            }
        })
        .collect();
    if bytes.is_empty() {
        return;
    }
    if let Err(e) = input_tx.send(InputEvent::Key(bytes)) {
        error!("Failed to send key input to PTY consumer: {e}");
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
            );
        }
    } else {
        // Primary screen: adjust scroll offset and send to PTY thread.
        // Multiply by 3 so each wheel tick scrolls 3 lines — matching the
        // default behavior of most terminal emulators (iTerm2, Alacritty,
        // kitty, GNOME Terminal, etc.).
        const SCROLL_MULTIPLIER: usize = 3;
        let n = abs_lines.approx_as::<usize>().unwrap_or(0).max(1) * SCROLL_MULTIPLIER;

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
    y: usize,
    abs_row: usize,
) -> usize {
    if view_state.click_count >= 3 {
        let anchor_row = view_state.selection.anchor.map_or(abs_row, |a| a.row);
        let (line_start, line_end) =
            crate::gui::view_state::line_boundaries(&snap.visible_chars, y);
        if abs_row >= anchor_row {
            line_end
        } else {
            line_start
        }
    } else if view_state.click_count == 2 {
        let anchor_row = view_state.selection.anchor.map_or(abs_row, |a| a.row);
        let anchor_col = view_state.selection.anchor.map_or(x, |a| a.col);
        let (word_start, word_end) =
            crate::gui::view_state::word_boundaries(&snap.visible_chars, y, x);
        if abs_row > anchor_row || (abs_row == anchor_row && word_end >= anchor_col) {
            word_end
        } else {
            word_start
        }
    } else {
        x
    }
}

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
/// Returns `(state_changed, last_reported_mouse_pos, previous_key, scroll_amount, clipboard_pending)`:
/// - `state_changed` — true if the view state was mutated (scroll, selection) and a repaint is needed.
/// - `last_reported_mouse_pos` — updated mouse tracking state for the next call.
/// - `previous_key` — last pressed key (used for key-repeat deduplication).
/// - `scroll_amount` — accumulated fractional scroll pixels not yet converted to full line units.
/// - `clipboard_pending` — true if a selection-copy was queued; the caller reads the clipboard channel.
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
) -> (
    bool,
    Option<PreviousMouseState>,
    Option<Key>,
    f32,
    bool,
    Vec<KeyAction>,
) {
    if input.raw.events.is_empty() {
        return (
            false,
            last_reported_mouse_pos,
            previous_key,
            scroll_amount,
            false,
            Vec::new(),
        );
    }

    let mut previous_key = previous_key;
    let mut state_changed = false;
    let mut last_reported_mouse_pos = last_reported_mouse_pos;
    let mut left_mouse_button_pressed = false;
    let mut scroll_amount = scroll_amount;
    let mut clipboard_pending = false;
    let mut deferred_actions: Vec<KeyAction> = Vec::new();

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
        debug!("event: {:?}", event);
        if let Event::Key { pressed: false, .. } = event {
            previous_key = None;
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

        let inputs: Cow<'static, [TerminalInput]> = match event {
            // LIMITATION (egui#3653): egui unifies numpad and main-row keys.
            // Application keypad mode cannot distinguish them until egui exposes
            // separate key variants.
            Event::Text(text) => {
                if repeat_characters == Decarm::RepeatKey || previous_key.is_none() {
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

                let res = handle_pointer_moved(
                    &current,
                    &previous,
                    effective_mouse_tracking,
                    mouse_encoding,
                );

                last_reported_mouse_pos = Some(current);

                if let Some(res) = res {
                    res
                } else {
                    // Mouse tracking is off — update text selection if a drag
                    // is in progress.
                    if view_state.selection.is_selecting {
                        let abs_row = visible_window_start(snap) + y;
                        let end_col = if view_state.click_count >= 3 {
                            // Triple-click drag — snap end to line boundaries.
                            let anchor_row = view_state.selection.anchor.map_or(abs_row, |a| a.row);
                            let (line_start, line_end) =
                                crate::gui::view_state::line_boundaries(&snap.visible_chars, y);
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
                let mouse_pos = FreminalMousePosition::new(x, y, pos.x, pos.y);
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

                if *button == PointerButton::Primary && *pressed {
                    left_mouse_button_pressed = true;
                }

                if let Some(response) = response {
                    response
                } else {
                    // Mouse tracking is off (or overridden by Shift) — handle
                    // text selection and right-click context menu.
                    if *button == PointerButton::Secondary && *pressed {
                        // Record the right-clicked cell so the widget layer
                        // can open the context menu and detect URLs.
                        let abs_row = visible_window_start(snap) + y;
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
                            let abs_row = visible_window_start(snap) + y;
                            let coord = CellCoord {
                                col: x,
                                row: abs_row,
                            };
                            let click_count =
                                view_state.register_click(coord, std::time::Instant::now());

                            if click_count >= 3 {
                                // Triple-click — select the entire visual line.
                                let (start_col, end_col) =
                                    crate::gui::view_state::line_boundaries(&snap.visible_chars, y);
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
                                    y,
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
                            let abs_row = visible_window_start(snap) + y;
                            let end_col = release_end_col(view_state, snap, x, y, abs_row);
                            let end_coord = CellCoord {
                                col: end_col,
                                row: abs_row,
                            };
                            view_state.selection.end = Some(end_coord);
                            view_state.selection.is_selecting = false;

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
                    eframe::egui::MouseWheelUnit::Point => {
                        scroll_amount += delta.y;
                    }
                    eframe::egui::MouseWheelUnit::Line => {
                        scroll_amount += delta.y * character_size_y;
                    }
                    eframe::egui::MouseWheelUnit::Page => {
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
                    let position = FreminalMousePosition::new(x, y, hover.x, hover.y);
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
                    let unit_delta = eframe::egui::Vec2::new(0.0, direction);

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
            send_terminal_inputs(inputs.as_ref(), input_tx, &InputModes::from_snapshot(snap));
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
    )
}
