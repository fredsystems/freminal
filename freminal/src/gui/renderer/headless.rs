// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Headless GL driver for the recording harness (Task 123, 123.7).
//!
//! This module drives [`TerminalRenderer`] directly, **below** the GUI event
//! layer. It does not reproduce `App::update`, event triage, damage
//! decisions (`gui/frame_damage.rs`), or pointer handling
//! (`gui/pointer_motion.rs`). Its numbers are therefore **per-frame GL cost
//! given that a frame is drawn** — they say nothing about *how often* a
//! frame is drawn, which for event-driven workloads is the dominant term.
//!
//! Pointer motion is the case where this distinction matters most: the
//! per-event CPU work measured here is real and worth fixing, but what makes
//! pointer motion a worst case in practice is the **rate** at which
//! compositors deliver events — macOS delivers motion events even to
//! unfocused windows, and Wayland is comparably chatty. This harness
//! supplies per-event cost; it cannot supply the delivery rate. The two must
//! always be reported as a pair and never fused into a single figure.
//!
//! 123.7's original plan text describes the driver as constructing
//! `RenderState`. It cannot: `RenderState`'s fields are `pub(super)` within
//! `gui::terminal`, unreachable from here, and it exposes no accessor for
//! its `TerminalRenderer`/`GlyphAtlas`. This module constructs
//! [`TerminalRenderer`] and [`GlyphAtlas`] directly instead — the same thing
//! `freminal/benches/render_loop_bench.rs` already does headlessly.

use std::collections::HashMap;

use conv2::ConvUtil;

use super::gl_facade::Gl;
use super::gl_facade::recording::GlCall;
use super::gpu::TerminalRenderer;
use super::toast_pass::{ToastQuad, ToastRenderer};
use super::toast_text_pass::{ToastTextRenderer, ToastTextRun};
use super::vertex::{
    BackgroundFrame, FgRenderOptions, build_background_instances, build_cursor_verts_only,
    build_foreground_instances,
};
use crate::gui::atlas::GlyphAtlas;
use crate::gui::font_manager::FontManager;
use crate::gui::shaping::ShapingCache;
use freminal_common::buffer_states::format_tag::FormatTag;
use freminal_common::buffer_states::tchar::TChar;
use freminal_common::config::BackgroundImageMode;
use freminal_common::cursor::CursorVisualStyle;
use freminal_common::themes::CATPPUCCIN_MOCHA;

/// Deterministic cell content for a synthetic grid, plus a single
/// full-span format tag.
///
/// The content is generated from the cell coordinates, never randomly, so
/// that a given [`SyntheticFrame`] produces a byte-identical call log on
/// every run. That reproducibility is what makes 123.8's exact-count
/// assertions possible at all — a randomised grid would force the
/// assertions down to ranges, and a range wide enough to be stable is
/// usually wide enough to miss the regression it was meant to catch.
fn synthetic_grid(cols: usize, rows: usize) -> (Vec<TChar>, Vec<FormatTag>) {
    let mut chars = Vec::with_capacity(rows * (cols + 1));
    for row in 0..rows {
        for col in 0..cols {
            let offset = u8::try_from((col + row) % 26).unwrap_or(0);
            chars.push(TChar::Ascii(b'a' + offset));
        }
        chars.push(TChar::NewLine);
    }
    let tags = vec![FormatTag {
        start: 0,
        end: chars.len(),
        ..FormatTag::default()
    }];
    (chars, tags)
}

/// Whether the synthetic frame includes a visible cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorPresence {
    /// The cursor is drawn.
    Shown,
    /// The cursor is not drawn.
    Hidden,
}

/// Whether the synthetic frame includes a toast overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastPresence {
    /// A toast is present and drawn on top of the terminal content.
    Present,
    /// No toast is drawn.
    Absent,
}

/// A description of one synthetic frame to drive through the headless
/// renderer.
///
/// Carries only the inputs that affect what gets drawn; it holds no cell
/// content itself (that is constructed by part B).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyntheticFrame {
    /// Number of terminal columns to render.
    pub cols: usize,
    /// Number of terminal rows to render.
    pub rows: usize,
    /// Whether this frame draws a cursor.
    pub cursor: CursorPresence,
    /// Whether this frame draws a toast overlay.
    pub toast: ToastPresence,
}

impl SyntheticFrame {
    /// Construct a frame of the given size with a visible cursor and no
    /// toast.
    #[must_use]
    pub const fn new(cols: usize, rows: usize) -> Self {
        Self {
            cols,
            rows,
            cursor: CursorPresence::Shown,
            toast: ToastPresence::Absent,
        }
    }

    /// Override the cursor presence.
    #[must_use]
    pub const fn with_cursor(self, cursor: CursorPresence) -> Self {
        Self { cursor, ..self }
    }

    /// Override the toast presence.
    #[must_use]
    pub const fn with_toast(self, toast: ToastPresence) -> Self {
        Self { toast, ..self }
    }
}

/// Errors from constructing or initializing a [`HeadlessRenderer`].
#[derive(Debug, thiserror::Error)]
pub enum HeadlessDriverError {
    /// Font manager construction failed.
    #[error("font manager init failed: {0}")]
    FontManager(#[from] crate::gui::font_manager::FontManagerError),
    /// Renderer GL initialization failed.
    #[error("renderer GL init failed: {0}")]
    GpuInit(#[from] super::errors::GpuInitError),
}

/// Drives [`TerminalRenderer`] (plus the toast passes) without a GUI event
/// loop, for use with the GL recording facade (Task 123).
///
/// See the module documentation for what this driver does and does not
/// represent.
pub struct HeadlessRenderer {
    renderer: TerminalRenderer,
    atlas: GlyphAtlas,
    font_manager: FontManager,
    shaping: ShapingCache,
    toast_pill: ToastRenderer,
    toast_text: ToastTextRenderer,
    /// Length (in `f32`s) of the last background instance buffer built, fed
    /// to `build_cursor_verts_only`'s pre-computed-length parameters by part
    /// B so the cursor-only fast path can be exercised without rebuilding
    /// the full frame.
    last_bg_floats: usize,
    /// Length (in `f32`s) of the last foreground instance buffer built; see
    /// `last_bg_floats`.
    last_fg_floats: usize,
    /// Length (in `f32`s) of the last image vertex buffer built; see
    /// `last_bg_floats`.
    last_image_floats: usize,
}

impl HeadlessRenderer {
    /// Construct a new headless renderer.
    ///
    /// Touches no GL — only CPU-side state is created here. Call
    /// [`Self::init`] with a [`Gl`] facade before drawing.
    ///
    /// # Errors
    ///
    /// Returns [`HeadlessDriverError::FontManager`] if font loading fails.
    pub fn new() -> Result<Self, HeadlessDriverError> {
        let font_manager = FontManager::new(&freminal_common::config::Config::default(), 1.0)?;
        Ok(Self {
            renderer: TerminalRenderer::new(),
            atlas: GlyphAtlas::default(),
            font_manager,
            shaping: ShapingCache::new(),
            toast_pill: ToastRenderer::new(),
            toast_text: ToastTextRenderer::new(),
            last_bg_floats: 0,
            last_fg_floats: 0,
            last_image_floats: 0,
        })
    }

    /// Create GPU resources for the terminal renderer and both toast passes.
    ///
    /// This has been verified to succeed against the `Recording` arm of
    /// [`Gl`]: `create_*` calls return `Ok`, compile/link status queries
    /// return `true`, `get_uniform_location` returns `Some`, and
    /// `check_framebuffer_status` returns `FRAMEBUFFER_COMPLETE` — see
    /// `gl_facade::facade`.
    ///
    /// # Errors
    ///
    /// Returns [`HeadlessDriverError::GpuInit`] if any GL resource creation,
    /// shader compilation, or program link fails.
    pub fn init(&mut self, gl: &Gl<'_>) -> Result<(), HeadlessDriverError> {
        self.renderer.init(gl)?;
        self.toast_pill.init(gl)?;
        self.toast_text.init(gl)?;
        Ok(())
    }

    /// Pixel dimensions of `frame` at the current font metrics.
    fn viewport_px(&self, frame: &SyntheticFrame) -> (i32, i32) {
        let w = frame
            .cols
            .saturating_mul(self.font_manager.cell_width().value_as().unwrap_or(0));
        let h = frame
            .rows
            .saturating_mul(self.font_manager.cell_height().value_as().unwrap_or(0));
        (w.value_as().unwrap_or(0), h.value_as().unwrap_or(0))
    }

    /// Draw one full synthetic frame through `gl`.
    ///
    /// This is the full-rebuild path: shape the grid, build background,
    /// decoration and foreground buffers, then hand them to
    /// [`TerminalRenderer::draw_with_verts`]. The toast overlay is drawn
    /// afterwards when `frame.toast` is [`ToastPresence::Present`].
    pub fn draw_frame(&mut self, gl: &Gl<'_>, frame: &SyntheticFrame) {
        let (vp_w, vp_h) = self.viewport_px(frame);
        let (chars, tags) = synthetic_grid(frame.cols, frame.rows);

        // Split borrow: `build_foreground_instances` needs `&mut atlas`
        // alongside `&font_manager`, and `draw_with_verts` needs `&mut
        // atlas` alongside `&mut renderer`. Destructuring gives each field
        // its own borrow instead of cloning the atlas or the font manager,
        // either of which would change what the harness measures.
        let Self {
            renderer,
            atlas,
            font_manager,
            shaping,
            ..
        } = self;

        let cell_width = font_manager.cell_width();
        let cell_height = font_manager.cell_height();
        let ascent = font_manager.ascent();
        let cell_width_f32: f32 = cell_width.value_as().unwrap_or(0.0);
        let cell_height_f32: f32 = cell_height.value_as().unwrap_or(0.0);

        let lines = shaping.shape_visible(
            &chars,
            &tags,
            frame.cols,
            font_manager,
            cell_width_f32,
            false,
            &[],
        );

        let cursor_style = CursorVisualStyle::BlockCursorSteady;
        let mut bg = Vec::new();
        let mut deco = Vec::new();
        let _cursor_appended = build_background_instances(
            &BackgroundFrame {
                shaped_lines: &lines,
                cell_width,
                cell_height,
                ascent,
                underline_offset: font_manager.underline_offset(),
                strikeout_offset: font_manager.strikeout_offset(),
                stroke_size: font_manager.stroke_size(),
                show_cursor: matches!(frame.cursor, CursorPresence::Shown),
                cursor_blink_on: true,
                cursor_pixel_pos: (0.0, 0.0),
                cursor_width_scale: 1.0,
                cursor_visual_style: &cursor_style,
                selection: None,
                selection_is_block: false,
                match_highlights: &[],
                command_block_hover_rows: None,
                term_width_cols: frame.cols,
                theme: &CATPPUCCIN_MOCHA,
                cursor_color_override: None,
                reverse_screen: false,
            },
            &mut bg,
            &mut deco,
        );

        let mut fg = Vec::new();
        build_foreground_instances(
            &lines,
            atlas,
            font_manager,
            cell_height,
            ascent,
            &FgRenderOptions::all_visible(None),
            &CATPPUCCIN_MOCHA,
            &mut fg,
        );

        renderer.draw_with_verts(
            gl,
            atlas,
            &bg,
            &deco,
            &fg,
            &[],
            &[],
            &HashMap::new(),
            vp_w,
            vp_h,
            cell_width_f32,
            cell_height_f32,
            1.0,
            1.0,
            BackgroundImageMode::default(),
            None,
        );

        self.last_bg_floats = bg.len();
        self.last_fg_floats = fg.len();
        self.last_image_floats = 0;

        if frame.toast == ToastPresence::Present {
            self.draw_toast(gl, vp_w, vp_h);
        }
    }

    /// Draw a single-toast overlay: one pill plus one text run.
    fn draw_toast(&mut self, gl: &Gl<'_>, vp_w: i32, vp_h: i32) {
        let quads = vec![ToastQuad {
            x: 16.0,
            y: 16.0,
            width: 240.0,
            height: 56.0,
            corner_radius: 8.0,
            color_top: [0.15, 0.15, 0.20, 1.0],
            color_bottom: [0.10, 0.10, 0.15, 1.0],
            border_color: [0.35, 0.35, 0.45, 1.0],
            border_width: 1.0,
            accent: [0.40, 0.70, 0.95, 1.0],
            opacity: 1.0,
        }];
        let runs = vec![ToastTextRun {
            text: "headless".to_owned(),
            origin_x: 24.0,
            baseline_y: 40.0,
            size_px: 14.0,
            color: [1.0, 1.0, 1.0, 1.0],
        }];

        let instances = self
            .toast_text
            .build_instances(&runs, &mut self.font_manager);
        self.toast_pill.draw(gl, &quads, vp_w, vp_h);
        self.toast_text.upload_and_draw(gl, &instances, vp_w, vp_h);
    }

    /// Draw one cursor-only frame through `gl`.
    ///
    /// The cursor-only fast path reuses the background, foreground and
    /// image buffers uploaded by the preceding [`Self::draw_frame`], so it
    /// is only meaningful after one has run — the `last_*_floats` values it
    /// passes are that frame's buffer lengths.
    pub fn draw_cursor_only(&mut self, gl: &Gl<'_>, frame: &SyntheticFrame) {
        let (vp_w, vp_h) = self.viewport_px(frame);
        let cell_width = self.font_manager.cell_width();
        let cell_height = self.font_manager.cell_height();
        let cell_width_f32: f32 = cell_width.value_as().unwrap_or(0.0);
        let cell_height_f32: f32 = cell_height.value_as().unwrap_or(0.0);

        let deco = build_cursor_verts_only(
            cell_width,
            cell_height,
            matches!(frame.cursor, CursorPresence::Shown),
            true,
            (0.0, 0.0),
            1.0,
            &CursorVisualStyle::BlockCursorSteady,
            &CATPPUCCIN_MOCHA,
            None,
        );

        self.renderer.draw_with_cursor_only_update(
            gl,
            &mut self.atlas,
            &deco,
            self.last_bg_floats,
            self.last_fg_floats,
            self.last_image_floats,
            &[],
            vp_w,
            vp_h,
            cell_width_f32,
            cell_height_f32,
            1.0,
            1.0,
            BackgroundImageMode::default(),
            None,
        );
    }
}

/// Drain a recording facade's log, treating a non-recording facade as an
/// empty log.
///
/// `recorded()` returns `None` only for a [`Gl::real`] instance, which this
/// module never constructs — every entry point below builds its own
/// [`Gl::recording`]. The `None` arm is therefore unreachable, and is
/// mapped to an empty log rather than unwrapped, since production code in
/// this crate may not panic.
fn drain(gl: &Gl<'_>) -> Vec<GlCall> {
    gl.recorded().map_or_else(Vec::new, |state| {
        let calls = state.calls();
        state.clear();
        calls
    })
}

/// Record one full frame, **including** the one-time GL initialization.
///
/// Use this to inspect what `init` costs. For per-frame numbers use
/// [`record_steady_state`] instead.
///
/// # Errors
///
/// Propagates font-manager and GL-init failures.
pub fn record_frame(frame: &SyntheticFrame) -> Result<Vec<GlCall>, HeadlessDriverError> {
    let gl = Gl::recording();
    let mut driver = HeadlessRenderer::new()?;
    driver.init(&gl)?;
    driver.draw_frame(&gl, frame);
    Ok(drain(&gl))
}

/// Record `frames` steady-state full frames, **excluding** initialization.
///
/// Initialization is drained and discarded before the measured frames
/// begin, and this is the whole point of the function. One-time GL object
/// creation — six shader programs, their VAOs and VBOs, the glyph-atlas
/// texture — is a large, fixed cost that would otherwise swamp the
/// per-frame figures and make them meaningless at small frame counts.
/// Separating one-time from per-frame cost is exactly the distinction
/// 123.14's reporting rests on, and the reason it reports frame rate and
/// per-frame cost as a pair rather than a single total.
///
/// # Errors
///
/// Propagates font-manager and GL-init failures.
pub fn record_steady_state(
    frame: &SyntheticFrame,
    frames: usize,
) -> Result<Vec<GlCall>, HeadlessDriverError> {
    let gl = Gl::recording();
    let mut driver = HeadlessRenderer::new()?;
    driver.init(&gl)?;
    drop(drain(&gl));
    for _ in 0..frames {
        driver.draw_frame(&gl, frame);
    }
    Ok(drain(&gl))
}

/// Record `frames` cursor-only frames, excluding initialization and the
/// priming full frame.
///
/// A cursor-only frame is only meaningful after a full rebuild has
/// uploaded the background, foreground and image buffers it reuses, so one
/// [`HeadlessRenderer::draw_frame`] runs first and is discarded along with
/// initialization.
///
/// # Errors
///
/// Propagates font-manager and GL-init failures.
pub fn record_cursor_only(
    frame: &SyntheticFrame,
    frames: usize,
) -> Result<Vec<GlCall>, HeadlessDriverError> {
    let gl = Gl::recording();
    let mut driver = HeadlessRenderer::new()?;
    driver.init(&gl)?;
    driver.draw_frame(&gl, frame);
    drop(drain(&gl));
    for _ in 0..frames {
        driver.draw_cursor_only(&gl, frame);
    }
    Ok(drain(&gl))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::super::gl_facade::surface::GL_CALL_SURFACE;
    use super::{SyntheticFrame, record_frame};

    #[test]
    fn records_a_well_formed_log_for_a_trivial_frame() {
        let calls = record_frame(&SyntheticFrame::new(8, 2)).expect("headless frame records");

        assert!(!calls.is_empty(), "a drawn frame records at least one call");

        let unknown: Vec<&str> = calls
            .iter()
            .map(|call| call.method)
            .filter(|method| !GL_CALL_SURFACE.contains(method))
            .collect();
        assert!(
            unknown.is_empty(),
            "recorded method(s) {unknown:?} are not in the frozen call surface"
        );

        let draws = calls
            .iter()
            .filter(|call| call.method == "draw_arrays_instanced")
            .count();
        assert!(draws > 0, "a frame with content issues instanced draws");

        let programs = calls
            .iter()
            .filter(|call| call.method == "use_program")
            .count();
        assert!(programs > 0, "a frame with content binds a shader program");
    }
}
