// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! The monitor size reported to egui as `ViewportInfo::monitor_size`.
//!
//! # Why this exists
//!
//! `egui_winit::State::take_egui_input` does not populate
//! `ViewportInfo::monitor_size`; only `egui_winit::update_viewport_info`
//! does, and that function also fills the title, inner/outer rects, focus,
//! maximised/minimised and fullscreen fields, which would change behaviour
//! elsewhere in the app. So freminal fills in just this one field itself.
//! The consumer is the XTWINOPS resize/move clamp (Task 148.17,
//! `freminal/src/gui/rendering.rs::monitor_bounds`), which falls back to a
//! fixed extent whenever the size is `None` (cleanup 148.C4).
//!
//! # Computation
//!
//! Mirrors egui-winit 0.36.2 `update_viewport_info`:
//!
//! ```text
//! window.current_monitor()
//!     .map(|m| m.size().to_logical::<f32>(pixels_per_point))
//! ```
//!
//! with `window.scale_factor()` standing in for `pixels_per_point`. The two
//! are equal while egui's zoom factor is `1.0`, which freminal guarantees
//! (see `Documents/EGUI_UPGRADE_ASSUMPTIONS.md` A13).
//!
//! # Refresh policy
//!
//! Querying the monitor is a windowing-system round trip (on X11, several),
//! so the result is cached per window ([`MonitorSize`]) and refreshed only
//! when it can have changed: at window creation and on winit `Moved`,
//! `Resized` and `ScaleFactorChanged` for that window (plus one retry of a
//! debounced refresh, below). It is never queried every frame; the cached value is injected into each frame's `RawInput` by
//! [`MonitorSize::inject`].
//!
//! `Moved` and `Resized` fire for every pixel of a drag, so they are debounced
//! (148.RC6): they refresh only when the cache is empty or at least
//! [`MONITOR_REFRESH_INTERVAL`] old ([`decide_refresh`]).  `ScaleFactorChanged`
//! always refreshes.  A deferred refresh is remembered
//! ([`DeferredRefresh::Pending`]) and retried on the next frame after the
//! interval has passed, so a drag that ends inside the interval still picks
//! up the new monitor; the per-frame check compares timestamps only and
//! queries the monitor at most once per owed refresh.
//!
//! `None` (compositors that cannot report a monitor, e.g. some Wayland
//! setups) is a valid, expected value and is injected as-is.

use std::time::{Duration, Instant};

use winit::dpi::{PhysicalSize, validate_scale_factor};
use winit::window::Window;

/// Least time between two refreshes triggered by `Moved` / `Resized`.
pub const MONITOR_REFRESH_INTERVAL: Duration = Duration::from_millis(250);

/// The window event asking for a monitor-size refresh.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshCause {
    /// `Moved` or `Resized`: fires for every pixel of a drag, so debounced.
    Geometry,
    /// `ScaleFactorChanged`: rare, and the cached logical size is wrong until
    /// it is refreshed, so never debounced.
    ScaleFactor,
}

/// Whether a refresh should query the monitor now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MonitorRefresh {
    /// Query the monitor.
    Now,
    /// Keep the cached value.
    Defer,
}

/// Pure debounce decision for a monitor-size refresh (148.RC6).
///
/// `cached` is the currently cached size and `since_last_refresh` how long ago
/// it was queried (`None` if never).  A [`RefreshCause::ScaleFactor`] always
/// refreshes; a [`RefreshCause::Geometry`] refreshes when nothing is cached, or
/// the cache was never stamped, or it is at least [`MONITOR_REFRESH_INTERVAL`]
/// old.
pub fn decide_refresh(
    cause: RefreshCause,
    cached: Option<egui::Vec2>,
    since_last_refresh: Option<Duration>,
) -> MonitorRefresh {
    match (cause, cached, since_last_refresh) {
        (RefreshCause::ScaleFactor, _, _) | (RefreshCause::Geometry, None, _) => {
            MonitorRefresh::Now
        }
        (RefreshCause::Geometry, Some(_), Some(elapsed)) if elapsed < MONITOR_REFRESH_INTERVAL => {
            MonitorRefresh::Defer
        }
        (RefreshCause::Geometry, Some(_), _) => MonitorRefresh::Now,
    }
}

/// Whether a debounced geometry refresh is still owed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DeferredRefresh {
    /// No refresh is owed.
    #[default]
    Clear,
    /// A `Moved` / `Resized` refresh was deferred; it is retried on a later
    /// frame once [`MONITOR_REFRESH_INTERVAL`] has passed, so a drag that ends
    /// inside the interval still picks up the new monitor.
    Pending,
}

/// A window's monitor size in egui logical points, or `None` when unknown.
#[derive(Debug, Clone, Copy, Default)]
pub struct MonitorSize {
    logical: Option<egui::Vec2>,
    /// When the monitor was last queried; `None` for a value never queried.
    queried_at: Option<Instant>,
    /// Whether a deferred geometry refresh is owed.
    deferred: DeferredRefresh,
}

impl MonitorSize {
    /// Convert a physical monitor size to logical points.
    ///
    /// `None` for a scale factor winit would reject (non-finite, zero or
    /// negative; `to_logical` panics on those) rather than panicking.
    pub(crate) fn from_physical(size: PhysicalSize<u32>, scale_factor: f64) -> Self {
        if !validate_scale_factor(scale_factor) {
            return Self::default();
        }
        let logical = size.to_logical::<f32>(scale_factor);
        Self {
            logical: Some(egui::vec2(logical.width, logical.height)),
            queried_at: None,
            deferred: DeferredRefresh::Clear,
        }
    }

    /// Query the monitor `window` is currently on.
    ///
    /// `scale_factor` is passed in rather than read from the window so the
    /// `ScaleFactorChanged` handler can supply the event's new value.
    pub(crate) fn query(window: &Window, scale_factor: f64) -> Self {
        let queried_at = Some(Instant::now());
        let measured = window
            .current_monitor()
            .map_or_else(Self::default, |monitor| {
                Self::from_physical(monitor.size(), scale_factor)
            });
        Self {
            queried_at,
            ..measured
        }
    }

    /// Whether `cause` should re-query the monitor at `now` (see
    /// [`decide_refresh`]).
    pub(crate) fn refresh_for(self, cause: RefreshCause, now: Instant) -> MonitorRefresh {
        decide_refresh(
            cause,
            self.logical,
            self.queried_at.map(|at| now.saturating_duration_since(at)),
        )
    }

    /// Record that a geometry refresh was deferred.
    pub(crate) const fn mark_deferred(&mut self) {
        self.deferred = DeferredRefresh::Pending;
    }

    /// Whether an owed (deferred) refresh is due at `now`: only when one is
    /// pending and [`MONITOR_REFRESH_INTERVAL`] has passed since the last
    /// query. Cheap enough to check every frame (no monitor query).
    pub(crate) fn deferred_due(self, now: Instant) -> MonitorRefresh {
        match self.deferred {
            DeferredRefresh::Clear => MonitorRefresh::Defer,
            DeferredRefresh::Pending => self.refresh_for(RefreshCause::Geometry, now),
        }
    }

    /// Write the cached size into the current viewport's `ViewportInfo`.
    ///
    /// Creates the viewport entry if `raw_input` has none, as
    /// `egui_winit::State::take_egui_input` does.
    pub(crate) fn inject(self, raw_input: &mut egui::RawInput) {
        raw_input
            .viewports
            .entry(raw_input.viewport_id)
            .or_default()
            .monitor_size = self.logical;
    }

    /// The cached size, for tests and diagnostics.
    #[cfg(test)]
    pub(crate) const fn logical(self) -> Option<egui::Vec2> {
        self.logical
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZE: Option<egui::Vec2> = Some(egui::vec2(1920.0, 1080.0));

    #[test]
    fn scale_factor_changes_always_refresh() {
        for since in [None, Some(Duration::ZERO), Some(MONITOR_REFRESH_INTERVAL)] {
            assert_eq!(
                decide_refresh(RefreshCause::ScaleFactor, SIZE, since),
                MonitorRefresh::Now
            );
        }
    }

    #[test]
    fn geometry_events_inside_the_interval_are_deferred() {
        for since in [
            Duration::ZERO,
            Duration::from_millis(1),
            Duration::from_millis(249),
        ] {
            assert_eq!(
                decide_refresh(RefreshCause::Geometry, SIZE, Some(since)),
                MonitorRefresh::Defer,
                "{since:?} after the last refresh"
            );
        }
    }

    #[test]
    fn geometry_events_at_or_past_the_interval_refresh() {
        for since in [MONITOR_REFRESH_INTERVAL, Duration::from_secs(60)] {
            assert_eq!(
                decide_refresh(RefreshCause::Geometry, SIZE, Some(since)),
                MonitorRefresh::Now
            );
        }
    }

    #[test]
    fn geometry_events_refresh_when_nothing_is_cached_however_recent() {
        assert_eq!(
            decide_refresh(RefreshCause::Geometry, None, Some(Duration::ZERO)),
            MonitorRefresh::Now
        );
        assert_eq!(
            decide_refresh(RefreshCause::Geometry, None, None),
            MonitorRefresh::Now
        );
    }

    #[test]
    fn geometry_events_refresh_a_value_that_was_never_queried() {
        assert_eq!(
            decide_refresh(RefreshCause::Geometry, SIZE, None),
            MonitorRefresh::Now
        );
    }

    #[test]
    fn refresh_for_measures_elapsed_time_from_the_query_stamp() {
        let then = Instant::now();
        let cached = MonitorSize {
            logical: SIZE,
            queried_at: Some(then),
            deferred: DeferredRefresh::Clear,
        };
        assert_eq!(
            cached.refresh_for(RefreshCause::Geometry, then + Duration::from_millis(10)),
            MonitorRefresh::Defer
        );
        assert_eq!(
            cached.refresh_for(RefreshCause::Geometry, then + MONITOR_REFRESH_INTERVAL),
            MonitorRefresh::Now
        );
        // A `now` before the stamp (clock skew between callers) is zero elapsed.
        assert_eq!(
            cached.refresh_for(RefreshCause::Geometry, then),
            MonitorRefresh::Defer
        );
        assert_eq!(
            MonitorSize::default().refresh_for(RefreshCause::Geometry, then),
            MonitorRefresh::Now
        );
    }

    #[test]
    fn deferred_refresh_is_due_only_when_pending_and_past_the_interval() {
        let now = Instant::now();
        let mut size = MonitorSize {
            logical: Some(egui::vec2(1920.0, 1080.0)),
            queried_at: Some(now),
            deferred: DeferredRefresh::Clear,
        };
        let later = now + MONITOR_REFRESH_INTERVAL;
        assert_eq!(size.deferred_due(later), MonitorRefresh::Defer);
        size.mark_deferred();
        assert_eq!(size.deferred_due(now), MonitorRefresh::Defer);
        assert_eq!(size.deferred_due(later), MonitorRefresh::Now);
    }

    #[test]
    fn physical_size_is_divided_by_scale_factor() {
        let size = MonitorSize::from_physical(PhysicalSize::new(3840, 2160), 2.0);
        assert_eq!(size.logical(), Some(egui::vec2(1920.0, 1080.0)));
    }

    #[test]
    fn unit_scale_factor_is_identity() {
        let size = MonitorSize::from_physical(PhysicalSize::new(2560, 1440), 1.0);
        assert_eq!(size.logical(), Some(egui::vec2(2560.0, 1440.0)));
    }

    #[test]
    fn fractional_scale_factor_is_supported() {
        let size = MonitorSize::from_physical(PhysicalSize::new(3000, 1500), 1.5);
        assert_eq!(size.logical(), Some(egui::vec2(2000.0, 1000.0)));
    }

    #[test]
    fn invalid_scale_factors_yield_unknown_rather_than_panicking() {
        for scale in [0.0, -1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let size = MonitorSize::from_physical(PhysicalSize::new(1920, 1080), scale);
            assert_eq!(size.logical(), None, "scale factor {scale}");
        }
    }

    #[test]
    fn default_is_unknown() {
        assert_eq!(MonitorSize::default().logical(), None);
    }

    #[test]
    fn inject_sets_monitor_size_on_the_current_viewport() {
        let mut raw_input = egui::RawInput::default();
        let size = MonitorSize::from_physical(PhysicalSize::new(3840, 2160), 2.0);

        size.inject(&mut raw_input);

        assert_eq!(
            raw_input.viewport().monitor_size,
            Some(egui::vec2(1920.0, 1080.0))
        );
    }

    #[test]
    fn inject_creates_the_viewport_entry_when_absent() {
        let mut raw_input = egui::RawInput::default();
        raw_input.viewports.clear();
        let size = MonitorSize::from_physical(PhysicalSize::new(1000, 500), 1.0);

        size.inject(&mut raw_input);

        assert_eq!(
            raw_input.viewport().monitor_size,
            Some(egui::vec2(1000.0, 500.0))
        );
    }

    #[test]
    fn inject_unknown_clears_a_previous_size() {
        let mut raw_input = egui::RawInput::default();
        MonitorSize::from_physical(PhysicalSize::new(1000, 500), 1.0).inject(&mut raw_input);

        MonitorSize::default().inject(&mut raw_input);

        assert_eq!(raw_input.viewport().monitor_size, None);
    }

    #[test]
    fn inject_leaves_other_viewport_fields_untouched() {
        let mut raw_input = egui::RawInput::default();
        raw_input
            .viewports
            .entry(raw_input.viewport_id)
            .or_default()
            .title = Some("freminal".to_string());

        MonitorSize::from_physical(PhysicalSize::new(1000, 500), 1.0).inject(&mut raw_input);

        let info = raw_input.viewport();
        assert_eq!(info.title.as_deref(), Some("freminal"));
        assert_eq!(info.focused, None);
        assert_eq!(info.inner_rect, None);
    }
}
