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
}

/// Central storage for all inline images in a buffer.
///
/// Images are inserted here when received from the PTY, and removed when
/// no cell references them any longer (or when scrollback eviction occurs).
#[derive(Debug, Clone, Default)]
pub struct ImageStore {
    images: HashMap<u64, InlineImage>,
}

impl ImageStore {
    /// Create an empty image store.
    #[must_use]
    pub fn new() -> Self {
        Self {
            images: HashMap::new(),
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
        self.images.remove(&id)
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
    }

    /// Iterate over all images.
    pub fn iter(&self) -> impl Iterator<Item = (&u64, &InlineImage)> {
        self.images.iter()
    }

    /// Remove all stored images.
    pub fn clear(&mut self) {
        self.images.clear();
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
        };
        let p2 = ImagePlacement {
            image_id: 1,
            col_in_image: 0,
            row_in_image: 0,
        };
        let p3 = ImagePlacement {
            image_id: 1,
            col_in_image: 1,
            row_in_image: 0,
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
}
