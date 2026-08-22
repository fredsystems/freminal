// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! The frozen GL call surface.
//!
//! Task 123, `PLAN_123_GL_MEASUREMENT_HARNESS.md`, subtask 123.1 — the
//! audited enumeration of every `glow::HasContext` method the `freminal`
//! crate calls — plus the test-time guard that keeps that enumeration
//! honest until the facade migration (123.2–123.5) lands.

/// Every `glow::HasContext` method freminal calls, sorted and de-duplicated.
///
/// This is the frozen surface `PLAN_123` subtask 123.1 audited. The plan
/// document's own original enumeration (written before this audit) said
/// **47** and omitted two call sites: `create_program` (`gpu.rs:1614`) and
/// `create_shader` (`gpu.rs:1643`), both written as multi-line method
/// chains (`gl` on one line, `.create_program()` / `.create_shader(..)` on
/// the next) that the plan's grep-based audit missed. The audited figure
/// enumerated here is **49**.
///
/// Adding a 50th entry to this array means a new GL call was introduced
/// somewhere in the crate — that is a deliberate act and should be
/// reviewed, not waved through. There is no automated check that this
/// array itself stays exhaustive against future call sites (that guard
/// would require parsing method-call syntax, not just scanning for an
/// import); [`tests::has_context_use_sites_match_allowlist`] instead
/// guards the narrower, checkable invariant that no *new file* starts
/// calling `glow::HasContext` methods outside the facade without going
/// through [`NOT_YET_MIGRATED`](tests::NOT_YET_MIGRATED).
pub const GL_CALL_SURFACE: [&str; 49] = [
    "active_texture",
    "attach_shader",
    "bind_buffer",
    "bind_framebuffer",
    "bind_texture",
    "bind_vertex_array",
    "buffer_data_size",
    "buffer_data_u8_slice",
    "buffer_sub_data_u8_slice",
    "check_framebuffer_status",
    "clear",
    "clear_color",
    "compile_shader",
    "create_buffer",
    "create_framebuffer",
    "create_program",
    "create_shader",
    "create_texture",
    "create_vertex_array",
    "delete_buffer",
    "delete_framebuffer",
    "delete_program",
    "delete_shader",
    "delete_texture",
    "delete_vertex_array",
    "disable",
    "draw_arrays",
    "draw_arrays_instanced",
    "enable",
    "enable_vertex_attrib_array",
    "framebuffer_texture_2d",
    "get_program_info_log",
    "get_program_link_status",
    "get_shader_compile_status",
    "get_shader_info_log",
    "get_uniform_location",
    "link_program",
    "pixel_store_i32",
    "scissor",
    "shader_source",
    "tex_image_2d",
    "tex_parameter_i32",
    "tex_sub_image_2d",
    "uniform_1_f32",
    "uniform_1_i32",
    "uniform_2_f32",
    "use_program",
    "vertex_attrib_divisor",
    "vertex_attrib_pointer_f32",
];

/// The draw-call subset of [`GL_CALL_SURFACE`].
///
/// Per the plan: draw calls are `draw_arrays` + `draw_arrays_instanced`.
/// Consumed by 123.8's workload assertions (draw-call count per frame).
pub const DRAW_CALL_METHODS: [&str; 2] = ["draw_arrays", "draw_arrays_instanced"];

/// The state-change subset of [`GL_CALL_SURFACE`].
///
/// Per the plan: state changes are the `bind_*` family plus `use_program` /
/// `enable` / `disable` / `scissor`. `active_texture` is included beyond
/// the plan's literal wording because it is a texture-unit selector — a
/// bind-family state change — and omitting it would undercount. Consumed
/// by 123.8's workload assertions (state-change count per frame).
pub const STATE_CHANGE_METHODS: [&str; 9] = [
    "active_texture",
    "bind_buffer",
    "bind_framebuffer",
    "bind_texture",
    "bind_vertex_array",
    "disable",
    "enable",
    "scissor",
    "use_program",
];

/// The upload subset of [`GL_CALL_SURFACE`].
///
/// Per the plan: uploads are `buffer_data_*` / `buffer_sub_data_u8_slice` /
/// `tex_image_2d` / `tex_sub_image_2d`. Consumed by 123.8's workload
/// assertions (upload count/bytes per frame).
pub const UPLOAD_METHODS: [&str; 5] = [
    "buffer_data_size",
    "buffer_data_u8_slice",
    "buffer_sub_data_u8_slice",
    "tex_image_2d",
    "tex_sub_image_2d",
];

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::{DRAW_CALL_METHODS, GL_CALL_SURFACE, STATE_CHANGE_METHODS, UPLOAD_METHODS};
    use std::collections::BTreeSet;
    use std::path::{Path, PathBuf};

    /// Files that still call `glow::HasContext` methods directly, outside
    /// the [`super`] facade.
    ///
    /// This is **empty, and empty is the intended permanent state**: every
    /// `glow::HasContext` call in the `freminal` crate goes through
    /// [`gl_facade::Gl`](super::Gl). Adding an entry back means someone is
    /// calling `glow::Context` directly again; that is what this guard
    /// exists to stop, and it should be reviewed rather than accommodated.
    ///
    /// This guard deliberately covers the **whole `freminal` crate**, not
    /// just `src/gui/renderer/`, per the maintainer decision of
    /// 2026-08-21: `widget.rs` and `app_impl.rs` interleave their own raw
    /// `bind_framebuffer` / `enable` / `scissor` / `disable` /
    /// `clear_color` / `clear` calls *between* calls into `gpu.rs` inside
    /// the same `PaintCallback`, so leaving them raw would make the
    /// recording log's state-change metric silently undercount. 123.4
    /// migrated `gpu.rs`, `widget.rs`, and `app_impl.rs` together for
    /// exactly that reason; that same rationale is why the two toast
    /// passes (123.5) also had to go through the facade rather than stay
    /// an accepted exception.
    const NOT_YET_MIGRATED: [&str; 0] = [];

    /// The facade module is the one place in the crate that is *allowed* —
    /// and from 123.2, required — to reference `glow::HasContext` directly:
    /// its `Real` arm delegates straight to the driver. Excluding it from
    /// the walk is therefore part of the guard's definition, not an escape
    /// hatch, and it is what lets the detector and its own failure messages
    /// spell the trait name normally.
    const FACADE_MODULE_DIR: &str = "gl_facade";

    /// Recursively collect every `*.rs` file under `dir` into `out`,
    /// skipping the [`FACADE_MODULE_DIR`] subtree entirely.
    fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
        let entries = std::fs::read_dir(dir).expect("read_dir on src tree");
        for entry in entries {
            let entry = entry.expect("dir entry");
            let path = entry.path();
            if path.is_dir() {
                if path
                    .file_name()
                    .is_some_and(|name| name == FACADE_MODULE_DIR)
                {
                    continue;
                }
                collect_rs_files(&path, out);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                out.push(path);
            }
        }
    }

    /// A line counts as a `HasContext` reference iff, after `trim_start()`,
    /// it does not begin with `//` and it contains `HasContext`. This
    /// deliberately excludes doc comments (`gl_facade/mod.rs` and this
    /// file both discuss `HasContext` in prose) while catching both real
    /// import forms present in the tree — `use glow::HasContext;` and
    /// `use glow::{self, HasContext};` — as well as any fully-qualified
    /// `<_ as glow::HasContext>` usage someone might add. The
    /// [`FACADE_MODULE_DIR`] subtree (including this file) is excluded at
    /// the walk level in [`collect_rs_files`], not here, so this needle can
    /// stay a plain literal.
    /// # What this guard does not catch
    ///
    /// It is a textual heuristic, not a semantic check, and its own
    /// justification ("catches a new raw call added later") is broader than
    /// a substring scan can deliver. Two known gaps, neither live today:
    ///
    /// - A re-export under a different name from inside the excluded
    ///   `gl_facade` subtree (`pub use glow::HasContext as GlExt;`) would
    ///   let a consuming file call trait methods on a raw `&glow::Context`
    ///   without its text ever containing `HasContext`.
    /// - It scans the `freminal` crate only. `freminal-windowing` calls
    ///   `clear_color`/`clear` directly on its own `glow::Context`
    ///   (`gl_context.rs`), and this guard is blind to it by construction.
    ///   See the "disclosed gap" note in `PLAN_123`'s Findings.
    ///
    /// Both require deliberate action rather than an accident, which is why
    /// the heuristic is judged worth having — but it should not be mistaken
    /// for a completeness guarantee.
    fn references_has_context(contents: &str) -> bool {
        contents.lines().any(|line| {
            let trimmed = line.trim_start();
            !trimmed.starts_with("//") && trimmed.contains("HasContext")
        })
    }

    #[test]
    fn has_context_use_sites_match_allowlist() {
        let src_root = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src"));
        let mut files = Vec::new();
        collect_rs_files(src_root, &mut files);

        let mut found = BTreeSet::new();
        for path in files {
            let contents = std::fs::read_to_string(&path).expect("read source file");
            if references_has_context(&contents) {
                let relative = path
                    .strip_prefix(env!("CARGO_MANIFEST_DIR"))
                    .expect("path under CARGO_MANIFEST_DIR")
                    .to_string_lossy()
                    .replace('\\', "/");
                found.insert(relative);
            }
        }

        let allowlist: BTreeSet<String> =
            NOT_YET_MIGRATED.iter().map(|s| (*s).to_string()).collect();

        let unexpected: Vec<&String> = found.difference(&allowlist).collect();
        assert!(
            unexpected.is_empty(),
            "unexpected file(s): {unexpected:?} — route GL calls through \
             `gui::renderer::gl_facade::Gl` (Task 123) instead of calling \
             `glow::Context` directly. If this file genuinely cannot go \
             through the facade, add it to NOT_YET_MIGRATED with a written \
             justification."
        );

        let stale: Vec<&String> = allowlist.difference(&found).collect();
        assert!(
            stale.is_empty(),
            "stale NOT_YET_MIGRATED entry(ies): {stale:?} — these files no \
             longer reference `glow::HasContext`. Remove them; the \
             allowlist is meant to shrink to empty at 123.5."
        );
    }

    #[test]
    fn call_surface_is_sorted_and_unique() {
        // The length itself is enforced by the array's `[&str; 49]` type
        // parameter and is a compile-time constant, so it needs no
        // separate assertion here — only sortedness (and, by extension,
        // uniqueness) is a runtime property worth checking.
        assert!(
            GL_CALL_SURFACE.windows(2).all(|w| w[0] < w[1]),
            "GL_CALL_SURFACE must be strictly ascending (sorted and \
             duplicate-free) — it is a frozen audited list, and duplicates \
             would silently corrupt 123.8's metric-group subset checks"
        );
    }

    #[test]
    fn metric_groups_are_subsets_of_the_surface() {
        for method in DRAW_CALL_METHODS {
            assert!(GL_CALL_SURFACE.contains(&method));
        }
        for method in STATE_CHANGE_METHODS {
            assert!(GL_CALL_SURFACE.contains(&method));
        }
        for method in UPLOAD_METHODS {
            assert!(GL_CALL_SURFACE.contains(&method));
        }
    }
}
