// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use anyhow::{Error, Result};
use std::str::FromStr;

use crate::buffer_states::{ftcs::FtcsMarker, url::Url};

#[derive(Eq, PartialEq, Debug, Clone)]
pub enum AnsiOscInternalType {
    Query,
    //SetColor(Color32),
    String(String),
    Unknown(Option<AnsiOscToken>),
}

impl std::fmt::Display for AnsiOscInternalType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Query => write!(f, "Query"),
            //Self::SetColor(value) => write!(f, "SetColor({value:?})"),
            Self::String(value) => write!(f, "{value}"),
            Self::Unknown(value) => write!(f, "Unknown({value:?})"),
        }
    }
}

impl From<&Vec<Option<AnsiOscToken>>> for AnsiOscInternalType {
    fn from(value: &Vec<Option<AnsiOscToken>>) -> Self {
        // The first value is the type of the OSC sequence
        // if the first value is b'?', then it is a query
        // otherwise, it is a set but we'll leave that as unknown for now

        value
            .get(1)
            .map_or(Self::Unknown(None), |value| match value {
                Some(AnsiOscToken::String(value)) => {
                    if value.as_str() == "?" {
                        Self::Query
                    } else {
                        Self::String(value.clone())
                    }
                }
                Some(value) => Self::Unknown(Some(value.clone())),
                None => Self::Unknown(None),
            })
    }
}

#[derive(Eq, PartialEq, Debug)]
pub enum OscTarget {
    TitleBar,
    IconName,
    Background,
    Foreground,
    // https://iterm2.com/documentation-escape-codes.html
    Ftcs,
    Clipboard,
    PaletteColor,
    ResetPaletteColor,
    RemoteHost,
    Url,
    ResetCursorColor,
    /// OSC 110 — reset text foreground color to the theme default.
    ResetForeground,
    /// OSC 111 — reset text background color to the theme default.
    ResetBackground,
    Unknown,
    ITerm2,
}

// A list of command we may need to handle. I'm sure there is more.

// OSC 0	SETTITLE	Change Window & Icon Title
// OSC 1	SETICON	Change Icon Title
// OSC 2	SETWINTITLE	Change Window Title
// OSC 3	SETXPROP	Set X11 property
// OSC 4	SETCOLPAL	Set/Query color palette
// OSC 7	SETCWD	Set current working directory
// OSC 8	HYPERLINK	Hyperlinked Text
// OSC 10	COLORFG	Change or request text foreground color.
// OSC 11	COLORBG	Change or request text background color.
// OSC 12	COLORCURSOR	Change text cursor color to Pt.
// OSC 13	COLORMOUSEFG	Change mouse foreground color.
// OSC 14	COLORMOUSEBG	Change mouse background color.
// OSC 50	SETFONT	Get or set font.
// OSC 52	CLIPBOARD	Clipboard management.
// OSC 60	SETFONTALL	Get or set all font faces, styles, size.
// OSC 104	RCOLPAL	Reset color full palette or entry
// OSC 106	COLORSPECIAL	Enable/disable Special Color Number c.
// OSC 110	RCOLORFG	Reset VT100 text foreground color.
// OSC 111	RCOLORBG	Reset VT100 text background color.
// OSC 112	RCOLORCURSOR	Reset text cursor color.
// OSC 113	RCOLORMOUSEFG	Reset mouse foreground color.
// OSC 114	RCOLORMOUSEBG	Reset mouse background color.
// OSC 117	RCOLORHIGHLIGHTBG	Reset highlight background color.
// OSC 119	RCOLORHIGHLIGHTFG	Reset highlight foreground color.
// OSC 777	NOTIFY	Send Notification.
// OSC 888	DUMPSTATE	Dumps internal state to debug stream.

impl From<&AnsiOscToken> for OscTarget {
    fn from(value: &AnsiOscToken) -> Self {
        match value {
            AnsiOscToken::OscValue(0 | 2) => Self::TitleBar,
            AnsiOscToken::OscValue(1) => Self::IconName,
            AnsiOscToken::OscValue(4) => Self::PaletteColor,
            AnsiOscToken::OscValue(7) => Self::RemoteHost,
            AnsiOscToken::OscValue(8) => Self::Url,
            AnsiOscToken::OscValue(11) => Self::Background,
            AnsiOscToken::OscValue(10) => Self::Foreground,
            AnsiOscToken::OscValue(52) => Self::Clipboard,
            AnsiOscToken::OscValue(104) => Self::ResetPaletteColor,
            AnsiOscToken::OscValue(112) => Self::ResetCursorColor,
            AnsiOscToken::OscValue(133) => Self::Ftcs,
            AnsiOscToken::OscValue(1337) => Self::ITerm2,
            AnsiOscToken::OscValue(110) => Self::ResetForeground,
            AnsiOscToken::OscValue(111) => Self::ResetBackground,
            _ => Self::Unknown,
        }
    }
}

#[derive(Eq, PartialEq, Debug, Clone)]
pub enum UrlResponse {
    Url(Url),
    End,
}

impl std::fmt::Display for UrlResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Url(url) => write!(f, "Url({url})"),
            Self::End => write!(f, "End"),
        }
    }
}

impl From<Vec<Option<AnsiOscToken>>> for UrlResponse {
    fn from(value: Vec<Option<AnsiOscToken>>) -> Self {
        // There are two tokens that we care about
        // if BOTH tokens are None, then it is the end of the URL

        // Otherwise, the first token is the ID, and the second token is the URL
        match value.as_slice() {
            [
                Some(AnsiOscToken::OscValue(8)),
                Some(AnsiOscToken::String(id)),
                Some(AnsiOscToken::String(url)),
            ] => Self::Url(Url {
                id: Some(id.clone()),
                url: url.clone(),
            }),
            [
                Some(AnsiOscToken::OscValue(8)),
                None,
                Some(AnsiOscToken::String(url)),
            ] => Self::Url(Url {
                id: None,
                url: url.clone(),
            }),
            _ => Self::End,
        }
    }
}

#[derive(Eq, PartialEq, Debug, Default, Clone)]
pub enum AnsiOscType {
    #[default]
    NoOp,
    RequestColorQueryBackground(AnsiOscInternalType),
    RequestColorQueryForeground(AnsiOscInternalType),
    Ftcs(FtcsMarker),
    // FIXME: We're handling 0 and 2 as just title bar for now
    // if we go tabbed, we'll need to handle 2 differently
    SetTitleBar(String),
    Url(UrlResponse),
    RemoteHost(String),
    ResetCursorColor,
    ITerm2,
    /// OSC 52 clipboard set: selection name + decoded (plaintext) content.
    SetClipboard(String, String),
    /// OSC 52 clipboard query: selection name.
    QueryClipboard(String),
    /// OSC 4 set palette color: index, r, g, b.
    SetPaletteColor(u8, u8, u8, u8),
    /// OSC 4 query palette color at index.
    QueryPaletteColor(u8),
    /// OSC 104 reset palette color at index, or all if `None`.
    ResetPaletteColor(Option<u8>),
    /// OSC 110 — reset the dynamic foreground color override.
    ResetForegroundColor,
    /// OSC 111 — reset the dynamic background color override.
    ResetBackgroundColor,
}

impl std::fmt::Display for AnsiOscType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoOp => write!(f, "NoOp"),
            Self::RequestColorQueryBackground(value) => {
                write!(f, "RequestColorQueryBackground({value:?})")
            }
            Self::RequestColorQueryForeground(value) => {
                write!(f, "RequestColorQueryForeground({value:?})")
            }
            Self::Url(url) => write!(f, "Url({url})"),
            Self::SetTitleBar(value) => write!(f, "SetTitleBar({value:?})"),
            Self::Ftcs(marker) => write!(f, "Ftcs ({marker})"),
            Self::RemoteHost(value) => write!(f, "RemoteHost ({value:?})"),
            Self::ResetCursorColor => write!(f, "ResetCursorColor"),
            Self::ITerm2 => write!(f, "ITerm2"),
            Self::SetClipboard(sel, content) => write!(f, "SetClipboard({sel:?}, {content:?})"),
            Self::QueryClipboard(sel) => write!(f, "QueryClipboard({sel:?})"),
            Self::SetPaletteColor(idx, r, g, b) => {
                write!(f, "SetPaletteColor({idx}, {r}, {g}, {b})")
            }
            Self::QueryPaletteColor(idx) => write!(f, "QueryPaletteColor({idx})"),
            Self::ResetPaletteColor(idx) => write!(f, "ResetPaletteColor({idx:?})"),
            Self::ResetForegroundColor => write!(f, "ResetForegroundColor"),
            Self::ResetBackgroundColor => write!(f, "ResetBackgroundColor"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnsiOscToken {
    OscValue(u16),
    String(String),
}

impl FromStr for AnsiOscToken {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        s.parse::<u16>().map_or_else(
            |_| Ok(Self::String(s.to_string())),
            |value| Ok(Self::OscValue(value)),
        )
    }
}
