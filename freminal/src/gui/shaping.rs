// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Run segmentation and text shaping via `rustybuzz`.
//!
//! Splits visible terminal content into `TextRun` spans based on format changes
//! and font-face boundaries, then shapes each run to produce glyph IDs and advances.
//! Results are cached per-line for incremental updates.

/// A contiguous span of characters that share the same format and font face,
/// suitable for a single `rustybuzz::shape()` call.
///
/// This struct will be fleshed out in subtask 1.3.
pub struct TextRun {
    /// Byte range into the source text for this run.
    pub start: usize,
    /// One past the last byte of this run.
    pub end: usize,
}

/// The output of shaping a single `TextRun`.
///
/// Contains glyph IDs, advances, and cluster mapping produced by `rustybuzz`.
///
/// This struct will be fleshed out in subtask 1.3.
pub struct ShapedRun {
    /// Glyph IDs produced by the shaper.
    pub glyph_ids: Vec<u16>,
}

/// Validate that `rustybuzz` types are reachable.
/// This function exists solely to ensure the `rustybuzz` dependency is used and
/// not flagged as unused by cargo-machete. It will be replaced by real shaping
/// logic in subtask 1.3.
#[cfg(test)]
fn _rustybuzz_usage_check() {
    // UnicodeBuffer is the primary input type for rustybuzz::shape().
    let _buf = rustybuzz::UnicodeBuffer::new();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_run_fields() {
        let run = TextRun { start: 0, end: 10 };
        assert_eq!(run.start, 0);
        assert_eq!(run.end, 10);
    }

    #[test]
    fn shaped_run_fields() {
        let run = ShapedRun {
            glyph_ids: vec![1, 2, 3],
        };
        assert_eq!(run.glyph_ids.len(), 3);
    }
}
