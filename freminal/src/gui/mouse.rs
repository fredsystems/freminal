// Copyright (C) 2024-2025 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
#![allow(clippy::module_name_repetitions)]
use std::borrow::Cow;

use conv2::ConvUtil;
use eframe::egui::{Modifiers, PointerButton, Vec2};
use freminal_terminal_emulator::{
    ansi_components::modes::mouse::{MouseEncoding, MouseTrack},
    interface::{TerminalInput, collect_text, raw_ascii_bytes_to_terminal_input},
};

#[derive(Debug, PartialEq, Clone)]
pub struct PreviousMouseState {
    pub(crate) button: PointerButton,
    pub(crate) button_pressed: bool,
    pub(crate) mouse_position: FreminalMousePosition,
    pub(crate) modifiers: Modifiers,
}

impl Default for PreviousMouseState {
    fn default() -> Self {
        Self {
            button: PointerButton::Primary,
            button_pressed: false,
            mouse_position: FreminalMousePosition::new(0, 0, 0.0, 0.0),
            modifiers: Modifiers::default(),
        }
    }
}

impl PreviousMouseState {
    #[must_use]
    pub const fn new_from_previous_mouse_state(&self, position: FreminalMousePosition) -> Self {
        Self {
            button: self.button,
            button_pressed: self.button_pressed,
            mouse_position: position,
            modifiers: self.modifiers,
        }
    }

    #[must_use]
    pub const fn new(
        button: PointerButton,
        button_pressed: bool,
        mouse_position: FreminalMousePosition,
        modifiers: Modifiers,
    ) -> Self {
        Self {
            button,
            button_pressed,
            mouse_position,
            modifiers,
        }
    }

    #[must_use]
    pub fn should_report(&self, new: &Self) -> bool {
        self.mouse_position != new.mouse_position
    }
}

pub enum MouseEvent {
    Button(PointerButton),
    Scroll(Vec2),
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct FreminalMousePosition {
    pub(crate) x_as_character_column: usize,
    pub(crate) y_as_character_row: usize,
    pub(crate) x: f32,
    pub(crate) y: f32,
}

impl FreminalMousePosition {
    #[must_use]
    pub const fn new(
        x_as_character_column: usize,
        y_as_character_row: usize,
        x: f32,
        y: f32,
    ) -> Self {
        Self {
            x_as_character_column,
            y_as_character_row,
            x,
            y,
        }
    }
}

impl PartialEq for FreminalMousePosition {
    fn eq(&self, other: &Self) -> bool {
        self.y_as_character_row == other.y_as_character_row
    }
}

#[must_use]
pub fn handle_pointer_button(
    button: PointerButton,
    current_state: &PreviousMouseState,
    mouse_track: &MouseTrack,
) -> Option<Cow<'static, [TerminalInput]>> {
    match mouse_track {
        MouseTrack::XtMsex10 => {
            if current_state.button_pressed {
                return Some(encode_x11_mouse_button(
                    button,
                    true,
                    current_state.modifiers,
                    &current_state.mouse_position,
                    false,
                    &mouse_track.get_encoding(),
                ));
            }
            None
        }
        MouseTrack::XtMseX11
        | MouseTrack::XtMseBtn
        | MouseTrack::XtMseAny
        | MouseTrack::XtMseSgr => Some(encode_x11_mouse_button(
            button,
            current_state.button_pressed,
            current_state.modifiers,
            &current_state.mouse_position,
            false,
            &mouse_track.get_encoding(),
        )),
        MouseTrack::NoTracking
        | MouseTrack::XtMseUtf
        | MouseTrack::XtMseUrXvt
        | MouseTrack::XtMseSgrPixels
        | MouseTrack::Query(_) => None,
    }
}

#[must_use]
pub fn handle_pointer_moved(
    current_state: &PreviousMouseState,
    previous_state: &PreviousMouseState,
    mouse_track: &MouseTrack,
) -> Option<Cow<'static, [TerminalInput]>> {
    match mouse_track {
        MouseTrack::XtMseBtn => {
            if current_state.button_pressed && previous_state.should_report(current_state) {
                return Some(encode_x11_mouse_button(
                    current_state.button,
                    true,
                    current_state.modifiers,
                    &current_state.mouse_position,
                    true,
                    &mouse_track.get_encoding(),
                ));
            }

            None
        }
        MouseTrack::XtMseAny | MouseTrack::XtMseSgr => {
            if previous_state.should_report(current_state) {
                return Some(encode_x11_mouse_button(
                    current_state.button,
                    current_state.button_pressed,
                    current_state.modifiers,
                    &current_state.mouse_position,
                    true,
                    &mouse_track.get_encoding(),
                ));
            }

            None
        }
        MouseTrack::NoTracking
        | MouseTrack::XtMsex10
        | MouseTrack::XtMseX11
        | MouseTrack::XtMseUtf
        | MouseTrack::XtMseUrXvt
        | MouseTrack::XtMseSgrPixels
        | MouseTrack::Query(_) => None,
    }
}

#[must_use]
pub fn handle_pointer_scroll(
    delta: Vec2,
    current_state: &PreviousMouseState,
    mouse_track: &MouseTrack,
) -> Option<Cow<'static, [TerminalInput]>> {
    match mouse_track {
        MouseTrack::XtMseX11
        | MouseTrack::XtMseBtn
        | MouseTrack::XtMseAny
        | MouseTrack::XtMseSgr => encode_x11_mouse_wheel(
            delta,
            current_state.modifiers,
            &current_state.mouse_position,
            &mouse_track.get_encoding(),
        ),
        MouseTrack::NoTracking
        | MouseTrack::XtMsex10
        | MouseTrack::XtMseUtf
        | MouseTrack::XtMseUrXvt
        | MouseTrack::XtMseSgrPixels
        | MouseTrack::Query(_) => None,
    }
}

fn encode_mouse_for_x11(button: &MouseEvent, pressed: bool) -> usize {
    if pressed {
        match button {
            MouseEvent::Button(PointerButton::Primary) => 0,
            MouseEvent::Button(PointerButton::Middle) => 1,
            MouseEvent::Button(PointerButton::Secondary) => 2,
            MouseEvent::Button(_) => {
                error!("Unsupported mouse button. Treating as left mouse button");
                0
            }
            MouseEvent::Scroll(amount) => {
                // FIXME: This is not correct. eframe encodes a x and y event together I think.
                // For now we'll prefer the y event as the driver for the scroll
                // If that is the case should we be sending a two different events for scroll?

                if amount.y != 0.0 {
                    if amount.y > 0.0 {
                        return 64;
                    }
                    return 65;
                }

                0
            }
        }
    } else {
        3
    }
}

const fn encode_modifiers_for_x11(modifiers: Modifiers) -> usize {
    let mut cb = 0;

    if modifiers.ctrl || modifiers.command {
        cb += 16;
    }

    if modifiers.shift {
        cb += 4;
    }

    // This is for meta, but wezterm seems to use alt as the meta?
    if modifiers.alt {
        cb += 8;
    }

    cb
}

fn encode_cb_and_x_and_y_as_u8_from_usize(cb: usize, x: usize, y: usize) -> (u8, u8, u8) {
    if x > 0x100 {
        error!("X: {x} is out of range");
    }
    if y > 0x100 {
        error!("Y: {y} is out of range");
    }

    let cb = cb.approx_as::<u8>().unwrap_or_else(|_| {
        error!("Failed to convert {} to char. Using default of 255", cb);
        255
    });

    let x = x.approx_as::<u8>().unwrap_or_else(|_| {
        error!("Failed to convert {} to char. Using default of 255", x);
        255
    });
    let y = y.approx_as::<u8>().unwrap_or_else(|_| {
        error!("Failed to convert {} to char. Using default of 255", y);
        255
    });

    (cb, x, y)
}

#[must_use]
fn encode_x11_mouse_wheel(
    delta: Vec2,
    modifiers: Modifiers,
    pos: &FreminalMousePosition,
    encoding: &MouseEncoding,
) -> Option<Cow<'static, [TerminalInput]>> {
    let padding = if encoding == &MouseEncoding::X11 {
        32
    } else {
        0
    };

    let mut cb = padding;

    cb += encode_mouse_for_x11(&MouseEvent::Scroll(delta), true);
    if cb == 32 {
        return None;
    }
    cb += encode_modifiers_for_x11(modifiers);

    let x = pos.x_as_character_column + padding;
    let y = pos.y_as_character_row + padding;
    let (cb, x, y) = encode_cb_and_x_and_y_as_u8_from_usize(cb, x, y);

    if encoding == &MouseEncoding::X11 {
        Some(raw_ascii_bytes_to_terminal_input(&[
            b'\x1b', b'[', b'M', cb, x, y,
        ]))
    } else {
        Some(collect_text(&format!("\x1b[<{cb};{x};{y}M",)))
    }
}

fn encode_x11_mouse_button(
    button: PointerButton,
    pressed: bool,
    modifiers: Modifiers,
    pos: &FreminalMousePosition,
    report_motion: bool,
    encoding: &MouseEncoding,
) -> Cow<'static, [TerminalInput]> {
    //Normal tracking mode sends an escape sequence on both button press and release. Modifier key (shift, ctrl, meta) information is also sent. It is enabled by specifying parameter 1000 to DECSET. On button press or release, xterm sends CSI M C b C x C y . The low two bits of C b encode button information: 0=MB1 pressed, 1=MB2 pressed, 2=MB3 pressed, 3=release. The next three bits encode the modifiers which were down when the button was pressed and are added together: 4=Shift, 8=Meta, 16=Control

    let padding = if encoding == &MouseEncoding::X11 {
        32
    } else {
        0
    };

    let motion = if report_motion { 32 } else { 0 };
    let mut cb: usize = padding + motion;
    let internal_pressed = if encoding == &MouseEncoding::X11 {
        pressed
    } else {
        true
    };

    cb += encode_mouse_for_x11(&MouseEvent::Button(button), internal_pressed);
    cb += encode_modifiers_for_x11(modifiers);

    let x = pos.x_as_character_column + padding;
    let y = pos.y_as_character_row + padding;
    let (cb, x, y) = encode_cb_and_x_and_y_as_u8_from_usize(cb, x, y);
    if encoding == &MouseEncoding::X11 {
        raw_ascii_bytes_to_terminal_input(&[b'\x1b', b'[', b'M', cb, x, y])
    } else {
        collect_text(&format!(
            "\x1b[<{cb};{x};{y}{}",
            if pressed { "M" } else { "m" }
        ))
    }
}
