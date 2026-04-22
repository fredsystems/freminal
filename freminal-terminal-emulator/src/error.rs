// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use std::str::Utf8Error;
use thiserror::Error;

/// Errors produced by the low-level ANSI parameter parsers in
/// [`crate::ansi`] and related modules.
#[derive(Debug, Error)]
pub enum AnsiParseError {
    /// A parameter byte slice was not valid UTF-8.
    #[error("parameter bytes were not valid UTF-8")]
    InvalidUtf8(#[from] Utf8Error),
    /// A parameter segment could not be parsed as the requested numeric type.
    #[error("failed to parse parameter {bytes:?} as {type_name}")]
    ParseFailed {
        /// Raw bytes that failed to parse.
        bytes: Vec<u8>,
        /// The Rust type name the caller was trying to parse into.
        type_name: &'static str,
    },
}

/// Errors produced while handling OSC (Operating System Command) sequences.
#[derive(Debug, Error)]
pub enum OscHandlerError {
    /// A sub-parameter inside an OSC payload failed to parse.
    #[error("failed to parse OSC sub-parameter")]
    ParamParse(#[from] AnsiParseError),
    /// An OSC dispatch site reached a branch that should be unreachable given
    /// the grammar. See 70.D.2.
    #[error("OSC dispatch reached an unreachable branch: {context}")]
    UnreachableDispatch {
        /// Free-form context identifying the call site.
        context: &'static str,
    },
}

/// Errors produced while handling CSI (Control Sequence Introducer) sequences.
#[derive(Debug, Error)]
pub enum CsiHandlerError {
    /// A CSI dispatch site reached a branch that should be unreachable given
    /// the grammar. See 70.D.3.
    #[error("CSI dispatch reached an unreachable branch: {context}")]
    UnreachableDispatch {
        /// Free-form context identifying the call site.
        context: &'static str,
    },
}

/// Errors produced by the [`crate::interface::TerminalEmulator`] public API.
#[derive(Debug, Error)]
pub enum InterfaceError {
    /// Failed to send a message to the PTY write channel.
    #[error("failed to send to PTY write channel: {0}")]
    PtySendFailed(String),
    /// PTY initialization failed while constructing a new emulator.
    #[error("failed to initialize PTY")]
    PtyInit(#[from] crate::io::pty::PtyInitError),
}

/// Errors produced by the `TerminalState` write path.
#[derive(Debug, Error)]
pub enum InternalStateError {
    /// Failed to send a message to the PTY write channel.
    #[error("failed to send to PTY write channel: {0}")]
    PtySendFailed(String),
}

#[derive(Debug, Error, Eq, PartialEq, Clone)]
#[error(transparent)]
pub enum ParserFailures {
    #[error("Parsed pushed to once finished")]
    ParsedPushedToOnceFinished,
    #[error("Unhandled Inner Escape: {0}")]
    UnhandledInnerEscape(String),
    #[error("Invalid cursor (CHA) set cursor position sequence: {0}")]
    UnhandledCHACommand(String),
    #[error("Invalid cursor (CUU) set position sequence: {0}")]
    UnhandledCUUCommand(String),
    #[error("Invalid cursor (CUB) move left: {0}")]
    UnhandledCUBCommand(String),
    #[error("Invalid cursor (CUD) set position sequence: {0}")]
    UnhandledCUDCommand(String),
    #[error("Invalid cursor (CUF) set position sequence: {0}")]
    UnhandledCUFCommand(String),
    #[error("Invalid cursor (CUP) set position sequence: {0:?}")]
    UnhandledCUPCommand(Vec<u8>),
    #[error("Invalid delete character (DCH) sequence: {0}")]
    UnhandledDCHCommand(String),
    #[error("Invalid erase character (ECH) sequence: {0}")]
    UnhandledECHCommand(String),
    #[error("Invalid cursor (ED) set position sequence: {0}")]
    UnhandledEDCommand(String),
    #[error("Invalid cursor (EL) set position sequence: {0}")]
    UnhandledELCommand(String),
    #[error("Invalid cursor (IL) set position sequence: {0}")]
    UnhandledILCommand(String),
    #[error("Invalid delete lines (DL) sequence: {0}")]
    UnhandledDLCommand(String),
    #[error("Unhandled SGR (Select Graphic Rendition) command: {0}")]
    UnhandledSGRCommand(String),
    #[error("Invalid cursor (ICH) set position sequence: {0}")]
    UnhandledICHCommand(String),
    #[error("Invalid TChar: {0:?}")]
    InvalidTChar(Vec<u8>),
    #[error("Invalid set cursor style (DECSCUSR) set position sequence: {0}")]
    UnhandledDECSCUSRCommand(String),
    #[error("Invalid window manipulation (DECSLPP) set position sequence: {0}")]
    UnhandledDECSLPPCommand(String),
    #[error("Invalid set margins (DECSTBM) set position sequence: {0}")]
    UnhandledDECSTBMCommand(String),
    #[error("Invalid set left/right margins (DECSLRM) sequence: {0}")]
    UnhandledDECSLRMCommand(String),
    #[error("Invalid set margins (DECRQM) set position sequence: {0:?}")]
    UnhandledDECRQMCommand(Vec<u8>),
    #[error("Invalid send device attributes (DA) set position sequence: {0}")]
    UnhandledDACommand(String),
    #[error("Invalid request device name and version (XTVERSION) set position sequence: {0}")]
    UnhandledXTVERSIONCommand(String),
    #[error("Invalid cursor (VPA) vertical position absolute sequence: {0}")]
    UnhandledVPACommand(String),
    #[error("Invalid cursor next line (CNL) sequence: {0}")]
    UnhandledCNLCommand(String),
    #[error("Invalid cursor previous line (CPL) sequence: {0}")]
    UnhandledCPLCommand(String),
    #[error("Invalid scroll up (SU) sequence: {0}")]
    UnhandledSUCommand(String),
    #[error("Invalid scroll down (SD) sequence: {0}")]
    UnhandledSDCommand(String),
    #[error("Invalid device status report (DSR) sequence: {0}")]
    UnhandledDSRCommand(String),
    #[error("Invalid tab clear (TBC) sequence: {0}")]
    UnhandledTBCCommand(String),
    #[error("Invalid cursor forward tabulation (CHT) sequence: {0}")]
    UnhandledCHTCommand(String),
    #[error("Invalid cursor backward tabulation (CBT) sequence: {0}")]
    UnhandledCBTCommand(String),
    #[error("Invalid repeat character (REP) sequence: {0}")]
    UnhandledREPCommand(String),
}
