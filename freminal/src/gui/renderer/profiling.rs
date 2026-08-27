// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Live render-work profiling foundation (Task 125.4).
//!
//! This module is entirely gated behind the existing `frame-profiling`
//! Cargo feature (see `freminal/Cargo.toml`) and compiled out completely in
//! a default build -- see the `#[cfg(feature = "frame-profiling")]` on this
//! module's declaration in `super`. It defines the cohesive types 125.5,
//! 125.6 and 125.8 will record into: [`RenderWorkClass`],
//! [`ChangedRowBucket`], [`UploadByteCounts`], and [`LiveRenderProfile`]
//! itself. **No renderer or widget call site is wired to any of this yet**
//! -- that is 125.5's and 125.6's scope. This subtask is the pure state
//! machine plus its exhaustive unit tests.
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
//! [`PaneFrameOutcome::Unpainted`] (zero upload bytes) -- this is precisely
//! the `FrameDamage::None` case: `show()` ran and got a token, no paint
//! callback ever ran, and the next `show()` call is what notices.
//!
//! # State machine summary
//!
//! - [`LiveRenderProfile::start`] always succeeds and always returns a new
//!   token. If a previous token was left outstanding, it is finalized as
//!   unpainted first.
//! - [`LiveRenderProfile::complete`] only finalizes the record when the
//!   supplied token equals the currently outstanding one. A late
//!   completion (for a token superseded by a newer [`start`] call), a
//!   duplicate completion (the same token completed twice), or an
//!   out-of-order completion (a token that was never the outstanding one)
//!   are all silently ignored and reported back to the caller as `false` --
//!   see the `paned`/`unpainted`/`late`/`duplicate`/`out_of_order` tests
//!   below for one scenario each.

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

/// What became of one [`PaneFrameToken`] (125.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneFrameOutcome {
    /// The matching paint callback ran and reported a resolved render-work
    /// class plus its upload-byte attribution.
    Painted {
        /// The resolved render-work classification.
        class: RenderWorkClass,
        /// The exact uploads issued by that paint callback.
        uploads: UploadByteCounts,
    },
    /// No paint callback ever completed this token before a newer
    /// [`LiveRenderProfile::start`] call superseded it (see the module doc
    /// for the `FrameDamage::None` case this represents). Implicitly
    /// zero-upload.
    Unpainted,
}

/// One finalized observation in [`LiveRenderProfile`]'s bounded queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaneFrameRecord {
    /// The token this record finalizes.
    pub token: PaneFrameToken,
    /// What became of it.
    pub outcome: PaneFrameOutcome,
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
    /// The currently outstanding token, if one has been started but not
    /// yet finalized (by either a matching [`Self::complete`] or a
    /// superseding [`Self::start`]).
    pending: Option<PaneFrameToken>,
    /// The most recently finalized records, bounded to
    /// [`Self::RECORD_QUEUE_CAPACITY`] entries (oldest evicted first).
    records: VecDeque<PaneFrameRecord>,
    /// Cumulative [`RenderWorkClass`] occurrence counts since creation.
    class_counts: RenderWorkClassCounts,
    /// Cumulative [`ChangedRowBucket`] occurrence counts since creation
    /// (only [`RenderWorkClass::Bounded`] completions contribute).
    row_bucket_counts: ChangedRowHistogram,
    /// Cumulative upload-byte totals since creation.
    upload_totals: UploadByteCounts,
    /// Painted completions since creation.
    painted_count: u64,
    /// Unpainted finalizations since creation.
    unpainted_count: u64,
    /// Total finalized observations since creation (`painted_count +
    /// unpainted_count`), used by [`Self::is_flush_due`].
    observation_count: u64,
}

impl LiveRenderProfile {
    /// Emit a flush-worthy summary once every this many finalized
    /// observations -- the same cadence as
    /// `freminal::gui::window::FrameStats::FLUSH_EVERY` and
    /// `freminal_windowing::egui_integration::FrameProfile::FLUSH_EVERY`,
    /// so a live session's log lines stay easy to correlate by eye. This
    /// subtask (125.4) only exposes [`Self::is_flush_due`] as a pure
    /// predicate; emitting an actual `tracing::debug!` line is a later
    /// subtask's call-site responsibility.
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

    /// Construct a fresh profile with no outstanding token and all-zero
    /// cumulative totals.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Begin a new pane-frame observation.
    ///
    /// If a previously started token is still outstanding (its matching
    /// [`Self::complete`] never arrived before this call), it is finalized
    /// first as [`PaneFrameOutcome::Unpainted`] -- see the module doc for
    /// why this is the `FrameDamage::None` case.
    pub fn start(&mut self) -> PaneFrameToken {
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
        self.pending = Some(token);
        token
    }

    /// Finalize the outstanding token as painted, with its resolved class
    /// and upload attribution.
    ///
    /// Returns `true` when `token` was the currently outstanding one and
    /// this call finalized it. Returns `false` -- taking no action at all
    /// -- when `token` does not match the outstanding one: a late
    /// completion (superseded by a newer [`Self::start`]), a duplicate
    /// completion (already finalized), or an out-of-order completion (a
    /// token that was never the outstanding one) are all rejected the same
    /// way. See the module doc and this file's tests for one scenario
    /// each.
    pub fn complete(
        &mut self,
        token: PaneFrameToken,
        class: RenderWorkClass,
        uploads: UploadByteCounts,
    ) -> bool {
        if self.pending != Some(token) {
            return false;
        }
        self.pending = None;
        self.class_counts.record(class);
        if let RenderWorkClass::Bounded(bucket) = class {
            self.row_bucket_counts.record(bucket);
        }
        self.upload_totals = self.upload_totals.merge(uploads);
        self.painted_count = self.painted_count.saturating_add(1);
        self.observation_count = self.observation_count.saturating_add(1);
        self.push_record(PaneFrameRecord {
            token,
            outcome: PaneFrameOutcome::Painted { class, uploads },
        });
        true
    }

    /// Finalize `token` as unpainted (zero-upload), from either
    /// [`Self::start`] superseding it or (in a future subtask) an explicit
    /// end-of-frame sweep for a token that egui never painted.
    fn finalize_unpainted(&mut self, token: PaneFrameToken) {
        self.unpainted_count = self.unpainted_count.saturating_add(1);
        self.observation_count = self.observation_count.saturating_add(1);
        self.push_record(PaneFrameRecord {
            token,
            outcome: PaneFrameOutcome::Unpainted,
        });
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
        self.pending
    }

    /// The most recently finalized records, oldest first, bounded to
    /// [`Self::RECORD_QUEUE_CAPACITY`] entries.
    #[must_use]
    pub fn records(&self) -> impl DoubleEndedIterator<Item = &PaneFrameRecord> + '_ {
        self.records.iter()
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

    /// Whether the observation count has just crossed a
    /// [`Self::FLUSH_EVERY`] boundary -- i.e. a caller driving this profile
    /// once per finalized observation should emit its periodic summary now.
    /// `false` at zero observations (nothing to summarize yet).
    #[must_use]
    pub const fn is_flush_due(&self) -> bool {
        self.observation_count > 0 && self.observation_count.is_multiple_of(Self::FLUSH_EVERY)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::{
        ChangedRowBucket, LiveRenderProfile, PaneFrameOutcome, RenderWorkClass, UploadByteCounts,
    };

    fn sample_uploads(total: u64) -> UploadByteCounts {
        UploadByteCounts {
            foreground_instance_vbo_bytes: total,
            ..UploadByteCounts::default()
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
        let token = profile.start();
        assert_eq!(profile.pending(), Some(token));

        let class = RenderWorkClass::Bounded(ChangedRowBucket::from_count(3));
        let uploads = sample_uploads(42);
        let finalized = profile.complete(token, class, uploads);

        assert!(finalized);
        assert_eq!(profile.pending(), None);
        assert_eq!(profile.painted_count(), 1);
        assert_eq!(profile.unpainted_count(), 0);
        assert_eq!(profile.observation_count(), 1);
        assert_eq!(profile.class_counts().bounded, 1);
        assert_eq!(profile.class_counts().reuse, 0);
        assert_eq!(profile.row_bucket_counts().two_to_four, 1);
        assert_eq!(profile.upload_totals().total(), 42);

        let records: Vec<_> = profile.records().collect();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].token, token);
        match records[0].outcome {
            PaneFrameOutcome::Painted {
                class: recorded_class,
                uploads: recorded_uploads,
            } => {
                assert_eq!(recorded_class, class);
                assert_eq!(recorded_uploads.total(), 42);
            }
            PaneFrameOutcome::Unpainted => panic!("expected a painted outcome"),
        }
    }

    #[test]
    fn starting_a_new_token_finalizes_the_stale_one_as_unpainted() {
        let mut profile = LiveRenderProfile::new();
        let first = profile.start();
        // The first token's matching `complete` never arrives -- the next
        // `show()` call starts a new token before it does, exactly the
        // `FrameDamage::None` case from the module doc.
        let second = profile.start();

        assert_eq!(profile.pending(), Some(second));
        assert_eq!(profile.unpainted_count(), 1);
        assert_eq!(profile.painted_count(), 0);
        assert_eq!(profile.observation_count(), 1);

        let records: Vec<_> = profile.records().collect();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].token, first);
        assert_eq!(records[0].outcome, PaneFrameOutcome::Unpainted);
    }

    #[test]
    fn late_completion_after_a_newer_token_started_is_rejected() {
        let mut profile = LiveRenderProfile::new();
        let first = profile.start();
        let _second = profile.start(); // finalizes `first` as unpainted

        let finalized = profile.complete(first, RenderWorkClass::Full, sample_uploads(99));

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
        let token = profile.start();
        let first_complete =
            profile.complete(token, RenderWorkClass::CursorOnly, sample_uploads(7));
        assert!(first_complete);

        let second_complete = profile.complete(token, RenderWorkClass::Full, sample_uploads(500));

        assert!(!second_complete);
        // Only the first completion's data is recorded.
        assert_eq!(profile.painted_count(), 1);
        assert_eq!(profile.upload_totals().total(), 7);
        assert_eq!(profile.class_counts().cursor_only, 1);
        assert_eq!(profile.class_counts().full, 0);
        assert_eq!(profile.records().count(), 1);
    }

    #[test]
    fn out_of_order_completion_for_a_never_pending_token_is_rejected() {
        let mut profile = LiveRenderProfile::new();
        let first = profile.start();
        let second = profile.start(); // finalizes `first` as unpainted
        let completed_second = profile.complete(second, RenderWorkClass::Reuse, sample_uploads(0));
        assert!(completed_second);

        // Fabricate a token value that was never actually issued by
        // `start` (one past the highest issued so far) and attempt to
        // complete it. This can never happen causally through the public
        // `start`/`complete` API, but the state machine must still reject
        // it defensively rather than panicking or corrupting totals.
        let never_issued = super::PaneFrameToken(first.value().max(second.value()) + 1);
        let finalized = profile.complete(never_issued, RenderWorkClass::Full, sample_uploads(3));

        assert!(!finalized);
        assert_eq!(profile.painted_count(), 1);
        assert_eq!(profile.unpainted_count(), 1);
        assert_eq!(profile.upload_totals().total(), 0);
        assert_eq!(profile.records().count(), 2);
    }

    #[test]
    fn bounded_records_do_not_finalize_unrelated_reuse_or_full_buckets() {
        let mut profile = LiveRenderProfile::new();
        let token = profile.start();
        profile.complete(token, RenderWorkClass::Reuse, UploadByteCounts::default());
        assert_eq!(profile.row_bucket_counts().total(), 0);
        assert_eq!(profile.class_counts().reuse, 1);
    }

    #[test]
    fn record_queue_is_bounded_and_drops_the_oldest_first() {
        let mut profile = LiveRenderProfile::new();
        let capacity = LiveRenderProfile::RECORD_QUEUE_CAPACITY;
        let mut last_token = None;
        for _ in 0..(capacity + 10) {
            let token = profile.start();
            profile.complete(token, RenderWorkClass::Reuse, UploadByteCounts::default());
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
    fn flush_cadence_fires_every_flush_every_observations_and_not_at_zero() {
        let mut profile = LiveRenderProfile::new();
        assert!(!profile.is_flush_due());

        for i in 1..=(LiveRenderProfile::FLUSH_EVERY * 2) {
            let token = profile.start();
            profile.complete(token, RenderWorkClass::Reuse, UploadByteCounts::default());
            let expected = i.is_multiple_of(LiveRenderProfile::FLUSH_EVERY);
            assert_eq!(
                profile.is_flush_due(),
                expected,
                "observation {i} flush-due mismatch"
            );
        }
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
}
