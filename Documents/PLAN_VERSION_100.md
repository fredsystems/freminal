# PLAN_VERSION_100.md — v0.10.0 "Beautification & Fonts"

> **STATUS: ENRICHED STUB.** Durable design decisions are captured below;
> per-subtask decomposition happens at activation in a dedicated session,
> against the code as it then exists (see the `freminal-version-activation`
> skill). Do not invent subtasks early.

## Goal

Pay down the visual debt accumulated across v0.2.0–v0.9.0 before later versions
add more surfaces on top of it. Two intertwined fronts, shipped together because
they solve the same problem — the bare-bones visual baseline — from different
angles:

1. **A major UI beautification pass.** Theme-consistent menu bars, modals, dialogs, and toast notifications; bundled glyph/icon assets so action symbols (close, recording, broadcast, etc.) render identically on every platform instead of relying on whatever the system font happens to supply.
2. **A bundled-font overhaul.** Out-of-the-box ligature _and_ powerline glyph
   coverage, so the default experience is complete without the user installing
   a Nerd Font.

Depends on v0.8.0 and v0.9.0. This version lands **before** the AI-assist
versions (v0.18.0–v0.19.0) deliberately: those versions introduce more modals
and overlays, and every new modal inherits whatever visual baseline exists.
Styling the baseline first means later work is styled by construction rather
than retrofitted.

**The terminal buffer is explicitly out of scope for the beautification pass.**
This version restyles chrome (modals, toasts, menus, dialogs) only. Buffer
rendering — cell grid, glyph atlas, shaping — is untouched except where the
bundled-font change alters the default glyph set.

---

## Task Summary

| #   | Feature             | Scope | Status | Depends On     |
| --- | ------------------- | ----- | ------ | -------------- |
| TBD | UI Beautification   | Large | Stub   | v0.8.0, v0.9.0 |
| TBD | Bundled Font / Icon | Large | Stub   | v0.8.0, v0.9.0 |

Task numbers are assigned at activation. Source: `Documents/PLANNING.MD`
"Pre-1.0 Remediations" (UI aesthetic, built-in fonts).

---

## UI Beautification

The chrome is visually inconsistent with the user's selected theme and leans on
fallible font resources for action glyphs.

Durable problems to solve (from `PLANNING.MD`):

- **Theme inconsistency.** Modal/dialog/toast/Menu bar colors do not follow the
  user-selected theme. The beautification pass themes all chrome to the active
  theme. The terminal buffer is NOT affected.
- **Glyph reliability.** Action symbols (close, recording indicator, broadcast
  keyboard, etc.) currently rely on an emoji/powerline glyph from the system
  font. On Linux some render as empty squares; macOS/Windows untested. Relying
  on a fallible external resource for our own UI is a failure mode we must
  eliminate — **we ship the asset ourselves** so every action symbol looks the
  same everywhere and matches our aesthetic.
- **Inspiration:** ACARS Hub and modern web apps — rounded corners, consistent
  color theming, cohesive spacing.

Open questions (decide at activation):

- **Asset format for bundled icons.** Working assumption is bundled SVGs for
  all action symbols, but alternatives (icon font, pre-rasterized PNG atlas)
  are on the table. Decide against the renderer as it then exists.
- **Toolkit constraints.** Is the visual ceiling platform-specific, and are we
  hamstrung by egui? An early subtask must research what egui can and cannot do
  for rounded corners, theming, and custom-drawn chrome before committing to an
  approach.

---

## Bundled Font / Icon Coverage

The bundled font does not guarantee ligature + powerline glyph coverage, so the
default experience depends on the user installing a suitable font.

Durable problems to solve (from `PLANNING.MD`):

- **Maximum ligature AND powerline coverage out of the box**, regardless of what
  the user has installed.

Options to investigate at activation (capture the decision, do not pre-commit):

- **Change the bundled font to a Nerd Font** (or equivalent with broad
  ligature and powerline coverage). **Licensing is a hard gate** — confirm the
  license permits redistribution before this option is viable. This mirrors the
  model
  other terminals (e.g. WezTerm) use. Working preference, pending license
  confirmation.
- **Detect the platform default monospace font and use it** over a bundled
  font. Disfavoured: platform fonts typically lack ligatures and powerline
  glyphs, which defeats the goal.

---

## Design Decisions (provisional; revisit only with explicit user approval)

- **Beautification and fonts ship together.** They solve the same bare-bones
  baseline problem from different angles and are highly intertwined (both touch
  glyph rendering). Splitting them across versions would leave a half-styled
  baseline.
- **This version lands before the AI-assist versions** so later modals inherit
  a styled baseline rather than retrofitting it.
- **The terminal buffer is out of scope** for the beautification pass.
- **We bundle our own action-glyph assets.** Relying on the system font for our
  own UI symbols is a failure mode, not an acceptable fallback.
- **Bundled-font choice is license-gated.** No font is bundled until its license
  is confirmed to permit redistribution.
- **egui-capability research is the first beautification subtask** at
  activation — the approach depends on what the toolkit allows.
- Any new or restyled dialog/overlay is a focusable surface and MUST follow
  `freminal-modal-input-suppression`.
- Escape-sequence support is not touched by this version (no
  `freminal-escape-sequence-docs` dual-doc update expected).
