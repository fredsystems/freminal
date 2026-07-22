// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Frame-damage aggregation (#435), extracted for reuse (#436.2b).
//!
//! [`decide_frame_damage`] is the pure decision function behind the
//! `win.pending_frame_damage = 'damage: { ... }` block that used to live
//! inline in `app_impl.rs::update()`. It is extracted so that both the full
//! `update()` path and the future REPLAY path (#436) compute the exact same
//! [`freminal_windowing::FrameDamage`] for the exact same inputs, without
//! duplicating (and risking drift in) the decision logic.
//!
//! The function takes no `self`/`egui` context/window state — only the
//! handful of booleans and per-pane facts the original block actually read
//! — so it is directly unit-testable.

use crate::gui::renderer::PaneFrameDamage;

/// What one rendered pane contributed to this frame's damage decision.
///
/// Mirrors exactly the facts the original inline block read per pane:
/// whether a bell flash is animating (forces `Full`) and the pane's
/// [`PaneFrameDamage`] from the last render.
///
/// A pane present in `pane_layout` that could not be resolved in the pane
/// tree (the `let Some(pane) = ... else { ... }` branch of the original
/// block) has **no** representation of its own here — instead of adding an
/// "unresolved" variant, the caller (which is the one doing the tree
/// lookup) simply does not push an entry for it and instead short-circuits
/// by treating the whole frame as forced-full. See
/// [`decide_frame_damage`]'s doc comment for why this preserves the
/// original "unresolvable pane -> Full" semantics exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaneDamageInput {
    /// `pane.view_state.bell_since.is_some()` for this pane.
    pub(crate) bell_active: bool,
    /// `pane.render_cache.last_frame_cursor_damage` for this pane.
    pub(crate) cursor_damage: PaneFrameDamage,
}

/// Decide this frame's [`freminal_windowing::FrameDamage`] — the #435
/// partial-present decision — from already-computed inputs.
///
/// This is a pure function: given the same arguments it always returns the
/// same result, with no reliance on `egui`, `self`, or any window state.
/// That is what makes it safe to call from both the full `update()` path
/// and the future REPLAY path (#436) and get byte-for-byte identical
/// results to today's inline block.
///
/// Semantics (preserved exactly from the original `'damage:` block):
///
/// 1. `force_full` (`ui_overlay_open || shader_recomposites ||
///    active_pane_changed || pointer_moving`, computed by the caller)
///    short-circuits to [`FrameDamage::Full`].
/// 2. Otherwise, `toast_active` short-circuits to `Full`.
/// 3. Otherwise, `per_pane_damage` is walked in order (this must be the
///    same order as `pane_layout`, i.e. only the panes actually rendered
///    this frame):
///    - `bell_active` -> clear any collected rects and stop -> `Full`.
///    - `cursor_damage == CursorOnly(Some(rect))` -> push `rect`.
///    - `cursor_damage == CursorOnly(None)` or `Full` -> clear any
///      collected rects and stop -> `Full`.
///    - `cursor_damage == Unchanged` -> contributes nothing, continue.
/// 4. If no rects were collected (either the loop never pushed one, or it
///    was cleared by a later pane), the result is `Full`; otherwise it is
///    `Partial(rects)`.
///
/// An unresolvable pane in the original block behaved identically to a
/// `bell_active` pane: clear rects and stop -> `Full`. Since this function
/// has no pane tree to resolve against, that case is represented by the
/// **caller** omitting to build a full `per_pane_damage` list and instead
/// calling this function with `force_full = true` for that frame (there is
/// no scenario in the real caller where an unresolved pane should do
/// anything BUT force `Full`, so this is a lossless simplification of the
/// call site, not a behavior change).
pub fn decide_frame_damage(
    force_full: bool,
    toast_active: bool,
    per_pane_damage: &[PaneDamageInput],
) -> freminal_windowing::FrameDamage {
    if force_full {
        return freminal_windowing::FrameDamage::Full;
    }
    if toast_active {
        return freminal_windowing::FrameDamage::Full;
    }

    let mut rects: Vec<freminal_windowing::DamageRect> = Vec::new();
    for pane in per_pane_damage {
        if pane.bell_active {
            rects.clear();
            break;
        }
        match pane.cursor_damage {
            PaneFrameDamage::Unchanged => {}
            PaneFrameDamage::CursorOnly(Some(d)) => {
                rects.push(freminal_windowing::DamageRect {
                    x: d.x,
                    y: d.y,
                    width: d.width,
                    height: d.height,
                });
            }
            PaneFrameDamage::CursorOnly(None) | PaneFrameDamage::Full => {
                rects.clear();
                break;
            }
        }
    }

    if rects.is_empty() {
        freminal_windowing::FrameDamage::Full
    } else {
        freminal_windowing::FrameDamage::Partial(rects)
    }
}

/// #435/#436 composition (§6): reconcile the #435 partial-present decision
/// with the #436 chrome-cache decision. They are computed separately but must
/// agree: if the chrome changed pixels this frame
/// ([`freminal_windowing::ChromeDamage::Changed`]), the frame must NOT be
/// presented [`freminal_windowing::FrameDamage::Partial`] — #435's
/// `buffer_age() == 1` fast path assumes every pixel outside the damage rect
/// is bit-identical to the previous frame, but a chrome rebuild may have
/// changed pixels outside a cursor rect, which would then be left stale on
/// screen. So a `Changed` chrome frame forces `Full`.
///
/// On a REPLAY frame `chrome_damage` is `Unchanged` by construction (REPLAY is
/// only chosen when the previous frame was `Unchanged` and no chrome input
/// landed this frame), so this is a no-op there and the headline cursor-only
/// partial-present path (idle blink) is preserved. When `chrome_damage` is
/// `Changed` this conservatively presents `Full` rather than risk a stale
/// `Partial`.
#[must_use]
pub fn compose_with_chrome_damage(
    frame_damage: freminal_windowing::FrameDamage,
    chrome_damage: freminal_windowing::ChromeDamage,
) -> freminal_windowing::FrameDamage {
    match chrome_damage {
        freminal_windowing::ChromeDamage::Changed => freminal_windowing::FrameDamage::Full,
        freminal_windowing::ChromeDamage::Unchanged => frame_damage,
    }
}

#[cfg(test)]
mod tests {
    use super::{PaneDamageInput, compose_with_chrome_damage, decide_frame_damage};
    use crate::gui::renderer::{CursorDamage, PaneFrameDamage};
    use freminal_windowing::{ChromeDamage, DamageRect, FrameDamage};

    /// `FrameDamage` does not implement `PartialEq` (see its definition),
    /// so tests compare it structurally by hand.
    fn assert_full(damage: &FrameDamage) {
        assert!(
            matches!(damage, FrameDamage::Full),
            "expected FrameDamage::Full, got {damage:?}"
        );
    }

    fn assert_partial(damage: &FrameDamage, expected_rects: &[DamageRect]) {
        match damage {
            FrameDamage::Partial(rects) => {
                assert_eq!(rects, expected_rects);
            }
            FrameDamage::Full => {
                panic!("expected FrameDamage::Partial({expected_rects:?}), got Full")
            }
        }
    }

    fn rect(x: i32, y: i32, width: i32, height: i32) -> DamageRect {
        DamageRect {
            x,
            y,
            width,
            height,
        }
    }

    fn unchanged_pane() -> PaneDamageInput {
        PaneDamageInput {
            bell_active: false,
            cursor_damage: PaneFrameDamage::Unchanged,
        }
    }

    fn cursor_only_pane(d: CursorDamage) -> PaneDamageInput {
        PaneDamageInput {
            bell_active: false,
            cursor_damage: PaneFrameDamage::CursorOnly(Some(d)),
        }
    }

    #[test]
    fn force_full_wins_regardless_of_other_inputs() {
        let panes = [cursor_only_pane(CursorDamage {
            x: 1,
            y: 2,
            width: 3,
            height: 4,
        })];
        // toast_active also true, and a valid cursor rect present: force_full
        // still must win.
        assert_full(&decide_frame_damage(true, true, &panes));
        assert_full(&decide_frame_damage(true, false, &panes));
    }

    #[test]
    fn toast_active_forces_full_when_not_force_full() {
        let panes = [cursor_only_pane(CursorDamage {
            x: 1,
            y: 2,
            width: 3,
            height: 4,
        })];
        assert_full(&decide_frame_damage(false, true, &panes));
    }

    #[test]
    fn empty_pane_list_is_full() {
        assert_full(&decide_frame_damage(false, false, &[]));
    }

    #[test]
    fn all_panes_unchanged_is_full() {
        let panes = [unchanged_pane(), unchanged_pane()];
        assert_full(&decide_frame_damage(false, false, &panes));
    }

    #[test]
    fn single_cursor_only_rect_is_partial() {
        let d = CursorDamage {
            x: 10,
            y: 20,
            width: 8,
            height: 16,
        };
        let panes = [cursor_only_pane(d)];
        let damage = decide_frame_damage(false, false, &panes);
        assert_partial(&damage, &[rect(10, 20, 8, 16)]);
    }

    #[test]
    fn two_cursor_only_rects_is_partial_pane_switch_case() {
        let d1 = CursorDamage {
            x: 0,
            y: 0,
            width: 8,
            height: 16,
        };
        let d2 = CursorDamage {
            x: 100,
            y: 0,
            width: 8,
            height: 16,
        };
        let panes = [cursor_only_pane(d1), cursor_only_pane(d2)];
        let damage = decide_frame_damage(false, false, &panes);
        assert_partial(&damage, &[rect(0, 0, 8, 16), rect(100, 0, 8, 16)]);
    }

    #[test]
    fn bell_active_pane_forces_full_and_clears_prior_rects() {
        let d = CursorDamage {
            x: 0,
            y: 0,
            width: 8,
            height: 16,
        };
        let panes = [
            cursor_only_pane(d),
            PaneDamageInput {
                bell_active: true,
                cursor_damage: PaneFrameDamage::Unchanged,
            },
        ];
        assert_full(&decide_frame_damage(false, false, &panes));
    }

    #[test]
    fn cursor_only_none_forces_full() {
        let panes = [PaneDamageInput {
            bell_active: false,
            cursor_damage: PaneFrameDamage::CursorOnly(None),
        }];
        assert_full(&decide_frame_damage(false, false, &panes));
    }

    #[test]
    fn pane_full_forces_full() {
        let panes = [PaneDamageInput {
            bell_active: false,
            cursor_damage: PaneFrameDamage::Full,
        }];
        assert_full(&decide_frame_damage(false, false, &panes));
    }

    #[test]
    fn unresolvable_pane_is_represented_by_caller_forcing_full() {
        // The caller cannot build a `PaneDamageInput` for a pane it failed
        // to resolve in the tree; it instead calls this function with
        // `force_full = true` for the whole frame. Verify that path yields
        // `Full` even with an otherwise-empty pane list.
        assert_full(&decide_frame_damage(true, false, &[]));
    }

    #[test]
    fn rects_cleared_when_a_later_pane_is_full() {
        let d = CursorDamage {
            x: 0,
            y: 0,
            width: 8,
            height: 16,
        };
        let panes = [
            cursor_only_pane(d),
            PaneDamageInput {
                bell_active: false,
                cursor_damage: PaneFrameDamage::Full,
            },
        ];
        assert_full(&decide_frame_damage(false, false, &panes));
    }

    // ── #435/#436 composition (§6): compose_with_chrome_damage ──────────

    #[test]
    fn chrome_changed_forces_full_even_when_frame_damage_was_partial() {
        // The load-bearing case: a Partial (cursor-only) present must be
        // upgraded to Full when chrome changed pixels this frame, or the
        // changed chrome outside the cursor rect would be left stale.
        let partial = FrameDamage::Partial(vec![rect(0, 0, 8, 16)]);
        let composed = compose_with_chrome_damage(partial, ChromeDamage::Changed);
        assert_full(&composed);
    }

    #[test]
    fn chrome_changed_forces_full_when_frame_damage_was_already_full() {
        let composed = compose_with_chrome_damage(FrameDamage::Full, ChromeDamage::Changed);
        assert_full(&composed);
    }

    #[test]
    fn chrome_unchanged_preserves_partial() {
        // The headline REPLAY / idle-blink case: chrome is Unchanged, so the
        // cursor-only Partial present survives untouched.
        let rects = [rect(10, 20, 8, 16)];
        let partial = FrameDamage::Partial(rects.to_vec());
        let composed = compose_with_chrome_damage(partial, ChromeDamage::Unchanged);
        assert_partial(&composed, &rects);
    }

    #[test]
    fn chrome_unchanged_preserves_full() {
        let composed = compose_with_chrome_damage(FrameDamage::Full, ChromeDamage::Unchanged);
        assert_full(&composed);
    }
}
