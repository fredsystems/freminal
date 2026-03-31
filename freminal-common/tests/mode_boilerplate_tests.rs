// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

#![allow(clippy::unwrap_used)]

use freminal_common::buffer_states::mode::SetMode;
use freminal_common::buffer_states::modes::{
    ReportMode,
    allow_alt_screen::AllowAltScreen,
    allow_column_mode_switch::AllowColumnModeSwitch,
    alternate_scroll::AlternateScroll,
    decarm::Decarm,
    decawm::Decawm,
    decbkm::Decbkm,
    decckm::Decckm,
    deccolm::Deccolm,
    decnkm::Decnkm,
    decom::Decom,
    decsclm::Decsclm,
    decscnm::Decscnm,
    decsdm::Decsdm,
    dectcem::Dectcem,
    grapheme::GraphemeClustering,
    keypad::KeypadMode,
    lnm::Lnm,
    reverse_wrap_around::ReverseWrapAround,
    rl_bracket::RlBracket,
    sync_updates::SynchronizedUpdates,
    theme::Theming,
    xtcblink::XtCBlink,
    xtextscrn::{AltScreen47, SaveCursor1048, XtExtscrn},
    xtmsewin::XtMseWin,
};

// ---------------------------------------------------------------------------
// Macro: generates a full test module for any standard 3-variant mode type.
//
// Parameters:
//   $mod_name        — name of the generated test module (ident)
//   $type            — the mode type under test (ty)
//   $param           — the decimal parameter number embedded in DECRPM strings (expr)
//   $default         — expected Default::default() value (expr)
//   $set             — expected new(DecSet) value (expr)
//   $reset           — expected new(DecRst) value (expr)
//   $query           — expected new(DecQuery) value (expr)
//   $set_display     — expected Display string for the Set variant (expr)
//   $reset_display   — expected Display string for the Reset variant (expr)
//   $query_display   — expected Display string for the Query variant (expr)
// ---------------------------------------------------------------------------
macro_rules! test_mode_type {
    (
        $mod_name:ident,
        $type:ty,
        $param:expr,
        $default:expr,
        $set:expr,
        $reset:expr,
        $query:expr,
        $set_display:expr,
        $reset_display:expr,
        $query_display:expr
    ) => {
        mod $mod_name {
            use super::*;

            // ---------------------------------------------------------------
            // 1. Default value
            // ---------------------------------------------------------------
            #[test]
            fn default_value() {
                assert_eq!(<$type>::default(), $default);
            }

            // ---------------------------------------------------------------
            // 2–4. new() constructor
            // ---------------------------------------------------------------
            #[test]
            fn new_dec_set() {
                assert_eq!(<$type>::new(&SetMode::DecSet), $set);
            }

            #[test]
            fn new_dec_rst() {
                assert_eq!(<$type>::new(&SetMode::DecRst), $reset);
            }

            #[test]
            fn new_dec_query() {
                assert_eq!(<$type>::new(&SetMode::DecQuery), $query);
            }

            // ---------------------------------------------------------------
            // 5. report(None) — reflects the current variant
            // ---------------------------------------------------------------
            #[test]
            fn report_none_set_variant() {
                assert_eq!($set.report(None), format!("\x1b[?{};1$y", $param));
            }

            #[test]
            fn report_none_reset_variant() {
                assert_eq!($reset.report(None), format!("\x1b[?{};2$y", $param));
            }

            #[test]
            fn report_none_query_variant() {
                assert_eq!($query.report(None), format!("\x1b[?{};0$y", $param));
            }

            // ---------------------------------------------------------------
            // 6–8. report(Some(_)) — override_mode wins regardless of variant
            // ---------------------------------------------------------------
            #[test]
            fn report_override_dec_set() {
                assert_eq!(
                    $default.report(Some(SetMode::DecSet)),
                    format!("\x1b[?{};1$y", $param)
                );
            }

            #[test]
            fn report_override_dec_rst() {
                assert_eq!(
                    $default.report(Some(SetMode::DecRst)),
                    format!("\x1b[?{};2$y", $param)
                );
            }

            #[test]
            fn report_override_dec_query() {
                assert_eq!(
                    $default.report(Some(SetMode::DecQuery)),
                    format!("\x1b[?{};0$y", $param)
                );
            }

            // ---------------------------------------------------------------
            // 9. Display impl
            // ---------------------------------------------------------------
            #[test]
            fn display_set_variant() {
                assert_eq!(format!("{}", $set), $set_display);
            }

            #[test]
            fn display_reset_variant() {
                assert_eq!(format!("{}", $reset), $reset_display);
            }

            #[test]
            fn display_query_variant() {
                assert_eq!(format!("{}", $query), $query_display);
            }
        }
    };
}

// ===========================================================================
// Standard mode types — invoke the macro
// ===========================================================================

// Decarm (?8): default=RepeatKey, Set=RepeatKey, Reset=NoRepeatKey
test_mode_type!(
    decarm_tests,
    Decarm,
    8,
    Decarm::RepeatKey,
    Decarm::RepeatKey,
    Decarm::NoRepeatKey,
    Decarm::Query,
    "Repeat Key (DECARM)",
    "No Repeat Key (DECARM)",
    "Query Repeat Key (DECARM)"
);

// Lnm (20): default=LineFeed, Set=NewLine, Reset=LineFeed
// Note: parameter in the DECRPM string uses ?20 notation.
test_mode_type!(
    lnm_tests,
    Lnm,
    20,
    Lnm::LineFeed,
    Lnm::NewLine,
    Lnm::LineFeed,
    Lnm::Query,
    "New Line Mode (LNM)",
    "Line Feed Mode (LNM)",
    "Query Line Mode (LNM)"
);

// ReverseWrapAround (?45): default=WrapAround, Set=WrapAround, Reset=DontWrap
test_mode_type!(
    reverse_wrap_around_tests,
    ReverseWrapAround,
    45,
    ReverseWrapAround::WrapAround,
    ReverseWrapAround::WrapAround,
    ReverseWrapAround::DontWrap,
    ReverseWrapAround::Query,
    "Wrap Around",
    "No Wrap Around",
    "Query Wrap Around"
);

// SynchronizedUpdates (?2026): default=Draw, Set=DontDraw, Reset=Draw
test_mode_type!(
    synchronized_updates_tests,
    SynchronizedUpdates,
    2026,
    SynchronizedUpdates::Draw,
    SynchronizedUpdates::DontDraw,
    SynchronizedUpdates::Draw,
    SynchronizedUpdates::Query,
    "Synchronized Updates Mode (DEC 2026) Don't Draw",
    "Synchronized Updates Mode (DEC 2026) Draw",
    "Synchronized Updates Mode (DEC 2026) Query"
);

// XtMseWin (?1004): default=Disabled, Set=Enabled, Reset=Disabled
test_mode_type!(
    xtmsewin_tests,
    XtMseWin,
    1004,
    XtMseWin::Disabled,
    XtMseWin::Enabled,
    XtMseWin::Disabled,
    XtMseWin::Query,
    "Focus Reporting Mode (XT_MSE_WIN) Enabled",
    "Focus Reporting Mode (XT_MSE_WIN) Disabled",
    "Focus Reporting Mode (XT_MSE_WIN) Query"
);

// AllowAltScreen (?1046): default=Allow, Set=Allow, Reset=Disallow
test_mode_type!(
    allow_alt_screen_tests,
    AllowAltScreen,
    1046,
    AllowAltScreen::Allow,
    AllowAltScreen::Allow,
    AllowAltScreen::Disallow,
    AllowAltScreen::Query,
    "Allow Alternate Screen Switching (?1046)",
    "Disallow Alternate Screen Switching (?1046)",
    "Query Allow Alternate Screen Switching (?1046)"
);

// AllowColumnModeSwitch (?40): default=AllowColumnModeSwitch, Set=AllowColumnModeSwitch,
//                               Reset=NoAllowColumnModeSwitch
test_mode_type!(
    allow_column_mode_switch_tests,
    AllowColumnModeSwitch,
    40,
    AllowColumnModeSwitch::AllowColumnModeSwitch,
    AllowColumnModeSwitch::AllowColumnModeSwitch,
    AllowColumnModeSwitch::NoAllowColumnModeSwitch,
    AllowColumnModeSwitch::Query,
    "AllowColumnModeSwitch",
    "NoAllowColumnModeSwitch",
    "Query"
);

// Theming (?2031): default=Light, Set=Light, Reset=Dark
test_mode_type!(
    theming_tests,
    Theming,
    2031,
    Theming::Light,
    Theming::Light,
    Theming::Dark,
    Theming::Query,
    "Theming Mode (DEC 2031) Light",
    "Theming Mode (DEC 2031) Dark",
    "Theming Mode (DEC 2031) Query"
);

// XtCBlink (?12): default=Steady, Set=Blinking, Reset=Steady
test_mode_type!(
    xtcblink_tests,
    XtCBlink,
    12,
    XtCBlink::Steady,
    XtCBlink::Blinking,
    XtCBlink::Steady,
    XtCBlink::Query,
    "XT_CBLINK (SET) Cursor Blinking",
    "XT_CBLINK (RESET) Cursor Steady",
    "XT_CBLINK (QUERY)"
);

// Dectcem (?25): default=Show, Set=Show, Reset=Hide
test_mode_type!(
    dectcem_tests,
    Dectcem,
    25,
    Dectcem::Show,
    Dectcem::Show,
    Dectcem::Hide,
    Dectcem::Query,
    "Show Cursor (DECTCEM)",
    "Hide Cursor (DECTCEM)",
    "Query Cursor (DECTCEM)"
);

// Decckm (?1): default=Ansi, Set=Application, Reset=Ansi
test_mode_type!(
    decckm_tests,
    Decckm,
    1,
    Decckm::Ansi,
    Decckm::Application,
    Decckm::Ansi,
    Decckm::Query,
    "Cursor Key Mode (DECCKM) Application",
    "Cursor Key Mode (DECCKM) ANSI",
    "Cursor Key Mode (DECCKM) Query"
);

// Decawm (?7): default=AutoWrap, Set=AutoWrap, Reset=NoAutoWrap
test_mode_type!(
    decawm_tests,
    Decawm,
    7,
    Decawm::AutoWrap,
    Decawm::AutoWrap,
    Decawm::NoAutoWrap,
    Decawm::Query,
    "Autowrap Mode (DECAWM) Enabled",
    "Autowrap Mode (DECAWM) Disabled",
    "Autowrap Mode (DECAWM) Query"
);

// Deccolm (?3): default=Column132, Set=Column132, Reset=Column80
test_mode_type!(
    deccolm_tests,
    Deccolm,
    3,
    Deccolm::Column132,
    Deccolm::Column132,
    Deccolm::Column80,
    Deccolm::Query,
    "132 Column Mode (DECCOLM)",
    "80 Column Mode (DECCOLM)",
    "Query Column Mode (DECCOLM)"
);

// Decom (?6): default=NormalCursor, Set=OriginMode, Reset=NormalCursor
test_mode_type!(
    decom_tests,
    Decom,
    6,
    Decom::NormalCursor,
    Decom::OriginMode,
    Decom::NormalCursor,
    Decom::Query,
    "Origin Mode",
    "Normal Cursor",
    "Query"
);

// RlBracket (?2004): default=Disabled, Set=Enabled, Reset=Disabled
test_mode_type!(
    rl_bracket_tests,
    RlBracket,
    2004,
    RlBracket::Disabled,
    RlBracket::Enabled,
    RlBracket::Disabled,
    RlBracket::Query,
    "Bracketed Paste Mode (DEC 2004) Enabled",
    "Bracketed Paste Mode (DEC 2004) Disabled",
    "Bracketed Paste Mode (DEC 2004) Query"
);

// Decnkm (?66): default=Numeric, Set=Application, Reset=Numeric
test_mode_type!(
    decnkm_tests,
    Decnkm,
    66,
    Decnkm::Numeric,
    Decnkm::Application,
    Decnkm::Numeric,
    Decnkm::Query,
    "Keypad Application Mode (DECNKM)",
    "Keypad Numeric Mode (DECNKM)",
    "Query Keypad Mode (DECNKM)"
);

// Decbkm (?67): default=BackarrowSendsBs, Set=BackarrowSendsBs, Reset=BackarrowSendsDel
test_mode_type!(
    decbkm_tests,
    Decbkm,
    67,
    Decbkm::BackarrowSendsBs,
    Decbkm::BackarrowSendsBs,
    Decbkm::BackarrowSendsDel,
    Decbkm::Query,
    "Backarrow sends BS (DECBKM set)",
    "Backarrow sends DEL (DECBKM reset)",
    "Query Backarrow Key Mode (DECBKM)"
);

// AlternateScroll (?1007): default=Disabled, Set=Enabled, Reset=Disabled
test_mode_type!(
    alternate_scroll_tests,
    AlternateScroll,
    1007,
    AlternateScroll::Disabled,
    AlternateScroll::Enabled,
    AlternateScroll::Disabled,
    AlternateScroll::Query,
    "Alternate Scroll Enabled (?1007)",
    "Alternate Scroll Disabled (?1007)",
    "Query Alternate Scroll Mode (?1007)"
);

// Decsdm (?80): default=ScrollingMode, Set=DisplayMode, Reset=ScrollingMode
test_mode_type!(
    decsdm_tests,
    Decsdm,
    80,
    Decsdm::ScrollingMode,
    Decsdm::DisplayMode,
    Decsdm::ScrollingMode,
    Decsdm::Query,
    "Sixel Display Mode (DECSDM)",
    "Sixel Scrolling Mode (DECSDM)",
    "Query Sixel Display Mode (DECSDM)"
);

// ===========================================================================
// Decscnm (?5) — standard tests + is_normal_display()
// ===========================================================================
mod decscnm_tests {
    use super::*;

    #[test]
    fn default_value() {
        assert_eq!(Decscnm::default(), Decscnm::NormalDisplay);
    }

    #[test]
    fn new_dec_set() {
        assert_eq!(Decscnm::new(&SetMode::DecSet), Decscnm::ReverseDisplay);
    }

    #[test]
    fn new_dec_rst() {
        assert_eq!(Decscnm::new(&SetMode::DecRst), Decscnm::NormalDisplay);
    }

    #[test]
    fn new_dec_query() {
        assert_eq!(Decscnm::new(&SetMode::DecQuery), Decscnm::Query);
    }

    // report(None)
    #[test]
    fn report_none_set_variant() {
        assert_eq!(Decscnm::ReverseDisplay.report(None), "\x1b[?5;1$y");
    }

    #[test]
    fn report_none_reset_variant() {
        assert_eq!(Decscnm::NormalDisplay.report(None), "\x1b[?5;2$y");
    }

    #[test]
    fn report_none_query_variant() {
        assert_eq!(Decscnm::Query.report(None), "\x1b[?5;0$y");
    }

    // report(Some(_)) — override wins
    #[test]
    fn report_override_dec_set() {
        assert_eq!(
            Decscnm::NormalDisplay.report(Some(SetMode::DecSet)),
            "\x1b[?5;1$y"
        );
    }

    #[test]
    fn report_override_dec_rst() {
        assert_eq!(
            Decscnm::NormalDisplay.report(Some(SetMode::DecRst)),
            "\x1b[?5;2$y"
        );
    }

    #[test]
    fn report_override_dec_query() {
        assert_eq!(
            Decscnm::NormalDisplay.report(Some(SetMode::DecQuery)),
            "\x1b[?5;0$y"
        );
    }

    // Display
    #[test]
    fn display_set_variant() {
        assert_eq!(format!("{}", Decscnm::ReverseDisplay), "Reverse Display");
    }

    #[test]
    fn display_reset_variant() {
        assert_eq!(format!("{}", Decscnm::NormalDisplay), "Normal Display");
    }

    #[test]
    fn display_query_variant() {
        assert_eq!(format!("{}", Decscnm::Query), "Query");
    }

    // is_normal_display()
    #[test]
    fn is_normal_display_for_normal_display() {
        assert!(Decscnm::NormalDisplay.is_normal_display());
    }

    #[test]
    fn is_normal_display_for_reverse_display() {
        assert!(!Decscnm::ReverseDisplay.is_normal_display());
    }

    #[test]
    fn is_normal_display_for_query() {
        assert!(!Decscnm::Query.is_normal_display());
    }
}

// ===========================================================================
// Decsclm (?4) — SPECIAL: report() ALWAYS returns "\x1b[?4;0$y"
// ===========================================================================
mod decsclm_tests {
    use super::*;

    #[test]
    fn default_value() {
        assert_eq!(Decsclm::default(), Decsclm::FastScroll);
    }

    #[test]
    fn new_dec_set() {
        assert_eq!(Decsclm::new(&SetMode::DecSet), Decsclm::SmoothScroll);
    }

    #[test]
    fn new_dec_rst() {
        assert_eq!(Decsclm::new(&SetMode::DecRst), Decsclm::FastScroll);
    }

    #[test]
    fn new_dec_query() {
        assert_eq!(Decsclm::new(&SetMode::DecQuery), Decsclm::Query);
    }

    // report() ALWAYS returns "\x1b[?4;0$y" regardless of variant or override
    #[test]
    fn report_smooth_scroll_none() {
        assert_eq!(Decsclm::SmoothScroll.report(None), "\x1b[?4;0$y");
    }

    #[test]
    fn report_fast_scroll_none() {
        assert_eq!(Decsclm::FastScroll.report(None), "\x1b[?4;0$y");
    }

    #[test]
    fn report_query_none() {
        assert_eq!(Decsclm::Query.report(None), "\x1b[?4;0$y");
    }

    #[test]
    fn report_override_dec_set() {
        assert_eq!(
            Decsclm::FastScroll.report(Some(SetMode::DecSet)),
            "\x1b[?4;0$y"
        );
    }

    #[test]
    fn report_override_dec_rst() {
        assert_eq!(
            Decsclm::SmoothScroll.report(Some(SetMode::DecRst)),
            "\x1b[?4;0$y"
        );
    }

    #[test]
    fn report_override_dec_query() {
        assert_eq!(
            Decsclm::Query.report(Some(SetMode::DecQuery)),
            "\x1b[?4;0$y"
        );
    }

    // Display
    #[test]
    fn display_smooth_scroll() {
        assert_eq!(
            format!("{}", Decsclm::SmoothScroll),
            "Smooth Scroll (DECSCLM)"
        );
    }

    #[test]
    fn display_fast_scroll() {
        assert_eq!(format!("{}", Decsclm::FastScroll), "Fast Scroll (DECSCLM)");
    }

    #[test]
    fn display_query() {
        assert_eq!(format!("{}", Decsclm::Query), "Query Scroll (DECSCLM)");
    }
}

// ===========================================================================
// GraphemeClustering (?2027) — SPECIAL: permanently set → "\x1b[?2027;3$y"
// report(None) for Unicode/Legacy → ;3, for Query → ;0
// report(Some(DecSet/DecRst)) → ;3, report(Some(DecQuery)) → ;0
// ===========================================================================
mod grapheme_clustering_tests {
    use super::*;

    #[test]
    fn default_value() {
        assert_eq!(GraphemeClustering::default(), GraphemeClustering::Unicode);
    }

    #[test]
    fn new_dec_set() {
        assert_eq!(
            GraphemeClustering::new(&SetMode::DecSet),
            GraphemeClustering::Legacy
        );
    }

    #[test]
    fn new_dec_rst() {
        assert_eq!(
            GraphemeClustering::new(&SetMode::DecRst),
            GraphemeClustering::Unicode
        );
    }

    #[test]
    fn new_dec_query() {
        assert_eq!(
            GraphemeClustering::new(&SetMode::DecQuery),
            GraphemeClustering::Query
        );
    }

    // report(None) — Unicode and Legacy both report permanently-set (;3)
    #[test]
    fn report_none_unicode_variant() {
        assert_eq!(GraphemeClustering::Unicode.report(None), "\x1b[?2027;3$y");
    }

    #[test]
    fn report_none_legacy_variant() {
        assert_eq!(GraphemeClustering::Legacy.report(None), "\x1b[?2027;3$y");
    }

    #[test]
    fn report_none_query_variant() {
        assert_eq!(GraphemeClustering::Query.report(None), "\x1b[?2027;0$y");
    }

    // report(Some(_)) — DecSet/DecRst → ;3, DecQuery → ;0
    #[test]
    fn report_override_dec_set() {
        assert_eq!(
            GraphemeClustering::Unicode.report(Some(SetMode::DecSet)),
            "\x1b[?2027;3$y"
        );
    }

    #[test]
    fn report_override_dec_rst() {
        assert_eq!(
            GraphemeClustering::Unicode.report(Some(SetMode::DecRst)),
            "\x1b[?2027;3$y"
        );
    }

    #[test]
    fn report_override_dec_query() {
        assert_eq!(
            GraphemeClustering::Unicode.report(Some(SetMode::DecQuery)),
            "\x1b[?2027;0$y"
        );
    }

    // Display
    #[test]
    fn display_unicode_variant() {
        assert_eq!(
            format!("{}", GraphemeClustering::Unicode),
            "Grapheme Clustering Mode (DEC 2027) Unicode"
        );
    }

    #[test]
    fn display_legacy_variant() {
        assert_eq!(
            format!("{}", GraphemeClustering::Legacy),
            "Grapheme Clustering Mode (DEC 2027) Legacy"
        );
    }

    #[test]
    fn display_query_variant() {
        assert_eq!(
            format!("{}", GraphemeClustering::Query),
            "Grapheme Clustering Mode (DEC 2027) Query"
        );
    }
}

// ===========================================================================
// KeypadMode — no ReportMode, no new(SetMode). Test Default + Display only.
// ===========================================================================
mod keypad_mode_tests {
    use super::*;

    #[test]
    fn default_value() {
        assert_eq!(KeypadMode::default(), KeypadMode::Numeric);
    }

    #[test]
    fn display_numeric() {
        assert_eq!(
            format!("{}", KeypadMode::Numeric),
            "Keypad Mode: Numeric (DECPNM)"
        );
    }

    #[test]
    fn display_application() {
        assert_eq!(
            format!("{}", KeypadMode::Application),
            "Keypad Mode: Application (DECPAM)"
        );
    }
}

// ===========================================================================
// XtExtscrn (?1049): default=Primary, Set=Alternate, Reset=Primary
// ===========================================================================
mod xtextscrn_tests {
    use super::*;

    #[test]
    fn default_value() {
        assert_eq!(XtExtscrn::default(), XtExtscrn::Primary);
    }

    #[test]
    fn new_dec_set() {
        assert_eq!(XtExtscrn::new(&SetMode::DecSet), XtExtscrn::Alternate);
    }

    #[test]
    fn new_dec_rst() {
        assert_eq!(XtExtscrn::new(&SetMode::DecRst), XtExtscrn::Primary);
    }

    #[test]
    fn new_dec_query() {
        assert_eq!(XtExtscrn::new(&SetMode::DecQuery), XtExtscrn::Query);
    }

    // report(None)
    #[test]
    fn report_none_alternate_variant() {
        assert_eq!(XtExtscrn::Alternate.report(None), "\x1b[?1049;1$y");
    }

    #[test]
    fn report_none_primary_variant() {
        assert_eq!(XtExtscrn::Primary.report(None), "\x1b[?1049;2$y");
    }

    #[test]
    fn report_none_query_variant() {
        assert_eq!(XtExtscrn::Query.report(None), "\x1b[?1049;0$y");
    }

    // report(Some(_))
    #[test]
    fn report_override_dec_set() {
        assert_eq!(
            XtExtscrn::Primary.report(Some(SetMode::DecSet)),
            "\x1b[?1049;1$y"
        );
    }

    #[test]
    fn report_override_dec_rst() {
        assert_eq!(
            XtExtscrn::Primary.report(Some(SetMode::DecRst)),
            "\x1b[?1049;2$y"
        );
    }

    #[test]
    fn report_override_dec_query() {
        assert_eq!(
            XtExtscrn::Primary.report(Some(SetMode::DecQuery)),
            "\x1b[?1049;0$y"
        );
    }

    // Display
    #[test]
    fn display_alternate_variant() {
        assert_eq!(
            format!("{}", XtExtscrn::Alternate),
            "XT_EXTSCRN (SET) Alternate Screen"
        );
    }

    #[test]
    fn display_primary_variant() {
        assert_eq!(
            format!("{}", XtExtscrn::Primary),
            "XT_EXTSCRN (RESET) Primary Screen"
        );
    }

    #[test]
    fn display_query_variant() {
        assert_eq!(format!("{}", XtExtscrn::Query), "XT_EXTSCRN (QUERY)");
    }
}

// ===========================================================================
// AltScreen47 (?47): default=Primary, Set=Alternate, Reset=Primary
// ===========================================================================
mod altscreen47_tests {
    use super::*;

    #[test]
    fn default_value() {
        assert_eq!(AltScreen47::default(), AltScreen47::Primary);
    }

    #[test]
    fn new_dec_set() {
        assert_eq!(AltScreen47::new(&SetMode::DecSet), AltScreen47::Alternate);
    }

    #[test]
    fn new_dec_rst() {
        assert_eq!(AltScreen47::new(&SetMode::DecRst), AltScreen47::Primary);
    }

    #[test]
    fn new_dec_query() {
        assert_eq!(AltScreen47::new(&SetMode::DecQuery), AltScreen47::Query);
    }

    // report(None)
    #[test]
    fn report_none_alternate_variant() {
        assert_eq!(AltScreen47::Alternate.report(None), "\x1b[?47;1$y");
    }

    #[test]
    fn report_none_primary_variant() {
        assert_eq!(AltScreen47::Primary.report(None), "\x1b[?47;2$y");
    }

    #[test]
    fn report_none_query_variant() {
        assert_eq!(AltScreen47::Query.report(None), "\x1b[?47;0$y");
    }

    // report(Some(_))
    #[test]
    fn report_override_dec_set() {
        assert_eq!(
            AltScreen47::Primary.report(Some(SetMode::DecSet)),
            "\x1b[?47;1$y"
        );
    }

    #[test]
    fn report_override_dec_rst() {
        assert_eq!(
            AltScreen47::Primary.report(Some(SetMode::DecRst)),
            "\x1b[?47;2$y"
        );
    }

    #[test]
    fn report_override_dec_query() {
        assert_eq!(
            AltScreen47::Primary.report(Some(SetMode::DecQuery)),
            "\x1b[?47;0$y"
        );
    }

    // Display
    #[test]
    fn display_alternate_variant() {
        assert_eq!(
            format!("{}", AltScreen47::Alternate),
            "AltScreen47 (SET) Alternate Screen"
        );
    }

    #[test]
    fn display_primary_variant() {
        assert_eq!(
            format!("{}", AltScreen47::Primary),
            "AltScreen47 (RESET) Primary Screen"
        );
    }

    #[test]
    fn display_query_variant() {
        assert_eq!(format!("{}", AltScreen47::Query), "AltScreen47 (QUERY)");
    }
}

// ===========================================================================
// SaveCursor1048 (?1048): default=Restore, Set=Save, Reset=Restore
// ===========================================================================
mod save_cursor1048_tests {
    use super::*;

    #[test]
    fn default_value() {
        assert_eq!(SaveCursor1048::default(), SaveCursor1048::Restore);
    }

    #[test]
    fn new_dec_set() {
        assert_eq!(SaveCursor1048::new(&SetMode::DecSet), SaveCursor1048::Save);
    }

    #[test]
    fn new_dec_rst() {
        assert_eq!(
            SaveCursor1048::new(&SetMode::DecRst),
            SaveCursor1048::Restore
        );
    }

    #[test]
    fn new_dec_query() {
        assert_eq!(
            SaveCursor1048::new(&SetMode::DecQuery),
            SaveCursor1048::Query
        );
    }

    // report(None)
    #[test]
    fn report_none_save_variant() {
        assert_eq!(SaveCursor1048::Save.report(None), "\x1b[?1048;1$y");
    }

    #[test]
    fn report_none_restore_variant() {
        assert_eq!(SaveCursor1048::Restore.report(None), "\x1b[?1048;2$y");
    }

    #[test]
    fn report_none_query_variant() {
        assert_eq!(SaveCursor1048::Query.report(None), "\x1b[?1048;0$y");
    }

    // report(Some(_))
    #[test]
    fn report_override_dec_set() {
        assert_eq!(
            SaveCursor1048::Restore.report(Some(SetMode::DecSet)),
            "\x1b[?1048;1$y"
        );
    }

    #[test]
    fn report_override_dec_rst() {
        assert_eq!(
            SaveCursor1048::Restore.report(Some(SetMode::DecRst)),
            "\x1b[?1048;2$y"
        );
    }

    #[test]
    fn report_override_dec_query() {
        assert_eq!(
            SaveCursor1048::Restore.report(Some(SetMode::DecQuery)),
            "\x1b[?1048;0$y"
        );
    }

    // Display
    #[test]
    fn display_save_variant() {
        assert_eq!(
            format!("{}", SaveCursor1048::Save),
            "SaveCursor1048 (SET) Save Cursor"
        );
    }

    #[test]
    fn display_restore_variant() {
        assert_eq!(
            format!("{}", SaveCursor1048::Restore),
            "SaveCursor1048 (RESET) Restore Cursor"
        );
    }

    #[test]
    fn display_query_variant() {
        assert_eq!(
            format!("{}", SaveCursor1048::Query),
            "SaveCursor1048 (QUERY)"
        );
    }
}
