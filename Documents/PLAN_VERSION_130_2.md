# PLAN_VERSION_130_2.md — v0.13.2 "New Protocols: Small & Medium"

Part of v0.13.x "Kitty: Full Protocol Compliance". The compliance audit, common-work
analysis, reference specs and cross-milestone design decisions live in the overview,
`PLAN_VERSION_130.md`. This document holds the v0.13.2 tasks only.

**Status:** Planned.

New kitty protocols of small and medium size: the misc-protocol extensions, unscroll,
DECCARA, colour control and the colour stack, kitty shell-integration compatibility, and
multiple cursors. Every task is an enriched stub, decomposed at activation.

## Tasks

| #   | Task                                  | Milestone | Scope | Status  | Depends on               |
| --- | ------------------------------------- | --------- | ----- | ------- | ------------------------ |
| 103 | Multiple Cursors                      | v0.13.2   | L     | Planned | 127, 128, 131, 132, 141  |
| 141 | Misc Protocol Extensions              | v0.13.2   | M     | Planned | 128, 131                 |
| 142 | Unscroll (`CSI Ps + T`)               | v0.13.2   | M     | Planned | 128                      |
| 143 | DECCARA / DECSACE                     | v0.13.2   | M     | Planned | 128, 140                 |
| 144 | Color Control (OSC 21) & Colour Stack | v0.13.2   | L     | Planned | 128, 129, 130, 132       |
| 145 | Kitty Shell-Integration Compatibility | v0.13.2   | M     | Planned | 129; maintainer decision |

---

## Task 141 — Misc Protocol Extensions (stub, v0.13.2)

**Goal.** Implement every item on the misc-protocol page.

**Audit findings.** All of the following are missing:

- **Bare XTSAVE/XTRESTORE (`CSI ? s`/`CSI ? r`)**, plus the explicit `CSI ? Pm s/r` xterm
  form. Kitty's side-effect-free set: LNM, IRM, DECARM, 2004, 1004, 2031, 2048, 5522,
  DECCKM, DECTCEM, DECAWM, mouse tracking and encoding, DECSCNM. There is one global slot.
- **`CSI 22 J`** (move screen to scrollback, then ED 2; main screen with full margins only).
  `EraseDisplayMode` has no variant for it. **Task 103 needs it**, because ED 22 clears extra
  cursors.
- **Mouse-leave report.** `CSI < 288;x;y M` is sent when the encoding is SGR-pixel, on window
  leave and pane-to-pane transitions. Bit `1<<8 | MOTION` per kitty `mouse.c`; the docs'
  "eighth bit" wording is ambiguous.
- (SGR 221/222 belong to Task 140.)

**Seams.**

- Mode state is split three ways: `TerminalModes`, handler-owned modes and buffer-owned
  modes. XTSAVE must snapshot all three consistently.

**Open questions.**

- Whether mouse-leave also requires a tracking mode other than none (kitty doesn't check).
- How the explicit-list XTSAVE form handles DECOM and DECCOLM side effects.

## Task 142 — Unscroll `CSI Ps + T` (stub, v0.13.2)

**Goal.** Pull `Ps` lines back from scrollback into the top of the screen, filling with blank
lines when scrollback is empty or on the alternate screen.

**Seams.**

- `RowStore::pop` is documented as popping blank padding only.
- Stable-`RowNumber` anchors (selection, search, command blocks, placements) shift.
- Scrollback rows may be compressed.
- Cursor buffer-row accounting changes.
- `row_epochs` damage over the shifted rows.

**Open questions.** Behaviour with DECSTBM/DECLRMM margins: the spec is silent, and kitty's
behaviour looks like a quirk. This is a maintainer decision; do not guess terminal semantics.

## Task 143 — DECCARA / DECSACE (stub, v0.13.2)

**Goal.** Kitty's extended DECCARA.

- `CSI Pt;Pl;Pb;Pr;<SGR…> $ r` applies **any** SGR (colours, underline styles, colon forms) to
  a rectangle or stream extent.
- `CSI Ps * x` (DECSACE) selects the extent: 2 = rectangle, 0/1 = stream.
- DECOM-relative; visible screen only.
- Parameterless `CSI $ r` is a full-screen SGR reset (kitty).

**Seams.**

- A `Buffer` method taking a rectangle, an extent and a `FnMut(&mut FormatTag)`.
- `Cell::set_tag`.
- Reuse `apply_sgr` (`terminal_handler/sgr.rs:178`).
- Compact-row storage (Task 118).
- `row_epochs` bumps: region-bounded damage.
- Advertise DA1 feature 28 once implemented.

DECRARA is out of scope; kitty does not implement it.

## Task 144 — Color Control (OSC 21) & Colour Stack (stub, v0.13.2)

**Goal.** Full OSC 21 and the colour stack.

- **OSC 21 keys:** foreground, background, selection_background/foreground, cursor,
  cursor_text, visual_bell, transparent_background_colorN, and palette indices 0–255. Each
  supports `?` queries and the three-state set/empty/bare value. `unknown=` replies are
  unpadded base64.
- **Colour stack:** `CSI Ps # P` / `# Q` / `# R` (XTPUSHCOLORS/XTPOPCOLORS/XTREPORTCOLORS)
  and kitty's `OSC 30001`/`30101`. Depth 10, per window (not per screen). The reply is
  `CSI idx;count # Q`.
- **Saved state** covers dynamic colours, `cursor_text`, `visual_bell`, transparent-bg
  overrides and the 256-colour palette.
- **Owned here, not by Task 132 (maintainer decision 2026-10-11):** the `cursor_text` and
  `visual_bell` colours (fields, rendering and setters; their only setter is OSC 21; today
  `ThemePalette::cursor_text` has no reader and the bell colour is hard-coded) and kitty's
  `@alpha` colour suffix (meaningful only for `transparent_background_colorN`; the shared
  colour-spec parser rejects `@`).
- **Builds on Task 132:** add the new slots to `DynamicColorOverrides` /
  `DynamicColorSlot` and, where they are content colours, to `ResolvedColors`; parse OSC 21
  values with the shared `parse_color_spec` (which already accepts names, `rgbi:`,
  `oklch()` and `lab()`); push/pop must go through `ColorState`'s mutators so the resolved
  `Arc` and the GUI's damage trigger follow.

**Depends on.**

- Task 132: rendered dynamic colours and render-time palette lookup. Without them a pop does
  not recolour existing text.
- Task 128: `#` routing.

**Open questions.**

- `transparent_background_color1..7` (spec table) versus `1..8` (kitty code).
- Whether per-colour background opacity is in scope or split out.
- From Task 132 review R18: with `background_opacity` < 1, the OSC 11 base fill is drawn at
  that opacity over the theme panel fill, so a `(1 − opacity)` share of the theme background
  shows through. A replace blend for the fill would fix it but needs `blend_func` in the
  frozen GL facade; decide here alongside per-colour opacity.

## Task 145 — Kitty Shell-Integration Compatibility (stub, v0.13.2; decision-gated)

**Goal.** Accept the shell-integration escape codes that kitty's own scripts, fish 3.8+ and
nushell emit.

**Audit findings.**

- **`kitty-shell-cwd://host/path` OSC 7 is rejected**, with a warn on every prompt
  (`shell_integration.rs:25`). Kitty accepts both schemes.
- **`OSC 133;k;…`** (bash-kitty integration, six per prompt) warns on every prompt.
- **Untagged OSC 133 A/C/D is dropped by design.** The same goes for `k=s` secondary
  prompts, `cmdline=`/`cmdline_url=`, `redraw=`, `special_key=` and `click_events=`. The
  `freminal=1; fid=` gate is recorded in `DESIGN_DECISIONS.md:837-848`.
- **Click-to-move-cursor-at-prompt is missing.**

**Gate.** Accepting untagged OSC 133 reverses a recorded design decision and rewrites tests
that pin the drop, so it needs maintainer approval before decomposition. The OSC 7 scheme fix
and the silent consumption of `133;k;…` are independent and decision-free; they may be pulled
forward into v0.13.0 as cleanup.

## Task 103 — Multiple Cursors (stub, v0.13.2; previous breakdown superseded)

**Goal.** Implement the multiple-cursors protocol: `CSI > SHAPE ; (TYPE:COORDS)* SP q`.

**Spec surface.**

- Shapes: 0 clear, 1 block, 2 beam, 3 underline, 29 follow-main.
- Coordinate types: 0 main, 2 `y:x` points, 4 `top:left:bottom:right` rectangles (no numbers
  means the full screen).
- Colours: 40 cursor, 30 text-under-cursor. Colour spaces: 0 unset, 1 special
  (reverse-video), 2 sRGB, 5 indexed.
- Queries and replies:
  - support: `CSI > SP q` → `CSI > 1;2;3;29;30;40;100;101 SP q`;
  - cursors: `CSI > 100 SP q`;
  - colours: `CSI > 101 SP q`.
- Extras share the main cursor's blink and opacity, are **not** hidden by DECTCEM (kitty
  0.49 fix), do not scroll, and clear on ED 2/3/22, reset and alternate-screen switch.
- Stable since the warning was removed on 2026-01-08.

**Corrections to the previous breakdown.**

- The `q` dispatch collision is handled by 126.3 and 128.
- **103.3 is much larger than "iterate in `build_cursor_verts_only`".** It needs:
  - a variable-length cursor region (**delivered by 127.3/127.4**);
  - a text-under-cursor colour capability in the foreground pass;
  - change detection: an extras epoch;
  - BOUNDABLE-NOW damage via `PaneDamageRect::from_cursor_cells`;
  - its own visibility, scroll and fold gating.
- No reusable `2:r:g:b`/`5:i` colour parser exists (Task 132).
- `CSI 22 J` is a prerequisite (Task 141).
- The state lives in a new handler module, not inline in `terminal_handler/mod.rs`.
- Indexed colours are resolved at snapshot-build time.
- Query-100 replies are emitted in row-major order, for determinism.

**Open questions.**

- Type-4 rect with 1–3 numbers: the spec says ignore; kitty treats it as the full screen.
- Colour component outside 0–255: clamp or reject (kitty masks).
- DECSTR clears extras (kitty does).
- Extras while scrolled back: hide, or draw at screen coordinates.
- Extras in inactive panes: follow Task 127's hollow rule, or hide.
- Resize: clip out-of-range extras.
