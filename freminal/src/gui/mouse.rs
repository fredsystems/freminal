// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.
#![allow(clippy::module_name_repetitions)]
use std::borrow::Cow;

use conv2::ConvUtil;
use egui::{Modifiers, PointerButton, Vec2};
use freminal_common::buffer_states::modes::mouse::{MouseEncoding, MouseTrack};
use freminal_terminal_emulator::input::{
    TerminalInput, collect_text, raw_ascii_bytes_to_terminal_input,
};

/// Snapshot of the mouse button and position state from the previous frame.
///
/// Used to detect state transitions (e.g. press → release) and to suppress
/// redundant mouse-tracking reports to the PTY when nothing has changed.
#[derive(Debug, PartialEq, Eq, Clone)]
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
            mouse_position: FreminalMousePosition::new(0, 0),
            modifiers: Modifiers::default(),
        }
    }
}

impl PreviousMouseState {
    /// Create a new `PreviousMouseState` from `self`, updating only the position.
    #[must_use]
    pub const fn new_from_previous_mouse_state(&self, position: FreminalMousePosition) -> Self {
        Self {
            button: self.button,
            button_pressed: self.button_pressed,
            mouse_position: position,
            modifiers: self.modifiers,
        }
    }

    /// Create a new `PreviousMouseState` with all fields specified explicitly.
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

    /// Returns `true` if the CELL-granular mouse position has changed
    /// relative to `new`. Used by cell-based mouse encodings (X11, SGR,
    /// UTF-8), via [`motion_position_changed`], which picks this or
    /// [`Self::pixel_position_changed`] based on the active encoding —
    /// deliberately compares only the cell fields, not the whole
    /// [`FreminalMousePosition`], so gaining pixel fields (Task 124.3a)
    /// cannot change this method's answer for callers that never populate
    /// them.
    #[must_use]
    pub const fn should_report(&self, new: &Self) -> bool {
        self.mouse_position.x_as_character_column != new.mouse_position.x_as_character_column
            || self.mouse_position.y_as_character_row != new.mouse_position.y_as_character_row
    }

    /// Returns `true` if the PIXEL-granular mouse position has changed
    /// relative to `new` — the precision [`MouseEncoding::SgrPixels`]
    /// (`?1016`) needs. Distinct from [`Self::should_report`] so two
    /// pointer moves that stay within one cell still compare unequal here
    /// (Task 124.3a).
    #[must_use]
    pub const fn pixel_position_changed(&self, new: &Self) -> bool {
        self.mouse_position.x_as_physical_pixel != new.mouse_position.x_as_physical_pixel
            || self.mouse_position.y_as_physical_pixel != new.mouse_position.y_as_physical_pixel
    }
}

/// Whether a pointer button is currently held, for
/// [`motion_track_wants_report`]'s `?1002` gate.
///
/// Named domain enum (`freminal-state-representation`), not a bare `bool`:
/// `motion_track_wants_report` takes this instead of `button_pressed: bool`
/// so a call site cannot pass an unnamed `true`/`false`. Each of the two
/// call sites (`handle_pointer_moved` here, and the frame-time
/// selection-drag gate in `write_input_to_terminal`) classifies its own
/// pre-existing `PreviousMouseState::button_pressed: bool` field into this
/// enum inline with an explicit `if`, rather than through a shared
/// `bool`-taking conversion function — a `From<bool>`/`from_button_pressed`
/// helper would itself be exactly the kind of unnamed-bool entry point this
/// enum exists to close off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerButtonHold {
    /// A pointer button is currently held down.
    Held,
    /// No pointer button is currently held.
    NotHeld,
}

/// Whether the mouse position changed, for [`motion_track_wants_report`]'s
/// gate — the named-enum return type of [`motion_position_changed`].
///
/// Named domain enum (`freminal-state-representation`), not a bare `bool`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MotionPositionChange {
    /// The position changed (at whatever granularity was compared).
    Changed,
    /// The position did not change.
    Unchanged,
}

/// Whether `mouse_track`'s motion-reporting rule considers this motion
/// event reportable.
///
/// Given whether the tracked position actually changed (at whatever
/// granularity the caller compared — cell for ordinary tracking, pixel for
/// [`MouseEncoding::SgrPixels`], see [`handle_pointer_moved`]) and whether a
/// button is currently held.
///
/// Pure decision, shared by [`handle_pointer_moved`] (which additionally
/// encodes the escape sequence) and the frame-time selection-drag gate in
/// `write_input_to_terminal`, which needs the SAME reportable-or-not
/// decision `handle_pointer_moved` used to return, without asking it to
/// encode bytes nobody will send — Task 124.3a moved PTY motion-report
/// sending to the out-of-frame immediate-report hook path
/// (`terminal::pty_mouse_report`), so the frame-time arm must decide
/// whether a report *would* have been sent without producing one.
#[must_use]
pub const fn motion_track_wants_report(
    mouse_track: &MouseTrack,
    button_hold: PointerButtonHold,
    position_change: MotionPositionChange,
) -> bool {
    match mouse_track {
        MouseTrack::XtMseBtn => {
            matches!(button_hold, PointerButtonHold::Held)
                && matches!(position_change, MotionPositionChange::Changed)
        }
        MouseTrack::XtMseAny => matches!(position_change, MotionPositionChange::Changed),
        MouseTrack::NoTracking
        | MouseTrack::XtMsex10
        | MouseTrack::XtMseX11
        | MouseTrack::XtMseHilite
        | MouseTrack::Query(_) => false,
    }
}

/// GUI-owned, per-pane state for the immediate (out-of-frame) PTY
/// motion-report path (Task 124.3a).
///
/// Lives on `ViewState` (see that field's doc) — read and written by
/// `FreminalGui::on_pointer_moved` (`terminal::pty_mouse_report`), which
/// runs outside any frame — rather than on `PaneRenderCache`, which is
/// frame-scoped render-cache state and must not gain out-of-frame
/// readers/writers.
///
/// Carries ONLY the last-reported position, per pane: each pane's PTY
/// stream needs its own independent report history, since two panes'
/// terminal content are entirely unrelated. The held pointer BUTTON is
/// deliberately NOT here — see
/// [`PerWindowState::held_pointer_button`](super::window::PerWindowState::held_pointer_button)'s
/// doc for why that is window-owned instead: a physical button press and
/// its matching release are events on the SAME OS pointer device,
/// independent of which pane happens to be active at either moment, and
/// storing that identity per-pane (this type's original Task 124.3a shape)
/// could leave a pane's button "permanently held" across a focus change
/// between press and release.
///
/// Deliberately NOT [`PreviousMouseState`]: that type is the frame-local
/// carry `write_input_to_terminal` threads through `InputCarryState` on
/// every call. The underlying concept — "what got last reported, for
/// change-detection" — is related, which is why this type's `position`
/// field has the same shape as `PreviousMouseState::mouse_position`, but
/// the OWNER and LIFETIME differ, and conflating the two would blur which
/// state belongs to which control path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ImmediateMouseReportState {
    /// The position (cell + physical-pixel) the last immediate report was
    /// computed from. `None` before the first observed motion in this
    /// pane — deliberately not a synthetic `(0, 0)` default; see
    /// [`motion_position_changed`]'s doc for why that distinction matters.
    pub position: Option<FreminalMousePosition>,
}

impl ImmediateMouseReportState {
    /// Reset this pane's report baseline (Task 124.3a, review correction —
    /// pointer-presence-loss reset).
    ///
    /// Called when the app observes `App::on_pointer_presence_lost` (the
    /// pointer left the window, or the window lost focus): once no further
    /// pointer events are guaranteed to arrive, `position` can no longer be
    /// trusted as a real "last reported position" baseline for THIS pane —
    /// re-entry or focus-regain must be treated as a fresh start. Setting
    /// `position` back to `None`, rather than leaving it at its last real
    /// value, makes the next observed motion unambiguously "changed" per
    /// [`motion_position_changed`]'s `None`-previous semantics, even if it
    /// lands at the exact same position the pane last reported.
    pub const fn reset(&mut self) {
        self.position = None;
    }
}

/// A mouse input event to encode for the PTY.
pub enum MouseEvent {
    /// A pointer button press or release.
    Button(PointerButton),
    /// A scroll-wheel delta.
    Scroll(Vec2),
}

/// Terminal mouse position, carrying both cell and pixel coordinates.
///
/// Character-cell coordinates are used by every cell-granular mouse
/// encoding (X11, SGR, UTF-8); physical-pixel coordinates are used only by
/// `MouseEncoding::SgrPixels` (`?1016` — Task 124.3a).
///
/// Both coordinate pairs are relative to the terminal content area's
/// top-left, zero-based. The pixel fields default to `0` via
/// [`Self::new`] for callers that only need cell precision; use
/// [`Self::new_with_pixels`] when pixel precision is also needed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FreminalMousePosition {
    pub(crate) x_as_character_column: usize,
    pub(crate) y_as_character_row: usize,
    pub(crate) x_as_physical_pixel: usize,
    pub(crate) y_as_physical_pixel: usize,
}

impl FreminalMousePosition {
    /// Create a new `FreminalMousePosition` from cell coordinates alone.
    /// The physical-pixel fields are `0` — only use this constructor where
    /// the caller is certain `MouseEncoding::SgrPixels` cannot be in play,
    /// or where pixel precision has no effect (e.g. purely cell-based
    /// comparisons via [`PreviousMouseState::should_report`]).
    #[must_use]
    pub const fn new(x_as_character_column: usize, y_as_character_row: usize) -> Self {
        Self {
            x_as_character_column,
            y_as_character_row,
            x_as_physical_pixel: 0,
            y_as_physical_pixel: 0,
        }
    }

    /// Create a new `FreminalMousePosition` with both cell and
    /// physical-pixel coordinates specified explicitly.
    #[must_use]
    pub const fn new_with_pixels(
        x_as_character_column: usize,
        y_as_character_row: usize,
        x_as_physical_pixel: usize,
        y_as_physical_pixel: usize,
    ) -> Self {
        Self {
            x_as_character_column,
            y_as_character_row,
            x_as_physical_pixel,
            y_as_physical_pixel,
        }
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

/// Whether the mouse position has changed between `previous` and `current`.
///
/// Compares at the granularity `encoding` needs (pixel for
/// [`MouseEncoding::SgrPixels`], cell otherwise) — see
/// [`PreviousMouseState::should_report`] and
/// [`PreviousMouseState::pixel_position_changed`] for what each
/// granularity compares.
///
/// `previous == None` (no prior observed position at all — the very
/// first motion event on this report stream, or the report baseline was
/// just reset via [`ImmediateMouseReportState::reset`] after a
/// pointer-presence loss) is unambiguously [`MotionPositionChange::Changed`],
/// regardless of where `current` lands. Task 124.3a's original
/// implementation substituted a synthetic cell/pixel `(0, 0)` baseline
/// for "no previous position", which conflated "never observed a
/// position" with "previously observed at the origin": a genuine first
/// motion that itself lands at cell/pixel `(0, 0)` would then compare
/// equal to that synthetic baseline and be wrongly treated as
/// unchanged — silently dropping the first `?1003` report of a session
/// that happens to start at the terminal's top-left corner. Threading
/// `Option` through explicitly, rather than a sentinel value, makes that
/// case structurally unrepresentable.
#[must_use]
pub fn motion_position_changed(
    previous: Option<&PreviousMouseState>,
    current: &PreviousMouseState,
    encoding: &MouseEncoding,
) -> MotionPositionChange {
    let Some(previous) = previous else {
        return MotionPositionChange::Changed;
    };
    let changed = if *encoding == MouseEncoding::SgrPixels {
        previous.pixel_position_changed(current)
    } else {
        previous.should_report(current)
    };
    if changed {
        MotionPositionChange::Changed
    } else {
        MotionPositionChange::Unchanged
    }
}

/// Encode a mouse motion event for the PTY.
///
/// `mouse_track` determines whether this tracking level reports motion
/// events. `encoding` determines the wire format (X11 binary vs SGR text).
/// `previous_state` is `None` when there is no prior observed position at
/// all (the first motion event on this report stream) — see
/// [`motion_position_changed`]'s doc for why that must NOT be modeled as
/// a synthetic `(0, 0)` `PreviousMouseState`.
#[must_use]
pub fn handle_pointer_moved(
    current_state: &PreviousMouseState,
    previous_state: Option<&PreviousMouseState>,
    mouse_track: &MouseTrack,
    encoding: &MouseEncoding,
) -> Option<Cow<'static, [TerminalInput]>> {
    // Task 124.3a: `?1016` (SgrPixels) motion must be distinguishable at
    // pixel granularity — two moves that stay within one cell must still
    // compare unequal — so the position-changed check is granularity-aware
    // rather than always comparing cell coordinates.
    let position_change = motion_position_changed(previous_state, current_state, encoding);
    let button_hold = if current_state.button_pressed {
        PointerButtonHold::Held
    } else {
        PointerButtonHold::NotHeld
    };

    if !motion_track_wants_report(mouse_track, button_hold, position_change) {
        return None;
    }

    Some(encode_x11_mouse_button(
        current_state.button,
        current_state.button_pressed,
        current_state.modifiers,
        &current_state.mouse_position,
        true,
        encoding,
    ))
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
    match encoding {
        MouseEncoding::X11 => {
            let padding: usize = 32;
            let cb = padding + button_code + modifiers_code;
            let x = pos.x_as_character_column + 1 + padding;
            let y = pos.y_as_character_row + 1 + padding;
            let (cb, x, y) = encode_cb_and_x_and_y_as_u8_from_usize(cb, x, y);
            Some(raw_ascii_bytes_to_terminal_input(&[
                b'\x1b', b'[', b'M', cb, x, y,
            ]))
        }
        MouseEncoding::SgrPixels => {
            // Task 124.3a: same SGR framing as `?1006`, but `x`/`y` are
            // one-based PHYSICAL PIXEL coordinates relative to the
            // terminal content area's top-left, not cell column/row.
            let cb = button_code + modifiers_code;
            let x = pos.x_as_physical_pixel + 1;
            let y = pos.y_as_physical_pixel + 1;
            Some(collect_text(&format!("\x1b[<{cb};{x};{y}M")))
        }
        MouseEncoding::Sgr | MouseEncoding::Utf8 => {
            // SGR encoding: coordinates are decimal text — do NOT truncate to
            // u8. Terminals wider or taller than 255 columns/rows would
            // produce wrong output if we truncated before formatting.
            let cb = button_code + modifiers_code;
            let x = pos.x_as_character_column + 1;
            let y = pos.y_as_character_row + 1;
            Some(collect_text(&format!("\x1b[<{cb};{x};{y}M")))
        }
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
    match encoding {
        MouseEncoding::X11 => {
            // X11 binary encoding: add the printability padding (32) and encode as bytes.
            let x = pos.x_as_character_column + 1 + padding;
            let y = pos.y_as_character_row + 1 + padding;
            let (cb, x, y) = encode_cb_and_x_and_y_as_u8_from_usize(cb, x, y);
            raw_ascii_bytes_to_terminal_input(&[b'\x1b', b'[', b'M', cb, x, y])
        }
        MouseEncoding::SgrPixels => {
            // Task 124.3a: same SGR framing as `?1006`, but `x`/`y` are
            // one-based PHYSICAL PIXEL coordinates relative to the
            // terminal content area's top-left, not cell column/row.
            let x = pos.x_as_physical_pixel + 1;
            let y = pos.y_as_physical_pixel + 1;
            collect_text(&format!(
                "\x1b[<{cb};{x};{y}{}",
                if pressed { "M" } else { "m" }
            ))
        }
        MouseEncoding::Sgr | MouseEncoding::Utf8 => {
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
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use egui::Vec2;

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
        let pos = FreminalMousePosition::new(4, 2); // col=4, row=2
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
        let pos = FreminalMousePosition::new(300, 10);
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
        let pos = FreminalMousePosition::new(5, 260);
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
        let pos = FreminalMousePosition::new(10, 10);
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
        let pos = FreminalMousePosition::new(10, 10);
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
        let pos = FreminalMousePosition::new(5, 5);
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
        let pos = FreminalMousePosition::new(10, 10);
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
        let pos = FreminalMousePosition::new(10, 10);
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
        let pos = FreminalMousePosition::new(3, 7);
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
        let pos = FreminalMousePosition::new(3, 7);
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
        let pos = FreminalMousePosition::new(0, 0);
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
        let pos = FreminalMousePosition::new(0, 0);
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
        let pos = FreminalMousePosition::new(5, 5);
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
        let pos = FreminalMousePosition::new(10, 5);
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
        let pos = FreminalMousePosition::new(12, 7);
        let current =
            PreviousMouseState::new(PointerButton::Primary, false, pos, Modifiers::default());
        let prev_pos = FreminalMousePosition::new(11, 7);
        let previous = PreviousMouseState::new(
            PointerButton::Primary,
            false,
            prev_pos,
            Modifiers::default(),
        );
        let result = handle_pointer_moved(
            &current,
            Some(&previous),
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
        let pos = FreminalMousePosition::new(10, 5);
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
        let pos = FreminalMousePosition::new(5, 3);
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

    // ── Task 124.3a: `motion_track_wants_report` ─────────────────────────

    #[test]
    fn motion_track_wants_report_xtmsebtn_needs_both_button_and_position_change() {
        assert!(motion_track_wants_report(
            &MouseTrack::XtMseBtn,
            PointerButtonHold::Held,
            MotionPositionChange::Changed
        ));
        assert!(!motion_track_wants_report(
            &MouseTrack::XtMseBtn,
            PointerButtonHold::NotHeld,
            MotionPositionChange::Changed
        ));
        assert!(!motion_track_wants_report(
            &MouseTrack::XtMseBtn,
            PointerButtonHold::Held,
            MotionPositionChange::Unchanged
        ));
    }

    #[test]
    fn motion_track_wants_report_xtmseany_needs_only_position_change() {
        assert!(motion_track_wants_report(
            &MouseTrack::XtMseAny,
            PointerButtonHold::NotHeld,
            MotionPositionChange::Changed
        ));
        assert!(!motion_track_wants_report(
            &MouseTrack::XtMseAny,
            PointerButtonHold::NotHeld,
            MotionPositionChange::Unchanged
        ));
    }

    #[test]
    fn motion_track_wants_report_other_tracks_never_report_motion() {
        for track in [
            MouseTrack::NoTracking,
            MouseTrack::XtMsex10,
            MouseTrack::XtMseX11,
            MouseTrack::XtMseHilite,
            MouseTrack::Query(1000),
        ] {
            assert!(!motion_track_wants_report(
                &track,
                PointerButtonHold::Held,
                MotionPositionChange::Changed
            ));
        }
    }

    // ── Task 124.3a (review correction #2): `ImmediateMouseReportState::reset` ─

    #[test]
    fn immediate_mouse_report_state_reset_clears_a_real_position() {
        let mut report = ImmediateMouseReportState {
            position: Some(FreminalMousePosition::new(5, 3)),
        };
        report.reset();
        assert_eq!(report.position, None);
    }

    #[test]
    fn immediate_mouse_report_state_reset_is_a_no_op_when_already_none() {
        let mut report = ImmediateMouseReportState::default();
        assert_eq!(report.position, None);
        report.reset();
        assert_eq!(report.position, None);
    }

    // ── Task 124.3a: `PreviousMouseState::pixel_position_changed` ────────

    #[test]
    fn pixel_position_changed_true_when_pixels_differ_within_the_same_cell() {
        // Same cell (2, 3), different sub-cell pixel offset — must still
        // report a change at pixel granularity.
        let a = PreviousMouseState::new(
            PointerButton::Primary,
            false,
            FreminalMousePosition::new_with_pixels(2, 3, 20, 30),
            Modifiers::default(),
        );
        let b = PreviousMouseState::new(
            PointerButton::Primary,
            false,
            FreminalMousePosition::new_with_pixels(2, 3, 21, 30),
            Modifiers::default(),
        );
        assert!(a.pixel_position_changed(&b));
        // `should_report` (cell-granular) must NOT see a change here.
        assert!(!a.should_report(&b));
    }

    #[test]
    fn pixel_position_changed_false_when_pixels_are_identical() {
        let a = PreviousMouseState::new(
            PointerButton::Primary,
            false,
            FreminalMousePosition::new_with_pixels(2, 3, 20, 30),
            Modifiers::default(),
        );
        let b = a.clone();
        assert!(!a.pixel_position_changed(&b));
    }

    // ── Task 124.3a: `?1016` (SgrPixels) exact-byte encoding ─────────────

    #[test]
    fn sgr_pixels_button_press_uses_physical_pixel_coordinates() {
        // Cell (4, 2) but pixel (37, 19) — the encoded bytes must reflect
        // the PIXEL position, not the cell position.
        let pos = FreminalMousePosition::new_with_pixels(4, 2, 37, 19);
        let state =
            PreviousMouseState::new(PointerButton::Primary, true, pos, Modifiers::default());
        let result = handle_pointer_button(
            PointerButton::Primary,
            &state,
            &MouseTrack::XtMseAny,
            &MouseEncoding::SgrPixels,
        )
        .expect("SgrPixels button press should produce output");

        let bytes = inputs_to_bytes(result.as_ref());
        // 1-based pixel coords: 37+1=38, 19+1=20. Cell coords (5, 3) must
        // NOT appear.
        assert_eq!(bytes, b"\x1b[<0;38;20M");
    }

    #[test]
    fn sgr_pixels_wheel_uses_physical_pixel_coordinates() {
        let pos = FreminalMousePosition::new_with_pixels(10, 10, 123, 45);
        let state =
            PreviousMouseState::new(PointerButton::Primary, false, pos, Modifiers::default());
        let result = handle_pointer_scroll(
            Vec2::new(0.0, 1.0),
            &state,
            &MouseTrack::XtMseAny,
            &MouseEncoding::SgrPixels,
        )
        .expect("SgrPixels scroll should produce output");

        let bytes = inputs_to_bytes(result.as_ref());
        let s = std::str::from_utf8(&bytes).expect("SGR sequence must be valid UTF-8");
        // Button 64 = scroll up, 1-based pixel coords 124, 46.
        assert_eq!(s, "\x1b[<64;124;46M");
    }

    #[test]
    fn sgr_pixels_motion_uses_physical_pixel_coordinates_and_lowercase_m_when_not_pressed() {
        let current_pos = FreminalMousePosition::new_with_pixels(12, 7, 200, 100);
        let current = PreviousMouseState::new(
            PointerButton::Primary,
            false,
            current_pos,
            Modifiers::default(),
        );
        let previous_pos = FreminalMousePosition::new_with_pixels(11, 7, 190, 100);
        let previous = PreviousMouseState::new(
            PointerButton::Primary,
            false,
            previous_pos,
            Modifiers::default(),
        );
        let result = handle_pointer_moved(
            &current,
            Some(&previous),
            &MouseTrack::XtMseAny,
            &MouseEncoding::SgrPixels,
        )
        .expect("SgrPixels motion should produce output");

        let bytes = inputs_to_bytes(result.as_ref());
        let s = std::str::from_utf8(&bytes).expect("SGR sequence must be valid UTF-8");
        // motion bit = 32, button 0 -> cb = 32; pixel coords 201, 101.
        assert_eq!(s, "\x1b[<32;201;101m");
    }

    #[test]
    fn sgr_pixels_two_moves_within_one_cell_produce_two_distinct_reports() {
        // Both positions floor to the same cell (5, 5), but the pixel
        // offset differs — under SgrPixels this must still be treated as
        // motion (Task 124.3a's headline distinctness requirement).
        let start_pos = FreminalMousePosition::new_with_pixels(5, 5, 50, 50);
        let start = PreviousMouseState::new(
            PointerButton::Primary,
            false,
            start_pos,
            Modifiers::default(),
        );
        let mid_pos = FreminalMousePosition::new_with_pixels(5, 5, 52, 50);
        let mid =
            PreviousMouseState::new(PointerButton::Primary, false, mid_pos, Modifiers::default());
        let end_pos = FreminalMousePosition::new_with_pixels(5, 5, 55, 50);
        let end =
            PreviousMouseState::new(PointerButton::Primary, false, end_pos, Modifiers::default());

        let first = handle_pointer_moved(
            &mid,
            Some(&start),
            &MouseTrack::XtMseAny,
            &MouseEncoding::SgrPixels,
        )
        .expect("first sub-cell move should report");
        let second = handle_pointer_moved(
            &end,
            Some(&mid),
            &MouseTrack::XtMseAny,
            &MouseEncoding::SgrPixels,
        )
        .expect("second sub-cell move should report");

        let first_bytes = inputs_to_bytes(first.as_ref());
        let second_bytes = inputs_to_bytes(second.as_ref());
        assert_ne!(
            first_bytes, second_bytes,
            "two distinct sub-cell pixel positions must produce two distinct reports"
        );
    }

    #[test]
    fn sgr_cell_encoding_unaffected_by_pixel_fields() {
        // Regression guard: ordinary `?1006` SGR output must be byte-for-
        // byte unchanged now that `FreminalMousePosition` also carries
        // pixel fields — this pins that `Sgr`/`X11` never read them.
        let pos = FreminalMousePosition::new_with_pixels(4, 2, 999, 999);
        let state =
            PreviousMouseState::new(PointerButton::Primary, true, pos, Modifiers::default());
        let result = handle_pointer_button(
            PointerButton::Primary,
            &state,
            &MouseTrack::XtMseAny,
            &MouseEncoding::Sgr,
        )
        .expect("SGR button press should produce output");

        let bytes = inputs_to_bytes(result.as_ref());
        let expected = b"\x1b[<0;5;3M";
        assert_eq!(
            bytes, expected,
            "SGR cell-based encoding must ignore populated pixel fields"
        );
    }

    // ── Task 124.3a (review correction #3): first motion at the origin ──

    #[test]
    fn motion_position_changed_is_true_with_no_previous_position_even_at_the_origin() {
        // The headline regression: a genuine first motion landing at cell
        // (0, 0) must NOT compare equal to a "no previous position" state.
        let current = PreviousMouseState::new(
            PointerButton::Primary,
            false,
            FreminalMousePosition::new(0, 0),
            Modifiers::default(),
        );
        assert_eq!(
            motion_position_changed(None, &current, &MouseEncoding::Sgr),
            MotionPositionChange::Changed
        );
        assert_eq!(
            motion_position_changed(None, &current, &MouseEncoding::SgrPixels),
            MotionPositionChange::Changed
        );
    }

    #[test]
    fn motion_position_changed_with_a_real_previous_position_still_compares_normally() {
        let previous = PreviousMouseState::new(
            PointerButton::Primary,
            false,
            FreminalMousePosition::new(0, 0),
            Modifiers::default(),
        );
        let same = PreviousMouseState::new(
            PointerButton::Primary,
            false,
            FreminalMousePosition::new(0, 0),
            Modifiers::default(),
        );
        assert_eq!(
            motion_position_changed(Some(&previous), &same, &MouseEncoding::Sgr),
            MotionPositionChange::Unchanged,
            "identical real positions must still compare unchanged"
        );

        let moved = PreviousMouseState::new(
            PointerButton::Primary,
            false,
            FreminalMousePosition::new(1, 0),
            Modifiers::default(),
        );
        assert_eq!(
            motion_position_changed(Some(&previous), &moved, &MouseEncoding::Sgr),
            MotionPositionChange::Changed
        );
    }

    #[test]
    fn xtmseany_first_motion_at_cell_origin_reports_exact_bytes() {
        // Exact-byte regression: the very first `?1003` motion, landing at
        // cell (0, 0), must produce a real SGR report -- not be silently
        // dropped by a synthetic-zero-baseline comparison.
        let current = PreviousMouseState::new(
            PointerButton::Primary,
            false,
            FreminalMousePosition::new(0, 0),
            Modifiers::default(),
        );
        let result =
            handle_pointer_moved(&current, None, &MouseTrack::XtMseAny, &MouseEncoding::Sgr)
                .expect("first motion at the cell origin must report under XtMseAny");
        let bytes = inputs_to_bytes(result.as_ref());
        let s = std::str::from_utf8(&bytes).expect("SGR sequence must be valid UTF-8");
        // motion bit 32, button 0 (Primary, not held) -> cb=32; 1-based
        // coords (0+1, 0+1) = (1, 1). Lowercase 'm' since button not held.
        assert_eq!(s, "\x1b[<32;1;1m");
    }

    #[test]
    fn xtmseany_first_motion_at_pixel_origin_reports_exact_bytes() {
        // Same regression, at pixel granularity under SgrPixels.
        let current = PreviousMouseState::new(
            PointerButton::Primary,
            false,
            FreminalMousePosition::new_with_pixels(0, 0, 0, 0),
            Modifiers::default(),
        );
        let result = handle_pointer_moved(
            &current,
            None,
            &MouseTrack::XtMseAny,
            &MouseEncoding::SgrPixels,
        )
        .expect("first motion at the pixel origin must report under XtMseAny/SgrPixels");
        let bytes = inputs_to_bytes(result.as_ref());
        let s = std::str::from_utf8(&bytes).expect("SGR sequence must be valid UTF-8");
        assert_eq!(s, "\x1b[<32;1;1m");
    }

    #[test]
    fn xtmsebtn_first_motion_still_requires_a_held_button_even_at_the_origin() {
        // `motion_position_changed` being unconditionally `true` on the
        // first motion must not bypass the SEPARATE button-held gate for
        // `?1002` -- a first motion with no button held must still not
        // report.
        let current = PreviousMouseState::new(
            PointerButton::Primary,
            false,
            FreminalMousePosition::new(0, 0),
            Modifiers::default(),
        );
        let result =
            handle_pointer_moved(&current, None, &MouseTrack::XtMseBtn, &MouseEncoding::Sgr);
        assert!(
            result.is_none(),
            "?1002 must not report a first motion with no button held, got: {result:?}"
        );
    }
}
