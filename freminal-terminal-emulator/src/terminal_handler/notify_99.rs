// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! OSC 99 notification identity and chunk-reassembly state for [`TerminalHandler`].
//!
//! A multi-chunk OSC 99 notification arrives as a series of escape sequences,
//! each carrying `d=0` (more chunks follow) or `d=1`/default (final chunk).
//! This module holds the in-flight accumulator ([`PendingNotification`]), the
//! bounded store of in-flight accumulators ([`PendingOsc99`]), the finalized
//! output type ([`FinalizedNotification`]), the finalisation helpers
//! (control-character stripping, size limits), and the entry point
//! [`TerminalHandler::handle_osc99`].
//!
//! Control payloads (`p=?`, `p=close`, `p=alive`) act immediately, whatever
//! `d=` says, and never touch the pending store. Only content payloads
//! (`title`, `body`, `icon`, `buttons`, and unknown types) are reassembled.

use std::collections::HashMap;

use freminal_common::buffer_states::osc_notify_99::{
    MAX_OSC99_SEQUENCE_BYTES, NotificationOccasion, NotificationUrgency, Osc99ChunkState,
    Osc99Command, Osc99PayloadEncoding, Osc99PayloadType, resolve_osc99_actions,
};
use freminal_common::buffer_states::osc99_notification::{
    Notification99Data, Osc99CloseReporting, Osc99ControlKind, Osc99Expiry, Osc99Sound,
};
use freminal_common::buffer_states::window_manipulation::WindowManipulation;
use freminal_common::host_capabilities::{
    Osc99ActivationReport, Osc99AutoExpiry, Osc99BodySupport, Osc99ButtonSupport, Osc99CloseEvents,
    Osc99Features, Osc99FocusAction, Osc99IconSupport, Osc99SoundNames, Osc99Support,
    Osc99UrgencySupport,
};

use super::TerminalHandler;
use super::chunk_assembler::{BoundedChunkAssembler, ChunkEncoding, ChunkError, ChunkLimits};
use super::pty_writer::osc99_reply_id;
use crate::ansi_components::tracer::lossy_sequence_for_log_bounded;

// ── Accumulator ──────────────────────────────────────────────────────────────

/// Accumulator for a multi-chunk OSC 99 notification, keyed by its `i=` id
/// (or held in the single unidentified slot when it has none).
///
/// OSC 99 notifications may arrive across multiple escape sequences: each
/// non-final chunk carries `d=0`, and the final chunk carries `d=1` (or omits
/// `d`, defaulting to done). Same-typed payloads (`p=title`/`p=body`) are
/// concatenated across chunks. This holds the in-flight accumulation until the
/// terminating chunk finalizes it.
///
/// Each payload type has its own [`BoundedChunkAssembler`], which decodes
/// base64 chunks as one continuous stream (the spec lets a sender chunk before
/// or after encoding) and enforces the size caps.
#[derive(Debug)]
pub(in crate::terminal_handler) struct PendingNotification {
    /// Accumulated `p=title` payload bytes.
    title: BoundedChunkAssembler,
    /// Accumulated `p=body` payload bytes.
    body: BoundedChunkAssembler,
    /// Accumulated `p=icon` payload bytes.
    icon: BoundedChunkAssembler,
    /// Accumulated `p=buttons` payload bytes (U+2028-separated labels).
    buttons: BoundedChunkAssembler,
    /// The metadata merged across every chunk received so far (see
    /// [`merge_osc99_metadata`]).
    ///
    /// `None` until the first chunk arrives. The `payload` field of the
    /// stored command is always empty: the accumulators above are
    /// authoritative.
    meta: Option<Osc99Command>,
    /// Running cost of the list-valued metadata in `meta` (`n=` names, `t=`
    /// types and `a=` edits), charged once per newly merged entry at
    /// [`OSC99_LIST_ENTRY_OVERHEAD`] plus the entry's byte length. Kept as a
    /// counter so a chunk costs time proportional to its own entries, not to
    /// everything merged so far.
    list_cost: usize,
}

/// Maximum number of distinct in-flight (`i=`) OSC 99 notifications retained
/// while awaiting their terminating chunk. Terminal input is untrusted, so
/// this is a hard cap; when a new id arrives at the cap, the
/// least-recently-touched entry is evicted rather than the chunk refused.
const MAX_PENDING_OSC99_NOTIFICATIONS: usize = 128;

/// Maximum accumulated payload bytes for a single in-flight notification
/// (across title/body/icon/buttons), plus its list-valued metadata cost. A
/// stream of `d=0` chunks under one id could otherwise grow a single
/// accumulator without bound.
const MAX_OSC99_NOTIFICATION_BYTES: usize = 1_048_576;

/// Maximum accumulated bytes for the title, body or buttons payload of one
/// notification. Finalisation cuts a title or body to
/// [`MAX_OSC99_TEXT_CHARS`] characters and the buttons to
/// [`MAX_OSC99_BUTTONS`] labels of [`MAX_OSC99_BUTTON_CHARS`] characters, so
/// bytes beyond this cap could never be shown.
const MAX_OSC99_TEXT_PAYLOAD_BYTES: usize = 65_536;

/// Maximum accumulated bytes for the icon payload of one notification.
const MAX_OSC99_ICON_PAYLOAD_BYTES: usize = 1_048_576;

/// Maximum number of `n=` icon names merged into one notification.
const MAX_OSC99_ICON_NAMES: usize = 64;

/// Maximum number of `t=` notification types merged into one notification.
const MAX_OSC99_NOTIFICATION_TYPES: usize = 64;

/// Maximum number of `a=` action edits merged into one notification.
const MAX_OSC99_ACTION_EDITS: usize = 64;

/// Bytes charged for each list-valued metadata entry on top of its own
/// length, standing in for the entry's allocation and bookkeeping.
const OSC99_LIST_ENTRY_OVERHEAD: usize = 32;

/// Budget for the bytes held by every in-flight notification of one handler
/// together, the unidentified slot included: accumulated payload bytes plus
/// list-valued metadata cost. Per-notification caps alone leave the total
/// bounded only by [`MAX_PENDING_OSC99_NOTIFICATIONS`] times the per-
/// notification cap.
const MAX_OSC99_PENDING_TOTAL_BYTES: usize = 8 * 1_048_576;

/// Maximum length, in characters, of a notification's title or body.
const MAX_OSC99_TEXT_CHARS: usize = 1024;

/// Maximum length, in characters, of one button label.
const MAX_OSC99_BUTTON_CHARS: usize = 256;

/// Maximum number of button labels kept.
const MAX_OSC99_BUTTONS: usize = 8;

/// Caps for the title, body and buttons accumulators.
const OSC99_TEXT_CHUNK_LIMITS: ChunkLimits = ChunkLimits {
    max_chunk_bytes: MAX_OSC99_SEQUENCE_BYTES,
    max_total_bytes: MAX_OSC99_TEXT_PAYLOAD_BYTES,
};

/// Caps for the icon accumulator.
const OSC99_ICON_CHUNK_LIMITS: ChunkLimits = ChunkLimits {
    max_chunk_bytes: MAX_OSC99_SEQUENCE_BYTES,
    max_total_bytes: MAX_OSC99_ICON_PAYLOAD_BYTES,
};

/// Why a chunk made the handler abandon its notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(in crate::terminal_handler) enum Osc99Reject {
    /// A payload accumulator or the per-notification total rejected it.
    #[error(transparent)]
    Chunk(#[from] ChunkError),
    /// The notification would hold more than [`MAX_OSC99_ICON_NAMES`] `n=`
    /// names.
    #[error("more than {max} icon names")]
    TooManyIconNames {
        /// The cap.
        max: usize,
    },
    /// The notification would hold more than [`MAX_OSC99_NOTIFICATION_TYPES`]
    /// `t=` types.
    #[error("more than {max} notification types")]
    TooManyNotificationTypes {
        /// The cap.
        max: usize,
    },
    /// The notification would hold more than [`MAX_OSC99_ACTION_EDITS`] `a=`
    /// edits.
    #[error("more than {max} action edits")]
    TooManyActionEdits {
        /// The cap.
        max: usize,
    },
    /// The notification alone exceeds the handler-wide pending budget
    /// ([`MAX_OSC99_PENDING_TOTAL_BYTES`]).
    #[error("notification exceeds the pending budget of {max} bytes")]
    PendingBudget {
        /// The budget.
        max: usize,
    },
}

/// The four payload byte strings of a notification once every chunk has been
/// concatenated and decoded.
struct FinishedPayloads {
    /// The `p=title` bytes.
    title: Vec<u8>,
    /// The `p=body` bytes.
    body: Vec<u8>,
    /// The `p=icon` bytes.
    icon: Vec<u8>,
    /// The `p=buttons` bytes.
    buttons: Vec<u8>,
}

impl Default for PendingNotification {
    fn default() -> Self {
        Self {
            title: BoundedChunkAssembler::new(OSC99_TEXT_CHUNK_LIMITS),
            body: BoundedChunkAssembler::new(OSC99_TEXT_CHUNK_LIMITS),
            icon: BoundedChunkAssembler::new(OSC99_ICON_CHUNK_LIMITS),
            buttons: BoundedChunkAssembler::new(OSC99_TEXT_CHUNK_LIMITS),
            meta: None,
            list_cost: 0,
        }
    }
}

/// How a chunk's declared `e=` encoding maps onto the assembler's.
const fn chunk_encoding(encoding: Osc99PayloadEncoding) -> ChunkEncoding {
    match encoding {
        Osc99PayloadEncoding::Plain => ChunkEncoding::Raw,
        Osc99PayloadEncoding::Base64 => ChunkEncoding::Base64,
    }
}

/// An empty metadata accumulator: every scalar unset and every list empty.
///
/// Merging the first chunk into this is how the accumulator is seeded, so the
/// first chunk is merged exactly like every later one (list caps and costs
/// included) and nothing is cloned wholesale.
const fn blank_osc99_metadata() -> Osc99Command {
    Osc99Command {
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

/// Fold the metadata of a later `chunk` of the same notification into the
/// metadata `acc` accumulated so far.
///
/// - Scalars (`c`, `o`, `u`, `w`, `f`, `g`, `s`) take the chunk's value when
///   the chunk set one, and otherwise keep the accumulated value.
/// - Lists (`n`, `t`) and the `a=` edits extend in arrival order. The edits
///   are resolved once, at finalisation, so a later chunk that omits `a=`
///   cannot resurrect an action an earlier chunk removed.
/// - The id is the chunk's (it is the store key, so it is the same).
/// - `p`, `d` and `e` are per chunk: the latest chunk's values are stored.
///   Nothing reads them after finalisation.
///
/// The chunk is borrowed and only what is kept is cloned. `acc.payload` is
/// left alone (it is always empty) and `chunk.payload` is never copied:
/// payload bytes live in the accumulators, not in the metadata.
fn merge_osc99_metadata(acc: &mut Osc99Command, chunk: &Osc99Command) {
    acc.id.clone_from(&chunk.id);
    acc.payload_type = chunk.payload_type;
    acc.chunk_state = chunk.chunk_state;
    acc.payload_encoding = chunk.payload_encoding;
    if chunk.close_reporting.is_some() {
        acc.close_reporting = chunk.close_reporting;
    }
    if chunk.occasion.is_some() {
        acc.occasion = chunk.occasion;
    }
    if chunk.urgency.is_some() {
        acc.urgency = chunk.urgency;
    }
    if chunk.expiry.is_some() {
        acc.expiry = chunk.expiry;
    }
    if chunk.app_name.is_some() {
        acc.app_name.clone_from(&chunk.app_name);
    }
    if chunk.icon_cache_key.is_some() {
        acc.icon_cache_key.clone_from(&chunk.icon_cache_key);
    }
    if chunk.sound.is_some() {
        acc.sound.clone_from(&chunk.sound);
    }
    acc.icon_names.extend_from_slice(&chunk.icon_names);
    acc.notification_types
        .extend_from_slice(&chunk.notification_types);
    acc.action_edits.extend_from_slice(&chunk.action_edits);
}

impl PendingNotification {
    /// Total accumulated (decoded) payload bytes across all four content
    /// accumulators.
    const fn accumulated_len(&self) -> usize {
        self.title
            .len()
            .saturating_add(self.body.len())
            .saturating_add(self.icon.len())
            .saturating_add(self.buttons.len())
    }

    /// Bytes this notification holds: accumulated payloads plus the running
    /// cost of the list-valued metadata.
    const fn cost(&self) -> usize {
        self.accumulated_len().saturating_add(self.list_cost)
    }

    /// Check that merging `chunk`'s lists stays within the entry caps and
    /// return the cost they add.
    ///
    /// Only the chunk's own entries are visited; the entries already merged
    /// are accounted for by their lengths and by [`list_cost`](Self::list_cost).
    ///
    /// # Errors
    ///
    /// Returns the [`Osc99Reject`] for the first list that would pass its cap.
    fn admit_list_entries(&self, chunk: &Osc99Command) -> Result<usize, Osc99Reject> {
        let (held_names, held_types, held_edits) = self.meta.as_ref().map_or((0, 0, 0), |meta| {
            (
                meta.icon_names.len(),
                meta.notification_types.len(),
                meta.action_edits.len(),
            )
        });
        if held_names.saturating_add(chunk.icon_names.len()) > MAX_OSC99_ICON_NAMES {
            return Err(Osc99Reject::TooManyIconNames {
                max: MAX_OSC99_ICON_NAMES,
            });
        }
        if held_types.saturating_add(chunk.notification_types.len()) > MAX_OSC99_NOTIFICATION_TYPES
        {
            return Err(Osc99Reject::TooManyNotificationTypes {
                max: MAX_OSC99_NOTIFICATION_TYPES,
            });
        }
        if held_edits.saturating_add(chunk.action_edits.len()) > MAX_OSC99_ACTION_EDITS {
            return Err(Osc99Reject::TooManyActionEdits {
                max: MAX_OSC99_ACTION_EDITS,
            });
        }
        let strings = chunk
            .icon_names
            .iter()
            .chain(&chunk.notification_types)
            .fold(0usize, |total, s| {
                total
                    .saturating_add(s.len())
                    .saturating_add(OSC99_LIST_ENTRY_OVERHEAD)
            });
        let edits = chunk
            .action_edits
            .len()
            .saturating_mul(OSC99_LIST_ENTRY_OVERHEAD);
        Ok(strings.saturating_add(edits))
    }

    /// Append `payload` to the accumulator selected by `payload_type`,
    /// decoding it according to `encoding`.
    ///
    /// For `Unknown` (whose payload is discarded) and the control types
    /// `Close`, `Alive` and `Query` nothing is accumulated; only the metadata
    /// is merged (done by [`accept`](Self::accept)).
    ///
    /// # Errors
    ///
    /// Returns the accumulator's [`ChunkError`] (cap exceeded or invalid
    /// base64); the caller drops the whole notification.
    fn push_payload(
        &mut self,
        payload_type: Osc99PayloadType,
        payload: &[u8],
        encoding: Osc99PayloadEncoding,
    ) -> Result<(), ChunkError> {
        let target = match payload_type {
            Osc99PayloadType::Title => &mut self.title,
            Osc99PayloadType::Body => &mut self.body,
            Osc99PayloadType::Icon => &mut self.icon,
            Osc99PayloadType::Buttons => &mut self.buttons,
            // Non-accumulating types. Control payloads never reach the
            // reassembler (they act immediately); an unknown type's payload
            // is discarded and its metadata still applies.
            Osc99PayloadType::Close
            | Osc99PayloadType::Alive
            | Osc99PayloadType::Query
            | Osc99PayloadType::Unknown => {
                return Ok(());
            }
        };
        target.push(payload, chunk_encoding(encoding))
    }

    /// Take one chunk into this accumulation: append its payload, merge its
    /// metadata, and check the combined size.
    ///
    /// The chunk is borrowed: its payload goes straight into the accumulator
    /// and only the metadata that is kept is cloned, so a large final
    /// icon/body chunk is never copied.
    ///
    /// # Errors
    ///
    /// Returns [`Osc99Reject::Chunk`] for an accumulator's [`ChunkError`]
    /// (cap exceeded or invalid base64) or when the payloads plus the list
    /// metadata exceed [`MAX_OSC99_NOTIFICATION_BYTES`], and the
    /// `TooMany...` variants when a list passes its entry cap. The caller
    /// drops the whole notification.
    fn accept(&mut self, chunk: &Osc99Command) -> Result<(), Osc99Reject> {
        self.push_payload(chunk.payload_type, &chunk.payload, chunk.payload_encoding)?;
        let added = self.admit_list_entries(chunk)?;

        let acc = self.meta.get_or_insert_with(blank_osc99_metadata);
        merge_osc99_metadata(acc, chunk);
        self.list_cost = self.list_cost.saturating_add(added);

        if self.cost() > MAX_OSC99_NOTIFICATION_BYTES {
            return Err(ChunkError::TotalTooLarge {
                max: MAX_OSC99_NOTIFICATION_BYTES,
            }
            .into());
        }
        Ok(())
    }

    /// Finish all four accumulators, flushing any pending base64 quantum.
    ///
    /// # Errors
    ///
    /// Returns the first [`ChunkError`] from any accumulator, or
    /// [`ChunkError::TotalTooLarge`] if the flushed payloads together exceed
    /// [`MAX_OSC99_NOTIFICATION_BYTES`]. Flushing a pending 2- or 3-character
    /// base64 tail adds bytes after the per-chunk combined check, so the
    /// combined cap is re-checked here.
    fn finish_payloads(self) -> Result<FinishedPayloads, ChunkError> {
        let payloads = FinishedPayloads {
            title: self.title.finish()?,
            body: self.body.finish()?,
            icon: self.icon.finish()?,
            buttons: self.buttons.finish()?,
        };
        let total = payloads
            .title
            .len()
            .saturating_add(payloads.body.len())
            .saturating_add(payloads.icon.len())
            .saturating_add(payloads.buttons.len());
        if total > MAX_OSC99_NOTIFICATION_BYTES {
            return Err(ChunkError::TotalTooLarge {
                max: MAX_OSC99_NOTIFICATION_BYTES,
            });
        }
        Ok(payloads)
    }
}

// ── Finalized output type ─────────────────────────────────────────────────────

/// A fully-reassembled OSC 99 notification (all chunks concatenated, controls
/// stripped, sizes limited).
///
/// Produced by [`TerminalHandler::reassemble_osc99`] when a terminating
/// (`done == true`) chunk arrives with something to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinalizedNotification {
    /// The notification title. **Never empty**: when no title arrived the
    /// body is promoted into it, and a notification with neither is dropped
    /// before this type is built.
    pub title: String,
    /// The notification body, if any (non-empty when present).
    pub body: Option<String>,
    /// The concatenated icon bytes, if any icon chunks arrived.
    pub icon: Option<Vec<u8>>,
    /// Button labels (`p=buttons`), split from the U+2028-separated payload,
    /// at most eight, none empty.
    pub buttons: Vec<String>,
    /// The metadata merged across every chunk (id, actions, urgency, occasion,
    /// sound, app name, icon names/cache key, close/report flags, expiry).
    pub meta: Osc99Command,
}

/// Map the raw `s=` sound name to an [`Osc99Sound`].
///
/// Unset and the well-known `system` both mean the daemon's default sound;
/// `silent` means no sound; anything else is forwarded verbatim.
pub(in crate::terminal_handler) fn sound_from_name(sound: Option<String>) -> Osc99Sound {
    sound.map_or(Osc99Sound::System, |name| match name.as_str() {
        "system" => Osc99Sound::System,
        "silent" => Osc99Sound::Silent,
        _ => Osc99Sound::Named(name),
    })
}

impl FinalizedNotification {
    /// Map this finalized OSC 99 notification into the transport type
    /// [`Notification99Data`] carried by `WindowManipulation::Notification99`.
    ///
    /// Field mapping: `id`/`title`/`body` direct (the title is non-empty);
    /// `icon_data` from `self.icon`; `buttons` from `self.buttons`;
    /// `icon_names`/`icon_cache_key`/`app_name`/`notification_types` from
    /// `meta`; `activation_reporting`/`activation_focus` from
    /// [`resolve_osc99_actions`] over the merged `meta.action_edits` (default:
    /// focus only); `close_reporting` defaults to `Silent`; `urgency` defaults
    /// to `Normal`; `occasion` defaults to `Always`; `expiry` defaults to
    /// `OsDefault`; `sound` via [`sound_from_name`].
    pub(in crate::terminal_handler) fn into_notification99_data(self) -> Notification99Data {
        let (activation_reporting, activation_focus) =
            resolve_osc99_actions(&self.meta.action_edits);
        Notification99Data {
            id: self.meta.id,
            title: self.title,
            body: self.body,
            icon_data: self.icon,
            icon_names: self.meta.icon_names,
            icon_cache_key: self.meta.icon_cache_key,
            buttons: self.buttons,
            activation_reporting,
            activation_focus,
            close_reporting: self
                .meta
                .close_reporting
                .unwrap_or(Osc99CloseReporting::Silent),
            urgency: self.meta.urgency.unwrap_or(NotificationUrgency::Normal),
            occasion: self.meta.occasion.unwrap_or(NotificationOccasion::Always),
            sound: sound_from_name(self.meta.sound),
            app_name: self.meta.app_name,
            notification_types: self.meta.notification_types,
            expiry: self.meta.expiry.unwrap_or(Osc99Expiry::OsDefault),
        }
    }
}

// ── Capability query reply (Task 130.5) ──────────────────────────────────────

/// Build the capability list of the reply to an OSC 99 `p=?` query, for a host
/// that honours OSC 99 with the given `features`.
///
/// The result is the part after the `;` in
/// `ESC ] 99 ; i=<id> : p=? ; <capabilities> ST`: colon-separated `key=value`
/// pairs in kitty's key order, with no leading or trailing `:`. Every key is
/// derived from `features`, so the reply advertises only what the host
/// genuinely implements:
///
/// - `a=` the supported activation actions, as a comma list in the order
///   `focus,report`. `focus` is present only when activating a notification
///   focuses the source window; `report` only when activation can be reported.
///   With no supported action the `a` key is omitted entirely, per the spec's
///   query table ("If no actions are supported, the `a` key must be absent");
/// - `o=always,unfocused,invisible` (all three occasions, always present);
/// - `u=0,1,2` (urgency levels), present only when urgency is honoured;
/// - `p=` the payload types, in the order
///   `title,body,?,close,icon,alive,buttons`. `title`, `?`, `close` and
///   `alive` are always present; `body`, `icon` and `buttons` are present only
///   when the host displays them;
/// - `c=1` (close reports), present only when close events can be reported;
/// - `w=1` (auto-expiry), present only when the requested expiry is honoured;
/// - `s=` the sound names: `system,silent` when only the well-known names are
///   understood, plus `error,info,question,warn,warning` when the standard
///   names are understood too.
pub(in crate::terminal_handler) fn osc99_query_reply_body(features: Osc99Features) -> String {
    let mut keys: Vec<String> = Vec::with_capacity(7);

    let mut actions: Vec<&str> = Vec::with_capacity(2);
    match features.activation_focus {
        Osc99FocusAction::Supported => actions.push("focus"),
        Osc99FocusAction::Unsupported => {}
    }
    match features.activation_report {
        Osc99ActivationReport::Reported => actions.push("report"),
        Osc99ActivationReport::NotReported => {}
    }
    if !actions.is_empty() {
        keys.push(format!("a={}", actions.join(",")));
    }

    keys.push("o=always,unfocused,invisible".to_owned());

    match features.urgency {
        Osc99UrgencySupport::Supported => keys.push("u=0,1,2".to_owned()),
        Osc99UrgencySupport::Unsupported => {}
    }

    let mut payload_types: Vec<&str> = Vec::with_capacity(7);
    payload_types.push("title");
    match features.body {
        Osc99BodySupport::Supported => payload_types.push("body"),
        Osc99BodySupport::Unsupported => {}
    }
    payload_types.extend(["?", "close"]);
    match features.icons {
        Osc99IconSupport::Supported => payload_types.push("icon"),
        Osc99IconSupport::Unsupported => {}
    }
    payload_types.push("alive");
    match features.buttons {
        Osc99ButtonSupport::Supported => payload_types.push("buttons"),
        Osc99ButtonSupport::Unsupported => {}
    }
    keys.push(format!("p={}", payload_types.join(",")));

    match features.close_events {
        Osc99CloseEvents::Reported => keys.push("c=1".to_owned()),
        Osc99CloseEvents::NotReported => {}
    }

    match features.auto_expiry {
        Osc99AutoExpiry::Supported => keys.push("w=1".to_owned()),
        Osc99AutoExpiry::Unsupported => {}
    }

    keys.push(match features.sound_names {
        Osc99SoundNames::SystemAndSilent => "s=system,silent".to_owned(),
        Osc99SoundNames::Standard => "s=system,silent,error,info,question,warn,warning".to_owned(),
    });

    keys.join(":")
}

// ── Finalisation helpers ──────────────────────────────────────────────────────

/// Whether `c` is removed from notification text: the C0 controls except LF
/// (U+0000–U+0009, U+000B–U+001F), DEL, and the C1 controls (U+007F–U+009F).
///
/// This is kitty's `[\x00-\x09\x0b-\x1f\x7f-\x9f]`.
const fn is_osc99_control(c: char) -> bool {
    matches!(c, '\u{0}'..='\u{9}' | '\u{b}'..='\u{1f}' | '\u{7f}'..='\u{9f}')
}

/// Remove every [`is_osc99_control`] character from `text`. LF is kept.
fn strip_osc99_controls(text: &str) -> String {
    text.chars().filter(|&c| !is_osc99_control(c)).collect()
}

/// Limit `text` to `max_chars` characters (Unicode scalar values), as kitty's
/// `limit_size` does: a longer string keeps its first `max_chars - 1`
/// characters and gains a trailing `…` (U+2026), so the result has exactly
/// `max_chars` characters. A string that already fits is returned unchanged.
fn limit_osc99_text(text: String, max_chars: usize) -> String {
    if text.chars().nth(max_chars).is_none() {
        return text;
    }
    let mut limited: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    limited.push('\u{2026}');
    limited
}

/// Strip then limit one piece of text (title, body).
fn sanitize_osc99_text(text: &str, max_chars: usize) -> String {
    limit_osc99_text(strip_osc99_controls(text), max_chars)
}

/// Strip an optional name, treating an empty result as absent.
fn sanitize_optional_name(name: Option<String>) -> Option<String> {
    name.map(|n| strip_osc99_controls(&n))
        .filter(|n| !n.is_empty())
}

/// Strip each name in a list, dropping those that become empty.
fn sanitize_name_list(names: Vec<String>) -> Vec<String> {
    names
        .into_iter()
        .map(|n| strip_osc99_controls(&n))
        .filter(|n| !n.is_empty())
        .collect()
}

/// Split an accumulated `p=buttons` payload into individual labels.
///
/// kitty separates button labels with U+2028 (LINE SEPARATOR). The payload is
/// decoded as UTF-8 (lossy — never panics) and split on U+2028; empty labels
/// (from leading/trailing/doubled separators) are dropped.
fn split_button_labels(bytes: &[u8]) -> Vec<String> {
    if bytes.is_empty() {
        return Vec::new();
    }
    String::from_utf8_lossy(bytes)
        .split('\u{2028}')
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Sanitise button labels: strip controls, limit each to
/// [`MAX_OSC99_BUTTON_CHARS`], drop labels that became empty, then keep at
/// most [`MAX_OSC99_BUTTONS`].
fn sanitize_button_labels(labels: Vec<String>) -> Vec<String> {
    labels
        .into_iter()
        .map(|label| sanitize_osc99_text(&label, MAX_OSC99_BUTTON_CHARS))
        .filter(|label| !label.is_empty())
        .take(MAX_OSC99_BUTTONS)
        .collect()
}

/// Build a [`FinalizedNotification`] from the finished title/body/icon/
/// buttons bytes and the merged `meta` command.
///
/// Decodes the text lossily, strips control characters from every
/// user-visible string, limits sizes, and promotes the body into the title
/// when there is no title. Returns `None` when there is neither a title nor a
/// body: such a notification is dropped without a reply (spec).
fn build_finalized(
    payloads: FinishedPayloads,
    mut meta: Osc99Command,
) -> Option<FinalizedNotification> {
    let mut title = sanitize_osc99_text(
        &String::from_utf8_lossy(&payloads.title),
        MAX_OSC99_TEXT_CHARS,
    );
    let mut body = sanitize_osc99_text(
        &String::from_utf8_lossy(&payloads.body),
        MAX_OSC99_TEXT_CHARS,
    );
    if title.is_empty() {
        title = std::mem::take(&mut body);
    }
    if title.is_empty() {
        return None;
    }

    meta.app_name = sanitize_optional_name(meta.app_name.take());
    meta.sound = sanitize_optional_name(meta.sound.take());
    meta.icon_names = sanitize_name_list(std::mem::take(&mut meta.icon_names));
    meta.notification_types = sanitize_name_list(std::mem::take(&mut meta.notification_types));

    Some(FinalizedNotification {
        title,
        body: (!body.is_empty()).then_some(body),
        icon: (!payloads.icon.is_empty()).then_some(payloads.icon),
        buttons: sanitize_button_labels(split_button_labels(&payloads.buttons)),
        meta,
    })
}

// ── Pending store ─────────────────────────────────────────────────────────────

/// The state held for one in-flight notification.
#[derive(Debug)]
pub(in crate::terminal_handler) enum PendingEntry {
    /// Chunks are being accumulated; the notification is still valid.
    ///
    /// Boxed so the tombstone variant does not pay for four assemblers.
    Accumulating(Box<PendingNotification>),
    /// The notification was dropped mid-transfer (invalid base64 or a cap
    /// breach). It is remembered as a tombstone so the remaining chunks of
    /// the same transfer are discarded instead of starting a fresh
    /// accumulation from a tail fragment. The terminating (`d=1`) chunk
    /// removes it. A tombstone is an ordinary entry for eviction purposes, so
    /// the store stays bounded.
    Dropped,
}

/// A [`PendingEntry`] plus the logical time it was last touched and the bytes
/// it is charged for.
#[derive(Debug)]
struct PendingSlot {
    /// The in-flight state.
    entry: PendingEntry,
    /// The store's clock at the last touch; the smallest value is evicted
    /// first.
    last_touched: u64,
    /// Bytes charged against the store's budget: the entry's
    /// [`PendingNotification::cost`] as of the last accepted chunk, and zero
    /// for a tombstone.
    cost: usize,
}

impl PendingSlot {
    /// An empty accumulation touched at `now`.
    fn new(now: u64) -> Self {
        Self {
            entry: PendingEntry::Accumulating(Box::default()),
            last_touched: now,
            cost: 0,
        }
    }
}

/// Which slot of a [`PendingOsc99`] store.
#[derive(Debug)]
enum SlotKey {
    /// The entry for an `i=` id.
    Identified(String),
    /// The single entry for chunks without an id.
    Unidentified,
}

/// The in-flight OSC 99 notifications of one terminal handler.
///
/// Identified notifications live in a map keyed by `i=`, bounded at
/// [`MAX_PENDING_OSC99_NOTIFICATIONS`] with least-recently-touched eviction
/// (tombstones included), so the map never refuses a chunk. Content chunks
/// without an id share the single `unidentified` slot, like kitty's one
/// pending command keyed by "no id".
///
/// The bytes held by every entry together are bounded by
/// [`MAX_OSC99_PENDING_TOTAL_BYTES`]. `total_cost` is the sum of every slot's
/// `cost`, maintained on each insert, accepted chunk, removal, eviction and
/// clear rather than recomputed.
#[derive(Debug)]
pub(in crate::terminal_handler) struct PendingOsc99 {
    /// Identified entries.
    by_id: HashMap<String, PendingSlot>,
    /// Monotonic touch counter; incremented on every touch of an entry.
    clock: u64,
    /// The single entry for chunks that carry no id.
    unidentified: Option<PendingSlot>,
    /// The sum of every slot's `cost`.
    total_cost: usize,
    /// The most `total_cost` may reach.
    budget: usize,
}

impl Default for PendingOsc99 {
    fn default() -> Self {
        Self {
            by_id: HashMap::new(),
            clock: 0,
            unidentified: None,
            total_cost: 0,
            budget: MAX_OSC99_PENDING_TOTAL_BYTES,
        }
    }
}

impl PendingOsc99 {
    /// A store with a custom byte budget.
    #[cfg(test)]
    fn with_budget(budget: usize) -> Self {
        Self {
            budget,
            ..Self::default()
        }
    }

    /// Advance and return the clock.
    const fn tick(&mut self) -> u64 {
        self.clock = self.clock.saturating_add(1);
        self.clock
    }

    /// Remove the identified entry `id`, releasing its cost.
    fn remove_identified(&mut self, id: &str) -> Option<PendingSlot> {
        let slot = self.by_id.remove(id)?;
        self.total_cost = self.total_cost.saturating_sub(slot.cost);
        Some(slot)
    }

    /// Remove the unidentified entry, releasing its cost.
    fn take_unidentified(&mut self) -> Option<PendingSlot> {
        let slot = self.unidentified.take()?;
        self.total_cost = self.total_cost.saturating_sub(slot.cost);
        Some(slot)
    }

    /// Remove the identified entry with the smallest `last_touched`.
    fn evict_least_recently_touched(&mut self) {
        let oldest = self
            .by_id
            .iter()
            .min_by_key(|(_, slot)| slot.last_touched)
            .map(|(id, _)| id.clone());
        if let Some(oldest) = oldest {
            self.remove_identified(&oldest);
        }
    }

    /// Whether `id` currently holds a tombstone. A tombstone that is looked
    /// up counts as touched: its transfer is still in progress.
    fn is_tombstone(&mut self, id: Option<&str>) -> bool {
        let now = self.tick();
        let slot = match id {
            Some(id) => self.by_id.get_mut(id),
            None => self.unidentified.as_mut(),
        };
        slot.is_some_and(|slot| {
            let dropped = matches!(slot.entry, PendingEntry::Dropped);
            if dropped {
                slot.last_touched = now;
            }
            dropped
        })
    }

    /// The slot for `id` (`None` is the unidentified slot), created as an
    /// empty accumulation when absent, and marked as touched.
    ///
    /// Inserting a new id into a full map first evicts the
    /// least-recently-touched entry, tombstones included.
    fn slot_mut(&mut self, id: Option<&str>) -> &mut PendingSlot {
        let now = self.tick();
        let Some(id) = id else {
            let slot = self
                .unidentified
                .get_or_insert_with(|| PendingSlot::new(now));
            slot.last_touched = now;
            return slot;
        };
        if !self.by_id.contains_key(id) && self.by_id.len() >= MAX_PENDING_OSC99_NOTIFICATIONS {
            self.evict_least_recently_touched();
        }
        let slot = self
            .by_id
            .entry(id.to_owned())
            .or_insert_with(|| PendingSlot::new(now));
        slot.last_touched = now;
        slot
    }

    /// Feed one content chunk into the accumulation for `id` and keep the
    /// store within its byte budget.
    ///
    /// The accumulation is created when absent. A tombstone ignores the chunk.
    /// When accepting the chunk pushes the store past its budget, the
    /// least-recently-touched entries *other than* `id` that hold bytes are
    /// evicted until it fits; the budget is checked after the chunk lands, so
    /// it can be overshot transiently by at most one chunk.
    ///
    /// # Errors
    ///
    /// Returns the [`Osc99Reject`] of the notification's own caps, or
    /// [`Osc99Reject::PendingBudget`] when `id`'s accumulation alone exceeds
    /// the budget (nothing else is evicted in that case). The caller then
    /// abandons the transfer.
    fn accept(&mut self, id: Option<&str>, chunk: &Osc99Command) -> Result<(), Osc99Reject> {
        let slot = self.slot_mut(id);
        let PendingEntry::Accumulating(entry) = &mut slot.entry else {
            return Ok(());
        };
        entry.accept(chunk)?;
        let new_cost = entry.cost();
        let old_cost = std::mem::replace(&mut slot.cost, new_cost);
        self.total_cost = self
            .total_cost
            .saturating_sub(old_cost)
            .saturating_add(new_cost);
        self.enforce_budget(id, new_cost)
    }

    /// Evict until `total_cost` fits the budget, never touching `keep`.
    fn enforce_budget(&mut self, keep: Option<&str>, keep_cost: usize) -> Result<(), Osc99Reject> {
        let over = Osc99Reject::PendingBudget { max: self.budget };
        if keep_cost > self.budget {
            return Err(over);
        }
        while self.total_cost > self.budget {
            let Some(victim) = self.least_recently_touched_other(keep) else {
                return Err(over);
            };
            match victim {
                SlotKey::Identified(id) => {
                    self.remove_identified(&id);
                }
                SlotKey::Unidentified => {
                    self.take_unidentified();
                }
            }
        }
        Ok(())
    }

    /// The least-recently-touched slot other than `keep` that holds bytes
    /// (evicting a zero-cost tombstone would free nothing).
    fn least_recently_touched_other(&self, keep: Option<&str>) -> Option<SlotKey> {
        let identified = self
            .by_id
            .iter()
            .filter(|(id, slot)| slot.cost > 0 && keep != Some(id.as_str()))
            .min_by_key(|(_, slot)| slot.last_touched)
            .map(|(id, slot)| (slot.last_touched, SlotKey::Identified(id.clone())));
        let unidentified = self
            .unidentified
            .as_ref()
            .filter(|slot| slot.cost > 0 && keep.is_some())
            .map(|slot| (slot.last_touched, SlotKey::Unidentified));
        [identified, unidentified]
            .into_iter()
            .flatten()
            .min_by_key(|(touched, _)| *touched)
            .map(|(_, key)| key)
    }

    /// Whether an entry (accumulating or tombstone) exists for `id`.
    fn contains(&self, id: Option<&str>) -> bool {
        id.map_or_else(
            || self.unidentified.is_some(),
            |id| self.by_id.contains_key(id),
        )
    }

    /// Replace the entry for `id` with a tombstone, releasing its cost.
    fn tombstone(&mut self, id: Option<&str>) {
        let slot = self.slot_mut(id);
        slot.entry = PendingEntry::Dropped;
        let released = std::mem::take(&mut slot.cost);
        self.total_cost = self.total_cost.saturating_sub(released);
    }

    /// Remove and return the entry for `id`, releasing its cost.
    fn remove(&mut self, id: Option<&str>) -> Option<PendingEntry> {
        let slot = match id {
            Some(id) => self.remove_identified(id),
            None => self.take_unidentified(),
        };
        slot.map(|slot| slot.entry)
    }

    /// Discard every in-flight notification, identified or not.
    pub(in crate::terminal_handler) fn clear(&mut self) {
        self.by_id.clear();
        self.unidentified = None;
        self.total_cost = 0;
    }

    /// Whether nothing is in flight.
    #[cfg(test)]
    pub(in crate::terminal_handler) fn is_empty(&self) -> bool {
        self.by_id.is_empty() && self.unidentified.is_none()
    }

    /// The number of identified entries (tombstones included).
    #[cfg(test)]
    pub(in crate::terminal_handler) fn identified_len(&self) -> usize {
        self.by_id.len()
    }

    /// The entry for `id` (`None` is the unidentified slot), without touching it.
    #[cfg(test)]
    pub(in crate::terminal_handler) fn get(&self, id: Option<&str>) -> Option<&PendingEntry> {
        id.map_or(self.unidentified.as_ref(), |id| self.by_id.get(id))
            .map(|slot| &slot.entry)
    }

    /// The bytes currently charged against the budget.
    #[cfg(test)]
    pub(in crate::terminal_handler) const fn total_cost(&self) -> usize {
        self.total_cost
    }

    /// Recompute every cost from the entries and assert the running counters
    /// agree: each slot's `cost` against its entry, and `total_cost` against
    /// the sum.
    #[cfg(test)]
    pub(in crate::terminal_handler) fn assert_costs_consistent(&self) {
        let mut sum = 0usize;
        for slot in self.by_id.values().chain(self.unidentified.as_ref()) {
            let recomputed = match &slot.entry {
                PendingEntry::Accumulating(entry) => entry.cost(),
                PendingEntry::Dropped => 0,
            };
            assert_eq!(slot.cost, recomputed, "slot cost must match its entry");
            sum += recomputed;
        }
        assert_eq!(self.total_cost, sum, "total cost must match the sum");
        assert!(self.total_cost <= self.budget, "budget must hold");
    }
}

// ── Handling on TerminalHandler ───────────────────────────────────────────────

/// A short, bounded, log-safe description of a notification key.
fn log_key(id: Option<&str>) -> String {
    id.map_or_else(
        || "<unidentified>".to_owned(),
        |id| lossy_sequence_for_log_bounded(id.as_bytes()),
    )
}

impl TerminalHandler {
    /// Handle one parsed OSC 99 chunk.
    ///
    /// While OSC 99 is unsupported by the host
    /// ([`TerminalHandler::host_capabilities`]) the chunk is dropped BEFORE
    /// anything else, with a single payload-free debug line, so the terminal
    /// looks like one that does not speak the protocol and no state
    /// accumulates for a transfer that could complete after the host later
    /// enables it.
    ///
    /// While supported, control payloads act at once whatever `d=` says, and
    /// never read or change the pending store:
    ///
    /// - `p=?` is answered here from the host capabilities (Task 130.5) and
    ///   never reaches the GUI; an id-less query is answered with `i=0`.
    /// - `p=close` and `p=alive` with an id are forwarded as `Osc99Control`;
    ///   without an id they are no-ops.
    ///
    /// Everything else is content and goes through
    /// [`reassemble_osc99`](Self::reassemble_osc99); a finalised notification
    /// is pushed as `Notification99`.
    pub(in crate::terminal_handler) fn handle_osc99(&mut self, cmd: &Osc99Command) {
        let Osc99Support::Supported(features) = self.host_capabilities().osc99 else {
            tracing::debug!("OSC 99 request dropped: OSC 99 is unsupported");
            return;
        };

        match cmd.payload_type {
            Osc99PayloadType::Query => {
                let id = osc99_reply_id(cmd.id.as_deref());
                let caps = osc99_query_reply_body(features);
                self.write_osc_response(&format!("99;i={id}:p=?;{caps}"));
            }
            Osc99PayloadType::Close => self.forward_osc99_control(cmd, Osc99ControlKind::Close),
            Osc99PayloadType::Alive => self.forward_osc99_control(cmd, Osc99ControlKind::Alive),
            Osc99PayloadType::Title
            | Osc99PayloadType::Body
            | Osc99PayloadType::Icon
            | Osc99PayloadType::Buttons
            | Osc99PayloadType::Unknown => {
                if let Some(finalized) = self.reassemble_osc99(cmd) {
                    self.window_commands
                        .push(WindowManipulation::Notification99(Box::new(
                            finalized.into_notification99_data(),
                        )));
                }
            }
        }
    }

    /// Forward a `p=close` / `p=alive` request to the GUI.
    ///
    /// Without an id there is nothing to refer to (spec: an id-less
    /// `p=close` is ignored), so nothing is sent.
    fn forward_osc99_control(&mut self, cmd: &Osc99Command, kind: Osc99ControlKind) {
        let Some(id) = &cmd.id else {
            tracing::trace!("OSC 99: ignoring a control request without an id");
            return;
        };
        self.window_commands.push(WindowManipulation::Osc99Control {
            id: id.clone(),
            kind,
        });
    }

    /// Abandon the in-flight notification `id` after a decode or cap failure.
    ///
    /// If the offending chunk was the terminating one the notification is
    /// over and the entry is removed. Otherwise the key is kept as a
    /// [`PendingEntry::Dropped`] tombstone so the remaining chunks of the same
    /// transfer cannot start a fresh, truncated notification.
    fn abandon_notification(&mut self, id: Option<&str>, chunk_state: Osc99ChunkState) {
        match chunk_state {
            Osc99ChunkState::Done => {
                self.pending_osc99.remove(id);
            }
            Osc99ChunkState::More => self.pending_osc99.tombstone(id),
        }
    }

    /// Feed one parsed OSC 99 content chunk into the notification reassembly
    /// machine.
    ///
    /// Returns `Some(finalized)` when the chunk terminates a notification
    /// (`d=1` or no `d=`) that has something to show, where `finalized`
    /// carries the sanitised title/body/buttons and the metadata merged over
    /// every chunk. Returns `None` while more chunks are expected, when the
    /// transfer was dropped, or when the finished notification has neither a
    /// title nor a body.
    ///
    /// Chunks are keyed by `i=`; chunks without one share a single
    /// unidentified slot. An id seen again after finalisation starts a fresh
    /// accumulation.
    ///
    /// A notification dropped mid-transfer (invalid base64, or a size cap)
    /// leaves a tombstone for its key: further chunks are ignored and the
    /// terminating chunk removes the tombstone without emitting anything.
    ///
    /// A notification that passes a list cap, its byte cap, or (alone) the
    /// handler-wide pending budget is dropped the same way. A chunk that
    /// pushes the pending store past its budget evicts other entries, least
    /// recently touched first.
    ///
    /// The chunk is borrowed (the parser's event stream owns it): its payload
    /// goes straight into the accumulator and only the metadata that is kept
    /// is cloned.
    ///
    /// Control payloads are handled by [`handle_osc99`](Self::handle_osc99)
    /// and never get here.
    pub(in crate::terminal_handler) fn reassemble_osc99(
        &mut self,
        chunk: &Osc99Command,
    ) -> Option<FinalizedNotification> {
        let key = chunk.id.as_deref();
        let chunk_state = chunk.chunk_state;
        let done = chunk_state == Osc99ChunkState::Done;

        if self.pending_osc99.is_tombstone(key) {
            // The rest of a dropped transfer: ignore the payload; the
            // terminating chunk ends the notification.
            if done {
                self.pending_osc99.remove(key);
            }
            tracing::trace!("OSC 99: ignoring chunk of a dropped notification");
            return None;
        }

        // A complete notification with no pending entry cannot be followed by
        // more chunks, so it is finalised without touching the store: on a
        // full map it must not evict another key's in-progress transfer.
        if done && !self.pending_osc99.contains(key) {
            let mut entry = PendingNotification::default();
            if let Err(err) = entry.accept(chunk) {
                tracing::debug!("OSC 99: dropping notification {}: {err}", log_key(key));
                return None;
            }
            let meta = entry.meta.take()?;
            return match entry.finish_payloads() {
                Ok(payloads) => build_finalized(payloads, meta),
                Err(err) => {
                    tracing::debug!("OSC 99: dropping notification {}: {err}", log_key(key));
                    None
                }
            };
        }

        // An invalid base64 stream or an exceeded cap drops the whole
        // in-flight notification rather than keeping a partial one.
        if let Err(err) = self.pending_osc99.accept(key, chunk) {
            self.abandon_notification(key, chunk_state);
            tracing::debug!("OSC 99: dropping notification {}: {err}", log_key(key));
            return None;
        }

        if !done {
            return None;
        }

        let Some(PendingEntry::Accumulating(mut entry)) = self.pending_osc99.remove(key) else {
            return None;
        };
        // `accept` stored the metadata, so it is present for a live entry.
        let meta = entry.meta.take()?;
        match entry.finish_payloads() {
            Ok(payloads) => build_finalized(payloads, meta),
            Err(err) => {
                tracing::debug!("OSC 99: dropping notification {}: {err}", log_key(key));
                None
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use freminal_common::buffer_states::osc_notify_99::{
        Osc99Action, Osc99ActionEdit, parse_osc_99,
    };
    use freminal_common::buffer_states::osc99_notification::{
        Osc99ActivationFocus, Osc99ActivationReporting,
    };
    use freminal_common::host_capabilities::{
        Osc99AutoExpiry, Osc99BodySupport, Osc99ButtonSupport, Osc99FocusAction, Osc99IconSupport,
        Osc99SoundNames, Osc99UrgencySupport,
    };
    use std::num::NonZeroU32;

    /// Build a default `Osc99Command` for use in mapping tests.
    fn default_cmd() -> Osc99Command {
        Osc99Command {
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

    /// Build a default `FinalizedNotification` wrapping `default_cmd()`.
    fn default_finalized() -> FinalizedNotification {
        FinalizedNotification {
            title: "t".to_owned(),
            body: None,
            icon: None,
            buttons: Vec::new(),
            meta: default_cmd(),
        }
    }

    #[test]
    fn urgency_critical_passes_through() {
        let finalized = FinalizedNotification {
            meta: Osc99Command {
                urgency: Some(NotificationUrgency::Critical),
                ..default_cmd()
            },
            ..default_finalized()
        };
        let data = finalized.into_notification99_data();
        assert_eq!(data.urgency, NotificationUrgency::Critical);
    }

    #[test]
    fn urgency_low_and_normal_pass_through() {
        let low = FinalizedNotification {
            meta: Osc99Command {
                urgency: Some(NotificationUrgency::Low),
                ..default_cmd()
            },
            ..default_finalized()
        };
        assert_eq!(
            low.into_notification99_data().urgency,
            NotificationUrgency::Low
        );

        let normal = FinalizedNotification {
            meta: Osc99Command {
                urgency: Some(NotificationUrgency::Normal),
                ..default_cmd()
            },
            ..default_finalized()
        };
        assert_eq!(
            normal.into_notification99_data().urgency,
            NotificationUrgency::Normal
        );
    }

    #[test]
    fn urgency_unset_maps_to_normal() {
        let finalized = default_finalized();
        let data = finalized.into_notification99_data();
        assert_eq!(data.urgency, NotificationUrgency::Normal);
    }

    #[test]
    fn occasion_passes_through_unchanged() {
        for occasion in [
            NotificationOccasion::Always,
            NotificationOccasion::Unfocused,
            NotificationOccasion::Invisible,
        ] {
            let finalized = FinalizedNotification {
                meta: Osc99Command {
                    occasion: Some(occasion),
                    ..default_cmd()
                },
                ..default_finalized()
            };
            assert_eq!(finalized.into_notification99_data().occasion, occasion);
        }
    }

    #[test]
    fn expiry_mapped_into_notification99_data() {
        let finalized = FinalizedNotification {
            meta: Osc99Command {
                expiry: Some(Osc99Expiry::After(NonZeroU32::new(5000).unwrap())),
                ..default_cmd()
            },
            ..default_finalized()
        };
        assert_eq!(
            finalized.into_notification99_data().expiry,
            Osc99Expiry::After(NonZeroU32::new(5000).unwrap())
        );
    }

    #[test]
    fn sound_unset_and_system_map_to_system() {
        assert_eq!(sound_from_name(None), Osc99Sound::System);
        assert_eq!(
            sound_from_name(Some("system".to_owned())),
            Osc99Sound::System
        );
    }

    #[test]
    fn sound_silent_maps_to_silent() {
        assert_eq!(
            sound_from_name(Some("silent".to_owned())),
            Osc99Sound::Silent
        );
    }

    #[test]
    fn sound_other_maps_to_named() {
        assert_eq!(
            sound_from_name(Some("bell".to_owned())),
            Osc99Sound::Named("bell".to_owned())
        );
    }

    #[test]
    fn sound_mapped_into_notification99_data() {
        let finalized = FinalizedNotification {
            meta: Osc99Command {
                sound: Some("silent".to_owned()),
                ..default_cmd()
            },
            ..default_finalized()
        };
        assert_eq!(
            finalized.into_notification99_data().sound,
            Osc99Sound::Silent
        );
    }

    #[test]
    fn title_body_icon_and_flags_carried_through() {
        let finalized = FinalizedNotification {
            title: "Title".to_owned(),
            body: Some("Body".to_owned()),
            icon: Some(vec![1, 2, 3]),
            buttons: Vec::new(),
            meta: Osc99Command {
                id: Some("id-1".to_owned()),
                close_reporting: Some(Osc99CloseReporting::Report),
                action_edits: vec![
                    Osc99ActionEdit::Add(Osc99Action::Report),
                    Osc99ActionEdit::Remove(Osc99Action::Focus),
                ],
                ..default_cmd()
            },
        };
        let data = finalized.into_notification99_data();
        assert_eq!(data.id.as_deref(), Some("id-1"));
        assert_eq!(data.title, "Title");
        assert_eq!(data.body.as_deref(), Some("Body"));
        assert_eq!(data.icon_data.as_deref(), Some(&[1u8, 2, 3][..]));
        assert_eq!(data.activation_reporting, Osc99ActivationReporting::Report);
        assert_eq!(data.activation_focus, Osc99ActivationFocus::NoFocus);
        assert_eq!(data.close_reporting, Osc99CloseReporting::Report);
        assert_eq!(data.buttons, [] as [String; 0]);
    }

    #[test]
    fn default_flags_map_to_silent_and_focus() {
        let data = default_finalized().into_notification99_data();
        assert_eq!(data.activation_reporting, Osc99ActivationReporting::Silent);
        assert_eq!(data.activation_focus, Osc99ActivationFocus::Focus);
        assert_eq!(data.close_reporting, Osc99CloseReporting::Silent);
    }

    #[test]
    fn action_edits_resolve_from_the_focus_only_default() {
        let finalized = FinalizedNotification {
            meta: Osc99Command {
                action_edits: vec![Osc99ActionEdit::Add(Osc99Action::Report)],
                ..default_cmd()
            },
            ..default_finalized()
        };
        let data = finalized.into_notification99_data();
        assert_eq!(data.activation_reporting, Osc99ActivationReporting::Report);
        assert_eq!(data.activation_focus, Osc99ActivationFocus::Focus);
    }

    #[test]
    fn close_reporting_unset_is_silent_and_explicit_values_pass_through() {
        assert_eq!(
            default_finalized()
                .into_notification99_data()
                .close_reporting,
            Osc99CloseReporting::Silent
        );
        let reported = FinalizedNotification {
            meta: Osc99Command {
                close_reporting: Some(Osc99CloseReporting::Report),
                ..default_cmd()
            },
            ..default_finalized()
        };
        assert_eq!(
            reported.into_notification99_data().close_reporting,
            Osc99CloseReporting::Report
        );
    }

    #[test]
    fn unset_occasion_and_expiry_take_their_defaults() {
        let data = default_finalized().into_notification99_data();
        assert_eq!(data.occasion, NotificationOccasion::Always);
        assert_eq!(data.expiry, Osc99Expiry::OsDefault);
    }

    #[test]
    fn notification_types_are_carried_through() {
        let finalized = FinalizedNotification {
            meta: Osc99Command {
                notification_types: vec!["alert".to_owned(), "info".to_owned()],
                ..default_cmd()
            },
            ..default_finalized()
        };
        assert_eq!(
            finalized.into_notification99_data().notification_types,
            vec!["alert".to_owned(), "info".to_owned()]
        );
    }

    /// An unknown `p=` type discards its payload, but still finalizes on `d=1`
    /// with the content accumulated earlier, and its metadata still applies.
    #[test]
    fn unknown_payload_type_discards_payload_but_keeps_metadata() {
        let mut handler = TerminalHandler::new(80, 24);
        let first = Osc99Command {
            id: Some("u".to_owned()),
            chunk_state: Osc99ChunkState::More,
            payload: b"Title".to_vec(),
            ..default_cmd()
        };
        assert!(handler.reassemble_osc99(&first).is_none());
        let cmd = Osc99Command {
            id: Some("u".to_owned()),
            payload_type: Osc99PayloadType::Unknown,
            payload: b"ignored".to_vec(),
            urgency: Some(NotificationUrgency::Critical),
            ..default_cmd()
        };
        let finalized = handler
            .reassemble_osc99(&cmd)
            .expect("a done unknown chunk finalizes the accumulated content");
        assert_eq!(finalized.title, "Title");
        assert_eq!(finalized.body, None);
        assert_eq!(finalized.icon, None);
        assert_eq!(finalized.meta.urgency, Some(NotificationUrgency::Critical));
    }

    /// An unknown-typed final chunk with no accumulated content finalizes
    /// nothing: there is neither a title nor a body.
    #[test]
    fn unknown_payload_type_alone_finalizes_nothing() {
        let mut handler = TerminalHandler::new(80, 24);
        let cmd = Osc99Command {
            id: Some("u".to_owned()),
            payload_type: Osc99PayloadType::Unknown,
            payload: b"ignored".to_vec(),
            ..default_cmd()
        };
        assert!(handler.reassemble_osc99(&cmd).is_none());
        assert!(handler.pending_osc99.is_empty());
    }

    #[test]
    fn unknown_payload_type_chunk_with_d0_stays_pending_without_content() {
        let mut handler = TerminalHandler::new(80, 24);
        let cmd = Osc99Command {
            id: Some("u".to_owned()),
            payload_type: Osc99PayloadType::Unknown,
            chunk_state: Osc99ChunkState::More,
            payload: b"ignored".to_vec(),
            ..default_cmd()
        };
        assert!(handler.reassemble_osc99(&cmd).is_none());
        let Some(PendingEntry::Accumulating(entry)) = handler.pending_osc99.get(Some("u")) else {
            panic!("entry should be accumulating");
        };
        assert_eq!(entry.accumulated_len(), 0);
    }

    // ── 99.9: button-label extraction ────────────────────────────────────────

    #[test]
    fn split_button_labels_splits_on_u2028() {
        assert_eq!(
            split_button_labels("OK\u{2028}Cancel".as_bytes()),
            vec!["OK".to_owned(), "Cancel".to_owned()]
        );
    }

    #[test]
    fn split_button_labels_empty_input_yields_empty_vec() {
        assert_eq!(split_button_labels(b""), [] as [String; 0]);
    }

    #[test]
    fn split_button_labels_drops_leading_trailing_and_doubled_separators() {
        // Leading, trailing, and doubled U+2028 separators must not produce
        // empty-string labels.
        let input = "\u{2028}OK\u{2028}\u{2028}Cancel\u{2028}".as_bytes();
        assert_eq!(
            split_button_labels(input),
            vec!["OK".to_owned(), "Cancel".to_owned()]
        );
    }

    /// Standalone (no-id) `Buttons` chunk completing a notification that has
    /// a title finalizes with the split labels.
    #[test]
    fn standalone_done_buttons_no_id_splits_labels() {
        let mut handler = TerminalHandler::new(80, 24);
        let title = Osc99Command {
            chunk_state: Osc99ChunkState::More,
            payload: b"T".to_vec(),
            ..default_cmd()
        };
        assert!(handler.reassemble_osc99(&title).is_none());
        let cmd = Osc99Command {
            payload_type: Osc99PayloadType::Buttons,
            payload: "OK\u{2028}Cancel".as_bytes().to_vec(),
            ..default_cmd()
        };
        let finalized = handler
            .reassemble_osc99(&cmd)
            .expect("single done Buttons chunk must finalize");
        assert_eq!(
            finalized.buttons,
            vec!["OK".to_owned(), "Cancel".to_owned()]
        );
    }

    /// Two `Buttons` chunks sharing an id, split across the U+2028 boundary,
    /// concatenate correctly before splitting.
    #[test]
    fn chunked_buttons_by_id_concatenate_across_separator_boundary() {
        let mut handler = TerminalHandler::new(80, 24);

        let title = Osc99Command {
            id: Some("btn-1".to_owned()),
            chunk_state: Osc99ChunkState::More,
            payload: b"T".to_vec(),
            ..default_cmd()
        };
        assert!(handler.reassemble_osc99(&title).is_none());

        let chunk1 = Osc99Command {
            id: Some("btn-1".to_owned()),
            payload_type: Osc99PayloadType::Buttons,
            chunk_state: Osc99ChunkState::More,
            payload: "Yes\u{2028}N".as_bytes().to_vec(),
            ..default_cmd()
        };
        let r1 = handler.reassemble_osc99(&chunk1);
        assert!(r1.is_none(), "first chunk must not finalize");

        let chunk2 = Osc99Command {
            id: Some("btn-1".to_owned()),
            payload_type: Osc99PayloadType::Buttons,
            chunk_state: Osc99ChunkState::Done,
            payload: b"o".to_vec(),
            ..default_cmd()
        };
        let finalized = handler
            .reassemble_osc99(&chunk2)
            .expect("terminating chunk must finalize");
        assert_eq!(finalized.buttons, vec!["Yes".to_owned(), "No".to_owned()]);
    }

    #[test]
    fn buttons_mapped_into_notification99_data() {
        let finalized = FinalizedNotification {
            buttons: vec!["A".to_owned(), "B".to_owned()],
            ..default_finalized()
        };
        let data = finalized.into_notification99_data();
        assert_eq!(data.buttons, vec!["A".to_owned(), "B".to_owned()]);
    }

    // ── Reassembly bounds (memory-growth guards) ──────────────────────────────

    /// A chunk with a new id always reaches the store: at the cap the
    /// least-recently-touched entry is evicted instead of the chunk refused.
    #[test]
    fn reassembly_evicts_the_least_recently_touched_id_at_the_cap() {
        let mut handler = TerminalHandler::new(80, 24);
        let chunk_for = |id: String, payload: &[u8]| Osc99Command {
            id: Some(id),
            payload_type: Osc99PayloadType::Title,
            chunk_state: Osc99ChunkState::More,
            payload: payload.to_vec(),
            ..default_cmd()
        };

        for i in 0..MAX_PENDING_OSC99_NOTIFICATIONS {
            assert!(
                handler
                    .reassemble_osc99(&chunk_for(format!("id-{i}"), b"x"))
                    .is_none()
            );
        }
        assert_eq!(
            handler.pending_osc99.identified_len(),
            MAX_PENDING_OSC99_NOTIFICATIONS
        );

        // Touching `id-0` makes `id-1` the oldest.
        assert!(
            handler
                .reassemble_osc99(&chunk_for("id-0".to_owned(), b"y"))
                .is_none()
        );

        // One more distinct id is accepted; the map does not grow.
        assert!(
            handler
                .reassemble_osc99(&chunk_for("overflow".to_owned(), b"x"))
                .is_none()
        );
        assert_eq!(
            handler.pending_osc99.identified_len(),
            MAX_PENDING_OSC99_NOTIFICATIONS
        );
        assert!(handler.pending_osc99.get(Some("overflow")).is_some());
        assert!(handler.pending_osc99.get(Some("id-1")).is_none());
        assert!(handler.pending_osc99.get(Some("id-0")).is_some());

        // The protected id still finalizes with everything it accumulated.
        let finish = Osc99Command {
            id: Some("id-0".to_owned()),
            payload_type: Osc99PayloadType::Title,
            chunk_state: Osc99ChunkState::Done,
            payload: b"z".to_vec(),
            ..default_cmd()
        };
        let finalized = handler.reassemble_osc99(&finish).expect("finalizes");
        assert_eq!(finalized.title, "xyz");
    }

    #[test]
    fn reassembly_drops_notification_exceeding_byte_cap() {
        let mut handler = TerminalHandler::new(80, 24);
        let id = "big".to_owned();

        // First chunk under the cap: accepted, still pending.
        let chunk1 = Osc99Command {
            id: Some(id.clone()),
            payload_type: Osc99PayloadType::Body,
            chunk_state: Osc99ChunkState::More,
            payload: vec![b'a'; 1024],
            ..default_cmd()
        };
        assert!(handler.reassemble_osc99(&chunk1).is_none());
        assert!(handler.pending_osc99.get(Some(&id)).is_some());

        // Second chunk that would push the accumulation past the cap: the
        // whole in-flight notification is dropped rather than grown.
        let chunk2 = Osc99Command {
            id: Some(id.clone()),
            payload_type: Osc99PayloadType::Body,
            chunk_state: Osc99ChunkState::More,
            payload: vec![b'b'; MAX_OSC99_NOTIFICATION_BYTES],
            ..default_cmd()
        };
        assert!(handler.reassemble_osc99(&chunk2).is_none());
        // The id stays as a tombstone until the terminating chunk.
        assert!(is_dropped(&handler, &id));
    }

    #[test]
    fn reassembly_does_not_retain_payload_bytes_twice_in_meta() {
        let mut handler = TerminalHandler::new(80, 24);
        let id = "meta".to_owned();

        let chunk = Osc99Command {
            id: Some(id.clone()),
            payload_type: Osc99PayloadType::Body,
            chunk_state: Osc99ChunkState::More,
            payload: b"hello".to_vec(),
            ..default_cmd()
        };
        assert!(handler.reassemble_osc99(&chunk).is_none());

        // The stored metadata must not keep a copy of the payload bytes: they
        // live only in the body accumulator now.
        let Some(PendingEntry::Accumulating(entry)) = handler.pending_osc99.get(Some(&id)) else {
            panic!("entry should be accumulating");
        };
        assert_eq!(entry.body.len(), 5);
        assert!(
            entry.meta.as_ref().is_some_and(|m| m.payload.is_empty()),
            "meta.payload must be cleared after accumulation"
        );

        // And the accumulated bytes are the payload itself.
        let done = Osc99Command {
            id: Some(id),
            payload_type: Osc99PayloadType::Body,
            chunk_state: Osc99ChunkState::Done,
            payload: Vec::new(),
            ..default_cmd()
        };
        let finalized = handler
            .reassemble_osc99(&done)
            .expect("terminating chunk finalizes");
        // A body-only notification is promoted into the title.
        assert_eq!(finalized.title, "hello");
        assert_eq!(finalized.body, None);
    }

    // ── 129.12: stream decoding through the real wire path ───────────────────

    /// Feed one OSC 99 sequence (`metadata`, `payload`) through
    /// `parse_osc_99` and the reassembler, exactly as the handler does.
    fn feed(
        handler: &mut TerminalHandler,
        metadata: &str,
        payload: &[u8],
    ) -> Option<FinalizedNotification> {
        let cmd = parse_osc_99(metadata.as_bytes(), payload).expect("sequence must parse");
        handler.reassemble_osc99(&cmd)
    }

    /// Reassemble a two-chunk `p=title` notification whose payload slices are
    /// `first` and `second`, both declared base64.
    fn two_base64_title_chunks(first: &[u8], second: &[u8]) -> Option<FinalizedNotification> {
        let mut handler = TerminalHandler::new(80, 24);
        assert!(feed(&mut handler, "i=x:d=0:e=1", first).is_none());
        feed(&mut handler, "i=x:e=1", second)
    }

    const SAMPLE_TITLE: &str = "Build finished: 42 tests passed (naïve ✓)";

    /// One base64 encoding of the title, split into two chunks at every
    /// offset (including mid-quantum), must reassemble exactly.
    #[test]
    fn base64_title_split_at_every_offset_reassembles() {
        let encoded = freminal_common::base64::encode(SAMPLE_TITLE.as_bytes());
        for at in 0..=encoded.len() {
            let (a, b) = encoded.as_bytes().split_at(at);
            let finalized = two_base64_title_chunks(a, b)
                .unwrap_or_else(|| panic!("split at {at} must finalize"));
            assert_eq!(finalized.title, SAMPLE_TITLE, "split at {at}");
        }
    }

    /// The final chunk may omit padding; every split offset must still work.
    #[test]
    fn unpadded_base64_title_split_at_every_offset_reassembles() {
        let encoded = freminal_common::base64::encode_unpadded(SAMPLE_TITLE.as_bytes());
        for at in 0..=encoded.len() {
            let (a, b) = encoded.as_bytes().split_at(at);
            let finalized = two_base64_title_chunks(a, b)
                .unwrap_or_else(|| panic!("split at {at} must finalize"));
            assert_eq!(finalized.title, SAMPLE_TITLE, "split at {at}");
        }
    }

    /// Chunk-then-encode: each plaintext slice is encoded (and padded) on its
    /// own, so padding appears mid-stream.
    #[test]
    fn chunk_then_encode_with_padding_in_every_chunk_reassembles() {
        let plain = b"Hello, notification world";
        let mut handler = TerminalHandler::new(80, 24);
        // Slice lengths 5, 4, 7, 9 leave remainders 2, 1, 1, 0 so most
        // chunks end in padding.
        let mut rest = &plain[..];
        let mut result = None;
        for len in [5usize, 4, 7, 9] {
            let (head, tail) = rest.split_at(len);
            rest = tail;
            let done = rest.is_empty();
            let metadata = if done { "i=x:e=1" } else { "i=x:d=0:e=1" };
            result = feed(
                &mut handler,
                metadata,
                freminal_common::base64::encode(head).as_bytes(),
            );
        }
        let finalized = result.expect("last chunk must finalize");
        assert_eq!(finalized.title, "Hello, notification world");
        assert!(handler.pending_osc99.is_empty());
    }

    /// A `Plain` chunk, then a `Base64` chunk ending on a quantum boundary,
    /// then a `Plain` chunk: each is decoded by its own declared encoding.
    #[test]
    fn body_mixing_plain_and_base64_chunks_reassembles() {
        let mut handler = TerminalHandler::new(80, 24);
        assert!(feed(&mut handler, "i=m:p=body:d=0", b"Hello, ").is_none());
        // "world" -> "d29ybGQ=" (padded, so the quantum is complete).
        assert!(feed(&mut handler, "i=m:p=body:d=0:e=1", b"d29ybGQ=").is_none());
        let finalized = feed(&mut handler, "i=m:p=body", b"!").expect("final chunk must finalize");
        // A body-only notification is promoted into the title.
        assert_eq!(finalized.title, "Hello, world!");
        assert_eq!(finalized.body, None);
    }

    /// A `Plain` chunk arriving while a base64 quantum is incomplete is an
    /// error for the whole notification.
    #[test]
    fn plain_chunk_mid_quantum_drops_notification() {
        let mut handler = TerminalHandler::new(80, 24);
        assert!(feed(&mut handler, "i=q:p=body:d=0:e=1", b"d29y").is_none());
        assert!(feed(&mut handler, "i=q:p=body:d=0:e=1", b"bG").is_none());
        assert!(feed(&mut handler, "i=q:p=body:d=0", b"!").is_none());
        assert!(is_dropped(&handler, "q"));
    }

    /// Each payload type decodes its own stream: a title may stop
    /// mid-quantum while a body chunk is interleaved.
    #[test]
    fn interleaved_payload_types_keep_independent_streams() {
        let mut handler = TerminalHandler::new(80, 24);
        // "Hello" -> "SGVsbG8=": split mid-quantum around the body chunk.
        assert!(feed(&mut handler, "i=t:d=0:e=1", b"SGVs").is_none());
        assert!(feed(&mut handler, "i=t:p=body:d=0", b"plain body").is_none());
        let finalized = feed(&mut handler, "i=t:e=1", b"bG8=").expect("must finalize");
        assert_eq!(finalized.title, "Hello");
        assert_eq!(finalized.body.as_deref(), Some("plain body"));
    }

    /// Invalid base64 in a middle chunk drops the whole in-flight
    /// notification immediately.
    #[test]
    fn invalid_base64_in_middle_chunk_drops_notification() {
        let mut handler = TerminalHandler::new(80, 24);
        assert!(feed(&mut handler, "i=bad:d=0:e=1", b"SGVs").is_none());
        assert!(handler.pending_osc99.get(Some("bad")).is_some());
        assert!(feed(&mut handler, "i=bad:d=0:e=1", b"!!!!").is_none());
        assert!(
            is_dropped(&handler, "bad"),
            "the invalid chunk must drop the pending notification"
        );
    }

    /// A stream left mid-quantum at finalisation (dangling character) drops
    /// the notification instead of emitting a truncated one.
    #[test]
    fn dangling_base64_character_at_finalisation_drops_notification() {
        let mut handler = TerminalHandler::new(80, 24);
        assert!(feed(&mut handler, "i=d:d=0:e=1", b"SGVs").is_none());
        assert!(feed(&mut handler, "i=d:e=1", b"b").is_none());
        assert!(handler.pending_osc99.is_empty());
    }

    /// Standalone (no id) base64 payload still decodes.
    #[test]
    fn standalone_base64_title_decodes() {
        let mut handler = TerminalHandler::new(80, 24);
        let finalized = feed(&mut handler, "e=1", b"SGVsbG8=").expect("must finalize");
        assert_eq!(finalized.title, "Hello");
        assert!(handler.pending_osc99.is_empty());
    }

    /// Standalone base64 without trailing padding still decodes.
    #[test]
    fn standalone_unpadded_base64_title_decodes() {
        let mut handler = TerminalHandler::new(80, 24);
        let finalized = feed(&mut handler, "e=1", b"SGVsbG8").expect("must finalize");
        assert_eq!(finalized.title, "Hello");
    }

    /// Standalone invalid base64 drops the notification.
    #[test]
    fn standalone_invalid_base64_is_dropped() {
        let mut handler = TerminalHandler::new(80, 24);
        assert!(feed(&mut handler, "e=1", b"not base64!").is_none());
        // A lone dangling character is invalid too.
        assert!(feed(&mut handler, "e=1", b"SGVsb").is_none());
    }

    /// An id'd base64 notification that fits one sequence still decodes.
    #[test]
    fn single_chunk_base64_body_with_id_decodes() {
        let mut handler = TerminalHandler::new(80, 24);
        let finalized = feed(&mut handler, "i=one:p=body:e=1", b"SGVsbG8=").expect("must finalize");
        // A body-only notification is promoted into the title.
        assert_eq!(finalized.title, "Hello");
        assert_eq!(finalized.body, None);
        assert_eq!(
            finalized.meta.payload_encoding,
            Osc99PayloadEncoding::Base64
        );
    }

    /// The notification cap applies to the decoded length: two chunks whose
    /// decoded bytes together pass 1 MiB are dropped.
    #[test]
    fn base64_notification_exceeding_decoded_cap_is_dropped() {
        let mut handler = TerminalHandler::new(80, 24);
        let id = "huge";
        // Each chunk is under the per-sequence cap and decodes to ~786 KB;
        // together they exceed the 1 MiB notification cap.
        let chunk = vec![b'A'; 1_048_572 - 100];
        assert!(feed(&mut handler, "i=huge:p=icon:d=0:e=1", &chunk).is_none());
        assert!(handler.pending_osc99.get(Some(id)).is_some());
        assert!(feed(&mut handler, "i=huge:p=icon:d=0:e=1", &chunk).is_none());
        assert!(is_dropped(&handler, id));
    }

    /// PR #536 review: the combined cap must also hold for the bytes a pending
    /// base64 tail adds when the accumulators are finished.
    #[test]
    fn base64_tail_flushed_at_finish_cannot_exceed_the_combined_cap() {
        let half = vec![0x89; MAX_OSC99_NOTIFICATION_BYTES / 2];
        let almost_half = vec![0x89; MAX_OSC99_NOTIFICATION_BYTES / 2 - 1];
        let mut handler = TerminalHandler::new(80, 24);
        // Exactly 1 MiB across icon and title: at the cap, still accepted.
        assert!(feed(&mut handler, "i=cap:d=0:p=icon", &half).is_none());
        assert!(feed(&mut handler, "i=cap:d=0:p=icon", &almost_half).is_none());
        assert!(feed(&mut handler, "i=cap:d=0:p=title", b"T").is_none());
        // `YQ` decodes to one byte, but only when the stream is finished.
        assert!(feed(&mut handler, "i=cap:d=1:p=body:e=1", b"YQ").is_none());
        assert!(handler.pending_osc99.get(Some("cap")).is_none());

        // The same shape one byte smaller finalises with every part intact.
        let mut handler = TerminalHandler::new(80, 24);
        let shorter = vec![0x89; MAX_OSC99_NOTIFICATION_BYTES / 2 - 2];
        assert!(feed(&mut handler, "i=ok:d=0:p=icon", &half).is_none());
        assert!(feed(&mut handler, "i=ok:d=0:p=icon", &shorter).is_none());
        assert!(feed(&mut handler, "i=ok:d=0:p=title", b"T").is_none());
        let done = feed(&mut handler, "i=ok:d=1:p=body:e=1", b"YQ").expect("within the cap");
        assert_eq!(done.body.as_deref(), Some("a"));
        assert_eq!(done.title, "T");
        assert_eq!(
            done.icon.as_ref().map(Vec::len),
            Some(MAX_OSC99_NOTIFICATION_BYTES - 2)
        );
    }

    // ── Tombstones for notifications dropped mid-transfer ────────────────────

    /// Whether `id` is currently held as a [`PendingEntry::Dropped`] tombstone.
    fn is_dropped(handler: &TerminalHandler, id: &str) -> bool {
        matches!(
            handler.pending_osc99.get(Some(id)),
            Some(PendingEntry::Dropped)
        )
    }

    /// The reviewer's repro: invalid base64 mid-transfer, then a plain
    /// terminating chunk for the same id, must not yield a notification
    /// built from the tail fragment.
    #[test]
    fn dropped_notification_tail_chunk_emits_nothing() {
        let mut handler = TerminalHandler::new(80, 24);
        assert!(feed(&mut handler, "i=1:d=0:p=title:e=1", b"@@@@").is_none());
        assert!(is_dropped(&handler, "1"));
        assert!(feed(&mut handler, "i=1:d=1:p=title", b"TAILONLY").is_none());
        assert!(
            handler.pending_osc99.is_empty(),
            "the terminating chunk must remove the tombstone"
        );
    }

    /// After the tombstone is consumed by `d=1`, the same id starts a fresh,
    /// valid notification that finalizes normally.
    #[test]
    fn id_is_reusable_after_dropped_notification_terminates() {
        let mut handler = TerminalHandler::new(80, 24);
        assert!(feed(&mut handler, "i=1:d=0:p=title:e=1", b"@@@@").is_none());
        assert!(feed(&mut handler, "i=1:d=1:p=title", b"TAILONLY").is_none());

        assert!(feed(&mut handler, "i=1:d=0:p=title", b"Fresh ").is_none());
        let finalized = feed(&mut handler, "i=1:d=1:p=title", b"start").expect("must finalize");
        assert_eq!(finalized.title, "Fresh start");
        assert!(handler.pending_osc99.is_empty());
    }

    /// Intermediate chunks of a dropped transfer are ignored and keep the
    /// tombstone; they never accumulate.
    #[test]
    fn dropped_notification_ignores_intermediate_chunks() {
        let mut handler = TerminalHandler::new(80, 24);
        assert!(feed(&mut handler, "i=1:d=0:p=title:e=1", b"@@@@").is_none());
        assert!(feed(&mut handler, "i=1:d=0:p=title", b"more").is_none());
        assert!(feed(&mut handler, "i=1:d=0:p=body", b"more").is_none());
        assert!(is_dropped(&handler, "1"));
        assert_eq!(handler.pending_osc99.identified_len(), 1);
    }

    /// If the offending chunk is itself the terminating one, the notification
    /// is over: no tombstone is left behind.
    #[test]
    fn invalid_terminating_chunk_leaves_no_tombstone() {
        let mut handler = TerminalHandler::new(80, 24);
        assert!(feed(&mut handler, "i=1:d=0:p=title:e=1", b"SGVs").is_none());
        assert!(feed(&mut handler, "i=1:d=1:p=title:e=1", b"@@@@").is_none());
        assert!(handler.pending_osc99.is_empty());
    }

    /// A combined-cap breach tombstones the id: further chunks and the
    /// terminating chunk emit nothing.
    #[test]
    fn combined_cap_breach_then_tail_chunks_emit_nothing() {
        let mut handler = TerminalHandler::new(80, 24);
        // Title and body each fit their own accumulator, but together they
        // exceed the per-notification cap.
        let half = vec![b'a'; MAX_OSC99_NOTIFICATION_BYTES / 2 + 1];
        assert!(feed(&mut handler, "i=big:d=0:p=title", &half).is_none());
        assert!(feed(&mut handler, "i=big:d=0:p=body", &half).is_none());
        assert!(is_dropped(&handler, "big"));

        assert!(feed(&mut handler, "i=big:d=0:p=title", b"tail").is_none());
        assert!(feed(&mut handler, "i=big:d=1:p=title", b"tail").is_none());
        assert!(handler.pending_osc99.is_empty());
    }

    /// Tombstones occupy slots in the pending map like any other entry, so
    /// the 128-entry cap bounds them too: a new id evicts the
    /// least-recently-touched tombstone rather than being refused.
    #[test]
    fn tombstones_are_evicted_least_recently_touched_first() {
        let mut handler = TerminalHandler::new(80, 24);
        for i in 0..MAX_PENDING_OSC99_NOTIFICATIONS {
            let metadata = format!("i=id-{i}:d=0:p=title:e=1");
            assert!(feed(&mut handler, &metadata, b"@@@@").is_none());
        }
        assert_eq!(
            handler.pending_osc99.identified_len(),
            MAX_PENDING_OSC99_NOTIFICATIONS
        );
        assert!(
            (0..MAX_PENDING_OSC99_NOTIFICATIONS).all(|i| is_dropped(&handler, &format!("id-{i}")))
        );

        // A chunk of a dropped transfer touches its tombstone.
        assert!(feed(&mut handler, "i=id-0:d=0:p=title", b"x").is_none());

        // A new id evicts `id-1`, now the least recently touched: the map
        // does not grow past the cap.
        assert!(feed(&mut handler, "i=fresh:d=0:p=title", b"x").is_none());
        assert_eq!(
            handler.pending_osc99.identified_len(),
            MAX_PENDING_OSC99_NOTIFICATIONS
        );
        assert!(handler.pending_osc99.get(Some("fresh")).is_some());
        assert!(handler.pending_osc99.get(Some("id-1")).is_none());
        assert!(is_dropped(&handler, "id-0"));

        // Terminating a tombstoned id frees its slot.
        assert!(feed(&mut handler, "i=id-0:d=1:p=title", b"x").is_none());
        assert_eq!(
            handler.pending_osc99.identified_len(),
            MAX_PENDING_OSC99_NOTIFICATIONS - 1
        );
    }

    // ── 138.3: metadata merge ─────────────────────────────────────────────────

    /// Merge `chunks` in order and return the accumulated metadata.
    fn merged(chunks: &[Osc99Command]) -> Osc99Command {
        let mut acc = blank_osc99_metadata();
        for chunk in chunks {
            merge_osc99_metadata(&mut acc, chunk);
        }
        acc
    }

    fn edits(edits: &[Osc99ActionEdit]) -> Osc99Command {
        Osc99Command {
            action_edits: edits.to_vec(),
            ..default_cmd()
        }
    }

    #[test]
    fn merge_report_then_chunk_without_a_resolves_to_report_and_focus() {
        let acc = merged(&[
            edits(&[Osc99ActionEdit::Add(Osc99Action::Report)]),
            default_cmd(),
        ]);
        assert_eq!(
            resolve_osc99_actions(&acc.action_edits),
            (
                Osc99ActivationReporting::Report,
                Osc99ActivationFocus::Focus
            )
        );
    }

    #[test]
    fn merge_minus_focus_then_focus_resolves_to_focus() {
        let acc = merged(&[
            edits(&[Osc99ActionEdit::Remove(Osc99Action::Focus)]),
            edits(&[Osc99ActionEdit::Add(Osc99Action::Focus)]),
        ]);
        assert_eq!(
            resolve_osc99_actions(&acc.action_edits),
            (
                Osc99ActivationReporting::Silent,
                Osc99ActivationFocus::Focus
            )
        );
    }

    #[test]
    fn merge_minus_focus_then_chunk_without_a_stays_unfocused() {
        // A later chunk's silence must not resurrect a removed action.
        let acc = merged(&[
            edits(&[Osc99ActionEdit::Remove(Osc99Action::Focus)]),
            default_cmd(),
        ]);
        assert_eq!(
            resolve_osc99_actions(&acc.action_edits).1,
            Osc99ActivationFocus::NoFocus
        );
    }

    #[test]
    fn merge_urgency_takes_the_latest_explicit_value() {
        let with = |urgency| Osc99Command {
            urgency,
            ..default_cmd()
        };
        let kept = merged(&[with(Some(NotificationUrgency::Critical)), with(None)]);
        assert_eq!(kept.urgency, Some(NotificationUrgency::Critical));

        let overridden = merged(&[
            with(Some(NotificationUrgency::Low)),
            with(Some(NotificationUrgency::Critical)),
        ]);
        assert_eq!(overridden.urgency, Some(NotificationUrgency::Critical));
    }

    #[test]
    fn merge_lists_extend_in_arrival_order() {
        let with = |name: &str, kind: &str| Osc99Command {
            icon_names: vec![name.to_owned()],
            notification_types: vec![kind.to_owned()],
            ..default_cmd()
        };
        let acc = merged(&[with("a", "ta"), with("b", "tb")]);
        assert_eq!(acc.icon_names, ["a", "b"]);
        assert_eq!(acc.notification_types, ["ta", "tb"]);
    }

    #[test]
    fn merge_close_reporting_and_other_scalars_survive_a_silent_chunk() {
        let first = Osc99Command {
            close_reporting: Some(Osc99CloseReporting::Report),
            occasion: Some(NotificationOccasion::Unfocused),
            expiry: Some(Osc99Expiry::Never),
            app_name: Some("app".to_owned()),
            icon_cache_key: Some("key".to_owned()),
            sound: Some("silent".to_owned()),
            ..default_cmd()
        };
        let acc = merged(&[first, default_cmd()]);
        assert_eq!(acc.close_reporting, Some(Osc99CloseReporting::Report));
        assert_eq!(acc.occasion, Some(NotificationOccasion::Unfocused));
        assert_eq!(acc.expiry, Some(Osc99Expiry::Never));
        assert_eq!(acc.app_name.as_deref(), Some("app"));
        assert_eq!(acc.icon_cache_key.as_deref(), Some("key"));
        assert_eq!(acc.sound.as_deref(), Some("silent"));

        let later = Osc99Command {
            close_reporting: Some(Osc99CloseReporting::Silent),
            app_name: Some("other".to_owned()),
            ..default_cmd()
        };
        let first = Osc99Command {
            close_reporting: Some(Osc99CloseReporting::Report),
            app_name: Some("app".to_owned()),
            ..default_cmd()
        };
        let acc = merged(&[first, later]);
        assert_eq!(acc.close_reporting, Some(Osc99CloseReporting::Silent));
        assert_eq!(acc.app_name.as_deref(), Some("other"));
    }

    #[test]
    fn merge_per_chunk_fields_take_the_latest_chunk() {
        let first = Osc99Command {
            id: Some("i".to_owned()),
            payload_type: Osc99PayloadType::Body,
            chunk_state: Osc99ChunkState::More,
            payload_encoding: Osc99PayloadEncoding::Base64,
            ..default_cmd()
        };
        let last = Osc99Command {
            id: Some("i".to_owned()),
            ..default_cmd()
        };
        let acc = merged(&[first, last]);
        assert_eq!(acc.id.as_deref(), Some("i"));
        assert_eq!(acc.payload_type, Osc99PayloadType::Title);
        assert_eq!(acc.chunk_state, Osc99ChunkState::Done);
        assert_eq!(acc.payload_encoding, Osc99PayloadEncoding::Plain);
        assert_eq!(acc.payload, [] as [u8; 0]);
    }

    #[test]
    fn list_metadata_counts_toward_the_notification_cap() {
        let mut pending = PendingNotification::default();
        let chunk = |name: String| Osc99Command {
            chunk_state: Osc99ChunkState::More,
            icon_names: vec![name],
            ..default_cmd()
        };
        let big = "n".repeat(MAX_OSC99_NOTIFICATION_BYTES / 2 + 1);
        assert!(pending.accept(&chunk(big.clone())).is_ok());
        assert_eq!(
            pending.accept(&chunk(big)),
            Err(Osc99Reject::Chunk(ChunkError::TotalTooLarge {
                max: MAX_OSC99_NOTIFICATION_BYTES
            }))
        );
    }

    // ── 138.3: finalisation helpers ───────────────────────────────────────────

    #[test]
    fn strip_removes_c0_del_and_c1_but_keeps_lf() {
        assert_eq!(
            strip_osc99_controls(
                "a\u{0}b\u{9}c\u{b}d\u{1b}e\u{1f}f\u{7f}g\u{80}h\u{85}i\u{9f}j\nk"
            ),
            "abcdefghij\nk"
        );
    }

    #[test]
    fn strip_keeps_printable_and_neighbouring_code_points() {
        // U+0020 and U+00A0 sit just outside the removed ranges.
        let text = "a b\u{a0}c é ✓ \u{fffd}";
        assert_eq!(strip_osc99_controls(text), text);
    }

    #[test]
    fn limit_leaves_a_fitting_string_alone() {
        assert_eq!(limit_osc99_text("abcd".to_owned(), 4), "abcd");
        assert_eq!(limit_osc99_text(String::new(), 4), "");
    }

    #[test]
    fn limit_keeps_n_minus_one_chars_and_an_ellipsis() {
        let limited = limit_osc99_text("abcde".to_owned(), 4);
        assert_eq!(limited, "abc\u{2026}");
        assert_eq!(limited.chars().count(), 4);
    }

    #[test]
    fn limit_counts_characters_not_bytes() {
        // Each of these is more than one byte, so a byte count would cut early.
        let limited = limit_osc99_text("✓✓✓✓✓".to_owned(), 4);
        assert_eq!(limited, "✓✓✓\u{2026}");
        assert_eq!(limit_osc99_text("✓✓✓✓".to_owned(), 4), "✓✓✓✓");
    }

    #[test]
    fn button_labels_are_stripped_limited_filtered_and_capped() {
        let long = "b".repeat(300);
        let mut labels = vec![
            "\u{1b}\u{7f}".to_owned(), // strips to nothing: dropped
            long,
            "ok\u{9}".to_owned(),
        ];
        labels.extend((0..10).map(|i| format!("{i}")));
        let out = sanitize_button_labels(labels);
        assert_eq!(out.len(), MAX_OSC99_BUTTONS);
        assert_eq!(out[0].chars().count(), MAX_OSC99_BUTTON_CHARS);
        assert!(out[0].ends_with('\u{2026}'));
        assert_eq!(out[1], "ok");
        assert_eq!(&out[2..], ["0", "1", "2", "3", "4", "5"]);
    }

    fn payloads(title: &str, body: &str) -> FinishedPayloads {
        FinishedPayloads {
            title: title.as_bytes().to_vec(),
            body: body.as_bytes().to_vec(),
            icon: Vec::new(),
            buttons: Vec::new(),
        }
    }

    #[test]
    fn build_promotes_the_body_when_there_is_no_title() {
        let f = build_finalized(payloads("", "Body"), default_cmd()).expect("has content");
        assert_eq!(f.title, "Body");
        assert_eq!(f.body, None);
    }

    #[test]
    fn build_promotes_the_body_when_the_title_strips_to_nothing() {
        let f = build_finalized(payloads("\u{1b}\u{85}", "Body"), default_cmd()).expect("content");
        assert_eq!(f.title, "Body");
        assert_eq!(f.body, None);
    }

    #[test]
    fn build_keeps_title_and_body_apart() {
        let f = build_finalized(payloads("Title", "Body"), default_cmd()).expect("content");
        assert_eq!(f.title, "Title");
        assert_eq!(f.body.as_deref(), Some("Body"));
    }

    #[test]
    fn build_without_title_or_body_is_none() {
        assert!(build_finalized(payloads("", ""), default_cmd()).is_none());
        assert!(build_finalized(payloads("\u{1}", "\u{2}"), default_cmd()).is_none());
        // Icon bytes and buttons alone are not a notification.
        let mut only_icon = payloads("", "");
        only_icon.icon = vec![1, 2, 3];
        only_icon.buttons = b"OK".to_vec();
        assert!(build_finalized(only_icon, default_cmd()).is_none());
    }

    #[test]
    fn build_decodes_lossily_and_leaves_icon_bytes_untouched() {
        let mut p = payloads("", "");
        p.title = vec![b'a', 0xff, b'b'];
        p.icon = vec![0, 1, 0x1b, 0x7f, 0xff];
        let f = build_finalized(p, default_cmd()).expect("content");
        assert_eq!(f.title, "a\u{fffd}b");
        assert_eq!(f.icon.as_deref(), Some(&[0u8, 1, 0x1b, 0x7f, 0xff][..]));
    }

    #[test]
    fn build_limits_title_and_body_to_1024_chars() {
        let f = build_finalized(
            payloads(&"t".repeat(2000), &"b".repeat(1024)),
            default_cmd(),
        )
        .expect("content");
        assert_eq!(f.title.chars().count(), 1024);
        assert!(f.title.ends_with('\u{2026}'));
        assert_eq!(f.body.as_deref().map(|b| b.chars().count()), Some(1024));
        assert!(f.body.as_deref().is_some_and(|b| !b.ends_with('\u{2026}')));
    }

    #[test]
    fn build_sanitises_the_metadata_names() {
        let meta = Osc99Command {
            app_name: Some("\u{1b}".to_owned()),
            sound: Some("\u{7f}".to_owned()),
            icon_names: vec!["a\u{1b}".to_owned(), "\u{85}".to_owned(), "b".to_owned()],
            notification_types: vec!["\u{9}".to_owned(), "t\u{0}x".to_owned()],
            ..default_cmd()
        };
        let f = build_finalized(payloads("T", ""), meta).expect("content");
        assert_eq!(f.meta.app_name, None);
        assert_eq!(f.meta.sound, None);
        assert_eq!(f.meta.icon_names, ["a", "b"]);
        assert_eq!(f.meta.notification_types, ["tx"]);
        assert_eq!(
            f.into_notification99_data().sound,
            Osc99Sound::System,
            "a sound that strips to nothing is the system default"
        );
    }

    #[test]
    fn build_keeps_a_named_sound_and_app_name() {
        let meta = Osc99Command {
            app_name: Some("my app".to_owned()),
            sound: Some("bell".to_owned()),
            ..default_cmd()
        };
        let data = build_finalized(payloads("T", ""), meta)
            .expect("content")
            .into_notification99_data();
        assert_eq!(data.app_name.as_deref(), Some("my app"));
        assert_eq!(data.sound, Osc99Sound::Named("bell".to_owned()));
    }

    // ── 138.3: pending store ──────────────────────────────────────────────────

    #[test]
    fn store_creates_touches_and_removes_entries() {
        let mut store = PendingOsc99::default();
        assert!(store.is_empty());
        assert!(matches!(
            store.slot_mut(Some("a")).entry,
            PendingEntry::Accumulating(_)
        ));
        assert!(matches!(
            store.slot_mut(None).entry,
            PendingEntry::Accumulating(_)
        ));
        assert_eq!(store.identified_len(), 1);
        assert!(!store.is_empty());
        assert!(store.remove(Some("a")).is_some());
        assert!(store.remove(Some("a")).is_none());
        assert!(store.remove(None).is_some());
        assert!(store.is_empty());
    }

    #[test]
    fn store_evicts_the_smallest_last_touched_at_the_cap() {
        let mut store = PendingOsc99::default();
        for i in 0..MAX_PENDING_OSC99_NOTIFICATIONS {
            store.slot_mut(Some(&format!("id-{i}")));
        }
        // Touch the two oldest again, leaving `id-2` as the least recent.
        store.slot_mut(Some("id-0"));
        store.slot_mut(Some("id-1"));

        store.slot_mut(Some("new"));
        assert_eq!(store.identified_len(), MAX_PENDING_OSC99_NOTIFICATIONS);
        assert!(store.get(Some("id-2")).is_none());
        for kept in ["id-0", "id-1", "id-3", "new"] {
            assert!(store.get(Some(kept)).is_some(), "{kept} must survive");
        }

        // Re-touching an existing id at the cap evicts nothing.
        store.slot_mut(Some("id-3"));
        assert_eq!(store.identified_len(), MAX_PENDING_OSC99_NOTIFICATIONS);
        assert!(store.get(Some("id-4")).is_some());
    }

    #[test]
    fn store_never_evicts_the_unidentified_slot() {
        let mut store = PendingOsc99::default();
        store.slot_mut(None);
        for i in 0..=MAX_PENDING_OSC99_NOTIFICATIONS {
            store.slot_mut(Some(&format!("id-{i}")));
        }
        assert!(store.get(None).is_some());
    }

    #[test]
    fn store_tombstone_replaces_and_is_detected() {
        let mut store = PendingOsc99::default();
        assert!(!store.is_tombstone(Some("a")));
        assert!(!store.is_tombstone(None));
        store.slot_mut(Some("a"));
        assert!(!store.is_tombstone(Some("a")));
        store.tombstone(Some("a"));
        assert!(store.is_tombstone(Some("a")));
        store.tombstone(None);
        assert!(store.is_tombstone(None));
    }

    #[test]
    fn store_clear_empties_both_halves() {
        let mut store = PendingOsc99::default();
        store.slot_mut(Some("a"));
        store.slot_mut(None);
        store.clear();
        assert!(store.is_empty());
        assert!(store.get(None).is_none());
        assert_eq!(store.identified_len(), 0);
    }

    // ── 138.C6: bounded, linear-cost reassembly ───────────────────────────────

    /// How many entries the pending notification for `id` holds in the list
    /// selected by `len`, or `None` when it holds no live accumulation.
    fn live_list_len(
        handler: &TerminalHandler,
        id: &str,
        len: fn(&Osc99Command) -> usize,
    ) -> Option<usize> {
        let Some(PendingEntry::Accumulating(entry)) = handler.pending_osc99.get(Some(id)) else {
            return None;
        };
        entry.meta.as_ref().map(len)
    }

    /// Stream 100k tiny `d=0` chunks that each add one entry to one list and
    /// assert the list never passes `cap` and the transfer ends as a tombstone.
    fn assert_hostile_list_stream_is_capped(
        metadata: &str,
        cap: usize,
        len: fn(&Osc99Command) -> usize,
    ) {
        let mut handler = TerminalHandler::new(80, 24);
        for _ in 0..100_000 {
            assert!(feed(&mut handler, metadata, b"").is_none());
            if let Some(held) = live_list_len(&handler, "hostile", len) {
                assert!(held <= cap, "list grew to {held}, past the cap of {cap}");
            }
        }
        assert!(is_dropped(&handler, "hostile"));
        assert_eq!(handler.pending_osc99.identified_len(), 1);
        handler.pending_osc99.assert_costs_consistent();
        assert_eq!(handler.pending_osc99.total_cost(), 0);
    }

    #[test]
    fn hostile_icon_name_stream_is_dropped_at_the_cap() {
        assert_hostile_list_stream_is_capped(
            "i=hostile:d=0:p=title:n=YQ",
            MAX_OSC99_ICON_NAMES,
            |m| m.icon_names.len(),
        );
    }

    #[test]
    fn hostile_notification_type_stream_is_dropped_at_the_cap() {
        assert_hostile_list_stream_is_capped(
            "i=hostile:d=0:p=title:t=YQ",
            MAX_OSC99_NOTIFICATION_TYPES,
            |m| m.notification_types.len(),
        );
    }

    #[test]
    fn hostile_action_edit_stream_is_dropped_at_the_cap() {
        assert_hostile_list_stream_is_capped(
            "i=hostile:d=0:p=title:a=report",
            MAX_OSC99_ACTION_EDITS,
            |m| m.action_edits.len(),
        );
    }

    #[test]
    fn list_caps_admit_exactly_the_cap_and_reject_one_more() {
        let chunk = |names: usize| Osc99Command {
            chunk_state: Osc99ChunkState::More,
            icon_names: vec!["a".to_owned(); names],
            ..default_cmd()
        };
        let mut pending = PendingNotification::default();
        assert!(pending.accept(&chunk(MAX_OSC99_ICON_NAMES)).is_ok());
        assert_eq!(
            pending.accept(&chunk(1)),
            Err(Osc99Reject::TooManyIconNames {
                max: MAX_OSC99_ICON_NAMES
            })
        );
        // A rejected chunk merges nothing.
        assert_eq!(
            pending.meta.as_ref().map(|m| m.icon_names.len()),
            Some(MAX_OSC99_ICON_NAMES)
        );
    }

    #[test]
    fn list_cost_charges_len_plus_overhead_per_string_and_overhead_per_edit() {
        let mut pending = PendingNotification::default();
        let chunk = Osc99Command {
            chunk_state: Osc99ChunkState::More,
            icon_names: vec!["ab".to_owned()],
            notification_types: vec!["c".to_owned(), "defg".to_owned()],
            action_edits: vec![Osc99ActionEdit::Add(Osc99Action::Report)],
            ..default_cmd()
        };
        pending.accept(&chunk).unwrap();
        let expected = (2 + OSC99_LIST_ENTRY_OVERHEAD)
            + (1 + OSC99_LIST_ENTRY_OVERHEAD)
            + (4 + OSC99_LIST_ENTRY_OVERHEAD)
            + OSC99_LIST_ENTRY_OVERHEAD;
        assert_eq!(pending.list_cost, expected);

        // A chunk without lists adds nothing, however much is already held.
        pending.accept(&default_cmd()).unwrap();
        assert_eq!(pending.list_cost, expected);
        assert_eq!(pending.cost(), expected);
    }

    #[test]
    fn list_cost_counts_toward_the_byte_budget_before_the_entry_cap() {
        let mut pending = PendingNotification::default();
        let name_len = 20_000;
        let chunk = Osc99Command {
            chunk_state: Osc99ChunkState::More,
            icon_names: vec!["n".repeat(name_len)],
            ..default_cmd()
        };
        let mut accepted = 0usize;
        let rejection = loop {
            match pending.accept(&chunk) {
                Ok(()) => accepted += 1,
                Err(err) => break err,
            }
        };
        assert_eq!(
            rejection,
            Osc99Reject::Chunk(ChunkError::TotalTooLarge {
                max: MAX_OSC99_NOTIFICATION_BYTES
            })
        );
        // The byte budget bites before the 64-entry cap does.
        assert!(accepted < MAX_OSC99_ICON_NAMES);
        assert_eq!(
            accepted,
            MAX_OSC99_NOTIFICATION_BYTES / (name_len + OSC99_LIST_ENTRY_OVERHEAD)
        );
    }

    #[test]
    fn title_body_and_buttons_are_capped_at_64_kib() {
        for (payload_type, name) in [
            (Osc99PayloadType::Title, "title"),
            (Osc99PayloadType::Body, "body"),
            (Osc99PayloadType::Buttons, "buttons"),
        ] {
            let chunk = |len: usize| Osc99Command {
                payload_type,
                chunk_state: Osc99ChunkState::More,
                payload: vec![b'x'; len],
                ..default_cmd()
            };
            let mut at_cap = PendingNotification::default();
            assert!(
                at_cap.accept(&chunk(MAX_OSC99_TEXT_PAYLOAD_BYTES)).is_ok(),
                "{name} at the cap must be accepted"
            );
            assert!(
                matches!(
                    at_cap.accept(&chunk(1)),
                    Err(Osc99Reject::Chunk(ChunkError::TotalTooLarge {
                        max: MAX_OSC99_TEXT_PAYLOAD_BYTES
                    }))
                ),
                "{name} one byte over the cap must be rejected"
            );
        }
    }

    #[test]
    fn oversized_title_drops_the_transfer() {
        let mut handler = TerminalHandler::new(80, 24);
        let chunk = vec![b't'; MAX_OSC99_TEXT_PAYLOAD_BYTES + 1];
        assert!(feed(&mut handler, "i=big:d=0:p=title", &chunk).is_none());
        assert!(is_dropped(&handler, "big"));
        assert_eq!(handler.pending_osc99.total_cost(), 0);
    }

    #[test]
    fn icon_keeps_the_one_mebibyte_cap() {
        let half = Osc99Command {
            payload_type: Osc99PayloadType::Icon,
            chunk_state: Osc99ChunkState::More,
            payload: vec![0x89; MAX_OSC99_ICON_PAYLOAD_BYTES / 2],
            ..default_cmd()
        };
        let mut pending = PendingNotification::default();
        assert!(pending.accept(&half).is_ok());
        assert!(pending.accept(&half).is_ok());
        assert_eq!(pending.cost(), MAX_OSC99_ICON_PAYLOAD_BYTES);
        let one_more = Osc99Command {
            payload: vec![0x89],
            ..half
        };
        assert!(pending.accept(&one_more).is_err());
    }

    /// A content chunk of `len` payload bytes for the transfer `id`.
    fn icon_chunk(id: Option<&str>, len: usize) -> Osc99Command {
        Osc99Command {
            id: id.map(str::to_owned),
            payload_type: Osc99PayloadType::Icon,
            chunk_state: Osc99ChunkState::More,
            payload: vec![0x89; len],
            ..default_cmd()
        }
    }

    #[test]
    fn aggregate_budget_evicts_the_least_recently_touched_other_entry() {
        let mut handler = TerminalHandler::new(80, 24);
        let each = 1_000_000;
        for i in 0..8 {
            assert!(
                handler
                    .reassemble_osc99(&icon_chunk(Some(&format!("a{i}")), each))
                    .is_none()
            );
        }
        assert_eq!(handler.pending_osc99.total_cost(), 8 * each);
        handler.pending_osc99.assert_costs_consistent();

        // Touching `a0` makes `a1` the least recently touched.
        assert!(
            handler
                .reassemble_osc99(&icon_chunk(Some("a0"), 1))
                .is_none()
        );
        assert!(
            handler
                .reassemble_osc99(&icon_chunk(Some("b"), each))
                .is_none()
        );

        assert!(handler.pending_osc99.get(Some("a1")).is_none());
        for kept in ["a0", "a2", "a3", "a4", "a5", "a6", "a7", "b"] {
            assert!(
                handler.pending_osc99.get(Some(kept)).is_some(),
                "{kept} must survive"
            );
        }
        assert_eq!(handler.pending_osc99.total_cost(), 8 * each + 1);
        handler.pending_osc99.assert_costs_consistent();
    }

    #[test]
    fn aggregate_budget_never_evicts_the_chunks_own_entry() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.pending_osc99 = PendingOsc99::with_budget(1_000);
        assert!(
            handler
                .reassemble_osc99(&icon_chunk(Some("old"), 600))
                .is_none()
        );
        // The same id keeps growing: the other entry is evicted, not this one.
        assert!(
            handler
                .reassemble_osc99(&icon_chunk(Some("new"), 600))
                .is_none()
        );
        assert!(handler.pending_osc99.get(Some("old")).is_none());
        assert!(
            handler
                .reassemble_osc99(&icon_chunk(Some("new"), 300))
                .is_none()
        );
        assert!(matches!(
            handler.pending_osc99.get(Some("new")),
            Some(PendingEntry::Accumulating(_))
        ));
        assert_eq!(handler.pending_osc99.total_cost(), 900);
        handler.pending_osc99.assert_costs_consistent();
    }

    #[test]
    fn aggregate_budget_can_evict_the_unidentified_slot() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.pending_osc99 = PendingOsc99::with_budget(1_000);
        assert!(handler.reassemble_osc99(&icon_chunk(None, 600)).is_none());
        assert!(
            handler
                .reassemble_osc99(&icon_chunk(Some("id"), 600))
                .is_none()
        );
        assert!(handler.pending_osc99.get(None).is_none());
        assert!(handler.pending_osc99.get(Some("id")).is_some());
        assert_eq!(handler.pending_osc99.total_cost(), 600);
        handler.pending_osc99.assert_costs_consistent();
    }

    #[test]
    fn aggregate_budget_drops_an_entry_that_alone_exceeds_it() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.pending_osc99 = PendingOsc99::with_budget(1_000);
        assert!(
            handler
                .reassemble_osc99(&icon_chunk(Some("other"), 400))
                .is_none()
        );
        assert!(
            handler
                .reassemble_osc99(&icon_chunk(Some("huge"), 1_001))
                .is_none()
        );
        // The oversized transfer is a tombstone; the bystander was not evicted.
        assert!(is_dropped(&handler, "huge"));
        assert!(matches!(
            handler.pending_osc99.get(Some("other")),
            Some(PendingEntry::Accumulating(_))
        ));
        assert_eq!(handler.pending_osc99.total_cost(), 400);
        handler.pending_osc99.assert_costs_consistent();

        // Its terminating chunk removes the tombstone and emits nothing.
        let done = Osc99Command {
            chunk_state: Osc99ChunkState::Done,
            ..icon_chunk(Some("huge"), 0)
        };
        assert!(handler.reassemble_osc99(&done).is_none());
        assert!(handler.pending_osc99.get(Some("huge")).is_none());
        handler.pending_osc99.assert_costs_consistent();
    }

    #[test]
    fn running_total_matches_a_recomputed_total_through_a_mixed_sequence() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.pending_osc99 = PendingOsc99::with_budget(5_000);
        let check = |handler: &TerminalHandler| handler.pending_osc99.assert_costs_consistent();

        for i in 0..6 {
            handler.reassemble_osc99(&icon_chunk(Some(&format!("x{i}")), 1_200));
            check(&handler);
        }
        handler.reassemble_osc99(&icon_chunk(None, 700));
        check(&handler);

        // List metadata is charged too.
        let named = Osc99Command {
            id: Some("named".to_owned()),
            chunk_state: Osc99ChunkState::More,
            icon_names: vec!["n".repeat(100)],
            ..default_cmd()
        };
        handler.reassemble_osc99(&named);
        check(&handler);

        // Tombstone, then its terminating chunk.
        handler.reassemble_osc99(&icon_chunk(Some("bad"), 9_999));
        check(&handler);
        let done_bad = Osc99Command {
            chunk_state: Osc99ChunkState::Done,
            ..icon_chunk(Some("bad"), 0)
        };
        handler.reassemble_osc99(&done_bad);
        check(&handler);

        // Finalise one, standalone-finalise another, then clear.
        let finish = Osc99Command {
            payload_type: Osc99PayloadType::Title,
            chunk_state: Osc99ChunkState::Done,
            payload: b"t".to_vec(),
            ..icon_chunk(Some("named"), 0)
        };
        assert!(handler.reassemble_osc99(&finish).is_some());
        check(&handler);
        let standalone = Osc99Command {
            payload: b"solo".to_vec(),
            ..default_cmd()
        };
        assert!(handler.reassemble_osc99(&standalone).is_some());
        check(&handler);

        handler.pending_osc99.clear();
        check(&handler);
        assert_eq!(handler.pending_osc99.total_cost(), 0);
    }

    #[test]
    fn reassembly_borrows_the_chunk_and_never_stores_its_payload_in_meta() {
        let mut handler = TerminalHandler::new(80, 24);
        let chunk = Osc99Command {
            id: Some("borrow".to_owned()),
            payload_type: Osc99PayloadType::Body,
            chunk_state: Osc99ChunkState::More,
            payload: vec![b'p'; 4096],
            ..default_cmd()
        };
        assert!(handler.reassemble_osc99(&chunk).is_none());
        // The caller still owns the payload: nothing was moved out of it, and
        // the entry's metadata holds no copy.
        assert_eq!(chunk.payload.len(), 4096);
        let Some(PendingEntry::Accumulating(entry)) = handler.pending_osc99.get(Some("borrow"))
        else {
            panic!("entry should be accumulating");
        };
        assert_eq!(entry.body.len(), 4096);
        assert!(entry.meta.as_ref().is_some_and(|m| m.payload.is_empty()));
        assert_eq!(handler.pending_osc99.total_cost(), 4096);
    }

    // ── 138.4: osc99_query_reply_body ─────────────────────────────────────────

    const ALL_SUPPORTED_REPLY: &str = "a=focus,report:o=always,unfocused,invisible:u=0,1,2:p=title,body,?,close,icon,alive,buttons:c=1:w=1:s=system,silent,error,info,question,warn,warning";

    /// Every feature supported, with the standard sound names.
    fn all_supported() -> Osc99Features {
        Osc99Features {
            activation_report: Osc99ActivationReport::Reported,
            close_events: Osc99CloseEvents::Reported,
            activation_focus: Osc99FocusAction::Supported,
            auto_expiry: Osc99AutoExpiry::Supported,
            urgency: Osc99UrgencySupport::Supported,
            sound_names: Osc99SoundNames::Standard,
            body: Osc99BodySupport::Supported,
            buttons: Osc99ButtonSupport::Supported,
            icons: Osc99IconSupport::Supported,
        }
    }

    /// Every feature unsupported, with only the well-known sound names.
    fn none_supported() -> Osc99Features {
        Osc99Features {
            activation_report: Osc99ActivationReport::NotReported,
            close_events: Osc99CloseEvents::NotReported,
            activation_focus: Osc99FocusAction::Unsupported,
            auto_expiry: Osc99AutoExpiry::Unsupported,
            urgency: Osc99UrgencySupport::Unsupported,
            sound_names: Osc99SoundNames::SystemAndSilent,
            body: Osc99BodySupport::Unsupported,
            buttons: Osc99ButtonSupport::Unsupported,
            icons: Osc99IconSupport::Unsupported,
        }
    }

    #[test]
    fn query_reply_body_table() {
        let cases: Vec<(&str, Osc99Features, &str)> = vec![
            ("all supported", all_supported(), ALL_SUPPORTED_REPLY),
            (
                "none supported",
                none_supported(),
                "o=always,unfocused,invisible:p=title,?,close,alive:s=system,silent",
            ),
            (
                "focus only",
                Osc99Features {
                    activation_focus: Osc99FocusAction::Supported,
                    ..none_supported()
                },
                "a=focus:o=always,unfocused,invisible:p=title,?,close,alive:s=system,silent",
            ),
            (
                "report only",
                Osc99Features {
                    activation_report: Osc99ActivationReport::Reported,
                    ..none_supported()
                },
                "a=report:o=always,unfocused,invisible:p=title,?,close,alive:s=system,silent",
            ),
            (
                "standard sounds only",
                Osc99Features {
                    sound_names: Osc99SoundNames::Standard,
                    ..none_supported()
                },
                "o=always,unfocused,invisible:p=title,?,close,alive:s=system,silent,error,info,question,warn,warning",
            ),
            (
                "no body",
                Osc99Features {
                    body: Osc99BodySupport::Unsupported,
                    ..all_supported()
                },
                "a=focus,report:o=always,unfocused,invisible:u=0,1,2:p=title,?,close,icon,alive,buttons:c=1:w=1:s=system,silent,error,info,question,warn,warning",
            ),
            (
                "no icon",
                Osc99Features {
                    icons: Osc99IconSupport::Unsupported,
                    ..all_supported()
                },
                "a=focus,report:o=always,unfocused,invisible:u=0,1,2:p=title,body,?,close,alive,buttons:c=1:w=1:s=system,silent,error,info,question,warn,warning",
            ),
            (
                "no buttons",
                Osc99Features {
                    buttons: Osc99ButtonSupport::Unsupported,
                    ..all_supported()
                },
                "a=focus,report:o=always,unfocused,invisible:u=0,1,2:p=title,body,?,close,icon,alive:c=1:w=1:s=system,silent,error,info,question,warn,warning",
            ),
            (
                "no urgency",
                Osc99Features {
                    urgency: Osc99UrgencySupport::Unsupported,
                    ..all_supported()
                },
                "a=focus,report:o=always,unfocused,invisible:p=title,body,?,close,icon,alive,buttons:c=1:w=1:s=system,silent,error,info,question,warn,warning",
            ),
            (
                "no close events",
                Osc99Features {
                    close_events: Osc99CloseEvents::NotReported,
                    ..all_supported()
                },
                "a=focus,report:o=always,unfocused,invisible:u=0,1,2:p=title,body,?,close,icon,alive,buttons:w=1:s=system,silent,error,info,question,warn,warning",
            ),
            (
                "no auto expiry",
                Osc99Features {
                    auto_expiry: Osc99AutoExpiry::Unsupported,
                    ..all_supported()
                },
                "a=focus,report:o=always,unfocused,invisible:u=0,1,2:p=title,body,?,close,icon,alive,buttons:c=1:s=system,silent,error,info,question,warn,warning",
            ),
        ];
        for (name, features, expected) in cases {
            assert_eq!(osc99_query_reply_body(features), expected, "case: {name}");
        }
    }

    #[test]
    fn query_reply_body_all_supported_is_the_full_reply() {
        assert_eq!(osc99_query_reply_body(all_supported()), ALL_SUPPORTED_REPLY);
    }

    #[test]
    fn query_reply_body_none_supported_has_no_a_key() {
        let body = osc99_query_reply_body(none_supported());
        assert!(body.starts_with("o="), "reply: {body}");
    }

    #[test]
    fn query_reply_body_focus_only_begins_with_a_focus() {
        let body = osc99_query_reply_body(Osc99Features {
            activation_focus: Osc99FocusAction::Supported,
            ..none_supported()
        });
        assert!(body.starts_with("a=focus:"), "reply: {body}");
    }

    #[test]
    fn query_reply_body_report_only_begins_with_a_report() {
        let body = osc99_query_reply_body(Osc99Features {
            activation_report: Osc99ActivationReport::Reported,
            ..none_supported()
        });
        assert!(body.starts_with("a=report:"), "reply: {body}");
    }
}
