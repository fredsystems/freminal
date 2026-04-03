// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
#![allow(clippy::module_name_repetitions)]
use std::borrow::Cow;

use conv2::ConvUtil;
use eframe::egui::{Modifiers, PointerButton, Vec2};
use freminal_common::buffer_states::modes::mouse::{MouseEncoding, MouseTrack};
use freminal_terminal_emulator::input::{
    TerminalInput, collect_text, raw_ascii_bytes_to_terminal_input,
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

// The `x` and `y` pixel-coordinate fields are stored for potential future use
// (e.g. pixel-precise hover reporting) but are not currently read after
// construction.  The allow suppresses the dead-code lint for those two fields
// without removing the information from the struct.
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
        self.x_as_character_column == other.x_as_character_column
            && self.y_as_character_row == other.y_as_character_row
    }
}

/// Encode a mouse button press/release event for the PTY.
///
/// `mouse_track` determines whether this tracking level reports button events.
/// `encoding` determines the wire format (X11 binary vs SGR text).
#[must_use]
pub fn handle_pointer_button(
    button: PointerButton,
    current_state: &PreviousMouseState,
    mouse_track: &MouseTrack,
    encoding: &MouseEncoding,
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
                    encoding,
                ));
            }
            None
        }
        // XtMseHilite (?1001) is an X11-era protocol that highlights the region
        // between press and release at the X11 window level.  Freminal does not
        // implement X11 highlighting, so we report events the same as XtMseX11.
        MouseTrack::XtMseX11
        | MouseTrack::XtMseHilite
        | MouseTrack::XtMseBtn
        | MouseTrack::XtMseAny => Some(encode_x11_mouse_button(
            button,
            current_state.button_pressed,
            current_state.modifiers,
            &current_state.mouse_position,
            false,
            encoding,
        )),
        MouseTrack::NoTracking | MouseTrack::Query(_) => None,
    }
}

/// Encode a mouse motion event for the PTY.
///
/// `mouse_track` determines whether this tracking level reports motion events.
/// `encoding` determines the wire format (X11 binary vs SGR text).
#[must_use]
pub fn handle_pointer_moved(
    current_state: &PreviousMouseState,
    previous_state: &PreviousMouseState,
    mouse_track: &MouseTrack,
    encoding: &MouseEncoding,
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
                    encoding,
                ));
            }

            None
        }
        MouseTrack::XtMseAny => {
            if previous_state.should_report(current_state) {
                return Some(encode_x11_mouse_button(
                    current_state.button,
                    current_state.button_pressed,
                    current_state.modifiers,
                    &current_state.mouse_position,
                    true,
                    encoding,
                ));
            }

            None
        }
        MouseTrack::NoTracking
        | MouseTrack::XtMsex10
        | MouseTrack::XtMseX11
        | MouseTrack::XtMseHilite
        | MouseTrack::Query(_) => None,
    }
}

/// Encode a mouse scroll event for the PTY.
///
/// `mouse_track` determines whether this tracking level reports scroll events.
/// `encoding` determines the wire format (X11 binary vs SGR text).
#[must_use]
pub fn handle_pointer_scroll(
    delta: Vec2,
    current_state: &PreviousMouseState,
    mouse_track: &MouseTrack,
    encoding: &MouseEncoding,
) -> Option<Cow<'static, [TerminalInput]>> {
    match mouse_track {
        MouseTrack::XtMseX11
        | MouseTrack::XtMseHilite
        | MouseTrack::XtMseBtn
        | MouseTrack::XtMseAny => encode_x11_mouse_wheel(
            delta,
            current_state.modifiers,
            &current_state.mouse_position,
            encoding,
        ),
        MouseTrack::NoTracking | MouseTrack::XtMsex10 | MouseTrack::Query(_) => None,
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

    // The X11 mouse protocol uses bit 3 (value 8) for the Meta modifier.
    // In practice, most terminal emulators (including WezTerm) map the Alt key
    // to Meta for mouse reporting purposes, matching the behavior of xterm.
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
    // Guard: ignore events with no vertical scroll component.  The terminal
    // mouse wheel protocol only defines vertical scroll (buttons 64/65).  If
    // `delta.y` is zero we must bail out *before* encoding, because:
    //
    // - For X11 encoding, `encode_mouse_for_x11` would produce button code 0
    //   (after the padding of 32 is added), which looks like a left-button
    //   press.
    // - For SGR encoding (padding = 0), button code 0 is an explicit
    //   left-button press — silently emitting phantom clicks in yazi and
    //   similar apps that enable SGR mouse mode.
    //
    // Horizontal-only scroll (`delta.y == 0, delta.x != 0`) is therefore
    // intentionally ignored here.
    if delta.y == 0.0 {
        return None;
    }

    let button_code = encode_mouse_for_x11(&MouseEvent::Scroll(delta), true);
    let modifiers_code = encode_modifiers_for_x11(modifiers);

    // Both X11 and SGR protocols use 1-based coordinates.
    // X11 additionally adds 32 as a "padding" offset to make the byte printable.
    if *encoding == MouseEncoding::X11 {
        let padding: usize = 32;
        let cb = padding + button_code + modifiers_code;
        let x = pos.x_as_character_column + 1 + padding;
        let y = pos.y_as_character_row + 1 + padding;
        let (cb, x, y) = encode_cb_and_x_and_y_as_u8_from_usize(cb, x, y);
        Some(raw_ascii_bytes_to_terminal_input(&[
            b'\x1b', b'[', b'M', cb, x, y,
        ]))
    } else {
        // SGR encoding: coordinates are decimal text — do NOT truncate to u8.
        // Terminals wider or taller than 255 columns/rows would produce wrong
        // output if we truncated before formatting.
        let cb = button_code + modifiers_code;
        let x = pos.x_as_character_column + 1;
        let y = pos.y_as_character_row + 1;
        Some(collect_text(&format!("\x1b[<{cb};{x};{y}M")))
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

    let padding = if *encoding == MouseEncoding::X11 {
        32
    } else {
        0
    };

    let motion = if report_motion { 32 } else { 0 };
    let mut cb: usize = padding + motion;
    let internal_pressed = if *encoding == MouseEncoding::X11 {
        pressed
    } else {
        true
    };

    cb += encode_mouse_for_x11(&MouseEvent::Button(button), internal_pressed);
    cb += encode_modifiers_for_x11(modifiers);

    // Both X11 and SGR protocols use 1-based coordinates.
    // X11 additionally adds 32 as a "padding" offset to make the byte printable.
    if *encoding == MouseEncoding::X11 {
        // X11 binary encoding: add the printability padding (32) and encode as bytes.
        let x = pos.x_as_character_column + 1 + padding;
        let y = pos.y_as_character_row + 1 + padding;
        let (cb, x, y) = encode_cb_and_x_and_y_as_u8_from_usize(cb, x, y);
        raw_ascii_bytes_to_terminal_input(&[b'\x1b', b'[', b'M', cb, x, y])
    } else {
        // SGR text encoding: coordinates are decimal — do NOT truncate to u8.
        // Terminals wider or taller than 255 columns/rows would produce wrong
        // output if we truncated before formatting.
        let x = pos.x_as_character_column + 1;
        let y = pos.y_as_character_row + 1;
        collect_text(&format!(
            "\x1b[<{cb};{x};{y}{}",
            if pressed { "M" } else { "m" }
        ))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use eframe::egui::Vec2;

    // Helper: extract the raw bytes from a Cow<[TerminalInput]> for comparison.
    fn inputs_to_bytes(inputs: &[TerminalInput]) -> Vec<u8> {
        inputs
            .iter()
            .map(|i| match i {
                TerminalInput::Ascii(b) => *b,
                other => panic!("unexpected TerminalInput variant: {other:?}"),
            })
            .collect()
    }

    // ── Regression tests: correct encoding is used based on separate encoding param ──

    #[test]
    fn sgr_button_press_is_single_contiguous_sequence() {
        let pos = FreminalMousePosition::new(4, 2, 0.0, 0.0); // col=4, row=2
        let state =
            PreviousMouseState::new(PointerButton::Primary, true, pos, Modifiers::default());
        let result = handle_pointer_button(
            PointerButton::Primary,
            &state,
            &MouseTrack::XtMseAny,
            &MouseEncoding::Sgr,
        )
        .expect("SGR button press should produce output");

        // The whole sequence must arrive as a single Cow slice.
        let bytes = inputs_to_bytes(result.as_ref());
        // Expected: ESC [ < 0 ; 5 ; 3 M  (1-based, col+1=5, row+1=3)
        let expected = b"\x1b[<0;5;3M";
        assert_eq!(
            bytes, expected,
            "SGR button press sequence fragmented or wrong: got {bytes:?}"
        );
    }

    #[test]
    fn sgr_button_press_wide_terminal_column_not_truncated() {
        // Column 300 would wrap to 44 if truncated to u8 (300 % 256 = 44).
        // With the fix, the decimal SGR string must contain "301" (1-based).
        let pos = FreminalMousePosition::new(300, 10, 0.0, 0.0);
        let state =
            PreviousMouseState::new(PointerButton::Primary, true, pos, Modifiers::default());
        let result = handle_pointer_button(
            PointerButton::Primary,
            &state,
            &MouseTrack::XtMseAny,
            &MouseEncoding::Sgr,
        )
        .expect("wide-terminal SGR button press should produce output");

        let bytes = inputs_to_bytes(result.as_ref());
        let s = std::str::from_utf8(&bytes).expect("SGR sequence must be valid UTF-8");
        assert!(
            s.contains(";301;"),
            "SGR sequence should contain ';301;' for column 300, got: {s:?}"
        );
    }

    #[test]
    fn sgr_scroll_wide_terminal_row_not_truncated() {
        // Row 260 would wrap to 4 if truncated to u8 (260 % 256 = 4).
        // With the fix, the decimal SGR string must contain "261" (1-based).
        let pos = FreminalMousePosition::new(5, 260, 0.0, 0.0);
        let state =
            PreviousMouseState::new(PointerButton::Primary, false, pos, Modifiers::default());
        let result = handle_pointer_scroll(
            Vec2::new(0.0, 1.0), // scroll up
            &state,
            &MouseTrack::XtMseAny,
            &MouseEncoding::Sgr,
        )
        .expect("wide-terminal SGR scroll should produce output");

        let bytes = inputs_to_bytes(result.as_ref());
        let s = std::str::from_utf8(&bytes).expect("SGR sequence must be valid UTF-8");
        assert!(
            s.ends_with(";261M"),
            "SGR scroll sequence should end with ';261M' for row 260, got: {s:?}"
        );
    }

    // ── Zero-delta scroll guard ──

    #[test]
    fn zero_delta_scroll_returns_none_for_sgr() {
        let pos = FreminalMousePosition::new(10, 10, 0.0, 0.0);
        let state =
            PreviousMouseState::new(PointerButton::Primary, false, pos, Modifiers::default());
        // A zero-delta scroll event must produce None, not a phantom click.
        let result = handle_pointer_scroll(
            Vec2::ZERO,
            &state,
            &MouseTrack::XtMseAny,
            &MouseEncoding::Sgr,
        );
        assert!(
            result.is_none(),
            "zero-delta SGR scroll should return None to avoid phantom clicks, got: {result:?}"
        );
    }

    #[test]
    fn zero_delta_scroll_returns_none_for_x11() {
        let pos = FreminalMousePosition::new(10, 10, 0.0, 0.0);
        let state =
            PreviousMouseState::new(PointerButton::Primary, false, pos, Modifiers::default());
        let result = handle_pointer_scroll(
            Vec2::ZERO,
            &state,
            &MouseTrack::XtMseX11,
            &MouseEncoding::X11,
        );
        assert!(
            result.is_none(),
            "zero-delta X11 scroll should return None, got: {result:?}"
        );
    }

    #[test]
    fn nonzero_delta_scroll_produces_output_for_sgr() {
        let pos = FreminalMousePosition::new(5, 5, 0.0, 0.0);
        let state =
            PreviousMouseState::new(PointerButton::Primary, false, pos, Modifiers::default());
        // Scroll up (positive y delta) must produce a real mouse report.
        let result = handle_pointer_scroll(
            Vec2::new(0.0, 1.0),
            &state,
            &MouseTrack::XtMseAny,
            &MouseEncoding::Sgr,
        );
        assert!(
            result.is_some(),
            "non-zero SGR scroll should produce output"
        );
        let bytes = inputs_to_bytes(result.unwrap().as_ref());
        let s = std::str::from_utf8(&bytes).expect("SGR sequence must be valid UTF-8");
        // Button code 64 for scroll-up, 1-based coords (5+1=6, 5+1=6)
        assert_eq!(s, "\x1b[<64;6;6M", "SGR scroll-up sequence wrong: {s:?}");
    }

    // ── Horizontal-only scroll returns None ──

    #[test]
    fn horizontal_only_scroll_returns_none_for_sgr() {
        let pos = FreminalMousePosition::new(10, 10, 0.0, 0.0);
        let state =
            PreviousMouseState::new(PointerButton::Primary, false, pos, Modifiers::default());
        let result = handle_pointer_scroll(
            Vec2::new(3.0, 0.0),
            &state,
            &MouseTrack::XtMseAny,
            &MouseEncoding::Sgr,
        );
        assert!(
            result.is_none(),
            "horizontal-only SGR scroll should return None, got: {result:?}"
        );
    }

    #[test]
    fn horizontal_only_scroll_returns_none_for_x11() {
        let pos = FreminalMousePosition::new(10, 10, 0.0, 0.0);
        let state =
            PreviousMouseState::new(PointerButton::Primary, false, pos, Modifiers::default());
        let result = handle_pointer_scroll(
            Vec2::new(-5.0, 0.0),
            &state,
            &MouseTrack::XtMseX11,
            &MouseEncoding::X11,
        );
        assert!(
            result.is_none(),
            "horizontal-only X11 scroll should return None, got: {result:?}"
        );
    }

    // ── Unit-delta scroll tests ──

    #[test]
    fn unit_scroll_up_sgr_produces_button_64() {
        let pos = FreminalMousePosition::new(3, 7, 0.0, 0.0);
        let state =
            PreviousMouseState::new(PointerButton::Primary, false, pos, Modifiers::default());
        let result = handle_pointer_scroll(
            Vec2::new(0.0, 1.0),
            &state,
            &MouseTrack::XtMseAny,
            &MouseEncoding::Sgr,
        )
        .expect("unit scroll-up should produce output");
        let bytes = inputs_to_bytes(result.as_ref());
        let s = std::str::from_utf8(&bytes).expect("SGR sequence must be valid UTF-8");
        // Button 64 = scroll up, col 3+1=4, row 7+1=8
        assert_eq!(s, "\x1b[<64;4;8M", "SGR unit scroll-up wrong: {s:?}");
    }

    #[test]
    fn unit_scroll_down_sgr_produces_button_65() {
        let pos = FreminalMousePosition::new(3, 7, 0.0, 0.0);
        let state =
            PreviousMouseState::new(PointerButton::Primary, false, pos, Modifiers::default());
        let result = handle_pointer_scroll(
            Vec2::new(0.0, -1.0),
            &state,
            &MouseTrack::XtMseAny,
            &MouseEncoding::Sgr,
        )
        .expect("unit scroll-down should produce output");
        let bytes = inputs_to_bytes(result.as_ref());
        let s = std::str::from_utf8(&bytes).expect("SGR sequence must be valid UTF-8");
        // Button 65 = scroll down, col 3+1=4, row 7+1=8
        assert_eq!(s, "\x1b[<65;4;8M", "SGR unit scroll-down wrong: {s:?}");
    }

    #[test]
    fn unit_scroll_up_x11_produces_button_64() {
        let pos = FreminalMousePosition::new(0, 0, 0.0, 0.0);
        let state =
            PreviousMouseState::new(PointerButton::Primary, false, pos, Modifiers::default());
        let result = handle_pointer_scroll(
            Vec2::new(0.0, 1.0),
            &state,
            &MouseTrack::XtMseX11,
            &MouseEncoding::X11,
        )
        .expect("unit scroll-up X11 should produce output");
        let bytes = inputs_to_bytes(result.as_ref());
        // X11: ESC [ M <cb> <x> <y>
        // cb = 32 (padding) + 64 (button) = 96
        // x = 0 + 1 + 32 = 33
        // y = 0 + 1 + 32 = 33
        assert_eq!(bytes, b"\x1b[M`!!", "X11 unit scroll-up wrong: {bytes:?}");
    }

    #[test]
    fn unit_scroll_down_x11_produces_button_65() {
        let pos = FreminalMousePosition::new(0, 0, 0.0, 0.0);
        let state =
            PreviousMouseState::new(PointerButton::Primary, false, pos, Modifiers::default());
        let result = handle_pointer_scroll(
            Vec2::new(0.0, -1.0),
            &state,
            &MouseTrack::XtMseX11,
            &MouseEncoding::X11,
        )
        .expect("unit scroll-down X11 should produce output");
        let bytes = inputs_to_bytes(result.as_ref());
        // cb = 32 + 65 = 97 = 'a'
        assert_eq!(bytes, b"\x1b[Ma!!", "X11 unit scroll-down wrong: {bytes:?}");
    }

    // ── No-tracking returns None ──

    #[test]
    fn scroll_with_no_tracking_returns_none() {
        let pos = FreminalMousePosition::new(5, 5, 0.0, 0.0);
        let state =
            PreviousMouseState::new(PointerButton::Primary, false, pos, Modifiers::default());
        let result = handle_pointer_scroll(
            Vec2::new(0.0, 1.0),
            &state,
            &MouseTrack::NoTracking,
            &MouseEncoding::X11,
        );
        assert!(
            result.is_none(),
            "scroll with NoTracking should return None"
        );
    }

    // ── The lazygit scenario: tracking=XtMseAny + encoding=Sgr ──
    // This is the exact combination that was broken before the decoupling fix.
    // lazygit sends: ?1006h (SGR encoding), then ?1000h (X11 tracking), then
    // ?1002h (button tracking), then ?1003h (any-event tracking).  With the
    // old conflated design, ?1003h overwrote the SGR encoding.

    #[test]
    fn lazygit_scenario_any_tracking_sgr_encoding_button_press() {
        let pos = FreminalMousePosition::new(10, 5, 0.0, 0.0);
        let state =
            PreviousMouseState::new(PointerButton::Primary, true, pos, Modifiers::default());
        let result = handle_pointer_button(
            PointerButton::Primary,
            &state,
            &MouseTrack::XtMseAny,
            &MouseEncoding::Sgr,
        )
        .expect("any-tracking + SGR encoding should produce output");

        let bytes = inputs_to_bytes(result.as_ref());
        let s = std::str::from_utf8(&bytes).expect("SGR sequence must be valid UTF-8");
        // Must be SGR format, not X11 binary
        assert!(
            s.starts_with("\x1b[<"),
            "expected SGR format (ESC[<...), got: {s:?}"
        );
        assert_eq!(s, "\x1b[<0;11;6M");
    }

    #[test]
    fn lazygit_scenario_any_tracking_sgr_encoding_motion() {
        let pos = FreminalMousePosition::new(12, 7, 0.0, 0.0);
        let current =
            PreviousMouseState::new(PointerButton::Primary, false, pos, Modifiers::default());
        let prev_pos = FreminalMousePosition::new(11, 7, 0.0, 0.0);
        let previous = PreviousMouseState::new(
            PointerButton::Primary,
            false,
            prev_pos,
            Modifiers::default(),
        );
        let result = handle_pointer_moved(
            &current,
            &previous,
            &MouseTrack::XtMseAny,
            &MouseEncoding::Sgr,
        )
        .expect("any-tracking + SGR encoding should produce motion output");

        let bytes = inputs_to_bytes(result.as_ref());
        let s = std::str::from_utf8(&bytes).expect("SGR sequence must be valid UTF-8");
        assert!(
            s.starts_with("\x1b[<"),
            "expected SGR format for motion, got: {s:?}"
        );
        // motion bit = 32, button 0 (Primary, not held), cb = 32 + 0 = 32
        // Lowercase 'm' because button_pressed is false (release suffix in SGR).
        assert_eq!(s, "\x1b[<32;13;8m");
    }

    #[test]
    fn lazygit_scenario_any_tracking_sgr_encoding_scroll() {
        let pos = FreminalMousePosition::new(10, 5, 0.0, 0.0);
        let state =
            PreviousMouseState::new(PointerButton::Primary, false, pos, Modifiers::default());
        let result = handle_pointer_scroll(
            Vec2::new(0.0, 1.0),
            &state,
            &MouseTrack::XtMseAny,
            &MouseEncoding::Sgr,
        )
        .expect("any-tracking + SGR encoding should produce scroll output");

        let bytes = inputs_to_bytes(result.as_ref());
        let s = std::str::from_utf8(&bytes).expect("SGR sequence must be valid UTF-8");
        assert!(
            s.starts_with("\x1b[<"),
            "expected SGR format for scroll, got: {s:?}"
        );
        assert_eq!(s, "\x1b[<64;11;6M");
    }

    // ── Verify X11 encoding works correctly with various tracking levels ──

    #[test]
    fn x11_encoding_with_any_tracking_button_press() {
        let pos = FreminalMousePosition::new(5, 3, 0.0, 0.0);
        let state =
            PreviousMouseState::new(PointerButton::Primary, true, pos, Modifiers::default());
        let result = handle_pointer_button(
            PointerButton::Primary,
            &state,
            &MouseTrack::XtMseAny,
            &MouseEncoding::X11,
        )
        .expect("any-tracking + X11 encoding should produce output");

        let bytes = inputs_to_bytes(result.as_ref());
        // Must be X11 binary format: ESC [ M <cb> <x> <y>
        assert_eq!(
            bytes[0..3],
            *b"\x1b[M",
            "expected X11 format, got: {bytes:?}"
        );
        // cb = 32 + 0 (left press) = 32, x = 5+1+32 = 38, y = 3+1+32 = 36
        assert_eq!(bytes, b"\x1b[M &$", "X11 button press wrong: {bytes:?}");
    }
}
