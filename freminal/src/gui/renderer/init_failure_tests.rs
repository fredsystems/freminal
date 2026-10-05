// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Task 125.C16: a failed renderer `init` must not leak GL objects.
//!
//! Every test drives a real renderer through the recording facade with a
//! fault armed ([`RecordingState::fail_nth_call`]) and then checks the one
//! invariant that matters: for every GL object type, the number of objects
//! actually created equals the number deleted, and the renderer reports
//! holding none.
//!
//! The sweep is exhaustive rather than hand-picked: a clean `init` measures
//! how many calls of each fallible method it makes, and the test then fails
//! each of those calls in turn. Adding a pass or an object to a renderer
//! therefore extends the coverage with no change here.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::errors::GpuInitError;
use super::gl_facade::Gl;
use super::gl_facade::recording::RecordingState;
use super::{TerminalRenderer, ToastRenderer, ToastTextRenderer, WindowPostRenderer};

/// The fallible facade methods an `init` can hit.
const FALLIBLE_METHODS: [&str; 7] = [
    "create_buffer",
    "create_vertex_array",
    "create_texture",
    "create_program",
    "create_shader",
    "get_shader_compile_status",
    "get_program_link_status",
];

/// `(create method, delete method)` for every GL object type the renderers
/// can create.
const OBJECT_TYPES: [(&str, &str); 6] = [
    ("create_buffer", "delete_buffer"),
    ("create_shader", "delete_shader"),
    ("create_program", "delete_program"),
    ("create_vertex_array", "delete_vertex_array"),
    ("create_texture", "delete_texture"),
    ("create_framebuffer", "delete_framebuffer"),
];

/// What the sweep needs from a renderer.
trait GlOwner: Default {
    fn init(&mut self, gl: &Gl<'_>) -> Result<(), GpuInitError>;
    fn destroy(&mut self, gl: &Gl<'_>);
    fn holds_gl_objects(&self) -> bool;
    fn initialized(&self) -> bool;
    fn should_attempt_init(&self) -> bool;
}

macro_rules! impl_gl_owner {
    ($ty:ty) => {
        impl GlOwner for $ty {
            fn init(&mut self, gl: &Gl<'_>) -> Result<(), GpuInitError> {
                <$ty>::init(self, gl)
            }
            fn destroy(&mut self, gl: &Gl<'_>) {
                <$ty>::destroy(self, gl);
            }
            fn holds_gl_objects(&self) -> bool {
                <$ty>::holds_gl_objects(self)
            }
            fn initialized(&self) -> bool {
                <$ty>::initialized(self)
            }
            fn should_attempt_init(&self) -> bool {
                <$ty>::should_attempt_init(self)
            }
        }
    };
}

impl_gl_owner!(TerminalRenderer);
impl_gl_owner!(WindowPostRenderer);
impl_gl_owner!(ToastRenderer);
impl_gl_owner!(ToastTextRenderer);

const fn state<'a>(gl: &'a Gl<'_>) -> &'a RecordingState {
    gl.recorded().expect("a recording Gl always has state")
}

/// Assert every object type has as many deletes as successful creates.
fn assert_no_net_leak(gl: &Gl<'_>, context: &str) {
    let state = state(gl);
    for (create, delete) in OBJECT_TYPES {
        assert_eq!(
            state.succeeded_count_of(create),
            state.count_of(delete),
            "{context}: {create} / {delete} must balance"
        );
    }
}

/// How many calls of each fallible method a clean `init` makes.
fn clean_init_call_counts<T: GlOwner>() -> Vec<(&'static str, usize)> {
    let gl = Gl::recording();
    let mut renderer = T::default();
    renderer.init(&gl).expect("a clean recording init succeeds");
    FALLIBLE_METHODS
        .iter()
        .map(|&m| (m, state(&gl).count_of(m)))
        .collect()
}

/// Fail each fallible call of `init` in turn and check nothing leaks.
fn sweep_every_failure_point<T: GlOwner>() {
    let mut points = 0usize;
    for (method, calls) in clean_init_call_counts::<T>() {
        for nth in 0..calls {
            points += 1;
            let context = format!("{method} #{nth}");
            let gl = Gl::recording();
            state(&gl).fail_nth_call(method, nth);
            let mut renderer = T::default();

            assert!(
                renderer.init(&gl).is_err(),
                "{context}: the armed fault must fail init"
            );

            assert_no_net_leak(&gl, &context);
            assert!(
                !renderer.holds_gl_objects(),
                "{context}: a failed init must hold nothing"
            );
            assert!(!renderer.initialized(), "{context}: not ready");
            assert!(
                !renderer.should_attempt_init(),
                "{context}: the failure is latched, not retried per frame"
            );
        }
    }
    assert!(points > 0, "the sweep must exercise at least one point");
}

/// After a failed init, an explicit retry succeeds and the whole lifetime
/// balances once destroyed.
fn retry_after_failure_leaves_no_net_leak<T: GlOwner>() {
    for (method, calls) in clean_init_call_counts::<T>() {
        for nth in 0..calls {
            let context = format!("{method} #{nth}");
            let gl = Gl::recording();
            state(&gl).fail_nth_call(method, nth);
            let mut renderer = T::default();
            assert!(renderer.init(&gl).is_err(), "{context}: first init fails");

            renderer.init(&gl).expect("the retry has no fault armed");
            assert!(renderer.initialized(), "{context}: retry succeeded");
            assert!(renderer.holds_gl_objects());

            renderer.destroy(&gl);
            assert!(!renderer.holds_gl_objects(), "{context}: destroyed");
            assert!(
                renderer.should_attempt_init(),
                "{context}: destroy clears the latch"
            );
            assert_no_net_leak(&gl, &context);
        }
    }
}

macro_rules! init_failure_tests {
    ($module:ident, $ty:ty) => {
        mod $module {
            use super::*;

            #[test]
            fn a_failed_init_releases_everything_at_every_failure_point() {
                sweep_every_failure_point::<$ty>();
            }

            #[test]
            fn a_retry_after_failure_leaves_no_net_leak() {
                retry_after_failure_leaves_no_net_leak::<$ty>();
            }

            #[test]
            fn destroying_a_clean_renderer_issues_no_gl_call() {
                let gl = Gl::recording();
                let mut renderer = <$ty>::default();
                renderer.destroy(&gl);
                renderer.destroy(&gl);
                assert!(
                    state(&gl).is_empty(),
                    "nothing was created, nothing deleted"
                );
            }

            #[test]
            fn destroy_is_idempotent_after_a_successful_init() {
                let gl = Gl::recording();
                let mut renderer = <$ty>::default();
                renderer.init(&gl).expect("clean init");
                renderer.destroy(&gl);
                let calls = state(&gl).len();
                renderer.destroy(&gl);
                assert_eq!(state(&gl).len(), calls, "second destroy is a no-op");
                assert_no_net_leak(&gl, "clean lifetime");
            }
        }
    };
}

init_failure_tests!(terminal_renderer, TerminalRenderer);
init_failure_tests!(window_post_renderer, WindowPostRenderer);
init_failure_tests!(toast_renderer, ToastRenderer);
init_failure_tests!(toast_text_renderer, ToastTextRenderer);

#[test]
fn a_fragment_compile_failure_deletes_the_vertex_shader() {
    // The first program's fragment shader is the second shader compiled, so
    // its status check is the second `get_shader_compile_status` call.
    let gl = Gl::recording();
    state(&gl).fail_nth_call("get_shader_compile_status", 1);
    let mut renderer = ToastRenderer::default();

    assert!(renderer.init(&gl).is_err());

    let state = state(&gl);
    assert_eq!(state.succeeded_count_of("create_shader"), 2);
    assert_eq!(
        state.count_of("delete_shader"),
        2,
        "both the failed fragment shader and the vertex shader are deleted"
    );
    assert_eq!(state.count_of("create_program"), 0, "never reached linking");
}

#[test]
fn a_create_program_failure_deletes_both_compiled_shaders() {
    let gl = Gl::recording();
    state(&gl).fail_nth_call("create_program", 0);
    let mut renderer = ToastRenderer::default();

    assert!(renderer.init(&gl).is_err());

    let state = state(&gl);
    assert_eq!(state.succeeded_count_of("create_shader"), 2);
    assert_eq!(state.count_of("delete_shader"), 2);
    assert_eq!(state.succeeded_count_of("create_program"), 0);
    assert_eq!(state.count_of("delete_program"), 0);
}

#[test]
fn a_failed_init_is_latched_until_destroy() {
    let gl = Gl::recording();
    state(&gl).fail_nth_call("create_buffer", 0);
    let mut renderer = TerminalRenderer::default();
    assert!(
        renderer.should_attempt_init(),
        "fresh renderer attempts init"
    );

    assert!(renderer.init(&gl).is_err());

    assert!(!renderer.should_attempt_init(), "latched");
    renderer.destroy(&gl);
    assert!(renderer.should_attempt_init(), "destroy clears the latch");
}

#[test]
fn retiring_a_renderer_whose_init_failed_queues_nothing() {
    use super::GlRetireQueue;

    let gl = Gl::recording();
    state(&gl).fail_nth_call("create_vertex_array", 2);
    let mut renderer = TerminalRenderer::default();
    assert!(renderer.init(&gl).is_err());

    let queue = GlRetireQueue::new();
    queue.retire(renderer);

    assert!(
        queue.is_empty(),
        "it holds nothing, so there is nothing to delete"
    );
    assert_no_net_leak(&gl, "failed init then retire");
}

#[test]
fn window_post_destroy_forgets_the_cached_fbo_size() {
    let gl = Gl::recording();
    let mut wpr = WindowPostRenderer::default();
    wpr.init(&gl).expect("clean init");
    wpr.ensure_fbo(&gl, 64, 32);
    assert_eq!(state(&gl).succeeded_count_of("create_framebuffer"), 1);

    wpr.destroy(&gl);
    assert!(!wpr.holds_gl_objects());
    assert_no_net_leak(&gl, "after destroy");

    // The same dimensions must recreate the FBO, not trust the stale cache.
    wpr.ensure_fbo(&gl, 64, 32);
    assert_eq!(state(&gl).succeeded_count_of("create_framebuffer"), 2);
    assert!(wpr.holds_gl_objects());
}
