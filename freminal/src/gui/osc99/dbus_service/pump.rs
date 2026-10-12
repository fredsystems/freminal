// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! The signal pumps: the threads that read signals off the bus connection
//! and forward them to the dispatcher, and the match rules and parsers they
//! use.

use crossbeam_channel::Sender;
use zbus::MatchRule;
use zbus::blocking::MessageIterator;
use zbus::message::{Message, Type as MessageType};

use super::{
    BUS_DRIVER_INTERFACE, BUS_DRIVER_NAME, NOTIFICATIONS_INTERFACE, NOTIFICATIONS_PATH,
    NOTIFICATIONS_SERVICE, StartError,
};

/// Which of the two signal pumps a [`PumpSignal::Ended`] came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PumpKind {
    /// `freminal-osc99-signals`: `ActionInvoked` / `NotificationClosed`.
    Notifications,
    /// `freminal-osc99-owner`: `NameOwnerChanged`.
    OwnerChanges,
}

/// A signal a pump forwards to the dispatcher.
///
/// The notification signals carry the unique name of their sender (`None` when
/// the message has no sender header, as over a peer-to-peer connection) so the
/// dispatcher can drop forgeries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PumpSignal {
    /// `ActionInvoked(id, action_key)`.
    ActionInvoked {
        sender: Option<String>,
        dbus_id: u32,
        action_key: String,
    },
    /// `NotificationClosed(id, reason)`. The reason does not matter to the
    /// protocol, so it is not carried.
    NotificationClosed {
        sender: Option<String>,
        dbus_id: u32,
    },
    /// `NameOwnerChanged` for `org.freedesktop.Notifications`: the unique name
    /// that owns it now, or `None` when nobody does.
    OwnerChanged { new_owner: Option<String> },
    /// The pump of this kind has ended (the connection closed or the stream
    /// failed): its signals will not arrive any more. Sent by the pump itself
    /// just before its thread exits.
    Ended(PumpKind),
}

/// The match rule for the daemon's `ActionInvoked` / `NotificationClosed`
/// signals. It names no sender: see [`Osc99SignalSender`](super::state::Osc99SignalSender).
pub(super) fn notification_signals_rule() -> zbus::Result<MatchRule<'static>> {
    Ok(MatchRule::builder()
        .msg_type(MessageType::Signal)
        .interface(NOTIFICATIONS_INTERFACE)?
        .path(NOTIFICATIONS_PATH)?
        .build())
}

/// The match rule for the bus driver's `NameOwnerChanged` signal about the
/// notification daemon's name.
pub(super) fn owner_changed_rule() -> zbus::Result<MatchRule<'static>> {
    Ok(MatchRule::builder()
        .msg_type(MessageType::Signal)
        .sender(BUS_DRIVER_NAME)?
        .interface(BUS_DRIVER_INTERFACE)?
        .member("NameOwnerChanged")?
        .arg(0, NOTIFICATIONS_SERVICE)?
        .build())
}

/// Start a pump thread: forward the signals `parse` recognises from `messages`
/// to the dispatcher.
pub(super) fn spawn_pump(
    name: &str,
    kind: PumpKind,
    messages: MessageIterator,
    signals: Sender<PumpSignal>,
    parse: fn(&Message) -> Option<PumpSignal>,
) -> Result<(), StartError> {
    std::thread::Builder::new()
        .name(name.to_owned())
        .spawn(move || run_pump(kind, messages, &signals, parse))
        .map(|_| ())
        .map_err(StartError::PumpThread)
}

/// A pump thread: forward until the stream ends or the dispatcher is gone,
/// then tell the dispatcher (if it is still there) that this pump is over.
fn run_pump(
    kind: PumpKind,
    messages: MessageIterator,
    signals: &Sender<PumpSignal>,
    parse: fn(&Message) -> Option<PumpSignal>,
) {
    forward_signals(messages, signals, parse);
    if signals.send(PumpSignal::Ended(kind)).is_err() {
        tracing::debug!("OSC 99: the dispatcher is gone; the {kind:?} pump exits quietly");
    }
}

/// Forward the signals `parse` recognises until the bus closes or the
/// dispatcher is gone.
fn forward_signals(
    messages: MessageIterator,
    signals: &Sender<PumpSignal>,
    parse: fn(&Message) -> Option<PumpSignal>,
) {
    for message in messages {
        let message = match message {
            Ok(message) => message,
            Err(e) => {
                tracing::warn!("OSC 99: notification signal stream failed: {e}");
                return;
            }
        };
        if let Some(signal) = parse(&message)
            && signals.send(signal).is_err()
        {
            // The dispatcher dropped its receiver: the GUI is gone.
            return;
        }
    }
}

/// The signal `message` carries, or `None` for any member other than
/// `ActionInvoked` and `NotificationClosed` and for a malformed body.
pub(super) fn parse_signal(message: &Message) -> Option<PumpSignal> {
    let header = message.header();
    let member = header.member()?;
    let sender = header.sender().map(ToString::to_string);
    let body = message.body();
    match member.as_str() {
        "ActionInvoked" => {
            let (dbus_id, action_key) = body.deserialize::<(u32, String)>().ok()?;
            Some(PumpSignal::ActionInvoked {
                sender,
                dbus_id,
                action_key,
            })
        }
        "NotificationClosed" => {
            let (dbus_id, _reason) = body.deserialize::<(u32, u32)>().ok()?;
            Some(PumpSignal::NotificationClosed { sender, dbus_id })
        }
        _ => None,
    }
}

/// The [`PumpSignal::OwnerChanged`] `message` carries, or `None` for anything
/// else: another member, another name, a malformed body, or a sender that is
/// not the bus driver. The match rule names the bus driver as sender, but
/// zbus's client-side match cannot check a well-known sender, so it is checked
/// here: otherwise a peer could emit a forged `NameOwnerChanged` and name
/// itself the daemon.
pub(super) fn parse_owner_changed(message: &Message) -> Option<PumpSignal> {
    let header = message.header();
    if header.member()?.as_str() != "NameOwnerChanged"
        || header.sender()?.as_str() != BUS_DRIVER_NAME
    {
        return None;
    }
    let (name, _old_owner, new_owner) = message
        .body()
        .deserialize::<(String, String, String)>()
        .ok()?;
    if name != NOTIFICATIONS_SERVICE {
        return None;
    }
    Some(PumpSignal::OwnerChanged {
        new_owner: (!new_owner.is_empty()).then_some(new_owner),
    })
}

#[cfg(test)]
mod tests {
    use super::super::BUS_DRIVER_PATH;
    use super::super::test_rig::*;
    use super::*;

    // ── Signal parsing ──────────────────────────────────────────────────

    #[test]
    fn parse_signal_reads_action_invoked() -> TestResult {
        let message = signal("ActionInvoked")?.build(&(7u32, "2"))?;

        assert_eq!(parse_signal(&message), Some(action_from(None, 7, "2")));
        Ok(())
    }

    #[test]
    fn parse_signal_carries_the_sender() -> TestResult {
        let message = signal("NotificationClosed")?
            .sender(":1.5")?
            .build(&(7u32, 3u32))?;

        assert_eq!(parse_signal(&message), Some(closed_from(Some(":1.5"), 7)));
        Ok(())
    }

    #[test]
    fn parse_signal_reads_notification_closed_whatever_the_reason() -> TestResult {
        let message = signal("NotificationClosed")?.build(&(7u32, 3u32))?;

        assert_eq!(parse_signal(&message), Some(closed_from(None, 7)));
        Ok(())
    }

    #[test]
    fn parse_signal_ignores_other_members() -> TestResult {
        let message = signal("ActivationToken")?.build(&(7u32, "token"))?;

        assert_eq!(parse_signal(&message), None);
        Ok(())
    }

    #[test]
    fn parse_signal_ignores_a_malformed_body() -> TestResult {
        let message = signal("ActionInvoked")?.build(&"not a tuple")?;

        assert_eq!(parse_signal(&message), None);
        Ok(())
    }

    #[test]
    fn parse_owner_changed_reads_the_new_owner() -> TestResult {
        let message = name_owner_changed(BUS_DRIVER_NAME, NOTIFICATIONS_SERVICE, ":1.6")?;

        assert_eq!(
            parse_owner_changed(&message),
            Some(owner_changed(Some(":1.6")))
        );
        Ok(())
    }

    #[test]
    fn parse_owner_changed_reads_an_empty_new_owner_as_no_owner() -> TestResult {
        let message = name_owner_changed(BUS_DRIVER_NAME, NOTIFICATIONS_SERVICE, "")?;

        assert_eq!(parse_owner_changed(&message), Some(owner_changed(None)));
        Ok(())
    }

    #[test]
    fn parse_owner_changed_ignores_other_names() -> TestResult {
        let message = name_owner_changed(BUS_DRIVER_NAME, "org.example.Other", ":1.6")?;

        assert_eq!(parse_owner_changed(&message), None);
        Ok(())
    }

    #[test]
    fn parse_owner_changed_ignores_a_sender_that_is_not_the_bus_driver() -> TestResult {
        let forged = name_owner_changed(":1.9", NOTIFICATIONS_SERVICE, ":1.9")?;
        let senderless =
            Message::signal(BUS_DRIVER_PATH, BUS_DRIVER_INTERFACE, "NameOwnerChanged")?
                .build(&(NOTIFICATIONS_SERVICE, "", ":1.9"))?;

        assert_eq!(parse_owner_changed(&forged), None);
        assert_eq!(parse_owner_changed(&senderless), None);
        Ok(())
    }

    #[test]
    fn parse_owner_changed_ignores_other_members_and_malformed_bodies() -> TestResult {
        let other_member = Message::signal(BUS_DRIVER_PATH, BUS_DRIVER_INTERFACE, "NameLost")?
            .sender(BUS_DRIVER_NAME)?
            .build(&(NOTIFICATIONS_SERVICE, "", ":1.6"))?;
        let malformed = Message::signal(BUS_DRIVER_PATH, BUS_DRIVER_INTERFACE, "NameOwnerChanged")?
            .sender(BUS_DRIVER_NAME)?
            .build(&"not a tuple")?;

        assert_eq!(parse_owner_changed(&other_member), None);
        assert_eq!(parse_owner_changed(&malformed), None);
        Ok(())
    }

    #[test]
    fn the_match_rules_build() -> TestResult {
        let rule = owner_changed_rule()?.to_string();

        assert_eq!(
            rule,
            "type='signal',sender='org.freedesktop.DBus',interface='org.freedesktop.DBus',\
             member='NameOwnerChanged',arg0='org.freedesktop.Notifications'"
        );
        assert!(
            notification_signals_rule()?
                .to_string()
                .contains("path='/org/freedesktop/Notifications'")
        );
        Ok(())
    }
}
