// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Toolkit-agnostic 2D geometry primitives for the Freminal UI.
//!
//! This module defines a minimal [`Point`] / [`Rect`] pair with **no
//! dependency on a rendering toolkit** — in particular, no `egui`. It exists
//! so that `freminal/src/gui/panes` (the built-in multiplexer's pane-tree
//! layout and hit-testing) can express its layout maths without pulling
//! `egui::Rect` / `egui::Pos2` into pure geometry code. See Task 122
//! ("Orchestration Extraction"), subtask 122.2, and
//! `Documents/DECOUPLING_FRAMEWORK.md` for the broader context that motivates
//! keeping toolkit types out of non-rendering logic.
//!
//! Where this module provides an equivalent to an `egui`/`emath` operation,
//! it **deliberately mirrors that operation's float semantics exactly** —
//! see the doc comments on [`Rect::contains`], [`Rect::center`],
//! [`Rect::width`] and [`Rect::height`] for the precise behaviour and why it
//! matters. Subtask 122.3 migrates `panes/mod.rs`'s production geometry onto
//! these types as a no-behaviour-change refactor, and its existing tests
//! assert exact widths, heights, and boundary hit-tests; any divergence from
//! `egui`/`emath` here would silently break that migration.
//!
//! The API is **intentionally minimal**: only the operations actually used
//! by `panes/mod.rs` are provided (see subtask 122.2's exhaustive usage
//! count). It is grown on demand by later subtasks, not spread out
//! speculatively — do not add `shrink`, `expand`, `translate`, `intersect`,
//! `union`, `from_min_size`, edge accessors (`left`/`right`/`top`/`bottom`),
//! or arithmetic operator impls here unless a real call site needs them.

/// A single 2D point (or, equivalently, a 2D vector/offset) using `f32`
/// coordinates.
///
/// Mirrors the fields of `egui::Pos2` so that call sites converting from
/// `egui` types can do so with a plain field-for-field copy.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    /// The horizontal coordinate.
    pub x: f32,
    /// The vertical coordinate.
    pub y: f32,
}

/// Construct a [`Point`] from `x` and `y` coordinates.
///
/// The toolkit-neutral equivalent of `egui::pos2(x, y)`.
#[must_use]
pub const fn point(x: f32, y: f32) -> Point {
    Point { x, y }
}

/// An axis-aligned rectangle described by its minimum (top-left) and maximum
/// (bottom-right) corners.
///
/// Mirrors the field layout of `egui::Rect`. As with `egui::Rect`, the
/// `min`/`max` corners are stored verbatim by [`Rect::from_min_max`] and are
/// **not** normalised: a rectangle whose `max` is less than its `min` in one
/// or both axes is a valid, representable "negative-extent" rectangle whose
/// [`Rect::width`] and/or [`Rect::height`] are negative. See the method docs
/// below for the exact semantics this type reproduces.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    /// The minimum (top-left, in the usual screen coordinate convention)
    /// corner of the rectangle.
    pub min: Point,
    /// The maximum (bottom-right) corner of the rectangle.
    pub max: Point,
}

impl Rect {
    /// Construct a [`Rect`] from its minimum and maximum corners.
    ///
    /// The two points are stored verbatim: this constructor does **not**
    /// normalise, sort, or validate them. If `max` is less than `min` in
    /// either axis, the resulting rectangle has negative
    /// [`width`](Self::width) and/or [`height`](Self::height); this mirrors
    /// `egui::Rect::from_min_max`, which has the same non-normalising
    /// behaviour.
    #[must_use]
    pub const fn from_min_max(min: Point, max: Point) -> Self {
        Self { min, max }
    }

    /// Returns `true` if `p` lies within this rectangle, **inclusive of all
    /// four boundaries**.
    ///
    /// This matches `egui::Rect::contains` (`emath` 0.35.0,
    /// `emath-0.35.0/src/rect.rs:274-276`) exactly: a point exactly on an
    /// edge or corner counts as contained.
    ///
    /// The inclusivity is **load-bearing for pane hit-testing**, not an
    /// arbitrary choice. `freminal::gui::panes::split_rect` gives the two
    /// halves of a split the *same* boundary coordinate — the left/top half
    /// ends at `split_x`/`split_y` and the right/bottom half begins there —
    /// so adjacent pane rects share an edge rather than tiling half-open.
    /// A pointer exactly on that shared edge is therefore contained by
    /// **both**, and `panes::pane_at_pos` resolves the ambiguity by taking
    /// the first match in layout order (as its own doc states). Switching
    /// this to a half-open interval would leave the shared edge belonging to
    /// neither pane, turning every split boundary into a one-pixel dead
    /// stripe. Do not "correct" it.
    ///
    /// Note that a half-open convention *does* exist elsewhere in the
    /// codebase for a different job — `app_impl.rs`'s `pointer_in_gutter_strip`
    /// deliberately uses `<` at its far edge — but that predicate is plain
    /// scalar arithmetic and does not go through this type.
    #[must_use]
    pub const fn contains(&self, p: Point) -> bool {
        self.min.x <= p.x && p.x <= self.max.x && self.min.y <= p.y && p.y <= self.max.y
    }

    /// Returns the width of the rectangle: `max.x - min.x`.
    ///
    /// This can be **negative** for a rectangle whose corners were not given
    /// in min/max order (see [`Rect::from_min_max`]); the value is not
    /// clamped or made absolute. This matches `egui::Rect::width`
    /// (`emath` 0.35.0), which documents the same behaviour.
    #[must_use]
    pub const fn width(&self) -> f32 {
        self.max.x - self.min.x
    }

    /// Returns the height of the rectangle: `max.y - min.y`.
    ///
    /// This can be **negative** for a rectangle whose corners were not given
    /// in min/max order (see [`Rect::from_min_max`]); the value is not
    /// clamped or made absolute. This matches `egui::Rect::height`
    /// (`emath` 0.35.0), which documents the same behaviour.
    #[must_use]
    pub const fn height(&self) -> f32 {
        self.max.y - self.min.y
    }

    /// Returns the midpoint of the rectangle's `min` and `max` corners.
    ///
    /// Computed per-component as `(min + max) / 2.0`, **not** as
    /// `min + (max - min) / 2.0`. `emath` 0.35.0 computes the midpoint of two
    /// floats with `fast_midpoint(a, b) = (a + b) / 2.0`
    /// (`emath-0.35.0/src/lib.rs:122-128`), and the two formulas can round
    /// differently in `f32` arithmetic — by one ULP for values such as
    /// `(1.0, 0.001)`, which the test below pins.
    ///
    /// This matters because `freminal::gui::panes::active_highlight_segment`
    /// compares `border.rect.center().x` against a pane edge within an
    /// epsilon (`edge_epsilon`, `1.0` at its only production call site,
    /// `app_impl.rs:3556`). A one-ULP shift will not cross a 1.0 epsilon on
    /// its own, so this is parity **by construction** rather than a fix for
    /// an observed bug: subtask 122.3 is a no-behaviour-change refactor, and
    /// the cheapest way to guarantee that is to compute what `emath`
    /// computes rather than to argue about which differences are too small
    /// to matter.
    // `clippy::manual_midpoint` wants `f32::midpoint`, which on the targets
    // freminal supports computes `((a as f64 + b as f64) / 2.0) as f32` --
    // an f64 intermediate, specifically so the sum cannot overflow. That is
    // a different algorithm from `emath::fast_midpoint`'s plain
    // `(a + b) / 2.0` in f32, and the two DO disagree: for
    // `a = b = f32::MAX` the naive form gives `inf` while `f32::midpoint`
    // gives `f32::MAX`. emath's own doc for `fast_midpoint` acknowledges it
    // does not handle overflow.
    //
    // Being honest about the stakes: that divergence is reachable only near
    // 1e38, and pane geometry is screen pixels, so `f32::midpoint` would be
    // observationally identical for every input this type will ever see.
    // The allow is kept anyway because this type's entire contract is
    // "mirrors emath exactly", and a guarantee that holds by construction is
    // worth more than one that holds because the inputs happen to be small.
    // If that contract is ever dropped, drop this allow with it.
    #[allow(clippy::manual_midpoint)]
    #[must_use]
    pub const fn center(&self) -> Point {
        Point {
            x: (self.min.x + self.max.x) / 2.0,
            y: (self.min.y + self.max.y) / 2.0,
        }
    }
}

#[cfg(test)]
// These tests deliberately assert exact `f32` equality: the values are
// chosen so the arithmetic is exact, and the point of several tests (the
// `center` rounding regression guard in particular) is to pin an exact bit
// pattern, not an approximation. Introducing an epsilon here would defeat
// the purpose of the test.
// This module also computes `(a + b) / 2.0` manually (rather than via
// `f32::midpoint`) to reproduce exactly what `Rect::center` computes; see
// the `#[allow(clippy::manual_midpoint)]` justification on that method.
// Note that for the values used below, `(a + b) / 2.0` and `f32::midpoint`
// agree bit-for-bit -- the divergence between those two is an overflow-only
// effect near `f32::MAX`. What the `center` test pins is the *other*
// divergence: `(a + b) / 2.0` versus `min + (max - min) / 2.0`.
#[allow(clippy::float_cmp, clippy::manual_midpoint)]
mod tests {
    use super::{Rect, point};

    /// A point exactly on each of the four edges of a rectangle is
    /// considered contained (inclusive boundary semantics, matching
    /// `egui::Rect::contains`).
    #[test]
    fn contains_is_inclusive_on_all_four_edges() {
        let rect = Rect::from_min_max(point(0.0, 0.0), point(10.0, 20.0));

        // Edges (midpoints of each side).
        assert!(rect.contains(point(0.0, 10.0)), "left edge");
        assert!(rect.contains(point(10.0, 10.0)), "right edge");
        assert!(rect.contains(point(5.0, 0.0)), "top edge");
        assert!(rect.contains(point(5.0, 20.0)), "bottom edge");
    }

    /// A point exactly on each of the four corners of a rectangle is
    /// considered contained.
    #[test]
    fn contains_is_inclusive_on_all_four_corners() {
        let rect = Rect::from_min_max(point(0.0, 0.0), point(10.0, 20.0));

        assert!(rect.contains(point(0.0, 0.0)), "top-left corner");
        assert!(rect.contains(point(10.0, 0.0)), "top-right corner");
        assert!(rect.contains(point(0.0, 20.0)), "bottom-left corner");
        assert!(rect.contains(point(10.0, 20.0)), "bottom-right corner");
    }

    /// A point just outside each of the four edges is not contained.
    #[test]
    fn contains_is_false_just_outside_each_edge() {
        let rect = Rect::from_min_max(point(0.0, 0.0), point(10.0, 20.0));

        assert!(!rect.contains(point(-0.001, 10.0)), "left of left edge");
        assert!(!rect.contains(point(10.001, 10.0)), "right of right edge");
        assert!(!rect.contains(point(5.0, -0.001)), "above top edge");
        assert!(!rect.contains(point(5.0, 20.001)), "below bottom edge");
    }

    /// `center` on an ordinary rectangle: the plain, non-degenerate case the
    /// rounding-regression test below deliberately does not cover.
    #[test]
    fn center_on_a_normal_rect() {
        let rect = Rect::from_min_max(point(0.0, 0.0), point(10.0, 20.0));

        assert_eq!(rect.center(), point(5.0, 10.0));
    }

    /// `center` must use `(min + max) / 2.0`, not `min + (max - min) / 2.0`:
    /// for these chosen values the two formulas round to different `f32`
    /// values, so this pins the correct one. Verified numerically before
    /// writing this test: `(1.0f32 + 0.001f32) / 2.0 == 0.5005`, while
    /// `1.0f32 + (0.001f32 - 1.0f32) / 2.0 == 0.50049996` — a different
    /// `f32` bit pattern.
    #[test]
    fn center_uses_fast_midpoint_rounding_not_min_plus_half_extent() {
        let min = point(1.0, 0.0);
        let max = point(0.001, 0.0);
        let rect = Rect::from_min_max(min, max);

        let fast_midpoint = (min.x + max.x) / 2.0;
        let min_plus_half_extent = min.x + (max.x - min.x) / 2.0;
        assert_ne!(
            fast_midpoint, min_plus_half_extent,
            "test fixture must exercise a genuine f32 rounding difference"
        );

        let center = rect.center();
        assert_eq!(center.x, fast_midpoint);
        assert_eq!(center.x, 0.5005);
        assert_eq!(center.y, 0.0);
    }

    /// A zero-extent rectangle (`min == max`) has zero width and height, its
    /// own point as its center, and contains that point.
    #[test]
    fn zero_extent_rect() {
        let p = point(3.5, -2.25);
        let rect = Rect::from_min_max(p, p);

        assert_eq!(rect.width(), 0.0);
        assert_eq!(rect.height(), 0.0);
        assert_eq!(rect.center(), p);
        assert!(rect.contains(p));
    }

    /// A rectangle whose `max` is less than its `min` (not normalised by
    /// `from_min_max`) has negative width/height, and does not contain a
    /// point between the two corners.
    #[test]
    fn negative_extent_rect_is_not_normalised() {
        let rect = Rect::from_min_max(point(10.0, 10.0), point(0.0, 0.0));

        assert_eq!(rect.width(), -10.0);
        assert_eq!(rect.height(), -10.0);
        assert!(!rect.contains(point(5.0, 5.0)));
    }

    /// A rectangle inverted in only ONE axis. Every operation on this type is
    /// per-axis-independent arithmetic, so the mixed case must behave as the
    /// per-axis composition of the normal and inverted cases — this pins that
    /// rather than leaving it inferred from the both-axes test above.
    #[test]
    fn negative_extent_in_one_axis_only() {
        // Normal in x (0 -> 10), inverted in y (10 -> 0).
        let rect = Rect::from_min_max(point(0.0, 10.0), point(10.0, 0.0));

        assert_eq!(rect.width(), 10.0);
        assert_eq!(rect.height(), -10.0);
        assert_eq!(rect.center(), point(5.0, 5.0));
        // Inside the x span, but the inverted y span contains nothing.
        assert!(!rect.contains(point(5.0, 5.0)));
    }

    /// `width`/`height` on a normal (non-negative-extent) rectangle.
    #[test]
    fn width_and_height_on_a_normal_rect() {
        let rect = Rect::from_min_max(point(1.0, 2.0), point(4.0, 9.0));

        assert_eq!(rect.width(), 3.0);
        assert_eq!(rect.height(), 7.0);
    }

    /// `PartialEq` on `Point` and `Rect`, and confirmation that `Copy`
    /// semantics hold: a moved-from value remains usable afterward.
    #[test]
    fn partial_eq_and_copy_semantics() {
        let a = point(1.0, 2.0);
        let b = point(1.0, 2.0);
        let c = point(1.0, 3.0);
        assert_eq!(a, b);
        assert_ne!(a, c);

        let rect_a = Rect::from_min_max(a, c);
        let rect_b = Rect::from_min_max(b, c);
        assert_eq!(rect_a, rect_b);

        // `a` is `Copy`; using it here after it was already copied into
        // `rect_a`/`rect_b` above confirms it was not moved.
        let a_again = a;
        assert_eq!(a, a_again);
    }
}
