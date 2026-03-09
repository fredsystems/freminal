// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use std::fmt;

use crate::buffer_states::{
    mode::SetMode,
    modes::{MouseModeNumber, ReportMode},
};

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum MouseEncoding {
    X11,
    Sgr,
}

// https://invisible-island.net/xterm/ctlseqs/ctlseqs.html#h2-Mouse-Tracking
#[derive(Debug, Eq, PartialEq, Default, Clone)]
pub enum MouseTrack {
    #[default]
    NoTracking,
    XtMsex10,       // ?9
    XtMseX11,       // ?1000
    XtMseBtn,       // ?1002
    XtMseAny,       // ?1003
    XtMseUtf,       // ?1005
    XtMseSgr,       // ?1006
    XtMseUrXvt,     // ?1015
    XtMseSgrPixels, // ?1016
    Query(usize),
}

impl MouseModeNumber for MouseTrack {
    fn mouse_mode_number(&self) -> usize {
        match self {
            Self::NoTracking => 0,
            Self::XtMsex10 => 9,
            Self::XtMseX11 => 1000,
            Self::XtMseBtn => 1002,
            Self::XtMseAny => 1003,
            Self::XtMseUtf => 1005,
            Self::XtMseSgr => 1006,
            Self::XtMseUrXvt => 1015,
            Self::XtMseSgrPixels => 1016,
            Self::Query(v) => *v,
        }
    }
}

impl ReportMode for MouseTrack {
    fn report(&self, override_mode: Option<SetMode>) -> String {
        let mode_number = match self {
            Self::NoTracking => 0,
            Self::Query(a) => *a,
            Self::XtMsex10 => 9,
            Self::XtMseX11 => 1000,
            Self::XtMseBtn => 1002,
            Self::XtMseAny => 1003,
            Self::XtMseUtf => 1005,
            Self::XtMseSgr => 1006,
            Self::XtMseUrXvt => 1015,
            Self::XtMseSgrPixels => 1016,
        };

        let set_mode = match override_mode {
            Some(SetMode::DecSet) => i32::from(
                *self != Self::NoTracking
                    && *self != Self::Query(mode_number)
                    && *self != Self::XtMseUrXvt,
            ),
            // The way the callers for this should call with None, and we should never hit the None Case.
            // Just in case, because maybe I am stupid and have this broken somewhere
            // we'll treat the None case as a Reset.
            Some(SetMode::DecRst) | None => {
                if *self == Self::NoTracking
                    || *self == Self::Query(mode_number)
                    || *self == Self::XtMseUrXvt
                {
                    0
                } else {
                    2
                }
            }
            Some(SetMode::DecQuery) => 0,
        };
        format!("\x1b[?{mode_number};{set_mode}$y")
    }
}

impl MouseTrack {
    #[must_use]
    pub fn get_encoding(&self) -> MouseEncoding {
        if self == &Self::XtMseSgr || self == &Self::XtMseSgrPixels {
            MouseEncoding::Sgr
        } else {
            MouseEncoding::X11
        }
    }
}

impl fmt::Display for MouseTrack {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::XtMseX11 => write!(f, "XtMseX11"),
            Self::NoTracking => write!(f, "NoTracking"),
            Self::XtMsex10 => write!(f, "XtMsex10"),
            Self::XtMseBtn => write!(f, "XtMseBtn"),
            Self::XtMseAny => write!(f, "XtMseAny"),
            Self::XtMseUtf => write!(f, "XtMseUtf"),
            Self::XtMseSgr => write!(f, "XtMseSgr"),
            Self::XtMseUrXvt => write!(f, "XtMseUrXvt"),
            Self::XtMseSgrPixels => write!(f, "XtMseSgrPixels"),
            Self::Query(v) => write!(f, "Query Mouse Tracking({v})"),
        }
    }
}
