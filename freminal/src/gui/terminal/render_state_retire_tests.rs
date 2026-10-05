// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! `RenderState`'s drop-time GL retirement (Task 125.C2).
//!
//! These drive the real `new_render_state` / `Drop` path against the
//! `gl-recording` facade (no GL context needed), proving that discarding a
//! pane's render state hands its renderer to the window's retire queue
//! exactly once, never early, and that draining deletes its GL objects.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Arc, Mutex};

use super::new_render_state;
use crate::gui::renderer::WindowPostRenderer;
use crate::gui::renderer::gl_facade::Gl;

/// A window's shared post-renderer, standing in for `PerWindowState::window_post`.
fn window_post() -> Arc<Mutex<WindowPostRenderer>> {
    Arc::new(Mutex::new(WindowPostRenderer::new()))
}

/// Initialize the renderer inside `rs` against `gl`, as the first paint
/// callback would.
fn init_renderer(rs: &Arc<Mutex<super::RenderState>>, gl: &Gl<'_>) {
    rs.lock()
        .unwrap()
        .renderer
        .init(gl)
        .expect("recording init succeeds");
}

fn count(gl: &Gl<'_>, method: &str) -> usize {
    gl.recorded()
        .unwrap()
        .calls()
        .iter()
        .filter(|c| c.method == method)
        .count()
}

#[test]
fn dropping_a_painted_pane_queues_its_renderer_and_drain_deletes_it() {
    let gl = Gl::recording();
    let post = window_post();
    let queue = post.lock().unwrap().retire_queue();

    let rs = new_render_state(Arc::clone(&post));
    init_renderer(&rs, &gl);
    assert!(queue.is_empty(), "a live pane has nothing retired");

    drop(rs); // pane closed

    assert_eq!(queue.len(), 1, "closing the pane retired its renderer");
    let programs = count(&gl, "create_program");
    assert_eq!(
        count(&gl, "delete_program"),
        0,
        "retire itself issues no GL"
    );

    assert_eq!(post.lock().unwrap().drain_retired(&gl), 1);
    assert_eq!(count(&gl, "delete_program"), programs);
    assert_eq!(
        count(&gl, "delete_vertex_array"),
        count(&gl, "create_vertex_array")
    );
    assert_eq!(count(&gl, "delete_buffer"), count(&gl, "create_buffer"));
    assert_eq!(count(&gl, "delete_texture"), count(&gl, "create_texture"));

    // Idempotent: nothing left, nothing destroyed twice.
    assert_eq!(post.lock().unwrap().drain_retired(&gl), 0);
    assert_eq!(count(&gl, "delete_program"), programs);
}

#[test]
fn dropping_a_never_painted_pane_retires_nothing() {
    let post = window_post();
    let queue = post.lock().unwrap().retire_queue();

    drop(new_render_state(Arc::clone(&post)));

    assert!(
        queue.is_empty(),
        "an uninitialized renderer owns no GL objects"
    );
}

#[test]
fn renderer_is_not_retired_while_a_paint_callback_still_holds_the_state() {
    let gl = Gl::recording();
    let post = window_post();
    let queue = post.lock().unwrap().retire_queue();

    let rs = new_render_state(Arc::clone(&post));
    init_renderer(&rs, &gl);
    // The paint callback captures a clone of the Arc.
    let in_flight_callback = Arc::clone(&rs);

    drop(rs); // the pane is closed mid-frame ...
    assert!(
        queue.is_empty(),
        "... but the in-flight callback may still draw through it"
    );
    assert_eq!(post.lock().unwrap().drain_retired(&gl), 0);
    assert_eq!(count(&gl, "delete_program"), 0);

    drop(in_flight_callback); // egui releases the callback after painting
    assert_eq!(queue.len(), 1, "now it is retired, exactly once");
}

#[test]
fn every_pane_of_a_window_retires_into_the_same_queue() {
    let gl = Gl::recording();
    let post = window_post();
    let queue = post.lock().unwrap().retire_queue();

    let panes: Vec<_> = (0..3)
        .map(|_| {
            let rs = new_render_state(Arc::clone(&post));
            init_renderer(&rs, &gl);
            rs
        })
        .collect();
    drop(panes);

    assert_eq!(queue.len(), 3);
    assert_eq!(post.lock().unwrap().drain_retired(&gl), 3);
    assert!(queue.is_empty());
}

#[test]
fn a_second_windows_panes_do_not_land_in_the_first_windows_queue() {
    let gl = Gl::recording();
    let window_a = window_post();
    let window_b = window_post();
    let queue_a = window_a.lock().unwrap().retire_queue();
    let queue_b = window_b.lock().unwrap().retire_queue();

    let rs = new_render_state(Arc::clone(&window_b));
    init_renderer(&rs, &gl);
    drop(rs);

    assert!(
        queue_a.is_empty(),
        "each window's context owns its own queue"
    );
    assert_eq!(queue_b.len(), 1);
}

#[test]
fn a_retirement_after_update_returned_wakes_the_window() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let gl = Gl::recording();
    let post = window_post();
    let queue = post.lock().unwrap().retire_queue();
    let wakes = Arc::new(AtomicUsize::new(0));
    {
        let wakes = Arc::clone(&wakes);
        queue.set_repaint_wake(Arc::new(move || {
            wakes.fetch_add(1, Ordering::SeqCst);
        }));
    }

    let rs = new_render_state(Arc::clone(&post));
    init_renderer(&rs, &gl);
    let in_flight_callback = Arc::clone(&rs);

    // `update()` closes the pane and then checks the queue: still empty,
    // because the paint callback of this very frame holds the state.
    drop(rs);
    assert!(queue.is_empty());
    assert_eq!(wakes.load(Ordering::SeqCst), 0, "not retired yet: no wake");

    // The windowing layer releases the paint jobs after `update` returned.
    // This is the retirement `update` could not see; it must request a frame.
    drop(in_flight_callback);
    assert_eq!(queue.len(), 1);
    assert_eq!(
        wakes.load(Ordering::SeqCst),
        1,
        "the late retirement must ask the idle window for a draining frame"
    );
}

#[test]
fn a_pane_created_before_the_wake_was_installed_still_wakes_the_window() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let gl = Gl::recording();
    let post = window_post();
    let rs = new_render_state(Arc::clone(&post)); // clones the queue first
    init_renderer(&rs, &gl);

    let wakes = Arc::new(AtomicUsize::new(0));
    {
        let wakes = Arc::clone(&wakes);
        post.lock()
            .unwrap()
            .retire_queue()
            .set_repaint_wake(Arc::new(move || {
                wakes.fetch_add(1, Ordering::SeqCst);
            }));
    }

    drop(rs);
    assert_eq!(wakes.load(Ordering::SeqCst), 1);
}
