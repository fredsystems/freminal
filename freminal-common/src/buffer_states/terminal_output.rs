// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use crate::{
    buffer_states::{
        line_draw::DecSpecialGraphics, mode::Mode, osc::AnsiOscType,
        window_manipulation::WindowManipulation,
    },
    cursor::CursorVisualStyle,
    sgr::SelectGraphicRendition,
};

/// High-level actions produced by the ANSI/OSC parser.
///
/// This enum represents normalized terminal effects (cursor movement,
/// erasures, SGR, window ops, etc.) emitted by parsing.
/// The set may grow; match exhaustively with a wildcard for forward-compat.
///
/// All referenced types (`SelectGraphicRendition`, `Mode`, etc.) are defined
/// within this crate (`freminal-common`).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalOutput {
    SetCursorPos {
        x: Option<usize>,
        y: Option<usize>,
    },
    SetCursorPosRel {
        x: Option<i32>,
        y: Option<i32>,
    },
    ClearDisplayfromCursortoEndofDisplay,
    ClearDisplayfromStartofDisplaytoCursor,
    ClearScrollbackandDisplay,
    ClearDisplay,
    CarriageReturn,
    ClearLineForwards,
    ClearLineBackwards,
    ClearLine,
    Newline,
    Backspace,
    Bell,
    Tab,
    ApplicationKeypadMode,
    NormalKeypadMode,
    InsertLines(usize),
    DeleteLines(usize),
    /// SU — Scroll Up Ps lines (content moves up, blank at bottom)
    ScrollUp(usize),
    /// SD — Scroll Down Ps lines (content moves down, blank at top)
    ScrollDown(usize),
    Delete(usize),
    Erase(usize),
    Sgr(SelectGraphicRendition),
    Data(Vec<u8>),
    Mode(Mode),
    // ich (8.3.64 of ecma-48)
    InsertSpaces(usize),
    OscResponse(AnsiOscType),
    CursorReport,
    /// DSR ?996 — Color theme query.
    /// Respond with `CSI ? 997 ; Ps n` where Ps = 1 (light) or 2 (dark).
    ColorThemeReport,
    DeviceStatusReport,
    Invalid,
    Skipped,
    DecSpecialGraphics(DecSpecialGraphics),
    CursorVisualStyle(CursorVisualStyle),
    WindowManipulation(WindowManipulation),
    RequestDeviceAttributes,
    SetLeftAndRightMargins {
        left_margin: usize,
        right_margin: usize,
    },
    SetTopAndBottomMargins {
        top_margin: usize,
        bottom_margin: usize,
    },
    EightBitControl,
    SevenBitControl,
    AnsiConformanceLevelOne,
    AnsiConformanceLevelTwo,
    AnsiConformanceLevelThree,
    DoubleLineHeightTop,
    DoubleLineHeightBottom,
    SingleWidthLine,
    DoubleWidthLine,
    ScreenAlignmentTest,
    CharsetDefault,
    CharsetUTF8,
    CharsetG0,
    CharsetG1,
    CharsetG1AsGR,
    CharsetG2,
    CharsetG2AsGR,
    CharsetG2AsGL,
    CharsetG3,
    CharsetG3AsGR,
    CharsetG3AsGL,
    DecSpecial,
    CharsetUK,
    CharsetUS,
    CharsetUSASCII,
    CharsetDutch,
    CharsetFinnish,
    CharsetFrench,
    CharsetFrenchCanadian,
    CharsetGerman,
    CharsetItalian,
    CharsetNorwegianDanish,
    CharsetSpanish,
    CharsetSwedish,
    CharsetSwiss,
    SaveCursor,
    RestoreCursor,
    CursorToLowerLeftCorner,
    ResetDevice,
    MemoryLock,
    MemoryUnlock,
    DeviceControlString(Vec<u8>),
    ApplicationProgramCommand(Vec<u8>),
    RequestDeviceNameAndVersion,
    RequestSecondaryDeviceAttributes {
        param: usize,
    }, // for ESC[>c / ESC[>Ps c
    /// ESC D — IND (Index): move cursor down, scroll if at bottom margin
    Index,
    /// ESC M — RI (Reverse Index): move cursor up, scroll if at top margin
    ReverseIndex,
    /// ESC E — NEL (Next Line): move cursor to col 0 of next line, scroll if at bottom
    NextLine,
    /// ESC H — HTS (Horizontal Tab Set): set a tab stop at the current cursor column
    HorizontalTabSet,
    /// CSI Ps g — TBC (Tab Clear): Ps=0 clear at current column, Ps=3 clear all
    TabClear(usize),
    /// CSI Ps I — CHT (Cursor Forward Tabulation): advance cursor by Ps tab stops
    CursorForwardTab(usize),
    /// CSI Ps Z — CBT (Cursor Backward Tabulation): move cursor back by Ps tab stops
    CursorBackwardTab(usize),
    /// CSI Ps b — REP (Repeat): repeat the preceding graphic character Ps times
    RepeatCharacter(usize),
    /// CSI ? u — Kitty keyboard protocol query.
    /// Respond with `CSI ? 0 u` (mode flags = 0, protocol not active).
    KittyKeyboardQuery,
    /// CSI > 4 ; Pv m — xterm `modifyOtherKeys` resource.
    ///
    /// Level 0: disabled (default).
    /// Level 1: modified keys that would produce control chars get extended format.
    /// Level 2: ALL modified keys get extended format.
    ModifyOtherKeys(u8),
    /// CSI = c — Tertiary Device Attributes (DA3, VT400+).
    /// Respond with `DCS ! | <8 hex digits> ST`.
    RequestTertiaryDeviceAttributes,
    /// CSI Ps x — DECREQTPARM: Request Terminal Parameters.
    /// Ps=0 → respond with `CSI 2 ; ... x`; Ps=1 → respond with `CSI 3 ; ... x`.
    RequestTerminalParameters(u8),
    /// ENQ (0x05) — transmit the answerback message back to the PTY.
    ///
    /// The VT100 spec requires the terminal to send its configured answerback
    /// string when it receives ENQ.  Most modern terminals respond with an
    /// empty string.
    Enq,
}

// Inherently large: exhaustive `Display` impl for all `TerminalOutput` variants used in
// diagnostic output. Each arm is a single format call; splitting is not warranted.
#[allow(clippy::too_many_lines)]
impl std::fmt::Display for TerminalOutput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SetCursorPos { x, y } => {
                write!(f, "SetCursorPos: x: {x:?}, y: {y:?}")
            }
            Self::SetCursorPosRel { x, y } => {
                write!(f, "SetCursorPosRel: x: {x:?}, y: {y:?}")
            }
            Self::ClearDisplayfromCursortoEndofDisplay => write!(f, "ClearForwards"),
            Self::ClearScrollbackandDisplay => write!(f, "ClearAll"),
            Self::ClearDisplayfromStartofDisplaytoCursor => write!(f, "ClearBackwards"),
            Self::ClearDisplay => write!(f, "ClearDisplay"),
            Self::CarriageReturn => write!(f, "CarriageReturn"),
            Self::ClearLineForwards => write!(f, "ClearLineForwards"),
            Self::ClearLineBackwards => write!(f, "ClearLineBackwards"),
            Self::ClearLine => write!(f, "ClearLine"),
            Self::Newline => write!(f, "Newline"),
            Self::Backspace => write!(f, "Backspace"),
            Self::Bell => write!(f, "Bell"),
            Self::Tab => write!(f, "Tab"),
            Self::InsertLines(n) => write!(f, "InsertLines({n})"),
            Self::DeleteLines(n) => write!(f, "DeleteLines({n})"),
            Self::ScrollUp(n) => write!(f, "ScrollUp({n})"),
            Self::ScrollDown(n) => write!(f, "ScrollDown({n})"),
            Self::Delete(n) => write!(f, "Delete({n})"),
            Self::Erase(n) => write!(f, "Erase({n})"),
            Self::Sgr(sgr) => write!(f, "Sgr({sgr:?})"),
            Self::Data(data) => {
                write!(f, "Data({})", String::from_utf8_lossy(data))
            }
            Self::Mode(mode) => write!(f, "SetMode({mode})"),
            Self::InsertSpaces(n) => write!(f, "InsertSpaces({n})"),
            Self::OscResponse(n) => write!(f, "OscResponse({n})"),
            Self::DecSpecialGraphics(dec_special_graphics) => {
                write!(f, "DecSpecialGraphics({dec_special_graphics:?})")
            }
            Self::Invalid => write!(f, "Invalid"),
            Self::CursorReport => write!(f, "CursorReport"),
            Self::ColorThemeReport => write!(f, "ColorThemeReport"),
            Self::DeviceStatusReport => write!(f, "DeviceStatusReport"),
            Self::Skipped => write!(f, "Skipped"),
            Self::ApplicationKeypadMode => write!(f, "ApplicationKeypadMode"),
            Self::NormalKeypadMode => write!(f, "NormalKeypadMode"),
            Self::CursorVisualStyle(cursor_visual_style) => {
                write!(f, "CursorVisualStyle({cursor_visual_style:?})")
            }
            Self::WindowManipulation(window_manipulation) => {
                write!(f, "WindowManipulation({window_manipulation:?})")
            }
            Self::SetLeftAndRightMargins {
                left_margin,
                right_margin,
            } => {
                write!(f, "SetLeftAndRightMargins({left_margin}, {right_margin})")
            }
            Self::SetTopAndBottomMargins {
                top_margin,
                bottom_margin,
            } => {
                write!(f, "SetTopAndBottomMargins({top_margin}, {bottom_margin})")
            }
            Self::RequestDeviceAttributes => write!(f, "RequestDeviceAttributes"),
            Self::EightBitControl => write!(f, "EightBitControl"),
            Self::SevenBitControl => write!(f, "SevenBitControl"),
            Self::AnsiConformanceLevelOne => write!(f, "AnsiConformanceLevelOne"),
            Self::AnsiConformanceLevelTwo => write!(f, "AnsiConformanceLevelTwo"),
            Self::AnsiConformanceLevelThree => write!(f, "AnsiConformanceLevelThree"),
            Self::DoubleLineHeightTop => write!(f, "DoubleLineHeightTop"),
            Self::DoubleLineHeightBottom => write!(f, "DoubleLineHeightBottom"),
            Self::SingleWidthLine => write!(f, "SingleWidthLine"),
            Self::DoubleWidthLine => write!(f, "DoubleWidthLine"),
            Self::ScreenAlignmentTest => write!(f, "ScreenAlignmentTest"),
            Self::CharsetDefault => write!(f, "CharsetDefault"),
            Self::CharsetUTF8 => write!(f, "CharsetUTF8"),
            Self::CharsetG0 => write!(f, "CharsetG0"),
            Self::CharsetG1 => write!(f, "CharsetG1"),
            Self::CharsetG1AsGR => write!(f, "CharsetG1AsGR"),
            Self::CharsetG2 => write!(f, "CharsetG2"),
            Self::CharsetG2AsGR => write!(f, "CharsetG2AsGR"),
            Self::CharsetG2AsGL => write!(f, "CharsetG2AsGL"),
            Self::CharsetG3 => write!(f, "CharsetG3"),
            Self::CharsetG3AsGR => write!(f, "CharsetG3AsGR"),
            Self::CharsetG3AsGL => write!(f, "CharsetG3AsGL"),
            Self::DecSpecial => write!(f, "DecSpecial"),
            Self::CharsetUK => write!(f, "CharsetUK"),
            Self::CharsetUS => write!(f, "CharsetUS"),
            Self::CharsetUSASCII => write!(f, "CharsetUSASCII"),
            Self::CharsetDutch => write!(f, "CharsetDutch"),
            Self::CharsetFinnish => write!(f, "CharsetFinnish"),
            Self::CharsetFrench => write!(f, "CharsetFrench"),
            Self::CharsetFrenchCanadian => write!(f, "CharsetFrenchCanadian"),
            Self::CharsetGerman => write!(f, "CharsetGerman"),
            Self::CharsetItalian => write!(f, "CharsetItalian"),
            Self::CharsetNorwegianDanish => write!(f, "CharsetNorwegianDanish"),
            Self::CharsetSpanish => write!(f, "CharsetSpanish"),
            Self::CharsetSwedish => write!(f, "CharsetSwedish"),
            Self::CharsetSwiss => write!(f, "CharsetSwiss"),
            Self::SaveCursor => write!(f, "SaveCursor"),
            Self::RestoreCursor => write!(f, "RestoreCursor"),
            Self::CursorToLowerLeftCorner => write!(f, "CursorToLowerLeftCorner"),
            Self::ResetDevice => write!(f, "ResetDevice"),
            Self::MemoryLock => write!(f, "MemoryLock"),
            Self::MemoryUnlock => write!(f, "MemoryUnlock"),
            Self::DeviceControlString(data) => {
                write!(f, "DeviceControlString({})", String::from_utf8_lossy(data))
            }
            Self::ApplicationProgramCommand(data) => {
                write!(
                    f,
                    "ApplicationProgramCommand({})",
                    String::from_utf8_lossy(data)
                )
            }
            Self::RequestDeviceNameAndVersion => write!(f, "RequestDeviceNameandVersion"),
            Self::RequestSecondaryDeviceAttributes { param } => {
                write!(f, "RequestSecondaryDeviceAttributes({param})")
            }
            Self::Index => write!(f, "Index"),
            Self::ReverseIndex => write!(f, "ReverseIndex"),
            Self::NextLine => write!(f, "NextLine"),
            Self::HorizontalTabSet => write!(f, "HorizontalTabSet"),
            Self::TabClear(n) => write!(f, "TabClear({n})"),
            Self::CursorForwardTab(n) => write!(f, "CursorForwardTab({n})"),
            Self::CursorBackwardTab(n) => write!(f, "CursorBackwardTab({n})"),
            Self::RepeatCharacter(n) => write!(f, "RepeatCharacter({n})"),
            Self::KittyKeyboardQuery => write!(f, "KittyKeyboardQuery"),
            Self::ModifyOtherKeys(level) => write!(f, "ModifyOtherKeys({level})"),
            Self::RequestTertiaryDeviceAttributes => write!(f, "RequestTertiaryDeviceAttributes"),
            Self::RequestTerminalParameters(ps) => write!(f, "RequestTerminalParameters({ps})"),
            Self::Enq => write!(f, "Enq"),
        }
    }
}
