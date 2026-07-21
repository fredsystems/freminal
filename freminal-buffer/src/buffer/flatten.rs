// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Row flattening and text extraction operations for [`Buffer`].
//!
//! Converts buffer rows into flat `(Vec<TChar>, Vec<FormatTag>)` pairs
//! for the GUI renderer (`visible_as_tchars_and_tags`,
//! `scrollback_as_tchars_and_tags`, `rows_as_tchars_and_tags_cached`),
//! and provides plain-text extraction (`extract_text`, `extract_block_text`).
//!
//! ## Per-row flatten cache
//!
//! Each entry in [`Buffer::row_cache`](super::Buffer::row_cache) is a
//! [`RowCacheEntry`] that bundles:
//!
//! - `chars`: flat per-row `TChar` sequence (wide-continuation cells skipped)
//! - `tags`: per-row `FormatTag`s with row-relative offsets
//! - `bytes`: UTF-8 byte buffer mirroring `chars`, used for byte-based URL
//!   regex matching without per-row `String` allocation
//! - `byte_to_char`: parallel map from byte offset in `bytes` to character
//!   index in `chars`
//! - `auto_urls`: ranges (in character indices) where plain-text URLs were
//!   auto-detected, with the canonical URL string ready to be spliced into
//!   `FormatTag.url` at merge time
//!
//! All five fields are produced in a single pass over the row's cells. The
//! merge step in [`Buffer::rows_as_tchars_and_tags_cached`] rebases tag
//! offsets to global indices and splices auto-URL ranges into the merged
//! `FormatTag` vec. When an existing `FormatTag.url` is already `Some(_)`
//! (e.g. an OSC 8 hyperlink), auto-detected URLs are suppressed within that
//! range — OSC 8 always wins.

use std::sync::Arc;

use freminal_common::buffer_states::{
    buffer_type::BufferType, format_tag::FormatTag, tchar::TChar, url::Url,
};

use crate::row::{Row, RowJoin};
use crate::url_detect;

use super::tags_same_format;
use crate::buffer::Buffer;

/// A single auto-detected URL range within one row's flat character stream.
///
/// Offsets are character indices into the row's `chars` vec (half-open
/// `[char_start, char_end)`), not byte offsets. The `url` field holds the
/// canonical URL string (already stripped of trailing punctuation) behind
/// an `Arc` so that tag splicing at merge time is a cheap refcount bump.
#[derive(Debug, Clone)]
pub struct AutoUrlRange {
    /// Inclusive start character index into the row's `chars`.
    pub char_start: usize,
    /// Exclusive end character index into the row's `chars`.
    pub char_end: usize,
    /// The detected URL, wrapped for cheap splicing into multiple tags.
    pub url: Arc<Url>,
    /// Mirrors [`url_detect::UrlMatch::touches_buffer_end`]: `true` when the
    /// raw (pre-trim) match reached the end of the row's byte buffer, i.e.
    /// this range might be a DECAWM-wrapped URL continuing onto the next
    /// row. Used as the cheap, precise signal for whether a soft-wrapped
    /// group of rows needs the group-level URL redetection in
    /// [`Buffer::refresh_row_cache_and_refine_wrapped_urls`].
    pub touches_row_end: bool,
}

/// Per-row flatten cache entry.
///
/// Produced by [`Buffer::flatten_row`] and consumed by
/// [`Buffer::rows_as_tchars_and_tags_cached`] at merge time. See the module
/// docs for field semantics.
#[derive(Debug, Clone)]
pub struct RowCacheEntry {
    /// Flat per-row character sequence (wide-continuation cells skipped).
    pub chars: Vec<TChar>,
    /// Per-row format tags with **row-relative** offsets into `chars`.
    pub tags: Vec<FormatTag>,
    /// UTF-8 byte representation of `chars`, used for byte-based URL regex
    /// matching. Empty when `auto_detect_urls` was disabled at flatten time.
    pub bytes: Vec<u8>,
    /// Parallel map from byte offset in `bytes` to character index in `chars`.
    /// `byte_to_char[i]` is the character index for the character that starts
    /// at byte `i` (entries for continuation bytes of a multi-byte codepoint
    /// repeat the starting character index). Empty when `auto_detect_urls`
    /// was disabled.
    pub byte_to_char: Vec<u32>,
    /// Auto-detected URL ranges (character indices). Empty when detection
    /// was disabled or no URLs were found.
    pub auto_urls: Vec<AutoUrlRange>,
    /// Precomputed [`url_detect::row_tail_could_be_wrapped_scheme`] result
    /// for this row's `bytes`. Always `false` when detection was disabled.
    ///
    /// Computed once here (at the same time as `auto_urls`, only when the
    /// row is actually rebuilt) rather than on every merge call, so the
    /// group-signal check in
    /// [`Buffer::refresh_row_cache_and_refine_wrapped_urls`] stays an O(1)
    /// cached lookup instead of re-scanning every row's byte tail on every
    /// flatten — the tail-scan itself is cheap per call, but paying it again
    /// on every already-cached (non-dirty) row on every single frame is not.
    pub tail_could_be_wrapped_scheme: bool,
}

impl RowCacheEntry {
    /// Create an empty cache entry. Used by `Default` and for test scaffolding.
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            chars: Vec::new(),
            tags: Vec::new(),
            bytes: Vec::new(),
            byte_to_char: Vec::new(),
            auto_urls: Vec::new(),
            tail_could_be_wrapped_scheme: false,
        }
    }
}

impl Default for RowCacheEntry {
    fn default() -> Self {
        Self::empty()
    }
}

/// Identifies which flatten "window" a cached merge was built from.
///
/// Two calls with an identical `MergeWindowFp` cover *the same row range at
/// the same detection setting* — a necessary (but not sufficient; see
/// [`MergeCache`]'s doc comment) precondition for reusing a cached merge's
/// prefix instead of redoing it from scratch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::buffer) struct MergeWindowFp {
    /// Absolute (buffer-wide) row index the flatten window starts at, as
    /// returned by `Buffer::visible_window_bounds`.
    visible_start: usize,
    /// Absolute (buffer-wide, exclusive) row index the flatten window ends
    /// at, as returned by `Buffer::visible_window_bounds`.
    visible_end: usize,
    /// `Buffer::auto_detect_urls` at the time of the merge. A toggle changes
    /// what `RowCacheEntry::auto_urls`/`bytes` contain for every row, so it
    /// must invalidate a cached merge exactly like a window-bounds change.
    auto_detect: bool,
}

/// Task 121 Part C (pass C): incremental cache of the last full merge over a
/// flatten window (see [`Buffer::merge_cache`]'s field doc for the full
/// invalidation policy).
///
/// Holds the exact same four vectors [`Buffer::rows_as_tchars_and_tags_cached`]
/// (Step 2) would return, plus the [`MergeWindowFp`] it was computed against.
/// [`Buffer::visible_as_tchars_and_tags_extended`] uses this to reuse the
/// prefix of a previous merge — everything strictly before the first row
/// that changed since that merge — and only re-merges from that row onward,
/// instead of re-walking and re-rebasing every row's tags on every single
/// frame.
///
/// # Why `fp` + `first_rebuilt_row` alone are not sufficient
///
/// `MergeWindowFp` equality only proves the window covers the *same row
/// range* at the *same detection setting* — not that every individual row
/// within that range still holds the content it held when this cache was
/// built. The `first_rebuilt_row` signal (any row that was `dirty` or had a
/// `None` cache entry on the current call) catches ordinary edits, but
/// **cannot** catch an in-place *rotation* of already-clean cache entries
/// between row indices without touching their `dirty`/`None` status — which
/// is exactly what `scroll.rs`'s confined scroll-region primitives
/// (`scroll_slice_up`/`scroll_slice_down`) and the whole-buffer `scroll_up`
/// do. Those three sites don't rely on `fp`/`first_rebuilt_row` at all: each
/// one sets `self.merge_cache = None` explicitly, at the exact point where
/// it performs the rotation, forcing the next flatten to take the
/// full-merge fallback instead of reusing a now-stale prefix. See
/// `Buffer::merge_cache`'s field doc for the complete accounting of every
/// invalidation mechanism (fingerprint, `first_rebuilt_row`, and the
/// explicit-`None` sites) and which sites rely on which.
///
/// Every fast-path use of this cache is ALSO cross-checked against the
/// full-merge oracle under `debug_assert_eq!` before being returned (`#405`
/// Part C's load-bearing safety net) — release-cost-free. This is not the
/// mechanism the confined-rotation case above relies on (that's the
/// explicit `merge_cache = None` in `scroll.rs`); it is a general backstop
/// that turns any *other*, not-yet-discovered divergence here into an
/// immediate, loud test failure instead of a silently wrong render. See
/// `incremental_merge_tests` below for regression tests that exercise the
/// confined-rotation sites directly against an independent oracle.
///
/// ## Regression fix: `Arc` storage, not a second deep clone
///
/// The four fields are `Arc<Vec<_>>`, not plain `Vec<_>`. Adversarial
/// benchmarking (200x50 window) proved that cloning all four freshly-built
/// `Vec`s a *second* time purely to populate this cache — on top of the
/// clone already implied by handing the caller an owned copy — is a net
/// regression: the extra copy scales with window area and outweighs the
/// tag-rebase work the incremental fast path saves. Wrapping the
/// just-built vectors in `Arc::new` once and storing `Arc::clone`s here
/// turns that population into a refcount bump instead of a memcpy.
///
/// This alone does not make the existing `Vec`-returning public methods
/// (`visible_as_tchars_and_tags[_extended]`) any cheaper: since this cache
/// always retains one strong reference, `Arc::try_unwrap` on the sibling
/// reference handed back to the caller always fails (refcount 2), forcing
/// the same one deep clone those methods paid before this change — just
/// relocated from an explicit `.clone()` into the `unwrap_or_else` fallback
/// of the strong-count check. The win is realised by
/// [`Buffer::visible_as_tchars_and_tags_extended_arc`], which returns these
/// `Arc`s directly with **zero** deep clone. See that method's doc comment
/// for why exploiting it on the current hot path (`interface.rs`) requires
/// a follow-up change outside this crate.
#[derive(Debug)]
pub(in crate::buffer) struct MergeCache {
    /// The window this merge was computed over.
    fp: MergeWindowFp,
    /// The full merged character stream for the window.
    chars: Arc<Vec<TChar>>,
    /// The full merged, globally-rebased, coalesced format tags.
    tags: Arc<Vec<FormatTag>>,
    /// `row_offsets[r]` is the flat index into `chars` where row `r`
    /// (window-relative) begins. Always has exactly one entry per row in
    /// the window.
    row_offsets: Arc<Vec<usize>>,
    /// Indices into `tags` where `tag.url.is_some()`.
    url_tag_indices: Arc<Vec<usize>>,
}

/// `Arc`-wrapped `(chars, tags, row_offsets, url_tag_indices)`.
///
/// The shape [`MergeCache`] stores and
/// [`Buffer::rows_as_tchars_and_tags_incremental`] /
/// [`Buffer::visible_as_tchars_and_tags_extended_arc`] return. Factored into
/// a named alias purely to satisfy `clippy::type_complexity`; carries no
/// additional semantics beyond the four-tuple it names. `pub` (not `pub(in
/// crate::buffer)`) because it appears in the return type of the `pub`
/// [`Buffer::visible_as_tchars_and_tags_extended_arc`].
pub type ArcFlattenResult = (
    Arc<Vec<TChar>>,
    Arc<Vec<FormatTag>>,
    Arc<Vec<usize>>,
    Arc<Vec<usize>>,
);

impl Buffer {
    /// Convert the currently visible rows into a flat `(Vec<TChar>, Vec<FormatTag>)` pair
    /// Convert visible rows (with the given `scroll_offset`) into flat
    /// `(Vec<TChar>, Vec<FormatTag>)` suitable for the GUI renderer.
    ///
    /// Pass `scroll_offset = 0` when calling from the PTY thread (which always
    /// operates at the live bottom).
    ///
    /// Takes `&mut self` because it updates the per-row cache and clears dirty
    /// flags on rows that are freshly flattened.
    #[must_use]
    pub fn visible_as_tchars_and_tags(
        &mut self,
        scroll_offset: usize,
    ) -> (Vec<TChar>, Vec<FormatTag>, Vec<usize>, Vec<usize>) {
        self.visible_as_tchars_and_tags_extended(scroll_offset, 0)
    }

    /// Like [`Self::visible_as_tchars_and_tags`] but extends the flatten window
    /// upward by `extra_rows` (see
    /// [`Buffer::visible_window_bounds`](super::Buffer::visible_window_bounds)).
    ///
    /// The GUI passes a non-zero `extra_rows` when command-block folds collapse
    /// rows in the visible window: the extra rows above the normal window
    /// provide real content to fill the screen once the folds are collapsed,
    /// so the live bottom stays pinned instead of leaving a blank gap.
    ///
    /// When `extra_rows == 0` this is identical to
    /// [`Self::visible_as_tchars_and_tags`].
    ///
    /// ## Task 121 Part C: incremental merge
    ///
    /// Unlike [`Self::scrollback_as_tchars_and_tags`] (which always does a
    /// full [`Self::merge_row_caches_full`]), this is the hot per-frame path,
    /// so it maintains [`Buffer::merge_cache`] and takes an incremental
    /// shortcut whenever possible: reuse everything in the previous merge
    /// strictly before the first row that changed, and only re-merge from
    /// there onward via [`Self::merge_rows_range`], instead of re-walking and
    /// re-rebasing every row's tags on every frame.
    ///
    /// The fast path requires ALL of:
    /// - a previous [`MergeCache`] exists,
    /// - its [`MergeWindowFp`] (window bounds + auto-detect) matches this
    ///   call's,
    /// - a `boundary` row index exists (`min` of `first_rebuilt_row` — from
    ///   Step 1 — and the smallest `row_idx` in `refined_auto_urls` — from
    ///   Step 1.5; either can be `None`, in which case the other alone is
    ///   the boundary, and if both are `None` there is no boundary at all,
    ///   meaning nothing in the window needs re-merging),
    /// - `boundary >= 1` (row 0 itself must not have changed, or there is no
    ///   prefix to reuse),
    /// - `boundary` is in range of both the current window and the cached
    ///   `row_offsets`.
    ///
    /// When `boundary` doesn't exist (nothing changed at all since the last
    /// call) the cached tuple is returned verbatim — no re-merge work at
    /// all, not even a partial one.
    ///
    /// See [`MergeCache`]'s doc comment for why this is not a 100%
    /// airtight optimization in the face of `scroll.rs`'s confined
    /// scroll-region row rotations, and why every fast-path return is
    /// cross-checked against the full-merge oracle under `debug_assert_eq!`
    /// first.
    #[must_use]
    pub fn visible_as_tchars_and_tags_extended(
        &mut self,
        scroll_offset: usize,
        extra_rows: usize,
    ) -> (Vec<TChar>, Vec<FormatTag>, Vec<usize>, Vec<usize>) {
        let (chars, tags, row_offsets, url_tag_indices) =
            self.visible_as_tchars_and_tags_extended_arc(scroll_offset, extra_rows);
        (
            Self::unwrap_or_clone(chars),
            Self::unwrap_or_clone(tags),
            Self::unwrap_or_clone(row_offsets),
            Self::unwrap_or_clone(url_tag_indices),
        )
    }

    /// `Arc`-returning counterpart of [`Self::visible_as_tchars_and_tags_extended`].
    ///
    /// Identical merge behaviour (same [`MergeCache`] fast path, same
    /// debug-only oracle cross-check) but returns the four result vectors
    /// wrapped in `Arc` instead of by value.
    ///
    /// ## Why this method exists (regression fix, Task 121 Part C follow-up)
    ///
    /// [`MergeCache`] must retain its own strong reference to the merged
    /// `(chars, tags, row_offsets, url_tag_indices)` across calls — that
    /// retained copy is the entire mechanism the incremental fast path
    /// reuses a prefix from. Given that, and given that
    /// [`Self::visible_as_tchars_and_tags_extended`] must keep returning
    /// owned `Vec`s (many callers/tests depend on that exact signature),
    /// **one** deep clone per populating call is mathematically
    /// unavoidable through that method: the cache's `Arc` and the
    /// caller's `Arc` are both alive at the point an owned `Vec` must be
    /// produced, so `Arc::try_unwrap` always finds a strong count of 2 and
    /// falls back to cloning.
    ///
    /// This method sidesteps that wall entirely by handing the `Arc`s back
    /// directly — **zero** deep clone, only the refcount bumps
    /// [`MergeCache`] population already costs. Any caller that can accept
    /// `Arc<Vec<_>>` (i.e. anything that would otherwise immediately wrap
    /// the returned `Vec`s in `Arc::new`) should call this method instead
    /// of `visible_as_tchars_and_tags_extended` to realise the actual
    /// regression fix on the hot per-frame path. The hot-path consumer,
    /// `freminal_terminal_emulator::interface::TerminalEmulator::flatten_visible`,
    /// already calls this Arc-returning method directly, so the snapshot
    /// path pays no extra deep clone.
    #[must_use]
    pub fn visible_as_tchars_and_tags_extended_arc(
        &mut self,
        scroll_offset: usize,
        extra_rows: usize,
    ) -> ArcFlattenResult {
        let (visible_start, visible_end) = self.visible_window_bounds(scroll_offset, extra_rows);
        // Task 119: when the user scrolls back, this window can reach into
        // compressed scrollback (a nonzero `scroll_offset` lowers
        // `visible_window_start`). Decompress any evicted rows in the window
        // before reading their cells, so `flatten_row` never touches an
        // evicted placeholder. A no-op when nothing in the window is
        // compressed (the common live-view case).
        self.ensure_decompressed(visible_start..visible_end);
        let auto_detect = self.auto_detect_urls;
        let fp = MergeWindowFp {
            visible_start,
            visible_end,
            auto_detect,
        };

        let rows_slice = &mut self.rows[visible_start..visible_end];
        let cache_slice = &mut self.row_cache[visible_start..visible_end];
        let merge_cache = &mut self.merge_cache;

        Self::rows_as_tchars_and_tags_incremental(
            rows_slice,
            cache_slice,
            auto_detect,
            fp,
            merge_cache,
        )
    }

    /// Extract an owned `Vec<T>` from an `Arc<Vec<T>>`, avoiding the clone
    /// whenever this happens to be the sole strong reference (`strong_count
    /// == 1`), and falling back to `(*arc).clone()` otherwise.
    ///
    /// For the current [`MergeCache`]-backed callers this fallback always
    /// fires (the cache itself holds the other strong reference) — see
    /// [`Self::visible_as_tchars_and_tags_extended_arc`]'s doc comment for
    /// why that is an inherent limit of returning owned `Vec`s from a
    /// method whose whole point is to retain a persistent cache. Factored
    /// out as a named helper (rather than inlined four times) so the
    /// "cheapest possible extraction" intent is documented once and
    /// automatically benefits from any future call site where the
    /// sole-owner case does hold.
    fn unwrap_or_clone<T: Clone>(arc: Arc<Vec<T>>) -> Vec<T> {
        Arc::try_unwrap(arc).unwrap_or_else(|shared| (*shared).clone())
    }

    /// The incremental-merge counterpart of [`Self::rows_as_tchars_and_tags_cached`]:
    /// runs the same Step 1 (+ 1.5) refresh, then either reuses/extends
    /// `merge_cache` or falls back to a full [`Self::merge_row_caches_full`],
    /// storing the fresh result back into `merge_cache` either way.
    ///
    /// See [`Self::visible_as_tchars_and_tags_extended`]'s doc comment for
    /// the fast-path precondition list and the debug-only safety net.
    ///
    /// Returns `Arc`s rather than owned `Vec`s: every return path stores an
    /// `Arc::clone` (refcount bump) of the exact same allocation into
    /// `merge_cache`, so populating the cache never costs a second deep
    /// copy on top of building the result. See [`MergeCache`]'s doc comment
    /// and [`Self::visible_as_tchars_and_tags_extended_arc`]'s doc comment
    /// for the full accounting of where the remaining unavoidable clone
    /// (converting back to an owned `Vec` for the legacy signature) lives.
    fn rows_as_tchars_and_tags_incremental(
        rows: &mut [Row],
        cache: &mut [Option<RowCacheEntry>],
        auto_detect: bool,
        fp: MergeWindowFp,
        merge_cache: &mut Option<MergeCache>,
    ) -> ArcFlattenResult {
        // `reuse_available` is the promise, checked BEFORE Step 1 runs, that
        // IF a usable incremental boundary comes out of this call, its
        // reused prefix will come from a merge that is still valid for this
        // exact window. Only when this holds is it safe to let Step 1 skip
        // redetecting an unchanged wrapped-URL group — see
        // `refresh_row_cache_and_refine_wrapped_urls`'s doc comment.
        let reuse_available = merge_cache.as_ref().is_some_and(|cached| cached.fp == fp);

        let (refined_auto_urls, first_rebuilt_row) =
            Self::refresh_row_cache_and_refine_wrapped_urls(
                rows,
                cache,
                auto_detect,
                reuse_available,
            );

        let refined_min_row = refined_auto_urls.first().map(|(idx, _)| *idx);
        let boundary = match (first_rebuilt_row, refined_min_row) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(a), None) | (None, Some(a)) => Some(a),
            (None, None) => None,
        };

        // ── No-op fast path: `reuse_available` held, and nothing in the
        // window needed rebuilding this call — the previous merge is still
        // exactly correct, verbatim. (Every group's redetection was
        // legitimately skipped: with `boundary` absent, every group in the
        // window has zero rebuilt rows, so `reuse_available`'s promise that
        // skipped groups are "entirely before the boundary" holds trivially
        // — there IS no non-reused tail at all.)
        if reuse_available
            && boundary.is_none()
            && let Some(cached) = merge_cache.as_ref()
        {
            Self::debug_verify_against_oracle(
                rows,
                cache,
                auto_detect,
                (
                    &cached.chars,
                    &cached.tags,
                    &cached.row_offsets,
                    &cached.url_tag_indices,
                ),
                "no-op incremental merge",
            );
            // Refcount bumps only — `cached` already holds the exact `Arc`s
            // being handed back, nothing here touches the underlying `Vec`
            // data.
            return (
                Arc::clone(&cached.chars),
                Arc::clone(&cached.tags),
                Arc::clone(&cached.row_offsets),
                Arc::clone(&cached.url_tag_indices),
            );
        }

        // ── Incremental fast path: `reuse_available` held and a usable
        // `boundary` exists — reuse the cached prefix strictly before
        // `boundary`, re-merge only from `boundary` onward. Every group
        // Step 1 skipped either lies wholly in this reused prefix (correct)
        // or had a rebuilt row and so WAS redetected (also correct) — see
        // `refresh_row_cache_and_refine_wrapped_urls`'s doc comment.
        if reuse_available
            && let Some(boundary) = boundary
            && let Some(cached) = merge_cache.as_ref()
            && boundary >= 1
            && boundary <= cached.row_offsets.len()
            && boundary < cache.len()
        {
            let (mut chars, mut tags, mut row_offsets) =
                Self::build_reused_prefix(cached, boundary);

            Self::merge_rows_range(
                cache,
                &refined_auto_urls,
                boundary,
                &mut chars,
                &mut tags,
                &mut row_offsets,
            );

            Self::finish_merge(&chars, &mut tags);

            let url_tag_indices = Self::collect_url_tag_indices(&tags);

            // #405 Part C load-bearing safety net: the incremental fast
            // path is a hand-maintained shortcut around the full-merge
            // oracle. Any divergence here is a real correctness bug that
            // must be caught before it reaches a production release build
            // (where this check compiles out entirely — release-cost-free).
            Self::debug_verify_against_oracle(
                rows,
                cache,
                auto_detect,
                (&chars, &tags, &row_offsets, &url_tag_indices),
                "incremental fast-path merge",
            );

            // Wrap each freshly-built `Vec` in `Arc` exactly once (a cheap
            // move of the `Vec` header into a new small allocation, not a
            // data copy), store an `Arc::clone` (refcount bump) into
            // `merge_cache`, and return the original `Arc`s. Both the cache
            // and the return value end up sharing the same underlying
            // buffers — no second deep clone of `chars`/`tags` is performed
            // to populate the cache.
            let chars = Arc::new(chars);
            let tags = Arc::new(tags);
            let row_offsets = Arc::new(row_offsets);
            let url_tag_indices = Arc::new(url_tag_indices);

            *merge_cache = Some(MergeCache {
                fp,
                chars: Arc::clone(&chars),
                tags: Arc::clone(&tags),
                row_offsets: Arc::clone(&row_offsets),
                url_tag_indices: Arc::clone(&url_tag_indices),
            });

            return (chars, tags, row_offsets, url_tag_indices);
        }

        // ── Fallback: fast-path preconditions not met — full merge, then
        // (re)populate `merge_cache` so the *next* call can go fast.
        //
        // If `reuse_available` was `true` but the boundary turned out
        // unusable (e.g. row 0 itself changed, so there is no prefix left to
        // reuse at all), `refined_auto_urls` may be under-detected: Step 1
        // gated some clean group's redetection on the (now falsified)
        // assumption that group would be served from a reused prefix. A
        // full, from-scratch merge must not consume that gated result —
        // recompute Step 1 with gating disabled first. When
        // `reuse_available` was already `false`, `refined_auto_urls` is
        // already fully (ungated) correct and reused as-is.
        let full_refined_auto_urls = if reuse_available {
            Self::refresh_row_cache_and_refine_wrapped_urls(rows, cache, auto_detect, false).0
        } else {
            refined_auto_urls
        };

        let (chars, tags, row_offsets, url_tag_indices) =
            Self::merge_row_caches_full(cache, &full_refined_auto_urls);

        // Same `Arc`-once, clone-the-`Arc`-not-the-`Vec` pattern as the
        // incremental fast path above.
        let chars = Arc::new(chars);
        let tags = Arc::new(tags);
        let row_offsets = Arc::new(row_offsets);
        let url_tag_indices = Arc::new(url_tag_indices);

        *merge_cache = Some(MergeCache {
            fp,
            chars: Arc::clone(&chars),
            tags: Arc::clone(&tags),
            row_offsets: Arc::clone(&row_offsets),
            url_tag_indices: Arc::clone(&url_tag_indices),
        });
        (chars, tags, row_offsets, url_tag_indices)
    }

    /// Debug-only cross-check: recompute the full-merge oracle from scratch
    /// (Step 1 with gating **disabled**, guaranteeing a trustworthy,
    /// fully-redetected `refined_auto_urls`, then [`Self::merge_row_caches_full`])
    /// and assert it is byte-identical to `actual`.
    ///
    /// This is the `#405` Part C load-bearing safety net for both the
    /// no-op and incremental fast paths in
    /// [`Self::rows_as_tchars_and_tags_incremental`]: it re-derives the
    /// *true* oracle rather than reusing whatever (possibly gated)
    /// `refined_auto_urls` the fast path itself computed, so a real
    /// divergence is never masked by comparing two equally-gated results
    /// against each other. `rows`/`cache` are already fully clean at this
    /// point (Step 1 already ran once this call), so this second pass does
    /// no rebuilding — only redetection, which stays a debug-only cost.
    ///
    /// Release builds compile this to nothing: `debug_assert_eq!` inside is
    /// itself a no-op there, but this wrapper is `#[cfg(debug_assertions)]`-gated
    /// too so the oracle recomputation itself never runs in release.
    #[cfg(debug_assertions)]
    fn debug_verify_against_oracle(
        rows: &mut [Row],
        cache: &mut [Option<RowCacheEntry>],
        auto_detect: bool,
        actual: (&[TChar], &[FormatTag], &[usize], &[usize]),
        context: &str,
    ) {
        let (oracle_refined, _) =
            Self::refresh_row_cache_and_refine_wrapped_urls(rows, cache, auto_detect, false);
        let oracle = Self::merge_row_caches_full(cache, &oracle_refined);
        assert_eq!(
            actual,
            (
                oracle.0.as_slice(),
                oracle.1.as_slice(),
                oracle.2.as_slice(),
                oracle.3.as_slice()
            ),
            "#405 Part C: {context} diverged from the full-merge oracle"
        );
    }

    #[cfg(not(debug_assertions))]
    #[inline]
    fn debug_verify_against_oracle(
        _rows: &mut [Row],
        _cache: &mut [Option<RowCacheEntry>],
        _auto_detect: bool,
        _actual: (&[TChar], &[FormatTag], &[usize], &[usize]),
        _context: &str,
    ) {
    }

    /// Flatten all scrollback rows (everything before the visible window) into
    /// a linear `(Vec<TChar>, Vec<FormatTag>, Vec<usize>, Vec<usize>)` tuple
    /// using the same algorithm as [`Self::visible_as_tchars_and_tags`].
    ///
    /// Returns `(vec![], vec![], vec![], vec![])` for the alternate screen
    /// buffer, which never accumulates scrollback.
    ///
    /// Pass `scroll_offset = 0` when calling from the PTY thread.
    pub fn scrollback_as_tchars_and_tags(
        &mut self,
        scroll_offset: usize,
    ) -> (Vec<TChar>, Vec<FormatTag>, Vec<usize>, Vec<usize>) {
        // Alternate buffer has no scrollback.
        if self.kind == BufferType::Alternate {
            return (vec![], vec![], vec![], vec![]);
        }

        let visible_start = self.visible_window_start(scroll_offset);

        if visible_start == 0 {
            // No scrollback rows exist yet.
            return (vec![], vec![], vec![], vec![]);
        }

        // Task 119.4: restore any deep-cold compressed rows in this range
        // back to real (Task-118 `Compact`) content before flattening. This
        // is the decompress-on-read seam — the visible window (never
        // reached here) never needs it.
        self.ensure_decompressed(0..visible_start);

        let auto_detect = self.auto_detect_urls;
        let result = Self::rows_as_tchars_and_tags_cached(
            &mut self.rows[..visible_start],
            &mut self.row_cache[..visible_start],
            auto_detect,
        );

        // Task 118.4: a full-scrollback flatten (the Ctrl-F search-buffer
        // path) is the only caller that reads cold scrollback history in
        // bulk. `rows_as_tchars_and_tags_cached` just built (or reused) a
        // `RowCacheEntry` per row, and reading a compact row's cells along
        // the way (in `flatten_row`) also warmed its `OnceCell` decompaction
        // memo — so every compact row now momentarily holds three
        // representations at once (its `CompactRow`, the memoized
        // `Vec<Cell>`, and the `RowCacheEntry`). Cold scrollback rows are
        // rarely re-read, so we drop the two larger, cheaply-rebuildable
        // copies here and keep only the small `CompactRow`. The next
        // scrollback flatten rebuilds an identical `RowCacheEntry` from the
        // `CompactRow` (`entry.is_none()` forces a rebuild in Step 1 of
        // `rows_as_tchars_and_tags_cached`), so output is unaffected — only
        // resident memory changes.
        //
        // Visible rows are never in this slice (`..visible_start` excludes
        // them), so their cache is untouched: they re-render every frame and
        // must stay warm.
        for (row, entry) in self.rows[..visible_start]
            .iter_mut()
            .zip(self.row_cache[..visible_start].iter_mut())
        {
            if row.is_compact() {
                *entry = None;
                row.release_decompacted_cache();
            }
        }

        result
    }

    /// Shared helper: flatten a slice of [`Row`]s into `(Vec<TChar>,
    /// Vec<FormatTag>, Vec<usize>)`, using a per-row cache to skip rows that
    /// have not changed since the last snapshot.
    ///
    /// For each row:
    /// - If `row.dirty` or the cache entry is `None`, flatten the row, populate
    ///   the cache entry, and call `row.mark_clean()`.
    /// - Otherwise reuse the cached per-row `RowCacheEntry` directly.
    ///
    /// Per-row tag offsets are stored relative to each row's own character
    /// slice (starting at 0).  The merge step below re-computes global offsets
    /// each time, so the cache never stores stale absolute positions.
    ///
    /// `auto_detect` controls whether the per-row byte buffer and auto-URL
    /// detection are populated at flatten time. When `false`, `bytes`,
    /// `byte_to_char`, and `auto_urls` on each cache entry are empty.
    ///
    /// At merge time, any `AutoUrlRange` whose covered character range does
    /// not already carry a `FormatTag.url` is spliced into the merged tag
    /// stream: covering tags are split into (pre, overlap-with-url, post)
    /// segments. OSC 8 links always win over auto-detected ones.
    ///
    /// The returned tuple contains:
    /// - `Vec<TChar>` — flat character data
    /// - `Vec<FormatTag>` — merged format tags with global offsets
    /// - `Vec<usize>` — row offsets (`row_offsets[r]` is the flat index where
    ///   row `r` begins)
    /// - `Vec<usize>` — URL tag indices (indices into the tags vec where
    ///   `url.is_some()`)
    fn rows_as_tchars_and_tags_cached(
        rows: &mut [Row],
        cache: &mut [Option<RowCacheEntry>],
        auto_detect: bool,
    ) -> (Vec<TChar>, Vec<FormatTag>, Vec<usize>, Vec<usize>) {
        // ── Step 1 (+ 1.5): ensure every row has an up-to-date cache entry,
        // and fix up auto-detected URLs that wrap across rows. See
        // `refresh_row_cache_and_refine_wrapped_urls`'s doc comment. This
        // (full, uncached-merge) path never needs the incremental merge
        // boundary, so `first_rebuilt_row` is discarded. `reuse_available =
        // false`: there is no merge cache here for a skipped group's rows to
        // ever be reused from (see that parameter's doc comment), so
        // redetection gating must stay disabled — every group with URL
        // signal is always redetected, matching the pre-Part-C behavior.
        let (refined_auto_urls, _first_rebuilt_row) =
            Self::refresh_row_cache_and_refine_wrapped_urls(rows, cache, auto_detect, false);

        // ── Step 2: merge per-row results into the global flat vectors. This
        // is a pure function of `(cache, refined_auto_urls)` — see
        // `merge_row_caches_full`'s doc comment.
        Self::merge_row_caches_full(cache, &refined_auto_urls)
    }

    /// Step 2 of [`Self::rows_as_tchars_and_tags_cached`] (the **full**
    /// merge): merge every row's cached flat representation into one global
    /// `(chars, tags, row_offsets, url_tag_indices)` tuple, from scratch.
    ///
    /// This is a **pure function** of `(cache, refined_auto_urls)` — it does
    /// not read `rows` or any other `Buffer` state. Thin wrapper around
    /// [`Self::merge_rows_range`] seeded with empty accumulators starting at
    /// row 0 (i.e. "merge the whole window"), plus the trailing-tag-end
    /// fixup and `url_tag_indices` computation that both the full merge and
    /// the incremental merge need at the end.
    ///
    /// Serves as the correctness oracle a future (and now present, see
    /// [`Buffer::visible_as_tchars_and_tags_extended`]) incremental merge
    /// path is checked against: both take the same cache + refined-URL
    /// inputs and must produce byte-identical output.
    ///
    /// `cache.len()` is the number of rows being merged; every entry is
    /// expected to be `Some` (Step 1 populates every entry unconditionally).
    /// A `None` entry contributes no characters or tags for that row (only
    /// the `NewLine` separator, if any), rather than panicking — this
    /// function never assumes its precondition holds.
    fn merge_row_caches_full(
        cache: &[Option<RowCacheEntry>],
        refined_auto_urls: &[(usize, Vec<AutoUrlRange>)],
    ) -> (Vec<TChar>, Vec<FormatTag>, Vec<usize>, Vec<usize>) {
        let mut chars: Vec<TChar> = Vec::new();
        let mut tags: Vec<FormatTag> = Vec::new();
        let mut row_offsets: Vec<usize> = Vec::with_capacity(cache.len());

        Self::merge_rows_range(
            cache,
            refined_auto_urls,
            0,
            &mut chars,
            &mut tags,
            &mut row_offsets,
        );

        Self::finish_merge(&chars, &mut tags);

        let url_tag_indices = Self::collect_url_tag_indices(&tags);
        (chars, tags, row_offsets, url_tag_indices)
    }

    /// Merge rows `[start_row, cache.len())` (window-relative indices) onto
    /// the END of the given `chars`/`tags`/`row_offsets` accumulators.
    ///
    /// This is the per-row loop body shared by both [`Self::merge_row_caches_full`]
    /// (called with `start_row = 0` and empty accumulators) and the
    /// incremental fast path in
    /// [`Buffer::visible_as_tchars_and_tags_extended`] (called with
    /// `start_row = boundary` and accumulators pre-seeded with a reused
    /// prefix). Same rebase/coalesce/`NewLine`/splice logic either way —
    /// seeding `tags` with a clamped prefix tail before calling this
    /// reproduces the correct coalescing seam automatically, because the
    /// very first thing this loop does for `start_row` is compare against
    /// `tags.last_mut()` (whatever the caller seeded) exactly like it would
    /// against the previous row's tags in a full merge.
    ///
    /// Does **not** apply the trailing-tag-end fixup or compute
    /// `url_tag_indices` — callers that need those (both current callers do)
    /// call [`Self::finish_merge`] / [`Self::collect_url_tag_indices`]
    /// afterward, since a truly partial (mid-window) call would not want
    /// those applied prematurely.
    ///
    /// `cache` is always the **whole** window slice (not pre-sliced to
    /// `[start_row..]`) so that `row_idx` here matches the window-relative
    /// indices `refined_auto_urls` and `row_offsets` are keyed on.
    fn merge_rows_range(
        cache: &[Option<RowCacheEntry>],
        refined_auto_urls: &[(usize, Vec<AutoUrlRange>)],
        start_row: usize,
        chars: &mut Vec<TChar>,
        tags: &mut Vec<FormatTag>,
        row_offsets: &mut Vec<usize>,
    ) {
        let row_count = cache.len();
        let mut refined_cursor = 0usize;
        // Advance the cursor past any refined entries whose row_idx is
        // before `start_row`, so a partial merge (start_row > 0) begins
        // looking at the correct position instead of re-scanning entries
        // that can never apply to the rows this call will actually visit.
        while refined_auto_urls
            .get(refined_cursor)
            .is_some_and(|(idx, _)| *idx < start_row)
        {
            refined_cursor += 1;
        }

        for (row_idx, entry) in cache.iter().enumerate().skip(start_row) {
            // Step 1 populated every entry unconditionally, so `None` cannot
            // occur here.  We use `if let` to satisfy the no-unwrap/expect rule;
            // the `else` branch is unreachable in practice.
            if let Some(row_entry) = entry.as_ref() {
                let global_offset = chars.len();

                // Record the flat index where this row begins.
                row_offsets.push(global_offset);

                // Rebase per-row tags into the global index space, then splice
                // any auto-detected URL ranges on top. Splicing is done in
                // row-local character space first (cheap) and the result is
                // then rebased. Prefer the Step 1.5 group-corrected ranges
                // (whole-URL text, wrap-boundary-safe) when present — `refined_auto_urls`
                // is sorted by `row_idx`, so a single forward cursor finds the
                // match (if any) in amortised O(1).
                while refined_auto_urls
                    .get(refined_cursor)
                    .is_some_and(|(idx, _)| *idx < row_idx)
                {
                    refined_cursor += 1;
                }
                let row_auto_urls = match refined_auto_urls.get(refined_cursor) {
                    Some((idx, ranges)) if *idx == row_idx => ranges.as_slice(),
                    _ => &row_entry.auto_urls,
                };
                let spliced = splice_auto_urls(&row_entry.tags, row_auto_urls);

                // Append this row's characters, adjusting tag offsets.
                for row_tag in &spliced {
                    let rebased = FormatTag {
                        start: global_offset + row_tag.start,
                        end: global_offset + row_tag.end,
                        colors: row_tag.colors,
                        font_weight: row_tag.font_weight,
                        font_decorations: row_tag.font_decorations,
                        url: row_tag.url.clone(),
                        blink: row_tag.blink,
                    };

                    // Merge with the previous tag when format is identical and
                    // the ranges are contiguous (same logic as the original helper).
                    if let Some(last) = tags.last_mut() {
                        if last.end == rebased.start && tags_same_format(last, &rebased) {
                            last.end = rebased.end;
                        } else {
                            tags.push(rebased);
                        }
                    } else {
                        tags.push(rebased);
                    }
                }

                chars.extend_from_slice(&row_entry.chars);
            }

            // Append a NewLine separator after every row except the last.
            let is_last_row = row_idx + 1 == row_count;
            if !is_last_row {
                let byte_pos = chars.len();
                chars.push(TChar::NewLine);
                if let Some(last) = tags.last_mut() {
                    if last.end == byte_pos {
                        last.end += 1;
                    } else {
                        tags.push(FormatTag {
                            start: byte_pos,
                            end: byte_pos + 1,
                            ..FormatTag::default()
                        });
                    }
                } else {
                    tags.push(FormatTag {
                        start: byte_pos,
                        end: byte_pos + 1,
                        ..FormatTag::default()
                    });
                }
            }
        }
    }

    /// Shared trailing fixup applied once after all rows have been merged
    /// (by either [`Self::merge_row_caches_full`] or the incremental fast
    /// path): guarantee at least one tag exists, and that the final tag's
    /// `end` reaches exactly `chars.len()`.
    fn finish_merge(chars: &[TChar], tags: &mut Vec<FormatTag>) {
        if tags.is_empty() {
            tags.push(FormatTag {
                start: 0,
                end: if chars.is_empty() {
                    usize::MAX
                } else {
                    chars.len()
                },
                ..FormatTag::default()
            });
        } else if let Some(last) = tags.last_mut() {
            last.end = chars.len();
        }
    }

    /// Build the reused prefix (`chars`, `tags`, `row_offsets`) the
    /// incremental fast path in
    /// [`Self::rows_as_tchars_and_tags_incremental`] seeds
    /// [`Self::merge_rows_range`] with: everything from `cached` up to
    /// (window-relative) row `boundary`, exclusive.
    ///
    /// `chars`/`row_offsets` are plain prefix slices. `tags` needs care at
    /// the cut point `cached.row_offsets[boundary]`: tags are sorted and
    /// non-overlapping, so this keeps every tag with `end <= cut` verbatim,
    /// clamps the one tag that straddles `cut` (if any) to `end = cut`, and
    /// drops everything from the first tag with `start >= cut` onward.
    fn build_reused_prefix(
        cached: &MergeCache,
        boundary: usize,
    ) -> (Vec<TChar>, Vec<FormatTag>, Vec<usize>) {
        let cut = cached.row_offsets[boundary];

        let chars: Vec<TChar> = cached.chars[..cut].to_vec();
        let row_offsets: Vec<usize> = cached.row_offsets[..boundary].to_vec();
        let mut tags: Vec<FormatTag> = Vec::with_capacity(cached.tags.len());
        for tag in cached.tags.iter() {
            if tag.start >= cut {
                // Tags are sorted and non-overlapping: once start >= cut,
                // nothing further can start before cut either.
                break;
            }
            if tag.end <= cut {
                tags.push(tag.clone());
            } else {
                // Straddles the cut: clamp the reused copy to the prefix.
                let mut clamped = tag.clone();
                clamped.end = cut;
                tags.push(clamped);
                break;
            }
        }

        (chars, tags, row_offsets)
    }

    /// Step 1 (+ 1.5) of [`Self::rows_as_tchars_and_tags_cached`]: ensure
    /// every row has an up-to-date [`RowCacheEntry`], and fix up
    /// auto-detected URLs that wrap across rows, in one pass over `rows`.
    ///
    /// [`Self::flatten_row`] detects URLs using only that row's own bytes, so
    /// a URL that DECAWM soft-wraps across two or more physical rows is seen
    /// as several independent, truncated matches. This pass finds contiguous
    /// runs of rows joined by `RowJoin::ContinueLogicalLine` (soft-wrap
    /// continuations of one logical line) and, only when the run actually
    /// contains URL-looking content, re-runs URL detection on the rows'
    /// concatenated bytes via [`redetect_urls_for_group`] so wrap boundaries
    /// stop being treated as the end of the URL (both for truncation and for
    /// the trailing sentence-punctuation heuristic in
    /// `url_detect::trim_trailing`, which otherwise misfires when a wrap
    /// boundary lands right before stripped punctuation).
    ///
    /// The grouping scan is fused into the same loop that rebuilds dirty row
    /// caches — rather than a second full pass over `rows` — so the common
    /// case (nothing wraps, or a wrap has no URL content) costs only a few
    /// extra cheap comparisons per row instead of a whole extra traversal.
    ///
    /// Returns a **sparse** list of `(row_idx, ranges)` pairs, sorted by
    /// ascending `row_idx`, covering only rows whose `auto_urls` were
    /// replaced by a group-level redetection. A row not present means "no
    /// change; use the row's own cached `auto_urls`". It stays empty (no heap
    /// allocation) whenever no row needed correction — the overwhelmingly
    /// common case.
    ///
    /// Also returns `first_rebuilt_row`: the smallest `row_idx` (window
    /// relative) whose cache entry was actually rebuilt this call (`row.dirty`
    /// was set, or its cache entry was `None`/stale) — `None` when nothing in
    /// the window needed rebuilding at all. [`Buffer::visible_as_tchars_and_tags_extended`]
    /// uses this as the incremental merge boundary: see its doc comment.
    ///
    /// ## Task 121 Part C: gating group redetection on dirtiness
    ///
    /// When `reuse_available` is `true`, [`redetect_urls_for_group`] is only
    /// called for a wrapped-URL group when the group has URL signal **and**
    /// at least one row in that group was actually rebuilt this call
    /// (tracked below as `group_has_rebuilt_row`). When `reuse_available` is
    /// `false`, gating is disabled entirely and every group with URL signal
    /// is always redetected — the original (Pass A) behavior.
    ///
    /// `reuse_available` must be `true` **only** when the caller is about to
    /// reuse rows skipped here from a previous, still-valid, already-merged
    /// result (i.e. the incremental fast path in
    /// `visible_as_tchars_and_tags_extended`, and only once it has confirmed
    /// `MergeCache::fp` still matches). Every other caller —
    /// [`Self::rows_as_tchars_and_tags_cached`] (used by the scrollback path,
    /// which has no merge cache at all) and any debug-only oracle
    /// recomputation — must pass `false`. Without a previous merge to fall
    /// back on, skipping redetection for a clean-but-never-refined group
    /// would permanently bake the group's raw, wrap-truncated per-row
    /// `auto_urls` into the merge output instead of the correct
    /// wrap-boundary-safe URL, since nothing else will ever re-derive it.
    ///
    /// **Why skipping a clean group cannot diverge from re-detecting it
    /// anyway, when `reuse_available` is `true`:** when a group is skipped,
    /// every row in it keeps whatever `auto_urls` its (unchanged)
    /// `RowCacheEntry` already holds — the exact same `auto_urls` the
    /// *previous* call's redetection left in place, because nothing rebuilt
    /// those entries in between. Two cases for where that group sits
    /// relative to the incremental merge boundary computed from
    /// `first_rebuilt_row`:
    ///
    /// - The group lies **entirely before** the boundary: those rows are
    ///   reused verbatim from the previous merge's cached prefix (never
    ///   re-merged), so their previously-spliced URL tags are exactly what
    ///   ships — consistent by construction. This is exactly the case
    ///   `reuse_available` promises will hold.
    /// - The group has **any row at or after** the boundary: that row's
    ///   cache entry was itself rebuilt (`dirty` or `None`), which is exactly
    ///   the condition that sets `group_has_rebuilt_row` for this group, so
    ///   redetection **does** fire and the tail (incremental) merge consumes
    ///   the freshly refined entry — never silently falls back to a stale
    ///   per-row `auto_urls`.
    ///
    /// So a group is only ever skipped when either (a) it is wholly reused
    /// from the cached prefix (correct: nothing new to redetect), or (b) it
    /// is wholly within the freshly-merged tail but happens to have no
    /// rebuilt row — impossible, since "within the tail" is defined by
    /// having a rebuilt row. There is no third case — **provided the
    /// prefix-reuse promise `reuse_available` encodes actually holds**,
    /// which is the caller's responsibility to verify before passing `true`.
    fn refresh_row_cache_and_refine_wrapped_urls(
        rows: &mut [Row],
        cache: &mut [Option<RowCacheEntry>],
        auto_detect: bool,
        reuse_available: bool,
    ) -> (Vec<(usize, Vec<AutoUrlRange>)>, Option<usize>) {
        let row_count = rows.len();
        let mut refined_auto_urls: Vec<(usize, Vec<AutoUrlRange>)> = Vec::new();
        let mut group_start = 0usize;
        let mut group_has_url_signal = false;
        let mut group_has_rebuilt_row = false;
        let mut first_rebuilt_row: Option<usize> = None;

        for row_idx in 0..row_count {
            // Invalidate cache entries that were built with a different
            // `auto_detect` mode than the one currently in effect. When
            // auto_detect is true we need `bytes` populated; when false the
            // cache entry may still have them but that is harmless — we keep
            // the entry in that case.
            let rebuilt_this_row = {
                let row = &mut rows[row_idx];
                let needs_rebuild = row.dirty
                    || cache[row_idx].is_none()
                    || (auto_detect
                        && cache[row_idx]
                            .as_ref()
                            .is_some_and(|e| e.bytes.is_empty() && !e.chars.is_empty()));
                if needs_rebuild {
                    cache[row_idx] = Some(Self::flatten_row(row, auto_detect));
                    row.mark_clean();
                }
                needs_rebuild
            };

            if rebuilt_this_row {
                if first_rebuilt_row.is_none() {
                    first_rebuilt_row = Some(row_idx);
                }
                group_has_rebuilt_row = true;
            }

            if !auto_detect {
                continue;
            }

            // A new logical-line group starts at row 0, or wherever a row is
            // not a soft-wrap continuation of the previous one. Runs of
            // `RowJoin::ContinueLogicalLine` rows are one DECAWM-wrapped
            // logical line; see `redetect_urls_for_group`'s doc comment.
            let starts_new_group =
                row_idx == 0 || rows[row_idx].join != RowJoin::ContinueLogicalLine;
            if starts_new_group {
                if row_idx - group_start > 1
                    && group_has_url_signal
                    && (!reuse_available || group_has_rebuilt_row)
                {
                    redetect_urls_for_group(cache, group_start, row_idx, &mut refined_auto_urls);
                }
                group_start = row_idx;
                group_has_url_signal = false;
                group_has_rebuilt_row = rebuilt_this_row;
            }
            // Two independent signals that this row might be part of a
            // wrapped URL:
            //
            // 1. A match whose raw (pre-trim) end reached this row's raw
            //    byte end — a URL that ends naturally mid-row (the common
            //    case: followed by whitespace or more prose) can never be
            //    split by a wrap. Only the last match in a row can possibly
            //    reach the row's end (matches are found in increasing
            //    order), so checking `.last()` suffices.
            // 2. The row's tail looks like a *partial* scheme prefix (e.g.
            //    "see htt"), meaning the wrap split the scheme itself — in
            //    that case `find_urls_bytes` found no match at all on this
            //    row, so signal 1 alone would miss it entirely (not just
            //    truncate it).
            if let Some(entry) = cache[row_idx].as_ref() {
                let touches_end = entry.auto_urls.last().is_some_and(|r| r.touches_row_end);
                if touches_end || entry.tail_could_be_wrapped_scheme {
                    group_has_url_signal = true;
                }
            }
        }
        // Finalize the last group (the loop above only finalizes a group
        // once it sees the *next* group start).
        if auto_detect
            && row_count - group_start > 1
            && group_has_url_signal
            && (!reuse_available || group_has_rebuilt_row)
        {
            redetect_urls_for_group(cache, group_start, row_count, &mut refined_auto_urls);
        }

        (refined_auto_urls, first_rebuilt_row)
    }

    /// Collect the indices of tags in `tags` that carry a URL.
    ///
    /// This is a cheap post-pass over the already-built tag vector — typically
    /// O(tags) where tags is small.  The result enables the GUI to iterate
    /// only URL-bearing tags instead of scanning all tags during hover
    /// detection.
    fn collect_url_tag_indices(tags: &[FormatTag]) -> Vec<usize> {
        tags.iter()
            .enumerate()
            .filter_map(|(i, tag)| tag.url.as_ref().map(|_| i))
            .collect()
    }

    /// Flatten a single [`Row`] into a [`RowCacheEntry`].
    ///
    /// Tag offsets are **row-relative** (start at 0 for the first character in
    /// this row).  The caller is responsible for re-basing them into global
    /// offsets when merging multiple rows.
    ///
    /// When `auto_detect` is `true`, also builds the UTF-8 byte buffer and
    /// the byte→char map in the same cell loop, and runs
    /// [`url_detect::find_urls_bytes`] to populate `auto_urls`.
    fn flatten_row(row: &Row, auto_detect: bool) -> RowCacheEntry {
        let mut chars: Vec<TChar> = Vec::new();
        let mut tags: Vec<FormatTag> = Vec::new();
        let mut bytes: Vec<u8> = Vec::new();
        let mut byte_to_char: Vec<u32> = Vec::new();

        for cell in row.characters() {
            // Skip wide-glyph continuation cells.
            if cell.is_continuation() {
                continue;
            }

            let char_idx = chars.len();
            let tc = *cell.tchar();
            chars.push(tc);

            // Build the byte mirror in the same pass when auto-detect is on.
            if auto_detect {
                let tc_bytes = tc.as_bytes();
                // `char_idx` fits in u32 for any reasonable row width; saturate
                // defensively. Rows never exceed a few thousand cells.
                let char_idx_u32 = u32::try_from(char_idx).unwrap_or(u32::MAX);
                for &b in tc_bytes {
                    bytes.push(b);
                    byte_to_char.push(char_idx_u32);
                }
            }

            let cell_tag = cell.tag();
            if let Some(last) = tags.last_mut() {
                if last.end == char_idx && tags_same_format(last, cell_tag) {
                    last.end += 1;
                } else {
                    tags.push(FormatTag {
                        start: char_idx,
                        end: char_idx + 1,
                        colors: cell_tag.colors,
                        font_weight: cell_tag.font_weight,
                        font_decorations: cell_tag.font_decorations,
                        url: cell_tag.url.clone(),
                        blink: cell_tag.blink,
                    });
                }
            } else {
                tags.push(FormatTag {
                    start: char_idx,
                    end: char_idx + 1,
                    colors: cell_tag.colors,
                    font_weight: cell_tag.font_weight,
                    font_decorations: cell_tag.font_decorations,
                    url: cell_tag.url.clone(),
                    blink: cell_tag.blink,
                });
            }
        }

        // Guarantee at least one tag even for an empty row.
        if tags.is_empty() {
            tags.push(FormatTag {
                start: 0,
                end: 0,
                ..FormatTag::default()
            });
        }

        // Run URL detection on the byte buffer, translate byte offsets into
        // character indices via `byte_to_char`.
        let auto_urls = if auto_detect && !bytes.is_empty() {
            build_auto_urls(&bytes, &byte_to_char)
        } else {
            Vec::new()
        };

        // Precompute the partial-scheme-tail signal once here (see
        // `RowCacheEntry::tail_could_be_wrapped_scheme`'s doc comment) so it
        // is a cached O(1) read at merge time instead of a fresh byte scan
        // on every flatten call.
        let tail_could_be_wrapped_scheme =
            auto_detect && url_detect::row_tail_could_be_wrapped_scheme(&bytes);

        RowCacheEntry {
            chars,
            tags,
            bytes,
            byte_to_char,
            auto_urls,
            tail_could_be_wrapped_scheme,
        }
    }

    /// Return `true` when the alternate screen is currently active.
    #[must_use]
    pub const fn is_alternate_screen(&self) -> bool {
        matches!(self.kind, BufferType::Alternate)
    }

    /// Return `true` when a cursor has been saved via DECSC (ESC 7 / `\x1b[?1048h`).
    #[must_use]
    pub const fn has_saved_cursor(&self) -> bool {
        self.saved_cursor.is_some()
    }

    /// Return the terminal width (columns).
    #[must_use]
    pub const fn terminal_width(&self) -> usize {
        self.width
    }

    /// Return the terminal height (rows).
    #[must_use]
    pub const fn terminal_height(&self) -> usize {
        self.height
    }

    /// Extract the text content of a selection range from the buffer.
    ///
    /// Coordinates are buffer-absolute row indices (0 = first row in the full
    /// buffer including scrollback). Columns are 0-indexed cell positions.
    /// The range is inclusive on both ends: `[start_row, start_col]` through
    /// `[end_row, end_col]`.
    ///
    /// Trailing whitespace on each row is trimmed (standard terminal behaviour).
    /// Rows are separated by `'\n'`.
    #[must_use]
    pub fn extract_text(
        &self,
        start_row: usize,
        start_col: usize,
        end_row: usize,
        end_col: usize,
    ) -> String {
        use std::fmt::Write as _;

        if start_row >= self.rows.len() {
            return String::new();
        }
        let end_row = end_row.min(self.rows.len().saturating_sub(1));

        let mut result = String::new();

        for row_idx in start_row..=end_row {
            // Task 119.4: `extract_text` takes `&self`, so it cannot call
            // `ensure_decompressed` (which needs `&mut self`). A row
            // evicted to a compressed block is resolved via a transient,
            // non-mutating peek instead — see `row_cells_for_read`.
            let cells = self.row_cells_for_read(row_idx);

            let col_begin = if row_idx == start_row { start_col } else { 0 };
            let col_end = if row_idx == end_row {
                end_col
            } else {
                cells.len().saturating_sub(1)
            };

            let mut row_text = String::new();
            for col in col_begin..=col_end {
                if col >= cells.len() {
                    break;
                }
                let cell = &cells[col];
                if cell.is_continuation() {
                    continue;
                }
                let tc = cell.tchar();
                if matches!(tc, TChar::NewLine) {
                    break;
                }
                write!(&mut row_text, "{tc}").unwrap_or_default();
            }

            let trimmed = row_text.trim_end();
            result.push_str(trimmed);

            if row_idx < end_row {
                result.push('\n');
            }
        }

        result
    }

    /// Extract a rectangular block of text from the buffer.
    ///
    /// Every row from `start_row` to `end_row` (inclusive) is sampled between
    /// the same `col_min`..=`col_max` column range, where
    /// `col_min = start_col.min(end_col)` and `col_max = start_col.max(end_col)`.
    /// Rows are joined with `\n`.  Trailing whitespace is trimmed per row.
    ///
    /// This is the copy behaviour for Alt+drag (block/rectangular) selections.
    #[must_use]
    pub fn extract_block_text(
        &self,
        start_row: usize,
        start_col: usize,
        end_row: usize,
        end_col: usize,
    ) -> String {
        use std::fmt::Write as _;

        if start_row >= self.rows.len() {
            return String::new();
        }
        let end_row = end_row.min(self.rows.len().saturating_sub(1));
        let col_min = start_col.min(end_col);
        let col_max = start_col.max(end_col);

        let mut result = String::new();

        for row_idx in start_row..=end_row {
            // Task 119.4: see the matching comment in `extract_text` — this
            // also takes `&self` and must resolve an evicted row without
            // mutating `Buffer` state.
            let cells = self.row_cells_for_read(row_idx);

            let mut row_text = String::new();
            for col in col_min..=col_max {
                if col >= cells.len() {
                    break;
                }
                let cell = &cells[col];
                if cell.is_continuation() {
                    continue;
                }
                let tc = cell.tchar();
                if matches!(tc, TChar::NewLine) {
                    break;
                }
                write!(&mut row_text, "{tc}").unwrap_or_default();
            }

            let trimmed = row_text.trim_end();
            result.push_str(trimmed);

            if row_idx < end_row {
                result.push('\n');
            }
        }

        result
    }
}

/// Convert a byte range into a character range using a `byte_to_char` map.
///
/// `byte_to_char[i]` is the character index for the character that starts at
/// byte `i` of the buffer `byte_to_char` was built for (`bytes.len()` bytes
/// long). Returns `None` when the map is malformed (should never happen, but
/// this is production code so we cannot unwrap) or when the resulting range
/// is empty.
fn byte_range_to_char_range(
    byte_start: usize,
    byte_end: usize,
    bytes_len: usize,
    byte_to_char: &[u32],
) -> Option<(usize, usize)> {
    let &start_u32 = byte_to_char.get(byte_start)?;
    // `byte_end` is exclusive; we want the character index *after* the last
    // included character. If `byte_end` reaches the end of the buffer, use
    // one past the last character index (inferred from the last byte's char
    // index).
    let end_char_u32 = if byte_end >= bytes_len {
        byte_to_char
            .last()
            .copied()
            .map_or(0, |c| c.saturating_add(1))
    } else {
        *byte_to_char.get(byte_end)?
    };

    let char_start = usize::try_from(start_u32).ok()?;
    let char_end = usize::try_from(end_char_u32).ok()?;

    if char_end <= char_start {
        return None;
    }

    Some((char_start, char_end))
}

/// Convert the detected URL byte ranges into row-local character ranges.
///
/// `bytes` is the row's UTF-8 byte buffer; `byte_to_char` maps each byte
/// offset to the starting character index of the character at that byte
/// position. The returned [`AutoUrlRange`]s carry character indices and the
/// URL string as an `Arc<Url>` ready to splice into `FormatTag`s.
fn build_auto_urls(bytes: &[u8], byte_to_char: &[u32]) -> Vec<AutoUrlRange> {
    let matches = url_detect::find_urls_bytes(bytes);
    let mut out = Vec::with_capacity(matches.len());

    for m in matches {
        let Some((char_start, char_end)) =
            byte_range_to_char_range(m.byte_start, m.byte_end, bytes.len(), byte_to_char)
        else {
            continue;
        };

        // Build the URL string from the matched byte range. This is the only
        // per-match string allocation in the pipeline; it is amortised by the
        // row-level cache.
        let url_str = match std::str::from_utf8(&bytes[m.byte_start..m.byte_end]) {
            Ok(s) => s.to_string(),
            Err(_) => continue,
        };

        out.push(AutoUrlRange {
            char_start,
            char_end,
            url: Arc::new(Url {
                id: None,
                url: url_str,
            }),
            touches_row_end: m.touches_buffer_end,
        });
    }

    out
}

/// Re-detect auto URLs across a run of rows joined by
/// `RowJoin::ContinueLogicalLine` (`rows[group_start..group_end)`).
///
/// Concatenates the already-cached per-row byte buffers into one buffer and
/// runs [`url_detect::find_urls_bytes`] once on the result, so that a URL
/// wrap boundary is no longer mistaken for the URL's real end — this both
/// stops truncation and avoids `trim_trailing` misfiring on a wrap boundary
/// that happens to land right before stripped punctuation. Each match's byte
/// range is then mapped back across the contributing rows (via each row's own
/// `byte_to_char` map) into per-row [`AutoUrlRange`]s that all share one
/// `Arc<Url>` holding the full, untruncated URL text.
///
/// Appends an entry for every row in the group (an empty `Vec` when no match
/// touches that row), replacing whatever the row's own single-row detection
/// found. Caller is expected to have already checked that the group actually
/// contains URL-looking content before calling this (this function does not
/// check `auto_detect` or the join flags itself — it operates purely on the
/// cache), and to call groups in ascending `group_start` order so `refined`
/// stays sorted by `row_idx`.
fn redetect_urls_for_group(
    cache: &[Option<RowCacheEntry>],
    group_start: usize,
    group_end: usize,
    refined: &mut Vec<(usize, Vec<AutoUrlRange>)>,
) {
    let mut group_bytes: Vec<u8> = Vec::new();
    let mut row_byte_start: Vec<usize> = Vec::with_capacity(group_end - group_start);
    for entry in &cache[group_start..group_end] {
        row_byte_start.push(group_bytes.len());
        if let Some(entry) = entry.as_ref() {
            group_bytes.extend_from_slice(&entry.bytes);
        }
    }
    let group_total_len = group_bytes.len();

    let matches = url_detect::find_urls_bytes(&group_bytes);

    // Every row in the group gets an authoritative (possibly empty) refined
    // entry, superseding its own single-row `auto_urls` — the group
    // redetection reproduces equivalent matches for URLs fully contained in
    // one row too, so nothing is lost by replacing wholesale.
    let first_new_idx = refined.len();
    refined.extend((group_start..group_end).map(|row_idx| (row_idx, Vec::new())));

    if matches.is_empty() {
        return;
    }

    for m in matches {
        let Ok(url_str) = std::str::from_utf8(&group_bytes[m.byte_start..m.byte_end]) else {
            continue;
        };
        let shared_url = Arc::new(Url {
            id: None,
            url: url_str.to_string(),
        });

        for offset_idx in 0..(group_end - group_start) {
            let row_idx = group_start + offset_idx;
            let row_byte_lo = row_byte_start[offset_idx];
            let row_byte_hi = row_byte_start
                .get(offset_idx + 1)
                .copied()
                .unwrap_or(group_total_len);

            // No overlap between the match and this row's byte span.
            if m.byte_end <= row_byte_lo || m.byte_start >= row_byte_hi {
                continue;
            }

            let Some(entry) = cache[row_idx].as_ref() else {
                continue;
            };
            if entry.bytes.is_empty() {
                continue;
            }

            let local_start = m.byte_start.max(row_byte_lo) - row_byte_lo;
            let local_end = m.byte_end.min(row_byte_hi) - row_byte_lo;

            let Some((char_start, char_end)) = byte_range_to_char_range(
                local_start,
                local_end,
                entry.bytes.len(),
                &entry.byte_to_char,
            ) else {
                continue;
            };

            let (_, row_ranges) = &mut refined[first_new_idx + offset_idx];
            row_ranges.push(AutoUrlRange {
                char_start,
                char_end,
                url: shared_url.clone(),
                touches_row_end: local_end >= entry.bytes.len(),
            });
        }
    }
}

/// Splice auto-detected URL ranges into a row's per-row tag vec.
///
/// For each [`AutoUrlRange`], covering tags (those whose
/// `[start, end)` overlaps `[char_start, char_end)`) are split into up to
/// three pieces: pre-range (unchanged), overlapping (inheriting the base
/// tag's visual attributes but with `url = Some(range.url)`), and post-range
/// (unchanged).
///
/// **OSC 8 precedence**: when a covering tag already has `url.is_some()`,
/// the auto-URL is suppressed within that tag — the OSC 8 link wins. This
/// check happens per-tag, so a range that starts inside an OSC 8 link and
/// extends past it is still partially spliced into the non-OSC 8 segments.
///
/// The returned vec is sorted by `start` and has no overlapping tags, the
/// same invariants the merge step downstream expects.
fn splice_auto_urls(tags: &[FormatTag], ranges: &[AutoUrlRange]) -> Vec<FormatTag> {
    if ranges.is_empty() {
        return tags.to_vec();
    }

    // Accumulator for output tags. We splice one range at a time against the
    // current accumulator, which keeps invariants simple.
    let mut current: Vec<FormatTag> = tags.to_vec();

    for range in ranges {
        let mut next: Vec<FormatTag> = Vec::with_capacity(current.len() + 2);
        for tag in &current {
            // No overlap → keep as-is.
            if tag.end <= range.char_start || tag.start >= range.char_end {
                next.push(tag.clone());
                continue;
            }

            // OSC 8 precedence: tag already has a URL → keep entire tag
            // unchanged within the range's span.
            if tag.url.is_some() {
                next.push(tag.clone());
                continue;
            }

            // Compute split points, clamped into `tag`'s own bounds.
            let mid_start = range.char_start.max(tag.start);
            let mid_end = range.char_end.min(tag.end);

            // Pre-overlap segment (if any).
            if tag.start < mid_start {
                next.push(FormatTag {
                    start: tag.start,
                    end: mid_start,
                    colors: tag.colors,
                    font_weight: tag.font_weight,
                    font_decorations: tag.font_decorations,
                    url: tag.url.clone(),
                    blink: tag.blink,
                });
            }

            // Overlap segment with the auto-URL attached.
            next.push(FormatTag {
                start: mid_start,
                end: mid_end,
                colors: tag.colors,
                font_weight: tag.font_weight,
                font_decorations: tag.font_decorations,
                url: Some(range.url.clone()),
                blink: tag.blink,
            });

            // Post-overlap segment (if any).
            if mid_end < tag.end {
                next.push(FormatTag {
                    start: mid_end,
                    end: tag.end,
                    colors: tag.colors,
                    font_weight: tag.font_weight,
                    font_decorations: tag.font_decorations,
                    url: tag.url.clone(),
                    blink: tag.blink,
                });
            }
        }
        current = next;
    }

    current
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod extended_window_tests {
    use crate::buffer::Buffer;
    use freminal_common::buffer_states::tchar::TChar;

    fn t(s: &str) -> Vec<TChar> {
        s.chars().map(TChar::from).collect()
    }

    /// Build a 4-wide, 3-tall buffer containing 6 rows of distinct content
    /// ("r0".."r5"), so rows 0..=2 are scrollback and rows 3..=5 are the live
    /// visible window.
    fn buffer_with_scrollback() -> Buffer {
        let mut buf = Buffer::new(4, 3);
        for i in 0..6 {
            buf.insert_text(&t(&format!("r{i}")));
            if i < 5 {
                buf.handle_lf();
                buf.handle_cr();
            }
        }
        assert_eq!(buf.rows().len(), 6, "expected 6 total rows");
        buf
    }

    #[test]
    fn bounds_no_extra_is_normal_window() {
        let buf = buffer_with_scrollback();
        // height = 3, total = 6, scroll 0 → normal window [3, 6).
        let (start, end) = buf.visible_window_bounds(0, 0);
        assert_eq!((start, end), (3, 6));
    }

    #[test]
    fn bounds_extra_extends_window_upward() {
        let buf = buffer_with_scrollback();
        // Pull in 2 extra rows above the window: [1, 6).
        let (start, end) = buf.visible_window_bounds(0, 2);
        assert_eq!((start, end), (1, 6));
    }

    #[test]
    fn bounds_extra_clamped_at_row_zero() {
        let buf = buffer_with_scrollback();
        // Request more extra rows than exist above the window: clamps to 0.
        let (start, end) = buf.visible_window_bounds(0, 99);
        assert_eq!((start, end), (0, 6));
    }

    #[test]
    fn bounds_extra_with_scroll_offset() {
        let buf = buffer_with_scrollback();
        // Scrolled back 1 row → normal window [2, 5); plus 1 extra → [1, 5).
        let (start, end) = buf.visible_window_bounds(1, 1);
        assert_eq!((start, end), (1, 5));
    }

    #[test]
    fn extended_flatten_has_more_rows() {
        let mut buf = buffer_with_scrollback();
        let (_c0, _t0, ro0, _u0) = buf.visible_as_tchars_and_tags_extended(0, 0);
        let (_c2, _t2, ro2, _u2) = buf.visible_as_tchars_and_tags_extended(0, 2);
        assert_eq!(ro0.len(), 3, "normal window has term_height rows");
        assert_eq!(ro2.len(), 5, "extended window has term_height + extra rows");
    }

    #[test]
    fn extended_flatten_starts_earlier() {
        let mut buf = buffer_with_scrollback();
        // The extended window's first row should be buffer row 1 ("r1").
        let (chars, _tags, row_offsets, _url) = buf.visible_as_tchars_and_tags_extended(0, 2);
        // First row's first char is 'r'.
        assert_eq!(chars[row_offsets[0]], TChar::from('r'));
        // Second char of first row is '1' (buffer row 1).
        assert_eq!(chars[row_offsets[0] + 1], TChar::from('1'));
    }

    #[test]
    fn extended_line_widths_match_extended_window() {
        let buf = buffer_with_scrollback();
        assert_eq!(buf.visible_line_widths_extended(0, 0).len(), 3);
        assert_eq!(buf.visible_line_widths_extended(0, 2).len(), 5);
    }

    #[test]
    fn extended_dirty_check_covers_extra_rows() {
        let mut buf = buffer_with_scrollback();
        // Flatten the extended window to clear dirty flags across all 5 rows.
        let _ = buf.visible_as_tchars_and_tags_extended(0, 2);
        assert!(
            !buf.any_visible_dirty_extended(0, 2),
            "freshly flattened extended window must be clean"
        );
    }
}

/// Task 118.4: cold scrollback rows must not retain the second (row-cache)
/// and third (decompaction-memo) copies of their cell data after a
/// full-scrollback flatten, while output stays byte-identical across
/// repeated flattens and visible-row caches stay warm.
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod scrollback_eviction_tests {
    use crate::buffer::Buffer;
    use crate::image_store::{ImagePlacement, ImageProtocol};
    use crate::row::Row;
    use freminal_common::buffer_states::{format_tag::FormatTag, tchar::TChar};

    fn text(s: &str) -> Vec<TChar> {
        s.chars().map(TChar::from).collect()
    }

    /// Push `n` numbered lines (LF+CR terminated, matching real PTY output)
    /// into `buf`. Mirrors the helper in
    /// `crate::buffer::scrollback_compaction_tests`. This is a hot-path
    /// fill only: compaction is deferred (Task 118 follow-up), so pushing
    /// lines past the visible window does NOT compact anything on its own
    /// — callers that need compacted scrollback must call
    /// `Buffer::compact_idle_scrollback` explicitly afterward.
    fn push_numbered_lines(buf: &mut Buffer, n: usize) {
        for i in 0..n {
            buf.insert_text(&text(&format!("line{i:04}content")));
            buf.handle_lf();
            buf.handle_cr();
        }
    }

    fn buffer_with_compacted_scrollback() -> Buffer {
        let mut buf = Buffer::new(20, 3).with_scrollback_limit(50);
        push_numbered_lines(&mut buf, 20);
        let _ = buf.compact_idle_scrollback(usize::MAX);
        let visible_start = buf.visible_window_start(0);
        assert!(visible_start > 0, "test needs scrollback rows to exist");
        assert!(
            buf.rows[..visible_start].iter().any(Row::is_compact),
            "expected scrollback compaction to have engaged"
        );
        buf
    }

    #[test]
    fn two_consecutive_scrollback_flattens_are_byte_identical() {
        let mut buf = buffer_with_compacted_scrollback();

        // First flatten: populates row_cache + decompaction memos, then
        // (per Task 118.4) evicts both for every compact row.
        let (chars1, tags1, offsets1, urls1) = buf.scrollback_as_tchars_and_tags(0);

        // Second flatten: must rebuild from the still-resident `CompactRow`
        // data and produce byte-identical output.
        let (chars2, tags2, offsets2, urls2) = buf.scrollback_as_tchars_and_tags(0);

        assert_eq!(chars1, chars2, "flattened characters must be identical");
        assert_eq!(tags1, tags2, "flattened format tags must be identical");
        assert_eq!(offsets1, offsets2, "row offsets must be identical");
        assert_eq!(urls1, urls2, "url tag indices must be identical");
    }

    #[test]
    fn scrollback_flatten_evicts_compact_row_cache_and_memo() {
        let mut buf = buffer_with_compacted_scrollback();
        let visible_start = buf.visible_window_start(0);

        let _ = buf.scrollback_as_tchars_and_tags(0);

        for (row, entry) in buf.rows[..visible_start]
            .iter()
            .zip(buf.row_cache[..visible_start].iter())
        {
            if row.is_compact() {
                assert!(
                    entry.is_none(),
                    "a compact scrollback row's RowCacheEntry must be evicted after a scrollback flatten"
                );
            }
        }

        // Rows must remain compact — eviction drops the cache/memo, not the
        // compact representation itself.
        assert!(
            buf.rows[..visible_start].iter().any(Row::is_compact),
            "rows must remain compact after cache eviction"
        );

        buf.debug_assert_invariants();
    }

    #[test]
    fn scrollback_flatten_does_not_evict_visible_row_cache() {
        let mut buf = buffer_with_compacted_scrollback();
        let visible_start = buf.visible_window_start(0);

        // Warm the visible row cache first.
        let _ = buf.visible_as_tchars_and_tags(0);
        assert!(
            buf.row_cache[visible_start..].iter().all(Option::is_some),
            "sanity: visible row cache should be populated before the scrollback flatten"
        );

        let _ = buf.scrollback_as_tchars_and_tags(0);

        assert!(
            buf.row_cache[visible_start..].iter().all(Option::is_some),
            "a scrollback flatten must not evict the visible window's row cache"
        );
    }

    #[test]
    fn url_in_compacted_scrollback_row_detected_after_eviction_rebuild() {
        let mut buf = Buffer::new(40, 3).with_scrollback_limit(50);
        assert!(buf.auto_detect_urls(), "test relies on default auto-detect");

        buf.insert_text(&text("see http://example.com for info"));
        buf.handle_lf();
        buf.handle_cr();
        push_numbered_lines(&mut buf, 20);
        let _ = buf.compact_idle_scrollback(usize::MAX);

        let visible_start = buf.visible_window_start(0);
        assert!(
            buf.rows[..visible_start].iter().any(Row::is_compact),
            "expected scrollback compaction to have engaged"
        );

        // First flatten: builds the RowCacheEntry (running URL detection),
        // then evicts it (and the decompaction memo) per Task 118.4.
        let (_chars1, tags1, _offsets1, url_indices1) = buf.scrollback_as_tchars_and_tags(0);
        assert!(
            !url_indices1.is_empty(),
            "URL must be auto-detected on the first (populating) flatten"
        );

        // Second flatten: RowCacheEntry is `None` (evicted), so the row is
        // rebuilt via `flatten_row`, re-running URL detection from scratch.
        let (_chars2, tags2, _offsets2, url_indices2) = buf.scrollback_as_tchars_and_tags(0);
        assert!(
            !url_indices2.is_empty(),
            "URL must still be auto-detected on the second flatten after eviction"
        );
        assert_eq!(
            tags1, tags2,
            "re-detected URL tags must match the original detection exactly"
        );
    }

    #[test]
    fn image_opt_out_row_is_not_evicted_by_scrollback_flatten() {
        let mut buf = Buffer::new(10, 3).with_scrollback_limit(50);

        // Stamp an image cell into row 0 before it scrolls into history —
        // image rows opt out of compaction (`CompactRow::from_row`), so this
        // row must stay `Live` all the way through.
        let placement = ImagePlacement {
            image_id: 1,
            col_in_image: 0,
            row_in_image: 0,
            protocol: ImageProtocol::Sixel,
            image_number: None,
            placement_id: None,
            z_index: 0,
            source_crop: None,
            placement_instance: 1,
            subcell_offset: None,
        };
        buf.set_image_cell_at(0, 0, placement, FormatTag::default());
        buf.handle_lf();
        buf.handle_cr();
        push_numbered_lines(&mut buf, 20);

        let visible_start = buf.visible_window_start(0);
        assert!(visible_start > 0, "test needs the image row in scrollback");
        assert!(
            !buf.rows[0].is_compact(),
            "an image row must never be compacted, even in scrollback"
        );

        let _ = buf.scrollback_as_tchars_and_tags(0);

        // Eviction only targets `is_compact()` rows; a Live (opt-out) row's
        // cache is left untouched.
        assert!(
            buf.row_cache[0].is_some(),
            "a non-compact (image) scrollback row's cache must not be evicted"
        );
        assert!(!buf.rows[0].is_compact());
        assert!(
            buf.rows[0].cells().iter().any(crate::cell::Cell::has_image),
            "image cell data must survive the scrollback flatten"
        );

        // A second flatten must still work without panicking and keep
        // reporting the image row intact.
        let _ = buf.scrollback_as_tchars_and_tags(0);
        assert!(buf.rows[0].cells().iter().any(crate::cell::Cell::has_image));
    }
}

/// Regression coverage for GitHub issue #418: a plain-text URL that
/// DECAWM-wraps across two or more physical rows must be auto-detected as
/// one full, untruncated URL — not as several independent per-row fragments.
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod wrapped_url_tests {
    use crate::buffer::Buffer;
    use crate::row::RowJoin;
    use freminal_common::buffer_states::tchar::TChar;

    fn text(s: &str) -> Vec<TChar> {
        s.chars().map(TChar::from).collect()
    }

    /// Collect the distinct URL strings carried by the tags at `url_indices`.
    fn url_strings(
        tags: &[freminal_common::buffer_states::format_tag::FormatTag],
        url_indices: &[usize],
    ) -> Vec<String> {
        url_indices
            .iter()
            .filter_map(|&i| tags[i].url.as_ref().map(|u| u.url.clone()))
            .collect()
    }

    #[test]
    fn url_wrapping_across_two_rows_is_detected_in_full() {
        // Width chosen so the URL itself (not just surrounding prose) spans
        // the row boundary: "see " (4) + first part of the URL fills row 0,
        // the remainder wraps onto row 1.
        let mut buf = Buffer::new(20, 3);
        assert!(buf.auto_detect_urls());
        let url = "https://example.com/a/very/long/path";
        buf.insert_text(&text(&format!("see {url} end")));

        assert_eq!(
            buf.rows()[1].join,
            RowJoin::ContinueLogicalLine,
            "test setup: row 1 must be a soft-wrap continuation of row 0"
        );

        let (_chars, tags, _row_offsets, url_indices) = buf.visible_as_tchars_and_tags(0);
        assert!(!url_indices.is_empty(), "URL must be auto-detected");

        let urls = url_strings(&tags, &url_indices);
        assert!(
            urls.iter().all(|u| u == url),
            "every URL-tagged fragment must carry the full, untruncated URL; got {urls:?}"
        );
    }

    #[test]
    fn url_wrapping_across_three_rows_is_detected_in_full() {
        // A long URL that wraps twice (spans three physical rows), to
        // exercise the multi-row chaining (not just a single adjacent pair).
        let mut buf = Buffer::new(15, 5);
        assert!(buf.auto_detect_urls());
        let url = "https://example.com/a/very/long/path/that/keeps/going/and/going";
        buf.insert_text(&text(url));

        assert_eq!(buf.rows()[1].join, RowJoin::ContinueLogicalLine);
        assert_eq!(buf.rows()[2].join, RowJoin::ContinueLogicalLine);

        let (_chars, tags, _row_offsets, url_indices) = buf.visible_as_tchars_and_tags(0);
        assert!(!url_indices.is_empty(), "URL must be auto-detected");

        let urls = url_strings(&tags, &url_indices);
        assert!(
            urls.iter().all(|u| u == url),
            "every URL-tagged fragment must carry the full, untruncated URL; got {urls:?}"
        );
    }

    #[test]
    fn wrap_boundary_on_trailing_punctuation_char_is_not_stripped() {
        // Width chosen so the wrap boundary lands exactly on the '.' in
        // "index.html" — a single-row detector's `trim_trailing` heuristic
        // would misidentify that '.' as sentence punctuation and strip it,
        // even though it is a real path separator continued on the next row.
        let mut buf = Buffer::new(26, 3);
        assert!(buf.auto_detect_urls());
        let url = "https://example.com/index.html";
        buf.insert_text(&text(url));

        assert_eq!(
            buf.rows()[0].cells().len(),
            26,
            "row 0 must be exactly full width"
        );
        assert_eq!(buf.rows()[1].join, RowJoin::ContinueLogicalLine);

        let (_chars, tags, _row_offsets, url_indices) = buf.visible_as_tchars_and_tags(0);
        assert!(!url_indices.is_empty(), "URL must be auto-detected");

        let urls = url_strings(&tags, &url_indices);
        assert!(
            urls.iter().all(|u| u == url),
            "the '.' before the wrap must be preserved, not stripped; got {urls:?}"
        );
    }

    #[test]
    fn hard_break_after_full_width_url_does_not_merge_with_next_line() {
        // Row 0 is filled exactly by a URL with no trailing content (so its
        // per-row match already reaches the row's raw end), but row 1 is a
        // genuine new logical line (hard break, not a soft wrap) containing
        // an unrelated URL starting at column 0. These must NOT be merged
        // into one URL.
        let url_a = "https://a.example.com/xxxxx"; // 28 chars
        let mut buf = Buffer::new(28, 3);
        assert!(buf.auto_detect_urls());
        buf.insert_text(&text(url_a));
        buf.handle_lf();
        buf.handle_cr();
        buf.insert_text(&text("https://b.example.com"));

        assert_eq!(
            buf.rows()[1].join,
            RowJoin::NewLogicalLine,
            "test setup: row 1 must be a hard break, not a soft wrap"
        );

        let (_chars, tags, _row_offsets, url_indices) = buf.visible_as_tchars_and_tags(0);
        let urls = url_strings(&tags, &url_indices);
        assert!(urls.contains(&url_a.to_string()), "got {urls:?}");
        assert!(
            urls.contains(&"https://b.example.com".to_string()),
            "got {urls:?}"
        );
        assert!(
            urls.iter()
                .all(|u| u != &format!("{url_a}https://b.example.com")),
            "unrelated URLs on a hard-broken next line must not be merged; got {urls:?}"
        );
    }

    #[test]
    fn wrapped_line_without_any_url_is_unaffected() {
        // A long wrapped line with no URL content at all must not produce
        // any URL tags (and must not panic in the group-redetect pre-check
        // skip path).
        let mut buf = Buffer::new(10, 5);
        assert!(buf.auto_detect_urls());
        buf.insert_text(&text(
            "the quick brown fox jumps over the lazy dog again and again",
        ));
        assert_eq!(buf.rows()[1].join, RowJoin::ContinueLogicalLine);

        let (_chars, _tags, _row_offsets, url_indices) = buf.visible_as_tchars_and_tags(0);
        assert!(
            url_indices.is_empty(),
            "a wrapped line with no URL content must not produce URL tags"
        );
    }

    #[test]
    fn url_wrapping_mid_scheme_is_still_detected() {
        // Width chosen so "see http" fills row 0 exactly, splitting the
        // "https://" scheme prefix itself across the wrap boundary. Neither
        // row's own per-row `find_urls_bytes` call matches anything on its
        // own ("see http" and "s://example.com" are each missing a
        // recognized complete scheme), so `touches_row_end` alone can never
        // fire here — this is what `row_tail_could_be_wrapped_scheme` (the
        // partial-scheme-prefix signal) exists for. Regression test for a
        // CodeRabbit review finding on PR #426.
        let mut buf = Buffer::new(8, 4);
        assert!(buf.auto_detect_urls());
        let url = "https://example.com";
        buf.insert_text(&text(&format!("see {url}")));

        assert_eq!(buf.rows()[1].join, RowJoin::ContinueLogicalLine);

        let (_chars, tags, _row_offsets, url_indices) = buf.visible_as_tchars_and_tags(0);
        assert!(
            !url_indices.is_empty(),
            "a URL whose scheme is split by a wrap must still be auto-detected"
        );

        let urls = url_strings(&tags, &url_indices);
        assert!(
            urls.iter().all(|u| u == url),
            "the full URL must be reconstructed across the scheme split; got {urls:?}"
        );
    }
}

/// Task 121 Part C (pass A): property-test harness for the "full merge"
/// oracle, [`Buffer::merge_row_caches_full`], extracted from
/// [`Buffer::rows_as_tchars_and_tags_cached`] in this pass.
///
/// There is no incremental merge path yet — these tests currently only ever
/// exercise the full merge — but they pin the harness, the
/// oracle-equivalence check, and the output invariants that a later
/// incremental merge path must also satisfy against this same oracle.
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod incremental_merge_equivalence_tests {
    use crate::buffer::Buffer;
    use freminal_common::buffer_states::{
        fonts::FontWeight, format_tag::FormatTag, tchar::TChar, url::Url,
    };
    use freminal_common::colors::TerminalColor;
    use std::sync::Arc;

    fn text(s: &str) -> Vec<TChar> {
        s.chars().map(TChar::from).collect()
    }

    /// Build a 20-wide, 6-tall buffer covering a representative mix of
    /// content the merge step must handle:
    ///
    /// - a plain text row
    /// - an SGR color + bold run
    /// - a soft-wrapped multi-row URL (width 20 forces the 46-char URL to
    ///   wrap across several physical rows)
    /// - an OSC-8 hyperlink row
    /// - a blank row
    ///
    /// This produces more rows than the 6-row visible window, so the
    /// resulting buffer also has scrollback — exercising the same window
    /// slicing `visible_as_tchars_and_tags` uses in production.
    fn build_mixed_content_buffer() -> Buffer {
        let mut buf = Buffer::new(20, 6).with_scrollback_limit(50);
        assert!(
            buf.auto_detect_urls(),
            "test relies on default auto-detect being enabled"
        );

        // Row: plain text.
        buf.insert_text(&text("plain text row"));
        buf.handle_lf();
        buf.handle_cr();

        // Row: SGR color + bold run.
        let mut colored = buf.current_tag.clone();
        colored.colors.color = TerminalColor::Red;
        colored.font_weight = FontWeight::Bold;
        buf.current_tag = colored;
        buf.insert_text(&text("bold red text"));
        buf.current_tag = FormatTag::default();
        buf.handle_lf();
        buf.handle_cr();

        // Rows: soft-wrapped multi-row URL. At width 20 this 46-char URL
        // wraps across three physical rows.
        buf.insert_text(&text("https://example.com/very/long/path/that/wraps"));
        buf.handle_lf();
        buf.handle_cr();

        // Row: OSC-8 hyperlink.
        let hyperlink = Arc::new(Url {
            id: None,
            url: "https://osc8.example/target".to_string(),
        });
        buf.current_tag.url = Some(hyperlink);
        buf.insert_text(&text("click here"));
        buf.current_tag = FormatTag::default();
        buf.handle_lf();
        buf.handle_cr();

        // Row: blank.
        buf.handle_lf();
        buf.handle_cr();

        buf
    }

    /// Mirrors `two_consecutive_scrollback_flattens_are_byte_identical`
    /// (`scrollback_eviction_tests`) but for the VISIBLE window: flattening
    /// twice with no mutation in between must produce byte-identical
    /// output.
    #[test]
    fn full_merge_is_deterministic() {
        let mut buf = build_mixed_content_buffer();

        let first = buf.visible_as_tchars_and_tags(0);
        let second = buf.visible_as_tchars_and_tags(0);

        assert_eq!(
            first, second,
            "flattening the visible window twice with no mutation must be byte-identical"
        );
    }

    /// Pins the Part A extraction as behavior-preserving: replaying Step 1
    /// (`refresh_row_cache_and_refine_wrapped_urls`) followed by the
    /// extracted Step 2 (`merge_row_caches_full`) directly over the visible
    /// window must produce exactly the same tuple as the public
    /// `visible_as_tchars_and_tags` path.
    #[test]
    fn merge_helper_matches_cached_path() {
        let mut buf = build_mixed_content_buffer();

        // Reference: the real public path. This also warms the cache and
        // clears dirty flags, so the direct replay below hits the same
        // clean cache state (nothing dirtied in between).
        let expected = buf.visible_as_tchars_and_tags(0);

        let (visible_start, visible_end) = buf.visible_window_bounds(0, 0);
        buf.ensure_decompressed(visible_start..visible_end);
        let auto_detect = buf.auto_detect_urls;
        // `reuse_available = false`: this test replays Step 1 + Step 2
        // directly (bypassing `merge_cache` entirely), so gating must stay
        // disabled here too — same as the real `merge_row_caches_full`
        // (scrollback/full-merge) callers.
        let (refined, _first_rebuilt_row) = Buffer::refresh_row_cache_and_refine_wrapped_urls(
            &mut buf.rows[visible_start..visible_end],
            &mut buf.row_cache[visible_start..visible_end],
            auto_detect,
            false,
        );
        let actual =
            Buffer::merge_row_caches_full(&buf.row_cache[visible_start..visible_end], &refined);

        assert_eq!(
            expected, actual,
            "merge_row_caches_full must reproduce the cached path's output exactly"
        );
    }

    /// Minimal deterministic linear congruential generator.
    ///
    /// `rand` is not a dev-dependency of `freminal-buffer` (only `criterion`,
    /// `proptest`, and `test-log` are — see `Cargo.toml`), and this task
    /// must not add one for a single hand-rollable test-only PRNG. Constants
    /// are the widely used Knuth MMIX LCG parameters.
    struct Lcg(u64);

    impl Lcg {
        const fn new(seed: u64) -> Self {
            Self(seed)
        }

        fn next_u64(&mut self) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            self.0
        }

        /// Return a value in `[0, bound)`. Returns `0` when `bound == 0`.
        fn next_range(&mut self, bound: usize) -> usize {
            if bound == 0 {
                return 0;
            }
            let bound_u64 = u64::try_from(bound).unwrap_or(u64::MAX);
            let r = self.next_u64() % bound_u64;
            usize::try_from(r).unwrap_or(0)
        }
    }

    /// Assert the structural invariants a flattened `(chars, tags,
    /// row_offsets, url_tag_indices)` tuple must always satisfy. These are
    /// exactly the invariants a future incremental merge path must preserve
    /// relative to the full-merge oracle.
    fn assert_merge_invariants(
        chars: &[TChar],
        tags: &[FormatTag],
        row_offsets: &[usize],
        url_tag_indices: &[usize],
    ) {
        for pair in row_offsets.windows(2) {
            assert!(
                pair[0] <= pair[1],
                "row_offsets must be non-decreasing: {row_offsets:?}"
            );
        }

        for tag in tags {
            assert!(
                tag.start <= chars.len(),
                "tag.start out of bounds: {tag:?}, chars.len()={}",
                chars.len()
            );
            if tag.end == usize::MAX {
                assert!(
                    chars.is_empty(),
                    "an open-ended (usize::MAX) tag end is only valid for an empty flatten: {tag:?}"
                );
            } else {
                assert!(
                    tag.end <= chars.len(),
                    "tag.end out of bounds: {tag:?}, chars.len()={}",
                    chars.len()
                );
                assert!(
                    tag.start <= tag.end,
                    "tag.start must be <= tag.end: {tag:?}"
                );
            }
        }

        for pair in tags.windows(2) {
            assert!(
                pair[0].end <= pair[1].start,
                "tags must be sorted and non-overlapping: {pair:?}"
            );
        }

        for &idx in url_tag_indices {
            assert!(
                tags.get(idx).is_some_and(|t| t.url.is_some()),
                "url_tag_indices[{idx}] must reference a tag with url.is_some()"
            );
        }
    }

    /// Randomized (LCG-seeded, deterministic) stress test: repeatedly mutate
    /// one cell in the visible window (random ASCII char, sometimes with a
    /// random SGR color) and re-flatten, asserting the structural
    /// invariants hold after every mutation.
    ///
    /// This documents the invariants an incremental merge path must
    /// preserve; today it only exercises the full merge (there is no
    /// incremental path yet).
    #[test]
    fn full_merge_stable_under_repeated_flatten() {
        let mut buf = build_mixed_content_buffer();
        let mut rng = Lcg::new(0x0DDB_1A5E_5EED_C0DE);

        let colors = [
            TerminalColor::Red,
            TerminalColor::Green,
            TerminalColor::Blue,
            TerminalColor::Default,
        ];

        let (visible_start, visible_end) = buf.visible_window_bounds(0, 0);
        let row_span = visible_end - visible_start;
        let width = buf.terminal_width();

        for _ in 0..200 {
            let row = visible_start + rng.next_range(row_span);
            let col = rng.next_range(width);

            if rng.next_range(2) == 0 {
                let color_idx = rng.next_range(colors.len());
                let mut tag = FormatTag::default();
                tag.colors.color = colors[color_idx];
                buf.current_tag = tag;
            } else {
                buf.current_tag = FormatTag::default();
            }

            let letter_idx = rng.next_range(26);
            let letter_offset = u8::try_from(letter_idx).unwrap_or(0);
            let ascii_char = char::from(b'a' + letter_offset);

            buf.cursor.pos.y = row;
            buf.cursor.pos.x = col;
            buf.insert_text(&[TChar::from(ascii_char)]);

            let (chars, tags, row_offsets, url_tag_indices) = buf.visible_as_tchars_and_tags(0);
            assert_merge_invariants(&chars, &tags, &row_offsets, &url_tag_indices);
        }
    }
}

/// Task 121 Part C (pass C): tests for the **incremental** merge path
/// (`Buffer::rows_as_tchars_and_tags_incremental`, driven through the public
/// `visible_as_tchars_and_tags[_extended]` entry points).
///
/// Every test compares the incremental path's output against
/// [`independent_oracle`] — a from-scratch, ungated full merge computed
/// independently of `merge_cache` — rather than trusting the incremental
/// path's own internal `debug_assert_eq!` cross-check alone, so a failure
/// here produces a clear test failure (not just a panic message) and the
/// check still exists even if these tests were ever run with
/// `debug_assertions` off.
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod incremental_merge_tests {
    use crate::buffer::Buffer;
    use crate::row::RowJoin;
    use freminal_common::buffer_states::{
        fonts::FontWeight, format_tag::FormatTag, tchar::TChar, url::Url,
    };
    use freminal_common::colors::TerminalColor;
    use proptest::prelude::*;
    use std::sync::Arc;

    fn text(s: &str) -> Vec<TChar> {
        s.chars().map(TChar::from).collect()
    }

    /// Build a `height`-row, `width`-column buffer with no scrollback (each
    /// row holds distinct, easily-recognizable plain text), so the visible
    /// window is the entire buffer and window bounds never shift across
    /// calls in these tests.
    fn build_plain_buffer(width: usize, height: usize) -> Buffer {
        let mut buf = Buffer::new(width, height);
        for i in 0..height {
            buf.insert_text(&text(&format!("row{i:02}")));
            if i + 1 < height {
                buf.handle_lf();
                buf.handle_cr();
            }
        }
        assert_eq!(
            buf.rows().len(),
            height,
            "test setup must have no scrollback"
        );
        buf
    }

    /// Independently recompute the full-merge oracle for the buffer's
    /// current visible window: Step 1 with redetection gating **disabled**
    /// (guaranteeing a trustworthy, fully-redetected `refined_auto_urls`),
    /// then [`Buffer::merge_row_caches_full`]. Mirrors exactly what
    /// `Buffer::debug_verify_against_oracle` does internally, but as an
    /// explicit, independent assertion in the test itself rather than
    /// relying solely on the (debug-build-only) internal cross-check.
    ///
    /// Must be called AFTER the real path (`visible_as_tchars_and_tags`) so
    /// every row is already clean — this only re-runs (ungated) redetection,
    /// never a row rebuild.
    fn independent_oracle(
        buf: &mut Buffer,
    ) -> (Vec<TChar>, Vec<FormatTag>, Vec<usize>, Vec<usize>) {
        let (visible_start, visible_end) = buf.visible_window_bounds(0, 0);
        buf.ensure_decompressed(visible_start..visible_end);
        let auto_detect = buf.auto_detect_urls;
        let (refined, _first_rebuilt_row) = Buffer::refresh_row_cache_and_refine_wrapped_urls(
            &mut buf.rows[visible_start..visible_end],
            &mut buf.row_cache[visible_start..visible_end],
            auto_detect,
            false,
        );
        Buffer::merge_row_caches_full(&buf.row_cache[visible_start..visible_end], &refined)
    }

    /// Minimal deterministic linear congruential generator (mirrors the one
    /// in `incremental_merge_equivalence_tests` — duplicated here rather
    /// than shared, so this module stays self-contained; see that module's
    /// copy for the constant-choice rationale).
    struct Lcg(u64);

    impl Lcg {
        const fn new(seed: u64) -> Self {
            Self(seed)
        }

        fn next_u64(&mut self) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            self.0
        }

        fn next_range(&mut self, bound: usize) -> usize {
            if bound == 0 {
                return 0;
            }
            let bound_u64 = u64::try_from(bound).unwrap_or(u64::MAX);
            let r = self.next_u64() % bound_u64;
            usize::try_from(r).unwrap_or(0)
        }
    }

    // ────────────────────────────────────────────────────────────────
    // Unit tests
    // ────────────────────────────────────────────────────────────────

    /// Sweep a single dirty row across every row index in the visible
    /// window (0..height), re-flattening after each and comparing against
    /// the independent oracle every time. Exercises the `boundary == 0`
    /// (full-merge fallback), interior-boundary (incremental fast path),
    /// and (trivially, since content never actually changes) the ordinary
    /// dirty-row rebuild path.
    #[test]
    fn single_dirty_row_swept_matches_full_merge_at_every_position() {
        let mut buf = build_plain_buffer(20, 6);
        let _ = buf.visible_as_tchars_and_tags(0); // warm merge_cache

        let (visible_start, visible_end) = buf.visible_window_bounds(0, 0);
        for row_idx in visible_start..visible_end {
            buf.rows[row_idx].mark_dirty();
            let actual = buf.visible_as_tchars_and_tags(0);
            let oracle = independent_oracle(&mut buf);
            assert_eq!(
                actual, oracle,
                "row {row_idx} dirty sweep diverged from the full-merge oracle"
            );
        }
    }

    /// Two non-contiguous dirty rows (3 and 19) in a realistically-sized
    /// (24-row) buffer: the incremental boundary must land at the smaller
    /// index (3), and the fast path's tail re-merge must still correctly
    /// reach and refresh row 19 even though row 19 has no rebuilt
    /// neighbors between it and the boundary.
    #[test]
    fn two_non_contiguous_dirty_rows_match_full_merge() {
        let mut buf = build_plain_buffer(20, 24);
        let _ = buf.visible_as_tchars_and_tags(0);

        buf.rows[3].mark_dirty();
        buf.rows[19].mark_dirty();

        let actual = buf.visible_as_tchars_and_tags(0);
        let oracle = independent_oracle(&mut buf);
        assert_eq!(
            actual, oracle,
            "two non-contiguous dirty rows (3, 19) diverged from the full-merge oracle"
        );
    }

    /// A 3-row soft-wrapped URL group where only the group's LAST row is
    /// marked dirty: the incremental boundary must retreat all the way to
    /// the group's first row (not just the dirty row) so the whole group
    /// gets redetected as one unit, reproducing the exact same
    /// wrap-boundary-safe URL text as the original (pre-dirty) merge.
    #[test]
    fn wrapped_url_group_dirty_last_row_redetects_whole_group() {
        let mut buf = Buffer::new(20, 4);
        assert!(buf.auto_detect_urls());

        // Row 0: unrelated content.
        buf.insert_text(&text("unrelated row"));
        buf.handle_lf();
        buf.handle_cr();

        // Rows 1-3: a URL that wraps across all three remaining rows.
        let url = "https://example.com/a/very/long/path/that/keeps/going";
        buf.insert_text(&text(url));

        assert_eq!(buf.rows().len(), 4, "test setup must have no scrollback");
        assert_eq!(buf.rows()[2].join, RowJoin::ContinueLogicalLine);
        assert_eq!(buf.rows()[3].join, RowJoin::ContinueLogicalLine);

        let expected = buf.visible_as_tchars_and_tags(0);
        let (_c, expected_tags, _ro, expected_urls) = &expected;
        assert!(!expected_urls.is_empty(), "URL must be detected initially");
        assert!(
            expected_urls
                .iter()
                .all(|&i| expected_tags[i].url.as_ref().is_some_and(|u| u.url == url)),
            "initial detection must carry the full, untruncated URL"
        );

        // Mark only row 3 (the group's LAST row, window-relative index 3)
        // dirty — the group's first two rows (1, 2) are untouched.
        buf.rows[3].mark_dirty();

        let actual = buf.visible_as_tchars_and_tags(0);
        let oracle = independent_oracle(&mut buf);
        assert_eq!(
            actual, oracle,
            "dirtying only the group's last row diverged from the full-merge oracle"
        );

        // The whole group must still report the full, untruncated URL —
        // not a truncated per-row fragment from a skipped redetection.
        let (_chars, tags, _row_offsets, url_indices) = actual;
        assert!(!url_indices.is_empty(), "URL must still be detected");
        assert!(
            url_indices
                .iter()
                .all(|&i| tags[i].url.as_ref().is_some_and(|u| u.url == url)),
            "redetection after a partial-group dirty must still carry the full URL"
        );
    }

    /// A static 3-row wrapped-URL group, flattened many times with no
    /// mutation in between, must produce byte-identical output every time
    /// — the no-op fast path must engage (returning the cached merge
    /// verbatim) rather than the (safe, but pointless) gating disabling
    /// itself and silently falling back to stale per-row URL fragments.
    #[test]
    fn static_wrapped_url_group_repeated_flatten_is_stable() {
        let mut buf = Buffer::new(20, 4);
        assert!(buf.auto_detect_urls());
        buf.insert_text(&text("unrelated row"));
        buf.handle_lf();
        buf.handle_cr();
        let url = "https://example.com/a/very/long/path/that/keeps/going";
        buf.insert_text(&text(url));

        let first = buf.visible_as_tchars_and_tags(0);
        for i in 0..10 {
            let again = buf.visible_as_tchars_and_tags(0);
            assert_eq!(
                first, again,
                "flatten #{i} diverged from the first, unmutated flatten"
            );
        }

        let (_chars, tags, _row_offsets, url_indices) = &first;
        assert!(!url_indices.is_empty());
        assert!(
            url_indices
                .iter()
                .all(|&i| tags[i].url.as_ref().is_some_and(|u| u.url == url)),
            "the full URL must still be reported after repeated no-mutation flattens"
        );
    }

    /// Seam coalescing, "matches" direction: the reused prefix's last tag
    /// (clamped at the incremental boundary) has the SAME visual format as
    /// the freshly-merged tail's first tag — they must coalesce into one
    /// continuous tag spanning the seam, exactly as a full merge would
    /// produce.
    #[test]
    fn seam_coalesces_when_tail_matches_prefix_format() {
        let mut buf = build_plain_buffer(10, 4);

        let mut red = FormatTag::default();
        red.colors.color = TerminalColor::Red;

        // Rows 1 and 2 both fully red.
        buf.current_tag = red.clone();
        buf.set_cursor_pos(Some(0), Some(1));
        buf.insert_text(&text("bbbbbbbbbb"));
        buf.set_cursor_pos(Some(0), Some(2));
        buf.insert_text(&text("cccccccccc"));
        buf.current_tag = FormatTag::default();

        let _ = buf.visible_as_tchars_and_tags(0); // warm merge_cache

        // Re-write row 2 with the SAME red text (dirties it without
        // changing content), forcing boundary = 2 with a red/red seam.
        buf.current_tag = red;
        buf.set_cursor_pos(Some(0), Some(2));
        buf.insert_text(&text("cccccccccc"));
        buf.current_tag = FormatTag::default();

        let actual = buf.visible_as_tchars_and_tags(0);
        let oracle = independent_oracle(&mut buf);
        assert_eq!(
            actual, oracle,
            "matching-format seam diverged from the oracle"
        );

        // The red run must be ONE continuous tag spanning row1+row2 (i.e.
        // strictly fewer tags than if the seam had NOT coalesced).
        let (_chars, tags, row_offsets, _url_indices) = &actual;
        let row1_start = row_offsets[1];
        let row2_end = row_offsets[3];
        let spanning_tag = tags
            .iter()
            .find(|t| t.start <= row1_start && t.end >= row2_end);
        assert!(
            spanning_tag.is_some_and(|t| t.colors.color == TerminalColor::Red),
            "expected one red tag spanning row1..row2, got {tags:?}"
        );
    }

    /// Seam coalescing, "differs" direction: the reused prefix's last tag
    /// (clamped at the incremental boundary) has a DIFFERENT visual format
    /// than the freshly-merged tail's first tag — they must NOT coalesce;
    /// the seam must show two distinct tags.
    #[test]
    fn seam_stays_separate_when_tail_differs_from_prefix_format() {
        let mut buf = build_plain_buffer(10, 4);

        let mut red = FormatTag::default();
        red.colors.color = TerminalColor::Red;
        let mut green = FormatTag::default();
        green.colors.color = TerminalColor::Green;

        buf.current_tag = red;
        buf.set_cursor_pos(Some(0), Some(1));
        buf.insert_text(&text("bbbbbbbbbb"));
        buf.set_cursor_pos(Some(0), Some(2));
        buf.insert_text(&text("cccccccccc"));
        buf.current_tag = FormatTag::default();

        let _ = buf.visible_as_tchars_and_tags(0); // warm merge_cache

        // Re-write row 2 with GREEN instead — dirties it AND changes its
        // format relative to row 1's (reused-prefix) trailing red tag.
        buf.current_tag = green;
        buf.set_cursor_pos(Some(0), Some(2));
        buf.insert_text(&text("cccccccccc"));
        buf.current_tag = FormatTag::default();

        let actual = buf.visible_as_tchars_and_tags(0);
        let oracle = independent_oracle(&mut buf);
        assert_eq!(
            actual, oracle,
            "differing-format seam diverged from the oracle"
        );

        let (_chars, tags, row_offsets, _url_indices) = &actual;
        let row1_start = row_offsets[1];
        let row2_start = row_offsets[2];
        let row2_end = row_offsets[3];
        let row1_tag = tags
            .iter()
            .find(|t| t.start <= row1_start && t.end > row1_start);
        let row2_tag = tags
            .iter()
            .find(|t| t.start >= row2_start && t.start < row2_end);
        assert_eq!(
            row1_tag.map(|t| t.colors.color),
            Some(TerminalColor::Red),
            "row1 must remain red: {tags:?}"
        );
        assert_eq!(
            row2_tag.map(|t| t.colors.color),
            Some(TerminalColor::Green),
            "row2 must be green (not coalesced with row1's red): {tags:?}"
        );
        assert_ne!(row1_tag.map(|t| t.end), None, "sanity: row1 tag must exist");
    }

    /// A URL tag (OSC-8 hyperlink) that soft-wraps across two rows, where
    /// the incremental boundary lands exactly at the row that the URL
    /// continues INTO: `build_reused_prefix` must clamp the straddling
    /// URL-carrying tag rather than dropping or duplicating it, and the
    /// re-merged tail must reconstruct `url_tag_indices` correctly (via
    /// re-coalescing with the clamped prefix tag).
    #[test]
    fn url_tag_indices_correct_when_url_tag_straddles_boundary() {
        let mut buf = Buffer::new(10, 4);

        // Row 0: unrelated.
        buf.insert_text(&text("row0text.."));
        buf.handle_lf();
        buf.handle_cr();

        // Rows 1-2: an OSC-8 hyperlink spanning the wrap boundary (15 chars
        // at width 10 -> row1 gets "AAAAABBBBB", row2 starts with "CCCCC"),
        // followed by plain trailing text finishing out row2.
        let link = Arc::new(Url {
            id: None,
            url: "https://osc8.example/target".to_string(),
        });
        buf.current_tag.url = Some(link.clone());
        buf.insert_text(&text("AAAAABBBBBCCCCC"));
        buf.current_tag = FormatTag::default();
        buf.insert_text(&text("DDDDD"));
        buf.handle_lf();
        buf.handle_cr();

        // Row 3: unrelated.
        buf.insert_text(&text("row3text.."));

        assert_eq!(buf.rows().len(), 4, "test setup must have no scrollback");
        assert_eq!(buf.rows()[2].join, RowJoin::ContinueLogicalLine);

        let expected = buf.visible_as_tchars_and_tags(0);
        let (_c, expected_tags, _ro, expected_urls) = &expected;
        assert!(
            !expected_urls.is_empty(),
            "hyperlink must be detected initially"
        );
        assert!(
            expected_urls
                .iter()
                .any(|&i| expected_tags[i].url.as_ref() == Some(&link)),
            "initial detection must carry the hyperlink"
        );

        // Dirty row 2 (re-write with the SAME content) so the incremental
        // boundary lands exactly where the hyperlink continues into row 2.
        buf.rows[2].mark_dirty();

        let actual = buf.visible_as_tchars_and_tags(0);
        let oracle = independent_oracle(&mut buf);
        assert_eq!(
            actual, oracle,
            "straddling URL tag diverged from the full-merge oracle"
        );

        let (_chars, tags, _row_offsets, url_indices) = &actual;
        assert!(
            !url_indices.is_empty(),
            "hyperlink must still be present after the incremental re-merge"
        );
        for &idx in url_indices {
            assert!(
                tags[idx].url.is_some(),
                "url_tag_indices[{idx}] must reference a url-carrying tag"
            );
        }
        assert!(
            url_indices
                .iter()
                .any(|&i| tags[i].url.as_ref() == Some(&link)),
            "the hyperlink must still be reachable via url_tag_indices"
        );
    }

    // ────────────────────────────────────────────────────────────────
    // Arc-returning variant (regression-fix follow-up): same behaviour as
    // the owned-`Vec` path, but proves the cache-population cost is a
    // refcount bump rather than a second deep clone.
    // ────────────────────────────────────────────────────────────────

    /// [`Buffer::visible_as_tchars_and_tags_extended_arc`] must produce
    /// content identical to [`Buffer::visible_as_tchars_and_tags`] on an
    /// otherwise-identical buffer (covers the fallback / first-populate
    /// path, since no `merge_cache` exists yet on a freshly built buffer).
    #[test]
    fn arc_variant_matches_owned_variant_on_fallback_path() {
        let mut owned_buf = build_plain_buffer(20, 6);
        let mut arc_buf = build_plain_buffer(20, 6);

        let (chars, tags, row_offsets, url_tag_indices) = owned_buf.visible_as_tchars_and_tags(0);
        let (achars, atags, arow_offsets, aurl_tag_indices) =
            arc_buf.visible_as_tchars_and_tags_extended_arc(0, 0);

        assert_eq!(
            chars, *achars,
            "chars must match between the two entry points"
        );
        assert_eq!(tags, *atags, "tags must match between the two entry points");
        assert_eq!(
            row_offsets, *arow_offsets,
            "row_offsets must match between the two entry points"
        );
        assert_eq!(
            url_tag_indices, *aurl_tag_indices,
            "url_tag_indices must match between the two entry points"
        );
    }

    /// Same equivalence check, but for the **incremental fast path**: warm
    /// the cache, dirty a single interior row, then compare.
    #[test]
    fn arc_variant_matches_owned_variant_on_incremental_fast_path() {
        let mut owned_buf = build_plain_buffer(20, 6);
        let mut arc_buf = build_plain_buffer(20, 6);

        let _ = owned_buf.visible_as_tchars_and_tags(0);
        let _ = arc_buf.visible_as_tchars_and_tags_extended_arc(0, 0);

        owned_buf.rows[3].mark_dirty();
        arc_buf.rows[3].mark_dirty();

        let (chars, tags, row_offsets, url_tag_indices) = owned_buf.visible_as_tchars_and_tags(0);
        let (achars, atags, arow_offsets, aurl_tag_indices) =
            arc_buf.visible_as_tchars_and_tags_extended_arc(0, 0);

        assert_eq!(chars, *achars);
        assert_eq!(tags, *atags);
        assert_eq!(row_offsets, *arow_offsets);
        assert_eq!(url_tag_indices, *aurl_tag_indices);
    }

    /// The regression-fix property itself: after the incremental fast path
    /// (or the fallback full merge) populates `merge_cache`, the `Arc`
    /// handed back to the caller and the `Arc` retained in `merge_cache`
    /// must point at the exact same heap allocation — proving population
    /// cost is a refcount bump (`Arc::clone`), never a `Vec` deep clone.
    #[test]
    fn arc_variant_shares_allocation_with_merge_cache_on_fallback_path() {
        let mut buf = build_plain_buffer(20, 6);

        let (chars, tags, row_offsets, url_tag_indices) =
            buf.visible_as_tchars_and_tags_extended_arc(0, 0);

        assert_eq!(
            Arc::strong_count(&chars),
            2,
            "exactly two owners must exist: the returned Arc and merge_cache's clone"
        );

        let cached = buf
            .merge_cache
            .as_ref()
            .expect("merge_cache must be populated after a flatten call");
        assert!(
            Arc::ptr_eq(&chars, &cached.chars),
            "merge_cache.chars must be the SAME allocation as the returned Arc"
        );
        assert!(Arc::ptr_eq(&tags, &cached.tags));
        assert!(Arc::ptr_eq(&row_offsets, &cached.row_offsets));
        assert!(Arc::ptr_eq(&url_tag_indices, &cached.url_tag_indices));
    }

    /// Same allocation-sharing property, but reached via the incremental
    /// fast path (rather than the fallback/full-merge path exercised
    /// above).
    #[test]
    fn arc_variant_shares_allocation_with_merge_cache_on_incremental_fast_path() {
        let mut buf = build_plain_buffer(20, 6);
        let _ = buf.visible_as_tchars_and_tags_extended_arc(0, 0);
        buf.rows[3].mark_dirty();

        let (chars, tags, row_offsets, url_tag_indices) =
            buf.visible_as_tchars_and_tags_extended_arc(0, 0);

        let cached = buf
            .merge_cache
            .as_ref()
            .expect("merge_cache must still be populated");
        assert!(Arc::ptr_eq(&chars, &cached.chars));
        assert!(Arc::ptr_eq(&tags, &cached.tags));
        assert!(Arc::ptr_eq(&row_offsets, &cached.row_offsets));
        assert!(Arc::ptr_eq(&url_tag_indices, &cached.url_tag_indices));
    }

    /// The no-op fast path (nothing dirtied since the last call) must hand
    /// back the exact same allocations across repeated calls — not just
    /// equal content, but the identical `Arc` — since nothing was rebuilt
    /// and the previous merge is reused verbatim.
    #[test]
    fn arc_variant_no_op_path_reuses_identical_allocation_across_calls() {
        let mut buf = build_plain_buffer(20, 6);

        let (chars1, tags1, row_offsets1, url_tag_indices1) =
            buf.visible_as_tchars_and_tags_extended_arc(0, 0);
        let (chars2, tags2, row_offsets2, url_tag_indices2) =
            buf.visible_as_tchars_and_tags_extended_arc(0, 0);

        assert!(
            Arc::ptr_eq(&chars1, &chars2),
            "no-op fast path must reuse the exact cached chars allocation"
        );
        assert!(Arc::ptr_eq(&tags1, &tags2));
        assert!(Arc::ptr_eq(&row_offsets1, &row_offsets2));
        assert!(Arc::ptr_eq(&url_tag_indices1, &url_tag_indices2));
    }

    /// [`Buffer::unwrap_or_clone`] must return the correct values whether it
    /// takes the zero-copy `try_unwrap` branch (sole owner) or the
    /// `unwrap_or_else` clone branch (shared).
    #[test]
    fn unwrap_or_clone_is_correct_when_sole_owner() {
        let arc = Arc::new(vec![1_usize, 2, 3]);
        assert_eq!(Buffer::unwrap_or_clone(arc), vec![1_usize, 2, 3]);
    }

    #[test]
    fn unwrap_or_clone_is_correct_when_shared() {
        let arc = Arc::new(vec![4_usize, 5, 6]);
        let sibling = Arc::clone(&arc);
        assert_eq!(Buffer::unwrap_or_clone(arc), vec![4_usize, 5, 6]);
        // The sibling reference must be untouched by the other side's
        // (forced-clone) extraction.
        assert_eq!(*sibling, vec![4_usize, 5, 6]);
    }

    // ────────────────────────────────────────────────────────────────
    // Confined scroll-region rotation regression tests
    //
    // These prove the fix named in `Buffer::merge_cache`'s field doc: the
    // explicit `self.merge_cache = None;` in `scroll_slice_up`,
    // `scroll_slice_down`, and `scroll_up` (scroll.rs) forces a full
    // re-merge after a confined in-place row rotation, instead of serving
    // a cached incremental merge whose prefix reuse can't observe that
    // already-clean row_cache entries moved to different indices.
    // ────────────────────────────────────────────────────────────────

    /// `scroll_slice_up(first, last)` rotates rows `[first, last]` up by
    /// one within an 8-row buffer taller than the rotated region (rows 0,
    /// 1, 6, 7 stay untouched), each row holding distinct content so the
    /// rotation is observable. After warming `merge_cache` and performing
    /// the rotation, the next incremental flatten must match a from-scratch
    /// oracle computed over the post-rotation state — this is exactly the
    /// scenario that reproducibly diverged before `scroll_slice_up` nulled
    /// `merge_cache`.
    #[test]
    fn incremental_merge_matches_oracle_after_confined_scroll_slice_up() {
        let mut buf = build_plain_buffer(20, 8);
        let _ = buf.visible_as_tchars_and_tags(0); // warm merge_cache

        buf.scroll_slice_up(2, 5);

        let actual = buf.visible_as_tchars_and_tags(0);
        let oracle = independent_oracle(&mut buf);
        assert_eq!(
            actual, oracle,
            "confined scroll_slice_up(2, 5) diverged from the full-merge oracle"
        );
    }

    /// Mirror of the above for `scroll_slice_down`, the downward confined
    /// rotation (blank row inserted at `first` instead of `last`).
    #[test]
    fn incremental_merge_matches_oracle_after_confined_scroll_slice_down() {
        let mut buf = build_plain_buffer(20, 8);
        let _ = buf.visible_as_tchars_and_tags(0); // warm merge_cache

        buf.scroll_slice_down(2, 5);

        let actual = buf.visible_as_tchars_and_tags(0);
        let oracle = independent_oracle(&mut buf);
        assert_eq!(
            actual, oracle,
            "confined scroll_slice_down(2, 5) diverged from the full-merge oracle"
        );
    }

    /// Whole-buffer `scroll_up` (used e.g. for autowrap-at-bottom-margin and
    /// primary-buffer LF at the live bottom) is the same rotation bug class
    /// as the confined `scroll_slice_up`/`_down` above — `rows.remove(0)` +
    /// `rows.push(new_row)` nets to the same `rows.len()`, silently
    /// shifting every clean cache entry down by one index. Verifies its own
    /// explicit `merge_cache = None` (named alongside the other two in
    /// `Buffer::merge_cache`'s field doc) is equally load-bearing.
    #[test]
    fn incremental_merge_matches_oracle_after_whole_buffer_scroll_up() {
        let mut buf = build_plain_buffer(20, 8);
        let _ = buf.visible_as_tchars_and_tags(0); // warm merge_cache

        buf.scroll_up();

        let actual = buf.visible_as_tchars_and_tags(0);
        let oracle = independent_oracle(&mut buf);
        assert_eq!(
            actual, oracle,
            "whole-buffer scroll_up diverged from the full-merge oracle"
        );
    }

    // ────────────────────────────────────────────────────────────────
    // Property test
    // ────────────────────────────────────────────────────────────────

    proptest! {
        /// Random buffer size + random per-cell mutation sequence: after
        /// EVERY mutation, the incremental public path
        /// (`visible_as_tchars_and_tags`) must match the independently
        /// recomputed full-merge oracle exactly.
        #[test]
        fn incremental_merge_matches_full_merge(
            width in 5usize..15,
            height in 3usize..8,
            actions in proptest::collection::vec(0u8..=255, 5..80),
        ) {
            let mut buf = Buffer::new(width, height);
            let width_u64 = u64::try_from(width).unwrap_or(0);
            let height_u64 = u64::try_from(height).unwrap_or(0);
            let mut rng = Lcg::new(width_u64 ^ (height_u64 << 32));

            for a in actions {
                let row = rng.next_range(height);
                let col = rng.next_range(width);

                match a % 4 {
                    0 => buf.current_tag = FormatTag::default(),
                    1 => {
                        let mut tag = FormatTag::default();
                        tag.colors.color = TerminalColor::Red;
                        tag.font_weight = FontWeight::Bold;
                        buf.current_tag = tag;
                    }
                    2 => {
                        let mut tag = FormatTag::default();
                        tag.colors.color = TerminalColor::Green;
                        buf.current_tag = tag;
                    }
                    _ => {}
                }

                buf.set_cursor_pos(Some(col), Some(row));
                let ch = char::from(b'a' + (a % 26));
                buf.insert_text(&[TChar::from(ch)]);

                let actual = buf.visible_as_tchars_and_tags(0);
                let oracle = independent_oracle(&mut buf);
                prop_assert_eq!(actual, oracle);
            }
        }
    }
}
