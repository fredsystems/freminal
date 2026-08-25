// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Task 124.3a: the out-of-frame immediate PTY mouse-report path.
//!
//! `FreminalGui::on_pointer_moved` (in `app_impl.rs`) resolves the active
//! pane's `ViewState`, `TerminalSnapshot`, `input_tx`, and published
//! [`PanePointerReportInputs`], plus the window-owned held-button state
//! (`PerWindowState::held_pointer_button`), then delegates here to decide
//! whether an immediate PTY mouse report should be sent for this pointer
//! event, and to send it. Mirrors the rest of `gui::terminal`'s convention
//! (see `dispatch_context_menu_action`'s doc in `widget.rs`) of receiving
//! explicit parameters rather than reaching into `FreminalGui` itself, so
//! every decision here is headlessly unit-testable.
//!
//! The held-button OBSERVATION itself (`App::on_pointer_button`) lives in
//! `gui::window` now, not here — see
//! `PerWindowState::held_pointer_button`'s doc for why button identity
//! moved to window ownership while this module keeps only the per-pane
//! report-position history.
//!
//! Repaint scheduling (`App::pointer_motion_needs_repaint`) is a
//! completely separate axis, untouched by this module — see
//! `Documents/PLAN_124_RENDER_EFFICIENCY.md` 124.3/124.3a/124.3b.

use crossbeam_channel::Sender;

use freminal_common::buffer_states::modes::mouse::MouseTrack;
use freminal_terminal_emulator::{input::KeyEventMeta, io::InputEvent, snapshot::TerminalSnapshot};

use crate::gui::mouse::{PreviousMouseState, handle_pointer_moved};
use crate::gui::published_frame_state::PanePointerReportInputs;
use crate::gui::view_state::ViewState;

use super::coords::encode_egui_mouse_pos;
use super::input::{InputModes, send_terminal_inputs};

/// Task 124.3a (review correction #6): whether `inputs`' cell dimensions
/// and pixels-per-point are safe to feed into [`encode_egui_mouse_pos`].
///
/// Zero, negative, NaN, or infinite cell dimensions would divide the
/// pointer position by zero or a non-finite value inside the cell-index
/// computation; a non-finite or non-positive `pixels_per_point` would do
/// the same to the physical-pixel computation. Rather than let either flow
/// into that conversion's own clamping fallback (designed for "a real but
/// out-of-range value", not "the geometry itself is nonsensical") and
/// silently emit a report at a bogus synthetic position, this rejects the
/// frame's published geometry outright — conservative, exactly like every
/// other early-return in [`maybe_send_immediate_motion_report`].
fn has_valid_report_geometry(inputs: &PanePointerReportInputs) -> bool {
    inputs.cell_size.x.is_finite()
        && inputs.cell_size.x > 0.0
        && inputs.cell_size.y.is_finite()
        && inputs.cell_size.y > 0.0
        && inputs.pixels_per_point.is_finite()
        && inputs.pixels_per_point > 0.0
}

/// Task 124.3a: decide whether to send an immediate PTY motion report for
/// this pointer position, and send it if so.
///
/// `report_inputs` is `None` when this pane has not yet published a
/// geometry/suppressor snapshot (no frame has rendered it yet, or the
/// active pane resolution failed upstream) — conservative: no report.
/// Every other early-return below is equally conservative and mirrors the
/// exact gates `write_input_to_terminal`'s old frame-time `PointerMoved`
/// arm applied before Task 124.3a moved report delivery out of it:
/// scrolled-back suppression, every `InputSuppressors` field, the
/// terminal-rect containment check, `MouseTrack::NoTracking`, and (review
/// correction #6) invalid published geometry.
///
/// `held_button` is the WINDOW-owned physical button state
/// (`PerWindowState::held_pointer_button`) — see that field's doc for why
/// button identity is not tracked per-pane. `None` means no button is
/// currently held, which `PreviousMouseState::new`'s `button` field
/// defaults to `Primary` for (matching its own `Default` impl) since the
/// identity is meaningless when nothing is held.
pub(in crate::gui) fn maybe_send_immediate_motion_report(
    view_state: &mut ViewState,
    snap: &TerminalSnapshot,
    input_tx: &Sender<InputEvent>,
    report_inputs: Option<PanePointerReportInputs>,
    held_button: Option<egui::PointerButton>,
    pos: egui::Pos2,
    modifiers: egui::Modifiers,
) {
    let Some(report_inputs) = report_inputs else {
        return;
    };

    // A scrolled-back pane must not emit reports for content that is not
    // live — standard terminal-emulator behavior, mirrored from the old
    // frame-time gate (`effective_mouse_tracking` in `write_input_to_terminal`).
    if view_state.scroll_offset > 0 {
        return;
    }

    if report_inputs.any_suppressor() {
        return;
    }

    if !report_inputs.terminal_rect.contains(pos) {
        return;
    }

    if snap.mouse_tracking == MouseTrack::NoTracking {
        return;
    }

    if !has_valid_report_geometry(&report_inputs) {
        return;
    }

    let position = encode_egui_mouse_pos(
        pos,
        (report_inputs.cell_size.x, report_inputs.cell_size.y),
        report_inputs.terminal_rect.min,
        report_inputs.pixels_per_point,
    );

    let button = held_button.unwrap_or(egui::PointerButton::Primary);
    let button_held = held_button.is_some();

    let report = &mut view_state.immediate_mouse_report;
    // Task 124.3a (review correction #3): `report.position` is `None`
    // before the first observed motion in this pane — that must produce an
    // unambiguous "changed" answer even when `position` itself is the
    // cell/pixel origin, so no synthetic `(0, 0)` `PreviousMouseState` is
    // built here. See `motion_position_changed`'s doc.
    let previous = report.position.as_ref().map(|prev_position| {
        PreviousMouseState::new(button, button_held, *prev_position, modifiers)
    });
    let current = PreviousMouseState::new(button, button_held, position, modifiers);

    if let Some(bytes) = handle_pointer_moved(
        &current,
        previous.as_ref(),
        &snap.mouse_tracking,
        &snap.mouse_encoding,
    ) {
        let modes = InputModes::from_snapshot(snap);
        send_terminal_inputs(bytes.as_ref(), input_tx, &modes, &KeyEventMeta::PRESS);
    }

    report.position = Some(position);
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use freminal_common::buffer_states::modes::mouse::MouseEncoding;

    /// A minimal live `Receiver`/`Sender` pair plus the pieces
    /// `maybe_send_immediate_motion_report` needs, constructible headlessly
    /// (no `FreminalGui`/windowing types required).
    struct Fixture {
        view_state: ViewState,
        snap: TerminalSnapshot,
        input_tx: Sender<InputEvent>,
        input_rx: crossbeam_channel::Receiver<InputEvent>,
    }

    fn fixture(mouse_track: MouseTrack, encoding: MouseEncoding) -> Fixture {
        let (input_tx, input_rx) = crossbeam_channel::unbounded();
        let mut snap = TerminalSnapshot::empty();
        snap.mouse_tracking = mouse_track;
        snap.mouse_encoding = encoding;
        Fixture {
            view_state: ViewState::new(),
            snap,
            input_tx,
            input_rx,
        }
    }

    /// A published geometry snapshot covering a 100x100 logical-point
    /// terminal area at the window origin, 8x16 cells, unity pixel scale,
    /// no suppressors active.
    fn clean_report_inputs() -> PanePointerReportInputs {
        PanePointerReportInputs {
            terminal_rect: egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(100.0, 100.0),
            ),
            cell_size: egui::vec2(8.0, 16.0),
            pixels_per_point: 1.0,
            modal_or_drag: false,
            context_menu: false,
            search_overlay: false,
            command_history: false,
            scrollbar_drag: false,
            scrollbar_hit_rect: None,
        }
    }

    fn sent_bytes(rx: &crossbeam_channel::Receiver<InputEvent>) -> Option<Vec<u8>> {
        match rx.try_recv() {
            Ok(InputEvent::Key(bytes)) => Some(bytes),
            Ok(_) | Err(_) => None,
        }
    }

    // ── Conservative-fallback tests ───────────────────────────────────

    #[test]
    fn unpublished_geometry_sends_nothing() {
        let mut f = fixture(MouseTrack::XtMseAny, MouseEncoding::Sgr);
        maybe_send_immediate_motion_report(
            &mut f.view_state,
            &f.snap,
            &f.input_tx,
            None,
            None,
            egui::pos2(10.0, 10.0),
            egui::Modifiers::default(),
        );
        assert_eq!(sent_bytes(&f.input_rx), None);
    }

    #[test]
    fn scrolled_back_sends_nothing() {
        let mut f = fixture(MouseTrack::XtMseAny, MouseEncoding::Sgr);
        f.view_state.scroll_offset = 1;
        maybe_send_immediate_motion_report(
            &mut f.view_state,
            &f.snap,
            &f.input_tx,
            Some(clean_report_inputs()),
            None,
            egui::pos2(10.0, 10.0),
            egui::Modifiers::default(),
        );
        assert_eq!(sent_bytes(&f.input_rx), None);
    }

    #[test]
    fn each_suppressor_individually_sends_nothing() {
        for field in [
            "modal_or_drag",
            "context_menu",
            "search_overlay",
            "command_history",
            "scrollbar_drag",
        ] {
            let mut f = fixture(MouseTrack::XtMseAny, MouseEncoding::Sgr);
            let mut inputs = clean_report_inputs();
            match field {
                "modal_or_drag" => inputs.modal_or_drag = true,
                "context_menu" => inputs.context_menu = true,
                "search_overlay" => inputs.search_overlay = true,
                "command_history" => inputs.command_history = true,
                "scrollbar_drag" => inputs.scrollbar_drag = true,
                _ => unreachable!(),
            }
            maybe_send_immediate_motion_report(
                &mut f.view_state,
                &f.snap,
                &f.input_tx,
                Some(inputs),
                None,
                egui::pos2(10.0, 10.0),
                egui::Modifiers::default(),
            );
            assert_eq!(
                sent_bytes(&f.input_rx),
                None,
                "suppressor {field} must block delivery"
            );
        }
    }

    #[test]
    fn position_outside_terminal_rect_sends_nothing() {
        let mut f = fixture(MouseTrack::XtMseAny, MouseEncoding::Sgr);
        maybe_send_immediate_motion_report(
            &mut f.view_state,
            &f.snap,
            &f.input_tx,
            Some(clean_report_inputs()),
            None,
            egui::pos2(500.0, 500.0),
            egui::Modifiers::default(),
        );
        assert_eq!(sent_bytes(&f.input_rx), None);
    }

    #[test]
    fn no_tracking_sends_nothing() {
        let mut f = fixture(MouseTrack::NoTracking, MouseEncoding::Sgr);
        maybe_send_immediate_motion_report(
            &mut f.view_state,
            &f.snap,
            &f.input_tx,
            Some(clean_report_inputs()),
            None,
            egui::pos2(10.0, 10.0),
            egui::Modifiers::default(),
        );
        assert_eq!(sent_bytes(&f.input_rx), None);
    }

    // ── Task 124.3a (review correction #6): geometry validation ────────

    #[test]
    fn zero_cell_size_sends_nothing() {
        let mut f = fixture(MouseTrack::XtMseAny, MouseEncoding::Sgr);
        let inputs = PanePointerReportInputs {
            cell_size: egui::vec2(0.0, 16.0),
            ..clean_report_inputs()
        };
        maybe_send_immediate_motion_report(
            &mut f.view_state,
            &f.snap,
            &f.input_tx,
            Some(inputs),
            None,
            egui::pos2(10.0, 10.0),
            egui::Modifiers::default(),
        );
        assert_eq!(sent_bytes(&f.input_rx), None);
    }

    #[test]
    fn negative_cell_size_sends_nothing() {
        let mut f = fixture(MouseTrack::XtMseAny, MouseEncoding::Sgr);
        let inputs = PanePointerReportInputs {
            cell_size: egui::vec2(8.0, -16.0),
            ..clean_report_inputs()
        };
        maybe_send_immediate_motion_report(
            &mut f.view_state,
            &f.snap,
            &f.input_tx,
            Some(inputs),
            None,
            egui::pos2(10.0, 10.0),
            egui::Modifiers::default(),
        );
        assert_eq!(sent_bytes(&f.input_rx), None);
    }

    #[test]
    fn nan_cell_size_sends_nothing() {
        let mut f = fixture(MouseTrack::XtMseAny, MouseEncoding::Sgr);
        let inputs = PanePointerReportInputs {
            cell_size: egui::vec2(f32::NAN, 16.0),
            ..clean_report_inputs()
        };
        maybe_send_immediate_motion_report(
            &mut f.view_state,
            &f.snap,
            &f.input_tx,
            Some(inputs),
            None,
            egui::pos2(10.0, 10.0),
            egui::Modifiers::default(),
        );
        assert_eq!(sent_bytes(&f.input_rx), None);
    }

    #[test]
    fn infinite_cell_size_sends_nothing() {
        let mut f = fixture(MouseTrack::XtMseAny, MouseEncoding::Sgr);
        let inputs = PanePointerReportInputs {
            cell_size: egui::vec2(f32::INFINITY, 16.0),
            ..clean_report_inputs()
        };
        maybe_send_immediate_motion_report(
            &mut f.view_state,
            &f.snap,
            &f.input_tx,
            Some(inputs),
            None,
            egui::pos2(10.0, 10.0),
            egui::Modifiers::default(),
        );
        assert_eq!(sent_bytes(&f.input_rx), None);
    }

    #[test]
    fn zero_pixels_per_point_sends_nothing() {
        let mut f = fixture(MouseTrack::XtMseAny, MouseEncoding::SgrPixels);
        let inputs = PanePointerReportInputs {
            pixels_per_point: 0.0,
            ..clean_report_inputs()
        };
        maybe_send_immediate_motion_report(
            &mut f.view_state,
            &f.snap,
            &f.input_tx,
            Some(inputs),
            None,
            egui::pos2(10.0, 10.0),
            egui::Modifiers::default(),
        );
        assert_eq!(sent_bytes(&f.input_rx), None);
    }

    #[test]
    fn negative_pixels_per_point_sends_nothing() {
        let mut f = fixture(MouseTrack::XtMseAny, MouseEncoding::SgrPixels);
        let inputs = PanePointerReportInputs {
            pixels_per_point: -1.0,
            ..clean_report_inputs()
        };
        maybe_send_immediate_motion_report(
            &mut f.view_state,
            &f.snap,
            &f.input_tx,
            Some(inputs),
            None,
            egui::pos2(10.0, 10.0),
            egui::Modifiers::default(),
        );
        assert_eq!(sent_bytes(&f.input_rx), None);
    }

    #[test]
    fn nan_pixels_per_point_sends_nothing() {
        let mut f = fixture(MouseTrack::XtMseAny, MouseEncoding::SgrPixels);
        let inputs = PanePointerReportInputs {
            pixels_per_point: f32::NAN,
            ..clean_report_inputs()
        };
        maybe_send_immediate_motion_report(
            &mut f.view_state,
            &f.snap,
            &f.input_tx,
            Some(inputs),
            None,
            egui::pos2(10.0, 10.0),
            egui::Modifiers::default(),
        );
        assert_eq!(sent_bytes(&f.input_rx), None);
    }

    #[test]
    fn infinite_pixels_per_point_sends_nothing() {
        let mut f = fixture(MouseTrack::XtMseAny, MouseEncoding::SgrPixels);
        let inputs = PanePointerReportInputs {
            pixels_per_point: f32::INFINITY,
            ..clean_report_inputs()
        };
        maybe_send_immediate_motion_report(
            &mut f.view_state,
            &f.snap,
            &f.input_tx,
            Some(inputs),
            None,
            egui::pos2(10.0, 10.0),
            egui::Modifiers::default(),
        );
        assert_eq!(sent_bytes(&f.input_rx), None);
    }

    #[test]
    fn valid_geometry_with_unity_scale_still_reports() {
        // Regression guard for the geometry-validation gate itself: a
        // perfectly ordinary frame (matching `clean_report_inputs`) must
        // not be caught by the new finite/positive checks.
        let mut f = fixture(MouseTrack::XtMseAny, MouseEncoding::Sgr);
        maybe_send_immediate_motion_report(
            &mut f.view_state,
            &f.snap,
            &f.input_tx,
            Some(clean_report_inputs()),
            None,
            egui::pos2(10.0, 10.0),
            egui::Modifiers::default(),
        );
        assert!(sent_bytes(&f.input_rx).is_some());
    }

    // ── Delivery tests ─────────────────────────────────────────────────

    #[test]
    fn xtmseany_reports_plain_motion_regardless_of_button_state() {
        let mut f = fixture(MouseTrack::XtMseAny, MouseEncoding::Sgr);
        maybe_send_immediate_motion_report(
            &mut f.view_state,
            &f.snap,
            &f.input_tx,
            Some(clean_report_inputs()),
            None,
            egui::pos2(16.0, 16.0), // cell (2, 1) at 8x16 cells
            egui::Modifiers::default(),
        );
        let bytes = sent_bytes(&f.input_rx).expect("XtMseAny motion must report");
        let s = std::str::from_utf8(&bytes).expect("valid UTF-8");
        // motion bit 32, button 0 (not held), cell (2+1=3, 1+1=2).
        assert_eq!(s, "\x1b[<32;3;2m");
    }

    #[test]
    fn xtmsebtn_reports_only_while_a_button_is_held() {
        let mut f = fixture(MouseTrack::XtMseBtn, MouseEncoding::Sgr);

        // No button held -> no report.
        maybe_send_immediate_motion_report(
            &mut f.view_state,
            &f.snap,
            &f.input_tx,
            Some(clean_report_inputs()),
            None,
            egui::pos2(16.0, 16.0),
            egui::Modifiers::default(),
        );
        assert_eq!(
            sent_bytes(&f.input_rx),
            None,
            "?1002 must not report without a held button"
        );

        // Hold the primary button (as `app_impl.rs` would pass in from
        // `PerWindowState::held_pointer_button`), then move again.
        maybe_send_immediate_motion_report(
            &mut f.view_state,
            &f.snap,
            &f.input_tx,
            Some(clean_report_inputs()),
            Some(egui::PointerButton::Primary),
            egui::pos2(24.0, 16.0), // moved one cell right
            egui::Modifiers::default(),
        );
        let bytes = sent_bytes(&f.input_rx).expect("?1002 must report while a button is held");
        let s = std::str::from_utf8(&bytes).expect("valid UTF-8");
        assert!(s.starts_with("\x1b[<"), "expected SGR framing, got {s:?}");
    }

    #[test]
    fn sgr_pixels_two_sub_cell_moves_produce_two_distinct_reports_and_no_third_repeat() {
        let mut f = fixture(MouseTrack::XtMseAny, MouseEncoding::SgrPixels);

        maybe_send_immediate_motion_report(
            &mut f.view_state,
            &f.snap,
            &f.input_tx,
            Some(clean_report_inputs()),
            None,
            egui::pos2(10.0, 10.0),
            egui::Modifiers::default(),
        );
        let first = sent_bytes(&f.input_rx).expect("first sub-cell move should report");

        maybe_send_immediate_motion_report(
            &mut f.view_state,
            &f.snap,
            &f.input_tx,
            Some(clean_report_inputs()),
            None,
            egui::pos2(11.0, 10.0), // same cell (8x16), different pixel
            egui::Modifiers::default(),
        );
        let second = sent_bytes(&f.input_rx).expect("second sub-cell move should also report");
        assert_ne!(
            first, second,
            "distinct sub-cell pixel positions must produce distinct reports"
        );

        // Repeating the exact same position produces no third report (no
        // spurious duplicate at a stationary pointer).
        maybe_send_immediate_motion_report(
            &mut f.view_state,
            &f.snap,
            &f.input_tx,
            Some(clean_report_inputs()),
            None,
            egui::pos2(11.0, 10.0),
            egui::Modifiers::default(),
        );
        assert_eq!(
            sent_bytes(&f.input_rx),
            None,
            "an unchanged position must not produce a repeat report"
        );
    }

    // ── Task 124.3a (review correction #3): first motion at the origin ──

    #[test]
    fn first_motion_landing_at_the_cell_origin_still_reports() {
        // Regression: the very first call on a fresh `ViewState`
        // (`immediate_mouse_report.position == None`) landing exactly at
        // cell (0, 0) must still report -- it must not compare equal to a
        // synthetic "no previous position" baseline.
        let mut f = fixture(MouseTrack::XtMseAny, MouseEncoding::Sgr);
        assert_eq!(f.view_state.immediate_mouse_report.position, None);

        maybe_send_immediate_motion_report(
            &mut f.view_state,
            &f.snap,
            &f.input_tx,
            Some(clean_report_inputs()),
            None,
            egui::pos2(0.0, 0.0),
            egui::Modifiers::default(),
        );
        let bytes = sent_bytes(&f.input_rx)
            .expect("the first motion at the origin must report, not be suppressed");
        let s = std::str::from_utf8(&bytes).expect("valid UTF-8");
        assert_eq!(s, "\x1b[<32;1;1m");
    }

    #[test]
    fn first_motion_landing_at_the_pixel_origin_still_reports() {
        let mut f = fixture(MouseTrack::XtMseAny, MouseEncoding::SgrPixels);
        maybe_send_immediate_motion_report(
            &mut f.view_state,
            &f.snap,
            &f.input_tx,
            Some(clean_report_inputs()),
            None,
            egui::pos2(0.0, 0.0),
            egui::Modifiers::default(),
        );
        let bytes = sent_bytes(&f.input_rx)
            .expect("the first pixel-granular motion at the origin must report");
        let s = std::str::from_utf8(&bytes).expect("valid UTF-8");
        assert_eq!(s, "\x1b[<32;1;1m");
    }

    // ── Task 124.3a (review correction #2): first report after a
    // pointer-presence-loss reset ─────────────────────────────────────

    #[test]
    fn first_report_after_reset_is_unambiguous_even_at_a_previously_reported_position() {
        // Simulates the full sequence `App::on_pointer_presence_lost`
        // exists to protect: a report goes out for position `pos`, the
        // SAME position is then a no-op (unchanged), presence is lost
        // (`ImmediateMouseReportState::reset`, as `on_pointer_presence_lost`
        // calls), and the pointer returns to the EXACT SAME `pos` — which
        // must still report, because reset cleared the baseline entirely
        // rather than merely repeating the last-known position.
        let mut f = fixture(MouseTrack::XtMseAny, MouseEncoding::Sgr);
        let pos = egui::pos2(16.0, 16.0);

        maybe_send_immediate_motion_report(
            &mut f.view_state,
            &f.snap,
            &f.input_tx,
            Some(clean_report_inputs()),
            None,
            pos,
            egui::Modifiers::default(),
        );
        assert!(
            sent_bytes(&f.input_rx).is_some(),
            "the first motion to `pos` must report"
        );

        maybe_send_immediate_motion_report(
            &mut f.view_state,
            &f.snap,
            &f.input_tx,
            Some(clean_report_inputs()),
            None,
            pos,
            egui::Modifiers::default(),
        );
        assert_eq!(
            sent_bytes(&f.input_rx),
            None,
            "without a reset, repeating the identical position must not report again"
        );

        // Pointer presence lost — `App::on_pointer_presence_lost` would
        // call this on the active pane's `ImmediateMouseReportState`.
        f.view_state.immediate_mouse_report.reset();

        maybe_send_immediate_motion_report(
            &mut f.view_state,
            &f.snap,
            &f.input_tx,
            Some(clean_report_inputs()),
            None,
            pos,
            egui::Modifiers::default(),
        );
        assert!(
            sent_bytes(&f.input_rx).is_some(),
            "after reset, the identical position must be treated as a fresh \
             first motion and report again -- not be compared against the \
             pre-loss baseline"
        );
    }
}
