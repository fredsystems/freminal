// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Helpers only the crate's own tests use: notification fixtures, signal and
//! error builders, and the rig variants and methods that the callers' tests
//! (which see only [`test_rig`](super::test_rig)) do not need.

use std::error::Error;
use std::sync::PoisonError;
use std::time::Duration;

use freminal_common::buffer_states::osc99_notification::{
    Notification99Data, NotificationOccasion, NotificationUrgency, Osc99ActivationFocus,
    Osc99ActivationReporting, Osc99CloseReporting, Osc99Expiry, Osc99Sound,
};
use zbus::message::Message;

use super::super::NotificationInstanceId;
use super::pump::PumpSignal;
use super::test_rig::{
    Activation, CloseAnswer, EVENT_TIMEOUT, NameOwnerAnswer, Rig, rig, rig_with, strings,
};
use super::wire::{NotifyReply, fetch_capabilities};
use super::{
    BUS_DRIVER_INTERFACE, BUS_DRIVER_NAME, BUS_DRIVER_PATH, NOTIFICATIONS_INTERFACE,
    NOTIFICATIONS_PATH, NOTIFICATIONS_SERVICE,
};

/// How long a client built by [`rig_with_method_timeout`] waits for a reply in
/// the timeout tests. Only calls that are never answered ever wait it out, and
/// they must: a shorter value cannot make such a test flaky, it only shortens
/// it. It is not used for any call the daemon does answer.
pub(super) const SHORT_METHOD_TIMEOUT: Duration = Duration::from_millis(200);

pub(super) const fn instance(n: u64) -> NotificationInstanceId {
    NotificationInstanceId::new(n)
}

pub(super) fn notification() -> Notification99Data {
    Notification99Data {
        id: None,
        title: "Title".to_owned(),
        body: None,
        icon_data: None,
        icon_names: Vec::new(),
        icon_cache_key: None,
        buttons: Vec::new(),
        activation_reporting: Osc99ActivationReporting::Silent,
        activation_focus: Osc99ActivationFocus::NoFocus,
        close_reporting: Osc99CloseReporting::Silent,
        urgency: NotificationUrgency::Normal,
        occasion: NotificationOccasion::Always,
        sound: Osc99Sound::System,
        app_name: None,
        notification_types: Vec::new(),
        expiry: Osc99Expiry::OsDefault,
    }
}

pub(super) fn with_buttons(labels: &[&str]) -> Notification99Data {
    Notification99Data {
        buttons: labels.iter().map(|label| (*label).to_owned()).collect(),
        ..notification()
    }
}

/// A `Notify` reply with no sender, as over a peer-to-peer connection.
pub(super) const fn peer_reply(dbus_id: u32) -> NotifyReply {
    NotifyReply {
        dbus_id,
        sender: None,
    }
}

/// A `Notify` reply sent by `sender`, as a bus stamps it.
pub(super) fn reply_from(dbus_id: u32, sender: &str) -> NotifyReply {
    NotifyReply {
        dbus_id,
        sender: Some(sender.to_owned()),
    }
}

/// A `width` x `height` PNG of one opaque colour.
pub(super) fn png(width: u32, height: u32) -> Result<Vec<u8>, image::ImageError> {
    let pixels = image::RgbaImage::from_pixel(width, height, image::Rgba([1, 2, 3, 255]));
    let mut bytes = Vec::new();
    pixels.write_to(
        &mut std::io::Cursor::new(&mut bytes),
        image::ImageFormat::Png,
    )?;
    Ok(bytes)
}

pub(super) fn action_from(sender: Option<&str>, dbus_id: u32, key: &str) -> PumpSignal {
    PumpSignal::ActionInvoked {
        sender: sender.map(str::to_owned),
        dbus_id,
        action_key: key.to_owned(),
    }
}

pub(super) fn closed_from(sender: Option<&str>, dbus_id: u32) -> PumpSignal {
    PumpSignal::NotificationClosed {
        sender: sender.map(str::to_owned),
        dbus_id,
    }
}

pub(super) fn owner_changed(new_owner: Option<&str>) -> PumpSignal {
    PumpSignal::OwnerChanged {
        new_owner: new_owner.map(str::to_owned),
    }
}

pub(super) fn timeout_error() -> zbus::Error {
    zbus::Error::from(std::io::Error::new(
        std::io::ErrorKind::TimedOut,
        "timed out",
    ))
}

/// An error reply named `name`, as a daemon or the bus sends it.
pub(super) fn error_reply(name: &str) -> Result<zbus::Error, Box<dyn Error>> {
    let call = Message::method_call("/", "Ping").build(&())?;
    let reply = Message::error(&call.header(), name.to_owned()).build(&"details")?;
    Ok(zbus::Error::from(reply))
}

/// A signal message from the notification daemon, awaiting its body.
pub(super) fn signal(member: &str) -> zbus::message::Builder<'_> {
    Message::signal(NOTIFICATIONS_PATH, NOTIFICATIONS_INTERFACE, member)
}

/// A `NameOwnerChanged` signal as the bus driver would send it.
pub(super) fn name_owner_changed(
    sender: &str,
    name: &str,
    new_owner: &str,
) -> zbus::Result<Message> {
    Message::signal(BUS_DRIVER_PATH, BUS_DRIVER_INTERFACE, "NameOwnerChanged")
        .sender(sender)
        .build(&(name, "", new_owner))
}

/// A rig whose client gives up on a method call after `timeout`, as the
/// production connection does after `DBUS_METHOD_TIMEOUT`.
pub(super) fn rig_with_method_timeout(
    advertised: &[&str],
    timeout: Duration,
) -> Result<Rig, Box<dyn Error>> {
    rig_with(advertised, Some(timeout))
}

/// A rig for a service that tracks owners as on a bus: `GetNameOwner`
/// answers `owner` and every reply is stamped as sent by `replies_from`.
pub(super) fn bus_rig(
    advertised: &[&str],
    owner: NameOwnerAnswer,
    replies_from: Option<&str>,
) -> Result<Rig, Box<dyn Error>> {
    let rig = rig(advertised)?;
    *rig.log
        .name_owner
        .lock()
        .unwrap_or_else(PoisonError::into_inner) = owner;
    *rig.log
        .reply_sender
        .lock()
        .unwrap_or_else(PoisonError::into_inner) = replies_from.map(str::to_owned);
    Ok(rig)
}

impl Rig {
    /// Wait until the daemon has handled every call sent before this one. It
    /// serves calls in order, so once it answers a `GetCapabilities` (which
    /// the client's `method_timeout` does not bound) the log holds everything
    /// sent earlier, including calls it never answered.
    pub(super) fn settle(&self) -> zbus::Result<()> {
        fetch_capabilities(&self.client).map(|_| ())
    }

    /// Set how the daemon answers `CloseNotification` from now on.
    pub(super) fn answer_close(&self, answer: CloseAnswer) {
        *self
            .log
            .close_answer
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = answer;
    }

    /// The id of every `CloseNotification` call the daemon served.
    pub(super) fn closes(&self) -> Vec<u32> {
        self.log
            .closes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Every method call the daemon served, with its flag.
    pub(super) fn calls(&self) -> Vec<(String, Activation)> {
        self.log
            .calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// What the daemon will answer `GetCapabilities` with from now on.
    pub(super) fn advertise(&self, capabilities: &[&str]) {
        *self
            .log
            .capabilities
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = strings(capabilities);
    }

    /// Stamp every reply from now on as sent by `sender`.
    pub(super) fn reply_as(&self, sender: &str) {
        *self
            .log
            .reply_sender
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(sender.to_owned());
    }

    pub(super) fn emit_action(&self, dbus_id: u32, key: &str) -> zbus::Result<()> {
        zbus::block_on(self.daemon.emit_signal(
            None::<&str>,
            NOTIFICATIONS_PATH,
            NOTIFICATIONS_INTERFACE,
            "ActionInvoked",
            &(dbus_id, key),
        ))
    }

    pub(super) fn emit_closed(&self, dbus_id: u32, reason: u32) -> zbus::Result<()> {
        zbus::block_on(self.daemon.emit_signal(
            None::<&str>,
            NOTIFICATIONS_PATH,
            NOTIFICATIONS_INTERFACE,
            "NotificationClosed",
            &(dbus_id, reason),
        ))
    }

    /// A `NotificationClosed` signal stamped as sent by `sender`: what a
    /// bus delivers, which `emit_signal` over a peer-to-peer connection
    /// cannot produce.
    pub(super) fn emit_closed_from(&self, sender: &str, dbus_id: u32) -> zbus::Result<()> {
        let message = signal("NotificationClosed")
            .sender(sender.to_owned())
            .build(&(dbus_id, 1u32))?;
        zbus::block_on(self.daemon.send(&message))
    }

    /// An `ActionInvoked` signal stamped as sent by `sender`.
    pub(super) fn emit_action_from(
        &self,
        sender: &str,
        dbus_id: u32,
        key: &str,
    ) -> zbus::Result<()> {
        let message = signal("ActionInvoked")
            .sender(sender.to_owned())
            .build(&(dbus_id, key))?;
        zbus::block_on(self.daemon.send(&message))
    }

    /// The bus driver's `NameOwnerChanged` for the daemon's name.
    pub(super) fn emit_owner_changed(&self, new_owner: &str) -> zbus::Result<()> {
        let message = name_owner_changed(BUS_DRIVER_NAME, NOTIFICATIONS_SERVICE, new_owner)?;
        zbus::block_on(self.daemon.send(&message))
    }
}

/// Run `work` on its own thread and return its result, failing the test if it
/// has not finished in [`EVENT_TIMEOUT`], so a call that is meant to time out
/// but does not fails the test instead of hanging it.
pub(super) fn within_deadline<T, F>(work: F) -> Result<T, Box<dyn Error>>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let (tx, rx) = crossbeam_channel::bounded(1);
    std::thread::Builder::new()
        .name("test-deadline".to_owned())
        .spawn(move || {
            let _ = tx.send(work());
        })?;
    Ok(rx.recv_timeout(EVENT_TIMEOUT)?)
}
