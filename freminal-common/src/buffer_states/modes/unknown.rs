// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use std::fmt;

use crate::buffer_states::{mode::SetMode, modes::ReportMode};

/// Whether the unrecognised mode arrived as a DEC private mode (`?` prefix) or
/// a plain ANSI mode (no prefix).  This controls the DECRPM/ANSI-RMP report
/// prefix emitted by [`UnknownMode::report`].
#[derive(Debug, Eq, PartialEq, Clone, Copy)]
pub enum ModeNamespace {
    /// DEC private mode — params were prefixed with `?` (e.g. `\x1b[?999h`).
    /// Report format: `\x1b[?{N};0$y`
    Dec,
    /// Standard ANSI mode — no `?` prefix (e.g. `\x1b[42h`).
    /// Report format: `\x1b[{N};0$y`
    Ansi,
}

impl fmt::Display for ModeNamespace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Dec => write!(f, "DEC"),
            Self::Ansi => write!(f, "ANSI"),
        }
    }
}

#[derive(Debug, Eq, PartialEq, Clone)]
pub struct UnknownMode {
    pub params: String,
    pub mode: SetMode,
    pub namespace: ModeNamespace,
}

impl UnknownMode {
    #[must_use]
    pub fn new(params: &[u8], set_mode: SetMode, namespace: ModeNamespace) -> Self {
        let params_s = std::str::from_utf8(params).unwrap_or("Unknown");

        Self {
            params: params_s.to_string(),
            mode: set_mode,
            namespace,
        }
    }
}

impl ReportMode for UnknownMode {
    fn report(&self, _override_mode: Option<SetMode>) -> String {
        match self.namespace {
            ModeNamespace::Dec => format!("\x1b[?{};0$y", self.params),
            ModeNamespace::Ansi => format!("\x1b[{};0$y", self.params),
        }
    }
}

impl fmt::Display for UnknownMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} {} Unknown Mode({})",
            self.namespace, self.mode, self.params
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer_states::modes::ReportMode;

    #[test]
    fn dec_unknown_mode_report_includes_question_mark_prefix() {
        let mode = UnknownMode::new(b"999", SetMode::DecRst, ModeNamespace::Dec);
        assert_eq!(mode.report(None), "\x1b[?999;0$y");
    }

    #[test]
    fn ansi_unknown_mode_report_has_no_question_mark_prefix() {
        let mode = UnknownMode::new(b"42", SetMode::DecRst, ModeNamespace::Ansi);
        assert_eq!(mode.report(None), "\x1b[42;0$y");
    }

    #[test]
    fn dec_unknown_mode_report_with_override_still_emits_dec_prefix() {
        let mode = UnknownMode::new(b"1234", SetMode::DecSet, ModeNamespace::Dec);
        assert_eq!(mode.report(Some(SetMode::DecSet)), "\x1b[?1234;0$y");
    }

    #[test]
    fn ansi_unknown_mode_report_with_override_still_omits_dec_prefix() {
        let mode = UnknownMode::new(b"20", SetMode::DecSet, ModeNamespace::Ansi);
        assert_eq!(mode.report(Some(SetMode::DecSet)), "\x1b[20;0$y");
    }

    #[test]
    fn display_dec_unknown_mode_contains_dec_label() {
        let mode = UnknownMode::new(b"999", SetMode::DecSet, ModeNamespace::Dec);
        let s = format!("{mode}");
        assert!(s.contains("DEC"), "expected 'DEC' in display: {s}");
        assert!(s.contains("999"), "expected params in display: {s}");
    }

    #[test]
    fn display_ansi_unknown_mode_contains_ansi_label() {
        let mode = UnknownMode::new(b"42", SetMode::DecRst, ModeNamespace::Ansi);
        let s = format!("{mode}");
        assert!(s.contains("ANSI"), "expected 'ANSI' in display: {s}");
        assert!(s.contains("42"), "expected params in display: {s}");
    }
}
