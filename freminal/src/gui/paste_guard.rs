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

use std::fmt::Write as _;

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

/// A one-line, human-readable explanation of why the paste was flagged,
/// shown in the dialog banner.
///
/// Pure and unit-tested; the dialog renderer only formats this string.
fn banner_text(analysis: &PasteAnalysis) -> String {
    match analysis {
        // `Safe` never reaches the dialog, but give a sane string rather than
        // panicking if it somehow does.
        PasteAnalysis::Safe => "Paste".to_owned(),
        PasteAnalysis::Multiline {
            line_count,
            byte_count,
        } => {
            format!("Multi-line paste — {line_count} lines, {byte_count} bytes")
        }
        PasteAnalysis::ControlChars { chars } => {
            let rendered = chars
                .iter()
                .map(|c| format!("U+{:04X}", u32::from(*c)))
                .collect::<Vec<_>>()
                .join(", ");
            format!("Control characters detected: {rendered}")
        }
        PasteAnalysis::Patterns { matched } => {
            format!("Dangerous patterns detected: {}", matched.join(", "))
        }
        PasteAnalysis::Multiple { triggers } => {
            let parts = triggers
                .iter()
                .map(banner_text)
                .collect::<Vec<_>>()
                .join("; ");
            let mut out = String::from("Multiple triggers — ");
            // `write!` into a String is infallible; ignore the Result without
            // an `unwrap` to satisfy the panic-free policy.
            let _ = write!(out, "{parts}");
            out
        }
    }
}

/// The result of rendering the confirm-paste dialog for one frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::gui) enum PasteDialogOutcome {
    /// The dialog is closed, or open and awaiting a decision. Nothing to do.
    Idle,
    /// The user cancelled. The pending paste must be discarded.
    Cancelled,
    /// The user confirmed. The carried payload (original or edited) must be
    /// sent to the PTY by the caller (77.4), which owns bracketed-paste
    /// wrapping and PTY routing.
    Paste(String),
}

/// State for a single open confirm-paste dialog.
#[derive(Debug, Clone)]
struct PasteDialogState {
    /// The raw, unwrapped paste payload as analysed.
    payload: String,
    /// Why the paste was flagged (drives the banner).
    analysis: PasteAnalysis,
    /// `true` once the user clicked "Edit and Paste"; the content area becomes
    /// an editable `TextEdit` bound to `edit_buffer`.
    editing: bool,
    /// Scratch buffer for the edit mode, initialised from `payload`.
    edit_buffer: String,
    /// `true` only on the first frame after opening, used to focus the edit
    /// field exactly once when entering edit mode.
    just_entered_edit: bool,
}

/// The confirm-paste modal dialog (Task 77.3).
///
/// Lives on `PerWindowState`. Opened by the paste path (77.4) when the
/// analyzer flags a payload, rendered every frame while open via [`show`],
/// and resolved when the user confirms, cancels, or presses a shortcut.
///
/// [`show`]: PasteDialog::show
#[derive(Debug, Default)]
pub(in crate::gui) struct PasteDialog {
    state: Option<PasteDialogState>,
}

impl PasteDialog {
    /// Open the dialog for `payload` with its precomputed `analysis`.
    ///
    /// A `Safe` analysis is ignored — the dialog is only for flagged pastes.
    pub(in crate::gui) fn open(&mut self, payload: String, analysis: PasteAnalysis) {
        if analysis.is_safe() {
            return;
        }
        self.state = Some(PasteDialogState {
            edit_buffer: payload.clone(),
            payload,
            analysis,
            editing: false,
            just_entered_edit: false,
        });
    }

    /// Whether the dialog is currently open.
    pub(in crate::gui) const fn is_open(&self) -> bool {
        self.state.is_some()
    }

    /// Discard any open dialog without producing an outcome.
    ///
    /// Used by 77.4 when the target pane closes out from under an open dialog.
    pub(in crate::gui) fn close(&mut self) {
        self.state = None;
    }

    /// Render the dialog for one frame and return the resulting outcome.
    ///
    /// Returns [`PasteDialogOutcome::Idle`] when the dialog is closed or still
    /// awaiting a decision. On `Cancelled` or `Paste`, the dialog closes
    /// itself before returning.
    ///
    /// Keyboard shortcuts: `Escape` cancels; `Ctrl+Enter` pastes the current
    /// payload (edited content when in edit mode).
    pub(in crate::gui) fn show(&mut self, ctx: &egui::Context) -> PasteDialogOutcome {
        let Some(state) = self.state.as_mut() else {
            return PasteDialogOutcome::Idle;
        };

        let mut outcome = PasteDialogOutcome::Idle;

        let escape = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        let ctrl_enter = ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Enter));

        egui::Window::new("Confirm Paste")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.set_max_width(560.0);

                // Trigger banner.
                ui.label(
                    egui::RichText::new(banner_text(&state.analysis))
                        .strong()
                        .color(egui::Color32::from_rgb(0xE0, 0x6C, 0x4B)),
                );
                ui.add_space(6.0);

                // Content area: read-only preview, or editable buffer.
                egui::ScrollArea::vertical()
                    .max_height(280.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        if state.editing {
                            let response = ui.add(
                                egui::TextEdit::multiline(&mut state.edit_buffer)
                                    .font(egui::TextStyle::Monospace)
                                    .desired_width(f32::INFINITY)
                                    .desired_rows(10),
                            );
                            if state.just_entered_edit {
                                response.request_focus();
                                state.just_entered_edit = false;
                            }
                        } else {
                            // Read-only: a disabled multiline TextEdit keeps the
                            // monospace + selectable behaviour without allowing
                            // edits.
                            let mut view = state.payload.as_str();
                            ui.add_enabled(
                                false,
                                egui::TextEdit::multiline(&mut view)
                                    .font(egui::TextStyle::Monospace)
                                    .desired_width(f32::INFINITY)
                                    .desired_rows(10),
                            );
                        }
                    });

                ui.add_space(8.0);

                ui.horizontal(|ui| {
                    // Cancel is the default (safest) action.
                    if ui.button("Cancel").clicked() {
                        outcome = PasteDialogOutcome::Cancelled;
                    }
                    if ui.button("Paste Anyway").clicked() {
                        let payload = if state.editing {
                            state.edit_buffer.clone()
                        } else {
                            state.payload.clone()
                        };
                        outcome = PasteDialogOutcome::Paste(payload);
                    }
                    if !state.editing && ui.button("Edit and Paste").clicked() {
                        state.editing = true;
                        state.just_entered_edit = true;
                    }
                });
            });

        // Keyboard shortcuts resolve after rendering so a click in the same
        // frame still wins if both happen (clicks set `outcome` above).
        if outcome == PasteDialogOutcome::Idle {
            if escape {
                outcome = PasteDialogOutcome::Cancelled;
            } else if ctrl_enter {
                let payload = if state.editing {
                    state.edit_buffer.clone()
                } else {
                    state.payload.clone()
                };
                outcome = PasteDialogOutcome::Paste(payload);
            }
        }

        if outcome != PasteDialogOutcome::Idle {
            self.state = None;
        }
        outcome
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

    #[test]
    fn banner_text_for_each_variant() {
        assert_eq!(
            banner_text(&PasteAnalysis::Multiline {
                line_count: 17,
                byte_count: 420,
            }),
            "Multi-line paste — 17 lines, 420 bytes"
        );
        assert_eq!(
            banner_text(&PasteAnalysis::ControlChars {
                chars: vec!['\x1b', '\x07'],
            }),
            "Control characters detected: U+001B, U+0007"
        );
        assert_eq!(
            banner_text(&PasteAnalysis::Patterns {
                matched: vec![r"\bsudo\b".to_owned(), r"\brm\s+-rf?\b".to_owned()],
            }),
            r"Dangerous patterns detected: \bsudo\b, \brm\s+-rf?\b"
        );
        assert_eq!(banner_text(&PasteAnalysis::Safe), "Paste");
    }

    #[test]
    fn banner_text_flattens_multiple() {
        let banner = banner_text(&PasteAnalysis::Multiple {
            triggers: vec![
                PasteAnalysis::Multiline {
                    line_count: 2,
                    byte_count: 8,
                },
                PasteAnalysis::Patterns {
                    matched: vec![r"\bsudo\b".to_owned()],
                },
            ],
        });
        assert_eq!(
            banner,
            r"Multiple triggers — Multi-line paste — 2 lines, 8 bytes; Dangerous patterns detected: \bsudo\b"
        );
    }

    #[test]
    fn dialog_opens_and_closes() {
        let mut dialog = PasteDialog::default();
        assert!(!dialog.is_open());

        dialog.open(
            "sudo rm -rf /".to_owned(),
            PasteAnalysis::Patterns {
                matched: vec![r"\bsudo\b".to_owned()],
            },
        );
        assert!(dialog.is_open());

        dialog.close();
        assert!(!dialog.is_open());
    }

    #[test]
    fn dialog_ignores_safe_analysis() {
        let mut dialog = PasteDialog::default();
        dialog.open("harmless".to_owned(), PasteAnalysis::Safe);
        assert!(
            !dialog.is_open(),
            "a Safe analysis must never open the dialog"
        );
    }

    #[test]
    fn dialog_outcome_equality() {
        // Sanity-check the outcome enum used by 77.4's wire-in.
        assert_eq!(PasteDialogOutcome::Idle, PasteDialogOutcome::Idle);
        assert_ne!(
            PasteDialogOutcome::Paste("a".to_owned()),
            PasteDialogOutcome::Paste("b".to_owned())
        );
        assert_ne!(PasteDialogOutcome::Cancelled, PasteDialogOutcome::Idle);
    }
}
