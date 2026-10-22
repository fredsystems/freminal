// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! The Linux/BSD OSC 99 desktop-notification service (Task 138.12).
//!
//! # Threads
//!
//! One *dispatcher* thread (`freminal-osc99-dbus`) owns one D-Bus connection
//! and calls `org.freedesktop.Notifications` on it (`GetCapabilities` at start
//! and after every owner change, `Notify` per show, `CloseNotification` per
//! close). Up to two *signal pump* threads each read one match rule's
//! [`MessageStream`] off that connection and forward what they see **into the
//! dispatcher's input channel**, not to the GUI:
//!
//! - `freminal-osc99-signals` forwards `ActionInvoked` / `NotificationClosed`;
//! - `freminal-osc99-owner` (bus connections only) forwards `NameOwnerChanged`
//!   for the daemon's name.
//!
//! The dispatcher owns the D-Bus-id to instance map, so a signal that arrives
//! while a `Notify` call is in flight is only processed after the call has
//! recorded its id. That removes the ordering race between the threads.
//! `GetNameOwner` and `GetCapabilities` go through the async proxy, raced
//! against a timer on the dispatcher thread, so that they can carry the
//! `NO_AUTO_START` flag *and* be bounded by [`OSC99_DBUS_METHOD_TIMEOUT`]; see
//! `wire::call_no_auto_start`.
//!
//! zbus 6 has no blocking API, so every zbus operation the dispatcher makes
//! is one [`zbus::block_on`] around the async call, and each pump is one
//! [`zbus::block_on`] around a loop over its stream. The threads are the
//! dispatcher's `crossbeam` `select!` loop (never inside a `block_on`) and the
//! pumps; a pump only ever sends on an unbounded channel, so no thread blocks
//! inside a `block_on` waiting for another. Several threads each calling
//! `block_on` on futures of the same connection is what zruntime's driver
//! seat hand-off supports: the connection's tasks run on the thread that built
//! it (or a helper thread while none is inside `block_on`), and the other
//! threads poll their own futures when those tasks wake them.
//!
//! The GUI talks to the service through an [`Osc99DbusHandle`]: commands go in
//! over one bounded crossbeam channel (a command that does not fit fails at
//! the handle) and [`Osc99DbusEvent`]s come back over another,
//! each event followed by a repaint wake so an idle window still drains it.
//!
//! # Shutdown
//!
//! Dropping the handle closes the command channel, and the dispatcher thread
//! returns the next time it looks at it: at once when idle, or after the call
//! in flight (at most [`OSC99_DBUS_METHOD_TIMEOUT`]). Returning drops the
//! event sender and the signal receiver. A signal pump is waiting on its
//! stream and notices only when it next has something to forward: it exits
//! when that send fails, or when the connection ends (a connection's streams
//! end once it is closed). Until then it is idle and holds a clone of the
//! connection. Neither pump does any work after the dispatcher is gone, and
//! both end with the process.
//!
//! # Owner tracking
//!
//! Signals are filtered by sender (Task 138.C2). On a bus connection the
//! dispatcher resolves the unique name that owns `org.freedesktop.Notifications`
//! (`GetNameOwner`) and ignores `ActionInvoked` / `NotificationClosed` from any
//! other sender, so a session-bus peer cannot forge them. The owner pump
//! follows `NameOwnerChanged` so a daemon restart re-targets the filter.
//! zbus's client-side match cannot resolve a well-known sender, so the sender
//! is compared in the dispatcher rather than put in the match rule. A
//! peer-to-peer connection has no bus, no owner and no sender to check, so it
//! is [`Osc99SignalSender::Unfiltered`]. Which of the two applies is the
//! [`OwnerTracking`] mode, chosen by whoever supplies the connection.
//!
//! Each mapping records the unique name that answered its `Notify`. When the
//! owner changes, only instances recorded under a *different* owner are closed
//! (their daemon is gone and will never send the signals that would end
//! them). An instance with no recorded owner (a peer-to-peer reply has no
//! sender) is never closed by an owner change. If `GetNameOwner` fails or finds
//! nobody, the owner is unknown and the sender of the next successful `Notify`
//! reply is adopted as the owner, which also covers a daemon that D-Bus
//! activated for that very call. Every change to a new owner fetches
//! `GetCapabilities` again.
//!
//! # Failure handling
//!
//! - **Pump loss.** If either pump ends, every mapped instance is reported
//!   closed, [`Osc99DbusEvent::Unavailable`] is emitted, the connection is
//!   closed (so a pump that is still waiting on it ends too), and the service
//!   is unavailable, as after a failed connect (below).
//! - **Hung daemon.** A `Notify`, `CloseNotification` or `GetCapabilities`
//!   that times out puts the daemon in `state::Osc99DaemonHealth::Unresponsive`:
//!   for `state::OSC99_DAEMON_COOLDOWN` shows fail and closes complete without
//!   calling it, then the next command tries again.
//! - **No daemon at start.** `GetNameOwner` and `GetCapabilities` never
//!   D-Bus-activate a daemon (`NO_AUTO_START`); the first `Notify` does. A
//!   missing daemon is a debug log, not a warning.
//! - **Reconnecting.** While the service is unavailable (the connect failed at
//!   start, or the signals were lost), the dispatcher answers every command as
//!   if the bus were down (a show is `ShowFailed`, a close is `Closed`) and
//!   tries to connect again every [`OSC99_RECONNECT_INTERVAL`] on its own
//!   timer; no command is needed to trigger an attempt. Only the first failure
//!   is reported ([`Osc99DbusEvent::Unavailable`]); a failed retry is a debug
//!   log. A successful reconnect starts a new session, which reports
//!   [`Osc99DbusEvent::Capabilities`] (the daemon's, or the assumed ones when
//!   nobody can be asked yet), so the GUI's daemon state becomes available
//!   again and the capabilities are re-broadcast.
//!
//! Every state transition lives in the pure [`DispatcherState`], which has no
//! bus and no threads and is unit-tested directly.
//!
//! Update in place sends the replaced instance's D-Bus id as `replaces_id`.
//! If that `Notify` fails, a best-effort `CloseNotification` for the old id
//! follows, so the old notification does not stay on screen unmanaged. Close
//! calls `CloseNotification` and lets the daemon's `NotificationClosed`
//! signal produce the [`Osc99DbusEvent::Closed`]; an instance with no D-Bus id
//! (its show failed, or it was replaced) is reported closed at once.
//!
//! How a notification is presented (icons, sounds, category, markup) is decided
//! by the pure functions in `presentation`, and the wire form of the hints is
//! built in `wire`; this module only carries it. The XDG icon scan behind
//! `n=` and `f=` starts on the first notification that needs it, not with the
//! service (see `icon_lookup`).

#[cfg(test)]
mod breaker_tests;
mod pump;
mod state;
#[cfg(test)]
mod test_rig;
mod wake;
mod wire;

pub use wake::Osc99Wake;

use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, TrySendError, select};
use freminal_common::buffer_states::osc99_notification::Notification99Data;
use freminal_common::send_or_log;
use zbus::{Connection, MessageStream};

use super::icon_lookup::XdgIconLookup;
use super::presentation::Osc99IconLookup;
use super::{
    Osc99DaemonCapabilities, Osc99InstanceId, Osc99LegActivation, Osc99SystemBackend,
    Osc99SystemDispatch,
};
use pump::{
    PumpKind, PumpSignal, notification_signals_rule, owner_changed_rule, parse_owner_changed,
    parse_signal, spawn_pump,
};
use state::{CloseTarget, DaemonGate, DispatcherState, Osc99SignalSender, OwnerTracking};
use wire::{
    close_notification, fetch_capabilities, fetch_daemon_owner, is_no_daemon, is_no_name_owner,
    notify,
};

/// The well-known bus name of the notification daemon.
const NOTIFICATIONS_SERVICE: &str = "org.freedesktop.Notifications";
/// The object path the notification daemon serves.
const NOTIFICATIONS_PATH: &str = "/org/freedesktop/Notifications";
/// The notification interface.
const NOTIFICATIONS_INTERFACE: &str = "org.freedesktop.Notifications";
/// The bus driver's name, which is also the sender of its signals.
const BUS_DRIVER_NAME: &str = "org.freedesktop.DBus";
/// The object path of the bus driver.
const BUS_DRIVER_PATH: &str = "/org/freedesktop/DBus";
/// The bus driver's interface.
const BUS_DRIVER_INTERFACE: &str = "org.freedesktop.DBus";
/// The error the bus driver answers `GetNameOwner` with when nobody owns the
/// name. Production code matches `fdo::Error::NameHasNoOwner`; the name is for
/// the tests' fake daemon.
#[cfg(test)]
const ERROR_NAME_HAS_NO_OWNER: &str = "org.freedesktop.DBus.Error.NameHasNoOwner";
/// The error the bus answers a `NO_AUTO_START` call to a name that nobody owns
/// and that no service file can start. Production code matches
/// `fdo::Error::ServiceUnknown`; the name is for the tests' fake daemon.
#[cfg(test)]
const ERROR_SERVICE_UNKNOWN: &str = "org.freedesktop.DBus.Error.ServiceUnknown";

/// The time between attempts to reconnect while the service is unavailable.
const OSC99_RECONNECT_INTERVAL: Duration = Duration::from_secs(30);

/// How long a D-Bus method call waits for its reply before it fails. This is
/// the libdbus default reply timeout. zbus's calls have no timeout of their
/// own by default, so without it a daemon that never answers would block the
/// dispatcher thread forever and silently end the Linux leg. The bus
/// connection carries it as its `method_timeout` (which bounds `Notify` and
/// `CloseNotification`), and `wire::call_no_auto_start` applies it to the calls zbus
/// cannot bound itself. A timed-out `Notify` becomes
/// [`Osc99DbusEvent::ShowFailed`], a timed-out `CloseNotification` falls back
/// to [`Osc99DbusEvent::Closed`], and any timeout starts the
/// `state::OSC99_DAEMON_COOLDOWN`.
const OSC99_DBUS_METHOD_TIMEOUT: Duration = Duration::from_secs(25);

/// How many commands may wait for the dispatcher. The dispatcher serves one
/// at a time, and a hung daemon holds each call for up to
/// [`OSC99_DBUS_METHOD_TIMEOUT`], so a program flooding OSC 99 must not grow
/// the queue without bound. A command that does not fit fails at the handle
/// (the registry then ends the leg), as the untracked leg drops what does not
/// fit its queue (138.C25).
const COMMAND_QUEUE_CAPACITY: usize = 256;

// ── Public vocabulary ──────────────────────────────────────────────────────

/// The action a user invoked on a desktop notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Osc99SystemAction {
    /// The notification itself (action key `"default"`).
    Default,
    /// The 1-based button (action key `"1"` to `"8"`).
    Button(u8),
}

impl Osc99SystemAction {
    /// The registry's vocabulary for the same activation.
    #[must_use]
    pub const fn to_leg_activation(self) -> Osc99LegActivation {
        match self {
            Self::Default => Osc99LegActivation::Body,
            Self::Button(n) => Osc99LegActivation::Button(n),
        }
    }

    /// The action an `ActionInvoked` key stands for, or `None` for a key
    /// freminal never registered.
    fn from_key(key: &str) -> Option<Self> {
        match key.as_bytes() {
            b"default" => Some(Self::Default),
            [digit @ b'1'..=b'8'] => Some(Self::Button(digit - b'0')),
            _ => None,
        }
    }
}

/// What the service reports to the GUI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Osc99DbusEvent {
    /// The daemon accepted `instance`.
    Shown {
        /// The instance that was displayed.
        instance: Osc99InstanceId,
    },
    /// The daemon did not display `instance`.
    ShowFailed {
        /// The instance that was not displayed.
        instance: Osc99InstanceId,
    },
    /// The user invoked an action on `instance`.
    Activated {
        /// The instance that was activated.
        instance: Osc99InstanceId,
        /// What the user invoked.
        action: Osc99SystemAction,
    },
    /// The daemon closed `instance`.
    Closed {
        /// The instance that was closed.
        instance: Osc99InstanceId,
    },
    /// The daemon answered `GetCapabilities`.
    Capabilities(Osc99DaemonCapabilities),
    /// The session bus (or the signal subscription) could not be set up: the
    /// system leg cannot display or observe anything.
    Unavailable,
}

// ── Handle ─────────────────────────────────────────────────────────────────

/// A request from the GUI to the dispatcher.
#[derive(Debug)]
enum DbusCommand {
    /// Display `notification` for `instance`, replacing `replaces`'.
    Show {
        instance: Osc99InstanceId,
        replaces: Option<Osc99InstanceId>,
        notification: Box<Notification99Data>,
    },
    /// Close `instance`'s notification. The dispatcher answers with
    /// [`Osc99DbusEvent::Closed`] (from the daemon's signal, or at once when
    /// there is nothing to close).
    Close { instance: Osc99InstanceId },
}

/// The GUI's end of the service.
///
/// Dropping it stops the dispatcher; see the module docs for when each thread
/// exits.
pub struct Osc99DbusHandle {
    /// Commands for the dispatcher.
    commands: Sender<DbusCommand>,
    /// Events from the dispatcher.
    events: Receiver<Osc99DbusEvent>,
}

impl Osc99DbusHandle {
    /// The events that have arrived since the last call, oldest first. Never
    /// blocks.
    pub fn try_events(&self) -> impl Iterator<Item = Osc99DbusEvent> + '_ {
        self.events.try_iter()
    }
}

impl Osc99DbusHandle {
    /// Queue `command` without blocking. A full queue or a dispatcher that is
    /// gone will never answer, so the caller must end the instance's system
    /// leg itself.
    fn dispatch(&self, command: DbusCommand, what: &'static str) -> Osc99SystemDispatch {
        match self.commands.try_send(command) {
            Ok(()) => Osc99SystemDispatch::Queued,
            Err(TrySendError::Full(_)) => {
                tracing::debug!(
                    "OSC 99: notification service queue is full ({COMMAND_QUEUE_CAPACITY}); \
                     {what} failed"
                );
                Osc99SystemDispatch::Failed
            }
            Err(TrySendError::Disconnected(_)) => {
                tracing::warn!("OSC 99: notification service is gone; {what} failed");
                Osc99SystemDispatch::Failed
            }
        }
    }
}

impl Osc99SystemBackend for Osc99DbusHandle {
    fn show(
        &mut self,
        instance: Osc99InstanceId,
        replaces: Option<Osc99InstanceId>,
        notification: &Notification99Data,
    ) -> Osc99SystemDispatch {
        let command = DbusCommand::Show {
            instance,
            replaces,
            notification: Box::new(notification.clone()),
        };
        self.dispatch(command, "show")
    }

    fn close(&mut self, instance: Osc99InstanceId) -> Osc99SystemDispatch {
        self.dispatch(DbusCommand::Close { instance }, "close")
    }
}

// ── Service ────────────────────────────────────────────────────────────────

/// Why the service could not start.
#[derive(Debug, thiserror::Error)]
enum StartError {
    /// The bus connection or the signal subscription failed.
    #[error("session bus unavailable: {0}")]
    Bus(#[from] zbus::Error),
    /// A signal pump thread could not be spawned.
    #[error("could not spawn the signal pump thread: {0}")]
    PumpThread(#[source] std::io::Error),
}

/// The dispatcher's way back to the GUI: an event channel plus the repaint
/// wake that follows every event.
struct EventSink {
    events: Sender<Osc99DbusEvent>,
    waker: Box<dyn Fn() + Send>,
}

impl EventSink {
    /// Send `event` to the GUI, then wake it.
    fn emit(&self, event: Osc99DbusEvent) {
        if let Err(e) = self.events.send(event) {
            tracing::debug!("OSC 99: GUI is gone; dropping a notification event: {e}");
            return;
        }
        (self.waker)();
    }
}

/// Namespace for starting the notification service.
pub struct Osc99DbusService;

impl Osc99DbusService {
    /// Start the service on the session bus. `waker` is called after every
    /// event so an idle GUI drains it.
    ///
    /// Never blocks: connecting happens on the dispatcher thread. If the bus
    /// cannot be reached the handle reports [`Osc99DbusEvent::Unavailable`]
    /// once, a single warning is logged, and every show answers
    /// [`Osc99DbusEvent::ShowFailed`] and every close
    /// [`Osc99DbusEvent::Closed`], until a reconnect attempt, made every
    /// [`OSC99_RECONNECT_INTERVAL`], succeeds.
    #[must_use]
    pub fn spawn(waker: Box<dyn Fn() + Send>) -> Osc99DbusHandle {
        start(
            connect_session,
            waker,
            Box::new(XdgIconLookup::from_env()),
            OSC99_RECONNECT_INTERVAL,
        )
    }

    /// Start the service on an existing connection (a peer-to-peer one, in
    /// tests), tracking the daemon's owner as `owner_tracking` says.
    #[cfg(test)]
    fn spawn_with_connection(
        connection: Connection,
        owner_tracking: OwnerTracking,
        waker: Box<dyn Fn() + Send>,
    ) -> Osc99DbusHandle {
        start(
            move || {
                Ok(Link {
                    connection: connection.clone(),
                    owner_tracking,
                })
            },
            waker,
            Box::new(test_rig::NoIconLookup),
            OSC99_RECONNECT_INTERVAL,
        )
    }
}

/// A connection and how the daemon's owner is tracked on it.
struct Link {
    connection: Connection,
    owner_tracking: OwnerTracking,
}

/// Connect to the session bus with [`OSC99_DBUS_METHOD_TIMEOUT`] on every
/// method call.
fn connect_session() -> zbus::Result<Link> {
    let connection = zbus::block_on(
        zbus::connection::Builder::session()
            .method_timeout(OSC99_DBUS_METHOD_TIMEOUT)
            .build(),
    )?;
    Ok(Link {
        owner_tracking: OwnerTracking::of(&connection),
        connection,
    })
}

/// Start the dispatcher thread, which obtains its connection from `connect`
/// (again every `reconnect_interval` while it is unavailable) and resolves
/// icons with `icon_lookup` (whose scan runs on its own background thread,
/// started by its first question).
fn start<C>(
    connect: C,
    waker: Box<dyn Fn() + Send>,
    icon_lookup: Box<dyn Osc99IconLookup + Send>,
    reconnect_interval: Duration,
) -> Osc99DbusHandle
where
    C: FnMut() -> zbus::Result<Link> + Send + 'static,
{
    let (command_tx, command_rx) = crossbeam_channel::bounded(COMMAND_QUEUE_CAPACITY);
    let (event_tx, event_rx) = crossbeam_channel::unbounded();
    let sink = EventSink {
        events: event_tx.clone(),
        waker,
    };

    let spawned = std::thread::Builder::new()
        .name("freminal-osc99-dbus".to_owned())
        .spawn(move || {
            run_dispatcher(connect, &command_rx, &sink, icon_lookup, reconnect_interval);
        });
    if let Err(e) = spawned {
        // The closure (and the command receiver in it) is dropped, so every
        // later show fails at the handle.
        tracing::warn!("OSC 99: could not spawn the notification service thread: {e}");
        send_or_log!(
            event_tx,
            Osc99DbusEvent::Unavailable,
            "Failed to report the OSC 99 service as unavailable"
        );
    }

    Osc99DbusHandle {
        commands: command_tx,
        events: event_rx,
    }
}

/// A live connection: the dispatcher's calls go out over `connection`, and the
/// pumps' signals come back on `signals`.
struct Session {
    connection: Connection,
    owner_tracking: OwnerTracking,
    signals: Receiver<PumpSignal>,
}

/// How [`serve`] ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ServeEnd {
    /// The GUI dropped its command sender.
    GuiGone,
    /// A signal pump ended: nothing can be tracked any more.
    SignalsLost,
}

/// Which session [`serve`] is starting, which decides whether the GUI must be
/// told the daemon's capabilities even when nobody can be asked for them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionStart {
    /// The first connection: the GUI assumes a daemon until told otherwise.
    Initial,
    /// A connection made after an outage, which the GUI was told about
    /// ([`Osc99DbusEvent::Unavailable`]): it needs a
    /// [`Osc99DbusEvent::Capabilities`] to see the service as available again.
    Reconnected,
}

/// Whether [`refresh_capabilities`] sent [`Osc99DbusEvent::Capabilities`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CapabilitiesReport {
    Sent,
    Withheld,
}

/// The dispatcher thread: connect, serve commands until the GUI drops its
/// command sender, and reconnect (every `reconnect_interval`) whenever there
/// is no connection to serve on.
fn run_dispatcher<C>(
    mut connect: C,
    commands: &Receiver<DbusCommand>,
    sink: &EventSink,
    icon_lookup: Box<dyn Osc99IconLookup + Send>,
    reconnect_interval: Duration,
) where
    C: FnMut() -> zbus::Result<Link>,
{
    let mut state = DispatcherState::new(Osc99SignalSender::NoOwner, icon_lookup);
    let mut start = SessionStart::Initial;
    let mut first = match connect_and_subscribe(&mut connect) {
        Ok(session) => Some(session),
        Err(e) => {
            tracing::warn!("OSC 99: system notifications are unavailable: {e}");
            sink.emit(Osc99DbusEvent::Unavailable);
            None
        }
    };
    loop {
        let session = if let Some(session) = first.take() {
            session
        } else {
            start = SessionStart::Reconnected;
            let Some(session) = reconnect(&mut connect, commands, sink, reconnect_interval) else {
                return;
            };
            session
        };
        match serve(session, &mut state, commands, sink, start) {
            ServeEnd::GuiGone => return,
            // `serve` reported the loss and closed the connection; wait for
            // the next attempt as after a failed connect.
            ServeEnd::SignalsLost => {}
        }
    }
}

/// While unavailable: fail every command, and every `interval` try to connect.
/// Returns the new session, or `None` when the GUI is gone.
///
/// A failed attempt is not reported again ([`Osc99DbusEvent::Unavailable`]
/// was sent when the service became unavailable); the wait starts over.
fn reconnect<C>(
    connect: &mut C,
    commands: &Receiver<DbusCommand>,
    sink: &EventSink,
    interval: Duration,
) -> Option<Session>
where
    C: FnMut() -> zbus::Result<Link>,
{
    loop {
        let due = Instant::now() + interval;
        loop {
            match commands.recv_deadline(due) {
                Ok(command) => report_unserved(sink, &command),
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => return None,
            }
        }
        match connect_and_subscribe(connect) {
            Ok(session) => return Some(session),
            Err(e) => {
                tracing::debug!("OSC 99: reconnecting to the notification service failed: {e}");
            }
        }
    }
}

/// Connect, subscribe to the notification signals, and start the pump(s).
///
/// The subscriptions are made here, before any pump thread starts and before
/// the first `Notify` or `GetNameOwner`, so no signal can be missed. With
/// [`OwnerTracking::Bus`] there is a second subscription, to `NameOwnerChanged`
/// for the daemon's name, with its own pump: a pump drives one stream inside
/// one `block_on`, and match rules cannot be OR-ed, so two streams need two
/// threads.
/// Both pumps feed the one signal channel. [`OwnerTracking::Peer`] has no bus
/// driver to ask, so it gets only the first.
///
/// If a pump thread cannot be spawned, the connection is closed before the
/// error is returned. Nothing will serve it, and a pump that did start would
/// otherwise stay blocked on its stream (deaf, but alive and holding the
/// connection) until its next message, which on a quiet bus may never come.
/// Closing ends the stream, so that pump exits now; the signal receiver is
/// dropped with the error as well, so the `Ended` it sends goes nowhere.
fn connect_and_subscribe<C>(connect: &mut C) -> Result<Session, StartError>
where
    C: FnMut() -> zbus::Result<Link>,
{
    let Link {
        connection,
        owner_tracking,
    } = connect()?;
    let notifications = zbus::block_on(MessageStream::for_match_rule(
        notification_signals_rule()?,
        &connection,
        None,
    ))?;
    let owner_changes = match owner_tracking {
        OwnerTracking::Bus => Some(zbus::block_on(MessageStream::for_match_rule(
            owner_changed_rule()?,
            &connection,
            None,
        ))?),
        OwnerTracking::Peer => None,
    };

    let (signal_tx, signals) = crossbeam_channel::unbounded();
    let spawned = owner_changes
        .map_or(Ok(()), |messages| {
            spawn_pump(
                "freminal-osc99-owner",
                PumpKind::OwnerChanges,
                messages,
                signal_tx.clone(),
                parse_owner_changed,
            )
        })
        .and_then(|()| {
            spawn_pump(
                "freminal-osc99-signals",
                PumpKind::Notifications,
                notifications,
                signal_tx,
                parse_signal,
            )
        });
    if let Err(e) = spawned {
        if let Err(close_error) = close_connection(&connection) {
            tracing::debug!("OSC 99: closing the unused connection failed: {close_error}");
        }
        return Err(e);
    }
    Ok(Session {
        connection,
        owner_tracking,
        signals,
    })
}

/// Close `connection` (every clone of it: they share one socket), which ends
/// the pumps' streams.
fn close_connection(connection: &Connection) -> zbus::Result<()> {
    zbus::block_on(connection.clone().close())
}

/// Answer a command that nothing will serve: a show fails and a close
/// completes (with nothing displayed, there is nothing to close).
fn report_unserved(sink: &EventSink, command: &DbusCommand) {
    match command {
        DbusCommand::Show { instance, .. } => {
            sink.emit(Osc99DbusEvent::ShowFailed {
                instance: *instance,
            });
        }
        DbusCommand::Close { instance } => sink.emit(Osc99DbusEvent::Closed {
            instance: *instance,
        }),
    }
}

/// Serve `commands` and the pumps' signals until the GUI drops its command
/// sender or a pump ends (in which case the connection is closed, so a pump
/// that is still blocked on it ends too).
///
/// The capabilities are fetched when serving starts (unless nobody owns the
/// name) and again whenever the owner changes to a name they were not
/// fetched from, which also covers an owner adopted from a `Notify` reply. A
/// [`SessionStart::Reconnected`] session that could not ask anyone reports the
/// assumed capabilities instead, so the GUI sees the service as available.
fn serve(
    session: Session,
    state: &mut DispatcherState,
    commands: &Receiver<DbusCommand>,
    sink: &EventSink,
    start: SessionStart,
) -> ServeEnd {
    let Session {
        connection,
        owner_tracking,
        signals,
    } = session;
    state.begin_session(resolve_daemon_owner(&connection, owner_tracking));
    let report = if matches!(state.daemon_owner, Osc99SignalSender::NoOwner) {
        CapabilitiesReport::Withheld
    } else {
        refresh_capabilities(&connection, state, sink)
    };
    if start == SessionStart::Reconnected && report == CapabilitiesReport::Withheld {
        sink.emit(Osc99DbusEvent::Capabilities(state.capabilities));
    }
    loop {
        select! {
            recv(commands) -> command => match command {
                Ok(command) => handle_command(&connection, state, sink, command),
                Err(_) => return ServeEnd::GuiGone,
            },
            recv(signals) -> signal => {
                if let Some(end) = handle_signal(&connection, state, sink, signal.ok()) {
                    if let Err(e) = close_connection(&connection) {
                        tracing::debug!("OSC 99: closing the lost connection failed: {e}");
                    }
                    return end;
                }
            },
        }
        refresh_stale_capabilities(&connection, state, sink);
    }
}

/// Apply a pump `signal` (`None`: every pump is gone and the channel closed
/// without a word). Returns [`ServeEnd::SignalsLost`] once no more signals can
/// come, so the caller stops serving.
///
/// An activation is followed by a close of the activated notification.
/// freminal never sends the `resident` hint, so the specification has the
/// daemon remove the notification itself once an action is invoked and emit
/// `NotificationClosed`. Not every daemon emits it (wayle 0.1.3 does not for
/// a notification sent with `expire_timeout` 0), and without it the leg would
/// stay open forever. Closing it explicitly ends the leg either way: the call
/// fails harmlessly when the daemon already removed it, and the daemon's own
/// signal, if it comes, arrives for a forgotten id and is ignored (138.C23).
fn handle_signal(
    connection: &Connection,
    state: &mut DispatcherState,
    sink: &EventSink,
    signal: Option<PumpSignal>,
) -> Option<ServeEnd> {
    let (events, end) = match signal {
        Some(signal @ PumpSignal::Ended(_)) => {
            (state.on_signal(&signal), Some(ServeEnd::SignalsLost))
        }
        Some(signal) => (state.on_signal(&signal), None),
        None => {
            tracing::warn!("OSC 99: every signal pump is gone; no more signals");
            (state.on_signals_lost(), Some(ServeEnd::SignalsLost))
        }
    };
    for event in events {
        let activated = match event {
            Osc99DbusEvent::Activated { instance, .. } => Some(instance),
            _ => None,
        };
        sink.emit(event);
        if let Some(instance) = activated {
            close_instance(connection, state, sink, instance);
        }
    }
    end
}

/// Carry out one GUI command.
fn handle_command(
    connection: &Connection,
    state: &mut DispatcherState,
    sink: &EventSink,
    command: DbusCommand,
) {
    match command {
        DbusCommand::Show {
            instance,
            replaces,
            notification,
        } => show_notification(connection, state, sink, instance, replaces, &notification),
        DbusCommand::Close { instance } => close_instance(connection, state, sink, instance),
    }
}

/// Carry out a `Show`: call `Notify` and record or report the outcome.
///
/// While the daemon is in its cooldown nothing is called and the show fails.
fn show_notification(
    connection: &Connection,
    state: &mut DispatcherState,
    sink: &EventSink,
    instance: Osc99InstanceId,
    replaces: Option<Osc99InstanceId>,
    notification: &Notification99Data,
) {
    if state.daemon_gate(Instant::now()) == DaemonGate::Skip {
        tracing::debug!("OSC 99: the notification daemon is unresponsive; failing a show");
        fail_show(connection, state, sink, instance, replaces);
        return;
    }
    let args = state.notify_args(replaces, notification);
    let outcome = notify(connection, &args);
    state.note_call(&outcome, Instant::now());
    match outcome {
        Ok(reply) => {
            if let Some(evicted) = state.record_shown(instance, replaces, reply) {
                sink.emit(Osc99DbusEvent::Closed { instance: evicted });
            }
            sink.emit(Osc99DbusEvent::Shown { instance });
        }
        Err(e) => {
            tracing::warn!("OSC 99: Notify failed: {e}");
            fail_show(connection, state, sink, instance, replaces);
        }
    }
}

/// Report a show as failed. The registry already forgot the replaced
/// instance, so its signals are of no interest either; but the daemon may
/// still display it, so it gets a best-effort `CloseNotification` rather than
/// staying on screen with nothing tracking it.
fn fail_show(
    connection: &Connection,
    state: &mut DispatcherState,
    sink: &EventSink,
    instance: Osc99InstanceId,
    replaces: Option<Osc99InstanceId>,
) {
    if let Some(old) = replaces {
        let old_id = state.dbus_id_of(old);
        state.forget_instance(old);
        if let Some(old_id) = old_id {
            close_replaced(connection, state, old_id);
        }
    }
    sink.emit(Osc99DbusEvent::ShowFailed { instance });
}

/// Ask the daemon to close a notification nobody tracks any more. The outcome
/// matters to nothing but the daemon's health, so a failure is only logged.
fn close_replaced(connection: &Connection, state: &mut DispatcherState, dbus_id: u32) {
    if state.daemon_gate(Instant::now()) == DaemonGate::Skip {
        return;
    }
    let outcome = close_notification(connection, dbus_id);
    state.note_call(&outcome, Instant::now());
    if let Err(e) = outcome {
        tracing::debug!("OSC 99: closing the replaced notification failed: {e}");
    }
}

/// Carry out a `Close`.
///
/// An instance the daemon displays is closed with `CloseNotification`. The
/// mappings are then dropped and `Closed` is emitted here, whatever the
/// outcome, so the daemon's own `NotificationClosed` for it (if it sends one)
/// arrives for an unknown id and is ignored. An instance with no D-Bus id is
/// reported closed at once.
///
/// **Differs from kitty**, which waits for `NotificationClosed` after a
/// successful call. The specification says the call closes the notification
/// and emits the signal, but not every daemon emits it: wayle 0.1.3 silently
/// drops a notification sent with `expire_timeout` 0 ("never expires") from
/// its own tracking, and a later `CloseNotification` then emits nothing.
/// Waiting would leave the leg open forever: no `c=1` close report, and a
/// `p=alive` that keeps listing the notification. A successful call is taken
/// as the close it is specified to be (138.C23).
fn close_instance(
    connection: &Connection,
    state: &mut DispatcherState,
    sink: &EventSink,
    instance: Osc99InstanceId,
) {
    let CloseTarget::CallDaemon(dbus_id) = state.close_target(instance) else {
        sink.emit(Osc99DbusEvent::Closed { instance });
        return;
    };
    if state.daemon_gate(Instant::now()) == DaemonGate::Skip {
        tracing::debug!("OSC 99: the notification daemon is unresponsive; closing locally");
    } else {
        let outcome = close_notification(connection, dbus_id);
        state.note_call(&outcome, Instant::now());
        if let Err(e) = outcome {
            tracing::debug!("OSC 99: CloseNotification failed: {e}");
        }
    }
    state.forget_instance(instance);
    sink.emit(Osc99DbusEvent::Closed { instance });
}

/// Fetch the capabilities again if the owner changed since they were fetched.
fn refresh_stale_capabilities(
    connection: &Connection,
    state: &mut DispatcherState,
    sink: &EventSink,
) {
    if state.capabilities_stale() {
        refresh_capabilities(connection, state, sink);
    }
}

/// Call `GetCapabilities` and, if it answers, record and report the result.
/// While the daemon is in its cooldown the call is not made and the
/// capabilities stay stale, so the first command after the cooldown asks.
fn refresh_capabilities(
    connection: &Connection,
    state: &mut DispatcherState,
    sink: &EventSink,
) -> CapabilitiesReport {
    if state.daemon_gate(Instant::now()) == DaemonGate::Skip {
        tracing::debug!("OSC 99: the notification daemon is unresponsive; not asking it");
        return CapabilitiesReport::Withheld;
    }
    state.note_capabilities_asked();
    let outcome = fetch_capabilities(connection);
    state.note_call(&outcome, Instant::now());
    match outcome {
        Ok(advertised) => {
            state.set_capabilities(&advertised);
            sink.emit(Osc99DbusEvent::Capabilities(state.capabilities));
            CapabilitiesReport::Sent
        }
        Err(e) if is_no_daemon(&e) => {
            tracing::debug!("OSC 99: no notification daemon to ask for its capabilities: {e}");
            CapabilitiesReport::Withheld
        }
        Err(e) => {
            tracing::warn!("OSC 99: GetCapabilities failed; assuming defaults: {e}");
            CapabilitiesReport::Withheld
        }
    }
}

/// Who owns `org.freedesktop.Notifications` at start.
///
/// A peer-to-peer connection has no bus driver and no sender to check, so it
/// is [`Osc99SignalSender::Unfiltered`]. On a bus, only `NameHasNoOwner` means
/// that nobody owns the name; that and every other failure leave the owner
/// unknown ([`Osc99SignalSender::NoOwner`]): signals are ignored until a
/// `NameOwnerChanged` names an owner, or the sender of a `Notify` reply is
/// adopted (see [`DispatcherState::record_shown`]), so a lookup that merely
/// failed cannot leave the service deaf for good. Only an unexpected failure
/// is worth a warning: a missing daemon is the normal state of a machine
/// without one, and the call is sent with `NO_AUTO_START`, so it does not
/// start one. The `NameOwnerChanged` subscription was made before this call, so
/// an owner appearing meanwhile is queued for the dispatcher, and processing
/// it after a matching answer here is a no-op.
fn resolve_daemon_owner(connection: &Connection, tracking: OwnerTracking) -> Osc99SignalSender {
    match tracking {
        OwnerTracking::Peer => Osc99SignalSender::Unfiltered,
        OwnerTracking::Bus => match fetch_daemon_owner(connection) {
            Ok(owner) => Osc99SignalSender::Owner(owner),
            Err(e) if is_no_name_owner(&e) => {
                tracing::debug!("OSC 99: no notification daemon owns its name yet");
                Osc99SignalSender::NoOwner
            }
            Err(e) => {
                tracing::warn!(
                    "OSC 99: GetNameOwner failed ({e}); the next Notify reply names the daemon"
                );
                Osc99SignalSender::NoOwner
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use crossbeam_channel::{Receiver, RecvTimeoutError};
    use freminal_common::buffer_states::osc99_notification::{
        NotificationUrgency, Osc99Expiry, Osc99Sound,
    };

    use super::test_rig::*;
    use super::*;
    use crate::gui::osc99::Osc99DaemonSupport;

    #[test]
    fn system_actions_map_to_the_registry_activations() {
        assert_eq!(
            Osc99SystemAction::Default.to_leg_activation(),
            Osc99LegActivation::Body
        );
        assert_eq!(
            Osc99SystemAction::Button(3).to_leg_activation(),
            Osc99LegActivation::Button(3)
        );
    }

    #[test]
    fn show_calls_notify_then_activation_and_close_signals_become_events() -> TestResult {
        let rig = rig(&["body", "actions"])?;
        let (mut handle, woken) = spawn_on(&rig, OwnerTracking::Peer);

        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Capabilities(Osc99DaemonCapabilities {
                body: Osc99DaemonSupport::Supported,
                actions: Osc99DaemonSupport::Supported,
                body_markup: Osc99DaemonSupport::Unsupported,
                sound: Osc99DaemonSupport::Unsupported,
            })
        );

        assert_eq!(
            handle.show(
                instance(1),
                None,
                &Notification99Data {
                    body: Some("Body".to_owned()),
                    urgency: NotificationUrgency::Critical,
                    expiry: Osc99Expiry::After(NonZeroU32::new(5000).ok_or("zero")?),
                    ..with_buttons(&["A", "B"])
                },
            ),
            Osc99SystemDispatch::Queued
        );
        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Shown {
                instance: instance(1)
            }
        );

        let recorded = rig.recorded();
        assert_eq!(
            recorded,
            [RecordedNotify {
                app_name: "freminal".to_owned(),
                replaces_id: 0,
                app_icon: "freminal".to_owned(),
                summary: "Title".to_owned(),
                body: "Body".to_owned(),
                actions: strings(&["default", "", "1", "A", "2", "B"]),
                urgency: Some(2),
                hint_keys: strings(&["desktop-entry", "urgency"]),
                category: None,
                desktop_entry: Some("freminal".to_owned()),
                sound_name: None,
                image: None,
                expire_timeout: 5000,
                assigned_id: 1,
            }]
        );
        let dbus_id = recorded[0].assigned_id;

        rig.emit_action(dbus_id, "2")?;
        rig.emit_closed(dbus_id, 2)?;
        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Activated {
                instance: instance(1),
                action: Osc99SystemAction::Button(2),
            }
        );
        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Closed {
                instance: instance(1)
            }
        );

        // The activation itself asked the daemon to close the notification
        // (138.C23); the daemon's own signal after it was ignored.
        assert_eq!(rig.closes(), [dbus_id]);
        assert_no_queued_events(&mut handle)?;

        // Every event was followed by a wake.
        for _ in 0..4 {
            woken.recv_timeout(EVENT_TIMEOUT)?;
        }
        Ok(())
    }

    #[test]
    fn presentation_hints_arrive_over_the_wire() -> TestResult {
        let (rig, mut handle, _woken) = started(&["body", "actions", "sound"])?;

        assert_eq!(
            handle.show(
                instance(1),
                None,
                &Notification99Data {
                    icon_data: Some(png(4, 3)?),
                    notification_types: strings(&["email.arrived"]),
                    sound: Osc99Sound::Named("info".to_owned()),
                    ..notification()
                },
            ),
            Osc99SystemDispatch::Queued
        );
        assert!(matches!(next_event(&handle)?, Osc99DbusEvent::Shown { .. }));

        let recorded = rig.recorded();
        assert_eq!(recorded.len(), 1);
        let notify = &recorded[0];
        assert_eq!(
            notify.hint_keys,
            strings(&[
                "category",
                "desktop-entry",
                "image-data",
                "sound-name",
                "urgency"
            ])
        );
        assert_eq!(notify.category.as_deref(), Some("email.arrived"));
        assert_eq!(notify.desktop_entry.as_deref(), Some("freminal"));
        assert_eq!(notify.sound_name.as_deref(), Some("dialog-information"));
        assert_eq!(notify.image, Some((4, 3, 16, true, 8, 4, 4 * 3 * 4)));
        assert_eq!(notify.app_icon, "");
        Ok(())
    }

    #[test]
    fn a_silent_notification_sends_suppress_sound() -> TestResult {
        let (rig, mut handle, _woken) = started(&["body", "actions"])?;

        assert_eq!(
            handle.show(
                instance(1),
                None,
                &Notification99Data {
                    sound: Osc99Sound::Silent,
                    ..notification()
                },
            ),
            Osc99SystemDispatch::Queued
        );
        assert!(matches!(next_event(&handle)?, Osc99DbusEvent::Shown { .. }));

        assert_eq!(
            rig.recorded()[0].hint_keys,
            strings(&["desktop-entry", "suppress-sound", "urgency"])
        );
        Ok(())
    }

    #[test]
    fn a_daemon_without_actions_gets_no_action_list() -> TestResult {
        let (rig, mut handle, _woken) = started(&["body"])?;

        assert_eq!(
            handle.show(instance(1), None, &with_buttons(&["A"])),
            Osc99SystemDispatch::Queued
        );
        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Shown {
                instance: instance(1)
            }
        );

        assert_eq!(rig.recorded()[0].actions, Vec::<String>::new());
        Ok(())
    }

    #[test]
    fn signals_for_unknown_ids_and_other_members_produce_no_event() -> TestResult {
        let (rig, mut handle, _woken) = started(&["body", "actions"])?;
        assert_eq!(
            handle.show(instance(1), None, &notification()),
            Osc99SystemDispatch::Queued
        );
        assert!(matches!(next_event(&handle)?, Osc99DbusEvent::Shown { .. }));

        // Signals for ids nobody shown, and an unrelated member, then the
        // real close: the close is the first (and only) event, so the
        // earlier signals were ignored, in order, by the same dispatcher.
        rig.emit_action(999, "default")?;
        rig.emit_closed(999, 1)?;
        zbus::block_on(rig.daemon.emit_signal(
            None::<&str>,
            NOTIFICATIONS_PATH,
            NOTIFICATIONS_INTERFACE,
            "ActivationToken",
            &(1u32, "token"),
        ))?;
        rig.emit_closed(rig.recorded()[0].assigned_id, 1)?;

        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Closed {
                instance: instance(1)
            }
        );
        assert_no_queued_events(&mut handle)
    }

    #[test]
    fn close_calls_close_notification_once_and_the_daemons_signal_becomes_closed() -> TestResult {
        let (rig, mut handle, _woken) = started(&["body", "actions"])?;
        assert_eq!(
            handle.show(instance(1), None, &notification()),
            Osc99SystemDispatch::Queued
        );
        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Shown {
                instance: instance(1)
            }
        );
        let dbus_id = rig.recorded()[0].assigned_id;

        assert_eq!(handle.close(instance(1)), Osc99SystemDispatch::Queued);

        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Closed {
                instance: instance(1)
            }
        );
        assert_eq!(rig.closes(), [dbus_id], "exactly one call, with the id");
        // One `Closed`: the signal, not also a local one. A local one would
        // be emitted by the close command itself, so it would precede the
        // sentinel's event.
        assert_no_queued_events(&mut handle)
    }

    #[test]
    fn an_update_replaces_the_old_id_and_the_old_ids_close_produces_no_event() -> TestResult {
        let (rig, mut handle, _woken) = started(&["body", "actions"])?;
        let _ = handle.show(instance(1), None, &notification());
        assert!(matches!(next_event(&handle)?, Osc99DbusEvent::Shown { .. }));

        let _ = handle.show(instance(2), Some(instance(1)), &notification());
        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Shown {
                instance: instance(2)
            }
        );

        let recorded = rig.recorded();
        assert_eq!(recorded[1].replaces_id, recorded[0].assigned_id);
        assert_ne!(
            recorded[1].assigned_id, recorded[0].assigned_id,
            "the fake daemon hands out a fresh id, so the two are distinguishable"
        );

        // The old id closes first, then the new one: the new one's `Closed`
        // is the first event, so the old id's was ignored by the same
        // dispatcher, in order.
        rig.emit_closed(recorded[0].assigned_id, 2)?;
        rig.emit_closed(recorded[1].assigned_id, 2)?;
        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Closed {
                instance: instance(2)
            }
        );
        assert_no_queued_events(&mut handle)
    }

    #[test]
    fn closing_a_replaced_instance_reports_closed_without_calling_the_daemon() -> TestResult {
        let (rig, mut handle, _woken) = started(&["body", "actions"])?;
        let _ = handle.show(instance(1), None, &notification());
        assert!(matches!(next_event(&handle)?, Osc99DbusEvent::Shown { .. }));
        let _ = handle.show(instance(2), Some(instance(1)), &notification());
        assert!(matches!(next_event(&handle)?, Osc99DbusEvent::Shown { .. }));

        let _ = handle.close(instance(1));

        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Closed {
                instance: instance(1)
            }
        );
        assert_eq!(rig.closes(), Vec::<u32>::new());
        Ok(())
    }

    #[test]
    fn close_after_a_failed_show_emits_closed_at_once_without_calling_the_daemon() -> TestResult {
        let (rig, mut handle, _woken) = started(&["body", "actions"])?;
        let _ = handle.show(
            instance(1),
            None,
            &Notification99Data {
                title: REJECTED_SUMMARY.to_owned(),
                ..notification()
            },
        );
        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::ShowFailed {
                instance: instance(1)
            }
        );

        assert_eq!(handle.close(instance(1)), Osc99SystemDispatch::Queued);
        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Closed {
                instance: instance(1)
            }
        );

        // A later round trip proves no `CloseNotification` was in flight: the
        // dispatcher handles commands in order.
        let _ = handle.show(instance(2), None, &notification());
        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Shown {
                instance: instance(2)
            }
        );
        assert_eq!(rig.closes(), Vec::<u32>::new());
        Ok(())
    }

    #[test]
    fn close_of_a_never_shown_instance_emits_closed_at_once() -> TestResult {
        let (rig, mut handle, _woken) = started(&["body"])?;

        let _ = handle.close(instance(9));

        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Closed {
                instance: instance(9)
            }
        );
        assert_eq!(rig.closes(), Vec::<u32>::new());
        Ok(())
    }

    #[test]
    fn a_close_the_daemon_never_signals_is_still_reported_once() -> TestResult {
        let (rig, mut handle, _woken) = started(&["body", "actions"])?;
        rig.answer_close(CloseAnswer::CloseSilently);
        let _ = handle.show(instance(1), None, &notification());
        assert!(matches!(next_event(&handle)?, Osc99DbusEvent::Shown { .. }));
        let dbus_id = rig.recorded()[0].assigned_id;

        let _ = handle.close(instance(1));

        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Closed {
                instance: instance(1)
            }
        );
        assert_eq!(rig.closes(), [dbus_id]);
        // A late signal for the forgotten id changes nothing.
        rig.emit_closed(dbus_id, 3)?;
        assert_no_queued_events(&mut handle)
    }

    #[test]
    fn an_activation_the_daemon_never_follows_with_a_close_still_closes() -> TestResult {
        let (rig, mut handle, _woken) = started(&["body", "actions"])?;
        rig.answer_close(CloseAnswer::CloseSilently);
        let _ = handle.show(instance(1), None, &notification());
        assert!(matches!(next_event(&handle)?, Osc99DbusEvent::Shown { .. }));
        let dbus_id = rig.recorded()[0].assigned_id;

        rig.emit_action(dbus_id, "default")?;

        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Activated {
                instance: instance(1),
                action: Osc99SystemAction::Default,
            }
        );
        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Closed {
                instance: instance(1)
            }
        );
        assert_eq!(rig.closes(), [dbus_id]);
        assert_no_queued_events(&mut handle)
    }

    #[test]
    fn a_named_icon_is_sent_as_app_icon_and_image_path() -> TestResult {
        let (rig, mut handle, _woken) = started(&["body", "actions"])?;
        let _ = handle.show(
            instance(1),
            None,
            &Notification99Data {
                icon_names: strings(&["warn"]),
                ..notification()
            },
        );
        assert!(matches!(next_event(&handle)?, Osc99DbusEvent::Shown { .. }));

        let notify = &rig.recorded()[0];
        assert_eq!(notify.app_icon, "dialog-warning");
        assert_eq!(
            notify.hint_keys,
            strings(&["desktop-entry", "image-path", "urgency"])
        );
        Ok(())
    }

    #[test]
    fn a_failing_close_notification_call_falls_back_to_closed() -> TestResult {
        let (rig, mut handle, _woken) = started(&["body", "actions"])?;
        rig.answer_close(CloseAnswer::Fail);
        let _ = handle.show(instance(1), None, &notification());
        assert!(matches!(next_event(&handle)?, Osc99DbusEvent::Shown { .. }));
        let dbus_id = rig.recorded()[0].assigned_id;

        let _ = handle.close(instance(1));

        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Closed {
                instance: instance(1)
            }
        );
        assert_eq!(rig.closes(), [dbus_id], "the call was attempted once");

        // The mappings were dropped, so a late signal for the id is ignored.
        let _ = handle.show(instance(2), None, &notification());
        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Shown {
                instance: instance(2)
            }
        );
        rig.emit_closed(dbus_id, 1)?;
        rig.emit_closed(rig.recorded()[1].assigned_id, 1)?;
        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Closed {
                instance: instance(2)
            }
        );
        assert_no_queued_events(&mut handle)
    }

    #[test]
    fn a_failed_replacing_show_closes_the_old_notification() -> TestResult {
        let (rig, mut handle, _woken) = started(&["body", "actions"])?;
        let _ = handle.show(instance(1), None, &notification());
        assert!(matches!(next_event(&handle)?, Osc99DbusEvent::Shown { .. }));
        let old_id = rig.recorded()[0].assigned_id;

        let _ = handle.show(
            instance(2),
            Some(instance(1)),
            &Notification99Data {
                title: REJECTED_SUMMARY.to_owned(),
                ..notification()
            },
        );

        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::ShowFailed {
                instance: instance(2)
            }
        );
        // The best-effort close is made before `ShowFailed` is reported.
        assert_eq!(rig.closes(), [old_id]);

        // The old instance is forgotten: closing it calls nothing.
        let _ = handle.close(instance(1));
        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Closed {
                instance: instance(1)
            }
        );
        assert_eq!(rig.closes(), [old_id]);
        Ok(())
    }

    #[test]
    fn the_capability_and_owner_calls_are_sent_without_auto_start() -> TestResult {
        let rig = bus_rig(&["body"], NameOwnerAnswer::Owner(":1.5".to_owned()), None)?;
        let (mut handle, _woken) = spawn_on(&rig, OwnerTracking::Bus);
        assert!(matches!(
            next_event(&handle)?,
            Osc99DbusEvent::Capabilities(_)
        ));

        // `Notify` is the call that is allowed to start a daemon.
        let _ = handle.show(instance(1), None, &notification());
        assert!(matches!(next_event(&handle)?, Osc99DbusEvent::Shown { .. }));

        assert_eq!(
            rig.calls(),
            [
                ("GetNameOwner".to_owned(), Activation::Suppressed),
                ("GetCapabilities".to_owned(), Activation::Suppressed),
                ("Notify".to_owned(), Activation::Allowed),
            ]
        );
        Ok(())
    }

    #[test]
    fn dropping_the_handle_ends_the_dispatcher() -> TestResult {
        let (_rig, handle, _woken) = started(&["body"])?;
        let Osc99DbusHandle { commands, events } = handle;

        drop(commands);

        // The event sender lives on the dispatcher thread (and nowhere else),
        // so the channel disconnects exactly when that thread has returned.
        assert!(matches!(
            events.recv_timeout(EVENT_TIMEOUT),
            Err(RecvTimeoutError::Disconnected)
        ));
        Ok(())
    }

    #[test]
    fn losing_the_connection_closes_every_instance_and_reports_unavailable() -> TestResult {
        let (rig, mut handle, _woken) = started(&["body", "actions"])?;
        for n in [1, 2] {
            let _ = handle.show(instance(n), None, &notification());
            assert_eq!(
                next_event(&handle)?,
                Osc99DbusEvent::Shown {
                    instance: instance(n)
                }
            );
        }

        // The daemon's end of the socket goes away: the client's pump sees the
        // stream end.
        zbus::block_on(rig.daemon.close())?;

        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Closed {
                instance: instance(1)
            }
        );
        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Closed {
                instance: instance(2)
            }
        );
        assert_eq!(next_event(&handle)?, Osc99DbusEvent::Unavailable);

        // From now on the service answers as if the bus were down.
        assert_eq!(
            handle.show(instance(3), None, &notification()),
            Osc99SystemDispatch::Queued
        );
        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::ShowFailed {
                instance: instance(3)
            }
        );
        assert_eq!(
            handle.close(instance(2)),
            Osc99SystemDispatch::Queued,
            "queued; the dispatcher answers Closed itself"
        );
        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Closed {
                instance: instance(2)
            }
        );
        Ok(())
    }

    // ── Owner tracking over the rig (bus mode on a peer-to-peer link) ──

    #[test]
    fn signals_from_anyone_but_the_owner_are_ignored() -> TestResult {
        let rig = bus_rig(
            &["body", "actions"],
            NameOwnerAnswer::Owner(":1.5".to_owned()),
            Some(":1.5"),
        )?;
        let (mut handle, _woken) = spawn_on(&rig, OwnerTracking::Bus);
        assert!(matches!(
            next_event(&handle)?,
            Osc99DbusEvent::Capabilities(_)
        ));
        let _ = handle.show(instance(1), None, &notification());
        assert!(matches!(next_event(&handle)?, Osc99DbusEvent::Shown { .. }));
        let dbus_id = rig.recorded()[0].assigned_id;

        // Forged signals first, then the real one: the real one is the first
        // event, so the forgeries (one pump, in order) were ignored.
        rig.emit_action_from(":1.9", dbus_id, "default")?;
        rig.emit_closed_from(":1.9", dbus_id)?;
        rig.emit_action_from(":1.5", dbus_id, "1")?;

        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Activated {
                instance: instance(1),
                action: Osc99SystemAction::Button(1),
            }
        );
        // The activation closes the notification (138.C23).
        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Closed {
                instance: instance(1)
            }
        );
        assert_no_queued_events(&mut handle)
    }

    #[test]
    fn a_new_owner_closes_only_the_old_daemons_instances_and_refreshes_the_capabilities()
    -> TestResult {
        let rig = bus_rig(
            &["body", "actions"],
            NameOwnerAnswer::Owner(":1.5".to_owned()),
            Some(":1.5"),
        )?;
        let (mut handle, _woken) = spawn_on(&rig, OwnerTracking::Bus);
        assert!(matches!(
            next_event(&handle)?,
            Osc99DbusEvent::Capabilities(_)
        ));
        let _ = handle.show(instance(1), None, &notification());
        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Shown {
                instance: instance(1)
            }
        );

        // The daemon restarts as `:1.6`, and a show is answered by it before
        // the bus driver's `NameOwnerChanged` has been processed.
        rig.reply_as(":1.6");
        rig.advertise(&["body", "sound"]);
        let _ = handle.show(instance(2), None, &notification());
        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Shown {
                instance: instance(2)
            }
        );
        rig.emit_owner_changed(":1.6")?;

        // Only the old daemon's instance is closed, and the new daemon is
        // asked what it can do.
        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Closed {
                instance: instance(1)
            }
        );
        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Capabilities(Osc99DaemonCapabilities {
                body: Osc99DaemonSupport::Supported,
                actions: Osc99DaemonSupport::Unsupported,
                body_markup: Osc99DaemonSupport::Unsupported,
                sound: Osc99DaemonSupport::Supported,
            })
        );

        // The new daemon's instance survived, and its signals are believed.
        let new_id = rig.recorded()[1].assigned_id;
        rig.emit_closed_from(":1.5", new_id)?;
        rig.emit_closed_from(":1.6", new_id)?;
        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Closed {
                instance: instance(2)
            }
        );
        assert_no_queued_events(&mut handle)
    }

    #[test]
    fn a_failed_owner_lookup_adopts_the_notify_senders_name() -> TestResult {
        let rig = bus_rig(&["body", "actions"], NameOwnerAnswer::Broken, Some(":1.7"))?;
        let (mut handle, _woken) = spawn_on(&rig, OwnerTracking::Bus);

        // Nobody is known to own the name, so nobody is asked what it can do.
        assert_no_queued_events(&mut handle)?;
        assert_eq!(
            rig.calls(),
            [("GetNameOwner".to_owned(), Activation::Suppressed)]
        );

        // The reply to the first `Notify` names the daemon; that is also a
        // new owner, so the capabilities are fetched.
        let _ = handle.show(instance(1), None, &notification());
        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Shown {
                instance: instance(1)
            }
        );
        assert!(matches!(
            next_event(&handle)?,
            Osc99DbusEvent::Capabilities(_)
        ));

        // The adopted owner's signals are believed; others' are not.
        let dbus_id = rig.recorded()[0].assigned_id;
        rig.emit_action_from(":1.9", dbus_id, "default")?;
        rig.emit_action_from(":1.7", dbus_id, "default")?;
        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Activated {
                instance: instance(1),
                action: Osc99SystemAction::Default,
            }
        );
        // The activation closes the notification (138.C23).
        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Closed {
                instance: instance(1)
            }
        );
        assert_no_queued_events(&mut handle)
    }

    #[test]
    fn no_daemon_at_start_means_no_capabilities_call_until_one_answers_a_notify() -> TestResult {
        let rig = bus_rig(&["body"], NameOwnerAnswer::Nobody, Some(":1.8"))?;
        let (mut handle, _woken) = spawn_on(&rig, OwnerTracking::Bus);

        assert_no_queued_events(&mut handle)?;
        assert_eq!(
            rig.calls(),
            [("GetNameOwner".to_owned(), Activation::Suppressed)],
            "a missing daemon is not asked, so it cannot be started by asking"
        );

        let _ = handle.show(instance(1), None, &notification());
        assert!(matches!(next_event(&handle)?, Osc99DbusEvent::Shown { .. }));
        assert!(matches!(
            next_event(&handle)?,
            Osc99DbusEvent::Capabilities(_)
        ));
        assert_eq!(
            rig.calls(),
            [
                ("GetNameOwner".to_owned(), Activation::Suppressed),
                ("Notify".to_owned(), Activation::Allowed),
                ("GetCapabilities".to_owned(), Activation::Suppressed),
            ]
        );
        Ok(())
    }

    // ── Reconnecting while unavailable ──────────────────────────────────

    /// How often a test's reconnect timer fires. Short, and nothing depends on
    /// it: the connect closure below waits for the test, not the clock.
    const FAST_RECONNECT: Duration = Duration::from_millis(1);

    /// A connect closure that answers each attempt as the test scripts it
    /// (`None`: the attempt fails), and how many attempts it has seen. An
    /// attempt with nothing scripted waits for the test to script it.
    fn scripted_connect(
        script: Receiver<Option<Link>>,
    ) -> (
        impl FnMut() -> zbus::Result<Link> + Send + 'static,
        Arc<AtomicUsize>,
    ) {
        let attempts = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&attempts);
        (
            move || {
                counter.fetch_add(1, Ordering::SeqCst);
                script
                    .recv_timeout(EVENT_TIMEOUT)
                    .ok()
                    .flatten()
                    .ok_or_else(|| zbus::Error::Failure("no bus".to_owned()))
            },
            attempts,
        )
    }

    fn link_to(rig: &Rig, owner_tracking: OwnerTracking) -> Link {
        Link {
            connection: rig.client.clone(),
            owner_tracking,
        }
    }

    #[test]
    fn a_failed_connect_is_retried_on_the_timer_without_any_command() -> TestResult {
        let rig = rig(&["body", "actions"])?;
        let (scripted, script) = crossbeam_channel::unbounded();
        for attempt in [None, None, Some(link_to(&rig, OwnerTracking::Peer))] {
            scripted.send(attempt)?;
        }
        let (connect, attempts) = scripted_connect(script);
        let (waker, _woken) = counting_waker();
        let mut handle = start(connect, waker, Box::new(NoIconLookup), FAST_RECONNECT);

        // Reported once, for the first failure; the failed retry is silent.
        assert_eq!(next_event(&handle)?, Osc99DbusEvent::Unavailable);
        // The third attempt succeeds: the new session reports the daemon.
        assert!(matches!(
            next_event(&handle)?,
            Osc99DbusEvent::Capabilities(_)
        ));
        assert_eq!(attempts.load(Ordering::SeqCst), 3);

        // And it serves: a show reaches the daemon.
        let _ = handle.show(instance(1), None, &notification());
        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Shown {
                instance: instance(1)
            }
        );
        assert_eq!(rig.recorded().len(), 1);
        assert_eq!(attempts.load(Ordering::SeqCst), 3, "no further attempt");
        Ok(())
    }

    #[test]
    fn commands_while_unavailable_fail_fast_and_do_not_trigger_a_reconnect() -> TestResult {
        let (scripted, script) = crossbeam_channel::unbounded();
        scripted.send(None)?;
        let (connect, attempts) = scripted_connect(script);
        let (waker, _woken) = counting_waker();
        let mut handle = start(
            connect,
            waker,
            Box::new(NoIconLookup),
            Duration::from_secs(3600),
        );
        assert_eq!(next_event(&handle)?, Osc99DbusEvent::Unavailable);

        let _ = handle.show(instance(1), None, &notification());
        let _ = handle.close(instance(2));
        let _ = handle.show(instance(3), None, &notification());

        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::ShowFailed {
                instance: instance(1)
            }
        );
        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Closed {
                instance: instance(2)
            }
        );
        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::ShowFailed {
                instance: instance(3)
            }
        );
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        Ok(())
    }

    #[test]
    fn lost_signals_are_followed_by_a_reconnect_on_a_new_connection() -> TestResult {
        let first = rig(&["body", "actions"])?;
        let second = rig(&["body", "sound"])?;
        let (scripted, script) = crossbeam_channel::unbounded();
        scripted.send(Some(link_to(&first, OwnerTracking::Peer)))?;
        let (connect, attempts) = scripted_connect(script);
        let (waker, _woken) = counting_waker();
        let mut handle = start(connect, waker, Box::new(NoIconLookup), FAST_RECONNECT);
        assert!(matches!(
            next_event(&handle)?,
            Osc99DbusEvent::Capabilities(_)
        ));
        let _ = handle.show(instance(1), None, &notification());
        assert!(matches!(next_event(&handle)?, Osc99DbusEvent::Shown { .. }));

        zbus::block_on(first.daemon.clone().close())?;

        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Closed {
                instance: instance(1)
            }
        );
        assert_eq!(next_event(&handle)?, Osc99DbusEvent::Unavailable);

        // The next attempt finds a daemon on a new connection.
        scripted.send(Some(link_to(&second, OwnerTracking::Peer)))?;
        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Capabilities(Osc99DaemonCapabilities {
                body: Osc99DaemonSupport::Supported,
                actions: Osc99DaemonSupport::Unsupported,
                body_markup: Osc99DaemonSupport::Unsupported,
                sound: Osc99DaemonSupport::Supported,
            })
        );
        let _ = handle.show(instance(2), None, &notification());
        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Shown {
                instance: instance(2)
            }
        );
        assert_eq!(first.recorded().len(), 1, "the old connection is not used");
        assert_eq!(second.recorded().len(), 1);
        assert!(attempts.load(Ordering::SeqCst) >= 2);
        Ok(())
    }

    #[test]
    fn a_reconnect_that_finds_no_daemon_owner_reports_the_assumed_capabilities() -> TestResult {
        let rig = bus_rig(&["body"], NameOwnerAnswer::Nobody, None)?;
        let (scripted, script) = crossbeam_channel::unbounded();
        scripted.send(None)?;
        scripted.send(Some(link_to(&rig, OwnerTracking::Bus)))?;
        let (connect, _attempts) = scripted_connect(script);
        let (waker, _woken) = counting_waker();
        let handle = start(connect, waker, Box::new(NoIconLookup), FAST_RECONNECT);

        assert_eq!(next_event(&handle)?, Osc99DbusEvent::Unavailable);

        // Nobody can be asked what it can do, but the GUI must still see the
        // service as available again.
        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Capabilities(Osc99DaemonCapabilities::default())
        );
        assert_eq!(
            rig.calls(),
            [("GetNameOwner".to_owned(), Activation::Suppressed)],
            "a missing daemon is still not asked"
        );
        Ok(())
    }

    #[test]
    fn an_unreachable_bus_reports_unavailable_once_and_fails_every_show() -> TestResult {
        let (waker, woken) = counting_waker();
        let mut handle = start(
            || Err(zbus::Error::Failure("no bus".to_owned())),
            waker,
            Box::new(NoIconLookup),
            Duration::from_secs(3600),
        );

        assert_eq!(next_event(&handle)?, Osc99DbusEvent::Unavailable);

        assert_eq!(
            handle.show(instance(1), None, &notification()),
            Osc99SystemDispatch::Queued,
            "queued; the dispatcher answers ShowFailed itself"
        );
        assert_eq!(
            handle.show(instance(2), None, &notification()),
            Osc99SystemDispatch::Queued
        );
        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::ShowFailed {
                instance: instance(1)
            }
        );
        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::ShowFailed {
                instance: instance(2)
            }
        );
        assert_eq!(
            handle.close(instance(3)),
            Osc99SystemDispatch::Queued,
            "queued; the dispatcher answers Closed itself"
        );
        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Closed {
                instance: instance(3)
            }
        );
        for _ in 0..4 {
            woken.recv_timeout(EVENT_TIMEOUT)?;
        }
        Ok(())
    }

    #[test]
    fn a_request_after_the_dispatcher_is_gone_returns_failed_and_queues_no_event() -> TestResult {
        let (waker, _woken) = counting_waker();
        let mut handle = start(
            || Err(zbus::Error::Failure("no bus".to_owned())),
            waker,
            Box::new(NoIconLookup),
            Duration::from_secs(3600),
        );
        assert_eq!(next_event(&handle)?, Osc99DbusEvent::Unavailable);

        // Replace the command sender with one whose receiver is already gone,
        // as after a dispatcher exit.
        let (dead_tx, dead_rx) = crossbeam_channel::unbounded();
        drop(dead_rx);
        handle.commands = dead_tx;

        assert_eq!(
            handle.show(instance(5), None, &notification()),
            Osc99SystemDispatch::Failed
        );
        assert_eq!(
            handle.try_events().next(),
            None,
            "no local event: the caller ends the leg"
        );
        assert_eq!(
            handle.close(instance(5)),
            Osc99SystemDispatch::Failed,
            "a close fails the same way"
        );
        assert_eq!(handle.try_events().next(), None);
        Ok(())
    }

    #[test]
    fn a_request_to_a_full_queue_returns_failed_without_blocking() -> TestResult {
        let (waker, _woken) = counting_waker();
        let mut handle = start(
            || Err(zbus::Error::Failure("no bus".to_owned())),
            waker,
            Box::new(NoIconLookup),
            Duration::from_secs(3600),
        );
        assert_eq!(next_event(&handle)?, Osc99DbusEvent::Unavailable);

        // A queue nobody drains, filled to its capacity, as when the
        // dispatcher is stuck on a hung daemon.
        let (full_tx, _full_rx) = crossbeam_channel::bounded(COMMAND_QUEUE_CAPACITY);
        for n in 0..COMMAND_QUEUE_CAPACITY {
            full_tx.try_send(DbusCommand::Close {
                instance: instance(u64::try_from(n)?),
            })?;
        }
        handle.commands = full_tx;

        assert_eq!(
            handle.show(instance(9_000), None, &notification()),
            Osc99SystemDispatch::Failed
        );
        assert_eq!(handle.close(instance(9_000)), Osc99SystemDispatch::Failed);
        assert_eq!(handle.try_events().next(), None, "the caller ends the leg");
        Ok(())
    }
}
