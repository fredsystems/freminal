# PLAN_VERSION_030.md — v0.3.0 "Daily Driver"

## Goal

Make Freminal viable as a primary terminal emulator. This release delivers the core UX features
that users expect from any modern terminal: tabs, proper selection, font zoom, bell handling,
configurable keybindings, clipboard access, drag-and-drop, and a smooth cursor animation.

---

## Task Summary

| #   | Feature                       | Scope        | Status   |
| --- | ----------------------------- | ------------ | -------- |
| 36  | Tabs                          | Large        | Complete |
| 37  | Configurable Key Bindings     | Medium-Large | Complete |
| 38  | Double/Triple-Click Selection | Small-Medium | Complete |
| 39  | Right-Click Context Menu      | Small-Medium | Pending  |
| 40  | Font Zoom                     | Small-Medium | Pending  |
| 41  | Bell Handling (Visual Only)   | Small        | Pending  |
| 42  | Drag-and-Drop                 | Small        | Pending  |
| 43  | OSC 52 Clipboard Read         | Small        | Pending  |
| 44  | Cursor Trail / Smooth Cursor  | Small-Medium | Pending  |

---

## Task 36 — Tabs

### 36 Overview

Add a tab bar to Freminal. Each tab owns its own PTY thread, ArcSwap snapshot, and channel set.
Must include a config option to disable tabs entirely (single-terminal mode).

### 36 Design

**Tab Model:** A new `TabManager` struct in `freminal/src/gui/tabs.rs` owns a `Vec<Tab>` where
each `Tab` holds:

- `arc_swap: Arc<ArcSwap<TerminalSnapshot>>`
- `input_tx: Sender<InputEvent>`
- `pty_write_tx: Sender<PtyWrite>`
- `window_cmd_rx: Receiver<WindowCommand>`
- `clipboard_rx: Receiver<String>`
- `title: String` (from OSC 0/2 title commands)
- `bell_active: bool` (set by bell events, cleared on tab focus + delay)

**Tab Lifecycle:**

- _Create tab:_ Extract the PTY setup code from `normal_run()` in `main.rs` into a reusable
  function that returns a `Tab`. Spawn a new PTY consumer thread per tab.
- _Close tab:_ Drop the `input_tx` sender, which signals the PTY consumer thread to exit.
  If the closed tab is the last tab, close the window.
- _Switch tab:_ Update the active tab index. The GUI `update()` loop reads from
  `tabs[active].arc_swap` instead of a single `arc_swap`.
- _Reorder tab:_ Drag-and-drop reordering within the tab bar (stretch goal; at minimum,
  keyboard shortcuts to move tabs left/right).
- _Rename tab:_ Double-click tab label to edit inline, or use a keyboard shortcut.

**Tab Bar UI:**

- Render as a horizontal strip between the menu bar and the terminal area.
- Each tab shows: title text, close button (×) on hover.
- Active tab is visually distinguished (background color, underline, or bold text).
- A "+" button at the end to create a new tab.
- When `tabs.enabled = false` in config, the tab bar is hidden and only one terminal exists
  (current behavior, no overhead).

**Config Section:**

```toml
[tabs]
# Enable tab support. When false, Freminal runs in single-terminal mode
# with no tab bar (current behavior).
enabled = true

# Where to place the tab bar: "top" or "bottom".
position = "top"

# Show the tab bar even when only one tab is open.
show_single_tab = false
```

**Threading Impact:**

The current `main.rs` creates one PTY thread and passes one `ArcSwap` to the GUI. With tabs,
`FreminalGui` owns a `TabManager` instead of a single `arc_swap`. The `gui::run()` signature
changes to accept initial tab configuration rather than a single channel set — or the initial
tab is created inside `FreminalGui::new()` from the channels passed at startup.

### 36 Subtasks

1. **36.1 — Tab data model and `TabManager`** ✅ _Complete (2026-04-07)_
   Create `freminal/src/gui/tabs.rs` with `Tab` struct and `TabManager`. Include methods:
   `new_tab()`, `close_tab(index)`, `active_tab()`, `switch_to(index)`, `move_tab(from, to)`.
   Unit tests for all operations.
   - Created `Tab` struct with `TabId`, `ArcSwap`, channel senders/receivers, title, bell state,
     and per-tab `ViewState`
   - Created `TabManager` with `new()`, `next_tab_id()`, `active_tab()`, `active_tab_mut()`,
     `active_index()`, `tab_count()`, `iter()`, `iter_mut()`, `add_tab()`, `close_tab()`,
     `switch_to()`, `next_tab()`, `prev_tab()`, `move_tab()`, `move_active_left()`,
     `move_active_right()`
   - Created `TabError` enum with `IndexOutOfBounds`, `CannotCloseLastTab`, `MoveToSelf`
   - 22 unit tests covering all operations, edge cases, and error conditions

2. **36.2 — Extract PTY setup into reusable function** ✅ _Complete (2026-04-07)_
   Refactor `normal_run()` in `main.rs` to extract the PTY thread creation (TerminalEmulator,
   channels, ArcSwap, thread spawn) into a function that returns the components needed for a
   `Tab`. This function will be called for the initial tab and for each subsequent `new_tab()`.
   - Created `TabChannels` struct holding GUI-side endpoints (arc_swap, input_tx, pty_write_tx,
     window_cmd_rx, clipboard_rx)
   - Created `spawn_pty_tab()` function that creates TerminalEmulator, sets theme, creates
     channels, and spawns the PTY consumer thread
   - Extracted `spawn_pty_consumer_thread()` containing the full PTY event loop
   - Simplified `normal_run()` to a 15-line function calling `spawn_pty_tab()` then `gui::run()`

3. **36.3 — Wire `TabManager` into `FreminalGui`** ✅ _Complete (2026-04-07)_
   Replace the single `arc_swap`, `input_tx`, `pty_write_tx`, `window_cmd_rx`, `clipboard_rx`
   fields on `FreminalGui` with a `TabManager`. Update `gui::run()` signature. The `update()`
   loop reads from `tabs.active_tab().arc_swap`. Input events go to `tabs.active_tab().input_tx`.
   - Replaced 5 individual channel/snapshot fields on `FreminalGui` with a single `tabs: TabManager`
   - Removed `view_state` field (now lives per-tab in `Tab`)
   - Changed `gui::run()` signature to accept `Tab` instead of individual channels
   - Updated `FreminalGui::new()` to accept initial `Tab` and build `TabManager`
   - Updated all access sites in `ui()`: snapshot load, scroll sync, resize, window manipulation,
     terminal widget show, blink tick, theme change/preview/revert — all go through
     `self.tabs.active_tab()` / `self.tabs.active_tab_mut()`
   - Updated `normal_run()` and playback path in `main.rs` to construct `Tab` from `TabChannels`
   - Added `TabId::first()` constructor
   - Removed `#[allow(clippy::too_many_arguments)]` from `gui::run()` (down from 9 to 5 params)

4. **36.4 — Tab bar UI rendering** ✅ _Complete (2026-04-07)_
   Implement the tab bar as an egui `TopBottomPanel` (or `CentralPanel` child). Render tab
   labels, close buttons, "+" button. Handle click-to-switch, click-close, and the "+" button.
   Respect `tabs.position` and `tabs.show_single_tab` config.
   - Implemented `show_tab_bar()` with `TabBarAction` enum (`None`, `NewTab`, `SwitchTo`, `Close`)
   - Tab labels render with close "×" buttons and "+" new-tab button
   - Moved PTY spawn logic from `main.rs` to `gui/pty.rs` (`TabChannels`, `spawn_pty_tab()`,
     `spawn_pty_consumer_thread()`)
   - `spawn_new_tab()` and `close_tab()` methods on `FreminalGui`
   - `FreminalGui` stores `args: Args` and `egui_ctx: Arc<OnceLock<egui::Context>>` for spawning

5. **36.5 — Keyboard shortcuts for tabs** ✅ _Complete (2026-04-07)_
   Wire up default shortcuts: Ctrl+Shift+T (new), Ctrl+Shift+W (close), Ctrl+Tab /
   Ctrl+Shift+Tab (next/prev), Ctrl+Shift+1-9 (switch to tab N). These must go through the
   keybindings system (Task 37) if it is implemented first, otherwise hardcode with a TODO
   to migrate.
   - All tab actions wired through Task 37's `BindingMap`/`KeyAction` dispatch system
   - `dispatch_deferred_action()` handles `NewTab`, `CloseTab`, `NextTab`, `PrevTab`,
     `SwitchToTab1-9`, `MoveTabLeft`, `MoveTabRight`
   - `switch_to_tab_n()` helper for 1-indexed tab switching

6. **36.6 — Tab titles from OSC 0/2** ✅ _Complete (2026-04-07)_
   The `WindowManipulation::SetTitle` command currently sets the window title. With tabs, it
   should set the _tab_ title instead. The window title becomes the active tab's title (or
   "Freminal" if no title is set).
   - `handle_window_manipulation()` now takes `tab_title: &mut String` parameter
   - `SetTitleBarText` (OSC 0/2) updates the active tab's title
   - Per-frame window title sync in `ui()`: viewport title bar reflects active tab title

7. **36.7 — Config: `[tabs]` section** ✅ _Complete (2026-04-07)_
   Add `TabsConfig` to `freminal-common/src/config.rs`. Add to `Config`, `ConfigPartial`,
   `apply_partial()`, `validate()`. Update `config_example.toml`. Update
   `nix/home-manager-module.nix`.
   - Added `TabBarPosition` enum (Top/Bottom) and `TabsConfig` struct
   - Wired into `Config`, `ConfigPartial`, `apply_partial()`
   - Tab bar visibility: `tab_count > 1 || config.tabs.show_single_tab`
   - Tab bar position: `Panel::top` or `Panel::bottom` based on config
   - Settings Modal "Tabs" tab with checkbox and ComboBox
   - Updated `config_example.toml` and `nix/home-manager-module.nix`

8. **36.8 — Per-tab `ViewState`** ✅ _Complete (2026-04-07)_
   Each tab needs its own `ViewState` (scroll offset, selection, blink state, mouse state).
   Move `ViewState` into `Tab` or maintain a parallel `Vec<ViewState>` in `TabManager`.
   Ensure switching tabs preserves each tab's scroll position and selection.
   - `ViewState` stored per-tab in `Tab` struct since 36.3
   - All GUI code accesses view state via `self.tabs.active_tab().view_state`
   - 4 unit tests: isolation across switch, preserved after close, preserved after move,
     new tabs start with default ViewState

9. **36.9 — Tests and verification** ✅ _Complete (2026-04-07)_
   Unit tests for `TabManager` operations, config parsing, and keyboard shortcut dispatch.
   Integration test: create tab, switch, close, verify no panic or leak.
   - 22 TabManager unit tests (36.1), config parsing tests (36.7), 4 ViewState isolation
     tests (36.8)
   - Full verification suite passes: `cargo test --all`, `cargo clippy`, `cargo-machete`

### 36 Primary Files

- `freminal/src/gui/tabs.rs` (new)
- `freminal/src/gui/mod.rs` (`FreminalGui`, `gui::run()`)
- `freminal/src/main.rs` (`normal_run()`)
- `freminal-common/src/config.rs` (`TabsConfig`)
- `freminal/src/gui/view_state.rs` (per-tab state)
- `config_example.toml`
- `nix/home-manager-module.nix`

---

## Task 37 — Configurable Key Bindings

### 37 Overview

Replace all hardcoded keyboard shortcuts with a data-driven keybinding system. Users can
remap any action via the `[keybindings]` config section. The Settings Modal displays and
allows editing of bindings.

### 37 Design

**Action Enum:** A `KeyAction` enum in `freminal-common/src/keybindings.rs` enumerating every
bindable action:

```rust
pub enum KeyAction {
    // Tab actions
    NewTab,
    CloseTab,
    NextTab,
    PrevTab,
    SwitchToTab(u8),  // 1-9
    MoveTabLeft,
    MoveTabRight,
    RenameTab,

    // Selection / clipboard
    Copy,
    Paste,
    SelectAll,

    // Search
    OpenSearch,

    // Font
    ZoomIn,
    ZoomOut,
    ZoomReset,

    // UI
    ToggleMenuBar,
    OpenSettings,

    // Scrollback
    ScrollPageUp,
    ScrollPageDown,
    ScrollToTop,
    ScrollToBottom,
    ScrollLineUp,
    ScrollLineDown,

    // Future actions added here...
}
```

**Key Combination:** A `KeyCombo` struct: `{ key: egui::Key, modifiers: egui::Modifiers }`.

**Binding Map:** `KeyBindings` wraps a `HashMap<KeyCombo, KeyAction>` with a `default()` that
matches the current hardcoded behavior. Config deserialization overlays user customizations.

**Dispatch:** In `terminal/input.rs`, before any hardcoded match arms, check
`keybindings.get(&current_combo)`. If a match is found, dispatch the action. Otherwise, fall
through to the existing terminal input handling (character input, KKP, mouse reports, etc.).

### agents.md Requirement

Task 37 establishes a rule: **every future feature that introduces a keyboard shortcut MUST
add a corresponding `KeyAction` variant and a default binding.** This ensures all shortcuts
are discoverable and configurable.

### 37 Subtasks

1. **37.1 — `KeyAction` enum and `KeyCombo` struct** ✅ _Complete (2026-04-06)_
   Create `freminal-common/src/keybindings.rs`. Define `KeyAction`, `KeyCombo`, `KeyBindings`.
   Implement `Default` for `KeyBindings` matching current hardcoded shortcuts.
   - Added `BindingKey` enum (letters, digits, F-keys, navigation, editing, symbols)
   - Added `BindingModifiers` struct with constants (`NONE`, `CTRL`, `SHIFT`, `CTRL_SHIFT`, `ALT`)
   - Added `KeyCombo` with `Display`/`FromStr` (parses "Ctrl+Shift+T" format)
   - Added `KeyAction` enum (31 variants) with `Display`/`FromStr`/`Serialize`/`Deserialize`
   - Added `BindingMap` with `lookup`/`bind`/`unbind`/`apply_overrides` and standard defaults
   - 46 unit tests — all passing

2. **37.2 — Config: `[keybindings]` section** ✅ Complete
   Design TOML syntax for keybindings. Add deserialization. Add to `Config`, `ConfigPartial`,
   `validate()`. Update `config_example.toml` with documented examples.
   - Added `KeybindingsConfig` struct with `HashMap<String, String>` overrides, `#[serde(flatten)]`
   - Added to `Config`, `ConfigPartial`, `apply_partial()` (additive merge across layers)
   - Added validation: rejects unknown action names and invalid combo strings
   - Added `build_binding_map()` method on `Config` to produce a `BindingMap` from defaults + overrides
   - `skip_serializing_if` keeps empty keybindings out of serialized output
   - Updated `config_example.toml` with full documentation of all available actions and their defaults
   - 16 new unit tests covering deserialization, partial merging, round-trip, validation, and binding map construction

3. **37.3 — Key dispatch refactor** ✅ _Complete (2026-04-06)_
   Refactor `terminal/input.rs` to check `KeyBindings` before hardcoded logic. All current
   shortcuts (copy, paste, scroll, etc.) must go through the binding system.
   - Added `egui_key_to_binding_key()` and `egui_mods_to_binding_mods()` conversion functions
   - Added `dispatch_binding_action()` handling Copy and all 6 scroll actions
   - Added binding-map pre-check in event loop before PTY dispatch (both `Event::Key` and `Event::Copy`)
   - `BindingMap` stored on `FreminalGui`, rebuilt on settings apply, threaded to widget and input
   - Simplified `Event::Copy` arm — `Ctrl+Shift+C → Copy` now handled by pre-check

4. **37.4 — Settings Modal: keybindings tab** ✅ _Complete (2026-04-06)_
   Add a "Keybindings" tab to the Settings Modal showing all actions and their current bindings.
   Allow editing (click binding → press new key combo → save). Respect `managed_by` read-only
   mode.
   - Added `Keybindings` variant to `SettingsTab` enum and `ALL` array
   - Added `show_keybindings_tab()` method rendering a grid of all 31 actions
   - Added `show_keybinding_row()` free function with text-edit fields seeded from effective map
   - Added `KeyAction::display_label()` for human-friendly action names in UI
   - Extracted `draw_active_tab()` helper to keep `show()` under 100-line limit
   - Read-only mode (managed_by) automatically disables all edit fields
   - Tests updated: `all_tabs_present` (7→8), `settings_tab_labels` (+Keybindings)

5. **37.5 — Home-manager module update** ✅ _Complete (2026-04-06)_
   Add `keybindings` options to `nix/home-manager-module.nix` so Nix users can declaratively
   configure keybindings.
   - Added `keybindings` option as `attrsOf str` with example and full action list in description
   - Added `keybindingsSection` to config attrset builder with conditional inclusion
   - Default is empty attrset (no overrides), only included in generated TOML when non-empty

6. **37.6 — Update `agents.md`** ✅ _Complete (2026-04-06)_
   Add the keybinding mapping rule to `agents.md` under a new "Keybinding Convention" section:
   all new features with keyboard shortcuts must add `KeyAction` variants and default bindings.
   - Added "Keybinding Convention" section with 4-step checklist (KeyAction variant, default
     binding, dispatch handler, config_example.toml documentation)
   - Forbids hardcoded shortcuts outside the BindingMap system

7. **37.7 — Tests** ✅ _Complete (2026-04-06)_
   Unit tests: default bindings produce correct actions, custom bindings override defaults,
   config round-trip, invalid combos rejected. Integration: verify dispatch works end-to-end.
   - 14 new tests in `keybindings.rs`: `display_label` non-empty/distinct, `name()` round-trip,
     default bindings for NextTab/PrevTab/ZoomOut/ZoomReset/CloseTab, ZoomIn specific combos,
     total binding count (26), unbound actions confirmed absent, combo_for determinism
   - Fixed `combo_for()` non-deterministic iteration by deriving `Ord` on `BindingKey`,
     `BindingModifiers`, `KeyCombo` and using `.min()` instead of `.find()`
   - `all_combos_for()` now returns sorted results for consistency

### 37 Primary Files

- `freminal-common/src/keybindings.rs` (new)
- `freminal-common/src/config.rs`
- `freminal/src/gui/terminal/input.rs`
- `freminal/src/gui/settings.rs`
- `nix/home-manager-module.nix`
- `config_example.toml`
- `agents.md`

---

## Task 38 — Double/Triple-Click Selection

### 38 Overview

Implement double-click word selection and triple-click line selection. Currently the selection
model in `view_state.rs` (`SelectionState`) only supports single-click press-drag-release.

### 38 Design

**Click-Count Tracking:** Add to `ViewState`:

- `last_click_time: Option<Instant>` — timestamp of the previous primary click
- `last_click_pos: Option<CellCoord>` — position of the previous primary click
- `click_count: u8` — 1 for single, 2 for double, 3 for triple

On primary button press: if the new click is within ~400ms and ~1 cell of the previous click,
increment `click_count` (capping at 3). Otherwise reset to 1.

**Word Selection (double-click):** On `click_count == 2`, expand the selection anchor and end
to the word boundaries around the clicked cell. Word characters: alphanumeric + configurable
set (default: `_`). When dragging after a double-click, extend by whole words.

**Line Selection (triple-click):** On `click_count == 3`, expand selection to the entire
logical line (including soft-wrapped continuations). When dragging after a triple-click,
extend by whole lines.

### 38 Subtasks

1. ✅ **38.1 — Click-count tracking in `ViewState`** (2026-04-07)
   Added `last_click_time`, `last_click_pos`, `click_count` fields to `ViewState`.
   `CellCoord` struct, `DOUBLE_CLICK_TIMEOUT` (400ms), `DOUBLE_CLICK_MAX_CELL_DISTANCE` (1 cell),
   `register_click()` method with proximity and timeout logic.

2. ✅ **38.2 — Word boundary detection** (2026-04-07)
   `word_boundaries(col, row, visible_chars)` and `line_boundaries(row, visible_chars)` free
   functions. `is_word_char()` helper (alphanumeric + underscore). `collect_row_cells()` helper
   for extracting cell content from `TChar` data.

3. ✅ **38.3 — Word selection and drag-by-word** (2026-04-07)
   On double-click, set anchor to word start, end to word end. During drag with `click_count == 2`,
   snap the moving endpoint to word boundaries.

4. ✅ **38.4 — Line selection and drag-by-line** (2026-04-07)
   On triple-click, set anchor to line start (col 0), end to line end. During drag with
   `click_count == 3`, snap to whole lines.

5. ✅ **38.5 — Tests** (2026-04-07)
   24 unit tests: click-count transitions (first click, rapid clicks, slow reset, distant reset,
   proximity threshold), word boundary detection (single word, punctuation, underscore, empty row,
   second row, clamp beyond row), line boundary detection (simple, single char, empty, second row),
   integration tests (single/double/triple click → point/word/line selection).

### 38 Primary Files

- `freminal/src/gui/view_state.rs` (`ViewState`, `SelectionState`, `CellCoord`, boundary helpers)
- `freminal/src/gui/terminal/input.rs` (mouse press/drag handlers with click-count branching)

---

## Task 39 — Right-Click Context Menu

### 39 Overview

Add a right-click context menu in the terminal area offering Copy, Paste, Select All, Clear,
and Open URL (when right-clicking a detected URL).

### 39 Design

Use egui's built-in context menu (`response.context_menu()`). Menu items:

- **Copy** — enabled when selection exists; copies selected text to clipboard
- **Paste** — always enabled; pastes clipboard contents as terminal input
- **Select All** — selects the entire visible buffer
- **Clear** — clears the terminal (sends `\x1b[2J\x1b[H` or similar)
- **Search** — opens the search overlay (when A.3 exists in v0.4.0; stub for now)
- **Open URL** — enabled when the right-clicked cell is part of a detected URL; opens in
  default browser

### 39 Subtasks

1. **39.1 — Context menu rendering**
   Add context menu to the terminal widget in `terminal/widget.rs`. Wire up Copy, Paste,
   Select All, Clear actions.

2. **39.2 — URL detection on right-click**
   The snapshot already contains URL data. On right-click, determine if the clicked cell is
   within a URL span. If so, add "Open URL" menu item that calls `open::that(url)`.

3. **39.3 — Tests**
   Unit tests for URL span detection at a given cell coordinate.

### 39 Primary Files

- `freminal/src/gui/terminal/widget.rs`
- `freminal/src/gui/terminal/input.rs`

---

## Task 40 — Font Zoom

### 40 Overview

Implement Ctrl+Plus / Ctrl+Minus / Ctrl+0 font size changes. Currently
`options.zoom_with_keyboard = false` at `gui/mod.rs:57`. eframe's built-in zoom changes the
UI scale, not the font size — we need to change the actual font size, rebuild the glyph atlas,
and reflow the terminal.

### 40 Design

On zoom keystroke:

1. Adjust `config.font.size` (clamp to 4.0–96.0 range)
2. Trigger atlas rebuild via `font_manager` (new font metrics → new cell size)
3. Send `InputEvent::Resize` with the new character dimensions
4. Persist the new font size to config (optional; could be session-only)

Ctrl+0 resets to the configured default font size.

### 40 Subtasks

1. **40.1 — Zoom key handling** ✅
   `ZoomIn`/`ZoomOut`/`ZoomReset` `KeyAction` variants and default bindings already existed
   from Task 37. Wired `dispatch_deferred_action` to adjust `zoom_delta` on the active tab's
   `ViewState` and call `apply_font_zoom`.

2. **40.2 — Atlas rebuild on font size change** ✅
   Added `FontManager::set_font_size()` for direct size changes without a full config rebuild.
   Added `FreminalTerminalWidget::apply_font_zoom()` that sets the size and clears atlas/shaping
   caches when the size actually changed.

3. **40.3 — Terminal reflow on cell size change** ✅
   Per-frame zoom sync in the `CentralPanel` closure calls `apply_font_zoom(effective)` before
   `cell_size()` is read, so the existing resize-detection logic automatically sends
   `InputEvent::Resize` when cell dimensions change. Also handles tab switching: when the
   active tab changes, its `zoom_delta` produces a different effective size, triggering a
   font rebuild and resize on the next frame.

4. **40.4 — Tests** ✅
   15 new unit tests: `ViewState` zoom helpers (effective size, adjust, clamp min/max, reset,
   accumulate, negative step, preserved across base change) and `FontManager::set_font_size`
   (metrics change, no-op on same size, cache clearing).

### 40 Primary Files

- `freminal/src/gui/view_state.rs` (`zoom_delta` field, `effective_font_size`, `adjust_zoom`, `reset_zoom`)
- `freminal/src/gui/font_manager.rs` (`set_font_size`)
- `freminal/src/gui/terminal/widget.rs` (`apply_font_zoom`)
- `freminal/src/gui/mod.rs` (zoom dispatch + per-frame zoom sync)

---

## Task 41 — Bell Handling (Visual Only)

### 41 Overview

Implement visual bell. Currently `TerminalOutput::Bell` is logged and ignored at
`terminal_handler/mod.rs:1579`. No audio bell — visual only.

### 41 Behavior

When a bell is received:

- **If tabs are enabled and multiple tabs exist:** Flag both the originating tab and the
  window title bar with a visual indicator (colored dot, highlight, or brief flash).
- **If tabs are disabled or only one tab exists:** Flag the title bar only.
- **Bell clears** when the user performs any action (key press, mouse move, mouse click)
  after a small delay (~200ms minimum display time).

### 41 Design

**PTY → GUI:** Add a `WindowCommand::Bell` variant. The PTY thread sends this when
`TerminalOutput::Bell` is processed.

**GUI State:** Add `bell_active: bool` and `bell_since: Option<Instant>` to `ViewState`
(or per-tab state if tabs exist). On receiving `WindowCommand::Bell`, set `bell_active = true`
and `bell_since = Some(Instant::now())`.

**Rendering:** When `bell_active`, draw a subtle visual indicator:

- Title bar: append " 🔔" or flash the title bar background briefly.
- Tab bar: highlight the tab that fired the bell (different background color).

**Clearing:** On any user input event, if `bell_active` and at least 200ms has elapsed since
`bell_since`, clear the bell state.

### 41 Subtasks

1. **41.1 — Forward bell from terminal handler to GUI**
   In `terminal_handler/mod.rs`, replace the `debug!("Bell (ignored)")` with code that pushes
   a bell event to the window command list. Add `WindowCommand::Bell` (or use
   `WindowManipulation`).

2. **41.2 — GUI bell state and rendering**
   Add bell state to `ViewState`. In `update()`, check for `WindowCommand::Bell` and set state.
   Render visual indicator. Clear on user interaction.

3. **41.3 — Tab-aware bell display**
   If tabs exist (Task 36), bell flags the specific tab. When the user switches to that tab,
   the tab bell clears.

4. **41.4 — Config**
   Add `[bell]` section to config:

   ```toml
   [bell]
   # "visual" or "none". Default: "visual".
   mode = "visual"
   ```

   Add to `Config`, `ConfigPartial`, home-manager module, settings modal, `config_example.toml`.

5. **41.5 — Tests**
   Unit tests: bell state transitions, clearing logic, config parsing.

### 41 Primary Files

- `freminal-buffer/src/terminal_handler/mod.rs` (bell forwarding)
- `freminal-terminal-emulator/src/io/mod.rs` (`WindowCommand::Bell` or equivalent)
- `freminal/src/gui/mod.rs` (bell handling in `update()`)
- `freminal/src/gui/view_state.rs` (bell state)
- `freminal-common/src/config.rs` (`BellConfig`)

---

## Task 42 — Drag-and-Drop

### 42 Overview

Handle file drag-and-drop onto the terminal window. When files are dropped, paste the
shell-escaped file path(s) into the terminal input stream.

### 42 Design

egui provides `ctx.input(|i| i.raw.dropped_files.clone())`. On drop:

1. For each dropped file, get the path.
2. Shell-escape the path (handle spaces, special characters).
3. If multiple files, join with spaces.
4. Send the escaped string as `InputEvent::Key(bytes)`.

Also handle `hovered_files` to show a visual indicator (border flash or overlay) when files
are being dragged over the window.

### 42 Subtasks

1. **42.1 — Drop handling**
   In the terminal widget's `update()`, check for `dropped_files`. Shell-escape paths and
   send as key input.

2. **42.2 — Hover indicator**
   When `hovered_files` is non-empty, render a subtle border or overlay to indicate the drop
   target.

3. **42.3 — Shell escape utility**
   Implement `shell_escape(path: &str) -> String` that handles spaces, quotes, backslashes,
   and other special characters. Unit tests for edge cases.

4. **42.4 — Tests**
   Unit tests for shell escaping. Integration: drop event produces correct bytes.

### 42 Primary Files

- `freminal/src/gui/terminal/widget.rs` (drop handling)
- `freminal/src/gui/mod.rs` (hover indicator, if needed at the window level)

---

## Task 43 — OSC 52 Clipboard Read

### 43 Overview

Fix OSC 52 clipboard query to return actual clipboard contents instead of an empty payload.
The current code at `gui/mod.rs:598` responds with an empty OSC 52 because the comment claims
"egui's public API does not support reading the clipboard." This is incorrect — egui provides
`ui.ctx().input(|i| i.raw.events.clone())` and the `clipboard_text()` method through
`egui::Context`.

### 43 Design

In the `WindowManipulation::QueryClipboard(sel)` handler:

1. Read the system clipboard via egui's `ctx.input(|i| i.events)` looking for paste events,
   or use `ctx.output_mut(|o| ...)` — investigate the exact egui API.
2. If clipboard reading through egui is not sufficient (the API may only provide paste events,
   not arbitrary reads), use the `arboard` crate which is already a transitive dependency of
   egui.
3. Base64-encode the clipboard contents.
4. Send `\x1b]52;{sel};{base64}\x1b\\` as the response.

**Security:** Clipboard read should be gated behind a config option:

```toml
[security]
# Allow applications to read the system clipboard via OSC 52.
# Default: false (applications can write but not read the clipboard).
allow_clipboard_read = false
```

### 43 Subtasks

1. **43.1 — Clipboard read implementation**
   Determine the correct egui or arboard API for reading clipboard contents. Implement in
   the `QueryClipboard` handler.

2. **43.2 — Config: `[security]` section**
   Add `SecurityConfig` with `allow_clipboard_read: bool` (default false). Add to `Config`,
   `ConfigPartial`, `config_example.toml`, home-manager module.

3. **43.3 — Tests**
   Unit tests: config parsing, base64 encoding of response. Integration: verify response
   format matches OSC 52 spec.

### 43 Primary Files

- `freminal/src/gui/mod.rs` (QueryClipboard handler)
- `freminal-common/src/config.rs` (`SecurityConfig`)
- `config_example.toml`
- `nix/home-manager-module.nix`

---

## Task 44 — Cursor Trail / Smooth Cursor

### 44 Overview

Animate cursor movement — the cursor smoothly interpolates between positions instead of
jumping instantly. A visual polish feature popularized by Ghostty.

### 44 Design

**Animation State:** Add to `ViewState`:

- `cursor_visual_pos: (f32, f32)` — current rendered position (fractional cell coords)
- `cursor_target_pos: (f32, f32)` — target position from the snapshot's cursor
- `cursor_anim_start: Instant` — when the animation started

**Animation Logic:** Each frame in `update()`:

1. Read the cursor position from the snapshot.
2. If it differs from `cursor_target_pos`, start a new animation: set target, record start time.
3. Interpolate `cursor_visual_pos` toward `cursor_target_pos` using ease-out over ~80-120ms.
4. If the animation is in progress, `request_repaint()` to ensure smooth frames.

**Config:**

```toml
[cursor]
# Enable smooth cursor animation between positions.
trail = false
# Animation duration in milliseconds.
trail_duration_ms = 100
```

### 44 Subtasks

1. **44.1 — Cursor animation state in `ViewState`**
   Add animation fields. Implement interpolation logic with configurable duration and
   ease-out curve.

2. **44.2 — Wire animation into render loop**
   In the terminal widget, use `cursor_visual_pos` instead of the snapshot's cursor position
   when rendering the cursor. Request repaints during active animations.

3. **44.3 — Config: cursor trail options**
   Add `trail` and `trail_duration_ms` to `CursorConfig`. Update `config_example.toml`,
   home-manager module, settings modal.

4. **44.4 — Tests**
   Unit tests: interpolation math, animation completion, config parsing.

### 44 Primary Files

- `freminal/src/gui/view_state.rs` (animation state)
- `freminal/src/gui/renderer/gpu.rs` (cursor rendering position)
- `freminal-common/src/config.rs` (`CursorConfig` extension)
- `config_example.toml`
- `nix/home-manager-module.nix`

---

## Dependency Graph

```text
Task 37 (Key Bindings) ─── should complete before or alongside ──► Task 36 (Tabs)
  │                                                                    │
  └── Tab shortcuts go through keybinding system                       │
                                                                       │
Task 41 (Bell) ─── tab-aware bell depends on ──────────────────────────┘
Task 36 (Tabs) ─── tab titles from ──► OSC 0/2 (already implemented)

All other tasks are independent of each other.
```

**Recommended order:** 37 → 36 → 41 → (38, 39, 40, 42, 43, 44 in parallel)

If speed is preferred over ideal ordering, 36 and 37 can be developed in parallel with hardcoded
tab shortcuts migrated to keybindings afterward.

---

## Config Schema Additions Summary

This release adds the following config sections:

```toml
[tabs]
enabled = true
position = "top"
show_single_tab = false

[keybindings]
# Action = "Modifier+Key" pairs
# new_tab = "Ctrl+Shift+T"
# close_tab = "Ctrl+Shift+W"
# ... (documented examples in config_example.toml)

[bell]
mode = "visual"

[security]
allow_clipboard_read = false

[cursor]
# Existing fields...
trail = false
trail_duration_ms = 100
```

All new config sections must be propagated to:

- `freminal-common/src/config.rs` (structs, defaults, validation)
- `config_example.toml` (documented examples)
- `nix/home-manager-module.nix` (Nix options)
- `freminal/src/gui/settings.rs` (Settings Modal UI)

---

## Completion Criteria

Per `agents.md`, each task is complete when:

1. All subtasks marked complete
2. `cargo test --all` passes
3. `cargo clippy --all-targets --all-features -- -D warnings` passes
4. `cargo-machete` passes
5. Benchmarks show no unexplained regressions for render/buffer changes
6. Config schema additions propagated to config.rs, config_example.toml, home-manager, settings
