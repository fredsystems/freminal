// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use std::convert::Infallible;
use std::str::FromStr;

use crate::buffer_states::{ftcs::FtcsMarker, pointer_shape::PointerShape, url::Url};
use std::fmt;

/// iTerm2 inline image dimension specification.
///
/// Used for `width` and `height` parameters in `OSC 1337 ; File=` sequences.
/// Possible values: `N` (cells), `Npx` (pixels), `N%` (percentage), `auto`.
#[derive(Eq, PartialEq, Debug, Clone)]
pub enum ImageDimension {
    /// Size in terminal cells.
    Cells(u32),
    /// Size in pixels.
    Pixels(u32),
    /// Size as a percentage of the terminal area.
    Percent(u32),
    /// Let the terminal decide automatically.
    Auto,
}

impl fmt::Display for ImageDimension {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cells(n) => write!(f, "{n}"),
            Self::Pixels(n) => write!(f, "{n}px"),
            Self::Percent(n) => write!(f, "{n}%"),
            Self::Auto => write!(f, "auto"),
        }
    }
}

impl ImageDimension {
    /// Parse an iTerm2 dimension spec string.
    ///
    /// Returns `None` if the string is empty or unparsable.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        if s.is_empty() || s.eq_ignore_ascii_case("auto") {
            return Some(Self::Auto);
        }

        s.strip_suffix("px").map_or_else(
            || {
                s.strip_suffix('%').map_or_else(
                    || s.parse::<u32>().ok().map(Self::Cells),
                    |rest| rest.parse::<u32>().ok().map(Self::Percent),
                )
            },
            |rest| rest.parse::<u32>().ok().map(Self::Pixels),
        )
    }
}

/// Parsed data from an iTerm2 `OSC 1337 ; File=` inline image sequence.
#[derive(Eq, PartialEq, Debug, Clone)]
pub struct ITerm2InlineImageData {
    /// Original filename (decoded from base64 name parameter), if provided.
    pub name: Option<String>,
    /// Declared file size in bytes, if provided.
    pub size: Option<usize>,
    /// Requested display width.
    pub width: Option<ImageDimension>,
    /// Requested display height.
    pub height: Option<ImageDimension>,
    /// Whether to preserve the image's aspect ratio (default: true).
    pub preserve_aspect_ratio: bool,
    /// Whether to display inline (true) or treat as download (false).
    pub inline: bool,
    /// When true, the cursor position is preserved after image placement
    /// (iTerm2 `doNotMoveCursor=1`).  Default: false.
    pub do_not_move_cursor: bool,
    /// Raw decoded file bytes (from base64 payload).
    pub data: Vec<u8>,
}

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
    /// OSC 12 — set or query the cursor color.
    CursorColor,
    ResetCursorColor,
    /// OSC 110 — reset text foreground color to the theme default.
    ResetForeground,
    /// OSC 111 — reset text background color to the theme default.
    ResetBackground,
    /// OSC 13 — mouse cursor foreground color (X11 concept; not applicable to
    /// GPU-rendered terminals).  Recognised and silently consumed.
    MouseForeground,
    /// OSC 14 — mouse cursor background color (X11 concept; not applicable to
    /// GPU-rendered terminals).  Recognised and silently consumed.
    MouseBackground,
    /// OSC 15 — Tektronix foreground color (legacy VT100 graphics mode;
    /// unimplemented).  Recognised and silently consumed.
    TekForeground,
    /// OSC 16 — Tektronix cursor/background color (legacy VT100 graphics mode;
    /// unimplemented).  Recognised and silently consumed.
    TekBackground,
    /// OSC 17 — highlight (selection) background color.  Recognised and
    /// silently consumed; candidate for future response implementation.
    HighlightBackground,
    /// OSC 19 — highlight (selection) foreground color.  Recognised and
    /// silently consumed; candidate for future response implementation.
    HighlightForeground,
    /// OSC 22 — set/reset the X11 pointer (mouse cursor) shape.  One-way
    /// command, no response expected.
    PointerShape,
    /// OSC 66 — Konsole/zsh color-scheme notification (one-way; no response).
    /// Silently consumed.
    ColorSchemeNotification,
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
            AnsiOscToken::OscValue(12) => Self::CursorColor,
            AnsiOscToken::OscValue(13) => Self::MouseForeground,
            AnsiOscToken::OscValue(14) => Self::MouseBackground,
            AnsiOscToken::OscValue(15) => Self::TekForeground,
            AnsiOscToken::OscValue(16) => Self::TekBackground,
            AnsiOscToken::OscValue(17) => Self::HighlightBackground,
            AnsiOscToken::OscValue(19) => Self::HighlightForeground,
            AnsiOscToken::OscValue(22) => Self::PointerShape,
            AnsiOscToken::OscValue(52) => Self::Clipboard,
            AnsiOscToken::OscValue(66) => Self::ColorSchemeNotification,
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
    // NOTE: OSC 0 and 2 are conflated as title-bar-only. If tabs are added,
    // OSC 0 should also set the icon name and OSC 2 should set only the title.
    SetTitleBar(String),
    Url(UrlResponse),
    RemoteHost(String),
    /// OSC 12 — query or set the cursor color.
    RequestColorQueryCursor(AnsiOscInternalType),
    ResetCursorColor,
    /// OSC 1337 File= inline image (iTerm2 protocol).
    ITerm2FileInline(ITerm2InlineImageData),
    /// OSC 1337 `MultipartFile`= begin (iTerm2 multipart protocol).
    /// Carries the metadata (name, size, width, height, etc.) with empty data.
    ITerm2MultipartBegin(ITerm2InlineImageData),
    /// OSC 1337 `FilePart`= chunk (iTerm2 multipart protocol).
    /// Carries decoded bytes from one base64-encoded chunk.
    ITerm2FilePart(Vec<u8>),
    /// OSC 1337 `FileEnd` (iTerm2 multipart protocol).
    /// Signals the end of a multipart file transfer.
    ITerm2FileEnd,
    /// OSC 1337 unrecognised sub-command (silently consumed).
    ITerm2Unknown,
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
    /// OSC 22 — set the pointer (mouse cursor) shape.
    ///
    /// An empty name or `"default"` resets to the OS default.
    SetPointerShape(PointerShape),
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
            Self::RequestColorQueryCursor(value) => {
                write!(f, "RequestColorQueryCursor({value:?})")
            }
            Self::ResetCursorColor => write!(f, "ResetCursorColor"),
            Self::ITerm2FileInline(data) => {
                write!(
                    f,
                    "ITerm2FileInline(name={:?}, size={:?}, {}B payload)",
                    data.name,
                    data.size,
                    data.data.len()
                )
            }
            Self::ITerm2MultipartBegin(data) => {
                write!(
                    f,
                    "ITerm2MultipartBegin(name={:?}, size={:?})",
                    data.name, data.size
                )
            }
            Self::ITerm2FilePart(bytes) => {
                write!(f, "ITerm2FilePart({}B)", bytes.len())
            }
            Self::ITerm2FileEnd => write!(f, "ITerm2FileEnd"),
            Self::ITerm2Unknown => write!(f, "ITerm2Unknown"),
            Self::SetClipboard(sel, content) => write!(f, "SetClipboard({sel:?}, {content:?})"),
            Self::QueryClipboard(sel) => write!(f, "QueryClipboard({sel:?})"),
            Self::SetPaletteColor(idx, r, g, b) => {
                write!(f, "SetPaletteColor({idx}, {r}, {g}, {b})")
            }
            Self::QueryPaletteColor(idx) => write!(f, "QueryPaletteColor({idx})"),
            Self::ResetPaletteColor(idx) => write!(f, "ResetPaletteColor({idx:?})"),
            Self::ResetForegroundColor => write!(f, "ResetForegroundColor"),
            Self::ResetBackgroundColor => write!(f, "ResetBackgroundColor"),
            Self::SetPointerShape(shape) => write!(f, "SetPointerShape({shape})"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnsiOscToken {
    OscValue(u16),
    String(String),
}

impl FromStr for AnsiOscToken {
    type Err = Infallible;

    fn from_str(s: &str) -> Result<Self, Infallible> {
        s.parse::<u16>().map_or_else(
            |_| Ok(Self::String(s.to_string())),
            |value| Ok(Self::OscValue(value)),
        )
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    // ------------------------------------------------------------------
    // ImageDimension::parse tests
    // ------------------------------------------------------------------

    #[test]
    fn parse_auto_lowercase() {
        assert_eq!(ImageDimension::parse("auto"), Some(ImageDimension::Auto));
    }

    #[test]
    fn parse_auto_mixed_case() {
        assert_eq!(ImageDimension::parse("Auto"), Some(ImageDimension::Auto));
        assert_eq!(ImageDimension::parse("AUTO"), Some(ImageDimension::Auto));
    }

    #[test]
    fn parse_empty_string_is_auto() {
        assert_eq!(ImageDimension::parse(""), Some(ImageDimension::Auto));
    }

    #[test]
    fn parse_whitespace_only_is_auto() {
        assert_eq!(ImageDimension::parse("   "), Some(ImageDimension::Auto));
    }

    #[test]
    fn parse_cells() {
        assert_eq!(ImageDimension::parse("10"), Some(ImageDimension::Cells(10)));
        assert_eq!(ImageDimension::parse("1"), Some(ImageDimension::Cells(1)));
        assert_eq!(ImageDimension::parse("0"), Some(ImageDimension::Cells(0)));
        assert_eq!(
            ImageDimension::parse("200"),
            Some(ImageDimension::Cells(200))
        );
    }

    #[test]
    fn parse_pixels() {
        assert_eq!(
            ImageDimension::parse("100px"),
            Some(ImageDimension::Pixels(100))
        );
        assert_eq!(
            ImageDimension::parse("0px"),
            Some(ImageDimension::Pixels(0))
        );
        assert_eq!(
            ImageDimension::parse("1920px"),
            Some(ImageDimension::Pixels(1920))
        );
    }

    #[test]
    fn parse_percent() {
        assert_eq!(
            ImageDimension::parse("50%"),
            Some(ImageDimension::Percent(50))
        );
        assert_eq!(
            ImageDimension::parse("100%"),
            Some(ImageDimension::Percent(100))
        );
        assert_eq!(
            ImageDimension::parse("0%"),
            Some(ImageDimension::Percent(0))
        );
    }

    #[test]
    fn parse_with_whitespace_padding() {
        assert_eq!(
            ImageDimension::parse("  10px  "),
            Some(ImageDimension::Pixels(10))
        );
        assert_eq!(
            ImageDimension::parse(" 50% "),
            Some(ImageDimension::Percent(50))
        );
        assert_eq!(
            ImageDimension::parse("  5  "),
            Some(ImageDimension::Cells(5))
        );
    }

    #[test]
    fn parse_invalid_returns_none() {
        assert_eq!(ImageDimension::parse("abc"), None);
        assert_eq!(ImageDimension::parse("pxpx"), None);
        assert_eq!(ImageDimension::parse("10em"), None);
        assert_eq!(ImageDimension::parse("-5"), None);
    }

    #[test]
    fn parse_invalid_numeric_suffix_returns_none() {
        // "abcpx" — the "abc" prefix is not a valid number
        assert_eq!(ImageDimension::parse("abcpx"), None);
        // "abc%" — the "abc" prefix is not a valid number
        assert_eq!(ImageDimension::parse("abc%"), None);
    }

    // ------------------------------------------------------------------
    // ImageDimension Display tests
    // ------------------------------------------------------------------

    #[test]
    fn display_cells() {
        assert_eq!(ImageDimension::Cells(10).to_string(), "10");
    }

    #[test]
    fn display_pixels() {
        assert_eq!(ImageDimension::Pixels(100).to_string(), "100px");
    }

    #[test]
    fn display_percent() {
        assert_eq!(ImageDimension::Percent(50).to_string(), "50%");
    }

    #[test]
    fn display_auto() {
        assert_eq!(ImageDimension::Auto.to_string(), "auto");
    }

    // ------------------------------------------------------------------
    // AnsiOscInternalType Display tests
    // ------------------------------------------------------------------

    #[test]
    fn display_osc_internal_query() {
        assert_eq!(AnsiOscInternalType::Query.to_string(), "Query");
    }

    #[test]
    fn display_osc_internal_unknown_none() {
        let s = AnsiOscInternalType::Unknown(None).to_string();
        assert!(s.contains("Unknown"), "got: {s}");
    }

    #[test]
    fn display_osc_internal_unknown_some() {
        let token = AnsiOscToken::String("hello".to_string());
        let s = AnsiOscInternalType::Unknown(Some(token)).to_string();
        assert!(s.contains("Unknown"), "got: {s}");
    }

    #[test]
    fn display_osc_internal_string() {
        let s = AnsiOscInternalType::String("xterm".to_string()).to_string();
        assert_eq!(s, "xterm");
    }

    // ------------------------------------------------------------------
    // UrlResponse Display tests
    // ------------------------------------------------------------------

    #[test]
    fn display_url_response_end() {
        assert_eq!(UrlResponse::End.to_string(), "End");
    }

    #[test]
    fn display_url_response_url_with_id() {
        use crate::buffer_states::url::Url;
        let r = UrlResponse::Url(Url {
            id: Some("myid".to_string()),
            url: "https://example.com".to_string(),
        });
        let s = r.to_string();
        assert!(s.contains("Url"), "got: {s}");
        assert!(s.contains("myid"), "got: {s}");
    }

    #[test]
    fn display_url_response_url_no_id() {
        use crate::buffer_states::url::Url;
        let r = UrlResponse::Url(Url {
            id: None,
            url: "https://example.com".to_string(),
        });
        let s = r.to_string();
        assert!(s.contains("Url"), "got: {s}");
    }

    // ------------------------------------------------------------------
    // From<Vec<Option<AnsiOscToken>>> for UrlResponse tests
    // ------------------------------------------------------------------

    #[test]
    fn url_response_from_tokens_with_id() {
        let tokens = vec![
            Some(AnsiOscToken::OscValue(8)),
            Some(AnsiOscToken::String("myid".to_string())),
            Some(AnsiOscToken::String("https://example.com".to_string())),
        ];
        let r = UrlResponse::from(tokens);
        match r {
            UrlResponse::Url(url) => {
                assert_eq!(url.id, Some("myid".to_string()));
                assert_eq!(url.url, "https://example.com");
            }
            UrlResponse::End => panic!("expected Url, got End"),
        }
    }

    #[test]
    fn url_response_from_tokens_without_id() {
        let tokens = vec![
            Some(AnsiOscToken::OscValue(8)),
            None,
            Some(AnsiOscToken::String("https://example.com".to_string())),
        ];
        let r = UrlResponse::from(tokens);
        match r {
            UrlResponse::Url(url) => {
                assert_eq!(url.id, None);
                assert_eq!(url.url, "https://example.com");
            }
            UrlResponse::End => panic!("expected Url, got End"),
        }
    }

    #[test]
    fn url_response_from_tokens_end() {
        // A pattern that doesn't match either URL arm → End
        let tokens: Vec<Option<AnsiOscToken>> = vec![None, None];
        let r = UrlResponse::from(tokens);
        assert_eq!(r, UrlResponse::End);
    }

    #[test]
    fn url_response_from_empty_tokens_is_end() {
        let tokens: Vec<Option<AnsiOscToken>> = vec![];
        let r = UrlResponse::from(tokens);
        assert_eq!(r, UrlResponse::End);
    }

    // ------------------------------------------------------------------
    // AnsiOscType Display tests
    // ------------------------------------------------------------------

    #[test]
    fn display_ansi_osc_type_noop() {
        assert_eq!(AnsiOscType::NoOp.to_string(), "NoOp");
    }

    #[test]
    fn display_ansi_osc_set_palette_color() {
        let s = AnsiOscType::SetPaletteColor(1, 255, 128, 0).to_string();
        assert!(s.contains("SetPaletteColor"), "got: {s}");
        assert!(s.contains("255"), "got: {s}");
    }

    #[test]
    fn display_ansi_osc_set_pointer_shape() {
        let s =
            AnsiOscType::SetPointerShape(crate::buffer_states::pointer_shape::PointerShape::Text)
                .to_string();
        assert!(s.contains("SetPointerShape"), "got: {s}");
        assert!(s.contains("text"), "got: {s}");
    }

    #[test]
    fn display_ansi_osc_set_clipboard() {
        let s = AnsiOscType::SetClipboard("c".to_string(), "hello".to_string()).to_string();
        assert!(s.contains("SetClipboard"), "got: {s}");
    }

    #[test]
    fn display_ansi_osc_query_clipboard() {
        let s = AnsiOscType::QueryClipboard("c".to_string()).to_string();
        assert!(s.contains("QueryClipboard"), "got: {s}");
    }

    #[test]
    fn display_ansi_osc_reset_palette_color_none() {
        let s = AnsiOscType::ResetPaletteColor(None).to_string();
        assert!(s.contains("ResetPaletteColor"), "got: {s}");
    }

    #[test]
    fn display_ansi_osc_reset_palette_color_some() {
        let s = AnsiOscType::ResetPaletteColor(Some(3)).to_string();
        assert!(s.contains("ResetPaletteColor"), "got: {s}");
    }

    #[test]
    fn display_ansi_osc_reset_foreground_color() {
        assert_eq!(
            AnsiOscType::ResetForegroundColor.to_string(),
            "ResetForegroundColor"
        );
    }

    #[test]
    fn display_ansi_osc_reset_background_color() {
        assert_eq!(
            AnsiOscType::ResetBackgroundColor.to_string(),
            "ResetBackgroundColor"
        );
    }

    #[test]
    fn display_ansi_osc_reset_cursor_color() {
        assert_eq!(
            AnsiOscType::ResetCursorColor.to_string(),
            "ResetCursorColor"
        );
    }

    #[test]
    fn display_ansi_osc_iterm2_file_end() {
        assert_eq!(AnsiOscType::ITerm2FileEnd.to_string(), "ITerm2FileEnd");
    }

    #[test]
    fn display_ansi_osc_iterm2_unknown() {
        assert_eq!(AnsiOscType::ITerm2Unknown.to_string(), "ITerm2Unknown");
    }

    // ------------------------------------------------------------------
    // AnsiOscType Display: remaining uncovered variants
    // ------------------------------------------------------------------

    #[test]
    fn display_ansi_osc_request_color_query_background() {
        let s = AnsiOscType::RequestColorQueryBackground(AnsiOscInternalType::Query).to_string();
        assert!(s.contains("RequestColorQueryBackground"), "got: {s}");
    }

    #[test]
    fn display_ansi_osc_request_color_query_foreground() {
        let s = AnsiOscType::RequestColorQueryForeground(AnsiOscInternalType::Query).to_string();
        assert!(s.contains("RequestColorQueryForeground"), "got: {s}");
    }

    #[test]
    fn display_ansi_osc_ftcs() {
        use crate::buffer_states::ftcs::FtcsMarker;
        let s = AnsiOscType::Ftcs(FtcsMarker::PromptStart).to_string();
        assert!(s.contains("Ftcs"), "got: {s}");
    }

    #[test]
    fn display_ansi_osc_set_title_bar() {
        let s = AnsiOscType::SetTitleBar("my title".to_string()).to_string();
        assert!(s.contains("SetTitleBar"), "got: {s}");
        assert!(s.contains("my title"), "got: {s}");
    }

    #[test]
    fn display_ansi_osc_url() {
        use crate::buffer_states::url::Url;
        let s = AnsiOscType::Url(UrlResponse::Url(Url {
            id: None,
            url: "https://example.com".to_string(),
        }))
        .to_string();
        assert!(s.contains("Url"), "got: {s}");
    }

    #[test]
    fn display_ansi_osc_remote_host() {
        let s = AnsiOscType::RemoteHost("user@host".to_string()).to_string();
        assert!(s.contains("RemoteHost"), "got: {s}");
    }

    #[test]
    fn display_ansi_osc_request_color_query_cursor() {
        let s = AnsiOscType::RequestColorQueryCursor(AnsiOscInternalType::Query).to_string();
        assert!(s.contains("RequestColorQueryCursor"), "got: {s}");
    }

    #[test]
    fn display_ansi_osc_iterm2_file_inline() {
        let data = ITerm2InlineImageData {
            name: Some("test.png".to_string()),
            size: Some(1024),
            width: None,
            height: None,
            preserve_aspect_ratio: true,
            inline: true,
            do_not_move_cursor: false,
            data: vec![1, 2, 3],
        };
        let s = AnsiOscType::ITerm2FileInline(data).to_string();
        assert!(s.contains("ITerm2FileInline"), "got: {s}");
        assert!(s.contains("test.png"), "got: {s}");
    }

    #[test]
    fn display_ansi_osc_iterm2_multipart_begin() {
        let data = ITerm2InlineImageData {
            name: Some("big.png".to_string()),
            size: Some(4096),
            width: None,
            height: None,
            preserve_aspect_ratio: true,
            inline: true,
            do_not_move_cursor: false,
            data: vec![],
        };
        let s = AnsiOscType::ITerm2MultipartBegin(data).to_string();
        assert!(s.contains("ITerm2MultipartBegin"), "got: {s}");
        assert!(s.contains("big.png"), "got: {s}");
    }

    #[test]
    fn display_ansi_osc_iterm2_file_part() {
        let s = AnsiOscType::ITerm2FilePart(vec![0u8; 16]).to_string();
        assert!(s.contains("ITerm2FilePart"), "got: {s}");
        assert!(s.contains("16"), "got: {s}");
    }

    #[test]
    fn display_ansi_osc_query_palette_color() {
        let s = AnsiOscType::QueryPaletteColor(7).to_string();
        assert!(s.contains("QueryPaletteColor"), "got: {s}");
        assert!(s.contains('7'), "got: {s}");
    }

    // ------------------------------------------------------------------
    // OscTarget::from(&AnsiOscToken) — cover the From impl arms
    // ------------------------------------------------------------------

    #[test]
    fn osc_target_from_token_title_bar() {
        assert_eq!(
            OscTarget::from(&AnsiOscToken::OscValue(0)),
            OscTarget::TitleBar
        );
        assert_eq!(
            OscTarget::from(&AnsiOscToken::OscValue(2)),
            OscTarget::TitleBar
        );
    }

    #[test]
    fn osc_target_from_token_icon_name() {
        assert_eq!(
            OscTarget::from(&AnsiOscToken::OscValue(1)),
            OscTarget::IconName
        );
    }

    #[test]
    fn osc_target_from_token_palette_color() {
        assert_eq!(
            OscTarget::from(&AnsiOscToken::OscValue(4)),
            OscTarget::PaletteColor
        );
    }

    #[test]
    fn osc_target_from_token_remote_host() {
        assert_eq!(
            OscTarget::from(&AnsiOscToken::OscValue(7)),
            OscTarget::RemoteHost
        );
    }

    #[test]
    fn osc_target_from_token_url() {
        assert_eq!(OscTarget::from(&AnsiOscToken::OscValue(8)), OscTarget::Url);
    }

    #[test]
    fn osc_target_from_token_background() {
        assert_eq!(
            OscTarget::from(&AnsiOscToken::OscValue(11)),
            OscTarget::Background
        );
    }

    #[test]
    fn osc_target_from_token_foreground() {
        assert_eq!(
            OscTarget::from(&AnsiOscToken::OscValue(10)),
            OscTarget::Foreground
        );
    }

    #[test]
    fn osc_target_from_token_cursor_color() {
        assert_eq!(
            OscTarget::from(&AnsiOscToken::OscValue(12)),
            OscTarget::CursorColor
        );
    }

    #[test]
    fn osc_target_from_token_mouse_fg_bg_tek() {
        assert_eq!(
            OscTarget::from(&AnsiOscToken::OscValue(13)),
            OscTarget::MouseForeground
        );
        assert_eq!(
            OscTarget::from(&AnsiOscToken::OscValue(14)),
            OscTarget::MouseBackground
        );
        assert_eq!(
            OscTarget::from(&AnsiOscToken::OscValue(15)),
            OscTarget::TekForeground
        );
        assert_eq!(
            OscTarget::from(&AnsiOscToken::OscValue(16)),
            OscTarget::TekBackground
        );
    }

    #[test]
    fn osc_target_from_token_highlight() {
        assert_eq!(
            OscTarget::from(&AnsiOscToken::OscValue(17)),
            OscTarget::HighlightBackground
        );
        assert_eq!(
            OscTarget::from(&AnsiOscToken::OscValue(19)),
            OscTarget::HighlightForeground
        );
    }

    #[test]
    fn osc_target_from_token_pointer_shape() {
        assert_eq!(
            OscTarget::from(&AnsiOscToken::OscValue(22)),
            OscTarget::PointerShape
        );
    }

    #[test]
    fn osc_target_from_token_clipboard() {
        assert_eq!(
            OscTarget::from(&AnsiOscToken::OscValue(52)),
            OscTarget::Clipboard
        );
    }

    #[test]
    fn osc_target_from_token_color_scheme_notification() {
        assert_eq!(
            OscTarget::from(&AnsiOscToken::OscValue(66)),
            OscTarget::ColorSchemeNotification
        );
    }

    #[test]
    fn osc_target_from_token_reset_palette() {
        assert_eq!(
            OscTarget::from(&AnsiOscToken::OscValue(104)),
            OscTarget::ResetPaletteColor
        );
    }

    #[test]
    fn osc_target_from_token_reset_cursor() {
        assert_eq!(
            OscTarget::from(&AnsiOscToken::OscValue(112)),
            OscTarget::ResetCursorColor
        );
    }

    #[test]
    fn osc_target_from_token_ftcs() {
        assert_eq!(
            OscTarget::from(&AnsiOscToken::OscValue(133)),
            OscTarget::Ftcs
        );
    }

    #[test]
    fn osc_target_from_token_iterm2() {
        assert_eq!(
            OscTarget::from(&AnsiOscToken::OscValue(1337)),
            OscTarget::ITerm2
        );
    }

    #[test]
    fn osc_target_from_token_reset_fg_bg() {
        assert_eq!(
            OscTarget::from(&AnsiOscToken::OscValue(110)),
            OscTarget::ResetForeground
        );
        assert_eq!(
            OscTarget::from(&AnsiOscToken::OscValue(111)),
            OscTarget::ResetBackground
        );
    }

    #[test]
    fn osc_target_from_token_unknown() {
        assert_eq!(
            OscTarget::from(&AnsiOscToken::OscValue(999)),
            OscTarget::Unknown
        );
        assert_eq!(
            OscTarget::from(&AnsiOscToken::String("something".to_string())),
            OscTarget::Unknown
        );
    }
}
