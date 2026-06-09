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

use freminal_common::buffer_states::command_block::{CommandBlock, CommandStatus};
use freminal_common::buffer_states::window_manipulation::NotificationKind;
use freminal_common::config::{NotificationRouting, NotificationsConfig};

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
        // Error-category notifications use Critical urgency so they persist
        // and pop as a banner; everything else is Normal. Many Linux
        // notification daemons only raise a banner (rather than silently
        // filing the notification in the tray) when an urgency hint is set.
        let urgency = match req.kind {
            NotificationKind::Error => notify_rust::Urgency::Critical,
            NotificationKind::OscText
            | NotificationKind::CommandFinished
            | NotificationKind::Info => notify_rust::Urgency::Normal,
        };
        let builder = std::thread::Builder::new().name("freminal-notify".to_owned());
        if let Err(e) = builder.spawn(move || {
            if let Err(e) = notify_rust::Notification::new()
                .appname("freminal")
                .summary(&summary)
                .body(&body)
                .urgency(urgency)
                .show()
            {
                tracing::warn!("failed to show desktop notification: {e}");
            }
        }) {
            tracing::warn!("failed to spawn notification thread: {e}");
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
}
