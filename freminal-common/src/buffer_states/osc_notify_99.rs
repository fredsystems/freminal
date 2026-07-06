// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Parser for OSC 99 (kitty desktop notifications) metadata and payload.
//!
//! Reference: <https://sw.kovidgoyal.net/kitty/desktop-notifications/>
//!
//! The protocol uses OSC sequences of the form:
//! `ESC ] 99 ; <colon-separated key=value metadata> ; <payload> ST`
//!
//! This module provides [`parse_osc_99`] which takes the already-extracted
//! `<metadata>` and `<payload>` byte slices (the OSC framing and the split on
//! the second `;` are done by the caller in Task 99.2) and returns a fully
//! typed [`Osc99Command`].
//!
//! This is a **pure parser** — no dispatch, no state machine, no reverse-write,
//! no GUI. Those are later subtasks (99.2+).

use std::fmt;

/// Payload type of an OSC 99 notification (`p=` key).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Osc99PayloadType {
    /// `p=title` (default): notification title.
    Title,
    /// `p=body`: notification body.
    Body,
    /// `p=close`: close the notification with this id.
    Close,
    /// `p=icon`: icon image bytes (must be base64, `e=1`).
    Icon,
    /// `p=alive`: liveness poll.
    Alive,
    /// `p=buttons`: button labels (U+2028-separated).
    Buttons,
    /// `p=?`: capability query.
    Query,
}

/// Urgency level of an OSC 99 notification (`u=` key).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationUrgency {
    /// `u=0`.
    Low,
    /// `u=1`.
    Normal,
    /// `u=2`.
    Critical,
}

/// Display occasion of an OSC 99 notification (`o=` key).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationOccasion {
    /// `o=always` (default): honour unconditionally.
    Always,
    /// `o=unfocused`: only when the source window lacks focus.
    Unfocused,
    /// `o=invisible`: only when unfocused and not visible.
    Invisible,
}

/// Error produced while parsing an OSC 99 metadata + payload pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Osc99ParseError {
    /// A metadata `key=value` token was malformed.
    InvalidMetadata(String),
    /// A metadata value was not valid UTF-8 / not decodable.
    InvalidValue(String),
    /// An integer-valued key (`u`, `w`, `c`, `d`, `e`) could not be parsed.
    InvalidInteger(String),
    /// An `i=`/`g=` identifier contained a disallowed character.
    InvalidId(String),
    /// The payload was declared base64 (`e=1`) but failed to decode.
    InvalidBase64(String),
    /// The payload was not valid escape-safe UTF-8 (non-base64 payloads).
    InvalidPayloadUtf8(String),
    /// The metadata + payload region exceeded [`MAX_OSC99_SEQUENCE_BYTES`].
    /// Rejected before any allocation/decode to bound memory use on
    /// untrusted terminal input.
    SequenceTooLarge(usize),
}

impl fmt::Display for Osc99ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidMetadata(s) => write!(f, "invalid OSC 99 metadata: {s}"),
            Self::InvalidValue(s) => write!(f, "invalid OSC 99 value: {s}"),
            Self::InvalidInteger(s) => write!(f, "invalid OSC 99 integer: {s}"),
            Self::InvalidId(s) => write!(f, "invalid OSC 99 id: {s}"),
            Self::InvalidBase64(s) => write!(f, "invalid OSC 99 base64: {s}"),
            Self::InvalidPayloadUtf8(s) => write!(f, "invalid OSC 99 payload UTF-8: {s}"),
            Self::SequenceTooLarge(n) => {
                write!(f, "OSC 99 sequence too large: {n} bytes")
            }
        }
    }
}

/// Maximum accepted size (in bytes) of a single OSC 99 metadata + payload
/// region before decode.
///
/// Terminal escape-sequence input is untrusted, and `base64::decode`
/// reserves `len * 3 / 4` up front, so an unbounded sequence could trigger a
/// large allocation. A single chunk's payload is capped here; multi-chunk
/// reassembly is additionally bounded by the terminal handler (see
/// `notify_99.rs`). 1 MiB comfortably covers any realistic notification icon
/// while refusing pathological input.
pub const MAX_OSC99_SEQUENCE_BYTES: usize = 1_048_576;

/// Activation behaviour flags from the `a=` metadata key.
///
/// Groups the two action-on-activation flags so that [`Osc99Command`] does not
/// exceed the clippy `struct_excessive_bools` limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Osc99Actions {
    /// Whether `a=report` was set (activation events should be sent back to the app).
    pub report_activation: bool,
    /// Whether `a=focus` was set (focus the source window on activation). Default `true`.
    pub focus_on_activation: bool,
}

impl Default for Osc99Actions {
    fn default() -> Self {
        Self {
            report_activation: false,
            focus_on_activation: true,
        }
    }
}

/// A fully-parsed OSC 99 notification command (one escape sequence).
///
/// The typed output of [`parse_osc_99`]. `WindowManipulation::Notification99`
/// (its transport shell, Task 99.4) is populated from this. Chunk reassembly,
/// dispatch, and reverse-write are NOT done here — this is a pure parse of a
/// single OSC 99 sequence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Osc99Command {
    /// Notification id (`i=`), if present.
    pub id: Option<String>,
    /// Payload type (`p=`), default `Title`.
    pub payload_type: Osc99PayloadType,
    /// Done/finalize flag (`d=`), default `true`.
    pub done: bool,
    /// The decoded payload bytes (base64-decoded if `e=1`, else raw UTF-8 bytes).
    pub payload: Vec<u8>,
    /// Activation behaviour flags (`a=`).
    pub actions: Osc99Actions,
    /// Whether `c=1` (close report wanted).
    pub close_report: bool,
    /// App name (`f=`, base64-decoded), if present.
    pub app_name: Option<String>,
    /// Icon-data cache key (`g=`), if present.
    pub icon_cache_key: Option<String>,
    /// Icon names (`n=`, base64-decoded), in order.
    pub icon_names: Vec<String>,
    /// Occasion (`o=`), default `Always`.
    pub occasion: NotificationOccasion,
    /// Sound name (`s=`, base64-decoded), if present.
    pub sound: Option<String>,
    /// Type/category tags (`t=`, base64-decoded), in order.
    pub notification_type: Vec<String>,
    /// Urgency (`u=`), if present.
    pub urgency: Option<NotificationUrgency>,
    /// Auto-expire ms (`w=`), default `-1`.
    pub expire_ms: i64,
}

/// Decode a base64-encoded byte slice into a UTF-8 `String`.
///
/// Converts the slice to a `str` first (UTF-8 check), then base64-decodes,
/// then converts the resulting bytes to a `String` (UTF-8 check).
fn decode_base64_utf8(value: &[u8]) -> Result<String, Osc99ParseError> {
    let s = std::str::from_utf8(value)
        .map_err(|_| Osc99ParseError::InvalidValue(String::from_utf8_lossy(value).into_owned()))?;
    let bytes = crate::base64::decode(s)
        .map_err(|e| Osc99ParseError::InvalidValue(format!("base64 decode error: {e}")))?;
    String::from_utf8(bytes)
        .map_err(|_| Osc99ParseError::InvalidValue(format!("base64 result is not UTF-8: {s}")))
}

/// Parse a `u8` strict-integer (only `0` or `1` allowed) from a byte slice.
///
/// Used for the `c`, `d`, and `e` metadata keys which are strict `0`/`1` flags.
fn parse_strict_bit(value: &[u8]) -> Result<bool, Osc99ParseError> {
    let s = std::str::from_utf8(value).map_err(|_| {
        Osc99ParseError::InvalidInteger(String::from_utf8_lossy(value).into_owned())
    })?;
    match s {
        "0" => Ok(false),
        "1" => Ok(true),
        _ => Err(Osc99ParseError::InvalidInteger(s.to_owned())),
    }
}

/// Parse a `i64` from a byte slice representing an ASCII decimal number.
///
/// Used for the `w` (auto-expire ms) metadata key.
fn parse_i64(value: &[u8]) -> Result<i64, Osc99ParseError> {
    let s = std::str::from_utf8(value).map_err(|_| {
        Osc99ParseError::InvalidInteger(String::from_utf8_lossy(value).into_owned())
    })?;
    s.parse::<i64>()
        .map_err(|_| Osc99ParseError::InvalidInteger(s.to_owned()))
}

/// Check that an identifier (`i=` or `g=`) contains only allowed characters.
///
/// The allowed set is `[a-zA-Z0-9_\-+.]` per the OSC 99 spec injection-guard rule.
fn validate_id(value: &[u8]) -> Result<String, Osc99ParseError> {
    let s = std::str::from_utf8(value)
        .map_err(|_| Osc99ParseError::InvalidId(String::from_utf8_lossy(value).into_owned()))?;
    for ch in s.chars() {
        if !matches!(ch, 'a'..='z' | 'A'..='Z' | '0'..='9' | '_' | '-' | '+' | '.') {
            return Err(Osc99ParseError::InvalidId(s.to_owned()));
        }
    }
    Ok(s.to_owned())
}

/// Mutable parse state threaded through [`apply_metadata_pair`].
///
/// All fields carry the correct protocol defaults so that absent keys produce
/// the right output without special-casing after the loop.
struct ParseState {
    id: Option<String>,
    payload_type: Osc99PayloadType,
    done: bool,
    base64_payload: bool,
    actions: Osc99Actions,
    close_report: bool,
    app_name: Option<String>,
    icon_cache_key: Option<String>,
    icon_names: Vec<String>,
    occasion: NotificationOccasion,
    sound: Option<String>,
    notification_type: Vec<String>,
    urgency: Option<NotificationUrgency>,
    expire_ms: i64,
}

impl Default for ParseState {
    fn default() -> Self {
        Self {
            id: None,
            payload_type: Osc99PayloadType::Title,
            done: true,
            base64_payload: false,
            actions: Osc99Actions::default(),
            close_report: false,
            app_name: None,
            icon_cache_key: None,
            icon_names: Vec::new(),
            occasion: NotificationOccasion::Always,
            sound: None,
            notification_type: Vec::new(),
            urgency: None,
            expire_ms: -1,
        }
    }
}

/// Apply a single validated `key=value` metadata pair to `state`.
///
/// The key byte is guaranteed by the caller to be exactly one byte; `value` is
/// everything after the first `=` in the colon-separated token.
fn apply_metadata_pair(
    state: &mut ParseState,
    key: u8,
    value: &[u8],
) -> Result<(), Osc99ParseError> {
    match key {
        // `a` — actions: comma-list of `report`/`focus`, each optionally `-`-prefixed.
        b'a' => {
            // Encountering `a=` resets both flags; we then apply only what is stated.
            // (The default when `a` is ABSENT is report=false, focus=true — handled by
            // `Osc99Actions::default()`; here we start fresh from false/false.)
            state.actions.report_activation = false;
            state.actions.focus_on_activation = false;
            for action_token in value.split(|&b| b == b',') {
                let (negated, name_bytes) = if action_token.first() == Some(&b'-') {
                    (true, &action_token[1..])
                } else {
                    (false, action_token)
                };
                match name_bytes {
                    b"report" => state.actions.report_activation = !negated,
                    b"focus" => state.actions.focus_on_activation = !negated,
                    // Unknown action words — ignore for forward compatibility.
                    _ => {}
                }
            }
        }

        // `c` — close report: strict 0/1.
        b'c' => state.close_report = parse_strict_bit(value)?,

        // `d` — done flag: strict 0/1.
        b'd' => state.done = parse_strict_bit(value)?,

        // `e` — base64 payload flag: strict 0/1.
        b'e' => state.base64_payload = parse_strict_bit(value)?,

        // `f` — app name: base64 UTF-8.
        b'f' => state.app_name = Some(decode_base64_utf8(value)?),

        // `g` — icon cache key: plain identifier, sanitized.
        b'g' => state.icon_cache_key = Some(validate_id(value)?),

        // `i` — notification id: plain identifier, sanitized.
        b'i' => state.id = Some(validate_id(value)?),

        // `n` — icon name: base64 UTF-8, may repeat (accumulate in order).
        b'n' => state.icon_names.push(decode_base64_utf8(value)?),

        // `o` — occasion: `always`/`unfocused`/`invisible`; unrecognized → ignore.
        b'o' => {
            state.occasion = match value {
                b"always" => NotificationOccasion::Always,
                b"unfocused" => NotificationOccasion::Unfocused,
                b"invisible" => NotificationOccasion::Invisible,
                // Unknown occasion — forward-compat: keep current value.
                _ => state.occasion,
            };
        }

        // `p` — payload type; unrecognized → ignore (forward-compat).
        b'p' => {
            state.payload_type = match value {
                b"title" => Osc99PayloadType::Title,
                b"body" => Osc99PayloadType::Body,
                b"close" => Osc99PayloadType::Close,
                b"icon" => Osc99PayloadType::Icon,
                b"alive" => Osc99PayloadType::Alive,
                b"buttons" => Osc99PayloadType::Buttons,
                b"?" => Osc99PayloadType::Query,
                // Unknown payload type — forward-compat: keep current value.
                _ => state.payload_type,
            };
        }

        // `s` — sound name: base64.
        b's' => state.sound = Some(decode_base64_utf8(value)?),

        // `t` — type/category: base64 UTF-8, may repeat (accumulate in order).
        b't' => state.notification_type.push(decode_base64_utf8(value)?),

        // `u` — urgency: plain integer 0/1/2.
        b'u' => {
            let s = std::str::from_utf8(value).map_err(|_| {
                Osc99ParseError::InvalidInteger(String::from_utf8_lossy(value).into_owned())
            })?;
            state.urgency = Some(match s {
                "0" => NotificationUrgency::Low,
                "1" => NotificationUrgency::Normal,
                "2" => NotificationUrgency::Critical,
                _ => return Err(Osc99ParseError::InvalidInteger(s.to_owned())),
            });
        }

        // `w` — auto-expire ms: i64, must be >= -1.
        b'w' => {
            let ms = parse_i64(value)?;
            if ms < -1 {
                return Err(Osc99ParseError::InvalidInteger(ms.to_string()));
            }
            state.expire_ms = ms;
        }

        // Unknown keys — ignore silently for forward compatibility.
        _ => {}
    }

    Ok(())
}

/// Decode the payload bytes based on the `e=` flag in `state`.
fn decode_payload(state: &ParseState, payload: &[u8]) -> Result<Vec<u8>, Osc99ParseError> {
    if state.base64_payload {
        // `e=1`: base64-decode the raw payload bytes.
        let s = std::str::from_utf8(payload).map_err(|_| {
            Osc99ParseError::InvalidBase64(String::from_utf8_lossy(payload).into_owned())
        })?;
        crate::base64::decode(s)
            .map_err(|e| Osc99ParseError::InvalidBase64(format!("base64 decode error: {e}")))
    } else {
        // `e=0` / absent: payload is escape-safe UTF-8 — validate it, store raw bytes.
        // Note: we do not reject C0/C1 here; that is a higher-level concern.
        std::str::from_utf8(payload).map_err(|_| {
            Osc99ParseError::InvalidPayloadUtf8(String::from_utf8_lossy(payload).into_owned())
        })?;
        Ok(payload.to_vec())
    }
}

/// Parse an OSC 99 metadata + payload byte pair into a typed [`Osc99Command`].
///
/// `metadata` is the colon-separated `key=value` region (between the two
/// semicolons of `ESC ] 99 ; <metadata> ; <payload> ST`); `payload` is the raw
/// payload region. The caller (Task 99.2) is responsible for extracting these
/// from `raw_params` (splitting on the second `;` only). Pure parser: no state,
/// no dispatch.
///
/// # Errors
/// Returns [`Osc99ParseError`] if metadata is malformed, an id is unsafe, an
/// integer/urgency is invalid, or the payload fails base64/UTF-8 decoding.
pub fn parse_osc_99(metadata: &[u8], payload: &[u8]) -> Result<Osc99Command, Osc99ParseError> {
    // Reject oversized input before any allocation/decode. `base64::decode`
    // reserves proportional to the input length, so an unbounded payload on
    // untrusted terminal input could force a large allocation.
    let total = metadata.len().saturating_add(payload.len());
    if total > MAX_OSC99_SEQUENCE_BYTES {
        return Err(Osc99ParseError::SequenceTooLarge(total));
    }

    let mut state = ParseState::default();

    // Parse the colon-separated key=value pairs in the metadata region.
    for token in metadata.split(|&b| b == b':') {
        // Skip empty tokens (leading/trailing/doubled colons).
        if token.is_empty() {
            continue;
        }

        // Find the first '=' — everything before is the key, everything after is
        // the value.  A token with no '=' is malformed.
        let eq_pos = token.iter().position(|&b| b == b'=').ok_or_else(|| {
            Osc99ParseError::InvalidMetadata(String::from_utf8_lossy(token).into_owned())
        })?;

        // Key must be exactly one ASCII letter (eq_pos == 1 means one byte before '=').
        if eq_pos != 1 {
            return Err(Osc99ParseError::InvalidMetadata(
                String::from_utf8_lossy(token).into_owned(),
            ));
        }

        let key = token[0];
        let value = &token[eq_pos + 1..];

        apply_metadata_pair(&mut state, key, value)?;
    }

    let decoded_payload = decode_payload(&state, payload)?;

    Ok(Osc99Command {
        id: state.id,
        payload_type: state.payload_type,
        done: state.done,
        payload: decoded_payload,
        actions: state.actions,
        close_report: state.close_report,
        app_name: state.app_name,
        icon_cache_key: state.icon_cache_key,
        icon_names: state.icon_names,
        occasion: state.occasion,
        sound: state.sound,
        notification_type: state.notification_type,
        urgency: state.urgency,
        expire_ms: state.expire_ms,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    /// Build a metadata byte slice from a plain string slice.
    fn meta(s: &str) -> Vec<u8> {
        s.as_bytes().to_vec()
    }

    /// Build a payload byte slice from a plain string slice.
    fn pay(s: &str) -> Vec<u8> {
        s.as_bytes().to_vec()
    }

    // ------------------------------------------------------------------
    // Minimal / default tests
    // ------------------------------------------------------------------

    #[test]
    fn minimal_title_empty_metadata_plain_payload() {
        // ESC ] 99 ; ; Hello — minimal, no metadata at all.
        let cmd = parse_osc_99(&meta(""), &pay("Hello")).unwrap();
        assert_eq!(cmd.payload_type, Osc99PayloadType::Title);
        assert_eq!(cmd.payload, b"Hello");
        assert!(cmd.done, "d defaults to true");
        assert_eq!(cmd.occasion, NotificationOccasion::Always);
        assert!(cmd.actions.focus_on_activation, "focus defaults true");
        assert!(!cmd.actions.report_activation, "report defaults false");
        assert_eq!(cmd.expire_ms, -1);
        assert!(cmd.id.is_none());
        assert!(!cmd.close_report);
        assert!(cmd.app_name.is_none());
        assert!(cmd.icon_cache_key.is_none());
        assert!(cmd.icon_names.is_empty());
        assert!(cmd.sound.is_none());
        assert!(cmd.notification_type.is_empty());
        assert!(cmd.urgency.is_none());
    }

    // ------------------------------------------------------------------
    // `p=` payload type
    // ------------------------------------------------------------------

    #[test]
    fn p_equals_title() {
        let cmd = parse_osc_99(&meta("p=title"), &pay("")).unwrap();
        assert_eq!(cmd.payload_type, Osc99PayloadType::Title);
    }

    #[test]
    fn p_equals_body() {
        let cmd = parse_osc_99(&meta("p=body"), &pay("")).unwrap();
        assert_eq!(cmd.payload_type, Osc99PayloadType::Body);
    }

    #[test]
    fn p_equals_close() {
        let cmd = parse_osc_99(&meta("p=close"), &pay("")).unwrap();
        assert_eq!(cmd.payload_type, Osc99PayloadType::Close);
    }

    #[test]
    fn p_equals_icon() {
        let cmd = parse_osc_99(&meta("p=icon"), &pay("")).unwrap();
        assert_eq!(cmd.payload_type, Osc99PayloadType::Icon);
    }

    #[test]
    fn p_equals_alive() {
        let cmd = parse_osc_99(&meta("p=alive"), &pay("")).unwrap();
        assert_eq!(cmd.payload_type, Osc99PayloadType::Alive);
    }

    #[test]
    fn p_equals_buttons() {
        let cmd = parse_osc_99(&meta("p=buttons"), &pay("")).unwrap();
        assert_eq!(cmd.payload_type, Osc99PayloadType::Buttons);
    }

    #[test]
    fn p_equals_query() {
        let cmd = parse_osc_99(&meta("p=?"), &pay("")).unwrap();
        assert_eq!(cmd.payload_type, Osc99PayloadType::Query);
    }

    #[test]
    fn p_equals_unknown_ignored_stays_title() {
        // Unknown `p=` value must be ignored (forward compat), default stays Title.
        let cmd = parse_osc_99(&meta("p=future_type"), &pay("")).unwrap();
        assert_eq!(cmd.payload_type, Osc99PayloadType::Title);
    }

    // ------------------------------------------------------------------
    // `i=` identifier
    // ------------------------------------------------------------------

    #[test]
    fn i_equals_valid() {
        let cmd = parse_osc_99(&meta("i=abc-123_def.ghi+"), &pay("")).unwrap();
        assert_eq!(cmd.id, Some("abc-123_def.ghi+".to_owned()));
    }

    #[test]
    fn i_equals_illegal_char_returns_invalid_id() {
        let err = parse_osc_99(&meta("i=abc!def"), &pay("")).unwrap_err();
        assert_eq!(err, Osc99ParseError::InvalidId("abc!def".to_owned()));
    }

    #[test]
    fn i_equals_space_returns_invalid_id() {
        let err = parse_osc_99(&meta("i=abc def"), &pay("")).unwrap_err();
        assert_eq!(err, Osc99ParseError::InvalidId("abc def".to_owned()));
    }

    // ------------------------------------------------------------------
    // `g=` cache key
    // ------------------------------------------------------------------

    #[test]
    fn g_equals_valid() {
        let cmd = parse_osc_99(&meta("g=cache-key.01"), &pay("")).unwrap();
        assert_eq!(cmd.icon_cache_key, Some("cache-key.01".to_owned()));
    }

    #[test]
    fn g_equals_illegal_char_returns_invalid_id() {
        let err = parse_osc_99(&meta("g=bad/key"), &pay("")).unwrap_err();
        assert!(matches!(err, Osc99ParseError::InvalidId(_)));
    }

    // ------------------------------------------------------------------
    // `d=` done flag
    // ------------------------------------------------------------------

    #[test]
    fn d_equals_zero_done_false() {
        let cmd = parse_osc_99(&meta("d=0"), &pay("")).unwrap();
        assert!(!cmd.done);
    }

    #[test]
    fn d_equals_one_done_true() {
        let cmd = parse_osc_99(&meta("d=1"), &pay("")).unwrap();
        assert!(cmd.done);
    }

    #[test]
    fn d_equals_two_invalid_integer() {
        let err = parse_osc_99(&meta("d=2"), &pay("")).unwrap_err();
        assert_eq!(err, Osc99ParseError::InvalidInteger("2".to_owned()));
    }

    // ------------------------------------------------------------------
    // `e=1` base64 payload
    // ------------------------------------------------------------------

    #[test]
    fn e_equals_one_base64_payload_decoded() {
        // "Hello" in base64 is "SGVsbG8="
        let cmd = parse_osc_99(&meta("e=1"), &pay("SGVsbG8=")).unwrap();
        assert_eq!(cmd.payload, b"Hello");
    }

    #[test]
    fn e_equals_one_invalid_base64_returns_error() {
        let err = parse_osc_99(&meta("e=1"), &pay("not-valid-base64!!!")).unwrap_err();
        assert!(matches!(err, Osc99ParseError::InvalidBase64(_)));
    }

    #[test]
    fn e_equals_zero_plain_utf8_payload() {
        let cmd = parse_osc_99(&meta("e=0"), &pay("plain text")).unwrap();
        assert_eq!(cmd.payload, b"plain text");
    }

    // ------------------------------------------------------------------
    // Base64 metadata keys: `f=`, `n=`, `s=`, `t=`
    // ------------------------------------------------------------------

    #[test]
    fn f_equals_app_name_decoded() {
        // "MyApp" in base64 is "TXlBcHA="
        let cmd = parse_osc_99(&meta("f=TXlBcHA="), &pay("")).unwrap();
        assert_eq!(cmd.app_name, Some("MyApp".to_owned()));
    }

    #[test]
    fn n_equals_icon_name_single() {
        // "error" in base64 is "ZXJyb3I="
        let cmd = parse_osc_99(&meta("n=ZXJyb3I="), &pay("")).unwrap();
        assert_eq!(cmd.icon_names, vec!["error".to_owned()]);
    }

    #[test]
    fn n_equals_icon_names_multiple_in_order() {
        // "error" = "ZXJyb3I=", "warn" = "d2Fybg=="
        let cmd = parse_osc_99(&meta("n=ZXJyb3I=:n=d2Fybg=="), &pay("")).unwrap();
        assert_eq!(cmd.icon_names, vec!["error".to_owned(), "warn".to_owned()]);
    }

    #[test]
    fn s_equals_sound_decoded() {
        // "system" in base64 is "c3lzdGVt"
        let cmd = parse_osc_99(&meta("s=c3lzdGVt"), &pay("")).unwrap();
        assert_eq!(cmd.sound, Some("system".to_owned()));
    }

    #[test]
    fn t_equals_single_type_decoded() {
        // "alert" in base64 is "YWxlcnQ="
        let cmd = parse_osc_99(&meta("t=YWxlcnQ="), &pay("")).unwrap();
        assert_eq!(cmd.notification_type, vec!["alert".to_owned()]);
    }

    #[test]
    fn t_equals_multiple_types_in_order() {
        // "alert" = "YWxlcnQ=", "info" = "aW5mbw=="
        let cmd = parse_osc_99(&meta("t=YWxlcnQ=:t=aW5mbw=="), &pay("")).unwrap();
        assert_eq!(
            cmd.notification_type,
            vec!["alert".to_owned(), "info".to_owned()]
        );
    }

    // ------------------------------------------------------------------
    // `u=` urgency
    // ------------------------------------------------------------------

    #[test]
    fn u_equals_zero_low() {
        let cmd = parse_osc_99(&meta("u=0"), &pay("")).unwrap();
        assert_eq!(cmd.urgency, Some(NotificationUrgency::Low));
    }

    #[test]
    fn u_equals_one_normal() {
        let cmd = parse_osc_99(&meta("u=1"), &pay("")).unwrap();
        assert_eq!(cmd.urgency, Some(NotificationUrgency::Normal));
    }

    #[test]
    fn u_equals_two_critical() {
        let cmd = parse_osc_99(&meta("u=2"), &pay("")).unwrap();
        assert_eq!(cmd.urgency, Some(NotificationUrgency::Critical));
    }

    #[test]
    fn u_equals_three_invalid() {
        let err = parse_osc_99(&meta("u=3"), &pay("")).unwrap_err();
        assert_eq!(err, Osc99ParseError::InvalidInteger("3".to_owned()));
    }

    // ------------------------------------------------------------------
    // `o=` occasion
    // ------------------------------------------------------------------

    #[test]
    fn o_equals_always() {
        let cmd = parse_osc_99(&meta("o=always"), &pay("")).unwrap();
        assert_eq!(cmd.occasion, NotificationOccasion::Always);
    }

    #[test]
    fn o_equals_unfocused() {
        let cmd = parse_osc_99(&meta("o=unfocused"), &pay("")).unwrap();
        assert_eq!(cmd.occasion, NotificationOccasion::Unfocused);
    }

    #[test]
    fn o_equals_invisible() {
        let cmd = parse_osc_99(&meta("o=invisible"), &pay("")).unwrap();
        assert_eq!(cmd.occasion, NotificationOccasion::Invisible);
    }

    #[test]
    fn o_equals_bogus_ignored_stays_always() {
        let cmd = parse_osc_99(&meta("o=bogus"), &pay("")).unwrap();
        assert_eq!(cmd.occasion, NotificationOccasion::Always);
    }

    // ------------------------------------------------------------------
    // `a=` action flags
    // ------------------------------------------------------------------

    #[test]
    fn a_absent_report_false_focus_true() {
        let cmd = parse_osc_99(&meta(""), &pay("")).unwrap();
        assert!(!cmd.actions.report_activation);
        assert!(cmd.actions.focus_on_activation);
    }

    #[test]
    fn a_equals_report_report_true_focus_false() {
        // Explicit `a=report` sets report=true; focus is not mentioned → false.
        let cmd = parse_osc_99(&meta("a=report"), &pay("")).unwrap();
        assert!(cmd.actions.report_activation);
        assert!(!cmd.actions.focus_on_activation);
    }

    #[test]
    fn a_equals_report_minus_focus() {
        let cmd = parse_osc_99(&meta("a=report,-focus"), &pay("")).unwrap();
        assert!(cmd.actions.report_activation);
        assert!(!cmd.actions.focus_on_activation);
    }

    #[test]
    fn a_equals_minus_report() {
        let cmd = parse_osc_99(&meta("a=-report"), &pay("")).unwrap();
        assert!(!cmd.actions.report_activation);
        assert!(!cmd.actions.focus_on_activation);
    }

    #[test]
    fn a_equals_focus_only() {
        let cmd = parse_osc_99(&meta("a=focus"), &pay("")).unwrap();
        assert!(!cmd.actions.report_activation);
        assert!(cmd.actions.focus_on_activation);
    }

    #[test]
    fn a_equals_report_comma_focus() {
        let cmd = parse_osc_99(&meta("a=report,focus"), &pay("")).unwrap();
        assert!(cmd.actions.report_activation);
        assert!(cmd.actions.focus_on_activation);
    }

    // ------------------------------------------------------------------
    // `c=` close report
    // ------------------------------------------------------------------

    #[test]
    fn c_equals_zero_close_report_false() {
        let cmd = parse_osc_99(&meta("c=0"), &pay("")).unwrap();
        assert!(!cmd.close_report);
    }

    #[test]
    fn c_equals_one_close_report_true() {
        let cmd = parse_osc_99(&meta("c=1"), &pay("")).unwrap();
        assert!(cmd.close_report);
    }

    // ------------------------------------------------------------------
    // `w=` expire ms
    // ------------------------------------------------------------------

    #[test]
    fn w_equals_5000() {
        let cmd = parse_osc_99(&meta("w=5000"), &pay("")).unwrap();
        assert_eq!(cmd.expire_ms, 5000);
    }

    #[test]
    fn w_equals_minus_one() {
        let cmd = parse_osc_99(&meta("w=-1"), &pay("")).unwrap();
        assert_eq!(cmd.expire_ms, -1);
    }

    #[test]
    fn w_equals_zero() {
        let cmd = parse_osc_99(&meta("w=0"), &pay("")).unwrap();
        assert_eq!(cmd.expire_ms, 0);
    }

    #[test]
    fn w_equals_minus_two_invalid() {
        let err = parse_osc_99(&meta("w=-2"), &pay("")).unwrap_err();
        assert_eq!(err, Osc99ParseError::InvalidInteger("-2".to_owned()));
    }

    // ------------------------------------------------------------------
    // `p=?` capability query
    // ------------------------------------------------------------------

    #[test]
    fn p_equals_query_form() {
        let cmd = parse_osc_99(&meta("i=myid:p=?"), &pay("")).unwrap();
        assert_eq!(cmd.payload_type, Osc99PayloadType::Query);
        assert_eq!(cmd.id, Some("myid".to_owned()));
    }

    // ------------------------------------------------------------------
    // Malformed metadata
    // ------------------------------------------------------------------

    #[test]
    fn malformed_token_no_equals() {
        // A token with no `=` that is non-empty is malformed → InvalidMetadata.
        let err = parse_osc_99(&meta("foo"), &pay("")).unwrap_err();
        assert!(matches!(err, Osc99ParseError::InvalidMetadata(_)));
    }

    #[test]
    fn malformed_multi_char_key() {
        // key part must be exactly 1 byte; "foo=bar" has a 3-char key.
        let err = parse_osc_99(&meta("foo=bar"), &pay("")).unwrap_err();
        assert!(matches!(err, Osc99ParseError::InvalidMetadata(_)));
    }

    #[test]
    fn empty_tokens_from_double_colon_are_skipped() {
        // `::` produces empty tokens which must be silently skipped.
        let cmd = parse_osc_99(&meta("::d=0::"), &pay("")).unwrap();
        assert!(!cmd.done);
    }

    #[test]
    fn leading_colon_skipped() {
        let cmd = parse_osc_99(&meta(":d=1"), &pay("")).unwrap();
        assert!(cmd.done);
    }

    #[test]
    fn trailing_colon_skipped() {
        let cmd = parse_osc_99(&meta("d=1:"), &pay("")).unwrap();
        assert!(cmd.done);
    }

    // ------------------------------------------------------------------
    // Realistic combined example
    // ------------------------------------------------------------------

    #[test]
    fn combined_example_with_base64_body() {
        // id, body, close-report, urgency, base64 payload
        // "Build complete" in base64 = "QnVpbGQgY29tcGxldGU="
        let cmd = parse_osc_99(
            &meta("i=notif-001:p=body:c=1:u=1:e=1"),
            &pay("QnVpbGQgY29tcGxldGU="),
        )
        .unwrap();
        assert_eq!(cmd.id, Some("notif-001".to_owned()));
        assert_eq!(cmd.payload_type, Osc99PayloadType::Body);
        assert!(cmd.close_report);
        assert_eq!(cmd.urgency, Some(NotificationUrgency::Normal));
        assert_eq!(cmd.payload, b"Build complete");
        assert!(cmd.done);
    }

    #[test]
    fn combined_example_multiple_repeating_keys_and_occasion() {
        // "MyApp" = "TXlBcHA=", "error" = "ZXJyb3I=", "warn" = "d2Fybg==",
        // "alert" = "YWxlcnQ=", "info" = "aW5mbw=="
        let cmd = parse_osc_99(
            &meta("i=abc123:f=TXlBcHA=:n=ZXJyb3I=:n=d2Fybg==:t=YWxlcnQ=:t=aW5mbw==:o=unfocused:u=2:w=3000"),
            &pay("Test notification"),
        )
        .unwrap();
        assert_eq!(cmd.id, Some("abc123".to_owned()));
        assert_eq!(cmd.app_name, Some("MyApp".to_owned()));
        assert_eq!(cmd.icon_names, vec!["error".to_owned(), "warn".to_owned()]);
        assert_eq!(
            cmd.notification_type,
            vec!["alert".to_owned(), "info".to_owned()]
        );
        assert_eq!(cmd.occasion, NotificationOccasion::Unfocused);
        assert_eq!(cmd.urgency, Some(NotificationUrgency::Critical));
        assert_eq!(cmd.expire_ms, 3000);
        assert_eq!(cmd.payload, b"Test notification");
    }

    // ------------------------------------------------------------------
    // Display impl — all variants produce non-empty strings
    // ------------------------------------------------------------------

    #[test]
    fn display_all_error_variants() {
        let errors = [
            Osc99ParseError::InvalidMetadata("tok".into()),
            Osc99ParseError::InvalidValue("val".into()),
            Osc99ParseError::InvalidInteger("99".into()),
            Osc99ParseError::InvalidId("bad!id".into()),
            Osc99ParseError::InvalidBase64("!!!".into()),
            Osc99ParseError::InvalidPayloadUtf8("bad".into()),
            Osc99ParseError::SequenceTooLarge(2_000_000),
        ];
        for e in &errors {
            let s = format!("{e}");
            assert!(!s.is_empty(), "Display was empty for {e:?}");
        }
    }

    #[test]
    fn parse_osc_99_rejects_oversized_sequence_before_decode() {
        // A payload larger than the cap must be rejected up front, without
        // attempting a (potentially huge) base64 allocation.
        let payload = vec![b'A'; MAX_OSC99_SEQUENCE_BYTES + 1];
        let err = parse_osc_99(b"p=title:e=1", &payload).unwrap_err();
        match err {
            Osc99ParseError::SequenceTooLarge(n) => {
                assert!(n > MAX_OSC99_SEQUENCE_BYTES);
            }
            other => panic!("expected SequenceTooLarge, got {other:?}"),
        }
    }

    #[test]
    fn parse_osc_99_accepts_sequence_at_the_cap() {
        // metadata + payload exactly at the cap is accepted (boundary).
        let metadata = b"p=title";
        let payload = vec![b'x'; MAX_OSC99_SEQUENCE_BYTES - metadata.len()];
        // Not base64 (no e=1), so it is treated as raw UTF-8 title bytes.
        assert!(parse_osc_99(metadata, &payload).is_ok());
    }
}
