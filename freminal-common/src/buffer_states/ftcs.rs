// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! OSC 133 — `FinalTerm` / Shell Integration (FTCS) protocol.
//!
//! These types represent the FTCS markers that shells emit to delineate
//! prompt, command, and output regions:
//!
//! - `OSC 133 ; A ; freminal=1 ; fid=<id> ST` — Prompt start
//! - `OSC 133 ; B ; freminal=1 ; fid=<id> ST` — Prompt end / command input start
//! - `OSC 133 ; C ; freminal=1 ; fid=<id> ST` — Command end / output start (pre-execution)
//! - `OSC 133 ; D [; exitcode] ; freminal=1 ; fid=<id> ST` — Command finished
//! - `OSC 133 ; P ; k=<kind> ST` — Prompt property (kind: `i`=initial, `c`=continuation, `r`=right)
//!
//! Markers from foreign emitters (WezTerm, Starship, iTerm2, Kitty) that lack
//! `freminal=1` are silently dropped by `parse_ftcs_params` to prevent duplicate
//! command blocks when multiple shell integrations are simultaneously active.
//!
//! The `P` (PromptProperty) marker does not require `freminal=1` — it is
//! informational only and carries no semantic effect on the buffer.

use std::fmt;

/// The kind of prompt annotated by an `OSC 133 ; P ; k=<kind>` marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptKind {
    /// `i` — Initial / primary prompt (PS1).
    Initial,
    /// `c` — Continuation prompt (PS2).
    Continuation,
    /// `r` — Right-aligned prompt (RPROMPT).
    Right,
}

impl fmt::Display for PromptKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Initial => write!(f, "i"),
            Self::Continuation => write!(f, "c"),
            Self::Right => write!(f, "r"),
        }
    }
}

/// A single FTCS marker, as emitted by the freminal shell integration scripts.
///
/// Markers `A`, `B`, `C`, and `D` carry a `fid` (freminal correlation ID) that
/// allows the parser to match `A` and `D` pairs explicitly even when other
/// shell integrations (`WezTerm`, Starship, `iTerm2`) are simultaneously emitting
/// OSC 133 markers.  Markers without `freminal=1` are rejected by
/// [`parse_ftcs_params`] and produce no buffer side-effects.
///
/// The `P` (`PromptProperty`) variant does not carry a `fid` because it is
/// purely informational and carries no semantic effect on the buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FtcsMarker {
    /// `A` — Prompt start.  Carries the freminal correlation ID for A/D pairing.
    PromptStart {
        /// Freminal correlation ID, matching the `fid=` param in the A marker.
        fid: String,
    },

    /// `B` — Prompt end / command input start.
    CommandStart {
        /// Freminal correlation ID, must match the `fid` of the corresponding `A`.
        fid: String,
    },

    /// `C` — Command end / output start.  The shell is about to execute the
    /// command; everything after this until `D` is command output.
    OutputStart {
        /// Freminal correlation ID, must match the `fid` of the corresponding `A`.
        fid: String,
    },

    /// `D` — Command finished.  Carries optional exit code and the `fid`
    /// of the matching `A` marker.
    CommandFinished {
        /// Optional exit code from the shell (`0` = success).
        exit_code: Option<i32>,
        /// Freminal correlation ID matching the `fid` of the corresponding `A`.
        fid: String,
    },

    /// `P` — Prompt property.  Annotates the kind of prompt that follows.
    /// Does not require `freminal=1`; no `fid` is carried.
    PromptProperty(PromptKind),
}

impl fmt::Display for FtcsMarker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PromptStart { fid } => write!(f, "A;freminal=1;fid={fid}"),
            Self::CommandStart { fid } => write!(f, "B;freminal=1;fid={fid}"),
            Self::OutputStart { fid } => write!(f, "C;freminal=1;fid={fid}"),
            Self::CommandFinished {
                exit_code: Some(code),
                fid,
            } => write!(f, "D;{code};freminal=1;fid={fid}"),
            Self::CommandFinished {
                exit_code: None,
                fid,
            } => write!(f, "D;freminal=1;fid={fid}"),
            Self::PromptProperty(kind) => write!(f, "P;k={kind}"),
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
/// - `OSC 133 ; A ; freminal=1 ; fid=x ST` → `params = ["A", "freminal=1", "fid=x"]`
/// - `OSC 133 ; D ; 0 ; freminal=1 ; fid=x ST` → `params = ["D", "0", "freminal=1", "fid=x"]`
/// - `OSC 133 ; P ; k=i ST`                    → `params = ["P", "k=i"]`
///
/// **Freminal tag requirement:** markers `A`, `B`, `C`, and `D` are only
/// accepted when `freminal=1` is present in the parameter list.  This prevents
/// duplicate command blocks when other shell integrations (`WezTerm`, Starship,
/// `iTerm2`, Kitty) emit OSC 133 markers simultaneously.
///
/// **`P` exception:** the `P` marker is informational only and does not require
/// `freminal=1`.  It is accepted from any emitter.
///
/// Returns `None` for unrecognised markers, empty parameter lists, or markers
/// that fail the `freminal=1` / `fid=` requirement.
///
/// # Foreign-marker rejection
///
/// Markers from foreign shell integrations (`WezTerm`, Starship, `iTerm2`) that
/// lack `freminal=1` are silently dropped.  This prevents duplicate command
/// blocks when multiple shell integrations are simultaneously active.
#[must_use]
pub fn parse_ftcs_params(params: &[&str]) -> Option<FtcsMarker> {
    let marker_char = params.first()?;

    // Walk the remaining params once, harvesting what we need.
    // We accept key-value params in any order; unknown params (e.g. `cl=m`,
    // `aid=12345` from WezTerm/iTerm2) are silently ignored.
    let mut freminal_tag: Option<&str> = None;
    let mut fid: Option<&str> = None;
    let mut prompt_kind: Option<PromptKind> = None;
    // The first positional (non-key=value) param after the marker letter is
    // used as the exit code for `D`.  We capture it at position i==1 so that
    // `D;0;freminal=1;fid=x` and `D;freminal=1;fid=x` are both handled.
    let mut first_positional: Option<&str> = None;

    for (i, p) in params.iter().enumerate().skip(1) {
        if let Some(v) = p.strip_prefix("freminal=") {
            freminal_tag = Some(v);
        } else if let Some(v) = p.strip_prefix("fid=") {
            fid = Some(v);
        } else if let Some(v) = p.strip_prefix("k=") {
            // P;k=<kind>
            prompt_kind = Some(match v {
                "c" => PromptKind::Continuation,
                "r" => PromptKind::Right,
                _ => PromptKind::Initial, // "i" and any unknown value → Initial
            });
        } else if i == 1 && first_positional.is_none() {
            // First positional param after the marker letter — exit code for D.
            first_positional = Some(p);
        }
        // Unknown params are intentionally ignored.
    }

    match *marker_char {
        "A" => {
            if freminal_tag != Some("1") {
                return None;
            }
            let fid = fid?.to_owned();
            Some(FtcsMarker::PromptStart { fid })
        }
        "B" => {
            if freminal_tag != Some("1") {
                return None;
            }
            let fid = fid?.to_owned();
            Some(FtcsMarker::CommandStart { fid })
        }
        "C" => {
            if freminal_tag != Some("1") {
                return None;
            }
            let fid = fid?.to_owned();
            Some(FtcsMarker::OutputStart { fid })
        }
        "D" => {
            if freminal_tag != Some("1") {
                return None;
            }
            let fid = fid?.to_owned();
            let exit_code = first_positional.and_then(|s| s.parse::<i32>().ok());
            Some(FtcsMarker::CommandFinished { exit_code, fid })
        }
        "P" => {
            // P is informational; no freminal=1 / fid required.
            // This preserves historical behaviour and means freminal still
            // recognises P markers from any emitter.
            Some(FtcsMarker::PromptProperty(
                prompt_kind.unwrap_or(PromptKind::Initial),
            ))
        }
        _ => None,
    }
}

/// Whether `marker_char` is an FTCS marker letter freminal knows about.
///
/// Distinguishes the two reasons [`parse_ftcs_params`] returns `None`:
///
/// - A **known** marker (`A`/`B`/`C`/`D`/`P`) returning `None` means a foreign
///   emitter sent it without the `freminal=1` tag (or without a required
///   field). Freminal understands the sequence and deliberately ignores it to
///   avoid duplicate command blocks — this is expected and should NOT be logged
///   as unhandled.
/// - An **unknown** marker (any other letter, e.g. `Z`, or a future FTCS
///   addition like `E`) or an empty parameter list means freminal does not
///   recognise the sequence at all. That is a genuine gap — a new/malformed
///   OSC 133 variant — and the caller should log it so the unhandled surface
///   can be audited (see `MASTER_PLAN` v0.16.0 crash reporting).
///
/// This is intentionally a separate, allocation-free classifier rather than a
/// richer return type on [`parse_ftcs_params`], so the "should I act on this?"
/// decision (the `Option`) and the "should I log this as unhandled?" decision
/// stay independent and the existing callers/tests are unaffected.
#[must_use]
pub fn is_known_ftcs_marker(params: &[&str]) -> bool {
    matches!(params.first(), Some(&("A" | "B" | "C" | "D" | "P")))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── FtcsMarker Display ──────────────────────────────────────────────

    #[test]
    fn display_prompt_start() {
        assert_eq!(
            FtcsMarker::PromptStart {
                fid: "foo".to_owned()
            }
            .to_string(),
            "A;freminal=1;fid=foo"
        );
    }

    #[test]
    fn display_command_start() {
        assert_eq!(
            FtcsMarker::CommandStart {
                fid: "foo".to_owned()
            }
            .to_string(),
            "B;freminal=1;fid=foo"
        );
    }

    #[test]
    fn display_output_start() {
        assert_eq!(
            FtcsMarker::OutputStart {
                fid: "foo".to_owned()
            }
            .to_string(),
            "C;freminal=1;fid=foo"
        );
    }

    #[test]
    fn display_command_finished_with_code() {
        assert_eq!(
            FtcsMarker::CommandFinished {
                exit_code: Some(0),
                fid: "foo".to_owned()
            }
            .to_string(),
            "D;0;freminal=1;fid=foo"
        );
        assert_eq!(
            FtcsMarker::CommandFinished {
                exit_code: Some(127),
                fid: "foo".to_owned()
            }
            .to_string(),
            "D;127;freminal=1;fid=foo"
        );
        assert_eq!(
            FtcsMarker::CommandFinished {
                exit_code: Some(-1),
                fid: "foo".to_owned()
            }
            .to_string(),
            "D;-1;freminal=1;fid=foo"
        );
    }

    #[test]
    fn display_command_finished_without_code() {
        assert_eq!(
            FtcsMarker::CommandFinished {
                exit_code: None,
                fid: "foo".to_owned()
            }
            .to_string(),
            "D;freminal=1;fid=foo"
        );
    }

    #[test]
    fn display_prompt_property_initial() {
        assert_eq!(
            FtcsMarker::PromptProperty(PromptKind::Initial).to_string(),
            "P;k=i"
        );
    }

    #[test]
    fn display_prompt_property_continuation() {
        assert_eq!(
            FtcsMarker::PromptProperty(PromptKind::Continuation).to_string(),
            "P;k=c"
        );
    }

    #[test]
    fn display_prompt_property_right() {
        assert_eq!(
            FtcsMarker::PromptProperty(PromptKind::Right).to_string(),
            "P;k=r"
        );
    }

    #[test]
    fn display_prompt_kind() {
        assert_eq!(PromptKind::Initial.to_string(), "i");
        assert_eq!(PromptKind::Continuation.to_string(), "c");
        assert_eq!(PromptKind::Right.to_string(), "r");
    }

    // ── Display round-trips through parse_ftcs_params ───────────────────

    #[test]
    fn display_round_trip_prompt_start() {
        let marker = FtcsMarker::PromptStart {
            fid: "rt1".to_owned(),
        };
        let s = marker.to_string();
        let parts: Vec<&str> = s.split(';').collect();
        let parsed = parse_ftcs_params(&parts);
        assert_eq!(parsed, Some(marker));
    }

    #[test]
    fn display_round_trip_command_start() {
        let marker = FtcsMarker::CommandStart {
            fid: "rt2".to_owned(),
        };
        let s = marker.to_string();
        let parts: Vec<&str> = s.split(';').collect();
        let parsed = parse_ftcs_params(&parts);
        assert_eq!(parsed, Some(marker));
    }

    #[test]
    fn display_round_trip_output_start() {
        let marker = FtcsMarker::OutputStart {
            fid: "rt3".to_owned(),
        };
        let s = marker.to_string();
        let parts: Vec<&str> = s.split(';').collect();
        let parsed = parse_ftcs_params(&parts);
        assert_eq!(parsed, Some(marker));
    }

    #[test]
    fn display_round_trip_command_finished_with_code() {
        let marker = FtcsMarker::CommandFinished {
            exit_code: Some(42),
            fid: "rt4".to_owned(),
        };
        let s = marker.to_string();
        let parts: Vec<&str> = s.split(';').collect();
        let parsed = parse_ftcs_params(&parts);
        assert_eq!(parsed, Some(marker));
    }

    #[test]
    fn display_round_trip_command_finished_no_code() {
        let marker = FtcsMarker::CommandFinished {
            exit_code: None,
            fid: "rt5".to_owned(),
        };
        let s = marker.to_string();
        let parts: Vec<&str> = s.split(';').collect();
        let parsed = parse_ftcs_params(&parts);
        assert_eq!(parsed, Some(marker));
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

    // ── parse_ftcs_params — A marker ────────────────────────────────────

    #[test]
    fn parse_a_with_freminal_tag_returns_prompt_start() {
        assert_eq!(
            parse_ftcs_params(&["A", "freminal=1", "fid=foo"]),
            Some(FtcsMarker::PromptStart {
                fid: "foo".to_owned()
            })
        );
    }

    #[test]
    fn parse_a_without_freminal_tag_returns_none() {
        // WezTerm-style marker: no freminal=1
        assert_eq!(parse_ftcs_params(&["A", "aid=12345"]), None);
    }

    #[test]
    fn parse_a_with_wrong_freminal_value_returns_none() {
        assert_eq!(parse_ftcs_params(&["A", "freminal=2", "fid=foo"]), None);
    }

    #[test]
    fn parse_a_without_fid_returns_none() {
        assert_eq!(parse_ftcs_params(&["A", "freminal=1"]), None);
    }

    #[test]
    fn parse_a_plain_returns_none() {
        // Old-style plain `A` marker (no params) — must return None.
        assert_eq!(parse_ftcs_params(&["A"]), None);
    }

    // ── parse_ftcs_params — B marker ────────────────────────────────────

    #[test]
    fn parse_b_with_freminal_tag() {
        assert_eq!(
            parse_ftcs_params(&["B", "freminal=1", "fid=bar"]),
            Some(FtcsMarker::CommandStart {
                fid: "bar".to_owned()
            })
        );
    }

    #[test]
    fn parse_b_without_freminal_tag_returns_none() {
        assert_eq!(parse_ftcs_params(&["B"]), None);
        assert_eq!(parse_ftcs_params(&["B", "aid=12345"]), None);
    }

    #[test]
    fn parse_b_without_fid_returns_none() {
        assert_eq!(parse_ftcs_params(&["B", "freminal=1"]), None);
    }

    // ── parse_ftcs_params — C marker ────────────────────────────────────

    #[test]
    fn parse_c_with_freminal_tag() {
        assert_eq!(
            parse_ftcs_params(&["C", "freminal=1", "fid=baz"]),
            Some(FtcsMarker::OutputStart {
                fid: "baz".to_owned()
            })
        );
    }

    #[test]
    fn parse_c_without_freminal_tag_returns_none() {
        assert_eq!(parse_ftcs_params(&["C"]), None);
    }

    #[test]
    fn parse_c_without_fid_returns_none() {
        assert_eq!(parse_ftcs_params(&["C", "freminal=1"]), None);
    }

    // ── parse_ftcs_params — D marker ────────────────────────────────────

    #[test]
    fn parse_d_with_freminal_tag_and_exit_code() {
        assert_eq!(
            parse_ftcs_params(&["D", "0", "freminal=1", "fid=foo"]),
            Some(FtcsMarker::CommandFinished {
                exit_code: Some(0),
                fid: "foo".to_owned()
            })
        );
    }

    #[test]
    fn parse_d_with_freminal_tag_and_failure_code() {
        assert_eq!(
            parse_ftcs_params(&["D", "127", "freminal=1", "fid=foo"]),
            Some(FtcsMarker::CommandFinished {
                exit_code: Some(127),
                fid: "foo".to_owned()
            })
        );
    }

    #[test]
    fn parse_d_with_freminal_tag_and_no_exit_code() {
        assert_eq!(
            parse_ftcs_params(&["D", "freminal=1", "fid=foo"]),
            Some(FtcsMarker::CommandFinished {
                exit_code: None,
                fid: "foo".to_owned()
            })
        );
    }

    #[test]
    fn parse_d_without_freminal_returns_none() {
        // Plain `D;0` — foreign emitter
        assert_eq!(parse_ftcs_params(&["D", "0"]), None);
        assert_eq!(parse_ftcs_params(&["D"]), None);
        assert_eq!(parse_ftcs_params(&["D", "0", "aid=12345"]), None);
    }

    #[test]
    fn parse_d_without_fid_returns_none() {
        assert_eq!(parse_ftcs_params(&["D", "freminal=1"]), None);
        assert_eq!(parse_ftcs_params(&["D", "0", "freminal=1"]), None);
    }

    // ── parse_ftcs_params — P marker ────────────────────────────────────

    #[test]
    fn parse_p_does_not_require_freminal_tag() {
        // P is informational; accepted from any emitter
        assert_eq!(
            parse_ftcs_params(&["P", "k=i"]),
            Some(FtcsMarker::PromptProperty(PromptKind::Initial))
        );
    }

    #[test]
    fn parse_prompt_property_initial() {
        assert_eq!(
            parse_ftcs_params(&["P", "k=i"]),
            Some(FtcsMarker::PromptProperty(PromptKind::Initial))
        );
    }

    #[test]
    fn parse_prompt_property_continuation() {
        assert_eq!(
            parse_ftcs_params(&["P", "k=c"]),
            Some(FtcsMarker::PromptProperty(PromptKind::Continuation))
        );
    }

    #[test]
    fn parse_prompt_property_right() {
        assert_eq!(
            parse_ftcs_params(&["P", "k=r"]),
            Some(FtcsMarker::PromptProperty(PromptKind::Right))
        );
    }

    #[test]
    fn parse_prompt_property_without_kind_defaults_to_initial() {
        // `P` without `k=` still parses, defaulting to Initial
        assert_eq!(
            parse_ftcs_params(&["P"]),
            Some(FtcsMarker::PromptProperty(PromptKind::Initial))
        );
    }

    #[test]
    fn parse_prompt_property_unknown_kind_defaults_to_initial() {
        // `P` with an unknown `k=` value defaults to Initial
        assert_eq!(
            parse_ftcs_params(&["P", "k=z"]),
            Some(FtcsMarker::PromptProperty(PromptKind::Initial))
        );
    }

    #[test]
    fn parse_prompt_property_non_kind_param_defaults_to_initial() {
        // `P` with a non-k parameter defaults to Initial
        assert_eq!(
            parse_ftcs_params(&["P", "x=1"]),
            Some(FtcsMarker::PromptProperty(PromptKind::Initial))
        );
    }

    // ── parse_ftcs_params — edge cases ──────────────────────────────────

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

    #[test]
    fn parse_a_with_extra_unknown_params_still_works() {
        // Unknown params from other integrations are silently ignored
        assert_eq!(
            parse_ftcs_params(&["A", "cl=m", "freminal=1", "fid=x", "aid=99"]),
            Some(FtcsMarker::PromptStart {
                fid: "x".to_owned()
            })
        );
    }

    // ── is_known_ftcs_marker ────────────────────────────────────────────

    #[test]
    fn known_ftcs_markers_are_recognised() {
        // All five FTCS marker letters are "known", regardless of whether they
        // carry freminal=1 (i.e. even foreign duplicates are known markers).
        for m in ["A", "B", "C", "D", "P"] {
            assert!(is_known_ftcs_marker(&[m]), "{m} should be known");
            assert!(
                is_known_ftcs_marker(&[m, "aid=12345"]),
                "{m} with foreign params should still be known"
            );
        }
    }

    #[test]
    fn unknown_or_empty_ftcs_markers_are_not_known() {
        // Unknown letters, future additions, lowercase, and empty lists are all
        // "not known" — these are the cases that must be logged as unhandled.
        assert!(!is_known_ftcs_marker(&["Z"]));
        assert!(!is_known_ftcs_marker(&["E"])); // plausible future FTCS marker
        assert!(!is_known_ftcs_marker(&["a"])); // lowercase is not a marker
        let empty: &[&str] = &[];
        assert!(!is_known_ftcs_marker(empty));
    }
}
