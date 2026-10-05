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
//!
//! # Frame-phase timing (Task 125.9)
//!
//! Below the ring, [`FrameGpuTiming`] is the windowing layer's own consumer
//! of it: a pure, GL-free state machine that turns the eight
//! [`FramePhaseBoundary`] stamps `paint_frame` emits on a painted frame into
//! four asynchronous timestamp pairs (chrome head, terminal band, chrome
//! tail, and the total clear/textures/paint/free interval). Like the ring, it
//! is driven entirely through a trait ([`GpuTimestampSource`]), so every
//! lifecycle rule is tested with fake handles; [`GlowTimestampSource`] is the
//! thin real-GL implementation.
//!
//! It lives in this file rather than its own module because the plan scopes
//! 125.9 to this file, `frame_paint.rs`, `egui_integration.rs` and
//! `PROFILING.md`; it is a second concept layered on the first and is the
//! natural first candidate to move to its own module if this file grows
//! again.

use std::collections::{HashSet, VecDeque};

use glow::HasContext;

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
    ///
    /// Saturating: [`GpuQueryRing::poll`] never completes a sample before its
    /// issue frame, but this reporting-only accessor must not panic (debug) or
    /// wrap to a huge latency (release) if a caller constructs one that did.
    #[must_use]
    pub const fn latency_frames(&self) -> u64 {
        self.completed_frame.saturating_sub(self.issue_frame)
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

// ---------------------------------------------------------------------------
// Task 125.9: chrome / terminal-band / total-frame GPU timing
// ---------------------------------------------------------------------------

/// The phase name for the chrome painted before the terminal band (the
/// head primitives of `paint_frame`'s head/band/tail split).
pub const PHASE_CHROME_HEAD: GpuProfilePhase = GpuProfilePhase("chrome_head");

/// The phase name for the terminal band's `paint_primitives` call -- the
/// span that contains the terminal's own `PaintCallback` GL work.
pub const PHASE_TERMINAL_BAND: GpuProfilePhase = GpuProfilePhase("terminal_band");

/// The phase name for the chrome painted after the terminal band.
pub const PHASE_CHROME_TAIL: GpuProfilePhase = GpuProfilePhase("chrome_tail");

/// The phase name for the total GPU interval of a painted frame: from just
/// before the clear to just after the texture frees. Excludes the buffer
/// swap and any compositor latency.
pub const PHASE_FRAME_TOTAL: GpuProfilePhase = GpuProfilePhase("frame_total");

/// Explicit `tracing` target for [`FrameGpuTiming`]'s periodic flush summary.
///
/// Distinct from every other Task 121/125 profiling target
/// (`"freminal_windowing::frame_profiling"`, `"freminal::task_125::gpu_timing"`
/// and the rest) so a live session can filter windowing-side GPU timing
/// independently of any of them.
pub const LOG_TARGET: &str = "freminal_windowing::task_125::gpu_timing";

/// How many simultaneously pending samples one window's ring tolerates.
///
/// A painted frame issues four samples (head, band, tail, total), so this
/// bounds a GPU-side backlog of roughly 16 painted frames' worth of unretired
/// queries -- generous for the one-to-few frames a query takes to retire,
/// while still bounded if a driver stops signalling availability.
const FRAME_TIMING_RING_CAPACITY: usize = 64;

/// Emit a flush-worthy summary once every this many completed
/// [`PHASE_FRAME_TOTAL`] samples (one per painted frame when capability is
/// available).
pub const FRAME_TIMING_FLUSH_EVERY: u64 = 60;

/// The eight GPU-timeline boundaries of one painted frame, in the exact
/// order `paint_frame` emits them.
///
/// Named variants rather than a `(phase, bool)` pair, per
/// `freminal-state-representation`: "start" vs "end" is a real branch in the
/// lifecycle, not an on/off flag. Adjacent boundaries that share a moment
/// (`HeadEnd`/`BandStart`, `BandEnd`/`TailStart`) are still two separate
/// calls, each of which creates its own query handle -- the ring destroys
/// every handle exactly once, so a handle is never shared between two
/// samples.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FramePhaseBoundary {
    /// Immediately before the clear: opens [`PHASE_FRAME_TOTAL`].
    TotalStart,
    /// After the texture uploads, immediately before head paint: opens
    /// [`PHASE_CHROME_HEAD`].
    HeadStart,
    /// Immediately after head paint: closes [`PHASE_CHROME_HEAD`].
    HeadEnd,
    /// Immediately before band paint: opens [`PHASE_TERMINAL_BAND`].
    BandStart,
    /// Immediately after band paint: closes [`PHASE_TERMINAL_BAND`].
    BandEnd,
    /// Immediately before tail paint: opens [`PHASE_CHROME_TAIL`].
    TailStart,
    /// Immediately after tail paint: closes [`PHASE_CHROME_TAIL`].
    TailEnd,
    /// Immediately after the texture frees: closes [`PHASE_FRAME_TOTAL`].
    TotalEnd,
}

/// Creates timestamp queries on demand, in addition to the availability /
/// read / destroy calls of [`GpuQuerySource`].
pub trait GpuTimestampSource<Q>: GpuQuerySource<Q> {
    /// Create a query object and record the current GPU timestamp into it
    /// (`glGenQueries` + `glQueryCounter(query, GL_TIMESTAMP)`), returning
    /// `None` if the query object could not be created.
    ///
    /// The caller owns the returned handle and must eventually route it
    /// through [`GpuQuerySource::destroy_query`] (directly or by handing it
    /// to a [`GpuQueryRing`]).
    fn stamp(&mut self) -> Option<Q>;
}

/// The four timed spans of a painted frame; private bookkeeping for the
/// start/end pairing in [`FrameGpuTiming`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TimedPhase {
    Total,
    Head,
    Band,
    Tail,
}

impl TimedPhase {
    const fn name(self) -> GpuProfilePhase {
        match self {
            Self::Total => PHASE_FRAME_TOTAL,
            Self::Head => PHASE_CHROME_HEAD,
            Self::Band => PHASE_TERMINAL_BAND,
            Self::Tail => PHASE_CHROME_TAIL,
        }
    }
}

/// Cumulative measured duration and completed-sample count for one phase.
#[derive(Debug, Clone, Copy, Default)]
struct PhaseTotals {
    ns: u64,
    samples: u64,
}

impl PhaseTotals {
    const fn record(&mut self, duration_ns: u64) {
        self.ns = self.ns.saturating_add(duration_ns);
        self.samples = self.samples.saturating_add(1);
    }
}

/// A plain-data snapshot of one window's cumulative frame GPU timing,
/// produced by [`FrameGpuTiming::report`].
///
/// Deliberately not a borrow of the live state machine: the caller logs it
/// after `paint_frame` has returned, so nothing here may reference GL state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameGpuTimingReport {
    /// Whether this context supports asynchronous timer queries at all.
    /// [`GpuTimerCapability::Unavailable`] means every counter below is
    /// permanently zero -- a structural absence, not a transient gap.
    pub capability: GpuTimerCapability,
    /// The `GL_RENDERER` string, captured once at detection time. Empty when
    /// capability is [`GpuTimerCapability::Unavailable`].
    pub renderer_string: String,
    /// Cumulative chrome GPU time, nanoseconds: head plus tail. The two spans
    /// are measured separately and summed; the terminal band between them is
    /// not part of this figure.
    pub chrome_ns_total: u64,
    /// Painted frames whose chrome (head and tail) has been measured: the
    /// lower of the head and tail completed-sample counts. They differ by at
    /// most the few samples in flight at the instant of the report.
    pub chrome_sample_count: u64,
    /// Cumulative terminal-band GPU time, nanoseconds. A cross-check against
    /// the terminal renderer's own upload + draw timings, never a
    /// subtraction-based substitute for them.
    pub band_ns_total: u64,
    /// Completed terminal-band samples.
    pub band_sample_count: u64,
    /// Cumulative total GPU time, nanoseconds, from just before the clear to
    /// just after the texture frees. Excludes swap and compositor latency.
    pub total_ns_total: u64,
    /// Completed total-frame samples.
    pub total_sample_count: u64,
    /// Frames between issue and availability for the most recently completed
    /// sample (any phase), in this window's own painted frames -- see
    /// [`FrameGpuTiming::begin_frame`].
    pub last_latency_frames: u64,
    /// Cumulative samples rejected because the ring was full.
    pub dropped_sample_count: u64,
    /// Samples currently issued but not yet read back.
    pub pending_sample_count: usize,
}

/// One window's asynchronous frame-phase GPU timing state machine.
///
/// Owns the [`GpuQueryRing`], the detected capability, a painted-frame
/// counter, the start handles of spans currently open, and cumulative totals.
/// Never touches GL itself: every query operation goes through the
/// [`GpuTimestampSource`] passed to each method.
///
/// # Call sequence a caller must follow
///
/// For each **painted** frame only -- a frame that submits no GPU work (one
/// whose damage is `FrameDamage::None`) must call nothing here, because it
/// has no GPU interval to measure:
///
/// 1. [`Self::begin_frame`] -- polls pending samples (never the ones issued
///    this frame), then advances the frame counter.
/// 2. [`Self::mark`] for each [`FramePhaseBoundary`], in declaration order,
///    with the frame number `begin_frame` returned.
/// 3. [`Self::take_flush_signal`] / [`Self::report`] as desired.
///
/// And once, when the owning GL context is about to be destroyed:
/// [`Self::shutdown`].
///
/// Every method is a no-op when capability is
/// [`GpuTimerCapability::Unavailable`]: no query is ever created, so an
/// unsupported context renders exactly as if this type were absent.
#[derive(Debug)]
pub struct FrameGpuTiming<Q> {
    ring: GpuQueryRing<Q>,
    capability: GpuTimerCapability,
    renderer_string: String,
    /// This window's painted-frame counter: advanced once per
    /// [`Self::begin_frame`], i.e. once per frame that submitted GPU work.
    /// Skipped (`FrameDamage::None`) frames do not advance it, so latencies
    /// derived from it are in painted frames, not window frames.
    frame: u64,
    pending_total: Option<Q>,
    pending_head: Option<Q>,
    pending_band: Option<Q>,
    pending_tail: Option<Q>,
    head: PhaseTotals,
    band: PhaseTotals,
    tail: PhaseTotals,
    total: PhaseTotals,
    last_latency_frames: u64,
    flush_signal: bool,
}

impl<Q> FrameGpuTiming<Q> {
    /// Construct a timing state with the given detected `capability` and
    /// `GL_RENDERER` string (see [`detect_from_glow`] for the real-context
    /// path) and all-zero totals.
    #[must_use]
    pub fn new(capability: GpuTimerCapability, renderer_string: String) -> Self {
        Self::with_capacity(capability, renderer_string, FRAME_TIMING_RING_CAPACITY)
    }

    /// As [`Self::new`] with an explicit ring capacity, used by this module's
    /// tests to exercise the full-ring path without issuing 64 samples.
    fn with_capacity(
        capability: GpuTimerCapability,
        renderer_string: String,
        capacity: usize,
    ) -> Self {
        Self {
            ring: GpuQueryRing::new(capacity),
            capability,
            renderer_string,
            frame: 0,
            pending_total: None,
            pending_head: None,
            pending_band: None,
            pending_tail: None,
            head: PhaseTotals::default(),
            band: PhaseTotals::default(),
            tail: PhaseTotals::default(),
            total: PhaseTotals::default(),
            last_latency_frames: 0,
            flush_signal: false,
        }
    }

    /// The detected capability.
    #[must_use]
    pub const fn capability(&self) -> GpuTimerCapability {
        self.capability
    }

    /// Whether any query may be issued -- the gate every other method checks
    /// before touching its source.
    const fn is_available(&self) -> bool {
        matches!(self.capability, GpuTimerCapability::Available)
    }

    /// Poll pending samples and return the frame number this painted frame's
    /// samples must be issued against.
    ///
    /// Call exactly once per painted frame, before the first
    /// [`Self::mark`]. Polling happens here, strictly before this frame's own
    /// samples exist, and the ring additionally refuses to poll a sample on
    /// its issue frame, so no result is ever read on the frame that issued
    /// it. The counter advances in painted frames of this window.
    pub fn begin_frame<S: GpuTimestampSource<Q>>(&mut self, source: &mut S) -> u64 {
        let frame = self.frame;
        if self.is_available() {
            for sample in self.ring.poll(frame, source) {
                self.record_completed(&sample);
            }
        }
        self.frame = self.frame.saturating_add(1);
        frame
    }

    /// Record one frame-phase boundary.
    ///
    /// A start boundary creates and stores a start timestamp. An end boundary
    /// creates the end timestamp and issues the completed pair to the ring
    /// under `frame`; an end with no matching start (the start's stamp
    /// failed) does nothing and creates no query. If the ring rejects the
    /// pair, both handles are destroyed here -- the ring never took
    /// ownership of them.
    pub fn mark<S: GpuTimestampSource<Q>>(
        &mut self,
        boundary: FramePhaseBoundary,
        frame: u64,
        source: &mut S,
    ) {
        if !self.is_available() {
            return;
        }
        match boundary {
            FramePhaseBoundary::TotalStart => self.open_phase(TimedPhase::Total, source),
            FramePhaseBoundary::HeadStart => self.open_phase(TimedPhase::Head, source),
            FramePhaseBoundary::HeadEnd => self.close_phase(TimedPhase::Head, frame, source),
            FramePhaseBoundary::BandStart => self.open_phase(TimedPhase::Band, source),
            FramePhaseBoundary::BandEnd => self.close_phase(TimedPhase::Band, frame, source),
            FramePhaseBoundary::TailStart => self.open_phase(TimedPhase::Tail, source),
            FramePhaseBoundary::TailEnd => self.close_phase(TimedPhase::Tail, frame, source),
            FramePhaseBoundary::TotalEnd => self.close_phase(TimedPhase::Total, frame, source),
        }
    }

    /// The pending-start slot for `phase`.
    const fn start_slot(&mut self, phase: TimedPhase) -> &mut Option<Q> {
        match phase {
            TimedPhase::Total => &mut self.pending_total,
            TimedPhase::Head => &mut self.pending_head,
            TimedPhase::Band => &mut self.pending_band,
            TimedPhase::Tail => &mut self.pending_tail,
        }
    }

    fn open_phase<S: GpuTimestampSource<Q>>(&mut self, phase: TimedPhase, source: &mut S) {
        // Defensive: a well-behaved caller always closes a span before
        // reopening it. A stale handle is destroyed rather than leaked.
        if let Some(stale) = self.start_slot(phase).take() {
            source.destroy_query(stale);
        }
        *self.start_slot(phase) = source.stamp();
    }

    fn close_phase<S: GpuTimestampSource<Q>>(
        &mut self,
        phase: TimedPhase,
        frame: u64,
        source: &mut S,
    ) {
        let Some(start) = self.start_slot(phase).take() else {
            return;
        };
        let Some(end) = source.stamp() else {
            source.destroy_query(start);
            return;
        };
        if let SampleIssueOutcome::DroppedRingFull { start, end } =
            self.ring.issue(phase.name(), start, end, frame)
        {
            source.destroy_query(start);
            source.destroy_query(end);
        }
    }

    /// Fold one completed sample into the cumulative totals and latch the
    /// flush signal when a [`FRAME_TIMING_FLUSH_EVERY`] boundary of completed
    /// [`PHASE_FRAME_TOTAL`] samples is crossed.
    fn record_completed(&mut self, sample: &CompletedSample) {
        self.last_latency_frames = sample.latency_frames();
        let duration = sample.duration_ns();
        if sample.phase == PHASE_CHROME_HEAD {
            self.head.record(duration);
        } else if sample.phase == PHASE_TERMINAL_BAND {
            self.band.record(duration);
        } else if sample.phase == PHASE_CHROME_TAIL {
            self.tail.record(duration);
        } else if sample.phase == PHASE_FRAME_TOTAL {
            self.total.record(duration);
            if self.total.samples.is_multiple_of(FRAME_TIMING_FLUSH_EVERY) {
                self.flush_signal = true;
            }
        }
    }

    /// Consume the pending flush signal: `true` at most once per crossed
    /// [`FRAME_TIMING_FLUSH_EVERY`] boundary.
    #[must_use]
    pub fn take_flush_signal(&mut self) -> bool {
        std::mem::take(&mut self.flush_signal)
    }

    /// Snapshot the cumulative totals for logging.
    #[must_use]
    pub fn report(&self) -> FrameGpuTimingReport {
        FrameGpuTimingReport {
            capability: self.capability,
            renderer_string: self.renderer_string.clone(),
            chrome_ns_total: self.head.ns.saturating_add(self.tail.ns),
            chrome_sample_count: self.head.samples.min(self.tail.samples),
            band_ns_total: self.band.ns,
            band_sample_count: self.band.samples,
            total_ns_total: self.total.ns,
            total_sample_count: self.total.samples,
            last_latency_frames: self.last_latency_frames,
            dropped_sample_count: self.ring.dropped_sample_count(),
            pending_sample_count: self.ring.len(),
        }
    }

    /// Destroy every query this state still owns -- open start handles first,
    /// then every pending ring sample -- without reading any result. Call
    /// once, with the GL context still current, before it is destroyed.
    pub fn shutdown<S: GpuTimestampSource<Q>>(&mut self, source: &mut S) {
        if !self.is_available() {
            return;
        }
        for phase in [
            TimedPhase::Total,
            TimedPhase::Head,
            TimedPhase::Band,
            TimedPhase::Tail,
        ] {
            if let Some(query) = self.start_slot(phase).take() {
                source.destroy_query(query);
            }
        }
        self.ring.drain_and_destroy(source);
    }
}

/// Decide timer-query capability from a raw `GL_VERSION` string and the
/// context's extension set. An unparsable version is conservatively
/// [`GpuTimerCapability::Unavailable`] rather than guessed.
#[must_use]
fn detect_capability(version_string: &str, extensions: &HashSet<String>) -> GpuTimerCapability {
    parse_gl_version_string(version_string).map_or(GpuTimerCapability::Unavailable, |info| {
        GpuTimerCapability::detect(info, extensions)
    })
}

/// Detect capability on a real context and build a [`FrameGpuTiming`] for it.
///
/// Reads `GL_VERSION` and the extension set, and reads `GL_RENDERER` only
/// when capability resolved [`GpuTimerCapability::Available`]: an unsupported
/// context never reports anything, so it needs no renderer string. Call once
/// per context, with that context current.
#[must_use]
pub fn detect_from_glow(gl: &glow::Context) -> FrameGpuTiming<glow::Query> {
    // SAFETY: `gl` is a live context the caller has current; reading a
    // string parameter has no preconditions beyond that.
    let version_string = unsafe { gl.get_parameter_string(glow::VERSION) };
    let capability = detect_capability(&version_string, gl.supported_extensions());
    let renderer_string = match capability {
        GpuTimerCapability::Available => {
            // SAFETY: as above.
            unsafe { gl.get_parameter_string(glow::RENDERER) }
        }
        GpuTimerCapability::Unavailable => String::new(),
    };
    FrameGpuTiming::new(capability, renderer_string)
}

/// The real-GL [`GpuTimestampSource`], borrowing the window's context for the
/// duration of one call sequence.
pub struct GlowTimestampSource<'a> {
    gl: &'a glow::Context,
}

impl<'a> GlowTimestampSource<'a> {
    /// Wrap `gl`. The context must be current whenever a method of the
    /// returned source (or of anything driving it) is called.
    #[must_use]
    pub const fn new(gl: &'a glow::Context) -> Self {
        Self { gl }
    }
}

impl GpuQuerySource<glow::Query> for GlowTimestampSource<'_> {
    fn is_result_available(&mut self, query: &glow::Query) -> bool {
        // SAFETY: `query` was created by `stamp` on this same context and
        // has not been destroyed -- the ring never checks a handle after
        // `destroy_query` has run for it. This is the non-blocking
        // `GL_QUERY_RESULT_AVAILABLE` form.
        unsafe {
            self.gl
                .get_query_parameter_u32(*query, glow::QUERY_RESULT_AVAILABLE)
                != 0
        }
    }

    fn read_result_ns(&mut self, query: &glow::Query) -> u64 {
        // SAFETY: the ring only calls this immediately after
        // `is_result_available` returned `true` for the same query, so the
        // read cannot stall the pipeline.
        unsafe { self.gl.get_query_parameter_u64(*query, glow::QUERY_RESULT) }
    }

    fn destroy_query(&mut self, query: glow::Query) {
        // SAFETY: `query` is a live handle created on this context and not
        // yet destroyed; ownership is consumed here.
        unsafe { self.gl.delete_query(query) };
    }
}

impl GpuTimestampSource<glow::Query> for GlowTimestampSource<'_> {
    fn stamp(&mut self) -> Option<glow::Query> {
        // SAFETY: the context is current (see `new`).
        let query = unsafe { self.gl.create_query() }.ok()?;
        // SAFETY: `query` was just created on this context. `GL_TIMESTAMP`
        // via `glQueryCounter` records the GPU timestamp when the preceding
        // commands retire and never blocks the CPU.
        unsafe { self.gl.query_counter(query, glow::TIMESTAMP) };
        Some(query)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::{
        CompletedSample, FRAME_TIMING_FLUSH_EVERY, FrameGpuTiming, FramePhaseBoundary, GlProfile,
        GlVersion, GlVersionInfo, GpuProfilePhase, GpuQueryRing, GpuQuerySource,
        GpuTimerCapability, GpuTimestampSource, PHASE_CHROME_HEAD, PHASE_CHROME_TAIL,
        PHASE_FRAME_TOTAL, PHASE_TERMINAL_BAND, SampleIssueOutcome, detect_capability,
        parse_gl_version_string,
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
        /// The handle most recently created by `stamp` (0 before the first).
        last_stamped: u32,
        /// Every handle `stamp` created, in order.
        stamped: Vec<u32>,
        /// How many times `stamp` was called, successful or not.
        stamp_attempts: usize,
        /// 1-based `stamp` call numbers that must fail (return `None`).
        failing_stamp_attempts: HashSet<usize>,
    }

    impl FakeQuerySource {
        fn mark_available(&mut self, handle: u32, result_ns: u64) {
            self.available.insert(handle);
            self.results.insert(handle, result_ns);
        }
    }

    impl GpuTimestampSource<u32> for FakeQuerySource {
        fn stamp(&mut self) -> Option<u32> {
            self.stamp_attempts += 1;
            if self.failing_stamp_attempts.contains(&self.stamp_attempts) {
                return None;
            }
            self.last_stamped += 1;
            self.stamped.push(self.last_stamped);
            Some(self.last_stamped)
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

    /// A sample that claims completion before issue must report zero latency,
    /// not panic in debug or wrap to a huge value in release.
    #[test]
    fn latency_frames_saturates_when_completed_before_issued() {
        let sample = CompletedSample {
            phase: PHASE,
            issue_frame: 10,
            completed_frame: 7,
            start_ns: 0,
            end_ns: 0,
        };
        assert_eq!(sample.latency_frames(), 0);
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

    // ── FrameGpuTiming (Task 125.9) ──────────────────────────────────────

    use FramePhaseBoundary::{
        BandEnd, BandStart, HeadEnd, HeadStart, TailEnd, TailStart, TotalEnd, TotalStart,
    };

    /// Every boundary of one painted frame, in the order `paint_frame` emits
    /// them.
    const ALL_BOUNDARIES: [FramePhaseBoundary; 8] = [
        TotalStart, HeadStart, HeadEnd, BandStart, BandEnd, TailStart, TailEnd, TotalEnd,
    ];

    fn available_timing() -> FrameGpuTiming<u32> {
        FrameGpuTiming::new(GpuTimerCapability::Available, "Test GPU".to_owned())
    }

    /// Run one painted frame: `begin_frame`, then all eight boundaries.
    /// Returns the frame number.
    fn paint_frame_once(timing: &mut FrameGpuTiming<u32>, source: &mut FakeQuerySource) -> u64 {
        let frame = timing.begin_frame(source);
        for boundary in ALL_BOUNDARIES {
            timing.mark(boundary, frame, source);
        }
        frame
    }

    /// Make every handle `source` has created so far available, reporting
    /// the handle number as its nanosecond timestamp.
    fn make_all_available(source: &mut FakeQuerySource) {
        for handle in source.stamped.clone() {
            source.mark_available(handle, u64::from(handle));
        }
    }

    /// Every handle ever created was destroyed exactly once.
    fn assert_no_leaks(source: &FakeQuerySource) {
        let mut created = source.stamped.clone();
        let mut destroyed = source.destroy_calls.clone();
        created.sort_unstable();
        destroyed.sort_unstable();
        assert_eq!(
            destroyed, created,
            "every created query must be destroyed exactly once"
        );
    }

    #[test]
    fn a_painted_frame_stamps_eight_boundaries_forming_four_phases_in_order() {
        let mut timing = available_timing();
        let mut source = FakeQuerySource::default();

        let frame = paint_frame_once(&mut timing, &mut source);

        assert_eq!(frame, 0);
        assert_eq!(source.stamped, vec![1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(source.stamp_attempts, 8);
        assert_eq!(timing.report().pending_sample_count, 4);
        assert_eq!(source.destroy_calls, [], "nothing destroyed while pending");

        // The pairs, in issue order: head (2,3), band (4,5), tail (6,7),
        // total (1,8). Availability is keyed off each pair's end handle.
        make_all_available(&mut source);
        timing.begin_frame(&mut source);
        assert_eq!(source.availability_calls, vec![3, 5, 7, 8]);
        assert_eq!(source.read_calls, vec![2, 3, 4, 5, 6, 7, 1, 8]);
    }

    #[test]
    fn nothing_is_polled_or_read_on_the_issue_frame() {
        let mut timing = available_timing();
        let mut source = FakeQuerySource::default();
        // Even a source that would report everything ready must not be asked
        // during the issuing frame.
        source.mark_available(1, 1);
        source.mark_available(2, 2);

        let _ = paint_frame_once(&mut timing, &mut source);

        assert_eq!(source.availability_calls, []);
        assert_eq!(source.read_calls, []);
        assert_eq!(timing.report().total_sample_count, 0);
    }

    #[test]
    fn results_arrive_on_a_later_frame_with_per_phase_durations() {
        let mut timing = available_timing();
        let mut source = FakeQuerySource::default();
        let _ = paint_frame_once(&mut timing, &mut source);

        // Handles: total 1..8, head 2..3, band 4..5, tail 6..7.
        for (handle, ns) in [
            (1, 1_000),
            (2, 1_100),
            (3, 1_400), // head: 300
            (4, 1_500),
            (5, 2_500), // band: 1_000
            (6, 2_600),
            (7, 2_700), // tail: 100
            (8, 3_000), // total: 2_000
        ] {
            source.mark_available(handle, ns);
        }

        let next = timing.begin_frame(&mut source);
        assert_eq!(next, 1);

        let report = timing.report();
        assert_eq!(report.capability, GpuTimerCapability::Available);
        assert_eq!(report.renderer_string, "Test GPU");
        assert_eq!(report.band_ns_total, 1_000);
        assert_eq!(report.band_sample_count, 1);
        assert_eq!(report.total_ns_total, 2_000);
        assert_eq!(report.total_sample_count, 1);
        assert_eq!(
            report.chrome_ns_total, 400,
            "chrome is head (300) + tail (100), not the band"
        );
        assert_eq!(report.chrome_sample_count, 1);
        assert_eq!(report.last_latency_frames, 1);
        assert_eq!(report.pending_sample_count, 0);
        assert_eq!(report.dropped_sample_count, 0);
        assert_no_leaks(&source);
    }

    #[test]
    fn latency_is_measured_in_painted_frames() {
        let mut timing = available_timing();
        let mut source = FakeQuerySource::default();
        let _ = paint_frame_once(&mut timing, &mut source);

        // Two later painted frames with nothing available yet.
        let _ = timing.begin_frame(&mut source);
        let _ = timing.begin_frame(&mut source);
        assert_eq!(timing.report().total_sample_count, 0);
        assert_eq!(timing.report().pending_sample_count, 4);

        make_all_available(&mut source);
        let frame = timing.begin_frame(&mut source);
        assert_eq!(frame, 3);
        assert_eq!(timing.report().last_latency_frames, 3);
    }

    #[test]
    fn a_full_ring_destroys_both_handles_of_each_rejected_pair_and_counts_them() {
        let mut timing: FrameGpuTiming<u32> =
            FrameGpuTiming::with_capacity(GpuTimerCapability::Available, String::new(), 2);
        let mut source = FakeQuerySource::default();

        let _ = paint_frame_once(&mut timing, &mut source);

        // Head (2,3) and band (4,5) fit; tail (6,7) and total (1,8) are
        // rejected, and both handles of each come straight back.
        let report = timing.report();
        assert_eq!(report.pending_sample_count, 2);
        assert_eq!(report.dropped_sample_count, 2);
        assert_eq!(source.destroy_calls, vec![6, 7, 1, 8]);

        timing.shutdown(&mut source);
        assert_no_leaks(&source);
    }

    #[test]
    fn an_unavailable_context_issues_nothing_at_all() {
        let mut timing: FrameGpuTiming<u32> =
            FrameGpuTiming::new(GpuTimerCapability::Unavailable, String::new());
        let mut source = FakeQuerySource::default();
        source.mark_available(1, 1);

        for _ in 0..3 {
            let _ = paint_frame_once(&mut timing, &mut source);
        }
        timing.shutdown(&mut source);

        assert_eq!(source.stamp_attempts, 0);
        assert_eq!(source.availability_calls, []);
        assert_eq!(source.read_calls, []);
        assert_eq!(source.destroy_calls, []);
        let report = timing.report();
        assert_eq!(report.capability, GpuTimerCapability::Unavailable);
        assert_eq!(report.total_sample_count, 0);
        assert_eq!(report.pending_sample_count, 0);
        assert!(!timing.take_flush_signal());
    }

    #[test]
    fn shutdown_destroys_open_starts_then_drains_the_ring() {
        let mut timing = available_timing();
        let mut source = FakeQuerySource::default();
        let frame = timing.begin_frame(&mut source);
        // Mid-frame: total and band spans are open, head is a pending pair.
        for boundary in [TotalStart, HeadStart, HeadEnd, BandStart] {
            timing.mark(boundary, frame, &mut source);
        }
        assert_eq!(source.stamped, vec![1, 2, 3, 4]);

        timing.shutdown(&mut source);

        // Open starts (total=1, band=4) first, then the ring's head pair.
        assert_eq!(source.destroy_calls, vec![1, 4, 2, 3]);
        assert_eq!(timing.report().pending_sample_count, 0);
        assert_no_leaks(&source);

        // Idempotent: a second shutdown has nothing left to destroy.
        timing.shutdown(&mut source);
        assert_eq!(source.destroy_calls.len(), 4);
    }

    #[test]
    fn a_failed_end_stamp_destroys_its_start_and_leaks_nothing() {
        let mut timing = available_timing();
        let mut source = FakeQuerySource::default();
        // Stamp attempts: 1 TotalStart, 2 HeadStart, 3 HeadEnd (fails).
        source.failing_stamp_attempts.insert(3);

        let _ = paint_frame_once(&mut timing, &mut source);

        // Head's start (handle 2) was destroyed; no head sample was issued;
        // band, tail and total still were.
        assert_eq!(source.destroy_calls, vec![2]);
        assert_eq!(timing.report().pending_sample_count, 3);
        timing.shutdown(&mut source);
        assert_no_leaks(&source);
    }

    #[test]
    fn a_failed_start_stamp_makes_its_end_a_no_op() {
        let mut timing = available_timing();
        let mut source = FakeQuerySource::default();
        // Attempt 1 is TotalStart: it fails, so TotalEnd must create nothing.
        source.failing_stamp_attempts.insert(1);

        let _ = paint_frame_once(&mut timing, &mut source);

        // 7 attempts succeeded-or-failed for the other 7 boundaries; the
        // total's end made none.
        assert_eq!(source.stamp_attempts, 7);
        assert_eq!(timing.report().pending_sample_count, 3);
        timing.shutdown(&mut source);
        assert_no_leaks(&source);
    }

    #[test]
    fn reopening_a_span_destroys_the_stale_start() {
        let mut timing = available_timing();
        let mut source = FakeQuerySource::default();
        let frame = timing.begin_frame(&mut source);

        timing.mark(HeadStart, frame, &mut source);
        timing.mark(HeadStart, frame, &mut source);

        assert_eq!(source.stamped, vec![1, 2]);
        assert_eq!(source.destroy_calls, vec![1]);
        timing.shutdown(&mut source);
        assert_no_leaks(&source);
    }

    #[test]
    fn an_end_without_any_start_does_nothing() {
        let mut timing = available_timing();
        let mut source = FakeQuerySource::default();
        let frame = timing.begin_frame(&mut source);

        timing.mark(TailEnd, frame, &mut source);

        assert_eq!(source.stamp_attempts, 0);
        assert_eq!(timing.report().pending_sample_count, 0);
    }

    #[test]
    fn the_flush_signal_latches_once_per_sixty_completed_frame_totals() {
        let mut timing = available_timing();
        let mut source = FakeQuerySource::default();
        let mut flushes_at: Vec<u64> = Vec::new();

        // Each loop iteration paints one frame; its samples complete (all
        // handles are available immediately) on the next iteration's poll.
        for _ in 0..=(FRAME_TIMING_FLUSH_EVERY * 2) {
            let _ = paint_frame_once(&mut timing, &mut source);
            make_all_available(&mut source);
            if timing.take_flush_signal() {
                flushes_at.push(timing.report().total_sample_count);
            }
            assert!(
                !timing.take_flush_signal(),
                "the signal is one-shot: a second take must be false"
            );
        }

        assert_eq!(
            flushes_at,
            vec![FRAME_TIMING_FLUSH_EVERY, FRAME_TIMING_FLUSH_EVERY * 2]
        );
        assert_eq!(timing.report().dropped_sample_count, 0);
    }

    #[test]
    fn phase_names_are_distinct_and_stable() {
        let names = [
            PHASE_CHROME_HEAD,
            PHASE_TERMINAL_BAND,
            PHASE_CHROME_TAIL,
            PHASE_FRAME_TOTAL,
        ];
        assert_eq!(names.iter().collect::<HashSet<_>>().len(), 4);
        assert_eq!(PHASE_CHROME_HEAD.0, "chrome_head");
        assert_eq!(PHASE_TERMINAL_BAND.0, "terminal_band");
        assert_eq!(PHASE_CHROME_TAIL.0, "chrome_tail");
        assert_eq!(PHASE_FRAME_TOTAL.0, "frame_total");
    }

    #[test]
    fn detect_capability_resolves_from_version_string_and_extensions() {
        let none = HashSet::new();
        let arb: HashSet<String> = std::iter::once("GL_ARB_timer_query".to_owned()).collect();

        assert_eq!(
            detect_capability("4.6.0 NVIDIA 470.63.01", &none),
            GpuTimerCapability::Available
        );
        assert_eq!(
            detect_capability("3.1 Mesa", &arb),
            GpuTimerCapability::Available
        );
        assert_eq!(
            detect_capability("3.1 Mesa", &none),
            GpuTimerCapability::Unavailable
        );
        assert_eq!(
            detect_capability("OpenGL ES 3.2 Mesa", &arb),
            GpuTimerCapability::Unavailable
        );
        assert_eq!(
            detect_capability("", &arb),
            GpuTimerCapability::Unavailable,
            "an unparsable version is never guessed available"
        );
    }
}
