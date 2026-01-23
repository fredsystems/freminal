// Copyright (C) 2024-2025 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

/// Erase mode for ED (Erase in Display) and EL (Erase in Line) operations
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EraseMode {
    /// Erase from cursor to end (ED 0, EL 0)
    ToEnd,
    /// Erase from beginning to cursor (ED 1, EL 1)
    ToBeginning,
    /// Erase entire display/line (ED 2, EL 2)
    All,
    /// Erase scrollback (ED 3)
    Scrollback,
}

/// Cursor movement direction
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorDirection {
    Up,
    Down,
    Forward,
    Backward,
}

/// Line insertion/deletion operations
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineOperation {
    Insert(usize),
    Delete(usize),
}

/// Character insertion/deletion operations
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CharOperation {
    Insert(usize),
    Delete(usize),
    Erase(usize),
}

/// High-level actions produced by the ANSI/OSC parser.
///
/// This enum represents normalized terminal effects (cursor movement,
/// erasures, SGR, window ops, etc.) emitted by parsing.
/// The set may grow; match exhaustively with a wildcard for forward-compat.
///
/// Note: Some variants contain types from freminal-terminal-emulator crate
/// (`SelectGraphicRendition`, Mode, etc.) which will be available when that
/// crate is in scope.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalOutput<SGR = (), MODE = (), OSC = (), DECSG = ()> {
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
    ApplicationKeypadMode,
    NormalKeypadMode,
    InsertLines(usize),
    Delete(usize),
    Erase(usize),
    Sgr(SGR),
    Data(Vec<u8>),
    Mode(MODE),
    InsertSpaces(usize),
    OscResponse(OSC),
    CursorReport,
    Invalid,
    Skipped,
    DecSpecialGraphics(DECSG),
    CursorVisualStyle(crate::cursor::CursorVisualStyle),
    WindowManipulation(crate::window_manipulation::WindowManipulation),
    RequestDeviceAttributes,
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
    },
    RequestXtVersion,
}
