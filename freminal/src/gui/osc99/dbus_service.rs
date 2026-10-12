// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! The Linux/BSD OSC 99 desktop-notification service (Task 138.12).
//!
//! One *dispatcher* thread (`freminal-osc99-dbus`) owns one D-Bus connection
//! and calls `org.freedesktop.Notifications` on it (`GetCapabilities` once at
//! start, `Notify` per show). One *signal pump* thread
//! (`freminal-osc99-signals`) iterates a single match rule over that
//! connection and forwards `ActionInvoked` / `NotificationClosed` **into the
//! dispatcher's input channel**, not to the GUI. The dispatcher owns the
//! D-Bus-id to instance map, so a signal that arrives while a `Notify` call is
//! in flight is only processed after the call has recorded its id. That
//! removes the ordering race between the two threads.
//!
//! The GUI talks to the service through an [`Osc99DbusHandle`]: commands go in
//! over one crossbeam channel and [`Osc99DbusEvent`]s come back over another,
//! each event followed by a repaint wake so an idle window still drains it.
//! The dispatcher exits when the handle (and with it the command sender) is
//! dropped; the pump exits on its next send after that.
//!
//! Signals are not filtered by sender: any session-bus peer can forge
//! `ActionInvoked` / `NotificationClosed` for a known notification id. This is
//! recorded follow-up 138.C2 and is deliberately not implemented here.
//!
//! Every state transition lives in the pure [`DispatcherState`], which has no
//! bus and no threads and is unit-tested directly.
//!
//! Presentation (icons, sounds, category, markup) is Task 138.14; update and
//! close through the service are Task 138.13.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use crossbeam_channel::{Receiver, Sender, select};
use freminal_common::buffer_states::osc_notify_99::NotificationUrgency;
use freminal_common::buffer_states::osc99_notification::Osc99Expiry;
use freminal_common::buffer_states::window_manipulation::Notification99Data;
use freminal_common::send_or_log;
use freminal_windowing::{RepaintProxy, WindowId};
use zbus::MatchRule;
use zbus::blocking::{Connection, MessageIterator};
use zbus::message::{Message, Type as MessageType};
use zbus::zvariant::Value;

use super::{Osc99InstanceId, Osc99LegActivation, Osc99ShowDispatch, Osc99SystemBackend};

/// The well-known bus name of the notification daemon.
const NOTIFICATIONS_SERVICE: &str = "org.freedesktop.Notifications";
/// The object path the notification daemon serves.
const NOTIFICATIONS_PATH: &str = "/org/freedesktop/Notifications";
/// The notification interface.
const NOTIFICATIONS_INTERFACE: &str = "org.freedesktop.Notifications";
/// The `app_name` sent when the notification names no application (`f=`).
const DEFAULT_APP_NAME: &str = "freminal";
/// The most button actions sent: OSC 99 keeps at most eight buttons, and
/// action keys `"1"`-`"8"` are the only ones mapped back.
const MAX_BUTTON_ACTIONS: usize = 8;

// ── Public vocabulary ──────────────────────────────────────────────────────

/// Whether the notification daemon has a capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Osc99DaemonSupport {
    /// The daemon advertises the capability (or it is assumed, before the
    /// daemon has answered).
    Supported,
    /// The daemon does not advertise the capability.
    Unsupported,
}

/// The `GetCapabilities` entries the OSC 99 system leg cares about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Osc99DaemonCapabilities {
    /// `body`: the daemon shows a notification body.
    pub body: Osc99DaemonSupport,
    /// `actions`: the daemon shows and reports actions (buttons and the
    /// default action).
    pub actions: Osc99DaemonSupport,
    /// `body-markup`: the daemon renders markup in the body.
    pub body_markup: Osc99DaemonSupport,
    /// `sound`: the daemon plays notification sounds.
    pub sound: Osc99DaemonSupport,
}

impl Default for Osc99DaemonCapabilities {
    /// What is assumed until the daemon answers `GetCapabilities`: kitty
    /// assumes `body`, `actions` and `body-markup`, and no `sound`.
    fn default() -> Self {
        Self {
            body: Osc99DaemonSupport::Supported,
            actions: Osc99DaemonSupport::Supported,
            body_markup: Osc99DaemonSupport::Supported,
            sound: Osc99DaemonSupport::Unsupported,
        }
    }
}

impl Osc99DaemonCapabilities {
    /// The capabilities a daemon advertised in its `GetCapabilities` reply.
    fn from_advertised(advertised: &[String]) -> Self {
        let support = |name: &str| {
            if advertised.iter().any(|capability| capability == name) {
                Osc99DaemonSupport::Supported
            } else {
                Osc99DaemonSupport::Unsupported
            }
        };
        Self {
            body: support("body"),
            actions: support("actions"),
            body_markup: support("body-markup"),
            sound: support("sound"),
        }
    }
}

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

// ── Repaint wake ───────────────────────────────────────────────────────────

/// Where the service's repaint wake-ups go.
///
/// The service runs on its own threads and must wake the GUI so an idle
/// window drains the events it just queued. Every window's `update()` drains
/// the one shared event channel, so any live window will do: the GUI
/// re-points this at the window it is rendering each frame
/// ([`Self::retarget`]), which keeps the target a window that exists. If that
/// window closes before another frame retargets, a wake is simply ignored and
/// the events wait for the next frame of any window.
#[derive(Clone, Default)]
pub struct Osc99Wake {
    shared: Arc<WakeShared>,
}

/// The state [`Osc99Wake`] shares with the service's waker.
#[derive(Default)]
struct WakeShared {
    /// The event-loop proxy, set once by the first frame.
    proxy: OnceLock<RepaintProxy>,
    /// The window to wake: the one that rendered most recently.
    window: Mutex<Option<WindowId>>,
}

impl Osc99Wake {
    /// Point the wake at `window`. `proxy` is only called the first time, to
    /// obtain the cross-thread repaint handle.
    pub fn retarget(&self, window: WindowId, proxy: impl FnOnce() -> RepaintProxy) {
        self.shared.proxy.get_or_init(proxy);
        *self
            .shared
            .window
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(window);
    }

    /// A waker for [`Osc99DbusService::spawn`].
    pub fn waker(&self) -> Box<dyn Fn() + Send> {
        let shared = Arc::clone(&self.shared);
        Box::new(move || shared.wake())
    }
}

impl WakeShared {
    /// Ask the event loop to repaint the target window, if there is one.
    fn wake(&self) {
        let window = *self.window.lock().unwrap_or_else(PoisonError::into_inner);
        if let (Some(proxy), Some(window)) = (self.proxy.get(), window) {
            proxy.request_repaint(window);
        }
    }
}

// ── Pure dispatcher state ──────────────────────────────────────────────────

/// The `urgency` hint's value (`0` low, `1` normal, `2` critical).
const fn urgency_byte(urgency: NotificationUrgency) -> u8 {
    match urgency {
        NotificationUrgency::Low => 0,
        NotificationUrgency::Normal => 1,
        NotificationUrgency::Critical => 2,
    }
}

/// The `hints` argument of `Notify`. Task 138.14 adds the presentation hints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NotifyHints {
    /// The `urgency` hint.
    urgency: u8,
}

impl NotifyHints {
    /// The wire form: an `a{sv}` dictionary.
    fn to_map(self) -> HashMap<&'static str, Value<'static>> {
        HashMap::from([("urgency", Value::from(self.urgency))])
    }
}

/// The arguments of one `Notify` call, in the order the method takes them.
#[derive(Debug, Clone, PartialEq, Eq)]
struct NotifyArgs {
    app_name: String,
    replaces_id: u32,
    app_icon: String,
    summary: String,
    body: String,
    actions: Vec<String>,
    hints: NotifyHints,
    expire_timeout: i32,
}

/// A signal the pump forwards to the dispatcher.
#[derive(Debug, Clone, PartialEq, Eq)]
enum PumpSignal {
    /// `ActionInvoked(id, action_key)`.
    ActionInvoked { dbus_id: u32, action_key: String },
    /// `NotificationClosed(id, reason)`. The reason does not matter to the
    /// protocol, so it is not carried.
    NotificationClosed { dbus_id: u32 },
}

/// The dispatcher's bookkeeping, free of any bus or thread.
#[derive(Debug, Default)]
struct DispatcherState {
    /// The daemon's id for each displayed instance.
    by_instance: HashMap<Osc99InstanceId, u32>,
    /// The instance behind each of the daemon's ids.
    by_dbus_id: HashMap<u32, Osc99InstanceId>,
    /// What the daemon can do; assumed until it answers.
    capabilities: Osc99DaemonCapabilities,
}

impl DispatcherState {
    /// Record the daemon's `GetCapabilities` answer.
    fn set_capabilities(&mut self, advertised: &[String]) {
        self.capabilities = Osc99DaemonCapabilities::from_advertised(advertised);
    }

    /// Build the `Notify` arguments for `notification`.
    ///
    /// `replaces_id` is the replaced instance's D-Bus id, or `0` when there is
    /// none (or it is not known). The `actions` array is the `default` action
    /// followed by one 1-based action per button, and is sent only when the
    /// daemon has the `actions` capability.
    fn notify_args(
        &self,
        replaces: Option<Osc99InstanceId>,
        notification: &Notification99Data,
    ) -> NotifyArgs {
        let actions = match self.capabilities.actions {
            Osc99DaemonSupport::Supported => {
                let mut actions = vec!["default".to_owned(), String::new()];
                for (index, label) in notification
                    .buttons
                    .iter()
                    .take(MAX_BUTTON_ACTIONS)
                    .enumerate()
                {
                    actions.push((index + 1).to_string());
                    actions.push(label.clone());
                }
                actions
            }
            Osc99DaemonSupport::Unsupported => Vec::new(),
        };
        let expire_timeout = match notification.expiry {
            Osc99Expiry::OsDefault => -1,
            Osc99Expiry::Never => 0,
            Osc99Expiry::After(ms) => i32::try_from(ms.get()).unwrap_or(i32::MAX),
        };
        NotifyArgs {
            app_name: notification
                .app_name
                .clone()
                .unwrap_or_else(|| DEFAULT_APP_NAME.to_owned()),
            replaces_id: replaces
                .and_then(|old| self.by_instance.get(&old).copied())
                .unwrap_or(0),
            // Task 138.14 resolves icons.
            app_icon: String::new(),
            summary: notification.title.clone(),
            body: notification.body.clone().unwrap_or_default(),
            actions,
            hints: NotifyHints {
                urgency: urgency_byte(notification.urgency),
            },
            expire_timeout,
        }
    }

    /// Record that the daemon displayed `instance` as `dbus_id`.
    ///
    /// When `replaces` is given the replaced instance's mappings are dropped
    /// first, so the replaced notification's later `NotificationClosed` is
    /// ignored (and a daemon that reuses the id for the update still maps it
    /// to the new instance).
    fn record_shown(
        &mut self,
        instance: Osc99InstanceId,
        replaces: Option<Osc99InstanceId>,
        dbus_id: u32,
    ) {
        if let Some(old) = replaces {
            self.forget_instance(old);
        }
        self.by_instance.insert(instance, dbus_id);
        self.by_dbus_id.insert(dbus_id, instance);
    }

    /// Drop `instance`'s mappings, if it has any.
    fn forget_instance(&mut self, instance: Osc99InstanceId) {
        if let Some(dbus_id) = self.by_instance.remove(&instance) {
            self.by_dbus_id.remove(&dbus_id);
        }
    }

    /// The event for an `ActionInvoked` signal. The mappings stay: a
    /// `NotificationClosed` normally follows. `None` for an id with no live
    /// instance or an action freminal never registered.
    fn on_action(&self, dbus_id: u32, action_key: &str) -> Option<Osc99DbusEvent> {
        let instance = *self.by_dbus_id.get(&dbus_id)?;
        let action = Osc99SystemAction::from_key(action_key)?;
        Some(Osc99DbusEvent::Activated { instance, action })
    }

    /// The event for a `NotificationClosed` signal, dropping the mappings.
    /// `None` for an id with no live instance (this covers replaced
    /// notifications).
    fn on_closed(&mut self, dbus_id: u32) -> Option<Osc99DbusEvent> {
        let instance = self.by_dbus_id.remove(&dbus_id)?;
        self.by_instance.remove(&instance);
        Some(Osc99DbusEvent::Closed { instance })
    }

    /// The event, if any, a pump signal stands for.
    fn on_signal(&mut self, signal: &PumpSignal) -> Option<Osc99DbusEvent> {
        match signal {
            PumpSignal::ActionInvoked {
                dbus_id,
                action_key,
            } => self.on_action(*dbus_id, action_key),
            PumpSignal::NotificationClosed { dbus_id } => self.on_closed(*dbus_id),
        }
    }
}

// ── Handle ─────────────────────────────────────────────────────────────────

/// A request from the GUI to the dispatcher. Task 138.13 adds `Close`.
#[derive(Debug)]
enum DbusCommand {
    /// Display `notification` for `instance`, replacing `replaces`'.
    Show {
        instance: Osc99InstanceId,
        replaces: Option<Osc99InstanceId>,
        notification: Box<Notification99Data>,
    },
}

/// The GUI's end of the service.
///
/// Dropping it stops the dispatcher (and, on its next send, the pump).
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

impl Osc99SystemBackend for Osc99DbusHandle {
    fn show(
        &mut self,
        instance: Osc99InstanceId,
        replaces: Option<Osc99InstanceId>,
        notification: &Notification99Data,
    ) -> Osc99ShowDispatch {
        let command = DbusCommand::Show {
            instance,
            replaces,
            notification: Box::new(notification.clone()),
        };
        match self.commands.send(command) {
            Ok(()) => Osc99ShowDispatch::Queued,
            Err(e) => {
                // No event will follow: the caller ends the leg itself.
                tracing::warn!("OSC 99: notification service is gone; show failed: {e}");
                Osc99ShowDispatch::Failed
            }
        }
    }
}

// ── Service ────────────────────────────────────────────────────────────────

/// Why the service could not start.
#[derive(Debug, thiserror::Error)]
enum StartError {
    /// The bus connection or the signal subscription failed.
    #[error("session bus unavailable: {0}")]
    Bus(#[from] zbus::Error),
    /// The signal pump thread could not be spawned.
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
    /// once, a single warning is logged, and every later show answers
    /// [`Osc99DbusEvent::ShowFailed`].
    #[must_use]
    pub fn spawn(waker: Box<dyn Fn() + Send>) -> Osc99DbusHandle {
        start(Connection::session, waker)
    }

    /// Start the service on an existing connection (a peer-to-peer one, in
    /// tests).
    #[cfg(test)]
    fn spawn_with_connection(
        connection: Connection,
        waker: Box<dyn Fn() + Send>,
    ) -> Osc99DbusHandle {
        start(move || Ok(connection), waker)
    }
}

/// Start the dispatcher thread, which obtains its connection from `connect`.
fn start<C>(connect: C, waker: Box<dyn Fn() + Send>) -> Osc99DbusHandle
where
    C: FnOnce() -> zbus::Result<Connection> + Send + 'static,
{
    let (command_tx, command_rx) = crossbeam_channel::unbounded();
    let (event_tx, event_rx) = crossbeam_channel::unbounded();
    let sink = EventSink {
        events: event_tx.clone(),
        waker,
    };

    let spawned = std::thread::Builder::new()
        .name("freminal-osc99-dbus".to_owned())
        .spawn(move || run_dispatcher(connect, &command_rx, &sink));
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

/// The dispatcher thread: connect, then serve commands until the GUI drops its
/// command sender.
fn run_dispatcher<C>(connect: C, commands: &Receiver<DbusCommand>, sink: &EventSink)
where
    C: FnOnce() -> zbus::Result<Connection>,
{
    match connect_and_subscribe(connect) {
        Ok((connection, signals)) => serve(&connection, commands, signals, sink),
        Err(e) => {
            tracing::warn!("OSC 99: system notifications are unavailable: {e}");
            sink.emit(Osc99DbusEvent::Unavailable);
            fail_every_show(commands, sink);
        }
    }
}

/// Connect, subscribe to the notification signals, and start the pump.
///
/// The subscription is made here, before the pump thread starts and before the
/// first `Notify`, so no signal can be missed.
fn connect_and_subscribe<C>(connect: C) -> Result<(Connection, Receiver<PumpSignal>), StartError>
where
    C: FnOnce() -> zbus::Result<Connection>,
{
    let connection = connect()?;
    let rule = MatchRule::builder()
        .msg_type(MessageType::Signal)
        .interface(NOTIFICATIONS_INTERFACE)?
        .path(NOTIFICATIONS_PATH)?
        .build();
    let messages = MessageIterator::for_match_rule(rule, &connection, None)?;

    let (signal_tx, signal_rx) = crossbeam_channel::unbounded();
    std::thread::Builder::new()
        .name("freminal-osc99-signals".to_owned())
        .spawn(move || run_pump(messages, &signal_tx))
        .map_err(StartError::PumpThread)?;
    Ok((connection, signal_rx))
}

/// Answer every show with `ShowFailed` until the GUI drops its sender.
fn fail_every_show(commands: &Receiver<DbusCommand>, sink: &EventSink) {
    for command in commands {
        let DbusCommand::Show { instance, .. } = command;
        sink.emit(Osc99DbusEvent::ShowFailed { instance });
    }
}

/// Serve `commands` and the pump's `signals` until the GUI drops its command
/// sender.
fn serve(
    connection: &Connection,
    commands: &Receiver<DbusCommand>,
    signals: Receiver<PumpSignal>,
    sink: &EventSink,
) {
    let mut state = DispatcherState::default();
    match fetch_capabilities(connection) {
        Ok(advertised) => {
            state.set_capabilities(&advertised);
            sink.emit(Osc99DbusEvent::Capabilities(state.capabilities));
        }
        Err(e) => tracing::warn!("OSC 99: GetCapabilities failed; assuming defaults: {e}"),
    }

    // A pump that ended leaves a receiver that is ready forever; once that is
    // seen the arm is parked on a channel that never delivers.
    let parked = crossbeam_channel::never::<PumpSignal>();
    let mut pump = Some(signals);
    loop {
        let signal_rx = pump.as_ref().unwrap_or(&parked);
        let pump_ended = select! {
            recv(commands) -> command => match command {
                Ok(command) => {
                    handle_command(connection, &mut state, sink, command);
                    false
                }
                Err(_) => return,
            },
            recv(signal_rx) -> signal => signal.map_or(true, |signal| {
                if let Some(event) = state.on_signal(&signal) {
                    sink.emit(event);
                }
                false
            }),
        };
        if pump_ended {
            tracing::warn!("OSC 99: the notification signal pump ended; no more signals");
            pump = None;
        }
    }
}

/// Carry out one GUI command.
fn handle_command(
    connection: &Connection,
    state: &mut DispatcherState,
    sink: &EventSink,
    command: DbusCommand,
) {
    let DbusCommand::Show {
        instance,
        replaces,
        notification,
    } = command;
    let args = state.notify_args(replaces, &notification);
    match notify(connection, &args) {
        Ok(dbus_id) => {
            state.record_shown(instance, replaces, dbus_id);
            sink.emit(Osc99DbusEvent::Shown { instance });
        }
        Err(e) => {
            tracing::warn!("OSC 99: Notify failed: {e}");
            if let Some(old) = replaces {
                // The registry already forgot the replaced instance, so its
                // signals are of no interest either.
                state.forget_instance(old);
            }
            sink.emit(Osc99DbusEvent::ShowFailed { instance });
        }
    }
}

/// Call `GetCapabilities`.
fn fetch_capabilities(connection: &Connection) -> zbus::Result<Vec<String>> {
    let reply = connection.call_method(
        Some(NOTIFICATIONS_SERVICE),
        NOTIFICATIONS_PATH,
        Some(NOTIFICATIONS_INTERFACE),
        "GetCapabilities",
        &(),
    )?;
    reply.body().deserialize::<Vec<String>>()
}

/// Call `Notify`, returning the daemon's id for the notification.
fn notify(connection: &Connection, args: &NotifyArgs) -> zbus::Result<u32> {
    let reply = connection.call_method(
        Some(NOTIFICATIONS_SERVICE),
        NOTIFICATIONS_PATH,
        Some(NOTIFICATIONS_INTERFACE),
        "Notify",
        &(
            args.app_name.as_str(),
            args.replaces_id,
            args.app_icon.as_str(),
            args.summary.as_str(),
            args.body.as_str(),
            args.actions.as_slice(),
            args.hints.to_map(),
            args.expire_timeout,
        ),
    )?;
    reply.body().deserialize::<u32>()
}

// ── Signal pump ────────────────────────────────────────────────────────────

/// The pump thread: forward the notification signals to the dispatcher until
/// the bus closes or the dispatcher is gone.
fn run_pump(messages: MessageIterator, signals: &Sender<PumpSignal>) {
    for message in messages {
        let message = match message {
            Ok(message) => message,
            Err(e) => {
                tracing::warn!("OSC 99: notification signal stream failed: {e}");
                return;
            }
        };
        if let Some(signal) = parse_signal(&message)
            && signals.send(signal).is_err()
        {
            // The dispatcher dropped its receiver: the GUI is gone.
            return;
        }
    }
}

/// The signal `message` carries, or `None` for any member other than
/// `ActionInvoked` and `NotificationClosed` and for a malformed body.
fn parse_signal(message: &Message) -> Option<PumpSignal> {
    let header = message.header();
    let member = header.member()?;
    let body = message.body();
    match member.as_str() {
        "ActionInvoked" => {
            let (dbus_id, action_key) = body.deserialize::<(u32, String)>().ok()?;
            Some(PumpSignal::ActionInvoked {
                dbus_id,
                action_key,
            })
        }
        "NotificationClosed" => {
            let (dbus_id, _reason) = body.deserialize::<(u32, u32)>().ok()?;
            Some(PumpSignal::NotificationClosed { dbus_id })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::num::NonZeroU32;
    use std::os::unix::net::UnixStream;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use freminal_common::buffer_states::osc_notify_99::NotificationOccasion;
    use freminal_common::buffer_states::osc99_notification::{
        Osc99ActivationFocus, Osc99ActivationReporting, Osc99CloseReporting, Osc99Sound,
    };
    use zbus::zvariant::OwnedValue;

    use super::*;

    /// How long a test waits for something that must happen. Generous so a
    /// loaded CI machine never trips it; a healthy run never waits it out.
    const EVENT_TIMEOUT: Duration = Duration::from_secs(20);

    type TestResult = Result<(), Box<dyn Error>>;

    fn instance(n: u64) -> Osc99InstanceId {
        Osc99InstanceId(n)
    }

    fn notification() -> Notification99Data {
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

    fn with_buttons(labels: &[&str]) -> Notification99Data {
        Notification99Data {
            buttons: labels.iter().map(|label| (*label).to_owned()).collect(),
            ..notification()
        }
    }

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_owned()).collect()
    }

    // ── DispatcherState ─────────────────────────────────────────────────

    #[test]
    fn notify_args_carry_title_body_app_name_and_urgency() {
        let state = DispatcherState::default();
        let args = state.notify_args(
            None,
            &Notification99Data {
                body: Some("Body".to_owned()),
                app_name: Some("my-app".to_owned()),
                urgency: NotificationUrgency::Critical,
                ..notification()
            },
        );

        assert_eq!(args.app_name, "my-app");
        assert_eq!(args.summary, "Title");
        assert_eq!(args.body, "Body");
        assert_eq!(args.app_icon, "");
        assert_eq!(args.hints, NotifyHints { urgency: 2 });
    }

    #[test]
    fn notify_args_default_the_app_name_and_body() {
        let args = DispatcherState::default().notify_args(None, &notification());

        assert_eq!(args.app_name, "freminal");
        assert_eq!(args.body, "");
    }

    #[test]
    fn notify_args_map_each_urgency_to_its_byte() {
        let state = DispatcherState::default();
        for (urgency, byte) in [
            (NotificationUrgency::Low, 0),
            (NotificationUrgency::Normal, 1),
            (NotificationUrgency::Critical, 2),
        ] {
            let args = state.notify_args(
                None,
                &Notification99Data {
                    urgency,
                    ..notification()
                },
            );
            assert_eq!(args.hints.urgency, byte, "{urgency:?}");
        }
    }

    #[test]
    fn notify_args_send_the_default_action_and_one_based_buttons() {
        let args = DispatcherState::default().notify_args(None, &with_buttons(&["A", "B"]));

        assert_eq!(args.actions, ["default", "", "1", "A", "2", "B"]);
    }

    #[test]
    fn notify_args_send_only_the_default_action_without_buttons() {
        let args = DispatcherState::default().notify_args(None, &notification());

        assert_eq!(args.actions, ["default", ""]);
    }

    #[test]
    fn notify_args_send_no_actions_when_the_daemon_lacks_the_capability() {
        let mut state = DispatcherState::default();
        state.set_capabilities(&strings(&["body"]));

        let args = state.notify_args(None, &with_buttons(&["A", "B"]));

        assert_eq!(args.actions, Vec::<String>::new());
    }

    #[test]
    fn notify_args_keep_at_most_eight_buttons() {
        let labels: Vec<String> = (0..12).map(|n| format!("b{n}")).collect();
        let args = DispatcherState::default().notify_args(
            None,
            &Notification99Data {
                buttons: labels,
                ..notification()
            },
        );

        assert_eq!(args.actions.len(), 2 + 8 * 2);
        assert_eq!(args.actions[args.actions.len() - 2], "8");
    }

    #[test]
    fn notify_args_map_expiry_to_the_timeout() -> TestResult {
        let state = DispatcherState::default();
        let timeout = |expiry| {
            state
                .notify_args(
                    None,
                    &Notification99Data {
                        expiry,
                        ..notification()
                    },
                )
                .expire_timeout
        };

        assert_eq!(timeout(Osc99Expiry::OsDefault), -1);
        assert_eq!(timeout(Osc99Expiry::Never), 0);
        assert_eq!(
            timeout(Osc99Expiry::After(NonZeroU32::new(5000).ok_or("zero")?)),
            5000
        );
        assert_eq!(
            timeout(Osc99Expiry::After(NonZeroU32::MAX)),
            i32::MAX,
            "a duration beyond i32 clamps"
        );
        Ok(())
    }

    #[test]
    fn notify_args_replace_id_is_the_replaced_instances_dbus_id() {
        let mut state = DispatcherState::default();
        state.record_shown(instance(1), None, 42);

        assert_eq!(
            state
                .notify_args(Some(instance(1)), &notification())
                .replaces_id,
            42
        );
        assert_eq!(
            state.notify_args(None, &notification()).replaces_id,
            0,
            "no replacement"
        );
        assert_eq!(
            state
                .notify_args(Some(instance(9)), &notification())
                .replaces_id,
            0,
            "an instance with no D-Bus id"
        );
    }

    #[test]
    fn capabilities_default_to_what_kitty_assumes() {
        let state = DispatcherState::default();

        assert_eq!(
            state.capabilities,
            Osc99DaemonCapabilities {
                body: Osc99DaemonSupport::Supported,
                actions: Osc99DaemonSupport::Supported,
                body_markup: Osc99DaemonSupport::Supported,
                sound: Osc99DaemonSupport::Unsupported,
            }
        );
    }

    #[test]
    fn capabilities_follow_the_advertised_list() {
        let mut state = DispatcherState::default();

        state.set_capabilities(&strings(&["body", "sound", "persistence"]));

        assert_eq!(
            state.capabilities,
            Osc99DaemonCapabilities {
                body: Osc99DaemonSupport::Supported,
                actions: Osc99DaemonSupport::Unsupported,
                body_markup: Osc99DaemonSupport::Unsupported,
                sound: Osc99DaemonSupport::Supported,
            }
        );
    }

    #[test]
    fn an_action_for_an_unknown_id_is_ignored() {
        let state = DispatcherState::default();

        assert_eq!(state.on_action(7, "default"), None);
    }

    #[test]
    fn the_default_action_activates_the_instance() {
        let mut state = DispatcherState::default();
        state.record_shown(instance(1), None, 7);

        assert_eq!(
            state.on_action(7, "default"),
            Some(Osc99DbusEvent::Activated {
                instance: instance(1),
                action: Osc99SystemAction::Default,
            })
        );
    }

    #[test]
    fn button_keys_one_to_eight_map_to_their_button() {
        let mut state = DispatcherState::default();
        state.record_shown(instance(1), None, 7);

        for n in 1..=8u8 {
            assert_eq!(
                state.on_action(7, &n.to_string()),
                Some(Osc99DbusEvent::Activated {
                    instance: instance(1),
                    action: Osc99SystemAction::Button(n),
                })
            );
        }
    }

    #[test]
    fn other_action_keys_are_ignored() {
        let mut state = DispatcherState::default();
        state.record_shown(instance(1), None, 7);

        for key in ["0", "9", "x", "", "10", "01", "+1", "Default", "default "] {
            assert_eq!(state.on_action(7, key), None, "key {key:?}");
        }
    }

    #[test]
    fn a_close_after_an_activation_maps_both() {
        let mut state = DispatcherState::default();
        state.record_shown(instance(1), None, 7);

        assert!(matches!(
            state.on_action(7, "2"),
            Some(Osc99DbusEvent::Activated { .. })
        ));
        assert_eq!(
            state.on_closed(7),
            Some(Osc99DbusEvent::Closed {
                instance: instance(1)
            })
        );
        assert_eq!(state.on_closed(7), None, "the close dropped the mapping");
        assert_eq!(state.on_action(7, "default"), None);
    }

    #[test]
    fn a_close_for_an_unknown_id_is_ignored() {
        let mut state = DispatcherState::default();

        assert_eq!(state.on_closed(7), None);
    }

    #[test]
    fn replacing_drops_the_old_mapping_so_its_close_is_ignored() {
        let mut state = DispatcherState::default();
        state.record_shown(instance(1), None, 7);

        state.record_shown(instance(2), Some(instance(1)), 8);

        assert_eq!(state.on_closed(7), None, "the replaced id is forgotten");
        assert_eq!(
            state.on_closed(8),
            Some(Osc99DbusEvent::Closed {
                instance: instance(2)
            })
        );
    }

    #[test]
    fn replacing_with_a_reused_dbus_id_maps_it_to_the_new_instance() {
        let mut state = DispatcherState::default();
        state.record_shown(instance(1), None, 7);

        state.record_shown(instance(2), Some(instance(1)), 7);

        assert_eq!(
            state.on_closed(7),
            Some(Osc99DbusEvent::Closed {
                instance: instance(2)
            })
        );
        assert!(state.by_instance.is_empty());
        assert!(state.by_dbus_id.is_empty());
    }

    #[test]
    fn signals_map_through_on_signal() {
        let mut state = DispatcherState::default();
        state.record_shown(instance(1), None, 7);

        assert_eq!(
            state.on_signal(&PumpSignal::ActionInvoked {
                dbus_id: 7,
                action_key: "1".to_owned(),
            }),
            Some(Osc99DbusEvent::Activated {
                instance: instance(1),
                action: Osc99SystemAction::Button(1),
            })
        );
        assert_eq!(
            state.on_signal(&PumpSignal::NotificationClosed { dbus_id: 7 }),
            Some(Osc99DbusEvent::Closed {
                instance: instance(1)
            })
        );
    }

    #[test]
    fn system_actions_map_to_the_registrys_activations() {
        assert_eq!(
            Osc99SystemAction::Default.to_leg_activation(),
            Osc99LegActivation::Body
        );
        assert_eq!(
            Osc99SystemAction::Button(3).to_leg_activation(),
            Osc99LegActivation::Button(3)
        );
    }

    // ── Signal parsing ──────────────────────────────────────────────────

    /// A signal message from the notification daemon, awaiting its body.
    fn signal(member: &str) -> zbus::Result<zbus::message::Builder<'_>> {
        Message::signal(NOTIFICATIONS_PATH, NOTIFICATIONS_INTERFACE, member)
    }

    #[test]
    fn parse_signal_reads_action_invoked() -> TestResult {
        let message = signal("ActionInvoked")?.build(&(7u32, "2"))?;

        assert_eq!(
            parse_signal(&message),
            Some(PumpSignal::ActionInvoked {
                dbus_id: 7,
                action_key: "2".to_owned(),
            })
        );
        Ok(())
    }

    #[test]
    fn parse_signal_reads_notification_closed_whatever_the_reason() -> TestResult {
        let message = signal("NotificationClosed")?.build(&(7u32, 3u32))?;

        assert_eq!(
            parse_signal(&message),
            Some(PumpSignal::NotificationClosed { dbus_id: 7 })
        );
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

    // ── Service over a peer-to-peer connection ──────────────────────────

    /// What the fake daemon saw in one `Notify` call.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct RecordedNotify {
        app_name: String,
        replaces_id: u32,
        summary: String,
        body: String,
        actions: Vec<String>,
        urgency: Option<u8>,
        expire_timeout: i32,
        assigned_id: u32,
    }

    /// The `Notify` arguments, as the fake daemon deserialises them.
    type NotifyBody = (
        String,
        u32,
        String,
        String,
        String,
        Vec<String>,
        HashMap<String, OwnedValue>,
        i32,
    );

    /// Serve `org.freedesktop.Notifications` on `daemon` until its connection
    /// closes: `Notify` records the call and returns an incrementing id,
    /// `GetCapabilities` answers `capabilities`, `CloseNotification` and
    /// `GetServerInformation` answer plausibly.
    ///
    /// Hand-written rather than a `#[zbus::interface]`: the macro's expansion
    /// of the eight-argument `Notify` trips pedantic lints that cannot be
    /// silenced without an `allow`.
    fn serve_fake_daemon(
        daemon: &Connection,
        notifies: &Mutex<Vec<RecordedNotify>>,
        capabilities: &[String],
    ) -> zbus::Result<()> {
        for message in MessageIterator::from(daemon) {
            let message = message?;
            if message.message_type() != MessageType::MethodCall {
                continue;
            }
            let header = message.header();
            let Some(member) = header.member() else {
                continue;
            };
            match member.as_str() {
                "Notify" => {
                    let (app_name, replaces_id, _icon, summary, body, actions, hints, timeout) =
                        message.body().deserialize::<NotifyBody>()?;
                    let mut notifies = notifies.lock().unwrap_or_else(PoisonError::into_inner);
                    let assigned_id = if replaces_id == 0 {
                        u32::try_from(notifies.len()).map_or(u32::MAX, |n| n + 1)
                    } else {
                        replaces_id
                    };
                    notifies.push(RecordedNotify {
                        app_name,
                        replaces_id,
                        summary,
                        body,
                        actions,
                        urgency: hints
                            .get("urgency")
                            .and_then(|value| u8::try_from(value).ok()),
                        expire_timeout: timeout,
                        assigned_id,
                    });
                    drop(notifies);
                    daemon.reply(&header, &assigned_id)?;
                }
                "GetCapabilities" => daemon.reply(&header, &capabilities)?,
                "CloseNotification" => daemon.reply(&header, &())?,
                "GetServerInformation" => {
                    daemon.reply(&header, &("fake", "freminal-tests", "0", "1.2"))?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// A connected client and fake-daemon pair over a socket pair, plus the
    /// daemon's record of the `Notify` calls it served.
    struct Rig {
        client: Connection,
        daemon: Connection,
        notifies: Arc<Mutex<Vec<RecordedNotify>>>,
    }

    fn rig(advertised: &[&str]) -> Result<Rig, Box<dyn Error>> {
        let (server_stream, client_stream) = UnixStream::pair()?;
        let guid = zbus::Guid::generate();

        // The handshake needs both ends running at once.
        let server = std::thread::spawn(move || {
            zbus::blocking::connection::Builder::async_io_unix_stream(server_stream)
                .server(guid)?
                .p2p()
                .build()
        });
        let client = zbus::blocking::connection::Builder::async_io_unix_stream(client_stream)
            .p2p()
            .build()?;
        let daemon = server
            .join()
            .map_err(|_| "the fake daemon thread panicked")??;

        let notifies = Arc::new(Mutex::new(Vec::new()));
        let capabilities = strings(advertised);
        let daemon_side = daemon.clone();
        let record = Arc::clone(&notifies);
        std::thread::Builder::new()
            .name("fake-notification-daemon".to_owned())
            .spawn(move || {
                if let Err(e) = serve_fake_daemon(&daemon_side, &record, &capabilities) {
                    tracing::debug!("fake notification daemon stopped: {e}");
                }
            })?;
        Ok(Rig {
            client,
            daemon,
            notifies,
        })
    }

    impl Rig {
        fn recorded(&self) -> Vec<RecordedNotify> {
            self.notifies
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }

        fn emit_action(&self, dbus_id: u32, key: &str) -> zbus::Result<()> {
            self.daemon.emit_signal(
                None::<&str>,
                NOTIFICATIONS_PATH,
                NOTIFICATIONS_INTERFACE,
                "ActionInvoked",
                &(dbus_id, key),
            )
        }

        fn emit_closed(&self, dbus_id: u32, reason: u32) -> zbus::Result<()> {
            self.daemon.emit_signal(
                None::<&str>,
                NOTIFICATIONS_PATH,
                NOTIFICATIONS_INTERFACE,
                "NotificationClosed",
                &(dbus_id, reason),
            )
        }
    }

    /// The next event, failing the test if none arrives in [`EVENT_TIMEOUT`].
    fn next_event(handle: &Osc99DbusHandle) -> Result<Osc99DbusEvent, Box<dyn Error>> {
        Ok(handle.events.recv_timeout(EVENT_TIMEOUT)?)
    }

    /// A waker that counts how often it was called.
    fn counting_waker() -> (Box<dyn Fn() + Send>, Receiver<()>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        (
            Box::new(move || {
                let _ = tx.send(());
            }),
            rx,
        )
    }

    #[test]
    fn show_calls_notify_then_activation_and_close_signals_become_events() -> TestResult {
        let rig = rig(&["body", "actions"])?;
        let (waker, woken) = counting_waker();
        let mut handle = Osc99DbusService::spawn_with_connection(rig.client.clone(), waker);

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
            Osc99ShowDispatch::Queued
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
                summary: "Title".to_owned(),
                body: "Body".to_owned(),
                actions: strings(&["default", "", "1", "A", "2", "B"]),
                urgency: Some(2),
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

        // Every event was followed by a wake.
        for _ in 0..4 {
            woken.recv_timeout(EVENT_TIMEOUT)?;
        }
        Ok(())
    }

    #[test]
    fn a_daemon_without_actions_gets_no_action_list() -> TestResult {
        let rig = rig(&["body"])?;
        let (waker, _woken) = counting_waker();
        let mut handle = Osc99DbusService::spawn_with_connection(rig.client.clone(), waker);
        assert!(matches!(
            next_event(&handle)?,
            Osc99DbusEvent::Capabilities(_)
        ));

        assert_eq!(
            handle.show(instance(1), None, &with_buttons(&["A"])),
            Osc99ShowDispatch::Queued
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
        let rig = rig(&["body", "actions"])?;
        let (waker, _woken) = counting_waker();
        let mut handle = Osc99DbusService::spawn_with_connection(rig.client.clone(), waker);
        assert!(matches!(
            next_event(&handle)?,
            Osc99DbusEvent::Capabilities(_)
        ));
        assert_eq!(
            handle.show(instance(1), None, &notification()),
            Osc99ShowDispatch::Queued
        );
        assert!(matches!(next_event(&handle)?, Osc99DbusEvent::Shown { .. }));

        // Signals for ids nobody shown, and an unrelated member, then the
        // real close: the close is the first (and only) event, so the
        // earlier signals were ignored, in order, by the same dispatcher.
        rig.emit_action(999, "default")?;
        rig.emit_closed(999, 1)?;
        rig.daemon.emit_signal(
            None::<&str>,
            NOTIFICATIONS_PATH,
            NOTIFICATIONS_INTERFACE,
            "ActivationToken",
            &(1u32, "token"),
        )?;
        rig.emit_closed(rig.recorded()[0].assigned_id, 1)?;

        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Closed {
                instance: instance(1)
            }
        );
        assert!(handle.try_events().next().is_none());
        Ok(())
    }

    #[test]
    fn an_unreachable_bus_reports_unavailable_once_and_fails_every_show() -> TestResult {
        let (waker, woken) = counting_waker();
        let mut handle = start(|| Err(zbus::Error::Failure("no bus".to_owned())), waker);

        assert_eq!(next_event(&handle)?, Osc99DbusEvent::Unavailable);

        assert_eq!(
            handle.show(instance(1), None, &notification()),
            Osc99ShowDispatch::Queued,
            "queued; the dispatcher answers ShowFailed itself"
        );
        assert_eq!(
            handle.show(instance(2), None, &notification()),
            Osc99ShowDispatch::Queued
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
        for _ in 0..3 {
            woken.recv_timeout(EVENT_TIMEOUT)?;
        }
        Ok(())
    }

    #[test]
    fn a_show_after_the_dispatcher_is_gone_returns_failed_and_queues_no_event() -> TestResult {
        let (waker, _woken) = counting_waker();
        let mut handle = start(|| Err(zbus::Error::Failure("no bus".to_owned())), waker);
        assert_eq!(next_event(&handle)?, Osc99DbusEvent::Unavailable);

        // Replace the command sender with one whose receiver is already gone,
        // as after a dispatcher exit.
        let (dead_tx, dead_rx) = crossbeam_channel::unbounded();
        drop(dead_rx);
        handle.commands = dead_tx;

        assert_eq!(
            handle.show(instance(5), None, &notification()),
            Osc99ShowDispatch::Failed
        );
        assert_eq!(
            handle.try_events().next(),
            None,
            "no local event: the caller ends the leg"
        );
        Ok(())
    }
}
