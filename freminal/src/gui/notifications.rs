// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! GUI-side notification router (Task 76).
//!
//! Consumes notification requests originating from OSC 9 / OSC 777
//! sequences (forwarded via `WindowManipulation::Notification`, Task 76.3)
//! and from OSC 133 `D` command-finished events (Task 72.9), then applies
//! the per-category routing policy from the `[notifications]` config:
//!
//! - **Toast leg:** push an entry onto the in-app [`ToastStack`].
//! - **System leg:** show a desktop notification, as an untracked "plain"
//!   notification handed to a [`PlainSink`]. On Linux/BSD the sink reaches
//!   freminal's own D-Bus notification service (the `freminal-notify` crate).
//!   On macOS and Windows it reaches a long-lived background worker thread:
//!   `notify-rust` on macOS, and a toast built directly with
//!   `tauri-winrt-notification` on Windows.
//!   (OSC 99 is the exception: its system leg is driven by the registry through
//!   the platform's own `freminal-notify` backend, see `osc99`.)
//!
//! Routing is decided per [`NotificationKind`] using the focus-aware
//! [`NotificationRouting::wants_toast`] / [`NotificationRouting::wants_system`]
//! helpers. The toast leg never spawns a thread.

use std::borrow::Cow;
use std::sync::Weak;
use std::time::Instant;

use crossbeam_channel::Sender;
use freminal_common::buffer_states::command_block::{CommandBlock, CommandStatus};
use freminal_common::buffer_states::osc::OscNotifySource;
use freminal_common::buffer_states::osc99_notification::{
    Notification99Data, NotificationOccasion, NotificationUrgency, Osc99ControlKind,
};
use freminal_common::buffer_states::window_manipulation::NotificationKind;
use freminal_common::config::{FreminalToastCategory, NotificationRouting, NotificationsConfig};
use freminal_common::send_or_log;
use freminal_notify::{LinuxDaemon, PlainNotification, PlainSink, SYSTEM_LEG, SystemLeg};
use freminal_terminal_emulator::io::{GuiReply, InputEvent};

use super::command_blocks::format_command_duration;
use super::frame_drain::WindowFocus;
use super::osc99::{
    Osc99DisplayRequest, Osc99Effect, Osc99FilterVerdict, Osc99IconCache, Osc99Leg, Osc99PaneState,
    Osc99Registry, Osc99Request, PaneKeyboardFocus, PaneVisibility,
};
use super::toast::{ToastKind, ToastStack};

/// A fully-formed notification ready to be routed.
///
/// Produced by the OSC 9 / OSC 777 path (`handle_window_manipulation`) and
/// the command-finished path (the per-pane drain in `update()`), then handed
/// to [`NotificationRouter::route`].
#[derive(Debug, Clone)]
pub(super) struct NotificationRequest {
    /// The notification category, selecting the routing policy.
    pub kind: NotificationKind,
    /// Which OSC sequence produced this notification, if any.
    ///
    /// `Some` only for the OSC 9 / OSC 777 path, so the router can honour
    /// the per-source `notifications.osc_9` / `notifications.osc_777`
    /// enable toggles. `None` for command-finished requests and the
    /// Settings "Test Notification" button, neither of which has an OSC
    /// source.
    pub source: Option<OscNotifySource>,
    /// The notification title. `None` falls back to a kind-derived default.
    pub title: Option<String>,
    /// The notification body text.
    pub body: String,
}

impl NotificationRequest {
    /// A sample notification used by the Settings "Test Notification" button
    /// so the user can verify their routing/template configuration without
    /// running a command. Categorised as [`NotificationKind::OscText`] so it
    /// follows the `routing_osc_text` policy.
    pub(super) fn sample() -> Self {
        Self {
            kind: NotificationKind::OscText,
            source: None,
            title: Some("Freminal".to_owned()),
            body: "Test notification".to_owned(),
        }
    }

    /// The summary/title string shown to the user, falling back to a
    /// category-derived default when the source supplied no title.
    fn summary(&self) -> &str {
        self.title.as_deref().unwrap_or(match self.kind {
            NotificationKind::OscText => "Notification",
            NotificationKind::CommandFinished => "Command finished",
            NotificationKind::Error => "Error",
        })
    }

    /// This request as a [`PlainNotification`], on every platform: the
    /// summary (with its per-kind fallback) and the body, raw, and `Critical`
    /// urgency for [`NotificationKind::Error`], `Normal` for everything else.
    ///
    /// The text is raw because what to do about markup depends on the
    /// platform: the Linux/BSD service escapes it as the daemon's capabilities
    /// call for (issue #560), while macOS and Windows interpret none, so
    /// escaping there would only show a stray zero-width character.
    ///
    /// Error-category notifications use `Critical` so they persist and pop as
    /// a banner. Many Linux notification daemons only raise a banner (rather
    /// than silently filing the notification in the tray) when an urgency
    /// hint is set. macOS and Windows do not read the urgency.
    fn plain_notification(&self) -> PlainNotification {
        PlainNotification {
            summary: self.summary().to_owned(),
            body: self.body.clone(),
            urgency: if matches!(self.kind, NotificationKind::Error) {
                NotificationUrgency::Critical
            } else {
                NotificationUrgency::Normal
            },
        }
    }
}

/// Build a command-finished [`NotificationRequest`] from a completed
/// [`CommandBlock`], applying the `[notifications]` enable and
/// duration-threshold gates.
///
/// Returns `None` (no notification) when any of the following hold:
///
/// - the notification system is disabled, or `on_command_finished` is off;
/// - the command is not actually finished (`status() == Running`);
/// - the command's duration is below `command_finished_threshold_secs`
///   (or the duration is unknown — clock skew / missing timestamps).
///
/// A failed command produces an [`NotificationKind::Error`]; success or
/// unknown-exit produces [`NotificationKind::CommandFinished`]. `command`
/// is the extracted command text (may be empty if it scrolled out of the
/// buffer before extraction). `tab_name` is the display name of the tab the
/// command ran in, used for the `{tab_name}` template token.
///
/// The body is rendered from
/// [`NotificationsConfig::command_finished_template`] via
/// [`render_command_finished_template`].
pub(super) fn command_finished_request(
    block: &CommandBlock,
    command: &str,
    tab_name: &str,
    config: &NotificationsConfig,
) -> Option<NotificationRequest> {
    if !config.enabled || !config.on_command_finished {
        return None;
    }

    let status = block.status();
    if status == CommandStatus::Running {
        return None;
    }

    // 106.3: a block that finished without ever executing a command (no
    // `OSC 133 C` — e.g. Ctrl-C on an idle prompt) must not notify.  No
    // command ran, so there is nothing to report.  This is explicit rather
    // than relying on `duration()` returning `None` for the same case, so a
    // zero/low `command_finished_threshold_secs` cannot resurrect the
    // spurious notification.
    if !block.executed() {
        return None;
    }

    let duration = block.duration()?;
    if duration.as_secs_f32() < config.command_finished_threshold_secs {
        return None;
    }

    let body = render_command_finished_template(
        &config.command_finished_template,
        block,
        command,
        tab_name,
    );

    // A non-zero exit is an error notification; success / unknown-exit
    // (shell omitted the code) are informational "finished" notifications.
    // `Running` was already excluded by the status check above.
    let kind = if matches!(status, CommandStatus::Failure(_)) {
        NotificationKind::Error
    } else {
        NotificationKind::CommandFinished
    };

    Some(NotificationRequest {
        kind,
        source: None,
        title: None,
        body,
    })
}

/// Render a command-finished notification body from a template string.
///
/// Substitutes the documented tokens with values derived from `block`,
/// `command`, and `tab_name`:
///
/// - `{command}` — `command` trimmed, or `"Command"` when empty;
/// - `{duration}` — [`format_command_duration`] of the block's duration, or
///   an empty string when the duration is unknown;
/// - `{exit_code}` — the block's exit code, or `"?"` when the shell omitted
///   it;
/// - `{cwd}` — the block's captured working directory, or an empty string;
/// - `{tab_name}` — `tab_name` verbatim.
///
/// Unknown tokens are left untouched.
fn render_command_finished_template(
    template: &str,
    block: &CommandBlock,
    command: &str,
    tab_name: &str,
) -> String {
    let command = command.trim();
    let command_label = if command.is_empty() {
        "Command"
    } else {
        command
    };
    let duration_label = block
        .duration()
        .map(format_command_duration)
        .unwrap_or_default();
    let exit_label = block
        .exit_code
        .map_or_else(|| "?".to_owned(), |code| code.to_string());
    let cwd_label = block.cwd.as_deref().unwrap_or("");

    template
        .replace(TOKEN_COMMAND, command_label)
        .replace(TOKEN_DURATION, &duration_label)
        .replace(TOKEN_EXIT_CODE, &exit_label)
        .replace(TOKEN_CWD, cwd_label)
        .replace(TOKEN_TAB_NAME, tab_name)
}

// Template token literals, defined as module constants so the
// brace-delimited placeholders are not mistaken for `format!`-style
// arguments by clippy's `literal_string_with_formatting_args` lint —
// these are user-facing template tokens, not Rust formatting directives.
const TOKEN_COMMAND: &str = "{command}";
const TOKEN_DURATION: &str = "{duration}";
const TOKEN_EXIT_CODE: &str = "{exit_code}";
const TOKEN_CWD: &str = "{cwd}";
const TOKEN_TAB_NAME: &str = "{tab_name}";

/// Stateless notification dispatcher.
///
/// The router holds no state of its own; it is a namespace for the routing
/// logic so the call sites read clearly and the policy is unit-testable in
/// isolation from the egui frame loop.
#[derive(Debug, Default, Clone, Copy)]
pub(super) struct NotificationRouter;

impl NotificationRouter {
    /// Route a single notification request according to `config` and the
    /// current window focus state.
    ///
    /// Does nothing when the notification system is disabled
    /// ([`NotificationsConfig::enabled`] is `false`). The toast leg pushes
    /// onto `toasts`; the system leg is handed to `system` (see
    /// [`Self::show_system`]).
    pub(super) fn route(
        req: &NotificationRequest,
        config: &NotificationsConfig,
        focused: bool,
        toasts: &mut ToastStack,
        system: &PlainSink,
    ) {
        if !config.enabled {
            return;
        }

        // Honour the per-source enable toggles for OSC-text notifications,
        // mirroring the osc_99 kill-switch in route_osc99.
        match req.source {
            Some(OscNotifySource::Osc9) if !config.osc_9 => return,
            Some(OscNotifySource::Osc777) if !config.osc_777 => return,
            _ => {}
        }

        let routing = Self::routing_for(req.kind, config);

        if routing.wants_toast(focused) {
            Self::push_toast(req, toasts);
        }

        if routing.wants_system(focused) {
            Self::show_system(req, system);
        }
    }

    /// Route a request for the Settings "Test Notification" button, ignoring
    /// the [`NotificationsConfig::enabled`] master switch.
    ///
    /// The test button must give the user feedback even while the master
    /// switch is off (they may be configuring routing before enabling the
    /// system), so the only difference from [`Self::route`] is the skipped
    /// `enabled` check. The per-category routing policy still applies.
    pub(super) fn route_test(
        req: &NotificationRequest,
        config: &NotificationsConfig,
        focused: bool,
        toasts: &mut ToastStack,
        system: &PlainSink,
    ) {
        // Same per-source enable-toggle gate as `route`. The Test button's
        // sample request always has `source: None`, so this is a no-op in
        // practice, but kept identical for uniformity between the two entry
        // points.
        match req.source {
            Some(OscNotifySource::Osc9) if !config.osc_9 => return,
            Some(OscNotifySource::Osc777) if !config.osc_777 => return,
            _ => {}
        }

        let routing = Self::routing_for(req.kind, config);

        if routing.wants_toast(focused) {
            Self::push_toast(req, toasts);
        }

        if routing.wants_system(focused) {
            Self::show_system(req, system);
        }
    }

    /// Select the routing policy for a notification category.
    const fn routing_for(
        kind: NotificationKind,
        config: &NotificationsConfig,
    ) -> NotificationRouting {
        match kind {
            NotificationKind::Error => config.routing_error,
            NotificationKind::CommandFinished => config.routing_command_finished,
            NotificationKind::OscText => config.routing_osc_text,
        }
    }

    /// Push the notification onto the in-app toast stack. Error-category
    /// notifications use the error styling; everything else is informational.
    fn push_toast(req: &NotificationRequest, toasts: &mut ToastStack) {
        let detail = (!req.body.is_empty()).then(|| req.body.clone());
        match req.kind {
            NotificationKind::Error => toasts.error(req.summary().to_owned(), detail),
            NotificationKind::OscText | NotificationKind::CommandFinished => {
                toasts.info(req.summary().to_owned(), detail);
            }
        }
    }

    /// Route a freminal-derived, toast-only notification (e.g. "Copied to
    /// clipboard", "Layout saved") through its per-category routing policy.
    ///
    /// Unlike [`Self::route`], there is no desktop-notification leg and no
    /// `enabled` master-switch gate: these are freminal's own UI feedback
    /// toasts, governed solely by their `routing_<category>` config field
    /// ([`FreminalToastRouting`], which is `Toast` or `Disabled`). When the
    /// category routes to `Disabled`, nothing is pushed.
    ///
    /// `kind` selects the toast severity (e.g. [`ToastKind::Warning`] for a
    /// blocked paste, [`ToastKind::Info`] for a normal confirmation).
    /// `placement` selects the anchor (see [`super::toast::ToastPlacement`]):
    /// freminal-derived toasts are typically window- or pane-centered.
    pub(super) fn route_freminal_toast(
        category: FreminalToastCategory,
        kind: ToastKind,
        title: impl Into<String>,
        detail: Option<String>,
        placement: super::toast::ToastPlacement,
        config: &NotificationsConfig,
        toasts: &mut ToastStack,
    ) {
        if !config.routing_for_category(category).wants_toast() {
            return;
        }
        toasts.push_positioned(kind, title, detail, placement);
    }

    /// Show a desktop notification: the request is mapped to a
    /// [`PlainNotification`] and handed to `system` without blocking.
    ///
    /// Where it goes is the sink's business (the `freminal-notify` crate's
    /// D-Bus service on Linux/BSD, its plain-notification worker thread,
    /// named `freminal-notify`, on macOS and Windows), and a notification
    /// that cannot be shown is dropped: a missing notification daemon is
    /// non-fatal.
    ///
    /// There is no test guard: the sink is explicit, so a test that passes
    /// [`PlainSink::nowhere`] reaches no desktop session, and one that passes
    /// `PlainSink::recording` observes exactly what would be sent.
    fn show_system(req: &NotificationRequest, system: &PlainSink) {
        system.show(req.plain_notification());
    }
}

/// Send `reply` to the originating pane's PTY consumer through a `Weak`
/// handle to its input sender.
///
/// OSC 99 replies can be produced long after the frame that collected the
/// request (the registry answers when a notification is activated or closed,
/// possibly long after), so they must never hold a strong
/// `Sender<InputEvent>`: that would keep the closed pane's PTY consumer thread
/// -- and its shell -- alive until the notification is dismissed. The handle is upgraded per
/// send; if the pane is gone the upgrade fails and the reply is dropped
/// (nobody is left to read it).
pub(super) fn send_reply_weak(reply_tx: &Weak<Sender<InputEvent>>, reply: GuiReply) {
    let Some(tx) = reply_tx.upgrade() else {
        tracing::debug!("dropping OSC 99 reply: originating pane is closed");
        return;
    };
    send_or_log!(tx, InputEvent::Reply(reply), "Failed to send OSC 99 reply");
}

/// An OSC 99 app→terminal control sequence collected from
/// `WindowManipulation::Osc99Control` during `handle_window_manipulation`
/// (Task 99.5c). Paired with a `Weak` handle to the pane's reply sender in
/// `app_impl::update()`'s post-loop routing, where it is answered
/// (Task 99.6: close/alive). `p=?` never reaches the GUI: the PTY thread
/// answers it itself (Task 130.5).
#[derive(Debug, Clone)]
pub(super) struct Osc99Control {
    /// Notification id (`i=`) the control refers to. Always present: id-less
    /// controls never leave the PTY thread.
    pub id: String,
    /// Which control payload type this is.
    pub kind: Osc99ControlKind,
}

/// Display context for an OSC 99 notification.
///
/// Carries the two kinds of state routing needs, which are deliberately
/// distinct: the originating pane's state, for the protocol's `o=` occasion
/// gate ([`occasion_allows_display`]), and OS-window focus, for freminal's own
/// toast/system routing choice (`wants_toast` / `wants_system`).
#[derive(Debug, Clone, Copy)]
pub(super) struct Osc99DisplayContext {
    /// Whether the source OS window currently has input focus. Routing only;
    /// the `o=` gate uses [`Self::pane`].
    pub window_focus: WindowFocus,
    /// The originating pane's focus and visibility, for the `o=` gate.
    pub pane: Osc99PaneState,
    /// The display time. An `w=` expiry is measured from it.
    pub now: Instant,
    /// What is known about the Linux/BSD notification daemon. A
    /// system leg the daemon cannot show falls back to the toast leg
    /// ([`resolve_osc99_legs`]). Ignored on macOS and Windows, whose system
    /// leg does not use the daemon.
    pub daemon: LinuxDaemon,
}

/// The legs an OSC 99 notification is displayed on: the toast and/or system
/// legs `routing` picks for `window_focus`, with the Linux/BSD system leg
/// replaced by the toast leg when `daemon` is
/// [`LinuxDaemon::Unavailable`] (no daemon could show it, and the
/// capability advertisement already promises the toast leg only), without
/// listing the toast leg twice.
///
/// `system_leg` is the platform's system leg, a parameter so the fallback is
/// testable on every platform.
fn resolve_osc99_legs(
    routing: NotificationRouting,
    window_focus: WindowFocus,
    system_leg: Osc99Leg,
    daemon: LinuxDaemon,
) -> Vec<Osc99Leg> {
    let focused = window_focus.is_focused();
    let mut legs = Vec::with_capacity(2);
    if routing.wants_toast(focused) {
        legs.push(Osc99Leg::Toast);
    }
    if routing.wants_system(focused) {
        let leg = if system_leg == Osc99Leg::LinuxSystem && daemon == LinuxDaemon::Unavailable {
            Osc99Leg::Toast
        } else {
            system_leg
        };
        if !legs.contains(&leg) {
            legs.push(leg);
        }
    }
    legs
}

/// Outcome of the OSC 99 `o=` occasion check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Osc99DisplayDecision {
    /// The notification may be displayed.
    Show,
    /// The notification must not be displayed.
    Suppress,
}

/// Evaluate the OSC 99 `o=` occasion gate for the originating pane.
///
/// - `Always` — always show.
/// - `Unfocused` — show only when the pane lacks keyboard focus (so a
///   notification from a background tab, or an inactive pane, shows even in a
///   focused window).
/// - `Invisible` — show only when the pane lacks keyboard focus *and* is not
///   visible (so an inactive pane that is still on screen suppresses it).
pub(super) const fn occasion_allows_display(
    occasion: NotificationOccasion,
    pane: Osc99PaneState,
) -> Osc99DisplayDecision {
    let unfocused = matches!(pane.focus, PaneKeyboardFocus::Unfocused);
    let hidden = matches!(pane.visibility, PaneVisibility::Hidden);
    let show = match occasion {
        NotificationOccasion::Always => true,
        NotificationOccasion::Unfocused => unfocused,
        NotificationOccasion::Invisible => unfocused && hidden,
    };
    if show {
        Osc99DisplayDecision::Show
    } else {
        Osc99DisplayDecision::Suppress
    }
}

impl NotificationRouter {
    /// Route an OSC 99 stateful notification to the toast leg and/or the OS
    /// notification daemon, honouring the occasion gate, urgency, sound,
    /// expiry, buttons, and icon. Every notification that at least one leg
    /// displays is recorded in `registry` (with the legs that fired, and
    /// whether or not it carries an `i=` id) for later `p=alive` / `p=close`
    /// handling.
    ///
    /// Does nothing when the notification system is disabled
    /// ([`NotificationsConfig::enabled`] is `false`), when the OSC 99
    /// kill-switch is off ([`NotificationsConfig::osc_99`] is `false`,
    /// Task 99.8), or when the occasion gate says the notification should
    /// not be displayed right now (in which case it is also not recorded in
    /// `registry` — an occasion-suppressed notification never happened as
    /// far as later update/close tracking is concerned), or when the user's
    /// `osc99_filters` (held by `registry`) drop it (likewise not recorded,
    /// and no reply is sent). The toast and
    /// system legs are additionally gated per-leg by
    /// [`NotificationsConfig::routing_osc99`] (3.3/3.4), mirroring
    /// [`Self::route`]; a request routed [`NotificationRouting::Disabled`]
    /// is not recorded either, since neither leg displayed it.
    ///
    /// On macOS and Windows a notification shown on the system leg is
    /// fire-and-forget and cannot be observed closing, so it is registered as
    /// an untracked leg: the registry returns an
    /// [`Osc99Effect::ShowUntrackedSystem`] effect, and answers a `c=1`
    /// request with the `untracked` close report at once. On Linux/BSD the
    /// system leg is driven by the registry: it is returned as an
    /// [`Osc99Effect::ShowLinuxSystem`] effect for the notification service,
    /// and its activation and close come back through the registry.
    ///
    /// The registry's effects (the toast and desktop notification to show,
    /// ...) are returned rather
    /// than applied, so the caller can execute them with the full
    /// [`apply_osc99_effects`](super::osc99::apply_osc99_effects) sink, which
    /// this function has no access to. The result is empty when nothing was
    /// displayed.
    pub(super) fn route_osc99(
        request: Osc99DisplayRequest,
        config: &NotificationsConfig,
        ctx: Osc99DisplayContext,
        icon_cache: &mut Osc99IconCache,
        registry: &mut Osc99Registry,
    ) -> Vec<Osc99Effect> {
        let Osc99DisplayRequest {
            mut data,
            reply_tx,
            origin,
            ..
        } = request;
        if !config.enabled {
            return Vec::new();
        }

        // OSC 99 kill-switch (Task 99.8): even with the master switch on, users can
        // disable the kitty stateful protocol specifically.
        if !config.osc_99 {
            return Vec::new();
        }

        // Resolve/cache icon bytes *before* the occasion gate: caching must
        // not depend on whether this particular instance is displayed. An
        // occasion-suppressed notification that transmits icon bytes with a
        // `g=` cache key must still populate the cache, so a later
        // (displayed) notification referencing the same `g=` key with bytes
        // omitted can resolve the icon.
        //
        // The notification handed to the legs carries the resolved bytes in
        // `icon_data` (copied only when the `g=` cache supplied them), so
        // the Linux/BSD notification service and the macOS/Windows system
        // leg both see the icon the program meant. The notification is owned
        // here, so nothing else is copied.
        let from_cache = match Self::resolve_icon_bytes(&data, icon_cache) {
            Some(bytes) if data.icon_data.is_none() => Some(bytes.into_owned()),
            _ => None,
        };
        if let Some(bytes) = from_cache {
            data.icon_data = Some(bytes);
        }

        if occasion_allows_display(data.occasion, ctx.pane) == Osc99DisplayDecision::Suppress {
            return Vec::new();
        }

        // User filter (`osc99_filters`), after the occasion check as in
        // kitty. A dropped notification is displayed nowhere, registered
        // nowhere and answered with nothing.
        if registry.filter_verdict(&data) == Osc99FilterVerdict::Drop {
            tracing::debug!(id = ?data.id, "OSC 99 notification dropped by osc99_filters");
            return Vec::new();
        }

        // Toast/system legs honour `routing_osc99` (3.3/3.4), mirroring
        // `route`'s focus-aware gating. One decision picks the legs, so the
        // toast leg, the system leg, and the registry below all agree. The
        // legs are shown by the effects `register` returns below. Only
        // Linux/BSD observe a close or activation (through the notification
        // service); elsewhere the system notification is fire-and-forget and
        // shown through the untracked backend.
        let system_leg = match SYSTEM_LEG {
            SystemLeg::Tracked => Osc99Leg::LinuxSystem,
            SystemLeg::Untracked => Osc99Leg::UntrackedSystem,
        };
        let legs = resolve_osc99_legs(
            config.routing_osc99,
            ctx.window_focus,
            system_leg,
            ctx.daemon,
        );

        // A notification displayed on no leg (routed `Disabled`) never
        // happened as far as later `p=alive` / `p=close` handling goes.
        // Every displayed notification is registered, whether or not it
        // carries an `i=` id and whichever leg showed it.
        if !legs.is_empty() {
            tracing::trace!(id = ?data.id, legs = ?legs, "registering OSC 99 notification");
            return registry.register(origin, reply_tx, data, &legs, ctx.now);
        }
        Vec::new()
    }

    /// Route one OSC 99 request of a frame's ordered list and return the
    /// effects to execute.
    ///
    /// The caller routes the frame's requests strictly in arrival order, one
    /// call each, so a `p=close` or `p=alive` sees exactly the instances the
    /// requests before it created, and a display sees the icons the primings
    /// before it stored:
    /// - [`Osc99Request::Display`] goes through [`Self::route_osc99`], with
    ///   the request's own pane state for the `o=` gate;
    /// - [`Osc99Request::Control`] answers `p=alive` from, and closes for
    ///   `p=close` in, `registry`;
    /// - [`Osc99Request::PrimeIcon`] stores the bytes in `icon_cache` and
    ///   returns no effects.
    pub(super) fn route_osc99_request(
        request: Osc99Request,
        config: &NotificationsConfig,
        window_focus: WindowFocus,
        daemon: LinuxDaemon,
        icon_cache: &mut Osc99IconCache,
        registry: &mut Osc99Registry,
    ) -> Vec<Osc99Effect> {
        match request {
            Osc99Request::Display(notification) => {
                tracing::trace!(
                    origin = ?notification.origin,
                    pane_state = ?notification.pane_state,
                    "routing OSC 99 notification"
                );
                let ctx = Osc99DisplayContext {
                    window_focus,
                    pane: notification.pane_state,
                    now: Instant::now(),
                    daemon,
                };
                Self::route_osc99(*notification, config, ctx, icon_cache, registry)
            }
            Osc99Request::Control(control) => {
                tracing::trace!(
                    origin = ?control.origin,
                    kind = ?control.control.kind,
                    "routing OSC 99 control"
                );
                match control.control.kind {
                    // Answer the poll with the requesting pane's live ids.
                    Osc99ControlKind::Alive => {
                        registry.alive(control.origin, control.control.id, control.reply_tx)
                    }
                    // App-driven close: close the pane's toast, if shown; the
                    // close report follows when the toast's `Closed` event
                    // arrives.
                    Osc99ControlKind::Close => registry.close(control.origin, &control.control.id),
                }
            }
            Osc99Request::PrimeIcon(prime) => {
                prime.store_in(icon_cache);
                Vec::new()
            }
        }
    }

    /// Resolve the icon bytes to display, applying the `g=` cache.
    ///
    /// - Transmitted bytes (`icon_data`) always win and are cached under
    ///   `icon_cache_key` (`g=`) when present, so a later `g=`-only
    ///   notification can reuse them.
    /// - Otherwise, a `g=`-only notification looks up the cache.
    /// - `None` when neither is available (the OS leg then falls back to
    ///   icon-by-name).
    ///
    /// The bytes are borrowed from `data` or the cache, never copied here;
    /// the cache itself keeps the one copy it needs to own.
    fn resolve_icon_bytes<'a>(
        data: &'a Notification99Data,
        icon_cache: &'a mut Osc99IconCache,
    ) -> Option<Cow<'a, [u8]>> {
        if let Some(bytes) = &data.icon_data {
            if let Some(key) = &data.icon_cache_key {
                icon_cache.insert(key.clone(), bytes.clone());
            }
            Some(Cow::Borrowed(bytes.as_slice()))
        } else {
            data.icon_cache_key
                .as_ref()
                .and_then(|key| icon_cache.get(key))
                .map(Cow::Borrowed)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::host_capabilities::{PlainNotifyEntry, plain_notifications_may_use_system_leg};
    use crate::gui::osc99::{
        Osc99ControlRequest, Osc99EffectSink, Osc99Filter, Osc99IconPrime, Osc99Origin,
        apply_osc99_effects, osc99_toast_spec,
    };
    use freminal_common::buffer_states::osc99_notification::{
        NotificationUrgency, Osc99ActivationFocus, Osc99ActivationReporting, Osc99CloseReporting,
        Osc99Expiry, Osc99Sound,
    };
    use freminal_common::buffer_states::row_number::RowNumber;
    use freminal_common::config::FreminalToastRouting;
    use freminal_notify::{NoTrackedBackend, NoUntrackedBackend, TrackedBackend};
    use freminal_terminal_emulator::io::Osc99CloseTracking;

    /// Route `request` and execute the effects `route_osc99` returns against
    /// `toasts` and a backend that shows no desktop notifications, as the app
    /// does (minus the focus queue).
    fn route_osc99_into(
        request: &Osc99DisplayRequest,
        config: &NotificationsConfig,
        ctx: Osc99DisplayContext,
        toasts: &mut ToastStack,
        icon_cache: &mut Osc99IconCache,
        registry: &mut Osc99Registry,
    ) {
        route_osc99_with_system(
            request,
            config,
            ctx,
            toasts,
            icon_cache,
            registry,
            &mut NoTrackedBackend,
        );
    }

    /// [`route_osc99_into`] with a caller-supplied desktop-notification
    /// backend.
    fn route_osc99_with_system(
        request: &Osc99DisplayRequest,
        config: &NotificationsConfig,
        ctx: Osc99DisplayContext,
        toasts: &mut ToastStack,
        icon_cache: &mut Osc99IconCache,
        registry: &mut Osc99Registry,
        system: &mut dyn TrackedBackend,
    ) {
        let effects =
            NotificationRouter::route_osc99(request.clone(), config, ctx, icon_cache, registry);
        apply_osc99_effects(
            effects,
            &mut Osc99EffectSink::new(toasts, system, &mut NoUntrackedBackend),
        );
    }

    fn osc_req(body: &str) -> NotificationRequest {
        NotificationRequest {
            kind: NotificationKind::OscText,
            source: Some(OscNotifySource::Osc9),
            title: None,
            body: body.to_owned(),
        }
    }

    /// The text is carried raw, with no stray zero-width characters: macOS and
    /// Windows have no markup to defuse, and on Linux/BSD escaping depends on
    /// the daemon's capabilities, so the service does it (see the notification
    /// service's `state` module) and the mapping must not.
    #[test]
    fn plain_notification_carries_the_raw_summary_and_body() {
        let text = r#"<b>x</b> & <a href="http://e">y</a>"#;
        let req = NotificationRequest {
            kind: NotificationKind::OscText,
            source: Some(OscNotifySource::Osc777),
            title: Some(text.to_owned()),
            body: text.to_owned(),
        };

        let plain = req.plain_notification();
        assert_eq!(plain.summary, text);
        assert_eq!(plain.body, text);
        assert!(!plain.summary.contains('\u{200c}'));
        assert!(!plain.body.contains('\u{200c}'));
    }

    /// The mapping applies the per-kind summary fallback when the request
    /// carries no explicit title.
    #[test]
    fn plain_notification_falls_back_to_the_per_kind_summary() {
        for (kind, summary) in [
            (NotificationKind::OscText, "Notification"),
            (NotificationKind::CommandFinished, "Command finished"),
            (NotificationKind::Error, "Error"),
        ] {
            let req = NotificationRequest {
                kind,
                ..osc_req("body text")
            };
            let plain = req.plain_notification();
            assert_eq!(plain.summary, summary, "{kind:?}");
            assert_eq!(plain.body, "body text");
        }

        let titled = NotificationRequest {
            title: Some("Build".to_owned()),
            ..osc_req("done")
        };
        assert_eq!(titled.plain_notification().summary, "Build");
    }

    /// Error-category notifications carry Critical urgency; everything else is
    /// Normal.
    #[test]
    fn plain_notification_urgency_by_kind() {
        for (kind, urgency) in [
            (NotificationKind::Error, NotificationUrgency::Critical),
            (NotificationKind::OscText, NotificationUrgency::Normal),
            (
                NotificationKind::CommandFinished,
                NotificationUrgency::Normal,
            ),
        ] {
            let req = NotificationRequest {
                kind,
                ..osc_req("x")
            };
            assert_eq!(req.plain_notification().urgency, urgency, "{kind:?}");
        }
    }

    /// A sink with no service drops a system notification without panicking
    /// or blocking.
    #[test]
    fn a_sink_without_a_service_drops_the_notification() {
        PlainSink::nowhere().show(osc_req("hello").plain_notification());
    }

    #[test]
    fn summary_falls_back_per_kind() {
        let mut req = osc_req("x");
        assert_eq!(req.summary(), "Notification");
        req.kind = NotificationKind::CommandFinished;
        assert_eq!(req.summary(), "Command finished");
        req.kind = NotificationKind::Error;
        assert_eq!(req.summary(), "Error");
    }

    #[test]
    fn summary_uses_explicit_title_when_present() {
        let req = NotificationRequest {
            kind: NotificationKind::OscText,
            source: Some(OscNotifySource::Osc777),
            title: Some("Build".to_owned()),
            body: "done".to_owned(),
        };
        assert_eq!(req.summary(), "Build");
    }

    /// An explicit title and the body are carried through unchanged.
    #[test]
    fn plain_notification_carries_the_title_and_body() {
        let req = NotificationRequest {
            kind: NotificationKind::OscText,
            source: Some(OscNotifySource::Osc9),
            title: Some("Hello".to_owned()),
            body: "world".to_owned(),
        };
        assert_eq!(
            req.plain_notification(),
            PlainNotification {
                summary: "Hello".to_owned(),
                body: "world".to_owned(),
                urgency: NotificationUrgency::Normal,
            }
        );
    }

    #[test]
    fn route_osc99_toast_is_error_styled_only_for_critical_urgency() {
        for (urgency, expected) in [
            (NotificationUrgency::Low, ToastKind::Info),
            (NotificationUrgency::Normal, ToastKind::Info),
            (NotificationUrgency::Critical, ToastKind::Error),
        ] {
            let config = enabled_config(0.0);
            let mut toasts = ToastStack::default();
            let mut icon_cache = Osc99IconCache::default();
            let mut registry = Osc99Registry::default();
            let data = Notification99Data {
                urgency,
                ..default_n99()
            };

            route_osc99_into(
                &osc99_request(&data),
                &config,
                focused_ctx(),
                &mut toasts,
                &mut icon_cache,
                &mut registry,
            );

            assert_eq!(toasts.last_kind(), Some(expected), "urgency {urgency:?}");
        }
    }

    #[test]
    fn route_osc99_toast_shows_the_title_and_the_body_without_duplication() {
        // `ToastStack` exposes no title accessor, so the toast's content is
        // pinned through the spec the registry's `ShowToast` effect carries
        // (the same function `route_osc99` reaches via `register`), and the
        // routing is pinned by the stack gaining exactly one keyed toast.
        let config = enabled_config(0.0);
        let mut toasts = ToastStack::default();
        let mut icon_cache = Osc99IconCache::default();
        let mut registry = Osc99Registry::default();
        let data = Notification99Data {
            title: "The title".to_owned(),
            body: Some("The body".to_owned()),
            ..default_n99()
        };

        route_osc99_into(
            &osc99_request(&data),
            &config,
            focused_ctx(),
            &mut toasts,
            &mut icon_cache,
            &mut registry,
        );

        assert_eq!(toasts.len(), 1);
        let spec = osc99_toast_spec(&data);
        assert_eq!(spec.title, "The title");
        assert_eq!(spec.detail.as_deref(), Some("The body"));
    }

    #[test]
    fn routing_for_maps_each_kind() {
        let mut config = NotificationsConfig {
            routing_error: NotificationRouting::Both,
            routing_osc_text: NotificationRouting::Toast,
            routing_command_finished: NotificationRouting::System,
            ..NotificationsConfig::default()
        };
        config.enabled = true;

        assert_eq!(
            NotificationRouter::routing_for(NotificationKind::Error, &config),
            NotificationRouting::Both
        );
        assert_eq!(
            NotificationRouter::routing_for(NotificationKind::CommandFinished, &config),
            NotificationRouting::System
        );
        assert_eq!(
            NotificationRouter::routing_for(NotificationKind::OscText, &config),
            NotificationRouting::Toast
        );
    }

    /// Whether `routing` ever shows a desktop notification, at some focus
    /// state. An independent oracle for the plain gate: no wildcard, so a new
    /// routing variant must be classified here too.
    const fn routing_has_system_leg(routing: NotificationRouting) -> bool {
        match routing {
            NotificationRouting::System
            | NotificationRouting::Both
            | NotificationRouting::SystemWhenUnfocused => true,
            NotificationRouting::Toast | NotificationRouting::Disabled => false,
        }
    }

    const ALL_ROUTINGS: [NotificationRouting; 5] = [
        NotificationRouting::Toast,
        NotificationRouting::System,
        NotificationRouting::Both,
        NotificationRouting::SystemWhenUnfocused,
        NotificationRouting::Disabled,
    ];

    fn toast_only_notifications() -> NotificationsConfig {
        NotificationsConfig {
            enabled: true,
            routing_osc_text: NotificationRouting::Toast,
            routing_command_finished: NotificationRouting::Toast,
            routing_error: NotificationRouting::Toast,
            ..NotificationsConfig::default()
        }
    }

    #[test]
    fn the_plain_gate_reads_the_routing_the_router_uses_for_every_kind() {
        type Setter = fn(&mut NotificationsConfig, NotificationRouting);

        /// The only place that names every kind: adding a variant to
        /// `NotificationKind` stops this from compiling, which is the cue to
        /// add it to `ALL_KINDS` and to teach `plain_notifications_may_use_system_leg`
        /// its routing.
        const fn position(kind: NotificationKind) -> usize {
            match kind {
                NotificationKind::OscText => 0,
                NotificationKind::CommandFinished => 1,
                NotificationKind::Error => 2,
            }
        }
        const ALL_KINDS: [NotificationKind; 3] = [
            NotificationKind::OscText,
            NotificationKind::CommandFinished,
            NotificationKind::Error,
        ];
        let setters: [Setter; 3] = [
            |config, routing| config.routing_osc_text = routing,
            |config, routing| config.routing_command_finished = routing,
            |config, routing| config.routing_error = routing,
        ];

        for (index, kind) in ALL_KINDS.into_iter().enumerate() {
            assert_eq!(position(kind), index, "ALL_KINDS is out of order");
        }

        for kind in ALL_KINDS {
            // Which `routing_*` field the router reads for `kind`: the one that,
            // set to `System` with the others toast-only, it returns.
            let reads: Vec<Setter> = setters
                .into_iter()
                .filter(|set| {
                    let mut config = toast_only_notifications();
                    set(&mut config, NotificationRouting::System);
                    NotificationRouter::routing_for(kind, &config) == NotificationRouting::System
                })
                .collect();
            assert_eq!(reads.len(), 1, "{kind:?} must read exactly one routing");

            for routing in ALL_ROUTINGS {
                let mut config = toast_only_notifications();
                reads[0](&mut config, routing);
                assert_eq!(
                    NotificationRouter::routing_for(kind, &config),
                    routing,
                    "{kind:?}"
                );
                assert_eq!(
                    plain_notifications_may_use_system_leg(&config, PlainNotifyEntry::Routed),
                    routing_has_system_leg(routing),
                    "{kind:?} routed {routing:?}"
                );
            }
        }
    }

    #[test]
    fn disabled_config_pushes_no_toast() {
        let config = NotificationsConfig::default(); // enabled = false
        let mut toasts = ToastStack::default();
        NotificationRouter::route(
            &osc_req("hello"),
            &config,
            true,
            &mut toasts,
            &PlainSink::nowhere(),
        );
        assert_eq!(toasts.len(), 0);
    }

    #[test]
    fn route_test_ignores_enabled_master_switch() {
        // route() drops everything when disabled; route_test() must still
        // fire so the Settings "Test Notification" button gives feedback.
        let config = NotificationsConfig {
            enabled: false,
            routing_osc_text: NotificationRouting::Toast,
            ..NotificationsConfig::default()
        };
        let mut toasts = ToastStack::default();
        NotificationRouter::route_test(
            &NotificationRequest::sample(),
            &config,
            true,
            &mut toasts,
            &PlainSink::nowhere(),
        );
        assert_eq!(toasts.len(), 1);
    }

    #[test]
    fn sample_request_is_osc_text_kind() {
        let req = NotificationRequest::sample();
        assert_eq!(req.kind, NotificationKind::OscText);
        assert_eq!(req.summary(), "Freminal");
    }

    #[test]
    fn enabled_toast_routing_pushes_one_toast() {
        let config = NotificationsConfig {
            enabled: true,
            routing_osc_text: NotificationRouting::Toast,
            ..NotificationsConfig::default()
        };
        let mut toasts = ToastStack::default();
        // Focused or not, Toast routing always pushes a toast.
        NotificationRouter::route(
            &osc_req("hello"),
            &config,
            false,
            &mut toasts,
            &PlainSink::nowhere(),
        );
        assert_eq!(toasts.len(), 1);
    }

    #[test]
    fn system_only_routing_pushes_no_toast() {
        let config = NotificationsConfig {
            enabled: true,
            routing_osc_text: NotificationRouting::System,
            ..NotificationsConfig::default()
        };
        let mut toasts = ToastStack::default();
        // System-only routing never pushes a toast. The sink has nowhere to
        // send the system notification, so it is dropped.
        NotificationRouter::route(
            &osc_req("hello"),
            &config,
            false,
            &mut toasts,
            &PlainSink::nowhere(),
        );
        assert_eq!(toasts.len(), 0);
    }

    // ── Per-source enable-toggle gate (issue #433, subtask 3.2) ──────────

    /// Build an OSC-777-sourced request (used to test the `osc_777` toggle
    /// independently of `osc_req`'s OSC 9 source).
    fn osc777_req(body: &str) -> NotificationRequest {
        NotificationRequest {
            kind: NotificationKind::OscText,
            source: Some(OscNotifySource::Osc777),
            title: None,
            body: body.to_owned(),
        }
    }

    #[test]
    fn osc_9_disabled_suppresses_osc9_sourced_request() {
        let config = NotificationsConfig {
            enabled: true,
            osc_9: false,
            routing_osc_text: NotificationRouting::Toast,
            ..NotificationsConfig::default()
        };
        let mut toasts = ToastStack::default();
        NotificationRouter::route(
            &osc_req("hello"),
            &config,
            true,
            &mut toasts,
            &PlainSink::nowhere(),
        );
        assert_eq!(
            toasts.len(),
            0,
            "osc_9 = false must suppress an Osc9 request"
        );
    }

    #[test]
    fn osc_777_disabled_suppresses_osc777_sourced_request() {
        let config = NotificationsConfig {
            enabled: true,
            osc_777: false,
            routing_osc_text: NotificationRouting::Toast,
            ..NotificationsConfig::default()
        };
        let mut toasts = ToastStack::default();
        NotificationRouter::route(
            &osc777_req("hello"),
            &config,
            true,
            &mut toasts,
            &PlainSink::nowhere(),
        );
        assert_eq!(
            toasts.len(),
            0,
            "osc_777 = false must suppress an Osc777 request"
        );
    }

    #[test]
    fn both_osc_sources_enabled_still_route() {
        let config = NotificationsConfig {
            enabled: true,
            osc_9: true,
            osc_777: true,
            routing_osc_text: NotificationRouting::Toast,
            ..NotificationsConfig::default()
        };

        let mut toasts = ToastStack::default();
        NotificationRouter::route(
            &osc_req("from osc9"),
            &config,
            true,
            &mut toasts,
            &PlainSink::nowhere(),
        );
        assert_eq!(toasts.len(), 1, "osc_9 = true must still route");

        let mut toasts = ToastStack::default();
        NotificationRouter::route(
            &osc777_req("from osc777"),
            &config,
            true,
            &mut toasts,
            &PlainSink::nowhere(),
        );
        assert_eq!(toasts.len(), 1, "osc_777 = true must still route");
    }

    #[test]
    fn osc_9_disabled_does_not_suppress_osc777_sourced_request() {
        let config = NotificationsConfig {
            enabled: true,
            osc_9: false,
            osc_777: true,
            routing_osc_text: NotificationRouting::Toast,
            ..NotificationsConfig::default()
        };
        let mut toasts = ToastStack::default();
        NotificationRouter::route(
            &osc777_req("hello"),
            &config,
            true,
            &mut toasts,
            &PlainSink::nowhere(),
        );
        assert_eq!(
            toasts.len(),
            1,
            "the osc_9 toggle must not affect an Osc777-sourced request"
        );
    }

    #[test]
    fn no_source_request_is_unaffected_by_osc_toggles() {
        // Command-finished / Test-button requests carry `source: None` and
        // must never be suppressed by the OSC 9/777 kill-switches.
        let config = NotificationsConfig {
            enabled: false,
            osc_9: false,
            osc_777: false,
            routing_osc_text: NotificationRouting::Toast,
            ..NotificationsConfig::default()
        };
        let mut toasts = ToastStack::default();
        NotificationRouter::route_test(
            &NotificationRequest::sample(),
            &config,
            true,
            &mut toasts,
            &PlainSink::nowhere(),
        );
        assert_eq!(toasts.len(), 1);
    }

    // ── Freminal-derived toast gate (issue #433, subtask C) ──────────────

    #[test]
    fn freminal_toast_routes_when_enabled() {
        let config = NotificationsConfig::default();
        let mut toasts = ToastStack::default();
        NotificationRouter::route_freminal_toast(
            FreminalToastCategory::Layout,
            ToastKind::Info,
            "Layout saved",
            None,
            crate::gui::toast::ToastPlacement::TOP_RIGHT,
            &config,
            &mut toasts,
        );
        assert_eq!(toasts.len(), 1);
    }

    #[test]
    fn freminal_toast_suppressed_when_disabled() {
        let config = NotificationsConfig {
            routing_layout: FreminalToastRouting::Disabled,
            ..NotificationsConfig::default()
        };
        let mut toasts = ToastStack::default();
        NotificationRouter::route_freminal_toast(
            FreminalToastCategory::Layout,
            ToastKind::Info,
            "Layout saved",
            None,
            crate::gui::toast::ToastPlacement::TOP_RIGHT,
            &config,
            &mut toasts,
        );
        assert_eq!(toasts.len(), 0);
    }

    #[test]
    fn freminal_toast_ignores_enabled_master_switch() {
        // Freminal toasts are governed only by their `routing_<category>`
        // field, not by `NotificationsConfig::enabled` — proving these
        // bypass the master switch by design.
        let config = NotificationsConfig {
            enabled: false,
            routing_layout: FreminalToastRouting::Toast,
            ..NotificationsConfig::default()
        };
        let mut toasts = ToastStack::default();
        NotificationRouter::route_freminal_toast(
            FreminalToastCategory::Layout,
            ToastKind::Info,
            "Layout saved",
            None,
            crate::gui::toast::ToastPlacement::TOP_RIGHT,
            &config,
            &mut toasts,
        );
        assert_eq!(toasts.len(), 1);
    }

    #[test]
    fn freminal_toast_kind_maps_to_severity() {
        let config = NotificationsConfig::default();
        let mut toasts = ToastStack::default();
        NotificationRouter::route_freminal_toast(
            FreminalToastCategory::PasteBlocked,
            ToastKind::Warning,
            "Paste blocked",
            None,
            crate::gui::toast::ToastPlacement::TOP_RIGHT,
            &config,
            &mut toasts,
        );
        assert_eq!(toasts.len(), 1);
        // Assert the severity actually maps to Warning (not just that *a*
        // toast was pushed) — guards `route_freminal_toast`'s kind match.
        assert_eq!(toasts.last_kind(), Some(ToastKind::Warning));
    }

    fn finished_block(exit_code: Option<i32>, dur_secs: u64) -> CommandBlock {
        use freminal_common::buffer_states::command_block::CommandBlockId;
        use std::time::{Duration, SystemTime};
        let executed = SystemTime::now();
        CommandBlock {
            id: CommandBlockId::next(),
            fid: "t".to_owned(),
            prompt_start_row: RowNumber::ZERO,
            command_start_row: Some(RowNumber::new(0)),
            output_start_row: Some(RowNumber::new(0)),
            end_row: Some(RowNumber::new(1)),
            exit_code,
            cwd: None,
            started_at: executed,
            executed_at: Some(executed),
            finished_at: Some(executed + Duration::from_secs(dur_secs)),
        }
    }

    fn enabled_config(threshold: f32) -> NotificationsConfig {
        NotificationsConfig {
            enabled: true,
            command_finished_threshold_secs: threshold,
            ..NotificationsConfig::default()
        }
    }

    #[test]
    fn command_finished_request_gated_by_disabled() {
        let block = finished_block(Some(0), 30);
        let mut config = enabled_config(1.0);
        config.enabled = false;
        assert!(command_finished_request(&block, "ls", "tab", &config).is_none());
    }

    #[test]
    fn command_finished_request_gated_by_on_command_finished() {
        let block = finished_block(Some(0), 30);
        let mut config = enabled_config(1.0);
        config.on_command_finished = false;
        assert!(command_finished_request(&block, "ls", "tab", &config).is_none());
    }

    #[test]
    fn command_finished_request_gated_by_threshold() {
        // 2s command, 10s threshold -> suppressed.
        let block = finished_block(Some(0), 2);
        let config = enabled_config(10.0);
        assert!(command_finished_request(&block, "ls", "tab", &config).is_none());
    }

    #[test]
    fn command_finished_request_running_block_is_none() {
        use freminal_common::buffer_states::command_block::CommandBlockId;
        let running = CommandBlock {
            id: CommandBlockId::next(),
            fid: "t".to_owned(),
            prompt_start_row: RowNumber::ZERO,
            command_start_row: Some(RowNumber::new(0)),
            output_start_row: Some(RowNumber::new(0)),
            end_row: None,
            exit_code: None,
            cwd: None,
            started_at: std::time::SystemTime::now(),
            executed_at: Some(std::time::SystemTime::now()),
            finished_at: None,
        };
        let config = enabled_config(0.0);
        assert!(command_finished_request(&running, "ls", "tab", &config).is_none());
    }

    #[test]
    fn command_finished_request_unexecuted_ctrl_c_block_is_none() {
        // Regression for 106.3: Ctrl-C on an idle prompt produces a finished
        // block (A -> D) that never executed a command (no `OSC 133 C`, so
        // executed_at / output_start_row are None).  Even at a zero threshold
        // — which would let `duration()` of an executed instant command
        // through — no notification must fire.
        use freminal_common::buffer_states::command_block::CommandBlockId;
        use std::time::{Duration, SystemTime};
        let started = SystemTime::now();
        let ctrl_c = CommandBlock {
            id: CommandBlockId::next(),
            fid: "t".to_owned(),
            prompt_start_row: RowNumber::ZERO,
            command_start_row: Some(RowNumber::new(0)),
            // No C marker: the user aborted before any command ran.
            output_start_row: None,
            end_row: Some(RowNumber::new(0)),
            exit_code: Some(130),
            cwd: None,
            // User idled 30s at the prompt before Ctrl-C.
            started_at: started,
            executed_at: None,
            finished_at: Some(started + Duration::from_secs(30)),
        };
        let config = enabled_config(0.0);
        assert!(
            command_finished_request(&ctrl_c, "", "tab", &config).is_none(),
            "an aborted prompt that never executed a command must not notify"
        );
    }

    #[test]
    fn command_finished_request_success_is_command_finished_kind() {
        let block = finished_block(Some(0), 30);
        let config = enabled_config(1.0);
        let req = command_finished_request(&block, "cargo build", "tab", &config).expect("request");
        assert_eq!(req.kind, NotificationKind::CommandFinished);
        assert!(req.body.contains("cargo build"), "body: {}", req.body);
        assert!(req.body.contains("finished in"), "body: {}", req.body);
    }

    #[test]
    fn command_finished_request_failure_is_error_kind() {
        let block = finished_block(Some(127), 30);
        let config = enabled_config(1.0);
        let req = command_finished_request(&block, "nope", "tab", &config).expect("request");
        // A non-zero exit drives the Error kind regardless of the template
        // body (which uses {exit_code} rather than a "failed" verb).
        assert_eq!(req.kind, NotificationKind::Error);
        assert!(req.body.contains("exit 127"), "body: {}", req.body);
        assert!(req.body.contains("nope"), "body: {}", req.body);
    }

    #[test]
    fn command_finished_request_empty_command_uses_placeholder() {
        let block = finished_block(Some(0), 30);
        let config = enabled_config(1.0);
        let req = command_finished_request(&block, "   ", "tab", &config).expect("request");
        assert!(req.body.starts_with("Command "), "body: {}", req.body);
    }

    #[test]
    fn system_when_unfocused_pushes_toast_only_when_focused() {
        let config = NotificationsConfig {
            enabled: true,
            routing_osc_text: NotificationRouting::SystemWhenUnfocused,
            ..NotificationsConfig::default()
        };

        let mut focused_toasts = ToastStack::default();
        NotificationRouter::route(
            &osc_req("hi"),
            &config,
            true,
            &mut focused_toasts,
            &PlainSink::nowhere(),
        );
        assert_eq!(focused_toasts.len(), 1, "focused -> toast");

        let mut unfocused_toasts = ToastStack::default();
        NotificationRouter::route(
            &osc_req("hi"),
            &config,
            false,
            &mut unfocused_toasts,
            &PlainSink::nowhere(),
        );
        assert_eq!(unfocused_toasts.len(), 0, "unfocused -> system, no toast");
    }

    fn block_with_cwd(exit_code: Option<i32>, dur_secs: u64, cwd: Option<&str>) -> CommandBlock {
        use freminal_common::buffer_states::command_block::CommandBlockId;
        use std::time::{Duration, SystemTime};
        let executed = SystemTime::now();
        CommandBlock {
            id: CommandBlockId::next(),
            fid: "t".to_owned(),
            prompt_start_row: RowNumber::ZERO,
            command_start_row: Some(RowNumber::new(0)),
            output_start_row: Some(RowNumber::new(0)),
            end_row: Some(RowNumber::new(1)),
            exit_code,
            cwd: cwd.map(str::to_owned),
            started_at: executed,
            executed_at: Some(executed),
            finished_at: Some(executed + Duration::from_secs(dur_secs)),
        }
    }

    #[test]
    fn template_substitutes_all_tokens() {
        let block = block_with_cwd(Some(0), 30, Some("/home/fred"));
        let body = render_command_finished_template(
            "{command} | {duration} | {exit_code} | {cwd} | {tab_name}",
            &block,
            "cargo build",
            "work",
        );
        assert_eq!(body, "cargo build | 30s | 0 | /home/fred | work");
    }

    #[test]
    fn template_empty_command_falls_back_to_placeholder() {
        let block = block_with_cwd(Some(0), 5, None);
        let body = render_command_finished_template("{command}", &block, "   ", "tab");
        assert_eq!(body, "Command");
    }

    #[test]
    fn template_unknown_exit_code_renders_question_mark() {
        // Shell omitted the exit code.
        let block = block_with_cwd(None, 5, None);
        let body = render_command_finished_template("exit {exit_code}", &block, "ls", "tab");
        assert_eq!(body, "exit ?");
    }

    #[test]
    fn template_unknown_cwd_renders_empty() {
        let block = block_with_cwd(Some(0), 5, None);
        let body = render_command_finished_template("[{cwd}]", &block, "ls", "tab");
        assert_eq!(body, "[]");
    }

    #[test]
    fn template_unknown_token_is_left_untouched() {
        let block = block_with_cwd(Some(0), 5, None);
        let body = render_command_finished_template("{command} {unknown}", &block, "ls", "tab");
        assert_eq!(body, "ls {unknown}");
    }

    #[test]
    fn command_finished_request_uses_custom_template() {
        let block = block_with_cwd(Some(0), 30, Some("/srv"));
        let config = NotificationsConfig {
            command_finished_template: "{tab_name}: {command} ({cwd})".to_owned(),
            ..enabled_config(1.0)
        };
        let req = command_finished_request(&block, "make", "build", &config).expect("request");
        assert_eq!(req.body, "build: make (/srv)");
        assert_eq!(req.kind, NotificationKind::CommandFinished);
    }

    #[test]
    fn default_template_matches_documented_format() {
        let block = block_with_cwd(Some(2), 30, None);
        let config = enabled_config(1.0);
        let req = command_finished_request(&block, "test", "tab", &config).expect("request");
        // Default: "{command} finished in {duration} (exit {exit_code})".
        assert_eq!(req.body, "test finished in 30s (exit 2)");
        // Non-zero exit -> Error kind.
        assert_eq!(req.kind, NotificationKind::Error);
    }

    // ── OSC 99 (Task 99.5a) ──────────────────────────────────────────

    fn default_n99() -> Notification99Data {
        Notification99Data {
            id: None,
            title: "Title".to_owned(),
            body: Some("Body".to_owned()),
            icon_data: None,
            icon_names: Vec::new(),
            icon_cache_key: None,
            buttons: Vec::new(),
            activation_reporting: Osc99ActivationReporting::Silent,
            activation_focus: Osc99ActivationFocus::Focus,
            close_reporting: Osc99CloseReporting::Silent,
            urgency: NotificationUrgency::Normal,
            occasion: NotificationOccasion::Always,
            sound: Osc99Sound::System,
            app_name: None,
            notification_types: Vec::new(),
            expiry: Osc99Expiry::OsDefault,
        }
    }

    /// A display request for `data` from the first pane of window 1, with no
    /// reply handle (these tests never read replies).
    fn osc99_request(data: &Notification99Data) -> Osc99DisplayRequest {
        Osc99DisplayRequest {
            data: data.clone(),
            reply_tx: Weak::new(),
            origin: Osc99Origin {
                window_id: freminal_windowing::WindowId::for_test(1),
                pane_id: crate::gui::panes::PaneId::first(),
            },
            pane_state: pane_state(PaneKeyboardFocus::Focused, PaneVisibility::Visible),
        }
    }

    const fn pane_state(focus: PaneKeyboardFocus, visibility: PaneVisibility) -> Osc99PaneState {
        Osc99PaneState { focus, visibility }
    }

    fn osc99_ctx(window_focus: WindowFocus, pane: Osc99PaneState) -> Osc99DisplayContext {
        Osc99DisplayContext {
            window_focus,
            pane,
            now: Instant::now(),
            daemon: LinuxDaemon::assumed(),
        }
    }

    /// A focused window whose originating pane is the focused, visible one.
    fn focused_ctx() -> Osc99DisplayContext {
        osc99_ctx(
            WindowFocus::Focused,
            pane_state(PaneKeyboardFocus::Focused, PaneVisibility::Visible),
        )
    }

    #[test]
    fn route_osc99_master_gate_disabled_does_nothing() {
        let config = NotificationsConfig::default(); // enabled = false
        let mut toasts = ToastStack::default();
        let mut icon_cache = Osc99IconCache::default();
        let mut registry = Osc99Registry::default();
        let data = default_n99();

        route_osc99_into(
            &osc99_request(&data),
            &config,
            focused_ctx(),
            &mut toasts,
            &mut icon_cache,
            &mut registry,
        );

        assert_eq!(toasts.len(), 0);
        assert!(registry.instances().is_empty());
    }

    #[test]
    fn route_osc99_osc99_kill_switch_disabled_does_nothing() {
        // Task 99.8: `enabled = true` but `osc_99 = false` must still
        // suppress OSC 99 notifications specifically.
        let config = NotificationsConfig {
            enabled: true,
            osc_99: false,
            ..NotificationsConfig::default()
        };
        let mut toasts = ToastStack::default();
        let mut icon_cache = Osc99IconCache::default();
        let mut registry = Osc99Registry::default();
        let data = default_n99();

        route_osc99_into(
            &osc99_request(&data),
            &config,
            focused_ctx(),
            &mut toasts,
            &mut icon_cache,
            &mut registry,
        );

        assert_eq!(toasts.len(), 0);
        assert!(registry.instances().is_empty());
    }

    #[test]
    fn route_osc99_occasion_always_displays_regardless_of_focus() {
        let config = enabled_config(0.0);
        let mut toasts = ToastStack::default();
        let mut icon_cache = Osc99IconCache::default();
        let mut registry = Osc99Registry::default();
        let data = default_n99(); // occasion: Always

        route_osc99_into(
            &osc99_request(&data),
            &config,
            focused_ctx(),
            &mut toasts,
            &mut icon_cache,
            &mut registry,
        );

        assert_eq!(
            toasts.len(),
            1,
            "Always occasion must display even when focused"
        );
    }

    /// Route `data` with `ctx` into a fresh toast stack and return how many
    /// toasts it produced.
    fn route_and_count_toasts(data: &Notification99Data, ctx: Osc99DisplayContext) -> usize {
        let config = enabled_config(0.0);
        let mut toasts = ToastStack::default();
        let mut icon_cache = Osc99IconCache::default();
        let mut registry = Osc99Registry::default();
        route_osc99_into(
            &osc99_request(data),
            &config,
            ctx,
            &mut toasts,
            &mut icon_cache,
            &mut registry,
        );
        toasts.len()
    }

    #[test]
    fn route_osc99_occasion_unfocused_gates_on_pane_focus() {
        let data = Notification99Data {
            occasion: NotificationOccasion::Unfocused,
            ..default_n99()
        };

        assert_eq!(
            route_and_count_toasts(&data, focused_ctx()),
            0,
            "unfocused occasion must not fire for the focused pane"
        );

        let background_tab = osc99_ctx(
            WindowFocus::Focused,
            pane_state(PaneKeyboardFocus::Unfocused, PaneVisibility::Hidden),
        );
        assert_eq!(
            route_and_count_toasts(&data, background_tab),
            1,
            "unfocused occasion must fire for a background-tab pane of a focused window"
        );
    }

    #[test]
    fn route_osc99_occasion_invisible_gates_on_pane_visibility() {
        let data = Notification99Data {
            occasion: NotificationOccasion::Invisible,
            ..default_n99()
        };

        let inactive_but_visible = osc99_ctx(
            WindowFocus::Focused,
            pane_state(PaneKeyboardFocus::Unfocused, PaneVisibility::Visible),
        );
        assert_eq!(
            route_and_count_toasts(&data, inactive_but_visible),
            0,
            "invisible occasion must not fire for a pane that is on screen"
        );

        let hidden = osc99_ctx(
            WindowFocus::Focused,
            pane_state(PaneKeyboardFocus::Unfocused, PaneVisibility::Hidden),
        );
        assert_eq!(
            route_and_count_toasts(&data, hidden),
            1,
            "invisible occasion must fire for a hidden pane"
        );
    }

    #[test]
    fn route_osc99_routing_still_follows_os_window_focus_not_pane_state() {
        // Default routing is focus-aware; the pane state must not influence
        // which leg is chosen, only the OS-window focus does. A focused
        // window with an unfocused hidden pane still wants a toast.
        let data = default_n99();
        let ctx = osc99_ctx(
            WindowFocus::Focused,
            pane_state(PaneKeyboardFocus::Unfocused, PaneVisibility::Hidden),
        );
        assert_eq!(route_and_count_toasts(&data, ctx), 1);
    }

    // The `o=` decision table: (occasion, focus, visibility) -> decision.
    #[test]
    fn occasion_allows_display_decision_table() {
        use NotificationOccasion::{Always, Invisible, Unfocused};
        use Osc99DisplayDecision::{Show, Suppress};
        use PaneKeyboardFocus::{Focused, Unfocused as NoFocus};
        use PaneVisibility::{Hidden, Visible};

        let table = [
            // Always shows in every state.
            (Always, Focused, Visible, Show),
            (Always, Focused, Hidden, Show),
            (Always, NoFocus, Visible, Show),
            (Always, NoFocus, Hidden, Show),
            // `unfocused`: active pane of a focused window suppresses; a
            // background tab or inactive pane shows, visible or not.
            (Unfocused, Focused, Visible, Suppress),
            (Unfocused, Focused, Hidden, Suppress),
            (Unfocused, NoFocus, Visible, Show),
            (Unfocused, NoFocus, Hidden, Show),
            // `invisible`: only an unfocused, hidden pane shows. A
            // non-active pane in the active tab is visible -> suppressed; a
            // pane hidden by another pane's zoom, or by an unfocused window,
            // shows.
            (Invisible, Focused, Visible, Suppress),
            (Invisible, Focused, Hidden, Suppress),
            (Invisible, NoFocus, Visible, Suppress),
            (Invisible, NoFocus, Hidden, Show),
        ];
        for (occasion, focus, visibility, expected) in table {
            assert_eq!(
                occasion_allows_display(occasion, pane_state(focus, visibility)),
                expected,
                "{occasion:?} / {focus:?} / {visibility:?}"
            );
        }
    }

    #[test]
    fn occasion_allows_display_matches_the_named_scenarios() {
        use crate::gui::osc99::{PaneActivity, TabActivity, WindowMinimized, ZoomOcclusion};
        let state = |window, tab, pane, zoom| {
            Osc99PaneState::compute(window, WindowMinimized::Restored, tab, pane, zoom)
        };
        let nz = ZoomOcclusion::NotOccluded;

        // unfocused: background tab of a focused window -> Show.
        assert_eq!(
            occasion_allows_display(
                NotificationOccasion::Unfocused,
                state(
                    WindowFocus::Focused,
                    TabActivity::Background,
                    PaneActivity::Active,
                    nz
                )
            ),
            Osc99DisplayDecision::Show
        );
        // unfocused: active pane of a focused window -> Suppress.
        assert_eq!(
            occasion_allows_display(
                NotificationOccasion::Unfocused,
                state(
                    WindowFocus::Focused,
                    TabActivity::Active,
                    PaneActivity::Active,
                    nz
                )
            ),
            Osc99DisplayDecision::Suppress
        );
        // invisible: non-active pane in the active tab, no zoom -> Suppress.
        assert_eq!(
            occasion_allows_display(
                NotificationOccasion::Invisible,
                state(
                    WindowFocus::Focused,
                    TabActivity::Active,
                    PaneActivity::Inactive,
                    nz
                )
            ),
            Osc99DisplayDecision::Suppress
        );
        // invisible: pane hidden by another pane's zoom -> Show.
        assert_eq!(
            occasion_allows_display(
                NotificationOccasion::Invisible,
                state(
                    WindowFocus::Focused,
                    TabActivity::Active,
                    PaneActivity::Inactive,
                    ZoomOcclusion::Occluded
                )
            ),
            Osc99DisplayDecision::Show
        );
        // invisible: window unfocused -> Show.
        assert_eq!(
            occasion_allows_display(
                NotificationOccasion::Invisible,
                state(
                    WindowFocus::Unfocused,
                    TabActivity::Active,
                    PaneActivity::Active,
                    nz
                )
            ),
            Osc99DisplayDecision::Show
        );
    }

    #[test]
    fn route_osc99_icon_cache_populated_from_transmitted_bytes() {
        let mut icon_cache = Osc99IconCache::default();
        let data = Notification99Data {
            icon_data: Some(vec![1, 2, 3, 4]),
            icon_cache_key: Some("k".to_owned()),
            ..default_n99()
        };

        let resolved = NotificationRouter::resolve_icon_bytes(&data, &mut icon_cache);

        assert!(
            matches!(&resolved, Some(Cow::Borrowed(bytes)) if *bytes == [1u8, 2, 3, 4]),
            "transmitted bytes are borrowed, not copied: {resolved:?}"
        );
        drop(resolved);
        assert_eq!(icon_cache.get("k"), Some([1u8, 2, 3, 4].as_slice()));
    }

    #[test]
    fn route_osc99_icon_cache_reused_on_subsequent_g_only_notification() {
        let mut icon_cache = Osc99IconCache::default();
        icon_cache.insert("k".to_owned(), vec![9, 9, 9]);

        let data = Notification99Data {
            icon_data: None,
            icon_cache_key: Some("k".to_owned()),
            ..default_n99()
        };

        let resolved = NotificationRouter::resolve_icon_bytes(&data, &mut icon_cache);

        assert!(
            matches!(&resolved, Some(Cow::Borrowed(bytes)) if *bytes == [9u8, 9, 9]),
            "cached bytes are borrowed, not copied: {resolved:?}"
        );
    }

    #[test]
    fn route_osc99_hands_transmitted_icon_bytes_to_the_legs_without_copying_them() {
        let mut icon_cache = Osc99IconCache::default();
        let mut registry = Osc99Registry::default();
        let data = Notification99Data {
            icon_data: Some(vec![7, 8, 9]),
            ..default_n99()
        };
        let original = data.icon_data.as_ref().map(Vec::as_ptr);
        let mut request = osc99_request(&default_n99());
        request.data = data;

        let effects = NotificationRouter::route_osc99(
            request,
            &NotificationsConfig {
                enabled: true,
                routing_osc99: NotificationRouting::System,
                ..NotificationsConfig::default()
            },
            focused_ctx(),
            &mut icon_cache,
            &mut registry,
        );

        let shown = effects.iter().find_map(|effect| match effect {
            Osc99Effect::ShowLinuxSystem { notification, .. }
            | Osc99Effect::ShowUntrackedSystem(notification) => Some(notification),
            _ => None,
        });
        let Some(shown) = shown else {
            panic!("a system leg must show the notification, got {effects:?}");
        };
        assert_eq!(
            shown.icon_data.as_ref().map(Vec::as_ptr),
            original,
            "the caller's allocation is passed on, not a copy"
        );
    }

    #[test]
    fn route_osc99_icon_cache_miss_returns_none() {
        let mut icon_cache = Osc99IconCache::default();
        let data = Notification99Data {
            icon_data: None,
            icon_cache_key: Some("missing".to_owned()),
            ..default_n99()
        };

        assert!(NotificationRouter::resolve_icon_bytes(&data, &mut icon_cache).is_none());
    }

    #[test]
    fn route_osc99_no_icon_data_or_key_returns_none() {
        let mut icon_cache = Osc99IconCache::default();
        let data = default_n99();

        assert!(NotificationRouter::resolve_icon_bytes(&data, &mut icon_cache).is_none());
    }

    #[test]
    fn route_osc99_registers_instance_with_reporting_flags_when_id_present() {
        let config = enabled_config(0.0);
        let mut toasts = ToastStack::default();
        let mut icon_cache = Osc99IconCache::default();
        let mut registry = Osc99Registry::default();
        let data = Notification99Data {
            id: Some("n1".to_owned()),
            activation_reporting: Osc99ActivationReporting::Report,
            close_reporting: Osc99CloseReporting::Report,
            ..default_n99()
        };

        route_osc99_into(
            &osc99_request(&data),
            &config,
            focused_ctx(),
            &mut toasts,
            &mut icon_cache,
            &mut registry,
        );

        let [entry] = registry.instances() else {
            panic!("exactly one instance must be registered");
        };
        assert_eq!(entry.id(), Some("n1"));
        assert_eq!(
            entry.activation_reporting(),
            Osc99ActivationReporting::Report
        );
        assert_eq!(entry.close_reporting(), Osc99CloseReporting::Report);
        assert_ne!(entry.open_legs(), []);
    }

    #[test]
    fn route_osc99_registers_id_less_instance() {
        let config = enabled_config(0.0);
        let mut toasts = ToastStack::default();
        let mut icon_cache = Osc99IconCache::default();
        let mut registry = Osc99Registry::default();
        let data = default_n99(); // id: None

        route_osc99_into(
            &osc99_request(&data),
            &config,
            focused_ctx(),
            &mut toasts,
            &mut icon_cache,
            &mut registry,
        );

        let [entry] = registry.instances() else {
            panic!("an id-less notification is still registered");
        };
        assert_eq!(entry.id(), None);
    }

    #[test]
    fn route_osc99_title_and_body_push_a_toast() {
        let config = enabled_config(0.0);
        let mut toasts = ToastStack::default();
        let mut icon_cache = Osc99IconCache::default();
        let mut registry = Osc99Registry::default();
        let data = default_n99();

        route_osc99_into(
            &osc99_request(&data),
            &config,
            focused_ctx(),
            &mut toasts,
            &mut icon_cache,
            &mut registry,
        );

        assert_eq!(toasts.len(), 1);
    }

    #[test]
    fn route_osc99_occasion_suppressed_registers_nothing() {
        // An occasion-suppressed notification is not displayed, so it must
        // not be tracked for update/close either -- as far as later
        // reconciliation is concerned, it never happened.
        let config = enabled_config(0.0);
        let mut toasts = ToastStack::default();
        let mut icon_cache = Osc99IconCache::default();
        let mut registry = Osc99Registry::default();
        let data = Notification99Data {
            id: Some("n2".to_owned()),
            occasion: NotificationOccasion::Unfocused,
            ..default_n99()
        };

        route_osc99_into(
            &osc99_request(&data),
            &config,
            focused_ctx(),
            &mut toasts,
            &mut icon_cache,
            &mut registry,
        );

        assert_eq!(toasts.len(), 0);
        assert!(registry.instances().is_empty());
    }

    /// Install the compiled `rules` as `registry`'s user filter.
    fn set_filter(registry: &mut Osc99Registry, rules: &[&str]) {
        let owned: Vec<String> = rules.iter().map(|r| (*r).to_owned()).collect();
        let (filter, errors) = Osc99Filter::compile(&owned);
        assert_eq!(errors, Vec::new(), "unexpected filter errors");
        registry.set_filter(filter);
    }

    #[test]
    fn route_osc99_filtered_notification_produces_no_effects_and_registers_nothing() {
        let config = enabled_config(0.0);
        let mut icon_cache = Osc99IconCache::default();
        let mut registry = Osc99Registry::default();
        set_filter(&mut registry, &["title:^Spam"]);
        let data = Notification99Data {
            id: Some("filtered".to_owned()),
            title: "Spam offer".to_owned(),
            // Even a notification that asks to be reported on gets nothing.
            activation_reporting: Osc99ActivationReporting::Report,
            close_reporting: Osc99CloseReporting::Report,
            ..default_n99()
        };

        let effects = NotificationRouter::route_osc99(
            osc99_request(&data),
            &config,
            focused_ctx(),
            &mut icon_cache,
            &mut registry,
        );

        assert!(
            effects.is_empty(),
            "a filtered notification yields no toast and no reply: {effects:?}"
        );
        assert!(registry.instances().is_empty());
        let alive = registry.alive(osc99_request(&data).origin, "poll".to_owned(), Weak::new());
        let [
            Osc99Effect::Reply {
                reply: GuiReply::Osc99Alive { live_ids, .. },
                ..
            },
        ] = alive.as_slice()
        else {
            panic!("p=alive answers with exactly one alive reply: {alive:?}");
        };
        assert!(live_ids.is_empty(), "a filtered notification is not alive");
    }

    #[test]
    fn route_osc99_unfiltered_notification_still_displays_beside_a_filtered_one() {
        let config = enabled_config(0.0);
        let mut toasts = ToastStack::default();
        let mut icon_cache = Osc99IconCache::default();
        let mut registry = Osc99Registry::default();
        set_filter(&mut registry, &["title:^Spam"]);
        let spam = Notification99Data {
            id: Some("spam".to_owned()),
            title: "Spam offer".to_owned(),
            ..default_n99()
        };
        let wanted = Notification99Data {
            id: Some("wanted".to_owned()),
            title: "Build finished".to_owned(),
            ..default_n99()
        };

        for data in [&spam, &wanted] {
            route_osc99_into(
                &osc99_request(data),
                &config,
                focused_ctx(),
                &mut toasts,
                &mut icon_cache,
                &mut registry,
            );
        }

        assert_eq!(toasts.len(), 1, "only the unfiltered notification shows");
        let [entry] = registry.instances() else {
            panic!("only the unfiltered notification is registered");
        };
        assert_eq!(entry.id(), Some("wanted"));
    }

    #[test]
    fn route_osc99_filter_runs_after_the_occasion_check_and_the_kill_switches() {
        // A `Drop` verdict must not change what a non-displayed notification
        // does today: the master switch and kill-switch still short-circuit.
        let config = NotificationsConfig {
            osc_99: false,
            ..enabled_config(0.0)
        };
        let mut toasts = ToastStack::default();
        let mut icon_cache = Osc99IconCache::default();
        let mut registry = Osc99Registry::default();
        set_filter(&mut registry, &["all"]);

        route_osc99_into(
            &osc99_request(&default_n99()),
            &config,
            focused_ctx(),
            &mut toasts,
            &mut icon_cache,
            &mut registry,
        );

        assert_eq!(toasts.len(), 0);
        assert!(registry.instances().is_empty());
    }

    #[test]
    fn route_osc99_replacing_the_filter_takes_effect_for_the_next_notification() {
        let config = enabled_config(0.0);
        let mut toasts = ToastStack::default();
        let mut icon_cache = Osc99IconCache::default();
        let mut registry = Osc99Registry::default();
        let data = default_n99();

        set_filter(&mut registry, &["all"]);
        route_osc99_into(
            &osc99_request(&data),
            &config,
            focused_ctx(),
            &mut toasts,
            &mut icon_cache,
            &mut registry,
        );
        assert_eq!(toasts.len(), 0);

        registry.set_filter(Osc99Filter::default());
        route_osc99_into(
            &osc99_request(&data),
            &config,
            focused_ctx(),
            &mut toasts,
            &mut icon_cache,
            &mut registry,
        );
        assert_eq!(toasts.len(), 1);
    }

    #[test]
    fn route_osc99_routing_disabled_pushes_no_toast_and_registers_nothing() {
        // 3.3/3.4: `routing_osc99 = Disabled` must suppress both legs, and
        // since nothing was displayed, the notification must not be tracked
        // in the registry either.
        let config = NotificationsConfig {
            routing_osc99: NotificationRouting::Disabled,
            ..enabled_config(0.0)
        };
        let mut toasts = ToastStack::default();
        let mut icon_cache = Osc99IconCache::default();
        let mut registry = Osc99Registry::default();
        let data = Notification99Data {
            id: Some("n3".to_owned()),
            ..default_n99()
        };

        route_osc99_into(
            &osc99_request(&data),
            &config,
            focused_ctx(),
            &mut toasts,
            &mut icon_cache,
            &mut registry,
        );

        assert_eq!(toasts.len(), 0);
        assert!(registry.instances().is_empty());
    }

    #[test]
    fn route_osc99_routing_toast_pushes_a_toast() {
        let config = NotificationsConfig {
            routing_osc99: NotificationRouting::Toast,
            ..enabled_config(0.0)
        };
        let mut toasts = ToastStack::default();
        let mut icon_cache = Osc99IconCache::default();
        let mut registry = Osc99Registry::default();
        let data = default_n99();

        route_osc99_into(
            &osc99_request(&data),
            &config,
            focused_ctx(),
            &mut toasts,
            &mut icon_cache,
            &mut registry,
        );

        assert_eq!(toasts.len(), 1);
    }

    #[test]
    fn route_osc99_toast_only_routing_registers_a_toast_leg() {
        // A toast-only notification carrying an `id` is registered with
        // just the toast leg, so `p=alive` lists it and `p=close` can find
        // it. (Before the registry it was deliberately not tracked, because
        // nothing could reconcile a toast with the live map.)
        let config = NotificationsConfig {
            routing_osc99: NotificationRouting::Toast,
            ..enabled_config(0.0)
        };
        let mut toasts = ToastStack::default();
        let mut icon_cache = Osc99IconCache::default();
        let mut registry = Osc99Registry::default();
        let data = Notification99Data {
            id: Some("toast-only".to_owned()),
            ..default_n99()
        };

        route_osc99_into(
            &osc99_request(&data),
            &config,
            focused_ctx(),
            &mut toasts,
            &mut icon_cache,
            &mut registry,
        );

        assert_eq!(toasts.len(), 1, "toast leg should still fire");
        let [entry] = registry.instances() else {
            panic!("a toast-only notification must be registered");
        };
        assert_eq!(entry.id(), Some("toast-only"));
        assert_eq!(entry.open_legs(), [Osc99Leg::Toast]);
    }

    #[test]
    fn route_osc99_system_leg_is_registered_with_the_platform_leg() {
        // The complementary case to the toast-only test: default `Both`
        // routing fires the system leg, so an id-bearing notification IS
        // registered with both the toast and the platform's system leg.
        let config = NotificationsConfig {
            routing_osc99: NotificationRouting::Both,
            ..enabled_config(0.0)
        };
        let mut toasts = ToastStack::default();
        let mut icon_cache = Osc99IconCache::default();
        let mut registry = Osc99Registry::default();
        let data = Notification99Data {
            id: Some("tracked".to_owned()),
            ..default_n99()
        };

        route_osc99_into(
            &osc99_request(&data),
            &config,
            focused_ctx(),
            &mut toasts,
            &mut icon_cache,
            &mut registry,
        );

        let system_leg = match SYSTEM_LEG {
            SystemLeg::Tracked => Osc99Leg::LinuxSystem,
            SystemLeg::Untracked => Osc99Leg::UntrackedSystem,
        };
        let [entry] = registry.instances() else {
            panic!("a system-leg notification must be registered");
        };
        assert_eq!(entry.id(), Some("tracked"));
        assert_eq!(entry.shown_legs(), [Osc99Leg::Toast, system_leg]);
    }

    const NO_DAEMON: LinuxDaemon = LinuxDaemon::Unavailable;

    #[test]
    fn legs_follow_routing_when_the_daemon_is_available() {
        let daemon = LinuxDaemon::Available(freminal_notify::DaemonCapabilities::ASSUMED);
        let legs =
            |routing, focus| resolve_osc99_legs(routing, focus, Osc99Leg::LinuxSystem, daemon);
        assert_eq!(
            legs(NotificationRouting::Both, WindowFocus::Focused),
            [Osc99Leg::Toast, Osc99Leg::LinuxSystem]
        );
        assert_eq!(
            legs(NotificationRouting::System, WindowFocus::Focused),
            [Osc99Leg::LinuxSystem]
        );
        assert_eq!(
            legs(
                NotificationRouting::SystemWhenUnfocused,
                WindowFocus::Unfocused
            ),
            [Osc99Leg::LinuxSystem]
        );
        assert_eq!(
            legs(
                NotificationRouting::SystemWhenUnfocused,
                WindowFocus::Focused
            ),
            [Osc99Leg::Toast]
        );
        assert_eq!(
            legs(NotificationRouting::Disabled, WindowFocus::Focused),
            Vec::<Osc99Leg>::new()
        );
    }

    #[test]
    fn unfocused_system_when_unfocused_falls_back_to_the_toast_with_the_bus_down() {
        // G8: the system leg was the only one chosen, and nothing can show it.
        assert_eq!(
            resolve_osc99_legs(
                NotificationRouting::SystemWhenUnfocused,
                WindowFocus::Unfocused,
                Osc99Leg::LinuxSystem,
                NO_DAEMON,
            ),
            [Osc99Leg::Toast]
        );
    }

    #[test]
    fn system_routing_falls_back_to_the_toast_with_the_bus_down() {
        // `System` plus no daemon never reaches routing at runtime (the
        // capability combine makes it `Unsupported`), but routing is still
        // defined for it.
        assert_eq!(
            resolve_osc99_legs(
                NotificationRouting::System,
                WindowFocus::Focused,
                Osc99Leg::LinuxSystem,
                NO_DAEMON,
            ),
            [Osc99Leg::Toast]
        );
    }

    #[test]
    fn both_routing_lists_the_toast_leg_once_with_the_bus_down() {
        assert_eq!(
            resolve_osc99_legs(
                NotificationRouting::Both,
                WindowFocus::Focused,
                Osc99Leg::LinuxSystem,
                NO_DAEMON,
            ),
            [Osc99Leg::Toast]
        );
    }

    #[test]
    fn the_untracked_system_leg_ignores_the_linux_daemon() {
        assert_eq!(
            resolve_osc99_legs(
                NotificationRouting::Both,
                WindowFocus::Focused,
                Osc99Leg::UntrackedSystem,
                NO_DAEMON,
            ),
            [Osc99Leg::Toast, Osc99Leg::UntrackedSystem]
        );
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn route_osc99_with_the_bus_down_registers_and_shows_the_toast_leg_only() {
        use freminal_notify::test_support::RecordingTrackedBackend;

        let config = NotificationsConfig {
            routing_osc99: NotificationRouting::SystemWhenUnfocused,
            ..enabled_config(0.0)
        };
        let mut toasts = ToastStack::default();
        let mut icon_cache = Osc99IconCache::default();
        let mut registry = Osc99Registry::default();
        let mut system = RecordingTrackedBackend::default();
        let data = Notification99Data {
            id: Some("down".to_owned()),
            ..default_n99()
        };
        let ctx = Osc99DisplayContext {
            daemon: NO_DAEMON,
            ..osc99_ctx(
                WindowFocus::Unfocused,
                pane_state(PaneKeyboardFocus::Unfocused, PaneVisibility::Hidden),
            )
        };

        route_osc99_with_system(
            &osc99_request(&data),
            &config,
            ctx,
            &mut toasts,
            &mut icon_cache,
            &mut registry,
            &mut system,
        );

        assert!(system.shown.is_empty(), "no desktop notification is sent");
        assert_eq!(toasts.len(), 1, "the toast leg shows instead");
        let [entry] = registry.instances() else {
            panic!("the notification must be registered");
        };
        assert_eq!(entry.open_legs(), [Osc99Leg::Toast]);
    }

    /// Route `requests` strictly in order, as the app does for a frame, and
    /// collect every effect.
    fn route_in_order(
        requests: &[Osc99Request],
        config: &NotificationsConfig,
        icon_cache: &mut Osc99IconCache,
        registry: &mut Osc99Registry,
    ) -> Vec<Osc99Effect> {
        let mut effects = Vec::new();
        for request in requests {
            effects.extend(NotificationRouter::route_osc99_request(
                request.clone(),
                config,
                WindowFocus::Focused,
                LinuxDaemon::assumed(),
                icon_cache,
                registry,
            ));
        }
        effects
    }

    fn control_request(id: &str, kind: Osc99ControlKind) -> Osc99Request {
        Osc99Request::Control(Osc99ControlRequest {
            control: Osc99Control {
                id: id.to_owned(),
                kind,
            },
            reply_tx: Weak::new(),
            origin: Osc99Origin {
                window_id: freminal_windowing::WindowId::for_test(1),
                pane_id: crate::gui::panes::PaneId::first(),
            },
        })
    }

    fn display_request(id: &str, title: &str) -> Osc99Request {
        Osc99Request::Display(Box::new(osc99_request(&Notification99Data {
            id: Some(id.to_owned()),
            title: title.to_owned(),
            ..default_n99()
        })))
    }

    /// The ids a `p=alive` answer lists, among `effects`.
    fn alive_ids(effects: &[Osc99Effect]) -> Vec<String> {
        let [
            Osc99Effect::Reply {
                reply: GuiReply::Osc99Alive { live_ids, .. },
                ..
            },
        ] = effects
        else {
            panic!("expected exactly one alive reply, got {effects:?}");
        };
        live_ids.clone()
    }

    #[test]
    fn a_close_then_a_new_display_of_the_same_id_in_one_frame_leaves_the_new_one_live() {
        // G1: the close must act on the instance that existed when it was
        // sent, not on the one displayed after it.
        let config = enabled_config(0.0);
        let mut icon_cache = Osc99IconCache::default();
        let mut registry = Osc99Registry::default();
        route_in_order(
            &[display_request("x", "old")],
            &config,
            &mut icon_cache,
            &mut registry,
        );

        route_in_order(
            &[
                control_request("x", Osc99ControlKind::Close),
                display_request("x", "new"),
            ],
            &config,
            &mut icon_cache,
            &mut registry,
        );

        let alive = route_in_order(
            &[control_request("poll", Osc99ControlKind::Alive)],
            &config,
            &mut icon_cache,
            &mut registry,
        );
        assert_eq!(alive_ids(&alive), ["x"], "the new instance is live");
    }

    #[test]
    fn an_alive_poll_does_not_list_a_display_that_follows_it_in_the_frame() {
        let config = enabled_config(0.0);
        let mut icon_cache = Osc99IconCache::default();
        let mut registry = Osc99Registry::default();

        let effects = route_in_order(
            &[
                control_request("poll", Osc99ControlKind::Alive),
                display_request("y", "later"),
            ],
            &config,
            &mut icon_cache,
            &mut registry,
        );

        let alive: Vec<Osc99Effect> = effects
            .into_iter()
            .filter(|effect| matches!(effect, Osc99Effect::Reply { .. }))
            .collect();
        assert_eq!(alive_ids(&alive), Vec::<String>::new());
        assert_eq!(registry.instances().len(), 1, "the display still happened");
    }

    #[test]
    fn an_icon_priming_before_a_display_is_used_by_that_display() {
        // Routed `System` so the icon reaches a system-leg effect on every
        // platform (the toast leg would only carry a glyph).
        let config = NotificationsConfig {
            routing_osc99: NotificationRouting::System,
            ..enabled_config(0.0)
        };
        let mut icon_cache = Osc99IconCache::default();
        let mut registry = Osc99Registry::default();
        let g_only = Osc99Request::Display(Box::new(osc99_request(&Notification99Data {
            icon_cache_key: Some("k".to_owned()),
            ..default_n99()
        })));

        let effects = route_in_order(
            &[
                Osc99Request::PrimeIcon(Osc99IconPrime {
                    key: "k".to_owned(),
                    data: vec![7, 7, 7],
                }),
                g_only,
            ],
            &config,
            &mut icon_cache,
            &mut registry,
        );

        let shown = effects.iter().find_map(|effect| match effect {
            Osc99Effect::ShowLinuxSystem { notification, .. }
            | Osc99Effect::ShowUntrackedSystem(notification) => Some(notification),
            _ => None,
        });
        let Some(shown) = shown else {
            panic!("expected a system show, got {effects:?}");
        };
        assert_eq!(shown.icon_data.as_deref(), Some([7u8, 7, 7].as_slice()));
    }

    #[test]
    fn a_display_before_its_icon_priming_does_not_see_the_icon() {
        let config = NotificationsConfig {
            routing_osc99: NotificationRouting::System,
            ..enabled_config(0.0)
        };
        let mut icon_cache = Osc99IconCache::default();
        let mut registry = Osc99Registry::default();
        let g_only = Osc99Request::Display(Box::new(osc99_request(&Notification99Data {
            icon_cache_key: Some("k".to_owned()),
            ..default_n99()
        })));

        let effects = route_in_order(
            &[
                g_only,
                Osc99Request::PrimeIcon(Osc99IconPrime {
                    key: "k".to_owned(),
                    data: vec![7, 7, 7],
                }),
            ],
            &config,
            &mut icon_cache,
            &mut registry,
        );

        let shown = effects.iter().find_map(|effect| match effect {
            Osc99Effect::ShowLinuxSystem { notification, .. }
            | Osc99Effect::ShowUntrackedSystem(notification) => Some(notification),
            _ => None,
        });
        let Some(shown) = shown else {
            panic!("expected a system show, got {effects:?}");
        };
        assert_eq!(shown.icon_data, None, "the priming came too late");
        assert_eq!(icon_cache.get("k"), Some([7u8, 7, 7].as_slice()));
    }

    #[test]
    fn route_osc99_routing_system_pushes_no_toast() {
        // The test executor's system and untracked backends display nothing,
        // so this only asserts the toast leg was correctly gated off.
        let config = NotificationsConfig {
            routing_osc99: NotificationRouting::System,
            ..enabled_config(0.0)
        };
        let mut toasts = ToastStack::default();
        let mut icon_cache = Osc99IconCache::default();
        let mut registry = Osc99Registry::default();
        let data = default_n99();

        route_osc99_into(
            &osc99_request(&data),
            &config,
            focused_ctx(),
            &mut toasts,
            &mut icon_cache,
            &mut registry,
        );

        assert_eq!(toasts.len(), 0);
    }

    /// On Linux/BSD the system leg is the registry's `ShowLinuxSystem`
    /// effect: the router no longer shows anything itself, the executor hands
    /// the notification to the backend.
    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn route_osc99_system_routing_shows_through_the_system_backend() {
        use freminal_notify::test_support::RecordingTrackedBackend;

        let config = NotificationsConfig {
            routing_osc99: NotificationRouting::System,
            ..enabled_config(0.0)
        };
        let mut toasts = ToastStack::default();
        let mut icon_cache = Osc99IconCache::default();
        let mut registry = Osc99Registry::default();
        let mut system = RecordingTrackedBackend::default();
        let data = Notification99Data {
            id: Some("sys".to_owned()),
            ..default_n99()
        };

        route_osc99_with_system(
            &osc99_request(&data),
            &config,
            focused_ctx(),
            &mut toasts,
            &mut icon_cache,
            &mut registry,
            &mut system,
        );

        let [shown] = system.shown.as_slice() else {
            panic!("expected one desktop notification, got {:?}", system.shown);
        };
        assert_eq!(shown.notification, data);
        assert_eq!(shown.replaces, None);
        assert_eq!(toasts.len(), 0, "routed System: no toast");
        let [entry] = registry.instances() else {
            panic!("the notification must be registered");
        };
        assert_eq!(entry.instance(), shown.instance);
        assert_eq!(entry.open_legs(), [Osc99Leg::LinuxSystem]);
    }

    /// The notification handed to the desktop-notification backend carries the
    /// resolved icon bytes: the transmitted ones, and the cached ones when a
    /// later notification refers to them by `g=` alone.
    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn route_osc99_hands_resolved_icon_bytes_to_the_system_backend() {
        use freminal_notify::test_support::RecordingTrackedBackend;

        let config = NotificationsConfig {
            routing_osc99: NotificationRouting::System,
            ..enabled_config(0.0)
        };
        let mut toasts = ToastStack::default();
        let mut icon_cache = Osc99IconCache::default();
        let mut registry = Osc99Registry::default();
        let mut system = RecordingTrackedBackend::default();
        let transmitted = Notification99Data {
            icon_data: Some(vec![1, 2, 3]),
            icon_cache_key: Some("k".to_owned()),
            ..default_n99()
        };
        let by_key = Notification99Data {
            icon_cache_key: Some("k".to_owned()),
            ..default_n99()
        };
        let uncached = Notification99Data {
            icon_cache_key: Some("other".to_owned()),
            ..default_n99()
        };

        for data in [&transmitted, &by_key, &uncached] {
            route_osc99_with_system(
                &osc99_request(data),
                &config,
                focused_ctx(),
                &mut toasts,
                &mut icon_cache,
                &mut registry,
                &mut system,
            );
        }

        let icons: Vec<Option<Vec<u8>>> = system
            .shown
            .iter()
            .map(|shown| shown.notification.icon_data.clone())
            .collect();
        assert_eq!(icons, [Some(vec![1, 2, 3]), Some(vec![1, 2, 3]), None]);
    }

    /// A toast-only routing never reaches the desktop-notification backend.
    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn route_osc99_toast_routing_does_not_touch_the_system_backend() {
        use freminal_notify::test_support::RecordingTrackedBackend;

        let config = NotificationsConfig {
            routing_osc99: NotificationRouting::Toast,
            ..enabled_config(0.0)
        };
        let mut toasts = ToastStack::default();
        let mut registry = Osc99Registry::default();
        let mut system = RecordingTrackedBackend::default();

        route_osc99_with_system(
            &osc99_request(&default_n99()),
            &config,
            focused_ctx(),
            &mut toasts,
            &mut Osc99IconCache::default(),
            &mut registry,
            &mut system,
        );

        assert_eq!(system.shown, Vec::new());
        assert_eq!(toasts.len(), 1);
    }

    // ── Weak reply handle (Task 130.8) ───────────────────────────────

    fn reply_channel() -> (
        std::sync::Arc<Sender<InputEvent>>,
        crossbeam_channel::Receiver<InputEvent>,
    ) {
        let (tx, rx) = crossbeam_channel::unbounded();
        (std::sync::Arc::new(tx), rx)
    }

    #[test]
    fn send_reply_weak_delivers_while_the_pane_handle_is_alive() {
        let (arc, rx) = reply_channel();
        let weak = std::sync::Arc::downgrade(&arc);

        send_reply_weak(
            &weak,
            GuiReply::Osc99Closed {
                id: Some("n1".to_owned()),
                tracking: Osc99CloseTracking::Untracked,
            },
        );

        match rx.try_recv() {
            Ok(InputEvent::Reply(GuiReply::Osc99Closed { id, tracking })) => {
                assert_eq!(id.as_deref(), Some("n1"));
                assert_eq!(tracking, Osc99CloseTracking::Untracked);
            }
            other => panic!("expected an OSC 99 closed reply, got {other:?}"),
        }
    }

    #[test]
    fn send_reply_weak_drops_the_reply_after_the_pane_handle_is_gone() {
        let (arc, rx) = reply_channel();
        let weak = std::sync::Arc::downgrade(&arc);
        drop(arc);

        // Must not panic and must not deliver anything.
        send_reply_weak(
            &weak,
            GuiReply::Osc99Closed {
                id: None,
                tracking: Osc99CloseTracking::Untracked,
            },
        );

        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn weak_reply_handle_does_not_keep_the_input_channel_connected() {
        // Mirrors `Pane::reply_tx`: the pane's `input_tx` clone is wrapped in
        // an `Arc`, and only `Weak` handles escape. Dropping the pane's
        // strong handles must disconnect the channel so the PTY consumer
        // (blocked in `recv`) exits even while a `Weak` is still held by a
        // notification thread.
        let (tx, rx) = crossbeam_channel::unbounded::<InputEvent>();
        let arc = std::sync::Arc::new(tx);
        let weak = std::sync::Arc::downgrade(&arc);
        drop(arc);

        assert!(matches!(
            rx.recv_timeout(std::time::Duration::from_millis(50)),
            Err(crossbeam_channel::RecvTimeoutError::Disconnected)
        ));
        assert!(weak.upgrade().is_none());
    }

    /// A config with the notification system on and every category routed by
    /// `routing`.
    fn notifications(routing: NotificationRouting) -> NotificationsConfig {
        NotificationsConfig {
            enabled: true,
            routing_osc_text: routing,
            routing_error: routing,
            routing_command_finished: routing,
            ..NotificationsConfig::default()
        }
    }

    #[test]
    fn an_osc_9_request_routed_to_system_reaches_the_sink_once_with_normal_urgency() {
        let (sink, recorder) = PlainSink::recording();
        let request = NotificationRequest {
            kind: NotificationKind::OscText,
            source: Some(OscNotifySource::Osc9),
            title: Some("a<b>".to_owned()),
            body: "x & y".to_owned(),
        };
        let mut toasts = ToastStack::default();

        NotificationRouter::route(
            &request,
            &notifications(NotificationRouting::System),
            false,
            &mut toasts,
            &sink,
        );

        assert_eq!(
            recorder.taken(),
            [PlainNotification {
                summary: "a<b>".to_owned(),
                body: "x & y".to_owned(),
                urgency: NotificationUrgency::Normal,
            }],
            "exactly one notification, raw"
        );
        assert_eq!(toasts.len(), 0, "System routing pushes no toast");
    }

    #[test]
    fn an_error_request_routed_to_system_reaches_the_sink_with_critical_urgency() {
        let (sink, recorder) = PlainSink::recording();
        let request = NotificationRequest {
            kind: NotificationKind::Error,
            source: None,
            title: None,
            body: "exit 2".to_owned(),
        };

        NotificationRouter::route(
            &request,
            &notifications(NotificationRouting::System),
            false,
            &mut ToastStack::default(),
            &sink,
        );

        assert_eq!(
            recorder.taken(),
            [PlainNotification {
                summary: "Error".to_owned(),
                body: "exit 2".to_owned(),
                urgency: NotificationUrgency::Critical,
            }]
        );
    }

    #[test]
    fn toast_only_routing_delivers_nothing_to_the_sink() {
        let (sink, recorder) = PlainSink::recording();
        let mut toasts = ToastStack::default();

        NotificationRouter::route(
            &osc_req("world"),
            &notifications(NotificationRouting::Toast),
            false,
            &mut toasts,
            &sink,
        );

        assert_eq!(toasts.len(), 1, "the toast leg ran");
        assert_eq!(recorder.taken(), [], "the system leg did not");
    }

    #[test]
    fn the_test_button_delivers_one_notification_even_with_the_master_switch_off() {
        let (sink, recorder) = PlainSink::recording();
        let config = NotificationsConfig {
            enabled: false,
            ..notifications(NotificationRouting::System)
        };
        let request = NotificationRequest::sample();

        // `route` honours the master switch: nothing is shown.
        NotificationRouter::route(&request, &config, true, &mut ToastStack::default(), &sink);
        assert_eq!(recorder.taken(), []);

        NotificationRouter::route_test(&request, &config, true, &mut ToastStack::default(), &sink);

        assert_eq!(
            recorder.taken(),
            [PlainNotification {
                summary: "Freminal".to_owned(),
                body: "Test notification".to_owned(),
                urgency: NotificationUrgency::Normal,
            }]
        );
    }

    /// End to end: the router, through the sink, to the fake daemon.
    #[cfg(all(unix, not(target_os = "macos")))]
    mod fake_daemon {
        use super::*;
        use freminal_notify::test_support::{TestResult, assert_no_queued_events, started};

        #[test]
        fn an_osc_9_request_routed_to_system_is_one_escaped_notify_with_normal_urgency()
        -> TestResult {
            let (rig, mut handle, _woken) = started(&["body", "body-markup"])?;
            let request = NotificationRequest {
                kind: NotificationKind::OscText,
                source: Some(OscNotifySource::Osc9),
                title: Some("a<b>".to_owned()),
                body: "x & y".to_owned(),
            };
            let mut toasts = ToastStack::default();

            NotificationRouter::route(
                &request,
                &notifications(NotificationRouting::System),
                false,
                &mut toasts,
                &PlainSink::through_service(handle.plain_sender()),
            );
            assert_no_queued_events(&mut handle)?;

            let recorded = rig.recorded();
            assert_eq!(recorded.len(), 1, "exactly one Notify call");
            assert_eq!(recorded[0].summary, "a<\u{200c}b>");
            assert_eq!(recorded[0].body, "x &\u{200c} y");
            assert_eq!(recorded[0].urgency, Some(1));
            assert_eq!(toasts.len(), 0, "System routing pushes no toast");
            Ok(())
        }

        #[test]
        fn an_error_request_routed_through_its_category_has_critical_urgency() -> TestResult {
            let (rig, mut handle, _woken) = started(&["body"])?;
            let request = NotificationRequest {
                kind: NotificationKind::Error,
                source: None,
                title: None,
                body: "exit 2".to_owned(),
            };
            let config = NotificationsConfig {
                routing_error: NotificationRouting::System,
                routing_osc_text: NotificationRouting::Disabled,
                ..notifications(NotificationRouting::Disabled)
            };

            NotificationRouter::route(
                &request,
                &config,
                false,
                &mut ToastStack::default(),
                &PlainSink::through_service(handle.plain_sender()),
            );
            assert_no_queued_events(&mut handle)?;

            let recorded = rig.recorded();
            assert_eq!(recorded.len(), 1);
            assert_eq!(recorded[0].summary, "Error");
            assert_eq!(recorded[0].body, "exit 2");
            assert_eq!(recorded[0].urgency, Some(2));
            Ok(())
        }

        #[test]
        fn the_test_button_shows_a_notification_even_with_the_master_switch_off() -> TestResult {
            let (rig, mut handle, _woken) = started(&["body"])?;
            let config = NotificationsConfig {
                enabled: false,
                ..notifications(NotificationRouting::System)
            };
            let sink = PlainSink::through_service(handle.plain_sender());
            let request = NotificationRequest::sample();

            // `route` honours the master switch: nothing is shown.
            NotificationRouter::route(&request, &config, true, &mut ToastStack::default(), &sink);
            assert_no_queued_events(&mut handle)?;
            assert_eq!(rig.recorded(), []);

            NotificationRouter::route_test(
                &request,
                &config,
                true,
                &mut ToastStack::default(),
                &sink,
            );
            assert_no_queued_events(&mut handle)?;

            let recorded = rig.recorded();
            assert_eq!(recorded.len(), 1);
            assert_eq!(recorded[0].summary, "Freminal");
            assert_eq!(recorded[0].body, "Test notification");
            assert_eq!(recorded[0].urgency, Some(1));
            Ok(())
        }

        #[test]
        fn toast_only_routing_never_reaches_the_daemon() -> TestResult {
            let (rig, mut handle, _woken) = started(&["body"])?;
            let request = NotificationRequest {
                kind: NotificationKind::OscText,
                source: Some(OscNotifySource::Osc9),
                title: Some("Hello".to_owned()),
                body: "world".to_owned(),
            };
            let mut toasts = ToastStack::default();

            NotificationRouter::route(
                &request,
                &notifications(NotificationRouting::Toast),
                false,
                &mut toasts,
                &PlainSink::through_service(handle.plain_sender()),
            );
            assert_no_queued_events(&mut handle)?;

            assert_eq!(toasts.len(), 1, "the toast leg ran");
            assert_eq!(rig.recorded(), [], "the system leg did not");
            Ok(())
        }
    }
}
