# PLAN_VERSION_100.md — v0.10.0 "The Power-User Toolkit"

## Goal

Ship the features that turn freminal into a power user's daily driver: named profiles,
live theme preview, in-session ligature toggles, GPU-accelerated scrollback search, and
quick-select / hint mode for keyboard-driven selection.

Depends on v0.8.0 (correctness) and v0.9.0 (modern workflow primitives).

---

## Task Summary

| #   | Feature                                 | Scope  | Status | Depends On |
| --- | --------------------------------------- | ------ | ------ | ---------- |
| 78  | Profiles + Quick Profile Switching      | Medium | Stub   | v0.8.0     |
| 79  | Theme Preview + Color Picker            | Medium | Stub   | v0.8.0     |
| 80  | Font Ligatures Per-Profile Toggle       | Small  | Stub   | Task 78    |
| 81  | GPU-Accelerated Scrollback Regex Search | Medium | Stub   | v0.8.0     |
| 82  | Quick-Select / Hints Mode               | Medium | Stub   | v0.8.0     |
| 83  | Command Palette                         | Medium | Stub   | v0.8.0     |
| 83a | Expanded Auto-Detection (TENTATIVE)     | Medium | Stub   | 71.7b      |

Tasks 82 and 83 absorb FUTURE_PLANS.md items B.3 and B.2 respectively.

**Task 83a is TENTATIVE and may be removed.** It is a follow-up brainstorm from Task 71.7b
(auto-detect plain URLs). A future agent MUST NOT begin work on Task 83a without the
maintainer explicitly greenlighting it. Do not promote it to "Pending" or activate it
autonomously.

---

## Task 78 — Profiles + Quick Profile Switching

### 78 Summary

A profile is a named bundle of (theme, font, shell, startup command, env vars, bell mode,
ligature setting, opacity). Profiles live in `~/.config/freminal/profiles/*.toml`. The
new-tab menu gains a profile picker; a configurable keybinding cycles profiles.

### 78 Open Questions (decide at activation)

- Relationship to Workspace Env (Task 75) — is a profile a subset of a workspace, or
  orthogonal?
- Profile inheritance / composition semantics.
- Migration from current single-config users.

---

## Task 79 — Theme Preview + Color Picker

### 79 Summary

Settings → Theme hovers produce a live preview of the terminal with that theme applied.
For custom themes, a dedicated palette editor with a visible sample line beats hand-editing
TOML. Applies immediately or on explicit Save.

### 79 Open Questions (decide at activation)

- Preview scope: preview the Settings panel only vs. preview the real active terminal.
- Custom theme format / location (`~/.config/freminal/themes/*.toml`).
- Color picker widget: egui-native, or custom for better accessibility.

---

## Task 80 — Font Ligatures Per-Profile Toggle

### 80 Summary

Ligatures (Task 5) are currently a global toggle. Move to per-profile (Task 78) and add an
in-session keybinding to toggle on/off without reloading config.

### 80 Open Questions (decide at activation)

- Default: on or off per new profile.
- Whether to toggle individual ligature groups (`liga`, `calt`, `dlig`) independently.

---

## Task 81 — GPU-Accelerated Scrollback Regex Search

### 81 Summary

Leverage the existing glyph atlas / GPU pipeline to run regex search across the entire
scrollback buffer efficiently. Live highlight of matches as the user types the query. Jump
to next/previous match with keybindings.

### 81 Open Questions (decide at activation)

- Regex engine: `regex` crate vs. `fancy-regex`.
- Highlight rendering: quad overlay vs. cell background override.
- Search scope: current pane, current tab, all panes in window.

---

## Task 82 — Quick-Select / Hints Mode

### 82 Summary

Absorbs `FUTURE_PLANS.md` item B.3. Enter hints mode (configurable keybinding), and every
detected pattern (URL, file path, git hash, IP, number) is highlighted with a two-letter
label. Typing the label copies or opens the target.

### 82 Open Questions (decide at activation)

- Pattern registry: built-in vs. user-configurable regex list.
- Label alphabet (home-row Colemak-friendly vs. QWERTY).
- Action per pattern type (open URL, copy path, open file, `$EDITOR file:line`).

---

## Task 83 — Command Palette

### 83 Summary

Absorbs `FUTURE_PLANS.md` item B.2. `Ctrl+Shift+P` (configurable) opens a fuzzy-search
palette listing all `KeyAction`s with their current bindings. Selecting an action executes
it.

### 83 Open Questions (decide at activation)

- Fuzzy-match algorithm (skim, fzf-style, simple substring).
- Whether to include non-`KeyAction` commands (layout load, profile switch, theme picker).
- History and frecency ordering.

---

## Design Decisions (provisional)

- **v0.10.0 is keyboard-first.** Every feature has a keybinding and is usable without the
  mouse.
- **No scripting yet.** Scripting is the backbone of v0.11.0; we do not introduce it
  halfway here.

---

## Task 83a — Expanded Auto-Detection (TENTATIVE)

**STATUS: TENTATIVE. May be removed. Do not start without maintainer approval.**

This task is a brainstorm follow-up to Task 71.7b, which landed auto-detection of plain
`http`/`https`/`file`/`ftp`/`mailto` URLs in terminal output. The maintainer has not
committed to shipping any of the expansions below. A future agent encountering this task
MUST stop and ask before doing any implementation work. It exists here so the ideas are
not lost, not as a commitment to build them.

### 83a Summary

Extend the existing `freminal-buffer/src/url_detect.rs` machinery to recognize more
clickable patterns beyond fully-qualified URLs. Three independent sub-features, each
shippable on its own:

1. **Absolute file paths** — `/etc/nginx/nginx.conf`, `/Users/fred/src/foo.rs:42:8`
2. **Schemeless URLs** — `github.com`, `docs.rs/regex`, `localhost:8080`
3. **Explicit relative paths** — `./src/main.rs`, `../Cargo.toml`

All of these fit into the existing flatten-cache overlay architecture with no new PTY
hot-path work. `UrlMatch` would gain a `kind: UrlKind` field so click dispatch can branch
on the match type.

### 83a.1 Absolute file paths with optional `:LINE[:COL]` suffix

Detection via `(?:^|[\s("'\[])(/[^\s"'<>()\[\]]+)` with trailing-punctuation stripping and
an optional `:NNN` or `:NNN:NNN` tail.

**Existence checking:** optimistic detection, no `stat()` during flatten. Filesystem I/O
on the flatten path is forbidden (blocking NFS, stalled mounts). Click attempts `xdg-open`
or spawns `$EDITOR +LINE path` for paths with a line suffix; failure shows an error toast.

Highest-value of the three — covers rustc/clippy/grep/cargo output. Low risk. Main false
positive is `/etc/passwd`-style references in prose, which users can live with.

### 83a.2 Schemeless URLs

Detection via `\b([a-z0-9-]+\.)+[a-z]{2,}(:\d+)?(/[^\s]*)?\b` gated on a TLD allow-list
(~30 entries from the Public Suffix List — `com`, `org`, `io`, `dev`, `net`, `rs`, `gov`,
`edu`, …).

**Critical caveat:** the `.rs` / `.io` / `.sh` / `.dev` TLDs collide with common Rust/shell
filenames. Mitigation: for ambiguous TLDs, require either a path segment (`/foo`) or port
(`:8080`) after the host. Drops bare `module.rs` but keeps `docs.rs/regex`.

Click behavior prepends `https://` (configurable).

**Opt-in only.** Default `false` in config. The false-positive risk on `.rs` files in a
Rust-first terminal is bad enough that it must not be default-on.

Config surface:

```toml
[ui]
auto_detect_schemeless_urls = false
auto_detect_schemeless_scheme = "https"
```

### 83a.3 Explicit relative paths (`./` and `../` only)

Detection via `(?:^|\s)(\.{1,2}/[^\s"'<>()\[\]]+)`. Nearly zero false positives. Cheap.

**Requires OSC 7** for click-to-open. Relative paths only resolve cleanly when the pane's
CWD is known. Freminal already tracks CWD via `/proc/PID/cwd` for layout save, but many
shells need configuration to emit OSC 7 reliably. If OSC 7 is absent and `/proc` lookup
fails, the path is still detected (styled) but click surfaces an error.

**Bare relative paths** (`src/buffer/flatten.rs` with no leading `./`) are explicitly
**out of scope.** False-positive risk is too high and every token containing a `/` would
light up.

### 83a Open Questions (decide at activation)

- Should `UrlMatch.kind` be exposed to the Settings UI for per-kind enable/disable, or is
  one global `auto_detect_urls` toggle enough?
- `$EDITOR` vs `$VISUAL` vs a config-driven `editor_command` template for file:line clicks.
- TLD allow-list: hardcoded static list vs. embedded PSL subset vs. user-configurable.
- Do we also want git SHA detection (`abc1234` → `$BROWSER origin/tree/abc1234`) and issue
  references (`#1234`, `JIRA-123`)? Both would need their own config schema. Out of scope
  for 83a but natural extensions.
- Behavior when OSC 7 is unavailable: silently skip relative path detection, or detect
  with a "missing CWD" hover tooltip?

### 83a Non-goals

- No filesystem I/O on the flatten path. Ever.
- No bare relative path detection (too lossy).
- No hover-time existence verification in v1 (possible polish pass later).
- No integration with scripting (v0.11.0 Task 84).

### 83a Effort estimate (rough, for planning only)

| Sub-feature                          | Effort | Risk                              |
| ------------------------------------ | ------ | --------------------------------- |
| 83a.1 Absolute paths + `:LINE[:COL]` | 1–2 d  | Prose false positives (tolerable) |
| 83a.2 Schemeless URLs                | 2 d    | `.rs`/`.io` TLD collisions        |
| 83a.3 Explicit relative paths        | 1 d    | OSC 7 dependency, CWD plumbing    |

Total: ~5 days if all three ship together. Each is independently landable.

### 83a Reminder

**This task is TENTATIVE.** It exists as captured thinking, not a commitment. Do not start
work without explicit maintainer approval.
