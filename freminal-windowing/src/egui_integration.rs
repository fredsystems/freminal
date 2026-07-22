// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! egui integration: input translation and rendering via `egui-winit` and `egui_glow`.

use std::sync::Arc;

use conv2::ConvUtil;
use winit::window::Window;

use crate::error::Error;
use crate::gl_context::GlState;

/// Output from a single egui frame.
pub struct FrameOutput {
    /// Viewport commands emitted by the app during this frame.
    pub commands: Vec<egui::ViewportCommand>,
    /// Requested repaint delay (`Duration::MAX` = no repaint needed).
    pub repaint_delay: std::time::Duration,
}

/// Cached chrome (head + tail) shapes and tessellated primitives from the
/// most recent FULL frame (#436.4a scaffolding).
///
/// Populated at the end of every frame (which, in this subtask, is always a
/// FULL frame — replay does not exist yet) but never read to decide replay;
/// 436.4b is what starts consulting this cache to skip re-tessellating
/// chrome on eligible frames.
// TODO(#436.4b): every field here is read once the replay decision path
// lands (cache-validity check against `ppp`/`size`, then reuse of
// `head_primitives`/`tail_primitives` in place of re-tessellating
// `head_shapes`/`tail_shapes`). Until then the fields are write-only
// scaffolding, landed in 436.4a alongside the paint-order split they
// describe so the two changes review together.
#[allow(dead_code)]
struct ChromeCache {
    /// Chrome shapes painted *before* the terminal band in `full_output.shapes`
    /// (e.g. the `CentralPanel` background fill, menu bar, tab bar).
    head_shapes: Vec<egui::epaint::ClippedShape>,
    /// Chrome shapes painted *after* the terminal band (overlays, pane
    /// borders drawn as part of chrome, modals, tooltips).
    tail_shapes: Vec<egui::epaint::ClippedShape>,
    /// Tessellation of `head_shapes` at `ppp`/`size`.
    head_primitives: Vec<egui::ClippedPrimitive>,
    /// Tessellation of `tail_shapes` at `ppp`/`size`.
    tail_primitives: Vec<egui::ClippedPrimitive>,
    /// `pixels_per_point` the cached primitives were tessellated at. A
    /// mismatch on a later frame invalidates the cache (436.4b).
    ppp: f32,
    /// Physical framebuffer size the cached primitives were tessellated
    /// for. A mismatch on a later frame invalidates the cache (436.4b).
    size: [u32; 2],
}

/// Per-window egui state.
pub struct EguiState {
    pub(crate) ctx: egui::Context,
    pub(crate) winit_state: egui_winit::State,
    pub(crate) painter: egui_glow::Painter,
    /// Cached chrome primitives from the last FULL frame (#436.4a). Not yet
    /// consulted for replay decisions — see [`ChromeCache`].
    chrome_cache: Option<ChromeCache>,
    /// The repaint delay `run_frame` returned last frame (#436.4a
    /// scaffolding). Not yet consulted — 436.4b uses this alongside
    /// `prev_chrome_damage` to help decide replay eligibility.
    prev_repaint_delay: std::time::Duration,
    /// The chrome-damage decision drained last frame (#436.4a scaffolding).
    /// Not yet updated per-frame or consulted — 436.4b wires both the write
    /// (from `App::take_chrome_damage`) and the read (replay gate).
    // TODO(#436.4b): read (and start writing, per-frame) to gate the
    // chrome-replay decision. Write-only scaffolding until then.
    #[allow(dead_code)]
    prev_chrome_damage: crate::ChromeDamage,
}

impl EguiState {
    /// Create egui state for a window.
    pub(crate) fn new(window: &Window, gl_state: &GlState) -> Result<Self, Error> {
        let ctx = egui::Context::default();

        let winit_state = egui_winit::State::new(
            ctx.clone(),
            egui::ViewportId::ROOT,
            window,
            // Scale factor is inherently a float; `approx_as` is the lossy but
            // well-defined conversion. `1.0` fallback matches the default DPI.
            Some(window.scale_factor().approx_as::<f32>().unwrap_or(1.0)),
            None,
            None,
        );

        let painter = egui_glow::Painter::new(Arc::clone(&gl_state.glow_context), "", None, false)
            .map_err(|e| Error::GlContextCreation(format!("egui painter creation failed: {e}")))?;

        Ok(Self {
            ctx,
            winit_state,
            painter,
            chrome_cache: None,
            prev_repaint_delay: std::time::Duration::MAX,
            prev_chrome_damage: crate::ChromeDamage::Changed,
        })
    }

    /// Collect raw input from winit for the current frame.
    pub(crate) fn take_egui_input(&mut self, window: &Window) -> egui::RawInput {
        self.winit_state.take_egui_input(window)
    }

    /// Run a single egui frame and paint, using pre-collected raw input.
    ///
    /// Returns [`FrameOutput`] containing viewport commands and repaint timing.
    pub(crate) fn run_frame<F>(
        &mut self,
        window: &Window,
        gl_state: &GlState,
        clear_color: [f32; 4],
        raw_input: egui::RawInput,
        present_flag: Option<&std::sync::Arc<std::sync::atomic::AtomicBool>>,
        ui_fn: F,
    ) -> FrameOutput
    where
        F: FnMut(&egui::Context, &glow::Context, crate::ChromeMode) -> crate::FrameSignals,
    {
        let mut ui_fn = ui_fn;

        // egui 0.35 replaced `Context::run` (closure took `&Context`) with
        // `Context::run_ui` (closure takes the root `&mut Ui`).  Our `App`
        // trait still works in terms of `&Context`; `Ui` derefs to `Context`,
        // so deref explicitly rather than relying on a silent coercion.
        //
        // The closure both runs the app's `update` and returns this frame's
        // signals (damage report + terminal-band range, #436.4a); we capture
        // them here to decide the clear/present path and the head/band/tail
        // split below. Running both inside the one closure avoids two
        // simultaneous `&mut app` borrows in the caller.
        //
        // `chrome_mode` is always `Full` in this subtask — 436.4b is what
        // starts computing a real replay decision and passing `Replay`.
        let mut frame_damage = crate::FrameDamage::Full;
        let mut band_range: std::ops::Range<usize> = 0..0;
        let full_output = self.ctx.run_ui(raw_input, |root_ui| {
            let signals = ui_fn(&*root_ui, &gl_state.glow_context, crate::ChromeMode::Full);
            frame_damage = signals.frame_damage;
            band_range = signals.band_range;
        });

        self.winit_state
            .handle_platform_output(window, full_output.platform_output);

        let pixels_per_point = self.ctx.pixels_per_point();

        // ── 3-way paint-order split (#436.4a) ──────────────────────────
        //
        // Slice `full_output.shapes` into head (chrome painted before the
        // terminal band — e.g. the `CentralPanel` background fill, menu
        // bar, tab bar), band (terminal content, rebuilt every frame), and
        // tail (chrome painted after the band — overlays, borders, modals)
        // by `band_range`, then tessellate and paint each slice separately,
        // in that order. `LayerId::background()`'s `PaintList` drains
        // first into `full_output.shapes`, and the band occupies a
        // contiguous range within it (see `App::take_terminal_band_range`),
        // so `band_range` is valid as an index range into `full_output.shapes`
        // directly.
        //
        // Clamp defensively: an app that reports a range referring to a
        // shape count larger than what actually drained (e.g. stale state)
        // must never panic on the slice below. `start` is clamped to the
        // shape count; `end` is then clamped to `[start, shape count]`, so
        // `start <= end <= shapes.len()` always holds.
        let shapes = full_output.shapes;
        let start = band_range.start.min(shapes.len());
        let end = band_range.end.clamp(start, shapes.len());
        let head_shapes: Vec<egui::epaint::ClippedShape> = shapes[..start].to_vec();
        let band_shapes: Vec<egui::epaint::ClippedShape> = shapes[start..end].to_vec();
        let tail_shapes: Vec<egui::epaint::ClippedShape> = shapes[end..].to_vec();

        // When `band_range` is the default `0..0` (an app that has not
        // wired up the band range), `start == end == 0`, so `head_shapes`
        // and `band_shapes` are empty and `tail_shapes` is the ENTIRE shape
        // list. Painting all shapes as a single "tail" `paint_primitives`
        // call is exactly what the pre-#436.4a single-call path did —
        // byte-identical rendering for an app that does not participate.
        let head_primitives = self.ctx.tessellate(head_shapes.clone(), pixels_per_point);
        let band_primitives = self.ctx.tessellate(band_shapes, pixels_per_point);
        let tail_primitives = self.ctx.tessellate(tail_shapes.clone(), pixels_per_point);

        let size = window.inner_size();

        // Populate the chrome cache (#436.4a scaffolding). Never read to
        // decide replay yet — that is 436.4b.
        self.chrome_cache = Some(ChromeCache {
            head_shapes,
            tail_shapes,
            head_primitives: head_primitives.clone(),
            tail_primitives: tail_primitives.clone(),
            ppp: pixels_per_point,
            size: [size.width, size.height],
        });

        // Decide whether this frame may skip the full clear and present only
        // its damaged region. This is a two-part gate:
        //   1. The app reports the frame as `Partial` (only the listed rects
        //      changed; everything else is identical to the previous frame).
        //   2. The back buffer still holds the previous frame's contents
        //      (`buffer_age() == 1`), and the surface can present a sub-region.
        // If either fails we fall back to the always-correct full path:
        // clear + full paint + full swap.
        let partial = match frame_damage {
            crate::FrameDamage::Partial(rects)
                if !rects.is_empty()
                    && gl_state.supports_partial_present()
                    && gl_state.buffer_age() == 1 =>
            {
                Some(rects)
            }
            _ => None,
        };

        if partial.is_none() {
            gl_state.clear(clear_color);
        }

        // Publish the authoritative decision BEFORE the paint callbacks run
        // (they execute inside the `paint_primitives` calls below), so any
        // callback that scissors to the damage region gates on the same
        // value that decided whether the clear was skipped. Same-thread store
        // immediately before the reads -> `Relaxed` is sufficient.
        if let Some(flag) = present_flag {
            flag.store(partial.is_some(), std::sync::atomic::Ordering::Relaxed);
        }

        // Paint: set all textures, then three `paint_primitives` calls in
        // head -> band -> tail order, then free all textures. This is
        // exactly what `paint_and_update_textures` does internally (set
        // all -> paint -> free all), just split across three paint calls so
        // the band can be painted independently of chrome. Order matters:
        // `paint_primitives` re-establishes GL state (scissor/blend, unbound
        // VBO/EBO/texture/program) independently on every call, so three
        // sequential calls over a partition of the same shape list paint
        // identically to one call over the concatenation — head paints
        // first (e.g. the `CentralPanel` background fill, which must be
        // UNDER the band), then band, then tail (overlays/borders, which
        // must be OVER the band).
        let size_px = [size.width, size.height];
        for (id, image_delta) in &full_output.textures_delta.set {
            self.painter.set_texture(*id, image_delta);
        }
        self.painter
            .paint_primitives(size_px, pixels_per_point, &head_primitives);
        self.painter
            .paint_primitives(size_px, pixels_per_point, &band_primitives);
        self.painter
            .paint_primitives(size_px, pixels_per_point, &tail_primitives);
        for id in &full_output.textures_delta.free {
            self.painter.free_texture(*id);
        }

        // Pre-present notify for Wayland frame pacing
        window.pre_present_notify();

        let swap_result = partial.as_ref().map_or_else(
            || gl_state.swap_buffers(),
            |rects| gl_state.swap_buffers_with_damage(rects),
        );
        if let Err(e) = swap_result {
            tracing::error!("swap_buffers failed: {e}");
        }

        let viewport_output = full_output.viewport_output.get(&egui::ViewportId::ROOT);

        let repaint_delay = viewport_output.map_or(std::time::Duration::MAX, |vo| vo.repaint_delay);

        let commands = viewport_output
            .map(|vo| vo.commands.clone())
            .unwrap_or_default();

        // Do NOT call `window.request_redraw()` here — let the event loop
        // manage scheduling via `repaint_at` / `about_to_wait`.  Calling
        // `request_redraw()` directly bypasses `ControlFlow::WaitUntil` and
        // causes an unbounded render loop on platforms where `swap_buffers`
        // returns immediately (macOS with vsync disabled).

        // Stash this frame's repaint delay for 436.4b's replay-eligibility
        // gate. Not consumed yet.
        self.prev_repaint_delay = repaint_delay;

        FrameOutput {
            commands,
            repaint_delay,
        }
    }

    /// Pass a winit `WindowEvent` to egui.
    ///
    /// Forward a window event to egui-winit.
    pub(crate) fn on_window_event(
        &mut self,
        window: &Window,
        event: &winit::event::WindowEvent,
    ) -> egui_winit::EventResponse {
        self.winit_state.on_window_event(window, event)
    }

    /// Inject a paste event directly into egui's input queue.
    pub(crate) fn inject_paste(&mut self, text: String) {
        self.winit_state
            .egui_input_mut()
            .events
            .push(egui::Event::Paste(text));
    }

    /// Read clipboard text via this window's egui-winit clipboard.
    pub(crate) fn clipboard_text(&mut self) -> Option<String> {
        self.winit_state.clipboard_text()
    }

    /// Read the current egui modifier state.
    pub(crate) fn modifiers(&self) -> egui::Modifiers {
        self.winit_state.egui_input().modifiers
    }

    /// Free the painter's OpenGL resources.
    ///
    /// Must be called while this window's GL context is current and before the
    /// painter is dropped. `egui_glow::Painter::destroy` is idempotent (guarded
    /// by an internal `destroyed` flag), so calling it more than once is safe.
    pub(crate) fn destroy_painter(&mut self) {
        self.painter.destroy();
    }
}

#[cfg(test)]
mod tests {
    use egui::epaint::Primitive;
    use egui::{Color32, Rect, pos2, vec2};

    /// Sum the vertex/index counts across every `Mesh` primitive in a
    /// tessellation result. `Callback` primitives (paint callbacks) carry no
    /// mesh data of their own, so they are not part of this count; the test
    /// below paints only `rect_filled` shapes, which always tessellate to
    /// `Mesh` primitives, so no callback primitives appear.
    fn total_verts_indices(primitives: &[egui::ClippedPrimitive]) -> (usize, usize) {
        let mut vertices = 0;
        let mut indices = 0;
        for clipped in primitives {
            if let Primitive::Mesh(mesh) = &clipped.primitive {
                vertices += mesh.vertices.len();
                indices += mesh.indices.len();
            }
        }
        (vertices, indices)
    }

    /// A single mesh vertex flattened to comparable primitives (egui's
    /// `Vertex`/`Pos2`/`Color32` are `PartialEq`, but bundling the fields
    /// makes the assertion failure message readable and avoids depending on
    /// `Vertex: PartialEq` staying derived). Field order: position, uv, color.
    type FlatVertex = ([f32; 2], [f32; 2], [u8; 4]);

    /// Flatten a primitive list into an ORDERED sequence of vertices and an
    /// ORDERED sequence of indices (offset so indices are global across the
    /// whole list, matching how the meshes would be drawn back-to-back).
    /// Comparing these sequences — not just their lengths — pins that the
    /// 3-call split preserves geometry *order*, which is the exact property
    /// `paint_primitives`' head->band->tail sequencing depends on.
    fn flatten_mesh_geometry(primitives: &[egui::ClippedPrimitive]) -> (Vec<FlatVertex>, Vec<u32>) {
        let mut verts: Vec<FlatVertex> = Vec::new();
        let mut idxs: Vec<u32> = Vec::new();
        for clipped in primitives {
            if let Primitive::Mesh(mesh) = &clipped.primitive {
                let base = u32::try_from(verts.len()).unwrap_or(u32::MAX);
                for v in &mesh.vertices {
                    verts.push((
                        [v.pos.x, v.pos.y],
                        [v.uv.x, v.uv.y],
                        [v.color.r(), v.color.g(), v.color.b(), v.color.a()],
                    ));
                }
                for &i in &mesh.indices {
                    idxs.push(base + i);
                }
            }
        }
        (verts, idxs)
    }

    /// Pins the losslessness of `run_frame`'s head/band/tail split
    /// (#436.4a): tessellating `full_output.shapes` as three slices
    /// (`[..start]`, `[start..end]`, `[end..]`) and summing the resulting
    /// primitives' vertex/index counts must equal tessellating the whole
    /// list at once. `egui::Context::tessellate` builds a fresh
    /// `Tessellator` per call from only `pixels_per_point`,
    /// `tessellation_options`, and the font texture atlas size — none of
    /// which vary between the whole-list call and the three sliced calls —
    /// so per-shape tessellation is independent of what else is in the
    /// list. The *batching* of tessellated meshes into `ClippedPrimitive`s
    /// may differ (adjacent same-clip-rect shapes can merge into fewer,
    /// larger meshes when tessellated together), but the underlying vertex
    /// and index data — and therefore the pixels drawn — must be identical
    /// either way. This is the property `run_frame`'s 3-call paint depends
    /// on for byte-identical rendering.
    #[test]
    fn head_band_tail_split_is_lossless_vs_whole_tessellation() {
        let ctx = egui::Context::default();
        let pixels_per_point = 1.0;

        let full_output = ctx.run_ui(egui::RawInput::default(), |ui| {
            // Shape 0: "head" (chrome painted before the band).
            ui.painter().rect_filled(
                Rect::from_min_size(pos2(0.0, 0.0), vec2(5.0, 5.0)),
                0.0,
                Color32::RED,
            );
            // Shapes 1-2: "band" (terminal content).
            ui.painter().rect_filled(
                Rect::from_min_size(pos2(10.0, 10.0), vec2(5.0, 5.0)),
                0.0,
                Color32::GREEN,
            );
            ui.painter().rect_filled(
                Rect::from_min_size(pos2(20.0, 20.0), vec2(5.0, 5.0)),
                0.0,
                Color32::BLUE,
            );
            // Shape 3: "tail" (chrome painted after the band).
            ui.painter().rect_filled(
                Rect::from_min_size(pos2(30.0, 30.0), vec2(5.0, 5.0)),
                0.0,
                Color32::YELLOW,
            );
        });

        let shapes = full_output.shapes;
        assert_eq!(shapes.len(), 4, "sanity: exactly the four shapes painted");

        let whole_primitives = ctx.tessellate(shapes.clone(), pixels_per_point);

        // Band range covering shapes 1..3 (the green and blue rects), as
        // `App::take_terminal_band_range` would report.
        let start = 1;
        let end = 3;
        let head_shapes = shapes[..start].to_vec();
        let band_shapes = shapes[start..end].to_vec();
        let tail_shapes = shapes[end..].to_vec();

        let head_primitives = ctx.tessellate(head_shapes, pixels_per_point);
        let band_primitives = ctx.tessellate(band_shapes, pixels_per_point);
        let tail_primitives = ctx.tessellate(tail_shapes, pixels_per_point);

        let (whole_vertices, whole_indices) = total_verts_indices(&whole_primitives);
        let (head_vertices, head_indices) = total_verts_indices(&head_primitives);
        let (band_vertices, band_indices) = total_verts_indices(&band_primitives);
        let (tail_vertices, tail_indices) = total_verts_indices(&tail_primitives);

        assert_eq!(
            whole_vertices,
            head_vertices + band_vertices + tail_vertices,
            "split tessellation must produce the same total vertex count as \
             tessellating the whole shape list at once"
        );
        assert_eq!(
            whole_indices,
            head_indices + band_indices + tail_indices,
            "split tessellation must produce the same total index count as \
             tessellating the whole shape list at once"
        );
        assert!(
            whole_vertices > 0,
            "sanity: the shapes actually tessellated to something"
        );

        // Stronger than counts: the ORDERED vertex/index sequences must be
        // identical. head ++ band ++ tail (concatenated in paint order, with
        // indices re-based across the concatenation) must equal the whole
        // list's geometry vertex-for-vertex and index-for-index. This is the
        // exact property `run_frame`'s head->band->tail `paint_primitives`
        // sequence relies on for byte-identical pixels — counts matching
        // alone would not rule out a reordering.
        let (whole_verts, whole_idxs) = flatten_mesh_geometry(&whole_primitives);

        let mut split_primitives = head_primitives;
        split_primitives.extend(band_primitives);
        split_primitives.extend(tail_primitives);
        let (split_verts, split_idxs) = flatten_mesh_geometry(&split_primitives);

        assert_eq!(
            whole_verts, split_verts,
            "split tessellation must produce the same vertex SEQUENCE (order \
             included) as the whole-list tessellation"
        );
        assert_eq!(
            whole_idxs, split_idxs,
            "split tessellation must produce the same index SEQUENCE (order \
             included) as the whole-list tessellation"
        );
    }

    /// Confirms the `0..0` default `band_range` (an app that has not wired
    /// up `App::take_terminal_band_range`) behaves as `run_frame` assumes:
    /// `head_shapes` and `band_shapes` are empty, and `tail_shapes` is the
    /// ENTIRE shape list — i.e. painting all shapes as a single "tail"
    /// `paint_primitives` call, byte-identical to the pre-#436.4a
    /// single-call path.
    #[test]
    fn default_band_range_puts_everything_in_tail() {
        let ctx = egui::Context::default();

        let full_output = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.painter().rect_filled(
                Rect::from_min_size(pos2(0.0, 0.0), vec2(5.0, 5.0)),
                0.0,
                Color32::RED,
            );
            ui.painter().rect_filled(
                Rect::from_min_size(pos2(10.0, 10.0), vec2(5.0, 5.0)),
                0.0,
                Color32::GREEN,
            );
        });

        let shapes = full_output.shapes;
        assert_eq!(shapes.len(), 2, "sanity: exactly the two shapes painted");

        let band_range: std::ops::Range<usize> = 0..0;
        let start = band_range.start.min(shapes.len());
        let end = band_range.end.clamp(start, shapes.len());

        let head_shapes = &shapes[..start];
        let band_shapes = &shapes[start..end];
        let tail_shapes = &shapes[end..];

        assert!(head_shapes.is_empty());
        assert!(band_shapes.is_empty());
        assert_eq!(tail_shapes.len(), shapes.len());
    }
}
