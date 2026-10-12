// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use thiserror::Error;

pub use super::osc99_notification::{Notification99Data, Osc99ControlKind};

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
    /// config.  `source` records which OSC sequence produced this
    /// notification, so the GUI can honour the per-source
    /// `notifications.osc_9` / `notifications.osc_777` enable toggles.
    /// `title` is `None` for OSC 9 and `Some` for OSC 777 when a title is
    /// present.
    Notification {
        /// The notification category, selecting the routing policy.
        kind: NotificationKind,
        /// Which OSC sequence produced this notification.
        source: crate::buffer_states::osc::OscNotifySource,
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
    /// OSC 99 app→terminal control sequence (`p=close` / `p=alive`, Task 99).
    ///
    /// Routed distinctly from display notifications: it drives close
    /// reconciliation or the alive-id list reply rather than a banner. The
    /// capability query `p=?` is never carried here: it is answered on the PTY
    /// thread from the host capabilities (Task 130.5) and does not reach the
    /// GUI, so it never becomes an `Osc99Control`.
    Osc99Control {
        /// Notification id (`i=`) the control refers to. Mandatory: an
        /// id-less `p=close` / `p=alive` is a no-op that never leaves the PTY
        /// thread.
        id: String,
        /// Which control payload type this is.
        kind: Osc99ControlKind,
    },
    /// OSC 99 icon priming: icon bytes transmitted under a `g=` cache key by
    /// a notification that has no title and no body, so nothing is displayed
    /// (kitty caches the icon before it checks for text).
    ///
    /// The GUI stores the bytes in its `g=` icon cache, where a later
    /// notification that names the same `g=` key without transmitting the
    /// bytes finds them. The cache bounds its own size.
    Osc99CacheIcon {
        /// The cache key (`g=`), sanitised. Never empty.
        key: String,
        /// The icon image bytes (`p=icon`, base64-decoded). Never empty.
        data: Vec<u8>,
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
    use crate::buffer_states::osc_notify_99::{NotificationOccasion, NotificationUrgency};
    use crate::buffer_states::osc99_notification::{
        Osc99ActivationFocus, Osc99ActivationReporting, Osc99CloseReporting, Osc99Expiry,
        Osc99Sound,
    };
    use std::num::NonZeroU32;

    #[test]
    fn notification99_variant_round_trips() {
        let data = Notification99Data {
            id: Some("test-id-42".to_owned()),
            title: "Test Title".to_owned(),
            body: Some("Test body text.".to_owned()),
            icon_data: Some(vec![0x89, 0x50, 0x4e, 0x47]),
            icon_names: vec!["dialog-information".to_owned(), "info".to_owned()],
            icon_cache_key: Some("cache-key-1".to_owned()),
            buttons: vec!["OK".to_owned(), "Cancel".to_owned()],
            activation_reporting: Osc99ActivationReporting::Report,
            activation_focus: Osc99ActivationFocus::Focus,
            close_reporting: Osc99CloseReporting::Silent,
            urgency: NotificationUrgency::Normal,
            occasion: NotificationOccasion::Unfocused,
            sound: Osc99Sound::Named("bell".to_owned()),
            app_name: Some("MyApp".to_owned()),
            notification_types: vec!["email".to_owned()],
            expiry: Osc99Expiry::After(NonZeroU32::new(5000).unwrap()),
        };
        let original = WindowManipulation::Notification99(Box::new(data));
        let cloned = original.clone();

        // Pattern-match the clone and assert all fields survived.
        let WindowManipulation::Notification99(ref d) = cloned else {
            panic!("expected Notification99 variant");
        };

        assert_eq!(d.id.as_deref(), Some("test-id-42"));
        assert_eq!(d.title, "Test Title");
        assert_eq!(d.body.as_deref(), Some("Test body text."));
        assert_eq!(
            d.icon_data.as_deref(),
            Some(&[0x89u8, 0x50, 0x4e, 0x47][..])
        );
        assert_eq!(d.icon_names, vec!["dialog-information", "info"]);
        assert_eq!(d.icon_cache_key.as_deref(), Some("cache-key-1"));
        assert_eq!(d.buttons, vec!["OK", "Cancel"]);
        assert_eq!(d.activation_reporting, Osc99ActivationReporting::Report);
        assert_eq!(d.activation_focus, Osc99ActivationFocus::Focus);
        assert_eq!(d.close_reporting, Osc99CloseReporting::Silent);
        assert_eq!(d.urgency, NotificationUrgency::Normal);
        assert_eq!(d.occasion, NotificationOccasion::Unfocused);
        assert_eq!(d.sound, Osc99Sound::Named("bell".to_owned()));
        assert_eq!(d.app_name.as_deref(), Some("MyApp"));
        assert_eq!(d.notification_types, vec!["email"]);
        assert_eq!(d.expiry, Osc99Expiry::After(NonZeroU32::new(5000).unwrap()));

        // Confirm the variant equals the original.
        assert_eq!(original, cloned);
    }
}
