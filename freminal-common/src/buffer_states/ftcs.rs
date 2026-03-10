// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! OSC 133 — `FinalTerm` / Shell Integration (FTCS) protocol.
//!
//! These types represent the four FTCS markers that shells emit to delineate
//! prompt, command, and output regions:
//!
//! - `OSC 133 ; A ST` — Prompt start
//! - `OSC 133 ; B ST` — Prompt end / command input start
//! - `OSC 133 ; C ST` — Command end / output start (pre-execution)
//! - `OSC 133 ; D [; exitcode] ST` — Command finished (with optional exit code)
//!
//! The terminal stores these as `FtcsMarker` values alongside cursor positions
//! to track prompt/command/output boundaries.

use std::fmt;

/// A single FTCS marker, as emitted by the shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FtcsMarker {
    /// `A` — Prompt start.  The shell is about to draw the prompt.
    PromptStart,

    /// `B` — Prompt end / command input start.  The user can now type.
    CommandStart,

    /// `C` — Command end / output start.  The shell is about to execute the
    /// command; everything after this until `D` is command output.
    OutputStart,

    /// `D` — Command finished.  Carries an optional exit code (`0` = success).
    CommandFinished(Option<i32>),
}

impl fmt::Display for FtcsMarker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PromptStart => write!(f, "A"),
            Self::CommandStart => write!(f, "B"),
            Self::OutputStart => write!(f, "C"),
            Self::CommandFinished(Some(code)) => write!(f, "D;{code}"),
            Self::CommandFinished(None) => write!(f, "D"),
        }
    }
}

/// The current state of the shell integration state machine.
///
/// Tracks which FTCS region the terminal is currently inside, based on the
/// most recent marker received.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FtcsState {
    /// No FTCS markers have been received yet, or the state machine has been
    /// reset.
    #[default]
    None,

    /// Inside a prompt region (after `A`, before `B`).
    InPrompt,

    /// Inside a command-input region (after `B`, before `C`).
    InCommand,

    /// Inside command output (after `C`, before `D`).
    InOutput,
}

impl fmt::Display for FtcsState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => write!(f, "None"),
            Self::InPrompt => write!(f, "InPrompt"),
            Self::InCommand => write!(f, "InCommand"),
            Self::InOutput => write!(f, "InOutput"),
        }
    }
}

/// Parse an FTCS marker from the semicolon-delimited parameter tokens that
/// follow `OSC 133`.
///
/// The input `params` contains the portions after the `133` prefix, split on
/// `;`.  For example:
/// - `OSC 133 ; A ST`       → `params = ["A"]`
/// - `OSC 133 ; D ; 0 ST`   → `params = ["D", "0"]`
///
/// Returns `None` for unrecognised or empty parameter lists.
#[must_use]
pub fn parse_ftcs_params(params: &[&str]) -> Option<FtcsMarker> {
    let marker_char = params.first()?;
    match *marker_char {
        "A" => Some(FtcsMarker::PromptStart),
        "B" => Some(FtcsMarker::CommandStart),
        "C" => Some(FtcsMarker::OutputStart),
        "D" => {
            let exit_code = params.get(1).and_then(|s| s.parse::<i32>().ok());
            Some(FtcsMarker::CommandFinished(exit_code))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── FtcsMarker Display ──────────────────────────────────────────────

    #[test]
    fn display_prompt_start() {
        assert_eq!(FtcsMarker::PromptStart.to_string(), "A");
    }

    #[test]
    fn display_command_start() {
        assert_eq!(FtcsMarker::CommandStart.to_string(), "B");
    }

    #[test]
    fn display_output_start() {
        assert_eq!(FtcsMarker::OutputStart.to_string(), "C");
    }

    #[test]
    fn display_command_finished_with_code() {
        assert_eq!(FtcsMarker::CommandFinished(Some(0)).to_string(), "D;0");
        assert_eq!(FtcsMarker::CommandFinished(Some(127)).to_string(), "D;127");
        assert_eq!(FtcsMarker::CommandFinished(Some(-1)).to_string(), "D;-1");
    }

    #[test]
    fn display_command_finished_without_code() {
        assert_eq!(FtcsMarker::CommandFinished(None).to_string(), "D");
    }

    // ── FtcsState Display ───────────────────────────────────────────────

    #[test]
    fn state_display() {
        assert_eq!(FtcsState::None.to_string(), "None");
        assert_eq!(FtcsState::InPrompt.to_string(), "InPrompt");
        assert_eq!(FtcsState::InCommand.to_string(), "InCommand");
        assert_eq!(FtcsState::InOutput.to_string(), "InOutput");
    }

    #[test]
    fn state_default_is_none() {
        assert_eq!(FtcsState::default(), FtcsState::None);
    }

    // ── parse_ftcs_params ───────────────────────────────────────────────

    #[test]
    fn parse_prompt_start() {
        assert_eq!(parse_ftcs_params(&["A"]), Some(FtcsMarker::PromptStart));
    }

    #[test]
    fn parse_command_start() {
        assert_eq!(parse_ftcs_params(&["B"]), Some(FtcsMarker::CommandStart));
    }

    #[test]
    fn parse_output_start() {
        assert_eq!(parse_ftcs_params(&["C"]), Some(FtcsMarker::OutputStart));
    }

    #[test]
    fn parse_command_finished_with_exit_code() {
        assert_eq!(
            parse_ftcs_params(&["D", "0"]),
            Some(FtcsMarker::CommandFinished(Some(0)))
        );
        assert_eq!(
            parse_ftcs_params(&["D", "1"]),
            Some(FtcsMarker::CommandFinished(Some(1)))
        );
        assert_eq!(
            parse_ftcs_params(&["D", "127"]),
            Some(FtcsMarker::CommandFinished(Some(127)))
        );
    }

    #[test]
    fn parse_command_finished_without_exit_code() {
        assert_eq!(
            parse_ftcs_params(&["D"]),
            Some(FtcsMarker::CommandFinished(None))
        );
    }

    #[test]
    fn parse_command_finished_with_invalid_exit_code_returns_none_code() {
        // Non-numeric exit code — treat as D with no code
        assert_eq!(
            parse_ftcs_params(&["D", "abc"]),
            Some(FtcsMarker::CommandFinished(None))
        );
    }

    #[test]
    fn parse_empty_params() {
        let empty: &[&str] = &[];
        assert_eq!(parse_ftcs_params(empty), None);
    }

    #[test]
    fn parse_unknown_marker() {
        assert_eq!(parse_ftcs_params(&["X"]), None);
        assert_eq!(parse_ftcs_params(&["E"]), None);
        assert_eq!(parse_ftcs_params(&["a"]), None); // lowercase
    }
}
