// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use std::fmt;

use crate::buffer_states::modes::{
    ReportMode,
    allow_column_mode_switch::AllowColumnModeSwitch,
    decarm::Decarm,
    decawm::Decawm,
    decckm::Decckm,
    deccolm::Deccolm,
    decom::Decom,
    decsclm::Decsclm,
    decscnm::Decscnm,
    dectcem::Dectcem,
    grapheme::GraphemeClustering,
    keypad::KeypadMode,
    lnm::Lnm,
    mouse::{MouseEncoding, MouseTrack},
    reverse_wrap_around::ReverseWrapAround,
    rl_bracket::RlBracket,
    sync_updates::SynchronizedUpdates,
    theme::Theming,
    unknown::UnknownMode,
    xtcblink::XtCBlink,
    xtextscrn::{AltScreen47, SaveCursor1048, XtExtscrn},
    xtmsewin::XtMseWin,
};

#[allow(clippy::module_name_repetitions)]
#[derive(Debug, Eq, PartialEq, Default, Clone, Copy)]
pub enum SetMode {
    DecSet,
    #[default]
    DecRst,
    DecQuery,
}

impl fmt::Display for SetMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DecSet => write!(f, "Mode Set"),
            Self::DecRst => write!(f, "Mode Reset"),
            Self::DecQuery => write!(f, "Mode Query"),
        }
    }
}

#[derive(Debug, Eq, PartialEq, Default)]
pub struct TerminalModes {
    pub cursor_key: Decckm,
    pub bracketed_paste: RlBracket,
    pub focus_reporting: XtMseWin,
    pub cursor_blinking: XtCBlink,
    pub mouse_tracking: MouseTrack,
    /// The wire format for mouse reports, set independently of `mouse_tracking`.
    ///
    /// `?1005` → `Utf8`, `?1006` → `Sgr`, `?1016` → `SgrPixels`.
    /// Default is `X11` (legacy binary encoding).
    pub mouse_encoding: MouseEncoding,
    pub synchronized_updates: SynchronizedUpdates,
    pub invert_screen: Decscnm,
    pub repeat_keys: Decarm,
    pub reverse_wrap_around: ReverseWrapAround,
    pub line_feed_mode: Lnm,
    pub keypad_mode: KeypadMode,
}

#[derive(Eq, PartialEq, Debug, Default, Clone)]
pub enum Mode {
    #[default]
    NoOp,
    // Cursor keys mode
    // https://vt100.net/docs/vt100-ug/chapter3.html
    AllowColumnModeSwitch(AllowColumnModeSwitch),
    Decckm(Decckm),
    Decawm(Decawm),
    Dectem(Dectcem),
    Deccolm(Deccolm),
    Decsclm(Decsclm),
    Decscnm(Decscnm),
    Decom(Decom),
    Decarm(Decarm),
    LineFeedMode(Lnm),
    XtCBlink(XtCBlink),
    XtExtscrn(XtExtscrn),
    AltScreen47(AltScreen47),
    SaveCursor1048(SaveCursor1048),
    XtMseWin(XtMseWin),
    BracketedPaste(RlBracket),
    MouseMode(MouseTrack),
    /// Mouse encoding format (?1005/?1006/?1016) — orthogonal to `MouseMode`.
    MouseEncodingMode(MouseEncoding),
    ReverseWrapAround(ReverseWrapAround),
    SynchronizedUpdates(SynchronizedUpdates),
    GraphemeClustering(GraphemeClustering),
    Theming(Theming),
    UnknownQuery(Vec<u8>),
    Unknown(UnknownMode),
}

impl Mode {
    /// Map a mouse-tracking param to the appropriate `MouseMode` variant.
    const fn mouse_mode(mode: SetMode, set: MouseTrack, query_id: usize) -> Self {
        match mode {
            SetMode::DecSet => Self::MouseMode(set),
            SetMode::DecRst => Self::MouseMode(MouseTrack::NoTracking),
            SetMode::DecQuery => Self::MouseMode(MouseTrack::Query(query_id)),
        }
    }

    /// Map a mouse-encoding param to the appropriate `MouseEncodingMode` variant.
    const fn mouse_encoding_mode(mode: SetMode, set: MouseEncoding, query_id: usize) -> Self {
        match mode {
            SetMode::DecSet => Self::MouseEncodingMode(set),
            SetMode::DecRst => Self::MouseEncodingMode(MouseEncoding::X11),
            SetMode::DecQuery => Self::MouseMode(MouseTrack::Query(query_id)),
        }
    }

    #[must_use]
    pub fn terminal_mode_from_params(params: &[u8], mode: SetMode) -> Self {
        match params {
            // https://vt100.net/docs/vt510-rm/DECCKM.html
            b"?1" => Self::Decckm(Decckm::new(&mode)),
            b"?3" => Self::Deccolm(Deccolm::new(&mode)),
            b"?4" => Self::Decsclm(Decsclm::new(&mode)),
            b"?5" => Self::Decscnm(Decscnm::new(&mode)),
            b"?6" => Self::Decom(Decom::new(&mode)),
            b"?7" => Self::Decawm(Decawm::new(&mode)),
            b"?8" => Self::Decarm(Decarm::new(&mode)),
            b"?9" => Self::mouse_mode(mode, MouseTrack::XtMsex10, 9),
            b"?12" => Self::XtCBlink(XtCBlink::new(&mode)),
            b"20" => Self::LineFeedMode(Lnm::new(&mode)),
            b"?25" => Self::Dectem(Dectcem::new(&mode)),
            b"?40" => Self::AllowColumnModeSwitch(AllowColumnModeSwitch::new(&mode)),
            b"?45" => Self::ReverseWrapAround(ReverseWrapAround::new(&mode)),
            b"?1000" => Self::mouse_mode(mode, MouseTrack::XtMseX11, 1000),
            b"?1002" => Self::mouse_mode(mode, MouseTrack::XtMseBtn, 1002),
            b"?1003" => Self::mouse_mode(mode, MouseTrack::XtMseAny, 1003),
            b"?1004" => Self::XtMseWin(XtMseWin::new(&mode)),
            b"?1005" => Self::mouse_encoding_mode(mode, MouseEncoding::Utf8, 1005),
            b"?1006" => Self::mouse_encoding_mode(mode, MouseEncoding::Sgr, 1006),
            // ?1015 (urxvt mouse) intentionally omitted — the format clashes
            // with DL / SD / window manipulation sequences and is not
            // recommended; ?1006 (SGR) is the preferred replacement.
            b"?1016" => Self::mouse_encoding_mode(mode, MouseEncoding::SgrPixels, 1016),
            b"?1049" => Self::XtExtscrn(XtExtscrn::new(&mode)),
            b"?47" | b"?1047" => Self::AltScreen47(AltScreen47::new(&mode)),
            b"?1048" => Self::SaveCursor1048(SaveCursor1048::new(&mode)),
            b"?2004" => Self::BracketedPaste(RlBracket::new(&mode)),
            b"?2026" => Self::SynchronizedUpdates(SynchronizedUpdates::new(&mode)),
            b"?2027" => Self::GraphemeClustering(GraphemeClustering::new(&mode)),
            b"?2031" => Self::Theming(Theming::new(&mode)),
            _ => {
                let output_params = params
                    .to_vec()
                    .iter()
                    .skip(usize::from(params.first().unwrap_or(&b'?') == &b'?'))
                    .copied()
                    .collect::<Vec<u8>>();

                if mode == SetMode::DecQuery {
                    Self::UnknownQuery(output_params)
                } else {
                    Self::Unknown(UnknownMode::new(&output_params, mode))
                }
            }
        }
    }
}

impl ReportMode for Mode {
    fn report(&self, override_mode: Option<SetMode>) -> String {
        match self {
            Self::NoOp => "NoOp".into(),
            Self::AllowColumnModeSwitch(allow_column_mode_switch) => {
                allow_column_mode_switch.report(override_mode)
            }
            Self::Decarm(decarm) => decarm.report(override_mode),
            Self::Decckm(decckm) => decckm.report(override_mode),
            Self::Decom(decom) => decom.report(override_mode),
            Self::Deccolm(deccolm) => deccolm.report(override_mode),
            Self::Decsclm(decsclm) => decsclm.report(override_mode),
            Self::Decawm(decawm) => decawm.report(override_mode),
            Self::Dectem(dectem) => dectem.report(override_mode),
            Self::Decscnm(decscnm) => decscnm.report(override_mode),
            Self::LineFeedMode(lnm) => lnm.report(override_mode),
            Self::XtCBlink(xt_cblink) => xt_cblink.report(override_mode),
            Self::XtExtscrn(xt_extscrn) => xt_extscrn.report(override_mode),
            Self::AltScreen47(alt47) => alt47.report(override_mode),
            Self::SaveCursor1048(sc1048) => sc1048.report(override_mode),
            Self::XtMseWin(xt_mse_win) => xt_mse_win.report(override_mode),
            Self::BracketedPaste(rl_bracket) => rl_bracket.report(override_mode),
            Self::MouseMode(mouse_mode) => mouse_mode.report(override_mode),
            Self::MouseEncodingMode(mouse_encoding) => mouse_encoding.report(override_mode),
            Self::ReverseWrapAround(reverse_wrap_around) => {
                reverse_wrap_around.report(override_mode)
            }
            Self::SynchronizedUpdates(sync_updates) => sync_updates.report(override_mode),
            Self::GraphemeClustering(grapheme_clustering) => {
                grapheme_clustering.report(override_mode)
            }
            Self::Theming(theming) => theming.report(override_mode),
            Self::Unknown(mode) => mode.report(override_mode),
            Self::UnknownQuery(v) => {
                // convert each digit to a char
                let digits = v.iter().map(|&x| x as char).collect::<String>();
                format!("\x1b[?{digits};0$y")
            }
        }
    }
}

impl fmt::Display for Mode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoOp => write!(f, "NoOp"),
            Self::AllowColumnModeSwitch(allow_column_mode_switch) => {
                write!(f, "{allow_column_mode_switch}")
            }
            Self::Decarm(decarm) => write!(f, "{decarm}"),
            Self::Decckm(decckm) => write!(f, "{decckm}"),
            Self::Decawm(decawm) => write!(f, "{decawm}"),
            Self::Decom(decom) => write!(f, "{decom}"),
            Self::Dectem(dectem) => write!(f, "{dectem}"),
            Self::Decscnm(decscnm) => write!(f, "{decscnm}"),
            Self::Decsclm(decsclm) => write!(f, "{decsclm}"),
            Self::Deccolm(deccolm) => write!(f, "{deccolm}"),
            Self::LineFeedMode(lnm) => write!(f, "{lnm}"),
            Self::XtCBlink(xt_cblink) => write!(f, "{xt_cblink}"),
            Self::MouseMode(mouse_mode) => write!(f, "{mouse_mode}"),
            Self::MouseEncodingMode(mouse_encoding) => {
                write!(f, "MouseEncoding({mouse_encoding})")
            }
            Self::XtMseWin(xt_mse_win) => write!(f, "{xt_mse_win}"),
            Self::XtExtscrn(xt_extscrn) => write!(f, "{xt_extscrn}"),
            Self::AltScreen47(alt47) => write!(f, "{alt47}"),
            Self::SaveCursor1048(sc1048) => write!(f, "{sc1048}"),
            Self::BracketedPaste(bracketed_paste) => write!(f, "{bracketed_paste}"),
            Self::ReverseWrapAround(reverse_wrap_around) => write!(f, "{reverse_wrap_around}"),
            Self::SynchronizedUpdates(sync_updates) => write!(f, "{sync_updates}"),
            Self::GraphemeClustering(grapheme_clustering) => write!(f, "{grapheme_clustering}"),
            Self::Theming(theming) => write!(f, "{theming}"),
            Self::Unknown(params) => write!(f, "{params}"),
            Self::UnknownQuery(v) => write!(f, "Unknown Query({v:?})"),
        }
    }
}
