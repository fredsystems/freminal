// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! User-visible transient notifications.
//!
//! Toasts are short messages rendered as an overlay in the top-right corner
//! of every window.  They are used to surface non-fatal errors that used to
//! disappear into `tracing::error!` logs — PTY spawn failures, layout load
//! failures, shader compile errors, and similar.
//!
//! The stack lives at app-level on [`super::FreminalGui`] (not per-window) so
//! a failure that happens before a window exists (e.g. PTY spawn for a new
//! window) still has a place to be reported.  Every window renders the same
//! stack; a dismissal clears the toast for all windows at once.
//!
//! Toasts auto-expire after a kind-dependent duration unless the user hovers
//! over them, in which case the timer is paused.  Error toasts expire after
//! 10 seconds, warnings after 6, and info after 3.  The user can dismiss any
//! toast immediately by clicking its `x` button.

use std::time::{Duration, Instant};

/// Severity of a toast.  Drives color and default duration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ToastKind {
    /// Non-fatal error that the user should see.
    Error,
    /// Warning that does not prevent continued operation.
    #[allow(dead_code)]
    // Reserved for future subtasks (71.3 layout non-fatal, 71.4 shader warnings).
    Warning,
    /// Informational message.
    #[allow(dead_code)] // Reserved for future subtasks.
    Info,
}

impl ToastKind {
    /// Default lifetime before auto-dismissal, when not hovered.
    const fn default_duration(self) -> Duration {
        match self {
            Self::Error => Duration::from_secs(10),
            Self::Warning => Duration::from_secs(6),
            Self::Info => Duration::from_secs(3),
        }
    }

    /// Background tint for the toast bubble.
    //
    // Not `const fn`: `Color32::from_rgba_unmultiplied` is not itself const.
    #[allow(clippy::missing_const_for_fn)]
    fn background(self) -> egui::Color32 {
        match self {
            Self::Error => egui::Color32::from_rgba_unmultiplied(120, 30, 30, 240),
            Self::Warning => egui::Color32::from_rgba_unmultiplied(120, 90, 20, 240),
            Self::Info => egui::Color32::from_rgba_unmultiplied(30, 60, 110, 240),
        }
    }

    /// Short prefix shown before the message (e.g. "Error").
    const fn label(self) -> &'static str {
        match self {
            Self::Error => "Error",
            Self::Warning => "Warning",
            Self::Info => "Info",
        }
    }
}

/// A single toast entry in the stack.
#[derive(Debug, Clone)]
pub(super) struct Toast {
    kind: ToastKind,
    /// One-line headline (bold).
    title: String,
    /// Optional multi-line detail (wrapped).
    detail: Option<String>,
    /// Monotonic id used as the egui widget id seed, so each toast has
    /// a stable id across frames even after list reordering.
    id: u64,
    /// When the toast was created.  Used with `default_duration()` to
    /// compute expiry (unless hovered).
    created: Instant,
    /// When the toast was last hovered.  `None` if never hovered.  Used
    /// to extend the lifetime of a toast the user is reading.
    last_hovered: Option<Instant>,
}

impl Toast {
    fn new(kind: ToastKind, title: String, detail: Option<String>, id: u64) -> Self {
        Self {
            kind,
            title,
            detail,
            id,
            created: Instant::now(),
            last_hovered: None,
        }
    }

    /// Returns `true` if the toast should be removed this frame.
    fn is_expired(&self, now: Instant) -> bool {
        // If currently hovered (within ~100ms), keep the toast alive.
        if let Some(hover) = self.last_hovered
            && now.duration_since(hover) < Duration::from_millis(200)
        {
            return false;
        }
        now.duration_since(self.created) > self.kind.default_duration()
    }
}

/// Ordered stack of active toasts, rendered top-to-bottom from the most
/// recent.  Capped at [`MAX_TOASTS`] entries; older entries are evicted
/// when the cap is exceeded.
#[derive(Debug, Default)]
pub(super) struct ToastStack {
    entries: Vec<Toast>,
    next_id: u64,
}

/// Maximum simultaneous toasts.  Older ones are evicted.
const MAX_TOASTS: usize = 5;

impl ToastStack {
    /// Push a new error toast onto the stack.
    pub(super) fn error(&mut self, title: impl Into<String>, detail: Option<String>) {
        self.push(ToastKind::Error, title.into(), detail);
    }

    fn push(&mut self, kind: ToastKind, title: String, detail: Option<String>) {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        self.entries.push(Toast::new(kind, title, detail, id));
        // Evict oldest entries beyond the cap.
        while self.entries.len() > MAX_TOASTS {
            self.entries.remove(0);
        }
    }

    /// Render the stack as an overlay in the top-right corner of the window.
    ///
    /// Clears auto-expired toasts and any the user dismissed via the `x` button.
    pub(super) fn show(&mut self, ctx: &egui::Context) {
        if self.entries.is_empty() {
            return;
        }

        let now = Instant::now();
        let mut to_remove: Vec<u64> = Vec::new();

        egui::Area::new(egui::Id::new("toast_overlay"))
            .anchor(egui::Align2::RIGHT_TOP, [-12.0, 44.0])
            .order(egui::Order::Foreground)
            .interactable(true)
            .show(ctx, |ui| {
                ui.set_max_width(360.0);
                ui.vertical(|ui| {
                    for toast in &mut self.entries {
                        let bg = toast.kind.background();
                        let frame = egui::Frame::NONE
                            .fill(bg)
                            .stroke(egui::Stroke::new(
                                1.0,
                                egui::Color32::from_rgba_unmultiplied(255, 255, 255, 32),
                            ))
                            .corner_radius(egui::CornerRadius::same(6))
                            .inner_margin(egui::Margin::symmetric(10, 8));
                        let resp = frame.show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.vertical(|ui| {
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "{}: {}",
                                            toast.kind.label(),
                                            toast.title
                                        ))
                                        .color(egui::Color32::WHITE)
                                        .strong(),
                                    );
                                    if let Some(ref detail) = toast.detail {
                                        ui.label(
                                            egui::RichText::new(detail)
                                                .color(egui::Color32::from_gray(230))
                                                .small(),
                                        );
                                    }
                                });
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::TOP),
                                    |ui| {
                                        if ui
                                            .add(
                                                egui::Button::new(
                                                    egui::RichText::new("×")
                                                        .color(egui::Color32::WHITE)
                                                        .strong(),
                                                )
                                                .frame(false)
                                                .small(),
                                            )
                                            .on_hover_text("Dismiss")
                                            .clicked()
                                        {
                                            to_remove.push(toast.id);
                                        }
                                    },
                                );
                            });
                        });
                        if resp.response.hovered() {
                            toast.last_hovered = Some(now);
                        }
                        ui.add_space(6.0);
                    }
                });
            });

        // Remove dismissed toasts.
        if !to_remove.is_empty() {
            self.entries.retain(|t| !to_remove.contains(&t.id));
        }
        // Remove expired toasts.
        self.entries.retain(|t| !t.is_expired(now));

        // Request a repaint soon so expiry happens even without input.
        if !self.entries.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stack_starts_empty() {
        let s = ToastStack::default();
        assert!(s.entries.is_empty());
    }

    #[test]
    fn push_error_appears_in_stack() {
        let mut s = ToastStack::default();
        s.error("spawn failed", Some("no such file".to_owned()));
        assert_eq!(s.entries.len(), 1);
        assert_eq!(s.entries[0].kind, ToastKind::Error);
        assert_eq!(s.entries[0].title, "spawn failed");
        assert_eq!(s.entries[0].detail.as_deref(), Some("no such file"));
    }

    #[test]
    fn stack_evicts_oldest_past_cap() {
        let mut s = ToastStack::default();
        for i in 0..(MAX_TOASTS + 3) {
            s.error(format!("err {i}"), None);
        }
        assert_eq!(s.entries.len(), MAX_TOASTS);
        // Oldest three should have been evicted; first surviving title is "err 3".
        assert_eq!(s.entries[0].title, "err 3");
    }

    #[test]
    fn toast_ids_are_monotonic() {
        let mut s = ToastStack::default();
        s.error("a", None);
        s.error("b", None);
        s.error("c", None);
        assert!(s.entries[0].id < s.entries[1].id);
        assert!(s.entries[1].id < s.entries[2].id);
    }

    #[test]
    fn expired_toast_is_detected() {
        // Drive expiry by synthesising a `now` that is 1 minute after the
        // toast's creation time, avoiding `Instant` subtraction which
        // clippy flags as unchecked.
        let created = Instant::now();
        let toast = Toast {
            kind: ToastKind::Info,
            title: "t".to_owned(),
            detail: None,
            id: 0,
            created,
            last_hovered: None,
        };
        let later = created + Duration::from_mins(1);
        assert!(toast.is_expired(later));
    }

    #[test]
    fn hovered_toast_is_not_expired() {
        let created = Instant::now();
        let later = created + Duration::from_mins(1);
        let toast = Toast {
            kind: ToastKind::Info,
            title: "t".to_owned(),
            detail: None,
            id: 0,
            created,
            // Hover event coincides with `later` — within the 200 ms
            // keep-alive window.
            last_hovered: Some(later),
        };
        assert!(!toast.is_expired(later));
    }

    #[test]
    fn stale_hover_does_not_preserve_toast() {
        let created = Instant::now();
        let stale_hover = created + Duration::from_secs(55);
        let later = created + Duration::from_mins(1);
        let toast = Toast {
            kind: ToastKind::Info,
            title: "t".to_owned(),
            detail: None,
            id: 0,
            created,
            // Hover was 5 s ago — well outside the 200 ms keep-alive window.
            last_hovered: Some(stale_hover),
        };
        assert!(toast.is_expired(later));
    }
}
