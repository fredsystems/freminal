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
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

/// Global monotonic counter for generating unique image IDs.
static NEXT_IMAGE_ID: AtomicU64 = AtomicU64::new(1);

/// Generate a unique image ID.
///
/// IDs are monotonically increasing and never reused within a process.
pub fn next_image_id() -> u64 {
    NEXT_IMAGE_ID.fetch_add(1, Ordering::Relaxed)
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
}

impl ImageStore {
    /// Create an empty image store.
    #[must_use]
    pub fn new() -> Self {
        Self {
            images: HashMap::new(),
            number_to_id: HashMap::new(),
        }
    }

    /// Insert an image.  If an image with the same ID already exists, it is
    /// replaced.
    pub fn insert(&mut self, image: InlineImage) {
        self.images.insert(image.id, image);
    }

    /// Look up an image by ID.
    #[must_use]
    pub fn get(&self, id: u64) -> Option<&InlineImage> {
        self.images.get(&id)
    }

    /// Remove an image by ID.  Returns the removed image, if any.
    pub fn remove(&mut self, id: u64) -> Option<InlineImage> {
        let removed = self.images.remove(&id);
        if removed.is_some() {
            self.number_to_id.retain(|_, v| *v != id);
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

    /// Remove all images whose IDs are not referenced by any cell in the
    /// provided row iterator.
    ///
    /// This is called after scrollback eviction to garbage-collect images
    /// that are no longer visible or reachable.
    pub fn retain_referenced<'a, I>(&mut self, rows: I)
    where
        I: Iterator<Item = &'a [crate::cell::Cell]>,
    {
        if self.images.is_empty() {
            return;
        }

        let mut referenced: std::collections::HashSet<u64> =
            std::collections::HashSet::with_capacity(self.images.len());

        for cells in rows {
            for cell in cells {
                if let Some(placement) = cell.image_placement() {
                    referenced.insert(placement.image_id);
                }
            }
        }

        self.images.retain(|id, _| referenced.contains(id));
        self.number_to_id.retain(|_, v| self.images.contains_key(v));
    }

    /// Iterate over all images.
    pub fn iter(&self) -> impl Iterator<Item = (&u64, &InlineImage)> {
        self.images.iter()
    }

    /// Remove all stored images.
    pub fn clear(&mut self) {
        self.images.clear();
        self.number_to_id.clear();
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
            frames: Vec::new(),
            root_gap_ms: 0,
            animation: AnimationControl::default(),
        }
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
        };
        let p2 = ImagePlacement {
            image_id: 1,
            col_in_image: 0,
            row_in_image: 0,
            protocol: ImageProtocol::Sixel,
            image_number: None,
            placement_id: None,
            z_index: 0,
        };
        let p3 = ImagePlacement {
            image_id: 1,
            col_in_image: 1,
            row_in_image: 0,
            protocol: ImageProtocol::Sixel,
            image_number: None,
            placement_id: None,
            z_index: 0,
        };

        assert_eq!(p1, p2);
        assert_ne!(p1, p3);
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
}
