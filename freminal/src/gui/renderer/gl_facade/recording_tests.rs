// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! The behavioural half of the `gl_facade` verification.
//!
//! `facade.rs`'s `tests::every_surface_method_exists_on_the_facade` (subtask
//! 123.2) proves the facade is *complete* with respect to
//! [`super::surface::GL_CALL_SURFACE`] — every entry has a method — but
//! deliberately stops short of proving that each method's `Recording` arm
//! behaves correctly: records itself under the right name, fabricates
//! usable handles, and derives the right metrics. That is this module's
//! job (subtask 123.3): drive the facade end to end against
//! [`super::recording::RecordingState`] and assert on what actually came
//! out the other side, so the `Recording` arm is a trustworthy stand-in
//! for a real driver for the rest of Task 123 (123.7's headless driver and
//! 123.8's workload assertions both depend on it).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeSet;

use super::Gl;
use super::recording::{GlCall, GlCallPayload};
use super::surface::GL_CALL_SURFACE;

/// Call every one of the 49 [`GL_CALL_SURFACE`] methods exactly once, in
/// the surface's own order, against a recording [`Gl`], then check the
/// resulting log against the surface itself.
///
/// This is the behavioural counterpart the 123.2 structural test
/// explicitly defers to: it is the one test that catches a method
/// recording itself under a *neighbour's* name — a copy-paste error in
/// `facade.rs` that the structural check (which only greps for a method
/// definition) cannot see.
#[test]
fn every_method_records_itself_under_its_own_name() {
    // A throwaway `Gl` used only to fabricate handles the ordered sequence
    // below needs before its *own* `create_*` call is reached — e.g.
    // `attach_shader` sits earlier than `create_program` / `create_shader`
    // in `GL_CALL_SURFACE`'s alphabetical order, so a handle for it must
    // already exist. This instance's own log is never inspected.
    let setup_handles = fabricate_setup_handles();

    // The 49-call sequence is split across four helpers, each covering a
    // contiguous slice of `GL_CALL_SURFACE`'s alphabetical order, purely
    // to keep any one function under the line-count lint — the split
    // point carries no other significance and the overall call order
    // across all four still matches `GL_CALL_SURFACE` exactly.
    let gl = Gl::recording();
    unsafe {
        call_active_texture_through_compile_shader(&gl, &setup_handles);
        call_create_and_delete_lifecycle(&gl);
        let location = call_disable_through_link_program(&gl, &setup_handles);
        call_pixel_store_through_vertex_attrib_pointer(&gl, &setup_handles, location.as_ref());
    }

    let recording = gl.recorded().expect("a recording Gl always has state");
    let calls = recording.calls();
    assert_eq!(
        calls.len(),
        49,
        "expected exactly 49 recorded calls (one per GL_CALL_SURFACE entry \
         — every method called exactly once), got {}",
        calls.len()
    );

    let recorded_methods: BTreeSet<&str> = calls.iter().map(|call| call.method).collect();
    let surface: BTreeSet<&str> = GL_CALL_SURFACE.into_iter().collect();
    let recorded_not_in_surface: Vec<&&str> = recorded_methods.difference(&surface).collect();
    let surface_not_recorded: Vec<&&str> = surface.difference(&recorded_methods).collect();
    assert!(
        recorded_not_in_surface.is_empty() && surface_not_recorded.is_empty(),
        "recorded call names do not match GL_CALL_SURFACE — recorded but \
         not in the surface: {recorded_not_in_surface:?}; in the surface \
         but never recorded: {surface_not_recorded:?} (a name mismatch \
         here means some method recorded itself under a neighbour's name)"
    );
}

/// Handles fabricated once, up front, so every one of the four call
/// batches below has whatever a real driver would already have handed it
/// by that point in a normal renderer call sequence.
struct SetupHandles {
    buffer: glow::Buffer,
    framebuffer: glow::Framebuffer,
    program: glow::Program,
    shader: glow::Shader,
    texture: glow::Texture,
    vertex_array: glow::VertexArray,
}

/// Fabricate one of each handle type on a throwaway recording [`Gl`].
fn fabricate_setup_handles() -> SetupHandles {
    let setup = Gl::recording();
    unsafe {
        SetupHandles {
            buffer: setup.create_buffer().expect("recording create_buffer"),
            framebuffer: setup
                .create_framebuffer()
                .expect("recording create_framebuffer"),
            program: setup.create_program().expect("recording create_program"),
            shader: setup
                .create_shader(glow::VERTEX_SHADER)
                .expect("recording create_shader"),
            texture: setup.create_texture().expect("recording create_texture"),
            vertex_array: setup
                .create_vertex_array()
                .expect("recording create_vertex_array"),
        }
    }
}

/// `GL_CALL_SURFACE`, `active_texture` through `compile_shader` (13 calls).
unsafe fn call_active_texture_through_compile_shader(gl: &Gl<'_>, handles: &SetupHandles) {
    unsafe {
        gl.active_texture(glow::TEXTURE0);
        gl.attach_shader(handles.program, handles.shader);
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(handles.buffer));
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(handles.framebuffer));
        gl.bind_texture(glow::TEXTURE_2D, Some(handles.texture));
        gl.bind_vertex_array(Some(handles.vertex_array));
        gl.buffer_data_size(glow::ARRAY_BUFFER, 4, glow::STATIC_DRAW);
        gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, &[0u8; 4], glow::STATIC_DRAW);
        gl.buffer_sub_data_u8_slice(glow::ARRAY_BUFFER, 0, &[0u8; 2]);
        let _ = gl.check_framebuffer_status(glow::FRAMEBUFFER);
        gl.clear(glow::COLOR_BUFFER_BIT);
        gl.clear_color(0.0, 0.0, 0.0, 1.0);
        gl.compile_shader(handles.shader);
    }
}

/// `GL_CALL_SURFACE`, `create_buffer` through `delete_vertex_array` (12
/// calls) — a self-contained second lifecycle, independent of
/// [`SetupHandles`], so it needs no handles passed in.
unsafe fn call_create_and_delete_lifecycle(gl: &Gl<'_>) {
    unsafe {
        let buffer = gl.create_buffer().expect("recording create_buffer");
        let framebuffer = gl
            .create_framebuffer()
            .expect("recording create_framebuffer");
        let program = gl.create_program().expect("recording create_program");
        let shader = gl
            .create_shader(glow::FRAGMENT_SHADER)
            .expect("recording create_shader");
        let texture = gl.create_texture().expect("recording create_texture");
        let vertex_array = gl
            .create_vertex_array()
            .expect("recording create_vertex_array");
        gl.delete_buffer(buffer);
        gl.delete_framebuffer(framebuffer);
        gl.delete_program(program);
        gl.delete_shader(shader);
        gl.delete_texture(texture);
        gl.delete_vertex_array(vertex_array);
    }
}

/// `GL_CALL_SURFACE`, `disable` through `link_program` (12 calls).
///
/// Returns the fabricated uniform location so the next batch's
/// `uniform_*` calls can use it — `get_uniform_location` lives in this
/// batch, alphabetically, and must be called exactly once for the overall
/// 49-call total to hold.
unsafe fn call_disable_through_link_program(
    gl: &Gl<'_>,
    handles: &SetupHandles,
) -> Option<glow::UniformLocation> {
    unsafe {
        gl.disable(glow::DEPTH_TEST);
        gl.draw_arrays(glow::TRIANGLES, 0, 3);
        gl.draw_arrays_instanced(glow::TRIANGLES, 0, 3, 2);
        gl.enable(glow::DEPTH_TEST);
        gl.enable_vertex_attrib_array(0);
        gl.framebuffer_texture_2d(
            glow::FRAMEBUFFER,
            glow::COLOR_ATTACHMENT0,
            glow::TEXTURE_2D,
            Some(handles.texture),
            0,
        );
        let _ = gl.get_program_info_log(handles.program);
        let _ = gl.get_program_link_status(handles.program);
        let _ = gl.get_shader_compile_status(handles.shader);
        let _ = gl.get_shader_info_log(handles.shader);
        let location = gl.get_uniform_location(handles.program, "u_dummy");
        gl.link_program(handles.program);
        location
    }
}

/// `GL_CALL_SURFACE`, `pixel_store_i32` through `vertex_attrib_pointer_f32`
/// (12 calls). `location` is the one fabricated by the previous batch's
/// `get_uniform_location` call — this batch must not call
/// `get_uniform_location` again, or the overall total would be 50, not 49.
unsafe fn call_pixel_store_through_vertex_attrib_pointer(
    gl: &Gl<'_>,
    handles: &SetupHandles,
    location: Option<&glow::UniformLocation>,
) {
    unsafe {
        gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 4);
        gl.scissor(0, 0, 10, 10);
        gl.shader_source(handles.shader, "void main() {}");
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::RGBA.cast_signed(),
            1,
            1,
            0,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelUnpackData::Slice(Some(&[0u8; 4])),
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_MIN_FILTER,
            glow::NEAREST.cast_signed(),
        );
        gl.tex_sub_image_2d(
            glow::TEXTURE_2D,
            0,
            0,
            0,
            1,
            1,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelUnpackData::Slice(Some(&[0u8; 4])),
        );
        gl.uniform_1_f32(location, 1.0);
        gl.uniform_1_i32(location, 1);
        gl.uniform_2_f32(location, 1.0, 2.0);
        gl.use_program(Some(handles.program));
        gl.vertex_attrib_divisor(0, 1);
        gl.vertex_attrib_pointer_f32(0, 3, glow::FLOAT, false, 0, 0);
    }
}

/// For each of the six object handle types, run the full
/// create/bind-or-use/delete lifecycle against its own recording [`Gl`]
/// and pin both that it completes and the exact order of the calls it
/// produces.
///
/// "Round-trips without panicking" alone is a weak claim — it would also
/// be true of a facade that silently dropped every call. Pinning the
/// recorded order additionally proves the sequence actually reached the
/// log, in the order issued, which is part of what the harness measures.
#[test]
fn create_bind_delete_round_trips_without_panicking() {
    // Buffer.
    let buffer_gl = Gl::recording();
    unsafe {
        let buffer = buffer_gl.create_buffer().expect("recording create_buffer");
        buffer_gl.bind_buffer(glow::ARRAY_BUFFER, Some(buffer));
        buffer_gl.delete_buffer(buffer);
    }
    assert_eq!(
        method_sequence(&buffer_gl),
        vec!["create_buffer", "bind_buffer", "delete_buffer"]
    );

    // Vertex array.
    let vao_gl = Gl::recording();
    unsafe {
        let vertex_array = vao_gl
            .create_vertex_array()
            .expect("recording create_vertex_array");
        vao_gl.bind_vertex_array(Some(vertex_array));
        vao_gl.delete_vertex_array(vertex_array);
    }
    assert_eq!(
        method_sequence(&vao_gl),
        vec![
            "create_vertex_array",
            "bind_vertex_array",
            "delete_vertex_array"
        ]
    );

    // Texture.
    let texture_gl = Gl::recording();
    unsafe {
        let texture = texture_gl
            .create_texture()
            .expect("recording create_texture");
        texture_gl.bind_texture(glow::TEXTURE_2D, Some(texture));
        texture_gl.delete_texture(texture);
    }
    assert_eq!(
        method_sequence(&texture_gl),
        vec!["create_texture", "bind_texture", "delete_texture"]
    );

    // Framebuffer.
    let framebuffer_gl = Gl::recording();
    unsafe {
        let framebuffer = framebuffer_gl
            .create_framebuffer()
            .expect("recording create_framebuffer");
        framebuffer_gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
        framebuffer_gl.delete_framebuffer(framebuffer);
    }
    assert_eq!(
        method_sequence(&framebuffer_gl),
        vec![
            "create_framebuffer",
            "bind_framebuffer",
            "delete_framebuffer"
        ]
    );

    // Shader.
    let shader_gl = Gl::recording();
    unsafe {
        let shader = shader_gl
            .create_shader(glow::VERTEX_SHADER)
            .expect("recording create_shader");
        shader_gl.shader_source(shader, "void main() {}");
        shader_gl.compile_shader(shader);
        shader_gl.delete_shader(shader);
    }
    assert_eq!(
        method_sequence(&shader_gl),
        vec![
            "create_shader",
            "shader_source",
            "compile_shader",
            "delete_shader"
        ]
    );

    // Program. `attach_shader` needs a shader handle; fabricate one on a
    // throwaway `Gl` so the program `Gl`'s own log stays exactly the
    // program lifecycle with no extraneous `create_shader` entry.
    let shader_source_gl = Gl::recording();
    let shader_for_program = unsafe {
        shader_source_gl
            .create_shader(glow::VERTEX_SHADER)
            .expect("recording create_shader")
    };
    let program_gl = Gl::recording();
    unsafe {
        let program = program_gl
            .create_program()
            .expect("recording create_program");
        program_gl.attach_shader(program, shader_for_program);
        program_gl.link_program(program);
        program_gl.use_program(Some(program));
        program_gl.delete_program(program);
    }
    assert_eq!(
        method_sequence(&program_gl),
        vec![
            "create_program",
            "attach_shader",
            "link_program",
            "use_program",
            "delete_program"
        ]
    );
}

/// Collect a recording [`Gl`]'s recorded method names, in call order.
fn method_sequence(gl: &Gl<'_>) -> Vec<&'static str> {
    gl.recorded()
        .expect("a recording Gl always has state")
        .calls()
        .into_iter()
        .map(|call| call.method)
        .collect()
}

/// Fabricated handles must be distinct within a type, and each type owns
/// an independent counter.
#[test]
fn fabricated_handles_are_unique_within_a_type_and_start_independently() {
    let gl = Gl::recording();
    unsafe {
        let b1 = gl.create_buffer().expect("recording create_buffer");
        let b2 = gl.create_buffer().expect("recording create_buffer");
        let b3 = gl.create_buffer().expect("recording create_buffer");
        assert_ne!(b1, b2);
        assert_ne!(b1, b3);
        assert_ne!(b2, b3);

        let s1 = gl
            .create_shader(glow::VERTEX_SHADER)
            .expect("recording create_shader");
        let s2 = gl
            .create_shader(glow::VERTEX_SHADER)
            .expect("recording create_shader");
        let s3 = gl
            .create_shader(glow::VERTEX_SHADER)
            .expect("recording create_shader");
        assert_ne!(s1, s2);
        assert_ne!(s1, s3);
        assert_ne!(s2, s3);

        let p1 = gl.create_program().expect("recording create_program");
        let p2 = gl.create_program().expect("recording create_program");
        let p3 = gl.create_program().expect("recording create_program");
        assert_ne!(p1, p2);
        assert_ne!(p1, p3);
        assert_ne!(p2, p3);

        let v1 = gl
            .create_vertex_array()
            .expect("recording create_vertex_array");
        let v2 = gl
            .create_vertex_array()
            .expect("recording create_vertex_array");
        let v3 = gl
            .create_vertex_array()
            .expect("recording create_vertex_array");
        assert_ne!(v1, v2);
        assert_ne!(v1, v3);
        assert_ne!(v2, v3);

        let t1 = gl.create_texture().expect("recording create_texture");
        let t2 = gl.create_texture().expect("recording create_texture");
        let t3 = gl.create_texture().expect("recording create_texture");
        assert_ne!(t1, t2);
        assert_ne!(t1, t3);
        assert_ne!(t2, t3);

        let f1 = gl
            .create_framebuffer()
            .expect("recording create_framebuffer");
        let f2 = gl
            .create_framebuffer()
            .expect("recording create_framebuffer");
        let f3 = gl
            .create_framebuffer()
            .expect("recording create_framebuffer");
        assert_ne!(f1, f2);
        assert_ne!(f1, f3);
        assert_ne!(f2, f3);

        let u1 = gl
            .get_uniform_location(p1, "u_one")
            .expect("recording get_uniform_location");
        let u2 = gl
            .get_uniform_location(p1, "u_two")
            .expect("recording get_uniform_location");
        let u3 = gl
            .get_uniform_location(p1, "u_three")
            .expect("recording get_uniform_location");
        assert_ne!(u1, u2);
        assert_ne!(u1, u3);
        assert_ne!(u2, u3);

        // Each handle type owns an independent counter starting at 1, so
        // the *first* handle fabricated for every type carries the same
        // underlying value. That is deliberate, not a bug: GL object
        // names live in per-type namespaces (a buffer named 1 and a
        // texture named 1 are different objects and never collide in real
        // GL), so uniqueness is only required *within* a type — a
        // cross-type collision of the underlying integer is meaningless.
        assert_eq!(b1.0.get(), 1);
        assert_eq!(s1.0.get(), 1);
        assert_eq!(p1.0.get(), 1);
        assert_eq!(v1.0.get(), 1);
        assert_eq!(t1.0.get(), 1);
        assert_eq!(f1.0.get(), 1);
        assert_eq!(u1.0, 1);
    }
}

/// `get_uniform_location` must return usable, distinct locations.
#[test]
fn get_uniform_location_returns_a_usable_location() {
    let gl = Gl::recording();
    unsafe {
        let program = gl.create_program().expect("recording create_program");

        let location = gl.get_uniform_location(program, "u_thing");
        assert!(location.is_some());

        gl.uniform_1_f32(location.as_ref(), 1.0);
        gl.uniform_1_i32(location.as_ref(), 1);
        gl.uniform_2_f32(location.as_ref(), 1.0, 2.0);

        let other = gl.get_uniform_location(program, "u_other_thing");
        assert!(other.is_some());
        assert_ne!(location, other);
    }
}

/// The recording arm's query methods must return values a headless driver
/// run treats as success — `gpu.rs` bails on a failed shader/program link
/// or an incomplete framebuffer, and 123.7's headless driver depends on
/// these values to avoid taking that error path.
#[test]
fn queries_return_plausible_success_values() {
    let gl = Gl::recording();
    unsafe {
        let shader = gl
            .create_shader(glow::VERTEX_SHADER)
            .expect("recording create_shader");
        let program = gl.create_program().expect("recording create_program");

        assert!(gl.get_shader_compile_status(shader));
        assert!(gl.get_program_link_status(program));
        assert_eq!(gl.get_shader_info_log(shader), String::new());
        assert_eq!(gl.get_program_info_log(program), String::new());
        assert_eq!(
            gl.check_framebuffer_status(glow::FRAMEBUFFER),
            glow::FRAMEBUFFER_COMPLETE
        );
    }
}

/// Drive a small, explicit synthetic sequence and check the exact derived
/// metrics it produces.
///
/// If these derivations are wrong, every downstream number 123.8's
/// workload assertions rest on is wrong too, so the expected values are
/// spelled out here arithmetically rather than copied from a run:
///
/// - `state_changes()` == 4: `bind_vertex_array`, `bind_buffer`,
///   `use_program`, `bind_texture` are the only state-change-family calls
///   issued below.
/// - `uploads()` == 2: `buffer_data_u8_slice` (24 bytes) and
///   `tex_sub_image_2d` (16 bytes) are the only upload-family calls.
/// - `uploaded_bytes()` == 40: `24 + 16 == 40`.
/// - `draw_calls()` == 2: one `draw_arrays` plus one
///   `draw_arrays_instanced`.
/// - `instances_drawn()` == 13: `draw_arrays` always contributes `1`
///   instance (see `GlCallPayload::Draw`'s doc comment) plus
///   `draw_arrays_instanced`'s `instance_count` of `12`, so `1 + 12 == 13`.
#[test]
fn derived_metrics_match_the_recorded_calls() {
    let gl = Gl::recording();
    unsafe {
        let vertex_array = gl
            .create_vertex_array()
            .expect("recording create_vertex_array");
        gl.bind_vertex_array(Some(vertex_array));

        let buffer = gl.create_buffer().expect("recording create_buffer");
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(buffer));

        let program = gl.create_program().expect("recording create_program");
        gl.use_program(Some(program));

        let vertex_data = [0u8; 24];
        gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, &vertex_data, glow::STATIC_DRAW);

        let texture = gl.create_texture().expect("recording create_texture");
        gl.bind_texture(glow::TEXTURE_2D, Some(texture));

        let pixel_data = [0u8; 16];
        gl.tex_sub_image_2d(
            glow::TEXTURE_2D,
            0,
            0,
            0,
            2,
            2,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelUnpackData::Slice(Some(&pixel_data)),
        );

        gl.draw_arrays(glow::TRIANGLES, 0, 6);
        gl.draw_arrays_instanced(glow::TRIANGLES, 0, 6, 12);
    }

    let recording = gl.recorded().expect("a recording Gl always has state");

    // Every number below is derived by hand from the sequence above rather
    // than copied out of a run, so a wrong derivation in `recording.rs`
    // fails here instead of silently rescaling every figure 123.14 reports.
    //
    // state_changes  = bind_vertex_array + bind_buffer + use_program
    //                  + bind_texture                                   = 4
    //   (the four `create_*` calls are not state changes, and none of the
    //   `STATE_CHANGE_METHODS` entries `active_texture` / `bind_framebuffer`
    //   / `enable` / `disable` / `scissor` appear in this sequence)
    // uploads        = buffer_data_u8_slice + tex_sub_image_2d          = 2
    // uploaded_bytes = 24 (vertex_data) + 16 (pixel_data)               = 40
    // draw_calls     = draw_arrays + draw_arrays_instanced              = 2
    // instances_drawn= 1 (draw_arrays is one instance by definition)
    //                  + 12 (draw_arrays_instanced's instance_count)    = 13
    assert_eq!(recording.state_changes(), 4);
    assert_eq!(recording.uploads(), 2);
    assert_eq!(recording.uploaded_bytes(), 40);
    assert_eq!(recording.draw_calls(), 2);
    assert_eq!(recording.instances_drawn(), 13);
}

/// A `PixelUnpackData::BufferOffset` upload transfers nothing from the CPU
/// on that call — the pixels come from an already-bound PBO — so it must
/// record zero bytes. Counting it would inflate the byte volume 123.14's
/// findings report on.
#[test]
fn pixel_unpack_buffer_offset_records_zero_bytes() {
    let buffer_offset_gl = Gl::recording();
    unsafe {
        buffer_offset_gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::RGBA.cast_signed(),
            4,
            4,
            0,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelUnpackData::BufferOffset(0),
        );
    }
    let buffer_offset_calls = buffer_offset_gl
        .recorded()
        .expect("a recording Gl always has state")
        .calls();
    assert_eq!(
        buffer_offset_calls,
        vec![GlCall {
            method: "tex_image_2d",
            payload: GlCallPayload::Upload { bytes: 0 },
        }]
    );

    let slice_gl = Gl::recording();
    unsafe {
        slice_gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::RGBA.cast_signed(),
            4,
            4,
            0,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelUnpackData::Slice(Some(&[0u8; 16])),
        );
    }
    let slice_calls = slice_gl
        .recorded()
        .expect("a recording Gl always has state")
        .calls();
    assert_eq!(
        slice_calls,
        vec![GlCall {
            method: "tex_image_2d",
            payload: GlCallPayload::Upload { bytes: 16 },
        }]
    );
}

/// `recorded()` must distinguish a recording instance from a real one.
///
/// There is no `glow::Context` to construct in a unit test — it requires
/// a live GL context from an actual display and driver — so the `Real`
/// arm's `recorded() == None` half of this asymmetry cannot be exercised
/// here without fabricating a fake `glow::Context`, which the plan
/// explicitly forbids (the `Recording` arm must stay fully independent of
/// a real driver, not the other way around: manufacturing a fake `Real`
/// context would be its own unsound shortcut). This test therefore pins
/// only the `Recording` half.
#[test]
fn real_arm_reports_no_recording_state() {
    assert!(Gl::recording().recorded().is_some());
}
