// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

#![allow(clippy::unwrap_used)]

use freminal_common::buffer_states::mode::SetMode;
use freminal_common::buffer_states::modes::{
    MouseModeNumber, ReportMode,
    mouse::{MouseEncoding, MouseTrack},
};

// ---------------------------------------------------------------------------
// MouseEncoding — mode numbers
// ---------------------------------------------------------------------------

#[test]
fn test_encoding_mode_numbers() {
    assert_eq!(MouseEncoding::X11.mouse_mode_number(), 0);
    assert_eq!(MouseEncoding::Utf8.mouse_mode_number(), 1005);
    assert_eq!(MouseEncoding::Sgr.mouse_mode_number(), 1006);
    assert_eq!(MouseEncoding::SgrPixels.mouse_mode_number(), 1016);
}

// ---------------------------------------------------------------------------
// MouseEncoding — Display
// ---------------------------------------------------------------------------

#[test]
fn test_encoding_display() {
    assert_eq!(MouseEncoding::X11.to_string(), "X11");
    assert_eq!(MouseEncoding::Utf8.to_string(), "Utf8");
    assert_eq!(MouseEncoding::Sgr.to_string(), "Sgr");
    assert_eq!(MouseEncoding::SgrPixels.to_string(), "SgrPixels");
}

// ---------------------------------------------------------------------------
// MouseEncoding — Default
// ---------------------------------------------------------------------------

#[test]
fn test_encoding_default() {
    assert_eq!(MouseEncoding::default(), MouseEncoding::X11);
}

// ---------------------------------------------------------------------------
// MouseEncoding — report()
// ---------------------------------------------------------------------------

#[test]
fn test_encoding_report_x11_dec_set() {
    // i32::from(X11 != X11) == 0
    assert_eq!(
        MouseEncoding::X11.report(Some(SetMode::DecSet)),
        "\x1b[?0;0$y"
    );
}

#[test]
fn test_encoding_report_x11_dec_rst() {
    // X11 == X11 → branch returns 0
    assert_eq!(
        MouseEncoding::X11.report(Some(SetMode::DecRst)),
        "\x1b[?0;0$y"
    );
}

#[test]
fn test_encoding_report_x11_none() {
    // None treated same as DecRst; X11 == X11 → 0
    assert_eq!(MouseEncoding::X11.report(None), "\x1b[?0;0$y");
}

#[test]
fn test_encoding_report_sgr_dec_set() {
    // i32::from(Sgr != X11) == 1
    assert_eq!(
        MouseEncoding::Sgr.report(Some(SetMode::DecSet)),
        "\x1b[?1006;1$y"
    );
}

#[test]
fn test_encoding_report_sgr_dec_rst() {
    // Sgr != X11 → 2
    assert_eq!(
        MouseEncoding::Sgr.report(Some(SetMode::DecRst)),
        "\x1b[?1006;2$y"
    );
}

#[test]
fn test_encoding_report_sgr_dec_query() {
    assert_eq!(
        MouseEncoding::Sgr.report(Some(SetMode::DecQuery)),
        "\x1b[?1006;0$y"
    );
}

#[test]
fn test_encoding_report_utf8_dec_set() {
    // i32::from(Utf8 != X11) == 1
    assert_eq!(
        MouseEncoding::Utf8.report(Some(SetMode::DecSet)),
        "\x1b[?1005;1$y"
    );
}

#[test]
fn test_encoding_report_sgr_pixels_dec_set() {
    // i32::from(SgrPixels != X11) == 1
    assert_eq!(
        MouseEncoding::SgrPixels.report(Some(SetMode::DecSet)),
        "\x1b[?1016;1$y"
    );
}

// ---------------------------------------------------------------------------
// MouseTrack — mode numbers
// ---------------------------------------------------------------------------

#[test]
fn test_track_mode_numbers() {
    assert_eq!(MouseTrack::NoTracking.mouse_mode_number(), 0);
    assert_eq!(MouseTrack::XtMsex10.mouse_mode_number(), 9);
    assert_eq!(MouseTrack::XtMseX11.mouse_mode_number(), 1000);
    assert_eq!(MouseTrack::XtMseBtn.mouse_mode_number(), 1002);
    assert_eq!(MouseTrack::XtMseAny.mouse_mode_number(), 1003);
    assert_eq!(MouseTrack::Query(42).mouse_mode_number(), 42);
}

// ---------------------------------------------------------------------------
// MouseTrack — Display
// ---------------------------------------------------------------------------

#[test]
fn test_track_display() {
    assert_eq!(MouseTrack::NoTracking.to_string(), "NoTracking");
    assert_eq!(MouseTrack::XtMsex10.to_string(), "XtMsex10");
    assert_eq!(MouseTrack::XtMseX11.to_string(), "XtMseX11");
    assert_eq!(MouseTrack::XtMseBtn.to_string(), "XtMseBtn");
    assert_eq!(MouseTrack::XtMseAny.to_string(), "XtMseAny");
    assert_eq!(
        MouseTrack::Query(42).to_string(),
        "Query Mouse Tracking(42)"
    );
}

// ---------------------------------------------------------------------------
// MouseTrack — Default
// ---------------------------------------------------------------------------

#[test]
fn test_track_default() {
    assert_eq!(MouseTrack::default(), MouseTrack::NoTracking);
}

// ---------------------------------------------------------------------------
// MouseTrack — report()
// ---------------------------------------------------------------------------

#[test]
fn test_track_report_no_tracking_dec_set() {
    // NoTracking == NoTracking → i32::from(false) == 0
    assert_eq!(
        MouseTrack::NoTracking.report(Some(SetMode::DecSet)),
        "\x1b[?0;0$y"
    );
}

#[test]
fn test_track_report_no_tracking_dec_rst() {
    // NoTracking == NoTracking → 0
    assert_eq!(
        MouseTrack::NoTracking.report(Some(SetMode::DecRst)),
        "\x1b[?0;0$y"
    );
}

#[test]
fn test_track_report_xt_mse_x11_dec_set() {
    // XtMseX11 != NoTracking and != Query(1000) → i32::from(true) == 1
    assert_eq!(
        MouseTrack::XtMseX11.report(Some(SetMode::DecSet)),
        "\x1b[?1000;1$y"
    );
}

#[test]
fn test_track_report_xt_mse_x11_dec_rst() {
    // XtMseX11 is neither NoTracking nor Query(1000) → 2
    assert_eq!(
        MouseTrack::XtMseX11.report(Some(SetMode::DecRst)),
        "\x1b[?1000;2$y"
    );
}

#[test]
fn test_track_report_xt_mse_x11_none() {
    // None treated as DecRst; XtMseX11 is neither NoTracking nor Query → 2
    assert_eq!(MouseTrack::XtMseX11.report(None), "\x1b[?1000;2$y");
}

#[test]
fn test_track_report_xt_mse_x11_dec_query() {
    assert_eq!(
        MouseTrack::XtMseX11.report(Some(SetMode::DecQuery)),
        "\x1b[?1000;0$y"
    );
}

#[test]
fn test_track_report_query_1000_dec_set() {
    // mode_number == 1000; Query(1000) == Query(mode_number) → i32::from(false) == 0
    assert_eq!(
        MouseTrack::Query(1000).report(Some(SetMode::DecSet)),
        "\x1b[?1000;0$y"
    );
}

#[test]
fn test_track_report_query_1000_dec_rst() {
    // Query(1000) == Query(mode_number=1000) → 0
    assert_eq!(
        MouseTrack::Query(1000).report(Some(SetMode::DecRst)),
        "\x1b[?1000;0$y"
    );
}

#[test]
fn test_track_report_xt_mse_any_dec_set() {
    // XtMseAny != NoTracking and != Query(1003) → i32::from(true) == 1
    assert_eq!(
        MouseTrack::XtMseAny.report(Some(SetMode::DecSet)),
        "\x1b[?1003;1$y"
    );
}
