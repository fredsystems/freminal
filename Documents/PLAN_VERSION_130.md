# PLAN_VERSION_130.md — v0.13.0 "Power-User Toolkit"

> **STATUS: ENRICHED STUB.** Durable design decisions are captured below;
> per-subtask decomposition happens at activation in a dedicated session,
> against the code as it then exists (see the `freminal-version-activation`
> skill). Do not invent subtasks early.

## Goal

Turn freminal into a power user's daily driver: named profiles, theme preview + custom
color picker, per-profile/in-session ligature toggles, regex scrollback search,
quick-select/hints mode, a command palette, per-pane title bars, and dynamic tab width.
Keyboard-first throughout.

Depends on v0.8.0 (correctness) and v0.9.0 (modern workflow primitives). No dependency on
the event hook API (Task 84) — that lands last (v0.19.0).

---

## Task Summary

| #   | Feature                             | Scope  | Status    | Depends On      |
| --- | ----------------------------------- | ------ | --------- | --------------- |
| 78  | Profiles + Quick Profile Switching  | Medium | Stub      | v0.8.0          |
| 79  | Theme Preview + Color Picker        | Medium | Stub      | v0.8.0          |
| 80  | Font Ligatures Per-Profile Toggle   | Small  | Stub      | Task 78         |
| 81  | Regex Scrollback Search             | Medium | Stub      | v0.8.0, Task 45 |
| 82  | Quick-Select / Hints Mode           | Medium | Stub      | v0.8.0          |
| 83  | Command Palette                     | Medium | Stub      | v0.8.0          |
| 83a | Expanded Auto-Detection (TENTATIVE) | Medium | Tentative | Task 71.7b      |
| 96  | Per-Pane Title Bar                  | Small  | Stub      | Task 58         |
| 97  | Dynamic Tab Width & Overflow        | Small  | Stub      | v0.8.0 (71.1)   |

Tasks 82 and 83 absorb `FUTURE_PLANS.md` items B.3 and B.2 respectively.

**Task 83a is TENTATIVE and may be removed.** It is a follow-up brainstorm from Task 71.7b
(auto-detect plain URLs). A future agent MUST NOT begin work on Task 83a without the
maintainer explicitly greenlighting it. Do not promote it to "Pending" or activate it
autonomously.

---

## Task 78 — Profiles + Quick Profile Switching

A profile is a named bundle of (theme, font, shell, startup command, env vars, bell mode,
ligature setting, opacity). Profiles live in `~/.config/freminal/profiles/*.toml`. The
new-tab menu gains a profile picker; a configurable keybinding cycles profiles.

Open questions (decide at activation):

- Relationship to Workspace Env (Task 75) — is a profile a subset of a workspace, or
  orthogonal?
- Profile inheritance / composition semantics.
- Migration from current single-config users.

---

## Task 79 — Theme Preview + Color Picker

Two distinct pieces, both retained per the post-v0.9.0 roadmap review:

1. **Hover preview** — hovering a theme in Settings → Theme shows a live preview without
   committing. Note: freminal already _live-applies_ a theme on selection (hot-reload);
   the new behaviour here is the non-committal hover preview.
2. **Custom palette editor / color picker** — a dedicated palette editor (16+ ANSI
   colours, fg/bg/cursor) with a visible sample line, so custom themes are built in a UI
   instead of hand-edited TOML. This is the higher-value half.

Open questions (decide at activation):

- Preview scope: preview the Settings panel only vs. preview the real active terminal.
- Custom theme format / location (`~/.config/freminal/themes/*.toml`).
- Color picker widget: egui-native, or custom for better accessibility.
- Any in-window color-picker panel is a focusable overlay and MUST follow
  `freminal-modal-input-suppression`.

---

## Task 80 — Font Ligatures Per-Profile Toggle

Ligatures (Task 5) are currently a global toggle. Move to per-profile (Task 78) and add an
in-session keybinding to toggle on/off without reloading config.

Open questions (decide at activation):

- Default: on or off per new profile.
- Whether to toggle individual ligature groups (`liga`, `calt`, `dlig`) independently.

---

## Task 81 — Regex Scrollback Search

Upgrade the existing search (Task 45, v0.4.0 "Search & Protocol") to support regular
expressions with live highlighting of matches as the query is typed, and jump to
next/previous match.

**Naming/scoping note:** earlier roadmaps called this "GPU-Accelerated Scrollback Regex
Search". The label oversold it: the regex matching runs on CPU (the `regex` crate over
scrollback text); only the match-highlight overlays use the existing GPU glyph/quad
pipeline. It is "regex search with GPU-rendered highlights", and it builds on the search
freminal already has rather than being a from-scratch feature. Scope against the existing
search at activation.

Open questions (decide at activation):

- Regex engine: `regex` crate vs. `fancy-regex`.
- Highlight rendering: quad overlay vs. cell background override.
- Search scope: current pane, current tab, all panes in window.

---

## Task 82 — Quick-Select / Hints Mode

Absorbs `FUTURE_PLANS.md` item B.3. Enter hints mode (configurable keybinding), and every
detected pattern (URL, file path, git hash, IP, number) is highlighted with a two-letter
label. Typing the label copies or opens the target.

Open questions (decide at activation):

- Pattern registry: built-in vs. user-configurable regex list.
- Label alphabet (home-row Colemak-friendly vs. QWERTY).
- Action per pattern type (open URL, copy path, open file, `$EDITOR file:line`).

---

## Task 83 — Command Palette

Absorbs `FUTURE_PLANS.md` item B.2. `Ctrl+Shift+P` (configurable) opens a fuzzy-search
palette listing all `KeyAction`s with their current bindings. Selecting an action executes
it. The palette is a focusable overlay and MUST follow `freminal-modal-input-suppression`.

Open questions (decide at activation):

- Fuzzy-match algorithm (skim, fzf-style, simple substring).
- Whether to include non-`KeyAction` commands (layout load, profile switch, theme picker).
- History and frecency ordering.

---

## Task 83a — Expanded Auto-Detection (TENTATIVE)

**STATUS: TENTATIVE. May be removed. Do not start without maintainer approval.**

Follow-up brainstorm to Task 71.7b (auto-detection of plain
`http`/`https`/`file`/`ftp`/`mailto` URLs). The maintainer has not committed to shipping
any of the expansions below. A future agent MUST stop and ask before any implementation.

Extend `freminal-buffer/src/url_detect.rs` to recognise more clickable patterns. Three
independent sub-features, each shippable alone:

1. **Absolute file paths** — `/etc/nginx/nginx.conf`, `/Users/fred/src/foo.rs:42:8`.
   Optimistic detection, **no filesystem I/O on the flatten path** (blocking NFS/stalled
   mounts forbidden). Click attempts `xdg-open` or `$EDITOR +LINE path`.
2. **Schemeless URLs** — `github.com`, `docs.rs/regex`, `localhost:8080`. TLD allow-list.
   **Opt-in only, default false** — the `.rs`/`.io`/`.sh`/`.dev` TLD collision with
   common filenames is too dangerous to be default-on in a Rust-first terminal.
3. **Explicit relative paths** (`./`, `../` only) — requires OSC 7 for click-to-open.
   Bare relative paths (`src/foo.rs`) are explicitly out of scope (too lossy).

`UrlMatch` would gain a `kind: UrlKind` field for click-dispatch branching.

Non-goals: no filesystem I/O on the flatten path ever; no bare relative path detection; no
hover-time existence verification in v1.

Open questions (decide at activation, if greenlit): per-kind Settings toggles vs one
global switch; `$EDITOR`/`$VISUAL`/config template for file:line; TLD list source; git SHA
/ issue-ref detection (out of scope for 83a but natural extensions).

---

## Task 96 — Per-Pane Title Bar

Optional per-pane title bar at the top of each split pane showing that pane's title (and
possibly running command / CWD basename / PID). Tabs already show a single title (the
active pane's); inactive panes' titles are invisible today — a loss when they run `htop`,
`cargo watch`, or a remote SSH session. Toggleable globally and configurable per layout.
Coexists with the tab bar (Task 94 precedence) and the future status bar (Task 85, a
different bar at a different scale).

Scope: `freminal/src/gui/terminal/` (pane rendering), `freminal/src/gui/panes/` (per-pane
title field exists), `freminal-common/src/config.rs` (toggle — follow
`freminal-config-options`).

Open questions (decide at activation): always-on vs only-when-multiple-panes vs
config-driven; bar height (cells vs pixel band); focused-pane accent; read-only vs
affordances (close/zoom/rename). Triaged from `bugs.txt` Idea 2 (2026-05-17).

---

## Task 97 — Dynamic Tab Width & Overflow

Today's tab bar uses fixed-width tabs; OSC-driven titles truncate while user-set names
render full-width (an inconsistency to fix), and there is no defined overflow behaviour.
Audit current behaviour, then implement dynamic per-tab width (bounded min ~12 / max ~32
cells), equal-share shrinking, and an overflow strategy (scroll-with-chevrons, dropdown,
or hybrid), with consistent truncation.

Scope: `freminal/src/gui/menu.rs` (tab bar rendering), `freminal/src/gui/window.rs` (tab
rect computation).

Open questions (decide at activation): min/max widths (cell vs pixel); overflow strategy;
tab pinning; whether OSC titles and custom names follow the same truncation rules (the
bug report notes OSC titles truncate where custom names of the same length don't — confirm
consistency first). Triaged from `bugs.txt` Idea 4 (2026-05-17).

---

## Design Decisions (provisional)

- **v0.13.0 is keyboard-first.** Every feature has a keybinding and is usable without the
  mouse.
- **No scripting here.** The event hook API (Task 84) is the v0.19.0 capstone; this
  version does not introduce it. Features that might later gain hooks (palette, hints)
  ship self-contained.
- **Every in-window overlay (color picker, palette, hints input) follows
  `freminal-modal-input-suppression`.** A panel the user can't type into is a bug.
