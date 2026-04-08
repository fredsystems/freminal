// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Terminal input encoding.
//!
//! This module converts high-level key events (represented as [`TerminalInput`])
//! into the byte sequences that a PTY expects to receive.  It is the sole
//! source of truth for xterm/VT key encoding in this codebase.

use std::borrow::Cow;

use freminal_common::buffer_states::modes::{
    application_escape_key::ApplicationEscapeKey, decbkm::Decbkm, decckm::Decckm,
    keypad::KeypadMode, lnm::Lnm,
};

/// Key event type for KKP flag 2 (report event types).
///
/// Maps to the `:event-type` field in CSI u sequences:
/// - `1` = key press (default, omitted when flag 2 is not active)
/// - `2` = key repeat
/// - `3` = key release
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum KeyEventType {
    #[default]
    Press,
    Repeat,
    Release,
}

impl KeyEventType {
    /// The KKP numeric event type code, or `None` for press (the default
    /// that can be elided from the sequence).
    const fn kkp_code(self) -> Option<u8> {
        match self {
            Self::Press => None,
            Self::Repeat => Some(2),
            Self::Release => Some(3),
        }
    }
}

/// Optional metadata attached to a key event for KKP flags 2/4/16.
///
/// When the Kitty Keyboard Protocol is active with flags beyond the basic
/// disambiguation (flag 1) and report-all (flag 8), this struct carries
/// the additional fields that get appended to CSI u sequences.
///
/// For callers that don't have this metadata (e.g. internal replay), use
/// [`KeyEventMeta::PRESS`].
#[derive(Clone, Debug, Default)]
pub struct KeyEventMeta {
    /// Press / repeat / release state (flag 2).
    pub event_type: KeyEventType,
    /// The Unicode text that this key produces, if any (flag 16).
    /// For printable keys this is the character itself; for function
    /// keys and modifiers this is empty/`None`.
    pub associated_text: Option<String>,
}

impl KeyEventMeta {
    /// A default key-press with no extra metadata.
    pub const PRESS: Self = Self {
        event_type: KeyEventType::Press,
        associated_text: None,
    };
}

const fn char_to_ctrl_code(c: u8) -> u8 {
    // https://catern.com/posts/terminal_quirks.html
    // man ascii
    c & 0b0001_1111
}

/// Build an xterm-style modified key sequence: `ESC [ 1 ; <mod> <final>`.
///
/// Used for arrow keys and Home/End when a modifier is held.
fn modified_csi_final(modifier: u8, final_byte: u8) -> TerminalInputPayload {
    TerminalInputPayload::Owned(format!("\x1b[1;{modifier}{}", final_byte as char).into_bytes())
}

/// Build an xterm-style modified tilde key sequence: `ESC [ <code> ; <mod> ~`.
///
/// Used for Insert/Delete/PageUp/PageDown and F5–F12 when a modifier is held.
fn modified_csi_tilde(code: u8, modifier: u8) -> TerminalInputPayload {
    TerminalInputPayload::Owned(format!("\x1b[{code};{modifier}~").into_bytes())
}

/// Build an xterm `modifyOtherKeys` level 2 sequence: `ESC [ 27 ; <mod> ; <code> ~`.
///
/// The format uses 27 as a fixed identifier (chosen because decimal 27 was
/// unused in the VT function-key numbering).  The modifier parameter follows
/// the standard formula: `1 + (shift ? 1 : 0) + (alt ? 2 : 0) + (ctrl ? 4 : 0)`.
/// The code is the decimal ASCII/Unicode value of the key.
///
/// Reference: xterm ctlseqs §`modifyOtherKeys`.
fn modify_other_keys_encoding(modifier: u8, code: u32) -> TerminalInputPayload {
    TerminalInputPayload::Owned(format!("\x1b[27;{modifier};{code}~").into_bytes())
}

/// US QWERTY shifted-key mapping for KKP flag 4.
///
/// Given a lowercase ASCII byte, returns the Unicode codepoint of the shifted
/// key on a US QWERTY layout. For letters, shifted is the uppercase form.
/// For digits and punctuation, returns the Shift symbol (e.g. `1` → `!`).
/// Returns `None` for bytes outside the printable ASCII range or that have
/// no distinct shifted form.
const fn us_qwerty_shifted(c: u8) -> Option<u32> {
    match c {
        b'a'..=b'z' => Some((c - 32) as u32), // lowercase → uppercase
        b'1' => Some(b'!' as u32),
        b'2' => Some(b'@' as u32),
        b'3' => Some(b'#' as u32),
        b'4' => Some(b'$' as u32),
        b'5' => Some(b'%' as u32),
        b'6' => Some(b'^' as u32),
        b'7' => Some(b'&' as u32),
        b'8' => Some(b'*' as u32),
        b'9' => Some(b'(' as u32),
        b'0' => Some(b')' as u32),
        b'-' => Some(b'_' as u32),
        b'=' => Some(b'+' as u32),
        b'[' => Some(b'{' as u32),
        b']' => Some(b'}' as u32),
        b'\\' => Some(b'|' as u32),
        b';' => Some(b':' as u32),
        b'\'' => Some(b'"' as u32),
        b',' => Some(b'<' as u32),
        b'.' => Some(b'>' as u32),
        b'/' => Some(b'?' as u32),
        b'`' => Some(b'~' as u32),
        _ => None,
    }
}

/// Collect a text string as a sequence of [`TerminalInput::Ascii`] values.
#[must_use]
pub fn collect_text(text: &String) -> Cow<'static, [TerminalInput]> {
    text.as_bytes()
        .iter()
        .map(|c| TerminalInput::Ascii(*c))
        .collect::<Vec<_>>()
        .into()
}

/// Convert a raw byte slice into a sequence of [`TerminalInput::Ascii`] values.
#[must_use]
pub fn raw_ascii_bytes_to_terminal_input(buf: &[u8]) -> Cow<'static, [TerminalInput]> {
    buf.iter()
        .map(|c| TerminalInput::Ascii(*c))
        .collect::<Vec<_>>()
        .into()
}

/// Modifier key state for xterm-style modified key encoding.
///
/// When any modifier is set, special keys (arrows, Home/End, function keys,
/// Insert/Delete/PageUp/PageDown) produce the xterm `CSI 1 ; Nm <final>`
/// form where N encodes the modifier combination:
///
/// | N | Modifiers       |
/// |---|-----------------|
/// | 2 | Shift           |
/// | 3 | Alt             |
/// | 4 | Shift+Alt       |
/// | 5 | Ctrl            |
/// | 6 | Ctrl+Shift      |
/// | 7 | Ctrl+Alt        |
/// | 8 | Ctrl+Alt+Shift  |
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct KeyModifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

impl KeyModifiers {
    /// No modifiers held.
    pub const NONE: Self = Self {
        shift: false,
        ctrl: false,
        alt: false,
    };

    /// Returns `true` when no modifier is held.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        !self.shift && !self.ctrl && !self.alt
    }

    /// Compute the xterm modifier parameter (2–8), or `None` if no modifier
    /// is held.
    ///
    /// Encoding: `1 + (shift ? 1 : 0) + (alt ? 2 : 0) + (ctrl ? 4 : 0)`
    #[must_use]
    pub const fn modifier_param(self) -> Option<u8> {
        if self.is_empty() {
            return None;
        }
        let mut n: u8 = 1;
        if self.shift {
            n += 1;
        }
        if self.alt {
            n += 2;
        }
        if self.ctrl {
            n += 4;
        }
        Some(n)
    }
}

/// The encoded byte payload produced by [`TerminalInput::to_payload`].
#[derive(Eq, PartialEq, Debug)]
pub enum TerminalInputPayload {
    Single(u8),
    Many(&'static [u8]),
    /// Variable-length payload for modified key sequences that cannot be
    /// represented as static byte slices.
    Owned(Vec<u8>),
}

/// A high-level terminal input event.
///
/// Each variant represents a key or key-combination that the user pressed.
/// Call [`TerminalInput::to_payload`] to obtain the byte sequence that should
/// be written to the PTY.
#[derive(Clone, Debug)]
pub enum TerminalInput {
    // Normal keypress
    Ascii(u8),
    // Normal keypress with ctrl
    Ctrl(u8),
    Enter,
    LineFeed,
    Backspace,
    ArrowRight(KeyModifiers),
    ArrowLeft(KeyModifiers),
    ArrowUp(KeyModifiers),
    ArrowDown(KeyModifiers),
    Home(KeyModifiers),
    End(KeyModifiers),
    Delete(KeyModifiers),
    Insert(KeyModifiers),
    PageUp(KeyModifiers),
    PageDown(KeyModifiers),
    Tab,
    Escape,
    InFocus,
    LostFocus,
    KeyPad(u8),
    // Function keys F1–F12
    FunctionKey(u8, KeyModifiers),
}

impl TerminalInput {
    #[must_use]
    // Inherently large: exhaustive match over every `TerminalInput` variant mapping to escape
    // byte sequences. Splitting into sub-functions adds indirection without improving clarity.
    #[allow(clippy::too_many_lines, clippy::too_many_arguments)]
    pub fn to_payload(
        &self,
        decckm_mode: Decckm,
        keypad_mode: KeypadMode,
        modify_other_keys: u8,
        application_escape_key: ApplicationEscapeKey,
        backarrow_sends_bs: Decbkm,
        line_feed_mode: Lnm,
        kitty_keyboard_flags: u32,
        meta: &KeyEventMeta,
    ) -> TerminalInputPayload {
        // KKP encoding is only activated when the flags that actually change
        // key encoding are set: DISAMBIGUATE (bit 0 = 1) or REPORT_ALL (bit
        // 3 = 8).  Flags 2/4/16 add metadata fields to sequences that are
        // already being generated but do not change base encoding — so when
        // only those flags are set the legacy path is used.
        if kitty_keyboard_flags & (1 | 8) != 0 {
            return self.to_payload_kkp(
                kitty_keyboard_flags,
                decckm_mode,
                keypad_mode,
                backarrow_sends_bs,
                line_feed_mode,
                meta,
            );
        }

        match self {
            Self::Ascii(c) => TerminalInputPayload::Single(*c),
            Self::Ctrl(c) => {
                // modifyOtherKeys level 2: encode Ctrl+key as
                // `CSI 27 ; modifier ; code ~` so tmux and programs
                // that request extended keys can distinguish all
                // Ctrl combinations unambiguously.
                //
                // Level 0/1: send legacy C0 control codes (letter & 0x1F).
                //
                // WezTerm sends the CSI 27;5;code~ form at level 2 — we
                // match that behavior.  tmux handles this format correctly
                // for its prefix key and all Ctrl shortcuts.
                if modify_other_keys >= 2 {
                    let code = u32::from(c.to_ascii_lowercase());
                    // Ctrl modifier param = 5 (1 + 4)
                    modify_other_keys_encoding(5, code)
                } else {
                    TerminalInputPayload::Single(char_to_ctrl_code(*c))
                }
            }
            // When LNM (Line Feed / New Line Mode, CSI 20 h) is set, pressing
            // Enter must send CR+LF (0x0D 0x0A).  When LNM is reset (the
            // default), Enter sends bare CR (0x0D).
            //
            // Reference: VT100 User Guide §3.3.1 — "When the new line mode is
            // set, it causes the RETURN key to transmit both a carriage return
            // and a line feed."  vttest's `tst_NLM` in `reports.c` verifies
            // this: with LNM set, RETURN must produce `\015\012`.
            //
            // Interactive shells handle `\n` fine because the POSIX tty layer
            // translates CR→NL on input (ICRNL), so sending CR is correct for
            // both TUI programs and shells.
            Self::Enter => {
                if line_feed_mode == Lnm::NewLine {
                    TerminalInputPayload::Many(b"\x0d\x0a")
                } else {
                    TerminalInputPayload::Single(char_to_ctrl_code(b'm'))
                }
            }
            Self::LineFeed => TerminalInputPayload::Single(b'\n'),
            // DECBKM (?67): set → BS (0x08), reset → DEL (0x7F).
            Self::Backspace => {
                if backarrow_sends_bs == Decbkm::BackarrowSendsBs {
                    TerminalInputPayload::Single(char_to_ctrl_code(b'H'))
                } else {
                    TerminalInputPayload::Single(0x7F)
                }
            }
            Self::Escape => {
                // Mode 7727 (Application Escape Key): send CSI 27 ; 1 ; 27 ~
                // instead of bare ESC so tmux can instantly distinguish the
                // Escape key from the start of an escape sequence.
                if application_escape_key == ApplicationEscapeKey::Set {
                    TerminalInputPayload::Owned(b"\x1b[27;1;27~".to_vec())
                } else {
                    TerminalInputPayload::Single(0x1b)
                }
            }
            // https://vt100.net/docs/vt100-ug/chapter3.html
            // Table 3-6
            //
            // When modifiers are held, always use CSI form (not SS3) even in
            // DECCKM mode — xterm convention.
            Self::ArrowRight(mods) => match mods.modifier_param() {
                Some(m) => modified_csi_final(m, b'C'),
                None if decckm_mode == Decckm::Application => TerminalInputPayload::Many(b"\x1bOC"),
                None => TerminalInputPayload::Many(b"\x1b[C"),
            },
            Self::ArrowLeft(mods) => match mods.modifier_param() {
                Some(m) => modified_csi_final(m, b'D'),
                None if decckm_mode == Decckm::Application => TerminalInputPayload::Many(b"\x1bOD"),
                None => TerminalInputPayload::Many(b"\x1b[D"),
            },
            Self::ArrowUp(mods) => match mods.modifier_param() {
                Some(m) => modified_csi_final(m, b'A'),
                None if decckm_mode == Decckm::Application => TerminalInputPayload::Many(b"\x1bOA"),
                None => TerminalInputPayload::Many(b"\x1b[A"),
            },
            Self::ArrowDown(mods) => match mods.modifier_param() {
                Some(m) => modified_csi_final(m, b'B'),
                None if decckm_mode == Decckm::Application => TerminalInputPayload::Many(b"\x1bOB"),
                None => TerminalInputPayload::Many(b"\x1b[B"),
            },
            Self::Home(mods) => match mods.modifier_param() {
                Some(m) => modified_csi_final(m, b'H'),
                None if decckm_mode == Decckm::Application => TerminalInputPayload::Many(b"\x1bOH"),
                None => TerminalInputPayload::Many(b"\x1b[H"),
            },
            Self::End(mods) => match mods.modifier_param() {
                Some(m) => modified_csi_final(m, b'F'),
                None if decckm_mode == Decckm::Application => TerminalInputPayload::Many(b"\x1bOF"),
                None => TerminalInputPayload::Many(b"\x1b[F"),
            },
            Self::KeyPad(c) => {
                if keypad_mode == KeypadMode::Numeric {
                    TerminalInputPayload::Single(*c)
                } else {
                    match c {
                        0 => TerminalInputPayload::Many(b"\x1b[Op"),
                        1 => TerminalInputPayload::Many(b"\x1b[Oq"),
                        2 => TerminalInputPayload::Many(b"\x1b[Or"),
                        3 => TerminalInputPayload::Many(b"\x1b[Os"),
                        4 => TerminalInputPayload::Many(b"\x1b[Ot"),
                        5 => TerminalInputPayload::Many(b"\x1b[Ou"),
                        6 => TerminalInputPayload::Many(b"\x1b[Ov"),
                        7 => TerminalInputPayload::Many(b"\x1b[Ow"),
                        8 => TerminalInputPayload::Many(b"\x1b[Ox"),
                        9 => TerminalInputPayload::Many(b"\x1b[Oy"),
                        b'-' => TerminalInputPayload::Many(b"\x1b[Om"),
                        b',' => TerminalInputPayload::Many(b"\x1b[Ol"),
                        b'.' => TerminalInputPayload::Many(b"\x1b[On"),
                        b'\n' => TerminalInputPayload::Many(b"\x1b[OM"),
                        _ => {
                            warn!("Unknown keypad key: {c}");
                            TerminalInputPayload::Single(*c)
                        }
                    }
                }
            }
            Self::Tab => TerminalInputPayload::Single(char_to_ctrl_code(b'i')),
            // Why \e[3~? It seems like we are emulating the vt510. Other terminals do it, so we
            // can too
            // https://web.archive.org/web/20160304024035/http://www.vt100.net/docs/vt510-rm/chapter8
            // https://en.wikipedia.org/wiki/Delete_character
            Self::Delete(mods) => mods
                .modifier_param()
                .map_or(TerminalInputPayload::Many(b"\x1b[3~"), |m| {
                    modified_csi_tilde(3, m)
                }),
            Self::Insert(mods) => mods
                .modifier_param()
                .map_or(TerminalInputPayload::Many(b"\x1b[2~"), |m| {
                    modified_csi_tilde(2, m)
                }),
            Self::PageUp(mods) => mods
                .modifier_param()
                .map_or(TerminalInputPayload::Many(b"\x1b[5~"), |m| {
                    modified_csi_tilde(5, m)
                }),
            Self::PageDown(mods) => mods
                .modifier_param()
                .map_or(TerminalInputPayload::Many(b"\x1b[6~"), |m| {
                    modified_csi_tilde(6, m)
                }),
            Self::LostFocus => TerminalInputPayload::Many(b"\x1b[O"),
            Self::InFocus => TerminalInputPayload::Many(b"\x1b[I"),
            // https://invisible-island.net/xterm/ctlseqs/ctlseqs.html#h2-PC-Style-Function-Keys
            //
            // F1–F4 use SS3 form without modifiers, CSI form with modifiers.
            // F5–F12 use CSI tilde form, with modifier inserted before `~`.
            Self::FunctionKey(n, mods) => {
                let mod_param = mods.modifier_param();
                match (n, mod_param) {
                    // F1–F4 with modifiers: CSI 1;Nm P/Q/R/S
                    (1, Some(m)) => modified_csi_final(m, b'P'),
                    (2, Some(m)) => modified_csi_final(m, b'Q'),
                    (3, Some(m)) => modified_csi_final(m, b'R'),
                    (4, Some(m)) => modified_csi_final(m, b'S'),
                    // F1–F4 without modifiers: SS3 P/Q/R/S
                    (1, None) => TerminalInputPayload::Many(b"\x1bOP"),
                    (2, None) => TerminalInputPayload::Many(b"\x1bOQ"),
                    (3, None) => TerminalInputPayload::Many(b"\x1bOR"),
                    (4, None) => TerminalInputPayload::Many(b"\x1bOS"),
                    // F5–F12 with modifiers: CSI code;Nm ~
                    (5, Some(m)) => modified_csi_tilde(15, m),
                    (6, Some(m)) => modified_csi_tilde(17, m),
                    (7, Some(m)) => modified_csi_tilde(18, m),
                    (8, Some(m)) => modified_csi_tilde(19, m),
                    (9, Some(m)) => modified_csi_tilde(20, m),
                    (10, Some(m)) => modified_csi_tilde(21, m),
                    (11, Some(m)) => modified_csi_tilde(23, m),
                    (12, Some(m)) => modified_csi_tilde(24, m),
                    // F5–F12 without modifiers
                    (5, None) => TerminalInputPayload::Many(b"\x1b[15~"),
                    (6, None) => TerminalInputPayload::Many(b"\x1b[17~"),
                    (7, None) => TerminalInputPayload::Many(b"\x1b[18~"),
                    (8, None) => TerminalInputPayload::Many(b"\x1b[19~"),
                    (9, None) => TerminalInputPayload::Many(b"\x1b[20~"),
                    (10, None) => TerminalInputPayload::Many(b"\x1b[21~"),
                    (11, None) => TerminalInputPayload::Many(b"\x1b[23~"),
                    (12, None) => TerminalInputPayload::Many(b"\x1b[24~"),
                    _ => {
                        warn!("Unhandled function key: F{n}");
                        TerminalInputPayload::Many(b"")
                    }
                }
            }
        }
    }

    /// Build a CSI u KKP sequence with optional metadata fields.
    ///
    /// Full KKP CSI u format:
    /// ```text
    /// CSI codepoint[:shifted[:base]] ; modifiers[:event_type] [; text_codepoints] u
    /// ```
    ///
    /// Trailing default fields and sub-fields are omitted.
    fn build_csi_u(
        codepoint: u32,
        modifier_param: Option<u8>,
        flags: u32,
        meta: &KeyEventMeta,
    ) -> TerminalInputPayload {
        let report_event = flags & 2 != 0;
        let report_alt = flags & 4 != 0;
        let report_text = flags & 16 != 0;

        let event_code = if report_event {
            meta.event_type.kkp_code()
        } else {
            None
        };

        // Build the codepoint field: `codepoint[:shifted[:base]]`
        let codepoint_field = if report_alt {
            // Only ASCII codepoints have meaningful shifted forms in US QWERTY.
            let shifted = if codepoint <= 127 {
                #[allow(clippy::cast_possible_truncation)]
                let byte = codepoint as u8;
                us_qwerty_shifted(byte)
            } else {
                None
            };
            let base = codepoint; // US QWERTY assumption
            match shifted {
                Some(s) if s != codepoint => format!("{codepoint}:{s}:{base}"),
                _ => codepoint.to_string(),
            }
        } else {
            codepoint.to_string()
        };

        // Build the text field (flag 16): colon-separated codepoints
        let text_field = if report_text {
            meta.associated_text.as_ref().and_then(|t| {
                if t.is_empty() {
                    None
                } else {
                    let cps: Vec<String> = t.chars().map(|ch| (ch as u32).to_string()).collect();
                    Some(cps.join(":"))
                }
            })
        } else {
            None
        };

        // Build the modifier field: `modifiers[:event_type]`
        let has_modifier = modifier_param.is_some();
        let has_event = event_code.is_some();
        let has_text = text_field.is_some();

        // Assemble: need modifier param if we have event_type or text_field
        // (to avoid ambiguous omission of the second param).
        let needs_mod = has_modifier || has_event || has_text;

        let mut seq = format!("\x1b[{codepoint_field}");

        if needs_mod {
            let mod_val = modifier_param.unwrap_or(1); // 1 = no modifiers
            seq.push(';');
            seq.push_str(&mod_val.to_string());
            if let Some(et) = event_code {
                seq.push(':');
                seq.push_str(&et.to_string());
            }
        }

        if let Some(ref tf) = text_field {
            // `needs_mod` is always true when `text_field` is present (since
            // `has_text` feeds into `needs_mod`), so the modifier parameter
            // has already been written above.
            seq.push(';');
            seq.push_str(tf);
        }

        seq.push('u');
        TerminalInputPayload::Owned(seq.into_bytes())
    }

    /// Kitty Keyboard Protocol encoding path.
    ///
    /// Called when `DISAMBIGUATE` (bit 0) or `REPORT_ALL` (bit 3) is set in the
    /// active KKP flags.  Functional keys (arrows, Home/End, F-keys,
    /// Insert/Delete/PageUp/PageDown) retain their legacy encoding — only the
    /// modifier bitmask formula is shared (and it already matches KKP's
    /// `1 + shift + alt*2 + ctrl*4` convention).
    ///
    /// Text keys and certain C0-origin keys (Escape, Enter, Tab, Backspace)
    /// are re-encoded according to the active flag bits.
    ///
    /// Currently implemented flags:
    /// - Flag 1 (`DISAMBIGUATE_ESCAPE_CODES`): Ctrl+letter → `CSI code;5u`,
    ///   Escape → `CSI 27u`.  Enter/Tab/Backspace remain legacy.
    /// - Flag 8 (`REPORT_ALL_KEYS_AS_ESCAPE_CODES`): Every key as CSI u,
    ///   including Enter → `CSI 13u`, Tab → `CSI 9u`, Backspace → `CSI 127u`,
    ///   plain ASCII → `CSI codepoint u`.
    ///
    /// Flags 2, 4, 16 append metadata to CSI u sequences when the base encoding
    /// is already CSI u (i.e. when flag 1 or 8 activates the KKP path):
    ///
    /// - Flag 2 (report event types): appends `:event-type` to the modifier
    ///   parameter.
    /// - Flag 4 (alternate keys): appends `:shifted-key:base-layout-key` to the
    ///   codepoint field. Best-effort US QWERTY only.
    /// - Flag 16 (associated text): appends `;text-as-codepoints` as a third
    ///   parameter.
    // The KKP encoding path is inherently large: it must cover every variant
    // for a complete implementation.
    #[allow(clippy::too_many_lines, clippy::too_many_arguments)]
    fn to_payload_kkp(
        &self,
        flags: u32,
        decckm_mode: Decckm,
        keypad_mode: KeypadMode,
        backarrow_sends_bs: Decbkm,
        line_feed_mode: Lnm,
        meta: &KeyEventMeta,
    ) -> TerminalInputPayload {
        let report_all = flags & 8 != 0;
        let disambiguate = flags & 1 != 0;

        match self {
            // ── Plain ASCII ─────────────────────────────────────────────
            Self::Ascii(c) => {
                if report_all {
                    // Flag 8: every printable key as CSI u.
                    // Uppercase letters are sent as the lowercase codepoint
                    // with Shift modifier.
                    if c.is_ascii_uppercase() {
                        let lower = u32::from(c.to_ascii_lowercase());
                        Self::build_csi_u(lower, Some(2), flags, meta)
                    } else {
                        let code = u32::from(*c);
                        Self::build_csi_u(code, None, flags, meta)
                    }
                } else {
                    // Flags 1/2/4/16 alone don't affect plain ASCII.
                    TerminalInputPayload::Single(*c)
                }
            }

            // ── Ctrl+letter ─────────────────────────────────────────────
            Self::Ctrl(c) => {
                if disambiguate || report_all {
                    // KKP: Ctrl+letter → CSI lowercase_code ; 5 u
                    let code = u32::from(c.to_ascii_lowercase());
                    Self::build_csi_u(code, Some(5), flags, meta)
                } else {
                    // Flags 2/4/16 alone: legacy C0 encoding.
                    TerminalInputPayload::Single(char_to_ctrl_code(*c))
                }
            }

            // ── Enter ───────────────────────────────────────────────────
            Self::Enter => {
                if report_all {
                    Self::build_csi_u(13, None, flags, meta)
                } else {
                    // Flag 1 exception: Enter still sends legacy bytes.
                    if line_feed_mode == Lnm::NewLine {
                        TerminalInputPayload::Many(b"\x0d\x0a")
                    } else {
                        TerminalInputPayload::Single(char_to_ctrl_code(b'm'))
                    }
                }
            }

            // ── LineFeed ────────────────────────────────────────────────
            Self::LineFeed => TerminalInputPayload::Single(b'\n'),

            // ── Backspace ───────────────────────────────────────────────
            Self::Backspace => {
                if report_all {
                    Self::build_csi_u(127, None, flags, meta)
                } else {
                    // Flag 1 exception: Backspace still sends legacy bytes.
                    if backarrow_sends_bs == Decbkm::BackarrowSendsBs {
                        TerminalInputPayload::Single(char_to_ctrl_code(b'H'))
                    } else {
                        TerminalInputPayload::Single(0x7F)
                    }
                }
            }

            // ── Tab ─────────────────────────────────────────────────────
            Self::Tab => {
                if report_all {
                    Self::build_csi_u(9, None, flags, meta)
                } else {
                    // Flag 1 exception: Tab still sends legacy byte.
                    TerminalInputPayload::Single(char_to_ctrl_code(b'i'))
                }
            }

            // ── Escape ──────────────────────────────────────────────────
            Self::Escape => {
                if disambiguate || report_all {
                    // KKP: Escape is disambiguated as CSI 27 u.
                    Self::build_csi_u(27, None, flags, meta)
                } else {
                    // Flags 2/4/16 alone: legacy bare ESC byte.
                    TerminalInputPayload::Single(b'\x1b')
                }
            }

            // ── Functional keys: retain legacy encoding ─────────────────
            //
            // Arrow keys, Home, End, F-keys, Insert, Delete, PageUp,
            // PageDown all keep their legacy xterm encoding.  The modifier
            // parameter formula (1 + shift + alt*2 + ctrl*4) is identical
            // between xterm and KKP, so no change is needed.
            Self::ArrowRight(mods) => match mods.modifier_param() {
                Some(m) => modified_csi_final(m, b'C'),
                None if decckm_mode == Decckm::Application => TerminalInputPayload::Many(b"\x1bOC"),
                None => TerminalInputPayload::Many(b"\x1b[C"),
            },
            Self::ArrowLeft(mods) => match mods.modifier_param() {
                Some(m) => modified_csi_final(m, b'D'),
                None if decckm_mode == Decckm::Application => TerminalInputPayload::Many(b"\x1bOD"),
                None => TerminalInputPayload::Many(b"\x1b[D"),
            },
            Self::ArrowUp(mods) => match mods.modifier_param() {
                Some(m) => modified_csi_final(m, b'A'),
                None if decckm_mode == Decckm::Application => TerminalInputPayload::Many(b"\x1bOA"),
                None => TerminalInputPayload::Many(b"\x1b[A"),
            },
            Self::ArrowDown(mods) => match mods.modifier_param() {
                Some(m) => modified_csi_final(m, b'B'),
                None if decckm_mode == Decckm::Application => TerminalInputPayload::Many(b"\x1bOB"),
                None => TerminalInputPayload::Many(b"\x1b[B"),
            },
            Self::Home(mods) => match mods.modifier_param() {
                Some(m) => modified_csi_final(m, b'H'),
                None if decckm_mode == Decckm::Application => TerminalInputPayload::Many(b"\x1bOH"),
                None => TerminalInputPayload::Many(b"\x1b[H"),
            },
            Self::End(mods) => match mods.modifier_param() {
                Some(m) => modified_csi_final(m, b'F'),
                None if decckm_mode == Decckm::Application => TerminalInputPayload::Many(b"\x1bOF"),
                None => TerminalInputPayload::Many(b"\x1b[F"),
            },
            Self::Delete(mods) => mods
                .modifier_param()
                .map_or(TerminalInputPayload::Many(b"\x1b[3~"), |m| {
                    modified_csi_tilde(3, m)
                }),
            Self::Insert(mods) => mods
                .modifier_param()
                .map_or(TerminalInputPayload::Many(b"\x1b[2~"), |m| {
                    modified_csi_tilde(2, m)
                }),
            Self::PageUp(mods) => mods
                .modifier_param()
                .map_or(TerminalInputPayload::Many(b"\x1b[5~"), |m| {
                    modified_csi_tilde(5, m)
                }),
            Self::PageDown(mods) => mods
                .modifier_param()
                .map_or(TerminalInputPayload::Many(b"\x1b[6~"), |m| {
                    modified_csi_tilde(6, m)
                }),
            Self::KeyPad(c) => {
                if keypad_mode == KeypadMode::Numeric {
                    TerminalInputPayload::Single(*c)
                } else {
                    match c {
                        0 => TerminalInputPayload::Many(b"\x1b[Op"),
                        1 => TerminalInputPayload::Many(b"\x1b[Oq"),
                        2 => TerminalInputPayload::Many(b"\x1b[Or"),
                        3 => TerminalInputPayload::Many(b"\x1b[Os"),
                        4 => TerminalInputPayload::Many(b"\x1b[Ot"),
                        5 => TerminalInputPayload::Many(b"\x1b[Ou"),
                        6 => TerminalInputPayload::Many(b"\x1b[Ov"),
                        7 => TerminalInputPayload::Many(b"\x1b[Ow"),
                        8 => TerminalInputPayload::Many(b"\x1b[Ox"),
                        9 => TerminalInputPayload::Many(b"\x1b[Oy"),
                        b'-' => TerminalInputPayload::Many(b"\x1b[Om"),
                        b',' => TerminalInputPayload::Many(b"\x1b[Ol"),
                        b'.' => TerminalInputPayload::Many(b"\x1b[On"),
                        b'\n' => TerminalInputPayload::Many(b"\x1b[OM"),
                        _ => {
                            warn!("Unknown keypad key: {c}");
                            TerminalInputPayload::Single(*c)
                        }
                    }
                }
            }
            Self::LostFocus => TerminalInputPayload::Many(b"\x1b[O"),
            Self::InFocus => TerminalInputPayload::Many(b"\x1b[I"),
            Self::FunctionKey(n, mods) => {
                let mod_param = mods.modifier_param();
                match (n, mod_param) {
                    (1, Some(m)) => modified_csi_final(m, b'P'),
                    (2, Some(m)) => modified_csi_final(m, b'Q'),
                    (3, Some(m)) => modified_csi_final(m, b'R'),
                    (4, Some(m)) => modified_csi_final(m, b'S'),
                    (1, None) => TerminalInputPayload::Many(b"\x1bOP"),
                    (2, None) => TerminalInputPayload::Many(b"\x1bOQ"),
                    (3, None) => TerminalInputPayload::Many(b"\x1bOR"),
                    (4, None) => TerminalInputPayload::Many(b"\x1bOS"),
                    (5, Some(m)) => modified_csi_tilde(15, m),
                    (6, Some(m)) => modified_csi_tilde(17, m),
                    (7, Some(m)) => modified_csi_tilde(18, m),
                    (8, Some(m)) => modified_csi_tilde(19, m),
                    (9, Some(m)) => modified_csi_tilde(20, m),
                    (10, Some(m)) => modified_csi_tilde(21, m),
                    (11, Some(m)) => modified_csi_tilde(23, m),
                    (12, Some(m)) => modified_csi_tilde(24, m),
                    (5, None) => TerminalInputPayload::Many(b"\x1b[15~"),
                    (6, None) => TerminalInputPayload::Many(b"\x1b[17~"),
                    (7, None) => TerminalInputPayload::Many(b"\x1b[18~"),
                    (8, None) => TerminalInputPayload::Many(b"\x1b[19~"),
                    (9, None) => TerminalInputPayload::Many(b"\x1b[20~"),
                    (10, None) => TerminalInputPayload::Many(b"\x1b[21~"),
                    (11, None) => TerminalInputPayload::Many(b"\x1b[23~"),
                    (12, None) => TerminalInputPayload::Many(b"\x1b[24~"),
                    _ => {
                        warn!("Unhandled function key: F{n}");
                        TerminalInputPayload::Many(b"")
                    }
                }
            }
        }
    }
}
