// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Fixtures shared by the toast module's test modules.

use super::input::{ToastButtonInput, ToastLayoutInput};
use super::layout::{ToastGeometry, ToastLayoutOutput, layout_toasts};
use super::{SCALE_IN, SLIDE_IN_PTS, ToastAnim, ToastKind, ToastPosition};
use crate::gui::renderer::ToastTextMetrics;

pub(super) const SETTLED_ANIM: ToastAnim = ToastAnim {
    opacity: 1.0,
    slide_x: 0.0,
    scale: 1.0,
};

pub(super) const ENTRY_ANIM: ToastAnim = ToastAnim {
    opacity: 0.0,
    slide_x: SLIDE_IN_PTS,
    scale: SCALE_IN,
};

/// Whether a test toast has a detail line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DetailLine {
    /// Title only.
    Absent,
    /// Title and a detail line.
    Present,
}

/// Build a [`ToastLayoutInput`] from simple numbers, so layout tests
/// never need real shaping/rasterisation.
pub(super) fn test_input(
    id: u64,
    label_width: f32,
    detail_line: DetailLine,
    anim: ToastAnim,
) -> ToastLayoutInput {
    let has_detail = detail_line == DetailLine::Present;
    ToastLayoutInput {
        id,
        kind: ToastKind::Info,
        icon_glyph: ToastKind::Info.icon(),
        label_text: "Info: hello".to_owned(),
        label_size_px: 16.0,
        label: ToastTextMetrics {
            width: label_width,
            height: 20.0,
            ascent: 15.0,
        },
        detail_text: if has_detail {
            "detail line".to_owned()
        } else {
            String::new()
        },
        detail_size_px: 13.0,
        detail: ToastTextMetrics {
            width: 80.0,
            height: 16.0,
            ascent: 12.0,
        },
        has_detail,
        icon_size_px: 16.0,
        icon: ToastTextMetrics {
            width: 16.0,
            height: 16.0,
            ascent: 12.0,
        },
        close_size_px: 16.0,
        close: ToastTextMetrics {
            width: 16.0,
            height: 16.0,
            ascent: 12.0,
        },
        anim,
        buttons: Vec::new(),
    }
}

pub(super) fn test_window_rect() -> egui::Rect {
    egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0))
}

/// Build layout outputs for a single settled error toast (helper for the
/// `topmost_hit` geometry tests). The interactive `hit_test` egui path is
/// covered separately with a headless egui context (see `hit`'s tests); these
/// cover the pure hit-precedence geometry it relies on.
pub(super) fn single_toast_outputs(id: u64) -> Vec<ToastLayoutOutput> {
    let geom = ToastGeometry {
        content_rect: test_window_rect(),
        active_pane_rect: None,
    };
    let visuals = egui::Visuals::dark();
    layout_toasts(
        &[test_input(id, 100.0, DetailLine::Absent, SETTLED_ANIM)],
        ToastPosition::TopRightStack,
        geom,
        &visuals,
        1.0,
    )
}

/// A measured button label `label_width` wide (12 px text, 14 tall).
pub(super) fn button_input(label: &str, label_width: f32) -> ToastButtonInput {
    ToastButtonInput {
        label_text: label.to_owned(),
        size_px: 12.0,
        label: ToastTextMetrics {
            width: label_width,
            height: 14.0,
            ascent: 11.0,
        },
    }
}

pub(super) fn buttons_output(
    buttons: Vec<ToastButtonInput>,
    detail_line: DetailLine,
    anim: ToastAnim,
    ppp: f32,
) -> ToastLayoutOutput {
    let geom = ToastGeometry {
        content_rect: test_window_rect(),
        active_pane_rect: None,
    };
    let mut input = test_input(3, 100.0, detail_line, anim);
    input.buttons = buttons;
    layout_toasts(
        &[input],
        ToastPosition::TopRightStack,
        geom,
        &egui::Visuals::dark(),
        ppp,
    )
    .remove(0)
}

pub(super) fn rect_inside(inner: [f32; 4], outer: [f32; 4]) -> bool {
    inner[0] >= outer[0] - 0.01
        && inner[1] >= outer[1] - 0.01
        && inner[2] <= outer[2] + 0.01
        && inner[3] <= outer[3] + 0.01
}
