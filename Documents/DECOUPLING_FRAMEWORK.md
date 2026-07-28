# Decoupling Framework — Owning the Frame Schedule

> **Status: DIRECTION REOPENED — leaning AGAINST the rewrite.** An earlier
> revision of this file declared the rewrite ratified. **Phase 0 overtook that
> ruling.** Measurement found three cheap fixes inside egui that between them
> recovered most of the benefit the rewrite was meant to deliver, so the
> maintainer's position is now *leaning no* — explicitly undecided, not closed.
>
> Read §2A before acting on anything below. Phases 1-5 are retained as the
> plan-of-record **if** the rewrite is chosen, and Phase 1 is worth doing
> regardless, but do **not** treat the rewrite as agreed.
>
> This is **not** a `PLAN_VERSION_*.md` and these tasks are **not** in
> `MASTER_PLAN.md`.

## 0. TL;DR

The original thesis was: egui cannot give freminal a demand-driven render
model, because its contract is "an input event runs a frame", `egui-winit`
returns `repaint: true` unconditionally for nearly every window event, and
upstream has declined to change this for three years.

**That thesis is correct about egui's design and wrong about the consequences
being unavoidable.** Phase 0 found three defects — two ours, one a workable
override — that together took idle frame cost down 13.4% and pointer-motion
frame rate from ~61fps to ~2fps. See §2A.

What survives as an argument for the rewrite is **not performance**. It is:

1. Reliance on undocumented egui internals (13 catalogued assumptions, two
   flagged untested by their own authors).
2. Edge cases that keep surfacing in the damage-tracking machinery.
3. How ugly that machinery becomes as those edge cases accumulate.

Those are real and unresolved, but they are a maintainability judgement, not a
measurement. **Nobody should read this document as evidence that the numbers
justify a rewrite. They do not, and after Phase 0 they justify it less than
when this file was first written.**

## 1. How we got here (evidence trail)

Real-workload CPU profiling (GitHub issue #459) on a slower laptop produced
three merged PRs, all still valid and independent of this plan:

1. **PR #460** — `FaceId` font caches to `FxHashMap`. Real idle-CPU win.
2. **PR #461** — command-block gutter hover repaint over-firing. The first fix
   attempt was caught in adversarial review as BLOCKING because it relied on
   `Response::hovered()` lagging `PointerState::latest_pos()` by one frame — an
   **egui internal**. Merged under the name of #459 item 2 without validating
   that it actually fixed item 2. It did not (§2).
3. **PR #464** — region-gate pointer-move full-present, and skip the redundant
   PTY-thread repaint for no-op inputs. Moved idle CPU to ~0.0 during typing.

### Why fighting egui kept failing review

An "input decoupling" design (forward PTY-bound events at winit-event time
without scheduling a GUI frame) was attempted. Each adversarial review pass
found a **new** egui-internal dependency:

- **Event-queue accumulation.** Calling `on_window_event` without running a
  frame accumulates `egui::Event`s (drained only at `take_egui_input`), so a
  later frame double-forwards. *Note: this obstacle turned out to be
  overstated — `State::egui_input_mut()` is public, so the queue can be
  drained manually. It does not rescue the overall position.*
- **`interact_pos` vs `latest_pos` one-frame lag** on window exit (PR #461).
- **`on_keyboard_input`'s `is_cmd` / `is_printable_char` gating** determines
  whether egui emits `Event::Text`; a fast path must mirror it exactly.
- **The winit-to-`egui::Key` map is private** (`key_from_winit_key`,
  `key_from_key_code`), so it must be duplicated.
- **IME composition state** must never be stolen by an intercept.
- **Overlay/modal focus suppression** is frame-local, recomputed inside
  `update()`; an event-time intercept has no flag to consult.
- **Zero-modifier keybindings** are legal config, so "no modifier implies not a
  binding" is unsafe; the intercept needs `BindingMap` access at event time.

The pattern is the point. This is not a converging bug list — each fix
re-derives a piece of egui's private frame/input model.

### Upstream position (checked directly, 2026-07)

`egui-winit 0.35.0`'s `on_window_event` returns `repaint: true` for **every**
`WindowEvent` variant except three (`ActivationTokenDone`, `AxisMotion`,
`DoubleTapGesture`). There is no gating on pointer position or hit-test.

- Issue #3017 (2023), emilk: *"there is no way to turn if off at the moment,
  but feel free to open a PR."*
- Restated in #5387 (2024, proposal never implemented — `raw_input_hook` still
  returns `()`), #7371 (2025), #8326 (2026). Never fixed.
- **0.35.0 is the latest release.** No upgrade path helps.
- No public API answers "would running a frame change any output?" There is no
  headless/no-paint mode; `run_ui` always re-runs every widget closure.
  `ViewportOutput::repaint_delay` is forward-looking only, computed after the
  frame already ran.

This is a considered design stance (mouse-move repaint is deemed correct for
hover highlighting), not an oversight awaiting a patch.

## 2. Measurements (Task 121 harness, commit `0620cc60`)

A feature-gated harness (`frame-profiling`, non-default) instruments the drawn
frame path. Nesting is `run_frame` wraps `run_ui` wraps `App::update` wraps
`central_body` wraps per-pane `show()`. Enable with:

```sh
cargo run --release --features frame-profiling
```

```text
RUST_LOG=none,freminal::frame_profiling=debug,freminal_windowing::frame_profiling=debug
```

### Genuine idle: single window, cold start, blinking cursor, untouched

Steady-state 120-frame interval, differenced between flushes to avoid warm-up
contamination of the cumulative means. Release build.

| Bucket                                 | us/frame | Share |
| -------------------------------------- | -------- | ----- |
| freminal's own (`phase_app_update`)    | 96       | 22%   |
| — of which chrome construction         | 69       | 16%   |
| — of which terminal band               | 14       | 3%    |
| — of which orchestration               | 13       | 3%    |
| egui's own                             | 89       | 21%   |
| present (`swap`)                       | 226      | 52%   |
| unmeasured residual                    | 23       | 5%    |
| total (`phase_total`)                  | 434      |       |

Also measured at idle: **1.95 fps, exactly the 2 Hz cursor blink** (idle
scheduling is already correct), and **115 of 120 frames presented `Partial`**
(the `#435` partial-present path works).

### The headline finding

**`ChromeMode::Replay` engaged 0 times in 360 idle frames. Duty cycle 0.0%.**

This confirms #459 item 2. The `#436` chrome-cache subsystem has never once
engaged in a real idle session. The 69 us/frame of chrome construction is
precisely what it exists to eliminate. If it worked, freminal's own per-frame
cost would drop from 96 us to roughly 27 us.

### Active workload (PTY output every 50 ms)

| Metric                        | Value                       |
| ----------------------------- | --------------------------- |
| `chrome_mode_replay`          | 0 of 840 frames             |
| `zero_change_presented`       | 179 of 840 (21%)            |
| freminal's own                | 77 us/frame (36%)           |
| egui's own                    | 56 us/frame (27%)           |
| present (`swap`)              | 70 us/frame (33%)           |
| observed frame rate           | ~40 fps for ~20 PTY events/s |

Two findings here: **21% of frames presented with nothing changed at all**
(there is no `FrameDamage::None` — an all-`Unchanged` result still returns
`Full`), and roughly **two frames per PTY output event**, matching the
"two real GL frames per keypress" smell noted in #459 item 9.

### What the numbers do and do not justify

- They **do not** justify removing egui on CPU grounds. egui is 21% of an idle
  frame; `swap` alone is 52% and no UI toolkit change touches it.
- Partial present works and **must survive** any rewrite.

## 2A. Phase 0 results — why the direction reopened

Three findings, in the order they landed. All are on
`task-121/repaint-gate-fixes`.

### Finding 1 — `ChromeMode::Replay` had never once engaged (FIXED)

Measured 0 Replay frames out of 360 at idle. Root cause: every frame is driven
by `WindowEvent::RedrawRequested`; `egui-winit` returns `repaint: true` for it
(it sits in a grouped arm commented *"Things that may require repaint"*); the
chrome-input gate consumed that as evidence of input; and the same event's
handler then read the flag back via `std::mem::take` ~110 lines later in the
same call. **The event that drove the frame disqualified the frame.**

A one-line carve-out fixed it. Steady-state Replay went 0% → 100%:

| Metric               | Before | After   | Change |
| -------------------- | ------ | ------- | ------ |
| chrome construction  | 69 us  | 10 us   | -86%   |
| freminal's own       | 96 us  | 42 us   | -56%   |
| total per idle frame | 434 us | 376 us  | -13.4% |
| partial present      | 115/120 | 120/120 | —     |

**This destroyed the strongest maintenance argument in this document.** The
claim was "we carry 13 undocumented assumptions for an optimisation that never
fires". It fires now, and it delivers.

### Finding 2 — the frequency axis is real (QUANTIFIED)

Pointer motion over static terminal content: **58-61fps versus 1.95fps idle**
(~2% of a core), with **95% of frames changing zero pixels**. The
repaint-cause harness named the culprit exactly — `egui-0.35.0/src/context.rs`
`begin_pass` → `InputState::wants_repaint_after()`, which returns
`Duration::ZERO` whenever `!self.events.is_empty()`. Any input event, and egui
demands an immediate repaint of itself. Not gated on hit-testing, not on
whether a pixel changed.

Zero freminal call sites appeared in the causes, which excluded the gutter,
scrollbar and cursor-trail hypotheses by measurement rather than argument.

### Finding 3 — it can be suppressed from outside egui (SPIKE, PROVISIONAL)

Suppressing the input side alone achieves **nothing**: suppressed events still
must go to `on_window_event` (egui's pointer state has to stay fresh), so they
queue in `RawInput.events`, so egui re-arms a 16ms frame from *inside* the
frame. Measured: 99.99% of pointer events suppressed, frame rate unchanged at
61fps. egui owns the schedule from both ends.

Overriding `frame_output.repaint_delay` when the only thing since the last
frame was suppressed pointer motion breaks the loop: **61fps → 2.05fps**,
matching the ~2Hz blink rate exactly (which is what shows the window is live,
not stalled). ~2% of a core → ~0.08%.

**PROVISIONAL.** That run was confounded — the tester accidentally
clicked/dragged and left the window partway — and could not be re-run. The
suppressed-event rate (478/s versus 425/s unsuppressed) argues the confound is
small, but **a clean before/after is required before this number is cited as
validated.**

### Where that leaves the decision

The performance case is spent. Idle was already 0.073% of a core; mouse-move
went from ~2% to ~0.08%. Nothing in the measured data justifies a multi-version
rewrite on CPU grounds.

What remains, and what the decision now rests on entirely:

- **Undocumented-internals reliance.** Finding 3's override is freminal
  overriding egui's judgement. It keys off *our* classification rather than
  egui source-line matching, so it should survive version bumps better than
  the 13 existing assumptions — but it is one more thing that has to be
  re-verified on every bump.
- **Edge cases.** The suppression predicate already needed four rounds:
  pane-wide gutter (too coarse, fixed), `has_urls` and `scroll_offset`
  (still coarse, accepted), and animation-in-flight (toasts / resize-HUD,
  structurally invisible to a positional predicate, folded in explicitly).
  Every future always-animating chrome element needs the same treatment.
- **Ugliness.** Judge `pointer_motion_needs_repaint` and
  `effective_repaint_delay` on the branch and decide whether that is a shape
  worth maintaining indefinitely.

**Maintainer position as of the end of Phase 0: leaning against the rewrite,
explicitly undecided.**

### Still unmeasured

**Typing** and **btop** (hidden-cursor / `DECTCEM`) workloads, plus the clean
re-run of Finding 3. All need a human at the machine.

## 3. Target end state (only if the rewrite is chosen)

```text
freminal (binary: GUI)
  ├─ freminal-orchestrator   (NEW, extracted in Phase 1; event triage,
  │                           view window, input encoding, frame decisions)
  ├─ freminal-ui             (NEW, Phase 2; layout, hit-test, popup stack,
  │                           widgets, text fields — replaces egui for the
  │                           main window and probably freminal-windowing)
  ├─ freminal-terminal-emulator ─ freminal-buffer ─ freminal-common
  └─ egui                    (auxiliary OS windows only — settings)
```

### The partition is per OS window, not per widget

This is the load-bearing design decision.

- **Main terminal window: zero egui.** Not negotiable, because a live egui
  `Context` receives every winit event and runs its own repaint bookkeeping
  regardless of what is drawn in it. Moving only the widgets out buys nothing.
- **Auxiliary OS windows: egui, indefinitely.** The settings window is already
  a separate OS window with its own `Context`, no `PerWindowState`, and it
  **already runs full frame-per-event forever by design** — there is a test
  asserting it never Replays. Nobody has ever cared, because it is open rarely
  and briefly. That is the escape hatch for the hardest UI work.

### Why the main window cannot simply drop egui today

The terminal band is not merely hosted by egui, it **is** an egui construct:

- Glyph GL draws are submitted as `egui::PaintCallback` via `ui.painter().add()`.
- The band is a contiguous index range inside `LayerId::background()`'s
  `PaintList`, read via `ctx.graphics()` mid-frame (assumption A8).
- Even under `Replay`, `run_frame` builds an `egui::Ui` and calls `central_body`.
- Scrollbar, gutter hover, bell flash, resize HUD and lock icon are all
  `ui.painter()` / `ui.interact()` calls.

So Phase 3 must move band **hosting** to a direct GL call, not just relocate
widgets.

### What egui removal deletes

Of the 13 documented assumptions in `EGUI_UPGRADE_ASSUMPTIONS.md`, **A5–A13
become dead code** once chrome leaves the main window's `Context`. A1–A4 shrink
but survive as band-hosting mechanics. The `#435`/`#436` three-axis machinery
(`ChromeMode` / `FrameDamage` / `ChromeDamage` / `chrome_input_pending` /
`chrome_repaint_settled` / the chrome cache) goes away by construction.

## 4. What is already built and reusable

Do not rebuild these. They exist, they work, and they are egui-free or nearly so.

| Asset                              | Location                             | Use                              |
| ---------------------------------- | ------------------------------------ | -------------------------------- |
| SDF rounded-rect + shadow shader   | `freminal/src/gui/renderer/toast_pass.rs` | Chrome surfaces, tabs, popups |
| Proportional text-run rendering    | `freminal/src/gui/renderer/toast_text_pass.rs` | All chrome text          |
| Glyph atlas, rustybuzz, swash      | `freminal/src/gui/{atlas,font_manager,shaping}.rs` | Text shaping            |
| EGL partial present, buffer age    | `freminal-windowing/src/gl_context.rs` | Keep verbatim, egui-free       |
| Toast stack (fully bespoke chrome) | `freminal/src/gui/toast.rs`          | Working precedent for the pattern |
| Keybinding model (`BindingMap`)    | `freminal-common/src/keybindings.rs` | Toolkit-neutral already          |

Toasts (issue #433) are the proof of concept: animated, hit-tested,
GL-drawn chrome with its own text pass and no egui widgets.

## 5. Chrome partition

**HOT** must be bespoke (always on screen during steady-state use).
**AUX** can stay on egui in a separate OS window.
Bespoke overlays are a deliberate art direction — visually distinct from
chrome is desirable, not a compromise.

| Element                        | File                  | Bucket | Text input |
| ------------------------------ | --------------------- | ------ | ---------- |
| Menu bar                       | `menu.rs`             | HOT    | no         |
| Tab bar                        | `menu.rs`             | HOT    | no         |
| Tab rename editor              | `menu.rs`             | HOT    | yes        |
| Scrollbar                      | `terminal/widget.rs`  | HOT    | no         |
| Command-block gutter and folds | `terminal/widget.rs`  | HOT    | no         |
| Bell flash                     | `terminal/widget.rs`  | HOT    | no         |
| Resize overlay HUD             | `app_impl.rs`         | HOT    | no         |
| Password lock icon             | `terminal/widget.rs`  | HOT    | no         |
| Context menu                   | `terminal/widget.rs`  | HOT    | no         |
| Toasts                         | `toast.rs`            | HOT    | no (done)  |
| Search overlay                 | `search.rs`           | HOT    | yes        |
| Command-history palette        | `command_history.rs`  | HOT    | yes        |
| Settings (approx. 20 tabs)     | `settings.rs`         | AUX    | yes        |
| Keybinding recorder            | `settings.rs`         | AUX    | yes        |
| Paste guard                    | `paste_guard.rs`      | AUX    | yes (multiline) |
| Close guard                    | `close_guard.rs`      | AUX    | no         |
| Broadcast guard                | `broadcast_guard.rs`  | AUX    | no         |
| About, Welcome, Save Layout    | `menu.rs`, `welcome.rs` | AUX  | yes (name field) |

Keeping AUX on egui reduces the bespoke text-input requirement from nine
surfaces (including a multiline editor and the keybinding recorder, which
depends on egui's already-translated `Event::Key` semantics) to **four
single-line fields**: search, command-history filter, tab rename, and — if it
stays in-window — the save-layout name.

**The menu bar is non-negotiable** (maintainer ruling). It may be disabled
during early phases via the existing `hide_menu_bar` config, but it must
eventually be integrated. It uses `menu_button` 19 times, `ui.close()` 24
times, plus nested submenus and `egui::MenuBar`. That is a real popup/menu
framework — nested submenus, hover-to-switch-sibling, screen-edge flipping,
click-outside dismissal, keyboard navigation, close-all-from-leaf. **No
terminal has prior art for this** (wezterm, kitty and Alacritty have no menu
bar; ghostty gets one free from GTK/AppKit). Consequence: the toolkit's
layout/hit-test/focus foundation must assume a **z-ordered popup stack from day
one**, or Phase 4 forces a foundation rewrite.

## 6. Platform strategy

We keep **winit + glutin**. We write **zero platform backends**. This was
initially muddled; the corrected split is:

| Layer                                              | Platform-specific | Owner              |
| -------------------------------------------------- | ----------------- | ------------------ |
| Window, event loop, raw input, clipboard, IME plumbing | yes           | winit              |
| GL context, surface, partial present               | yes               | glutin, `gl_context.rs` |
| Shape and text drawing                             | no                | ours (exists)      |
| Layout, hit-test, focus, popup stack               | no                | ours (new)         |
| Widgets                                            | no                | ours (new)         |
| IME consumption (preedit render, cursor area)       | no                | ours (new)         |
| Accessibility tree                                 | yes               | AccessKit, later   |

Consequences worth writing down:

- There is **no "Wayland first, X11 behind a feature flag" phase.** winit
  already supports both and freminal already enables both. Platform ordering is
  a **validation** concern only: Wayland, then X11, then macOS, then Windows.
- We do **not** need wezterm's four bespoke IME implementations (XIM,
  `zwp_text_input_v3`, `NSTextInputClient`, IMM32). wezterm needed those because
  it wrote its own windowing layer. winit surfaces `WindowEvent::Ime` for us.
  We build only the consumer side: render the preedit, call
  `Window::set_ime_cursor_area`, commit into our field.
- freminal currently has **zero** custom IME code. All of it comes from
  egui-winit today.

## 7. Rejected alternatives

Do not re-litigate these without new evidence.

| Option                                    | Verdict  | Reason                                                                                                 |
| ----------------------------------------- | -------- | ------------------------------------------------------------------------------------------------------ |
| Keep egui, push the damage machinery further | Rejected | Requires more undocumented-internals dependence; `chrome_repaint_settled` already reads private state (A10) |
| Adopt wezterm's `window` crate            | Rejected | A winit alternative, not a widget toolkit; unpublished; depends on wezterm-internal crates; wezterm does not hit the idle target itself |
| Native toolkit per platform (ghostty)     | Rejected | Triples chrome surface; contradicts one-renderer-everywhere; reintroduces per-platform event loops removed by Tasks 62–66; terminal-content a11y is bespoke regardless |
| Cache chrome to a texture, composite it   | Rejected | Hit-testing needs a live `Ui` behind the pixels; font-atlas staleness worse; adds a second staleness clock; only pays off exactly where `Replay` already should |
| Adopt an existing retained-mode Rust GUI  | Rejected | No mature crate composites into an existing GL context without owning the event loop; candidates are early-stage |
| Drop the menu bar for a command palette   | Rejected | Maintainer ruling: menu bar is non-negotiable                                                          |
| Minimal chrome (Alacritty model)          | Rejected | Tabs, muxing, layouts and settings are shipped product identity                                        |

On IME and accessibility as reasons to keep egui: freminal gets **zero
AccessKit today** — `egui-winit`'s `accesskit` feature is not in its default
set, freminal does not enable it, and `Cargo.lock` never resolves
`accesskit_winit`. That argument defends a future unbuilt capability, not
something that would be lost. egui's own README claims AccessKit for Windows
and macOS only. On IME, upstream #7975 (X11 + Fcitx5, Korean pre-edit) is open;
Wayland + IBus works.

## 8. Phases

Each phase must leave `cargo test --all` green and the app usable. No
big-bang cutover.

### Phase 0 — measure (mostly DONE, see §2A)

- **0.1** ~~Fix the two `ctx.request_repaint_after` call sites in
  `terminal/widget.rs`~~ — **DONE differently.** The hypothesis was wrong:
  those two sites surface as `not_settled`, which stops firing after warm-up.
  Instrumenting the gate instead identified `RedrawRequested` as the real and
  total blocker (Finding 1). The two call sites remain unfixed and are now
  known to be benign at idle. **Lesson: this hypothesis cost nothing only
  because it was instrumented rather than acted on.**
- **0.2** Capture mouse-move — **DONE** (Finding 2). **Typing and btop still
  outstanding**; both need a human at the machine.
- **0.3** Cross-validate with `perf record --call-graph dwarf,65528
  --no-inline` per the #459 methodology — **NOT DONE.** The in-app harness
  proved sufficient to root-cause all three findings, so this was never
  needed; keep it in reserve if a finding is ever disputed.
- **0.4** Rule on whether `#436` is salvageable — **DONE: yes, it works.**
  It was inert due to a one-line bug, now fixed. This inverts the earlier
  conclusion; see Finding 1.
- **0.5** Write the `DESIGN_DECISIONS.md` entry — **OUTSTANDING.** Must record
  the direction *and* the inconvenient numbers, including that Phase 0
  weakened the case for the rewrite rather than strengthening it.
- **0.6** **NEW, required before the spike is trusted:** clean, unconfounded
  before/after run of Finding 3.
- **0.7** **NEW:** decide whether the spike's animation-in-flight term is
  sufficient or needs a general "something is animating" signal, and whether
  the `has_urls` / `scroll_offset` pane-wide approximations are acceptable
  permanently or need per-span geometry.

### Phase 1 — orchestration extraction (no behaviour change)

Required under any outcome, including abandoning the rewrite. This is the
prerequisite that makes Phase 3 possible: a toolkit cannot be swapped under a
2,743-line function welded to egui's calling convention.

- **1.1** Decompose `App::update` (2,743 lines) and `central_body` (1,859).
- **1.2** Decompose `terminal/widget.rs::show` (1,851 lines).
- **1.3** Decompose `terminal/input.rs::write_input_to_terminal` (1,226 lines,
  16 parameters, returns a 7-tuple; mixes egui event parsing, VT byte encoding,
  `ViewState` mutation, PTY channel writes, clipboard, kitty-keyboard logic and
  broadcast fan-out).
- **1.4** Introduce toolkit-neutral `Rect` / `Point` in `freminal-common` and
  move `panes/mod.rs` layout, resize and hit-test math off `egui::Rect` /
  `Pos2` (44 occurrences). The math is already toolkit-agnostic in spirit.
- **1.5** Rename `gui_scroll_offset` / `gui_extra_rows` and their setters on
  `TerminalEmulator` (4 fields, 3 methods, one file, no propagation into
  `freminal-buffer`). Cosmetic naming leak only.
- **1.6** Design the orchestration layer **as if it were a crate** but land it
  as a module first. Extract the crate as the final mechanical step; crate
  boundaries are the highest-friction refactor to undo.

Note: the crate graph is **already clean**. `freminal-common`,
`freminal-buffer` and `freminal-terminal-emulator` have zero egui in
`Cargo.toml` and zero live egui-typed code — every apparent hit is a doc
comment. `TerminalSnapshot`, `InputEvent` and `WindowCommand` are egui-free.
The coupling is concentrated in the binary's god functions, not across crates.

### Phase 2 — UI layer foundation, standalone

Developed as a new crate against its own example binary, before touching the
freminal binary. Keeps winit + glutin.

- **2.1** Crate skeleton, example binary, winit + glutin integration reusing
  `gl_context.rs`.
- **2.2** Layered surfaces with a **z-ordered popup stack**, topmost-wins
  hit-testing, focus and dismissal semantics, keyboard navigation. Required on
  day one because of the menu bar ruling.
- **2.3** A `Response`-equivalent: hovered, clicked, dragged, drag_started,
  drag_stopped.
- **2.4** Retained/damage-aware draw model that answers "did anything visibly
  change?" natively — the question egui refuses to answer.
- **2.5** Wire in the existing SDF surface pass and text-run pass.
- **2.6 — THE GATE.** One text field, end to end, **including IME**: cursor,
  selection, clipboard, preedit rendering, `set_ime_cursor_area`. Prototype the
  search box. See "Abort criteria".
- **2.7** Scrollable list (needed by the palettes).

### Phase 3 — cut the main window over

- **3.1** Move band hosting off `egui::PaintCallback` to a direct GL call from
  `RedrawRequested`.
- **3.2** Own the frame schedule. Add a genuine "nothing changed, do not
  present" path (the `FrameDamage::None` that does not exist today).
- **3.3** Port HOT chrome: tab bar, scrollbar, gutter, bell, resize HUD, lock
  icon, context menu, search, command-history, tab rename. Toasts already done.
- **3.4** Ship with `hide_menu_bar` forced on.
- **3.5** Retire the `#435`/`#436` three-axis machinery and assumptions A5–A13.
  **Two things must be REPLACED, not merely deleted** — both are now measured
  to work and removing them without an equivalent is a regression:
  - **Partial present** — 120/120 idle frames present as `Partial`.
  - **Chrome caching** — since the Finding 1 fix, `Replay` engages on 100% of
    steady-state idle frames and saves 69 us → 10 us of chrome construction
    per frame (-86%). An earlier revision of this document said "delete"; that
    was written when the subsystem was inert and is wrong now.
- **3.6** Keep egui alive for auxiliary windows only.

### Phase 4 — menu bar

- **4.1** Nested popup/menu system on the Phase 2 foundation.
- **4.2** Re-enable the menu bar; remove the forced `hide_menu_bar`.
- **4.3** Decide the macOS system-menu-bar question (deferred until here).

### Phase 5 — egui endgame

Decide whether settings stays on egui permanently (legitimate) or is ported.
No decision needed before this point.

## 9. Abort criteria

A plan without a stop condition is a bad plan.

- **If subtask 2.6 (text field with IME) proves intractable, stop.** Keep egui,
  accept the residual, and treat Phases 0 and 1 as the deliverable. They are
  net wins regardless and carry no regret.
- **If Phase 0.2 shows the mouse-move frequency problem is not real**, the
  performance rationale collapses entirely and only the maintenance rationale
  remains. Re-ratify before starting Phase 2.
- **If any phase cannot preserve the partial-present path**, stop and redesign.

## 10. Invariants that must survive

From the `freminal-architecture` skill. A rewrite is not a licence to break these.

- The PTY thread owns `TerminalEmulator` exclusively; the GUI never holds a
  reference to it.
- Snapshot transport stays `ArcSwap<TerminalSnapshot>`, read-only on the GUI side.
- The GUI render path is a **pure read**. State changes go through `InputEvent`.
- `ViewState` is GUI-owned and never shared with the PTY thread. It currently
  has exactly 3 egui-typed fields, which Phase 1.4 neutralises.
- Crate dependencies point one direction only.
- Every keyboard shortcut goes through `BindingMap`. No hardcoded shortcuts.
- Panic-free production code; no `unwrap()` / `expect()` outside tests.
- No raw `as` numeric casts in production; use `conv2`.

## 11. Roadmap interaction

Deliberately **not** resolved here, by maintainer instruction: do not spend
effort now reordering future versions around this work. Re-sequence once
Phase 1 lands.

For whoever does that later, the relevant pressure is that Tasks 96 (per-pane
title bar), 97 (dynamic tab width and overflow) and 85 (powerline status bar)
are all always-visible chrome, and the v0.18/v0.19 AI-assist versions add
modals. Anything built on egui before Phase 3 inherits the port.

Also note: **Task 121 is not in `MASTER_PLAN.md`** despite four merged commits
under that name (PRs #460, #461, #464 and commit `0620cc60`). That tracking gap
should be closed when the roadmap is next touched.

## 12. Pointers

- Issue #459 — profiling methodology, the three-axis breakdown, the
  `perf --call-graph dwarf,65528` gotchas, and the still-open candidate list.
- Issue #440 — no headless-GL or pixel-readback harness exists; the "436.9
  pixel harness" never landed.
- PR #461 — canonical example of how subtle egui-internal dependence is, and a
  cautionary example of validation deferred and never done.
- PR #464 — the two landed fixes; the `post_event` classifier is a clean
  example of orchestration logic wanting a home.
- Commit `0620cc60` — the frame-profiling harness and the §2 numbers.
- Commit `436a54f1` — gate-blocker + per-signal instrumentation; how Finding 1
  was isolated rather than guessed.
- Commit `7d483998` — **the `RedrawRequested` fix (Finding 1).** Worth reading
  even if the rewrite is abandoned; it is the single highest-value change of
  the whole investigation.
- Commit `ab88d0f5` — repaint-cause instrumentation; how Finding 2 named egui's
  `context.rs` as the culprit and excluded freminal's own call sites.
- Commit `19780e16` — **the suppression spike (Finding 3).** Read
  `pointer_motion_needs_repaint` and `effective_repaint_delay` here to judge
  the "how ugly does this get" question directly.
- `Documents/EGUI_UPGRADE_ASSUMPTIONS.md` — assumptions A1–A13; A6 and A13 are
  flagged by their own authors as untested.
- `.opencode/skills/freminal-architecture` — invariants in §10.
- `.opencode/skills/freminal-egui-upgrade` — the re-verification tax this work
  eliminates.
