// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Real-GL proof that [`TerminalRenderer::destroy`] and the retire queue
//! actually delete GL objects (Task 125.C2).
//!
//! The recording-facade tests in `renderer::retire` prove the right
//! `delete_*` *calls* are issued; this proves the driver agrees -- every
//! object the renderer created stops being a valid GL name once it is
//! drained. Needs an offscreen context (Mesa + Xvfb), so it follows the
//! pixel harness's skip rules and runs under
//! `xvfb-run -a cargo test -p freminal --features gl-pixel`. It lives under
//! `gpu/` because it must read `TerminalRenderer`'s private handle fields.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use freminal_windowing::gl_context_offscreen::OffscreenGl;
use glow::HasContext;

use super::TerminalRenderer;
use crate::gui::renderer::gl_facade::Gl;
use crate::gui::renderer::retire::GlRetireQueue;

/// A GL object name the renderer created, tagged with its kind so the right
/// `is_*` query can be used on it.
enum Name {
    Program(glow::Program),
    Vao(glow::VertexArray),
    Buffer(glow::Buffer),
    Texture(glow::Texture),
}

/// `glow` has no `is_vertex_array`, so probe by binding: binding a name that
/// is not an existing VAO raises `INVALID_OPERATION`.
fn vao_exists(gl: &glow::Context, vao: glow::VertexArray) -> bool {
    unsafe {
        while gl.get_error() != glow::NO_ERROR {}
        gl.bind_vertex_array(Some(vao));
        let exists = gl.get_error() == glow::NO_ERROR;
        gl.bind_vertex_array(None);
        while gl.get_error() != glow::NO_ERROR {}
        exists
    }
}

impl Name {
    /// Make the name a real GL object.
    ///
    /// `glGenBuffers` / `glGenTextures` only *reserve* a name; the object
    /// exists (and `glIs*` reports true) after its first bind. The renderer
    /// double-buffers, so some of its names are still unbound reservations
    /// right after `init`. Binding each once makes "deleted" distinguishable
    /// from "never created" for every name.
    fn materialize(&self, gl: &glow::Context) {
        unsafe {
            match self {
                Self::Program(_) | Self::Vao(_) => {}
                Self::Buffer(n) => {
                    gl.bind_buffer(glow::ARRAY_BUFFER, Some(*n));
                    gl.bind_buffer(glow::ARRAY_BUFFER, None);
                }
                Self::Texture(n) => {
                    gl.bind_texture(glow::TEXTURE_2D, Some(*n));
                    gl.bind_texture(glow::TEXTURE_2D, None);
                }
            }
        }
    }

    fn is_live(&self, gl: &glow::Context) -> bool {
        unsafe {
            match self {
                Self::Program(n) => gl.is_program(*n),
                Self::Vao(n) => vao_exists(gl, *n),
                Self::Buffer(n) => gl.is_buffer(*n),
                Self::Texture(n) => gl.is_texture(*n),
            }
        }
    }
}

/// Every program, VAO, buffer and texture `renderer` currently owns.
fn owned_names(r: &TerminalRenderer) -> Vec<Name> {
    let mut names = Vec::new();
    let programs = [
        r.bg_inst_program,
        r.deco_program,
        r.fg_program,
        r.img_program,
        r.bg_img_program,
    ];
    names.extend(programs.into_iter().flatten().map(Name::Program));
    let vaos = [r.bg_inst_vao, r.deco_vao, r.fg_vao, r.img_vao, r.bg_img_vao];
    names.extend(vaos.into_iter().flatten().map(Name::Vao));
    let mut buffers = vec![r.bg_unit_quad_vbo, r.bg_img_vbo];
    buffers.extend(r.bg_inst_vbo);
    buffers.extend(r.deco_vbo);
    buffers.extend(r.fg_vbo);
    buffers.extend(r.img_vbo);
    names.extend(buffers.into_iter().flatten().map(Name::Buffer));
    names.extend(r.atlas_texture.map(Name::Texture));
    names
}

/// Create an offscreen context, or skip (return `None`) when none exists --
/// unless this environment promised one via `FREMINAL_REQUIRE_GL`.
fn context_or_skip() -> Option<OffscreenGl> {
    match OffscreenGl::new(64, 64) {
        Ok(off) => Some(off),
        Err(e) => {
            assert!(
                !std::env::var("FREMINAL_REQUIRE_GL").is_ok_and(|v| v != "0" && !v.is_empty()),
                "no offscreen GL context ({e}) despite FREMINAL_REQUIRE_GL being set"
            );
            eprintln!("SKIP destroy_gl_tests: no GL context ({e})");
            None
        }
    }
}

#[test]
fn pixel_harness_destroy_deletes_every_object_the_renderer_created() {
    let Some(off) = context_or_skip() else {
        return;
    };
    let gl = Gl::real(off.gl());
    let mut renderer = TerminalRenderer::new();
    renderer.init(&gl).expect("real GL init");

    let names = owned_names(&renderer);
    for n in &names {
        n.materialize(off.gl());
    }
    assert!(
        names.len() >= 20,
        "expected a full renderer's worth of objects, found {}",
        names.len()
    );
    assert!(
        names.iter().all(|n| n.is_live(off.gl())),
        "every object is a valid GL name after init"
    );

    renderer.destroy(&gl);

    assert!(
        names.iter().all(|n| !n.is_live(off.gl())),
        "no object survives destroy()"
    );
    assert!(!renderer.initialized());
    assert_eq!(unsafe { off.gl().get_error() }, glow::NO_ERROR);
}

#[test]
fn pixel_harness_retire_queue_drain_deletes_the_retired_renderers_objects() {
    let Some(off) = context_or_skip() else {
        return;
    };
    let gl = Gl::real(off.gl());
    let mut renderer = TerminalRenderer::new();
    renderer.init(&gl).expect("real GL init");
    let names = owned_names(&renderer);
    for n in &names {
        n.materialize(off.gl());
    }
    assert!(
        names.iter().all(|n| n.is_live(off.gl())),
        "every object is a valid GL name before retiring"
    );

    let queue = GlRetireQueue::new();
    queue.retire(renderer);
    assert!(
        names.iter().all(|n| n.is_live(off.gl())),
        "retiring alone deletes nothing: it has no GL context"
    );

    assert_eq!(queue.drain(&gl), 1);
    assert!(
        names.iter().all(|n| !n.is_live(off.gl())),
        "drain deleted every object of the retired renderer"
    );
    assert_eq!(queue.drain(&gl), 0, "a second drain finds nothing");
    assert_eq!(unsafe { off.gl().get_error() }, glow::NO_ERROR);
}
