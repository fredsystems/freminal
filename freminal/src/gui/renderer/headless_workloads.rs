// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Workload assertions against the headless recording harness (123.8).
//!
//! Each test below drives [`super::headless`] over a described synthetic
//! frame and asserts **concrete** derived-metric counts, so a future change
//! that alters how much GL work a frame costs fails here rather than being
//! noticed by eye months later. The counts are reproducible because
//! `synthetic_grid` generates content from cell coordinates rather than
//! randomly.
//!
//! # What these tests stand in for, and what they do not
//!
//! 123.8 requires each assertion to say which real GUI path it represents,
//! so a later reader can tell "this is exactly what production does" from
//! "this is a representative approximation". Each test carries that note.
//!
//! Two of the six workloads 123.14 names are **deliberately absent**, and
//! their absence is a finding rather than an omission:
//!
//! - **Pointer motion** (over inert content, and with a URL on screen).
//!   The renderer does nothing different on pointer motion; the cost lives
//!   in the GUI event layer (`gui/pointer_motion.rs`, `gui/frame_damage.rs`)
//!   multiplied by a compositor-determined event *rate* this harness cannot
//!   observe. Measuring it here would produce a number that looks
//!   authoritative and means nothing. It is covered instead by the
//!   decision-layer tests in `gui::frame_damage` and by
//!   `freminal/benches/pointer_motion_bench.rs`, which reports cost *per
//!   event* and leaves the rate as an explicit, unmeasured multiplier.
//! - **Alt screen.** Switching to the alternate screen changes what the
//!   buffer layer produces, not how the renderer draws it. There is no
//!   renderer-level distinction to assert, so asserting one would be
//!   inventing a difference.
//!
//! # Why byte volumes are not asserted here
//!
//! Decided before any test was written, per `flaky-tests-are-bugs`: these
//! tests assert **call counts** (draw calls, state changes, upload counts),
//! which are pure control flow and identical on every platform. They do
//! **not** assert upload *byte volumes* or glyph-rasterisation counts,
//! which depend on the bundled font's rasterised glyph extents. Those are
//! deterministic for a given font version but would shift if the bundled
//! font were replaced — pinning them would convert a font bump into a
//! mysterious test failure. Byte volumes are reported in 123.14 from a
//! single named reference platform instead.

// This module is declared `#[cfg(all(test, feature = "gl-recording"))]`, so
// it is test-only code and the repo's panic-free rule does not apply.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::gl_facade::recording::{GlCall, GlCallPayload};
use super::gl_facade::surface::{DRAW_CALL_METHODS, STATE_CHANGE_METHODS, UPLOAD_METHODS};
use super::headless::{
    CursorPresence, HeadlessRenderer, SyntheticFrame, ToastPresence, record_cursor_only,
    record_steady_state,
};

/// The derived metrics 123.8 asserts on, computed from a recorded log using
/// the groupings frozen in [`super::gl_facade::surface`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Metrics {
    total: usize,
    draws: usize,
    state_changes: usize,
    uploads: usize,
}

impl Metrics {
    fn of(calls: &[GlCall]) -> Self {
        Self {
            total: calls.len(),
            draws: calls
                .iter()
                .filter(|c| DRAW_CALL_METHODS.contains(&c.method))
                .count(),
            state_changes: calls
                .iter()
                .filter(|c| STATE_CHANGE_METHODS.contains(&c.method))
                .count(),
            uploads: calls
                .iter()
                .filter(|c| UPLOAD_METHODS.contains(&c.method))
                .count(),
        }
    }
}

/// Total instances submitted across every draw call in `calls`.
fn instances(calls: &[GlCall]) -> u64 {
    calls
        .iter()
        .filter_map(|c| match c.payload {
            GlCallPayload::Draw { instances, .. } => Some(u64::from(instances)),
            GlCallPayload::None | GlCallPayload::Upload { .. } => None,
        })
        .sum()
}

/// Bytes uploaded across every upload call in `calls`.
fn uploaded_bytes(calls: &[GlCall]) -> u64 {
    calls
        .iter()
        .filter_map(|c| match c.payload {
            GlCallPayload::Upload { bytes } => Some(bytes),
            GlCallPayload::None | GlCallPayload::Draw { .. } => None,
        })
        .sum()
}

/// A standard 80x24 grid — the size `PROFILING.md` uses for the
/// full-screen-redraw workload.
const fn standard() -> SyntheticFrame {
    SyntheticFrame::new(80, 24)
}

#[test]
fn init_dominates_a_single_frame() {
    // Stands in for: application startup, the first `renderer.init(gl)`
    // inside the terminal `PaintCallback` (`widget.rs:2960`). Runs once per
    // GL context, never per frame.
    //
    // This is the measurement that justifies `record_steady_state`
    // discarding init: one-time setup is roughly five times a whole frame,
    // so a harness that mixed them would report per-frame costs that were
    // mostly setup at low frame counts.
    let gl = super::gl_facade::Gl::recording();
    let mut driver = HeadlessRenderer::new().expect("headless driver constructs");
    driver
        .init(&gl)
        .expect("init succeeds against the recording arm");
    let init = gl
        .recorded()
        .map_or_else(Vec::new, super::gl_facade::recording::RecordingState::calls);
    let init = Metrics::of(&init);

    let frame = Metrics::of(&record_steady_state(&standard(), 1).expect("frame records"));

    assert_eq!(init.total, 261, "one-time GL setup call count");
    assert_eq!(init.draws, 0, "init draws nothing");
    assert_eq!(init.state_changes, 30);
    assert!(
        init.total > frame.total * 4,
        "init ({}) should dwarf one frame ({}) — if this ever stops being \
         true, `record_steady_state`'s init-discarding is no longer \
         load-bearing and the reporting in 123.14 can be simplified",
        init.total,
        frame.total
    );
}

#[test]
fn full_screen_redraw_costs_two_draw_calls() {
    // Stands in for: the full-rebuild branch of the terminal
    // `PaintCallback` (`widget.rs`, the `else` arm of `is_cursor_only`) —
    // i.e. `TerminalRenderer::draw_with_verts` with non-empty background,
    // decoration and foreground buffers. This is the TUI-redraw workload.
    let m = Metrics::of(&record_steady_state(&standard(), 1).expect("frame records"));

    // Two draws: one instanced background pass, one instanced foreground
    // pass. The decoration pass draws the cursor quad and is counted in the
    // same instanced family.
    assert_eq!(m.draws, 2, "full redraw issues two draw calls");
    assert_eq!(m.state_changes, 21);
    assert_eq!(m.uploads, 5);
    assert_eq!(m.total, 53);
}

#[test]
fn call_count_is_independent_of_grid_size() {
    // Stands in for: the same full-rebuild path at two terminal sizes.
    //
    // This is a genuine (and reassuring) property of the instanced
    // renderer, not an artefact: an 8x2 grid and an 80x24 grid issue
    // byte-identical *call* sequences. What scales is the instance count
    // and the uploaded byte volume, not the number of GL entry points
    // crossed. Any future change that makes call count scale with grid
    // size is a serious regression, and this test is what catches it.
    let small = record_steady_state(&SyntheticFrame::new(8, 2), 1).expect("small frame");
    let large = record_steady_state(&standard(), 1).expect("large frame");

    assert_eq!(
        Metrics::of(&small),
        Metrics::of(&large),
        "call counts must not scale with grid size"
    );
    assert!(
        instances(&small) < instances(&large),
        "instance counts must scale with grid size ({} vs {})",
        instances(&small),
        instances(&large)
    );
}

#[test]
fn hiding_the_cursor_removes_exactly_one_draw_call() {
    // Stands in for: a frame where the cursor is hidden (DECTCEM reset, or
    // the pane is unfocused), versus the same frame with it shown. The
    // difference is the decoration pass, which exists this frame only
    // because the cursor quad is in it.
    let shown = Metrics::of(&record_steady_state(&standard(), 1).expect("cursor shown"));
    let hidden = Metrics::of(
        &record_steady_state(&standard().with_cursor(CursorPresence::Hidden), 1)
            .expect("cursor hidden"),
    );

    assert_eq!(shown.draws, 2);
    assert_eq!(hidden.draws, 1, "no cursor means no decoration draw");
    assert_eq!(hidden.total, 38);
    assert_eq!(hidden.uploads, 3, "the decoration buffer is not uploaded");
}

#[test]
fn a_toast_more_than_doubles_frame_cost() {
    // Stands in for: the toast overlay's own `PaintCallback`
    // (`gui/toast.rs:1851`), which runs in addition to the terminal pane's.
    // Both toast passes are fully separate programs, VAOs and textures.
    //
    // Worth having a number for: a toast is not a decoration on an existing
    // frame, it is a second frame's worth of GL work layered on top, and it
    // additionally forces a Full present (`decide_frame_damage`'s
    // `toast_active` short-circuit) — which is the confound Obligation 2
    // has to hold fixed.
    let plain = Metrics::of(&record_steady_state(&standard(), 1).expect("no toast"));
    let toasted = Metrics::of(
        &record_steady_state(&standard().with_toast(ToastPresence::Present), 1)
            .expect("with toast"),
    );

    assert_eq!(plain.draws, 2);
    assert_eq!(toasted.draws, 4, "the toast adds a pill and a text draw");
    assert_eq!(toasted.total, 121);
    assert!(
        toasted.total > plain.total * 2,
        "toast frame ({}) versus plain frame ({})",
        toasted.total,
        plain.total
    );
}

/// Subtask 124.C2: the toast atlas must not replay its rasterisations
/// either.
///
/// The single-frame assertion above could not see this defect and did not
/// change when it was fixed. `sync_atlas_to_texture` takes the full-upload
/// branch on the frame that first rasterises the toast's glyphs, so the
/// stale-dirty-rect cost lands entirely on **frame 2** — which a one-frame
/// workload never reaches. That is exactly how the duplicated copy in
/// `toast_text_pass` survived 124.9.
///
/// Measured across the fix: frame 2's uploads went from 14 to 8 against a
/// steady-state 7, and the three-frame total from 370 calls to 358.
///
/// The residual one upload above steady state is the decoration buffer's
/// second double-buffer slot being sized for the first time, which is
/// subtask 124.7's expected and permanent warm-up cost, not a replay.
#[test]
fn a_toast_does_not_replay_its_glyph_uploads_on_the_second_frame() {
    let toast = standard().with_toast(ToastPresence::Present);
    let one = Metrics::of(&record_steady_state(&toast, 1).expect("toast frame 1"));
    let two = Metrics::of(&record_steady_state(&toast, 2).expect("toast frames 1-2"));
    let three = Metrics::of(&record_steady_state(&toast, 3).expect("toast frames 1-3"));

    let frame_two_uploads = two.uploads - one.uploads;
    let frame_three_uploads = three.uploads - two.uploads;

    assert_eq!(
        frame_two_uploads,
        frame_three_uploads + 1,
        "frame 2 must cost steady state plus only the decoration buffer's \
         one-off sizing orphan ({frame_two_uploads} versus \
         {frame_three_uploads}); anything more is the toast atlas replaying \
         glyphs its full upload already covered"
    );
}

#[test]
fn cursor_only_saves_bandwidth_not_call_count() {
    // Stands in for: the `is_cursor_only` fast path in the terminal
    // `PaintCallback` (`widget.rs`), i.e.
    // `TerminalRenderer::draw_with_cursor_only_update`. Compared against
    // the full-rebuild path over the same number of settled frames.
    //
    // The finding this pins is that the "fast path" is a *bandwidth*
    // optimisation, not a call-count one. It issues the same two draw
    // calls and a comparable number of GL entry points; what it avoids is
    // re-uploading the background and foreground instance buffers. Anyone
    // reasoning about it as "the cheap path" in call-count terms is
    // mistaken, and 123.14 reports it that way.
    let full = record_steady_state(&standard(), 3).expect("full frames");
    let cursor = record_cursor_only(&standard(), 3).expect("cursor-only frames");

    assert_eq!(
        Metrics::of(&full).draws,
        6,
        "three full frames, two draws each"
    );
    assert_eq!(
        Metrics::of(&cursor).draws,
        6,
        "cursor-only issues the same draws as a full rebuild"
    );
    assert!(
        uploaded_bytes(&cursor) * 10 < uploaded_bytes(&full),
        "cursor-only should move at least an order of magnitude fewer bytes \
         ({} vs {})",
        uploaded_bytes(&cursor),
        uploaded_bytes(&full)
    );
}

/// Subtask 124.7: the decoration buffer stops orphaning once its slot is
/// already big enough for a small payload.
///
/// Stands in for: the idle cursor-blink frame, which re-uploads only
/// `deco_verts` and whose floor is the cursor quad alone
/// (`CURSOR_QUAD_FLOATS` = 36 floats = 144 bytes).
///
/// The honest size of this win is asserted here rather than described:
/// **one zero-byte GL call per gated upload, and no bytes at all.** Per
/// 123.14's correction the cost model is bandwidth, not call count, so this
/// is deliberately not presented as a bandwidth improvement. The first
/// upload into each of the two double-buffer slots still orphans, because
/// it is what sizes the storage; only the reuses are gated.
#[test]
fn a_small_decoration_payload_reuses_its_allocation_instead_of_orphaning() {
    let cursor = record_cursor_only(&standard(), 3).expect("cursor-only frames");

    let orphans = cursor
        .iter()
        .filter(|c| c.method == "buffer_data_size")
        .count();
    let writes = cursor
        .iter()
        .filter(|c| c.method == "buffer_sub_data_u8_slice")
        .count();

    assert_eq!(
        writes, 3,
        "each of the three cursor-only frames still writes its decoration \
         payload; the gate removes the orphan, never the write"
    );
    assert!(
        orphans < writes,
        "at least one cursor-only frame must reuse its slot's existing \
         allocation, got {orphans} orphans for {writes} writes"
    );
}

#[test]
fn a_full_atlas_upload_consumes_the_dirty_rects_it_already_covered() {
    // REGRESSION GUARD FOR 124.9 — see `PLAN_124_RENDER_EFFICIENCY.md`.
    // This test was written by Task 123 to pin the *defect*, and inverted
    // (not deleted) by 124.9, which is the only guard this behaviour has.
    //
    // Stands in for: the first two frames after a GL context is created,
    // and after any later event that sets the atlas's `full_reupload` flag
    // (atlas growth, a font change, `RenderState::clear_atlas`).
    //
    // `TerminalRenderer::sync_atlas` takes the full-reupload branch on the
    // first frame and uploads the whole atlas with one `tex_image_2d`. That
    // upload covers every pixel, so the rects queued before it are redundant
    // by construction and are now dropped there. The *second* frame must
    // therefore look like steady state, not like a per-glyph replay of the
    // first frame's rasterisations.
    let one = Metrics::of(&record_steady_state(&standard(), 1).expect("frame 1"));
    let two = Metrics::of(&record_steady_state(&standard(), 2).expect("frames 1-2"));
    let three = Metrics::of(&record_steady_state(&standard(), 3).expect("frames 1-3"));

    let frame_two_uploads = two.uploads - one.uploads;
    let frame_three_uploads = three.uploads - two.uploads;

    // Exactly one more than steady state, and the one is nameable: subtask
    // 124.7 gates the decoration buffer's orphan on the slot already being
    // large enough, and `deco_vbo` is double-buffered, so frames 1 and 2
    // each pay a one-off sizing orphan for their own slot and frame 3 is the
    // first to reuse one. That is a known interaction with a known cause,
    // not a tolerance widened to make a red test green -- before 124.9 this
    // difference was 30 versus 4.
    assert_eq!(
        frame_two_uploads,
        frame_three_uploads + 1,
        "frame 2 must cost steady state plus only the second decoration \
         slot's one-off sizing orphan; the frame-1 full atlas upload already \
         covered every queued glyph ({frame_two_uploads} versus \
         {frame_three_uploads})"
    );

    // The paired control: frame 1 genuinely does more work than frame 2, so
    // the assertion above is not the degenerate "every frame is identical".
    assert!(
        one.total > frame_two_uploads,
        "frame 1 should still carry the one-off full-upload and setup cost \
         ({} total calls) that later frames do not",
        one.total
    );
    assert!(
        two.total < one.total * 2,
        "frame 2 must no longer roughly double the running GL call count \
         ({} for frames 1-2 versus {} for frame 1 alone)",
        two.total,
        one.total
    );
}
