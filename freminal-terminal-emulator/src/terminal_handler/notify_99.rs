// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! OSC 99 notification identity and chunk-reassembly state for [`TerminalHandler`].
//!
//! A multi-chunk OSC 99 notification arrives as a series of escape sequences,
//! each carrying `d=0` (more chunks follow) or `d=1`/default (final chunk).
//! This module holds the in-flight accumulator ([`PendingNotification`]), the
//! finalized output type ([`FinalizedNotification`]), and the reassembly method
//! [`TerminalHandler::reassemble_osc99`].

use std::collections::HashMap;

use freminal_common::buffer_states::osc_notify_99::{
    MAX_OSC99_SEQUENCE_BYTES, NotificationOccasion, NotificationUrgency, Osc99Command,
    Osc99PayloadEncoding, Osc99PayloadType,
};
use freminal_common::buffer_states::window_manipulation::{
    Notification99Data, Osc99ControlKind, WindowManipulation,
};
use freminal_common::host_capabilities::{
    Osc99ActivationReport, Osc99CloseEvents, Osc99Features, Osc99Support,
};

use super::TerminalHandler;
use super::chunk_assembler::{BoundedChunkAssembler, ChunkEncoding, ChunkError, ChunkLimits};
use crate::ansi_components::tracer::lossy_sequence_for_log_bounded;

// ── Accumulator ──────────────────────────────────────────────────────────────

/// Accumulator for a multi-chunk OSC 99 notification, keyed by its `i=` id.
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
    /// The most-recent non-payload metadata (id, actions, urgency, occasion,
    /// sound, app name, icon names/cache key, close/report flags, expiry).
    ///
    /// `None` until the first chunk arrives; updated to `Some(chunk)` on each
    /// subsequent chunk.  Later chunks override earlier scalar fields; the
    /// terminating chunk's metadata wins.  The `payload` field of the stored
    /// command is not meaningful here — the accumulators above are
    /// authoritative.
    meta: Option<Osc99Command>,
}

/// Maximum number of distinct in-flight (`i=`) OSC 99 notifications retained
/// while awaiting their terminating chunk. A process emitting endless `d=0`
/// chunks with unique ids could otherwise grow `pending_notifications`
/// without bound. Terminal input is untrusted, so this is a hard cap.
const MAX_PENDING_OSC99_NOTIFICATIONS: usize = 128;

/// Maximum accumulated payload bytes for a single in-flight notification
/// (across title/body/icon/buttons). A stream of `d=0` chunks under one id
/// could otherwise grow a single accumulator without bound.
const MAX_OSC99_NOTIFICATION_BYTES: usize = 1_048_576;

/// Caps for each of a notification's four payload accumulators.
const OSC99_CHUNK_LIMITS: ChunkLimits = ChunkLimits {
    max_chunk_bytes: MAX_OSC99_SEQUENCE_BYTES,
    max_total_bytes: MAX_OSC99_NOTIFICATION_BYTES,
};

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
            title: BoundedChunkAssembler::new(OSC99_CHUNK_LIMITS),
            body: BoundedChunkAssembler::new(OSC99_CHUNK_LIMITS),
            icon: BoundedChunkAssembler::new(OSC99_CHUNK_LIMITS),
            buttons: BoundedChunkAssembler::new(OSC99_CHUNK_LIMITS),
            meta: None,
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

    /// Append `payload` to the accumulator selected by `payload_type`,
    /// decoding it according to `encoding`.
    ///
    /// For `Close`, `Alive`, and `Query` payload types nothing is accumulated
    /// (they are not chunked content payloads); only the metadata is updated
    /// (done by the caller).
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
            // Non-accumulating types: Close, Alive, Query.
            Osc99PayloadType::Close | Osc99PayloadType::Alive | Osc99PayloadType::Query => {
                return Ok(());
            }
        };
        target.push(payload, chunk_encoding(encoding))
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

/// A fully-reassembled OSC 99 notification (all chunks concatenated).
///
/// Produced by [`TerminalHandler::reassemble_osc99`] when a terminating
/// (`done == true`) chunk arrives. Task 99.4 maps this into
/// `WindowManipulation::Notification99` for transport to the GUI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinalizedNotification {
    /// The concatenated title text (UTF-8), if any title chunks arrived.
    pub title: Option<String>,
    /// The concatenated body text (UTF-8), if any body chunks arrived.
    pub body: Option<String>,
    /// The concatenated icon bytes, if any icon chunks arrived.
    pub icon: Option<Vec<u8>>,
    /// Button labels (`p=buttons`), split from the U+2028-separated payload.
    /// Empty if no button chunks arrived.
    pub buttons: Vec<String>,
    /// The terminating chunk's metadata (id, actions, urgency, occasion, sound,
    /// app name, icon names/cache key, close/report flags, expiry, `payload_type`).
    pub meta: Osc99Command,
}

impl FinalizedNotification {
    /// Map this finalized OSC 99 notification into the transport shell
    /// [`Notification99Data`] carried by `WindowManipulation::Notification99`.
    ///
    /// Field mapping (Task 99.4 execution decisions): `id`/`title`/`body`
    /// direct; `icon_data` from `self.icon`; `icon_names`/`icon_cache_key`/
    /// `sound`/`app_name`/`notification_type` from `meta`;
    /// `report_activation`/`focus_on_activation` from `meta.actions`;
    /// `close_report` from `meta.close_report`; `urgency` maps
    /// `Low`/`Normal`/`Critical` to `Some(0)`/`Some(1)`/`Some(2)`; `occasion`
    /// maps `Always` to `None` (behaviourally identical to unset),
    /// `Unfocused`/`Invisible` to `Some("unfocused")`/`Some("invisible")`;
    /// `expire_ms` maps the spec's `-1` "OS default" sentinel to `None`, any
    /// other value to `Some(v)`. `button_labels` comes directly from
    /// `self.buttons` (the split `p=buttons` payload).
    pub(in crate::terminal_handler) fn into_notification99_data(self) -> Notification99Data {
        let urgency = self.meta.urgency.map(|u| match u {
            NotificationUrgency::Low => 0u8,
            NotificationUrgency::Normal => 1u8,
            NotificationUrgency::Critical => 2u8,
        });
        let occasion = match self.meta.occasion {
            // Always is the default — behaviourally identical to unset → None.
            NotificationOccasion::Always => None,
            NotificationOccasion::Unfocused => Some("unfocused".to_owned()),
            NotificationOccasion::Invisible => Some("invisible".to_owned()),
        };
        // -1 is the spec's "OS default" sentinel → None; any other value → Some.
        let expire_ms = (self.meta.expire_ms != -1).then_some(self.meta.expire_ms);
        Notification99Data {
            id: self.meta.id,
            title: self.title,
            body: self.body,
            icon_data: self.icon,
            icon_names: self.meta.icon_names,
            icon_cache_key: self.meta.icon_cache_key,
            button_labels: self.buttons,
            report_activation: self.meta.actions.report_activation,
            focus_on_activation: self.meta.actions.focus_on_activation,
            close_report: self.meta.close_report,
            urgency,
            occasion,
            sound: self.meta.sound,
            app_name: self.meta.app_name,
            notification_type: self.meta.notification_type,
            expire_ms,
        }
    }
}

// ── Control payload routing (Task 99.5c) ─────────────────────────────────────

/// Map an OSC 99 payload type to its GUI-bound control kind, if it is one
/// (`p=close`/`p=alive`).
///
/// `p=?` also returns `None`: the handler answers it itself and never forwards
/// it to the GUI (Task 130.5), so the `Notify99` arm checks for
/// [`Osc99PayloadType::Query`] before consulting this function. Display
/// payload types (`Title`/`Body`/`Icon`/`Buttons`) return `None` too — they
/// keep flowing to `WindowManipulation::Notification99` in
/// `terminal_handler/osc.rs`.
///
/// `Osc99PayloadType` is defined in `freminal-common`, so this cannot be an
/// inherent method on it from this crate (orphan rule) — a free function is
/// the correct shape.
pub(in crate::terminal_handler) const fn control_kind(
    payload_type: Osc99PayloadType,
) -> Option<Osc99ControlKind> {
    match payload_type {
        Osc99PayloadType::Close => Some(Osc99ControlKind::Close),
        Osc99PayloadType::Alive => Some(Osc99ControlKind::Alive),
        Osc99PayloadType::Query
        | Osc99PayloadType::Title
        | Osc99PayloadType::Body
        | Osc99PayloadType::Icon
        | Osc99PayloadType::Buttons => None,
    }
}

/// Whether an OSC 99 payload type is a control request (`p=close`,
/// `p=alive` or `p=?`) rather than display content.
///
/// Unlike [`control_kind`], this includes `p=?`, which the handler answers
/// itself and so has no GUI-bound control kind.
pub(in crate::terminal_handler) const fn is_control_payload(
    payload_type: Osc99PayloadType,
) -> bool {
    match payload_type {
        Osc99PayloadType::Close | Osc99PayloadType::Alive | Osc99PayloadType::Query => true,
        Osc99PayloadType::Title
        | Osc99PayloadType::Body
        | Osc99PayloadType::Icon
        | Osc99PayloadType::Buttons => false,
    }
}

// ── Capability query reply (Task 130.5) ──────────────────────────────────────

/// Build the capability list of the reply to an OSC 99 `p=?` query, for a host
/// that honours OSC 99 with the given `features`.
///
/// The result is the part after the `;` in
/// `ESC ] 99 ; i=<id> : p=? ; <capabilities> ST`: colon-separated `key=value`
/// pairs in a stable order, with no leading or trailing `:`. It advertises
/// only what is genuinely implemented (the truthful-advertisement rule from
/// Task 76):
///
/// - `a=report` (NOT `focus` — `focus_on_activation` is parsed but freminal
///   does not act on it), present only when activation can be reported. With
///   no supported actions the `a` key is omitted entirely, per the spec's
///   query table ("If no actions are supported, the `a` key must be
///   absent");
/// - `c=1` (close reports), present only when close events can be reported;
/// - `o=` all three occasions;
/// - `p=` the payload types freminal handles (display types plus the control
///   types it answers);
/// - `s=system,silent` (forwarded freedesktop sound-name hints — freminal
///   forwards the name, playback is the daemon's concern);
/// - `u=0,1,2` (urgency levels — advertised even though the setter is
///   unavailable on macOS, matching Task 76's handling of the same gap);
/// - `w=1` (auto-expiry, wired via `.timeout()`).
///
/// Full conformance of the advertised set against the spec is Task 138.
pub(in crate::terminal_handler) fn osc99_query_reply_body(features: Osc99Features) -> String {
    let mut keys: Vec<&str> = Vec::with_capacity(7);
    if features.activation_report == Osc99ActivationReport::Reported {
        keys.push("a=report");
    }
    if features.close_events == Osc99CloseEvents::Reported {
        keys.push("c=1");
    }
    keys.extend([
        "o=always,unfocused,invisible",
        "p=title,body,icon,buttons,alive,close,?",
        "s=system,silent",
        "u=0,1,2",
        "w=1",
    ]);
    keys.join(":")
}

// ── Helper: build a FinalizedNotification from accumulated bytes + meta ───────

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

/// Build a [`FinalizedNotification`] from the finished title/body/icon/
/// buttons bytes and the final `meta` command. The terminating chunk's
/// payload must already have been pushed into the appropriate accumulator
/// before the accumulators were finished.
fn build_finalized(payloads: FinishedPayloads, meta: Osc99Command) -> FinalizedNotification {
    let title = if payloads.title.is_empty() {
        None
    } else {
        Some(String::from_utf8_lossy(&payloads.title).into_owned())
    };
    let body = if payloads.body.is_empty() {
        None
    } else {
        Some(String::from_utf8_lossy(&payloads.body).into_owned())
    };
    let icon = if payloads.icon.is_empty() {
        None
    } else {
        Some(payloads.icon)
    };
    FinalizedNotification {
        title,
        body,
        icon,
        buttons: split_button_labels(&payloads.buttons),
        meta,
    }
}

// ── PendingNotifications type alias ──────────────────────────────────────────

/// The state held for one `i=` identifier in [`PendingNotifications`].
#[derive(Debug)]
pub(in crate::terminal_handler) enum PendingEntry {
    /// Chunks are being accumulated; the notification is still valid.
    ///
    /// Boxed so the tombstone variant does not pay for four assemblers.
    Accumulating(Box<PendingNotification>),
    /// The notification was dropped mid-transfer (invalid base64 or a cap
    /// breach). The id is remembered as a tombstone so the remaining chunks
    /// of the same transfer are discarded instead of starting a fresh
    /// accumulation from a tail fragment. The terminating (`d=1`) chunk
    /// removes it. It counts toward [`MAX_PENDING_OSC99_NOTIFICATIONS`] like
    /// any other pending entry, so the map stays bounded.
    Dropped,
}

/// Map of in-flight OSC 99 notifications keyed by their `i=` identifier.
pub(in crate::terminal_handler) type PendingNotifications = HashMap<String, PendingEntry>;

// ── Reassembly impl on TerminalHandler ───────────────────────────────────────

impl TerminalHandler {
    /// Abandon the in-flight notification `id` after a decode or cap failure.
    ///
    /// If the offending chunk was the terminating one (`done`) the
    /// notification is over and the entry is removed. Otherwise the id is
    /// kept as a [`PendingEntry::Dropped`] tombstone so the remaining chunks
    /// of the same transfer cannot start a fresh, truncated notification.
    fn abandon_notification(&mut self, id: String, done: bool) {
        if done {
            self.pending_notifications.remove(&id);
        } else {
            self.pending_notifications.insert(id, PendingEntry::Dropped);
        }
    }

    /// Feed one parsed OSC 99 chunk into the notification reassembly machine.
    ///
    /// Returns `Some(finalized)` when the chunk terminates a notification
    /// (`done == true`), where `finalized` carries the fully-concatenated
    /// bytes for that notification's payload fields and whose metadata reflects
    /// the latest chunk. Returns `None` while more chunks are expected
    /// (`done == false`).
    ///
    /// Chunking is keyed by `i=`. A chunk WITHOUT an `i=` id is never merged
    /// into the pending map: if it is `done`, it finalizes immediately as a
    /// standalone notification; if `done == false` (a non-final chunk with no
    /// id), it is dropped and `None` is returned.
    /// An `i=` seen again after finalize starts a fresh accumulation.
    ///
    /// A notification dropped mid-transfer (invalid base64, or a size cap)
    /// leaves a tombstone for its id: further content chunks are ignored and
    /// the terminating chunk removes the tombstone without emitting anything.
    /// Control chunks (`p=close`/`p=alive`/`p=?`) are not swallowed by a
    /// tombstone; they are handled as if the id had no entry.
    pub(in crate::terminal_handler) fn reassemble_osc99(
        &mut self,
        chunk: Osc99Command,
    ) -> Option<FinalizedNotification> {
        match &chunk.id {
            // ── No id: standalone or drop ────────────────────────────────────
            None => {
                if chunk.done {
                    // Standalone, single-chunk notification — decode it
                    // through a temporary accumulator and finalize immediately.
                    let mut pending = PendingNotification::default();
                    let decoded = pending
                        .push_payload(chunk.payload_type, &chunk.payload, chunk.payload_encoding)
                        .and_then(|()| pending.finish_payloads());
                    match decoded {
                        Ok(payloads) => Some(build_finalized(payloads, chunk)),
                        Err(err) => {
                            tracing::debug!("OSC 99: dropping standalone notification: {err}");
                            None
                        }
                    }
                } else {
                    // Non-final chunk with no id — no key to accumulate under; drop.
                    tracing::trace!(
                        "OSC 99: dropping non-final chunk with no id (no key to accumulate under)"
                    );
                    None
                }
            }

            // ── Has id: accumulate or finalize ────────────────────────────────
            Some(id) => {
                let id = id.clone();

                let done = chunk.done;

                if matches!(
                    self.pending_notifications.get(&id),
                    Some(PendingEntry::Dropped)
                ) {
                    if is_control_payload(chunk.payload_type) {
                        // A control request (`p=close`/`p=alive`/`p=?`) is not
                        // part of the dropped content transfer: forget the
                        // tombstone and handle it as if no entry existed.
                        self.pending_notifications.remove(&id);
                    } else {
                        // The rest of a dropped transfer: ignore the payload;
                        // the terminating chunk ends the notification.
                        if done {
                            self.pending_notifications.remove(&id);
                        }
                        tracing::trace!("OSC 99: ignoring chunk of a dropped notification");
                        return None;
                    }
                }

                // Bound the number of concurrent in-flight notifications: a
                // new id is refused once the map is full (existing ids may
                // still receive their remaining chunks and finalize).
                if !self.pending_notifications.contains_key(&id)
                    && self.pending_notifications.len() >= MAX_PENDING_OSC99_NOTIFICATIONS
                {
                    tracing::debug!(
                        "OSC 99: dropping chunk; too many pending notifications ({})",
                        MAX_PENDING_OSC99_NOTIFICATIONS
                    );
                    return None;
                }

                let PendingEntry::Accumulating(entry) = self
                    .pending_notifications
                    .entry(id.clone())
                    .or_insert_with(|| PendingEntry::Accumulating(Box::default()))
                else {
                    // Unreachable: a tombstone was handled above.
                    return None;
                };

                // Append the chunk's payload to the matching accumulator. An
                // invalid base64 stream or an exceeded cap drops the whole
                // in-flight notification rather than keeping a partial one.
                if let Err(err) =
                    entry.push_payload(chunk.payload_type, &chunk.payload, chunk.payload_encoding)
                {
                    self.abandon_notification(id.clone(), done);
                    tracing::debug!(
                        "OSC 99: dropping notification {:?}: {err}",
                        lossy_sequence_for_log_bounded(id.as_bytes())
                    );
                    return None;
                }

                // Bound the accumulated payload for this id across all four
                // accumulators: if it now exceeds the per-notification cap,
                // drop the whole in-flight accumulation rather than grow
                // without limit.
                if entry.accumulated_len() > MAX_OSC99_NOTIFICATION_BYTES {
                    self.abandon_notification(id, done);
                    tracing::debug!(
                        "OSC 99: dropping notification; accumulated payload exceeds {} bytes",
                        MAX_OSC99_NOTIFICATION_BYTES
                    );
                    return None;
                }

                // Latest metadata wins (payload bytes are accumulated above, not here).
                // Move (not clone) the payload out of the chunk before storing
                // metadata, so a large final icon/body chunk is not retained
                // twice (once in the accumulator, once in `meta`).
                let mut chunk = chunk;
                chunk.payload = Vec::new();
                entry.meta = Some(chunk);

                if done {
                    // Remove from the map and build the finalized notification.
                    let Some(PendingEntry::Accumulating(mut entry)) =
                        self.pending_notifications.remove(&id)
                    else {
                        return None;
                    };
                    // SAFETY: we just set entry.meta = Some(chunk) before inserting,
                    // so this is always Some when we removed a live entry.
                    let meta = entry.meta.take()?;

                    match entry.finish_payloads() {
                        Ok(payloads) => Some(build_finalized(payloads, meta)),
                        Err(err) => {
                            tracing::debug!(
                                "OSC 99: dropping notification {:?}: {err}",
                                lossy_sequence_for_log_bounded(id.as_bytes())
                            );
                            None
                        }
                    }
                } else {
                    None
                }
            }
        }
    }

    /// Act on a finalized OSC 99 request, branching on its payload type
    /// (Tasks 99.5c and 130.5).
    ///
    /// - `p=?` is answered here, from [`TerminalHandler::host_capabilities`],
    ///   and never reaches the GUI. While OSC 99 is unsupported it is not
    ///   answered at all, so the application sees a terminal that does not
    ///   speak the protocol.
    /// - `p=alive` is dropped while OSC 99 is unsupported (no notification
    ///   can be live), and forwarded to the GUI otherwise.
    /// - `p=close` is always forwarded.
    /// - Display payloads push `Notification99` as before.
    pub(in crate::terminal_handler) fn dispatch_finalized_osc99(
        &mut self,
        finalized: FinalizedNotification,
    ) {
        let support = self.host_capabilities().osc99;

        if finalized.meta.payload_type == Osc99PayloadType::Query {
            match support {
                Osc99Support::Supported(features) => {
                    // The id was validated by the OSC 99 parser (plain
                    // identifier characters only), so it is safe to echo.
                    let id = finalized.meta.id.as_deref().unwrap_or("0");
                    let caps = osc99_query_reply_body(features);
                    self.write_osc_response(&format!("99;i={id}:p=?;{caps}"));
                }
                Osc99Support::Unsupported => {
                    tracing::debug!("OSC 99 p=? query ignored: OSC 99 is unsupported");
                }
            }
            return;
        }

        if finalized.meta.payload_type == Osc99PayloadType::Alive
            && support == Osc99Support::Unsupported
        {
            tracing::debug!("OSC 99 p=alive ignored: OSC 99 is unsupported");
            return;
        }

        if let Some(kind) = control_kind(finalized.meta.payload_type) {
            self.window_commands.push(WindowManipulation::Osc99Control {
                id: finalized.meta.id,
                kind,
            });
        } else {
            self.window_commands
                .push(WindowManipulation::Notification99(Box::new(
                    finalized.into_notification99_data(),
                )));
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use freminal_common::buffer_states::osc_notify_99::{Osc99Actions, parse_osc_99};

    /// Build a default `Osc99Command` for use in mapping tests.
    fn default_cmd() -> Osc99Command {
        Osc99Command {
            id: None,
            payload_type: Osc99PayloadType::Title,
            done: true,
            payload: Vec::new(),
            payload_encoding: Osc99PayloadEncoding::Plain,
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

    /// Build a default `FinalizedNotification` wrapping `default_cmd()`.
    fn default_finalized() -> FinalizedNotification {
        FinalizedNotification {
            title: None,
            body: None,
            icon: None,
            buttons: Vec::new(),
            meta: default_cmd(),
        }
    }

    #[test]
    fn urgency_maps_critical_to_some_two() {
        let finalized = FinalizedNotification {
            meta: Osc99Command {
                urgency: Some(NotificationUrgency::Critical),
                ..default_cmd()
            },
            ..default_finalized()
        };
        let data = finalized.into_notification99_data();
        assert_eq!(data.urgency, Some(2));
    }

    #[test]
    fn urgency_low_and_normal_map_to_zero_and_one() {
        let low = FinalizedNotification {
            meta: Osc99Command {
                urgency: Some(NotificationUrgency::Low),
                ..default_cmd()
            },
            ..default_finalized()
        };
        assert_eq!(low.into_notification99_data().urgency, Some(0));

        let normal = FinalizedNotification {
            meta: Osc99Command {
                urgency: Some(NotificationUrgency::Normal),
                ..default_cmd()
            },
            ..default_finalized()
        };
        assert_eq!(normal.into_notification99_data().urgency, Some(1));
    }

    #[test]
    fn urgency_none_maps_to_none() {
        let finalized = default_finalized();
        let data = finalized.into_notification99_data();
        assert_eq!(data.urgency, None);
    }

    #[test]
    fn occasion_always_maps_to_none() {
        let finalized = FinalizedNotification {
            meta: Osc99Command {
                occasion: NotificationOccasion::Always,
                ..default_cmd()
            },
            ..default_finalized()
        };
        let data = finalized.into_notification99_data();
        assert_eq!(data.occasion, None);
    }

    #[test]
    fn occasion_unfocused_maps_to_some_string() {
        let finalized = FinalizedNotification {
            meta: Osc99Command {
                occasion: NotificationOccasion::Unfocused,
                ..default_cmd()
            },
            ..default_finalized()
        };
        let data = finalized.into_notification99_data();
        assert_eq!(data.occasion.as_deref(), Some("unfocused"));
    }

    #[test]
    fn occasion_invisible_maps_to_some_string() {
        let finalized = FinalizedNotification {
            meta: Osc99Command {
                occasion: NotificationOccasion::Invisible,
                ..default_cmd()
            },
            ..default_finalized()
        };
        let data = finalized.into_notification99_data();
        assert_eq!(data.occasion.as_deref(), Some("invisible"));
    }

    #[test]
    fn expire_ms_minus_one_maps_to_none() {
        let finalized = FinalizedNotification {
            meta: Osc99Command {
                expire_ms: -1,
                ..default_cmd()
            },
            ..default_finalized()
        };
        let data = finalized.into_notification99_data();
        assert_eq!(data.expire_ms, None);
    }

    #[test]
    fn expire_ms_5000_maps_to_some_5000() {
        let finalized = FinalizedNotification {
            meta: Osc99Command {
                expire_ms: 5000,
                ..default_cmd()
            },
            ..default_finalized()
        };
        let data = finalized.into_notification99_data();
        assert_eq!(data.expire_ms, Some(5000));
    }

    #[test]
    fn title_body_icon_and_flags_carried_through() {
        let finalized = FinalizedNotification {
            title: Some("Title".to_owned()),
            body: Some("Body".to_owned()),
            icon: Some(vec![1, 2, 3]),
            buttons: Vec::new(),
            meta: Osc99Command {
                id: Some("id-1".to_owned()),
                close_report: true,
                actions: Osc99Actions {
                    report_activation: true,
                    focus_on_activation: false,
                },
                ..default_cmd()
            },
        };
        let data = finalized.into_notification99_data();
        assert_eq!(data.id.as_deref(), Some("id-1"));
        assert_eq!(data.title.as_deref(), Some("Title"));
        assert_eq!(data.body.as_deref(), Some("Body"));
        assert_eq!(data.icon_data.as_deref(), Some(&[1u8, 2, 3][..]));
        assert!(data.report_activation);
        assert!(!data.focus_on_activation);
        assert!(data.close_report);
        assert_eq!(data.button_labels, [] as [String; 0]);
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

    /// Standalone (no-id), single-chunk `done` `Buttons` payload finalizes
    /// immediately with the split labels.
    #[test]
    fn standalone_done_buttons_no_id_splits_labels() {
        let mut handler = TerminalHandler::new(80, 24);
        let cmd = Osc99Command {
            payload_type: Osc99PayloadType::Buttons,
            payload: "OK\u{2028}Cancel".as_bytes().to_vec(),
            ..default_cmd()
        };
        let finalized = handler
            .reassemble_osc99(cmd)
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

        let chunk1 = Osc99Command {
            id: Some("btn-1".to_owned()),
            payload_type: Osc99PayloadType::Buttons,
            done: false,
            payload: "Yes\u{2028}N".as_bytes().to_vec(),
            ..default_cmd()
        };
        let r1 = handler.reassemble_osc99(chunk1);
        assert!(r1.is_none(), "first chunk must not finalize");

        let chunk2 = Osc99Command {
            id: Some("btn-1".to_owned()),
            payload_type: Osc99PayloadType::Buttons,
            done: true,
            payload: b"o".to_vec(),
            ..default_cmd()
        };
        let finalized = handler
            .reassemble_osc99(chunk2)
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
        assert_eq!(data.button_labels, vec!["A".to_owned(), "B".to_owned()]);
    }

    // ── 99.5c: control_kind mapping ───────────────────────────────────────────

    #[test]
    fn control_kind_maps_close_and_alive() {
        assert_eq!(
            control_kind(Osc99PayloadType::Close),
            Some(Osc99ControlKind::Close)
        );
        assert_eq!(
            control_kind(Osc99PayloadType::Alive),
            Some(Osc99ControlKind::Alive)
        );
    }

    #[test]
    fn control_kind_maps_query_to_none() {
        // The handler answers `p=?` itself; it is never forwarded to the GUI.
        assert_eq!(control_kind(Osc99PayloadType::Query), None);
    }

    #[test]
    fn is_control_payload_covers_close_alive_query_only() {
        assert!(is_control_payload(Osc99PayloadType::Close));
        assert!(is_control_payload(Osc99PayloadType::Alive));
        assert!(is_control_payload(Osc99PayloadType::Query));
        assert!(!is_control_payload(Osc99PayloadType::Title));
        assert!(!is_control_payload(Osc99PayloadType::Body));
        assert!(!is_control_payload(Osc99PayloadType::Icon));
        assert!(!is_control_payload(Osc99PayloadType::Buttons));
    }

    #[test]
    fn control_kind_maps_display_types_to_none() {
        assert_eq!(control_kind(Osc99PayloadType::Title), None);
        assert_eq!(control_kind(Osc99PayloadType::Body), None);
        assert_eq!(control_kind(Osc99PayloadType::Icon), None);
        assert_eq!(control_kind(Osc99PayloadType::Buttons), None);
    }

    // ── Reassembly bounds (memory-growth guards) ──────────────────────────────

    #[test]
    fn reassembly_caps_number_of_pending_notifications() {
        let mut handler = TerminalHandler::new(80, 24);

        // Fill the pending map to the cap with distinct, unfinished ids.
        for i in 0..MAX_PENDING_OSC99_NOTIFICATIONS {
            let chunk = Osc99Command {
                id: Some(format!("id-{i}")),
                payload_type: Osc99PayloadType::Title,
                done: false,
                payload: b"x".to_vec(),
                ..default_cmd()
            };
            assert!(handler.reassemble_osc99(chunk).is_none());
        }
        assert_eq!(
            handler.pending_notifications.len(),
            MAX_PENDING_OSC99_NOTIFICATIONS
        );

        // One more distinct id is refused; the map does not grow.
        let overflow = Osc99Command {
            id: Some("overflow".to_owned()),
            payload_type: Osc99PayloadType::Title,
            done: false,
            payload: b"x".to_vec(),
            ..default_cmd()
        };
        assert!(handler.reassemble_osc99(overflow).is_none());
        assert_eq!(
            handler.pending_notifications.len(),
            MAX_PENDING_OSC99_NOTIFICATIONS
        );

        // An existing id can still receive chunks and finalize.
        let finish = Osc99Command {
            id: Some("id-0".to_owned()),
            payload_type: Osc99PayloadType::Title,
            done: true,
            payload: b"y".to_vec(),
            ..default_cmd()
        };
        assert!(handler.reassemble_osc99(finish).is_some());
    }

    #[test]
    fn reassembly_drops_notification_exceeding_byte_cap() {
        let mut handler = TerminalHandler::new(80, 24);
        let id = "big".to_owned();

        // First chunk under the cap: accepted, still pending.
        let chunk1 = Osc99Command {
            id: Some(id.clone()),
            payload_type: Osc99PayloadType::Body,
            done: false,
            payload: vec![b'a'; 1024],
            ..default_cmd()
        };
        assert!(handler.reassemble_osc99(chunk1).is_none());
        assert!(handler.pending_notifications.contains_key(&id));

        // Second chunk that would push the accumulation past the cap: the
        // whole in-flight notification is dropped rather than grown.
        let chunk2 = Osc99Command {
            id: Some(id.clone()),
            payload_type: Osc99PayloadType::Body,
            done: false,
            payload: vec![b'b'; MAX_OSC99_NOTIFICATION_BYTES],
            ..default_cmd()
        };
        assert!(handler.reassemble_osc99(chunk2).is_none());
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
            done: false,
            payload: b"hello".to_vec(),
            ..default_cmd()
        };
        assert!(handler.reassemble_osc99(chunk).is_none());

        // The stored metadata must not keep a copy of the payload bytes: they
        // live only in the body accumulator now.
        let Some(PendingEntry::Accumulating(entry)) = handler.pending_notifications.get(&id) else {
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
            done: true,
            payload: Vec::new(),
            ..default_cmd()
        };
        let finalized = handler
            .reassemble_osc99(done)
            .expect("terminating chunk finalizes");
        assert_eq!(finalized.body.as_deref(), Some("hello"));
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
        handler.reassemble_osc99(cmd)
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
            assert_eq!(
                finalized.title.as_deref(),
                Some(SAMPLE_TITLE),
                "split at {at}"
            );
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
            assert_eq!(
                finalized.title.as_deref(),
                Some(SAMPLE_TITLE),
                "split at {at}"
            );
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
        assert_eq!(
            finalized.title.as_deref(),
            Some("Hello, notification world")
        );
        assert!(handler.pending_notifications.is_empty());
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
        assert_eq!(finalized.body.as_deref(), Some("Hello, world!"));
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
        assert_eq!(finalized.title.as_deref(), Some("Hello"));
        assert_eq!(finalized.body.as_deref(), Some("plain body"));
    }

    /// Invalid base64 in a middle chunk drops the whole in-flight
    /// notification immediately.
    #[test]
    fn invalid_base64_in_middle_chunk_drops_notification() {
        let mut handler = TerminalHandler::new(80, 24);
        assert!(feed(&mut handler, "i=bad:d=0:e=1", b"SGVs").is_none());
        assert!(handler.pending_notifications.contains_key("bad"));
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
        assert!(handler.pending_notifications.is_empty());
    }

    /// Standalone (no id) base64 payload still decodes.
    #[test]
    fn standalone_base64_title_decodes() {
        let mut handler = TerminalHandler::new(80, 24);
        let finalized = feed(&mut handler, "e=1", b"SGVsbG8=").expect("must finalize");
        assert_eq!(finalized.title.as_deref(), Some("Hello"));
        assert!(handler.pending_notifications.is_empty());
    }

    /// Standalone base64 without trailing padding still decodes.
    #[test]
    fn standalone_unpadded_base64_title_decodes() {
        let mut handler = TerminalHandler::new(80, 24);
        let finalized = feed(&mut handler, "e=1", b"SGVsbG8").expect("must finalize");
        assert_eq!(finalized.title.as_deref(), Some("Hello"));
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
        assert_eq!(finalized.body.as_deref(), Some("Hello"));
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
        assert!(handler.pending_notifications.contains_key(id));
        assert!(feed(&mut handler, "i=huge:p=icon:d=0:e=1", &chunk).is_none());
        assert!(is_dropped(&handler, id));
    }

    /// PR #536 review: the combined cap must also hold for the bytes a pending
    /// base64 tail adds when the accumulators are finished.
    #[test]
    fn base64_tail_flushed_at_finish_cannot_exceed_the_combined_cap() {
        let half = vec![b't'; MAX_OSC99_NOTIFICATION_BYTES / 2];
        let mut handler = TerminalHandler::new(80, 24);
        // Exactly 1 MiB of plain title: at the cap, still accepted.
        assert!(feed(&mut handler, "i=cap:d=0:p=title", &half).is_none());
        assert!(feed(&mut handler, "i=cap:d=0:p=title", &half).is_none());
        // `YQ` decodes to one byte, but only when the stream is finished.
        assert!(feed(&mut handler, "i=cap:d=1:p=body:e=1", b"YQ").is_none());
        assert!(!handler.pending_notifications.contains_key("cap"));

        // The same shape one byte smaller finalises with both parts intact.
        let mut handler = TerminalHandler::new(80, 24);
        let short = vec![b't'; MAX_OSC99_NOTIFICATION_BYTES / 2 - 1];
        assert!(feed(&mut handler, "i=ok:d=0:p=title", &half).is_none());
        assert!(feed(&mut handler, "i=ok:d=0:p=title", &short).is_none());
        let done = feed(&mut handler, "i=ok:d=1:p=body:e=1", b"YQ").expect("within the cap");
        assert_eq!(done.body.as_deref(), Some("a"));
        assert_eq!(
            done.title.map(|t| t.len()),
            Some(MAX_OSC99_NOTIFICATION_BYTES - 1)
        );
    }

    // ── Tombstones for notifications dropped mid-transfer ────────────────────

    /// Whether `id` is currently held as a [`PendingEntry::Dropped`] tombstone.
    fn is_dropped(handler: &TerminalHandler, id: &str) -> bool {
        matches!(
            handler.pending_notifications.get(id),
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
            handler.pending_notifications.is_empty(),
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
        assert_eq!(finalized.title.as_deref(), Some("Fresh start"));
        assert!(handler.pending_notifications.is_empty());
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
        assert_eq!(handler.pending_notifications.len(), 1);
    }

    /// If the offending chunk is itself the terminating one, the notification
    /// is over: no tombstone is left behind.
    #[test]
    fn invalid_terminating_chunk_leaves_no_tombstone() {
        let mut handler = TerminalHandler::new(80, 24);
        assert!(feed(&mut handler, "i=1:d=0:p=title:e=1", b"SGVs").is_none());
        assert!(feed(&mut handler, "i=1:d=1:p=title:e=1", b"@@@@").is_none());
        assert!(handler.pending_notifications.is_empty());
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
        assert!(handler.pending_notifications.is_empty());
    }

    /// Tombstones occupy slots in the pending map like any other entry, so
    /// the 128-entry cap bounds them too.
    #[test]
    fn tombstones_count_toward_pending_cap() {
        let mut handler = TerminalHandler::new(80, 24);
        for i in 0..MAX_PENDING_OSC99_NOTIFICATIONS {
            let metadata = format!("i=id-{i}:d=0:p=title:e=1");
            assert!(feed(&mut handler, &metadata, b"@@@@").is_none());
        }
        assert_eq!(
            handler.pending_notifications.len(),
            MAX_PENDING_OSC99_NOTIFICATIONS
        );
        assert!(
            handler
                .pending_notifications
                .values()
                .all(|e| matches!(e, PendingEntry::Dropped))
        );

        // A new id is refused: the map does not grow past the cap.
        assert!(feed(&mut handler, "i=overflow:d=0:p=title", b"x").is_none());
        assert_eq!(
            handler.pending_notifications.len(),
            MAX_PENDING_OSC99_NOTIFICATIONS
        );
        assert!(!handler.pending_notifications.contains_key("overflow"));

        // Terminating a tombstoned id frees its slot.
        assert!(feed(&mut handler, "i=id-0:d=1:p=title", b"x").is_none());
        assert_eq!(
            handler.pending_notifications.len(),
            MAX_PENDING_OSC99_NOTIFICATIONS - 1
        );
    }

    /// A control request for a tombstoned id is handled as if the id had no
    /// entry: a done `p=close` finalizes as a control notification and the
    /// tombstone is gone.
    #[test]
    fn close_for_tombstoned_id_behaves_as_if_no_entry() {
        let mut handler = TerminalHandler::new(80, 24);
        assert!(feed(&mut handler, "i=1:d=0:p=title:e=1", b"@@@@").is_none());
        assert!(is_dropped(&handler, "1"));

        let finalized = feed(&mut handler, "i=1:p=close", b"").expect("close must finalize");
        assert_eq!(finalized.meta.payload_type, Osc99PayloadType::Close);
        assert_eq!(finalized.meta.id.as_deref(), Some("1"));
        assert!(finalized.title.is_none());
        assert!(handler.pending_notifications.is_empty());
    }

    /// Same for `p=alive` and `p=?`, and a non-final control chunk leaves a
    /// normal accumulating entry (not a tombstone), as with no prior entry.
    #[test]
    fn alive_and_query_for_tombstoned_id_behave_as_if_no_entry() {
        for (metadata, kind) in [
            ("i=1:p=alive", Osc99PayloadType::Alive),
            ("i=1:p=?", Osc99PayloadType::Query),
        ] {
            let mut handler = TerminalHandler::new(80, 24);
            assert!(feed(&mut handler, "i=1:d=0:p=title:e=1", b"@@@@").is_none());
            let finalized = feed(&mut handler, metadata, b"").expect("must finalize");
            assert_eq!(finalized.meta.payload_type, kind);
            assert!(handler.pending_notifications.is_empty());
        }

        let mut handler = TerminalHandler::new(80, 24);
        assert!(feed(&mut handler, "i=1:d=0:p=title:e=1", b"@@@@").is_none());
        assert!(feed(&mut handler, "i=1:d=0:p=close", b"").is_none());
        assert!(!is_dropped(&handler, "1"));
        assert!(handler.pending_notifications.contains_key("1"));
    }

    // ── 130.5: osc99_query_reply_body ─────────────────────────────────────────

    const REPLY_TAIL: &str = "o=always,unfocused,invisible:p=title,body,icon,buttons,alive,close,?:s=system,silent:u=0,1,2:w=1";

    #[test]
    fn query_reply_body_full_features_matches_legacy_string() {
        let features = Osc99Features {
            activation_report: Osc99ActivationReport::Reported,
            close_events: Osc99CloseEvents::Reported,
        };
        assert_eq!(
            osc99_query_reply_body(features),
            "a=report:c=1:o=always,unfocused,invisible:p=title,body,icon,buttons,alive,close,?:s=system,silent:u=0,1,2:w=1"
        );
    }

    #[test]
    fn query_reply_body_activation_not_reported_omits_a_key() {
        let features = Osc99Features {
            activation_report: Osc99ActivationReport::NotReported,
            close_events: Osc99CloseEvents::Reported,
        };
        assert_eq!(
            osc99_query_reply_body(features),
            format!("c=1:{REPLY_TAIL}")
        );
    }

    #[test]
    fn query_reply_body_close_not_reported_omits_c_key() {
        let features = Osc99Features {
            activation_report: Osc99ActivationReport::Reported,
            close_events: Osc99CloseEvents::NotReported,
        };
        assert_eq!(
            osc99_query_reply_body(features),
            format!("a=report:{REPLY_TAIL}")
        );
    }

    #[test]
    fn query_reply_body_neither_reported_starts_with_occasion() {
        let features = Osc99Features {
            activation_report: Osc99ActivationReport::NotReported,
            close_events: Osc99CloseEvents::NotReported,
        };
        assert_eq!(osc99_query_reply_body(features), REPLY_TAIL);
    }
}
