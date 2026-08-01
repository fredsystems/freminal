// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Conversion seam between `egui`'s geometry types and the toolkit-neutral
//! [`freminal_common::geometry`] types.
//!
//! This module exists solely because the orphan rule blocks a `From` impl
//! in either direction: `freminal_common` must not depend on `egui` (see
//! `freminal_common::geometry`'s own doc comment), and inside this crate
//! both `egui::Rect`/`egui::Pos2` and `freminal_common::geometry::Rect`/
//! `Point` are foreign types, so no impl of a foreign trait for a foreign
//! type is possible here either. Plain field-copy free functions are the
//! only legal option.
//!
//! This is the **single sanctioned crossing point** between `egui`
//! geometry and the neutral geometry used by `freminal::gui::panes`. Call
//! sites that need to hand a `panes` function an `egui::Rect`, or paint an
//! `egui::Rect` computed by one, should convert here rather than
//! re-deriving the field copy inline. See Task 122 ("Orchestration
//! Extraction"), subtask 122.3.
//!
//! ## Converting a whole layout vector does not allocate twice
//!
//! Three per-frame call sites in `app_impl.rs` convert the `Vec` returned by
//! `PaneTree::layout` with `.into_iter().map(rect_to_egui).collect()`. That
//! looks like it allocates a second vector next to the one `layout` just
//! built, and a 122.3 review flagged it as new per-frame allocation
//! pressure. **It does not allocate.** `(PaneId, geometry::Rect)` and
//! `(PaneId, egui::Rect)` are both 24 bytes with 8-byte alignment (a `u64`
//! plus four `f32`s either way), which is exactly the condition for the
//! standard library's in-place-collect specialisation on
//! `Vec::into_iter().map(..).collect::<Vec<_>>()`; the buffer is reused and
//! the map runs in place. Verified empirically, not assumed.
//!
//! Two consequences worth knowing before "optimising" these sites:
//!
//! - The specialisation needs **`into_iter()`**. The same chain written with
//!   `.iter().map(..).collect()` borrows, cannot reuse, and *does* allocate.
//!   `app_impl.rs`'s `pane_at_pos` call at the drag-release handler uses the
//!   borrowing form deliberately — it runs once per drag release, not per
//!   frame, and the source vector is still needed afterwards.
//! - It also depends on the two element types keeping identical size and
//!   alignment. If either `Rect` ever grows a field, these sites silently
//!   start allocating again.

use freminal_common::geometry::{Point, Rect, point};

/// Convert an `egui::Pos2` into the toolkit-neutral [`Point`].
#[must_use]
pub const fn point_from_egui(p: egui::Pos2) -> Point {
    point(p.x, p.y)
}

/// Convert a toolkit-neutral [`Point`] into an `egui::Pos2`.
#[must_use]
pub const fn point_to_egui(p: Point) -> egui::Pos2 {
    egui::pos2(p.x, p.y)
}

/// Convert an `egui::Rect` into the toolkit-neutral [`Rect`].
#[must_use]
pub const fn rect_from_egui(r: egui::Rect) -> Rect {
    Rect::from_min_max(point_from_egui(r.min), point_from_egui(r.max))
}

/// Convert a toolkit-neutral [`Rect`] into an `egui::Rect`.
#[must_use]
pub const fn rect_to_egui(r: Rect) -> egui::Rect {
    egui::Rect::from_min_max(point_to_egui(r.min), point_to_egui(r.max))
}

#[cfg(test)]
// These conversions are exact field copies with no arithmetic, so a plain
// `assert_eq!` on the `f32` fields is the correct check, not an
// approximation — an epsilon here would hide a conversion bug.
#[allow(clippy::float_cmp)]
mod tests {
    use super::{point_from_egui, point_to_egui, rect_from_egui, rect_to_egui};
    use freminal_common::geometry::{Rect, point};

    #[test]
    fn point_round_trips_through_egui() {
        let p = point(3.5, -2.25);
        let egui_p = point_to_egui(p);
        assert_eq!(egui_p.x, p.x);
        assert_eq!(egui_p.y, p.y);
        assert_eq!(point_from_egui(egui_p), p);
    }

    #[test]
    fn rect_round_trips_through_egui() {
        let r = Rect::from_min_max(point(0.0, 0.0), point(800.0, 600.0));
        let egui_r = rect_to_egui(r);
        assert_eq!(egui_r.min.x, r.min.x);
        assert_eq!(egui_r.min.y, r.min.y);
        assert_eq!(egui_r.max.x, r.max.x);
        assert_eq!(egui_r.max.y, r.max.y);
        assert_eq!(rect_from_egui(egui_r), r);
    }

    #[test]
    fn rect_from_egui_is_a_plain_field_copy() {
        let egui_r = egui::Rect::from_min_max(egui::pos2(1.0, 2.0), egui::pos2(3.0, 4.0));
        let r = rect_from_egui(egui_r);
        assert_eq!(r.min.x, 1.0);
        assert_eq!(r.min.y, 2.0);
        assert_eq!(r.max.x, 3.0);
        assert_eq!(r.max.y, 4.0);
    }
}
