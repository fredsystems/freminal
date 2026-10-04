// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Live render-work profiling foundation (Task 125.4), wired to the
//! `gui::terminal::widget` pane render path by Task 125.5.
//!
//! This module is entirely gated behind the existing `frame-profiling`
//! Cargo feature (see `freminal/Cargo.toml`) and compiled out completely in
//! a default build -- see the `#[cfg(feature = "frame-profiling")]` on this
//! module's declaration in `super`. It defines the cohesive types
//! [`RenderWorkClass`], [`ChangedRowBucket`], [`UploadByteCounts`], and
//! [`LiveRenderProfile`] itself (125.4), plus the raw-decision mirror types
//! [`RawRebuildDecision`] / [`ReevaluatedRebuild`] and the pure
//! [`resolve_render_work_class`] mapping (125.5) that let
//! `gui::terminal::widget::FreminalTerminalWidget::show` and its pane
//! `PaintCallback` record a live observation per frame without this module
//! naming `gui::terminal::frame_dirty::VertexRebuild` directly (that type is
//! `pub(super)`-scoped to `gui::terminal` and unreachable from here -- see
//! [`RawRebuildDecision`]'s doc for why a mirror, not an import, is
//! correct). Task 125.6 completes real per-buffer upload-byte attribution:
//! `gui::renderer::gpu::TerminalRenderer::draw_with_verts` and
//! `draw_with_cursor_only_update` now return the exact bytes their own GL
//! upload calls issued, and the `widget.rs` paint callback finalizes each
//! observation with that real [`UploadByteCounts`] instead of
//! [`UploadByteCounts::default`] -- an uninvoked callback (the
//! `FrameDamage::None` case) still finalizes as zero-upload, which remains
//! correct because it is true: no GL upload was ever issued for it. 125.6
//! also adds [`RenderWorkClassUploadTotals`], attributing cumulative upload
//! bytes to each resolved [`RenderWorkClass`] so a later reconciliation
//! pass can divide bytes by [`RenderWorkClassCounts`] to get an
//! average-bytes-per-frame figure per class.
//!
//! # Why a token, not a plain counter
//!
//! `freminal`'s render path begins a pane's `show()` every egui update, but
//! egui only invokes that pane's registered `PaintCallback` when the frame
//! actually draws something (Task 124.2's `FrameDamage::None` means the
//! callback is never invoked at all for a frame where nothing changed
//! anywhere). A profiler that simply incremented a "started" counter and a
//! "finished" counter separately could not tell a genuinely-skipped paint
//! apart from a completion that arrived for the wrong frame. [`PaneFrameToken`]
//! exists to make that distinction load-bearing: [`LiveRenderProfile::start`]
//! hands out a fresh, strictly increasing token every call, and
//! [`LiveRenderProfile::complete`] only finalizes the record it names when
//! that token is still the one currently outstanding. Starting a new token
//! before the previous one completed finalizes the previous one as
//! [`PaneFrameOutcome::Unpainted`] -- this is precisely the
//! `FrameDamage::None` case: `show()` ran and got a token, no paint
//! callback ever ran, and the next `show()` call is what notices.
//!
//! [`LiveRenderProfile::start`] takes the raw decision and resolved class
//! for the observation it begins (not just a bare token) and retains it
//! internally for exactly this reason: an unpainted finalization still
//! needs a classification to count toward [`RawRebuildDecisionCounts`],
//! [`RenderWorkClassCounts`], and [`ChangedRowHistogram`] (a fix over an
//! earlier revision of this module, which stored only the token and
//! silently discarded a superseded observation's classification --
//! [`PaneFrameOutcome::Unpainted`] now carries it, so every finalized
//! observation, painted or not, is auditable and the cumulative totals
//! reconcile against `painted_count + unpainted_count`).
//! [`LiveRenderProfile::complete`] therefore takes back only the token and
//! the upload counts -- the classification a callback's `complete` call
//! reports can never disagree with what `start` recorded for that same
//! token, because it is not supplied a second time.
//!
//! # State machine summary
//!
//! - [`LiveRenderProfile::start`] always succeeds and always returns a new
//!   token. If a previous token was left outstanding, it is finalized as
//!   unpainted first, using the raw/class it was itself started with.
//! - [`LiveRenderProfile::complete`] only finalizes the record when the
//!   supplied token equals the currently outstanding one. A late
//!   completion (for a token superseded by a newer [`start`] call), a
//!   duplicate completion (the same token completed twice), or an
//!   out-of-order completion (a token that was never the outstanding one)
//!   are all silently ignored and reported back to the caller as `false` --
//!   see the `paned`/`unpainted`/`late`/`duplicate`/`out_of_order` tests
//!   below for one scenario each.
//! - [`LiveRenderProfile::take_flush_signal`] is a single, consumable
//!   signal shared by both finalizers: whichever of a stale-token sweep
//!   inside [`start`] or a matching [`complete`] happens to be the one
//!   that finalizes the observation which crosses a
//!   [`LiveRenderProfile::FLUSH_EVERY`] boundary latches it, and whichever
//!   caller (in `widget.rs`, both `show()` right after `start` and the
//!   paint callback right after `complete`) calls
//!   [`take_flush_signal`][`LiveRenderProfile::take_flush_signal`] first
//!   consumes it. This replaces an earlier stateless `is_flush_due`
//!   predicate that only the paint-callback call site ever consulted: a
//!   boundary crossed by the `start`-side sweep was reached, but nothing
//!   checked for it there, and by the time the next `complete()` ran the
//!   count had already advanced past the exact multiple, permanently
//!   losing that flush.

use std::collections::VecDeque;

/// A strictly increasing identifier for one pane's `show()` call (125.4).
///
/// Handed out by [`LiveRenderProfile::start`] and consumed by exactly one
/// matching [`LiveRenderProfile::complete`] call (see the module doc for
/// why a later, mismatched completion is rejected rather than accepted).
/// The wrapped counter is private; callers that need it for a diagnostic
/// log line (125.5/125.6) read it back through [`Self::value`] rather than
/// constructing or comparing raw integers directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaneFrameToken(u64);

impl PaneFrameToken {
    /// The raw monotonic counter value, for diagnostic logging only. Never
    /// meant to be reconstructed from a bare integer outside this module --
    /// the only way to obtain a live token is [`LiveRenderProfile::start`].
    #[must_use]
    pub const fn value(self) -> u64 {
        self.0
    }
}

/// Resolved render-work classification for one completed pane `show()`
/// call (125.4).
///
/// Matches the outcomes 125.5 resolves from the already-computed
/// `dirty.rebuild` / `changed_rows` / `full_rebuild` decision (see that
/// subtask's scope note -- this type is defined here so the state machine
/// can be built and tested before any call site records into it).
///
/// [`Self::Bounded`] carries its own [`ChangedRowBucket`] rather than
/// pairing a bare `Bounded` variant with a separate `Option<ChangedRowBucket>`
/// field elsewhere: the row-count bucket only has meaning for a bounded
/// rebuild, so making it part of the variant's data makes "a `Bounded`
/// record with no bucket" and "a `Full` record with a stray bucket"
/// unrepresentable rather than merely unused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderWorkClass {
    /// The pane reused its existing vertices with no rebuild at all
    /// (`VertexRebuild::Reuse`, resolved from a `ReevaluateFullRebuild`
    /// that concluded nothing changed).
    Reuse,
    /// The pane took the cursor-only fast path (`VertexRebuild::CursorOnly`).
    CursorOnly,
    /// A rebuild whose resolved damage is provably bounded to a
    /// changed-row count (`VertexRebuild::Bounded` / `ChangedRows::Rows`).
    Bounded(ChangedRowBucket),
    /// A full, unbounded rebuild (`VertexRebuild::Full`, or a
    /// `ReevaluateFullRebuild` that resolved to a full rebuild).
    Full,
}

/// The raw `VertexRebuild` decision, mirrored here for the live profiler
/// (Task 125.5).
///
/// `super::super::terminal::frame_dirty::VertexRebuild` is
/// `pub(super)`-scoped to the `gui::terminal` module tree and therefore
/// unreachable from `gui::renderer::profiling`, a sibling module -- see
/// `freminal-module-cohesion`'s path-visibility rules. Rather than
/// widening that type's visibility purely so this module could name it,
/// this is a small mirror carrying the identical three-way shape plus
/// exactly the extra data [`resolve_render_work_class`] needs to compute
/// the matching [`RenderWorkClass`]: the call site in `widget.rs`
/// constructs one of these from its own `VertexRebuild` match, so the two
/// enums are kept in lockstep by that single call site rather than by a
/// shared type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawRebuildDecision {
    /// Mirrors `VertexRebuild::CursorOnly`.
    CursorOnly,
    /// Mirrors `VertexRebuild::Bounded`, carrying the resolved changed-row
    /// count for this frame -- `ChangedRows::Rows(rows).len()`, or `0` for
    /// `ChangedRows::None` (a selection/hover/search-only bounded frame
    /// with no row-epoch change at all). See
    /// `super::super::terminal::frame_dirty::ChangedRows::bounded_row_count`,
    /// which the `widget.rs` call site uses to derive this without
    /// duplicating that match here.
    Bounded {
        /// The resolved changed-row count for this bounded frame.
        changed_row_count: usize,
    },
    /// Mirrors `VertexRebuild::ReevaluateFullRebuild`, carrying how the
    /// caller's own re-evaluation of the full-repaint trigger flags
    /// resolved.
    ReevaluateFullRebuild {
        /// Which way the re-evaluation resolved.
        resolved: ReevaluatedRebuild,
    },
}

/// How a [`RawRebuildDecision::ReevaluateFullRebuild`] resolved, once the
/// caller checked its own full-repaint trigger flags (Task 125.5).
///
/// A named two-state outcome, not a bare `bool`, per
/// `freminal-state-representation`: this is not an independent signal,
/// modifier set, or config toggle, so it does not qualify for that skill's
/// bool exemptions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReevaluatedRebuild {
    /// At least one full-repaint trigger fired: a full vertex rebuild ran
    /// this frame.
    Full,
    /// No full-repaint trigger fired: the previous frame's GPU buffers
    /// were reused as-is.
    Reuse,
}

/// Resolve a [`RawRebuildDecision`] into the [`RenderWorkClass`] the live
/// profiler records (Task 125.5).
///
/// Pure and total: every [`RawRebuildDecision`] variant maps to exactly
/// one [`RenderWorkClass`], with [`RawRebuildDecision::Bounded`]'s row
/// count run through [`ChangedRowBucket::from_count`] (which already
/// buckets `0` as [`ChangedRowBucket::Zero`], covering the
/// `ChangedRows::None` case described on that variant) and
/// [`RawRebuildDecision::ReevaluateFullRebuild`]'s resolution mapped
/// directly to [`RenderWorkClass::Full`] or [`RenderWorkClass::Reuse`].
#[must_use]
pub const fn resolve_render_work_class(raw: RawRebuildDecision) -> RenderWorkClass {
    match raw {
        RawRebuildDecision::CursorOnly => RenderWorkClass::CursorOnly,
        RawRebuildDecision::Bounded { changed_row_count } => {
            RenderWorkClass::Bounded(ChangedRowBucket::from_count(changed_row_count))
        }
        RawRebuildDecision::ReevaluateFullRebuild { resolved } => match resolved {
            ReevaluatedRebuild::Full => RenderWorkClass::Full,
            ReevaluatedRebuild::Reuse => RenderWorkClass::Reuse,
        },
    }
}

/// A histogram bucket for a pane's changed-row count on a
/// [`RenderWorkClass::Bounded`] frame (125.4).
///
/// Buckets, not raw counts: a live per-frame log line summarising 120
/// observations (the [`LiveRenderProfile::FLUSH_EVERY`] cadence) needs a
/// small, fixed set of bins rather than a distribution of arbitrary
/// integers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangedRowBucket {
    /// No rows changed (a bounded rebuild that resolved to zero rows --
    /// kept distinct from [`RenderWorkClass::Reuse`] because it still went
    /// through the bounded path, it just measured no changed rows).
    Zero,
    /// Exactly one changed row.
    One,
    /// 2 through 4 changed rows, inclusive.
    TwoToFour,
    /// 5 through 8 changed rows, inclusive.
    FiveToEight,
    /// 9 through 16 changed rows, inclusive.
    NineToSixteen,
    /// 17 through 32 changed rows, inclusive.
    SeventeenToThirtyTwo,
    /// 33 through 64 changed rows, inclusive.
    ThirtyThreeToSixtyFour,
    /// More than 64 changed rows.
    MoreThanSixtyFour,
}

impl ChangedRowBucket {
    /// Bucket a raw changed-row count.
    #[must_use]
    pub const fn from_count(count: usize) -> Self {
        match count {
            0 => Self::Zero,
            1 => Self::One,
            2..=4 => Self::TwoToFour,
            5..=8 => Self::FiveToEight,
            9..=16 => Self::NineToSixteen,
            17..=32 => Self::SeventeenToThirtyTwo,
            33..=64 => Self::ThirtyThreeToSixtyFour,
            _ => Self::MoreThanSixtyFour,
        }
    }
}

/// Cumulative occurrence counts for each [`ChangedRowBucket`] (125.4).
///
/// Only incremented for [`RenderWorkClass::Bounded`] completions -- see
/// [`LiveRenderProfile::complete`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ChangedRowHistogram {
    /// Occurrences of [`ChangedRowBucket::Zero`].
    pub zero: u64,
    /// Occurrences of [`ChangedRowBucket::One`].
    pub one: u64,
    /// Occurrences of [`ChangedRowBucket::TwoToFour`].
    pub two_to_four: u64,
    /// Occurrences of [`ChangedRowBucket::FiveToEight`].
    pub five_to_eight: u64,
    /// Occurrences of [`ChangedRowBucket::NineToSixteen`].
    pub nine_to_sixteen: u64,
    /// Occurrences of [`ChangedRowBucket::SeventeenToThirtyTwo`].
    pub seventeen_to_thirty_two: u64,
    /// Occurrences of [`ChangedRowBucket::ThirtyThreeToSixtyFour`].
    pub thirty_three_to_sixty_four: u64,
    /// Occurrences of [`ChangedRowBucket::MoreThanSixtyFour`].
    pub more_than_sixty_four: u64,
}

impl ChangedRowHistogram {
    /// Record one observed bucket.
    const fn record(&mut self, bucket: ChangedRowBucket) {
        let field = match bucket {
            ChangedRowBucket::Zero => &mut self.zero,
            ChangedRowBucket::One => &mut self.one,
            ChangedRowBucket::TwoToFour => &mut self.two_to_four,
            ChangedRowBucket::FiveToEight => &mut self.five_to_eight,
            ChangedRowBucket::NineToSixteen => &mut self.nine_to_sixteen,
            ChangedRowBucket::SeventeenToThirtyTwo => &mut self.seventeen_to_thirty_two,
            ChangedRowBucket::ThirtyThreeToSixtyFour => &mut self.thirty_three_to_sixty_four,
            ChangedRowBucket::MoreThanSixtyFour => &mut self.more_than_sixty_four,
        };
        *field = field.saturating_add(1);
    }

    /// Sum of every bucket -- equal to the number of
    /// [`RenderWorkClass::Bounded`] completions observed.
    #[must_use]
    pub const fn total(&self) -> u64 {
        self.zero
            .saturating_add(self.one)
            .saturating_add(self.two_to_four)
            .saturating_add(self.five_to_eight)
            .saturating_add(self.nine_to_sixteen)
            .saturating_add(self.seventeen_to_thirty_two)
            .saturating_add(self.thirty_three_to_sixty_four)
            .saturating_add(self.more_than_sixty_four)
    }
}

/// Cumulative occurrence counts for each [`RenderWorkClass`] (125.4).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RenderWorkClassCounts {
    /// [`RenderWorkClass::Reuse`] completions.
    pub reuse: u64,
    /// [`RenderWorkClass::CursorOnly`] completions.
    pub cursor_only: u64,
    /// [`RenderWorkClass::Bounded`] completions (any bucket).
    pub bounded: u64,
    /// [`RenderWorkClass::Full`] completions.
    pub full: u64,
}

impl RenderWorkClassCounts {
    /// Record one observed class (ignoring the [`ChangedRowBucket`] payload
    /// carried by [`RenderWorkClass::Bounded`] -- that goes to a separate
    /// [`ChangedRowHistogram`]).
    const fn record(&mut self, class: RenderWorkClass) {
        let field = match class {
            RenderWorkClass::Reuse => &mut self.reuse,
            RenderWorkClass::CursorOnly => &mut self.cursor_only,
            RenderWorkClass::Bounded(_) => &mut self.bounded,
            RenderWorkClass::Full => &mut self.full,
        };
        *field = field.saturating_add(1);
    }
}

/// Cumulative occurrence counts for each [`RawRebuildDecision`] (Task
/// 125.5).
///
/// Tracked alongside, but separately from, [`RenderWorkClassCounts`]: the
/// raw decision and the resolved class are two different questions about
/// the SAME painted completion (which branch `evaluate_frame_dirty_state`
/// selected, versus what render work that branch turned out to require),
/// and a live summary line reports both so a `Bounded` decision that
/// always resolves to a dense histogram bucket (say) is distinguishable
/// from one that does not.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RawRebuildDecisionCounts {
    /// [`RawRebuildDecision::CursorOnly`] completions.
    pub cursor_only: u64,
    /// [`RawRebuildDecision::Bounded`] completions (any row count).
    pub bounded: u64,
    /// [`RawRebuildDecision::ReevaluateFullRebuild`] completions (either
    /// resolution).
    pub reevaluate_full_rebuild: u64,
}

impl RawRebuildDecisionCounts {
    /// Record one observed raw decision.
    const fn record(&mut self, raw: RawRebuildDecision) {
        let field = match raw {
            RawRebuildDecision::CursorOnly => &mut self.cursor_only,
            RawRebuildDecision::Bounded { .. } => &mut self.bounded,
            RawRebuildDecision::ReevaluateFullRebuild { .. } => &mut self.reevaluate_full_rebuild,
        };
        *field = field.saturating_add(1);
    }
}

/// Per-buffer live upload-byte attribution for one pane's completed paint
/// callback (125.4).
///
/// 125.4 defines the type; 125.6 is the subtask that actually counts real
/// GL upload bytes into it at the call sites -- see that subtask's scope
/// note.
///
/// Every field is a separately-attributed GPU buffer/texture category so a
/// later flush line can show which class of upload dominates a workload,
/// rather than one opaque total. All-zero (via [`Default`]) is the correct
/// value for an unpainted or zero-upload observation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UploadByteCounts {
    /// Bytes uploaded to the instanced background VBO.
    pub background_instance_vbo_bytes: u64,
    /// Bytes uploaded to the instanced foreground (glyph) VBO.
    pub foreground_instance_vbo_bytes: u64,
    /// Bytes uploaded to the decoration (underline/strikethrough/box-drawing
    /// join) VBO.
    pub decoration_vbo_bytes: u64,
    /// Bytes uploaded to the inline-image vertex VBO.
    pub image_vertex_vbo_bytes: u64,
    /// Bytes uploaded to inline-image textures.
    pub image_texture_bytes: u64,
    /// Bytes uploaded as a full glyph-atlas texture upload.
    pub atlas_full_bytes: u64,
    /// Bytes uploaded as a glyph-atlas sub-rectangle (`glTexSubImage2D`)
    /// upload.
    pub atlas_subrect_bytes: u64,
}

impl UploadByteCounts {
    /// Sum of every category.
    #[must_use]
    pub const fn total(&self) -> u64 {
        self.background_instance_vbo_bytes
            .saturating_add(self.foreground_instance_vbo_bytes)
            .saturating_add(self.decoration_vbo_bytes)
            .saturating_add(self.image_vertex_vbo_bytes)
            .saturating_add(self.image_texture_bytes)
            .saturating_add(self.atlas_full_bytes)
            .saturating_add(self.atlas_subrect_bytes)
    }

    /// Merge another observation's counts into this one, category by
    /// category, saturating rather than overflowing.
    #[must_use]
    pub const fn merge(self, other: Self) -> Self {
        Self {
            background_instance_vbo_bytes: self
                .background_instance_vbo_bytes
                .saturating_add(other.background_instance_vbo_bytes),
            foreground_instance_vbo_bytes: self
                .foreground_instance_vbo_bytes
                .saturating_add(other.foreground_instance_vbo_bytes),
            decoration_vbo_bytes: self
                .decoration_vbo_bytes
                .saturating_add(other.decoration_vbo_bytes),
            image_vertex_vbo_bytes: self
                .image_vertex_vbo_bytes
                .saturating_add(other.image_vertex_vbo_bytes),
            image_texture_bytes: self
                .image_texture_bytes
                .saturating_add(other.image_texture_bytes),
            atlas_full_bytes: self.atlas_full_bytes.saturating_add(other.atlas_full_bytes),
            atlas_subrect_bytes: self
                .atlas_subrect_bytes
                .saturating_add(other.atlas_subrect_bytes),
        }
    }
}

/// Cumulative upload-byte totals attributed to each resolved
/// [`RenderWorkClass`] (Task 125.6).
///
/// Distinct from both [`UploadByteCounts`] (a per-buffer-category
/// breakdown for ONE observation) and [`RenderWorkClassCounts`]
/// (occurrence counts per class): this type answers "how many total
/// upload bytes did each resolved class account for", which -- divided by
/// the matching [`RenderWorkClassCounts`] field -- is what lets a later
/// reconciliation pass compute an average bytes-per-frame figure per
/// class. An unpainted completion attributes `0` bytes here (it issued no
/// GL upload at all, the same value its implicit zero-upload is already
/// worth), so this total always reconciles against the SAME
/// `painted_count + unpainted_count` observations [`RenderWorkClassCounts`]
/// does, not a painted-only subset.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RenderWorkClassUploadTotals {
    /// Total upload bytes across every [`RenderWorkClass::Reuse`] completion.
    pub reuse: u64,
    /// Total upload bytes across every [`RenderWorkClass::CursorOnly`]
    /// completion.
    pub cursor_only: u64,
    /// Total upload bytes across every [`RenderWorkClass::Bounded`]
    /// completion (any bucket).
    pub bounded: u64,
    /// Total upload bytes across every [`RenderWorkClass::Full`] completion.
    pub full: u64,
}

impl RenderWorkClassUploadTotals {
    /// Attribute `bytes` total upload bytes to `class`'s running total,
    /// saturating rather than overflowing.
    const fn record(&mut self, class: RenderWorkClass, bytes: u64) {
        let field = match class {
            RenderWorkClass::Reuse => &mut self.reuse,
            RenderWorkClass::CursorOnly => &mut self.cursor_only,
            RenderWorkClass::Bounded(_) => &mut self.bounded,
            RenderWorkClass::Full => &mut self.full,
        };
        *field = field.saturating_add(bytes);
    }
}

/// What became of one [`PaneFrameToken`] (125.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneFrameOutcome {
    /// The matching paint callback ran and reported a resolved render-work
    /// class plus its upload-byte attribution.
    Painted {
        /// The raw `VertexRebuild` decision this completion was resolved
        /// from (Task 125.5).
        raw: RawRebuildDecision,
        /// The resolved render-work classification.
        class: RenderWorkClass,
        /// The exact uploads issued by that paint callback.
        uploads: UploadByteCounts,
    },
    /// No paint callback ever completed this token before a newer
    /// [`LiveRenderProfile::start`] call superseded it (see the module doc
    /// for the `FrameDamage::None` case this represents). Implicitly
    /// zero-upload.
    ///
    /// Retains the raw decision and resolved class [`LiveRenderProfile::start`]
    /// was given for this token (Task 125.5 regression fix): a superseded
    /// observation is still a real CPU-side dirty-tracking decision that
    /// happened this frame, even though no GPU work for it was ever
    /// issued, and discarding its classification would make the
    /// cumulative raw/resolved totals silently under-count it.
    Unpainted {
        /// The raw `VertexRebuild` decision [`LiveRenderProfile::start`]
        /// was given for this token.
        raw: RawRebuildDecision,
        /// The resolved render-work classification
        /// [`LiveRenderProfile::start`] was given for this token.
        class: RenderWorkClass,
    },
}

/// One finalized observation in [`LiveRenderProfile`]'s bounded queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaneFrameRecord {
    /// The token this record finalizes.
    pub token: PaneFrameToken,
    /// What became of it.
    pub outcome: PaneFrameOutcome,
}

/// The raw decision and resolved class captured by
/// [`LiveRenderProfile::start`] for the currently outstanding token (Task
/// 125.5).
///
/// Retained so that if the token is later superseded rather than
/// completed, its classification survives into the finalized
/// [`PaneFrameOutcome::Unpainted`] record instead of being discarded --
/// see the module doc's regression-fix note. Entirely private:
/// [`LiveRenderProfile::complete`] receives back only the token and the
/// upload counts, using this stored classification rather than a second
/// copy supplied by the caller, so a paint callback's arguments can never
/// disagree with what `start` recorded for the same token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PendingObservation {
    /// The outstanding token this observation belongs to.
    token: PaneFrameToken,
    /// The raw decision supplied to `start`.
    raw: RawRebuildDecision,
    /// The resolved class supplied to `start`.
    class: RenderWorkClass,
}

/// Live, feature-gated render-work profiler (Task 125.4).
///
/// Owns exactly one outstanding [`PaneFrameToken`] at a time (see the
/// module doc for the start/complete state machine), a bounded queue of the
/// most recently finalized [`PaneFrameRecord`]s, and cumulative
/// class/histogram/upload totals since creation -- the same
/// "cumulative-since-creation" idiom as
/// `freminal::gui::window::FrameStats` and
/// `freminal_windowing::egui_integration::FrameProfile`, which this type is
/// deliberately modeled on rather than sharing code with (see those types'
/// docs for why that duplication is accepted).
///
/// This subtask (125.4) defines and tests the state machine only. No
/// renderer or widget call site constructs or drives one of these yet --
/// that is 125.5 (which will own one `LiveRenderProfile` per pane and call
/// [`Self::start`] from `show()` and [`Self::complete`] from the pane's
/// paint callback) and 125.6 (which feeds real upload byte counts into
/// [`Self::complete`]).
#[derive(Debug, Default)]
pub struct LiveRenderProfile {
    /// The next token [`Self::start`] will hand out.
    next_token: u64,
    /// The currently outstanding observation, if one has been started but
    /// not yet finalized (by either a matching [`Self::complete`] or a
    /// superseding [`Self::start`]). Carries the raw/resolved
    /// classification alongside the token (Task 125.5 regression fix) so
    /// a superseded observation can still be finalized with its
    /// classification intact.
    pending: Option<PendingObservation>,
    /// The most recently finalized records, bounded to
    /// [`Self::RECORD_QUEUE_CAPACITY`] entries (oldest evicted first).
    records: VecDeque<PaneFrameRecord>,
    /// Cumulative [`RawRebuildDecision`] occurrence counts since creation
    /// (Task 125.5).
    raw_counts: RawRebuildDecisionCounts,
    /// Cumulative [`RenderWorkClass`] occurrence counts since creation.
    class_counts: RenderWorkClassCounts,
    /// Cumulative [`ChangedRowBucket`] occurrence counts since creation
    /// (only [`RenderWorkClass::Bounded`] completions contribute).
    row_bucket_counts: ChangedRowHistogram,
    /// Cumulative upload-byte totals since creation.
    upload_totals: UploadByteCounts,
    /// Cumulative upload-byte totals since creation, attributed to each
    /// resolved [`RenderWorkClass`] (Task 125.6).
    class_upload_totals: RenderWorkClassUploadTotals,
    /// Painted completions since creation.
    painted_count: u64,
    /// Unpainted finalizations since creation.
    unpainted_count: u64,
    /// Total finalized observations since creation (`painted_count +
    /// unpainted_count`).
    observation_count: u64,
    /// Set when a finalization (by either [`Self::complete`] or the
    /// stale-token sweep inside [`Self::start`]) causes
    /// [`Self::observation_count`] to cross a [`Self::FLUSH_EVERY`]
    /// boundary, and cleared by [`Self::take_flush_signal`] (Task 125.5
    /// regression fix). This value never leaves `LiveRenderProfile` --
    /// callers only ever observe it through the one-shot
    /// [`Self::take_flush_signal`] accessor, the same shape as the
    /// `pending` field's own internal-only role.
    flush_signal: bool,
}

impl LiveRenderProfile {
    /// Emit a flush-worthy summary once every this many finalized
    /// observations -- the same cadence as
    /// `freminal::gui::window::FrameStats::FLUSH_EVERY` and
    /// `freminal_windowing::egui_integration::FrameProfile::FLUSH_EVERY`,
    /// so a live session's log lines stay easy to correlate by eye. This
    /// module only exposes [`Self::take_flush_signal`] as the consumable
    /// signal; emitting an actual `tracing::debug!` line is the
    /// `widget.rs` call site's responsibility.
    pub const FLUSH_EVERY: u64 = 120;

    /// Bound on [`Self::records`]'s length, so the queue always holds at
    /// least one full flush window's worth of observations without growing
    /// unbounded across a long-lived session.
    ///
    /// Deliberately a separate literal rather than cast from
    /// [`Self::FLUSH_EVERY`] (a `u64 -> usize` conversion is not lossless
    /// on every target) -- kept equal to it by eye, the same duplication
    /// already accepted for `mean_duration` between
    /// `freminal::gui::window::FrameStats` and
    /// `freminal_windowing::egui_integration::FrameProfile`.
    const RECORD_QUEUE_CAPACITY: usize = 120;

    /// Explicit `tracing` target for the periodic flush summary the
    /// `widget.rs` call site logs under (Task 125.5), kept distinct from
    /// the pre-existing `"freminal::frame_profiling"` target (Task 121's
    /// app-level GUI-thread duty-cycle stats,
    /// `freminal::gui::window::FrameStats`) and
    /// `"freminal_windowing::frame_profiling"` (the windowing crate's
    /// per-window phase-timing stats) so a live session can filter this
    /// pane-level render-work signal independently of either.
    pub const LOG_TARGET: &str = "freminal::task_125::live_render_profile";

    /// Construct a fresh profile with no outstanding token and all-zero
    /// cumulative totals.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Begin a new pane-frame observation, with the raw decision and
    /// resolved class already known (Task 125.5).
    ///
    /// If a previously started token is still outstanding (its matching
    /// [`Self::complete`] never arrived before this call), it is finalized
    /// first as [`PaneFrameOutcome::Unpainted`], carrying forward the
    /// raw/class IT was started with -- see the module doc for why this is
    /// the `FrameDamage::None` case, and why the classification travels
    /// with the token rather than being supplied again at completion
    /// time.
    pub fn start(&mut self, raw: RawRebuildDecision, class: RenderWorkClass) -> PaneFrameToken {
        if let Some(stale) = self.pending.take() {
            self.finalize_unpainted(stale);
        }
        let token = PaneFrameToken(self.next_token);
        // Saturating, not wrapping: a wrapped counter could eventually
        // reissue a token value already present in the (much smaller)
        // bounded queue, reintroducing exactly the ambiguity this type
        // exists to remove. Saturating at `u64::MAX` instead simply stops
        // minting new tokens, which is unreachable in any real session's
        // lifetime.
        self.next_token = self.next_token.saturating_add(1);
        self.pending = Some(PendingObservation { token, raw, class });
        token
    }

    /// Finalize the outstanding token as painted, with its upload
    /// attribution.
    ///
    /// Returns `true` when `token` was the currently outstanding one and
    /// this call finalized it. Returns `false` -- taking no action at all
    /// -- when `token` does not match the outstanding one: a late
    /// completion (superseded by a newer [`Self::start`]), a duplicate
    /// completion (already finalized), or an out-of-order completion (a
    /// token that was never the outstanding one) are all rejected the same
    /// way. See the module doc and this file's tests for one scenario
    /// each.
    ///
    /// Takes back only the token and `uploads` -- not the raw decision or
    /// resolved class, both of which [`Self::start`] already recorded for
    /// this exact token (Task 125.5 regression fix). A callback cannot
    /// therefore report a classification that disagrees with what `start`
    /// was given.
    pub fn complete(&mut self, token: PaneFrameToken, uploads: UploadByteCounts) -> bool {
        let Some(pending) = self.pending else {
            return false;
        };
        if pending.token != token {
            return false;
        }
        self.pending = None;
        self.record_classification(pending.raw, pending.class);
        self.upload_totals = self.upload_totals.merge(uploads);
        self.class_upload_totals
            .record(pending.class, uploads.total());
        self.painted_count = self.painted_count.saturating_add(1);
        self.note_observation();
        self.push_record(PaneFrameRecord {
            token,
            outcome: PaneFrameOutcome::Painted {
                raw: pending.raw,
                class: pending.class,
                uploads,
            },
        });
        true
    }

    /// Finalize a pending observation as unpainted (zero-upload),
    /// retaining its raw/resolved classification, from either
    /// [`Self::start`] superseding it or (in a future subtask) an explicit
    /// end-of-frame sweep for a token that egui never painted.
    fn finalize_unpainted(&mut self, pending: PendingObservation) {
        self.record_classification(pending.raw, pending.class);
        // Explicit zero-byte attribution (Task 125.6), not a silent skip:
        // an unpainted observation issued no GL upload at all, so it must
        // reconcile against `class_upload_totals` at the same granularity
        // `class_counts` already does -- see `RenderWorkClassUploadTotals`'s
        // doc for why this is a real (if numerically no-op) contribution,
        // not a gap in the accounting.
        self.class_upload_totals.record(pending.class, 0);
        self.unpainted_count = self.unpainted_count.saturating_add(1);
        self.note_observation();
        self.push_record(PaneFrameRecord {
            token: pending.token,
            outcome: PaneFrameOutcome::Unpainted {
                raw: pending.raw,
                class: pending.class,
            },
        });
    }

    /// Record one observation's classification into the cumulative
    /// [`Self::raw_counts`] / [`Self::class_counts`] / [`Self::row_bucket_counts`]
    /// totals, shared by [`Self::complete`] and [`Self::finalize_unpainted`]
    /// so painted and unpainted observations reconcile against the SAME
    /// totals (Task 125.5 regression fix) rather than a painted-only
    /// subset.
    const fn record_classification(&mut self, raw: RawRebuildDecision, class: RenderWorkClass) {
        self.raw_counts.record(raw);
        self.class_counts.record(class);
        if let RenderWorkClass::Bounded(bucket) = class {
            self.row_bucket_counts.record(bucket);
        }
    }

    /// Increment [`Self::observation_count`] for one finalized observation
    /// (painted or unpainted) and latch [`Self::flush_signal`] if this
    /// observation crosses a [`Self::FLUSH_EVERY`] boundary. Shared by
    /// [`Self::complete`] and [`Self::finalize_unpainted`] so either
    /// finalizer can trigger the same flush signal (Task 125.5 regression
    /// fix).
    const fn note_observation(&mut self) {
        self.observation_count = self.observation_count.saturating_add(1);
        if self.observation_count.is_multiple_of(Self::FLUSH_EVERY) {
            self.flush_signal = true;
        }
    }

    /// Push a finalized record, evicting the oldest entry first if the
    /// queue is already at [`Self::RECORD_QUEUE_CAPACITY`].
    fn push_record(&mut self, record: PaneFrameRecord) {
        if self.records.len() >= Self::RECORD_QUEUE_CAPACITY {
            self.records.pop_front();
        }
        self.records.push_back(record);
    }

    /// The currently outstanding token, if any (i.e. a [`Self::start`]
    /// whose matching [`Self::complete`] has not yet arrived and has not
    /// been superseded by a later [`Self::start`]).
    #[must_use]
    pub const fn pending(&self) -> Option<PaneFrameToken> {
        match self.pending {
            Some(p) => Some(p.token),
            None => None,
        }
    }

    /// The most recently finalized records, oldest first, bounded to
    /// [`Self::RECORD_QUEUE_CAPACITY`] entries.
    pub fn records(&self) -> impl DoubleEndedIterator<Item = &PaneFrameRecord> + '_ {
        self.records.iter()
    }

    /// Cumulative [`RawRebuildDecision`] occurrence counts since creation
    /// (Task 125.5).
    #[must_use]
    pub const fn raw_counts(&self) -> RawRebuildDecisionCounts {
        self.raw_counts
    }

    /// Cumulative [`RenderWorkClass`] occurrence counts since creation.
    #[must_use]
    pub const fn class_counts(&self) -> RenderWorkClassCounts {
        self.class_counts
    }

    /// Cumulative [`ChangedRowBucket`] occurrence counts since creation.
    #[must_use]
    pub const fn row_bucket_counts(&self) -> ChangedRowHistogram {
        self.row_bucket_counts
    }

    /// Cumulative upload-byte totals since creation.
    #[must_use]
    pub const fn upload_totals(&self) -> UploadByteCounts {
        self.upload_totals
    }

    /// Cumulative upload-byte totals since creation, attributed to each
    /// resolved [`RenderWorkClass`] (Task 125.6).
    #[must_use]
    pub const fn class_upload_totals(&self) -> RenderWorkClassUploadTotals {
        self.class_upload_totals
    }

    /// Painted completions since creation.
    #[must_use]
    pub const fn painted_count(&self) -> u64 {
        self.painted_count
    }

    /// Unpainted finalizations since creation.
    #[must_use]
    pub const fn unpainted_count(&self) -> u64 {
        self.unpainted_count
    }

    /// Total finalized observations since creation (`painted_count +
    /// unpainted_count`, saturating).
    #[must_use]
    pub const fn observation_count(&self) -> u64 {
        self.observation_count
    }

    /// Consume the pending flush signal, if one is set (Task 125.5
    /// regression fix).
    ///
    /// Returns `true` at most once per crossed [`Self::FLUSH_EVERY`]
    /// boundary: whichever finalizer crossed it -- [`Self::complete`] or
    /// the stale-token sweep inside [`Self::start`] -- latches
    /// [`Self::flush_signal`], and whichever caller invokes this method
    /// first afterward observes `true` and clears it; every other caller
    /// (including a second call from the SAME caller) observes `false`
    /// until the next boundary is crossed. This is why the signal is a
    /// stateful, one-shot `bool` rather than the earlier stateless
    /// `is_flush_due` predicate (`observation_count.is_multiple_of(...)`,
    /// recomputed fresh on every call): a predicate recomputed after the
    /// fact cannot tell "this boundary was already reported" apart from
    /// "this boundary is still due", and it can only ever be checked by a
    /// caller that happens to run again -- which the `start`-side sweep's
    /// caller (`show()`) previously never did, permanently losing any
    /// boundary crossed there. See the module doc's "Why a token, not a
    /// plain counter" section.
    #[must_use]
    pub fn take_flush_signal(&mut self) -> bool {
        std::mem::take(&mut self.flush_signal)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::{
        ChangedRowBucket, LiveRenderProfile, PaneFrameOutcome, RawRebuildDecision,
        ReevaluatedRebuild, RenderWorkClass, RenderWorkClassUploadTotals, UploadByteCounts,
        resolve_render_work_class,
    };

    fn sample_uploads(total: u64) -> UploadByteCounts {
        UploadByteCounts {
            foreground_instance_vbo_bytes: total,
            ..UploadByteCounts::default()
        }
    }

    /// `RawRebuildDecision::ReevaluateFullRebuild { resolved: Full }`, for
    /// tests that only care about a `RenderWorkClass::Full` completion and
    /// not about the raw/resolved mapping itself.
    const fn full_raw() -> RawRebuildDecision {
        RawRebuildDecision::ReevaluateFullRebuild {
            resolved: ReevaluatedRebuild::Full,
        }
    }

    /// `RawRebuildDecision::ReevaluateFullRebuild { resolved: Reuse }`, for
    /// tests that only care about a `RenderWorkClass::Reuse` completion.
    const fn reuse_raw() -> RawRebuildDecision {
        RawRebuildDecision::ReevaluateFullRebuild {
            resolved: ReevaluatedRebuild::Reuse,
        }
    }

    // ── ChangedRowBucket::from_count boundaries ────────────────────────

    #[test]
    fn changed_row_bucket_boundaries() {
        assert_eq!(ChangedRowBucket::from_count(0), ChangedRowBucket::Zero);
        assert_eq!(ChangedRowBucket::from_count(1), ChangedRowBucket::One);
        assert_eq!(ChangedRowBucket::from_count(2), ChangedRowBucket::TwoToFour);
        assert_eq!(ChangedRowBucket::from_count(4), ChangedRowBucket::TwoToFour);
        assert_eq!(
            ChangedRowBucket::from_count(5),
            ChangedRowBucket::FiveToEight
        );
        assert_eq!(
            ChangedRowBucket::from_count(8),
            ChangedRowBucket::FiveToEight
        );
        assert_eq!(
            ChangedRowBucket::from_count(9),
            ChangedRowBucket::NineToSixteen
        );
        assert_eq!(
            ChangedRowBucket::from_count(16),
            ChangedRowBucket::NineToSixteen
        );
        assert_eq!(
            ChangedRowBucket::from_count(17),
            ChangedRowBucket::SeventeenToThirtyTwo
        );
        assert_eq!(
            ChangedRowBucket::from_count(32),
            ChangedRowBucket::SeventeenToThirtyTwo
        );
        assert_eq!(
            ChangedRowBucket::from_count(33),
            ChangedRowBucket::ThirtyThreeToSixtyFour
        );
        assert_eq!(
            ChangedRowBucket::from_count(64),
            ChangedRowBucket::ThirtyThreeToSixtyFour
        );
        assert_eq!(
            ChangedRowBucket::from_count(65),
            ChangedRowBucket::MoreThanSixtyFour
        );
        assert_eq!(
            ChangedRowBucket::from_count(1_000_000),
            ChangedRowBucket::MoreThanSixtyFour
        );
    }

    // ── UploadByteCounts ────────────────────────────────────────────────

    #[test]
    fn upload_byte_counts_total_sums_every_category() {
        let counts = UploadByteCounts {
            background_instance_vbo_bytes: 1,
            foreground_instance_vbo_bytes: 2,
            decoration_vbo_bytes: 3,
            image_vertex_vbo_bytes: 4,
            image_texture_bytes: 5,
            atlas_full_bytes: 6,
            atlas_subrect_bytes: 7,
        };
        assert_eq!(counts.total(), 28);
    }

    #[test]
    fn upload_byte_counts_merge_adds_each_category() {
        let a = sample_uploads(10);
        let b = UploadByteCounts {
            atlas_subrect_bytes: 5,
            ..UploadByteCounts::default()
        };
        let merged = a.merge(b);
        assert_eq!(merged.foreground_instance_vbo_bytes, 10);
        assert_eq!(merged.atlas_subrect_bytes, 5);
        assert_eq!(merged.total(), 15);
    }

    #[test]
    fn upload_byte_counts_default_is_zero() {
        assert_eq!(UploadByteCounts::default().total(), 0);
    }

    // ── LiveRenderProfile state machine ────────────────────────────────

    #[test]
    fn painted_completion_records_class_histogram_and_uploads() {
        let mut profile = LiveRenderProfile::new();
        let raw = RawRebuildDecision::Bounded {
            changed_row_count: 3,
        };
        let class = RenderWorkClass::Bounded(ChangedRowBucket::from_count(3));
        let token = profile.start(raw, class);
        assert_eq!(profile.pending(), Some(token));

        let uploads = sample_uploads(42);
        let finalized = profile.complete(token, uploads);

        assert!(finalized);
        assert_eq!(profile.pending(), None);
        assert_eq!(profile.painted_count(), 1);
        assert_eq!(profile.unpainted_count(), 0);
        assert_eq!(profile.observation_count(), 1);
        assert_eq!(profile.raw_counts().bounded, 1);
        assert_eq!(profile.raw_counts().cursor_only, 0);
        assert_eq!(profile.class_counts().bounded, 1);
        assert_eq!(profile.class_counts().reuse, 0);
        assert_eq!(profile.row_bucket_counts().two_to_four, 1);
        assert_eq!(profile.upload_totals().total(), 42);

        let records: Vec<_> = profile.records().collect();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].token, token);
        match records[0].outcome {
            PaneFrameOutcome::Painted {
                raw: recorded_raw,
                class: recorded_class,
                uploads: recorded_uploads,
            } => {
                assert_eq!(recorded_raw, raw);
                assert_eq!(recorded_class, class);
                assert_eq!(recorded_uploads.total(), 42);
            }
            PaneFrameOutcome::Unpainted { .. } => panic!("expected a painted outcome"),
        }
    }

    #[test]
    fn starting_a_new_token_finalizes_the_stale_one_as_unpainted() {
        let mut profile = LiveRenderProfile::new();
        let raw = reuse_raw();
        let class = RenderWorkClass::Reuse;
        let first = profile.start(raw, class);
        // The first token's matching `complete` never arrives -- the next
        // `show()` call starts a new token before it does, exactly the
        // `FrameDamage::None` case from the module doc.
        let second = profile.start(RawRebuildDecision::CursorOnly, RenderWorkClass::CursorOnly);

        assert_eq!(profile.pending(), Some(second));
        assert_eq!(profile.unpainted_count(), 1);
        assert_eq!(profile.painted_count(), 0);
        assert_eq!(profile.observation_count(), 1);

        let records: Vec<_> = profile.records().collect();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].token, first);
        assert_eq!(
            records[0].outcome,
            PaneFrameOutcome::Unpainted { raw, class }
        );
    }

    /// Regression test (Task 125.5 review): an unpainted `Reuse`
    /// observation -- superseded by the next `start()` before any
    /// `complete()` arrived -- must still contribute to the cumulative
    /// raw/resolved counts, and its upload contribution must be zero.
    /// Under the pre-fix implementation, `start` stored only the token,
    /// so this classification was silently discarded on supersession.
    #[test]
    fn unpainted_reuse_observation_contributes_raw_and_resolved_counts_with_zero_upload() {
        let mut profile = LiveRenderProfile::new();
        let raw = reuse_raw();
        let class = RenderWorkClass::Reuse;
        let stale = profile.start(raw, class);
        let _next = profile.start(RawRebuildDecision::CursorOnly, RenderWorkClass::CursorOnly);

        assert_eq!(profile.unpainted_count(), 1);
        assert_eq!(profile.painted_count(), 0);
        assert_eq!(profile.observation_count(), 1);
        assert_eq!(profile.raw_counts().reevaluate_full_rebuild, 1);
        assert_eq!(profile.class_counts().reuse, 1);
        assert_eq!(profile.upload_totals().total(), 0);

        let records: Vec<_> = profile.records().collect();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].token, stale);
        assert_eq!(
            records[0].outcome,
            PaneFrameOutcome::Unpainted { raw, class }
        );
    }

    /// Regression test (Task 125.5 review): an unpainted `Bounded`
    /// observation must still contribute its changed-row bucket to
    /// [`ChangedRowHistogram`] -- not just to `class_counts().bounded`.
    #[test]
    fn unpainted_bounded_observation_contributes_its_row_bucket() {
        let mut profile = LiveRenderProfile::new();
        let raw = RawRebuildDecision::Bounded {
            changed_row_count: 10,
        };
        let class = resolve_render_work_class(raw);
        assert_eq!(
            class,
            RenderWorkClass::Bounded(ChangedRowBucket::NineToSixteen)
        );
        let stale = profile.start(raw, class);
        let _next = profile.start(RawRebuildDecision::CursorOnly, RenderWorkClass::CursorOnly);

        assert_eq!(profile.unpainted_count(), 1);
        assert_eq!(profile.raw_counts().bounded, 1);
        assert_eq!(profile.class_counts().bounded, 1);
        assert_eq!(profile.row_bucket_counts().nine_to_sixteen, 1);
        assert_eq!(profile.row_bucket_counts().total(), 1);

        let records: Vec<_> = profile.records().collect();
        assert_eq!(records[0].token, stale);
        assert_eq!(
            records[0].outcome,
            PaneFrameOutcome::Unpainted { raw, class }
        );
    }

    #[test]
    fn late_completion_after_a_newer_token_started_is_rejected() {
        let mut profile = LiveRenderProfile::new();
        let first = profile.start(reuse_raw(), RenderWorkClass::Reuse);
        let _second = profile.start(RawRebuildDecision::CursorOnly, RenderWorkClass::CursorOnly); // finalizes `first` as unpainted

        let finalized = profile.complete(first, sample_uploads(99));

        assert!(!finalized);
        // No change to totals: the late completion for `first` must not
        // retroactively convert its already-finalized unpainted record.
        assert_eq!(profile.painted_count(), 0);
        assert_eq!(profile.unpainted_count(), 1);
        assert_eq!(profile.upload_totals().total(), 0);
        assert_eq!(profile.records().count(), 1);
    }

    #[test]
    fn duplicate_completion_of_the_same_token_is_rejected() {
        let mut profile = LiveRenderProfile::new();
        let token = profile.start(RawRebuildDecision::CursorOnly, RenderWorkClass::CursorOnly);
        let first_complete = profile.complete(token, sample_uploads(7));
        assert!(first_complete);

        let second_complete = profile.complete(token, sample_uploads(500));

        assert!(!second_complete);
        // Only the first completion's data is recorded.
        assert_eq!(profile.painted_count(), 1);
        assert_eq!(profile.upload_totals().total(), 7);
        assert_eq!(profile.raw_counts().cursor_only, 1);
        assert_eq!(profile.raw_counts().reevaluate_full_rebuild, 0);
        assert_eq!(profile.class_counts().cursor_only, 1);
        assert_eq!(profile.class_counts().full, 0);
        assert_eq!(profile.records().count(), 1);
    }

    #[test]
    fn out_of_order_completion_for_a_never_pending_token_is_rejected() {
        let mut profile = LiveRenderProfile::new();
        let first = profile.start(reuse_raw(), RenderWorkClass::Reuse);
        let second = profile.start(reuse_raw(), RenderWorkClass::Reuse); // finalizes `first` as unpainted
        let completed_second = profile.complete(second, sample_uploads(0));
        assert!(completed_second);

        // Fabricate a token value that was never actually issued by
        // `start` (one past the highest issued so far) and attempt to
        // complete it. This can never happen causally through the public
        // `start`/`complete` API, but the state machine must still reject
        // it defensively rather than panicking or corrupting totals.
        let never_issued = super::PaneFrameToken(first.value().max(second.value()) + 1);
        let finalized = profile.complete(never_issued, sample_uploads(3));

        assert!(!finalized);
        assert_eq!(profile.painted_count(), 1);
        assert_eq!(profile.unpainted_count(), 1);
        assert_eq!(profile.upload_totals().total(), 0);
        assert_eq!(profile.records().count(), 2);
    }

    #[test]
    fn bounded_records_do_not_finalize_unrelated_reuse_or_full_buckets() {
        let mut profile = LiveRenderProfile::new();
        let token = profile.start(reuse_raw(), RenderWorkClass::Reuse);
        profile.complete(token, UploadByteCounts::default());
        assert_eq!(profile.row_bucket_counts().total(), 0);
        assert_eq!(profile.class_counts().reuse, 1);
    }

    #[test]
    fn record_queue_is_bounded_and_drops_the_oldest_first() {
        let mut profile = LiveRenderProfile::new();
        let capacity = LiveRenderProfile::RECORD_QUEUE_CAPACITY;
        let mut last_token = None;
        for _ in 0..(capacity + 10) {
            let token = profile.start(reuse_raw(), RenderWorkClass::Reuse);
            profile.complete(token, UploadByteCounts::default());
            last_token = Some(token);
        }
        assert_eq!(profile.records().count(), capacity);
        // The most recent completion must still be present -- it is the
        // oldest entries that get evicted, not the newest.
        let last_recorded = profile
            .records()
            .next_back()
            .expect("queue is non-empty")
            .token;
        assert_eq!(Some(last_recorded), last_token);
    }

    #[test]
    fn flush_signal_fires_once_per_flush_every_boundary_and_is_consumed() {
        let mut profile = LiveRenderProfile::new();
        assert!(!profile.take_flush_signal());

        for i in 1..=(LiveRenderProfile::FLUSH_EVERY * 2) {
            let token = profile.start(reuse_raw(), RenderWorkClass::Reuse);
            profile.complete(token, UploadByteCounts::default());
            let expected = i.is_multiple_of(LiveRenderProfile::FLUSH_EVERY);
            assert_eq!(
                profile.take_flush_signal(),
                expected,
                "observation {i} flush-signal mismatch"
            );
            // The signal is one-shot: consuming it again immediately,
            // with no new observation in between, must report `false`
            // even on a boundary observation.
            assert!(
                !profile.take_flush_signal(),
                "observation {i}: flush signal must not fire twice for the same boundary"
            );
        }
    }

    /// Regression test (Task 125.5 review): the 120th observation
    /// finalized by a superseding `start()` call (the stale-token sweep,
    /// not `complete()`) must still produce exactly one consumable flush
    /// signal, even when a LATER observation completes normally before
    /// anyone checks for it. Under the pre-fix stateless `is_flush_due`
    /// predicate, this boundary was permanently lost: nothing ever
    /// checked it at the moment `start()` crossed it, and by the time the
    /// next `complete()` ran, `observation_count` had already advanced
    /// past the exact multiple.
    #[test]
    fn a_boundary_crossed_by_a_stale_token_sweep_inside_start_produces_exactly_one_flush_signal() {
        let mut profile = LiveRenderProfile::new();
        for _ in 0..(LiveRenderProfile::FLUSH_EVERY - 1) {
            let token = profile.start(reuse_raw(), RenderWorkClass::Reuse);
            profile.complete(token, UploadByteCounts::default());
        }
        assert_eq!(
            profile.observation_count(),
            LiveRenderProfile::FLUSH_EVERY - 1
        );
        assert!(!profile.take_flush_signal());

        // The 120th observation is finalized by a SUPERSEDING `start()`
        // call (a stale token becoming unpainted), not by `complete()`.
        let _stale_120th = profile.start(reuse_raw(), RenderWorkClass::Reuse);
        let next_token = profile.start(reuse_raw(), RenderWorkClass::Reuse);
        assert_eq!(profile.observation_count(), LiveRenderProfile::FLUSH_EVERY);

        // A LATER observation completes normally before anyone has
        // consumed the signal yet -- the signal must survive this.
        profile.complete(next_token, UploadByteCounts::default());
        assert_eq!(
            profile.observation_count(),
            LiveRenderProfile::FLUSH_EVERY + 1
        );

        assert!(
            profile.take_flush_signal(),
            "the 120-observation boundary crossed by the stale-token sweep \
             inside `start` must still be observable after a LATER `complete`"
        );
        assert!(
            !profile.take_flush_signal(),
            "the signal must be consumed -- exactly one flush per boundary"
        );
    }

    #[test]
    fn default_profile_has_no_pending_token_and_zero_totals() {
        let profile = LiveRenderProfile::new();
        assert_eq!(profile.pending(), None);
        assert_eq!(profile.painted_count(), 0);
        assert_eq!(profile.unpainted_count(), 0);
        assert_eq!(profile.observation_count(), 0);
        assert_eq!(profile.records().count(), 0);
        assert_eq!(profile.upload_totals().total(), 0);
    }

    // ── resolve_render_work_class: every raw-to-resolved mapping ───────
    //
    // One test per `RawRebuildDecision` variant/payload combination Task
    // 125.5 introduces, pinning the exact mapping `widget.rs`'s call site
    // relies on rather than re-deriving it inline.

    #[test]
    fn resolve_cursor_only_maps_to_cursor_only() {
        assert_eq!(
            resolve_render_work_class(RawRebuildDecision::CursorOnly),
            RenderWorkClass::CursorOnly
        );
    }

    #[test]
    fn resolve_bounded_with_rows_maps_to_the_matching_bucket() {
        assert_eq!(
            resolve_render_work_class(RawRebuildDecision::Bounded {
                changed_row_count: 3,
            }),
            RenderWorkClass::Bounded(ChangedRowBucket::TwoToFour)
        );
        assert_eq!(
            resolve_render_work_class(RawRebuildDecision::Bounded {
                changed_row_count: 65,
            }),
            RenderWorkClass::Bounded(ChangedRowBucket::MoreThanSixtyFour)
        );
    }

    /// `ChangedRows::None` (a selection/hover/search-only bounded frame
    /// with no row-epoch change at all) is represented at this call
    /// boundary as `changed_row_count: 0` -- see
    /// `super::super::terminal::frame_dirty::ChangedRows::bounded_row_count`
    /// -- and must resolve to `ChangedRowBucket::Zero`, kept distinct from
    /// `RenderWorkClass::Reuse` because it still took the bounded path.
    #[test]
    fn resolve_bounded_with_no_changed_rows_maps_to_the_zero_bucket() {
        assert_eq!(
            resolve_render_work_class(RawRebuildDecision::Bounded {
                changed_row_count: 0,
            }),
            RenderWorkClass::Bounded(ChangedRowBucket::Zero)
        );
    }

    #[test]
    fn resolve_reevaluated_full_maps_to_full() {
        assert_eq!(resolve_render_work_class(full_raw()), RenderWorkClass::Full);
    }

    #[test]
    fn resolve_reevaluated_reuse_maps_to_reuse() {
        assert_eq!(
            resolve_render_work_class(reuse_raw()),
            RenderWorkClass::Reuse
        );
    }

    // ── Raw decision counts, tracked alongside the resolved class ──────

    #[test]
    fn raw_and_resolved_counts_are_both_recorded_on_completion() {
        let mut profile = LiveRenderProfile::new();

        let token = profile.start(
            RawRebuildDecision::CursorOnly,
            resolve_render_work_class(RawRebuildDecision::CursorOnly),
        );
        profile.complete(token, UploadByteCounts::default());

        let raw = RawRebuildDecision::Bounded {
            changed_row_count: 0,
        };
        let token = profile.start(raw, resolve_render_work_class(raw));
        profile.complete(token, UploadByteCounts::default());

        let token = profile.start(full_raw(), resolve_render_work_class(full_raw()));
        profile.complete(token, UploadByteCounts::default());

        let token = profile.start(reuse_raw(), resolve_render_work_class(reuse_raw()));
        profile.complete(token, UploadByteCounts::default());

        assert_eq!(profile.raw_counts().cursor_only, 1);
        assert_eq!(profile.raw_counts().bounded, 1);
        // Both `full_raw()` and `reuse_raw()` are
        // `RawRebuildDecision::ReevaluateFullRebuild`, just with different
        // `resolved` payloads -- the raw counter does not distinguish
        // them (that distinction lives entirely in `class_counts`).
        assert_eq!(profile.raw_counts().reevaluate_full_rebuild, 2);
        assert_eq!(profile.class_counts().cursor_only, 1);
        assert_eq!(profile.class_counts().bounded, 1);
        assert_eq!(profile.class_counts().full, 1);
        assert_eq!(profile.class_counts().reuse, 1);
        assert_eq!(profile.row_bucket_counts().zero, 1);
    }

    // ── RenderWorkClassUploadTotals (Task 125.6) ───────────────────────

    #[test]
    fn render_work_class_upload_totals_default_is_zero() {
        let totals = RenderWorkClassUploadTotals::default();
        assert_eq!(totals.reuse, 0);
        assert_eq!(totals.cursor_only, 0);
        assert_eq!(totals.bounded, 0);
        assert_eq!(totals.full, 0);
    }

    #[test]
    fn a_painted_completion_attributes_its_total_bytes_to_the_resolved_class() {
        let mut profile = LiveRenderProfile::new();

        let token = profile.start(RawRebuildDecision::CursorOnly, RenderWorkClass::CursorOnly);
        profile.complete(token, sample_uploads(42));

        let raw = RawRebuildDecision::Bounded {
            changed_row_count: 3,
        };
        let class = resolve_render_work_class(raw);
        let token = profile.start(raw, class);
        profile.complete(token, sample_uploads(7));

        let token = profile.start(full_raw(), RenderWorkClass::Full);
        profile.complete(token, sample_uploads(100));

        let totals = profile.class_upload_totals();
        assert_eq!(totals.cursor_only, 42);
        assert_eq!(totals.bounded, 7);
        assert_eq!(totals.full, 100);
        assert_eq!(totals.reuse, 0);
    }

    #[test]
    fn multiple_painted_completions_of_the_same_class_accumulate_bytes() {
        let mut profile = LiveRenderProfile::new();

        for total in [10_u64, 20, 30] {
            let token = profile.start(full_raw(), RenderWorkClass::Full);
            profile.complete(token, sample_uploads(total));
        }

        assert_eq!(profile.class_upload_totals().full, 60);
        assert_eq!(profile.class_counts().full, 3);
    }

    /// Regression-shape test (mirrors 125.5's unpainted-classification
    /// fix): an unpainted (superseded) observation must still be
    /// attributed to `class_upload_totals` -- with exactly zero bytes,
    /// since it issued no GL upload at all -- so the per-class byte total
    /// reconciles against the SAME `class_counts` denominator a later
    /// bytes-per-frame calculation would divide by.
    #[test]
    fn an_unpainted_observation_attributes_zero_bytes_to_its_resolved_class() {
        let mut profile = LiveRenderProfile::new();

        let stale = profile.start(reuse_raw(), RenderWorkClass::Reuse);
        let _next = profile.start(RawRebuildDecision::CursorOnly, RenderWorkClass::CursorOnly);

        assert_eq!(profile.unpainted_count(), 1);
        assert_eq!(profile.class_counts().reuse, 1);
        assert_eq!(profile.class_upload_totals().reuse, 0);
        assert_eq!(profile.upload_totals().total(), 0);

        // The stale token's outcome is unpainted, not painted -- confirms
        // this is genuinely the zero-upload path, not a completion that
        // happened to report zero bytes.
        let records: Vec<_> = profile.records().collect();
        assert_eq!(records[0].token, stale);
        assert!(matches!(
            records[0].outcome,
            PaneFrameOutcome::Unpainted { .. }
        ));
    }

    #[test]
    fn class_upload_totals_and_upload_totals_reconcile_across_mixed_observations() {
        let mut profile = LiveRenderProfile::new();

        // Two painted `Full` completions with real uploads.
        for total in [50_u64, 25] {
            let token = profile.start(full_raw(), RenderWorkClass::Full);
            profile.complete(token, sample_uploads(total));
        }
        // One `Full` observation that never gets completed and is
        // superseded (unpainted, zero bytes).
        let _stale = profile.start(full_raw(), RenderWorkClass::Full);
        let next = profile.start(RawRebuildDecision::CursorOnly, RenderWorkClass::CursorOnly);
        profile.complete(next, UploadByteCounts::default());

        // Cross-check: the sum of every category in `upload_totals()`
        // equals the sum across every class in `class_upload_totals()` --
        // two independent breakdowns of the exact same underlying bytes.
        let by_category = profile.upload_totals().total();
        let by_class = profile.class_upload_totals();
        let by_class_sum = by_class
            .reuse
            .saturating_add(by_class.cursor_only)
            .saturating_add(by_class.bounded)
            .saturating_add(by_class.full);
        assert_eq!(by_category, 75);
        assert_eq!(by_class_sum, 75);
        assert_eq!(by_class.full, 75);
        assert_eq!(by_class.cursor_only, 0);
        // Three `Full` observations (two painted, one unpainted) were
        // classified, but only 75 bytes total were ever uploaded.
        assert_eq!(profile.class_counts().full, 3);
    }
}
