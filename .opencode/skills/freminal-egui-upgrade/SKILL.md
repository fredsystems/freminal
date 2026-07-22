---
name: freminal-egui-upgrade
description: Use ONLY when working in the freminal repository AND bumping, unpinning, or otherwise changing the version of any crate in the egui rendering stack — `egui`, `epaint`, `egui_glow`, `egui-winit` (and closely-coupled `glow` / `glutin` / `winit` / `raw-window-handle`) — or editing their `=0.35.0` exact pins in the workspace `Cargo.toml`, or touching the Renovate "egui + windowing stack" group. The chrome-caching work (#435/#436) relies on undocumented internal behaviour of egui 0.35.0; this skill mandates walking the `Documents/EGUI_UPGRADE_ASSUMPTIONS.md` re-verification checklist and running the pixel-level smoke before any such bump lands, and explains why the exact pins and the no-auto-merge Renovate rule are deliberate.
---

# Freminal: verify the egui stack before bumping it

The chrome-caching work (issues #435 and #436) makes the GUI skip
re-recording / re-tessellating / re-painting chrome on frames where the
chrome did not change. To do this it depends on a set of **undocumented,
internal behaviours** of the egui stack (`egui`, `epaint`, `egui_glow`,
`egui-winit`) at version **0.35.0** — things that are not part of any
crate's public API contract and can change silently across versions.

The failure mode is the dangerous kind: **no compile error, no headless
test failure** (the repo has no headless-GL harness), just wrong pixels,
ghosted chrome, or stale overlays at runtime.

## The rules

1. **`egui`, `egui_glow`, and `egui-winit` are exact-pinned (`=0.35.0`) as a
   matched set** in the workspace `Cargo.toml`. This is deliberate. Do **not**
   relax them to a caret range as a "cleanup". A patch bump can change the
   internal behaviour we rely on.

2. **Never auto-merge an egui-stack bump.** Renovate's "egui + windowing
   stack" group is configured with `automerge: false` and
   `dependencyDashboardApproval: true`, so a bump PR is not even opened until a
   human approves it on the Dependency Dashboard. Do not remove those settings.
   (Dependabot cargo updates are disabled entirely via
   `open-pull-requests-limit: 0`.)

3. **Before bumping any crate in the egui stack, walk every row of
   `Documents/EGUI_UPGRADE_ASSUMPTIONS.md`** against the new version's source.
   For each assumption:
   - Read the new version's equivalent upstream code (line numbers will have
     drifted — find the equivalent, do not trust the old line number).
   - Confirm the behaviour still holds.
   - If it changed, fix the corresponding `Our code` site and update the
     assumptions doc _in the same PR_.

4. **A green `cargo test --all` is NOT sufficient.** It only catches the
   headless-verifiable subset (callback ordering, atlas-growth detection logic,
   the FULL/REPLAY decision, the damage composition). The load-bearing failures
   are pixel-only. You must additionally run the pixel-level verification:
   - the 436.9 pixel harness once it exists, OR
   - until then, a manual visual smoke of every "Symptom if broken" scenario in
     the assumptions doc (idle blink, multi-pane + active post-process shader,
     atlas growth from a never-before-seen glyph, an open context
     menu / search bar / command palette / URL-hover tooltip left idle, an OS
     dark/light switch, a DPI/scale-factor change).

5. **Only after all of the above passes may the `=0.35.0` pins move.**

## Why this is worth the friction

The whole point of #436 is to _not_ redraw chrome unless it changed. The
mechanism leans on egui internals that the egui authors are free to change
without warning (they are not public API). If one of those changes and we bump
blindly, the terminal ships with visibly broken or ghosted chrome and no test
tells us. The exact pins plus the manual gate make an egui bump a conscious,
verified act rather than an automated one.

## When to stop and ask

- The new egui version changed one of the assumptions and the fix is
  non-trivial (e.g. the atlas now relocates glyphs, invalidating A6's
  soundness). Stop and surface it — this may require redesigning part of the
  chrome-cache, not a quick patch.
- You are tempted to unpin or auto-merge "just this once" to unblock something.
  Don't. Surface the need instead.

See `Documents/EGUI_UPGRADE_ASSUMPTIONS.md` for the full assumption table
(A1–A12): what we rely on, where our code depends on it, the upstream source
that proves it in 0.35.0, and the visible symptom if a bump breaks it.
