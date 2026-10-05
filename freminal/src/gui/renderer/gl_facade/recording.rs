// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! The `gl-recording` backend: an append-only log of GL calls plus the
//! handle fabrication `facade::Gl`'s `Recording` arm needs to stand in for
//! a real driver.
//!
//! This entire module is compiled only under the `gl-recording` feature
//! (see the `#[cfg(feature = "gl-recording")]` on its declaration in
//! [`super`]). A default build does not see this code at all — there is no
//! per-call branch, no log, no counters, nothing to pay for.
//!
//! Task 123, `PLAN_123_GL_MEASUREMENT_HARNESS.md`, subtask 123.2.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::num::NonZeroU32;

use conv2::ConvUtil;

use super::surface::{DRAW_CALL_METHODS, STATE_CHANGE_METHODS, UPLOAD_METHODS};

/// What a recorded call carries beyond its name.
///
/// The plan permits either one enum variant per method or a single record
/// type plus an opcode, and forbids fields 123.8's assertions do not need.
/// 123.8 needs draw-call count, state-change count, upload count **and
/// byte volume**; this payload carries exactly that without 56 variants of
/// mostly-unused arguments. It also avoids sentinel-zero fields on
/// non-upload, non-draw calls — per the `state-representation` skill, a
/// variant that is simply absent beats a `bytes: 0` / `vertices: 0` field
/// meaning "not applicable".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlCallPayload {
    /// Neither an upload nor a draw — the overwhelming majority of the
    /// 56-method surface (binds, enables, shader/program lifecycle,
    /// uniforms, queries).
    None,
    /// An upload; `bytes` is the payload size actually handed to the
    /// driver (`0` for a `PixelUnpackData::BufferOffset` / `Slice(None)`
    /// call, which uploads nothing new from the CPU side).
    Upload {
        /// Byte volume of this single upload.
        bytes: u64,
    },
    /// A draw; `instances` is `1` for the non-instanced `draw_arrays`,
    /// matching `draw_arrays_instanced`'s own semantics for a call with
    /// `instance_count == 1` rather than inventing a separate
    /// "non-instanced" state.
    Draw {
        /// Vertex count passed to the driver for this draw.
        vertices: u32,
        /// Instance count for this draw.
        instances: u32,
    },
}

/// What a fallible recorded call should do: succeed as the fabricated driver
/// normally would, or fail because a test armed a fault for it.
///
/// Returned by [`RecordingState::record_fallible`]; see
/// [`RecordingState::fail_nth_call`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FaultOutcome {
    /// No fault applies: behave as the plausible-success recording default.
    Proceed,
    /// A fault was armed for exactly this call: report failure.
    Fail,
}

/// The error message a fault-injected `create_*` call returns.
pub(super) const INJECTED_FAULT: &str = "injected GL fault";

/// One recorded `glow::HasContext` call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GlCall {
    /// Always one of [`super::surface::GL_CALL_SURFACE`].
    pub method: &'static str,
    /// The call's derived-metric payload; see [`GlCallPayload`].
    pub payload: GlCallPayload,
}

/// The `gl-recording` backend's mutable state: the call log plus one
/// monotonic counter per fabricated handle type.
///
/// # Why interior mutability, not `&mut self`
///
/// The plan mandates renderer signatures take `&Gl<'_>` — a shared
/// reference, mirroring `&glow::Context`'s own borrow shape so the
/// 123.4/123.5 migration is a pure type substitution at call sites, not a
/// threading of `&mut` through code that never needed it for the real
/// driver. Recording must therefore mutate through `&self`, which is
/// exactly what interior mutability is for. `Gl` is always constructed
/// locally — inside a single `PaintCallback` or a single test — and never
/// shared across threads, so `RefCell`/`Cell` are correct and no `Sync`
/// bound is needed or wanted.
pub struct RecordingState {
    calls: RefCell<Vec<GlCall>>,
    next_buffer: Cell<u32>,
    next_shader: Cell<u32>,
    next_program: Cell<u32>,
    next_vertex_array: Cell<u32>,
    next_texture: Cell<u32>,
    next_framebuffer: Cell<u32>,
    next_uniform_location: Cell<u32>,
    /// Task 125.8: monotonic counter fabricating `glow::Query` handles for
    /// `create_query`, the same per-type-counter style as the six handle
    /// types above.
    next_query: Cell<u32>,
    /// Task 125.8: which fabricated query handles a test (or the
    /// `gpu_profiling` adapter's own test suite) has declared "available",
    /// and the nanosecond value each should report -- see
    /// [`Self::mark_query_available`]. Absent from this map means "not yet
    /// available", the correct default for a query that was just issued.
    query_results: RefCell<HashMap<glow::Query, u64>>,
    /// Task 125.8: the string [`super::facade::Gl::get_parameter_string`]'s
    /// `Recording` arm reports for `glow::VERSION`. Defaults to empty,
    /// which `freminal_windowing::gpu_profiling::parse_gl_version_string`
    /// cannot parse, so
    /// `gui::renderer::gpu_profiling::PaneGpuTimingProfile::detect_capability`
    /// conservatively resolves `Unavailable` on a fresh recording session
    /// by default -- **deliberately**, so the many other recording-driven
    /// test suites in this crate (Task 123's headless call-count workload
    /// assertions among them) that never call
    /// [`Self::set_gl_version_string`] see zero query-related GL calls and
    /// their existing, hand-derived call counts stay exactly as they were
    /// before Task 125.8. Only a test that explicitly opts in via
    /// [`Self::set_gl_version_string`] observes `Available` capability.
    gl_version_string: RefCell<String>,
    /// Armed faults: `(method, absolute index among that method's recorded
    /// calls)`. One-shot -- an entry is removed when it fires. See
    /// [`Self::fail_nth_call`].
    armed_faults: RefCell<Vec<(&'static str, usize)>>,
    /// Methods whose recorded call was made to fail (one entry per injected
    /// failure), so a test can tell the calls that created an object from
    /// those that did not. See [`Self::succeeded_count_of`].
    injected_failures: RefCell<Vec<&'static str>>,
}

impl Default for RecordingState {
    fn default() -> Self {
        Self::new()
    }
}

impl RecordingState {
    /// Build an empty recording state. All handle counters start at `1` —
    /// GL object names are non-zero, and the six handle types that wrap
    /// `NonZeroU32` require it; `next_uniform_location` starts at `1` too,
    /// purely for consistency, since `NativeUniformLocation` wraps a plain
    /// `u32` with no non-zero requirement.
    #[must_use]
    pub fn new() -> Self {
        Self {
            calls: RefCell::new(Vec::new()),
            next_buffer: Cell::new(1),
            next_shader: Cell::new(1),
            next_program: Cell::new(1),
            next_vertex_array: Cell::new(1),
            next_texture: Cell::new(1),
            next_framebuffer: Cell::new(1),
            next_uniform_location: Cell::new(1),
            next_query: Cell::new(1),
            query_results: RefCell::new(HashMap::new()),
            gl_version_string: RefCell::new(String::new()),
            armed_faults: RefCell::new(Vec::new()),
            injected_failures: RefCell::new(Vec::new()),
        }
    }

    /// Append one recorded call to the log.
    pub fn record(&self, call: GlCall) {
        self.calls.borrow_mut().push(call);
    }

    /// Clone the full call log out. Cheap: [`GlCall`] is `Copy`.
    #[must_use]
    pub fn calls(&self) -> Vec<GlCall> {
        self.calls.borrow().clone()
    }

    /// Number of calls recorded so far.
    #[must_use]
    pub fn len(&self) -> usize {
        self.calls.borrow().len()
    }

    /// Whether no calls have been recorded yet.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.calls.borrow().is_empty()
    }

    /// Discard the recorded log, keeping the handle counters as-is.
    pub fn clear(&self) {
        self.calls.borrow_mut().clear();
    }

    /// Count of recorded calls whose method name equals `method`.
    #[must_use]
    pub fn count_of(&self, method: &str) -> usize {
        self.calls
            .borrow()
            .iter()
            .filter(|call| call.method == method)
            .count()
    }

    /// Arm a one-shot fault: the `nth` (0-based) call to `method` made from
    /// now on fails, as a driver that ran out of names or rejected a shader
    /// would.
    ///
    /// Covers every fallible creation (`create_buffer`, `create_shader`,
    /// `create_program`, `create_vertex_array`, `create_texture`,
    /// `create_framebuffer`, `create_query`) and the two status queries
    /// (`get_shader_compile_status`, `get_program_link_status`). The failing
    /// call is still recorded -- the renderer did make it -- but creates no
    /// object; [`Self::succeeded_count_of`] counts only the calls that did.
    /// Any number of faults may be armed; each fires once.
    pub fn fail_nth_call(&self, method: &'static str, nth: usize) {
        let absolute = self.count_of(method) + nth;
        self.armed_faults.borrow_mut().push((method, absolute));
    }

    /// Record one call to `method` and report whether an armed fault makes it
    /// fail. Backs the fallible arms of `facade::Gl`.
    pub(super) fn record_fallible(&self, method: &'static str) -> FaultOutcome {
        let index = self.count_of(method);
        self.record(GlCall {
            method,
            payload: GlCallPayload::None,
        });
        let fired = {
            let mut faults = self.armed_faults.borrow_mut();
            let position = faults.iter().position(|&f| f == (method, index));
            position.map(|pos| faults.remove(pos))
        };
        match fired {
            Some(_) => {
                self.injected_failures.borrow_mut().push(method);
                FaultOutcome::Fail
            }
            None => FaultOutcome::Proceed,
        }
    }

    /// Count of recorded calls to `method` that were *not* made to fail by
    /// [`Self::fail_nth_call`]: for a `create_*` method, the number of objects
    /// actually created, which is what a `delete_*` count must match.
    #[must_use]
    pub fn succeeded_count_of(&self, method: &str) -> usize {
        let failed = self
            .injected_failures
            .borrow()
            .iter()
            .filter(|m| **m == method)
            .count();
        self.count_of(method) - failed
    }

    /// Count of recorded calls in [`super::surface::DRAW_CALL_METHODS`].
    #[must_use]
    pub fn draw_calls(&self) -> usize {
        self.calls
            .borrow()
            .iter()
            .filter(|call| DRAW_CALL_METHODS.contains(&call.method))
            .count()
    }

    /// Count of recorded calls in [`super::surface::STATE_CHANGE_METHODS`].
    #[must_use]
    pub fn state_changes(&self) -> usize {
        self.calls
            .borrow()
            .iter()
            .filter(|call| STATE_CHANGE_METHODS.contains(&call.method))
            .count()
    }

    /// Count of recorded calls in [`super::surface::UPLOAD_METHODS`].
    #[must_use]
    pub fn uploads(&self) -> usize {
        self.calls
            .borrow()
            .iter()
            .filter(|call| UPLOAD_METHODS.contains(&call.method))
            .count()
    }

    /// Sum of `bytes` across every recorded [`GlCallPayload::Upload`].
    #[must_use]
    pub fn uploaded_bytes(&self) -> u64 {
        self.calls
            .borrow()
            .iter()
            .filter_map(|call| match call.payload {
                GlCallPayload::Upload { bytes } => Some(bytes),
                GlCallPayload::None | GlCallPayload::Draw { .. } => None,
            })
            .sum()
    }

    /// Sum of `instances` across every recorded [`GlCallPayload::Draw`].
    #[must_use]
    pub fn instances_drawn(&self) -> u64 {
        self.calls
            .borrow()
            .iter()
            .filter_map(|call| match call.payload {
                GlCallPayload::Draw { instances, .. } => {
                    Some(instances.approx_as::<u64>().unwrap_or(0))
                }
                GlCallPayload::None | GlCallPayload::Upload { .. } => None,
            })
            .sum()
    }

    /// Bump `counter` and return the pre-bump value as a `NonZeroU32`.
    ///
    /// The `Recording` arm never sees a real driver handle, so a
    /// fabricated handle cannot collide with a real one within one `Gl`
    /// instance — the two are distinguishable only by convention (an
    /// instance is either wholly `Real` or wholly `Recording`), never by
    /// tagging the handle value itself.
    ///
    /// Production code may not `unwrap`/`expect`; `NonZeroU32::MIN` is the
    /// non-panicking fallback for the value `0`, which the counter cannot
    /// actually reach before wrapping past `u32::MAX` calls in a single
    /// recording session.
    fn bump(counter: &Cell<u32>) -> NonZeroU32 {
        let current = counter.get();
        counter.set(current.wrapping_add(1));
        NonZeroU32::new(current).unwrap_or(NonZeroU32::MIN)
    }

    /// Fabricate the next `glow::Buffer`.
    pub(super) fn next_buffer(&self) -> glow::Buffer {
        glow::NativeBuffer(Self::bump(&self.next_buffer))
    }

    /// Fabricate the next `glow::Shader`.
    pub(super) fn next_shader(&self) -> glow::Shader {
        glow::NativeShader(Self::bump(&self.next_shader))
    }

    /// Fabricate the next `glow::Program`.
    pub(super) fn next_program(&self) -> glow::Program {
        glow::NativeProgram(Self::bump(&self.next_program))
    }

    /// Fabricate the next `glow::VertexArray`.
    pub(super) fn next_vertex_array(&self) -> glow::VertexArray {
        glow::NativeVertexArray(Self::bump(&self.next_vertex_array))
    }

    /// Fabricate the next `glow::Texture`.
    pub(super) fn next_texture(&self) -> glow::Texture {
        glow::NativeTexture(Self::bump(&self.next_texture))
    }

    /// Fabricate the next `glow::Framebuffer`.
    pub(super) fn next_framebuffer(&self) -> glow::Framebuffer {
        glow::NativeFramebuffer(Self::bump(&self.next_framebuffer))
    }

    /// Fabricate the next `glow::UniformLocation`.
    ///
    /// Unlike the other six handle types, `NativeUniformLocation` wraps a
    /// plain `GLuint` (`u32`), not a `NonZeroU32`, so no non-zero fallback
    /// is needed here.
    pub(super) fn next_uniform_location(&self) -> glow::UniformLocation {
        let current = self.next_uniform_location.get();
        self.next_uniform_location.set(current.wrapping_add(1));
        glow::NativeUniformLocation(current)
    }

    /// Fabricate the next `glow::Query` (Task 125.8).
    pub(super) fn next_query(&self) -> glow::Query {
        glow::NativeQuery(Self::bump(&self.next_query))
    }

    /// Override the string [`super::facade::Gl::get_parameter_string`]'s
    /// `Recording` arm reports for `glow::VERSION` (Task 125.8). Test/
    /// adapter-support API, mirroring [`Self::mark_query_available`]: a
    /// test that wants `gui::renderer::gpu_profiling::PaneGpuTimingProfile::detect_capability`
    /// to resolve `Available` against a recording session calls this with
    /// a parseable desktop version string (e.g. `"4.6.0 Recording"`)
    /// before calling `detect_capability`. See [`Self::gl_version_string`]'s
    /// doc on the field for why the default is deliberately empty
    /// (unparsable) instead.
    pub fn set_gl_version_string(&self, version: impl Into<String>) {
        *self.gl_version_string.borrow_mut() = version.into();
    }

    /// The current fabricated `GL_VERSION` string (Task 125.8) -- empty
    /// until [`Self::set_gl_version_string`] is called. Backs the
    /// `Recording` arm of `Gl::get_parameter_string(glow::VERSION)`.
    pub(super) fn gl_version_string(&self) -> String {
        self.gl_version_string.borrow().clone()
    }

    /// Declare `query`'s result available, with the given nanosecond value
    /// (Task 125.8). Purely a test/adapter-support API: production code
    /// never calls this (a real driver decides availability on its own
    /// schedule); it exists so a deterministic test -- or the
    /// `gpu_profiling` adapter's own test suite -- can simulate a query's
    /// result becoming ready on a specific later poll, exactly the
    /// "delayed availability" scenario
    /// `freminal_windowing::gpu_profiling::GpuQueryRing` is built to
    /// handle. Before this is called for a given `query`, both
    /// [`super::facade::Gl::get_query_parameter_u32`] (with
    /// `GL_QUERY_RESULT_AVAILABLE`, via the `Recording` arm) reports
    /// unavailable (`0`) and
    /// [`super::facade::Gl::get_query_parameter_u64`] reports `0` for it.
    pub fn mark_query_available(&self, query: glow::Query, result_ns: u64) {
        self.query_results.borrow_mut().insert(query, result_ns);
    }

    /// Whether `query` has been marked available via
    /// [`Self::mark_query_available`] (Task 125.8). Backs the `Recording`
    /// arm of `Gl::get_query_parameter_u32(query, GL_QUERY_RESULT_AVAILABLE)`.
    pub(super) fn query_is_available(&self, query: glow::Query) -> bool {
        self.query_results.borrow().contains_key(&query)
    }

    /// The nanosecond value marked for `query` via
    /// [`Self::mark_query_available`], or `0` if it was never marked (Task
    /// 125.8). Backs the `Recording` arm of
    /// `Gl::get_query_parameter_u64(query, GL_QUERY_RESULT)`. The
    /// no-premature-read invariant is enforced by
    /// `freminal_windowing::gpu_profiling::GpuQueryRing` (never reads a
    /// result before availability succeeds), not by this method -- a
    /// non-panicking `0` fallback keeps this recording-support code itself
    /// free of `unwrap`/`expect`/`panic!`, per the crate's panic-free
    /// production rule.
    pub(super) fn query_result_ns(&self, query: glow::Query) -> u64 {
        self.query_results
            .borrow()
            .get(&query)
            .copied()
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::{FaultOutcome, GlCall, GlCallPayload, RecordingState};

    #[test]
    fn new_state_is_empty() {
        let state = RecordingState::new();
        assert!(state.is_empty());
        assert_eq!(state.len(), 0);
        assert_eq!(state.uploaded_bytes(), 0);
        assert_eq!(state.instances_drawn(), 0);
    }

    #[test]
    fn record_and_count() {
        let state = RecordingState::new();
        state.record(GlCall {
            method: "clear",
            payload: GlCallPayload::None,
        });
        state.record(GlCall {
            method: "draw_arrays",
            payload: GlCallPayload::Draw {
                vertices: 6,
                instances: 1,
            },
        });
        assert_eq!(state.len(), 2);
        assert_eq!(state.count_of("clear"), 1);
        assert_eq!(state.draw_calls(), 1);
        assert_eq!(state.state_changes(), 0);
        assert_eq!(state.instances_drawn(), 1);
    }

    #[test]
    fn uploads_sum_bytes() {
        let state = RecordingState::new();
        state.record(GlCall {
            method: "buffer_data_u8_slice",
            payload: GlCallPayload::Upload { bytes: 128 },
        });
        state.record(GlCall {
            method: "tex_image_2d",
            payload: GlCallPayload::Upload { bytes: 256 },
        });
        assert_eq!(state.uploads(), 2);
        assert_eq!(state.uploaded_bytes(), 384);
    }

    #[test]
    fn clear_empties_the_log_but_not_the_counters() {
        let state = RecordingState::new();
        let first = state.next_buffer();
        state.record(GlCall {
            method: "create_buffer",
            payload: GlCallPayload::None,
        });
        state.clear();
        assert!(state.is_empty());
        let second = state.next_buffer();
        assert_ne!(first, second);
    }

    #[test]
    fn fabricated_handles_are_monotonic_and_non_colliding() {
        let state = RecordingState::new();
        let b1 = state.next_buffer();
        let b2 = state.next_buffer();
        assert_ne!(b1, b2);

        let s1 = state.next_shader();
        let p1 = state.next_program();
        let v1 = state.next_vertex_array();
        let t1 = state.next_texture();
        let f1 = state.next_framebuffer();
        let u1 = state.next_uniform_location();
        let u2 = state.next_uniform_location();
        assert_ne!(u1, u2);

        // Each handle type has its own counter starting at 1, independent
        // of the others.
        assert_eq!(s1.0.get(), 1);
        assert_eq!(p1.0.get(), 1);
        assert_eq!(v1.0.get(), 1);
        assert_eq!(t1.0.get(), 1);
        assert_eq!(f1.0.get(), 1);
        assert_eq!(u1.0, 1);
    }

    #[test]
    fn fail_nth_call_fires_once_on_exactly_that_call() {
        let state = RecordingState::new();
        state.fail_nth_call("create_buffer", 1);

        assert_eq!(
            state.record_fallible("create_buffer"),
            FaultOutcome::Proceed
        );
        assert_eq!(state.record_fallible("create_buffer"), FaultOutcome::Fail);
        assert_eq!(
            state.record_fallible("create_buffer"),
            FaultOutcome::Proceed
        );

        assert_eq!(state.count_of("create_buffer"), 3, "failures are recorded");
        assert_eq!(state.succeeded_count_of("create_buffer"), 2);
    }

    #[test]
    fn fail_nth_call_counts_from_arming_and_per_method() {
        let state = RecordingState::new();
        let _ = state.record_fallible("create_buffer");
        let _ = state.record_fallible("create_shader");
        state.fail_nth_call("create_buffer", 0);
        state.fail_nth_call("get_shader_compile_status", 0);

        assert_eq!(
            state.record_fallible("create_shader"),
            FaultOutcome::Proceed
        );
        assert_eq!(state.record_fallible("create_buffer"), FaultOutcome::Fail);
        assert_eq!(
            state.record_fallible("get_shader_compile_status"),
            FaultOutcome::Fail
        );
        assert_eq!(
            state.record_fallible("get_shader_compile_status"),
            FaultOutcome::Proceed
        );
    }
}
