// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Test support for the OSC 99 D-Bus service: shared fixtures, a hand-written
//! fake notification daemon, and the rig that connects a client to it over a
//! socket pair.

use std::collections::HashMap;
use std::error::Error;
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use crossbeam_channel::Receiver;
use freminal_common::buffer_states::osc99_notification::{
    Notification99Data, NotificationOccasion, NotificationUrgency, Osc99ActivationFocus,
    Osc99ActivationReporting, Osc99CloseReporting, Osc99Expiry, Osc99Sound,
};
use futures_lite::StreamExt;
use zbus::message::{Message, Type as MessageType};
use zbus::{Connection, MessageStream, OwnedVariant, Structure, Variant};

use super::super::presentation::Osc99IconLookup;
use super::super::test_support::test_assets;
use super::super::{Osc99InstanceId, Osc99SystemBackend, Osc99SystemDispatch};
use super::pump::PumpSignal;
use super::state::OwnerTracking;
use super::wire::{NotifyReply, fetch_capabilities};
use super::{
    BUS_DRIVER_INTERFACE, BUS_DRIVER_NAME, BUS_DRIVER_PATH, NOTIFICATIONS_INTERFACE,
    NOTIFICATIONS_PATH, NOTIFICATIONS_SERVICE, Osc99DbusEvent, Osc99DbusHandle, Osc99DbusService,
};

/// How long a test waits for something that must happen. Generous so a
/// loaded CI machine never trips it; a healthy run never waits it out.
pub(super) const EVENT_TIMEOUT: Duration = Duration::from_secs(20);

pub type TestResult = Result<(), Box<dyn Error>>;

/// The error the bus driver answers `GetNameOwner` with when nobody owns the
/// name. Production code matches `fdo::Error::NameHasNoOwner`; the name is for
/// the fake daemon and the error fixtures.
pub(super) const ERROR_NAME_HAS_NO_OWNER: &str = "org.freedesktop.DBus.Error.NameHasNoOwner";
/// The error the bus answers a `NO_AUTO_START` call to a name that nobody owns
/// and that no service file can start. Production code matches
/// `fdo::Error::ServiceUnknown`; the name is for the fake daemon and the error
/// fixtures.
pub(super) const ERROR_SERVICE_UNKNOWN: &str = "org.freedesktop.DBus.Error.ServiceUnknown";

/// How long a client built by [`rig_with_method_timeout`] waits for a reply in
/// the timeout tests. Only calls that are never answered ever wait it out, and
/// they must: a shorter value cannot make such a test flaky, it only shortens
/// it. It is not used for any call the daemon does answer.
pub(super) const SHORT_METHOD_TIMEOUT: Duration = Duration::from_millis(200);

pub(super) fn instance(n: u64) -> Osc99InstanceId {
    Osc99InstanceId::new(n)
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

pub(super) fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| (*item).to_owned()).collect()
}

/// A `Notify` reply with no sender, as over a peer-to-peer connection.
pub(super) fn peer_reply(dbus_id: u32) -> NotifyReply {
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

/// What the fake daemon saw in one `Notify` call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedNotify {
    pub app_name: String,
    pub replaces_id: u32,
    pub app_icon: String,
    pub summary: String,
    pub body: String,
    pub actions: Vec<String>,
    pub urgency: Option<u8>,
    /// The key of every hint, sorted.
    pub hint_keys: Vec<String>,
    pub category: Option<String>,
    pub desktop_entry: Option<String>,
    pub sound_name: Option<String>,
    /// `(width, height, rowstride, has_alpha, bits_per_sample, channels,
    /// data length)` of the `image-data` hint.
    pub image: Option<RecordedImage>,
    pub expire_timeout: i32,
    pub assigned_id: u32,
}

/// The `image-data` hint's `(iiibiiay)` structure, summarised.
pub(super) type RecordedImage = (i32, i32, i32, bool, i32, i32, usize);

pub(super) fn recorded_image(value: &OwnedVariant) -> Option<RecordedImage> {
    let structure = Structure::try_from(Variant::from(value.try_clone().ok()?)).ok()?;
    let (w, h, stride, alpha, bits, channels, data) =
        <(i32, i32, i32, bool, i32, i32, Vec<u8>)>::try_from(structure).ok()?;
    Some((w, h, stride, alpha, bits, channels, data.len()))
}

/// The `Notify` arguments, as the fake daemon deserialises them.
pub(super) type NotifyBody = (
    String,
    u32,
    String,
    String,
    String,
    Vec<String>,
    HashMap<String, OwnedVariant>,
    i32,
);

/// The summary the fake daemon refuses to display (`Notify` errors).
pub(super) const REJECTED_SUMMARY: &str = "rejected";

/// The summary whose `Notify` the fake daemon records and never answers, as a
/// hung daemon does.
pub(super) const UNANSWERED_SUMMARY: &str = "unanswered";

/// How the fake bus driver answers `GetNameOwner`.
#[derive(Debug, Clone, Default)]
pub(super) enum NameOwnerAnswer {
    /// This unique name owns the name.
    Owner(String),
    /// Nobody does (`NameHasNoOwner`).
    #[default]
    Nobody,
    /// The bus knows no service file that could start the name
    /// (`ServiceUnknown`).
    ServiceUnknown,
    /// The lookup fails with an unrelated error.
    Broken,
}

/// How the fake daemon answers `CloseNotification`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum CloseAnswer {
    /// Reply, then emit `NotificationClosed(id, 3)`, as the specification
    /// says a daemon does.
    #[default]
    CloseAndSignal,
    /// Reply but emit no signal, as wayle 0.1.3 does for a notification sent
    /// with `expire_timeout` 0.
    CloseSilently,
    /// Answer with an error instead of closing.
    Fail,
    /// Record the call and never reply, as a hung daemon does.
    Never,
}

/// Whether a call carried the `NO_AUTO_START` flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Activation {
    Allowed,
    Suppressed,
}

/// What the fake daemon saw and how it should behave.
#[derive(Default)]
pub(super) struct DaemonLog {
    /// Every `Notify` it displayed, in call order.
    pub(super) notifies: Mutex<Vec<RecordedNotify>>,
    /// The id of every `CloseNotification` call, in call order.
    pub(super) closes: Mutex<Vec<u32>>,
    /// Every method call, in call order, with its `NO_AUTO_START` flag.
    pub(super) calls: Mutex<Vec<(String, Activation)>>,
    /// How `CloseNotification` is answered.
    pub(super) close_answer: Mutex<CloseAnswer>,
    /// What `GetCapabilities` answers.
    pub(super) capabilities: Mutex<Vec<String>>,
    /// The sender stamped on replies, as a bus stamps them. Over a
    /// peer-to-peer connection nothing does, hence `None`.
    pub(super) reply_sender: Mutex<Option<String>>,
    /// What `GetNameOwner` answers.
    pub(super) name_owner: Mutex<NameOwnerAnswer>,
}

/// Reply to the call `header` belongs to, with the body `build` makes,
/// stamped with `sender` (as a bus would) when there is one.
pub(super) async fn reply_with<F>(
    daemon: &Connection,
    header: &zbus::message::Header<'_>,
    sender: Option<&str>,
    build: F,
) -> zbus::Result<()>
where
    F: FnOnce(zbus::message::Builder<'static>) -> zbus::Result<Message>,
{
    let mut builder = Message::method_return(header);
    if let Some(sender) = sender {
        builder = builder.sender(sender.to_owned());
    }
    daemon.send(&build(builder)?).await
}

/// What the fake daemon decided about a `Notify` call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NotifyVerdict {
    /// It was recorded and got this id.
    Accepted(u32),
    /// Its summary is [`REJECTED_SUMMARY`].
    Rejected,
    /// It was recorded, and its summary is [`UNANSWERED_SUMMARY`]: it gets no
    /// reply.
    Unanswered(u32),
}

/// Record a `Notify` call in the log, unless its summary is
/// [`REJECTED_SUMMARY`]. Synchronous, so that no lock is held across an await.
/// A call with the summary [`UNANSWERED_SUMMARY`] is recorded like any other.
fn record_notify(log: &DaemonLog, message: &Message) -> zbus::Result<NotifyVerdict> {
    let (app_name, replaces_id, app_icon, summary, body, actions, hints, timeout) =
        message.body().deserialize::<NotifyBody>()?;
    if summary == REJECTED_SUMMARY {
        return Ok(NotifyVerdict::Rejected);
    }
    let is_unanswered = summary == UNANSWERED_SUMMARY;
    let mut notifies = log.notifies.lock().unwrap_or_else(PoisonError::into_inner);
    let assigned_id = u32::try_from(notifies.len()).map_or(u32::MAX, |n| n + 1);
    let mut hint_keys: Vec<String> = hints.keys().cloned().collect();
    hint_keys.sort();
    notifies.push(RecordedNotify {
        app_name,
        replaces_id,
        app_icon,
        summary,
        body,
        actions,
        urgency: hints
            .get("urgency")
            .and_then(|value| u8::try_from(value).ok()),
        hint_keys,
        category: hints
            .get("category")
            .and_then(|value| String::try_from(value.try_clone().ok()?).ok()),
        desktop_entry: hints
            .get("desktop-entry")
            .and_then(|value| String::try_from(value.try_clone().ok()?).ok()),
        sound_name: hints
            .get("sound-name")
            .and_then(|value| String::try_from(value.try_clone().ok()?).ok()),
        image: hints.get("image-data").and_then(recorded_image),
        expire_timeout: timeout,
        assigned_id,
    });
    drop(notifies);
    if is_unanswered {
        return Ok(NotifyVerdict::Unanswered(assigned_id));
    }
    Ok(NotifyVerdict::Accepted(assigned_id))
}

/// Serve one `Notify` call: record it and answer with the next id, refuse it
/// when its summary is [`REJECTED_SUMMARY`], or stay silent when it is
/// [`UNANSWERED_SUMMARY`].
pub(super) async fn fake_notify(
    daemon: &Connection,
    log: &DaemonLog,
    message: &Message,
    stamp: Option<&str>,
) -> zbus::Result<()> {
    let header = message.header();
    match record_notify(log, message)? {
        NotifyVerdict::Rejected => {
            daemon
                .reply_error(&header, "org.freedesktop.DBus.Error.Failed", &"rejected")
                .await
        }
        NotifyVerdict::Accepted(assigned_id) => {
            reply_with(daemon, &header, stamp, |reply| reply.build(&assigned_id)).await
        }
        NotifyVerdict::Unanswered(_) => Ok(()),
    }
}

/// Answer the bus driver's `GetNameOwner` as the log's [`NameOwnerAnswer`]
/// says.
async fn fake_get_name_owner(
    daemon: &Connection,
    log: &DaemonLog,
    header: &zbus::message::Header<'_>,
) -> zbus::Result<()> {
    let answer = log
        .name_owner
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    match answer {
        NameOwnerAnswer::Owner(owner) => {
            reply_with(daemon, header, None, |reply| reply.build(&owner)).await
        }
        NameOwnerAnswer::Nobody => {
            daemon
                .reply_error(header, ERROR_NAME_HAS_NO_OWNER, &"the name has no owner")
                .await
        }
        NameOwnerAnswer::ServiceUnknown => {
            daemon
                .reply_error(header, ERROR_SERVICE_UNKNOWN, &"no such service")
                .await
        }
        NameOwnerAnswer::Broken => {
            daemon
                .reply_error(
                    header,
                    "org.freedesktop.DBus.Error.Failed",
                    &"lookup failed",
                )
                .await
        }
    }
}

/// Serve one `CloseNotification` call: record the id, then answer as the
/// log's [`CloseAnswer`] says.
async fn fake_close_notification(
    daemon: &Connection,
    log: &DaemonLog,
    message: &Message,
    stamp: Option<&str>,
) -> zbus::Result<()> {
    let header = message.header();
    let dbus_id = message.body().deserialize::<u32>()?;
    log.closes
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(dbus_id);
    let answer = *log
        .close_answer
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    match answer {
        CloseAnswer::Never => Ok(()),
        CloseAnswer::Fail => {
            daemon
                .reply_error(
                    &header,
                    "org.freedesktop.DBus.Error.Failed",
                    &"cannot close",
                )
                .await
        }
        CloseAnswer::CloseSilently => {
            reply_with(daemon, &header, stamp, |reply| reply.build(&())).await
        }
        CloseAnswer::CloseAndSignal => {
            reply_with(daemon, &header, stamp, |reply| reply.build(&())).await?;
            daemon
                .emit_signal(
                    None::<&str>,
                    NOTIFICATIONS_PATH,
                    NOTIFICATIONS_INTERFACE,
                    "NotificationClosed",
                    &(dbus_id, 3u32),
                )
                .await
        }
    }
}

/// Serve one method call `message`, named `member`, stamping replies with
/// `stamp`.
async fn answer_call(
    daemon: &Connection,
    log: &DaemonLog,
    message: &Message,
    member: &str,
    stamp: Option<&str>,
) -> zbus::Result<()> {
    let header = message.header();
    match member {
        "Notify" => fake_notify(daemon, log, message, stamp).await,
        "GetCapabilities" => {
            let capabilities = log
                .capabilities
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone();
            reply_with(daemon, &header, stamp, |reply| reply.build(&capabilities)).await
        }
        "GetNameOwner" => fake_get_name_owner(daemon, log, &header).await,
        "CloseNotification" => fake_close_notification(daemon, log, message, stamp).await,
        "GetServerInformation" => {
            daemon
                .reply(&header, &("fake", "freminal-tests", "0", "1.2"))
                .await
        }
        _ => Ok(()),
    }
}

/// Serve `org.freedesktop.Notifications` (and the bus driver's
/// `GetNameOwner`) on `daemon` until its connection closes.
///
/// - `Notify` records the call and returns a fresh, incrementing id (even
///   for a replacement), errors for [`REJECTED_SUMMARY`], and never replies
///   for [`UNANSWERED_SUMMARY`].
/// - `CloseNotification` records the id, then replies and emits
///   `NotificationClosed(id, 3)` as a real daemon does, unless the log's
///   [`CloseAnswer`] says to stay silent, to fail or to never reply.
/// - `GetCapabilities` answers the log's capabilities.
/// - `GetNameOwner` answers as the log's [`NameOwnerAnswer`] says.
///
/// Replies carry the log's `reply_sender`. `messages` must have been
/// created before the client could send its first call: a message stream
/// only sees messages that arrive after it exists.
///
/// An `async fn`: the daemon thread drives it with one [`zbus::block_on`], as a
/// signal pump does, so none of the zbus calls in it may call `block_on`
/// themselves.
///
/// Hand-written rather than a `#[zbus::interface]`: the macro's expansion
/// of the eight-argument `Notify` trips pedantic lints that cannot be
/// silenced without an `allow`.
pub(super) async fn serve_fake_daemon(
    daemon: &Connection,
    mut messages: MessageStream,
    log: &DaemonLog,
) -> zbus::Result<()> {
    while let Some(message) = messages.next().await {
        let message = message?;
        if message.message_type() != MessageType::MethodCall {
            continue;
        }
        let Some(member) = message.header().member().map(ToString::to_string) else {
            continue;
        };
        let activation = if message
            .primary_header()
            .flags()
            .contains(zbus::message::Flags::NO_AUTO_START)
        {
            Activation::Suppressed
        } else {
            Activation::Allowed
        };
        log.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((member.clone(), activation));
        let stamp = log
            .reply_sender
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        answer_call(daemon, log, &message, &member, stamp.as_deref()).await?;
    }
    Ok(())
}

/// A connected client and fake-daemon pair over a socket pair, plus the
/// daemon's record of the calls it served.
pub struct Rig {
    pub(super) client: Connection,
    pub(super) daemon: Connection,
    pub(super) log: Arc<DaemonLog>,
    /// Receives one `()` when the fake daemon's serve loop has returned, which
    /// it does once the client's end of the connection is closed.
    pub(super) daemon_stopped: Receiver<()>,
}

pub(super) fn rig(advertised: &[&str]) -> Result<Rig, Box<dyn Error>> {
    rig_with(advertised, None)
}

/// A rig whose client gives up on a method call after `timeout`, as the
/// production connection does after `OSC99_DBUS_METHOD_TIMEOUT`.
pub(super) fn rig_with_method_timeout(
    advertised: &[&str],
    timeout: Duration,
) -> Result<Rig, Box<dyn Error>> {
    rig_with(advertised, Some(timeout))
}

fn rig_with(advertised: &[&str], method_timeout: Option<Duration>) -> Result<Rig, Box<dyn Error>> {
    let (server_stream, client_stream) = UnixStream::pair()?;
    let guid = zbus::Guid::generate();

    // The handshake needs both ends running at once.
    let server = std::thread::spawn(move || {
        zbus::block_on(
            zbus::connection::Builder::unix_stream(server_stream)
                .server(guid)
                .p2p()
                .build(),
        )
    });
    let mut client_builder = zbus::connection::Builder::unix_stream(client_stream).p2p();
    if let Some(timeout) = method_timeout {
        client_builder = client_builder.method_timeout(timeout);
    }
    let client = zbus::block_on(client_builder.build())?;
    let daemon = server
        .join()
        .map_err(|_| "the fake daemon thread panicked")??;
    // Subscribe before anything can call: the client only starts talking
    // once the service is spawned, after this function returns, and a
    // stream created later (say, on the daemon thread) would miss a call
    // that arrived first, leaving the client waiting forever.
    let messages = MessageStream::from(&daemon);

    let log = Arc::new(DaemonLog {
        capabilities: Mutex::new(strings(advertised)),
        ..DaemonLog::default()
    });
    let daemon_side = daemon.clone();
    let record = Arc::clone(&log);
    let (stopped_tx, daemon_stopped) = crossbeam_channel::bounded(1);
    std::thread::Builder::new()
        .name("fake-notification-daemon".to_owned())
        .spawn(move || {
            if let Err(e) = zbus::block_on(serve_fake_daemon(&daemon_side, messages, &record)) {
                tracing::debug!("fake notification daemon stopped: {e}");
            }
            let _ = stopped_tx.send(());
        })?;
    Ok(Rig {
        client,
        daemon,
        log,
        daemon_stopped,
    })
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
    pub fn recorded(&self) -> Vec<RecordedNotify> {
        self.log
            .notifies
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

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

/// The next event, failing the test if none arrives in [`EVENT_TIMEOUT`].
pub(super) fn next_event(handle: &Osc99DbusHandle) -> Result<Osc99DbusEvent, Box<dyn Error>> {
    Ok(handle.events.recv_timeout(EVENT_TIMEOUT)?)
}

/// The instance a test closes to prove nothing else is queued.
pub(super) const SENTINEL: Osc99InstanceId = Osc99InstanceId::new(u64::MAX);

/// Prove that no event is queued behind the last one received.
///
/// A close of an instance that was never shown is answered at once, from
/// the dispatcher itself, so its event comes after every event the
/// dispatcher produced for the commands and signals it had already taken:
/// if the next event is the sentinel's, nothing was queued before it. A
/// signal still travelling through a pump is not covered, so a test that
/// ignores signals orders a later, observable signal behind them instead.
pub fn assert_no_queued_events(handle: &mut Osc99DbusHandle) -> TestResult {
    assert_eq!(handle.close(SENTINEL), Osc99SystemDispatch::Queued);
    assert_eq!(
        next_event(handle)?,
        Osc99DbusEvent::Closed { instance: SENTINEL }
    );
    Ok(())
}

/// A waker that counts how often it was called.
pub(super) fn counting_waker() -> (Box<dyn Fn() + Send>, Receiver<()>) {
    let (tx, rx) = crossbeam_channel::unbounded();
    (
        Box::new(move || {
            let _ = tx.send(());
        }),
        rx,
    )
}

/// Start the service on `rig` with `owner_tracking`, without consuming
/// any event.
pub(super) fn spawn_on(
    rig: &Rig,
    owner_tracking: OwnerTracking,
) -> (Osc99DbusHandle, Receiver<()>) {
    let (waker, woken) = counting_waker();
    let handle = Osc99DbusService::spawn_with_connection(
        rig.client.clone(),
        owner_tracking,
        waker,
        test_assets(),
    );
    (handle, woken)
}

/// Start the service against a fresh peer-to-peer rig and consume the
/// `Capabilities` event, so a test begins with an empty event queue.
pub fn started(
    advertised: &[&str],
) -> Result<(Rig, Osc99DbusHandle, Receiver<()>), Box<dyn Error>> {
    started_with(rig(advertised)?)
}

/// [`started`] for an existing rig.
pub(super) fn started_with(
    rig: Rig,
) -> Result<(Rig, Osc99DbusHandle, Receiver<()>), Box<dyn Error>> {
    let (handle, woken) = spawn_on(&rig, OwnerTracking::Peer);
    assert!(matches!(
        next_event(&handle)?,
        Osc99DbusEvent::Capabilities(_)
    ));
    Ok((rig, handle, woken))
}

/// An icon lookup that knows nothing, so a test touches no filesystem.
#[cfg(test)]
pub(super) struct NoIconLookup;

#[cfg(test)]
impl Osc99IconLookup for NoIconLookup {
    fn matched_icon(&self, _name: &str) -> Option<String> {
        None
    }

    fn desktop_icon(&self, _app_name: &str) -> Option<String> {
        None
    }
}
