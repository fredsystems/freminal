// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Codegen probes for the zero-overhead claim (Task 123, subtask 123.6b).
//!
//! One concept: giving the compiler two functions that differ *only* in
//! whether they go through [`super::Gl`], so their emitted machine code can
//! be compared directly.
//!
//! # Why this replaces the benchmark 123.6b originally specified
//!
//! 123.6b was scoped as "benchmark the `Real` arm against a direct
//! `glow::Context` call". That measurement is not resolvable. The facade
//! adds at most a single pointer load; the cheapest real GL call is an
//! indirect jump into Mesa. The dispatch cost is two to three orders of
//! magnitude below the thing it would be measured against, so a benchmark
//! could only ever *fail to detect* a cost — it could never refute the
//! claim, and a "no difference" result would read as far stronger evidence
//! than it is.
//!
//! The actual question 123.6 left open is narrower and sharper: **is there
//! a per-call branch in the emitted code?** That is answered by reading the
//! code, not by timing it. Reading it is also deterministic, needs no GPU
//! or display, and gives a categorical answer rather than a statistical
//! one.
//!
//! # How to run it
//!
//! ```sh
//! ./assets/ci/check-gl-dispatch-codegen.sh
//! ```
//!
//! # The recorded result (`x86_64`, rustc 1.97.1, opt-level 3)
//!
//! Two shapes are probed, and the distinction between them matters.
//!
//! ## The realistic shape — what production actually does
//!
//! [`probe_site_facade`] builds the facade once and makes several calls
//! through it, mirroring `widget.rs`'s `let gl = &Gl::real(painter.gl());`
//! followed by a run of draw calls. Against the same sequence made directly
//! on `glow::Context`, the compiler emits:
//!
//! ```text
//! probe_site_facade = probe_site_direct
//! ```
//!
//! A symbol alias. Not "similar", not "one instruction longer" — LLVM
//! determined the two functions are byte-identical and folded them into the
//! same code. **At a real call site the facade costs nothing whatsoever.**
//!
//! ## The isolated shape — a deliberately pessimistic control
//!
//! [`probe_dispatch_facade`] is a single `#[inline(never)]` call taking
//! `&Gl` as a parameter:
//!
//! ```text
//! probe_dispatch_direct:  jmpq *<glow bind_buffer>@GOTPCREL(%rip)
//! probe_dispatch_facade:  movq (%rdi), %rdi
//!                         jmpq *<glow bind_buffer>@GOTPCREL(%rip)
//! ```
//!
//! One extra instruction. **That `mov` is an artefact of the probe, not a
//! property of the code**: `inline(never)` forces a call shape production
//! never takes, in which the `&Gl` -> `&glow::Context` indirection cannot
//! be folded into the caller. It is kept anyway, because it isolates the
//! question the enum actually raises — and answers it:
//!
//! **No branch, in either shape.** No `cmp`, no `test`, no conditional
//! jump, no discriminant load. The single-variant `match` compiles away
//! even when inlining is forbidden, which is what 123.6's layout test could
//! only argue for indirectly.
//!
//! An earlier revision of this file reported the isolated `mov` as a
//! correction to the "zero cost" claim. That was wrong, and the error is
//! recorded rather than deleted: the pessimistic probe was mistaken for the
//! real thing. The realistic shape is the one that describes production,
//! and there the cost is exactly zero.

// `no_mangle` on a Rust-ABI function normally warns because it is an FFI
// footgun: a C caller would be relying on an unstable ABI. These symbols are
// never called by anything — they exist only so the optimiser cannot delete
// them and so they can be found by name in the emitted assembly. Switching
// them to `extern "C"` would defeat the purpose twice over: it would change
// the calling convention being measured, and `Option<glow::Buffer>` is not
// FFI-safe, so it would trade this lint for `improper_ctypes_definitions`.
#![allow(clippy::no_mangle_with_rust_abi)]

use super::Gl;

/// Dispatch `bind_buffer` through the facade.
///
/// `#[inline(never)]` and `#[unsafe(no_mangle)]` exist so the symbol
/// survives optimisation and is findable by name in the emitted assembly.
/// Without `inline(never)` there would be nothing left to compare.
///
/// # Safety
///
/// Same contract as `glow::HasContext::bind_buffer`.
#[inline(never)]
#[unsafe(no_mangle)]
pub unsafe fn probe_dispatch_facade(gl: &Gl<'_>, target: u32, buffer: Option<glow::Buffer>) {
    unsafe { gl.bind_buffer(target, buffer) }
}

/// The control: the identical call made straight on `glow::Context`.
///
/// # Safety
///
/// Same contract as `glow::HasContext::bind_buffer`.
#[inline(never)]
#[unsafe(no_mangle)]
pub unsafe fn probe_dispatch_direct(
    ctx: &glow::Context,
    target: u32,
    buffer: Option<glow::Buffer>,
) {
    use glow::HasContext as _;
    unsafe { ctx.bind_buffer(target, buffer) }
}

/// A many-argument call through the facade, in case argument shuffling
/// differs from the two-argument case.
///
/// # Safety
///
/// Same contract as `glow::HasContext::draw_arrays_instanced`.
#[inline(never)]
#[unsafe(no_mangle)]
pub unsafe fn probe_draw_facade(gl: &Gl<'_>, mode: u32, first: i32, count: i32, instances: i32) {
    unsafe { gl.draw_arrays_instanced(mode, first, count, instances) }
}

/// The control for [`probe_draw_facade`].
///
/// # Safety
///
/// Same contract as `glow::HasContext::draw_arrays_instanced`.
#[inline(never)]
#[unsafe(no_mangle)]
pub unsafe fn probe_draw_direct(
    ctx: &glow::Context,
    mode: u32,
    first: i32,
    count: i32,
    instances: i32,
) {
    use glow::HasContext as _;
    unsafe { ctx.draw_arrays_instanced(mode, first, count, instances) }
}

/// The realistic shape: build the facade once, then make several calls
/// through it — exactly what `widget.rs` does with
/// `let gl = &Gl::real(painter.gl());` followed by a run of draw calls.
///
/// This is the probe that describes production. Compare its emitted code
/// against [`probe_site_direct`].
///
/// # Safety
///
/// Same contract as the underlying `glow::HasContext` methods.
#[inline(never)]
#[unsafe(no_mangle)]
pub unsafe fn probe_site_facade(ctx: &glow::Context, a: u32, b: u32) {
    let gl = Gl::real(ctx);
    unsafe {
        gl.bind_buffer(a, None);
        gl.enable(b);
        gl.draw_arrays(a, 0, 6);
        gl.disable(b);
    }
}

/// The control for [`probe_site_facade`]: the identical sequence made
/// straight on `glow::Context`.
///
/// # Safety
///
/// Same contract as the underlying `glow::HasContext` methods.
#[inline(never)]
#[unsafe(no_mangle)]
pub unsafe fn probe_site_direct(ctx: &glow::Context, a: u32, b: u32) {
    use glow::HasContext as _;
    unsafe {
        ctx.bind_buffer(a, None);
        ctx.enable(b);
        ctx.draw_arrays(a, 0, 6);
        ctx.disable(b);
    }
}
