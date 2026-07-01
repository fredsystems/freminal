// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use thiserror::Error;

/// Category of a desktop/in-app notification (Task 76).
///
/// Determines which routing policy in `[notifications]` applies and which
/// toast styling the GUI uses when surfacing the notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationKind {
    /// Free-form text from an OSC 9 / OSC 777 sequence.
    OscText,
    /// A shell command finished (OSC 133 D).
    CommandFinished,
    /// An error-category notification.
    Error,
    /// An informational notification.
    Info,
}

/// Errors produced when converting a raw `(Ps1, Ps2, Ps3)` XTWINOPS parameter
/// triple into a [`WindowManipulation`].
#[derive(Debug, Error, Eq, PartialEq, Clone)]
pub enum WindowManipulationError {
    /// The parameter triple did not match any known XTWINOPS command.
    #[error("unrecognized XTWINOPS command: ({command}, {param_ps2}, {param_ps3})")]
    UnrecognizedCommand {
        /// Primary command code (`Ps1`).
        command: usize,
        /// Second parameter (`Ps2`).
        param_ps2: usize,
        /// Third parameter (`Ps3`).
        param_ps3: usize,
    },
}

/// Data payload for a stateful OSC 99 desktop notification (Task 99).
///
/// Boxed in [`WindowManipulation::Notification99`] to keep the enum size
/// reasonable. All fields are primitive shells; domain enums for urgency,
/// occasion, and action live in the Task 99.1 typed parser, not here.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Notification99Data {
    /// Notification id (`i=`), if any. `None` = no id (report as `i=0`).
    pub id: Option<String>,
    /// Notification title (`p=title` payload), if any.
    pub title: Option<String>,
    /// Notification body (`p=body` payload), if any.
    pub body: Option<String>,
    /// Transmitted icon image bytes (`p=icon`, base64-decoded), if any.
    pub icon_data: Option<Vec<u8>>,
    /// Icon names to resolve (`n=`), first available wins.
    pub icon_names: Vec<String>,
    /// Icon-data cache key (`g=`), if any.
    pub icon_cache_key: Option<String>,
    /// Button labels (`p=buttons`), in order.
    pub button_labels: Vec<String>,
    /// Whether `a=report` was set (activation reports wanted).
    pub report_activation: bool,
    /// Whether `a=focus` was set (focus the source window on activation).
    /// Default `true` (the OSC 99 default is `focus`).
    pub focus_on_activation: bool,
    /// Whether `c=1` was set (close report wanted).
    pub close_report: bool,
    /// Urgency (`u=`): 0 low, 1 normal, 2 critical. `None` = unset/normal.
    pub urgency: Option<u8>,
    /// Occasion (`o=`): `always` / `unfocused` / `invisible`. `None` = default.
    pub occasion: Option<String>,
    /// Sound name (`s=`), if any.
    pub sound: Option<String>,
    /// Application name (`f=`), if any.
    pub app_name: Option<String>,
    /// Notification type/category (`t=`), may repeat.
    pub notification_type: Vec<String>,
    /// Auto-expire after N ms (`w=`). `None` = OS default.
    pub expire_ms: Option<i64>,
}

/// The three OSC 99 app→terminal control payload types that are NOT display
/// requests: they require a terminal response or state change rather than a
/// notification banner (Task 99).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Osc99ControlKind {
    /// `p=close`: the application asks to close the notification with this id.
    Close,
    /// `p=alive`: liveness poll; the terminal answers with the live-id list.
    Alive,
    /// `p=?`: capability query; the terminal answers with its supported keys.
    Query,
}

/// Window manipulation commands (XTWINOPS / xterm CSI Ps ; Ps ; Ps t).
///
/// This enum covers two categories:
///
/// **Viewport operations** (fully implemented) — forwarded to the egui
/// viewport via `send_viewport_cmd()` by the GUI's `handle_window_manipulation`
/// function:
/// - De-iconify / minimize, move, resize, raise/lower, maximize/restore,
///   enter/exit/toggle full-screen.
/// - Title-bar set/save/restore stack.
///
/// **Report queries** (implemented) — routing is split between two sites:
/// - **GUI-side** (`handle_window_manipulation`): `ReportWindowState`,
///   `ReportWindowPosition*`, `ReportWindowSize*`, `ReportIconLabel`,
///   `ReportTitle` — the GUI measures viewport geometry and sends a
///   formatted escape-sequence response directly to the PTY.
/// - **PTY-side** (`TerminalHandler::handle_window_manipulation`):
///   `ReportCharacterSizeInPixels`, `ReportTerminalSizeInCharacters`,
///   `ReportRootWindowSizeInCharacters` — handled synchronously on the PTY
///   thread so that responses arrive in the same batch as DA1.
///
/// **OSC 52 clipboard** (fully implemented) — handled in
/// `handle_window_manipulation` via dedicated match arms:
/// - `SetClipboard` copies decoded text to the system clipboard via
///   `ui.ctx().copy_text()`.
/// - `QueryClipboard` responds with an empty OSC 52 payload because egui's
///   public API does not support reading the clipboard.  This is the
///   safe/secure default adopted by many terminals.
///
/// **Intentional stubs / no-ops:**
/// - `RaiseWindowToTopOfStackingOrder`, `LowerWindowToBottomOfStackingOrder`,
///   `RefreshWindow` — these have no meaningful egui equivalent and are
///   silently accepted (the PTY application rarely depends on them).
/// - `ResizeWindowToLinesAndColumns` — implemented but the row/column to
///   pixel calculation uses `font_width` / `font_height` from the snapshot,
///   so it may be off by one on high-DPI displays until the metrics path is
///   pixel-perfect.
#[allow(clippy::module_name_repetitions)]
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum WindowManipulation {
    DeIconifyWindow,
    MinimizeWindow,
    MoveWindow(usize, usize),
    ResizeWindow(usize, usize),
    RaiseWindowToTopOfStackingOrder,
    LowerWindowToBottomOfStackingOrder,
    RefreshWindow,
    ResizeWindowToLinesAndColumns(usize, usize),
    MaximizeWindow,
    RestoreNonMaximizedWindow,
    NotFullScreen,
    FullScreen,
    ToggleFullScreen,
    ReportWindowState,
    ReportWindowPositionWholeWindow,
    ReportWindowPositionTextArea,
    ReportWindowSizeInPixels,
    ReportWindowTextAreaSizeInPixels,
    ReportRootWindowSizeInPixels,
    ReportCharacterSizeInPixels,
    ReportTerminalSizeInCharacters,
    ReportRootWindowSizeInCharacters,
    ReportIconLabel,
    ReportTitle,
    SetTitleBarText(String),
    SaveWindowTitleToStack,
    RestoreWindowTitleFromStack,
    /// OSC 52 clipboard set: selection name + decoded content.
    SetClipboard(String, String),
    /// OSC 52 clipboard query: selection name.  The GUI should read the
    /// clipboard and respond with `OSC 52 ; <sel> ; <base64> ST`.
    QueryClipboard(String),
    /// Terminal bell (BEL, `\x07`).
    ///
    /// Forwarded to the GUI so it can trigger a visual bell indicator
    /// and/or mark the originating tab as having an unacknowledged bell.
    Bell,
    /// Desktop/in-app notification request (OSC 9 / OSC 777, Task 76).
    ///
    /// Forwarded to the GUI so it can route the notification to an in-app
    /// toast and/or the system notification daemon per the `[notifications]`
    /// config.  `title` is `None` for OSC 9 and `Some` for OSC 777 when a
    /// title is present.
    Notification {
        /// The notification category, selecting the routing policy.
        kind: NotificationKind,
        /// The notification title, if any.
        title: Option<String>,
        /// The notification body text.
        body: String,
    },
    /// Stateful desktop notification request (OSC 99, Task 99).
    ///
    /// The stateful sibling of `Notification` (OSC 9/777). Carries the full
    /// OSC 99 metadata superset in a [`Box<Notification99Data>`] to avoid
    /// inflating the enum's in-memory size.  The boxed struct's fields are
    /// primitive "shells" populated by Task 99.4 from the typed `Osc99Command`
    /// parser (Task 99.1); domain enums for urgency/occasion/action live in
    /// that parser, not here.  Transported via the `WindowCommand` channel
    /// (not the snapshot) and rendered by Task 99.5.
    Notification99(Box<Notification99Data>),
    /// OSC 99 app→terminal control sequence (`p=close`/`p=alive`/`p=?`, Task 99).
    ///
    /// Routed distinctly from display notifications: it drives a terminal
    /// response (close reconciliation, alive-id list, or capability
    /// handshake) rather than a banner. The reverse writes land in Tasks
    /// 99.6/99.7.
    Osc99Control {
        /// Notification id (`i=`) the control refers to, if any.
        id: Option<String>,
        /// Which control payload type this is.
        kind: Osc99ControlKind,
    },
}

impl TryFrom<(usize, usize, usize)> for WindowManipulation {
    type Error = WindowManipulationError;

    fn try_from(
        (command, param_ps2, param_ps3): (usize, usize, usize),
    ) -> Result<Self, WindowManipulationError> {
        match (command, param_ps2, param_ps3) {
            (1, _, _) => Ok(Self::DeIconifyWindow),
            (2, _, _) => Ok(Self::MinimizeWindow),
            (3, x, y) => Ok(Self::MoveWindow(x, y)),
            (4, x, y) => Ok(Self::ResizeWindow(x, y)),
            (5, _, _) => Ok(Self::RaiseWindowToTopOfStackingOrder),
            (6, _, _) => Ok(Self::LowerWindowToBottomOfStackingOrder),
            (7, _, _) => Ok(Self::RefreshWindow),
            (8, x, y) => Ok(Self::ResizeWindowToLinesAndColumns(x, y)),
            (9, 1, _) => Ok(Self::MaximizeWindow),
            (9, 0, _) => Ok(Self::RestoreNonMaximizedWindow),
            (10, 0, _) => Ok(Self::NotFullScreen),
            (10, 1, _) => Ok(Self::FullScreen),
            (10, 2, _) => Ok(Self::ToggleFullScreen),
            (11, _, _) => Ok(Self::ReportWindowState),
            (13, 0 | 1, _) => Ok(Self::ReportWindowPositionWholeWindow),
            (13, 2, 0) => Ok(Self::ReportWindowPositionTextArea),
            (14, 0 | 1, _) => Ok(Self::ReportWindowSizeInPixels),
            (14, 2, _) => Ok(Self::ReportWindowTextAreaSizeInPixels),
            (15, _, _) => Ok(Self::ReportRootWindowSizeInPixels),
            (16, _, _) => Ok(Self::ReportCharacterSizeInPixels),
            (18, _, _) => Ok(Self::ReportTerminalSizeInCharacters),
            (19, _, _) => Ok(Self::ReportRootWindowSizeInCharacters),
            (20, _, _) => Ok(Self::ReportIconLabel),
            (21, _, _) => Ok(Self::ReportTitle),
            (22, 0..=2, _) => Ok(Self::SaveWindowTitleToStack),
            (23, 0..=2, _) => Ok(Self::RestoreWindowTitleFromStack),
            (24, 0..=2, _) => Ok(Self::SetTitleBarText(String::new())),
            _ => Err(WindowManipulationError::UnrecognizedCommand {
                command,
                param_ps2,
                param_ps3,
            }),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn notification99_variant_round_trips() {
        let data = Notification99Data {
            id: Some("test-id-42".to_owned()),
            title: Some("Test Title".to_owned()),
            body: Some("Test body text.".to_owned()),
            icon_data: Some(vec![0x89, 0x50, 0x4e, 0x47]),
            icon_names: vec!["dialog-information".to_owned(), "info".to_owned()],
            icon_cache_key: Some("cache-key-1".to_owned()),
            button_labels: vec!["OK".to_owned(), "Cancel".to_owned()],
            report_activation: true,
            focus_on_activation: true,
            close_report: false,
            urgency: Some(1),
            occasion: Some("unfocused".to_owned()),
            sound: Some("bell".to_owned()),
            app_name: Some("MyApp".to_owned()),
            notification_type: vec!["email".to_owned()],
            expire_ms: Some(5000),
        };
        let original = WindowManipulation::Notification99(Box::new(data));
        let cloned = original.clone();

        // Pattern-match the clone and assert all fields survived.
        let WindowManipulation::Notification99(ref d) = cloned else {
            panic!("expected Notification99 variant");
        };

        assert_eq!(d.id.as_deref(), Some("test-id-42"));
        assert_eq!(d.title.as_deref(), Some("Test Title"));
        assert_eq!(d.body.as_deref(), Some("Test body text."));
        assert_eq!(
            d.icon_data.as_deref(),
            Some(&[0x89u8, 0x50, 0x4e, 0x47][..])
        );
        assert_eq!(d.icon_names, vec!["dialog-information", "info"]);
        assert_eq!(d.icon_cache_key.as_deref(), Some("cache-key-1"));
        assert_eq!(d.button_labels, vec!["OK", "Cancel"]);
        assert!(d.report_activation);
        assert!(d.focus_on_activation);
        assert!(!d.close_report);
        assert_eq!(d.urgency, Some(1));
        assert_eq!(d.occasion.as_deref(), Some("unfocused"));
        assert_eq!(d.sound.as_deref(), Some("bell"));
        assert_eq!(d.app_name.as_deref(), Some("MyApp"));
        assert_eq!(d.notification_type, vec!["email"]);
        assert_eq!(d.expire_ms, Some(5000));

        // Confirm the variant equals the original.
        assert_eq!(original, cloned);
    }
}
