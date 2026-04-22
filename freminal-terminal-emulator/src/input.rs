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
                // SAFETY: The guard `codepoint <= 127` ensures the value fits in u8 without
                // truncation. The `as` cast is lossless here.
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
                } else if meta.event_type != KeyEventType::Press {
                    // Flag 2 without flag 8: plain ASCII normally uses legacy
                    // encoding, but release/repeat events MUST use CSI u so
                    // the event-type suffix can be included.  Sending a raw
                    // byte for a release would duplicate the key press.
                    if c.is_ascii_uppercase() {
                        let lower = u32::from(c.to_ascii_lowercase());
                        Self::build_csi_u(lower, Some(2), flags, meta)
                    } else {
                        let code = u32::from(*c);
                        Self::build_csi_u(code, None, flags, meta)
                    }
                } else {
                    // Flags 1/2/4/16 alone don't affect plain ASCII presses.
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
                if report_all || meta.event_type != KeyEventType::Press {
                    // Flag 8, or release/repeat: use CSI u so event type is
                    // included and legacy bytes aren't duplicated on release.
                    Self::build_csi_u(13, None, flags, meta)
                } else {
                    // Flag 1 exception: Enter press still sends legacy bytes.
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
                if report_all || meta.event_type != KeyEventType::Press {
                    // Flag 8, or release/repeat: use CSI u so event type is
                    // included and legacy bytes aren't duplicated on release.
                    Self::build_csi_u(127, None, flags, meta)
                } else {
                    // Flag 1 exception: Backspace press still sends legacy bytes.
                    if backarrow_sends_bs == Decbkm::BackarrowSendsBs {
                        TerminalInputPayload::Single(char_to_ctrl_code(b'H'))
                    } else {
                        TerminalInputPayload::Single(0x7F)
                    }
                }
            }

            // ── Tab ─────────────────────────────────────────────────────
            Self::Tab => {
                if report_all || meta.event_type != KeyEventType::Press {
                    // Flag 8, or release/repeat: use CSI u so event type is
                    // included and legacy bytes aren't duplicated on release.
                    Self::build_csi_u(9, None, flags, meta)
                } else {
                    // Flag 1 exception: Tab press still sends legacy byte.
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

#[cfg(test)]
mod tests {
    use super::*;
    use freminal_common::buffer_states::modes::{
        application_escape_key::ApplicationEscapeKey, decbkm::Decbkm, decckm::Decckm,
        keypad::KeypadMode, lnm::Lnm,
    };

    /// Convenience: call `to_payload` with all-default modes and zero KKP flags.
    fn to_payload_defaults(input: &TerminalInput) -> TerminalInputPayload {
        input.to_payload(
            Decckm::Ansi,
            KeypadMode::Numeric,
            0,
            ApplicationEscapeKey::Reset,
            Decbkm::BackarrowSendsDel,
            Lnm::LineFeed,
            0,
            &KeyEventMeta::PRESS,
        )
    }

    /// Convenience: call `to_payload` forcing the KKP path with given flags.
    fn to_payload_kkp(input: &TerminalInput, flags: u32) -> TerminalInputPayload {
        input.to_payload(
            Decckm::Ansi,
            KeypadMode::Numeric,
            0,
            ApplicationEscapeKey::Reset,
            Decbkm::BackarrowSendsDel,
            Lnm::LineFeed,
            flags,
            &KeyEventMeta::PRESS,
        )
    }

    // ── us_qwerty_shifted ────────────────────────────────────────────────────

    #[test]
    fn us_qwerty_shifted_digits() {
        assert_eq!(super::us_qwerty_shifted(b'1'), Some(u32::from(b'!')));
        assert_eq!(super::us_qwerty_shifted(b'2'), Some(u32::from(b'@')));
        assert_eq!(super::us_qwerty_shifted(b'3'), Some(u32::from(b'#')));
        assert_eq!(super::us_qwerty_shifted(b'4'), Some(u32::from(b'$')));
        assert_eq!(super::us_qwerty_shifted(b'5'), Some(u32::from(b'%')));
        assert_eq!(super::us_qwerty_shifted(b'6'), Some(u32::from(b'^')));
        assert_eq!(super::us_qwerty_shifted(b'7'), Some(u32::from(b'&')));
        assert_eq!(super::us_qwerty_shifted(b'8'), Some(u32::from(b'*')));
        assert_eq!(super::us_qwerty_shifted(b'9'), Some(u32::from(b'(')));
        assert_eq!(super::us_qwerty_shifted(b'0'), Some(u32::from(b')')));
    }

    #[test]
    fn us_qwerty_shifted_punctuation() {
        assert_eq!(super::us_qwerty_shifted(b'-'), Some(u32::from(b'_')));
        assert_eq!(super::us_qwerty_shifted(b'='), Some(u32::from(b'+')));
        assert_eq!(super::us_qwerty_shifted(b'['), Some(u32::from(b'{')));
        assert_eq!(super::us_qwerty_shifted(b']'), Some(u32::from(b'}')));
        assert_eq!(super::us_qwerty_shifted(b'\\'), Some(u32::from(b'|')));
        assert_eq!(super::us_qwerty_shifted(b';'), Some(u32::from(b':')));
        assert_eq!(super::us_qwerty_shifted(b'\''), Some(u32::from(b'"')));
        assert_eq!(super::us_qwerty_shifted(b','), Some(u32::from(b'<')));
        assert_eq!(super::us_qwerty_shifted(b'.'), Some(u32::from(b'>')));
        assert_eq!(super::us_qwerty_shifted(b'/'), Some(u32::from(b'?')));
        assert_eq!(super::us_qwerty_shifted(b'`'), Some(u32::from(b'~')));
    }

    #[test]
    fn us_qwerty_shifted_lowercase_letters() {
        assert_eq!(super::us_qwerty_shifted(b'a'), Some(u32::from(b'A')));
        assert_eq!(super::us_qwerty_shifted(b'z'), Some(u32::from(b'Z')));
        assert_eq!(super::us_qwerty_shifted(b'm'), Some(u32::from(b'M')));
    }

    #[test]
    fn us_qwerty_shifted_out_of_range() {
        // Uppercase and non-ASCII have no shifted form
        assert_eq!(super::us_qwerty_shifted(b'A'), None);
        assert_eq!(super::us_qwerty_shifted(b'Z'), None);
        assert_eq!(super::us_qwerty_shifted(0x80), None);
        assert_eq!(super::us_qwerty_shifted(0x00), None);
    }

    // ── to_payload: arrow keys ───────────────────────────────────────────────

    #[test]
    fn arrow_right_normal_mode() {
        let p = to_payload_defaults(&TerminalInput::ArrowRight(KeyModifiers::NONE));
        assert_eq!(p, TerminalInputPayload::Many(b"\x1b[C"));
    }

    #[test]
    fn arrow_right_application_mode() {
        let p = TerminalInput::ArrowRight(KeyModifiers::NONE).to_payload(
            Decckm::Application,
            KeypadMode::Numeric,
            0,
            ApplicationEscapeKey::Reset,
            Decbkm::BackarrowSendsDel,
            Lnm::LineFeed,
            0,
            &KeyEventMeta::PRESS,
        );
        assert_eq!(p, TerminalInputPayload::Many(b"\x1bOC"));
    }

    #[test]
    fn arrow_right_with_modifier() {
        let mods = KeyModifiers {
            shift: true,
            ctrl: false,
            alt: false,
        };
        let p = to_payload_defaults(&TerminalInput::ArrowRight(mods));
        assert_eq!(p, TerminalInputPayload::Owned(b"\x1b[1;2C".to_vec()));
    }

    #[test]
    fn arrow_left_normal_mode() {
        let p = to_payload_defaults(&TerminalInput::ArrowLeft(KeyModifiers::NONE));
        assert_eq!(p, TerminalInputPayload::Many(b"\x1b[D"));
    }

    #[test]
    fn arrow_left_application_mode() {
        let p = TerminalInput::ArrowLeft(KeyModifiers::NONE).to_payload(
            Decckm::Application,
            KeypadMode::Numeric,
            0,
            ApplicationEscapeKey::Reset,
            Decbkm::BackarrowSendsDel,
            Lnm::LineFeed,
            0,
            &KeyEventMeta::PRESS,
        );
        assert_eq!(p, TerminalInputPayload::Many(b"\x1bOD"));
    }

    #[test]
    fn arrow_up_normal_mode() {
        let p = to_payload_defaults(&TerminalInput::ArrowUp(KeyModifiers::NONE));
        assert_eq!(p, TerminalInputPayload::Many(b"\x1b[A"));
    }

    #[test]
    fn arrow_up_application_mode() {
        let p = TerminalInput::ArrowUp(KeyModifiers::NONE).to_payload(
            Decckm::Application,
            KeypadMode::Numeric,
            0,
            ApplicationEscapeKey::Reset,
            Decbkm::BackarrowSendsDel,
            Lnm::LineFeed,
            0,
            &KeyEventMeta::PRESS,
        );
        assert_eq!(p, TerminalInputPayload::Many(b"\x1bOA"));
    }

    #[test]
    fn arrow_down_normal_mode() {
        let p = to_payload_defaults(&TerminalInput::ArrowDown(KeyModifiers::NONE));
        assert_eq!(p, TerminalInputPayload::Many(b"\x1b[B"));
    }

    #[test]
    fn arrow_down_application_mode() {
        let p = TerminalInput::ArrowDown(KeyModifiers::NONE).to_payload(
            Decckm::Application,
            KeypadMode::Numeric,
            0,
            ApplicationEscapeKey::Reset,
            Decbkm::BackarrowSendsDel,
            Lnm::LineFeed,
            0,
            &KeyEventMeta::PRESS,
        );
        assert_eq!(p, TerminalInputPayload::Many(b"\x1bOB"));
    }

    // ── to_payload: Home/End ─────────────────────────────────────────────────

    #[test]
    fn home_normal_mode() {
        let p = to_payload_defaults(&TerminalInput::Home(KeyModifiers::NONE));
        assert_eq!(p, TerminalInputPayload::Many(b"\x1b[H"));
    }

    #[test]
    fn home_application_mode() {
        let p = TerminalInput::Home(KeyModifiers::NONE).to_payload(
            Decckm::Application,
            KeypadMode::Numeric,
            0,
            ApplicationEscapeKey::Reset,
            Decbkm::BackarrowSendsDel,
            Lnm::LineFeed,
            0,
            &KeyEventMeta::PRESS,
        );
        assert_eq!(p, TerminalInputPayload::Many(b"\x1bOH"));
    }

    #[test]
    fn home_with_ctrl_modifier() {
        let mods = KeyModifiers {
            shift: false,
            ctrl: true,
            alt: false,
        };
        let p = to_payload_defaults(&TerminalInput::Home(mods));
        assert_eq!(p, TerminalInputPayload::Owned(b"\x1b[1;5H".to_vec()));
    }

    #[test]
    fn end_normal_mode() {
        let p = to_payload_defaults(&TerminalInput::End(KeyModifiers::NONE));
        assert_eq!(p, TerminalInputPayload::Many(b"\x1b[F"));
    }

    #[test]
    fn end_application_mode() {
        let p = TerminalInput::End(KeyModifiers::NONE).to_payload(
            Decckm::Application,
            KeypadMode::Numeric,
            0,
            ApplicationEscapeKey::Reset,
            Decbkm::BackarrowSendsDel,
            Lnm::LineFeed,
            0,
            &KeyEventMeta::PRESS,
        );
        assert_eq!(p, TerminalInputPayload::Many(b"\x1bOF"));
    }

    // ── to_payload: Delete/Insert/PageUp/PageDown ────────────────────────────

    #[test]
    fn delete_no_modifier() {
        let p = to_payload_defaults(&TerminalInput::Delete(KeyModifiers::NONE));
        assert_eq!(p, TerminalInputPayload::Many(b"\x1b[3~"));
    }

    #[test]
    fn delete_with_shift() {
        let mods = KeyModifiers {
            shift: true,
            ctrl: false,
            alt: false,
        };
        let p = to_payload_defaults(&TerminalInput::Delete(mods));
        assert_eq!(p, TerminalInputPayload::Owned(b"\x1b[3;2~".to_vec()));
    }

    #[test]
    fn insert_no_modifier() {
        let p = to_payload_defaults(&TerminalInput::Insert(KeyModifiers::NONE));
        assert_eq!(p, TerminalInputPayload::Many(b"\x1b[2~"));
    }

    #[test]
    fn insert_with_modifier() {
        let mods = KeyModifiers {
            shift: false,
            ctrl: true,
            alt: false,
        };
        let p = to_payload_defaults(&TerminalInput::Insert(mods));
        assert_eq!(p, TerminalInputPayload::Owned(b"\x1b[2;5~".to_vec()));
    }

    #[test]
    fn pageup_no_modifier() {
        let p = to_payload_defaults(&TerminalInput::PageUp(KeyModifiers::NONE));
        assert_eq!(p, TerminalInputPayload::Many(b"\x1b[5~"));
    }

    #[test]
    fn pageup_with_modifier() {
        let mods = KeyModifiers {
            shift: true,
            ctrl: false,
            alt: false,
        };
        let p = to_payload_defaults(&TerminalInput::PageUp(mods));
        assert_eq!(p, TerminalInputPayload::Owned(b"\x1b[5;2~".to_vec()));
    }

    #[test]
    fn pagedown_no_modifier() {
        let p = to_payload_defaults(&TerminalInput::PageDown(KeyModifiers::NONE));
        assert_eq!(p, TerminalInputPayload::Many(b"\x1b[6~"));
    }

    #[test]
    fn pagedown_with_modifier() {
        let mods = KeyModifiers {
            shift: true,
            ctrl: true,
            alt: false,
        };
        let p = to_payload_defaults(&TerminalInput::PageDown(mods));
        assert_eq!(p, TerminalInputPayload::Owned(b"\x1b[6;6~".to_vec()));
    }

    // ── to_payload: Focus events ─────────────────────────────────────────────

    #[test]
    fn lost_focus_payload() {
        let p = to_payload_defaults(&TerminalInput::LostFocus);
        assert_eq!(p, TerminalInputPayload::Many(b"\x1b[O"));
    }

    #[test]
    fn in_focus_payload() {
        let p = to_payload_defaults(&TerminalInput::InFocus);
        assert_eq!(p, TerminalInputPayload::Many(b"\x1b[I"));
    }

    // ── to_payload: Function keys F1-F12 without modifiers ──────────────────

    #[test]
    fn function_keys_f1_f4_ss3_form() {
        assert_eq!(
            to_payload_defaults(&TerminalInput::FunctionKey(1, KeyModifiers::NONE)),
            TerminalInputPayload::Many(b"\x1bOP")
        );
        assert_eq!(
            to_payload_defaults(&TerminalInput::FunctionKey(2, KeyModifiers::NONE)),
            TerminalInputPayload::Many(b"\x1bOQ")
        );
        assert_eq!(
            to_payload_defaults(&TerminalInput::FunctionKey(3, KeyModifiers::NONE)),
            TerminalInputPayload::Many(b"\x1bOR")
        );
        assert_eq!(
            to_payload_defaults(&TerminalInput::FunctionKey(4, KeyModifiers::NONE)),
            TerminalInputPayload::Many(b"\x1bOS")
        );
    }

    #[test]
    fn function_keys_f5_f12_csi_tilde_form() {
        assert_eq!(
            to_payload_defaults(&TerminalInput::FunctionKey(5, KeyModifiers::NONE)),
            TerminalInputPayload::Many(b"\x1b[15~")
        );
        assert_eq!(
            to_payload_defaults(&TerminalInput::FunctionKey(6, KeyModifiers::NONE)),
            TerminalInputPayload::Many(b"\x1b[17~")
        );
        assert_eq!(
            to_payload_defaults(&TerminalInput::FunctionKey(7, KeyModifiers::NONE)),
            TerminalInputPayload::Many(b"\x1b[18~")
        );
        assert_eq!(
            to_payload_defaults(&TerminalInput::FunctionKey(8, KeyModifiers::NONE)),
            TerminalInputPayload::Many(b"\x1b[19~")
        );
        assert_eq!(
            to_payload_defaults(&TerminalInput::FunctionKey(9, KeyModifiers::NONE)),
            TerminalInputPayload::Many(b"\x1b[20~")
        );
        assert_eq!(
            to_payload_defaults(&TerminalInput::FunctionKey(10, KeyModifiers::NONE)),
            TerminalInputPayload::Many(b"\x1b[21~")
        );
        assert_eq!(
            to_payload_defaults(&TerminalInput::FunctionKey(11, KeyModifiers::NONE)),
            TerminalInputPayload::Many(b"\x1b[23~")
        );
        assert_eq!(
            to_payload_defaults(&TerminalInput::FunctionKey(12, KeyModifiers::NONE)),
            TerminalInputPayload::Many(b"\x1b[24~")
        );
    }

    #[test]
    fn function_key_f1_with_modifier() {
        let mods = KeyModifiers {
            shift: true,
            ctrl: false,
            alt: false,
        };
        let p = to_payload_defaults(&TerminalInput::FunctionKey(1, mods));
        assert_eq!(p, TerminalInputPayload::Owned(b"\x1b[1;2P".to_vec()));
    }

    #[test]
    fn function_key_f5_with_modifier() {
        let mods = KeyModifiers {
            shift: false,
            ctrl: true,
            alt: false,
        };
        let p = to_payload_defaults(&TerminalInput::FunctionKey(5, mods));
        assert_eq!(p, TerminalInputPayload::Owned(b"\x1b[15;5~".to_vec()));
    }

    #[test]
    fn function_key_f12_with_modifier() {
        let mods = KeyModifiers {
            shift: true,
            ctrl: false,
            alt: false,
        };
        let p = to_payload_defaults(&TerminalInput::FunctionKey(12, mods));
        assert_eq!(p, TerminalInputPayload::Owned(b"\x1b[24;2~".to_vec()));
    }

    #[test]
    fn function_key_unknown_returns_empty() {
        // F13 is not in the match — hits the fallback `_ =>` arm.
        let p = to_payload_defaults(&TerminalInput::FunctionKey(13, KeyModifiers::NONE));
        assert_eq!(p, TerminalInputPayload::Many(b""));
    }

    // ── to_payload: KeyPad ───────────────────────────────────────────────────

    #[test]
    fn keypad_numeric_mode_sends_raw_byte() {
        let p = TerminalInput::KeyPad(b'5').to_payload(
            Decckm::Ansi,
            KeypadMode::Numeric,
            0,
            ApplicationEscapeKey::Reset,
            Decbkm::BackarrowSendsDel,
            Lnm::LineFeed,
            0,
            &KeyEventMeta::PRESS,
        );
        assert_eq!(p, TerminalInputPayload::Single(b'5'));
    }

    #[test]
    fn keypad_application_mode_digits() {
        let payload_for = |c: u8| -> TerminalInputPayload {
            TerminalInput::KeyPad(c).to_payload(
                Decckm::Ansi,
                KeypadMode::Application,
                0,
                ApplicationEscapeKey::Reset,
                Decbkm::BackarrowSendsDel,
                Lnm::LineFeed,
                0,
                &KeyEventMeta::PRESS,
            )
        };
        assert_eq!(payload_for(0), TerminalInputPayload::Many(b"\x1b[Op"));
        assert_eq!(payload_for(1), TerminalInputPayload::Many(b"\x1b[Oq"));
        assert_eq!(payload_for(2), TerminalInputPayload::Many(b"\x1b[Or"));
        assert_eq!(payload_for(3), TerminalInputPayload::Many(b"\x1b[Os"));
        assert_eq!(payload_for(4), TerminalInputPayload::Many(b"\x1b[Ot"));
        assert_eq!(payload_for(5), TerminalInputPayload::Many(b"\x1b[Ou"));
        assert_eq!(payload_for(6), TerminalInputPayload::Many(b"\x1b[Ov"));
        assert_eq!(payload_for(7), TerminalInputPayload::Many(b"\x1b[Ow"));
        assert_eq!(payload_for(8), TerminalInputPayload::Many(b"\x1b[Ox"));
        assert_eq!(payload_for(9), TerminalInputPayload::Many(b"\x1b[Oy"));
    }

    #[test]
    fn keypad_application_mode_special_keys() {
        let payload_for = |c: u8| -> TerminalInputPayload {
            TerminalInput::KeyPad(c).to_payload(
                Decckm::Ansi,
                KeypadMode::Application,
                0,
                ApplicationEscapeKey::Reset,
                Decbkm::BackarrowSendsDel,
                Lnm::LineFeed,
                0,
                &KeyEventMeta::PRESS,
            )
        };
        assert_eq!(payload_for(b'-'), TerminalInputPayload::Many(b"\x1b[Om"));
        assert_eq!(payload_for(b','), TerminalInputPayload::Many(b"\x1b[Ol"));
        assert_eq!(payload_for(b'.'), TerminalInputPayload::Many(b"\x1b[On"));
        assert_eq!(payload_for(b'\n'), TerminalInputPayload::Many(b"\x1b[OM"));
    }

    #[test]
    fn keypad_application_mode_unknown_key_fallback() {
        // A byte not in the match → fallback to Single
        let p = TerminalInput::KeyPad(b'!').to_payload(
            Decckm::Ansi,
            KeypadMode::Application,
            0,
            ApplicationEscapeKey::Reset,
            Decbkm::BackarrowSendsDel,
            Lnm::LineFeed,
            0,
            &KeyEventMeta::PRESS,
        );
        assert_eq!(p, TerminalInputPayload::Single(b'!'));
    }

    // ── to_payload: Enter with LNM ───────────────────────────────────────────

    #[test]
    fn enter_lnm_normal_sends_cr() {
        let p = to_payload_defaults(&TerminalInput::Enter);
        assert_eq!(p, TerminalInputPayload::Single(b'\r'));
    }

    #[test]
    fn enter_lnm_newline_sends_crlf() {
        let p = TerminalInput::Enter.to_payload(
            Decckm::Ansi,
            KeypadMode::Numeric,
            0,
            ApplicationEscapeKey::Reset,
            Decbkm::BackarrowSendsDel,
            Lnm::NewLine,
            0,
            &KeyEventMeta::PRESS,
        );
        assert_eq!(p, TerminalInputPayload::Many(b"\x0d\x0a"));
    }

    // ── to_payload: Backspace with DECBKM ────────────────────────────────────

    #[test]
    fn backspace_decbkm_del_sends_del() {
        let p = to_payload_defaults(&TerminalInput::Backspace);
        assert_eq!(p, TerminalInputPayload::Single(0x7F));
    }

    #[test]
    fn backspace_decbkm_bs_sends_bs() {
        let p = TerminalInput::Backspace.to_payload(
            Decckm::Ansi,
            KeypadMode::Numeric,
            0,
            ApplicationEscapeKey::Reset,
            Decbkm::BackarrowSendsBs,
            Lnm::LineFeed,
            0,
            &KeyEventMeta::PRESS,
        );
        assert_eq!(p, TerminalInputPayload::Single(0x08));
    }

    // ── to_payload: Escape with ApplicationEscapeKey ─────────────────────────

    #[test]
    fn escape_plain_sends_esc_byte() {
        let p = to_payload_defaults(&TerminalInput::Escape);
        assert_eq!(p, TerminalInputPayload::Single(0x1b));
    }

    #[test]
    fn escape_application_escape_key_sends_csi_sequence() {
        let p = TerminalInput::Escape.to_payload(
            Decckm::Ansi,
            KeypadMode::Numeric,
            0,
            ApplicationEscapeKey::Set,
            Decbkm::BackarrowSendsDel,
            Lnm::LineFeed,
            0,
            &KeyEventMeta::PRESS,
        );
        assert_eq!(p, TerminalInputPayload::Owned(b"\x1b[27;1;27~".to_vec()));
    }

    // ── to_payload: Tab ──────────────────────────────────────────────────────

    #[test]
    fn tab_sends_ctrl_i() {
        let p = to_payload_defaults(&TerminalInput::Tab);
        assert_eq!(p, TerminalInputPayload::Single(0x09));
    }

    // ── to_payload: Ctrl with modifyOtherKeys ────────────────────────────────

    #[test]
    fn ctrl_c_level0_sends_etx() {
        let p = TerminalInput::Ctrl(b'c').to_payload(
            Decckm::Ansi,
            KeypadMode::Numeric,
            0, // modifyOtherKeys=0 → legacy
            ApplicationEscapeKey::Reset,
            Decbkm::BackarrowSendsDel,
            Lnm::LineFeed,
            0,
            &KeyEventMeta::PRESS,
        );
        assert_eq!(p, TerminalInputPayload::Single(0x03)); // 'c' & 0x1F
    }

    #[test]
    fn ctrl_c_level2_sends_csi27_sequence() {
        let p = TerminalInput::Ctrl(b'c').to_payload(
            Decckm::Ansi,
            KeypadMode::Numeric,
            2, // modifyOtherKeys=2
            ApplicationEscapeKey::Reset,
            Decbkm::BackarrowSendsDel,
            Lnm::LineFeed,
            0,
            &KeyEventMeta::PRESS,
        );
        // CSI 27 ; 5 ; 99 ~ (ctrl modifier = 5, 'c' = 99)
        assert_eq!(p, TerminalInputPayload::Owned(b"\x1b[27;5;99~".to_vec()));
    }

    // ── to_payload_kkp: Ctrl ─────────────────────────────────────────────────

    #[test]
    fn kkp_ctrl_disambiguate_sends_csi_u() {
        // Flag 1 (DISAMBIGUATE): Ctrl+c → CSI 99;5u
        let p = to_payload_kkp(&TerminalInput::Ctrl(b'c'), 1);
        assert_eq!(p, TerminalInputPayload::Owned(b"\x1b[99;5u".to_vec()));
    }

    #[test]
    fn kkp_ctrl_report_all_sends_csi_u() {
        // Flag 8 (REPORT_ALL): Ctrl+c → CSI 99;5u
        let p = to_payload_kkp(&TerminalInput::Ctrl(b'c'), 8);
        assert_eq!(p, TerminalInputPayload::Owned(b"\x1b[99;5u".to_vec()));
    }

    #[test]
    fn kkp_ctrl_uppercase_input_lowercased() {
        // Ctrl+C (uppercase) → same as Ctrl+c
        let p = to_payload_kkp(&TerminalInput::Ctrl(b'C'), 1);
        assert_eq!(p, TerminalInputPayload::Owned(b"\x1b[99;5u".to_vec()));
    }

    // ── to_payload_kkp: Enter ────────────────────────────────────────────────

    #[test]
    fn kkp_enter_report_all_sends_csi_13u() {
        let p = to_payload_kkp(&TerminalInput::Enter, 8);
        assert_eq!(p, TerminalInputPayload::Owned(b"\x1b[13u".to_vec()));
    }

    #[test]
    fn kkp_enter_disambiguate_only_lnm_normal_sends_cr() {
        // Flag 1 only (no report_all): Enter still sends legacy
        let p = to_payload_kkp(&TerminalInput::Enter, 1);
        assert_eq!(p, TerminalInputPayload::Single(b'\r'));
    }

    #[test]
    fn kkp_enter_disambiguate_only_lnm_newline_sends_crlf() {
        let p = TerminalInput::Enter.to_payload(
            Decckm::Ansi,
            KeypadMode::Numeric,
            0,
            ApplicationEscapeKey::Reset,
            Decbkm::BackarrowSendsDel,
            Lnm::NewLine,
            1, // flag 1 only
            &KeyEventMeta::PRESS,
        );
        assert_eq!(p, TerminalInputPayload::Many(b"\x0d\x0a"));
    }

    // ── to_payload_kkp: Backspace ────────────────────────────────────────────

    #[test]
    fn kkp_backspace_report_all_sends_csi_127u() {
        let p = to_payload_kkp(&TerminalInput::Backspace, 8);
        assert_eq!(p, TerminalInputPayload::Owned(b"\x1b[127u".to_vec()));
    }

    #[test]
    fn kkp_backspace_disambiguate_only_decbkm_del() {
        // Flag 1 only: legacy path, DECBKM=Del → 0x7F
        let p = to_payload_kkp(&TerminalInput::Backspace, 1);
        assert_eq!(p, TerminalInputPayload::Single(0x7F));
    }

    #[test]
    fn kkp_backspace_disambiguate_only_decbkm_bs() {
        let p = TerminalInput::Backspace.to_payload(
            Decckm::Ansi,
            KeypadMode::Numeric,
            0,
            ApplicationEscapeKey::Reset,
            Decbkm::BackarrowSendsBs,
            Lnm::LineFeed,
            1, // flag 1 only
            &KeyEventMeta::PRESS,
        );
        assert_eq!(p, TerminalInputPayload::Single(0x08));
    }

    // ── to_payload_kkp: Tab ──────────────────────────────────────────────────

    #[test]
    fn kkp_tab_report_all_sends_csi_9u() {
        let p = to_payload_kkp(&TerminalInput::Tab, 8);
        assert_eq!(p, TerminalInputPayload::Owned(b"\x1b[9u".to_vec()));
    }

    #[test]
    fn kkp_tab_disambiguate_only_sends_legacy() {
        // Flag 1 only: Tab still sends 0x09
        let p = to_payload_kkp(&TerminalInput::Tab, 1);
        assert_eq!(p, TerminalInputPayload::Single(0x09));
    }

    // ── to_payload_kkp: Escape ───────────────────────────────────────────────

    #[test]
    fn kkp_escape_disambiguate_sends_csi_27u() {
        let p = to_payload_kkp(&TerminalInput::Escape, 1);
        assert_eq!(p, TerminalInputPayload::Owned(b"\x1b[27u".to_vec()));
    }

    #[test]
    fn kkp_escape_report_all_sends_csi_27u() {
        let p = to_payload_kkp(&TerminalInput::Escape, 8);
        assert_eq!(p, TerminalInputPayload::Owned(b"\x1b[27u".to_vec()));
    }

    // ── to_payload_kkp: Arrow keys (retain legacy) ───────────────────────────

    #[test]
    fn kkp_arrow_right_normal_mode() {
        let p = to_payload_kkp(&TerminalInput::ArrowRight(KeyModifiers::NONE), 1);
        assert_eq!(p, TerminalInputPayload::Many(b"\x1b[C"));
    }

    #[test]
    fn kkp_arrow_right_application_mode() {
        let p = TerminalInput::ArrowRight(KeyModifiers::NONE).to_payload(
            Decckm::Application,
            KeypadMode::Numeric,
            0,
            ApplicationEscapeKey::Reset,
            Decbkm::BackarrowSendsDel,
            Lnm::LineFeed,
            1,
            &KeyEventMeta::PRESS,
        );
        assert_eq!(p, TerminalInputPayload::Many(b"\x1bOC"));
    }

    #[test]
    fn kkp_arrow_left_normal_mode() {
        let p = to_payload_kkp(&TerminalInput::ArrowLeft(KeyModifiers::NONE), 1);
        assert_eq!(p, TerminalInputPayload::Many(b"\x1b[D"));
    }

    #[test]
    fn kkp_arrow_up_normal_mode() {
        let p = to_payload_kkp(&TerminalInput::ArrowUp(KeyModifiers::NONE), 1);
        assert_eq!(p, TerminalInputPayload::Many(b"\x1b[A"));
    }

    #[test]
    fn kkp_arrow_down_normal_mode() {
        let p = to_payload_kkp(&TerminalInput::ArrowDown(KeyModifiers::NONE), 1);
        assert_eq!(p, TerminalInputPayload::Many(b"\x1b[B"));
    }

    // ── to_payload_kkp: Home/End ─────────────────────────────────────────────

    #[test]
    fn kkp_home_normal_mode() {
        let p = to_payload_kkp(&TerminalInput::Home(KeyModifiers::NONE), 1);
        assert_eq!(p, TerminalInputPayload::Many(b"\x1b[H"));
    }

    #[test]
    fn kkp_end_normal_mode() {
        let p = to_payload_kkp(&TerminalInput::End(KeyModifiers::NONE), 1);
        assert_eq!(p, TerminalInputPayload::Many(b"\x1b[F"));
    }

    // ── to_payload_kkp: Delete/Insert/PageUp/PageDown ────────────────────────

    #[test]
    fn kkp_delete_no_modifier() {
        let p = to_payload_kkp(&TerminalInput::Delete(KeyModifiers::NONE), 1);
        assert_eq!(p, TerminalInputPayload::Many(b"\x1b[3~"));
    }

    #[test]
    fn kkp_insert_no_modifier() {
        let p = to_payload_kkp(&TerminalInput::Insert(KeyModifiers::NONE), 1);
        assert_eq!(p, TerminalInputPayload::Many(b"\x1b[2~"));
    }

    #[test]
    fn kkp_pageup_no_modifier() {
        let p = to_payload_kkp(&TerminalInput::PageUp(KeyModifiers::NONE), 1);
        assert_eq!(p, TerminalInputPayload::Many(b"\x1b[5~"));
    }

    #[test]
    fn kkp_pagedown_no_modifier() {
        let p = to_payload_kkp(&TerminalInput::PageDown(KeyModifiers::NONE), 1);
        assert_eq!(p, TerminalInputPayload::Many(b"\x1b[6~"));
    }

    // ── to_payload_kkp: LostFocus/InFocus ───────────────────────────────────

    #[test]
    fn kkp_lost_focus() {
        let p = to_payload_kkp(&TerminalInput::LostFocus, 1);
        assert_eq!(p, TerminalInputPayload::Many(b"\x1b[O"));
    }

    #[test]
    fn kkp_in_focus() {
        let p = to_payload_kkp(&TerminalInput::InFocus, 1);
        assert_eq!(p, TerminalInputPayload::Many(b"\x1b[I"));
    }

    // ── to_payload_kkp: Function keys ────────────────────────────────────────

    #[test]
    fn kkp_function_keys_f1_f4_ss3_form() {
        assert_eq!(
            to_payload_kkp(&TerminalInput::FunctionKey(1, KeyModifiers::NONE), 1),
            TerminalInputPayload::Many(b"\x1bOP")
        );
        assert_eq!(
            to_payload_kkp(&TerminalInput::FunctionKey(2, KeyModifiers::NONE), 1),
            TerminalInputPayload::Many(b"\x1bOQ")
        );
        assert_eq!(
            to_payload_kkp(&TerminalInput::FunctionKey(3, KeyModifiers::NONE), 1),
            TerminalInputPayload::Many(b"\x1bOR")
        );
        assert_eq!(
            to_payload_kkp(&TerminalInput::FunctionKey(4, KeyModifiers::NONE), 1),
            TerminalInputPayload::Many(b"\x1bOS")
        );
    }

    #[test]
    fn kkp_function_keys_f5_f12_no_modifier() {
        assert_eq!(
            to_payload_kkp(&TerminalInput::FunctionKey(5, KeyModifiers::NONE), 1),
            TerminalInputPayload::Many(b"\x1b[15~")
        );
        assert_eq!(
            to_payload_kkp(&TerminalInput::FunctionKey(12, KeyModifiers::NONE), 1),
            TerminalInputPayload::Many(b"\x1b[24~")
        );
    }

    #[test]
    fn kkp_function_key_with_modifier() {
        let mods = KeyModifiers {
            shift: true,
            ctrl: false,
            alt: false,
        };
        let p = to_payload_kkp(&TerminalInput::FunctionKey(5, mods), 1);
        assert_eq!(p, TerminalInputPayload::Owned(b"\x1b[15;2~".to_vec()));
    }

    #[test]
    fn kkp_function_key_unknown_returns_empty() {
        let p = to_payload_kkp(&TerminalInput::FunctionKey(13, KeyModifiers::NONE), 1);
        assert_eq!(p, TerminalInputPayload::Many(b""));
    }

    // ── to_payload_kkp: KeyPad ───────────────────────────────────────────────

    #[test]
    fn kkp_keypad_numeric_mode() {
        let p = TerminalInput::KeyPad(b'5').to_payload(
            Decckm::Ansi,
            KeypadMode::Numeric,
            0,
            ApplicationEscapeKey::Reset,
            Decbkm::BackarrowSendsDel,
            Lnm::LineFeed,
            1,
            &KeyEventMeta::PRESS,
        );
        assert_eq!(p, TerminalInputPayload::Single(b'5'));
    }

    #[test]
    fn kkp_keypad_application_mode() {
        let p = TerminalInput::KeyPad(0).to_payload(
            Decckm::Ansi,
            KeypadMode::Application,
            0,
            ApplicationEscapeKey::Reset,
            Decbkm::BackarrowSendsDel,
            Lnm::LineFeed,
            1,
            &KeyEventMeta::PRESS,
        );
        assert_eq!(p, TerminalInputPayload::Many(b"\x1b[Op"));
    }

    // ── to_payload_kkp: ASCII plain (report_all flag) ────────────────────────

    #[test]
    fn kkp_report_all_plain_ascii_sends_csi_u() {
        // Flag 8: plain 'a' → CSI 97u
        let p = to_payload_kkp(&TerminalInput::Ascii(b'a'), 8);
        assert_eq!(p, TerminalInputPayload::Owned(b"\x1b[97u".to_vec()));
    }

    #[test]
    fn kkp_report_all_uppercase_ascii_sends_shifted_csi_u() {
        // Flag 8: 'A' → lowercase codepoint 97 with shift modifier 2
        let p = to_payload_kkp(&TerminalInput::Ascii(b'A'), 8);
        assert_eq!(p, TerminalInputPayload::Owned(b"\x1b[97;2u".to_vec()));
    }

    #[test]
    fn kkp_disambiguate_only_plain_ascii_sends_raw() {
        // Flag 1 only (no report_all): plain ASCII unchanged
        let p = to_payload_kkp(&TerminalInput::Ascii(b'x'), 1);
        assert_eq!(p, TerminalInputPayload::Single(b'x'));
    }

    // ── build_csi_u: flag 2 (report event type) ─────────────────────────────

    #[test]
    fn kkp_flag2_report_event_type_appended() {
        // Flags 1|2: Ctrl+c → CSI 99;5:1u (event type 1=Press is omitted)
        // Actually Press has no code so it is omitted; the modifier is just 5.
        let p = TerminalInput::Ctrl(b'c').to_payload(
            Decckm::Ansi,
            KeypadMode::Numeric,
            0,
            ApplicationEscapeKey::Reset,
            Decbkm::BackarrowSendsDel,
            Lnm::LineFeed,
            1 | 2, // DISAMBIGUATE + REPORT_EVENT_TYPE
            &KeyEventMeta::PRESS,
        );
        // Press event code is None so modifier field stays "5" without ":1"
        assert_eq!(p, TerminalInputPayload::Owned(b"\x1b[99;5u".to_vec()));
    }

    #[test]
    fn kkp_flag2_repeat_event_type_appended() {
        let meta = KeyEventMeta {
            event_type: KeyEventType::Repeat,
            associated_text: None,
        };
        let p = TerminalInput::Ctrl(b'c').to_payload(
            Decckm::Ansi,
            KeypadMode::Numeric,
            0,
            ApplicationEscapeKey::Reset,
            Decbkm::BackarrowSendsDel,
            Lnm::LineFeed,
            1 | 2,
            &meta,
        );
        // Repeat event code = 2: modifier field is "5:2"
        assert_eq!(p, TerminalInputPayload::Owned(b"\x1b[99;5:2u".to_vec()));
    }

    // ── build_csi_u: flag 16 (associated text) ───────────────────────────────

    #[test]
    fn kkp_flag16_associated_text_appended() {
        let meta = KeyEventMeta {
            event_type: KeyEventType::Press,
            associated_text: Some("a".to_string()),
        };
        let p = TerminalInput::Ctrl(b'c').to_payload(
            Decckm::Ansi,
            KeypadMode::Numeric,
            0,
            ApplicationEscapeKey::Reset,
            Decbkm::BackarrowSendsDel,
            Lnm::LineFeed,
            1 | 16, // DISAMBIGUATE + ASSOCIATED_TEXT
            &meta,
        );
        // Third param is "97" (codepoint of 'a')
        assert_eq!(p, TerminalInputPayload::Owned(b"\x1b[99;5;97u".to_vec()));
    }

    #[test]
    fn kkp_flag16_empty_associated_text_omitted() {
        let meta = KeyEventMeta {
            event_type: KeyEventType::Press,
            associated_text: Some(String::new()),
        };
        let p = TerminalInput::Ctrl(b'c').to_payload(
            Decckm::Ansi,
            KeypadMode::Numeric,
            0,
            ApplicationEscapeKey::Reset,
            Decbkm::BackarrowSendsDel,
            Lnm::LineFeed,
            1 | 16,
            &meta,
        );
        // Empty associated text is omitted
        assert_eq!(p, TerminalInputPayload::Owned(b"\x1b[99;5u".to_vec()));
    }

    // ── LineFeed ─────────────────────────────────────────────────────────────

    #[test]
    fn line_feed_sends_newline_byte() {
        let p = to_payload_defaults(&TerminalInput::LineFeed);
        assert_eq!(p, TerminalInputPayload::Single(b'\n'));
    }

    #[test]
    fn kkp_line_feed_sends_newline_byte() {
        let p = to_payload_kkp(&TerminalInput::LineFeed, 1);
        assert_eq!(p, TerminalInputPayload::Single(b'\n'));
    }

    // ── KeyEventMeta::PRESS constant ─────────────────────────────────────────

    #[test]
    fn key_event_meta_press_is_default() {
        let m = KeyEventMeta::PRESS;
        assert_eq!(m.event_type, KeyEventType::Press);
        assert!(m.associated_text.is_none());
    }

    // ── KeyModifiers::modifier_param ─────────────────────────────────────────

    #[test]
    fn modifier_param_all_combinations() {
        // No modifier
        assert_eq!(KeyModifiers::NONE.modifier_param(), None);
        // Shift only
        assert_eq!(
            KeyModifiers {
                shift: true,
                ctrl: false,
                alt: false
            }
            .modifier_param(),
            Some(2)
        );
        // Alt only
        assert_eq!(
            KeyModifiers {
                shift: false,
                ctrl: false,
                alt: true
            }
            .modifier_param(),
            Some(3)
        );
        // Shift+Alt
        assert_eq!(
            KeyModifiers {
                shift: true,
                ctrl: false,
                alt: true
            }
            .modifier_param(),
            Some(4)
        );
        // Ctrl only
        assert_eq!(
            KeyModifiers {
                shift: false,
                ctrl: true,
                alt: false
            }
            .modifier_param(),
            Some(5)
        );
        // Ctrl+Shift
        assert_eq!(
            KeyModifiers {
                shift: true,
                ctrl: true,
                alt: false
            }
            .modifier_param(),
            Some(6)
        );
        // Ctrl+Alt
        assert_eq!(
            KeyModifiers {
                shift: false,
                ctrl: true,
                alt: true
            }
            .modifier_param(),
            Some(7)
        );
        // Ctrl+Alt+Shift
        assert_eq!(
            KeyModifiers {
                shift: true,
                ctrl: true,
                alt: true
            }
            .modifier_param(),
            Some(8)
        );
    }

    // ── Coverage gap tests: legacy to_payload branches ───────────────────────

    const SHIFT: KeyModifiers = KeyModifiers {
        shift: true,
        ctrl: false,
        alt: false,
    };

    #[test]
    fn arrow_down_with_modifier_legacy() {
        let p = to_payload_defaults(&TerminalInput::ArrowDown(SHIFT));
        assert_eq!(p, TerminalInputPayload::Owned(b"\x1b[1;2B".to_vec()));
    }

    #[test]
    fn f2_with_modifier_legacy() {
        let p = to_payload_defaults(&TerminalInput::FunctionKey(2, SHIFT));
        assert_eq!(p, TerminalInputPayload::Owned(b"\x1b[1;2Q".to_vec()));
    }

    #[test]
    fn f3_with_modifier_legacy() {
        let p = to_payload_defaults(&TerminalInput::FunctionKey(3, SHIFT));
        assert_eq!(p, TerminalInputPayload::Owned(b"\x1b[1;2R".to_vec()));
    }

    #[test]
    fn f4_with_modifier_legacy() {
        let p = to_payload_defaults(&TerminalInput::FunctionKey(4, SHIFT));
        assert_eq!(p, TerminalInputPayload::Owned(b"\x1b[1;2S".to_vec()));
    }

    #[test]
    fn f6_with_modifier_legacy() {
        let p = to_payload_defaults(&TerminalInput::FunctionKey(6, SHIFT));
        assert_eq!(p, TerminalInputPayload::Owned(b"\x1b[17;2~".to_vec()));
    }

    #[test]
    fn f7_with_modifier_legacy() {
        let p = to_payload_defaults(&TerminalInput::FunctionKey(7, SHIFT));
        assert_eq!(p, TerminalInputPayload::Owned(b"\x1b[18;2~".to_vec()));
    }

    #[test]
    fn f8_with_modifier_legacy() {
        let p = to_payload_defaults(&TerminalInput::FunctionKey(8, SHIFT));
        assert_eq!(p, TerminalInputPayload::Owned(b"\x1b[19;2~".to_vec()));
    }

    #[test]
    fn f9_with_modifier_legacy() {
        let p = to_payload_defaults(&TerminalInput::FunctionKey(9, SHIFT));
        assert_eq!(p, TerminalInputPayload::Owned(b"\x1b[20;2~".to_vec()));
    }

    #[test]
    fn f10_with_modifier_legacy() {
        let p = to_payload_defaults(&TerminalInput::FunctionKey(10, SHIFT));
        assert_eq!(p, TerminalInputPayload::Owned(b"\x1b[21;2~".to_vec()));
    }

    #[test]
    fn f11_with_modifier_legacy() {
        let p = to_payload_defaults(&TerminalInput::FunctionKey(11, SHIFT));
        assert_eq!(p, TerminalInputPayload::Owned(b"\x1b[23;2~".to_vec()));
    }

    #[test]
    fn f12_with_modifier_legacy() {
        let p = to_payload_defaults(&TerminalInput::FunctionKey(12, SHIFT));
        assert_eq!(p, TerminalInputPayload::Owned(b"\x1b[24;2~".to_vec()));
    }

    // ── Coverage gap tests: KKP path (to_payload_kkp) ────────────────────────

    /// Helper: call `to_payload` with KKP flags and application cursor mode.
    fn to_payload_kkp_app(input: &TerminalInput, flags: u32) -> TerminalInputPayload {
        input.to_payload(
            Decckm::Application,
            KeypadMode::Application,
            0,
            ApplicationEscapeKey::Reset,
            Decbkm::BackarrowSendsDel,
            Lnm::LineFeed,
            flags,
            &KeyEventMeta::PRESS,
        )
    }

    #[test]
    fn kkp_ctrl_non_disambiguate_sends_legacy() {
        // Flags 2 alone: not disambiguate, not report_all → legacy C0
        let p = to_payload_kkp(&TerminalInput::Ctrl(b'c'), 2);
        // Legacy C0: Ctrl+C = 0x03
        assert_eq!(p, TerminalInputPayload::Single(0x03));
    }

    #[test]
    fn kkp_escape_non_disambiguate_sends_legacy() {
        // Flags 2 alone: not disambiguate, not report_all → legacy ESC byte
        let p = to_payload_kkp(&TerminalInput::Escape, 2);
        assert_eq!(p, TerminalInputPayload::Single(b'\x1b'));
    }

    #[test]
    fn kkp_arrow_right_no_modifier() {
        // Flag 1 (disambiguate) but arrow keys keep legacy encoding
        let p = to_payload_kkp(&TerminalInput::ArrowRight(KeyModifiers::NONE), 1);
        assert_eq!(p, TerminalInputPayload::Many(b"\x1b[C"));
    }

    #[test]
    fn kkp_arrow_right_with_modifier() {
        let p = to_payload_kkp(&TerminalInput::ArrowRight(SHIFT), 1);
        assert_eq!(p, TerminalInputPayload::Owned(b"\x1b[1;2C".to_vec()));
    }

    #[test]
    fn kkp_arrow_left_no_modifier() {
        let p = to_payload_kkp(&TerminalInput::ArrowLeft(KeyModifiers::NONE), 1);
        assert_eq!(p, TerminalInputPayload::Many(b"\x1b[D"));
    }

    #[test]
    fn kkp_arrow_left_application_mode() {
        let p = to_payload_kkp_app(&TerminalInput::ArrowLeft(KeyModifiers::NONE), 1);
        assert_eq!(p, TerminalInputPayload::Many(b"\x1bOD"));
    }

    #[test]
    fn kkp_arrow_up_no_modifier() {
        let p = to_payload_kkp(&TerminalInput::ArrowUp(KeyModifiers::NONE), 1);
        assert_eq!(p, TerminalInputPayload::Many(b"\x1b[A"));
    }

    #[test]
    fn kkp_arrow_up_application_mode() {
        let p = to_payload_kkp_app(&TerminalInput::ArrowUp(KeyModifiers::NONE), 1);
        assert_eq!(p, TerminalInputPayload::Many(b"\x1bOA"));
    }

    #[test]
    fn kkp_arrow_up_with_modifier() {
        let p = to_payload_kkp(&TerminalInput::ArrowUp(SHIFT), 1);
        assert_eq!(p, TerminalInputPayload::Owned(b"\x1b[1;2A".to_vec()));
    }

    #[test]
    fn kkp_arrow_down_no_modifier() {
        let p = to_payload_kkp(&TerminalInput::ArrowDown(KeyModifiers::NONE), 1);
        assert_eq!(p, TerminalInputPayload::Many(b"\x1b[B"));
    }

    #[test]
    fn kkp_arrow_down_application_mode() {
        let p = to_payload_kkp_app(&TerminalInput::ArrowDown(KeyModifiers::NONE), 1);
        assert_eq!(p, TerminalInputPayload::Many(b"\x1bOB"));
    }

    #[test]
    fn kkp_arrow_down_with_modifier() {
        let p = to_payload_kkp(&TerminalInput::ArrowDown(SHIFT), 1);
        assert_eq!(p, TerminalInputPayload::Owned(b"\x1b[1;2B".to_vec()));
    }

    #[test]
    fn kkp_home_no_modifier() {
        let p = to_payload_kkp(&TerminalInput::Home(KeyModifiers::NONE), 1);
        assert_eq!(p, TerminalInputPayload::Many(b"\x1b[H"));
    }

    #[test]
    fn kkp_home_application_mode() {
        let p = to_payload_kkp_app(&TerminalInput::Home(KeyModifiers::NONE), 1);
        assert_eq!(p, TerminalInputPayload::Many(b"\x1bOH"));
    }

    #[test]
    fn kkp_home_with_modifier() {
        let p = to_payload_kkp(&TerminalInput::Home(SHIFT), 1);
        assert_eq!(p, TerminalInputPayload::Owned(b"\x1b[1;2H".to_vec()));
    }

    #[test]
    fn kkp_end_no_modifier() {
        let p = to_payload_kkp(&TerminalInput::End(KeyModifiers::NONE), 1);
        assert_eq!(p, TerminalInputPayload::Many(b"\x1b[F"));
    }

    #[test]
    fn kkp_end_application_mode() {
        let p = to_payload_kkp_app(&TerminalInput::End(KeyModifiers::NONE), 1);
        assert_eq!(p, TerminalInputPayload::Many(b"\x1bOF"));
    }

    #[test]
    fn kkp_end_with_modifier() {
        let p = to_payload_kkp(&TerminalInput::End(SHIFT), 1);
        assert_eq!(p, TerminalInputPayload::Owned(b"\x1b[1;2F".to_vec()));
    }

    #[test]
    fn kkp_delete_with_modifier() {
        let p = to_payload_kkp(&TerminalInput::Delete(SHIFT), 1);
        assert_eq!(p, TerminalInputPayload::Owned(b"\x1b[3;2~".to_vec()));
    }

    #[test]
    fn kkp_insert_with_modifier() {
        let p = to_payload_kkp(&TerminalInput::Insert(SHIFT), 1);
        assert_eq!(p, TerminalInputPayload::Owned(b"\x1b[2;2~".to_vec()));
    }

    #[test]
    fn kkp_pageup_with_modifier() {
        let p = to_payload_kkp(&TerminalInput::PageUp(SHIFT), 1);
        assert_eq!(p, TerminalInputPayload::Owned(b"\x1b[5;2~".to_vec()));
    }

    #[test]
    fn kkp_pagedown_with_modifier() {
        let p = to_payload_kkp(&TerminalInput::PageDown(SHIFT), 1);
        assert_eq!(p, TerminalInputPayload::Owned(b"\x1b[6;2~".to_vec()));
    }

    // ── KKP: keypad in application mode ──────────────────────────────────────

    #[test]
    fn kkp_keypad_application_mode_digits() {
        let expected_suffixes = [b'p', b'q', b'r', b's', b't', b'u', b'v', b'w', b'x', b'y'];
        for (digit, suffix) in (0u8..=9).zip(expected_suffixes.iter()) {
            let p = to_payload_kkp_app(&TerminalInput::KeyPad(digit), 1);
            let expected = [0x1b, b'[', b'O', *suffix];
            assert_eq!(
                p,
                TerminalInputPayload::Many(
                    // The Many variant holds a &'static [u8]; match the exact expected
                    match digit {
                        0 => b"\x1b[Op",
                        1 => b"\x1b[Oq",
                        2 => b"\x1b[Or",
                        3 => b"\x1b[Os",
                        4 => b"\x1b[Ot",
                        5 => b"\x1b[Ou",
                        6 => b"\x1b[Ov",
                        7 => b"\x1b[Ow",
                        8 => b"\x1b[Ox",
                        9 => b"\x1b[Oy",
                        _ => unreachable!(),
                    }
                ),
                "keypad {digit} in app mode should produce ESC[O{}",
                expected[3] as char
            );
        }
    }

    #[test]
    fn kkp_keypad_application_mode_minus() {
        let p = to_payload_kkp_app(&TerminalInput::KeyPad(b'-'), 1);
        assert_eq!(p, TerminalInputPayload::Many(b"\x1b[Om"));
    }

    #[test]
    fn kkp_keypad_application_mode_comma() {
        let p = to_payload_kkp_app(&TerminalInput::KeyPad(b','), 1);
        assert_eq!(p, TerminalInputPayload::Many(b"\x1b[Ol"));
    }

    #[test]
    fn kkp_keypad_application_mode_dot() {
        let p = to_payload_kkp_app(&TerminalInput::KeyPad(b'.'), 1);
        assert_eq!(p, TerminalInputPayload::Many(b"\x1b[On"));
    }

    #[test]
    fn kkp_keypad_application_mode_enter() {
        let p = to_payload_kkp_app(&TerminalInput::KeyPad(b'\n'), 1);
        assert_eq!(p, TerminalInputPayload::Many(b"\x1b[OM"));
    }

    #[test]
    fn kkp_keypad_application_mode_unknown() {
        let p = to_payload_kkp_app(&TerminalInput::KeyPad(b'?'), 1);
        // Unknown keypad key in app mode should fallback to Single
        assert_eq!(p, TerminalInputPayload::Single(b'?'));
    }

    // ── KKP: function keys ──────────────────────────────────────────────────

    #[test]
    fn kkp_f1_no_modifier() {
        let p = to_payload_kkp(&TerminalInput::FunctionKey(1, KeyModifiers::NONE), 1);
        assert_eq!(p, TerminalInputPayload::Many(b"\x1bOP"));
    }

    #[test]
    fn kkp_f1_with_modifier() {
        let p = to_payload_kkp(&TerminalInput::FunctionKey(1, SHIFT), 1);
        assert_eq!(p, TerminalInputPayload::Owned(b"\x1b[1;2P".to_vec()));
    }

    #[test]
    fn kkp_f2_with_modifier() {
        let p = to_payload_kkp(&TerminalInput::FunctionKey(2, SHIFT), 1);
        assert_eq!(p, TerminalInputPayload::Owned(b"\x1b[1;2Q".to_vec()));
    }

    #[test]
    fn kkp_f3_with_modifier() {
        let p = to_payload_kkp(&TerminalInput::FunctionKey(3, SHIFT), 1);
        assert_eq!(p, TerminalInputPayload::Owned(b"\x1b[1;2R".to_vec()));
    }

    #[test]
    fn kkp_f4_with_modifier() {
        let p = to_payload_kkp(&TerminalInput::FunctionKey(4, SHIFT), 1);
        assert_eq!(p, TerminalInputPayload::Owned(b"\x1b[1;2S".to_vec()));
    }

    #[test]
    fn kkp_f5_no_modifier() {
        let p = to_payload_kkp(&TerminalInput::FunctionKey(5, KeyModifiers::NONE), 1);
        assert_eq!(p, TerminalInputPayload::Many(b"\x1b[15~"));
    }

    #[test]
    fn kkp_f5_with_modifier() {
        let p = to_payload_kkp(&TerminalInput::FunctionKey(5, SHIFT), 1);
        assert_eq!(p, TerminalInputPayload::Owned(b"\x1b[15;2~".to_vec()));
    }

    #[test]
    fn kkp_f6_with_modifier() {
        let p = to_payload_kkp(&TerminalInput::FunctionKey(6, SHIFT), 1);
        assert_eq!(p, TerminalInputPayload::Owned(b"\x1b[17;2~".to_vec()));
    }

    #[test]
    fn kkp_f7_with_modifier() {
        let p = to_payload_kkp(&TerminalInput::FunctionKey(7, SHIFT), 1);
        assert_eq!(p, TerminalInputPayload::Owned(b"\x1b[18;2~".to_vec()));
    }

    #[test]
    fn kkp_f8_with_modifier() {
        let p = to_payload_kkp(&TerminalInput::FunctionKey(8, SHIFT), 1);
        assert_eq!(p, TerminalInputPayload::Owned(b"\x1b[19;2~".to_vec()));
    }

    #[test]
    fn kkp_f9_with_modifier() {
        let p = to_payload_kkp(&TerminalInput::FunctionKey(9, SHIFT), 1);
        assert_eq!(p, TerminalInputPayload::Owned(b"\x1b[20;2~".to_vec()));
    }

    #[test]
    fn kkp_f10_with_modifier() {
        let p = to_payload_kkp(&TerminalInput::FunctionKey(10, SHIFT), 1);
        assert_eq!(p, TerminalInputPayload::Owned(b"\x1b[21;2~".to_vec()));
    }

    #[test]
    fn kkp_f11_with_modifier() {
        let p = to_payload_kkp(&TerminalInput::FunctionKey(11, SHIFT), 1);
        assert_eq!(p, TerminalInputPayload::Owned(b"\x1b[23;2~".to_vec()));
    }

    #[test]
    fn kkp_f12_with_modifier() {
        let p = to_payload_kkp(&TerminalInput::FunctionKey(12, SHIFT), 1);
        assert_eq!(p, TerminalInputPayload::Owned(b"\x1b[24;2~".to_vec()));
    }

    #[test]
    fn kkp_f6_no_modifier() {
        let p = to_payload_kkp(&TerminalInput::FunctionKey(6, KeyModifiers::NONE), 1);
        assert_eq!(p, TerminalInputPayload::Many(b"\x1b[17~"));
    }

    #[test]
    fn kkp_f7_no_modifier() {
        let p = to_payload_kkp(&TerminalInput::FunctionKey(7, KeyModifiers::NONE), 1);
        assert_eq!(p, TerminalInputPayload::Many(b"\x1b[18~"));
    }

    #[test]
    fn kkp_f8_no_modifier() {
        let p = to_payload_kkp(&TerminalInput::FunctionKey(8, KeyModifiers::NONE), 1);
        assert_eq!(p, TerminalInputPayload::Many(b"\x1b[19~"));
    }

    #[test]
    fn kkp_f9_no_modifier() {
        let p = to_payload_kkp(&TerminalInput::FunctionKey(9, KeyModifiers::NONE), 1);
        assert_eq!(p, TerminalInputPayload::Many(b"\x1b[20~"));
    }

    #[test]
    fn kkp_f10_no_modifier() {
        let p = to_payload_kkp(&TerminalInput::FunctionKey(10, KeyModifiers::NONE), 1);
        assert_eq!(p, TerminalInputPayload::Many(b"\x1b[21~"));
    }

    #[test]
    fn kkp_f11_no_modifier() {
        let p = to_payload_kkp(&TerminalInput::FunctionKey(11, KeyModifiers::NONE), 1);
        assert_eq!(p, TerminalInputPayload::Many(b"\x1b[23~"));
    }

    #[test]
    fn kkp_f12_no_modifier() {
        let p = to_payload_kkp(&TerminalInput::FunctionKey(12, KeyModifiers::NONE), 1);
        assert_eq!(p, TerminalInputPayload::Many(b"\x1b[24~"));
    }

    #[test]
    fn kkp_f_unknown() {
        let p = to_payload_kkp(&TerminalInput::FunctionKey(99, KeyModifiers::NONE), 1);
        assert_eq!(p, TerminalInputPayload::Many(b""));
    }

    // ── 70.A.1 regression: non-ASCII character encoding ─────────────────────

    /// `collect_text` must produce the raw UTF-8 byte sequence for non-ASCII
    /// characters, not a truncated single byte.  For example, 'é' (U+00E9) is
    /// encoded as the two-byte sequence [0xC3, 0xA9] in UTF-8.
    #[test]
    fn collect_text_non_ascii_utf8_encoding() {
        // 'é' = U+00E9, UTF-8: [0xC3, 0xA9]
        let inputs = collect_text(&"é".to_string());
        let bytes: Vec<u8> = inputs
            .iter()
            .filter_map(|i| {
                if let TerminalInput::Ascii(b) = i {
                    Some(*b)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(bytes, "é".as_bytes(), "é must encode as its UTF-8 bytes");
    }

    /// `collect_text` must handle multi-codepoint strings correctly.
    #[test]
    fn collect_text_mixed_ascii_and_non_ascii() {
        // "aé" = 'a' (0x61) + 'é' (0xC3 0xA9) = 3 bytes total
        let inputs = collect_text(&"aé".to_string());
        let bytes: Vec<u8> = inputs
            .iter()
            .filter_map(|i| {
                if let TerminalInput::Ascii(b) = i {
                    Some(*b)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(
            bytes,
            "aé".as_bytes(),
            "mixed ASCII+non-ASCII must encode as UTF-8"
        );
    }

    /// The `codepoint as u8` cast inside `build_csi_u` is only reached when
    /// `codepoint <= 127`, so it is lossless.  This test verifies that the
    /// `us_qwerty_shifted` helper is only called for ASCII codepoints.
    #[test]
    fn build_csi_u_report_alt_ascii_boundary() {
        let meta = KeyEventMeta::PRESS;
        // codepoint 127 (DEL) is the highest ASCII value — must not panic.
        let p = TerminalInput::build_csi_u(127, None, 4, &meta);
        // We just verify it produces some Owned payload without panic.
        assert!(matches!(p, TerminalInputPayload::Owned(_)));
    }
}
