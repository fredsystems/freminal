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

use crate::gui::renderer::{PaneDamageRect, PaneFrameDamage};

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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneDamageInput {
    /// `pane.view_state.bell_since.is_some()` for this pane.
    pub(crate) bell_active: bool,
    /// `pane.render_cache.last_frame_cursor_damage` for this pane.
    pub(crate) cursor_damage: PaneFrameDamage,
    /// This pane's search-overlay popup damage rects this frame (Task
    /// 124.14d), from `pane.render_cache.search_overlay_damage_rects()` --
    /// the deduplicated union of the search bar's previous and current
    /// paint bounds. Unioned into this frame's damage independently of
    /// `cursor_damage`: a query with no visible matches still moves the
    /// floating popup, and a bounded/`Unchanged` pane can still have moved
    /// it. Never populated except by the caller; `Vec::new()` when search
    /// is closed or was already settled closed last frame.
    pub(crate) search_overlay_rects: Vec<PaneDamageRect>,
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
/// 1. `force_full` short-circuits to [`FrameDamage::Full`]. The caller
///    composes it as `ui_overlay_open || shader_recomposites ||
///    active_pane_changed || pointer_forces_full_present(..) ||
///    unresolved_pane`.
///
///    The bare `pointer_moving` term this comment used to name was
///    **removed by issue #459 item 9** and replaced by
///    `pointer_forces_full_present`, which forces `Full` only when the
///    motion is over a chrome-interactive region or a pane-border drag is
///    in progress. Motion purely over terminal content does not. The
///    correction is recorded here because Task 123's Obligation 2
///    investigated exactly that hypothesis: a reader trusting the old
///    wording would have taken a **false confirmation** of a diagnosis
///    that 123 refuted (see 124.C1 in
///    `Documents/PLAN_124_RENDER_EFFICIENCY.md`).
/// 2. Otherwise, `toast_active` short-circuits to `Full`.
/// 3. Otherwise, `per_pane_damage` is walked in order (this must be the
///    same order as `pane_layout`, i.e. only the panes actually rendered
///    this frame):
///    - `bell_active` -> return `Full` immediately, discarding any rects
///      collected from earlier panes.
///    - `cursor_damage == CursorOnly(Some(rect))` -> push `rect`.
///    - `cursor_damage == Region(rects)` (Task 124.14) -> push every rect in
///      `rects` (never empty by construction).
///    - `cursor_damage == CursorOnly(None)` or `Full` -> return `Full`
///      immediately, discarding any rects collected from earlier panes.
///    - `cursor_damage == Unchanged` -> contributes nothing on its own.
///    - Then (Task 124.14d), unless this pane already returned `Full`
///      above: push every rect in `search_overlay_rects`. This is
///      independent of `cursor_damage`'s own class -- the search overlay's
///      popup can move on a frame the terminal band itself reports
///      `Unchanged`.
/// 4. If every pane in the loop above finished without returning `Full`
///    (Task 124.2 makes this structural -- `bell_active`, `CursorOnly(None)`,
///    and `Full` each `return freminal_windowing::FrameDamage::Full`
///    directly, rather than setting a flag the loop checks afterward), the
///    result depends on what was collected: if no rects were collected at
///    all -- every rendered pane's own damage was `Unchanged` and no pane
///    contributed search-overlay rects either -- the result is
///    [`freminal_windowing::FrameDamage::None`] (Task 124.2): nothing
///    whatsoever changed this frame, so the windowing layer may skip the
///    clear and every GL primitive paint entirely rather than presenting a
///    frame that changed zero pixels. Otherwise the result is
///    `Partial(rects)`.
///
/// An unresolvable pane in the original block behaved identically to a
/// `bell_active` pane: return `Full` immediately. Since this function has
/// no pane tree to resolve against, that case is represented by the
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
            // Deliberately NOT bounded to the bell's own pane, unlike the
            // `FullRebuildDamage::Full` case widget.rs now bounds (Task
            // 124.21 finding 2). A bell also feeds
            // `ChromeSignals::bell_active` (`app_impl.rs`), which forces the
            // whole window `Full` through `compose_with_chrome_damage`
            // regardless of what this function returns -- so bounding the
            // pane half here would buy nothing and would falsely suggest
            // bell damage is pane-bounded. Do not "fix" this.
            //
            // Task 124.2: returns `Full` immediately (rather than setting a
            // flag and breaking out of the loop to check it afterward), so
            // the precedence of `Full` over the newly possible `None` is
            // structural -- there is no later branch that could see this
            // pane's contribution and mistake it for "nothing changed".
            return freminal_windowing::FrameDamage::Full;
        }
        match &pane.cursor_damage {
            PaneFrameDamage::Unchanged => {}
            PaneFrameDamage::CursorOnly(Some(d)) => {
                rects.push(freminal_windowing::DamageRect {
                    x: d.x,
                    y: d.y,
                    width: d.width,
                    height: d.height,
                });
            }
            PaneFrameDamage::Region(region_rects) => {
                rects.extend(region_rects.iter().map(|d| freminal_windowing::DamageRect {
                    x: d.x,
                    y: d.y,
                    width: d.width,
                    height: d.height,
                }));
            }
            PaneFrameDamage::CursorOnly(None) | PaneFrameDamage::Full => {
                // Task 124.2: same reasoning as the `bell_active` arm above
                // -- return `Full` immediately rather than flagging it.
                return freminal_windowing::FrameDamage::Full;
            }
        }
        rects.extend(
            pane.search_overlay_rects
                .iter()
                .map(|d| freminal_windowing::DamageRect {
                    x: d.x,
                    y: d.y,
                    width: d.width,
                    height: d.height,
                }),
        );
        // Task 124.14d: the search overlay's popup damage is independent
        // of this pane's own terminal-content damage class -- a bounded or
        // `Unchanged` pane can still have moved its floating search bar.
        // Only reached when the match above did not already return `Full`.
    }

    if rects.is_empty() {
        // Task 124.2: no short-circuit fired (no `force_full`, no
        // `toast_active`, no bell/degenerate-cursor/pane-`Full` in the
        // loop above -- any of those returned `Full` immediately) AND no
        // pane or search overlay pushed a single rect -- every rendered
        // pane genuinely changed nothing this frame. This is the exact
        // case the doc comment above (step 4) and
        // `PLAN_124_RENDER_EFFICIENCY.md`'s 124.2 describe: present
        // nothing rather than a full clear + present of pixel-identical
        // content.
        freminal_windowing::FrameDamage::None
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
    use crate::gui::renderer::{PaneDamageRect, PaneFrameDamage};
    use freminal_windowing::{ChromeDamage, DamageRect, FrameDamage};

    /// `FrameDamage` does not implement `PartialEq` (see its definition),
    /// so tests compare it structurally by hand.
    fn assert_full(damage: &FrameDamage) {
        assert!(
            matches!(damage, FrameDamage::Full),
            "expected FrameDamage::Full, got {damage:?}"
        );
    }

    /// Task 124.2: assert `damage` is [`FrameDamage::None`] -- nothing
    /// whatsoever changed this frame, so the caller may skip presenting
    /// entirely.
    fn assert_none(damage: &FrameDamage) {
        assert!(
            matches!(damage, FrameDamage::None),
            "expected FrameDamage::None, got {damage:?}"
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
            FrameDamage::None => {
                panic!("expected FrameDamage::Partial({expected_rects:?}), got None")
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
            search_overlay_rects: Vec::new(),
        }
    }

    fn cursor_only_pane(d: PaneDamageRect) -> PaneDamageInput {
        PaneDamageInput {
            bell_active: false,
            cursor_damage: PaneFrameDamage::CursorOnly(Some(d)),
            search_overlay_rects: Vec::new(),
        }
    }

    /// A pane reporting [`PaneFrameDamage::Region`] (Task 124.14): a full
    /// vertex rebuild whose damage is provably bounded to `rects`.
    fn region_pane(rects: Vec<PaneDamageRect>) -> PaneDamageInput {
        PaneDamageInput {
            bell_active: false,
            cursor_damage: PaneFrameDamage::Region(rects),
            search_overlay_rects: Vec::new(),
        }
    }

    /// A pane reporting [`PaneFrameDamage::Full`], with no search overlay
    /// damage (Task 124.14d test helper).
    fn full_pane() -> PaneDamageInput {
        PaneDamageInput {
            bell_active: false,
            cursor_damage: PaneFrameDamage::Full,
            search_overlay_rects: Vec::new(),
        }
    }

    /// A pane with `bell_active: true` and otherwise-`Unchanged` cursor
    /// damage and no search overlay damage (Task 124.14d test helper).
    fn bell_pane() -> PaneDamageInput {
        PaneDamageInput {
            bell_active: true,
            cursor_damage: PaneFrameDamage::Unchanged,
            search_overlay_rects: Vec::new(),
        }
    }

    /// A pane whose ONLY contribution is search-overlay popup damage
    /// (Task 124.14d): `Unchanged` terminal-content damage, with the given
    /// popup rects.
    fn search_overlay_pane(rects: Vec<PaneDamageRect>) -> PaneDamageInput {
        PaneDamageInput {
            bell_active: false,
            cursor_damage: PaneFrameDamage::Unchanged,
            search_overlay_rects: rects,
        }
    }

    #[test]
    fn force_full_wins_regardless_of_other_inputs() {
        let panes = [cursor_only_pane(PaneDamageRect {
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
        let panes = [cursor_only_pane(PaneDamageRect {
            x: 1,
            y: 2,
            width: 3,
            height: 4,
        })];
        assert_full(&decide_frame_damage(false, true, &panes));
    }

    /// OBLIGATION 2 of `PLAN_123_GL_MEASUREMENT_HARNESS.md`: diagnose the
    /// 121.31 full-present-during-pointer-motion anomaly, with the startup
    /// toast held fixed.
    ///
    /// **Verdict: `pointer_forces_full_present` is REFUTED as the cause.
    /// The toast confounded the original observation — but it is not the
    /// whole story, and this test alone does not show that it is.**
    ///
    /// This test varies only `force_full` and `toast_active`, holding pane
    /// damage fixed at `CursorOnly(Some(rect))`. That is enough to refute
    /// the *named* cause, and no more. The mechanism that actually produces
    /// a full present during motion over inert content is the
    /// `PaneFrameDamage::Unchanged` fallback, which this fixture assumes
    /// away by supplying a rect — see
    /// [`pointer_motion_over_inert_content_is_full_via_the_unchanged_fallback`]
    /// below.
    ///
    /// The original observation was `frame_damage_full=120,
    /// frame_damage_partial=0` during pointer motion, versus `120/120`
    /// partial at idle. The confound recorded at the time was that
    /// `toast_active=48` fired in every run, because a startup toast was
    /// present — and `toast_active` is its own short-circuit to `Full`,
    /// entirely independent of the pointer.
    ///
    /// A live session cannot hold that variable fixed; the toast expires on
    /// a timer while the gesture is still in progress. Here it is a
    /// parameter, so the two cases separate cleanly. This is the whole
    /// reason 123's deterministic construction was worth building for this
    /// obligation: the experiment the original observation needed was not
    /// performable by hand.
    ///
    /// Note also that `decide_frame_damage`'s own doc comment above still
    /// describes `force_full` as including a bare `pointer_moving` term.
    /// It does not, and has not since #459 item 9: `app_impl.rs` computes
    /// `force_full` with `pointer_forces_full_present(pointer_moving,
    /// pointer_over_chrome, border_drag_active)`. Recorded as cleanup entry
    /// 123.C1 in `PLAN_123_GL_MEASUREMENT_HARNESS.md`.
    #[test]
    fn pointer_motion_over_content_is_partial_once_the_toast_confound_is_removed() {
        let d = PaneDamageRect {
            x: 10,
            y: 20,
            width: 8,
            height: 16,
        };
        let panes = [cursor_only_pane(d)];

        // Pointer moving over plain terminal content, not over chrome, no
        // border drag latched. `pointer_forces_full_present(true, false,
        // false)` is false (see its truth-table test in `app_impl`), so the
        // pointer contributes nothing to `force_full`.
        let pointer_term = false;

        // With no toast: the frame is Partial. Pointer motion alone does
        // NOT force a full present.
        assert_partial(
            &decide_frame_damage(pointer_term, false, &panes),
            &[rect(10, 20, 8, 16)],
        );

        // With a toast present and everything else identical: Full. This is
        // the only variable that changed, and it fully accounts for the
        // observed 120/0 split.
        assert_full(&decide_frame_damage(pointer_term, true, &panes));
    }

    /// OBLIGATION 2, the part the first two tests assumed away — and the
    /// one that actually explains a full present during pointer motion
    /// (before Task 124.2 fixed it).
    ///
    /// The tests above hold the *pane* damage fixed at
    /// `CursorOnly(Some(rect))` and vary only `force_full` / `toast_active`.
    /// That correctly refutes `pointer_forces_full_present` as a cause, but
    /// it assumes the pane had a cursor rect to report. During pointer
    /// motion over inert content it does not: nothing changed, so
    /// `evaluate_frame_dirty_state` reports no observations,
    /// `widget.rs` leaves `last_frame_cursor_damage` at
    /// [`PaneFrameDamage::Unchanged`], and no rect is ever pushed.
    ///
    /// **Inverted by Task 124.2, per the plan's explicit mandate ("must be
    /// revisited, not deleted").** Before this fix, `decide_frame_damage`
    /// fell through to `rects.is_empty()` and returned
    /// [`FrameDamage::Full`] — a frame in which nothing whatsoever changed
    /// was presented as a full clear plus a full present. Now it returns
    /// [`FrameDamage::None`]: the windowing layer skips the clear and every
    /// GL primitive paint entirely rather than presenting pixel-identical
    /// content.
    ///
    /// This is the real mechanism behind the 121.31 observation, and it was
    /// never the pointer predicate — it was structural: `FrameDamage` had
    /// only `Full` and `Partial` variants, with no way to say "nothing
    /// changed, present nothing", so any frame drawn at all while idle cost
    /// a full present.
    ///
    /// Cost, from Task 123's harness: a full present is a full clear plus
    /// ~52 GL calls and ~200 KB of uploads — now avoided entirely for a
    /// frame that is pixel-identical to its predecessor.
    #[test]
    fn pointer_motion_over_inert_content_is_none_now_that_124_2_fixed_the_unchanged_fallback() {
        // Pointer moving over terminal content: `pointer_forces_full_present`
        // is false, so `force_full` is false. No toast. And the pane reports
        // `Unchanged`, because motion over inert content changes nothing the
        // dirty tracker observes.
        let panes = [unchanged_pane()];

        assert_none(&decide_frame_damage(false, false, &panes));

        // The same holds for several settled panes -- it is not an artefact
        // of the single-pane case.
        let many = [unchanged_pane(), unchanged_pane(), unchanged_pane()];
        assert_none(&decide_frame_damage(false, false, &many));
    }

    /// The other half of Obligation 2: pointer motion *over chrome* really
    /// does force a full present, and correctly so.
    ///
    /// Keeps the refutation above honest. `pointer_forces_full_present` is
    /// not dead code and not always-false — it fires exactly when motion
    /// can change chrome pixels (a hover tint) or when a pane-border drag
    /// is latched. The 121.31 anomaly was not this term firing; it was the
    /// toast.
    #[test]
    fn pointer_motion_over_chrome_forces_full_even_with_no_toast() {
        let d = PaneDamageRect {
            x: 10,
            y: 20,
            width: 8,
            height: 16,
        };
        let panes = [cursor_only_pane(d)];

        // `pointer_forces_full_present(true, true, false)` is true, so the
        // caller passes `force_full = true`.
        assert_full(&decide_frame_damage(true, false, &panes));
    }

    /// Task 124.2: an empty pane list collects no rects and triggers no
    /// short-circuit, so per the function's own contract this is `None`,
    /// not `Full` -- an empty `pane_layout` is indistinguishable from "every
    /// pane was `Unchanged`" as far as this function can tell. The ONLY
    /// route to `Full` for a genuinely unresolvable/empty case is the
    /// caller passing `force_full = true` (see
    /// `unresolvable_pane_is_represented_by_caller_forcing_full` below).
    #[test]
    fn empty_pane_list_is_none_absent_a_caller_force_full() {
        assert_none(&decide_frame_damage(false, false, &[]));
    }

    /// Task 124.2: every rendered pane reporting `Unchanged` (no bell, no
    /// search-overlay rects) collects no rects and triggers no
    /// short-circuit, so the result is `None`, not `Full`.
    #[test]
    fn all_panes_unchanged_is_none() {
        let panes = [unchanged_pane(), unchanged_pane()];
        assert_none(&decide_frame_damage(false, false, &panes));
    }

    // ── Task 124.2: short-circuit precedence over `None` ─────────────────
    //
    // Every one of `decide_frame_damage`'s existing short-circuits must
    // still win over the newly possible `None` outcome -- an all-`Unchanged`
    // pane set is exactly the input that would otherwise produce `None`, so
    // each test below holds that input fixed and adds exactly one
    // short-circuit to prove it still overrides.

    /// `force_full` wins over `None` -- without this, a caller-forced
    /// full-window redraw (an open overlay, an active-pane change, chrome
    /// motion) could be silently discarded whenever every pane happened to
    /// be `Unchanged`.
    #[test]
    fn force_full_wins_over_all_panes_unchanged() {
        let panes = [unchanged_pane(), unchanged_pane()];
        assert_full(&decide_frame_damage(true, false, &panes));
    }

    /// `toast_active` wins over `None` for the same reason.
    #[test]
    fn toast_active_wins_over_all_panes_unchanged() {
        let panes = [unchanged_pane(), unchanged_pane()];
        assert_full(&decide_frame_damage(false, true, &panes));
    }

    /// A bell in one pane wins over `None`, even when every pane's own
    /// `cursor_damage` is `Unchanged` -- the bell-active flag is checked
    /// before the `Unchanged` arm contributes (or fails to contribute)
    /// anything.
    #[test]
    fn bell_active_wins_over_all_panes_unchanged() {
        let panes = [unchanged_pane(), bell_pane()];
        assert_full(&decide_frame_damage(false, false, &panes));
    }

    /// A degenerate `CursorOnly(None)` pane (the pane could not bound its
    /// own cursor rect) wins over `None`, even alongside otherwise-
    /// `Unchanged` siblings.
    #[test]
    fn cursor_only_none_wins_over_all_panes_unchanged() {
        let panes = [
            unchanged_pane(),
            PaneDamageInput {
                bell_active: false,
                cursor_damage: PaneFrameDamage::CursorOnly(None),
                search_overlay_rects: Vec::new(),
            },
        ];
        assert_full(&decide_frame_damage(false, false, &panes));
    }

    /// A `PaneFrameDamage::Full` pane wins over `None`, even alongside
    /// otherwise-`Unchanged` siblings.
    #[test]
    fn pane_full_wins_over_all_panes_unchanged() {
        let panes = [unchanged_pane(), full_pane()];
        assert_full(&decide_frame_damage(false, false, &panes));
    }

    #[test]
    fn single_cursor_only_rect_is_partial() {
        let d = PaneDamageRect {
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
        let d1 = PaneDamageRect {
            x: 0,
            y: 0,
            width: 8,
            height: 16,
        };
        let d2 = PaneDamageRect {
            x: 100,
            y: 0,
            width: 8,
            height: 16,
        };
        let panes = [cursor_only_pane(d1), cursor_only_pane(d2)];
        let damage = decide_frame_damage(false, false, &panes);
        assert_partial(&damage, &[rect(0, 0, 8, 16), rect(100, 0, 8, 16)]);
    }

    // ── PaneFrameDamage::Region aggregation (Task 124.14) ────────────────

    /// The base case: a single [`PaneFrameDamage::Region`] rect is
    /// aggregated exactly like a `CursorOnly(Some(rect))` one.
    #[test]
    fn single_region_rect_is_partial() {
        let d = PaneDamageRect {
            x: 4,
            y: 8,
            width: 16,
            height: 16,
        };
        let panes = [region_pane(vec![d])];
        let damage = decide_frame_damage(false, false, &panes);
        assert_partial(&damage, &[rect(4, 8, 16, 16)]);
    }

    /// The test that pins why `PaneFrameDamage::Region` carries a `Vec`
    /// rather than a single bounding box (124.14a's design decision,
    /// `PLAN_124_RENDER_EFFICIENCY.md` §124.14a: "rows 3 and 40 changed
    /// would present the 37 rows between them").
    ///
    /// Two non-contiguous rects must both survive aggregation, in order,
    /// rather than being collapsed into their enclosing bounding box. If a
    /// later change collapsed `Region` to a single rect at the source or in
    /// `decide_frame_damage`, this test would still see only one rect (or
    /// one covering the gap) and fail.
    #[test]
    fn two_non_contiguous_region_rects_are_both_partial_in_order() {
        let top = PaneDamageRect {
            x: 0,
            y: 0,
            width: 80,
            height: 16,
        };
        let bottom = PaneDamageRect {
            x: 0,
            y: 640,
            width: 80,
            height: 16,
        };
        let panes = [region_pane(vec![top, bottom])];
        let damage = decide_frame_damage(false, false, &panes);
        assert_partial(&damage, &[rect(0, 0, 80, 16), rect(0, 640, 80, 16)]);
    }

    /// A `Region` pane alongside an `Unchanged` pane: the unchanged pane
    /// contributes nothing, so the result is `Partial` with only the
    /// `Region` pane's rects.
    #[test]
    fn region_plus_unchanged_pane_is_partial_with_only_region_rects() {
        let d = PaneDamageRect {
            x: 1,
            y: 2,
            width: 3,
            height: 4,
        };
        let panes = [region_pane(vec![d]), unchanged_pane()];
        let damage = decide_frame_damage(false, false, &panes);
        assert_partial(&damage, &[rect(1, 2, 3, 4)]);
    }

    /// A `Region` pane followed by a `Full` pane: the `Full` pane's arm
    /// returns `Full` immediately, discarding the region's already-collected
    /// rects entirely, exactly as it already does for `CursorOnly(Some)`
    /// followed by `Full` (see `rects_cleared_when_a_later_pane_is_full`).
    #[test]
    fn region_plus_full_pane_discards_region_rects_and_is_full() {
        let d = PaneDamageRect {
            x: 1,
            y: 2,
            width: 3,
            height: 4,
        };
        let panes = [region_pane(vec![d]), full_pane()];
        assert_full(&decide_frame_damage(false, false, &panes));
    }

    /// A `Region` pane alongside another pane's `CursorOnly(Some(..))`:
    /// both contribute rects, and both survive aggregation.
    #[test]
    fn region_plus_cursor_only_rect_is_partial_with_both() {
        let region_rect = PaneDamageRect {
            x: 0,
            y: 0,
            width: 80,
            height: 16,
        };
        let cursor_rect = PaneDamageRect {
            x: 100,
            y: 50,
            width: 8,
            height: 16,
        };
        let panes = [
            region_pane(vec![region_rect]),
            cursor_only_pane(cursor_rect),
        ];
        let damage = decide_frame_damage(false, false, &panes);
        assert_partial(&damage, &[rect(0, 0, 80, 16), rect(100, 50, 8, 16)]);
    }

    // ── Search-overlay popup damage (Task 124.14d) ──────────────────────

    /// The base case named in the subtask spec: an `Unchanged` terminal
    /// pane with search-overlay popup damage must still present `Partial`
    /// on the popup rect -- the popup's own damage does not depend on the
    /// terminal band having changed at all.
    #[test]
    fn unchanged_pane_with_search_overlay_rect_is_partial() {
        let popup = PaneDamageRect {
            x: 500,
            y: 10,
            width: 60,
            height: 24,
        };
        let panes = [search_overlay_pane(vec![popup])];
        let damage = decide_frame_damage(false, false, &panes);
        assert_partial(&damage, &[rect(500, 10, 60, 24)]);
    }

    /// A pane reporting real cursor/row damage (`CursorOnly` or `Region`)
    /// AND search-overlay popup damage must present both, in order (the
    /// pane's own damage first, then the popup's).
    #[test]
    fn cursor_only_rect_plus_search_overlay_rect_is_partial_with_both_in_order() {
        let cursor_rect = PaneDamageRect {
            x: 0,
            y: 0,
            width: 8,
            height: 16,
        };
        let popup = PaneDamageRect {
            x: 500,
            y: 10,
            width: 60,
            height: 24,
        };
        let panes = [PaneDamageInput {
            bell_active: false,
            cursor_damage: PaneFrameDamage::CursorOnly(Some(cursor_rect)),
            search_overlay_rects: vec![popup],
        }];
        let damage = decide_frame_damage(false, false, &panes);
        assert_partial(&damage, &[rect(0, 0, 8, 16), rect(500, 10, 60, 24)]);
    }

    /// A `Region` pane's own rects and its search-overlay popup rects must
    /// both survive aggregation, in order.
    #[test]
    fn region_rect_plus_search_overlay_rect_is_partial_with_both_in_order() {
        let region_rect = PaneDamageRect {
            x: 0,
            y: 0,
            width: 80,
            height: 16,
        };
        let popup = PaneDamageRect {
            x: 500,
            y: 10,
            width: 60,
            height: 24,
        };
        let panes = [PaneDamageInput {
            bell_active: false,
            cursor_damage: PaneFrameDamage::Region(vec![region_rect]),
            search_overlay_rects: vec![popup],
        }];
        let damage = decide_frame_damage(false, false, &panes);
        assert_partial(&damage, &[rect(0, 0, 80, 16), rect(500, 10, 60, 24)]);
    }

    /// `Full` still clears a pane's own search-overlay popup rects -- a
    /// pane that needs a full rebuild has already made the popup's own
    /// bound moot.
    #[test]
    fn full_pane_clears_its_own_search_overlay_rects() {
        let popup = PaneDamageRect {
            x: 500,
            y: 10,
            width: 60,
            height: 24,
        };
        let panes = [PaneDamageInput {
            bell_active: false,
            cursor_damage: PaneFrameDamage::Full,
            search_overlay_rects: vec![popup],
        }];
        assert_full(&decide_frame_damage(false, false, &panes));
    }

    /// A bell in one pane still clears a DIFFERENT pane's search-overlay
    /// popup rects, same as it clears any other pane's rects.
    #[test]
    fn bell_clears_a_sibling_panes_search_overlay_rects() {
        let popup = PaneDamageRect {
            x: 500,
            y: 10,
            width: 60,
            height: 24,
        };
        let panes = [search_overlay_pane(vec![popup]), bell_pane()];
        assert_full(&decide_frame_damage(false, false, &panes));
    }

    /// Opening the search overlay (previous popup `None`, current
    /// `Some`) contributes damage on its own, with no terminal-content
    /// damage required.
    #[test]
    fn opening_search_overlay_rect_requires_no_terminal_damage() {
        let popup = PaneDamageRect {
            x: 500,
            y: 10,
            width: 60,
            height: 24,
        };
        let panes = [search_overlay_pane(vec![popup])];
        let damage = decide_frame_damage(false, false, &panes);
        assert_partial(&damage, &[rect(500, 10, 60, 24)]);
    }

    /// Closing the search overlay (only the previous popup rect
    /// remains, erasing it) likewise contributes damage on its own, with
    /// no terminal-content damage required.
    #[test]
    fn closing_search_overlay_rect_requires_no_terminal_damage() {
        let previous_popup = PaneDamageRect {
            x: 500,
            y: 10,
            width: 60,
            height: 24,
        };
        let panes = [search_overlay_pane(vec![previous_popup])];
        let damage = decide_frame_damage(false, false, &panes);
        assert_partial(&damage, &[rect(500, 10, 60, 24)]);
    }

    // ── Bounding a pane's own full rebuild (Task 124.21 finding 2 fix) ──
    //
    // These three tests use a pane-sized rect (much larger than the
    // cursor-sized rects used elsewhere in this file) to make clear they
    // are pinning the scenario `widget.rs`'s `full_pane_rebuild_damage_rect`
    // now produces for an ordinary full pane rebuild, not the generic
    // `Region` aggregation already covered above.

    /// The regression guard for the whole subtask. Before this fix, a pane
    /// needing a full rebuild reported `PaneFrameDamage::Full`, and
    /// `decide_frame_damage`'s `Full` arm returned immediately, discarding
    /// every rect collected from other panes -- so in a split, one busy
    /// pane forced a full clear + present of an `Unchanged` sibling too.
    /// Now the busy pane reports `Region` bounded to its own pane rect, so
    /// the sibling's `Unchanged` contribution (nothing) is untouched and
    /// the frame is `Partial` on only the busy pane's rect. If a future
    /// change silently reintroduced `PaneFrameDamage::Full` for the common
    /// full-rebuild case, this test would start seeing `Full` again.
    #[test]
    fn busy_pane_reports_only_its_own_pane_rect_not_full_alongside_unchanged_sibling() {
        let busy_pane_rect = PaneDamageRect {
            x: 0,
            y: 0,
            width: 960,
            height: 1080,
        };
        let panes = [region_pane(vec![busy_pane_rect]), unchanged_pane()];
        let damage = decide_frame_damage(false, false, &panes);
        assert_partial(&damage, &[rect(0, 0, 960, 1080)]);
    }

    /// The escalation path must remain intact and reachable: a pane whose
    /// own bounds could not be established (`widget.rs`'s degenerate-rect
    /// fallback) still reports `PaneFrameDamage::Full`, and that must still
    /// clear every rect collected from other panes and force the whole
    /// frame `Full` -- exactly as before this subtask, for the one case
    /// where a pane genuinely cannot bound its own damage.
    #[test]
    fn a_pane_that_cannot_bound_its_own_rect_still_forces_full_alongside_a_busy_sibling() {
        let sibling_rect = PaneDamageRect {
            x: 0,
            y: 0,
            width: 960,
            height: 1080,
        };
        let panes = [region_pane(vec![sibling_rect]), full_pane()];
        assert_full(&decide_frame_damage(false, false, &panes));
    }

    /// `bell_active` is deliberately NOT bounded to its own pane (see the
    /// comment at the `bell_active` arm in `decide_frame_damage`): even
    /// though a sibling pane now reports bounded `Region` damage instead of
    /// escalating to `Full` for an ordinary rebuild, a bell in ANY pane must
    /// still clear every collected rect and force the whole frame `Full`.
    #[test]
    fn bell_still_forces_full_even_when_a_sibling_pane_reports_bounded_region_damage() {
        let sibling_rect = PaneDamageRect {
            x: 0,
            y: 0,
            width: 960,
            height: 1080,
        };
        let panes = [region_pane(vec![sibling_rect]), bell_pane()];
        assert_full(&decide_frame_damage(false, false, &panes));
    }

    #[test]
    fn bell_active_pane_forces_full_and_clears_prior_rects() {
        let d = PaneDamageRect {
            x: 0,
            y: 0,
            width: 8,
            height: 16,
        };
        let panes = [cursor_only_pane(d), bell_pane()];
        assert_full(&decide_frame_damage(false, false, &panes));
    }

    #[test]
    fn cursor_only_none_forces_full() {
        let panes = [PaneDamageInput {
            bell_active: false,
            cursor_damage: PaneFrameDamage::CursorOnly(None),
            search_overlay_rects: Vec::new(),
        }];
        assert_full(&decide_frame_damage(false, false, &panes));
    }

    #[test]
    fn pane_full_forces_full() {
        let panes = [full_pane()];
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
        let d = PaneDamageRect {
            x: 0,
            y: 0,
            width: 8,
            height: 16,
        };
        let panes = [cursor_only_pane(d), full_pane()];
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

    /// Task 124.2: a chrome-`Changed` frame must upgrade `None` to `Full` --
    /// the same reasoning as the `Partial` -> `Full` upgrade above, since a
    /// chrome rebuild may have changed pixels this frame's #435 decision
    /// knows nothing about, and `None` promises even MORE strongly than
    /// `Partial` that nothing needs redrawing.
    #[test]
    fn chrome_changed_upgrades_none_to_full() {
        let composed = compose_with_chrome_damage(FrameDamage::None, ChromeDamage::Changed);
        assert_full(&composed);
    }

    /// Task 124.2: a chrome-`Unchanged` frame preserves `None` untouched --
    /// the headline case this subtask exists for: an idle frame where
    /// nothing changed anywhere must still resolve to `None` after
    /// composition, not be silently upgraded.
    #[test]
    fn chrome_unchanged_preserves_none() {
        let composed = compose_with_chrome_damage(FrameDamage::None, ChromeDamage::Unchanged);
        assert_none(&composed);
    }
}
