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

use freminal_common::buffer_states::osc_notify_99::{Osc99Command, Osc99PayloadType};

use super::TerminalHandler;

// ── Accumulator ──────────────────────────────────────────────────────────────

/// Accumulator for a multi-chunk OSC 99 notification, keyed by its `i=` id.
///
/// OSC 99 notifications may arrive across multiple escape sequences: each
/// non-final chunk carries `d=0`, and the final chunk carries `d=1` (or omits
/// `d`, defaulting to done). Same-typed payloads (`p=title`/`p=body`) are
/// concatenated across chunks. This holds the in-flight accumulation until the
/// terminating chunk finalizes it.
#[derive(Debug, Clone, Default)]
pub(in crate::terminal_handler) struct PendingNotification {
    /// Accumulated `p=title` payload bytes.
    title: Vec<u8>,
    /// Accumulated `p=body` payload bytes.
    body: Vec<u8>,
    /// Accumulated `p=icon` payload bytes.
    icon: Vec<u8>,
    /// The most-recent non-payload metadata (id, actions, urgency, occasion,
    /// sound, app name, icon names/cache key, close/report flags, expiry).
    ///
    /// `None` until the first chunk arrives; updated to `Some(chunk)` on each
    /// subsequent chunk.  Later chunks override earlier scalar fields; the
    /// terminating chunk's metadata wins.  The `payload` field of the stored
    /// command is not meaningful here — the accumulated title/body/icon vecs
    /// above are authoritative.
    meta: Option<Osc99Command>,
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
    /// The terminating chunk's metadata (id, actions, urgency, occasion, sound,
    /// app name, icon names/cache key, close/report flags, expiry, `payload_type`).
    pub meta: Osc99Command,
}

// ── Helper: build a FinalizedNotification from accumulated bytes + meta ───────

/// Build a [`FinalizedNotification`] from the accumulated title/body/icon bytes
/// and the final `meta` command. The terminating chunk's payload must already
/// have been appended to the appropriate accumulator before this is called.
fn build_finalized(
    title_bytes: &[u8],
    body_bytes: &[u8],
    icon_bytes: Vec<u8>,
    meta: Osc99Command,
) -> FinalizedNotification {
    let title = if title_bytes.is_empty() {
        None
    } else {
        Some(String::from_utf8_lossy(title_bytes).into_owned())
    };
    let body = if body_bytes.is_empty() {
        None
    } else {
        Some(String::from_utf8_lossy(body_bytes).into_owned())
    };
    let icon = if icon_bytes.is_empty() {
        None
    } else {
        Some(icon_bytes)
    };
    FinalizedNotification {
        title,
        body,
        icon,
        meta,
    }
}

// ── PendingNotifications type alias ──────────────────────────────────────────

/// Map of in-flight OSC 99 notifications keyed by their `i=` identifier.
pub(in crate::terminal_handler) type PendingNotifications = HashMap<String, PendingNotification>;

// ── Reassembly impl on TerminalHandler ───────────────────────────────────────

impl TerminalHandler {
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
    pub(in crate::terminal_handler) fn reassemble_osc99(
        &mut self,
        chunk: Osc99Command,
    ) -> Option<FinalizedNotification> {
        match &chunk.id {
            // ── No id: standalone or drop ────────────────────────────────────
            None => {
                if chunk.done {
                    // Standalone, single-chunk notification — finalize immediately.
                    let (title_bytes, body_bytes, icon_bytes) =
                        payload_into_accumulators(chunk.payload_type, chunk.payload.clone());
                    Some(build_finalized(
                        &title_bytes,
                        &body_bytes,
                        icon_bytes,
                        chunk,
                    ))
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
                let entry = self.pending_notifications.entry(id).or_default();

                // Append the chunk's payload bytes to the matching accumulator.
                append_payload(entry, chunk.payload_type, &chunk.payload);

                // Latest metadata wins (payload bytes are accumulated above, not here).
                let done = chunk.done;
                entry.meta = Some(chunk);

                if done {
                    // Remove from the map and build the finalized notification.
                    let id_key = entry.meta.as_ref().and_then(|m| m.id.clone());
                    let removed = id_key.and_then(|k| self.pending_notifications.remove(&k));
                    let entry = removed?;
                    // SAFETY: we just set entry.meta = Some(chunk) before inserting,
                    // so this is always Some when we removed a live entry.
                    let meta = entry.meta?;

                    Some(build_finalized(&entry.title, &entry.body, entry.icon, meta))
                } else {
                    None
                }
            }
        }
    }
}

/// Append `payload` bytes to the accumulator field selected by `payload_type`.
///
/// For `Close`, `Alive`, `Buttons`, and `Query` payload types, the payload
/// bytes are NOT accumulated into title/body/icon (they are not chunked content
/// payloads); only the metadata is updated (done by the caller).
fn append_payload(entry: &mut PendingNotification, payload_type: Osc99PayloadType, payload: &[u8]) {
    match payload_type {
        Osc99PayloadType::Title => entry.title.extend_from_slice(payload),
        Osc99PayloadType::Body => entry.body.extend_from_slice(payload),
        Osc99PayloadType::Icon => entry.icon.extend_from_slice(payload),
        // Non-accumulating types: Close, Alive, Buttons, Query.
        Osc99PayloadType::Close
        | Osc99PayloadType::Alive
        | Osc99PayloadType::Buttons
        | Osc99PayloadType::Query => {}
    }
}

/// Convert a standalone chunk's payload into the three accumulator vecs
/// `(title, body, icon)` based on `payload_type`.
///
/// For non-content payload types, the payload goes to no accumulator
/// (all three vecs stay empty; the meta carries the data).
fn payload_into_accumulators(
    payload_type: Osc99PayloadType,
    payload: Vec<u8>,
) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    match payload_type {
        Osc99PayloadType::Title => (payload, Vec::new(), Vec::new()),
        Osc99PayloadType::Body => (Vec::new(), payload, Vec::new()),
        Osc99PayloadType::Icon => (Vec::new(), Vec::new(), payload),
        Osc99PayloadType::Close
        | Osc99PayloadType::Alive
        | Osc99PayloadType::Buttons
        | Osc99PayloadType::Query => (Vec::new(), Vec::new(), Vec::new()),
    }
}
