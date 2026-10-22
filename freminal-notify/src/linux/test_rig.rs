// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Test support for the D-Bus notification service: shared fixtures, a hand-written
//! fake notification daemon, and the rig that connects a client to it over a
//! socket pair.

use std::collections::HashMap;
use std::error::Error;
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use crossbeam_channel::Receiver;
use futures_lite::StreamExt;
use zbus::message::{Message, Type as MessageType};
use zbus::{Connection, MessageStream, OwnedVariant, Structure, Variant};

use super::super::presentation::IconLookup;
use super::super::test_support::test_assets;
use super::super::{NotificationInstanceId, SystemDispatch, TrackedBackend};
use super::state::OwnerTracking;
use super::{
    LinuxNotifyHandle, LinuxNotifyService, NOTIFICATIONS_INTERFACE, NOTIFICATIONS_PATH,
    SystemNotifyEvent,
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

pub(super) fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| (*item).to_owned()).collect()
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
pub enum NameOwnerAnswer {
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
pub enum CloseAnswer {
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
pub enum Activation {
    Allowed,
    Suppressed,
}

/// What the fake daemon saw and how it should behave.
#[derive(Default)]
pub struct DaemonLog {
    /// Every `Notify` it displayed, in call order.
    pub notifies: Mutex<Vec<RecordedNotify>>,
    /// The id of every `CloseNotification` call, in call order.
    pub closes: Mutex<Vec<u32>>,
    /// Every method call, in call order, with its `NO_AUTO_START` flag.
    pub calls: Mutex<Vec<(String, Activation)>>,
    /// How `CloseNotification` is answered.
    pub close_answer: Mutex<CloseAnswer>,
    /// What `GetCapabilities` answers.
    pub capabilities: Mutex<Vec<String>>,
    /// The sender stamped on replies, as a bus stamps them. Over a
    /// peer-to-peer connection nothing does, hence `None`.
    pub reply_sender: Mutex<Option<String>>,
    /// What `GetNameOwner` answers.
    pub name_owner: Mutex<NameOwnerAnswer>,
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
    pub client: Connection,
    pub daemon: Connection,
    pub log: Arc<DaemonLog>,
    /// Receives one `()` when the fake daemon's serve loop has returned, which
    /// it does once the client's end of the connection is closed.
    pub daemon_stopped: Receiver<()>,
}

pub(super) fn rig(advertised: &[&str]) -> Result<Rig, Box<dyn Error>> {
    rig_with(advertised, None)
}

pub(super) fn rig_with(
    advertised: &[&str],
    method_timeout: Option<Duration>,
) -> Result<Rig, Box<dyn Error>> {
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

impl Rig {
    pub fn recorded(&self) -> Vec<RecordedNotify> {
        self.log
            .notifies
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// The next event, failing the test if none arrives in [`EVENT_TIMEOUT`].
pub(super) fn next_event(handle: &LinuxNotifyHandle) -> Result<SystemNotifyEvent, Box<dyn Error>> {
    Ok(handle.events.recv_timeout(EVENT_TIMEOUT)?)
}

/// The instance a test closes to prove nothing else is queued.
pub(super) const SENTINEL: NotificationInstanceId = NotificationInstanceId::new(u64::MAX);

/// Prove that no event is queued behind the last one received.
///
/// A close of an instance that was never shown is answered at once, from
/// the dispatcher itself, so its event comes after every event the
/// dispatcher produced for the commands and signals it had already taken:
/// if the next event is the sentinel's, nothing was queued before it. A
/// signal still travelling through a pump is not covered, so a test that
/// ignores signals orders a later, observable signal behind them instead.
///
/// # Errors
///
/// Returns an error if no event arrives within `EVENT_TIMEOUT`.
///
/// # Panics
///
/// Panics if the close is not queued, or if the next event is not the
/// sentinel's close.
pub fn assert_no_queued_events(handle: &mut LinuxNotifyHandle) -> TestResult {
    assert_eq!(handle.close(SENTINEL), SystemDispatch::Queued);
    assert_eq!(
        next_event(handle)?,
        SystemNotifyEvent::Closed { instance: SENTINEL }
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
) -> (LinuxNotifyHandle, Receiver<()>) {
    let (waker, woken) = counting_waker();
    let handle = LinuxNotifyService::spawn_with_connection(
        rig.client.clone(),
        owner_tracking,
        waker,
        test_assets(),
    );
    (handle, woken)
}

/// Start the service against a fresh peer-to-peer rig and consume the
/// `Capabilities` event, so a test begins with an empty event queue.
///
/// # Errors
///
/// Returns an error if the socket pair or the fake daemon cannot be set up,
/// or if no event arrives within `EVENT_TIMEOUT`.
///
/// # Panics
///
/// Panics if the first event is not `Capabilities`.
pub fn started(
    advertised: &[&str],
) -> Result<(Rig, LinuxNotifyHandle, Receiver<()>), Box<dyn Error>> {
    started_with(rig(advertised)?)
}

/// [`started`] for an existing rig.
pub(super) fn started_with(
    rig: Rig,
) -> Result<(Rig, LinuxNotifyHandle, Receiver<()>), Box<dyn Error>> {
    let (handle, woken) = spawn_on(&rig, OwnerTracking::Peer);
    assert!(matches!(
        next_event(&handle)?,
        SystemNotifyEvent::Capabilities(_)
    ));
    Ok((rig, handle, woken))
}

/// An icon lookup that knows nothing, so a test touches no filesystem.
pub(super) struct NoIconLookup;

impl IconLookup for NoIconLookup {
    fn matched_icon(&self, _name: &str) -> Option<String> {
        None
    }

    fn desktop_icon(&self, _app_name: &str) -> Option<String> {
        None
    }
}
