// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! The GL call boundary (Task 123, `PLAN_123_GL_MEASUREMENT_HARNESS.md`).
//!
//! This module owns exactly one concept: the seam between freminal's
//! renderer code and the concrete `glow::Context` it draws through. That
//! seam exists so a future test can record every GL call freminal makes
//! without a GPU, a display server, or a driver — see the plan document's
//! "Phase 1 — call-recording harness" section for the full motivation.
//!
//! # `glow::HasContext` is sealed — do not retry implementing it
//!
//! `glow::HasContext` is **sealed**: `glow-0.17.0/src/lib.rs:142` declares
//! `pub trait HasContext: __private::Sealed`, and `__private::Sealed`
//! (`lib.rs:4845-4849`) is implemented only for glow's own `native::Context`
//! (`native.rs:198`). Implementing `HasContext` on a wrapper type is a
//! **hard compile error**, not a design tradeoff to weigh. This is stated
//! here so nobody spends time re-discovering it: the trait-implementation
//! approach for a recording/real dispatch shim does not compile, full stop.
//!
//! # The chosen design: a concrete facade, not a trait, not generics
//!
//! The facade is a concrete struct (`Gl`) wrapping an internal `Real` /
//! `Recording` enum — never a trait object, never a generic parameter.
//! Rationale, recorded here because it is easy to re-litigate later: a
//! trait-based approach would force generic bounds at roughly 52
//! `&glow::Context` parameter sites across five files
//! (`gpu.rs`, `toast_pass.rs`, `toast_text_pass.rs`, `widget.rs`,
//! `app_impl.rs`), widening every one of those signatures. The enum keeps
//! codegen monomorphic in the `Real` arm — the arm every production build
//! actually takes — and turns the migration into a mechanical type
//! substitution (`&glow::Context` becomes `&Gl<'_>`) rather than a redesign
//! of call sites.
//!
//! # What lives here today
//!
//! This module now holds the `Gl` facade ([`facade`], re-exported as
//! [`Gl`]), the frozen call surface ([`surface`]) — the enumerated, audited
//! list of every `glow::HasContext` method freminal calls, plus the
//! test-time guard that keeps that enumeration honest until the migration
//! lands — and, behind the `gl-recording` feature, the recording backend
//! ([`recording`]). Migrating the five call sites above onto `Gl` is still
//! 123.4/123.5.

mod facade;
#[cfg(feature = "gl-recording")]
pub mod recording;
pub mod surface;

pub use facade::Gl;

// The behavioural verification suite (subtask 123.3) drives the
// `Recording` arm end to end, so it only compiles where that arm exists.
// It lives in its own file rather than inside `facade.rs` per
// `module-cohesion`: `facade.rs` already carries 49 methods plus its own
// structural completeness test, and the behavioural suite is a distinct
// body of work with its own handle-fabrication setup.
#[cfg(all(test, feature = "gl-recording"))]
mod recording_tests;
