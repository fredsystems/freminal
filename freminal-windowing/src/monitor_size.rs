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
//! Querying the monitor is a windowing-system round trip, so the result is
//! cached per window ([`MonitorSize`]) and refreshed only when it can have
//! changed: at window creation and on winit `Moved`, `Resized` and
//! `ScaleFactorChanged` for that window. It is never queried per frame; the
//! cached value is injected into each frame's `RawInput` by
//! [`MonitorSize::inject`].
//!
//! `None` (compositors that cannot report a monitor, e.g. some Wayland
//! setups) is a valid, expected value and is injected as-is.

use winit::dpi::{PhysicalSize, validate_scale_factor};
use winit::window::Window;

/// A window's monitor size in egui logical points, or `None` when unknown.
#[derive(Debug, Clone, Copy, Default)]
pub struct MonitorSize {
    logical: Option<egui::Vec2>,
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
        }
    }

    /// Query the monitor `window` is currently on.
    ///
    /// `scale_factor` is passed in rather than read from the window so the
    /// `ScaleFactorChanged` handler can supply the event's new value.
    pub(crate) fn query(window: &Window, scale_factor: f64) -> Self {
        window
            .current_monitor()
            .map_or_else(Self::default, |monitor| {
                Self::from_physical(monitor.size(), scale_factor)
            })
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
