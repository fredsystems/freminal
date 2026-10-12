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
//! the second `;` are done by the caller) and returns a fully typed
//! [`Osc99Command`].
//!
//! This is a **pure parser** — no dispatch, no state machine, no reverse-write,
//! no GUI.
//!
//! # Leniency
//!
//! The parser follows the spec's rule that a receiver may ignore what it does
//! not understand.  An invalid value for a known key ignores *that key only*;
//! a malformed metadata token (no `=`, or a key that is not exactly one byte)
//! is skipped; unknown keys are skipped.  The only errors are the two size
//! guards ([`Osc99ParseError`]), and even a rejected sequence reports the
//! `i=` and `d=` it carried so the terminal can abandon the transfer it
//! belonged to.  Every metadata field that has a protocol
//! default is *unset-aware* (`Option`, or an empty list) so that chunk
//! reassembly can tell "the sender said nothing" from "the sender said the
//! default"; defaults are applied once, at finalisation.

use std::fmt;
use std::num::{IntErrorKind, NonZeroU32};

use crate::buffer_states::osc99_notification::{
    NotificationOccasion, NotificationUrgency, Osc99ActivationFocus, Osc99ActivationReporting,
    Osc99CloseReporting, Osc99Expiry,
};
use crate::key_value::{KeyValueError, KeyValueItem, KeyValueSeparator, tokenize};

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
    /// Any other `p=` value (including an empty one).  Its payload is
    /// discarded by reassembly, but its metadata and `d=` still apply.
    Unknown,
}

/// How the payload of one OSC 99 chunk is encoded (`e=` key).
///
/// The payload carried by [`Osc99Command`] is always the raw bytes received;
/// this says whether they are verbatim UTF-8 text or base64 text.  The
/// terminal handler decodes base64 chunks as one continuous stream, because
/// the spec allows a payload to be split either before or after encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Osc99PayloadEncoding {
    /// `e=0` (default): the payload is escape-safe UTF-8 text.
    #[default]
    Plain,
    /// `e=1`: the payload is base64 text.
    Base64,
}

/// Whether more chunks of the same notification follow (`d=` key).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Osc99ChunkState {
    /// `d=0`: more chunks follow.
    More,
    /// `d=1`, any other `d=` value (an empty one included), or no `d=`
    /// (default): this chunk completes the notification.
    Done,
}

/// An activation action named by the `a=` key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Osc99Action {
    /// `report`: send an activation report back to the application.
    Report,
    /// `focus`: focus the source window on activation.
    Focus,
}

/// One edit to the activation-action set from the `a=` key.
///
/// Edits are applied in arrival order, starting from the default action set
/// (`focus` only); see [`resolve_osc99_actions`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Osc99ActionEdit {
    /// A bare action word (`report` / `focus`): add the action.
    Add(Osc99Action),
    /// A `-`-prefixed action word (`-report` / `-focus`): remove the action.
    Remove(Osc99Action),
}

/// What survives of a chunk the parser rejected: the identity of the transfer
/// it belonged to, scanned best-effort from the raw metadata.
///
/// A later chunk of the same transfer must not be assembled into a truncated
/// notification, so the terminal uses this to abandon the transfer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Osc99RejectedChunk {
    /// The notification id (`i=`), sanitised like a parsed one; the last
    /// valid `i=` in the metadata wins.
    pub id: Option<String>,
    /// Whether the rejected chunk was the last of its transfer (`d=`).
    pub chunk_state: Osc99ChunkState,
}

/// Error produced while parsing an OSC 99 metadata + payload pair.
///
/// Only the two size guards are errors: everything else the parser does not
/// understand is ignored.  Both variants carry the [`Osc99RejectedChunk`] so
/// the rejection can be acted on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Osc99ParseError {
    /// The metadata + payload region exceeded [`MAX_OSC99_SEQUENCE_BYTES`].
    /// Rejected before any allocation/decode to bound memory use on
    /// untrusted terminal input.
    SequenceTooLarge {
        /// The size of the metadata + payload region, in bytes.
        bytes: usize,
        /// The transfer the rejected chunk belonged to.
        rejected: Osc99RejectedChunk,
    },
    /// The metadata held more than [`MAX_OSC99_METADATA_ITEMS`] `key=value`
    /// items. Rejected to bound work on untrusted terminal input.
    TooManyMetadataItems {
        /// The item cap that was exceeded.
        max: usize,
        /// The transfer the rejected chunk belonged to.
        rejected: Osc99RejectedChunk,
    },
}

impl Osc99ParseError {
    /// The transfer the rejected chunk belonged to.
    #[must_use]
    pub const fn rejected(&self) -> &Osc99RejectedChunk {
        match self {
            Self::SequenceTooLarge { rejected, .. }
            | Self::TooManyMetadataItems { rejected, .. } => rejected,
        }
    }
}

impl fmt::Display for Osc99ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SequenceTooLarge { bytes, .. } => {
                write!(f, "OSC 99 sequence too large: {bytes} bytes")
            }
            Self::TooManyMetadataItems { max, .. } => {
                write!(f, "OSC 99 metadata has more than {max} items")
            }
        }
    }
}

/// Maximum accepted size (in bytes) of a single OSC 99 metadata + payload
/// region before decode.
///
/// Terminal escape-sequence input is untrusted, and the parser copies the
/// payload of every sequence it accepts. A single sequence is therefore
/// capped here, before that copy; the terminal handler bounds multi-chunk
/// reassembly separately with its own per-payload, per-notification and
/// handler-wide caps (see `notify_99.rs`). 1 MiB equals the handler's icon
/// payload cap, so one sequence can carry an icon of any size the handler
/// would accept, while pathological input is refused.
pub const MAX_OSC99_SEQUENCE_BYTES: usize = 1_048_576;

/// Maximum number of `key=value` items accepted in one OSC 99 metadata region.
///
/// The protocol defines fewer than twenty keys, some of which may repeat
/// (`n=`, `t=`), so 64 is generous for any real client while bounding the work
/// a hostile sequence can force. Exceeding it yields
/// [`Osc99ParseError::TooManyMetadataItems`].
pub const MAX_OSC99_METADATA_ITEMS: usize = 64;

/// Maximum length, in bytes, of a sanitised `i=` / `g=` identifier.
const MAX_OSC99_ID_BYTES: usize = 512;

/// A fully-parsed OSC 99 notification command (one escape sequence).
///
/// The typed output of [`parse_osc_99`]. Chunk reassembly, dispatch, and
/// reverse-write are NOT done here — this is a pure parse of a single OSC 99
/// sequence.
///
/// Every metadata field that has a protocol default is *unset-aware*: `None`
/// (or an empty list) means the sequence did not carry the key, so reassembly
/// can merge chunks without a later chunk's silence overriding an earlier
/// chunk's value.  The exceptions are [`payload_type`](Self::payload_type),
/// [`chunk_state`](Self::chunk_state) and
/// [`payload_encoding`](Self::payload_encoding), which are per chunk and
/// never merged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Osc99Command {
    /// Notification id (`i=`), sanitised; `None` when absent or empty after
    /// sanitising.
    pub id: Option<String>,
    /// Payload type (`p=`), default `Title`.
    pub payload_type: Osc99PayloadType,
    /// Whether more chunks follow (`d=`), default `Done`.
    pub chunk_state: Osc99ChunkState,
    /// The raw payload bytes exactly as received: text for
    /// [`Osc99PayloadEncoding::Plain`] (not validated as UTF-8 here), base64
    /// text for [`Osc99PayloadEncoding::Base64`].  Base64 is **not** decoded
    /// here; chunks are decoded as one stream during reassembly.
    pub payload: Vec<u8>,
    /// How [`payload`](Self::payload) is encoded (`e=`), default `Plain`.
    pub payload_encoding: Osc99PayloadEncoding,
    /// Activation-action edits (`a=`), in arrival order.  Empty when no
    /// action word was given; see [`resolve_osc99_actions`].
    pub action_edits: Vec<Osc99ActionEdit>,
    /// Close reporting (`c=`): `Some(Silent)` for `c=0`, `Some(Report)` for any
    /// other value (an empty one included), `None` when absent.
    pub close_reporting: Option<Osc99CloseReporting>,
    /// App name (`f=`, base64-decoded), if present and decodable.
    pub app_name: Option<String>,
    /// Icon-data cache key (`g=`), sanitised, if present.
    pub icon_cache_key: Option<String>,
    /// Icon names (`n=`, base64-decoded), in order.
    pub icon_names: Vec<String>,
    /// Occasion (`o=`), if present and recognised.
    pub occasion: Option<NotificationOccasion>,
    /// Raw sound name (`s=`, base64-decoded), if present and decodable.  It is
    /// mapped to a transport sound at finalisation.
    pub sound: Option<String>,
    /// Type/category tags (`t=`, base64-decoded), in order.
    pub notification_types: Vec<String>,
    /// Urgency (`u=`), if present and recognised.
    pub urgency: Option<NotificationUrgency>,
    /// Auto-expiry (`w=`), if present and numeric.
    pub expiry: Option<Osc99Expiry>,
}

impl Osc99Command {
    /// A command with every field at its protocol default: no id, a `Title`
    /// payload, `Done`, an empty plain payload, and nothing else set.
    const fn blank() -> Self {
        Self {
            id: None,
            payload_type: Osc99PayloadType::Title,
            chunk_state: Osc99ChunkState::Done,
            payload: Vec::new(),
            payload_encoding: Osc99PayloadEncoding::Plain,
            action_edits: Vec::new(),
            close_reporting: None,
            app_name: None,
            icon_cache_key: None,
            icon_names: Vec::new(),
            occasion: None,
            sound: None,
            notification_types: Vec::new(),
            urgency: None,
            expiry: None,
        }
    }
}

/// Map a `w=` value (milliseconds) to an [`Osc99Expiry`].
///
/// Any value below `-1` is clamped to `-1`.  `-1` is the spec's "OS default"
/// sentinel and maps to [`Osc99Expiry::OsDefault`]; `0` means "never expire";
/// a positive value is the lifetime in milliseconds, clamped to [`u32::MAX`].
#[must_use]
pub fn osc99_expiry_from_ms(ms: i64) -> Osc99Expiry {
    if ms < 0 {
        return Osc99Expiry::OsDefault;
    }
    // `try_from` only fails for values above `u32::MAX`, which clamp.
    let clamped = u32::try_from(ms).unwrap_or(u32::MAX);
    NonZeroU32::new(clamped).map_or(Osc99Expiry::Never, Osc99Expiry::After)
}

/// Apply `a=` edits, in order, to the default action set (`focus` only) and
/// return the resulting activation behaviour.
///
/// `report` / `focus` add an action and `-report` / `-focus` remove it, so a
/// later edit overrides an earlier one for the same action.
#[must_use]
pub fn resolve_osc99_actions(
    edits: &[Osc99ActionEdit],
) -> (Osc99ActivationReporting, Osc99ActivationFocus) {
    let mut reporting = Osc99ActivationReporting::Silent;
    let mut focus = Osc99ActivationFocus::Focus;
    for edit in edits {
        match edit {
            Osc99ActionEdit::Add(Osc99Action::Report) => {
                reporting = Osc99ActivationReporting::Report;
            }
            Osc99ActionEdit::Remove(Osc99Action::Report) => {
                reporting = Osc99ActivationReporting::Silent;
            }
            Osc99ActionEdit::Add(Osc99Action::Focus) => focus = Osc99ActivationFocus::Focus,
            Osc99ActionEdit::Remove(Osc99Action::Focus) => focus = Osc99ActivationFocus::NoFocus,
        }
    }
    (reporting, focus)
}

/// Base64-decode `value` and require the result to be non-empty UTF-8.
///
/// `None` when the value is not valid base64, does not decode to UTF-8, or
/// decodes to the empty string; the caller then ignores the key.  An empty
/// name carries no information (kitty treats an empty `f=` / `s=` as unset),
/// and an empty `n=` / `t=` entry must not become an icon name or a type.
fn decode_base64_utf8(value: &[u8]) -> Option<String> {
    let bytes = crate::base64::decode(value).ok()?;
    String::from_utf8(bytes).ok().filter(|s| !s.is_empty())
}

/// Whether `byte` is allowed in an `i=` / `g=` identifier: `[A-Za-z0-9_+.-]`.
const fn is_id_byte(byte: u8) -> bool {
    matches!(byte, b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | b'-' | b'+' | b'.')
}

/// Sanitise an identifier (`i=` or `g=`): drop every byte outside
/// `[A-Za-z0-9_+.-]` and truncate to 512 bytes.
///
/// Operating on bytes means non-UTF-8 input is simply filtered out.  The
/// result is pure ASCII, so the byte limit is also a character limit.
/// `None` when nothing survives, so an all-illegal value reads as absent.
fn sanitise_id(value: &[u8]) -> Option<String> {
    let id: String = value
        .iter()
        .copied()
        .filter(|&b| is_id_byte(b))
        .take(MAX_OSC99_ID_BYTES)
        .map(char::from)
        .collect();
    (!id.is_empty()).then_some(id)
}

/// Parse one `a=` comma-list token into an edit; unknown words give `None`.
///
/// A word beginning with `-` removes the action, and any further leading `+`
/// or `-` characters after that first `-` are skipped, as kitty does.  A
/// word without the leading `-` must be exactly an action name: kitty does
/// not treat a leading `+` as meaningful there, so `+report` is unknown and
/// ignored.
fn parse_action_edit(token: &[u8]) -> Option<Osc99ActionEdit> {
    let (negated, name) = token.strip_prefix(b"-").map_or((false, token), |rest| {
        let start = rest
            .iter()
            .position(|&b| b != b'+' && b != b'-')
            .unwrap_or(rest.len());
        (true, &rest[start..])
    });
    let action = match name {
        b"report" => Osc99Action::Report,
        b"focus" => Osc99Action::Focus,
        _ => return None,
    };
    Some(if negated {
        Osc99ActionEdit::Remove(action)
    } else {
        Osc99ActionEdit::Add(action)
    })
}

/// Parse a `p=` value into a payload type; anything unrecognised is
/// [`Osc99PayloadType::Unknown`].
const fn parse_payload_type(value: &[u8]) -> Osc99PayloadType {
    match value {
        b"title" => Osc99PayloadType::Title,
        b"body" => Osc99PayloadType::Body,
        b"close" => Osc99PayloadType::Close,
        b"icon" => Osc99PayloadType::Icon,
        b"alive" => Osc99PayloadType::Alive,
        b"buttons" => Osc99PayloadType::Buttons,
        b"?" => Osc99PayloadType::Query,
        _ => Osc99PayloadType::Unknown,
    }
}

/// Parse a `w=` value: an ASCII decimal integer.  `None` when it is not one.
///
/// An integer beyond the `i64` range is still an integer: kitty reads
/// arbitrary-precision ones, so it clamps here rather than being ignored (a
/// huge positive lifetime is the longest one, a huge negative one the OS
/// default).
fn parse_expiry(value: &[u8]) -> Option<Osc99Expiry> {
    let text = std::str::from_utf8(value).ok()?;
    let ms = match text.parse::<i64>() {
        Ok(ms) => ms,
        Err(err) => match err.kind() {
            IntErrorKind::PosOverflow => i64::MAX,
            IntErrorKind::NegOverflow => i64::MIN,
            _ => return None,
        },
    };
    Some(osc99_expiry_from_ms(ms))
}

/// Map a `d=` value to a chunk state: `0` means more chunks follow, and any
/// other value, an empty one included, means this chunk is the last.
fn chunk_state_from_value(value: &[u8]) -> Osc99ChunkState {
    if value == b"0" {
        Osc99ChunkState::More
    } else {
        Osc99ChunkState::Done
    }
}

/// Apply a single `key=value` metadata pair to `cmd`.
///
/// The key byte is guaranteed by the caller to be exactly one byte; `value` is
/// everything after the first `=` in the colon-separated token.  An invalid
/// value leaves the corresponding field unchanged.
fn apply_metadata_pair(cmd: &mut Osc99Command, key: u8, value: &[u8]) {
    match key {
        // `a` — actions: comma-list of `report`/`focus`, each optionally
        // `-`-prefixed.  Edits accumulate; unknown words add nothing.
        b'a' => cmd
            .action_edits
            .extend(value.split(|&b| b == b',').filter_map(parse_action_edit)),

        // `c` — close reporting: `0` is silent, any other value (an empty one
        // included, as in kitty) reports.
        b'c' => {
            cmd.close_reporting = Some(if value == b"0" {
                Osc99CloseReporting::Silent
            } else {
                Osc99CloseReporting::Report
            });
        }

        // `d` — done flag: `0` means more chunks follow, any other value (an
        // empty one included, as in kitty) is done.
        b'd' => cmd.chunk_state = chunk_state_from_value(value),

        // `e` — payload encoding: `1` is base64, any other value is plain.
        b'e' if !value.is_empty() => {
            cmd.payload_encoding = if value == b"1" {
                Osc99PayloadEncoding::Base64
            } else {
                Osc99PayloadEncoding::Plain
            };
        }

        // `f` — app name: base64 UTF-8.
        b'f' => {
            if let Some(name) = decode_base64_utf8(value) {
                cmd.app_name = Some(name);
            }
        }

        // `g` — icon cache key: plain identifier, sanitised.
        b'g' => {
            if let Some(key) = sanitise_id(value) {
                cmd.icon_cache_key = Some(key);
            }
        }

        // `i` — notification id: plain identifier, sanitised.
        b'i' => {
            if let Some(id) = sanitise_id(value) {
                cmd.id = Some(id);
            }
        }

        // `n` — icon name: base64 UTF-8, may repeat (accumulate in order).
        b'n' => {
            if let Some(name) = decode_base64_utf8(value) {
                cmd.icon_names.push(name);
            }
        }

        // `o` — occasion: `always`/`unfocused`/`invisible`; unrecognised is ignored.
        b'o' => {
            cmd.occasion = match value {
                b"always" => Some(NotificationOccasion::Always),
                b"unfocused" => Some(NotificationOccasion::Unfocused),
                b"invisible" => Some(NotificationOccasion::Invisible),
                _ => cmd.occasion,
            };
        }

        // `p` — payload type; unrecognised is its own type.
        b'p' => cmd.payload_type = parse_payload_type(value),

        // `s` — sound name: base64 UTF-8.
        b's' => {
            if let Some(name) = decode_base64_utf8(value) {
                cmd.sound = Some(name);
            }
        }

        // `t` — type/category: base64 UTF-8, may repeat (accumulate in order).
        b't' => {
            if let Some(name) = decode_base64_utf8(value) {
                cmd.notification_types.push(name);
            }
        }

        // `u` — urgency: plain integer 0/1/2; anything else is ignored.
        b'u' => {
            cmd.urgency = match value {
                b"0" => Some(NotificationUrgency::Low),
                b"1" => Some(NotificationUrgency::Normal),
                b"2" => Some(NotificationUrgency::Critical),
                _ => cmd.urgency,
            };
        }

        // `w` — auto-expiry ms; a non-integer is ignored.
        b'w' => {
            if let Some(expiry) = parse_expiry(value) {
                cmd.expiry = Some(expiry);
            }
        }

        // Unknown keys, and an empty `e` value — ignored for forward
        // compatibility.
        _ => {}
    }
}

/// Parse an OSC 99 metadata + payload byte pair into a typed [`Osc99Command`].
///
/// `metadata` is the colon-separated `key=value` region (between the two
/// semicolons of `ESC ] 99 ; <metadata> ; <payload> ST`); `payload` is the raw
/// payload region. The caller is responsible for extracting these from the
/// raw OSC parameters (splitting on the second `;` only). Pure parser: no
/// state, no dispatch.
///
/// Anything the parser does not understand is ignored rather than rejected:
/// see the module documentation.  A plain payload is returned as received, not
/// validated as UTF-8 (it is decoded lossily at finalisation), and a base64
/// payload is returned undecoded (see [`Osc99Command::payload`]).
///
/// # Errors
/// Returns [`Osc99ParseError::SequenceTooLarge`] if `metadata` plus `payload`
/// exceed [`MAX_OSC99_SEQUENCE_BYTES`], or
/// [`Osc99ParseError::TooManyMetadataItems`] if the metadata holds more than
/// [`MAX_OSC99_METADATA_ITEMS`] items.
pub fn parse_osc_99(metadata: &[u8], payload: &[u8]) -> Result<Osc99Command, Osc99ParseError> {
    // Reject oversized input before any allocation, so an unbounded payload
    // on untrusted terminal input cannot force a large allocation.
    let total = metadata.len().saturating_add(payload.len());
    if total > MAX_OSC99_SEQUENCE_BYTES {
        return Err(Osc99ParseError::SequenceTooLarge {
            bytes: total,
            rejected: scan_rejected_chunk(metadata),
        });
    }

    let mut cmd = Osc99Command::blank();

    // Parse the colon-separated key=value pairs in the metadata region. Empty
    // tokens (leading/trailing/doubled colons) are skipped by the tokenizer.
    for item in tokenize(metadata, KeyValueSeparator::Colon, MAX_OSC99_METADATA_ITEMS) {
        match item {
            // A key must be exactly one byte; a token with no `=`, or with a
            // longer (or empty) key, is malformed and skipped.
            Ok(KeyValueItem::Pair { key: &[key], value }) => {
                apply_metadata_pair(&mut cmd, key, value);
            }
            Ok(KeyValueItem::Pair { .. } | KeyValueItem::Bare(_)) => {}
            Err(KeyValueError::TooManyItems { max }) => {
                return Err(Osc99ParseError::TooManyMetadataItems {
                    max,
                    rejected: scan_rejected_chunk(metadata),
                });
            }
        }
    }

    // Copy the payload only once the metadata has been accepted, so a
    // sequence rejected by the item cap never allocates its payload.
    cmd.payload = payload.to_vec();
    Ok(cmd)
}

/// Read the `i=` and `d=` of a rejected sequence's raw metadata.
///
/// Only those two keys are looked at, with the same rules as a parsed chunk
/// (a key is one byte, the last valid `i=` and the last `d=` win), but over
/// the whole metadata rather than up to the item cap, because a hostile
/// sequence puts them wherever it likes.  The scan is linear in the metadata,
/// allocates only the sanitised id, and decodes nothing.
fn scan_rejected_chunk(metadata: &[u8]) -> Osc99RejectedChunk {
    let mut rejected = Osc99RejectedChunk {
        id: None,
        chunk_state: Osc99ChunkState::Done,
    };
    for item in tokenize(metadata, KeyValueSeparator::Colon, usize::MAX) {
        match item {
            Ok(KeyValueItem::Pair { key: b"i", value }) => {
                if let Some(id) = sanitise_id(value) {
                    rejected.id = Some(id);
                }
            }
            Ok(KeyValueItem::Pair { key: b"d", value }) => {
                rejected.chunk_state = chunk_state_from_value(value);
            }
            Ok(KeyValueItem::Pair { .. } | KeyValueItem::Bare(_)) => {}
            // An unbounded scan cannot hit the item cap.
            Err(KeyValueError::TooManyItems { .. }) => break,
        }
    }
    rejected
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

    /// Parse `metadata` with an empty payload.
    fn parse(metadata: &str) -> Osc99Command {
        parse_osc_99(&meta(metadata), &pay("")).unwrap()
    }

    /// `n` bytes of `A`, as a `String`.
    fn long_id(n: usize) -> String {
        "A".repeat(n)
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
        assert_eq!(cmd.chunk_state, Osc99ChunkState::Done, "d defaults to done");
        assert_eq!(cmd.payload_encoding, Osc99PayloadEncoding::Plain);
        assert!(cmd.id.is_none());
        assert_eq!(cmd.action_edits, []);
        assert!(cmd.close_reporting.is_none());
        assert!(cmd.app_name.is_none());
        assert!(cmd.icon_cache_key.is_none());
        assert_eq!(cmd.icon_names, [] as [String; 0]);
        assert!(cmd.occasion.is_none(), "o is unset, not Always");
        assert!(cmd.sound.is_none());
        assert_eq!(cmd.notification_types, [] as [String; 0]);
        assert!(cmd.urgency.is_none());
        assert!(cmd.expiry.is_none(), "w is unset, not OsDefault");
    }

    // ------------------------------------------------------------------
    // `p=` payload type
    // ------------------------------------------------------------------

    #[test]
    fn p_equals_title() {
        assert_eq!(parse("p=title").payload_type, Osc99PayloadType::Title);
    }

    #[test]
    fn p_equals_body() {
        assert_eq!(parse("p=body").payload_type, Osc99PayloadType::Body);
    }

    #[test]
    fn p_equals_close() {
        assert_eq!(parse("p=close").payload_type, Osc99PayloadType::Close);
    }

    #[test]
    fn p_equals_icon() {
        assert_eq!(parse("p=icon").payload_type, Osc99PayloadType::Icon);
    }

    #[test]
    fn p_equals_alive() {
        assert_eq!(parse("p=alive").payload_type, Osc99PayloadType::Alive);
    }

    #[test]
    fn p_equals_buttons() {
        assert_eq!(parse("p=buttons").payload_type, Osc99PayloadType::Buttons);
    }

    #[test]
    fn p_equals_query() {
        assert_eq!(parse("p=?").payload_type, Osc99PayloadType::Query);
    }

    #[test]
    fn p_equals_unrecognised_value_is_unknown() {
        assert_eq!(
            parse("p=frobnicate").payload_type,
            Osc99PayloadType::Unknown
        );
    }

    #[test]
    fn p_equals_empty_is_unknown() {
        assert_eq!(parse("p=").payload_type, Osc99PayloadType::Unknown);
    }

    #[test]
    fn p_unknown_keeps_the_rest_of_the_metadata() {
        let cmd = parse("i=x:p=future:d=0:u=2");
        assert_eq!(cmd.payload_type, Osc99PayloadType::Unknown);
        assert_eq!(cmd.id, Some("x".to_owned()));
        assert_eq!(cmd.chunk_state, Osc99ChunkState::More);
        assert_eq!(cmd.urgency, Some(NotificationUrgency::Critical));
    }

    #[test]
    fn p_last_value_wins() {
        assert_eq!(
            parse("p=body:p=title").payload_type,
            Osc99PayloadType::Title
        );
    }

    // ------------------------------------------------------------------
    // `i=` identifier
    // ------------------------------------------------------------------

    #[test]
    fn i_equals_valid() {
        let cmd = parse("i=abc-123_def.ghi+");
        assert_eq!(cmd.id, Some("abc-123_def.ghi+".to_owned()));
    }

    #[test]
    fn i_equals_illegal_chars_are_removed() {
        assert_eq!(parse("i=abc!def").id, Some("abcdef".to_owned()));
    }

    #[test]
    fn i_equals_space_is_removed() {
        assert_eq!(parse("i=abc def").id, Some("abcdef".to_owned()));
    }

    #[test]
    fn i_equals_slash_is_removed() {
        assert_eq!(parse("i=a/b").id, Some("ab".to_owned()));
    }

    #[test]
    fn i_equals_only_illegal_chars_is_absent() {
        assert_eq!(parse("i=///").id, None);
    }

    #[test]
    fn i_equals_empty_is_absent() {
        assert_eq!(parse("i=").id, None);
    }

    #[test]
    fn i_equals_zero_is_an_ordinary_id() {
        assert_eq!(parse("i=0").id, Some("0".to_owned()));
    }

    #[test]
    fn i_equals_long_id_is_truncated_to_512_bytes() {
        let cmd = parse(&format!("i={}", long_id(600)));
        assert_eq!(cmd.id, Some(long_id(512)));
    }

    #[test]
    fn i_equals_id_at_512_bytes_is_unchanged() {
        let cmd = parse(&format!("i={}", long_id(512)));
        assert_eq!(cmd.id, Some(long_id(512)));
    }

    #[test]
    fn i_truncation_counts_surviving_bytes_only() {
        // 600 illegal bytes then 3 legal ones: filtered before truncation.
        let cmd = parse(&format!("i={}abc", "!".repeat(600)));
        assert_eq!(cmd.id, Some("abc".to_owned()));
    }

    #[test]
    fn i_non_utf8_bytes_are_removed() {
        let cmd = parse_osc_99(b"i=a\xFF\xFEb", b"").unwrap();
        assert_eq!(cmd.id, Some("ab".to_owned()));
    }

    #[test]
    fn i_invalid_value_does_not_clear_an_earlier_id() {
        assert_eq!(parse("i=first:i=///").id, Some("first".to_owned()));
    }

    // ------------------------------------------------------------------
    // `g=` cache key
    // ------------------------------------------------------------------

    #[test]
    fn g_equals_valid() {
        assert_eq!(
            parse("g=cache-key.01").icon_cache_key,
            Some("cache-key.01".to_owned())
        );
    }

    #[test]
    fn g_equals_illegal_chars_are_removed() {
        assert_eq!(parse("g=bad/key").icon_cache_key, Some("badkey".to_owned()));
    }

    #[test]
    fn g_equals_only_illegal_chars_is_absent() {
        assert_eq!(parse("g=///").icon_cache_key, None);
    }

    #[test]
    fn g_equals_long_key_is_truncated_to_512_bytes() {
        let cmd = parse(&format!("g={}", long_id(600)));
        assert_eq!(cmd.icon_cache_key, Some(long_id(512)));
    }

    // ------------------------------------------------------------------
    // `d=` done flag
    // ------------------------------------------------------------------

    #[test]
    fn d_equals_zero_means_more() {
        assert_eq!(parse("d=0").chunk_state, Osc99ChunkState::More);
    }

    #[test]
    fn d_equals_one_is_done() {
        assert_eq!(parse("d=1").chunk_state, Osc99ChunkState::Done);
    }

    #[test]
    fn d_equals_two_is_done() {
        assert_eq!(parse("d=2").chunk_state, Osc99ChunkState::Done);
    }

    #[test]
    fn d_equals_non_numeric_is_done() {
        assert_eq!(parse("d=abc").chunk_state, Osc99ChunkState::Done);
    }

    #[test]
    fn d_equals_empty_is_not_zero_so_done() {
        // kitty: `done = v != '0'`, so an empty value completes the chunk and
        // overrides an earlier `d=0`.
        assert_eq!(parse("d=").chunk_state, Osc99ChunkState::Done);
        assert_eq!(parse("d=0:d=").chunk_state, Osc99ChunkState::Done);
        assert_eq!(parse("d=:d=0").chunk_state, Osc99ChunkState::More);
    }

    // ------------------------------------------------------------------
    // `e=1` base64 payload
    // ------------------------------------------------------------------

    #[test]
    fn e_equals_one_base64_payload_kept_raw() {
        // The payload is NOT decoded by the parser; reassembly decodes it.
        let cmd = parse_osc_99(&meta("e=1"), &pay("SGVsbG8=")).unwrap();
        assert_eq!(cmd.payload, b"SGVsbG8=");
        assert_eq!(cmd.payload_encoding, Osc99PayloadEncoding::Base64);
    }

    #[test]
    fn e_equals_one_payload_is_not_validated_by_the_parser() {
        // A base64 payload may be one slice of a larger stream, so the parser
        // cannot judge it; the stream decoder rejects it later.
        let cmd = parse_osc_99(&meta("e=1"), &pay("not-valid-base64!!!")).unwrap();
        assert_eq!(cmd.payload, b"not-valid-base64!!!");
        assert_eq!(cmd.payload_encoding, Osc99PayloadEncoding::Base64);
    }

    #[test]
    fn e_equals_one_non_utf8_payload_is_kept_raw() {
        let cmd = parse_osc_99(&meta("e=1"), &[0xFF, 0xFE]).unwrap();
        assert_eq!(cmd.payload, [0xFF, 0xFE]);
    }

    #[test]
    fn e_equals_zero_plain_utf8_payload() {
        let cmd = parse_osc_99(&meta("e=0"), &pay("plain text")).unwrap();
        assert_eq!(cmd.payload, b"plain text");
        assert_eq!(cmd.payload_encoding, Osc99PayloadEncoding::Plain);
    }

    #[test]
    fn e_equals_two_is_plain() {
        assert_eq!(parse("e=2").payload_encoding, Osc99PayloadEncoding::Plain);
    }

    #[test]
    fn e_equals_non_numeric_is_plain() {
        assert_eq!(parse("e=yes").payload_encoding, Osc99PayloadEncoding::Plain);
    }

    #[test]
    fn e_equals_empty_is_ignored() {
        assert_eq!(
            parse("e=1:e=").payload_encoding,
            Osc99PayloadEncoding::Base64
        );
    }

    #[test]
    fn e_absent_defaults_to_plain_encoding() {
        let cmd = parse_osc_99(&meta(""), &pay("x")).unwrap();
        assert_eq!(cmd.payload_encoding, Osc99PayloadEncoding::Plain);
    }

    #[test]
    fn e_equals_zero_non_utf8_payload_is_accepted_with_bytes_preserved() {
        // Decoded lossily at finalisation, not rejected here.
        let cmd = parse_osc_99(&meta("e=0"), &[0xFF, 0xFE]).unwrap();
        assert_eq!(cmd.payload, [0xFF, 0xFE]);
        assert_eq!(cmd.payload_encoding, Osc99PayloadEncoding::Plain);
    }

    #[test]
    fn default_encoding_non_utf8_payload_is_accepted_with_bytes_preserved() {
        let cmd = parse_osc_99(&meta("p=body"), b"ok \xF0\x28\x8C\x28").unwrap();
        assert_eq!(cmd.payload, b"ok \xF0\x28\x8C\x28");
    }

    // ------------------------------------------------------------------
    // Base64 metadata keys: `f=`, `n=`, `s=`, `t=`
    // ------------------------------------------------------------------

    #[test]
    fn f_equals_app_name_decoded() {
        // "MyApp" in base64 is "TXlBcHA="
        assert_eq!(parse("f=TXlBcHA=").app_name, Some("MyApp".to_owned()));
    }

    #[test]
    fn n_equals_icon_name_single() {
        // "error" in base64 is "ZXJyb3I="
        assert_eq!(parse("n=ZXJyb3I=").icon_names, vec!["error".to_owned()]);
    }

    #[test]
    fn n_equals_icon_names_multiple_in_order() {
        // "error" = "ZXJyb3I=", "warn" = "d2Fybg=="
        let cmd = parse("n=ZXJyb3I=:n=d2Fybg==");
        assert_eq!(cmd.icon_names, vec!["error".to_owned(), "warn".to_owned()]);
    }

    #[test]
    fn s_equals_sound_decoded() {
        // "system" in base64 is "c3lzdGVt"
        assert_eq!(parse("s=c3lzdGVt").sound, Some("system".to_owned()));
    }

    #[test]
    fn t_equals_single_type_decoded() {
        // "alert" in base64 is "YWxlcnQ="
        assert_eq!(
            parse("t=YWxlcnQ=").notification_types,
            vec!["alert".to_owned()]
        );
    }

    #[test]
    fn t_equals_multiple_types_in_order() {
        // "alert" = "YWxlcnQ=", "info" = "aW5mbw=="
        let cmd = parse("t=YWxlcnQ=:t=aW5mbw==");
        assert_eq!(
            cmd.notification_types,
            vec!["alert".to_owned(), "info".to_owned()]
        );
    }

    #[test]
    fn bad_f_is_ignored_while_the_rest_applies() {
        let cmd = parse("i=x:f=!!!not-base64!!!:p=body");
        assert_eq!(cmd.app_name, None);
        assert_eq!(cmd.id, Some("x".to_owned()));
        assert_eq!(cmd.payload_type, Osc99PayloadType::Body);
    }

    #[test]
    fn bad_f_does_not_clear_an_earlier_f() {
        let cmd = parse("f=TXlBcHA=:f=!!!");
        assert_eq!(cmd.app_name, Some("MyApp".to_owned()));
    }

    #[test]
    fn base64_value_that_decodes_to_non_utf8_is_ignored() {
        // "/w==" decodes to the single byte 0xFF.
        let cmd = parse("f=/w==:n=/w==:s=/w==:t=/w==:u=2");
        assert_eq!(cmd.app_name, None);
        assert_eq!(cmd.icon_names, [] as [String; 0]);
        assert_eq!(cmd.sound, None);
        assert_eq!(cmd.notification_types, [] as [String; 0]);
        assert_eq!(cmd.urgency, Some(NotificationUrgency::Critical));
    }

    #[test]
    fn bad_n_and_t_are_skipped_and_good_ones_still_append() {
        // "error" = "ZXJyb3I=", "alert" = "YWxlcnQ="
        let cmd = parse("n=ZXJyb3I=:n=!!!:n=ZXJyb3I=:t=!!!:t=YWxlcnQ=");
        assert_eq!(cmd.icon_names, vec!["error".to_owned(), "error".to_owned()]);
        assert_eq!(cmd.notification_types, vec!["alert".to_owned()]);
    }

    // ------------------------------------------------------------------
    // `u=` urgency
    // ------------------------------------------------------------------

    #[test]
    fn u_equals_zero_low() {
        assert_eq!(parse("u=0").urgency, Some(NotificationUrgency::Low));
    }

    #[test]
    fn u_equals_one_normal() {
        assert_eq!(parse("u=1").urgency, Some(NotificationUrgency::Normal));
    }

    #[test]
    fn u_equals_two_critical() {
        assert_eq!(parse("u=2").urgency, Some(NotificationUrgency::Critical));
    }

    #[test]
    fn u_equals_three_is_ignored() {
        assert_eq!(parse("u=3").urgency, None);
    }

    #[test]
    fn u_equals_three_does_not_override_an_earlier_u() {
        assert_eq!(
            parse("u=2:u=3").urgency,
            Some(NotificationUrgency::Critical)
        );
    }

    #[test]
    fn u_equals_non_numeric_or_empty_is_ignored() {
        assert_eq!(parse("u=high").urgency, None);
        assert_eq!(parse("u=").urgency, None);
    }

    // ------------------------------------------------------------------
    // `o=` occasion
    // ------------------------------------------------------------------

    #[test]
    fn o_equals_always() {
        assert_eq!(
            parse("o=always").occasion,
            Some(NotificationOccasion::Always)
        );
    }

    #[test]
    fn o_equals_unfocused() {
        assert_eq!(
            parse("o=unfocused").occasion,
            Some(NotificationOccasion::Unfocused)
        );
    }

    #[test]
    fn o_equals_invisible() {
        assert_eq!(
            parse("o=invisible").occasion,
            Some(NotificationOccasion::Invisible)
        );
    }

    #[test]
    fn o_equals_bogus_is_ignored() {
        assert_eq!(parse("o=bogus").occasion, None);
    }

    // ------------------------------------------------------------------
    // `a=` action edits
    // ------------------------------------------------------------------

    #[test]
    fn a_absent_gives_no_edits() {
        assert_eq!(parse("").action_edits, []);
    }

    #[test]
    fn a_equals_empty_gives_no_edits() {
        assert_eq!(parse("a=").action_edits, []);
    }

    #[test]
    fn a_equals_report_adds_report() {
        assert_eq!(
            parse("a=report").action_edits,
            [Osc99ActionEdit::Add(Osc99Action::Report)]
        );
    }

    #[test]
    fn a_equals_report_minus_focus() {
        assert_eq!(
            parse("a=report,-focus").action_edits,
            [
                Osc99ActionEdit::Add(Osc99Action::Report),
                Osc99ActionEdit::Remove(Osc99Action::Focus),
            ]
        );
    }

    #[test]
    fn a_equals_minus_report_removes_report() {
        assert_eq!(
            parse("a=-report").action_edits,
            [Osc99ActionEdit::Remove(Osc99Action::Report)]
        );
    }

    #[test]
    fn a_equals_focus_adds_focus() {
        assert_eq!(
            parse("a=focus").action_edits,
            [Osc99ActionEdit::Add(Osc99Action::Focus)]
        );
    }

    #[test]
    fn a_equals_report_comma_focus() {
        assert_eq!(
            parse("a=report,focus").action_edits,
            [
                Osc99ActionEdit::Add(Osc99Action::Report),
                Osc99ActionEdit::Add(Osc99Action::Focus),
            ]
        );
    }

    #[test]
    fn a_unknown_words_are_ignored() {
        assert_eq!(
            parse("a=bogus,report,-bogus,,-").action_edits,
            [Osc99ActionEdit::Add(Osc99Action::Report)]
        );
    }

    #[test]
    fn a_leading_plus_is_not_an_add_prefix_but_is_skipped_after_a_minus() {
        // kitty lstrips `+` and `-` only after the first `-` has marked the
        // word as a removal; a bare `+report` is just an unknown word.
        assert_eq!(parse("a=+report,+focus").action_edits, []);
        assert_eq!(
            parse("a=-+report,--focus,-+-+focus").action_edits,
            [
                Osc99ActionEdit::Remove(Osc99Action::Report),
                Osc99ActionEdit::Remove(Osc99Action::Focus),
                Osc99ActionEdit::Remove(Osc99Action::Focus),
            ]
        );
    }

    #[test]
    fn a_repeated_keys_keep_appending_in_arrival_order() {
        assert_eq!(
            parse("a=report:a=-focus:a=focus").action_edits,
            [
                Osc99ActionEdit::Add(Osc99Action::Report),
                Osc99ActionEdit::Remove(Osc99Action::Focus),
                Osc99ActionEdit::Add(Osc99Action::Focus),
            ]
        );
    }

    #[test]
    fn a_empty_after_edits_does_not_clear_them() {
        assert_eq!(
            parse("a=report:a=").action_edits,
            [Osc99ActionEdit::Add(Osc99Action::Report)]
        );
    }

    // ------------------------------------------------------------------
    // `resolve_osc99_actions`
    // ------------------------------------------------------------------

    #[test]
    fn resolve_actions_no_edits_is_silent_and_focus() {
        assert_eq!(
            resolve_osc99_actions(&[]),
            (
                Osc99ActivationReporting::Silent,
                Osc99ActivationFocus::Focus
            )
        );
    }

    #[test]
    fn resolve_actions_add_report_keeps_default_focus() {
        assert_eq!(
            resolve_osc99_actions(&[Osc99ActionEdit::Add(Osc99Action::Report)]),
            (
                Osc99ActivationReporting::Report,
                Osc99ActivationFocus::Focus
            )
        );
    }

    #[test]
    fn resolve_actions_remove_focus() {
        assert_eq!(
            resolve_osc99_actions(&[Osc99ActionEdit::Remove(Osc99Action::Focus)]),
            (
                Osc99ActivationReporting::Silent,
                Osc99ActivationFocus::NoFocus
            )
        );
    }

    #[test]
    fn resolve_actions_add_then_remove_report_is_silent() {
        assert_eq!(
            resolve_osc99_actions(&[
                Osc99ActionEdit::Add(Osc99Action::Report),
                Osc99ActionEdit::Remove(Osc99Action::Report),
            ]),
            (
                Osc99ActivationReporting::Silent,
                Osc99ActivationFocus::Focus
            )
        );
    }

    #[test]
    fn resolve_actions_remove_then_add_focus_is_focus() {
        assert_eq!(
            resolve_osc99_actions(&[
                Osc99ActionEdit::Remove(Osc99Action::Focus),
                Osc99ActionEdit::Add(Osc99Action::Focus),
            ]),
            (
                Osc99ActivationReporting::Silent,
                Osc99ActivationFocus::Focus
            )
        );
    }

    // ------------------------------------------------------------------
    // `c=` close reporting
    // ------------------------------------------------------------------

    #[test]
    fn c_equals_zero_is_silent() {
        assert_eq!(
            parse("c=0").close_reporting,
            Some(Osc99CloseReporting::Silent)
        );
    }

    #[test]
    fn c_equals_one_reports() {
        assert_eq!(
            parse("c=1").close_reporting,
            Some(Osc99CloseReporting::Report)
        );
    }

    #[test]
    fn c_equals_two_reports() {
        assert_eq!(
            parse("c=2").close_reporting,
            Some(Osc99CloseReporting::Report)
        );
    }

    #[test]
    fn c_equals_empty_is_not_zero_so_reports() {
        // kitty: `close_response_requested = v != '0'`.
        assert_eq!(
            parse("c=").close_reporting,
            Some(Osc99CloseReporting::Report)
        );
        assert_eq!(
            parse("c=0:c=").close_reporting,
            Some(Osc99CloseReporting::Report)
        );
        assert_eq!(
            parse("c=:c=0").close_reporting,
            Some(Osc99CloseReporting::Silent)
        );
    }

    // ------------------------------------------------------------------
    // `w=` expiry
    // ------------------------------------------------------------------

    #[test]
    fn w_equals_5000() {
        assert_eq!(
            parse("w=5000").expiry,
            Some(Osc99Expiry::After(NonZeroU32::new(5000).unwrap()))
        );
    }

    #[test]
    fn w_equals_minus_one_is_os_default() {
        assert_eq!(parse("w=-1").expiry, Some(Osc99Expiry::OsDefault));
    }

    #[test]
    fn w_equals_zero_is_never() {
        assert_eq!(parse("w=0").expiry, Some(Osc99Expiry::Never));
    }

    #[test]
    fn w_equals_minus_five_clamps_to_os_default() {
        assert_eq!(parse("w=-5").expiry, Some(Osc99Expiry::OsDefault));
    }

    #[test]
    fn w_equals_non_numeric_is_ignored() {
        assert_eq!(parse("w=abc").expiry, None);
        assert_eq!(parse("w=").expiry, None);
    }

    #[test]
    fn w_non_numeric_does_not_override_an_earlier_w() {
        assert_eq!(parse("w=0:w=abc").expiry, Some(Osc99Expiry::Never));
    }

    #[test]
    fn w_beyond_u32_max_clamps_to_u32_max() {
        assert_eq!(
            parse("w=99999999999").expiry,
            Some(Osc99Expiry::After(NonZeroU32::new(u32::MAX).unwrap()))
        );
    }

    #[test]
    fn w_beyond_i64_range_clamps_to_the_maximum() {
        // kitty reads arbitrary-precision integers, so this is a (huge)
        // lifetime, not garbage.
        let max = Some(Osc99Expiry::After(NonZeroU32::new(u32::MAX).unwrap()));
        assert_eq!(parse("w=99999999999999999999999").expiry, max);
        assert_eq!(parse("w=+99999999999999999999999").expiry, max);
    }

    #[test]
    fn w_below_the_i64_range_is_the_os_default() {
        assert_eq!(
            parse("w=-99999999999999999999999").expiry,
            Some(Osc99Expiry::OsDefault)
        );
    }

    #[test]
    fn w_sign_alone_or_with_junk_is_ignored() {
        assert_eq!(parse("w=-").expiry, None);
        assert_eq!(parse("w=+").expiry, None);
        assert_eq!(parse("w=12x").expiry, None);
    }

    // ------------------------------------------------------------------
    // `osc99_expiry_from_ms`
    // ------------------------------------------------------------------

    #[test]
    fn expiry_minus_one_is_os_default() {
        assert_eq!(osc99_expiry_from_ms(-1), Osc99Expiry::OsDefault);
    }

    #[test]
    fn expiry_zero_is_never() {
        assert_eq!(osc99_expiry_from_ms(0), Osc99Expiry::Never);
    }

    #[test]
    fn expiry_positive_is_after_milliseconds() {
        assert_eq!(
            osc99_expiry_from_ms(5000),
            Osc99Expiry::After(NonZeroU32::new(5000).unwrap())
        );
    }

    #[test]
    fn expiry_above_u32_max_clamps() {
        let too_big = i64::from(u32::MAX) + 1;
        assert_eq!(
            osc99_expiry_from_ms(too_big),
            Osc99Expiry::After(NonZeroU32::new(u32::MAX).unwrap())
        );
        assert_eq!(
            osc99_expiry_from_ms(i64::MAX),
            Osc99Expiry::After(NonZeroU32::new(u32::MAX).unwrap())
        );
    }

    #[test]
    fn expiry_other_negative_is_os_default() {
        assert_eq!(osc99_expiry_from_ms(-2), Osc99Expiry::OsDefault);
        assert_eq!(osc99_expiry_from_ms(i64::MIN), Osc99Expiry::OsDefault);
    }

    // ------------------------------------------------------------------
    // `p=?` capability query
    // ------------------------------------------------------------------

    #[test]
    fn p_equals_query_form() {
        let cmd = parse("i=myid:p=?");
        assert_eq!(cmd.payload_type, Osc99PayloadType::Query);
        assert_eq!(cmd.id, Some("myid".to_owned()));
    }

    // ------------------------------------------------------------------
    // Malformed metadata
    // ------------------------------------------------------------------

    #[test]
    fn bare_token_is_ignored() {
        // A token with no `=` is skipped; its neighbours still apply.
        let cmd = parse("d=0:foo:u=2");
        assert_eq!(cmd.chunk_state, Osc99ChunkState::More);
        assert_eq!(cmd.urgency, Some(NotificationUrgency::Critical));
    }

    #[test]
    fn bare_token_alone_leaves_every_default() {
        let cmd = parse_osc_99(&meta("foo"), &pay("Hello")).unwrap();
        assert_eq!(cmd.payload_type, Osc99PayloadType::Title);
        assert_eq!(cmd.payload, b"Hello");
        assert_eq!(cmd.chunk_state, Osc99ChunkState::Done);
    }

    #[test]
    fn two_byte_key_is_ignored() {
        // key part must be exactly 1 byte; "ab=1" has a 2-byte key.
        let cmd = parse("ab=1:u=1");
        assert_eq!(cmd.urgency, Some(NotificationUrgency::Normal));
    }

    #[test]
    fn multi_byte_key_is_not_read_by_its_first_byte() {
        // `foo=bar` must not be interpreted as `f=...` / `d=...`.
        let cmd = parse("foo=TXlBcHA=:dd=0");
        assert_eq!(cmd.app_name, None);
        assert_eq!(cmd.chunk_state, Osc99ChunkState::Done);
    }

    #[test]
    fn empty_key_is_ignored() {
        let cmd = parse("=x:d=0");
        assert_eq!(cmd.chunk_state, Osc99ChunkState::More);
    }

    #[test]
    fn multi_byte_key_with_equals_in_value_is_ignored() {
        let cmd = parse("ab=c=d:d=0");
        assert_eq!(cmd.chunk_state, Osc99ChunkState::More);
    }

    #[test]
    fn empty_tokens_from_double_colon_are_skipped() {
        // `::` produces empty tokens which must be silently skipped.
        assert_eq!(parse("::d=0::").chunk_state, Osc99ChunkState::More);
    }

    #[test]
    fn leading_colon_skipped() {
        assert_eq!(parse(":d=1").chunk_state, Osc99ChunkState::Done);
    }

    #[test]
    fn trailing_colon_skipped() {
        assert_eq!(parse("d=0:").chunk_state, Osc99ChunkState::More);
    }

    // ------------------------------------------------------------------
    // Pinned behaviour (Task 129.3): must survive the tokenizer migration
    // ------------------------------------------------------------------

    #[test]
    fn empty_value_for_flag_keys() {
        // `c` and `d` read an empty value as "not 0" (kitty), so it reports
        // and completes; `e`, `u` and `w` ignore it.
        let cmd = parse("c=:d=:e=:u=:w=");
        assert_eq!(cmd.close_reporting, Some(Osc99CloseReporting::Report));
        assert_eq!(cmd.chunk_state, Osc99ChunkState::Done);
        assert_eq!(cmd.payload_encoding, Osc99PayloadEncoding::Plain);
        assert_eq!(cmd.urgency, None);
        assert_eq!(cmd.expiry, None);
    }

    #[test]
    fn empty_value_for_id_keys_is_absent() {
        let cmd = parse("i=:g=");
        assert_eq!(cmd.id, None);
        assert_eq!(cmd.icon_cache_key, None);
    }

    #[test]
    fn empty_value_for_occasion_keeps_current_value() {
        assert_eq!(
            parse("o=unfocused:o=").occasion,
            Some(NotificationOccasion::Unfocused)
        );
    }

    #[test]
    fn empty_value_for_unknown_key_is_ignored() {
        assert_eq!(parse("z=:d=0").chunk_state, Osc99ChunkState::More);
    }

    #[test]
    fn duplicate_scalar_keys_last_valid_value_wins() {
        let cmd = parse("i=first:i=second:u=0:u=2:d=0:d=1");
        assert_eq!(cmd.id, Some("second".to_owned()));
        assert_eq!(cmd.urgency, Some(NotificationUrgency::Critical));
        assert_eq!(cmd.chunk_state, Osc99ChunkState::Done);
    }

    #[test]
    fn value_may_contain_equals() {
        // Base64 padding lands in the value; only the first `=` splits.
        assert_eq!(parse("f=TXlBcHA=").app_name, Some("MyApp".to_owned()));
    }

    // ------------------------------------------------------------------
    // Metadata item cap (Task 129.3)
    // ------------------------------------------------------------------

    fn repeated_metadata(count: usize) -> Vec<u8> {
        vec!["d=1"; count].join(":").into_bytes()
    }

    #[test]
    fn metadata_at_item_cap_is_accepted() {
        let metadata = repeated_metadata(MAX_OSC99_METADATA_ITEMS);
        assert!(parse_osc_99(&metadata, &pay("")).is_ok());
    }

    #[test]
    fn metadata_over_item_cap_is_rejected() {
        let metadata = repeated_metadata(MAX_OSC99_METADATA_ITEMS + 1);
        let err = parse_osc_99(&metadata, &pay("")).unwrap_err();
        assert_eq!(
            err,
            Osc99ParseError::TooManyMetadataItems {
                max: MAX_OSC99_METADATA_ITEMS,
                rejected: Osc99RejectedChunk {
                    id: None,
                    chunk_state: Osc99ChunkState::Done
                },
            }
        );
    }

    #[test]
    fn empty_segments_do_not_count_toward_metadata_cap() {
        let metadata = vec!["d=1"; MAX_OSC99_METADATA_ITEMS]
            .join("::")
            .into_bytes();
        assert!(parse_osc_99(&metadata, &pay("")).is_ok());
    }

    #[test]
    fn malformed_items_count_toward_the_cap() {
        // Ignored tokens still cost work, so they use up the item budget.
        let mut metadata = b"foo:".to_vec();
        metadata.extend(repeated_metadata(MAX_OSC99_METADATA_ITEMS));
        let err = parse_osc_99(&metadata, &pay("")).unwrap_err();
        assert!(matches!(
            err,
            Osc99ParseError::TooManyMetadataItems {
                max: MAX_OSC99_METADATA_ITEMS,
                ..
            }
        ));
    }

    // ------------------------------------------------------------------
    // What a rejected chunk reports (138.C7 P4)
    // ------------------------------------------------------------------

    /// The rejected-chunk report of a sequence that must be refused.
    fn rejected(metadata: &[u8], payload: &[u8]) -> Osc99RejectedChunk {
        parse_osc_99(metadata, payload)
            .unwrap_err()
            .rejected()
            .clone()
    }

    /// `i=` and `d=` placed after the item cap, so only a full scan finds them.
    fn metadata_with_tail(tail: &str) -> Vec<u8> {
        let mut metadata = repeated_metadata(MAX_OSC99_METADATA_ITEMS + 1);
        metadata.push(b':');
        metadata.extend_from_slice(tail.as_bytes());
        metadata
    }

    #[test]
    fn a_rejected_chunk_reports_the_id_and_chunk_state_it_carried() {
        let metadata = metadata_with_tail("i=my-id:d=0");
        assert_eq!(
            rejected(&metadata, b""),
            Osc99RejectedChunk {
                id: Some("my-id".to_owned()),
                chunk_state: Osc99ChunkState::More,
            }
        );
    }

    #[test]
    fn a_rejected_chunk_applies_the_parser_rules_to_i_and_d() {
        // Sanitised id, last valid `i=` wins, an empty or illegal `i=` is
        // skipped, the last `d=` wins and an empty `d=` is "not 0".
        let metadata = metadata_with_tail("i=a/b:i=///:i=:d=0:d=");
        assert_eq!(
            rejected(&metadata, b""),
            Osc99RejectedChunk {
                id: Some("ab".to_owned()),
                chunk_state: Osc99ChunkState::Done,
            }
        );
        // A key longer than one byte is not `i` or `d`.
        let metadata = metadata_with_tail("id=nope:dd=0");
        assert_eq!(
            rejected(&metadata, b""),
            Osc99RejectedChunk {
                id: None,
                chunk_state: Osc99ChunkState::Done,
            }
        );
    }

    #[test]
    fn an_oversized_sequence_reports_its_id_and_chunk_state_too() {
        let payload = vec![b'A'; MAX_OSC99_SEQUENCE_BYTES];
        assert_eq!(
            rejected(b"i=big:d=0:p=icon:e=1", &payload),
            Osc99RejectedChunk {
                id: Some("big".to_owned()),
                chunk_state: Osc99ChunkState::More,
            }
        );
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
        assert_eq!(cmd.close_reporting, Some(Osc99CloseReporting::Report));
        assert_eq!(cmd.urgency, Some(NotificationUrgency::Normal));
        // Raw base64 text; decoded to "Build complete" during reassembly.
        assert_eq!(cmd.payload, b"QnVpbGQgY29tcGxldGU=");
        assert_eq!(cmd.payload_encoding, Osc99PayloadEncoding::Base64);
        assert_eq!(cmd.chunk_state, Osc99ChunkState::Done);
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
            cmd.notification_types,
            vec!["alert".to_owned(), "info".to_owned()]
        );
        assert_eq!(cmd.occasion, Some(NotificationOccasion::Unfocused));
        assert_eq!(cmd.urgency, Some(NotificationUrgency::Critical));
        assert_eq!(
            cmd.expiry,
            Some(Osc99Expiry::After(NonZeroU32::new(3000).unwrap()))
        );
        assert_eq!(cmd.payload, b"Test notification");
    }

    // ------------------------------------------------------------------
    // Display impl — all variants produce non-empty strings
    // ------------------------------------------------------------------

    #[test]
    fn display_all_error_variants() {
        let rejected = Osc99RejectedChunk {
            id: None,
            chunk_state: Osc99ChunkState::Done,
        };
        let errors = [
            Osc99ParseError::SequenceTooLarge {
                bytes: 2_000_000,
                rejected: rejected.clone(),
            },
            Osc99ParseError::TooManyMetadataItems { max: 64, rejected },
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
            Osc99ParseError::SequenceTooLarge { bytes, .. } => {
                assert!(bytes > MAX_OSC99_SEQUENCE_BYTES);
            }
            other @ Osc99ParseError::TooManyMetadataItems { .. } => {
                panic!("expected SequenceTooLarge, got {other:?}")
            }
        }
    }

    #[test]
    fn parse_osc_99_accepts_sequence_at_the_cap() {
        // metadata + payload exactly at the cap is accepted (boundary).
        let metadata = b"p=title";
        let payload = vec![b'x'; MAX_OSC99_SEQUENCE_BYTES - metadata.len()];
        assert!(parse_osc_99(metadata, &payload).is_ok());
    }

    #[test]
    fn empty_base64_values_for_name_keys_are_ignored() {
        let cmd = parse_osc_99(&meta("f=:n=:s=:t=:n=aWNvbg=="), &pay("x")).unwrap();
        assert_eq!(cmd.app_name, None);
        assert_eq!(cmd.sound, None);
        assert_eq!(cmd.icon_names, vec!["icon".to_owned()]);
        assert_eq!(cmd.notification_types, Vec::<String>::new());
    }
}
