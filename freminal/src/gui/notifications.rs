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
//!
//! Routing is decided per [`NotificationKind`] using the focus-aware
//! [`NotificationRouting::wants_toast`] / [`NotificationRouting::wants_system`]
//! helpers. The system leg additionally requires that `notify-rust` is the
//! intended sink; the toast leg never spawns a thread.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use conv2::ValueInto;
use crossbeam_channel::Sender;
use freminal_common::buffer_states::command_block::{CommandBlock, CommandStatus};
use freminal_common::buffer_states::window_manipulation::{
    Notification99Data, NotificationKind, Osc99ControlKind,
};
use freminal_common::config::{NotificationRouting, NotificationsConfig};
use freminal_common::pty_write::PtyWrite;
use freminal_common::send_or_log;

use super::command_blocks::format_command_duration;
use super::toast::ToastStack;

/// A fully-formed notification ready to be routed.
///
/// Produced by the OSC 9 / OSC 777 path (`handle_window_manipulation`) and
/// the command-finished path (the per-pane drain in `update()`), then handed
/// to [`NotificationRouter::route`].
#[derive(Debug, Clone)]
pub(super) struct NotificationRequest {
    /// The notification category, selecting the routing policy.
    pub kind: NotificationKind,
    /// The notification title. `None` falls back to a kind-derived default.
    pub title: Option<String>,
    /// The notification body text.
    pub body: String,
}

impl NotificationRequest {
    /// A sample notification used by the Settings "Test Notification" button
    /// so the user can verify their routing/template configuration without
    /// running a command. Categorised as [`NotificationKind::Info`] so it
    /// follows the `routing_info` policy.
    pub(super) fn sample() -> Self {
        Self {
            kind: NotificationKind::Info,
            title: Some("Freminal".to_owned()),
            body: "Test notification".to_owned(),
        }
    }

    /// The summary/title string shown to the user, falling back to a
    /// category-derived default when the source supplied no title.
    fn summary(&self) -> &str {
        self.title.as_deref().unwrap_or(match self.kind {
            NotificationKind::OscText | NotificationKind::Info => "Notification",
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
            // OSC text and explicit Info both follow the info policy.
            NotificationKind::OscText | NotificationKind::Info => config.routing_info,
        }
    }

    /// Push the notification onto the in-app toast stack. Error-category
    /// notifications use the error styling; everything else is informational.
    fn push_toast(req: &NotificationRequest, toasts: &mut ToastStack) {
        let detail = (!req.body.is_empty()).then(|| req.body.clone());
        match req.kind {
            NotificationKind::Error => toasts.error(req.summary().to_owned(), detail),
            NotificationKind::OscText
            | NotificationKind::CommandFinished
            | NotificationKind::Info => toasts.info(req.summary().to_owned(), detail),
        }
    }

    /// Show a desktop notification on a short-lived background thread.
    ///
    /// `notify-rust`'s `show()` makes a synchronous D-Bus call on Linux, so
    /// it must not run on the egui frame thread. Failures are logged and
    /// otherwise ignored — a missing notification daemon is non-fatal.
    fn show_system(req: &NotificationRequest) {
        let summary = req.summary().to_owned();
        let body = req.body.clone();
        // `notify-rust`'s `urgency()` setter exists on Linux/BSD and Windows
        // but NOT on macOS, so capture whether this is a high-urgency
        // (error-category) notification here and apply the hint only on the
        // platforms that support it (see the cfg-gated block below).
        let critical = matches!(req.kind, NotificationKind::Error);
        let builder = std::thread::Builder::new().name("freminal-notify".to_owned());
        if let Err(e) = builder.spawn(move || {
            let mut notification = notify_rust::Notification::new();
            notification
                .appname("freminal")
                .summary(&summary)
                .body(&body);

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
                let urgency = if critical {
                    notify_rust::Urgency::Critical
                } else {
                    notify_rust::Urgency::Normal
                };
                notification.urgency(urgency);
            }
            // Silence the unused-variable warning on macOS, where the urgency
            // hint is unavailable.
            #[cfg(target_os = "macos")]
            let _ = critical;

            if let Err(e) = notification.show() {
                tracing::warn!("failed to show desktop notification: {e}");
            }
        }) {
            tracing::warn!("failed to spawn notification thread: {e}");
        }
    }
}

/// Record of a live OSC 99 notification (for update/close reconciliation).
///
/// Populated by [`NotificationRouter::route_osc99`] whenever the source
/// notification carried an `i=` id. 99.5a only records identity plus the
/// report flags 99.6 needs; it does not retain a `notify-rust` handle (that
/// is 99.5b).
#[derive(Debug, Clone)]
pub(super) struct Osc99LiveEntry {
    /// Whether `a=report` was requested (activation reports wanted — 99.6).
    pub report_activation: bool,
    /// Whether `c=1` was requested (close report wanted — 99.6).
    pub close_report: bool,
}

/// Remove a notification id from the live map.
///
/// Called from `app_impl`'s OSC 99 control loop (Task 99.6) when an
/// app-driven `p=close` control sequence arrives, to keep the `p=alive`
/// response accurate. An OS-observed close (the activation-thread
/// `__closed` event) does NOT call this — see the pruning-tradeoff note on
/// [`NotificationRouter::route_osc99`].
///
/// Returns whether an entry was actually removed.
pub(super) fn forget_osc99(live: &mut HashMap<String, Osc99LiveEntry>, id: &str) -> bool {
    live.remove(id).is_some()
}

/// Build an OSC 99 activation report: `ESC ] 99 ; i=<id> ; <button> ST`.
///
/// `id` defaults to `0` when absent; `button` is the (0-based) action-id
/// string freminal registered for the button, or `None` for
/// whole-notification activation (empty button field).
pub(super) fn osc99_activation_report(id: Option<&str>, button: Option<&str>) -> Vec<u8> {
    let id = id.unwrap_or("0");
    let button = button.unwrap_or("");
    format!("\x1b]99;i={id};{button}\x1b\\").into_bytes()
}

/// Build an OSC 99 close report: `ESC ] 99 ; i=<id> : p=close ; [untracked] ST`.
///
/// `untracked` marks a close freminal could not directly observe (macOS /
/// Windows, where the background thread cannot watch for a close event).
pub(super) fn osc99_close_report(id: Option<&str>, untracked: bool) -> Vec<u8> {
    let id = id.unwrap_or("0");
    let payload = if untracked { "untracked" } else { "" };
    format!("\x1b]99;i={id}:p=close;{payload}\x1b\\").into_bytes()
}

/// Build an OSC 99 alive report: `ESC ] 99 ; i=<req-id> : p=alive ; id1,id2 ST`.
///
/// `req_id` defaults to `0` when the poll carried no id; `live_ids` is
/// comma-joined verbatim (empty list -> empty payload).
pub(super) fn osc99_alive_report(req_id: Option<&str>, live_ids: &[String]) -> Vec<u8> {
    let req_id = req_id.unwrap_or("0");
    let list = live_ids.join(",");
    format!("\x1b]99;i={req_id}:p=alive;{list}\x1b\\").into_bytes()
}

/// The OSC 99 capabilities freminal truthfully advertises in a `p=?`
/// response.
///
/// Colon-separated `key=value` pairs, in a stable order. Advertises only
/// what is genuinely implemented (the truthful-advertisement rule from
/// Task 76): `a=report` (NOT `focus` — `focus_on_activation` is parsed but
/// freminal does not act on it), `c=1` (close reports), `o=` all three
/// occasions (99.5a), `p=` the payload types freminal handles (display
/// types plus the control types it answers), `s=system,silent` (forwarded
/// freedesktop sound-name hints — freminal forwards the name, playback is
/// the daemon's concern), `u=0,1,2` (urgency levels — advertised even
/// though the setter is unavailable on macOS, matching Task 76's handling
/// of the same gap), and `w=1` (auto-expiry, wired via `.timeout()`).
const OSC99_CAPABILITIES: &str = "a=report:c=1:o=always,unfocused,invisible:p=title,body,icon,buttons,alive,close,?:s=system,silent:u=0,1,2:w=1";

/// Build the OSC 99 `p=?` capability handshake response:
/// `ESC ] 99 ; i=<id> : p=? ; <capabilities> ST`.
///
/// `id` defaults to `0` when the query control carried no id.
pub(super) fn osc99_query_response(id: Option<&str>) -> Vec<u8> {
    let id = id.unwrap_or("0");
    format!("\x1b]99;i={id}:p=?;{OSC99_CAPABILITIES}\x1b\\").into_bytes()
}

/// Collect the live notification ids in sorted order (deterministic for the
/// `p=alive` response and for testing).
pub(super) fn live_ids_sorted(live: &HashMap<String, Osc99LiveEntry>) -> Vec<String> {
    let mut ids: Vec<String> = live.keys().cloned().collect();
    ids.sort();
    ids
}

/// Map a `notify-rust` xdg action id observed by `wait_for_action` to the
/// OSC 99 reverse-path report bytes to send, if any.
///
/// `action` is `"__closed"` (dismissed without action), `"default"`
/// (whole-notification activation), or the button's registered id string
/// (the 0-based `enumerate` index used when registering `.action(...)`).
/// Returns `None` when the source notification didn't request a report for
/// this event (`report_activation` / `close_report` both gate their
/// respective branches).
pub(super) fn osc99_action_report(
    action: &str,
    id: Option<&str>,
    report_activation: bool,
    close_report: bool,
) -> Option<Vec<u8>> {
    match action {
        "__closed" => close_report.then(|| osc99_close_report(id, false)),
        "default" => report_activation.then(|| osc99_activation_report(id, None)),
        other => report_activation.then(|| osc99_activation_report(id, Some(other))),
    }
}

/// An OSC 99 app→terminal control sequence collected from
/// `WindowManipulation::Osc99Control` during `handle_window_manipulation`
/// (Task 99.5c). Paired with a cloned `pty_write_tx` in
/// `app_impl::update()`'s post-loop routing, where it is answered:
/// Task 99.6 (close/alive) and Task 99.7 (query).
#[derive(Debug, Clone)]
pub(super) struct Osc99Control {
    /// Notification id (`i=`) the control refers to, if any.
    pub id: Option<String>,
    /// Which control payload type this is.
    pub kind: Osc99ControlKind,
}

/// Display-gate context for an OSC 99 notification.
///
/// Carries the window state inputs needed to evaluate the `o=` occasion
/// gate ([`NotificationRouter::occasion_allows_display`]).
#[derive(Debug, Clone, Copy)]
pub(super) struct Osc99DisplayContext {
    /// Whether the source window currently has OS focus.
    pub window_focused: bool,
    /// Whether the source window is currently minimized.
    pub window_minimized: bool,
}

/// Monotonic counter disambiguating concurrent OSC 99 icon temp files within
/// a single process run (paired with the process id in the filename).
static OSC99_ICON_TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

impl NotificationRouter {
    /// Route an OSC 99 stateful notification to the toast leg and/or the OS
    /// notification daemon, honouring the occasion gate, urgency, sound,
    /// expiry, buttons, and icon. 99.5a is fire-and-forget (no handle
    /// retention — that is 99.5b); it records the notification id in `live`
    /// for later update/close reconciliation.
    ///
    /// Does nothing when the notification system is disabled
    /// ([`NotificationsConfig::enabled`] is `false`), when the OSC 99
    /// kill-switch is off ([`NotificationsConfig::osc_99`] is `false`,
    /// Task 99.8), or when the occasion gate says the notification should
    /// not be displayed right now (in which case it is also not recorded in
    /// `live` — an occasion-suppressed notification never happened as far
    /// as later update/close tracking is concerned).
    pub(super) fn route_osc99(
        data: &Notification99Data,
        config: &NotificationsConfig,
        ctx: Osc99DisplayContext,
        toasts: &mut ToastStack,
        icon_cache: &mut HashMap<String, Vec<u8>>,
        live: &mut HashMap<String, Osc99LiveEntry>,
        pty_write_tx: &Sender<PtyWrite>,
    ) {
        if !config.enabled {
            return;
        }

        // OSC 99 kill-switch (Task 99.8): even with the master switch on, users can
        // disable the kitty stateful protocol specifically.
        if !config.osc_99 {
            return;
        }

        if !Self::occasion_allows_display(data.occasion.as_deref(), ctx) {
            return;
        }

        let resolved_icon = Self::resolve_icon_bytes(data, icon_cache);

        // Toast leg: OSC 99 has no per-category routing config row, so
        // 99.5a always attempts both legs (toast + OS notification) —
        // simple, honest, and works without a notification daemon.
        let summary = data
            .title
            .clone()
            .or_else(|| data.body.clone())
            .unwrap_or_default();
        let detail = data
            .body
            .as_deref()
            .filter(|body| !body.is_empty())
            .map(str::to_owned);
        if data.urgency == Some(2) {
            toasts.error(summary, detail);
        } else {
            toasts.info(summary, detail);
        }

        Self::show_system_osc99(data, resolved_icon, pty_write_tx.clone());

        if let Some(id) = &data.id {
            let entry = Osc99LiveEntry {
                report_activation: data.report_activation,
                close_report: data.close_report,
            };
            tracing::trace!(
                id,
                report_activation = entry.report_activation,
                close_report = entry.close_report,
                "tracking live OSC 99 notification"
            );
            // NOTE: OS-observed closes on Linux write the close report directly
            // from the notification thread but do NOT prune this map (it is a
            // !Send RefCell on the GUI thread, and adding a channel back is
            // disallowed). The map is pruned only on an app-driven p=close
            // (`Osc99Control::Close`, handled in `app_impl`'s control loop). A
            // `p=alive` response may thus transiently over-report a
            // user-dismissed notification — spec-tolerable for a best-effort
            // poll, and it avoids a new GUI<->thread channel.
            live.insert(id.clone(), entry);
        }
    }

    /// Evaluate the OSC 99 `o=` occasion gate.
    ///
    /// - `None` (Always) — always display.
    /// - `Some("unfocused")` — display only when the source window lacks
    ///   focus.
    /// - `Some("invisible")` — display only when the source window is
    ///   minimized. Background-tab occlusion is out of scope for 99.5a
    ///   (documented, not silently dropped — see the 99.5 execution
    ///   decisions in `PLAN_VERSION_110.md`).
    /// - Any other value — treated as Always (forward-compat with occasion
    ///   values not yet recognised by the parser).
    fn occasion_allows_display(occasion: Option<&str>, ctx: Osc99DisplayContext) -> bool {
        match occasion {
            Some("unfocused") => !ctx.window_focused,
            Some("invisible") => ctx.window_minimized,
            // `None` (Always) and any other/unrecognised value both display
            // unconditionally.
            None | Some(_) => true,
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
    fn resolve_icon_bytes(
        data: &Notification99Data,
        icon_cache: &mut HashMap<String, Vec<u8>>,
    ) -> Option<Vec<u8>> {
        if let Some(bytes) = &data.icon_data {
            if let Some(key) = &data.icon_cache_key {
                icon_cache.insert(key.clone(), bytes.clone());
            }
            Some(bytes.clone())
        } else {
            data.icon_cache_key
                .as_ref()
                .and_then(|key| icon_cache.get(key).cloned())
        }
    }

    /// Write transmitted icon bytes to a uniquely-named temp file.
    ///
    /// `tempfile` is only a dev-dependency of the `freminal` crate (not a
    /// production dependency), so this writes directly into
    /// [`std::env::temp_dir`] with a name disambiguated by the notification
    /// id, a monotonic per-process counter, and the process id. The caller
    /// is responsible for best-effort removal after use.
    fn write_icon_temp_file(id: Option<&str>, bytes: &[u8]) -> std::io::Result<std::path::PathBuf> {
        let counter = OSC99_ICON_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let sanitized_id: String = id
            .unwrap_or("noid")
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .collect();
        let filename = format!("freminal-osc99-icon-{sanitized_id}-{pid}-{counter}.png");
        let path = std::env::temp_dir().join(filename);
        std::fs::write(&path, bytes)?;
        Ok(path)
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

    /// Build a `notify-rust` `Notification` from the extracted OSC 99
    /// fields, applying urgency, buttons, sound, timeout, and icon (as a
    /// temp file when transmitted bytes are given, else by name).
    ///
    /// Returns the built notification plus the icon temp file path (if one
    /// was written), so the caller can clean it up after `.show()`.
    #[allow(clippy::too_many_arguments)]
    fn build_osc99_notification(
        id: Option<&str>,
        title: Option<&str>,
        body: &str,
        app_name: Option<&str>,
        button_labels: &[String],
        urgency: Option<u8>,
        sound: Option<&str>,
        expire_ms: Option<i64>,
        icon_names: &[String],
        resolved_icon_bytes: Option<Vec<u8>>,
    ) -> (notify_rust::Notification, Option<std::path::PathBuf>) {
        let mut notification = notify_rust::Notification::new();
        notification
            .appname(app_name.unwrap_or("freminal"))
            .summary(title.unwrap_or(body))
            .body(body);

        // See `Self::show_system` for the platform rationale: `urgency()` is
        // unavailable on macOS.
        #[cfg(not(target_os = "macos"))]
        {
            let urgency_level = match urgency {
                Some(0) => notify_rust::Urgency::Low,
                Some(2) => notify_rust::Urgency::Critical,
                _ => notify_rust::Urgency::Normal,
            };
            notification.urgency(urgency_level);
        }
        #[cfg(target_os = "macos")]
        let _ = urgency;

        for (idx, label) in button_labels.iter().enumerate() {
            notification.action(&idx.to_string(), label);
        }

        if let Some(sound_name) = sound {
            notification.sound_name(sound_name);
        }

        let timeout = match expire_ms {
            Some(0) => notify_rust::Timeout::Never,
            Some(ms) if ms > 0 => {
                let ms: u32 = ms.value_into().unwrap_or(u32::MAX);
                notify_rust::Timeout::Milliseconds(ms)
            }
            // `None` (OS default) and negative-and-nonzero (not a value the
            // 99.4 mapping produces -- `-1` is normalised to `None` -- but
            // matched exhaustively rather than relying on that invariant)
            // both fall back to the server default.
            None | Some(_) => notify_rust::Timeout::Default,
        };
        notification.timeout(timeout);

        let mut icon_temp_path: Option<std::path::PathBuf> = None;
        if let Some(bytes) = resolved_icon_bytes {
            match Self::write_icon_temp_file(id, &bytes) {
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
        } else if let Some(first) = icon_names.first() {
            notification.icon(first);
        }

        (notification, icon_temp_path)
    }

    /// Show an OS notification for an OSC 99 notification, retaining the
    /// handle where the platform allows it so activation/close can be
    /// reported back to the originating pane's PTY (Task 99.5b + 99.6).
    ///
    /// On Linux/BSD (`notify-rust`'s D-Bus backend), the handle's
    /// `wait_for_action` blocks the spawned thread for the notification's
    /// lifetime, observing whole-notification activation, button
    /// activation, and dismissal ("closed") — each writes the matching
    /// report to `pty_write_tx` when the source notification requested it
    /// (`a=report` / `c=1`). On macOS/Windows, `notify-rust` does not expose
    /// an observable handle from a background thread (the macOS callback
    /// needs the main run loop), so a `c=1` close report is emitted
    /// immediately in the `untracked` form and no activation reports are
    /// sent. The icon temp file (if any) is removed on a best-effort basis
    /// once the daemon has read it; cleanup failure never fails the
    /// notification.
    fn show_system_osc99(
        data: &Notification99Data,
        resolved_icon_bytes: Option<Vec<u8>>,
        pty_write_tx: Sender<PtyWrite>,
    ) {
        let id = data.id.clone();
        let title = data.title.clone();
        let body = data.body.clone().unwrap_or_default();
        let app_name = data.app_name.clone();
        let button_labels = data.button_labels.clone();
        let urgency = data.urgency;
        let sound = data.sound.clone();
        let expire_ms = data.expire_ms;
        let icon_names = data.icon_names.clone();
        let report_activation = data.report_activation;
        let close_report = data.close_report;

        let builder = std::thread::Builder::new().name("freminal-notify-99".to_owned());
        if let Err(e) = builder.spawn(move || {
            let (notification, mut icon_temp_path) = Self::build_osc99_notification(
                id.as_deref(),
                title.as_deref(),
                &body,
                app_name.as_deref(),
                &button_labels,
                urgency,
                sound.as_deref(),
                expire_ms,
                &icon_names,
                resolved_icon_bytes,
            );

            // Linux/BSD: `notify-rust`'s D-Bus backend returns a handle that
            // can observe activation/close, but `wait_for_action` blocks for
            // the notification's lifetime — so the icon temp file is cleaned
            // up as soon as the daemon has shown it, not after the blocking
            // call returns.
            #[cfg(all(unix, not(target_os = "macos")))]
            match notification.show() {
                Ok(handle) => {
                    if let Some(path) = icon_temp_path.take() {
                        Self::remove_icon_temp_file(&path);
                    }
                    handle.wait_for_action(move |action| {
                        if let Some(bytes) = osc99_action_report(
                            action,
                            id.as_deref(),
                            report_activation,
                            close_report,
                        ) {
                            send_or_log!(
                                pty_write_tx,
                                PtyWrite::Write(bytes),
                                "Failed to send OSC 99 activation/close report"
                            );
                        }
                    });
                }
                Err(e) => {
                    tracing::warn!("failed to show OSC 99 desktop notification: {e}");
                    if let Some(path) = icon_temp_path.take() {
                        Self::remove_icon_temp_file(&path);
                    }
                }
            }

            // macOS/Windows: no observable handle from a background thread
            // (the macOS activation callback requires the main run loop), so
            // emit the `untracked` close form immediately when requested and
            // send no activation reports this cycle.
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            {
                if let Err(e) = notification.show() {
                    tracing::warn!("failed to show OSC 99 desktop notification: {e}");
                }
                let _ = report_activation;
                if close_report {
                    let bytes = osc99_close_report(id.as_deref(), true);
                    send_or_log!(
                        pty_write_tx,
                        PtyWrite::Write(bytes),
                        "Failed to send OSC 99 untracked close report"
                    );
                }
                if let Some(path) = icon_temp_path.take() {
                    Self::remove_icon_temp_file(&path);
                }
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

    fn osc_req(body: &str) -> NotificationRequest {
        NotificationRequest {
            kind: NotificationKind::OscText,
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
        req.kind = NotificationKind::Info;
        assert_eq!(req.summary(), "Notification");
    }

    #[test]
    fn summary_uses_explicit_title_when_present() {
        let req = NotificationRequest {
            kind: NotificationKind::OscText,
            title: Some("Build".to_owned()),
            body: "done".to_owned(),
        };
        assert_eq!(req.summary(), "Build");
    }

    #[test]
    fn routing_for_maps_each_kind() {
        let mut config = NotificationsConfig {
            routing_error: NotificationRouting::Both,
            routing_info: NotificationRouting::Toast,
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
        assert_eq!(
            NotificationRouter::routing_for(NotificationKind::Info, &config),
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
            routing_info: NotificationRouting::Toast,
            ..NotificationsConfig::default()
        };
        let mut toasts = ToastStack::default();
        NotificationRouter::route_test(&NotificationRequest::sample(), &config, true, &mut toasts);
        assert_eq!(toasts.len(), 1);
    }

    #[test]
    fn sample_request_is_info_kind() {
        let req = NotificationRequest::sample();
        assert_eq!(req.kind, NotificationKind::Info);
        assert_eq!(req.summary(), "Freminal");
    }

    #[test]
    fn enabled_toast_routing_pushes_one_toast() {
        let config = NotificationsConfig {
            enabled: true,
            routing_info: NotificationRouting::Toast,
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
            routing_info: NotificationRouting::System,
            ..NotificationsConfig::default()
        };
        let mut toasts = ToastStack::default();
        // System-only routing never pushes a toast (the thread is best-effort
        // and not asserted here).
        NotificationRouter::route(&osc_req("hello"), &config, false, &mut toasts);
        assert_eq!(toasts.len(), 0);
    }

    fn finished_block(exit_code: Option<i32>, dur_secs: u64) -> CommandBlock {
        use freminal_common::buffer_states::command_block::CommandBlockId;
        use std::time::{Duration, SystemTime};
        let executed = SystemTime::now();
        CommandBlock {
            id: CommandBlockId::next(),
            fid: "t".to_owned(),
            prompt_start_row: 0,
            command_start_row: Some(0),
            output_start_row: Some(0),
            end_row: Some(1),
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
            prompt_start_row: 0,
            command_start_row: Some(0),
            output_start_row: Some(0),
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
            prompt_start_row: 0,
            command_start_row: Some(0),
            // No C marker: the user aborted before any command ran.
            output_start_row: None,
            end_row: Some(0),
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
            routing_info: NotificationRouting::SystemWhenUnfocused,
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
            prompt_start_row: 0,
            command_start_row: Some(0),
            output_start_row: Some(0),
            end_row: Some(1),
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
            title: Some("Title".to_owned()),
            body: Some("Body".to_owned()),
            icon_data: None,
            icon_names: Vec::new(),
            icon_cache_key: None,
            button_labels: Vec::new(),
            report_activation: false,
            focus_on_activation: true,
            close_report: false,
            urgency: None,
            occasion: None,
            sound: None,
            app_name: None,
            notification_type: Vec::new(),
            expire_ms: None,
        }
    }

    fn osc99_ctx(window_focused: bool, window_minimized: bool) -> Osc99DisplayContext {
        Osc99DisplayContext {
            window_focused,
            window_minimized,
        }
    }

    #[test]
    fn route_osc99_master_gate_disabled_does_nothing() {
        let config = NotificationsConfig::default(); // enabled = false
        let mut toasts = ToastStack::default();
        let mut icon_cache = HashMap::new();
        let mut live = HashMap::new();
        let (tx, _rx) = crossbeam_channel::unbounded();
        let data = default_n99();

        NotificationRouter::route_osc99(
            &data,
            &config,
            osc99_ctx(true, false),
            &mut toasts,
            &mut icon_cache,
            &mut live,
            &tx,
        );

        assert_eq!(toasts.len(), 0);
        assert!(live.is_empty());
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
        let mut icon_cache = HashMap::new();
        let mut live = HashMap::new();
        let (tx, _rx) = crossbeam_channel::unbounded();
        let data = default_n99();

        NotificationRouter::route_osc99(
            &data,
            &config,
            osc99_ctx(true, false),
            &mut toasts,
            &mut icon_cache,
            &mut live,
            &tx,
        );

        assert_eq!(toasts.len(), 0);
        assert!(live.is_empty());
    }

    #[test]
    fn route_osc99_occasion_always_displays_regardless_of_focus() {
        let config = enabled_config(0.0);
        let mut toasts = ToastStack::default();
        let mut icon_cache = HashMap::new();
        let mut live = HashMap::new();
        let (tx, _rx) = crossbeam_channel::unbounded();
        let data = default_n99(); // occasion: None => Always

        NotificationRouter::route_osc99(
            &data,
            &config,
            osc99_ctx(true, false),
            &mut toasts,
            &mut icon_cache,
            &mut live,
            &tx,
        );

        assert_eq!(
            toasts.len(),
            1,
            "Always occasion must display even when focused"
        );
    }

    #[test]
    fn route_osc99_occasion_unfocused_gates_on_focus() {
        let config = enabled_config(0.0);
        let data = Notification99Data {
            occasion: Some("unfocused".to_owned()),
            ..default_n99()
        };

        // Focused: must NOT display.
        let mut toasts = ToastStack::default();
        let mut icon_cache = HashMap::new();
        let mut live = HashMap::new();
        let (tx, _rx) = crossbeam_channel::unbounded();
        NotificationRouter::route_osc99(
            &data,
            &config,
            osc99_ctx(true, false),
            &mut toasts,
            &mut icon_cache,
            &mut live,
            &tx,
        );
        assert_eq!(
            toasts.len(),
            0,
            "unfocused occasion must not fire while focused"
        );
        assert!(live.is_empty());

        // Unfocused: must display.
        let mut toasts = ToastStack::default();
        let mut icon_cache = HashMap::new();
        let mut live = HashMap::new();
        let (tx, _rx) = crossbeam_channel::unbounded();
        NotificationRouter::route_osc99(
            &data,
            &config,
            osc99_ctx(false, false),
            &mut toasts,
            &mut icon_cache,
            &mut live,
            &tx,
        );
        assert_eq!(
            toasts.len(),
            1,
            "unfocused occasion must fire while unfocused"
        );
    }

    #[test]
    fn route_osc99_occasion_invisible_gates_on_minimized() {
        let config = enabled_config(0.0);
        let data = Notification99Data {
            occasion: Some("invisible".to_owned()),
            ..default_n99()
        };

        // Not minimized: must NOT display.
        let mut toasts = ToastStack::default();
        let mut icon_cache = HashMap::new();
        let mut live = HashMap::new();
        let (tx, _rx) = crossbeam_channel::unbounded();
        NotificationRouter::route_osc99(
            &data,
            &config,
            osc99_ctx(true, false),
            &mut toasts,
            &mut icon_cache,
            &mut live,
            &tx,
        );
        assert_eq!(
            toasts.len(),
            0,
            "invisible occasion must not fire while visible"
        );

        // Minimized: must display.
        let mut toasts = ToastStack::default();
        let mut icon_cache = HashMap::new();
        let mut live = HashMap::new();
        let (tx, _rx) = crossbeam_channel::unbounded();
        NotificationRouter::route_osc99(
            &data,
            &config,
            osc99_ctx(true, true),
            &mut toasts,
            &mut icon_cache,
            &mut live,
            &tx,
        );
        assert_eq!(
            toasts.len(),
            1,
            "invisible occasion must fire while minimized"
        );
    }

    #[test]
    fn route_osc99_unrecognised_occasion_treated_as_always() {
        let config = enabled_config(0.0);
        let data = Notification99Data {
            occasion: Some("something_new".to_owned()),
            ..default_n99()
        };
        let mut toasts = ToastStack::default();
        let mut icon_cache = HashMap::new();
        let mut live = HashMap::new();
        let (tx, _rx) = crossbeam_channel::unbounded();

        NotificationRouter::route_osc99(
            &data,
            &config,
            osc99_ctx(true, false),
            &mut toasts,
            &mut icon_cache,
            &mut live,
            &tx,
        );

        assert_eq!(toasts.len(), 1);
    }

    #[test]
    fn route_osc99_icon_cache_populated_from_transmitted_bytes() {
        let mut icon_cache: HashMap<String, Vec<u8>> = HashMap::new();
        let data = Notification99Data {
            icon_data: Some(vec![1, 2, 3, 4]),
            icon_cache_key: Some("k".to_owned()),
            ..default_n99()
        };

        let resolved = NotificationRouter::resolve_icon_bytes(&data, &mut icon_cache);

        assert_eq!(resolved, Some(vec![1, 2, 3, 4]));
        assert_eq!(icon_cache.get("k"), Some(&vec![1, 2, 3, 4]));
    }

    #[test]
    fn route_osc99_icon_cache_reused_on_subsequent_g_only_notification() {
        let mut icon_cache: HashMap<String, Vec<u8>> = HashMap::new();
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
        let mut icon_cache: HashMap<String, Vec<u8>> = HashMap::new();
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
        let mut icon_cache: HashMap<String, Vec<u8>> = HashMap::new();
        let data = default_n99();

        assert_eq!(
            NotificationRouter::resolve_icon_bytes(&data, &mut icon_cache),
            None
        );
    }

    #[test]
    fn route_osc99_records_live_entry_when_id_present() {
        let config = enabled_config(0.0);
        let mut toasts = ToastStack::default();
        let mut icon_cache = HashMap::new();
        let mut live = HashMap::new();
        let (tx, _rx) = crossbeam_channel::unbounded();
        let data = Notification99Data {
            id: Some("n1".to_owned()),
            report_activation: true,
            close_report: true,
            ..default_n99()
        };

        NotificationRouter::route_osc99(
            &data,
            &config,
            osc99_ctx(true, false),
            &mut toasts,
            &mut icon_cache,
            &mut live,
            &tx,
        );

        let entry = live.get("n1").expect("live entry recorded");
        assert!(entry.report_activation);
        assert!(entry.close_report);
    }

    #[test]
    fn route_osc99_no_live_entry_when_id_absent() {
        let config = enabled_config(0.0);
        let mut toasts = ToastStack::default();
        let mut icon_cache = HashMap::new();
        let mut live = HashMap::new();
        let (tx, _rx) = crossbeam_channel::unbounded();
        let data = default_n99(); // id: None

        NotificationRouter::route_osc99(
            &data,
            &config,
            osc99_ctx(true, false),
            &mut toasts,
            &mut icon_cache,
            &mut live,
            &tx,
        );

        assert!(live.is_empty());
    }

    #[test]
    fn route_osc99_title_and_body_push_a_toast() {
        let config = enabled_config(0.0);
        let mut toasts = ToastStack::default();
        let mut icon_cache = HashMap::new();
        let mut live = HashMap::new();
        let (tx, _rx) = crossbeam_channel::unbounded();
        let data = default_n99();

        NotificationRouter::route_osc99(
            &data,
            &config,
            osc99_ctx(true, false),
            &mut toasts,
            &mut icon_cache,
            &mut live,
            &tx,
        );

        assert_eq!(toasts.len(), 1);
    }

    #[test]
    fn route_osc99_occasion_suppressed_records_no_live_entry() {
        // An occasion-suppressed notification is not displayed, so it must
        // not be tracked for update/close either -- as far as later
        // reconciliation is concerned, it never happened.
        let config = enabled_config(0.0);
        let mut toasts = ToastStack::default();
        let mut icon_cache = HashMap::new();
        let mut live = HashMap::new();
        let (tx, _rx) = crossbeam_channel::unbounded();
        let data = Notification99Data {
            id: Some("n2".to_owned()),
            occasion: Some("unfocused".to_owned()),
            ..default_n99()
        };

        NotificationRouter::route_osc99(
            &data,
            &config,
            osc99_ctx(true, false),
            &mut toasts,
            &mut icon_cache,
            &mut live,
            &tx,
        );

        assert_eq!(toasts.len(), 0);
        assert!(live.is_empty());
    }

    // ── 99.5c: forget_osc99 prune helper ──────────────────────────────────────

    #[test]
    fn forget_osc99_removes_existing_entry() {
        let mut live = HashMap::new();
        live.insert(
            "n1".to_owned(),
            Osc99LiveEntry {
                report_activation: true,
                close_report: true,
            },
        );

        let removed = forget_osc99(&mut live, "n1");

        assert!(removed);
        assert!(live.is_empty());
    }

    #[test]
    fn forget_osc99_missing_id_returns_false() {
        let mut live: HashMap<String, Osc99LiveEntry> = HashMap::new();

        let removed = forget_osc99(&mut live, "does-not-exist");

        assert!(!removed);
    }

    // ── 99.5b + 99.6: reverse-path report builders ────────────────────────

    #[test]
    fn osc99_activation_report_with_id_no_button() {
        assert_eq!(
            osc99_activation_report(Some("abc"), None),
            b"\x1b]99;i=abc;\x1b\\".to_vec()
        );
    }

    #[test]
    fn osc99_activation_report_with_id_and_button() {
        assert_eq!(
            osc99_activation_report(Some("abc"), Some("2")),
            b"\x1b]99;i=abc;2\x1b\\".to_vec()
        );
    }

    #[test]
    fn osc99_activation_report_no_id_defaults_to_zero() {
        assert_eq!(
            osc99_activation_report(None, None),
            b"\x1b]99;i=0;\x1b\\".to_vec()
        );
    }

    #[test]
    fn osc99_close_report_tracked() {
        assert_eq!(
            osc99_close_report(Some("abc"), false),
            b"\x1b]99;i=abc:p=close;\x1b\\".to_vec()
        );
    }

    #[test]
    fn osc99_close_report_untracked() {
        assert_eq!(
            osc99_close_report(Some("abc"), true),
            b"\x1b]99;i=abc:p=close;untracked\x1b\\".to_vec()
        );
    }

    #[test]
    fn osc99_close_report_no_id_defaults_to_zero() {
        assert_eq!(
            osc99_close_report(None, false),
            b"\x1b]99;i=0:p=close;\x1b\\".to_vec()
        );
    }

    #[test]
    fn osc99_alive_report_with_req_id_and_ids() {
        assert_eq!(
            osc99_alive_report(Some("q1"), &["a".to_owned(), "b".to_owned()]),
            b"\x1b]99;i=q1:p=alive;a,b\x1b\\".to_vec()
        );
    }

    #[test]
    fn osc99_alive_report_empty_live_list() {
        assert_eq!(
            osc99_alive_report(Some("q1"), &[]),
            b"\x1b]99;i=q1:p=alive;\x1b\\".to_vec()
        );
    }

    #[test]
    fn osc99_alive_report_no_req_id_defaults_to_zero() {
        assert_eq!(
            osc99_alive_report(None, &["x".to_owned()]),
            b"\x1b]99;i=0:p=alive;x\x1b\\".to_vec()
        );
    }

    #[test]
    fn osc99_query_response_with_id_matches_exact_bytes() {
        assert_eq!(
            osc99_query_response(Some("q1")),
            b"\x1b]99;i=q1:p=?;a=report:c=1:o=always,unfocused,invisible:p=title,body,icon,buttons,alive,close,?:s=system,silent:u=0,1,2:w=1\x1b\\".to_vec()
        );
    }

    #[test]
    fn osc99_query_response_no_id_defaults_to_zero() {
        let bytes = osc99_query_response(None);
        assert!(bytes.starts_with(b"\x1b]99;i=0:p=?;"));
    }

    #[test]
    fn osc99_capabilities_are_truthful() {
        // Guards the truthful-advertisement decision: activation reporting
        // is advertised, but NOT the `focus` sub-capability (freminal parses
        // `focus_on_activation` but does not act on it). Scoped to the `a=`
        // field specifically — the `o=` field's `unfocused` value legitimately
        // contains the substring "focus".
        let a_field = OSC99_CAPABILITIES
            .split(':')
            .find(|kv| kv.starts_with("a="))
            .expect("a= field present");
        assert_eq!(a_field, "a=report");
        assert!(!a_field.contains("focus"));

        // `p=` payload types must include at least `title` (spec minimum).
        let p_field = OSC99_CAPABILITIES
            .split(':')
            .find(|kv| kv.starts_with("p="))
            .expect("p= field present");
        assert!(p_field.contains("title"));
    }

    #[test]
    fn live_ids_sorted_returns_deterministic_order() {
        let mut live = HashMap::new();
        for id in ["b", "a", "c"] {
            live.insert(
                id.to_owned(),
                Osc99LiveEntry {
                    report_activation: false,
                    close_report: false,
                },
            );
        }

        assert_eq!(live_ids_sorted(&live), vec!["a", "b", "c"]);
    }
}
