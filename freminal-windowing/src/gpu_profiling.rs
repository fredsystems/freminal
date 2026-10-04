// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! GPU timestamp-query foundation (Task 125.7).
//!
//! One concept: a bounded, asynchronous GPU timer-query lifecycle,
//! independent of any concrete GL context. The state machine here never
//! calls a GL function itself -- it is driven entirely through the
//! [`GpuQuerySource`] trait, so it can be exercised with fake handles in
//! tests and later wired to real `glow::Query` handles (Task 125.8) without
//! this module changing.
//!
//! # Why asynchronous
//!
//! A GPU timer query (`GL_TIMESTAMP`) is issued on one frame and its result
//! is not available until the GPU has actually retired the corresponding
//! command -- reading it before that would force a pipeline stall
//! (`glFinish`-equivalent), which is exactly the class of self-defeating
//! measurement `Documents/PROFILING.md` warns against. [`GpuQueryRing`]
//! therefore enforces two invariants no caller can bypass:
//!
//! 1. **No polling on the issue frame.** [`GpuQueryRing::poll`] never calls
//!    [`GpuQuerySource::is_result_available`] for a sample whose
//!    `issue_frame` equals the frame `poll` is called with -- the sample is
//!    left pending and re-checked on a later call.
//! 2. **No blocking reads.** A result is only read via
//!    [`GpuQuerySource::read_result_ns`] after
//!    [`GpuQuerySource::is_result_available`] has returned `true` for the
//!    same query in the same [`GpuQueryRing::poll`] call -- never
//!    speculatively.
//!
//! # Why bounded
//!
//! A driver that never signals availability (a lost context, a buggy
//! driver, `GL_ARB_timer_query` silently absent) must not let pending
//! samples grow without bound. [`GpuQueryRing`] is constructed with a fixed
//! `capacity`; once that many samples are pending, [`GpuQueryRing::issue`]
//! rejects the new sample rather than growing or blocking, and counts the
//! rejection via [`GpuQueryRing::dropped_sample_count`] (a saturating
//! counter -- see that method's doc). Rejection is not silent discarding:
//! [`SampleIssueOutcome::DroppedRingFull`] hands the caller back the exact
//! `start`/`end` handles it passed in, because the ring never took
//! ownership of them and therefore cannot destroy them either -- the
//! caller remains responsible for that, via [`GpuQuerySource::destroy_query`].
//!
//! # Why capability detection is a pure function
//!
//! [`GpuTimerCapability::detect`] takes an already-parsed [`GlVersionInfo`]
//! and an extension set rather than a live `glow::Context`, so the desktop
//! vs. GLES / version vs. extension decision matrix is testable without a
//! GL context at all. [`parse_gl_version_string`] is the pure parser for
//! the raw `GL_VERSION` string a real caller reads via
//! `glow::HasContext::get_parameter_string(glow::VERSION)`.

use std::collections::{HashSet, VecDeque};

/// Whether a GL context can service asynchronous timestamp queries.
///
/// Named domain enum (`freminal-state-representation`), not a bare `bool`:
/// [`GpuTimerCapability::detect`] is called once per context and the result
/// is threaded through the rest of the profiling machinery, so a bare bool
/// here would be exactly the "mode transported past the point it was
/// computed" case the skill calls out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuTimerCapability {
    /// Desktop OpenGL 3.3+ (where `GL_TIMESTAMP`/`glQueryCounter` are core),
    /// or an older desktop context that advertises `GL_ARB_timer_query`.
    Available,
    /// GLES (which has no standard timer-query extension freminal can rely
    /// on), or a desktop context below 3.3 without the ARB extension.
    Unavailable,
}

impl GpuTimerCapability {
    /// Decide capability from a parsed context version and its supported
    /// extension set.
    ///
    /// GLES is unconditionally [`GpuTimerCapability::Unavailable`] --
    /// `GL_EXT_disjoint_timer_query` exists but is not universally
    /// supported and is deliberately out of scope here. Desktop OpenGL is
    /// [`GpuTimerCapability::Available`] at 3.3+ (`GL_TIMESTAMP` is core)
    /// or when `extensions` contains `GL_ARB_timer_query`.
    #[must_use]
    pub fn detect(info: GlVersionInfo, extensions: &HashSet<String>) -> Self {
        match info.profile {
            GlProfile::Gles => Self::Unavailable,
            GlProfile::Desktop => {
                let is_33_or_newer =
                    info.version.major > 3 || (info.version.major == 3 && info.version.minor >= 3);
                if is_33_or_newer || extensions.contains("GL_ARB_timer_query") {
                    Self::Available
                } else {
                    Self::Unavailable
                }
            }
        }
    }
}

/// Whether a parsed `GL_VERSION` string describes a desktop or an
/// embedded-profile (GLES) context.
///
/// Named domain enum, not a bare `bool` -- the two profiles have entirely
/// different extension ecosystems and version-number meanings, not just an
/// on/off toggle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlProfile {
    /// Desktop OpenGL (`GL_VERSION` looks like `"4.6.0 NVIDIA 470.63.01"`).
    Desktop,
    /// OpenGL ES (`GL_VERSION` looks like `"OpenGL ES 3.2 Mesa 23.2.1"`).
    Gles,
}

/// A GL major/minor version pair, as reported by `GL_VERSION`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct GlVersion {
    /// Major version component.
    pub major: u32,
    /// Minor version component.
    pub minor: u32,
}

/// The result of parsing a raw `GL_VERSION` string: which profile it names
/// and at what version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GlVersionInfo {
    /// Desktop vs. GLES.
    pub profile: GlProfile,
    /// The parsed major/minor version.
    pub version: GlVersion,
}

/// Parse a raw `GL_VERSION` string (as returned by
/// `glow::HasContext::get_parameter_string(glow::VERSION)`) into a
/// [`GlVersionInfo`].
///
/// Per the OpenGL spec, a desktop string is `"<major>.<minor>[.<release>]
/// <vendor info>"` and a GLES string is `"OpenGL ES [Common
/// [-Lite]] <major>.<minor> <vendor info>"`. This is a pure string parse --
/// no GL context is consulted -- so it is fully unit-testable. Returns
/// `None` if the leading version token cannot be parsed as `<u32>.<u32>`.
#[must_use]
pub fn parse_gl_version_string(raw: &str) -> Option<GlVersionInfo> {
    let trimmed = raw.trim();
    if let Some(rest) = trimmed.strip_prefix("OpenGL ES ") {
        let version = parse_leading_version(rest)?;
        return Some(GlVersionInfo {
            profile: GlProfile::Gles,
            version,
        });
    }
    let version = parse_leading_version(trimmed)?;
    Some(GlVersionInfo {
        profile: GlProfile::Desktop,
        version,
    })
}

/// Parse the leading `<major>.<minor>` token from a version string,
/// ignoring anything after the first run of whitespace (release number,
/// vendor string, profile annotation).
fn parse_leading_version(s: &str) -> Option<GlVersion> {
    let leading_token = s.split_whitespace().next()?;
    let mut components = leading_token.split('.');
    let major = components.next()?.parse::<u32>().ok()?;
    let minor = components.next()?.parse::<u32>().ok()?;
    Some(GlVersion { major, minor })
}

/// A caller-supplied name identifying which timed operation a
/// [`GpuQueryRing`] sample measures (e.g. `"terminal_upload"`,
/// `"chrome_paint"`).
///
/// `freminal-windowing` does not know or care what operations exist --
/// naming them is entirely the owning crate's business (Task 125.8/125.9
/// wire concrete phase names). Kept as a distinct wrapper type rather than
/// a bare `&'static str` parameter so a phase can never be silently
/// transposed with some other unrelated string argument at a call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GpuProfilePhase(pub &'static str);

/// The outcome of [`GpuQueryRing::issue`].
///
/// Named domain enum, not a bare `bool`: a caller must be able to match
/// on -- and is likely to count -- the dropped case, and "issued vs.
/// dropped" is a real branch in the ring's lifecycle, not an on/off flag.
///
/// Generic over the query handle type `Q` because
/// [`Self::DroppedRingFull`] hands the rejected `start`/`end` handles back
/// to the caller rather than discarding them: the ring never took
/// ownership of a sample it refused to store, so it cannot destroy that
/// sample's handles on the caller's behalf either. Silently dropping a
/// live GL query object would leak it (or rely on the caller having some
/// other, easy-to-forget way to notice); returning ownership makes the
/// caller's own [`GpuQuerySource::destroy_query`] the only path, exactly as
/// for every handle the ring *did* accept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleIssueOutcome<Q> {
    /// The sample was accepted and is now pending.
    Issued,
    /// The ring was at capacity; the new sample was **not** stored, and
    /// [`GpuQueryRing::dropped_sample_count`] was incremented. `start` and
    /// `end` are the exact handles the caller passed to
    /// [`GpuQueryRing::issue`], handed back so the caller can destroy them
    /// via [`GpuQuerySource::destroy_query`] -- the ring holds no reference
    /// to them once this variant is returned.
    DroppedRingFull {
        /// The rejected sample's start-timestamp handle.
        start: Q,
        /// The rejected sample's end-timestamp handle.
        end: Q,
    },
}

/// A pending timestamp-query pair, tracked internally by [`GpuQueryRing`]
/// until its result becomes available (or it is explicitly destroyed).
#[derive(Debug, Clone)]
struct QuerySample<Q> {
    phase: GpuProfilePhase,
    start: Q,
    end: Q,
    issue_frame: u64,
}

/// A [`GpuQueryRing`] sample whose GPU timestamps have both become
/// available, returned by [`GpuQueryRing::poll`].
///
/// Query handles themselves are not part of this type: by the time a
/// `CompletedSample` is produced, [`GpuQuerySource::destroy_query`] has
/// already been called for both handles -- see that method's doc.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompletedSample {
    /// Which timed operation this sample measured.
    pub phase: GpuProfilePhase,
    /// The frame on which the sample was issued.
    pub issue_frame: u64,
    /// The frame on which [`GpuQueryRing::poll`] found both results ready.
    pub completed_frame: u64,
    /// The GPU timestamp at the start of the timed span, in nanoseconds
    /// (raw driver-reported value; not wall-clock-relative).
    pub start_ns: u64,
    /// The GPU timestamp at the end of the timed span, in nanoseconds.
    pub end_ns: u64,
}

impl CompletedSample {
    /// How many frames elapsed between issue and availability -- the
    /// "query latency in frames" the plan calls for reporting.
    #[must_use]
    pub const fn latency_frames(&self) -> u64 {
        self.completed_frame - self.issue_frame
    }

    /// The measured GPU duration in nanoseconds (`end_ns - start_ns`).
    ///
    /// Saturating: a driver is not expected to report `end_ns < start_ns`
    /// for a validly nested pair, but this reporting-only accessor must
    /// not panic if one somehow does.
    #[must_use]
    pub const fn duration_ns(&self) -> u64 {
        self.end_ns.saturating_sub(self.start_ns)
    }
}

/// Non-blocking access to GPU query state, implemented by the owning crate
/// over a real `glow::Context` (Task 125.8) or by a fake in tests.
///
/// [`GpuQueryRing`] calls this trait's methods and nothing else -- it never
/// touches a GL function directly, which is what lets the ring's lifecycle
/// be tested with fake handles and no GL context at all.
pub trait GpuQuerySource<Q> {
    /// Non-blocking availability check
    /// (`glGetQueryObjectuiv(query, GL_QUERY_RESULT_AVAILABLE)`).
    ///
    /// Implementers must never call the blocking `GL_QUERY_RESULT` form,
    /// and [`GpuQueryRing`] never calls this for a query issued on the
    /// current poll frame (see the module doc).
    fn is_result_available(&mut self, query: &Q) -> bool;

    /// Read a 64-bit query result in nanoseconds
    /// (`glGetQueryObjectui64v(query, GL_QUERY_RESULT)`).
    ///
    /// [`GpuQueryRing`] only calls this immediately after
    /// [`Self::is_result_available`] returned `true` for the same query
    /// within the same [`GpuQueryRing::poll`] call -- never speculatively,
    /// and never on the issue frame.
    fn read_result_ns(&mut self, query: &Q) -> u64;

    /// Explicitly destroy a query object (`glDeleteQueries`).
    ///
    /// Called exactly once for every handle [`GpuQueryRing`] ever accepted
    /// via [`GpuQueryRing::issue`] -- once its result has been read
    /// ([`GpuQueryRing::poll`]) or when the ring is torn down
    /// ([`GpuQueryRing::drain_and_destroy`]). A handle the ring *rejected*
    /// ([`SampleIssueOutcome::DroppedRingFull`]) is never stored by the
    /// ring in the first place and so never reaches this method through
    /// the ring's own calls -- the ring hands that handle back to the
    /// caller instead, and the caller is expected to route it through this
    /// same method itself.
    fn destroy_query(&mut self, query: Q);
}

/// A bounded, asynchronous GPU timestamp-query lifecycle state machine.
///
/// See the module doc for the two invariants this type enforces
/// (no-same-frame-poll, no-read-before-available) and why the ring is
/// bounded. `Q` is the query handle type -- `glow::Query` in production,
/// any `Clone` type (a bare `u32`, for instance) in tests.
#[derive(Debug)]
pub struct GpuQueryRing<Q> {
    capacity: usize,
    pending: VecDeque<QuerySample<Q>>,
    dropped_samples: u64,
}

impl<Q> GpuQueryRing<Q> {
    /// Create a new ring bounded to `capacity` simultaneously pending
    /// samples.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            pending: VecDeque::with_capacity(capacity),
            dropped_samples: 0,
        }
    }

    /// The ring's fixed capacity, as given to [`Self::new`].
    #[must_use]
    pub const fn capacity(&self) -> usize {
        self.capacity
    }

    /// How many samples are currently pending (issued, not yet completed
    /// or destroyed).
    #[must_use]
    pub fn len(&self) -> usize {
        self.pending.len()
    }

    /// Whether no samples are currently pending.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// How many samples have been dropped because the ring was full at
    /// [`Self::issue`] time. Monotonically non-decreasing for the life of
    /// the ring -- incremented with a saturating add, so a pathologically
    /// stuck driver that never frees a slot cannot overflow this counter
    /// (it simply pins at [`u64::MAX`] instead of wrapping or panicking).
    #[must_use]
    pub const fn dropped_sample_count(&self) -> u64 {
        self.dropped_samples
    }

    /// Issue a new start/end timestamp-query pair for `phase`, recorded as
    /// having been issued on `issue_frame`.
    ///
    /// If the ring is already at [`Self::capacity`], the new sample is
    /// rejected: the ring never stores it, and
    /// [`Self::dropped_sample_count`] is incremented (saturating -- it
    /// never wraps or panics even if a driver stays stuck long enough to
    /// exhaust a `u64`). Rejection returns ownership of `start` and `end`
    /// back to the caller via
    /// [`SampleIssueOutcome::DroppedRingFull`] rather than discarding
    /// them: they are live GL query objects the ring refused to take
    /// responsibility for, so the caller must destroy them itself (see
    /// that variant's doc). The ring never blocks and never grows past
    /// `capacity`.
    pub fn issue(
        &mut self,
        phase: GpuProfilePhase,
        start: Q,
        end: Q,
        issue_frame: u64,
    ) -> SampleIssueOutcome<Q> {
        if self.pending.len() >= self.capacity {
            self.dropped_samples = self.dropped_samples.saturating_add(1);
            return SampleIssueOutcome::DroppedRingFull { start, end };
        }
        self.pending.push_back(QuerySample {
            phase,
            start,
            end,
            issue_frame,
        });
        SampleIssueOutcome::Issued
    }

    /// Poll every pending sample for availability, called once per frame
    /// with the current frame number.
    ///
    /// A sample issued on `current_frame` is left untouched -- `source` is
    /// never consulted for it this call, enforcing the no-same-frame-poll
    /// invariant regardless of what `source` would have answered. Every
    /// other pending sample is checked via
    /// [`GpuQuerySource::is_result_available`] (keyed off the `end`
    /// timestamp, since it always completes at or after `start`); when
    /// available, both timestamps are read, both handles are destroyed via
    /// [`GpuQuerySource::destroy_query`], and the sample is returned as a
    /// [`CompletedSample`]. Samples that are not yet available remain
    /// pending for a later call. Order of returned samples matches issue
    /// order among those completed this call.
    pub fn poll<S: GpuQuerySource<Q>>(
        &mut self,
        current_frame: u64,
        source: &mut S,
    ) -> Vec<CompletedSample> {
        let mut completed = Vec::new();
        let mut still_pending = VecDeque::with_capacity(self.pending.len());

        while let Some(sample) = self.pending.pop_front() {
            if sample.issue_frame == current_frame {
                still_pending.push_back(sample);
                continue;
            }

            if source.is_result_available(&sample.end) {
                let start_ns = source.read_result_ns(&sample.start);
                let end_ns = source.read_result_ns(&sample.end);
                source.destroy_query(sample.start);
                source.destroy_query(sample.end);
                completed.push(CompletedSample {
                    phase: sample.phase,
                    issue_frame: sample.issue_frame,
                    completed_frame: current_frame,
                    start_ns,
                    end_ns,
                });
            } else {
                still_pending.push_back(sample);
            }
        }

        self.pending = still_pending;
        completed
    }

    /// Explicitly destroy every still-pending sample's handles (e.g. at
    /// shutdown, or when capability loss is detected) without reading their
    /// results. Returns how many samples were destroyed. The ring is empty
    /// afterward.
    pub fn drain_and_destroy<S: GpuQuerySource<Q>>(&mut self, source: &mut S) -> usize {
        let mut destroyed = 0usize;
        while let Some(sample) = self.pending.pop_front() {
            source.destroy_query(sample.start);
            source.destroy_query(sample.end);
            destroyed += 1;
        }
        destroyed
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::{
        CompletedSample, GlProfile, GlVersion, GlVersionInfo, GpuProfilePhase, GpuQueryRing,
        GpuQuerySource, GpuTimerCapability, SampleIssueOutcome, parse_gl_version_string,
    };
    use std::collections::{HashMap, HashSet};

    /// A fake [`GpuQuerySource`] over `u32` handles, driven entirely by
    /// test setup rather than a real GL context. Tracks every call it
    /// receives so tests can assert on exactly what the ring did and did
    /// not invoke -- the load-bearing check for the "no same-frame poll"
    /// and "no read before available" invariants.
    #[derive(Default)]
    struct FakeQuerySource {
        /// Handles whose result the test has decided is ready.
        available: HashSet<u32>,
        /// The nanosecond value each available handle should report.
        results: HashMap<u32, u64>,
        /// Every handle `is_result_available` was called with, in order.
        availability_calls: Vec<u32>,
        /// Every handle `read_result_ns` was called with, in order.
        read_calls: Vec<u32>,
        /// Every handle `destroy_query` was called with, in order.
        destroy_calls: Vec<u32>,
    }

    impl FakeQuerySource {
        fn mark_available(&mut self, handle: u32, result_ns: u64) {
            self.available.insert(handle);
            self.results.insert(handle, result_ns);
        }
    }

    impl GpuQuerySource<u32> for FakeQuerySource {
        fn is_result_available(&mut self, query: &u32) -> bool {
            self.availability_calls.push(*query);
            self.available.contains(query)
        }

        fn read_result_ns(&mut self, query: &u32) -> u64 {
            self.read_calls.push(*query);
            // Enforce the "no read before available" contract from the
            // source side too: a ring that violated it would panic here
            // rather than silently returning a bogus value.
            assert!(
                self.available.contains(query),
                "read_result_ns called before is_result_available succeeded for {query}"
            );
            self.results.get(query).copied().unwrap_or_default()
        }

        fn destroy_query(&mut self, query: u32) {
            self.destroy_calls.push(query);
        }
    }

    const PHASE: GpuProfilePhase = GpuProfilePhase("terminal_upload");

    // ── GpuQueryRing: delayed availability ───────────────────────────────

    #[test]
    fn poll_leaves_sample_pending_until_source_reports_available() {
        let mut ring: GpuQueryRing<u32> = GpuQueryRing::new(4);
        let mut source = FakeQuerySource::default();

        assert_eq!(ring.issue(PHASE, 1, 2, 10), SampleIssueOutcome::Issued);

        // Frame 11: source has not yet marked the query available.
        let completed = ring.poll(11, &mut source);
        assert_eq!(completed, []);
        assert_eq!(ring.len(), 1, "sample must remain pending");

        // Frame 12: still not available.
        let completed = ring.poll(12, &mut source);
        assert_eq!(completed, []);
        assert_eq!(ring.len(), 1);

        // Frame 13: now available.
        source.mark_available(2, 500);
        source.mark_available(1, 100);
        let completed = ring.poll(13, &mut source);
        assert_eq!(completed.len(), 1);
        let sample = completed[0];
        assert_eq!(sample.phase, PHASE);
        assert_eq!(sample.issue_frame, 10);
        assert_eq!(sample.completed_frame, 13);
        assert_eq!(sample.start_ns, 100);
        assert_eq!(sample.end_ns, 500);
        assert_eq!(sample.latency_frames(), 3);
        assert_eq!(sample.duration_ns(), 400);
        assert!(ring.is_empty());
    }

    // ── GpuQueryRing: no same-frame availability/read calls ──────────────

    #[test]
    fn poll_never_queries_source_for_a_sample_issued_this_frame() {
        let mut ring: GpuQueryRing<u32> = GpuQueryRing::new(4);
        let mut source = FakeQuerySource::default();
        // Even though the source WOULD report these as available, the ring
        // must not ask -- issue_frame == poll's current_frame.
        source.mark_available(1, 100);
        source.mark_available(2, 500);

        ring.issue(PHASE, 1, 2, 42);
        let completed = ring.poll(42, &mut source);

        assert_eq!(completed, []);
        assert_eq!(ring.len(), 1, "same-frame sample must stay pending");
        assert!(
            source.availability_calls.is_empty(),
            "is_result_available must not be called on the issue frame"
        );
        assert!(
            source.read_calls.is_empty(),
            "read_result_ns must not be called on the issue frame"
        );
    }

    #[test]
    fn poll_only_reads_results_after_availability_succeeds() {
        let mut ring: GpuQueryRing<u32> = GpuQueryRing::new(4);
        let mut source = FakeQuerySource::default();

        ring.issue(PHASE, 1, 2, 0);

        // Not available yet: read_result_ns must never be called (the
        // fake's own assert would panic if the ring got this wrong).
        let completed = ring.poll(1, &mut source);
        assert_eq!(completed, []);
        assert_eq!(source.read_calls, []);

        // Now available: reads happen, and only now.
        source.mark_available(1, 10);
        source.mark_available(2, 20);
        let completed = ring.poll(2, &mut source);
        assert_eq!(completed.len(), 1);
        assert_eq!(source.read_calls, vec![1, 2]);
    }

    // ── GpuQueryRing: full-ring drop and count ───────────────────────────

    #[test]
    fn issue_drops_and_counts_when_ring_is_full() {
        let mut ring: GpuQueryRing<u32> = GpuQueryRing::new(2);

        assert_eq!(ring.issue(PHASE, 1, 2, 0), SampleIssueOutcome::Issued);
        assert_eq!(ring.issue(PHASE, 3, 4, 0), SampleIssueOutcome::Issued);
        assert_eq!(ring.len(), 2);
        assert_eq!(ring.dropped_sample_count(), 0);

        assert_eq!(
            ring.issue(PHASE, 5, 6, 0),
            SampleIssueOutcome::DroppedRingFull { start: 5, end: 6 }
        );
        assert_eq!(ring.len(), 2, "ring must not grow past capacity");
        assert_eq!(ring.dropped_sample_count(), 1);

        // A second drop increments the counter again rather than
        // saturating or resetting.
        assert_eq!(
            ring.issue(PHASE, 7, 8, 0),
            SampleIssueOutcome::DroppedRingFull { start: 7, end: 8 }
        );
        assert_eq!(ring.dropped_sample_count(), 2);
    }

    /// Task 125.7 review fix: a rejected sample's handles must come back to
    /// the caller -- not be silently discarded -- so the caller can
    /// explicitly destroy them exactly as it would any accepted sample's
    /// handles. This is the exact-handle-identity + no-ownership-claim
    /// proof called for by the review.
    #[test]
    fn dropped_ring_full_returns_ownership_of_the_rejected_handles_for_explicit_destruction() {
        let mut ring: GpuQueryRing<u32> = GpuQueryRing::new(1);
        let mut source = FakeQuerySource::default();

        assert_eq!(ring.issue(PHASE, 1, 2, 0), SampleIssueOutcome::Issued);

        let outcome = ring.issue(PHASE, 3, 4, 0);
        let SampleIssueOutcome::DroppedRingFull { start, end } = outcome else {
            panic!("expected DroppedRingFull, got {outcome:?}");
        };
        assert_eq!(start, 3, "must be the exact handle the caller passed");
        assert_eq!(end, 4, "must be the exact handle the caller passed");

        // The ring's own accounting reflects exactly the one accepted
        // sample -- it never stored, and therefore never owns, the
        // rejected pair.
        assert_eq!(ring.len(), 1);
        assert_eq!(ring.dropped_sample_count(), 1);

        // Nothing the ring itself does (poll, drain_and_destroy) can ever
        // reach handles 3/4, because it never stored them. The caller is
        // the only path to destroying them, exactly as the trait doc
        // requires.
        assert_eq!(source.destroy_calls, []);
        source.destroy_query(start);
        source.destroy_query(end);
        assert_eq!(source.destroy_calls, vec![3, 4]);
    }

    /// Pins the saturating-add fix: the dropped-sample counter must never
    /// overflow-panic (debug builds) or silently wrap (release builds) even
    /// if a pathologically stuck driver keeps the ring full forever. Sets
    /// the counter directly (private-field access from the same module)
    /// rather than looping `u64::MAX` times.
    #[test]
    fn dropped_sample_count_saturates_rather_than_overflowing() {
        let mut ring: GpuQueryRing<u32> = GpuQueryRing::new(0);
        ring.dropped_samples = u64::MAX;

        let outcome = ring.issue(PHASE, 1, 2, 0);

        assert!(matches!(
            outcome,
            SampleIssueOutcome::DroppedRingFull { .. }
        ));
        assert_eq!(ring.dropped_sample_count(), u64::MAX);
    }

    // ── GpuQueryRing: wraparound / slot reuse as samples retire ──────────

    #[test]
    fn a_retired_slot_can_be_reused_by_a_later_issue() {
        let mut ring: GpuQueryRing<u32> = GpuQueryRing::new(1);
        let mut source = FakeQuerySource::default();

        assert_eq!(ring.issue(PHASE, 1, 2, 0), SampleIssueOutcome::Issued);
        // Ring is now full: a second issue is dropped, and the caller gets
        // its rejected handles back rather than losing track of them.
        assert_eq!(
            ring.issue(PHASE, 3, 4, 0),
            SampleIssueOutcome::DroppedRingFull { start: 3, end: 4 }
        );

        // Retire the first sample.
        source.mark_available(1, 10);
        source.mark_available(2, 20);
        let completed = ring.poll(1, &mut source);
        assert_eq!(completed.len(), 1);
        assert!(ring.is_empty());

        // The freed slot can now be reused without being treated as a drop.
        assert_eq!(ring.issue(PHASE, 5, 6, 1), SampleIssueOutcome::Issued);
        assert_eq!(ring.dropped_sample_count(), 1, "still just the one drop");
        assert_eq!(ring.len(), 1);

        source.mark_available(5, 30);
        source.mark_available(6, 40);
        let completed = ring.poll(2, &mut source);
        assert_eq!(completed.len(), 1);
        assert_eq!(completed[0].start_ns, 30);
        assert_eq!(completed[0].end_ns, 40);
    }

    #[test]
    fn many_cycles_of_full_ring_retire_and_reuse_never_exceed_capacity() {
        let mut ring: GpuQueryRing<u32> = GpuQueryRing::new(3);
        let mut source = FakeQuerySource::default();
        let mut next_handle = 0u32;
        let mut frame = 0u64;

        for _ in 0..10 {
            // Fill to capacity.
            for _ in 0..3 {
                let start = next_handle;
                let end = next_handle + 1;
                next_handle += 2;
                assert_eq!(
                    ring.issue(PHASE, start, end, frame),
                    SampleIssueOutcome::Issued
                );
                assert!(ring.len() <= ring.capacity());
            }
            // One more must be dropped (ring is full), with its handles
            // handed back rather than swallowed.
            assert_eq!(
                ring.issue(PHASE, next_handle, next_handle + 1, frame),
                SampleIssueOutcome::DroppedRingFull {
                    start: next_handle,
                    end: next_handle + 1,
                }
            );

            frame += 1;
            // Mark every currently pending handle available and retire the
            // whole batch this frame.
            for handle in next_handle.saturating_sub(6)..next_handle {
                source.mark_available(handle, u64::from(handle));
            }
            let completed = ring.poll(frame, &mut source);
            assert_eq!(completed.len(), 3);
            assert!(ring.is_empty());
        }

        assert_eq!(ring.dropped_sample_count(), 10);
    }

    // ── GpuQueryRing: explicit cleanup/destruction ───────────────────────

    #[test]
    fn drain_and_destroy_destroys_every_live_handle_and_empties_the_ring() {
        let mut ring: GpuQueryRing<u32> = GpuQueryRing::new(4);
        let mut source = FakeQuerySource::default();

        ring.issue(PHASE, 1, 2, 0);
        ring.issue(PHASE, 3, 4, 0);
        ring.issue(PHASE, 5, 6, 0);

        let destroyed = ring.drain_and_destroy(&mut source);

        assert_eq!(destroyed, 3);
        assert!(ring.is_empty());
        assert_eq!(source.destroy_calls, vec![1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn poll_destroys_handles_of_every_completed_sample_and_no_others() {
        let mut ring: GpuQueryRing<u32> = GpuQueryRing::new(4);
        let mut source = FakeQuerySource::default();

        ring.issue(PHASE, 1, 2, 0);
        ring.issue(PHASE, 3, 4, 0);

        // Only the first sample's handles become available.
        source.mark_available(1, 10);
        source.mark_available(2, 20);
        let completed = ring.poll(1, &mut source);

        assert_eq!(completed.len(), 1);
        assert_eq!(source.destroy_calls, vec![1, 2]);
        assert_eq!(ring.len(), 1, "the still-pending sample is untouched");
    }

    // ── GpuTimerCapability: unsupported / GLES ───────────────────────────

    #[test]
    fn gles_is_always_unavailable_regardless_of_version() {
        let info = GlVersionInfo {
            profile: GlProfile::Gles,
            version: GlVersion { major: 3, minor: 2 },
        };
        assert_eq!(
            GpuTimerCapability::detect(info, &HashSet::new()),
            GpuTimerCapability::Unavailable
        );

        // Even a GLES context whose extension set happens to contain the
        // desktop ARB string must not be treated as available -- profile
        // gates first.
        let mut extensions = HashSet::new();
        extensions.insert("GL_ARB_timer_query".to_owned());
        assert_eq!(
            GpuTimerCapability::detect(info, &extensions),
            GpuTimerCapability::Unavailable
        );
    }

    #[test]
    fn desktop_below_3_3_without_extension_is_unavailable() {
        let info = GlVersionInfo {
            profile: GlProfile::Desktop,
            version: GlVersion { major: 3, minor: 2 },
        };
        assert_eq!(
            GpuTimerCapability::detect(info, &HashSet::new()),
            GpuTimerCapability::Unavailable
        );
    }

    // ── GpuTimerCapability / parsing: desktop version and extension ─────

    #[test]
    fn desktop_3_3_exactly_is_available_without_the_extension() {
        let info = GlVersionInfo {
            profile: GlProfile::Desktop,
            version: GlVersion { major: 3, minor: 3 },
        };
        assert_eq!(
            GpuTimerCapability::detect(info, &HashSet::new()),
            GpuTimerCapability::Available
        );
    }

    #[test]
    fn desktop_newer_major_version_is_available() {
        let info = GlVersionInfo {
            profile: GlProfile::Desktop,
            version: GlVersion { major: 4, minor: 6 },
        };
        assert_eq!(
            GpuTimerCapability::detect(info, &HashSet::new()),
            GpuTimerCapability::Available
        );
    }

    #[test]
    fn desktop_below_3_3_with_arb_extension_is_available() {
        let info = GlVersionInfo {
            profile: GlProfile::Desktop,
            version: GlVersion { major: 3, minor: 1 },
        };
        let mut extensions = HashSet::new();
        extensions.insert("GL_ARB_timer_query".to_owned());
        assert_eq!(
            GpuTimerCapability::detect(info, &extensions),
            GpuTimerCapability::Available
        );
    }

    #[test]
    fn parse_gl_version_string_parses_plain_desktop_version() {
        let info = parse_gl_version_string("3.3.0 NVIDIA 470.63.01").expect("should parse");
        assert_eq!(info.profile, GlProfile::Desktop);
        assert_eq!(info.version, GlVersion { major: 3, minor: 3 });
    }

    #[test]
    fn parse_gl_version_string_parses_desktop_version_with_profile_annotation() {
        let info =
            parse_gl_version_string("4.6 (Compatibility Profile) Mesa 23.2.1").expect("parse");
        assert_eq!(info.profile, GlProfile::Desktop);
        assert_eq!(info.version, GlVersion { major: 4, minor: 6 });
    }

    #[test]
    fn parse_gl_version_string_parses_gles_version() {
        let info = parse_gl_version_string("OpenGL ES 3.2 Mesa 23.2.1").expect("parse");
        assert_eq!(info.profile, GlProfile::Gles);
        assert_eq!(info.version, GlVersion { major: 3, minor: 2 });
    }

    #[test]
    fn parse_gl_version_string_parses_bare_gles_version() {
        let info = parse_gl_version_string("OpenGL ES 3.2").expect("parse");
        assert_eq!(info.profile, GlProfile::Gles);
        assert_eq!(info.version, GlVersion { major: 3, minor: 2 });
    }

    #[test]
    fn parse_gl_version_string_rejects_unparseable_input() {
        assert_eq!(parse_gl_version_string(""), None);
        assert_eq!(parse_gl_version_string("garbage"), None);
        assert_eq!(parse_gl_version_string("OpenGL ES garbage"), None);
        assert_eq!(parse_gl_version_string("3"), None);
    }

    // ── CompletedSample: pure accessor edge case ─────────────────────────

    #[test]
    fn duration_ns_saturates_rather_than_panicking_on_end_before_start() {
        let sample = CompletedSample {
            phase: PHASE,
            issue_frame: 0,
            completed_frame: 1,
            start_ns: 500,
            end_ns: 100,
        };
        assert_eq!(sample.duration_ns(), 0);
    }

    // ── SampleIssueOutcome / GpuProfilePhase: basic equality ─────────────

    #[test]
    fn gpu_profile_phase_equality_and_debug() {
        let a = GpuProfilePhase("chrome_paint");
        let b = GpuProfilePhase("chrome_paint");
        let c = GpuProfilePhase("terminal_draw");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert!(format!("{a:?}").contains("chrome_paint"));
    }
}
