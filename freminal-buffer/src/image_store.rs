// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Storage types for inline images in the terminal buffer.
//!
//! Images are stored centrally in the buffer's `ImageStore` (a `HashMap<u64,
//! InlineImage>`), while individual cells reference their portion of an image
//! via `ImagePlacement`.  This keeps the per-cell overhead minimal — most cells
//! carry no image data, so the `Option<Box<ImagePlacement>>` on `Cell` is a
//! single null pointer (8 bytes).

use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

/// Global monotonic counter for generating unique image IDs.
static NEXT_IMAGE_ID: AtomicU64 = AtomicU64::new(1);

/// Global monotonic counter for generating unique placement INSTANCE ids.
///
/// Distinct from `NEXT_IMAGE_ID`: an image id identifies the pixel data
/// (`InlineImage`); a placement instance id identifies one on-screen
/// PLACEMENT of that image (Task 100.18). Two `a=p` puts of the same image
/// id with `p=0`/unspecified are separate, coexisting placements — each
/// must get its own instance id so the renderer buckets their cells into
/// two independent quads instead of merging them into one oversized quad.
static NEXT_PLACEMENT_INSTANCE_ID: AtomicU64 = AtomicU64::new(1);

/// Base-image storage budget per buffer, in bytes (kitty's reference value,
/// 320 MB). Still-image (root-frame) pixel data is charged against this pool.
pub const KITTY_IMAGE_BASE_QUOTA_BYTES: usize = 320 * 1024 * 1024;

/// Animation-frame storage budget per buffer, in bytes (kitty's reference
/// value, 5x the base pool). Animation frame pixel data (frames 2..N) is
/// charged against this separate pool.
pub const KITTY_IMAGE_ANIM_QUOTA_BYTES: usize = 5 * KITTY_IMAGE_BASE_QUOTA_BYTES;

/// Generate a unique image ID.
///
/// IDs are monotonically increasing and never reused within a process.
pub fn next_image_id() -> u64 {
    NEXT_IMAGE_ID.fetch_add(1, Ordering::Relaxed)
}

/// Generate a unique placement INSTANCE ID (Task 100.18).
///
/// IDs are monotonically increasing and never reused within a process.
/// Minted once per DISTINCT on-screen placement (not per image, and not per
/// cell) — every cell belonging to the same placement shares one instance
/// id, which the renderer uses to bucket cells into independent quads.
pub fn next_placement_instance_id() -> u64 {
    NEXT_PLACEMENT_INSTANCE_ID.fetch_add(1, Ordering::Relaxed)
}

/// One additional animation frame (frame 2..N) for an animated image.
///
/// Frame 1 (the root frame) is the base image's `InlineImage.pixels`; these
/// are the frames added by kitty `a=f` animation-frame commands. Frame pixels
/// are RGBA (4 bytes/pixel), the same width/height as the root image, behind
/// an `Arc` so snapshots clone by refcount, not deep copy.
///
/// This type carries NO wall-clock timing — only the per-frame gap in
/// milliseconds. Frame *selection* by elapsed time is a GUI-side concern
/// (`freminal`'s `ViewState`), never the buffer's.
#[derive(Debug, Clone)]
pub struct ImageFrame {
    /// Decoded pixel data for this frame (RGBA, 4 bytes per pixel).
    pub pixels: Arc<Vec<u8>>,
    /// Gap to the next frame, in milliseconds (kitty `z=`, root default handled
    /// separately). `0` means "use the protocol default".
    pub gap_ms: u32,
}

/// Animation run mode requested by `a=a s=` control commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AnimationRunMode {
    /// `s=1` — animation stopped (hold current frame).
    #[default]
    Stopped,
    /// `s=2` — run, but in "loading" mode: when the last available frame is
    /// reached, wait for more frames instead of looping.
    RunLoading,
    /// `s=3` — run normally (loop per `loop_count`).
    Running,
}

/// Declarative animation-playback state carried on an animated image.
///
/// Set by kitty `a=a` control commands on the PTY thread; shipped in the
/// snapshot; consumed by the GUI's wall-clock frame selector (100.2c). Holds
/// only *declared* parameters — NOT the wall-clock playback cursor (that is
/// GUI-side, ephemeral, and never in the buffer or snapshot).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnimationControl {
    /// Run/stop mode (`s=`).
    pub run_mode: AnimationRunMode,
    /// Loop count (`v=`): `0` ignored (keep prior), `1` = infinite (default),
    /// `N>=2` = play `N-1` loops then stop.
    pub loop_count: u32,
    /// App-forced current frame (`a=a c=`, 1-based). `0` = not forced; the GUI
    /// advances by wall-clock. When non-zero, the GUI shows this frame and (if
    /// running) resumes advancing from it.
    pub current_frame: u32,
}

impl Default for AnimationControl {
    fn default() -> Self {
        Self {
            run_mode: AnimationRunMode::Stopped,
            loop_count: 1,
            current_frame: 0,
        }
    }
}

/// How a displayed image's on-screen size is determined.
///
/// `NativePixels` (the default for kitty without `c`/`r`, iTerm2 `auto`, and
/// ALWAYS for sixel) means the image is drawn at its intrinsic
/// `width_px`x`height_px`, anchored at the top-left of its origin cell.
/// `ExplicitCells` (kitty `c`/`r`, iTerm2 explicit width/height) means the
/// image is scaled to fill its declared `display_cols`x`display_rows` grid.
/// Consumed by the renderer in Task 100.17b; set at handler-construction
/// time here in 100.17a while the protocol provenance is still known.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ImageSizeMode {
    #[default]
    NativePixels,
    ExplicitCells,
}

/// An inline image stored in the terminal buffer.
///
/// The pixel data is behind an `Arc` so that snapshots can reference it without
/// copying.  The image store owns the canonical `Arc`; snapshots hold clones.
#[derive(Debug, Clone)]
pub struct InlineImage {
    /// Unique image ID (auto-assigned or from Kitty protocol).
    pub id: u64,

    /// Decoded pixel data (RGBA, 4 bytes per pixel).
    pub pixels: Arc<Vec<u8>>,

    /// Image width in pixels.
    pub width_px: u32,

    /// Image height in pixels.
    pub height_px: u32,

    /// Display size in terminal columns.
    pub display_cols: usize,

    /// Display size in terminal rows.
    pub display_rows: usize,

    /// How `display_cols`x`display_rows` should be interpreted when drawing
    /// this image: at native pixel size, or scaled to fill the cell grid
    /// (Task 100.17). Set at construction time from protocol provenance;
    /// consumed by the renderer starting in Task 100.17b.
    pub size_mode: ImageSizeMode,

    /// Additional animation frames (frames 2..N). Empty for a still image.
    ///
    /// Frame 1 is the root frame in `pixels`; `frames[k]` is frame `k + 2`.
    /// A still (non-animated) image has an empty `frames` vec and behaves
    /// exactly as before this field existed.
    pub frames: Vec<ImageFrame>,

    /// Gap to the next frame for the ROOT frame (frame 1), in milliseconds
    /// (kitty root-frame gap, set via `a=a` control; default `0`).
    pub root_gap_ms: u32,

    /// Declarative animation-playback state (set by `a=a`). Default for a
    /// still image; only meaningful once `frames` is non-empty.
    pub animation: AnimationControl,
}

impl InlineImage {
    /// Total number of frames including the root frame (frame 1).
    /// A still image returns `1`.
    #[must_use]
    pub const fn frame_count(&self) -> usize {
        1 + self.frames.len()
    }

    /// Returns `true` if this image has more than one frame (is animated).
    #[must_use]
    pub const fn is_animated(&self) -> bool {
        !self.frames.is_empty()
    }

    /// Borrow a frame's pixel buffer by 1-based frame number (1 = root).
    ///
    /// Returns `None` if the frame does not exist.
    #[must_use]
    pub fn frame_pixels(&self, frame_1based: u32) -> Option<&Arc<Vec<u8>>> {
        match frame_1based {
            0 => None,
            1 => Some(&self.pixels),
            n => {
                let idx = usize::try_from(n - 2).ok()?;
                self.frames.get(idx).map(|f| &f.pixels)
            }
        }
    }

    /// Number of RGBA bytes one full frame occupies
    /// (`width_px * height_px * 4`).
    #[must_use]
    pub fn frame_byte_len(&self) -> usize {
        let w = usize::try_from(self.width_px).unwrap_or(0);
        let h = usize::try_from(self.height_px).unwrap_or(0);
        w.saturating_mul(h).saturating_mul(4)
    }

    /// Append a new frame (its frame number becomes `frame_count() + 1`
    /// as observed before this call).
    pub fn push_frame(&mut self, pixels: Arc<Vec<u8>>, gap_ms: u32) {
        self.frames.push(ImageFrame { pixels, gap_ms });
    }

    /// Replace an existing frame's pixel data by 1-based frame number
    /// (1 = root). Returns `false` if the frame does not exist.
    pub fn set_frame_pixels(&mut self, frame_1based: u32, pixels: Arc<Vec<u8>>) -> bool {
        match frame_1based {
            0 => false,
            1 => {
                self.pixels = pixels;
                true
            }
            n => {
                let Ok(idx) = usize::try_from(n - 2) else {
                    return false;
                };
                if let Some(frame) = self.frames.get_mut(idx) {
                    frame.pixels = pixels;
                    true
                } else {
                    false
                }
            }
        }
    }

    /// Set the gap (ms) for an existing frame by 1-based frame number
    /// (1 = root, which stores its gap in `root_gap_ms`). Returns `false`
    /// if the frame does not exist.
    pub fn set_frame_gap(&mut self, frame_1based: u32, gap_ms: u32) -> bool {
        match frame_1based {
            0 => false,
            1 => {
                self.root_gap_ms = gap_ms;
                true
            }
            n => {
                let Ok(idx) = usize::try_from(n - 2) else {
                    return false;
                };
                if let Some(frame) = self.frames.get_mut(idx) {
                    frame.gap_ms = gap_ms;
                    true
                } else {
                    false
                }
            }
        }
    }
}

/// Which image protocol placed this image.
///
/// Used to decide whether text writes should clear the image (Sixel/iTerm2)
/// or leave it alone (Kitty — cleared only via explicit `a=d` commands).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageProtocol {
    /// Sixel — images are cleared when text overwrites their cells.
    Sixel,
    /// Kitty graphics protocol — images persist until explicitly deleted.
    Kitty,
    /// iTerm2 inline image — images are cleared when text overwrites their cells.
    ITerm2,
}

/// A pixel-space source-crop rectangle for a kitty `a=p` placement.
///
/// The sub-region of the transmitted image this placement displays (`x=`/
/// `y=` = top-left px, `w=`/`h=` = size px). `None` = display the full image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceCrop {
    /// Left edge of the crop rectangle, in pixels.
    pub x: u32,
    /// Top edge of the crop rectangle, in pixels.
    pub y: u32,
    /// Width of the crop rectangle, in pixels.
    pub width: u32,
    /// Height of the crop rectangle, in pixels.
    pub height: u32,
}

/// A pixel-space sub-cell offset for a kitty `a=p`/`a=T` placement's `X=`/
/// `Y=` control keys.
///
/// Shifts the drawing origin within the placement's top-left cell by this
/// many PIXELS (must be `< cell_pixel_width`/`< cell_pixel_height` — the
/// resolving handler clamps this). Kitty-only; iTerm2 and Sixel have no
/// equivalent control key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubCellOffset {
    /// Horizontal pixel offset within the top-left cell (`X=`).
    pub x: u32,
    /// Vertical pixel offset within the top-left cell (`Y=`).
    pub y: u32,
}

/// A reference to a portion of an image within a single cell.
///
/// Each cell in the image's rectangular footprint carries one of these,
/// identifying which image it belongs to and which cell-sized tile within
/// that image it represents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImagePlacement {
    /// The image ID this placement refers to (key into `ImageStore`).
    pub image_id: u64,

    /// Column index of this cell within the image grid (0-indexed from the
    /// image's left edge).
    pub col_in_image: usize,

    /// Row index of this cell within the image grid (0-indexed from the
    /// image's top edge).
    pub row_in_image: usize,

    /// Which protocol placed this image.
    pub protocol: ImageProtocol,

    /// Kitty image number (`i=`), if any.
    pub image_number: Option<u32>,

    /// Kitty placement ID (`p=`), if any.
    pub placement_id: Option<u32>,

    /// Z-index for layering (Kitty `z=`).  Default 0.
    pub z_index: i32,

    /// Source-crop rectangle (kitty `a=p` `x/y/w/h`); `None` = full image.
    pub source_crop: Option<SourceCrop>,

    /// Unique identity of the on-screen PLACEMENT this cell belongs to
    /// (Task 100.18), minted via `next_placement_instance_id()`.
    ///
    /// Distinct from `image_id` (which identifies the pixel data, and may
    /// be shared by many coexisting placements) and from `placement_id`
    /// (the protocol-level `p=` value, which is often `0`/unspecified for
    /// multiple independent placements). The renderer buckets cells by
    /// this field, not `image_id`, so two placements of the same image
    /// never merge into one oversized quad.
    pub placement_instance: u64,

    /// Sub-cell pixel offset (kitty `a=p`/`a=T` `X=`/`Y=`, Task 100.19);
    /// `None` = no offset (draw at the cell's exact top-left).
    pub subcell_offset: Option<SubCellOffset>,
}

/// Central storage for all inline images in a buffer.
///
/// Images are inserted here when received from the PTY, and removed when
/// no cell references them any longer (or when scrollback eviction occurs).
#[derive(Debug, Clone, Default)]
pub struct ImageStore {
    images: HashMap<u64, InlineImage>,

    /// Maps a kitty image *number* (`I=`) to the id of the NEWEST image
    /// transmitted with that number. kitty resolves later by-number references
    /// (`a=p,I=`, `a=f,I=`, `d=n`) to the most-recent image with that number.
    number_to_id: HashMap<u32, u64>,

    /// Monotonic per-store insertion counter, stamped on each stored image for
    /// LRU age ordering. NOT the protocol image id (which a client may choose
    /// arbitrarily via `i=`), so this is the reliable age proxy.
    next_seq: u64,

    /// Insertion sequence per image id (age proxy for LRU eviction).
    seq: HashMap<u64, u64>,

    /// Ids known to be referenced by an on-screen placement, updated by
    /// `retain_referenced`. Eviction prefers images NOT in this set
    /// (placement-less first). May be slightly stale between refreshes —
    /// acceptable for a DoS-guard quota.
    placed: HashSet<u64>,

    /// Ids whose image DATA must survive scrollback trimming even with zero
    /// cell references. These are Kitty graphics protocol images: the spec
    /// keeps transmitted image data addressable by `id`/number (for a later
    /// `a=p` put, or after a lowercase placement-only delete) until an
    /// explicit uppercase free/delete or quota eviction. Sixel/iTerm2 images
    /// are NOT in this set — they have no id/number addressing or delete
    /// protocol, so they are correctly garbage-collected on scroll-out.
    ///
    /// Data lifetime is therefore controlled by explicit free/delete and the
    /// quota-LRU path (`enforce_quota_*`), NOT by scrollback trimming, for
    /// ids in this set.
    protocol_retained: HashSet<u64>,
}

impl ImageStore {
    /// Create an empty image store.
    #[must_use]
    pub fn new() -> Self {
        Self {
            images: HashMap::new(),
            number_to_id: HashMap::new(),
            next_seq: 0,
            seq: HashMap::new(),
            placed: HashSet::new(),
            protocol_retained: HashSet::new(),
        }
    }

    /// Insert an image.  If an image with the same ID already exists, it is
    /// replaced.
    ///
    /// This is the non-protocol-retained path: the image's data is eligible
    /// for garbage collection on scroll-out once no cell references it (see
    /// [`Self::retain_referenced`]). Use it for Sixel/iTerm2 images, which
    /// have no id/number addressing or delete protocol. Kitty images must
    /// use [`Self::insert_protocol_retained`] so their data survives
    /// scroll-out per the graphics protocol.
    ///
    /// After inserting, enforces the kitty graphics storage quota
    /// (`KITTY_IMAGE_BASE_QUOTA_BYTES` / `KITTY_IMAGE_ANIM_QUOTA_BYTES`),
    /// evicting the least-recently-inserted, placement-less image(s) as
    /// needed until the store is back under both caps (or only one image
    /// remains — a single over-quota image is never evicted to zero).
    pub fn insert(&mut self, image: InlineImage) {
        let id = image.id;
        self.insert_inner(image);
        // A non-retained (re)insert of an id clears any prior retention: the
        // latest transmit's provenance wins.
        self.protocol_retained.remove(&id);
    }

    /// Insert a Kitty graphics protocol image whose DATA must survive
    /// scrollback trimming even with zero cell references.
    ///
    /// The Kitty spec keeps transmitted image data addressable by `id`/number
    /// (for a later `a=p` put, or after a lowercase placement-only delete)
    /// until an explicit uppercase free/delete or quota eviction — it is NOT
    /// released merely because the placement scrolled off the end of
    /// scrollback. Data lifetime is instead controlled by explicit
    /// free/delete and the quota-LRU path. See [`Self::retain_referenced`].
    ///
    /// Enforces the storage quota after inserting, exactly like
    /// [`Self::insert`].
    pub fn insert_protocol_retained(&mut self, image: InlineImage) {
        let id = image.id;
        self.insert_inner(image);
        self.protocol_retained.insert(id);
    }

    /// Shared insert body: store the image, stamp its LRU sequence, and
    /// enforce the quota (protecting the just-inserted id).
    fn insert_inner(&mut self, image: InlineImage) {
        let id = image.id;
        self.images.insert(id, image);
        // Stamp/refresh the insertion sequence (a replace refreshes age — the
        // image was just (re)transmitted, so it is "newest").
        let seq = self.next_seq;
        self.next_seq = self.next_seq.wrapping_add(1);
        self.seq.insert(id, seq);
        // Protect the just-inserted image from immediate eviction: it is
        // still "unplaced" until the caller (`Buffer::place_image`) stamps
        // its cells, and the victim picker prefers unplaced images. Without
        // this, a store already full of placed images could evict `id`
        // before its cells exist, leaving them pointing at missing data.
        self.enforce_quota_protecting(Some(id));
    }

    /// Enforce the real kitty graphics storage quota constants, never
    /// evicting `protected_id` (the just-inserted image) while any other
    /// image is available as a victim.
    fn enforce_quota_protecting(&mut self, protected_id: Option<u64>) {
        self.enforce_quota_with_caps(
            KITTY_IMAGE_BASE_QUOTA_BYTES,
            KITTY_IMAGE_ANIM_QUOTA_BYTES,
            protected_id,
        );
    }

    /// Evict images until both the base (root-frame) and animation-frame
    /// byte totals are at or under the given caps, or only one image
    /// remains in the store.
    ///
    /// Eviction order: images NOT known to be currently placed (see
    /// `placed`) are preferred over placed ones; within that preference,
    /// the oldest insertion (`seq`) is evicted first. Because a newly
    /// inserted image always holds the highest `seq` in the store, it is
    /// never chosen as the victim unless it is the sole remaining image —
    /// which the "never evict to zero" floor below already prevents.
    fn enforce_quota_with_caps(
        &mut self,
        base_cap: usize,
        anim_cap: usize,
        protected_id: Option<u64>,
    ) {
        let mut base_bytes: usize = 0;
        let mut anim_bytes: usize = 0;
        for image in self.images.values() {
            base_bytes = base_bytes.saturating_add(image.pixels.len());
            for frame in &image.frames {
                anim_bytes = anim_bytes.saturating_add(frame.pixels.len());
            }
        }

        while (base_bytes > base_cap || anim_bytes > anim_cap) && self.images.len() > 1 {
            let Some(victim) = self.pick_eviction_victim(protected_id) else {
                break;
            };

            if let Some(removed) = self.images.remove(&victim) {
                base_bytes = base_bytes.saturating_sub(removed.pixels.len());
                for frame in &removed.frames {
                    anim_bytes = anim_bytes.saturating_sub(frame.pixels.len());
                }
            }
            self.seq.remove(&victim);
            self.placed.remove(&victim);
            self.protocol_retained.remove(&victim);
            self.number_to_id.retain(|_, v| *v != victim);
        }
    }

    /// Choose the eviction victim: prefer an id NOT in `placed`, then the
    /// oldest (lowest `seq`) among the preferred set. `protected_id` (the
    /// just-inserted image) is skipped while any other image is available.
    /// Returns `None` only if the store is empty or every remaining image is
    /// the protected one.
    fn pick_eviction_victim(&self, protected_id: Option<u64>) -> Option<u64> {
        self.images
            .keys()
            .copied()
            .filter(|id| protected_id != Some(*id))
            .min_by_key(|id| {
                let is_placed = self.placed.contains(id);
                let age = self.seq.get(id).copied().unwrap_or(0);
                (is_placed, age)
            })
    }

    /// Look up an image by ID.
    #[must_use]
    pub fn get(&self, id: u64) -> Option<&InlineImage> {
        self.images.get(&id)
    }

    /// Remove an image by ID.  Returns the removed image, if any.
    ///
    /// This is the explicit-free path (e.g. Kitty uppercase `d=I`/`d=N`
    /// delete): it releases the image data and clears its protocol-retention,
    /// which is the sanctioned way a retained Kitty image's data is freed.
    pub fn remove(&mut self, id: u64) -> Option<InlineImage> {
        let removed = self.images.remove(&id);
        if removed.is_some() {
            self.number_to_id.retain(|_, v| *v != id);
            self.seq.remove(&id);
            self.placed.remove(&id);
            self.protocol_retained.remove(&id);
        }
        removed
    }

    /// Record that image `id` is now the newest image with number `number`.
    /// Call this when an image is transmitted with an `I=` key.
    pub fn associate_number(&mut self, number: u32, id: u64) {
        self.number_to_id.insert(number, id);
    }

    /// Resolve a kitty image number (`I=`) to the id of the newest image with
    /// that number, if any is still stored.
    #[must_use]
    pub fn newest_id_for_number(&self, number: u32) -> Option<u64> {
        let id = *self.number_to_id.get(&number)?;
        // Only report it if the image is still present.
        if self.images.contains_key(&id) {
            Some(id)
        } else {
            None
        }
    }

    /// Returns `true` if the store contains an image with the given ID.
    #[must_use]
    pub fn contains(&self, id: u64) -> bool {
        self.images.contains_key(&id)
    }

    /// Number of images currently stored.
    #[must_use]
    pub fn len(&self) -> usize {
        self.images.len()
    }

    /// Returns `true` if no images are stored.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.images.is_empty()
    }

    /// Update placement knowledge after scrollback trimming, garbage-
    /// collecting only images whose lifetime is tied to their cells.
    ///
    /// Called after scrollback eviction. For each stored image:
    /// - **Protocol-retained (Kitty)** images are KEPT even with zero cell
    ///   references. The Kitty graphics protocol keeps transmitted image data
    ///   addressable by `id`/number (for a later `a=p` put, or after a
    ///   lowercase placement-only delete) until an explicit uppercase
    ///   free/delete ([`Self::remove`]) or quota eviction. Scrollback
    ///   trimming must not silently free it.
    /// - **Non-retained (Sixel/iTerm2)** images are GC'd once no cell
    ///   references them — they have no id/number addressing or delete
    ///   protocol, so a scrolled-off image is genuinely unreachable.
    ///
    /// The `placed` set is updated to reflect only truly-referenced images
    /// (independent of retention), so the quota-LRU picker still prefers
    /// evicting placement-less images first — matching the spec's "images
    /// without placements are evicted first" under quota pressure.
    pub fn retain_referenced<'a, I>(&mut self, rows: I)
    where
        I: Iterator<Item = &'a [crate::cell::Cell]>,
    {
        if self.images.is_empty() {
            return;
        }

        let mut referenced: HashSet<u64> = HashSet::with_capacity(self.images.len());

        for cells in rows {
            for cell in cells {
                if let Some(placement) = cell.image_placement() {
                    referenced.insert(placement.image_id);
                }
            }
        }

        // `placed` tracks genuine on-screen references only (drives quota
        // eviction preference), regardless of protocol retention.
        self.placed.clone_from(&referenced);

        // Keep an image if a cell references it OR its data is protocol-
        // retained (Kitty). Only cell-owned (Sixel/iTerm2) images without a
        // reference are dropped here.
        self.images
            .retain(|id, _| referenced.contains(id) || self.protocol_retained.contains(id));
        self.number_to_id.retain(|_, v| self.images.contains_key(v));
        self.seq.retain(|id, _| self.images.contains_key(id));
        self.placed.retain(|id| self.images.contains_key(id));
        self.protocol_retained
            .retain(|id| self.images.contains_key(id));
    }

    /// Iterate over all images.
    pub fn iter(&self) -> impl Iterator<Item = (&u64, &InlineImage)> {
        self.images.iter()
    }

    /// Remove all stored images.
    pub fn clear(&mut self) {
        self.images.clear();
        self.number_to_id.clear();
        self.seq.clear();
        self.placed.clear();
        self.protocol_retained.clear();
        self.next_seq = 0;
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn make_test_image(id: u64, cols: usize, rows: usize) -> InlineImage {
        let pixels = vec![0u8; cols * rows * 4];
        InlineImage {
            id,
            pixels: Arc::new(pixels),
            width_px: u32::try_from(cols * 8).unwrap(),
            height_px: u32::try_from(rows * 16).unwrap(),
            display_cols: cols,
            display_rows: rows,
            size_mode: ImageSizeMode::NativePixels,
            frames: Vec::new(),
            root_gap_ms: 0,
            animation: AnimationControl::default(),
        }
    }

    /// Build an image whose root-frame `pixels` buffer is exactly
    /// `byte_len` bytes, for precisely controlling the base-pool byte
    /// accounting exercised by quota/eviction tests.
    fn make_test_image_with_pixel_bytes(id: u64, byte_len: usize) -> InlineImage {
        let mut img = make_test_image(id, 1, 1);
        img.pixels = Arc::new(vec![0u8; byte_len]);
        img
    }

    /// Build an animated image with a root frame of `base_bytes` and a
    /// single additional frame of `frame_bytes`, for precisely controlling
    /// the base-pool vs. anim-pool byte accounting exercised by
    /// quota/eviction tests.
    fn make_test_image_with_frame(id: u64, base_bytes: usize, frame_bytes: usize) -> InlineImage {
        let mut img = make_test_image_with_pixel_bytes(id, base_bytes);
        img.frames.push(ImageFrame {
            pixels: Arc::new(vec![0u8; frame_bytes]),
            gap_ms: 0,
        });
        img
    }

    #[test]
    fn test_image_store_insert_and_get() {
        let mut store = ImageStore::new();
        assert!(store.is_empty());

        let img = make_test_image(1, 10, 5);
        store.insert(img);

        assert_eq!(store.len(), 1);
        assert!(!store.is_empty());
        assert!(store.contains(1));
        assert!(!store.contains(2));

        let retrieved = store.get(1).unwrap();
        assert_eq!(retrieved.id, 1);
        assert_eq!(retrieved.display_cols, 10);
        assert_eq!(retrieved.display_rows, 5);
    }

    #[test]
    fn test_image_store_remove() {
        let mut store = ImageStore::new();
        store.insert(make_test_image(1, 10, 5));
        store.insert(make_test_image(2, 20, 10));

        assert_eq!(store.len(), 2);

        let removed = store.remove(1).unwrap();
        assert_eq!(removed.id, 1);
        assert_eq!(store.len(), 1);
        assert!(!store.contains(1));
        assert!(store.contains(2));
    }

    #[test]
    fn test_image_store_replace() {
        let mut store = ImageStore::new();
        store.insert(make_test_image(1, 10, 5));

        // Insert again with same ID but different dimensions
        store.insert(make_test_image(1, 20, 10));
        assert_eq!(store.len(), 1);

        let img = store.get(1).unwrap();
        assert_eq!(img.display_cols, 20);
        assert_eq!(img.display_rows, 10);
    }

    #[test]
    fn test_image_store_clear() {
        let mut store = ImageStore::new();
        store.insert(make_test_image(1, 10, 5));
        store.insert(make_test_image(2, 20, 10));

        store.clear();
        assert!(store.is_empty());
        assert_eq!(store.len(), 0);
    }

    #[test]
    fn test_next_image_id_monotonic() {
        let id1 = next_image_id();
        let id2 = next_image_id();
        let id3 = next_image_id();

        assert!(id2 > id1);
        assert!(id3 > id2);
    }

    #[test]
    fn test_image_placement_equality() {
        let p1 = ImagePlacement {
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
        let p2 = ImagePlacement {
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
        let p3 = ImagePlacement {
            image_id: 1,
            col_in_image: 1,
            row_in_image: 0,
            protocol: ImageProtocol::Sixel,
            image_number: None,
            placement_id: None,
            z_index: 0,
            source_crop: None,
            placement_instance: 1,
            subcell_offset: None,
        };

        assert_eq!(p1, p2);
        assert_ne!(p1, p3);
    }

    #[test]
    fn image_placement_with_source_crop_round_trips_via_equality() {
        let cropped = SourceCrop {
            x: 25,
            y: 25,
            width: 50,
            height: 50,
        };
        let p1 = ImagePlacement {
            image_id: 1,
            col_in_image: 0,
            row_in_image: 0,
            protocol: ImageProtocol::Kitty,
            image_number: None,
            placement_id: None,
            z_index: 0,
            source_crop: Some(cropped),
            placement_instance: 1,
            subcell_offset: None,
        };
        let p2 = ImagePlacement {
            image_id: 1,
            col_in_image: 0,
            row_in_image: 0,
            protocol: ImageProtocol::Kitty,
            image_number: None,
            placement_id: None,
            z_index: 0,
            source_crop: Some(cropped),
            placement_instance: 1,
            subcell_offset: None,
        };
        let p3 = ImagePlacement {
            image_id: 1,
            col_in_image: 0,
            row_in_image: 0,
            protocol: ImageProtocol::Kitty,
            image_number: None,
            placement_id: None,
            z_index: 0,
            source_crop: None,
            placement_instance: 1,
            subcell_offset: None,
        };

        assert_eq!(p1, p2, "identical source_crop values must compare equal");
        assert_eq!(p1.source_crop, Some(cropped));
        assert_ne!(
            p1, p3,
            "Some(crop) vs None source_crop must not compare equal"
        );
    }

    #[test]
    fn test_inline_image_arc_sharing() {
        let img = make_test_image(1, 10, 5);
        let pixels_clone = Arc::clone(&img.pixels);

        // Both point to the same allocation
        assert!(Arc::ptr_eq(&img.pixels, &pixels_clone));
        assert_eq!(Arc::strong_count(&img.pixels), 2);
    }

    // -----------------------------------------------------------------------
    // retain_referenced — garbage-collects images not referenced by any cell
    // -----------------------------------------------------------------------

    #[test]
    fn retain_referenced_keeps_referenced_and_removes_unreferenced() {
        use crate::cell::Cell;
        use freminal_common::buffer_states::format_tag::FormatTag;

        let mut store = ImageStore::new();
        let id1 = next_image_id();
        let id2 = next_image_id();
        store.insert(make_test_image(id1, 2, 2));
        store.insert(make_test_image(id2, 2, 2));
        assert_eq!(store.len(), 2);

        // Build a row of cells: one references id1, the rest are plain text
        let placement_id1 = ImagePlacement {
            image_id: id1,
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
        let image_cell = Cell::image_cell(placement_id1, FormatTag::default());
        let plain_cell = Cell::blank_with_tag(FormatTag::default());
        let row_data: Vec<Cell> = vec![image_cell, plain_cell];

        // retain_referenced with rows that only reference id1
        let rows: Vec<&[Cell]> = vec![row_data.as_slice()];
        store.retain_referenced(rows.into_iter());

        // id1 is referenced → still present; id2 is unreferenced → removed
        assert!(
            store.contains(id1),
            "id1 should be retained (it is referenced)"
        );
        assert!(
            !store.contains(id2),
            "id2 should be removed (not referenced)"
        );
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn retain_referenced_keeps_protocol_retained_kitty_image_without_references() {
        // Kitty image data must survive scroll-out (retain_referenced with
        // no referencing cell) — it stays addressable by id/number for a
        // later a=p put or after a lowercase placement-only delete.
        let mut store = ImageStore::new();
        let kitty_id = next_image_id();
        store.insert_protocol_retained(make_test_image(kitty_id, 2, 2));

        // No rows reference it, but it is protocol-retained.
        store.retain_referenced(std::iter::empty());

        assert!(
            store.contains(kitty_id),
            "protocol-retained Kitty image must survive scroll-out"
        );
    }

    #[test]
    fn retain_referenced_still_gcs_non_retained_sixel_image_without_references() {
        // Sixel/iTerm2 images are cell-owned: with no referencing cell they
        // are genuinely unreachable and must be GC'd on scroll-out.
        let mut store = ImageStore::new();
        let sixel_id = next_image_id();
        store.insert(make_test_image(sixel_id, 2, 2));

        store.retain_referenced(std::iter::empty());

        assert!(
            !store.contains(sixel_id),
            "non-retained (Sixel/iTerm2) image must be GC'd on scroll-out"
        );
    }

    #[test]
    fn explicit_remove_frees_a_protocol_retained_image() {
        // The uppercase Kitty delete path (d=I/d=N) uses `remove`, which must
        // release the data and clear retention.
        let mut store = ImageStore::new();
        let kitty_id = next_image_id();
        store.insert_protocol_retained(make_test_image(kitty_id, 2, 2));
        assert!(store.contains(kitty_id));

        store.remove(kitty_id);

        assert!(!store.contains(kitty_id));
        // After free, a subsequent scroll-out pass keeps it gone (no dangling
        // retention entry resurrects it).
        store.retain_referenced(std::iter::empty());
        assert!(!store.contains(kitty_id));
    }

    #[test]
    fn quota_eviction_still_reclaims_unplaced_protocol_retained_image() {
        // Protocol retention protects against scroll-out GC, NOT against the
        // memory-bounding quota-LRU path: an unplaced Kitty image is still
        // evicted under quota pressure (spec: "images without placements are
        // evicted first").
        let mut store = ImageStore::new();
        store.insert_protocol_retained(make_test_image_with_pixel_bytes(1, 100));
        store.insert_protocol_retained(make_test_image_with_pixel_bytes(2, 100));

        // 200 bytes over a 150-byte cap: one unplaced retained image is
        // evicted, and its retention entry is cleared with it.
        store.enforce_quota_with_caps(150, usize::MAX, None);

        assert_eq!(store.len(), 1, "quota must still bound retained images");
    }

    #[test]
    fn non_retained_reinsert_of_retained_id_clears_retention() {
        // A later non-retained insert of the same id (provenance changed to a
        // cell-owned protocol) must make the id GC-eligible again.
        let mut store = ImageStore::new();
        let id = next_image_id();
        store.insert_protocol_retained(make_test_image(id, 2, 2));
        store.insert(make_test_image(id, 2, 2)); // now non-retained

        store.retain_referenced(std::iter::empty());

        assert!(
            !store.contains(id),
            "a non-retained reinsert must clear prior retention"
        );
    }

    #[test]
    fn retain_referenced_with_empty_store_is_noop() {
        use crate::cell::Cell;
        use freminal_common::buffer_states::format_tag::FormatTag;

        let mut store = ImageStore::new();
        let plain_cell = Cell::blank_with_tag(FormatTag::default());
        let row_data: Vec<Cell> = vec![plain_cell];
        let rows: Vec<&[Cell]> = vec![row_data.as_slice()];

        // Should not panic; store remains empty
        store.retain_referenced(rows.into_iter());
        assert!(store.is_empty());
    }

    #[test]
    fn retain_referenced_with_no_rows_removes_all() {
        let mut store = ImageStore::new();
        let id1 = next_image_id();
        store.insert(make_test_image(id1, 2, 2));
        assert_eq!(store.len(), 1);

        // No rows provided → no cells reference anything → all removed
        let rows: Vec<&[crate::cell::Cell]> = vec![];
        store.retain_referenced(rows.into_iter());

        assert!(
            store.is_empty(),
            "all images should be removed when no rows reference them"
        );
    }

    #[test]
    fn retain_referenced_all_images_referenced_keeps_all() {
        use crate::cell::Cell;
        use freminal_common::buffer_states::format_tag::FormatTag;

        let mut store = ImageStore::new();
        let id1 = next_image_id();
        let id2 = next_image_id();
        store.insert(make_test_image(id1, 2, 2));
        store.insert(make_test_image(id2, 2, 2));

        let cell1 = Cell::image_cell(
            ImagePlacement {
                image_id: id1,
                col_in_image: 0,
                row_in_image: 0,
                protocol: ImageProtocol::Sixel,
                image_number: None,
                placement_id: None,
                z_index: 0,
                source_crop: None,
                placement_instance: 1,
                subcell_offset: None,
            },
            FormatTag::default(),
        );
        let cell2 = Cell::image_cell(
            ImagePlacement {
                image_id: id2,
                col_in_image: 0,
                row_in_image: 0,
                protocol: ImageProtocol::Kitty,
                image_number: None,
                placement_id: None,
                z_index: 0,
                source_crop: None,
                placement_instance: 1,
                subcell_offset: None,
            },
            FormatTag::default(),
        );
        let row_data: Vec<Cell> = vec![cell1, cell2];
        let rows: Vec<&[Cell]> = vec![row_data.as_slice()];
        store.retain_referenced(rows.into_iter());

        assert_eq!(store.len(), 2, "both images should be retained");
        assert!(store.contains(id1));
        assert!(store.contains(id2));
    }

    // -----------------------------------------------------------------------
    // InlineImage animation frame model (Task 100.2a)
    // -----------------------------------------------------------------------

    #[test]
    fn inline_image_with_frames_reports_correct_frame_count_and_is_animated() {
        let mut img = make_test_image(1, 2, 2);
        img.frames.push(ImageFrame {
            pixels: Arc::new(vec![0u8; 16]),
            gap_ms: 100,
        });
        img.frames.push(ImageFrame {
            pixels: Arc::new(vec![0u8; 16]),
            gap_ms: 100,
        });

        assert_eq!(img.frame_count(), 3, "root frame + 2 additional frames");
        assert!(img.is_animated());
    }

    #[test]
    fn inline_image_still_image_has_frame_count_one_and_is_not_animated() {
        let img = make_test_image(1, 2, 2);

        assert_eq!(img.frame_count(), 1, "still image is just the root frame");
        assert!(!img.is_animated());
    }

    #[test]
    fn inline_image_clone_shares_frame_pixel_arcs() {
        let mut img = make_test_image(1, 2, 2);
        img.frames.push(ImageFrame {
            pixels: Arc::new(vec![1u8; 16]),
            gap_ms: 50,
        });

        let cloned = img.clone();

        assert!(
            Arc::ptr_eq(&img.frames[0].pixels, &cloned.frames[0].pixels),
            "cloning InlineImage must share frame pixel Arcs by refcount, not deep copy"
        );
        assert_eq!(Arc::strong_count(&img.frames[0].pixels), 2);
    }

    // -----------------------------------------------------------------------
    // InlineImage animation frame helpers (Task 100.2b)
    // -----------------------------------------------------------------------

    #[test]
    fn animation_control_default_is_stopped_with_infinite_loop() {
        let ctrl = AnimationControl::default();
        assert_eq!(ctrl.run_mode, AnimationRunMode::Stopped);
        assert_eq!(ctrl.loop_count, 1);
        assert_eq!(ctrl.current_frame, 0);
    }

    #[test]
    fn inline_image_default_animation_state_is_default() {
        let img = make_test_image(1, 2, 2);
        assert_eq!(img.animation, AnimationControl::default());
    }

    #[test]
    fn frame_pixels_root_and_additional_frames() {
        let mut img = make_test_image(1, 2, 2);
        img.frames.push(ImageFrame {
            pixels: Arc::new(vec![1u8; 16]),
            gap_ms: 40,
        });
        img.frames.push(ImageFrame {
            pixels: Arc::new(vec![2u8; 16]),
            gap_ms: 40,
        });

        assert!(
            Arc::ptr_eq(
                img.frame_pixels(1).expect("frame 1 (root) should exist"),
                &img.pixels
            ),
            "frame_pixels(1) should return the root pixels"
        );
        assert!(
            Arc::ptr_eq(
                img.frame_pixels(2).expect("frame 2 should exist"),
                &img.frames[0].pixels
            ),
            "frame_pixels(2) should return frames[0]"
        );
        assert!(
            Arc::ptr_eq(
                img.frame_pixels(3).expect("frame 3 should exist"),
                &img.frames[1].pixels
            ),
            "frame_pixels(3) should return frames[1]"
        );
        assert!(
            img.frame_pixels(99).is_none(),
            "frame_pixels(99) should be None for a nonexistent frame"
        );
        assert!(
            img.frame_pixels(0).is_none(),
            "frame_pixels(0) is not a valid 1-based frame number"
        );
    }

    #[test]
    fn frame_byte_len_computes_width_times_height_times_4() {
        let img = make_test_image(1, 4, 3); // width_px = 32, height_px = 48
        assert_eq!(img.frame_byte_len(), 32 * 48 * 4);
    }

    #[test]
    fn push_frame_appends_and_is_visible_via_frame_pixels() {
        let mut img = make_test_image(1, 2, 2);
        assert_eq!(img.frame_count(), 1);

        img.push_frame(Arc::new(vec![9u8; 16]), 40);

        assert_eq!(img.frame_count(), 2);
        assert!(img.is_animated());
        assert_eq!(
            img.frame_pixels(2).expect("frame 2 should exist").as_ref(),
            &vec![9u8; 16]
        );
        assert_eq!(img.frames[0].gap_ms, 40);
    }

    #[test]
    fn set_frame_pixels_edits_root_and_additional_frames() {
        let mut img = make_test_image(1, 2, 2);
        img.push_frame(Arc::new(vec![0u8; 16]), 40);

        assert!(img.set_frame_pixels(1, Arc::new(vec![7u8; 16])));
        assert_eq!(img.pixels.as_ref(), &vec![7u8; 16]);

        assert!(img.set_frame_pixels(2, Arc::new(vec![8u8; 16])));
        assert_eq!(img.frames[0].pixels.as_ref(), &vec![8u8; 16]);

        assert!(
            !img.set_frame_pixels(99, Arc::new(vec![0u8; 16])),
            "editing a nonexistent frame should return false"
        );
    }

    #[test]
    fn set_frame_gap_updates_root_gap_ms_and_frame_gap_ms() {
        let mut img = make_test_image(1, 2, 2);
        img.push_frame(Arc::new(vec![0u8; 16]), 0);

        assert!(img.set_frame_gap(1, 50));
        assert_eq!(img.root_gap_ms, 50);

        assert!(img.set_frame_gap(2, 75));
        assert_eq!(img.frames[0].gap_ms, 75);

        assert!(
            !img.set_frame_gap(99, 10),
            "setting gap on a nonexistent frame should return false"
        );
    }

    // -----------------------------------------------------------------------
    // Kitty image number (`I=`) index (Task 100.3)
    // -----------------------------------------------------------------------

    #[test]
    fn associate_number_then_resolve_returns_the_id() {
        let mut store = ImageStore::new();
        let id = next_image_id();
        store.insert(make_test_image(id, 2, 2));

        store.associate_number(13, id);

        assert_eq!(store.newest_id_for_number(13), Some(id));
    }

    #[test]
    fn newest_id_for_number_returns_none_for_unknown_number() {
        let store = ImageStore::new();
        assert_eq!(store.newest_id_for_number(999), None);
    }

    #[test]
    fn newest_id_for_number_returns_none_after_image_removed() {
        let mut store = ImageStore::new();
        let id = next_image_id();
        store.insert(make_test_image(id, 2, 2));
        store.associate_number(13, id);
        assert_eq!(store.newest_id_for_number(13), Some(id));

        store.remove(id);

        assert_eq!(
            store.newest_id_for_number(13),
            None,
            "resolving a number whose image was removed should return None"
        );
    }

    #[test]
    fn associate_number_with_newer_image_overrides_older_id() {
        let mut store = ImageStore::new();
        let id1 = next_image_id();
        let id2 = next_image_id();
        store.insert(make_test_image(id1, 2, 2));
        store.insert(make_test_image(id2, 2, 2));

        store.associate_number(5, id1);
        assert_eq!(store.newest_id_for_number(5), Some(id1));

        // A second transmit with the same number always creates a new image
        // and becomes the "newest" — the index must follow it.
        store.associate_number(5, id2);
        assert_eq!(store.newest_id_for_number(5), Some(id2));
    }

    #[test]
    fn clear_drops_the_number_index() {
        let mut store = ImageStore::new();
        let id = next_image_id();
        store.insert(make_test_image(id, 2, 2));
        store.associate_number(7, id);
        assert_eq!(store.newest_id_for_number(7), Some(id));

        store.clear();

        assert_eq!(store.newest_id_for_number(7), None);
    }

    #[test]
    fn retain_referenced_drops_number_index_entries_for_removed_images() {
        use crate::cell::Cell;
        use freminal_common::buffer_states::format_tag::FormatTag;

        let mut store = ImageStore::new();
        let id1 = next_image_id();
        let id2 = next_image_id();
        store.insert(make_test_image(id1, 2, 2));
        store.insert(make_test_image(id2, 2, 2));
        store.associate_number(1, id1);
        store.associate_number(2, id2);

        // Only id1 is referenced by a cell; id2 gets garbage-collected.
        let placement = ImagePlacement {
            image_id: id1,
            col_in_image: 0,
            row_in_image: 0,
            protocol: ImageProtocol::Kitty,
            image_number: Some(1),
            placement_id: None,
            z_index: 0,
            source_crop: None,
            placement_instance: 1,
            subcell_offset: None,
        };
        let row_data: Vec<Cell> = vec![Cell::image_cell(placement, FormatTag::default())];
        let rows: Vec<&[Cell]> = vec![row_data.as_slice()];
        store.retain_referenced(rows.into_iter());

        assert_eq!(store.newest_id_for_number(1), Some(id1));
        assert_eq!(
            store.newest_id_for_number(2),
            None,
            "number index entry for a GC'd image should be dropped"
        );
    }

    // -----------------------------------------------------------------------
    // Storage quotas + LRU eviction (Task 100.5)
    // -----------------------------------------------------------------------

    #[test]
    fn enforce_quota_evicts_oldest_first_when_all_unplaced() {
        let mut store = ImageStore::new();
        store.insert(make_test_image_with_pixel_bytes(1, 100)); // seq 0 (oldest)
        store.insert(make_test_image_with_pixel_bytes(2, 100)); // seq 1
        store.insert(make_test_image_with_pixel_bytes(3, 100)); // seq 2 (newest)
        assert_eq!(store.len(), 3);

        // Cap only allows one image's worth of base bytes; must evict the
        // two oldest first, in age order, and stop at the floor of 1 image.
        store.enforce_quota_with_caps(150, usize::MAX, None);

        assert_eq!(store.len(), 1, "should evict down to the floor of 1 image");
        assert!(
            store.contains(3),
            "the newest image (id 3) should survive; older images are evicted first"
        );
        assert!(!store.contains(1));
        assert!(!store.contains(2));
    }

    #[test]
    fn enforce_quota_prefers_evicting_unplaced_over_placed_even_if_older() {
        use crate::cell::Cell;
        use freminal_common::buffer_states::format_tag::FormatTag;

        let mut store = ImageStore::new();
        let id_placed = 1;
        let id_unplaced = 2;

        // id_placed is inserted first (older, seq 0)...
        store.insert(make_test_image_with_pixel_bytes(id_placed, 100));

        // ...and immediately confirmed on-screen via retain_referenced, which
        // marks it `placed`.
        let placement = ImagePlacement {
            image_id: id_placed,
            col_in_image: 0,
            row_in_image: 0,
            protocol: ImageProtocol::Kitty,
            image_number: None,
            placement_id: None,
            z_index: 0,
            source_crop: None,
            placement_instance: 1,
            subcell_offset: None,
        };
        let row_data: Vec<Cell> = vec![Cell::image_cell(placement, FormatTag::default())];
        let rows: Vec<&[Cell]> = vec![row_data.as_slice()];
        store.retain_referenced(rows.into_iter());
        assert!(store.contains(id_placed));

        // id_unplaced is inserted afterward (newer, seq 1) but has not yet
        // been through a retain_referenced pass, so it is NOT in `placed`.
        store.insert(make_test_image_with_pixel_bytes(id_unplaced, 100));

        // Force eviction: total base bytes (200) exceeds the cap (150).
        store.enforce_quota_with_caps(150, usize::MAX, None);

        assert_eq!(store.len(), 1);
        assert!(
            store.contains(id_placed),
            "the placed image should be kept even though it is older"
        );
        assert!(
            !store.contains(id_unplaced),
            "the unplaced (newer) image should be evicted first"
        );
    }

    #[test]
    fn enforce_quota_never_evicts_the_last_remaining_image() {
        let mut store = ImageStore::new();
        store.insert(make_test_image_with_pixel_bytes(1, 100_000));

        // Wildly over both caps, but only one image exists — must be kept.
        store.enforce_quota_with_caps(10, 10, None);

        assert_eq!(
            store.len(),
            1,
            "a single over-quota image must never be evicted to zero"
        );
        assert!(store.contains(1));
    }

    #[test]
    fn insert_protects_just_inserted_image_from_immediate_eviction() {
        use crate::cell::Cell;
        use freminal_common::buffer_states::format_tag::FormatTag;

        // A tiny quota so any second image forces eviction.
        // We cannot pass caps into `insert`, so use images sized to exceed
        // the real base cap only in aggregate is impractical here; instead we
        // exercise the protection directly via `enforce_quota_with_caps`.
        let mut store = ImageStore::new();

        // id_placed inserted first and marked placed.
        let id_placed = 1;
        store.insert(make_test_image_with_pixel_bytes(id_placed, 100));
        let placement = ImagePlacement {
            image_id: id_placed,
            col_in_image: 0,
            row_in_image: 0,
            protocol: ImageProtocol::Kitty,
            image_number: None,
            placement_id: None,
            z_index: 0,
            source_crop: None,
            placement_instance: 1,
            subcell_offset: None,
        };
        let row_data: Vec<Cell> = vec![Cell::image_cell(placement, FormatTag::default())];
        let rows: Vec<&[Cell]> = vec![row_data.as_slice()];
        store.retain_referenced(rows.into_iter());
        assert!(store.contains(id_placed));

        // id_new inserted second (unplaced, newest). Both images total 200
        // bytes, over a 150-byte cap. Without protection the victim picker
        // prefers the unplaced newest image (id_new) and would evict it,
        // leaving cells with no data. With `protected_id = Some(id_new)` the
        // placed older image is evicted instead.
        let id_new = 2;
        store.insert(make_test_image_with_pixel_bytes(id_new, 100));
        store.enforce_quota_with_caps(150, usize::MAX, Some(id_new));

        assert_eq!(store.len(), 1);
        assert!(
            store.contains(id_new),
            "the just-inserted (protected) image must survive eviction"
        );
        assert!(
            !store.contains(id_placed),
            "the older placed image should be evicted to protect the new one"
        );
    }

    #[test]
    fn enforce_quota_anim_pool_triggers_eviction_independent_of_base_pool() {
        let mut store = ImageStore::new();
        // Both images have tiny base (root-frame) pixels but large animation
        // frames, so only the anim pool cap can trigger eviction here.
        store.insert(make_test_image_with_frame(1, 10, 1000)); // seq 0 (oldest)
        store.insert(make_test_image_with_frame(2, 10, 1000)); // seq 1 (newest)

        // Base cap is effectively unlimited; anim cap (1500) is exceeded by
        // the combined frame bytes (2000).
        store.enforce_quota_with_caps(usize::MAX, 1500, None);

        assert_eq!(
            store.len(),
            1,
            "anim pool overage alone should force eviction"
        );
        assert!(
            store.contains(2),
            "the oldest image (id 1) should be evicted first"
        );
    }

    #[test]
    fn retain_referenced_updates_placed_set_and_prunes_seq_and_placed_for_removed_images() {
        use crate::cell::Cell;
        use freminal_common::buffer_states::format_tag::FormatTag;

        let mut store = ImageStore::new();
        let id1 = next_image_id();
        let id2 = next_image_id();
        store.insert(make_test_image(id1, 2, 2));
        store.insert(make_test_image(id2, 2, 2));

        // Only id1 is referenced by a cell; id2 is unreferenced and GC'd.
        let placement = ImagePlacement {
            image_id: id1,
            col_in_image: 0,
            row_in_image: 0,
            protocol: ImageProtocol::Kitty,
            image_number: None,
            placement_id: None,
            z_index: 0,
            source_crop: None,
            placement_instance: 1,
            subcell_offset: None,
        };
        let row_data: Vec<Cell> = vec![Cell::image_cell(placement, FormatTag::default())];
        let rows: Vec<&[Cell]> = vec![row_data.as_slice()];
        store.retain_referenced(rows.into_iter());

        assert!(
            store.placed.contains(&id1),
            "id1 is referenced on-screen, so it should be marked placed"
        );
        assert!(
            !store.placed.contains(&id2),
            "id2 was GC'd, so it must not remain in the placed set"
        );
        assert!(
            store.seq.contains_key(&id1),
            "seq entry for a surviving image should remain"
        );
        assert!(
            !store.seq.contains_key(&id2),
            "seq entry for a GC'd image should be pruned"
        );
    }

    #[test]
    fn remove_and_clear_keep_seq_and_placed_and_next_seq_in_sync() {
        let mut store = ImageStore::new();
        store.insert(make_test_image(1, 2, 2));
        store.insert(make_test_image(2, 2, 2));
        assert!(store.seq.contains_key(&1));
        assert!(store.seq.contains_key(&2));

        store.remove(1);
        assert!(
            !store.seq.contains_key(&1),
            "remove() should drop the seq entry for the removed image"
        );
        assert!(
            !store.placed.contains(&1),
            "remove() should drop the placed entry for the removed image"
        );
        assert!(store.seq.contains_key(&2), "unrelated entries must survive");

        store.clear();
        assert!(store.seq.is_empty(), "clear() should empty the seq map");
        assert!(
            store.placed.is_empty(),
            "clear() should empty the placed set"
        );
        assert_eq!(
            store.next_seq, 0,
            "clear() should reset the insertion sequence counter"
        );
    }

    #[test]
    fn normal_insert_under_real_quota_does_not_evict_anything() {
        let mut store = ImageStore::new();
        for i in 0..10u64 {
            // 1 KB each, 10 KB total — far under the real 320 MB quota.
            store.insert(make_test_image_with_pixel_bytes(i, 1024));
        }

        assert_eq!(
            store.len(),
            10,
            "inserts well under the real quota must not trigger eviction"
        );
    }
}
