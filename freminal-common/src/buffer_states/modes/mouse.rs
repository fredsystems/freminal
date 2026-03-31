// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use std::fmt;

use crate::buffer_states::{
    mode::SetMode,
    modes::{MouseModeNumber, ReportMode},
};

/// The wire format used to encode mouse reports sent to the PTY.
///
/// This is orthogonal to `MouseTrack` — the tracking level determines *which*
/// events are reported, while the encoding determines *how* they are formatted.
///
/// In xterm, modes `?1005`, `?1006`, and `?1016` set the encoding; `?9`,
/// `?1000`, `?1002`, `?1003` set the tracking level.  These are independent
/// axes.
#[derive(Debug, PartialEq, Eq, Default, Clone)]
pub enum MouseEncoding {
    /// Legacy X11 binary encoding (CSI M Cb Cx Cy).
    ///
    /// Default when no encoding mode has been set.  Limited to coordinates
    /// ≤ 223 (byte value 255 − 32).
    #[default]
    X11,
    /// UTF-8 extended encoding (?1005).
    ///
    /// Like X11 but Cb/Cx/Cy are encoded as UTF-8 characters, extending the
    /// coordinate range to 2015.  Rarely used in practice; SGR is preferred.
    Utf8,
    /// SGR text encoding (?1006).
    ///
    /// CSI < Cb ; Cx ; Cy M/m — coordinates are decimal text, no upper limit.
    /// Distinguishes press (M) from release (m).  The de-facto standard for
    /// modern terminal applications.
    Sgr,
    /// SGR-Pixels encoding (?1016).
    ///
    /// Like SGR but coordinates are in pixels rather than character cells.
    SgrPixels,
}

impl MouseModeNumber for MouseEncoding {
    fn mouse_mode_number(&self) -> usize {
        match self {
            Self::X11 => 0,
            Self::Utf8 => 1005,
            Self::Sgr => 1006,
            Self::SgrPixels => 1016,
        }
    }
}

impl ReportMode for MouseEncoding {
    fn report(&self, override_mode: Option<SetMode>) -> String {
        let mode_number = match self {
            Self::X11 => 0,
            Self::Utf8 => 1005,
            Self::Sgr => 1006,
            Self::SgrPixels => 1016,
        };

        let set_mode = match override_mode {
            Some(SetMode::DecSet) => i32::from(*self != Self::X11),
            Some(SetMode::DecRst) | None => {
                if *self == Self::X11 {
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

impl fmt::Display for MouseEncoding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::X11 => write!(f, "X11"),
            Self::Utf8 => write!(f, "Utf8"),
            Self::Sgr => write!(f, "Sgr"),
            Self::SgrPixels => write!(f, "SgrPixels"),
        }
    }
}

/// Mouse tracking level — determines *which* mouse events are reported to the
/// PTY.
///
/// This is orthogonal to `MouseEncoding`.  The tracking level is set by modes
/// `?9`, `?1000`, `?1001`, `?1002`, `?1003`; the encoding format is set by
/// `?1005`, `?1006`, `?1016`.
///
/// Reference: <https://invisible-island.net/xterm/ctlseqs/ctlseqs.html#h2-Mouse-Tracking>
#[derive(Debug, Eq, PartialEq, Default, Clone)]
pub enum MouseTrack {
    #[default]
    NoTracking,
    /// X10 compatibility mode (?9) — report button press only.
    XtMsex10,
    /// X11 / normal tracking (?1000) — report button press and release.
    XtMseX11,
    /// Hilite mouse tracking (?1001) — X11-era protocol where the terminal
    /// highlights the region between press and release.  Rarely used in
    /// practice; accepted for compatibility.
    XtMseHilite,
    /// Button-event tracking (?1002) — like X11 plus motion while button held.
    XtMseBtn,
    /// Any-event tracking (?1003) — report all motion, whether or not a button
    /// is held.
    XtMseAny,
    /// DECRPM query for a tracking-level mode.
    Query(usize),
}

impl MouseModeNumber for MouseTrack {
    fn mouse_mode_number(&self) -> usize {
        match self {
            Self::NoTracking => 0,
            Self::XtMsex10 => 9,
            Self::XtMseX11 => 1000,
            Self::XtMseHilite => 1001,
            Self::XtMseBtn => 1002,
            Self::XtMseAny => 1003,
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
            Self::XtMseHilite => 1001,
            Self::XtMseBtn => 1002,
            Self::XtMseAny => 1003,
        };

        let set_mode = match override_mode {
            Some(SetMode::DecSet) => {
                i32::from(*self != Self::NoTracking && *self != Self::Query(mode_number))
            }
            // The way the callers for this should call with None, and we should never hit the None Case.
            // Just in case, because maybe I am stupid and have this broken somewhere
            // we'll treat the None case as a Reset.
            Some(SetMode::DecRst) | None => {
                if *self == Self::NoTracking || *self == Self::Query(mode_number) {
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

impl fmt::Display for MouseTrack {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::XtMseX11 => write!(f, "XtMseX11"),
            Self::NoTracking => write!(f, "NoTracking"),
            Self::XtMsex10 => write!(f, "XtMsex10"),
            Self::XtMseHilite => write!(f, "XtMseHilite"),
            Self::XtMseBtn => write!(f, "XtMseBtn"),
            Self::XtMseAny => write!(f, "XtMseAny"),
            Self::Query(v) => write!(f, "Query Mouse Tracking({v})"),
        }
    }
}
