// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Task 124 subtask 124.15: what does freminal's chrome cost per frame, and
//! what does the **disabled** #436 chrome cache still waste?
//!
//! # Why this bench exists
//!
//! 124.15 gates 124.5 (the chrome cache's fate). Neither Task 123 harness
//! can answer it: Phase 1 records GL calls from the terminal renderer, which
//! never sees chrome, and Phase 2 captures pixels from that same renderer.
//! Chrome is painted by egui, so it needs an egui-level measurement.
//!
//! # What is measured, and what is a stand-in
//!
//! A headless [`egui::Context`] runs a chrome-shaped UI — a top menu bar
//! with the same five menus freminal builds, a tab strip, and a central
//! panel with pane-border line segments — and the resulting shapes are
//! tessellated. That is a **representative stand-in** for
//! `App::update`'s chrome, not freminal's literal chrome: building the real
//! thing needs a live `FreminalGui`, `PerWindowState` and `WindowId`, and
//! `freminal_windowing::WindowId` has no public constructor outside the real
//! winit event loop (the same obstacle `pointer_motion.rs` documents).
//!
//! The `cache_*` benches are **not** a stand-in. They measure the exact
//! clone operations `egui_integration.rs`'s `ChromeMode::Full` arm performs
//! every frame to populate `ChromeCache` — including on the ~100% of frames
//! that take that arm because the cache is disabled by default (121.32), and
//! where nothing ever reads what they produce. That is subtask 121.35's
//! "live waste", migrated into 124.5.
//!
//! Per `PROFILING.md`, per-frame costs here are reported without an implied
//! frame rate: a cost per frame only becomes a CPU share once multiplied by
//! a rate this bench does not observe.

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};

/// Build a chrome-shaped UI into `ctx` and return this frame's shapes.
///
/// Mirrors the structure of `App::update`'s `ChromeMode::Full` arm
/// (`app_impl.rs:1663-1717`): a root `Ui`, a `Panel::top("menu_bar")`
/// carrying the menu bar, a `Panel::top("tab_bar")` carrying the tab strip,
/// and a `CentralPanel` into which the per-pane border strokes are painted
/// (`app_impl.rs:3331`). Same panel ids and same nesting, so the layout and
/// id-allocation work egui does is representative.
fn run_chrome_frame(
    ctx: &egui::Context,
    tabs: usize,
    borders: usize,
) -> Vec<egui::epaint::ClippedShape> {
    let raw_input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1280.0, 800.0),
        )),
        ..Default::default()
    };

    let full = ctx.run_ui(raw_input, |root_ui| {
        egui::Panel::top("menu_bar").show(root_ui, |ui| {
            ui.horizontal(|ui| {
                for name in ["File", "Edit", "View", "Terminal", "Help"] {
                    let _ = ui.button(name);
                }
            });
        });

        egui::Panel::top("tab_bar").show(root_ui, |ui| {
            ui.horizontal(|ui| {
                for i in 0..tabs {
                    let _ = ui.selectable_label(i == 0, format!("tab {i}"));
                }
            });
        });

        egui::CentralPanel::default().show(root_ui, |ui| {
            let painter = ui.painter();
            let stroke = egui::Stroke::new(1.0, egui::Color32::from_gray(90));
            for i in 0..borders {
                #[allow(clippy::cast_precision_loss)]
                let x = 100.0 + i as f32 * 40.0;
                painter.line_segment([egui::pos2(x, 60.0), egui::pos2(x, 780.0)], stroke);
            }
        });
    });

    full.shapes
}

fn chrome_benches(c: &mut Criterion) {
    // A warmed context: egui's first frame allocates fonts and lays out
    // every widget for the first time, so benching a cold one would measure
    // startup, not steady state.
    let ctx = egui::Context::default();
    let _ = run_chrome_frame(&ctx, 4, 3);
    let _ = run_chrome_frame(&ctx, 4, 3);

    let ppp = 1.0_f32;
    let shapes = run_chrome_frame(&ctx, 4, 3);
    let primitives = ctx.tessellate(shapes.clone(), ppp);

    let mut group = c.benchmark_group("chrome");

    // Constructing chrome widgets: what `ChromeMode::Replay` skips, and
    // therefore the ceiling on what any correct chrome cache could save.
    group.bench_function("construct_4tabs_3borders", |b| {
        b.iter(|| black_box(run_chrome_frame(&ctx, black_box(4), black_box(3))));
    });

    // Tessellating chrome: paid on every `Full` frame, and on `Replay`
    // frames only via the atlas-grew self-heal path.
    group.bench_function("tessellate", |b| {
        b.iter(|| black_box(ctx.tessellate(black_box(shapes.clone()), ppp)));
    });

    // ── The disabled cache's live waste (121.35 -> 124.5) ──
    //
    // With `chrome_cache_enabled()` false, `chrome_mode` is forced to
    // `Full`, so this arm runs every frame and populates a cache no code
    // path can ever read. Deleting the cache removes exactly these clones;
    // the `to_vec()` slice copies feeding `tessellate` are NOT removable
    // (it takes an owned `Vec`) and so are excluded here.
    group.bench_function("cache_shape_clone", |b| {
        b.iter(|| black_box(black_box(&shapes).clone()));
    });
    group.bench_function("cache_primitive_clone", |b| {
        b.iter(|| black_box(black_box(&primitives).clone()));
    });

    group.finish();
}

criterion_group!(benches, chrome_benches);
criterion_main!(benches);
