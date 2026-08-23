// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Golden-image storage and comparison (Task 123, subtask 123.12).
//!
//! One concept: comparing a captured [`PixelFrame`] against a stored
//! reference PNG, and regenerating that reference on demand.
//!
//! Goldens live in `freminal/tests/golden_pixels/` as ordinary PNGs, so a
//! human investigating a failure can open them. Regeneration follows the
//! `UPDATE_GOLDEN=1` convention the vttest suite already established
//! (`freminal-terminal-emulator/tests/vttest_common.rs`), including the
//! failure message that points at it.
//!
//! # Tolerance policy — decided before the first golden was captured
//!
//! **The tolerance is exact. Zero channel difference, zero mismatched
//! pixels.**
//!
//! `flaky-tests-are-bugs` forbids discovering a tolerance after a flaky
//! run, so this was decided up front and is justified by measurement taken
//! *before* any golden existed:
//!
//! 1. **llvmpipe is deterministic.** Three consecutive captures of a
//!    400x200 frame containing 23,843 non-black pixels of shaped, rasterised
//!    text produced **0 differing pixels at a channel bound of 0**. Not
//!    "small differences" — bit-identical output.
//! 2. **Every input is pinned.** The bundled `CaskaydiaCove` font, a fixed
//!    `pixels_per_point` of 1.0, a coordinate-derived synthetic grid, a
//!    fixed theme, and a pbuffer sized from those. No system font, no DPI
//!    scaling, no window manager, no compositor.
//! 3. **A tolerance absorbs known nondeterminism, and there is none to
//!    absorb.** Adopting one "just in case" would mean choosing a number
//!    with no evidence behind it — and a bound wide enough to be safe is
//!    usually wide enough to hide the subtle rendering regression the
//!    harness exists to catch. A one-channel-off bound would mask exactly
//!    the class of anti-aliasing and blending bug most likely to appear.
//!
//! ## When output legitimately changes, version the golden — do not widen the bound
//!
//! llvmpipe's rasterisation *can* shift between Mesa releases. The correct
//! response is to regenerate the golden and record the Mesa version that
//! produced it, **not** to loosen the comparison. A tolerance would
//! silently absorb both a Mesa change and a real regression, making them
//! indistinguishable; a recorded version makes them distinguishable. Each
//! golden therefore has a `.renderer` sidecar naming the `GL_RENDERER`
//! string it was captured under, and a mismatch is reported as context on
//! failure rather than as a failure in itself.
//!
//! Cross-machine and cross-Mesa-version equality is explicitly **not**
//! claimed. 123.13 pins the Mesa version through the flake, so CI compares
//! like with like.
//!
//! [`PixelFrame::differing_pixels`] takes a bound so that a future
//! maintainer *can* introduce one if evidence ever demands it — but the
//! evidence must be recorded here first.

use std::path::{Path, PathBuf};

use super::pixel_harness::PixelFrame;

/// Directory holding golden PNGs, relative to the crate root.
const GOLDEN_DIR: &str = "tests/golden_pixels";

/// The per-channel difference bound. See the module docs: this is zero
/// deliberately and should not be raised without recorded evidence.
pub const CHANNEL_BOUND: u8 = 0;

/// The number of pixels permitted to exceed [`CHANNEL_BOUND`]. Also zero,
/// for the same reason.
pub const MAX_DIFFERING_PIXELS: usize = 0;

/// Outcome of comparing a capture against its golden.
#[derive(Debug, PartialEq, Eq)]
pub enum GoldenComparison {
    /// The capture matched within policy.
    Match,
    /// No golden exists yet for this name.
    Missing {
        /// Where the golden was expected.
        path: PathBuf,
    },
    /// The capture differs from the golden.
    Mismatch {
        /// Where the golden lives.
        path: PathBuf,
        /// Pixels exceeding [`CHANNEL_BOUND`].
        differing: usize,
        /// Total pixels compared.
        total: usize,
    },
    /// The capture and golden have different dimensions.
    SizeMismatch {
        /// Golden dimensions, `(width, height)`.
        golden: (u32, u32),
        /// Capture dimensions, `(width, height)`.
        actual: (u32, u32),
    },
}

/// Path of the golden PNG for `name`.
#[must_use]
pub fn golden_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(GOLDEN_DIR)
        .join(format!("{name}.png"))
}

/// Path of the sidecar recording which `GL_RENDERER` produced the golden.
#[must_use]
pub fn renderer_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(GOLDEN_DIR)
        .join(format!("{name}.renderer"))
}

/// Whether the caller asked for goldens to be regenerated.
///
/// Mirrors the vttest suite's `UPDATE_GOLDEN=1` convention exactly, so a
/// contributor only has to learn it once.
#[must_use]
pub fn update_requested() -> bool {
    std::env::var("UPDATE_GOLDEN").as_deref() == Ok("1")
}

/// Write `frame` as the golden for `name`, plus its renderer sidecar.
///
/// # Errors
///
/// Returns the underlying I/O or encoding error.
pub fn write_golden(
    name: &str,
    frame: &PixelFrame,
    renderer: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let path = golden_path(name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let buffer: image::RgbaImage =
        image::ImageBuffer::from_raw(frame.width, frame.height, frame.rgba.clone())
            .ok_or("pixel buffer size does not match dimensions")?;
    buffer.save(&path)?;
    std::fs::write(renderer_path(name), format!("{renderer}\n"))?;
    Ok(())
}

/// Compare `frame` against the golden stored for `name`.
///
/// # Errors
///
/// Returns the underlying I/O or decoding error. A *missing* golden is not
/// an error — it is [`GoldenComparison::Missing`], so the caller can print
/// the `UPDATE_GOLDEN=1` hint rather than a decode failure.
pub fn compare(
    name: &str,
    frame: &PixelFrame,
) -> Result<GoldenComparison, Box<dyn std::error::Error>> {
    let path = golden_path(name);
    if !path.exists() {
        return Ok(GoldenComparison::Missing { path });
    }

    let golden = image::open(&path)?.to_rgba8();
    let (gw, gh) = (golden.width(), golden.height());
    if gw != frame.width || gh != frame.height {
        return Ok(GoldenComparison::SizeMismatch {
            golden: (gw, gh),
            actual: (frame.width, frame.height),
        });
    }

    let golden_frame = PixelFrame {
        width: gw,
        height: gh,
        rgba: golden.into_raw(),
    };

    let differing = frame
        .differing_pixels(&golden_frame, CHANNEL_BOUND)
        .unwrap_or(usize::MAX);

    // Written as `==` rather than `<=` only because clippy objects to a
    // comparison against the type minimum; `MAX_DIFFERING_PIXELS` is a
    // ceiling and the intent is "at most".
    if differing == MAX_DIFFERING_PIXELS {
        Ok(GoldenComparison::Match)
    } else {
        Ok(GoldenComparison::Mismatch {
            path,
            differing,
            total: (frame.width as usize).saturating_mul(frame.height as usize),
        })
    }
}

/// The `GL_RENDERER` a golden was captured under, if recorded.
#[must_use]
pub fn recorded_renderer(name: &str) -> Option<String> {
    std::fs::read_to_string(renderer_path(name))
        .ok()
        .map(|s| s.trim().to_owned())
}
