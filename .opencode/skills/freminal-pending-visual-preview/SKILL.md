---
name: freminal-pending-visual-preview
description: Use ONLY when working in the freminal repository AND adding, renaming, or reasoning about a `Config` field that changes freminal's on-screen appearance -- a new theme/color/opacity/cursor/font/background-image/shader/gutter/tab-bar option, or any option a user would expect to see change *before* clicking Apply. Triggers on "should this preview live", "the Settings modal doesn't show this until Apply", "Cancel left the old value applied", "how do I revert a preview", or touching `VisualPreview` / `SettingsAction::Preview` / `DebouncedPreview` / `apply_visual_preview`. Codifies the whole-state-snapshot design that replaced five ad-hoc `SettingsAction::Preview*`/`Revert*` variants after `PreviewProfile` shipped with no matching revert (issue #452), and the two review-caught close-path bugs (a stuck broken shader, a stuck error route) that followed from gating the close path on "did the draft change".
---

# Freminal: an appearance-changing config option is a live-preview candidate, and the way to add it is a field on `VisualPreview`

If a new `Config` field changes what freminal looks like while it runs, it is
a candidate for the pending-preview architecture in
`freminal/src/gui/visual_preview.rs`. Adding it there means **adding a field
to `VisualPreview`** -- never a new `SettingsAction` variant, and never a
paired `Preview*`/`Revert*` pair of your own.

## Why it breaks: the bug this design replaced

Before this module existed, live preview was five flat `SettingsAction`
variants: `PreviewTheme`, `RevertTheme`, `PreviewOpacity`, `RevertOpacity`,
`PreviewProfile`. Each preview mutated live state directly and relied on a
hand-maintained `original_*` field to revert it. `PreviewProfile` shipped
with **no matching `RevertProfile`** -- it wrote `FreminalGui::gui_theme`
directly and nothing put it back. The result: preview a chrome style
profile, click Cancel, and the chrome stayed changed until the next Apply or
a process restart.

The fix was not a sixth variant. It was removing per-option revert as a
concept: `VisualPreview` is a **whole-state snapshot** of the previewable
subset of `Config`, there is exactly one
`SettingsAction::Preview(VisualPreview, PreviewTrigger)`, and reverting is
just "preview the committed config again"
(`VisualPreview::from_config(&committed_config, os_dark_mode)`). A field
that can be previewed is automatically revertible, because preview and
revert are the same code path. A forgotten revert is now structurally
impossible -- there is nothing per-option left to forget.

## The core mechanism

`VisualPreview::diff_from(&previous)` produces a `VisualPreviewDiff`: one
`Option<_>` per field, `Some(new_value)` only for fields that changed.
`apply_visual_preview` in `settings_dispatch.rs` calls one `apply_preview_*`
helper per field group, each of which does nothing unless its own diff
entry is `Some`. Adding a previewable option means:

1. Add the field to `VisualPreview` (and to `CursorPreview` or `FontPreview`
   if it belongs to one of those groupings -- see their doc comments for
   why they exist as separate structs).
2. Read it from `Config` in `VisualPreview::from_config`.
3. Add its `Option<_>` entry to `VisualPreviewDiff` and compute it in
   `diff_from` (an epsilon comparison for any `f32`, per
   `VisualPreview`'s manual `PartialEq`).
4. Write one `apply_preview_<field>` helper in `settings_dispatch.rs` that
   reads the diff entry and carries out the side effect for the right lane
   (below), and call it from `apply_visual_preview`.
5. Add it to the `from_config`/`diff_from` test coverage already in
   `visual_preview.rs` (`from_config_captures_phase_*_fields`,
   `diff_from_reports_each_phase_*_field_independently`, and a
   reset-to-default/cleared case if the field is an `Option<T>`).

Do not add a `SettingsAction::PreviewX` / `RevertX` pair. Do not add a
separate `original_x` field to stash for later. If you catch yourself
writing either, you are re-introducing the exact shape that produced the
`PreviewProfile` bug.

## The three lanes

An option's live effect travels one of three paths; which one determines
what the `apply_preview_*` helper actually does.

1. **PTY-owned** (theme, cursor shape/blink, auto-detect-urls): broadcast an
   `InputEvent` (e.g. `InputEvent::ThemeChange`, `InputEvent::CursorConfigChange`)
   to every pane in every window. The PTY thread rebuilds its snapshot
   asynchronously, so an idle terminal (no cursor blink, no output) would
   otherwise only pick up the change on the next external event. Call
   `self.schedule_pty_roundtrip_repaint(handle)`, which requests an
   immediate repaint plus a ~50ms follow-up on every window, rather than a
   single `request_repaint`.
2. **GUI-thread-owned** (background opacity, hide menu bar, tab bar
   position, cursor trail, ligatures): write the field directly (usually
   straight into `self.config` or a per-window `terminal_widget` toggle) and
   call `self.request_repaint_all_windows(handle)` -- effective next frame,
   no PTY round trip needed. Anything needing a live GL context (background
   image texture upload, shader compile) instead stashes into a
   `pending_*` field (`RenderState::set_pending_bg_image`,
   `WindowPostRenderer::pending_shader`) that is only consumed inside the
   render thread's `PaintCallback`.
3. **Chrome-only immediate override** (`preview_theme`, `gui_theme`): read
   directly by the per-frame style hook for same-frame effect on chrome
   widgets, independent of and in addition to lane 1's asynchronous
   terminal-cell catch-up for the same change (a theme change is both:
   `preview_theme` restyles chrome immediately, `InputEvent::ThemeChange`
   catches up the terminal buffer).

## When to debounce

Route through `DebouncedPreview<T>` (`visual_preview.rs`) instead of
applying on every draft change when the field is backed by a **text field**
or drives **expensive work**: font family (`FontManager::rebuild` reparses
font files), font line height (same rebuild), background image path
(filesystem read + GPU texture upload), and shader path (a GLSL compile
that can fail). `FONT_PREVIEW_DEBOUNCE` and `PATH_PREVIEW_DEBOUNCE` are both
200ms. Reuse the existing generic holder -- it is already generic over `T`
specifically so phase D could reuse it unchanged for two more fields; do
not write a second bespoke debounce.

Two things every debounced field needs, beyond `note()`/`poll()`:

- A **settle-on-idle wake**: a stable draft produces no further
  `SettingsAction::Preview` on its own, so schedule
  `handle.request_repaint_after(settings_window_id, INTERVAL)` when a value
  is freshly stashed, and poll every debounce holder unconditionally each
  settings-window frame (`tick_visual_preview_debounces`) -- that poll is
  the only path that ever applies a value once the user stops editing.
- An **immediate bypass on revert**: `PreviewTrigger::Revert` must call
  `DebouncedPreview::apply_immediately`, discarding any pending stash
  without ever applying it, then decide with `debounced_revert_needs_push`
  (see below) whether a push to the renderer is actually needed.

Font **size** alone is cheap enough to skip the rebuild path even when
settled: `apply_settled_font_preview` routes to `set_font_size`
(`apply_font_zoom`) rather than `FontManager::rebuild` when only size
changed, reusing the existing Ctrl+Scroll zoom path.

## The trap: do not gate the close path on "did the draft change"

This is the most valuable rule here, because it produced two real review
blockers on the same commit that built this module. The close-without-apply
path (both `show()` and `show_standalone()` in `settings.rs`) must emit
`SettingsAction::Preview(committed_preview, PreviewTrigger::Revert)`
**unconditionally** -- never gated on `committed_preview != preview_before`
or any other "did the visible draft change" comparison. Two kinds of state
are invisible to that comparison:

- **A debounced value that already reached the renderer.** Commit `A`,
  preview `B`, let it settle (the debounce holder's baseline is now `B`),
  then retype `A` and cancel before a second debounce settles. The draft is
  back to `A`, matching committed -- but the renderer is still showing `B`.
  Gating the revert on "draft changed" skips it entirely, leaving a broken
  shader (or stale font, or stale background image) live with no way back
  (issue #452 post-review Blocker 1).
- **State that is metadata about *how* a value was pushed, not a value in
  the snapshot at all.** `ShaderErrorRoute` is not a `VisualPreview` field
  and never will be -- it says where a *later, asynchronous* compile error
  should surface, not what the shader looks like now. A gated close path
  can leave it stuck on `SettingsStatus` with the window already gone,
  silently swallowing a subsequent genuine shader hot-reload failure
  (issue #452 post-review Blocker 2).

The rules that follow from those two bugs, all already implemented and not
to be re-broken:

- The close-path emission of the revert `Preview` action is unconditional.
  This is safe and cheap because `apply_visual_preview` diffs against
  `self.applied_preview` and each debounce holder's own baseline -- every
  field that already matches is a no-op.
- Each `apply_preview_<field>_immediate` helper (font, background image
  path, shader path) decides whether to push via `debounced_revert_needs_push`,
  which compares the debounce holder's own `applied()` baseline (what was
  actually last pushed to the running app) against the committed value --
  **not** against the draft or against `applied_preview`. Those two sources
  can disagree, and comparing against the draft is exactly Blocker 1.
- Any push-metadata flag (`shader_error_route`) is reset unconditionally in
  `end_settings_session` (`settings_dispatch.rs`, via
  `shader_error_route_on_settings_close`), not inside any diff-dependent or
  early-return branch. An `apply_preview_shader_path_immediate` early return
  (nothing to push) does **not** reset the route itself -- the session-end
  helper is the single place that guarantees the reset regardless of which
  branch got there.
- **Every way a Settings session ends must call `end_settings_session`.**
  Not every close path renders another Settings frame, so the revert that
  `show_standalone` emits on Cancel does not cover them all. Today: the
  Settings window's self-close branch in `app_impl.rs` (reached by Apply/OK,
  Cancel, the window's own close button, and the owning terminal window's
  `CloseNow`), plus the owning window's Discard prompt, which closes the
  Settings window directly. `on_close_requested` has no `WindowHandle`, so
  the Settings window's own close button vetoes the OS close and repaints
  the window to reach the self-close branch rather than closing it in place.
  A close path that skipped the helper once left discarded previews written
  into `self.config`, where the next session's draft picked them up and a
  later Apply saved them. If you add a new way to close Settings, route it
  through the self-close branch or call the helper.

If you add a new debounced or metadata-carrying preview field, replicate
this shape: unconditional close-path revert, baseline-vs-committed
comparison (not draft-vs-committed) for whether to push, and any
push-metadata reset in `end_settings_session` rather than inside the push
helper.

## Cell-size and layout changes: do not hand-roll a resize

Font size/family/line-height and the command-block gutter position change
the terminal's character-cell dimensions. Do **not** send
`InputEvent::Resize` from inside an `apply_preview_*` helper. The existing
reactive resize detection in `app_impl.rs` already re-derives
`pane_width_chars`/`pane_height_chars` every frame from the current config
and font metrics, and issues the resize itself on a mismatch -- a plain
repaint is all a gutter or font preview needs to trigger it, on both apply
and revert.

This means previewing these fields **resizes every running program** in
every pane, including full-screen TUIs. That disruption is an accepted
maintainer decision for this class of option, not an oversight -- do not
try to "fix" it by suppressing the resize during preview.

## Preview must never persist

A preview mutates only in-memory state (`self.config`, per-window widget
fields, debounce-holder baselines, GPU-side pending slots). Only Apply / OK
writes to `config.toml`. Every field you add must be fully restorable by
the revert path with no on-disk trace of an unapplied preview -- if a field
can't be cleanly reverted this way, it is not compatible with this
architecture and needs a design conversation before wiring it in as a
preview.

## Relationship to `freminal-config-options`

`freminal-config-options` owns the persistence ritual for a new `Config`
field: `ConfigPartial` wiring, `apply_partial`, the
`every_config_section_survives_partial_merge` guard test,
`config_example.toml`, and the Nix home-manager module. That checklist is
still fully required for any new option, previewable or not. This skill is
the **fourth thing to consider**, after persistence is wired: does this
field change appearance, and if so, does it belong on `VisualPreview` too.
Do not duplicate the persistence checklist here -- follow it as written in
that skill.

## Failure surfaces

A preview that can fail (today: shader compile) must report into the
Settings window's own status message (`set_preview_status_message`), never
onto the toast stack -- see `ShaderErrorRoute::SettingsStatus`. A toast
would mean a user typing a shader path character-by-character gets a toast
flood on every open terminal window before they finish. Apply-path and
`hot_reload` failures keep the existing toast behavior
(`ShaderErrorRoute::Toast`, the default) -- only a live, still-open,
still-editing preview reroutes.

## Tests that exist (and what they pin)

There is no compile-time tripwire here equivalent to
`every_config_section_survives_partial_merge`'s no-`..`-rest-pattern
destructure -- adding a field to `VisualPreview` does not force a build
failure anywhere. The safety net is the existing test suite in
`visual_preview.rs`, all pure and `FreminalGui`-free:

- `from_config_captures_phase_*_fields` -- every field round-trips out of
  `Config` correctly, including epsilon-aware float comparisons.
- `diff_from_reports_each_phase_*_field_independently` /
  `diff_from_reports_no_changes_when_equal` -- one field changing does not
  mask or get masked by another.
- `diff_from_reports_font_family_reset_to_default` /
  `..._background_image_path_reset_to_cleared` /
  `..._shader_path_reset_to_cleared` -- pin that resetting an `Option<T>`
  field to `None` is reported as a real change (`FontFamilyChange::Default`
  etc.), not silently treated as "unchanged" the way a naive `Option<Option<T>>`
  would risk.
- `reverting_to_committed_config_restores_the_original_profile` -- pins the
  `PreviewProfile` bug fix directly: preview a changed profile, then preview
  the committed config again, and assert the diff reports the original.
- `debounce_decision` / `debounced_revert_needs_push` /
  `shader_error_route_for` / `shader_error_route_on_settings_close` each
  have direct unit tests as pure functions -- these are the decision logic
  behind the two post-review blockers, extracted and tested precisely
  because the blockers existed due to that logic never having been isolated
  and tested before.

When you add a field, add to the existing `from_config_captures_*` and
`diff_from_reports_*_independently` tests rather than writing a new,
differently-shaped test -- keeping every field's coverage in the same shape
is what makes a missing one easy to spot in review.

## When to stop and ask

- The new option's live effect needs a **fourth** lane not described above
  (e.g. it must reach outside the current process, or needs a resize sent
  eagerly instead of picked up reactively). Surface the new lane rather than
  forcing it into an existing `apply_preview_*` pattern that doesn't fit.
- You're tempted to compare a debounced field's revert decision against the
  draft or `applied_preview` instead of the debounce holder's own
  `applied()` baseline. That is Blocker 1's exact shape -- stop and re-read
  `debounced_revert_needs_push`'s doc comment.
- A new field's failure can't cleanly be scoped to "Settings-status while
  editing, toast once committed" (e.g. it can fail asynchronously long after
  Settings has closed with no `PreviewTrigger` in scope). That is a new
  failure-routing design, not a rename of `ShaderErrorRoute`.
- The field genuinely cannot be reverted in memory (it has an irreversible
  side effect once triggered). That means it does not belong in this
  architecture at all -- raise it before wiring it into `VisualPreview`.
