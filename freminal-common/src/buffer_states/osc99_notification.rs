// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Transport types for OSC 99 (kitty desktop notifications).
//!
//! These are the typed values the terminal handler hands to the GUI inside
//! `WindowManipulation::Notification99` / `WindowManipulation::Osc99Control`.
//! The parser-side types (`Osc99Command` and the metadata enums only the
//! parser and handler use) live in [`super::osc_notify_99`], which imports the
//! shared value enums from here; this module holds what crosses the
//! PTY-thread -> GUI-thread boundary.
//!
//! Reference: <https://sw.kovidgoyal.net/kitty/desktop-notifications/>

use std::num::NonZeroU32;

/// Whether the application asked to be told when the notification is
/// activated (`a=report`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Osc99ActivationReporting {
    /// `a=report` was set: send an activation report back to the application.
    Report,
    /// `a=report` was not set: activation is not reported.
    Silent,
}

/// Whether activating the notification should focus the source window
/// (`a=focus`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Osc99ActivationFocus {
    /// `a=focus` is in effect (the OSC 99 default).
    Focus,
    /// `a=-focus` removed the focus action.
    NoFocus,
}

/// Whether the application asked to be told when the notification is closed
/// (`c=1`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Osc99CloseReporting {
    /// `c=1` was set: send a close report back to the application.
    Report,
    /// `c=1` was not set: closing is not reported.
    Silent,
}

/// Auto-expiry of the notification (`w=`), in milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Osc99Expiry {
    /// `w=-1` (the default) or unset: the OS / notification daemon decides.
    OsDefault,
    /// `w=0`: the notification never expires.
    Never,
    /// `w=N`, N > 0: expire after this many milliseconds.
    After(NonZeroU32),
}

/// Sound requested for the notification (`s=`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Osc99Sound {
    /// `s=system` or unset: the notification daemon's default sound.
    System,
    /// `s=silent`: no sound.
    Silent,
    /// Any other `s=` value: a freedesktop sound name forwarded verbatim.
    Named(String),
}

/// The OSC 99 app->terminal control payload types the GUI acts on.
///
/// They require a terminal response or state change rather than a
/// notification banner (Task 99). `p=?` is not among them: the PTY thread
/// answers it itself (Task 130.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Osc99ControlKind {
    /// `p=close`: the application asks to close the notification with this id.
    Close,
    /// `p=alive`: liveness poll; the terminal answers with the live-id list.
    Alive,
}

/// Urgency level of an OSC 99 notification (`u=` key).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationUrgency {
    /// `u=0`.
    Low,
    /// `u=1`.
    Normal,
    /// `u=2`.
    Critical,
}

/// Display occasion of an OSC 99 notification (`o=` key).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationOccasion {
    /// `o=always` (default): honour unconditionally.
    Always,
    /// `o=unfocused`: only when the source window lacks focus.
    Unfocused,
    /// `o=invisible`: only when unfocused and not visible.
    Invisible,
}

/// Data payload for a stateful OSC 99 desktop notification.
///
/// Boxed in `WindowManipulation::Notification99` to keep the enum size
/// reasonable. Built by the terminal handler from the reassembled
/// notification; consumed by the GUI's notification router.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Notification99Data {
    /// Notification id (`i=`), if any. `None` = no id (report as `i=0`).
    pub id: Option<String>,
    /// Notification title. Never empty: the terminal handler promotes the
    /// body into the title when no title arrived, and does not display a
    /// notification that has neither (see
    /// `WindowManipulation::Osc99CacheIcon` for the one such case that is
    /// still acted on).
    pub title: String,
    /// Notification body (`p=body` payload), if any.
    pub body: Option<String>,
    /// Transmitted icon image bytes (`p=icon`, base64-decoded), if any.
    pub icon_data: Option<Vec<u8>>,
    /// Icon names to resolve (`n=`), first available wins.
    pub icon_names: Vec<String>,
    /// Icon-data cache key (`g=`), if any.
    pub icon_cache_key: Option<String>,
    /// Button labels (`p=buttons`), in order.
    pub buttons: Vec<String>,
    /// Whether activation is reported back to the application (`a=report`).
    pub activation_reporting: Osc99ActivationReporting,
    /// Whether activation focuses the source window (`a=focus`).
    pub activation_focus: Osc99ActivationFocus,
    /// Whether closing is reported back to the application (`c=1`).
    pub close_reporting: Osc99CloseReporting,
    /// Urgency (`u=`); `Normal` when unset.
    pub urgency: NotificationUrgency,
    /// Occasion (`o=`) on which the notification may be displayed.
    pub occasion: NotificationOccasion,
    /// Requested sound (`s=`).
    pub sound: Osc99Sound,
    /// Application name (`f=`), if any.
    pub app_name: Option<String>,
    /// Notification type/category tags (`t=`), may repeat.
    pub notification_types: Vec<String>,
    /// Auto-expiry (`w=`).
    pub expiry: Osc99Expiry,
}
