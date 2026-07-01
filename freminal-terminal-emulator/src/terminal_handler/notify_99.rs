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
    NotificationOccasion, NotificationUrgency, Osc99Command, Osc99PayloadType,
};
use freminal_common::buffer_states::window_manipulation::Notification99Data;

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
    /// other value to `Some(v)`. `button_labels` is `Vec::new()` for now —
    /// button-label extraction is tracked as cleanup 99.9.
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
            // Button-label extraction is not implemented yet (plan cleanup 99.9).
            button_labels: Vec::new(),
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use freminal_common::buffer_states::osc_notify_99::Osc99Actions;

    /// Build a default `Osc99Command` for use in mapping tests.
    fn default_cmd() -> Osc99Command {
        Osc99Command {
            id: None,
            payload_type: Osc99PayloadType::Title,
            done: true,
            payload: Vec::new(),
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
        assert!(data.button_labels.is_empty());
    }
}
