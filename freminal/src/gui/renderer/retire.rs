// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Deferred GL teardown (Task 125.C2).
//!
//! One concept: getting a renderer's GL objects deleted **while the GL
//! context that owns them is current**, when the code that stops needing the
//! renderer (a pane close, a tab close, a window close) has no GL context at
//! hand.
//!
//! # The problem
//!
//! [`TerminalRenderer::destroy`], [`WindowPostRenderer::destroy`] and the two
//! toast passes' `destroy` all need a current GL context. Dropping a pane
//! does not have one: `Drop` runs wherever the last owner happens to let go
//! (the update path, a `Vec::remove`, a whole `PerWindowState` going out of
//! scope), and the GL context is only guaranteed current inside
//! `App::update` and egui's paint callbacks. Before this module nothing
//! called any of those `destroy` methods, so every pane ever opened leaked
//! its programs, VAOs, VBOs, atlas/image textures and (under
//! `gpu-profiling`) timer queries until its whole GL context died.
//!
//! # The design
//!
//! Three pieces, each with one job:
//!
//! 1. [`GlRetireQueue`] -- a cheaply-cloneable, window-scoped handle to a
//!    list of [`TerminalRenderer`]s that have been *retired* (their owner is
//!    gone) but not yet *destroyed* (their GL objects still exist). Pushing
//!    needs no GL context; [`GlRetireQueue::drain`] needs one and deletes
//!    everything queued.
//!    The queue can also carry a *repaint wake* ([`RepaintWake`]): when a
//!    renderer is retired it pokes the window's event loop so a frame runs
//!    and drains it (see "Waking the window" below).
//! 2. `RenderState`'s `Drop` (in `gui::terminal::widget`) -- moves its
//!    [`TerminalRenderer`] into the window's queue. Because it is `Drop`, it
//!    covers **every** path that discards a pane (pane close, tab close,
//!    layout replacement, window close) without each site having to
//!    remember. Because the paint callback captures an `Arc` clone of the
//!    `RenderState`, `Drop` does not run until the callback is gone, so a
//!    renderer an in-flight paint callback is still using can never be
//!    retired early.
//! 3. [`WindowGlTeardown`] -- the handles a window needs to free its own
//!    context-lifetime GL state (the shared [`WindowPostRenderer`], its
//!    retire queue, and the toast passes) once the app has already dropped
//!    the window's `PerWindowState`.
//!
//! # Waking the window
//!
//! A retirement can happen *after* `FreminalGui::update` has finished its
//! frame: a deferred `ClosePane` drops the pane's `RenderState` during
//! `update`, but the paint callback of that same frame still holds an `Arc`
//! clone, so `Drop` (and therefore the retirement) only runs once the
//! windowing layer releases the paint jobs -- after `update` returned. The
//! queue is empty when `update` looks, and an otherwise idle window would
//! never run another frame to drain it. [`GlRetireQueue::retire`] therefore
//! calls the queue's wake itself, after the push and with the queue lock
//! released.
//!
//! The wake is built on the window's cross-thread `RepaintProxy` (the same
//! one the PTY thread uses), **not** on `egui::Context::request_repaint`:
//! `retire` runs from `Drop`, possibly while egui or the windowing layer is
//! tearing down paint data, and egui's context lock is not re-entrant. The
//! proxy only enqueues an event-loop message, takes no egui lock, and a
//! closed window's id is ignored by the event loop. The wake runs no GL.
//!
//! # Where the drains run
//!
//! - **Live window:** `FreminalGui::update` drains the window's queue at the
//!   top of every frame, where the window's GL context is current. A
//!   renderer retired by frame *N*'s paint callbacks (which drop after
//!   painting) is therefore deleted at the start of frame *N+1*.
//! - **Window close / event-loop exit:** `freminal-windowing` calls
//!   `App::on_window_destroying` with the window's GL context current,
//!   immediately before the egui painter is destroyed and the context is
//!   torn down. [`WindowGlTeardown::run`] destroys everything the window
//!   still owns there.
//!
//! # Invariants
//!
//! - **Each renderer is destroyed at most once.** `drain` takes the pending
//!   list out of the lock before destroying, so two drains never see the
//!   same renderer; and `TerminalRenderer::destroy` is itself idempotent
//!   (every handle is `take()`n, `initialized` is cleared).
//! - **A retired renderer is never drawn through again.** It is moved into
//!   the queue by value; its former owner holds a fresh, uninitialized
//!   replacement.
//! - **GL-free until drained.** Retiring never touches GL, so it is safe
//!   from `Drop`, from any code path, and in tests with no context.
//! - **Per-window.** Each window has exactly one context, and each queue
//!   belongs to exactly one window, so a renderer is only ever destroyed in
//!   the context that created it. (Panes never migrate between windows.)

use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use freminal_windowing::{RepaintProxy, WindowId};

use super::ToastRenderState;
use super::gl_facade::Gl;
use super::gpu::{TerminalRenderer, WindowPostRenderer};

/// A thread-safe, GL-free callback that asks a window's event loop for a frame.
///
/// Invoked by [`GlRetireQueue::retire`]; must not take any lock the retiring
/// code path may already hold (notably egui's context lock).
pub type RepaintWake = Arc<dyn Fn() + Send + Sync>;

/// A window-scoped queue of retired [`TerminalRenderer`]s awaiting GL
/// destruction.
///
/// Cloning yields another handle to the **same** queue (it is an
/// `Arc<Mutex<…>>` internally): every pane of a window holds a clone, and the
/// window drains it. The mutex is a leaf lock -- held only to push or to swap
/// the vector out -- so it can never deadlock against `RenderState` or
/// `WindowPostRenderer` locks, and a poisoned lock is recovered rather than
/// propagated (the protected value is a plain `Vec`, which a panicking pusher
/// cannot leave inconsistent).
#[derive(Clone, Default)]
pub struct GlRetireQueue {
    pending: Arc<Mutex<Vec<TerminalRenderer>>>,
    /// Shared by every clone, so a pane that cloned the queue before the
    /// window installed its wake still sees it. Set at most once.
    wake: Arc<OnceLock<RepaintWake>>,
}

impl GlRetireQueue {
    /// Create an empty queue.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Install the callback [`Self::retire`] uses to ask the window for a
    /// frame that will drain the queue. Only the first call has an effect;
    /// the wake is shared by every clone of this queue.
    pub fn set_repaint_wake(&self, wake: RepaintWake) {
        // A second install is ignored: a window has exactly one event-loop
        // target for the lifetime of its queue.
        let _ = self.wake.set(wake);
    }

    /// Wake the window identified by `handle` (a `RepaintProxy` plus window
    /// id, filled in once the event loop exists) whenever a renderer is
    /// retired. A handle that is not yet populated wakes nothing.
    pub fn wake_window_on_retire(&self, handle: Arc<OnceLock<(RepaintProxy, WindowId)>>) {
        self.set_repaint_wake(Arc::new(move || {
            if let Some((proxy, window_id)) = handle.get() {
                proxy.request_repaint(*window_id);
            }
        }));
    }

    /// Queue `renderer` for destruction on the next [`Self::drain`], then
    /// wake the window (see [`RepaintWake`]) so a frame runs to drain it.
    ///
    /// Touches no GL. A renderer that owns no GL objects is simply dropped
    /// rather than queued -- this keeps the queue empty (and `drain` free) for
    /// every pane that never painted. The test is what the renderer *holds*,
    /// not whether its `init` reported success: a renderer whose `init` failed
    /// partway (or was interrupted) still owns whatever it had created
    /// (Task 125.C16).
    pub fn retire(&self, renderer: TerminalRenderer) {
        if !renderer.holds_gl_objects() {
            return;
        }
        self.pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(renderer);
        // The queue lock is released: the wake never runs under it.
        if let Some(wake) = self.wake.get() {
            wake();
        }
    }

    /// Number of renderers waiting to be destroyed.
    #[must_use]
    pub fn len(&self) -> usize {
        self.pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    /// Whether nothing is waiting to be destroyed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Destroy every queued renderer's GL objects and return how many were
    /// destroyed.
    ///
    /// The caller must have the window's GL context current (the same
    /// contract as [`TerminalRenderer::destroy`]). The pending list is taken
    /// out of the lock *before* any GL call, so a renderer is destroyed by
    /// exactly one drain, and the leaf lock is never held across GL work.
    /// A second drain with nothing newly retired is a no-op returning `0`.
    pub fn drain(&self, gl: &Gl<'_>) -> usize {
        let retired =
            std::mem::take(&mut *self.pending.lock().unwrap_or_else(PoisonError::into_inner));
        let count = retired.len();
        for mut renderer in retired {
            renderer.destroy(gl);
        }
        count
    }
}

/// The per-window handles needed to free a window's GL state after the app
/// has already dropped that window's `PerWindowState`.
///
/// `FreminalGui::on_close_requested` removes the window's state (dropping its
/// panes, which retire their renderers into the queue) *before* the windowing
/// layer tears the window's GL context down. This is what it keeps behind so
/// that [`Self::run`] can finish the job in `App::on_window_destroying`,
/// where the context is current.
pub struct WindowGlTeardown {
    window_post: Arc<Mutex<WindowPostRenderer>>,
    toast_render_state: Arc<Mutex<ToastRenderState>>,
}

impl WindowGlTeardown {
    /// Capture the handles a window needs to free its GL state.
    #[must_use]
    pub const fn new(
        window_post: Arc<Mutex<WindowPostRenderer>>,
        toast_render_state: Arc<Mutex<ToastRenderState>>,
    ) -> Self {
        Self {
            window_post,
            toast_render_state,
        }
    }

    /// Destroy everything this window still owns in its GL context: the
    /// retired pane renderers still queued, the shared post-processing
    /// renderer, and both toast passes.
    ///
    /// The window's GL context must be current. Each underlying `destroy` is
    /// idempotent, so running this against an already-clean window is a
    /// no-op.
    pub fn run(self, gl: &Gl<'_>) {
        // `WindowPostRenderer::destroy` drains its retire queue first, so
        // pane renderers and the window-level renderer go in one place.
        self.window_post
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .destroy(gl);

        let mut toast = self
            .toast_render_state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        toast.pill.destroy(gl);
        toast.text.destroy(gl);
    }
}

impl WindowGlTeardown {
    /// Drop this window's GL handles **without** issuing any GL call.
    ///
    /// Used when the window's context could not be made current. The GL
    /// objects are then lost along with the context; deleting their names in
    /// whatever context happens to be current could destroy another window's
    /// objects. Dropping the handles is safe: they hold plain GL names, with
    /// no `Drop` that touches GL.
    pub fn abandon(self) {
        tracing::warn!(
            "window GL context unavailable at teardown; abandoning its GL objects without deleting them"
        );
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    /// A queue whose wake bumps the returned counter.
    fn queue_with_counting_wake() -> (GlRetireQueue, Arc<AtomicUsize>) {
        let queue = GlRetireQueue::new();
        let wakes = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&wakes);
        queue.set_repaint_wake(Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        }));
        (queue, wakes)
    }

    #[test]
    fn retiring_a_renderer_with_no_gl_objects_queues_nothing() {
        let queue = GlRetireQueue::new();
        queue.retire(TerminalRenderer::new());
        assert!(queue.is_empty());
        assert_eq!(queue.len(), 0);
    }

    #[test]
    fn retiring_a_renderer_with_no_gl_objects_does_not_wake() {
        let (queue, wakes) = queue_with_counting_wake();
        queue.retire(TerminalRenderer::new());
        assert_eq!(wakes.load(Ordering::SeqCst), 0, "nothing was queued");
    }

    #[test]
    fn the_wake_is_shared_by_clones_and_installed_once() {
        let queue = GlRetireQueue::new();
        let early_clone = queue.clone();
        let first = Arc::new(AtomicUsize::new(0));
        let second = Arc::new(AtomicUsize::new(0));
        for counter in [&first, &second] {
            let counter = Arc::clone(counter);
            queue.set_repaint_wake(Arc::new(move || {
                counter.fetch_add(1, Ordering::SeqCst);
            }));
        }
        // A clone made before the wake was installed still sees the first one.
        early_clone.wake.get().expect("wake installed")();
        assert_eq!(first.load(Ordering::SeqCst), 1);
        assert_eq!(second.load(Ordering::SeqCst), 0, "second install ignored");
    }

    #[test]
    fn an_unpopulated_repaint_handle_wakes_nothing_and_does_not_panic() {
        let queue = GlRetireQueue::new();
        queue.wake_window_on_retire(Arc::new(OnceLock::new()));
        queue.wake.get().expect("wake installed")();
    }

    #[test]
    fn clones_share_one_queue() {
        let a = GlRetireQueue::new();
        let b = a.clone();
        assert!(Arc::ptr_eq(&a.pending, &b.pending));
    }
}

#[cfg(all(test, feature = "gl-recording"))]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod recording_tests {
    use super::*;

    /// Build a [`TerminalRenderer`] initialized against `gl` (a recording
    /// facade), so it owns fabricated GL handles to destroy.
    fn initialized_renderer(gl: &Gl<'_>) -> TerminalRenderer {
        let mut renderer = TerminalRenderer::new();
        renderer.init(gl).expect("recording init succeeds");
        assert!(renderer.holds_gl_objects());
        renderer
    }

    /// Count how many recorded calls were to `method`.
    fn count(gl: &Gl<'_>, method: &str) -> usize {
        gl.recorded()
            .expect("recording Gl")
            .calls()
            .iter()
            .filter(|c| c.method == method)
            .count()
    }

    #[test]
    fn retired_initialized_renderer_is_queued_then_destroyed_once() {
        let gl = Gl::recording();
        let queue = GlRetireQueue::new();
        queue.retire(initialized_renderer(&gl));
        assert_eq!(queue.len(), 1, "an initialized renderer is queued");

        // Retiring issued no destroy call: it is GL-free by contract.
        let deletes_before = count(&gl, "delete_program");
        assert_eq!(count(&gl, "delete_texture"), 0);

        assert_eq!(queue.drain(&gl), 1);
        assert!(queue.is_empty());
        // Five passes each own one program: bg-instanced, deco, fg, image,
        // bg-image.
        assert_eq!(count(&gl, "delete_program") - deletes_before, 5);

        // Idempotent: a second drain destroys nothing and issues no calls.
        let total_calls = gl.recorded().unwrap().calls().len();
        assert_eq!(queue.drain(&gl), 0);
        assert_eq!(gl.recorded().unwrap().calls().len(), total_calls);
    }

    #[test]
    fn retiring_an_initialized_renderer_wakes_the_window_after_queueing_it() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let gl = Gl::recording();
        let queue = GlRetireQueue::new();
        let wakes = Arc::new(AtomicUsize::new(0));
        let observed_len = Arc::new(AtomicUsize::new(usize::MAX));
        {
            let wakes = Arc::clone(&wakes);
            let observed_len = Arc::clone(&observed_len);
            let probe = queue.clone();
            queue.set_repaint_wake(Arc::new(move || {
                wakes.fetch_add(1, Ordering::SeqCst);
                // Reading the queue from inside the wake proves the queue
                // lock is not held across the wake (it would self-deadlock)
                // and that the renderer is already queued.
                observed_len.store(probe.len(), Ordering::SeqCst);
            }));
        }

        queue.retire(initialized_renderer(&gl));

        assert_eq!(wakes.load(Ordering::SeqCst), 1);
        assert_eq!(observed_len.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_pending_retiree_is_reported_until_drained() {
        let gl = Gl::recording();
        let window_post = WindowPostRenderer::new();
        assert!(!window_post.has_pending_retirees());

        window_post.retire_queue().retire(initialized_renderer(&gl));
        assert!(
            window_post.has_pending_retirees(),
            "a closed pane's renderer waits for a frame: update must ask for one"
        );

        assert_eq!(window_post.drain_retired(&gl), 1);
        assert!(!window_post.has_pending_retirees());
    }

    #[test]
    fn drain_issues_the_full_delete_set_for_a_closed_pane() {
        let gl = Gl::recording();
        let queue = GlRetireQueue::new();
        queue.retire(initialized_renderer(&gl));

        // What `init` created, by call name, must all be deleted by drain.
        let created = |m: &str| count(&gl, m);
        let programs = created("create_program");
        let vaos = created("create_vertex_array");
        let buffers = created("create_buffer");
        let textures = created("create_texture");
        assert!(programs > 0 && vaos > 0 && buffers > 0 && textures > 0);

        assert_eq!(queue.drain(&gl), 1);
        assert_eq!(count(&gl, "delete_program"), programs);
        assert_eq!(count(&gl, "delete_vertex_array"), vaos);
        assert_eq!(count(&gl, "delete_buffer"), buffers);
        assert_eq!(count(&gl, "delete_texture"), textures);
    }

    #[test]
    fn two_retired_renderers_are_each_destroyed_exactly_once() {
        let gl = Gl::recording();
        let queue = GlRetireQueue::new();
        queue.retire(initialized_renderer(&gl));
        queue.retire(initialized_renderer(&gl));
        let programs = count(&gl, "create_program");

        assert_eq!(queue.len(), 2);
        assert_eq!(queue.drain(&gl), 2);
        assert_eq!(count(&gl, "delete_program"), programs);
        assert_eq!(queue.drain(&gl), 0);
        assert_eq!(count(&gl, "delete_program"), programs);
    }

    #[test]
    fn window_teardown_destroys_queued_renderers_and_window_passes() {
        let gl = Gl::recording();
        let window_post = Arc::new(Mutex::new(WindowPostRenderer::new()));
        let toast = ToastRenderState::new_shared();

        let queue = window_post.lock().unwrap().retire_queue();
        queue.retire(initialized_renderer(&gl));
        window_post.lock().unwrap().init(&gl).unwrap();
        toast.lock().unwrap().pill.init(&gl).unwrap();

        let programs = count(&gl, "create_program");
        WindowGlTeardown::new(Arc::clone(&window_post), Arc::clone(&toast)).run(&gl);

        assert!(queue.is_empty(), "teardown drained the pane queue");
        assert!(!window_post.lock().unwrap().holds_gl_objects());
        assert_eq!(
            count(&gl, "delete_program"),
            programs,
            "every program created (pane + window + toast pill) was deleted"
        );

        // Running teardown again over the same (now clean) handles is a no-op.
        let total_calls = gl.recorded().unwrap().calls().len();
        WindowGlTeardown::new(window_post, toast).run(&gl);
        assert_eq!(gl.recorded().unwrap().calls().len(), total_calls);
    }
}
