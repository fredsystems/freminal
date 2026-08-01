// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Pane-resolution benchmarks (Task 122, subtask 122.14).
//!
//! Two distinct call sites are covered, for one reason: subtask 122.3
//! re-types **all five** of these functions onto toolkit-neutral geometry
//! (`freminal_common::geometry::{Rect, Point}`, migrated from `egui::Rect`/
//! `Pos2` by this subtask), so this file is 122.3's performance gate. Any
//! post-122.3 run must not regress more than the 15% threshold in
//! `performance-benchmarks`.
//!
//! **The pointer-motion chain** (`app_impl.rs:905-990`), which runs on every
//! `CursorMoved` event outside any frame, on the `freminal-windowing` fast
//! path, and is the dominant cost of the `pointer_motion_needs_repaint`
//! predicate. It resolves "which pane is the pointer over" by walking the
//! active tab's `PaneTree`: [`PaneTree::layout`] produces the per-pane
//! rects, an inline linear rect-containment scan (`app_impl.rs:930-933`,
//! the same work [`pane_at_pos`] does — it is inlined there only so the
//! pane's rect is returned alongside its id) finds the hit pane, and
//! [`PaneTree::find`] resolves that id back to a `&Pane`. The private O(1)
//! predicates that consume the result are the cheap tail of this chain, not
//! its cost.
//!
//! **The frame-path divider geometry**, which is a *different* call site:
//! [`PaneTree::split_borders`] (`app_impl.rs:2344`, `3552`) and
//! [`active_highlight_segment`] (`app_impl.rs:3611`) run inside
//! `central_body`. They are NOT part of the pointer-motion chain. Both are
//! gated on `has_multiple_panes && zoomed_pane.is_none()`
//! (`app_impl.rs:2339`, `3515`), and the drag-sensor call at `2344`
//! additionally on `!ui_overlay_open` — so they run every frame of a
//! multi-pane tab that is neither zoomed nor covered by an overlay, not
//! unconditionally.
//!
//! Deliberately **excluded**: the free functions
//! `pointer_motion_needs_repaint_decision`, `pane_hover_region_risk`,
//! `animation_in_flight_composed`, and `pointer_in_gutter_strip` in
//! `app_impl.rs`. All four are private (not reachable from an external bench
//! crate without widening `mod app_impl`'s visibility, which subtask
//! 122.14's scope forbids) and are O(1) boolean compositions over
//! already-resolved flags — a wall-clock benchmark of them would measure
//! Criterion's own harness overhead, not freminal. They are covered by 22
//! unit tests in `app_impl.rs:4805-5015` instead (9 + 5 + 4 + 4 in the order
//! listed above). The five functions benchmarked here are the ones that
//! actually walk the tree or scan geometry.
//!
//! ## Tree shape
//!
//! Pane counts 1, 2, 4, 8, and 16 are benchmarked against a **balanced**
//! binary tree (each round splits every current leaf, so depth grows as
//! `log2(pane_count)`) — this is the shape produced by the "split evenly"
//! interactive workflow most users actually build. A **chain**-shaped
//! (right-leaning, depth `pane_count - 1`) 16-pane case is added for
//! `layout` and `find` so the baseline records the second shape too. It is
//! the degenerate case for `layout`, which recurses through every internal
//! node; it is *not* expected to be worse for `find` — see
//! [`build_chain_tree`] for why, and the recorded baseline for what was
//! actually measured.
//!
//! `split_borders` is parameterised on a different axis instead — which pane
//! is active, which determines how much failed subtree searching it does. See
//! [`bench_split_borders`].
//!
//! ## Headless `Pane` construction
//!
//! Benches are an external crate, so the in-crate `dummy_pane` test helper
//! (`panes/mod.rs`) — a struct literal touching `pub(crate)` fields — is
//! unavailable. [`make_bench_pane`] instead uses the public
//! `Pane::from_channels` constructor with hand-built, disconnected
//! `TabChannels`. This is safe because none of the five functions
//! benchmarked in this file send or receive on any channel — they only walk
//! the pane tree and compute geometry — so a channel whose counterpart
//! endpoint has already been dropped is indistinguishable, for this
//! purpose, from a live one. `WindowPostRenderer::new()`
//! (`renderer/gpu.rs:1867`) allocates no GPU resources — its own doc says
//! they are created lazily on the first `init()` call, which this file never
//! makes — so no GL context is required.

use std::hint::black_box;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use arc_swap::ArcSwap;
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use freminal::gui::panes::{
    ActiveSubtree, Pane, PaneId, PaneIdGenerator, PaneTree, SplitBorder, SplitDirection,
    active_highlight_segment, pane_at_pos,
};
use freminal::gui::pty::{CommandFinishedEvent, TabChannels};
use freminal::gui::renderer::WindowPostRenderer;
use freminal::gui::shell_history::new_seeded_history;
use freminal_common::buffer_states::tchar::TChar;
use freminal_common::geometry::{Rect, point};
use freminal_common::pty_write::PtyWrite;
use freminal_terminal_emulator::io::{InputEvent, WindowCommand};
use freminal_terminal_emulator::snapshot::TerminalSnapshot;

/// Pane counts benchmarked for every pane-count-parameterised case.
const PANE_COUNTS: [usize; 5] = [1, 2, 4, 8, 16];

/// A realistic full-window rect, matching a common 1080p central-rect size.
const WINDOW_WIDTH: f32 = 1920.0;
const WINDOW_HEIGHT: f32 = 1080.0;

fn configure() -> Criterion {
    Criterion::default()
        .sample_size(50)
        .warm_up_time(Duration::from_millis(300))
        .measurement_time(Duration::from_secs(2))
        .with_plots()
}

// ---------------------------------------------------------------
// Headless Pane construction
// ---------------------------------------------------------------

/// Build a `Pane` with disconnected channels and a lazily-initialised
/// `WindowPostRenderer`, suitable for tree-shape benchmarks that never touch
/// PTY I/O or GPU resources. See the module doc for why dropping the
/// counterpart endpoints is safe here.
fn make_bench_pane(id: PaneId) -> Pane {
    let arc_swap: Arc<ArcSwap<TerminalSnapshot>> =
        Arc::new(ArcSwap::from_pointee(TerminalSnapshot::empty()));
    let (input_tx, _input_rx) = crossbeam_channel::unbounded::<InputEvent>();
    let (pty_write_tx, _pty_write_rx) = crossbeam_channel::unbounded::<PtyWrite>();
    let (_window_cmd_tx, window_cmd_rx) = crossbeam_channel::unbounded::<WindowCommand>();
    let (_clipboard_tx, clipboard_rx) = crossbeam_channel::unbounded::<String>();
    let (_search_buffer_tx, search_buffer_rx) =
        crossbeam_channel::unbounded::<(usize, Vec<TChar>)>();
    let (_pty_dead_tx, pty_dead_rx) = crossbeam_channel::unbounded::<()>();
    let (_command_event_tx, command_event_rx) =
        crossbeam_channel::unbounded::<CommandFinishedEvent>();

    let channels = TabChannels {
        arc_swap,
        input_tx,
        pty_write_tx,
        window_cmd_rx,
        clipboard_rx,
        search_buffer_rx,
        pty_dead_rx,
        command_event_rx,
        echo_off: Arc::new(AtomicBool::new(false)),
        child_pid: None,
        history_seed: new_seeded_history(),
        shell_program: None,
    };

    let window_post = Arc::new(Mutex::new(WindowPostRenderer::new()));
    Pane::from_channels(id, channels, window_post, "bench-pane".to_owned())
}

// ---------------------------------------------------------------
// Tree builders (setup only; never timed)
// ---------------------------------------------------------------

/// Build a balanced `PaneTree` with exactly `pane_count` leaves (must be a
/// power of two, matching [`PANE_COUNTS`]).
///
/// Each round splits every leaf currently in the tree, alternating split
/// direction per round, so depth grows as `log2(pane_count)` rather than
/// linearly. Returns the tree and the final leaf ids in creation order.
fn build_balanced_tree(pane_count: usize) -> (PaneTree, Vec<PaneId>) {
    let mut id_gen = PaneIdGenerator::new(0);
    let first_id = id_gen.next_id();
    let mut tree = PaneTree::new(make_bench_pane(first_id));
    let mut leaves = vec![first_id];
    let mut direction = SplitDirection::Horizontal;

    while leaves.len() < pane_count {
        let mut next_leaves = Vec::with_capacity(leaves.len() * 2);
        for leaf_id in leaves {
            let new_id = tree
                .split(leaf_id, direction, &mut id_gen, make_bench_pane)
                .unwrap_or_else(|err| {
                    unreachable!("leaf_id was just read from the tree being built: {err:?}")
                });
            next_leaves.push(leaf_id);
            next_leaves.push(new_id);
        }
        leaves = next_leaves;
        direction = match direction {
            SplitDirection::Horizontal => SplitDirection::Vertical,
            SplitDirection::Vertical => SplitDirection::Horizontal,
        };
    }

    (tree, leaves)
}

/// Build a chain-shaped (right-leaning) `PaneTree` with exactly `pane_count`
/// leaves: every split targets the most-recently-created pane, so depth
/// grows linearly with `pane_count` instead of logarithmically.
///
/// This is the degenerate shape for [`PaneTree::layout`], which recurses
/// through every internal node. It is **not** expected to be meaningfully
/// worse for [`PaneTree::find`] of the last leaf: a full depth-first search
/// visits the same `n` leaves and `n-1` internal nodes under either shape,
/// only in a different order. Both shapes are measured for both functions so
/// that the recorded baseline (see the "122.14 recorded baseline" section of
/// `Documents/PLAN_122_ORCHESTRATION_EXTRACTION.md`) states the difference
/// rather than leaving it to be assumed.
fn build_chain_tree(pane_count: usize) -> (PaneTree, Vec<PaneId>) {
    let mut id_gen = PaneIdGenerator::new(0);
    let first_id = id_gen.next_id();
    let mut tree = PaneTree::new(make_bench_pane(first_id));
    let mut leaves = vec![first_id];
    let mut current = first_id;
    let mut direction = SplitDirection::Horizontal;

    while leaves.len() < pane_count {
        let new_id = tree
            .split(current, direction, &mut id_gen, make_bench_pane)
            .unwrap_or_else(|err| {
                unreachable!("current was just read from the tree being built: {err:?}")
            });
        leaves.push(new_id);
        current = new_id;
        direction = match direction {
            SplitDirection::Horizontal => SplitDirection::Vertical,
            SplitDirection::Vertical => SplitDirection::Horizontal,
        };
    }

    (tree, leaves)
}

/// Build a synthetic `(PaneId, Rect)` layout of `pane_count` equal-width
/// vertical strips spanning [`WINDOW_WIDTH`]x[`WINDOW_HEIGHT`], without
/// constructing a `PaneTree` at all — `pane_at_pos` only needs the slice.
fn synthetic_layout(pane_count: usize) -> Vec<(PaneId, Rect)> {
    let mut id_gen = PaneIdGenerator::new(0);
    let pane_width = WINDOW_WIDTH / pane_count as f32;
    (0..pane_count)
        .map(|i| {
            let id = id_gen.next_id();
            let x0 = i as f32 * pane_width;
            let rect = Rect::from_min_max(point(x0, 0.0), point(x0 + pane_width, WINDOW_HEIGHT));
            (id, rect)
        })
        .collect()
}

// ---------------------------------------------------------------
// bench_layout
// ---------------------------------------------------------------
fn bench_layout(c: &mut Criterion) {
    let rect = Rect::from_min_max(point(0.0, 0.0), point(WINDOW_WIDTH, WINDOW_HEIGHT));
    let mut group = c.benchmark_group("layout");

    for &count in &PANE_COUNTS {
        let (tree, _leaves) = build_balanced_tree(count);
        group.bench_function(BenchmarkId::new("balanced", count), |b| {
            b.iter(|| {
                let result = tree.layout(black_box(rect));
                black_box(result.unwrap_or_default());
            });
        });
    }

    // Degenerate chain shape, 16 panes: records that shape (not just count)
    // affects traversal cost.
    let (chain_tree, _chain_leaves) = build_chain_tree(16);
    group.bench_function(BenchmarkId::new("chain", 16), |b| {
        b.iter(|| {
            let result = chain_tree.layout(black_box(rect));
            black_box(result.unwrap_or_default());
        });
    });

    group.finish();
}

// ---------------------------------------------------------------
// bench_split_borders
// ---------------------------------------------------------------
fn bench_split_borders(c: &mut Criterion) {
    let rect = Rect::from_min_max(point(0.0, 0.0), point(WINDOW_WIDTH, WINDOW_HEIGHT));
    let mut group = c.benchmark_group("split_borders");

    for &count in &PANE_COUNTS {
        let (tree, leaves) = build_balanced_tree(count);

        // `active_pane` is not just a passenger: `PaneNode::split_borders`
        // computes each border's `active_subtree` by calling
        // `first.contains(active_pane)` before `second.contains(active_pane)`
        // at every internal node, and a failed `contains` walks that whole
        // subtree before falling back. So the choice of active pane changes
        // how much wasted searching happens, and both ends are recorded:
        //
        //   `active_first` — the tree's original pane. `PaneNode::split`
        //     makes the existing pane the FIRST child, so pane 0 sits on the
        //     all-`first` spine and every ancestor's `first.contains` hits
        //     immediately. This is the cheapest possible input.
        //   `active_last`  — the last-created leaf, which is a `second`
        //     child, so at least one ancestor pays a failed exhaustive scan
        //     of its `first` subtree first. Representative of a user who
        //     just split the pane they were working in.
        let active_first = *leaves.first().unwrap_or(&PaneId::first());
        let active_last = *leaves.last().unwrap_or(&PaneId::first());

        group.bench_function(BenchmarkId::new("active_first", count), |b| {
            b.iter(|| {
                let result = tree.split_borders(black_box(rect), black_box(active_first));
                black_box(result.unwrap_or_default());
            });
        });
        group.bench_function(BenchmarkId::new("active_last", count), |b| {
            b.iter(|| {
                let result = tree.split_borders(black_box(rect), black_box(active_last));
                black_box(result.unwrap_or_default());
            });
        });
    }

    group.finish();
}

// ---------------------------------------------------------------
// bench_find
// ---------------------------------------------------------------
fn bench_find(c: &mut Criterion) {
    let mut group = c.benchmark_group("find");

    for &count in &PANE_COUNTS {
        let (tree, leaves) = build_balanced_tree(count);
        // Worst case: the last-inserted pane id.
        let target = *leaves.last().unwrap_or(&PaneId::first());
        group.bench_function(BenchmarkId::new("balanced", count), |b| {
            b.iter(|| {
                let result = tree.find(black_box(target));
                black_box(result);
            });
        });
    }

    // Degenerate chain shape, 16 panes.
    let (chain_tree, chain_leaves) = build_chain_tree(16);
    let chain_target = *chain_leaves.last().unwrap_or(&PaneId::first());
    group.bench_function(BenchmarkId::new("chain", 16), |b| {
        b.iter(|| {
            let result = chain_tree.find(black_box(chain_target));
            black_box(result);
        });
    });

    group.finish();
}

// ---------------------------------------------------------------
// bench_pane_at_pos
// ---------------------------------------------------------------
fn bench_pane_at_pos(c: &mut Criterion) {
    let mut group = c.benchmark_group("pane_at_pos");

    for &count in &PANE_COUNTS {
        let layout = synthetic_layout(count);
        let pane_width = WINDOW_WIDTH / count as f32;

        // Hit on the first pane: the linear scan matches immediately.
        let first_hit = point(pane_width / 2.0, WINDOW_HEIGHT / 2.0);
        // Hit on the last pane: worst-case scan through every prior rect
        // before matching.
        let last_hit = point(WINDOW_WIDTH - pane_width / 2.0, WINDOW_HEIGHT / 2.0);
        // Miss: full scan, no match. Negative coordinates guarantee a miss
        // regardless of `Rect::contains`'s edge inclusivity.
        let miss = point(-10.0, -10.0);

        group.bench_function(BenchmarkId::new("first_hit", count), |b| {
            b.iter(|| {
                let result = pane_at_pos(black_box(&layout), black_box(first_hit));
                black_box(result);
            });
        });
        group.bench_function(BenchmarkId::new("last_hit", count), |b| {
            b.iter(|| {
                let result = pane_at_pos(black_box(&layout), black_box(last_hit));
                black_box(result);
            });
        });
        group.bench_function(BenchmarkId::new("miss", count), |b| {
            b.iter(|| {
                let result = pane_at_pos(black_box(&layout), black_box(miss));
                black_box(result);
            });
        });
    }

    group.finish();
}

// ---------------------------------------------------------------
// bench_active_highlight_segment
// ---------------------------------------------------------------
fn bench_active_highlight_segment(c: &mut Criterion) {
    let epsilon = 0.5;

    // A vertical divider (Horizontal split) sitting exactly on the active
    // pane's left edge: the divider borders the active pane along its full
    // height, so this exercises the "compute overlap" path.
    let bordering = SplitBorder {
        direction: SplitDirection::Horizontal,
        first_child_pane: PaneId::first(),
        rect: Rect::from_min_max(point(959.0, 0.0), point(961.0, WINDOW_HEIGHT)),
        parent_extent: WINDOW_WIDTH,
        active_subtree: ActiveSubtree::First,
    };
    let bordering_active_rect = Rect::from_min_max(
        point(960.0, 0.0),
        point(960.0 + (WINDOW_WIDTH - 960.0), WINDOW_HEIGHT),
    );

    // A divider nowhere near the active pane: the fast "does not border"
    // path returns `None` immediately.
    let non_bordering = SplitBorder {
        direction: SplitDirection::Horizontal,
        first_child_pane: PaneId::first(),
        rect: Rect::from_min_max(point(99.0, 0.0), point(101.0, WINDOW_HEIGHT)),
        parent_extent: WINDOW_WIDTH,
        active_subtree: ActiveSubtree::Second,
    };
    let non_bordering_active_rect =
        Rect::from_min_max(point(500.0, 0.0), point(900.0, WINDOW_HEIGHT));

    let mut group = c.benchmark_group("active_highlight_segment");

    group.bench_function("bordering", |b| {
        b.iter(|| {
            let result = active_highlight_segment(
                black_box(&bordering),
                black_box(bordering_active_rect),
                black_box(epsilon),
            );
            black_box(result);
        });
    });

    group.bench_function("non_bordering", |b| {
        b.iter(|| {
            let result = active_highlight_segment(
                black_box(&non_bordering),
                black_box(non_bordering_active_rect),
                black_box(epsilon),
            );
            black_box(result);
        });
    });

    group.finish();
}

criterion_group!(
    name = benches;
    config = configure();
    targets =
        bench_layout,
        bench_split_borders,
        bench_find,
        bench_pane_at_pos,
        bench_active_highlight_segment,
);

criterion_main!(benches);
