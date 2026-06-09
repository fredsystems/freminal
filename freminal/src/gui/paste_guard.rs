// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Smart paste guard analyzer (Task 77).
//!
//! Classifies a paste payload against the `[paste_guard]` config so the GUI
//! can decide whether to send it straight to the PTY or first show a
//! confirmation dialog (77.3, wired in 77.4).
//!
//! Two pieces live here:
//!
//! - [`analyze`] — a pure function that classifies a payload into a
//!   [`PasteAnalysis`]. It takes the already-compiled dangerous-command
//!   patterns so the hot path never re-compiles regexes.
//! - [`PasteGuard`] — owns the compiled pattern cache. Rebuilt from
//!   [`PasteGuardConfig`] at startup and on config hot-reload;
//!   [`PasteGuard::rebuild`] returns the patterns that failed to compile so
//!   the caller can surface them (e.g. via the toast stack) and the guard
//!   silently skips them at match time.

use freminal_common::config::PasteGuardConfig;
use regex::Regex;

/// The classification of a paste payload produced by [`analyze`].
///
/// `Safe` means no enabled trigger fired and the payload may be sent
/// directly. Every other variant means the confirmation dialog should be
/// shown, carrying enough detail to explain *why* to the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::gui) enum PasteAnalysis {
    /// No enabled trigger fired; send the payload without confirmation.
    Safe,
    /// The payload contains at least one newline.
    Multiline {
        /// Number of lines in the payload (one more than the newline count).
        line_count: usize,
        /// Total byte length of the payload.
        byte_count: usize,
    },
    /// The payload contains control characters other than the line breaks
    /// and tabs that appear routinely in legitimate text.
    ControlChars {
        /// The distinct flagged control characters, in first-seen order.
        chars: Vec<char>,
    },
    /// The payload matched one or more dangerous-command patterns.
    Patterns {
        /// The source pattern strings that matched, in config order.
        matched: Vec<String>,
    },
    /// More than one trigger fired. `triggers` never contains `Safe` or a
    /// nested `Multiple`; it is a flat list of the individual triggers in a
    /// stable order (multiline, control chars, patterns).
    Multiple {
        /// The individual triggers that fired.
        triggers: Vec<Self>,
    },
}

impl PasteAnalysis {
    /// Whether the payload may be sent without confirmation.
    pub(in crate::gui) const fn is_safe(&self) -> bool {
        matches!(self, Self::Safe)
    }
}

/// Returns `true` for control characters the guard should flag.
///
/// Newline (`\n`) is handled by the multiline trigger, and carriage return
/// (`\r`) and tab (`\t`) appear routinely in legitimate pasted text, so none
/// of those count here. Everything else `char::is_control` reports — ESC,
/// BEL, NUL, the other C0 controls, and the C1 controls — is flagged.
fn is_flagged_control(c: char) -> bool {
    c.is_control() && c != '\n' && c != '\r' && c != '\t'
}

/// Classify `payload` against the enabled triggers in `config`.
///
/// `compiled` is the cache of successfully-compiled dangerous-command
/// patterns (see [`PasteGuard`]); it is consulted only when
/// [`PasteGuardConfig::patterns`] is enabled. This function performs no
/// allocation-heavy work beyond collecting the (small) trigger detail and
/// never compiles a regex, so it is safe to call on the GUI thread for every
/// paste.
///
/// When `config.enabled` is `false` the result is always [`PasteAnalysis::Safe`].
pub(in crate::gui) fn analyze(
    payload: &str,
    config: &PasteGuardConfig,
    compiled: &[Regex],
) -> PasteAnalysis {
    if !config.enabled {
        return PasteAnalysis::Safe;
    }

    let mut triggers: Vec<PasteAnalysis> = Vec::new();

    if config.multiline && payload.contains('\n') {
        // `lines()` would undercount a trailing newline; count breaks + 1.
        let newlines = payload.matches('\n').count();
        triggers.push(PasteAnalysis::Multiline {
            line_count: newlines + 1,
            byte_count: payload.len(),
        });
    }

    if config.control_chars {
        let mut chars: Vec<char> = Vec::new();
        for c in payload.chars() {
            if is_flagged_control(c) && !chars.contains(&c) {
                chars.push(c);
            }
        }
        if !chars.is_empty() {
            triggers.push(PasteAnalysis::ControlChars { chars });
        }
    }

    if config.patterns {
        let matched: Vec<String> = compiled
            .iter()
            .filter(|re| re.is_match(payload))
            .map(|re| re.as_str().to_owned())
            .collect();
        if !matched.is_empty() {
            triggers.push(PasteAnalysis::Patterns { matched });
        }
    }

    match triggers.len() {
        0 => PasteAnalysis::Safe,
        1 => {
            // Exactly one trigger: unwrap it out of the vec.
            triggers.into_iter().next().unwrap_or(PasteAnalysis::Safe)
        }
        _ => PasteAnalysis::Multiple { triggers },
    }
}

/// Owns the compiled dangerous-command pattern cache.
///
/// The cache is rebuilt from [`PasteGuardConfig::pattern_list`] at startup and
/// whenever the config is hot-reloaded. Patterns that fail to compile are
/// skipped (and reported by [`PasteGuard::rebuild`]) so a single malformed
/// user pattern never disables the guard.
#[derive(Debug, Default)]
pub(in crate::gui) struct PasteGuard {
    compiled: Vec<Regex>,
}

impl PasteGuard {
    /// Build a guard from `config`, discarding any compile errors.
    ///
    /// Prefer [`PasteGuard::rebuild`] on an existing guard when you need to
    /// surface compile errors to the user.
    pub(in crate::gui) fn new(config: &PasteGuardConfig) -> Self {
        let mut guard = Self::default();
        let _ = guard.rebuild(config);
        guard
    }

    /// Recompile the pattern cache from `config`, returning one
    /// `(pattern, error_message)` pair for every pattern that failed to
    /// compile. An empty result means every pattern compiled.
    ///
    /// Successfully-compiled patterns are always installed even when some
    /// siblings fail, so the guard stays maximally effective.
    pub(in crate::gui) fn rebuild(&mut self, config: &PasteGuardConfig) -> Vec<(String, String)> {
        let mut compiled = Vec::with_capacity(config.pattern_list.len());
        let mut errors = Vec::new();
        for pattern in &config.pattern_list {
            match Regex::new(pattern) {
                Ok(re) => compiled.push(re),
                Err(err) => errors.push((pattern.clone(), err.to_string())),
            }
        }
        self.compiled = compiled;
        errors
    }

    /// Classify `payload` using the cached patterns and `config`.
    pub(in crate::gui) fn analyze(
        &self,
        payload: &str,
        config: &PasteGuardConfig,
    ) -> PasteAnalysis {
        analyze(payload, config, &self.compiled)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_all_on() -> PasteGuardConfig {
        PasteGuardConfig {
            enabled: true,
            multiline: true,
            control_chars: true,
            patterns: true,
            pattern_list: vec![r"\brm\s+-rf?\b".to_owned(), r"\bsudo\b".to_owned()],
        }
    }

    #[test]
    fn safe_single_line_plain_text() {
        let cfg = config_all_on();
        let guard = PasteGuard::new(&cfg);
        assert_eq!(guard.analyze("echo hello", &cfg), PasteAnalysis::Safe);
    }

    #[test]
    fn disabled_master_switch_is_always_safe() {
        let cfg = PasteGuardConfig {
            enabled: false,
            ..config_all_on()
        };
        let guard = PasteGuard::new(&cfg);
        // Multi-line + dangerous pattern, but the guard is off.
        assert_eq!(
            guard.analyze("sudo rm -rf /\nsecond line", &cfg),
            PasteAnalysis::Safe
        );
    }

    #[test]
    fn multiline_trigger() {
        let cfg = PasteGuardConfig {
            patterns: false,
            control_chars: false,
            ..config_all_on()
        };
        let guard = PasteGuard::new(&cfg);
        let payload = "line one\nline two\nline three";
        assert_eq!(
            guard.analyze(payload, &cfg),
            PasteAnalysis::Multiline {
                line_count: 3,
                byte_count: payload.len(),
            }
        );
    }

    #[test]
    fn multiline_counts_trailing_newline_as_extra_line() {
        let cfg = PasteGuardConfig {
            patterns: false,
            control_chars: false,
            ..config_all_on()
        };
        let guard = PasteGuard::new(&cfg);
        assert_eq!(
            guard.analyze("only\n", &cfg),
            PasteAnalysis::Multiline {
                line_count: 2,
                byte_count: 5,
            }
        );
    }

    #[test]
    fn multiline_disabled_does_not_fire() {
        let cfg = PasteGuardConfig {
            multiline: false,
            patterns: false,
            control_chars: false,
            ..config_all_on()
        };
        let guard = PasteGuard::new(&cfg);
        assert_eq!(guard.analyze("a\nb\nc", &cfg), PasteAnalysis::Safe);
    }

    #[test]
    fn control_chars_trigger_on_esc_and_bel() {
        let cfg = PasteGuardConfig {
            multiline: false,
            patterns: false,
            ..config_all_on()
        };
        let guard = PasteGuard::new(&cfg);
        match guard.analyze("safe\x1b\x07text", &cfg) {
            PasteAnalysis::ControlChars { chars } => {
                assert_eq!(chars, vec!['\x1b', '\x07']);
            }
            other => panic!("expected ControlChars, got {other:?}"),
        }
    }

    #[test]
    fn control_chars_ignores_tab_and_carriage_return() {
        let cfg = PasteGuardConfig {
            multiline: false,
            patterns: false,
            ..config_all_on()
        };
        let guard = PasteGuard::new(&cfg);
        // Tab and CR are routine in legitimate text and must not trigger.
        assert_eq!(guard.analyze("a\tb\rc", &cfg), PasteAnalysis::Safe);
    }

    #[test]
    fn control_chars_deduplicates_in_first_seen_order() {
        let cfg = PasteGuardConfig {
            multiline: false,
            patterns: false,
            ..config_all_on()
        };
        let guard = PasteGuard::new(&cfg);
        match guard.analyze("\x07\x1b\x07\x1b", &cfg) {
            PasteAnalysis::ControlChars { chars } => {
                assert_eq!(chars, vec!['\x07', '\x1b']);
            }
            other => panic!("expected ControlChars, got {other:?}"),
        }
    }

    #[test]
    fn patterns_trigger_records_matched_sources() {
        let cfg = PasteGuardConfig {
            multiline: false,
            control_chars: false,
            ..config_all_on()
        };
        let guard = PasteGuard::new(&cfg);
        match guard.analyze("please sudo rm -rf /tmp/x", &cfg) {
            PasteAnalysis::Patterns { matched } => {
                assert!(matched.contains(&r"\brm\s+-rf?\b".to_owned()));
                assert!(matched.contains(&r"\bsudo\b".to_owned()));
            }
            other => panic!("expected Patterns, got {other:?}"),
        }
    }

    #[test]
    fn patterns_disabled_does_not_fire() {
        let cfg = PasteGuardConfig {
            multiline: false,
            control_chars: false,
            patterns: false,
            ..config_all_on()
        };
        let guard = PasteGuard::new(&cfg);
        assert_eq!(guard.analyze("sudo rm -rf /", &cfg), PasteAnalysis::Safe);
    }

    #[test]
    fn multiple_triggers_are_flattened() {
        let cfg = config_all_on();
        let guard = PasteGuard::new(&cfg);
        // Multi-line + control char + dangerous pattern.
        match guard.analyze("sudo something\nrm -rf /\x1b", &cfg) {
            PasteAnalysis::Multiple { triggers } => {
                assert_eq!(triggers.len(), 3);
                assert!(matches!(triggers[0], PasteAnalysis::Multiline { .. }));
                assert!(matches!(triggers[1], PasteAnalysis::ControlChars { .. }));
                assert!(matches!(triggers[2], PasteAnalysis::Patterns { .. }));
                // The flattened list never nests Multiple or Safe.
                assert!(
                    !triggers
                        .iter()
                        .any(|t| matches!(t, PasteAnalysis::Multiple { .. } | PasteAnalysis::Safe))
                );
            }
            other => panic!("expected Multiple, got {other:?}"),
        }
    }

    #[test]
    fn rebuild_reports_invalid_patterns_and_keeps_valid_ones() {
        let cfg = PasteGuardConfig {
            multiline: false,
            control_chars: false,
            pattern_list: vec![r"\bsudo\b".to_owned(), "[".to_owned()],
            ..config_all_on()
        };
        let mut guard = PasteGuard::default();
        let errors = guard.rebuild(&cfg);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].0, "[");
        // The valid `sudo` pattern still matches despite the malformed sibling.
        match guard.analyze("sudo reboot", &cfg) {
            PasteAnalysis::Patterns { matched } => {
                assert_eq!(matched, vec![r"\bsudo\b".to_owned()]);
            }
            other => panic!("expected Patterns, got {other:?}"),
        }
    }

    #[test]
    fn is_safe_helper() {
        assert!(PasteAnalysis::Safe.is_safe());
        assert!(
            !PasteAnalysis::Multiline {
                line_count: 2,
                byte_count: 4
            }
            .is_safe()
        );
    }
}
