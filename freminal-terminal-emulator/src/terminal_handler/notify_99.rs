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
use freminal_common::buffer_states::window_manipulation::{Notification99Data, Osc99ControlKind};

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
    /// Accumulated `p=buttons` payload bytes (U+2028-separated labels).
    buttons: Vec<u8>,
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

/// Map an OSC 99 payload type to its control kind, if it is a control
/// payload (`p=close`/`p=alive`/`p=?`).
///
/// Display payload types (`Title`/`Body`/`Icon`/`Buttons`) return `None` —
/// they keep flowing to `WindowManipulation::Notification99` in
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
        Osc99PayloadType::Query => Some(Osc99ControlKind::Query),
        Osc99PayloadType::Title
        | Osc99PayloadType::Body
        | Osc99PayloadType::Icon
        | Osc99PayloadType::Buttons => None,
    }
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

/// Build a [`FinalizedNotification`] from the accumulated title/body/icon/
/// buttons bytes and the final `meta` command. The terminating chunk's
/// payload must already have been appended to the appropriate accumulator
/// before this is called.
fn build_finalized(
    title_bytes: &[u8],
    body_bytes: &[u8],
    icon_bytes: Vec<u8>,
    button_bytes: &[u8],
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
        buttons: split_button_labels(button_bytes),
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
                    let (title_bytes, body_bytes, icon_bytes, button_bytes) =
                        payload_into_accumulators(chunk.payload_type, chunk.payload.clone());
                    Some(build_finalized(
                        &title_bytes,
                        &body_bytes,
                        icon_bytes,
                        &button_bytes,
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

                    Some(build_finalized(
                        &entry.title,
                        &entry.body,
                        entry.icon,
                        &entry.buttons,
                        meta,
                    ))
                } else {
                    None
                }
            }
        }
    }
}

/// Append `payload` bytes to the accumulator field selected by `payload_type`.
///
/// For `Close`, `Alive`, and `Query` payload types, the payload bytes are NOT
/// accumulated into title/body/icon/buttons (they are not chunked content
/// payloads); only the metadata is updated (done by the caller).
fn append_payload(entry: &mut PendingNotification, payload_type: Osc99PayloadType, payload: &[u8]) {
    match payload_type {
        Osc99PayloadType::Title => entry.title.extend_from_slice(payload),
        Osc99PayloadType::Body => entry.body.extend_from_slice(payload),
        Osc99PayloadType::Icon => entry.icon.extend_from_slice(payload),
        Osc99PayloadType::Buttons => entry.buttons.extend_from_slice(payload),
        // Non-accumulating types: Close, Alive, Query.
        Osc99PayloadType::Close | Osc99PayloadType::Alive | Osc99PayloadType::Query => {}
    }
}

/// Convert a standalone chunk's payload into the four accumulator vecs
/// `(title, body, icon, buttons)` based on `payload_type`.
///
/// For non-content payload types, the payload goes to no accumulator
/// (all four vecs stay empty; the meta carries the data).
fn payload_into_accumulators(
    payload_type: Osc99PayloadType,
    payload: Vec<u8>,
) -> (Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>) {
    match payload_type {
        Osc99PayloadType::Title => (payload, Vec::new(), Vec::new(), Vec::new()),
        Osc99PayloadType::Body => (Vec::new(), payload, Vec::new(), Vec::new()),
        Osc99PayloadType::Icon => (Vec::new(), Vec::new(), payload, Vec::new()),
        Osc99PayloadType::Buttons => (Vec::new(), Vec::new(), Vec::new(), payload),
        Osc99PayloadType::Close | Osc99PayloadType::Alive | Osc99PayloadType::Query => {
            (Vec::new(), Vec::new(), Vec::new(), Vec::new())
        }
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
        assert!(data.button_labels.is_empty());
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
        assert!(split_button_labels(b"").is_empty());
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
    fn control_kind_maps_close_alive_query() {
        assert_eq!(
            control_kind(Osc99PayloadType::Close),
            Some(Osc99ControlKind::Close)
        );
        assert_eq!(
            control_kind(Osc99PayloadType::Alive),
            Some(Osc99ControlKind::Alive)
        );
        assert_eq!(
            control_kind(Osc99PayloadType::Query),
            Some(Osc99ControlKind::Query)
        );
    }

    #[test]
    fn control_kind_maps_display_types_to_none() {
        assert_eq!(control_kind(Osc99PayloadType::Title), None);
        assert_eq!(control_kind(Osc99PayloadType::Body), None);
        assert_eq!(control_kind(Osc99PayloadType::Icon), None);
        assert_eq!(control_kind(Osc99PayloadType::Buttons), None);
    }
}
