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
//! - **System leg:** show a desktop notification via `notify-rust` on a
//!   short-lived background thread (the D-Bus call blocks briefly on Linux).
//!   (OSC 99 is the exception on Linux/BSD: its system leg is driven by the
//!   registry through the zbus notification service, see `osc99`.)
//!
//! Routing is decided per [`NotificationKind`] using the focus-aware
//! [`NotificationRouting::wants_toast`] / [`NotificationRouting::wants_system`]
//! helpers. The system leg additionally requires that `notify-rust` is the
//! intended sink; the toast leg never spawns a thread.

use std::borrow::Cow;
use std::sync::Weak;
#[cfg(any(target_os = "macos", target_os = "windows"))]
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use crossbeam_channel::Sender;
use freminal_common::buffer_states::command_block::{CommandBlock, CommandStatus};
use freminal_common::buffer_states::osc::OscNotifySource;
use freminal_common::buffer_states::osc_notify_99::NotificationOccasion;
use freminal_common::buffer_states::osc99_notification::{Notification99Data, Osc99ControlKind};
#[cfg(any(target_os = "macos", target_os = "windows"))]
use freminal_common::buffer_states::osc99_notification::{
    Osc99CloseReporting, Osc99Expiry, Osc99Sound,
};
use freminal_common::buffer_states::window_manipulation::NotificationKind;
use freminal_common::config::{FreminalToastCategory, NotificationRouting, NotificationsConfig};
use freminal_common::send_or_log;
#[cfg(any(target_os = "macos", target_os = "windows"))]
use freminal_terminal_emulator::io::Osc99CloseTracking;
use freminal_terminal_emulator::io::{GuiReply, InputEvent};

use super::command_blocks::format_command_duration;
use super::frame_drain::WindowFocus;
use super::osc99::{
    Osc99DisplayRequest, Osc99Effect, Osc99FilterVerdict, Osc99IconCache, Osc99Leg, Osc99PaneState,
    Osc99Registry, PaneKeyboardFocus, PaneVisibility,
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
    /// onto `toasts`; the system leg spawns a background thread.
    pub(super) fn route(
        req: &NotificationRequest,
        config: &NotificationsConfig,
        focused: bool,
        toasts: &mut ToastStack,
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
            Self::show_system(req);
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
            Self::show_system(req);
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

    /// Show a desktop notification on a short-lived background thread.
    ///
    /// `notify-rust`'s `show()` makes a synchronous D-Bus call on Linux, so
    /// it must not run on the egui frame thread. Failures are logged and
    /// otherwise ignored — a missing notification daemon is non-fatal.
    ///
    /// Under `cfg(test)` the real OS call is skipped entirely: on macOS,
    /// `notify-rust`'s `show()` funnels into the Objective-C notification
    /// runtime, which throws an *uncatchable foreign exception* when invoked
    /// from a non-`.app`-bundled binary — exactly what a `cargo test` binary
    /// is. Rust's unwinder cannot catch it, so the process `abort()`s
    /// (`SIGABRT`). Because the notification runs on a detached, best-effort
    /// thread, the abort lands nondeterministically during test teardown and
    /// takes the whole harness down after every test has already passed. The
    /// routing policy that these unit tests exercise is fully evaluated before
    /// this point, so skipping the spawn under test loses no coverage.
    fn show_system(req: &NotificationRequest) {
        // See the doc comment: the OS notification call aborts in test
        // binaries on macOS (foreign ObjC exception). Never spawn it here.
        if cfg!(test) {
            return;
        }
        let notification = Self::build_system_notification(req);
        let builder = std::thread::Builder::new().name("freminal-notify".to_owned());
        if let Err(e) = builder.spawn(move || {
            // Attach the inline `image-data` icon on the background thread, not
            // the GUI thread: the first call decodes the embedded PNG (~1 MiB
            // RGBA), which must never block the frame loop (adversarial review
            // finding 1). `build_system_notification` stays a pure, icon-less
            // builder so it remains unit-testable without the decode; the
            // Linux/BSD-only image attachment lives here. The `mut` rebind is
            // scoped to this cfg so it isn't an unused-mut on macOS/Windows,
            // where `image_data` does not exist.
            #[cfg(all(unix, not(target_os = "macos")))]
            let mut notification = notification;
            #[cfg(all(unix, not(target_os = "macos")))]
            if let Some(image) = freminal_icon_image() {
                notification.image_data(image.clone());
            }
            if let Err(e) = notification.show() {
                tracing::warn!("failed to show desktop notification: {e}");
            }
        }) {
            tracing::warn!("failed to spawn notification thread: {e}");
        }
    }

    /// Build the `notify-rust` notification for a freminal-derived / OSC 9 /
    /// OSC 777 / command-finished desktop notification (the [`show_system`]
    /// path — *not* the OSC 99 path, which builds its own notification with a
    /// transmitted icon in [`build_osc99_notification`]).
    ///
    /// Attaches the bundled freminal application icon by icon-theme **name**
    /// (`.icon("freminal")`), matching `assets/freminal.desktop`'s
    /// `Icon=freminal`. This is the secondary icon hint: it covers
    /// macOS/Windows and installed setups whose packaging drops
    /// `freminal.png` into the hicolor theme.
    ///
    /// The *primary*, always-works icon — the embedded app icon as inline
    /// `image-data` bytes (the mechanism Discord & co. use, which shows even
    /// when freminal is not installed into the system icon theme) — is
    /// attached separately, on the background notification thread, in
    /// [`show_system`], because decoding the embedded PNG (~1 MiB RGBA) must
    /// not run on the GUI thread. Keeping this builder icon-bytes-free is
    /// deliberate: it stays a pure, cheap, unit-testable function (no decode,
    /// no `image-data` platform gating) while the heavy Linux/BSD-only image
    /// work lives off the frame loop. A daemon that honours `image-data` uses
    /// that; one that only honours `icon` falls back to this name.
    ///
    /// The OSC 99 path deliberately builds its own notification with a
    /// *transmitted* icon (see [`build_osc99_notification`]) and must not be
    /// routed through here.
    ///
    /// Split out as a pure builder (rather than inlined in the spawned thread)
    /// so the icon/appname/summary/body/urgency wiring is unit-testable
    /// without invoking the real OS `show()` call (which is skipped under
    /// `cfg(test)`; see [`show_system`]).
    ///
    /// [`show_system`]: Self::show_system
    /// [`build_osc99_notification`]: Self::build_osc99_notification
    fn build_system_notification(req: &NotificationRequest) -> notify_rust::Notification {
        let mut notification = notify_rust::Notification::new();
        notification
            .appname("freminal")
            // Secondary, name-based icon hint (see doc above). `.icon()` is
            // available on every platform in notify-rust 4.18 (a no-op where
            // unsupported); it is the same name-based `.icon(name)` call
            // `build_osc99_notification` uses in its no-transmitted-bytes
            // branch. The primary inline-bytes icon is attached in
            // `show_system` on the background thread.
            .icon("freminal")
            .summary(req.summary())
            .body(&req.body);

        // Error-category notifications use Critical urgency so they persist
        // and pop as a banner; everything else is Normal. Many Linux
        // notification daemons only raise a banner (rather than silently
        // filing the notification in the tray) when an urgency hint is set.
        //
        // `notify_rust`'s `urgency()` exists on Linux/BSD and Windows but
        // NOT on macOS (the `Urgency` type is re-exported there but
        // deprecated and the method is absent), so gate the call on
        // `not(target_os = "macos")`.
        #[cfg(not(target_os = "macos"))]
        {
            let urgency = if matches!(req.kind, NotificationKind::Error) {
                notify_rust::Urgency::Critical
            } else {
                notify_rust::Urgency::Normal
            };
            notification.urgency(urgency);
        }

        notification
    }
}

/// The bundled freminal application icon as a notify-rust [`Image`], decoded
/// once from the embedded `assets/icon.png` and cached for the process
/// lifetime.
///
/// Returns `None` (and logs a warning once) if the embedded PNG fails to
/// decode or the pixels don't form a valid RGBA image — a missing
/// notification icon is non-fatal, so callers simply omit the `image-data`
/// hint in that case.
///
/// Linux/BSD only: notify-rust's [`notify_rust::Image`] /
/// [`notify_rust::Notification::image_data`] are gated
/// `#[cfg(all(unix, not(target_os = "macos")))]`.
///
/// [`Image`]: notify_rust::Image
#[cfg(all(unix, not(target_os = "macos")))]
fn freminal_icon_image() -> Option<&'static notify_rust::Image> {
    use std::sync::OnceLock;

    static ICON: OnceLock<Option<notify_rust::Image>> = OnceLock::new();

    ICON.get_or_init(|| {
        use conv2::ValueInto as _;

        // The same embedded PNG used for the OS window icon (see gui::run).
        const ICON_PNG: &[u8] = include_bytes!("../../../assets/icon.png");

        let rgba = match image::load_from_memory(ICON_PNG) {
            Ok(img) => img.to_rgba8(),
            Err(e) => {
                tracing::warn!("failed to decode embedded notification icon: {e}");
                return None;
            }
        };
        let (width, height) = rgba.dimensions();
        let (Ok(width), Ok(height)) = (width.value_into(), height.value_into()) else {
            tracing::warn!("embedded notification icon dimensions out of range");
            return None;
        };
        match notify_rust::Image::from_rgba(width, height, rgba.into_raw()) {
            Ok(image) => Some(image),
            Err(e) => {
                tracing::warn!("failed to build notification icon image: {e}");
                None
            }
        }
    })
    .as_ref()
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

/// Monotonic counter disambiguating concurrent OSC 99 icon temp files within
/// a single process run (paired with the process id in the filename).
#[cfg(any(target_os = "macos", target_os = "windows"))]
static OSC99_ICON_TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

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
    /// an untracked leg. On Linux/BSD the system leg is driven by the
    /// registry: it is returned as an
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
        request: &Osc99DisplayRequest,
        config: &NotificationsConfig,
        ctx: Osc99DisplayContext,
        icon_cache: &mut Osc99IconCache,
        registry: &mut Osc99Registry,
    ) -> Vec<Osc99Effect> {
        let data = &request.data;
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
        // `icon_data` (cloned only when the `g=` cache supplied them), so
        // the Linux/BSD notification service and the macOS/Windows system
        // leg both see the icon the program meant.
        let resolved_icon = Self::resolve_icon_bytes(data, icon_cache);
        let displayed: Cow<'_, Notification99Data> = match resolved_icon {
            Some(bytes) if data.icon_data.is_none() => Cow::Owned(Notification99Data {
                icon_data: Some(bytes),
                ..data.clone()
            }),
            _ => Cow::Borrowed(data),
        };
        let data = displayed.as_ref();

        if occasion_allows_display(data.occasion, ctx.pane) == Osc99DisplayDecision::Suppress {
            return Vec::new();
        }

        // User filter (`osc99_filters`), after the occasion check as in
        // kitty. A dropped notification is displayed nowhere, registered
        // nowhere and answered with nothing.
        if registry.filter_verdict(data) == Osc99FilterVerdict::Drop {
            tracing::debug!(id = ?data.id, "OSC 99 notification dropped by osc99_filters");
            return Vec::new();
        }

        // Toast/system legs honour `routing_osc99` (3.3/3.4), mirroring
        // `route`'s focus-aware gating. Bind each leg's gate once so the
        // toast leg, the system leg, and the registry below all agree on the
        // same decision.
        let routing = config.routing_osc99;
        let wants_toast = routing.wants_toast(ctx.window_focus.is_focused());
        let wants_system = routing.wants_system(ctx.window_focus.is_focused());

        let mut legs: Vec<Osc99Leg> = Vec::with_capacity(2);

        // The toast leg is shown by the effects `register` returns below.
        if wants_toast {
            legs.push(Osc99Leg::Toast);
        }

        if wants_system {
            // macOS/Windows: fire-and-forget, shown here. Linux/BSD: shown by
            // the `ShowLinuxSystem` effect `register` returns below.
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            Self::show_system_osc99(data, data.icon_data.clone(), Weak::clone(&request.reply_tx));
            // Only Linux/BSD observe a close or activation (through the
            // notification service); elsewhere the system notification is
            // fire-and-forget. `cfg!` rather than `#[cfg]` so both variants
            // are constructed on every platform.
            legs.push(if cfg!(all(unix, not(target_os = "macos"))) {
                Osc99Leg::LinuxSystem
            } else {
                Osc99Leg::UntrackedSystem
            });
        }

        // A notification displayed on no leg (routed `Disabled`) never
        // happened as far as later `p=alive` / `p=close` handling goes.
        // Every displayed notification is registered, whether or not it
        // carries an `i=` id and whichever leg showed it.
        if !legs.is_empty() {
            tracing::trace!(id = ?data.id, legs = ?legs, "registering OSC 99 notification");
            return registry.register(
                request.origin,
                request.reply_tx.clone(),
                data,
                &legs,
                ctx.now,
            );
        }
        Vec::new()
    }

    /// Resolve the icon bytes to display, applying the `g=` cache.
    ///
    /// - Transmitted bytes (`icon_data`) always win and are cached under
    ///   `icon_cache_key` (`g=`) when present, so a later `g=`-only
    ///   notification can reuse them.
    /// - Otherwise, a `g=`-only notification looks up the cache.
    /// - `None` when neither is available (the OS leg then falls back to
    ///   icon-by-name).
    fn resolve_icon_bytes(
        data: &Notification99Data,
        icon_cache: &mut Osc99IconCache,
    ) -> Option<Vec<u8>> {
        if let Some(bytes) = &data.icon_data {
            if let Some(key) = &data.icon_cache_key {
                icon_cache.insert(key.clone(), bytes.clone());
            }
            Some(bytes.clone())
        } else {
            data.icon_cache_key
                .as_ref()
                .and_then(|key| icon_cache.get(key).map(<[u8]>::to_vec))
        }
    }
}

/// The macOS and Windows OSC 99 system leg: fire-and-forget `notify-rust`.
///
/// Neither platform lets a background thread observe an activation or a close
/// (the macOS callback needs the main run loop), so the notification is shown
/// and forgotten, and a `c=1` request is answered at once in the `untracked`
/// form. Task 138.16 reworks this leg.
#[cfg(any(target_os = "macos", target_os = "windows"))]
impl NotificationRouter {
    /// Write transmitted icon bytes to a uniquely-named temp file.
    ///
    /// `tempfile` is only a dev-dependency of the `freminal` crate (not a
    /// production dependency), so this writes directly into
    /// [`std::env::temp_dir`] with a name disambiguated by the notification
    /// id, a monotonic per-process counter, and the process id. The caller
    /// is responsible for best-effort removal after use.
    fn write_icon_temp_file(id: Option<&str>, bytes: &[u8]) -> std::io::Result<std::path::PathBuf> {
        use std::io::Write as _;

        let pid = std::process::id();
        let sanitized_id: String = id
            .unwrap_or("noid")
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .collect();

        // The temp dir (e.g. `/tmp`) is typically world-writable, and the
        // filename is predictable (pid + a per-process counter), so a plain
        // `fs::write` (which follows symlinks and truncates) would be
        // vulnerable to a symlink pre-creation attack: a local attacker could
        // point a guessable future filename at a file the freminal user owns
        // and have us overwrite it. Use `create_new(true)` (O_CREAT|O_EXCL,
        // which refuses to follow an existing path/symlink) and retry on a
        // name collision so the write always lands in a fresh, exclusively
        // created file.
        loop {
            let counter = OSC99_ICON_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
            let filename = format!("freminal-osc99-icon-{sanitized_id}-{pid}-{counter}.png");
            let path = std::env::temp_dir().join(filename);
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(mut file) => {
                    file.write_all(bytes)?;
                    return Ok(path);
                }
                // A collision on the guessable name (or a pre-created
                // symlink) is retried with the next counter value.
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e),
            }
        }
    }

    /// Best-effort removal of an OSC 99 icon temp file; a leftover file is
    /// not fatal, so failures are logged at `debug` and otherwise ignored.
    fn remove_icon_temp_file(path: &std::path::Path) {
        if let Err(e) = std::fs::remove_file(path) {
            tracing::debug!(
                "failed to remove OSC 99 icon temp file {}: {e}",
                path.display()
            );
        }
    }

    /// Build a `notify-rust` `Notification` from `data`, applying urgency,
    /// buttons, sound, timeout, and icon (as a temp file when transmitted
    /// bytes are given, else by name).
    ///
    /// Returns the built notification plus the icon temp file path (if one
    /// was written), so the caller can clean it up after `.show()`.
    fn build_osc99_notification(
        data: &Notification99Data,
        resolved_icon_bytes: Option<Vec<u8>>,
    ) -> (notify_rust::Notification, Option<std::path::PathBuf>) {
        let body = data.body.as_deref().unwrap_or_default();
        let mut notification = notify_rust::Notification::new();
        notification
            .appname(data.app_name.as_deref().unwrap_or("freminal"))
            .summary(if data.title.is_empty() {
                body
            } else {
                &data.title
            })
            .body(body);

        // See `Self::build_system_notification` for the platform rationale:
        // `urgency()` is unavailable on macOS.
        #[cfg(not(target_os = "macos"))]
        {
            use freminal_common::buffer_states::osc_notify_99::NotificationUrgency;

            let urgency_level = match data.urgency {
                NotificationUrgency::Low => notify_rust::Urgency::Low,
                NotificationUrgency::Critical => notify_rust::Urgency::Critical,
                NotificationUrgency::Normal => notify_rust::Urgency::Normal,
            };
            notification.urgency(urgency_level);
        }

        for (idx, label) in data.buttons.iter().enumerate() {
            notification.action(&idx.to_string(), label);
        }

        match &data.sound {
            // The daemon's default sound: no sound-name hint is sent.
            Osc99Sound::System => {}
            Osc99Sound::Silent => {
                notification.sound_name("silent");
            }
            Osc99Sound::Named(name) => {
                notification.sound_name(name);
            }
        }

        let timeout = match data.expiry {
            Osc99Expiry::OsDefault => notify_rust::Timeout::Default,
            Osc99Expiry::Never => notify_rust::Timeout::Never,
            Osc99Expiry::After(ms) => notify_rust::Timeout::Milliseconds(ms.get()),
        };
        notification.timeout(timeout);

        let mut icon_temp_path: Option<std::path::PathBuf> = None;
        if let Some(bytes) = resolved_icon_bytes {
            match Self::write_icon_temp_file(data.id.as_deref(), &bytes) {
                Ok(path) => {
                    if let Some(path_str) = path.to_str() {
                        notification.image_path(path_str);
                    }
                    icon_temp_path = Some(path);
                }
                Err(e) => {
                    tracing::warn!("failed to write OSC 99 icon temp file: {e}");
                }
            }
        } else if let Some(first) = data.icon_names.first() {
            notification.icon(first);
        }

        (notification, icon_temp_path)
    }

    /// Show an OS notification for an OSC 99 notification on a short-lived
    /// background thread, and emit a `c=1` close report immediately in the
    /// `untracked` form (no activation reports are sent: `notify-rust` does
    /// not expose an observable handle from a background thread here).
    /// `reply_tx` is a `Weak` handle upgraded per send, so the thread never
    /// keeps a closed pane's PTY consumer alive. The icon temp file (if any)
    /// is removed on a best-effort basis after `show()`; cleanup failure
    /// never fails the notification.
    fn show_system_osc99(
        data: &Notification99Data,
        resolved_icon_bytes: Option<Vec<u8>>,
        reply_tx: Weak<Sender<InputEvent>>,
    ) {
        // See `show_system`: the OS notification call aborts in test binaries
        // on macOS (uncatchable foreign ObjC exception in a non-bundled
        // process). The OSC 99 unit tests assert only on registry and toast
        // state, both settled before this point, so skipping the spawn under
        // test loses no coverage.
        if cfg!(test) {
            return;
        }
        let data = data.clone();

        let builder = std::thread::Builder::new().name("freminal-notify-99".to_owned());
        if let Err(e) = builder.spawn(move || {
            let (notification, mut icon_temp_path) =
                Self::build_osc99_notification(&data, resolved_icon_bytes);

            if let Err(e) = notification.show() {
                tracing::warn!("failed to show OSC 99 desktop notification: {e}");
            }
            if data.close_reporting == Osc99CloseReporting::Report {
                let reply = GuiReply::Osc99Closed {
                    id: data.id.clone(),
                    tracking: Osc99CloseTracking::Untracked,
                };
                send_reply_weak(&reply_tx, reply);
            }
            if let Some(path) = icon_temp_path.take() {
                Self::remove_icon_temp_file(&path);
            }
        }) {
            tracing::warn!("failed to spawn OSC 99 notification thread: {e}");
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::gui::osc99::{
        NoSystemBackend, Osc99EffectSink, Osc99Filter, Osc99Origin, Osc99SystemBackend,
        apply_osc99_effects, osc99_toast_spec,
    };
    use freminal_common::buffer_states::osc_notify_99::NotificationUrgency;
    use freminal_common::buffer_states::osc99_notification::{
        Osc99ActivationFocus, Osc99ActivationReporting, Osc99CloseReporting, Osc99Expiry,
        Osc99Sound,
    };
    use freminal_common::buffer_states::row_number::RowNumber;
    use freminal_common::config::FreminalToastRouting;
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
            &mut NoSystemBackend,
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
        system: &mut dyn Osc99SystemBackend,
    ) {
        let effects = NotificationRouter::route_osc99(request, config, ctx, icon_cache, registry);
        apply_osc99_effects(
            effects,
            &mut Osc99EffectSink {
                toasts,
                system,
                focus_requests: Vec::new(),
                failed_system_shows: Vec::new(),
                failed_system_closes: Vec::new(),
            },
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

    /// Slice 3 (#433): the `show_system` path attaches the bundled freminal
    /// application icon by name (a secondary hint alongside the inline
    /// `image-data` bytes on Linux/BSD) so desktop notifications show the app
    /// icon. The name matches `assets/freminal.desktop`'s `Icon=freminal`.
    #[test]
    fn build_system_notification_sets_freminal_icon() {
        let req = NotificationRequest {
            kind: NotificationKind::OscText,
            source: Some(OscNotifySource::Osc9),
            title: Some("Hello".to_owned()),
            body: "world".to_owned(),
        };
        let notification = NotificationRouter::build_system_notification(&req);
        assert_eq!(notification.icon, "freminal");
        assert_eq!(notification.appname, "freminal");
        assert_eq!(notification.summary, "Hello");
        assert_eq!(notification.body, "world");
    }

    /// The embedded `assets/icon.png` must decode into a valid notify-rust
    /// `Image` so the inline `image-data` icon actually works (this is the
    /// path that shows the icon even when freminal is not installed into the
    /// system icon theme). A `None` here means the icon silently vanishes
    /// from every desktop notification. Linux/BSD only (matches the `cfg` on
    /// `freminal_icon_image`).
    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn embedded_notification_icon_decodes() {
        assert!(
            super::freminal_icon_image().is_some(),
            "embedded assets/icon.png must decode into a valid RGBA notification image"
        );
    }

    /// The builder still applies the per-kind summary fallback when the
    /// request carries no explicit title.
    #[test]
    fn build_system_notification_uses_summary_fallback() {
        let req = osc_req("body text");
        let notification = NotificationRouter::build_system_notification(&req);
        // `OscText` with no title falls back to the kind-derived default.
        assert_eq!(notification.summary, "Notification");
        assert_eq!(notification.icon, "freminal");
    }

    /// Slice 3 invariant guard: the OSC 99 path must NOT be given the
    /// `"freminal"` app icon — it carries its own transmitted icon (a named
    /// freedesktop icon, or transmitted bytes via `image_path`). This locks
    /// the "slice 3 only touches `show_system`, never the OSC 99 path"
    /// boundary so a future "make it consistent" edit can't silently override
    /// a program's transmitted OSC 99 icon with the freminal app icon.
    ///
    /// The builder only exists on macOS and Windows: the Linux/BSD system leg
    /// is the zbus notification service.
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    #[test]
    fn build_osc99_notification_uses_transmitted_icon_not_freminal() {
        // Named-icon branch: the transmitted freedesktop icon name is used.
        let (named, temp) = NotificationRouter::build_osc99_notification(
            &Notification99Data {
                id: Some("id-1".to_owned()),
                icon_names: vec!["weather-clear".to_owned()],
                ..default_n99()
            },
            None,
        );
        assert_eq!(
            named.icon, "weather-clear",
            "OSC 99 must use the transmitted icon name, never \"freminal\""
        );
        assert!(temp.is_none(), "no temp file when no bytes transmitted");

        // Transmitted-bytes branch: `.icon` stays empty (bytes go to
        // `image_path` via a temp file), and definitely is not "freminal".
        let (with_bytes, temp) = NotificationRouter::build_osc99_notification(
            &Notification99Data {
                id: Some("id-2".to_owned()),
                ..default_n99()
            },
            Some(vec![1, 2, 3, 4]),
        );
        assert_ne!(
            with_bytes.icon, "freminal",
            "OSC 99 transmitted-bytes path must not fall back to the freminal app icon"
        );
        // Clean up the temp file the bytes branch created.
        if let Some(path) = temp {
            NotificationRouter::remove_icon_temp_file(&path);
        }
    }

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    #[test]
    fn build_osc99_notification_maps_expiry_to_timeout() {
        use notify_rust::Timeout;

        let timeout_for = |expiry| {
            NotificationRouter::build_osc99_notification(
                &Notification99Data {
                    expiry,
                    ..default_n99()
                },
                None,
            )
            .0
            .timeout
        };

        assert_eq!(timeout_for(Osc99Expiry::OsDefault), Timeout::Default);
        assert_eq!(timeout_for(Osc99Expiry::Never), Timeout::Never);
        assert_eq!(
            timeout_for(Osc99Expiry::After(std::num::NonZeroU32::new(5000).unwrap())),
            Timeout::Milliseconds(5000)
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

    /// Error-category notifications carry Critical urgency on the platforms
    /// that support the hint; everything else is Normal. The urgency lives in
    /// `notify_rust::Notification::hints`, which the crate itself gates
    /// `#[cfg(all(unix, not(target_os = "macos")))]` — so this test must use
    /// the *same* cfg as the field. (macOS lacks the `urgency()` setter;
    /// Windows stores urgency in a private field with no `.hints`.) The
    /// production `#[cfg(not(target_os = "macos"))]` urgency call in
    /// `build_system_notification` still runs on Windows — this test just
    /// can't observe it there without a public accessor.
    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn build_system_notification_urgency_by_kind() {
        use notify_rust::{Hint, Urgency};

        let mut req = osc_req("x");
        req.kind = NotificationKind::Error;
        let critical = NotificationRouter::build_system_notification(&req);
        assert!(critical.hints.contains(&Hint::Urgency(Urgency::Critical)));

        req.kind = NotificationKind::OscText;
        let normal = NotificationRouter::build_system_notification(&req);
        assert!(normal.hints.contains(&Hint::Urgency(Urgency::Normal)));
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

    #[test]
    fn disabled_config_pushes_no_toast() {
        let config = NotificationsConfig::default(); // enabled = false
        let mut toasts = ToastStack::default();
        NotificationRouter::route(&osc_req("hello"), &config, true, &mut toasts);
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
        NotificationRouter::route_test(&NotificationRequest::sample(), &config, true, &mut toasts);
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
        NotificationRouter::route(&osc_req("hello"), &config, false, &mut toasts);
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
        // System-only routing never pushes a toast (the thread is best-effort
        // and not asserted here).
        NotificationRouter::route(&osc_req("hello"), &config, false, &mut toasts);
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
        NotificationRouter::route(&osc_req("hello"), &config, true, &mut toasts);
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
        NotificationRouter::route(&osc777_req("hello"), &config, true, &mut toasts);
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
        NotificationRouter::route(&osc_req("from osc9"), &config, true, &mut toasts);
        assert_eq!(toasts.len(), 1, "osc_9 = true must still route");

        let mut toasts = ToastStack::default();
        NotificationRouter::route(&osc777_req("from osc777"), &config, true, &mut toasts);
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
        NotificationRouter::route(&osc777_req("hello"), &config, true, &mut toasts);
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
        NotificationRouter::route_test(&NotificationRequest::sample(), &config, true, &mut toasts);
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
        NotificationRouter::route(&osc_req("hi"), &config, true, &mut focused_toasts);
        assert_eq!(focused_toasts.len(), 1, "focused -> toast");

        let mut unfocused_toasts = ToastStack::default();
        NotificationRouter::route(&osc_req("hi"), &config, false, &mut unfocused_toasts);
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

        assert_eq!(resolved, Some(vec![1, 2, 3, 4]));
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

        assert_eq!(resolved, Some(vec![9, 9, 9]));
    }

    #[test]
    fn route_osc99_icon_cache_miss_returns_none() {
        let mut icon_cache = Osc99IconCache::default();
        let data = Notification99Data {
            icon_data: None,
            icon_cache_key: Some("missing".to_owned()),
            ..default_n99()
        };

        assert_eq!(
            NotificationRouter::resolve_icon_bytes(&data, &mut icon_cache),
            None
        );
    }

    #[test]
    fn route_osc99_no_icon_data_or_key_returns_none() {
        let mut icon_cache = Osc99IconCache::default();
        let data = default_n99();

        assert_eq!(
            NotificationRouter::resolve_icon_bytes(&data, &mut icon_cache),
            None
        );
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
        assert_eq!(entry.id.as_deref(), Some("n1"));
        assert_eq!(entry.activation_reporting, Osc99ActivationReporting::Report);
        assert_eq!(entry.close_reporting, Osc99CloseReporting::Report);
        assert_ne!(entry.open_legs, Vec::<Osc99Leg>::new());
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
        assert_eq!(entry.id, None);
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
            &osc99_request(&data),
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
        assert_eq!(entry.id.as_deref(), Some("wanted"));
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
        assert_eq!(entry.id.as_deref(), Some("toast-only"));
        assert_eq!(entry.open_legs, [Osc99Leg::Toast]);
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

        let system_leg = if cfg!(all(unix, not(target_os = "macos"))) {
            Osc99Leg::LinuxSystem
        } else {
            Osc99Leg::UntrackedSystem
        };
        let [entry] = registry.instances() else {
            panic!("a system-leg notification must be registered");
        };
        assert_eq!(entry.id.as_deref(), Some("tracked"));
        assert_eq!(entry.shown_legs, [Osc99Leg::Toast, system_leg]);
    }

    #[test]
    fn route_osc99_routing_system_pushes_no_toast() {
        // The system leg is skipped under `cfg(test)` regardless (see
        // `show_system`'s doc comment for the equivalent OSC 9/777 case), so
        // this only asserts the toast leg was correctly gated off.
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
        use crate::gui::osc99::RecordingSystemBackend;

        let config = NotificationsConfig {
            routing_osc99: NotificationRouting::System,
            ..enabled_config(0.0)
        };
        let mut toasts = ToastStack::default();
        let mut icon_cache = Osc99IconCache::default();
        let mut registry = Osc99Registry::default();
        let mut system = RecordingSystemBackend::default();
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
        assert_eq!(entry.instance, shown.instance);
        assert_eq!(entry.open_legs, [Osc99Leg::LinuxSystem]);
    }

    /// The notification handed to the desktop-notification backend carries the
    /// resolved icon bytes: the transmitted ones, and the cached ones when a
    /// later notification refers to them by `g=` alone.
    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn route_osc99_hands_resolved_icon_bytes_to_the_system_backend() {
        use crate::gui::osc99::RecordingSystemBackend;

        let config = NotificationsConfig {
            routing_osc99: NotificationRouting::System,
            ..enabled_config(0.0)
        };
        let mut toasts = ToastStack::default();
        let mut icon_cache = Osc99IconCache::default();
        let mut registry = Osc99Registry::default();
        let mut system = RecordingSystemBackend::default();
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
        use crate::gui::osc99::RecordingSystemBackend;

        let config = NotificationsConfig {
            routing_osc99: NotificationRouting::Toast,
            ..enabled_config(0.0)
        };
        let mut toasts = ToastStack::default();
        let mut registry = Osc99Registry::default();
        let mut system = RecordingSystemBackend::default();

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

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    #[test]
    fn write_icon_temp_file_uses_exclusive_create() {
        // Two writes with the same id must not collide or overwrite; the
        // monotonic counter + O_EXCL retry guarantees distinct fresh files.
        let a = NotificationRouter::write_icon_temp_file(Some("id"), b"aaaa")
            .expect("first temp write");
        let b = NotificationRouter::write_icon_temp_file(Some("id"), b"bbbb")
            .expect("second temp write");
        assert_ne!(a, b, "temp files must be distinct");
        assert_eq!(std::fs::read(&a).unwrap(), b"aaaa");
        assert_eq!(std::fs::read(&b).unwrap(), b"bbbb");

        // Writing to an existing path must not follow/truncate it: calling
        // the raw create_new on an existing file fails.
        let err = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&a)
            .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);

        // Clean up.
        NotificationRouter::remove_icon_temp_file(&a);
        NotificationRouter::remove_icon_temp_file(&b);
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
}
