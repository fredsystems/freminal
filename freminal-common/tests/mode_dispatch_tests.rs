// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

#![allow(clippy::unwrap_used)]

use freminal_common::buffer_states::mode::{Mode, SetMode};
use freminal_common::buffer_states::modes::{
    allow_alt_screen::AllowAltScreen,
    allow_column_mode_switch::AllowColumnModeSwitch,
    alternate_scroll::AlternateScroll,
    decanm::Decanm,
    decarm::Decarm,
    decawm::Decawm,
    decbkm::Decbkm,
    decckm::Decckm,
    deccolm::Deccolm,
    decnkm::Decnkm,
    decnrcm::Decnrcm,
    decom::Decom,
    decsclm::Decsclm,
    decscnm::Decscnm,
    decsdm::Decsdm,
    dectcem::Dectcem,
    grapheme::GraphemeClustering,
    lnm::Lnm,
    mouse::{MouseEncoding, MouseTrack},
    private_color_registers::PrivateColorRegisters,
    reverse_wrap_around::ReverseWrapAround,
    rl_bracket::RlBracket,
    sync_updates::SynchronizedUpdates,
    theme::Theming,
    unknown::UnknownMode,
    xt_rev_wrap2::XtRevWrap2,
    xtcblink::XtCBlink,
    xtextscrn::{AltScreen47, SaveCursor1048, XtExtscrn},
    xtmsewin::XtMseWin,
};

// ---------------------------------------------------------------------------
// Helper
// ---------------------------------------------------------------------------

fn dispatch(params: &[u8], mode: SetMode) -> Mode {
    Mode::terminal_mode_from_params(params, mode)
}

// ---------------------------------------------------------------------------
// Group 1: Known params with DecSet — every entry in the dispatch table
// ---------------------------------------------------------------------------

#[test]
fn decset_q1_returns_decckm_application() {
    assert_eq!(
        dispatch(b"?1", SetMode::DecSet),
        Mode::Decckm(Decckm::Application)
    );
}

#[test]
fn decset_q3_returns_deccolm_column132() {
    assert_eq!(
        dispatch(b"?3", SetMode::DecSet),
        Mode::Deccolm(Deccolm::Column132)
    );
}

#[test]
fn decset_q4_returns_decsclm_smooth_scroll() {
    assert_eq!(
        dispatch(b"?4", SetMode::DecSet),
        Mode::Decsclm(Decsclm::SmoothScroll)
    );
}

#[test]
fn decset_q5_returns_decscnm_reverse_display() {
    assert_eq!(
        dispatch(b"?5", SetMode::DecSet),
        Mode::Decscnm(Decscnm::ReverseDisplay)
    );
}

#[test]
fn decset_q6_returns_decom_origin_mode() {
    assert_eq!(
        dispatch(b"?6", SetMode::DecSet),
        Mode::Decom(Decom::OriginMode)
    );
}

#[test]
fn decset_q7_returns_decawm_auto_wrap() {
    assert_eq!(
        dispatch(b"?7", SetMode::DecSet),
        Mode::Decawm(Decawm::AutoWrap)
    );
}

#[test]
fn decset_q8_returns_decarm_repeat_key() {
    assert_eq!(
        dispatch(b"?8", SetMode::DecSet),
        Mode::Decarm(Decarm::RepeatKey)
    );
}

#[test]
fn decset_q9_returns_mouse_mode_xtmsex10() {
    assert_eq!(
        dispatch(b"?9", SetMode::DecSet),
        Mode::MouseMode(MouseTrack::XtMsex10)
    );
}

#[test]
fn decset_q12_returns_xtcblink_blinking() {
    assert_eq!(
        dispatch(b"?12", SetMode::DecSet),
        Mode::XtCBlink(XtCBlink::Blinking)
    );
}

#[test]
fn decset_20_no_prefix_returns_line_feed_mode_new_line() {
    assert_eq!(
        dispatch(b"20", SetMode::DecSet),
        Mode::LineFeedMode(Lnm::NewLine)
    );
}

#[test]
fn decset_q25_returns_dectem_show() {
    assert_eq!(
        dispatch(b"?25", SetMode::DecSet),
        Mode::Dectem(Dectcem::Show)
    );
}

#[test]
fn decset_q40_returns_allow_column_mode_switch() {
    assert_eq!(
        dispatch(b"?40", SetMode::DecSet),
        Mode::AllowColumnModeSwitch(AllowColumnModeSwitch::AllowColumnModeSwitch)
    );
}

#[test]
fn decset_q45_returns_reverse_wrap_around_wrap_around() {
    assert_eq!(
        dispatch(b"?45", SetMode::DecSet),
        Mode::ReverseWrapAround(ReverseWrapAround::WrapAround)
    );
}

#[test]
fn decset_q66_returns_decnkm_application() {
    assert_eq!(
        dispatch(b"?66", SetMode::DecSet),
        Mode::Decnkm(Decnkm::Application)
    );
}

#[test]
fn decrst_q66_returns_decnkm_numeric() {
    assert_eq!(
        dispatch(b"?66", SetMode::DecRst),
        Mode::Decnkm(Decnkm::Numeric)
    );
}

#[test]
fn decquery_q66_returns_decnkm_query() {
    assert_eq!(
        dispatch(b"?66", SetMode::DecQuery),
        Mode::Decnkm(Decnkm::Query)
    );
}

#[test]
fn decset_q67_returns_decbkm_backarrow_sends_bs() {
    assert_eq!(
        dispatch(b"?67", SetMode::DecSet),
        Mode::Decbkm(Decbkm::BackarrowSendsBs)
    );
}

#[test]
fn decrst_q67_returns_decbkm_backarrow_sends_del() {
    assert_eq!(
        dispatch(b"?67", SetMode::DecRst),
        Mode::Decbkm(Decbkm::BackarrowSendsDel)
    );
}

#[test]
fn decquery_q67_returns_decbkm_query() {
    assert_eq!(
        dispatch(b"?67", SetMode::DecQuery),
        Mode::Decbkm(Decbkm::Query)
    );
}

#[test]
fn decset_q1000_returns_mouse_mode_xtmsex11() {
    assert_eq!(
        dispatch(b"?1000", SetMode::DecSet),
        Mode::MouseMode(MouseTrack::XtMseX11)
    );
}

#[test]
fn decset_q1002_returns_mouse_mode_xtmsebtn() {
    assert_eq!(
        dispatch(b"?1002", SetMode::DecSet),
        Mode::MouseMode(MouseTrack::XtMseBtn)
    );
}

#[test]
fn decset_q1003_returns_mouse_mode_xtmseany() {
    assert_eq!(
        dispatch(b"?1003", SetMode::DecSet),
        Mode::MouseMode(MouseTrack::XtMseAny)
    );
}

#[test]
fn decset_q1004_returns_xtmsewin_enabled() {
    assert_eq!(
        dispatch(b"?1004", SetMode::DecSet),
        Mode::XtMseWin(XtMseWin::Enabled)
    );
}

#[test]
fn decset_q1005_returns_mouse_encoding_mode_utf8() {
    assert_eq!(
        dispatch(b"?1005", SetMode::DecSet),
        Mode::MouseEncodingMode(MouseEncoding::Utf8)
    );
}

#[test]
fn decset_q1006_returns_mouse_encoding_mode_sgr() {
    assert_eq!(
        dispatch(b"?1006", SetMode::DecSet),
        Mode::MouseEncodingMode(MouseEncoding::Sgr)
    );
}

#[test]
fn decset_q1016_returns_mouse_encoding_mode_sgr_pixels() {
    assert_eq!(
        dispatch(b"?1016", SetMode::DecSet),
        Mode::MouseEncodingMode(MouseEncoding::SgrPixels)
    );
}

#[test]
fn decset_q1049_returns_xtextscrn_alternate() {
    assert_eq!(
        dispatch(b"?1049", SetMode::DecSet),
        Mode::XtExtscrn(XtExtscrn::Alternate)
    );
}

#[test]
fn decset_q47_returns_altscreen47_alternate() {
    assert_eq!(
        dispatch(b"?47", SetMode::DecSet),
        Mode::AltScreen47(AltScreen47::Alternate)
    );
}

#[test]
fn decset_q1047_returns_altscreen47_alternate() {
    assert_eq!(
        dispatch(b"?1047", SetMode::DecSet),
        Mode::AltScreen47(AltScreen47::Alternate)
    );
}

#[test]
fn decset_q1048_returns_save_cursor1048_save() {
    assert_eq!(
        dispatch(b"?1048", SetMode::DecSet),
        Mode::SaveCursor1048(SaveCursor1048::Save)
    );
}

#[test]
fn decset_q2004_returns_bracketed_paste_enabled() {
    assert_eq!(
        dispatch(b"?2004", SetMode::DecSet),
        Mode::BracketedPaste(RlBracket::Enabled)
    );
}

#[test]
fn decset_q2026_returns_synchronized_updates_dont_draw() {
    assert_eq!(
        dispatch(b"?2026", SetMode::DecSet),
        Mode::SynchronizedUpdates(SynchronizedUpdates::DontDraw)
    );
}

#[test]
fn decset_q2027_returns_grapheme_clustering_legacy() {
    assert_eq!(
        dispatch(b"?2027", SetMode::DecSet),
        Mode::GraphemeClustering(GraphemeClustering::Legacy)
    );
}

#[test]
fn decset_q2031_returns_theming_light() {
    assert_eq!(
        dispatch(b"?2031", SetMode::DecSet),
        Mode::Theming(Theming::Light)
    );
}

// ---------------------------------------------------------------------------
// Group 2: Known params with DecRst — spot-checks
// ---------------------------------------------------------------------------

#[test]
fn decrst_q1_returns_decckm_ansi() {
    assert_eq!(dispatch(b"?1", SetMode::DecRst), Mode::Decckm(Decckm::Ansi));
}

#[test]
fn decrst_q9_returns_mouse_mode_no_tracking() {
    assert_eq!(
        dispatch(b"?9", SetMode::DecRst),
        Mode::MouseMode(MouseTrack::NoTracking)
    );
}

#[test]
fn decrst_q1006_returns_mouse_encoding_mode_x11() {
    // mouse_encoding_mode resets to X11 on DecRst
    assert_eq!(
        dispatch(b"?1006", SetMode::DecRst),
        Mode::MouseEncodingMode(MouseEncoding::X11)
    );
}

#[test]
fn decrst_20_no_prefix_returns_line_feed_mode_line_feed() {
    assert_eq!(
        dispatch(b"20", SetMode::DecRst),
        Mode::LineFeedMode(Lnm::LineFeed)
    );
}

#[test]
fn decrst_q2004_returns_bracketed_paste_disabled() {
    assert_eq!(
        dispatch(b"?2004", SetMode::DecRst),
        Mode::BracketedPaste(RlBracket::Disabled)
    );
}

#[test]
fn decrst_q1000_returns_mouse_mode_no_tracking() {
    assert_eq!(
        dispatch(b"?1000", SetMode::DecRst),
        Mode::MouseMode(MouseTrack::NoTracking)
    );
}

#[test]
fn decrst_q2027_returns_grapheme_clustering_unicode() {
    assert_eq!(
        dispatch(b"?2027", SetMode::DecRst),
        Mode::GraphemeClustering(GraphemeClustering::Unicode)
    );
}

// ---------------------------------------------------------------------------
// Group 3: Known params with DecQuery
// ---------------------------------------------------------------------------

#[test]
fn decquery_q1_returns_decckm_query() {
    assert_eq!(
        dispatch(b"?1", SetMode::DecQuery),
        Mode::Decckm(Decckm::Query)
    );
}

#[test]
fn decquery_q9_returns_mouse_mode_query_9() {
    // mouse_mode with DecQuery returns MouseMode(Query(id))
    assert_eq!(
        dispatch(b"?9", SetMode::DecQuery),
        Mode::MouseMode(MouseTrack::Query(9))
    );
}

#[test]
fn decquery_q1000_returns_mouse_mode_query_1000() {
    assert_eq!(
        dispatch(b"?1000", SetMode::DecQuery),
        Mode::MouseMode(MouseTrack::Query(1000))
    );
}

#[test]
fn decquery_q1002_returns_mouse_mode_query_1002() {
    assert_eq!(
        dispatch(b"?1002", SetMode::DecQuery),
        Mode::MouseMode(MouseTrack::Query(1002))
    );
}

#[test]
fn decquery_q1003_returns_mouse_mode_query_1003() {
    assert_eq!(
        dispatch(b"?1003", SetMode::DecQuery),
        Mode::MouseMode(MouseTrack::Query(1003))
    );
}

#[test]
fn decquery_q1005_returns_mouse_mode_query_1005_not_encoding() {
    // Quirk: mouse_encoding_mode with DecQuery returns MouseMode(Query(id)),
    // NOT MouseEncodingMode.
    assert_eq!(
        dispatch(b"?1005", SetMode::DecQuery),
        Mode::MouseMode(MouseTrack::Query(1005))
    );
}

#[test]
fn decquery_q1006_returns_mouse_mode_query_1006_not_encoding() {
    // Same quirk for ?1006.
    assert_eq!(
        dispatch(b"?1006", SetMode::DecQuery),
        Mode::MouseMode(MouseTrack::Query(1006))
    );
}

#[test]
fn decquery_q1016_returns_mouse_mode_query_1016_not_encoding() {
    // Same quirk for ?1016.
    assert_eq!(
        dispatch(b"?1016", SetMode::DecQuery),
        Mode::MouseMode(MouseTrack::Query(1016))
    );
}

#[test]
fn decquery_q25_returns_dectem_query() {
    assert_eq!(
        dispatch(b"?25", SetMode::DecQuery),
        Mode::Dectem(Dectcem::Query)
    );
}

#[test]
fn decquery_q2004_returns_bracketed_paste_query() {
    assert_eq!(
        dispatch(b"?2004", SetMode::DecQuery),
        Mode::BracketedPaste(RlBracket::Query)
    );
}

#[test]
fn decquery_q2027_returns_grapheme_clustering_query() {
    assert_eq!(
        dispatch(b"?2027", SetMode::DecQuery),
        Mode::GraphemeClustering(GraphemeClustering::Query)
    );
}

// ---------------------------------------------------------------------------
// Group 4: Unknown params — fallback behaviour
// ---------------------------------------------------------------------------

#[test]
fn unknown_q999_with_decquery_returns_unknown_query_stripped() {
    // Params starting with '?' have the prefix stripped; DecQuery → UnknownQuery.
    assert_eq!(
        dispatch(b"?999", SetMode::DecQuery),
        Mode::UnknownQuery(vec![b'9', b'9', b'9'])
    );
}

#[test]
fn unknown_q999_with_decset_returns_unknown_stripped() {
    // Params starting with '?' have the prefix stripped; DecSet → Unknown.
    assert_eq!(
        dispatch(b"?999", SetMode::DecSet),
        Mode::Unknown(UnknownMode {
            params: "999".to_string(),
            mode: SetMode::DecSet,
        })
    );
}

#[test]
fn unknown_42_no_prefix_with_decset_returns_unknown_full_bytes() {
    // Params without '?' are kept as-is.
    assert_eq!(
        dispatch(b"42", SetMode::DecSet),
        Mode::Unknown(UnknownMode {
            params: "42".to_string(),
            mode: SetMode::DecSet,
        })
    );
}

#[test]
fn unknown_42_no_prefix_with_decquery_returns_unknown_query_full_bytes() {
    // Params without '?' are kept as-is; DecQuery → UnknownQuery.
    assert_eq!(
        dispatch(b"42", SetMode::DecQuery),
        Mode::UnknownQuery(vec![b'4', b'2'])
    );
}

#[test]
fn unknown_q999_with_decrst_returns_unknown_stripped() {
    assert_eq!(
        dispatch(b"?999", SetMode::DecRst),
        Mode::Unknown(UnknownMode {
            params: "999".to_string(),
            mode: SetMode::DecRst,
        })
    );
}

// ---------------------------------------------------------------------------
// Group 5: Both ?47 and ?1047 map to AltScreen47
// ---------------------------------------------------------------------------

#[test]
fn both_q47_and_q1047_map_to_altscreen47_decset() {
    let via_47 = dispatch(b"?47", SetMode::DecSet);
    let via_1047 = dispatch(b"?1047", SetMode::DecSet);
    assert_eq!(via_47, Mode::AltScreen47(AltScreen47::Alternate));
    assert_eq!(via_1047, Mode::AltScreen47(AltScreen47::Alternate));
    assert_eq!(via_47, via_1047);
}

#[test]
fn both_q47_and_q1047_map_to_altscreen47_decrst() {
    let via_47 = dispatch(b"?47", SetMode::DecRst);
    let via_1047 = dispatch(b"?1047", SetMode::DecRst);
    assert_eq!(via_47, Mode::AltScreen47(AltScreen47::Primary));
    assert_eq!(via_1047, Mode::AltScreen47(AltScreen47::Primary));
    assert_eq!(via_47, via_1047);
}

#[test]
fn both_q47_and_q1047_map_to_altscreen47_decquery() {
    let via_47 = dispatch(b"?47", SetMode::DecQuery);
    let via_1047 = dispatch(b"?1047", SetMode::DecQuery);
    assert_eq!(via_47, Mode::AltScreen47(AltScreen47::Query));
    assert_eq!(via_1047, Mode::AltScreen47(AltScreen47::Query));
    assert_eq!(via_47, via_1047);
}

#[test]
fn decset_q1007_returns_alternate_scroll_enabled() {
    assert_eq!(
        dispatch(b"?1007", SetMode::DecSet),
        Mode::AlternateScroll(AlternateScroll::Enabled)
    );
}

#[test]
fn decrst_q1007_returns_alternate_scroll_disabled() {
    assert_eq!(
        dispatch(b"?1007", SetMode::DecRst),
        Mode::AlternateScroll(AlternateScroll::Disabled)
    );
}

#[test]
fn decquery_q1007_returns_alternate_scroll_query() {
    assert_eq!(
        dispatch(b"?1007", SetMode::DecQuery),
        Mode::AlternateScroll(AlternateScroll::Query)
    );
}

#[test]
fn decset_q80_returns_decsdm_display_mode() {
    assert_eq!(
        dispatch(b"?80", SetMode::DecSet),
        Mode::Decsdm(Decsdm::DisplayMode)
    );
}

#[test]
fn decrst_q80_returns_decsdm_scrolling_mode() {
    assert_eq!(
        dispatch(b"?80", SetMode::DecRst),
        Mode::Decsdm(Decsdm::ScrollingMode)
    );
}

#[test]
fn decquery_q80_returns_decsdm_query() {
    assert_eq!(
        dispatch(b"?80", SetMode::DecQuery),
        Mode::Decsdm(Decsdm::Query)
    );
}

#[test]
fn decset_q1046_returns_allow_alt_screen() {
    assert_eq!(
        dispatch(b"?1046", SetMode::DecSet),
        Mode::AllowAltScreen(AllowAltScreen::Allow)
    );
}

#[test]
fn decrst_q1046_returns_disallow_alt_screen() {
    assert_eq!(
        dispatch(b"?1046", SetMode::DecRst),
        Mode::AllowAltScreen(AllowAltScreen::Disallow)
    );
}

#[test]
fn decquery_q1046_returns_allow_alt_screen_query() {
    assert_eq!(
        dispatch(b"?1046", SetMode::DecQuery),
        Mode::AllowAltScreen(AllowAltScreen::Query)
    );
}

// ── ?1001 (Hilite Mouse Tracking) ─────────────────────────────────────────

#[test]
fn decset_q1001_returns_mouse_mode_xtmsehilite() {
    assert_eq!(
        dispatch(b"?1001", SetMode::DecSet),
        Mode::MouseMode(MouseTrack::XtMseHilite)
    );
}

#[test]
fn decrst_q1001_returns_mouse_mode_no_tracking() {
    assert_eq!(
        dispatch(b"?1001", SetMode::DecRst),
        Mode::MouseMode(MouseTrack::NoTracking)
    );
}

#[test]
fn decquery_q1001_returns_mouse_mode_query_1001() {
    assert_eq!(
        dispatch(b"?1001", SetMode::DecQuery),
        Mode::MouseMode(MouseTrack::Query(1001))
    );
}

// ── ?1070 (Private Color Registers for Sixel) ─────────────────────────────

#[test]
fn decset_q1070_returns_private_color_registers_private() {
    assert_eq!(
        dispatch(b"?1070", SetMode::DecSet),
        Mode::PrivateColorRegisters(PrivateColorRegisters::Private)
    );
}

#[test]
fn decrst_q1070_returns_private_color_registers_shared() {
    assert_eq!(
        dispatch(b"?1070", SetMode::DecRst),
        Mode::PrivateColorRegisters(PrivateColorRegisters::Shared)
    );
}

#[test]
fn decquery_q1070_returns_private_color_registers_query() {
    assert_eq!(
        dispatch(b"?1070", SetMode::DecQuery),
        Mode::PrivateColorRegisters(PrivateColorRegisters::Query)
    );
}

// ── ?42 (DECNRCM — National Replacement Character Set Mode) ───────────────

#[test]
fn decset_q42_returns_decnrcm_enabled() {
    assert_eq!(
        dispatch(b"?42", SetMode::DecSet),
        Mode::Decnrcm(Decnrcm::NrcEnabled)
    );
}

#[test]
fn decrst_q42_returns_decnrcm_disabled() {
    assert_eq!(
        dispatch(b"?42", SetMode::DecRst),
        Mode::Decnrcm(Decnrcm::NrcDisabled)
    );
}

#[test]
fn decquery_q42_returns_decnrcm_query() {
    assert_eq!(
        dispatch(b"?42", SetMode::DecQuery),
        Mode::Decnrcm(Decnrcm::Query)
    );
}

// ── ?1045 (XTREVWRAP2 — Extended Reverse Wraparound Mode) ─────────────

#[test]
fn decset_q1045_returns_xt_rev_wrap2_enabled() {
    assert_eq!(
        dispatch(b"?1045", SetMode::DecSet),
        Mode::XtRevWrap2(XtRevWrap2::Enabled)
    );
}

#[test]
fn decrst_q1045_returns_xt_rev_wrap2_disabled() {
    assert_eq!(
        dispatch(b"?1045", SetMode::DecRst),
        Mode::XtRevWrap2(XtRevWrap2::Disabled)
    );
}

#[test]
fn decquery_q1045_returns_xt_rev_wrap2_query() {
    assert_eq!(
        dispatch(b"?1045", SetMode::DecQuery),
        Mode::XtRevWrap2(XtRevWrap2::Query)
    );
}

// ── ?2 (DECANM — ANSI/VT52 Mode) ─────────────────────────────────────────

#[test]
fn decset_q2_returns_decanm_ansi() {
    assert_eq!(dispatch(b"?2", SetMode::DecSet), Mode::Decanm(Decanm::Ansi));
}

#[test]
fn decrst_q2_returns_decanm_vt52() {
    assert_eq!(dispatch(b"?2", SetMode::DecRst), Mode::Decanm(Decanm::Vt52));
}

#[test]
fn decquery_q2_returns_decanm_query() {
    assert_eq!(
        dispatch(b"?2", SetMode::DecQuery),
        Mode::Decanm(Decanm::Query)
    );
}
