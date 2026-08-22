// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! The `Gl` facade — the concrete dispatch point every GL call in the
//! crate passes through.
//!
//! Each method here is a 1:1 delegation to the identically-named
//! `glow::HasContext` method (same name, same parameters, same return
//! type); `glow`'s own documentation is the reference for what each call
//! actually does. What this module adds is the second, `gl-recording`-only
//! arm: instead of issuing the call to a real `glow::Context`, it appends a
//! [`GlCall`] to a [`RecordingState`] log and fabricates a plausible return
//! value, so the Task 123 measurement harness can run without a GPU,
//! display server, or driver.

use glow::HasContext;

#[cfg(feature = "gl-recording")]
use super::recording::{GlCall, GlCallPayload, RecordingState};
#[cfg(feature = "gl-recording")]
use conv2::ConvUtil;

/// The GL call facade every renderer call site dispatches through.
///
/// # Why a concrete struct, not a trait
///
/// `glow::HasContext` is **sealed** (see the module-level docs on
/// [`super`]): implementing it on a wrapper type is a hard compile error,
/// not a design tradeoff. And even setting that aside, a trait-based
/// dispatch would force generic bounds at roughly 52 `&glow::Context`
/// parameter sites across five files, widening every one of those
/// signatures just to carry a type parameter most of them would never
/// otherwise need. The enum-backed struct instead keeps codegen
/// monomorphic in the `Real` arm — the arm every production build actually
/// takes — and turns the 123.4/123.5 migration into a mechanical type
/// substitution (`&glow::Context` becomes `&Gl<'_>`) rather than a
/// redesign of call sites.
///
/// # Why the methods stay `unsafe`
///
/// Every method mirrors its `glow::HasContext` namesake's signature
/// exactly — same name, same parameters, same return type — so that the
/// 123.4/123.5 migration is a pure type substitution: an existing
/// `unsafe { gl.foo(..) }` call site keeps its `unsafe` block verbatim and
/// only the binding's type changes from `&glow::Context` to `&Gl<'_>`.
/// Making these methods safe would hide a contract that is genuinely the
/// caller's to uphold — a valid current GL context, live handles, and
/// correctly-sized buffers — behind a facade that cannot itself verify any
/// of it.
///
/// # Safety
///
/// Every method on `Gl` carries the same safety contract as the
/// identically-named method on `glow::HasContext`: the caller must ensure
/// a GL context is current, that any handles passed in are valid and were
/// obtained from the same context, and that any sizes or offsets are
/// in-bounds for the buffers or textures involved. The `Recording` arm
/// issues no GL calls at all and is trivially sound regardless, but the
/// contract is stated here at the type level so the `Real` arm's
/// obligations are not understated by the presence of a harmless
/// alternative arm.
pub struct Gl<'a> {
    inner: GlTarget<'a>,
}

/// The two things a [`Gl`] can dispatch to.
///
/// `'a` is used only by [`GlTarget::Real`]; the `Recording` arm owns its
/// state outright and does not borrow from the caller. That asymmetry is
/// expected — a recording session and a live driver context have
/// different lifetimes by nature — and is not a sign the lifetime
/// parameter is misplaced.
enum GlTarget<'a> {
    /// Delegates every call straight to a live `glow::Context`.
    Real(&'a glow::Context),
    /// Logs every call into a [`RecordingState`] instead of issuing it.
    #[cfg(feature = "gl-recording")]
    Recording(RecordingState),
}

impl<'a> Gl<'a> {
    /// Build a facade that delegates every call to `gl`.
    #[must_use]
    pub const fn real(gl: &'a glow::Context) -> Self {
        Self {
            inner: GlTarget::Real(gl),
        }
    }

    /// Build a facade that records every call instead of issuing it.
    ///
    /// Only available under the `gl-recording` feature — a default build
    /// has no [`RecordingState`] to construct.
    #[cfg(feature = "gl-recording")]
    #[must_use]
    pub const fn recording() -> Self {
        Self {
            inner: GlTarget::Recording(RecordingState::new()),
        }
    }

    /// Borrow this facade's [`RecordingState`], if it is a recording
    /// instance.
    ///
    /// Returns `None` for a [`Gl::real`] instance; there is no recording
    /// state to hand back.
    #[cfg(feature = "gl-recording")]
    #[must_use]
    pub const fn recorded(&self) -> Option<&RecordingState> {
        match &self.inner {
            GlTarget::Real(_) => None,
            GlTarget::Recording(state) => Some(state),
        }
    }
}

impl Gl<'_> {
    pub(crate) unsafe fn active_texture(&self, unit: u32) {
        match &self.inner {
            GlTarget::Real(gl) => unsafe { gl.active_texture(unit) },
            #[cfg(feature = "gl-recording")]
            GlTarget::Recording(state) => state.record(GlCall {
                method: "active_texture",
                payload: GlCallPayload::None,
            }),
        }
    }

    pub(crate) unsafe fn attach_shader(&self, program: glow::Program, shader: glow::Shader) {
        match &self.inner {
            GlTarget::Real(gl) => unsafe { gl.attach_shader(program, shader) },
            #[cfg(feature = "gl-recording")]
            GlTarget::Recording(state) => state.record(GlCall {
                method: "attach_shader",
                payload: GlCallPayload::None,
            }),
        }
    }

    pub(crate) unsafe fn bind_buffer(&self, target: u32, buffer: Option<glow::Buffer>) {
        match &self.inner {
            GlTarget::Real(gl) => unsafe { gl.bind_buffer(target, buffer) },
            #[cfg(feature = "gl-recording")]
            GlTarget::Recording(state) => state.record(GlCall {
                method: "bind_buffer",
                payload: GlCallPayload::None,
            }),
        }
    }

    pub(crate) unsafe fn bind_framebuffer(
        &self,
        target: u32,
        framebuffer: Option<glow::Framebuffer>,
    ) {
        match &self.inner {
            GlTarget::Real(gl) => unsafe { gl.bind_framebuffer(target, framebuffer) },
            #[cfg(feature = "gl-recording")]
            GlTarget::Recording(state) => state.record(GlCall {
                method: "bind_framebuffer",
                payload: GlCallPayload::None,
            }),
        }
    }

    pub(crate) unsafe fn bind_texture(&self, target: u32, texture: Option<glow::Texture>) {
        match &self.inner {
            GlTarget::Real(gl) => unsafe { gl.bind_texture(target, texture) },
            #[cfg(feature = "gl-recording")]
            GlTarget::Recording(state) => state.record(GlCall {
                method: "bind_texture",
                payload: GlCallPayload::None,
            }),
        }
    }

    pub(crate) unsafe fn bind_vertex_array(&self, vertex_array: Option<glow::VertexArray>) {
        match &self.inner {
            GlTarget::Real(gl) => unsafe { gl.bind_vertex_array(vertex_array) },
            #[cfg(feature = "gl-recording")]
            GlTarget::Recording(state) => state.record(GlCall {
                method: "bind_vertex_array",
                payload: GlCallPayload::None,
            }),
        }
    }

    pub(crate) unsafe fn buffer_data_size(&self, target: u32, size: i32, usage: u32) {
        match &self.inner {
            GlTarget::Real(gl) => unsafe { gl.buffer_data_size(target, size, usage) },
            #[cfg(feature = "gl-recording")]
            GlTarget::Recording(state) => {
                state.record(GlCall {
                    method: "buffer_data_size",
                    payload: GlCallPayload::Upload {
                        bytes: size.approx_as::<u64>().unwrap_or(0),
                    },
                });
            }
        }
    }

    pub(crate) unsafe fn buffer_data_u8_slice(&self, target: u32, data: &[u8], usage: u32) {
        match &self.inner {
            GlTarget::Real(gl) => unsafe { gl.buffer_data_u8_slice(target, data, usage) },
            #[cfg(feature = "gl-recording")]
            GlTarget::Recording(state) => {
                state.record(GlCall {
                    method: "buffer_data_u8_slice",
                    payload: GlCallPayload::Upload {
                        bytes: data.len().approx_as::<u64>().unwrap_or(0),
                    },
                });
            }
        }
    }

    pub(crate) unsafe fn buffer_sub_data_u8_slice(
        &self,
        target: u32,
        offset: i32,
        src_data: &[u8],
    ) {
        match &self.inner {
            GlTarget::Real(gl) => unsafe { gl.buffer_sub_data_u8_slice(target, offset, src_data) },
            #[cfg(feature = "gl-recording")]
            GlTarget::Recording(state) => {
                state.record(GlCall {
                    method: "buffer_sub_data_u8_slice",
                    payload: GlCallPayload::Upload {
                        bytes: src_data.len().approx_as::<u64>().unwrap_or(0),
                    },
                });
            }
        }
    }

    pub(crate) unsafe fn check_framebuffer_status(&self, target: u32) -> u32 {
        match &self.inner {
            GlTarget::Real(gl) => unsafe { gl.check_framebuffer_status(target) },
            #[cfg(feature = "gl-recording")]
            GlTarget::Recording(state) => {
                state.record(GlCall {
                    method: "check_framebuffer_status",
                    payload: GlCallPayload::None,
                });
                // A plausible success value so a headless driver run does
                // not take an error path.
                glow::FRAMEBUFFER_COMPLETE
            }
        }
    }

    pub(crate) unsafe fn clear(&self, mask: u32) {
        match &self.inner {
            GlTarget::Real(gl) => unsafe { gl.clear(mask) },
            #[cfg(feature = "gl-recording")]
            GlTarget::Recording(state) => state.record(GlCall {
                method: "clear",
                payload: GlCallPayload::None,
            }),
        }
    }

    pub(crate) unsafe fn clear_color(&self, red: f32, green: f32, blue: f32, alpha: f32) {
        match &self.inner {
            GlTarget::Real(gl) => unsafe { gl.clear_color(red, green, blue, alpha) },
            #[cfg(feature = "gl-recording")]
            GlTarget::Recording(state) => state.record(GlCall {
                method: "clear_color",
                payload: GlCallPayload::None,
            }),
        }
    }

    pub(crate) unsafe fn compile_shader(&self, shader: glow::Shader) {
        match &self.inner {
            GlTarget::Real(gl) => unsafe { gl.compile_shader(shader) },
            #[cfg(feature = "gl-recording")]
            GlTarget::Recording(state) => state.record(GlCall {
                method: "compile_shader",
                payload: GlCallPayload::None,
            }),
        }
    }

    pub(crate) unsafe fn create_buffer(&self) -> Result<glow::Buffer, String> {
        match &self.inner {
            GlTarget::Real(gl) => unsafe { gl.create_buffer() },
            #[cfg(feature = "gl-recording")]
            GlTarget::Recording(state) => {
                state.record(GlCall {
                    method: "create_buffer",
                    payload: GlCallPayload::None,
                });
                Ok(state.next_buffer())
            }
        }
    }

    pub(crate) unsafe fn create_framebuffer(&self) -> Result<glow::Framebuffer, String> {
        match &self.inner {
            GlTarget::Real(gl) => unsafe { gl.create_framebuffer() },
            #[cfg(feature = "gl-recording")]
            GlTarget::Recording(state) => {
                state.record(GlCall {
                    method: "create_framebuffer",
                    payload: GlCallPayload::None,
                });
                Ok(state.next_framebuffer())
            }
        }
    }

    pub(crate) unsafe fn create_program(&self) -> Result<glow::Program, String> {
        match &self.inner {
            GlTarget::Real(gl) => unsafe { gl.create_program() },
            #[cfg(feature = "gl-recording")]
            GlTarget::Recording(state) => {
                state.record(GlCall {
                    method: "create_program",
                    payload: GlCallPayload::None,
                });
                Ok(state.next_program())
            }
        }
    }

    pub(crate) unsafe fn create_shader(&self, shader_type: u32) -> Result<glow::Shader, String> {
        match &self.inner {
            GlTarget::Real(gl) => unsafe { gl.create_shader(shader_type) },
            #[cfg(feature = "gl-recording")]
            GlTarget::Recording(state) => {
                state.record(GlCall {
                    method: "create_shader",
                    payload: GlCallPayload::None,
                });
                Ok(state.next_shader())
            }
        }
    }

    pub(crate) unsafe fn create_texture(&self) -> Result<glow::Texture, String> {
        match &self.inner {
            GlTarget::Real(gl) => unsafe { gl.create_texture() },
            #[cfg(feature = "gl-recording")]
            GlTarget::Recording(state) => {
                state.record(GlCall {
                    method: "create_texture",
                    payload: GlCallPayload::None,
                });
                Ok(state.next_texture())
            }
        }
    }

    pub(crate) unsafe fn create_vertex_array(&self) -> Result<glow::VertexArray, String> {
        match &self.inner {
            GlTarget::Real(gl) => unsafe { gl.create_vertex_array() },
            #[cfg(feature = "gl-recording")]
            GlTarget::Recording(state) => {
                state.record(GlCall {
                    method: "create_vertex_array",
                    payload: GlCallPayload::None,
                });
                Ok(state.next_vertex_array())
            }
        }
    }

    pub(crate) unsafe fn delete_buffer(&self, buffer: glow::Buffer) {
        match &self.inner {
            GlTarget::Real(gl) => unsafe { gl.delete_buffer(buffer) },
            #[cfg(feature = "gl-recording")]
            GlTarget::Recording(state) => state.record(GlCall {
                method: "delete_buffer",
                payload: GlCallPayload::None,
            }),
        }
    }

    pub(crate) unsafe fn delete_framebuffer(&self, framebuffer: glow::Framebuffer) {
        match &self.inner {
            GlTarget::Real(gl) => unsafe { gl.delete_framebuffer(framebuffer) },
            #[cfg(feature = "gl-recording")]
            GlTarget::Recording(state) => state.record(GlCall {
                method: "delete_framebuffer",
                payload: GlCallPayload::None,
            }),
        }
    }

    pub(crate) unsafe fn delete_program(&self, program: glow::Program) {
        match &self.inner {
            GlTarget::Real(gl) => unsafe { gl.delete_program(program) },
            #[cfg(feature = "gl-recording")]
            GlTarget::Recording(state) => state.record(GlCall {
                method: "delete_program",
                payload: GlCallPayload::None,
            }),
        }
    }

    pub(crate) unsafe fn delete_shader(&self, shader: glow::Shader) {
        match &self.inner {
            GlTarget::Real(gl) => unsafe { gl.delete_shader(shader) },
            #[cfg(feature = "gl-recording")]
            GlTarget::Recording(state) => state.record(GlCall {
                method: "delete_shader",
                payload: GlCallPayload::None,
            }),
        }
    }

    pub(crate) unsafe fn delete_texture(&self, texture: glow::Texture) {
        match &self.inner {
            GlTarget::Real(gl) => unsafe { gl.delete_texture(texture) },
            #[cfg(feature = "gl-recording")]
            GlTarget::Recording(state) => state.record(GlCall {
                method: "delete_texture",
                payload: GlCallPayload::None,
            }),
        }
    }

    pub(crate) unsafe fn delete_vertex_array(&self, vertex_array: glow::VertexArray) {
        match &self.inner {
            GlTarget::Real(gl) => unsafe { gl.delete_vertex_array(vertex_array) },
            #[cfg(feature = "gl-recording")]
            GlTarget::Recording(state) => state.record(GlCall {
                method: "delete_vertex_array",
                payload: GlCallPayload::None,
            }),
        }
    }

    pub(crate) unsafe fn disable(&self, parameter: u32) {
        match &self.inner {
            GlTarget::Real(gl) => unsafe { gl.disable(parameter) },
            #[cfg(feature = "gl-recording")]
            GlTarget::Recording(state) => state.record(GlCall {
                method: "disable",
                payload: GlCallPayload::None,
            }),
        }
    }

    pub(crate) unsafe fn draw_arrays(&self, mode: u32, first: i32, count: i32) {
        match &self.inner {
            GlTarget::Real(gl) => unsafe { gl.draw_arrays(mode, first, count) },
            #[cfg(feature = "gl-recording")]
            GlTarget::Recording(state) => {
                state.record(GlCall {
                    method: "draw_arrays",
                    payload: GlCallPayload::Draw {
                        vertices: count.value_as::<u32>().unwrap_or(0),
                        // `instances: 1` matches `draw_arrays_instanced`'s
                        // semantics for a single instance rather than
                        // inventing a separate non-instanced state.
                        instances: 1,
                    },
                });
            }
        }
    }

    pub(crate) unsafe fn draw_arrays_instanced(
        &self,
        mode: u32,
        first: i32,
        count: i32,
        instance_count: i32,
    ) {
        match &self.inner {
            GlTarget::Real(gl) => unsafe {
                gl.draw_arrays_instanced(mode, first, count, instance_count);
            },
            #[cfg(feature = "gl-recording")]
            GlTarget::Recording(state) => {
                state.record(GlCall {
                    method: "draw_arrays_instanced",
                    payload: GlCallPayload::Draw {
                        vertices: count.value_as::<u32>().unwrap_or(0),
                        instances: instance_count.value_as::<u32>().unwrap_or(0),
                    },
                });
            }
        }
    }

    pub(crate) unsafe fn enable(&self, parameter: u32) {
        match &self.inner {
            GlTarget::Real(gl) => unsafe { gl.enable(parameter) },
            #[cfg(feature = "gl-recording")]
            GlTarget::Recording(state) => state.record(GlCall {
                method: "enable",
                payload: GlCallPayload::None,
            }),
        }
    }

    pub(crate) unsafe fn enable_vertex_attrib_array(&self, index: u32) {
        match &self.inner {
            GlTarget::Real(gl) => unsafe { gl.enable_vertex_attrib_array(index) },
            #[cfg(feature = "gl-recording")]
            GlTarget::Recording(state) => state.record(GlCall {
                method: "enable_vertex_attrib_array",
                payload: GlCallPayload::None,
            }),
        }
    }

    pub(crate) unsafe fn framebuffer_texture_2d(
        &self,
        target: u32,
        attachment: u32,
        texture_target: u32,
        texture: Option<glow::Texture>,
        level: i32,
    ) {
        match &self.inner {
            GlTarget::Real(gl) => unsafe {
                gl.framebuffer_texture_2d(target, attachment, texture_target, texture, level);
            },
            #[cfg(feature = "gl-recording")]
            GlTarget::Recording(state) => state.record(GlCall {
                method: "framebuffer_texture_2d",
                payload: GlCallPayload::None,
            }),
        }
    }

    pub(crate) unsafe fn get_program_info_log(&self, program: glow::Program) -> String {
        match &self.inner {
            GlTarget::Real(gl) => unsafe { gl.get_program_info_log(program) },
            #[cfg(feature = "gl-recording")]
            GlTarget::Recording(state) => {
                state.record(GlCall {
                    method: "get_program_info_log",
                    payload: GlCallPayload::None,
                });
                // A plausible success value so a headless driver run does
                // not take an error path.
                String::new()
            }
        }
    }

    pub(crate) unsafe fn get_program_link_status(&self, program: glow::Program) -> bool {
        match &self.inner {
            GlTarget::Real(gl) => unsafe { gl.get_program_link_status(program) },
            #[cfg(feature = "gl-recording")]
            GlTarget::Recording(state) => {
                state.record(GlCall {
                    method: "get_program_link_status",
                    payload: GlCallPayload::None,
                });
                // A plausible success value so a headless driver run does
                // not take an error path.
                true
            }
        }
    }

    pub(crate) unsafe fn get_shader_compile_status(&self, shader: glow::Shader) -> bool {
        match &self.inner {
            GlTarget::Real(gl) => unsafe { gl.get_shader_compile_status(shader) },
            #[cfg(feature = "gl-recording")]
            GlTarget::Recording(state) => {
                state.record(GlCall {
                    method: "get_shader_compile_status",
                    payload: GlCallPayload::None,
                });
                // A plausible success value so a headless driver run does
                // not take an error path.
                true
            }
        }
    }

    pub(crate) unsafe fn get_shader_info_log(&self, shader: glow::Shader) -> String {
        match &self.inner {
            GlTarget::Real(gl) => unsafe { gl.get_shader_info_log(shader) },
            #[cfg(feature = "gl-recording")]
            GlTarget::Recording(state) => {
                state.record(GlCall {
                    method: "get_shader_info_log",
                    payload: GlCallPayload::None,
                });
                // A plausible success value so a headless driver run does
                // not take an error path.
                String::new()
            }
        }
    }

    pub(crate) unsafe fn get_uniform_location(
        &self,
        program: glow::Program,
        name: &str,
    ) -> Option<glow::UniformLocation> {
        match &self.inner {
            GlTarget::Real(gl) => unsafe { gl.get_uniform_location(program, name) },
            #[cfg(feature = "gl-recording")]
            GlTarget::Recording(state) => {
                state.record(GlCall {
                    method: "get_uniform_location",
                    payload: GlCallPayload::None,
                });
                Some(state.next_uniform_location())
            }
        }
    }

    pub(crate) unsafe fn link_program(&self, program: glow::Program) {
        match &self.inner {
            GlTarget::Real(gl) => unsafe { gl.link_program(program) },
            #[cfg(feature = "gl-recording")]
            GlTarget::Recording(state) => state.record(GlCall {
                method: "link_program",
                payload: GlCallPayload::None,
            }),
        }
    }

    pub(crate) unsafe fn pixel_store_i32(&self, parameter: u32, value: i32) {
        match &self.inner {
            GlTarget::Real(gl) => unsafe { gl.pixel_store_i32(parameter, value) },
            #[cfg(feature = "gl-recording")]
            GlTarget::Recording(state) => state.record(GlCall {
                method: "pixel_store_i32",
                payload: GlCallPayload::None,
            }),
        }
    }

    pub(crate) unsafe fn scissor(&self, x: i32, y: i32, width: i32, height: i32) {
        match &self.inner {
            GlTarget::Real(gl) => unsafe { gl.scissor(x, y, width, height) },
            #[cfg(feature = "gl-recording")]
            GlTarget::Recording(state) => state.record(GlCall {
                method: "scissor",
                payload: GlCallPayload::None,
            }),
        }
    }

    pub(crate) unsafe fn shader_source(&self, shader: glow::Shader, source: &str) {
        match &self.inner {
            GlTarget::Real(gl) => unsafe { gl.shader_source(shader, source) },
            #[cfg(feature = "gl-recording")]
            GlTarget::Recording(state) => state.record(GlCall {
                method: "shader_source",
                payload: GlCallPayload::None,
            }),
        }
    }

    // `glow::HasContext::tex_image_2d` takes nine arguments; this facade
    // method must mirror its signature exactly so the 123.4/123.5 migration
    // stays a pure type substitution. Reducing the count here would mean
    // inventing a parameter struct that every call site would then have to
    // build, which is a redesign of the call sites this facade exists to
    // leave untouched.
    #[allow(clippy::too_many_arguments)]
    pub(crate) unsafe fn tex_image_2d(
        &self,
        target: u32,
        level: i32,
        internal_format: i32,
        width: i32,
        height: i32,
        border: i32,
        format: u32,
        ty: u32,
        pixels: glow::PixelUnpackData<'_>,
    ) {
        match &self.inner {
            GlTarget::Real(gl) => unsafe {
                gl.tex_image_2d(
                    target,
                    level,
                    internal_format,
                    width,
                    height,
                    border,
                    format,
                    ty,
                    pixels,
                );
            },
            #[cfg(feature = "gl-recording")]
            GlTarget::Recording(state) => {
                // `pixels` is not `Copy`, so its byte count must be
                // computed here, in the arm that never forwards it to a
                // real driver, rather than after a `Real`-arm match that
                // would otherwise have consumed it.
                let bytes = match &pixels {
                    glow::PixelUnpackData::Slice(Some(data)) => {
                        data.len().value_as::<u64>().unwrap_or(0)
                    }
                    // `Slice(None)` uploads no pixel data (a size-only
                    // allocation call); `BufferOffset` pulls from a bound
                    // PBO, so nothing is uploaded from the CPU on this
                    // call either. Both cases upload zero CPU-side bytes.
                    glow::PixelUnpackData::Slice(None) | glow::PixelUnpackData::BufferOffset(_) => {
                        0
                    }
                };
                state.record(GlCall {
                    method: "tex_image_2d",
                    payload: GlCallPayload::Upload { bytes },
                });
            }
        }
    }

    pub(crate) unsafe fn tex_parameter_i32(&self, target: u32, parameter: u32, value: i32) {
        match &self.inner {
            GlTarget::Real(gl) => unsafe { gl.tex_parameter_i32(target, parameter, value) },
            #[cfg(feature = "gl-recording")]
            GlTarget::Recording(state) => state.record(GlCall {
                method: "tex_parameter_i32",
                payload: GlCallPayload::None,
            }),
        }
    }

    // `glow::HasContext::tex_sub_image_2d` takes nine arguments; this facade
    // method must mirror its signature exactly so the 123.4/123.5 migration
    // stays a pure type substitution. Reducing the count here would mean
    // inventing a parameter struct that every call site would then have to
    // build, which is a redesign of the call sites this facade exists to
    // leave untouched.
    #[allow(clippy::too_many_arguments)]
    pub(crate) unsafe fn tex_sub_image_2d(
        &self,
        target: u32,
        level: i32,
        x_offset: i32,
        y_offset: i32,
        width: i32,
        height: i32,
        format: u32,
        ty: u32,
        pixels: glow::PixelUnpackData<'_>,
    ) {
        match &self.inner {
            GlTarget::Real(gl) => unsafe {
                gl.tex_sub_image_2d(
                    target, level, x_offset, y_offset, width, height, format, ty, pixels,
                );
            },
            #[cfg(feature = "gl-recording")]
            GlTarget::Recording(state) => {
                // See `tex_image_2d`: `pixels` is not `Copy`, so its byte
                // count must be computed here, before the (unreachable in
                // this arm) point where the `Real` arm would consume it.
                let bytes = match &pixels {
                    glow::PixelUnpackData::Slice(Some(data)) => {
                        data.len().value_as::<u64>().unwrap_or(0)
                    }
                    // `Slice(None)` uploads no pixel data (a size-only
                    // allocation call); `BufferOffset` pulls from a bound
                    // PBO, so nothing is uploaded from the CPU on this
                    // call either. Both cases upload zero CPU-side bytes.
                    glow::PixelUnpackData::Slice(None) | glow::PixelUnpackData::BufferOffset(_) => {
                        0
                    }
                };
                state.record(GlCall {
                    method: "tex_sub_image_2d",
                    payload: GlCallPayload::Upload { bytes },
                });
            }
        }
    }

    pub(crate) unsafe fn uniform_1_f32(&self, location: Option<&glow::UniformLocation>, x: f32) {
        match &self.inner {
            GlTarget::Real(gl) => unsafe { gl.uniform_1_f32(location, x) },
            #[cfg(feature = "gl-recording")]
            GlTarget::Recording(state) => state.record(GlCall {
                method: "uniform_1_f32",
                payload: GlCallPayload::None,
            }),
        }
    }

    pub(crate) unsafe fn uniform_1_i32(&self, location: Option<&glow::UniformLocation>, x: i32) {
        match &self.inner {
            GlTarget::Real(gl) => unsafe { gl.uniform_1_i32(location, x) },
            #[cfg(feature = "gl-recording")]
            GlTarget::Recording(state) => state.record(GlCall {
                method: "uniform_1_i32",
                payload: GlCallPayload::None,
            }),
        }
    }

    pub(crate) unsafe fn uniform_2_f32(
        &self,
        location: Option<&glow::UniformLocation>,
        x: f32,
        y: f32,
    ) {
        match &self.inner {
            GlTarget::Real(gl) => unsafe { gl.uniform_2_f32(location, x, y) },
            #[cfg(feature = "gl-recording")]
            GlTarget::Recording(state) => state.record(GlCall {
                method: "uniform_2_f32",
                payload: GlCallPayload::None,
            }),
        }
    }

    pub(crate) unsafe fn use_program(&self, program: Option<glow::Program>) {
        match &self.inner {
            GlTarget::Real(gl) => unsafe { gl.use_program(program) },
            #[cfg(feature = "gl-recording")]
            GlTarget::Recording(state) => state.record(GlCall {
                method: "use_program",
                payload: GlCallPayload::None,
            }),
        }
    }

    pub(crate) unsafe fn vertex_attrib_divisor(&self, index: u32, divisor: u32) {
        match &self.inner {
            GlTarget::Real(gl) => unsafe { gl.vertex_attrib_divisor(index, divisor) },
            #[cfg(feature = "gl-recording")]
            GlTarget::Recording(state) => state.record(GlCall {
                method: "vertex_attrib_divisor",
                payload: GlCallPayload::None,
            }),
        }
    }

    pub(crate) unsafe fn vertex_attrib_pointer_f32(
        &self,
        index: u32,
        size: i32,
        data_type: u32,
        normalized: bool,
        stride: i32,
        offset: i32,
    ) {
        match &self.inner {
            GlTarget::Real(gl) => unsafe {
                gl.vertex_attrib_pointer_f32(index, size, data_type, normalized, stride, offset);
            },
            #[cfg(feature = "gl-recording")]
            GlTarget::Recording(state) => state.record(GlCall {
                method: "vertex_attrib_pointer_f32",
                payload: GlCallPayload::None,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::surface::GL_CALL_SURFACE;
    use super::Gl;

    /// Every entry in the frozen call surface has a method on `Gl`.
    ///
    /// A structural check, not a behavioural one: it proves the facade is
    /// *complete* with respect to `GL_CALL_SURFACE`, so a surface entry
    /// cannot silently lack a method. It deliberately does not check that
    /// each method records itself under the *correct* name — that needs
    /// the facade actually driven, and is subtask 123.3's behavioural
    /// round-trip suite, which calls every method against a recording
    /// `Gl` and compares the recorded set to `GL_CALL_SURFACE`.
    #[test]
    fn every_surface_method_exists_on_the_facade() {
        let source = include_str!("facade.rs");
        let missing: Vec<&str> = GL_CALL_SURFACE
            .into_iter()
            .filter(|name| !source.contains(&format!("unsafe fn {name}(")))
            .collect();
        assert!(
            missing.is_empty(),
            "surface entry(ies) {missing:?} have no method on `Gl` — the \
             facade is incomplete; every entry in `GL_CALL_SURFACE` must \
             have an identically-named method here"
        );
    }

    /// In a default build, `Gl` is `size_of`/`align_of`-identical to the
    /// bare `&glow::Context` it replaces — a structural proof of "no
    /// production overhead" in place of the benchmark 123.6 originally
    /// asked for.
    ///
    /// Without the `gl-recording` feature, `GlTarget` has exactly one
    /// variant: `Real(&glow::Context)`. Rust lays out a single-variant enum
    /// identically to its payload — there is nothing to discriminate, so no
    /// discriminant is stored — which makes `Gl` the same size and
    /// alignment as the reference alone. `size_of`/`align_of` are the
    /// observable consequence of that layout guarantee, and this test pins
    /// both.
    ///
    /// The same single-variant fact is why every one of the 49 methods'
    /// `match &self.inner { .. }` is *irrefutable*: with one variant there
    /// is nothing to branch on, so the match lowers to a field access, not
    /// a conditional. That is a property of the type the compiler must
    /// honor, not an outcome the optimiser merely tends to produce, which
    /// is why it can be asserted here rather than measured.
    ///
    /// This replaces a benchmark, not just supplements one: the plan asked
    /// for the `Real` arm to be benchmarked against a direct
    /// `glow::Context` call, but that comparison needs a live GL context —
    /// a display server and a driver — which is exactly the infrastructure
    /// Phase 2 builds (123.10's offscreen context, 123.11's pixel
    /// harness). Benchmarking the `Real` arm is re-scoped as **123.6b**,
    /// gated on 123.11. The compensating advantage of doing it this way now:
    /// unlike a Criterion benchmark, this check runs in the ordinary
    /// `cargo test` matrix on all four CI platforms, and it cannot quietly
    /// stop being checked the way a benchmark not wired into CI can.
    ///
    /// This test does **not** claim the `Recording` arm is free — it is
    /// not, and it is not supposed to be. It claims only that a build
    /// without `gl-recording` pays nothing for the facade existing.
    #[test]
    #[cfg(not(feature = "gl-recording"))]
    fn default_build_gl_is_a_zero_overhead_newtype() {
        let gl_size = size_of::<Gl<'_>>();
        let gl_align = align_of::<Gl<'_>>();
        let reference_size = size_of::<&glow::Context>();
        let reference_align = align_of::<&glow::Context>();
        assert_eq!(
            gl_size, reference_size,
            "Gl<'_> should be exactly as large as &glow::Context in a \
             default build — GlTarget has one variant and stores no \
             discriminant"
        );
        assert_eq!(
            gl_align, reference_align,
            "Gl<'_> should have the same alignment as &glow::Context in a \
             default build"
        );
    }

    /// Under `gl-recording`, `Gl` is strictly larger than a bare
    /// `&glow::Context` — the counterpart to
    /// `default_build_gl_is_a_zero_overhead_newtype` that makes that test's
    /// assertion meaningful rather than vacuous.
    ///
    /// With the feature on, `GlTarget` genuinely has two variants
    /// (`Real(&glow::Context)` and `Recording(RecordingState)`), so the
    /// enum must carry a discriminant plus room for the larger payload
    /// inline. It is necessarily bigger. Read together, the two tests show
    /// the cost of the facade appears exactly when `gl-recording` is
    /// enabled and nowhere else — which is also why the feature is off by
    /// default and must stay that way.
    #[test]
    #[cfg(feature = "gl-recording")]
    fn recording_build_gl_carries_its_state() {
        let gl_size = size_of::<Gl<'_>>();
        let reference_size = size_of::<&glow::Context>();
        assert!(
            gl_size > reference_size,
            "Gl<'_> ({gl_size} bytes) should be larger than \
             &glow::Context ({reference_size} bytes) under gl-recording — \
             GlTarget has two variants and must carry a discriminant plus \
             the inline RecordingState payload"
        );
    }
}
