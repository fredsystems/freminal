# PLAN_VERSION_070.md — v0.7.0 "Recording & Layouts"

## Goal

Remove the playback system entirely, redesign the recording format (FREC v2) for multi-window,
multi-pane session capture with rich event correlation, introduce a layout system that lets
users define, save, and restore complete workspace configurations including window geometry,
and triage platform-specific performance issues on Windows and macOS.

---

## Task Summary

| #   | Feature                           | Scope  | Status   | Dependencies     |
| --- | --------------------------------- | ------ | -------- | ---------------- |
| 59  | FREC v2: Recording Overhaul       | Large  | Complete | Task 58          |
| 61  | Saved Layouts (Session Templates) | Large  | Pending  | Task 36, Task 58 |
| 68  | Platform Performance Triage       | Medium | Complete | None             |
| 69  | UI Polish & Settings Completeness | Medium | Complete | None             |

Task 60 (Playback v2) has been removed — see "Design Decisions" below.

---

## Task 59 — FREC v2: Recording Overhaul

### 59 Overview

The current recording system has three problems:

1. **FREC v1 is inadequate.** It is a flat stream of timestamped PTY output chunks — no pane
   identity, no topology events, no input correlation, no window awareness. It cannot record
   multi-window or multi-pane sessions.
2. **The playback system is being removed.** Multi-window/multi-pane replay is not worth the
   complexity. Recording remains valuable for diagnostics, debugging, and session analysis.
3. **The `playback` feature flag adds 57 `cfg` sites.** The feature flag was introduced in
   Task 32 to gate playback code. With playback removed, the flag is unnecessary — recording
   should always be compiled and available via `--recording-path`.

Task 59 does three things:

1. **Delete all playback code** and the `playback` feature flag.
2. **Delete FREC v1** — no backward compatibility needed (only one user, ever).
3. **Implement FREC v2** — a rich, multi-window recording format with per-pane stream
   isolation, input correlation, topology tracking, and human-input event capture.

### 59 Format Design

#### File Structure

```text
┌─────────────────────────────────────────────────────┐
│ File Header                                          │
│   Magic: b"FREC"  (4 bytes)                          │
│   Version: 0x02   (1 byte)                           │
│   Flags: u32 LE   (4 bytes, reserved — e.g. compression) │
│   Metadata Length: u32 LE                             │
│   Metadata: MessagePack / bincode blob                │
├─────────────────────────────────────────────────────┤
│ Event Stream (sequential, variable-length records)    │
│   Record 0: { timestamp_us, event_type, ... }         │
│   Record 1: ...                                       │
│   ...                                                 │
│   Record N: ...                                       │
├─────────────────────────────────────────────────────┤
│ Seek Index (written at finalization)                  │
│   Index entry count: u64 LE                           │
│   Per entry: { timestamp_us: u64, file_offset: u64 }  │
│   Index interval: every ~1 second of recording time   │
├─────────────────────────────────────────────────────┤
│ Footer                                               │
│   Seek index offset: u64 LE (byte offset of index)   │
│   Total duration: u64 LE (microseconds)               │
│   Total events: u64 LE                                │
│   Magic: b"FREC" (4 bytes, for reverse scanning)      │
└─────────────────────────────────────────────────────┘
```

The seek index enables future tools (external replay, analysis scripts) to jump to arbitrary
positions without scanning the entire file. Even without playback in Freminal itself, the index
is cheap to produce and valuable for `sequence_decoder.py` and third-party tooling.

#### Metadata Block

Serialized as a structured binary format (MessagePack or bincode — decided during
implementation based on dependency weight). Contains:

```rust
struct RecordingMetadata {
    freminal_version: String,           // e.g. "0.7.0"
    created_at: u64,                    // Unix epoch seconds
    term: String,                       // e.g. "xterm-256color"
    initial_topology: TopologySnapshot, // full window/tab/pane tree at recording start
    scrollback_limit: u32,
}

struct TopologySnapshot {
    windows: Vec<WindowSnapshot>,
}

struct WindowSnapshot {
    window_id: u32,
    position: Option<(i32, i32)>,       // x, y in pixels (None if unknown/Wayland)
    size: (u32, u32),                   // width, height in pixels
    tabs: Vec<TabSnapshot>,
    active_tab: u32,                    // tab_id
}

struct TabSnapshot {
    tab_id: u32,
    window_id: u32,
    pane_tree: PaneTreeSnapshot,
    active_pane: u32,                   // pane_id
    zoomed_pane: Option<u32>,           // pane_id
}

struct PaneTreeSnapshot {
    node: PaneNodeSnapshot,
}

enum PaneNodeSnapshot {
    Leaf {
        pane_id: u32,
        cols: u32,
        rows: u32,
        cwd: Option<String>,
        shell: Option<String>,
        title: String,
    },
    Split {
        direction: SplitDirection,
        ratio: f32,
        first: Box<PaneNodeSnapshot>,
        second: Box<PaneNodeSnapshot>,
    },
}
```

#### Event Types

Each event record in the stream has a common header:

```text
[0..8]   u64 LE   timestamp_us (elapsed from recording start)
[8]      u8       event_type
[9..13]  u32 LE   payload_length
[13..]   payload  (event_type-specific)
```

Event types:

| Type ID | Name           | Payload                                                                                          |
| ------- | -------------- | ------------------------------------------------------------------------------------------------ |
| 0x01    | PtyOutput      | pane_id: u32, data: [u8]                                                                         |
| 0x02    | PtyInput       | pane_id: u32, data: [u8]                                                                         |
| 0x03    | PaneResize     | pane_id: u32, cols: u32, rows: u32                                                               |
| 0x04    | WindowResize   | window_id: u32, width_px: u32, height_px: u32                                                    |
| 0x05    | TabCreate      | window_id: u32, tab_id: u32, pane_id: u32, cols: u32, rows: u32                                  |
| 0x06    | TabClose       | window_id: u32, tab_id: u32                                                                      |
| 0x07    | PaneSplit      | window_id: u32, parent_pane: u32, new_pane: u32, direction: u8, ratio: f32, cols: u32, rows: u32 |
| 0x08    | PaneClose      | pane_id: u32                                                                                     |
| 0x09    | FocusChange    | window_id: u32, tab_id: u32, pane_id: u32                                                        |
| 0x0A    | ZoomToggle     | window_id: u32, tab_id: u32, pane_id: u32, zoomed: u8                                            |
| 0x0B    | TabSwitch      | window_id: u32, tab_id: u32                                                                      |
| 0x0C    | ThemeChange    | theme_name: String (length-prefixed)                                                             |
| 0x0D    | KeyboardInput  | window_id: u32, pane_id: u32, key_name_len: u16, key_name: [u8], modifiers: u8, encoded: [u8]    |
| 0x0E    | MouseMove      | window_id: u32, pane_id: u32, x: u32, y: u32, coalesced_count: u32                               |
| 0x0F    | MouseButton    | window_id: u32, pane_id: u32, button: u8, pressed: u8, x: u32, y: u32                            |
| 0x10    | MouseScroll    | window_id: u32, pane_id: u32, delta_x: f32, delta_y: f32                                         |
| 0x11    | WindowCreate   | window_id: u32, width_px: u32, height_px: u32, x: i32, y: i32                                    |
| 0x12    | WindowClose    | window_id: u32                                                                                   |
| 0x13    | WindowFocus    | window_id: u32, focused: u8                                                                      |
| 0x14    | ClipboardPaste | pane_id: u32, data_len: u32, data: [u8]                                                          |
| 0x15    | BellEvent      | pane_id: u32, bell_type: u8                                                                      |
| 0x16    | SelectionEvent | pane_id: u32, start_row: u32, start_col: u32, end_row: u32, end_col: u32, is_block: u8           |
| 0x17    | WindowMove     | window_id: u32, x: i32, y: i32                                                                   |

**Event design notes:**

- **PtyOutput (0x01)** is the primary data event — PTY read data tagged with a pane ID.
- **PtyInput (0x02)** records bytes sent TO the PTY. Exists for diagnostics — correlating
  what the user typed with what the terminal produced.
- **KeyboardInput (0x0D)** records the human-readable key name (e.g. "Ctrl+C", "Enter",
  "Alt+F4") plus the raw encoded bytes that were sent to the PTY. This enables correlating
  a PtyInput event with the keypress that generated it — the `key_name` field is the
  human-readable form, `encoded` is the raw bytes (which will also appear as a PtyInput
  event for the target pane).
- **MouseMove (0x0E)** is debounced to ~10 writes/second. The `coalesced_count` field
  records how many raw mouse move events occurred since the last MouseMove write, preserving
  the information that the mouse was actively moving without flooding the recording.
- **WindowCreate/WindowClose (0x11/0x12)** track window lifecycle. Tab and pane events
  include `window_id` to scope them to the correct window.
- **SelectionEvent (0x16)** records text selection for clipboard copy.
- **WindowMove (0x17)** records window position changes (no-op on Wayland where position
  is compositor-managed, but recorded on X11/macOS/Windows).

#### Window ID Assignment

`winit::window::WindowId` is opaque and not serializable. The recording system assigns its
own monotonically increasing `u32` window IDs at recording time. A mapping from
`winit::window::WindowId` → `u32` is maintained by the recording writer. The first window
gets ID 0, subsequent windows get incrementing IDs.

Similarly, `TabId` and `PaneId` are already `u64` internally but are recorded as `u32` in
FREC v2 (sufficient for any practical session — 4 billion panes).

### 59 Subtasks

1. **59.1 — Remove playback code and feature flag**
   Delete all playback-related code:
   - `freminal/src/playback.rs` (entire file)
   - `freminal/src/lib.rs` — remove `mod playback`
   - Remove all 57 `#[cfg(feature = "playback")]` sites across the workspace
   - Remove `playback` feature from all three `Cargo.toml` files
     (`freminal`, `freminal-terminal-emulator`, `freminal-common`)
   - Remove `--playback-file` / `--with-playback-file` CLI flag from args
   - Remove `PlaybackCommand`, `PlaybackMode`, `PlaybackState` types
   - Remove playback-related `InputEvent` variants
   - Remove playback UI code from `gui/menu.rs` and `gui/mod.rs`
   - Remove playback test infrastructure (`tmux_scroll_replay.rs` if playback-only)
   - Make `--recording-path` always available (no feature gate)
   - Run full verification suite to confirm clean compilation.

2. **59.2 — Remove FREC v1 format**
   Delete the v1 format code in `freminal-terminal-emulator/src/recording.rs`:
   - Remove `write_header()`, `write_frame()`, `parse_recording()`, `PlaybackFrame`
   - Remove any v1 test fixtures (`.bin` files in `tests/`)
   - The file will be gutted and rebuilt with v2 types in 59.3.

3. **59.3 — Define FREC v2 format types**
   Create the Rust types for the v2 format: `RecordingMetadataV2`, `RecordingEvent`,
   `EventType` enum (all 23 event types), `TopologySnapshot`, `WindowSnapshot`,
   `TabSnapshot`, `PaneNodeSnapshot`, `SeekIndexEntry`. Place in
   `freminal-terminal-emulator/src/recording.rs` (or a new `recording/` module directory
   if needed). All types must be serializable. Choose and justify the serialization format
   (MessagePack via `rmp-serde`, bincode, or raw manual encoding). Add unit tests for
   round-trip serialization of all types.

4. **59.4 — v2 file writer: header, metadata, events**
   Implement the full v2 writer:
   - `RecordingWriter` struct that owns a `BufWriter<File>` on a dedicated writer thread
   - `write_header()` — magic, version, flags, serialized metadata
   - `write_event()` — appends a single event record
   - Events arrive from multiple threads via a bounded crossbeam channel
   - On recording stop (or process exit), write the seek index and footer
   - Implement graceful finalization on both clean exit and SIGTERM
     Unit tests: write + read back header/metadata, write event sequences, verify ordering.

5. **59.5 — v2 file parser (for sequence_decoder.py and tests)**
   Implement `parse_recording_v2()` in Rust that reads header, metadata, and event stream.
   This is used by integration tests — the primary analysis tool is `sequence_decoder.py`.
   Two modes: full load (small files) and indexed streaming (large files via seek index).
   Unit tests: parse files written by 59.4, verify all fields.

6. **59.6 — Hook PTY output recording**
   In the PTY reader thread (`pty.rs`), replace the v1 `write_frame()` call with
   `write_event(PtyOutput { pane_id, data })`. Each pane's PTY reader knows its pane ID
   (threaded through at spawn time).

7. **59.7 — Hook PTY input recording**
   Every byte sent TO the PTY (`PtyWrite::Write` for keyboard, paste, report responses)
   is captured as a `PtyInput` event. `PtyWrite::Resize` additionally generates a
   `PaneResize` event.

8. **59.8 — Hook keyboard and mouse input events**
   In the GUI input handling path (`terminal/input.rs`):
   - Before encoding a key event to PTY bytes, emit a `KeyboardInput` event with the
     human-readable key name (e.g. "Ctrl+C", "Shift+Enter") and the encoded bytes.
   - For mouse events, emit `MouseButton` on press/release and `MouseScroll` on scroll.
   - For mouse move, implement debouncing: accumulate moves and emit a `MouseMove` event
     at most 10 times per second, with `coalesced_count` tracking how many raw events
     were absorbed since the last write.
     Emit `ClipboardPaste` when paste content is sent to a pane.
     Emit `SelectionEvent` when a text selection is made.

9. **59.9 — Hook topology and window events**
   In the GUI thread, emit events when:
   - A window is created (`WindowCreate`) or closed (`WindowClose`)
   - A window gains/loses focus (`WindowFocus`)
   - A window moves (`WindowMove`) or resizes (`WindowResize`)
   - A tab is created (`TabCreate`) or closed (`TabClose`)
   - A pane is split (`PaneSplit`) or closed (`PaneClose`)
   - Focus changes between panes (`FocusChange`)
   - Zoom toggles (`ZoomToggle`)
   - The active tab switches (`TabSwitch`)
   - The theme changes (`ThemeChange`)
   - A bell fires (`BellEvent`)
     All topology events include `window_id` for multi-window scoping.

10. **59.10 — Update `sequence_decoder.py` for FREC v2**
    Rewrite the Python decoder to handle v2 format:
    - Auto-detect version byte (reject v1 with a clear error message)
    - Parse and pretty-print the metadata block (requires matching the serialization
      format chosen in 59.3 — if MessagePack, use `msgpack` Python package; if bincode,
      implement the subset manually or use raw struct unpacking)
    - Decode all 23 event types with human-readable labels
    - New CLI flags:
      - `--pane <id>` — filter events to a specific pane
      - `--window <id>` — filter events to a specific window
      - `--event-type <name>` — filter by event type (e.g. `PtyOutput`, `KeyboardInput`)
      - `--events-only` — show only topology/lifecycle events (skip PtyOutput/PtyInput)
      - `--metadata` — show only the metadata block and exit
      - `--summary` — show recording summary (duration, event counts by type, panes, windows)
    - Existing flags (`--convert-escape`, `--split-commands`, `--show-timing`) continue to
      work, applied to PtyOutput/PtyInput payloads.
    - For `KeyboardInput` events, show both the human-readable key name and hex bytes.
    - For `MouseMove` events, show position and coalesced count.

11. **59.11 — Recording CLI updates**
    Update `--recording-path` to produce v2 files. Remove any v1 format selection flags.
    Update `config_example.toml` if any config-level recording options are added.

12. **59.12 — Tests and integration**
    End-to-end test: start a headless multi-pane session, feed input, split panes, resize,
    close panes, stop recording. Parse the resulting file and verify: correct event ordering,
    per-pane data isolation (no interleaving), topology events at correct timestamps, seek
    index validity, keyboard/mouse events present with correct metadata. Test graceful
    finalization on abrupt stop.

### 59 Primary Files

- `freminal-terminal-emulator/src/recording.rs` (or `recording/` module — format types, writer, parser)
- `freminal-terminal-emulator/src/io/pty.rs` (PTY output and input hooks)
- `freminal-terminal-emulator/src/io/mod.rs` (remove playback types)
- `freminal-terminal-emulator/src/interface.rs` (remove playback paths)
- `freminal-terminal-emulator/src/snapshot.rs` (remove playback fields)
- `freminal/src/playback.rs` (DELETE)
- `freminal/src/gui/mod.rs` (topology event emission, remove playback code)
- `freminal/src/gui/menu.rs` (remove playback UI)
- `freminal/src/gui/terminal/input.rs` (keyboard/mouse event capture)
- `freminal/src/gui/panes/mod.rs` (pane ID threading)
- `freminal/src/gui/tabs.rs` (tab lifecycle events)
- `freminal/src/gui/window.rs` (window lifecycle events)
- `freminal-common/src/args.rs` (CLI flag cleanup)
- `freminal-common/Cargo.toml` (remove feature)
- `freminal-terminal-emulator/Cargo.toml` (remove feature)
- `freminal/Cargo.toml` (remove feature)
- `sequence_decoder.py` (v2 format support)

### 59 Design Decisions

1. **Remove playback entirely.** Multi-window, multi-pane replay is a massive engineering
   effort (Task 60 was 12 subtasks) with marginal value. The recording format is still
   valuable for diagnostics and debugging — understanding what happened in a session,
   correlating input to output, analyzing escape sequence behavior. Playback can always
   be reconsidered as a separate project consuming FREC v2 files.

2. **Remove FREC v1.** No backward compatibility needed. Only one person has ever used FREC
   files. The v1 format is too limited to be worth maintaining alongside v2.

3. **Drop the `playback` feature flag.** Recording is lightweight (channel write per event,
   dedicated writer thread). It is always compiled but only active when `--recording-path`
   is specified at runtime. Removing the feature flag eliminates 57 `cfg` sites and
   significant conditional compilation complexity.

4. **Single writer thread via channel.** Events from multiple PTY reader threads and the
   GUI thread are funneled through a bounded crossbeam channel to a dedicated writer thread.
   This avoids file-level locking, guarantees chronological ordering (events are timestamped
   at the source), and keeps I/O off the hot paths.

5. **Mouse move debouncing.** Raw mouse moves can easily reach 100+ events/second. Recording
   all of them wastes space and provides no diagnostic value. Debouncing to ~10 Hz with a
   `coalesced_count` field preserves the information ("the mouse was moving rapidly") without
   flooding the file.

6. **KeyboardInput records both human-readable and raw bytes.** The human-readable form
   ("Ctrl+C") is for human analysis. The raw bytes are for correlation with PtyInput events
   (which contain the same bytes). Together they answer: "what did the user press, and what
   bytes did that produce?"

7. **Window IDs are recording-local.** `winit::window::WindowId` is opaque and
   non-serializable. The recording assigns monotonic `u32` IDs. This decouples the format
   from the windowing backend.

8. **Seek index retained despite no playback.** The index is trivial to produce (one entry
   per second of recording, written at finalization) and enables `sequence_decoder.py` to
   support `--seek-to <timestamp>` in the future, and any third-party tools that may want
   to analyze large recordings efficiently.

---

## Task 61 — Saved Layouts (Session Templates)

### 61 Overview

A layout is a complete description of a Freminal workspace: how many windows, how many tabs
per window, how each tab's panes are arranged, window positions and sizes, and what runs in
each pane. Layouts enable:

- **Startup configuration:** launch Freminal with a predefined multi-window, multi-tab,
  multi-pane workspace tailored to a project
- **Save current state:** capture the running session's topology, working directories,
  window geometry, and programs into a reusable layout file
- **Layout library:** a collection of named layouts in `~/.config/freminal/layouts/`,
  selectable from the menu or Settings Modal
- **Partial application:** load a layout into a new tab without replacing the entire session
- **Auto-restore:** optionally save the layout on exit and restore it on next launch

This task subsumes and expands Task 56 (Session Restore / Startup Commands), which was limited
to flat tab lists with no pane tree or window geometry support.

### 61 Design

#### Layout File Format

Layouts are TOML files. The format is human-readable and hand-editable — a key design goal.

```toml
# ~/.config/freminal/layouts/dev.toml

[layout]
name = "Development"
description = "Standard dev workspace: editor, server, logs"

# Variables — can be overridden from CLI: freminal --layout dev.toml ~/projects/myapp
# $1, $2, etc. are positional args. Named vars use ${VAR_NAME}.
[layout.variables]
project_dir = "~/projects/default"  # default value, overridden by $1 if provided

# --- Window definitions ---
# Each [[windows]] entry defines a separate OS window.
# A layout with no [[windows]] section uses a single default window.

[[windows]]
size = [1200, 800]          # width, height in pixels (optional)
position = [100, 200]       # x, y in pixels (optional, ignored on Wayland)
monitor = 0                 # preferred monitor index (optional, best-effort)

  # --- Tab definitions within this window ---

  [[windows.tabs]]
  title = "Editor"
  active = true  # this tab is focused on launch

    # Pane tree for this tab. The tree is defined as a flat list of nodes.
    # Each node has an "id" (local to this tab) and optionally a "parent" + "position".
    # The root node has no parent.

    [[windows.tabs.panes]]
    id = "root"
    split = "vertical"   # "vertical" (left/right) or "horizontal" (top/bottom)
    ratio = 0.65

    [[windows.tabs.panes]]
    id = "editor"
    parent = "root"
    position = "first"   # "first" (left/top) or "second" (right/bottom)
    directory = "${project_dir}"
    command = "nvim ."
    active = true         # this pane has focus within the tab

    [[windows.tabs.panes]]
    id = "sidebar"
    parent = "root"
    position = "second"
    split = "horizontal"
    ratio = 0.5

    [[windows.tabs.panes]]
    id = "terminal"
    parent = "sidebar"
    position = "first"
    directory = "${project_dir}"

    [[windows.tabs.panes]]
    id = "git"
    parent = "sidebar"
    position = "second"
    directory = "${project_dir}"
    command = "lazygit"

  [[windows.tabs]]
  title = "Server"

    [[windows.tabs.panes]]
    id = "server"
    directory = "${project_dir}"
    command = "cargo watch -x run"

[[windows]]
size = [800, 600]
position = [1350, 200]

  [[windows.tabs]]
  title = "Logs"

    [[windows.tabs.panes]]
    id = "logs"
    directory = "/var/log"
    command = "tail -f syslog"
```

**Backward-compatible shorthand:** A layout with no `[[windows]]` section and top-level
`[[tabs]]` entries is treated as a single-window layout. This keeps simple layouts simple:

```toml
[layout]
name = "Simple"

[[tabs]]
title = "Main"

  [[tabs.panes]]
  id = "main"
  directory = "~/projects"
```

#### Window Properties

| Field      | Type       | Required | Description                                       |
| ---------- | ---------- | -------- | ------------------------------------------------- |
| `size`     | [u32, u32] | No       | Width, height in pixels (default: system default) |
| `position` | [i32, i32] | No       | X, Y in pixels (ignored on Wayland, see below)    |
| `monitor`  | u32        | No       | Preferred monitor index, 0-based (best-effort)    |

**Platform behavior for `position`:**

- **X11:** Full support via `winit::Window::set_outer_position()`.
- **macOS:** Full support (same API, Y is from top-left).
- **Windows:** Full support (same API).
- **Wayland:** `set_outer_position()` is a no-op — the compositor manages window placement.
  Position is still _saved_ (via `outer_position()`, which works on some compositors like
  KDE/KWin) so that a layout saved on Wayland can be restored on X11. When restoring on
  Wayland, the position field is silently ignored. Size restoration works on all platforms.

#### Pane Node Properties

Each `[[windows.tabs.panes]]` entry is either a **split node** (has `split` and `ratio`) or a
**leaf node** (has no `split`). Leaf nodes represent actual terminal panes.

| Field       | Type   | Required | Description                                          |
| ----------- | ------ | -------- | ---------------------------------------------------- |
| `id`        | String | Yes      | Unique within the tab (for parent references)        |
| `parent`    | String | No       | ID of the parent split node (absent for root)        |
| `position`  | String | No       | "first" or "second" within the parent split          |
| `split`     | String | No       | "vertical" or "horizontal" — makes this a split node |
| `ratio`     | Float  | No       | Split ratio (0.0-1.0), default 0.5                   |
| `directory` | String | No       | Working directory (supports `~` and variables)       |
| `command`   | String | No       | Command to run after shell starts                    |
| `shell`     | String | No       | Override the default shell for this pane             |
| `env`       | Table  | No       | Extra environment variables: `env = { FOO = "bar" }` |
| `title`     | String | No       | Initial pane title (before shell OSC overrides)      |
| `active`    | Bool   | No       | If true, this pane/tab has focus on launch           |

A tab with a single pane omits `parent`, `position`, and `split` — just one pane entry with
the leaf properties.

#### Variable Substitution

Layouts support variable substitution for reusability across projects:

- **Positional args:** `$1`, `$2`, etc. from `freminal --layout dev.toml ~/projects/myapp`
  (where `~/projects/myapp` is `$1`)
- **Named variables:** `${VAR_NAME}` references `[layout.variables]` defaults, overridable
  via `--var NAME=VALUE`
- **Environment variables:** `$ENV{HOME}`, `$ENV{USER}` for system env vars
- **Tilde expansion:** `~` is expanded to `$HOME` in `directory` fields

This means one layout file works across multiple projects:

```sh
freminal --layout dev.toml ~/projects/frontend
freminal --layout dev.toml ~/projects/backend
```

#### Save Current Layout

"Save Layout" captures the running session:

- Window count, positions, and sizes
- Tab count and order per window
- Per-tab pane tree structure (split directions, ratios)
- Per-pane working directory (read from `/proc/<pid>/cwd` on Linux)
- Per-pane foreground process (read from `/proc/<pid>/cmdline` on Linux — best-effort,
  may be empty or show the shell)
- Per-pane title (current OSC title)

The output is a valid layout TOML file. Directories are written as absolute paths; the user
can edit the file to use variables afterward.

**Platform note:** CWD and process detection via `/proc` is Linux-specific. On other
platforms (if Freminal is ever ported), these fields are omitted and the saved layout
captures topology and geometry only.

#### Layout Application Modes

Layouts can be applied in three ways:

1. **Startup (replace):** `freminal --layout dev.toml` or `startup.layout = "dev"` in
   config. Creates the workspace from scratch on launch. This is the primary use case.

2. **On-demand replace:** Menu bar > "Layouts" > "Load Layout..." replaces the current
   session with the selected layout. Prompts for confirmation ("This will close all
   current tabs.").

3. **On-demand append:** Menu bar > "Layouts" > "Load Layout in New Tab..." creates the
   layout's tabs as additional tabs in the current session. Useful for "I want my dev
   layout alongside my existing work."

#### Auto-Save and Restore

```toml
[startup]
# Save the current layout on exit and restore it on next launch.
# The layout is saved to ~/.config/freminal/layouts/_last_session.toml
restore_last_session = false
```

When enabled, Freminal saves the current layout to `_last_session.toml` on clean exit.
On next launch (if no `--layout` flag is given), it restores from this file. This captures
topology, geometry, and CWDs but not running programs (since those have exited).

#### Layout Library

Layouts in `~/.config/freminal/layouts/` are discovered automatically. The menu bar shows
them under "Layouts" with their `layout.name` field. The Settings Modal's "Layouts" tab
(new tab) lists all discovered layouts with name, description, and a preview of the
topology.

### 61 Subtasks

1. **61.1 — Layout file format and parser**
   Define the `Layout`, `LayoutWindow`, `LayoutTab`, `LayoutPane`, `LayoutVariables` types.
   Implement TOML parsing with validation: detect orphan nodes (parent references a
   non-existent ID), multiple roots, cycles, missing position on non-root nodes. Validate
   window geometry values (non-negative size, reasonable bounds). Support both `[[windows]]`
   multi-window format and `[[tabs]]` shorthand for single-window layouts. Implement variable
   substitution (`$1`, `${name}`, `$ENV{...}`, `~`). Unit tests: parse valid layouts
   (including multi-window), reject malformed ones, verify variable substitution.

2. **61.2 — Layout application engine**
   Given a parsed `Layout`, create windows and tab/pane trees. For each window: create the
   OS window with specified size and position (best-effort on Wayland). For each leaf pane:
   use `spawn_pty_tab` with `directory` as CWD. Queue `command` for injection after shell
   ready. Set initial titles. Set active tab and active pane per the layout spec.

3. **61.3 — Startup command injection**
   Implement command injection: after a pane's shell is ready (detect via a short delay or
   by watching for the first prompt), send the `command` string as PTY input followed by
   a newline. Handle the "no command" case (just open a shell).

4. **61.4 — Shell and environment overrides**
   Support per-pane `shell` override (use this shell instead of the default). Support
   per-pane `env` table (set additional environment variables on the PTY child process).
   Requires extending `spawn_pty_tab` to accept optional shell and env overrides.

5. **61.5 — CLI integration: `--layout` and `--var`**
   Add `--layout <path_or_name>` flag: if a path, load directly; if a name, search
   `~/.config/freminal/layouts/<name>.toml`. Add `--var NAME=VALUE` (repeatable) for
   variable overrides. Positional args after the layout path become `$1`, `$2`, etc.
   Update arg parsing and config precedence.

6. **61.6 — Save current layout**
   Implement "Save Layout" that captures the current session topology into a `Layout`
   struct and serializes it to TOML. Capture per-window position and size. Read CWDs via
   `/proc/<pid>/cwd`. Read foreground processes via `/proc/<pid>/cmdline` (best-effort).
   Write to a user-chosen path (file dialog or prompt). Unit tests: round-trip save/load
   produces equivalent topology and geometry.

7. **61.7 — Layout library discovery**
   Scan `~/.config/freminal/layouts/` for `.toml` files on startup and when the directory
   changes. Parse each file's `[layout]` section (name, description) without fully parsing
   the tree. Make the list available to the GUI for the menu and Settings Modal.

8. **61.8 — Menu bar integration**
   Add "Layouts" menu to the menu bar with:
   - "Load Layout..." (file picker, replace mode)
   - "Load Layout in New Tab..." (file picker, append mode)
   - "Save Current Layout..." (save dialog)
   - Separator
   - List of discovered layouts from the library (click to load in replace mode)
     Add `KeyAction::LoadLayout` and `KeyAction::SaveLayout` to keybindings.

9. **61.9 — Auto-save and restore**
   Implement `startup.restore_last_session`: on exit, save to `_last_session.toml`
   (including window geometry). On startup (if no `--layout` and
   `restore_last_session = true`), load from `_last_session.toml`. The saved layout
   includes topology, window positions/sizes, and CWDs but not commands (since processes
   have exited — just open shells in the saved directories).

10. **61.10 — Window geometry restoration**
    Implement the platform-aware window position and size restoration:
    - On X11/macOS/Windows: set window position via `set_outer_position()` and size via
      `set_inner_size()` from the layout's window geometry fields.
    - On Wayland: set size only; silently skip position (log a debug-level message).
    - Monitor selection: if `monitor` is specified, attempt to place the window on that
      monitor (query available monitors via `winit`, use the monitor's position as offset
      for the window position). Fall back to primary monitor if the specified index is
      out of range.
    - Handle edge cases: saved position is off-screen (monitor removed), negative
      coordinates, zero size. Apply reasonable fallbacks.
      Unit tests: verify geometry is applied on X11-like platforms, verify graceful
      degradation on Wayland.

11. **61.11 — Config and settings integration**
    Add `[startup]` section to config: `layout`, `restore_last_session`. Add a "Layouts"
    tab to the Settings Modal showing discovered layouts with previews. Update
    `config_example.toml` and home-manager module.

12. **61.12 — Tests and integration** ✅ 2026-04-20
    End-to-end: write a layout file (including multi-window with geometry), launch with
    `--layout`, verify correct window count and geometry, correct tab/pane topology,
    correct CWDs, correct command injection. Test variable substitution with CLI overrides.
    Test save/load round-trip (verify window positions survive round-trip). Test
    auto-restore. Test malformed layouts produce clear error messages. Test partial
    application (append mode). Test single-window shorthand format.
    Added: `multi_window_layout_parses`, `multi_window_layout_round_trip`,
    `save_to_file_and_from_file_round_trip`, `from_file_rejects_malformed_toml`,
    `discover_layouts_finds_valid_files`. All 22 layout tests pass.

### 61 Primary Files

- `freminal-common/src/config.rs` (`StartupConfig`, `LayoutConfig`)
- `freminal-common/src/layout.rs` (new — layout types, parser, variable substitution)
- `freminal/src/gui/mod.rs` (layout application, menu integration)
- `freminal/src/gui/tabs.rs` (layout-driven tab creation)
- `freminal/src/gui/panes/mod.rs` (layout-driven pane tree construction)
- `freminal/src/gui/window.rs` (window geometry restoration)
- `freminal/src/main.rs` (startup layout loading)
- `freminal-common/src/args.rs` (`--layout`, `--var` flags)
- `freminal-common/src/keybindings.rs` (layout key actions)
- `freminal-windowing/src/lib.rs` (window position/size API if not already exposed)
- `config_example.toml`
- `nix/home-manager-module.nix`

### 61 Design Decisions

1. **TOML over JSON.** Consistent with the existing config format. TOML's nesting limitations
   are overcome by the flat node list with parent references — this is more readable than
   deeply nested inline tables would be, and allows the user to define nodes in any order.

2. **Flat node list with parent references.** A tree can be represented in TOML either as
   deeply nested inline tables (ugly, hard to edit) or as a flat list with explicit
   relationships. The flat approach is how many configuration systems handle tree structures.
   Each node is self-contained and easy to add/remove.

3. **Variables for reusability.** Without variables, layouts are project-specific (hardcoded
   paths). Variables make a single layout file work across projects. The `$1` positional
   convention is familiar from shell scripting.

4. **Save captures topology + geometry + CWD, not running programs.** We cannot reliably
   restart an arbitrary program the user was running. Saving CWDs and window geometry means
   the user gets windows in the right positions with shells in the right directories, which
   covers 90% of the restore value. If `command` was specified in the original layout, the
   saved layout preserves it — but the "detected" foreground process from `/proc` is stored
   as a comment, not as an auto-run command, to avoid surprising behavior.

5. **Subsumes Task 56 entirely.** Task 56's `[startup.tabs]` flat list is a strict subset
   of the layout system. The `[startup]` config section is unified under the layout system.

6. **Window geometry is best-effort.** Position restoration is inherently platform-dependent.
   The layout format stores it unconditionally (portable file), but restoration silently
   degrades on Wayland. This is the pragmatic approach — it works on X11/macOS/Windows and
   does the best it can on Wayland. As Wayland compositors add positioning protocols (e.g.,
   xdg-toplevel-position-v1), Freminal can adopt them without format changes.

7. **Multi-window format with single-window shorthand.** The `[[windows]]` wrapper is
   required for multi-window layouts but optional for single-window ones. This keeps
   simple layouts (one window, a few tabs) concise while supporting complex multi-window
   workspaces.

---

## Task 68 — Platform Performance Triage (Windows + macOS)

### 68 Overview

User reports and testing indicate two platform-specific performance problems:

1. **Windows:** High CPU spike on launch that persists for a noticeable duration before
   settling. The root cause is unknown — could be GL context initialization, font discovery,
   glyph atlas population, PTY startup overhead, or a hot loop in the event loop during
   initial rendering.

2. **macOS:** Sustained ~50% single-core CPU usage and ~50% GPU usage during steady-state
   idle (cursor blinking, no PTY output). On Linux the idle CPU is near-zero thanks to
   demand-driven rendering (Task 65). Something on macOS is preventing the event loop from
   sleeping properly — possibly a frame pacing issue, a repaint loop, CGL vsync behavior,
   or egui reporting continuous `needs_repaint`.

Both issues are critical for daily-driver viability on these platforms. This task is
diagnostic-first: measure, identify root causes, then fix. Do not guess at fixes without
profiling data.

### 68 Subtasks

1. **68.1 — Windows launch profiling**
   Profile Freminal startup on Windows using platform-appropriate tools (e.g. `cargo
flamegraph`, ETW traces, or `superluminal`/`tracy` if available). Capture CPU usage
   from process start through first stable idle. Identify the hot path(s) causing the
   spike. Document findings with flamegraph or trace output.

   Areas to investigate:
   - GL context creation and shader compilation (first `Resumed` event)
   - Font discovery and glyph atlas initial population
   - PTY process spawn (`CreatePseudoConsole` or conpty overhead)
   - Event loop spin during initial frames (are we rendering too many frames before settling?)
   - DPI scaling calculations
   - Any synchronous I/O on the main thread during startup

2. **68.2 — macOS idle profiling**
   Profile Freminal at steady-state idle on macOS using `Instruments.app` (Time Profiler +
   GPU profiler) or `cargo flamegraph`. Measure:
   - CPU usage with blinking cursor (should be ~0.1%, waking every 500ms)
   - CPU usage with steady cursor (should be ~0%)
   - GPU usage in both cases (should be ~0% at idle)

   Areas to investigate:
   - Is the event loop sleeping between cursor blink frames? Check `ControlFlow::WaitUntil`
     behavior on macOS — CGL may force vsync wakeups even when no redraw is needed.
   - Is `egui::Context::run()` reporting `needs_repaint = true` every frame? Check for
     animations, tooltip hover state, or widgets that always request repaint.
   - Is the custom terminal renderer being invoked on every frame even when nothing changed?
     Check the dirty-flag skip logic in the render path.
   - Is `swap_buffers()` blocking in a spin-wait rather than sleeping?
   - Are there any timers or channels that wake the event loop continuously?
   - Retina scaling: is the renderer doing 2x–3x the pixel work but frame pacing assumes
     1x timing?

3. **68.3 — Windows launch fix**
   Based on 68.1 findings, implement fixes for the launch CPU spike. Possible interventions
   (dependent on diagnosis):
   - Defer shader compilation or glyph atlas population to a background thread
   - Reduce the number of frames rendered during startup (don't redraw until the first
     PTY output arrives)
   - Move font discovery off the main thread
   - Add startup progress indicator if initialization is inherently slow

4. **68.4 — macOS idle fix**
   Based on 68.2 findings, implement fixes for excessive idle CPU/GPU. Possible interventions
   (dependent on diagnosis):
   - Fix frame pacing to properly sleep between cursor blink frames on CGL
   - Fix dirty-flag logic to skip `ctx.run()` when nothing has changed
   - Disable vsync-forced wakeups when the frame is known-clean
   - Investigate `CAMetalLayer` / CGL-specific behavior that may prevent proper idle

5. **68.5 — Cross-platform idle verification**
   After fixes, measure and document idle CPU/GPU on all three platforms:

   | Scenario              | Linux Target | macOS Target | Windows Target |
   | --------------------- | ------------ | ------------ | -------------- |
   | Steady cursor, idle   | ~0% CPU      | ~0% CPU      | ~0% CPU        |
   | Blinking cursor, idle | ~0.1% CPU    | ~0.1% CPU    | ~0.1% CPU      |
   | Active PTY output     | Proportional | Proportional | Proportional   |

   Any platform exceeding 1% CPU at steady idle is a bug that must be fixed before this
   task is considered complete.

6. **68.6 — Windows launch time verification**
   Measure time from process start to first stable idle frame on Windows. Document the
   baseline (pre-fix) and post-fix numbers. Target: comparable to Linux startup time
   (within 2x is acceptable given platform differences, >5x is a bug).

### 68 Primary Files

Files are TBD pending diagnosis. Likely candidates:

- `freminal-windowing/src/event_loop.rs` (frame pacing, sleep behavior)
- `freminal-windowing/src/gl_context.rs` (GL initialization, vsync)
- `freminal/src/gui/renderer/gpu.rs` (shader compilation, glyph atlas)
- `freminal/src/gui/mod.rs` (dirty-flag logic, repaint requests)
- `freminal/src/gui/terminal/widget.rs` (render skip logic)
- `freminal/src/main.rs` (startup sequence)

### 68 Design Decisions

1. **Diagnose first, fix second.** Performance issues on unfamiliar platforms are dangerous
   to fix by guessing. Each platform has its own profiling data requirement before any code
   changes are made. The subtask ordering enforces this: profiling subtasks (68.1, 68.2)
   must complete before fix subtasks (68.3, 68.4).

2. **Independent of other v0.7.0 tasks.** This task has no dependencies on Tasks 59 or 61
   and can run in parallel. It should be prioritized if either platform is intended for
   daily use.

---

## Task 69 — UI Polish & Settings Completeness

### 69 Overview

A collection of UI improvements and consistency fixes:

1. **Broken glyphs in close/dismiss buttons.** The `✕` (U+2715) character used for
   close/unbind buttons renders as a square on some systems where egui's default font lacks
   that glyph. Replace with a reliable ASCII `"X"` or an egui-native icon.
2. **Settings dialog missing config options.** Several `config.toml` fields have no
   corresponding settings UI: `background_image`, `background_image_mode`,
   `background_image_opacity`, `shader.path`, `shader.hot_reload`. Every user-facing
   config option must be editable in the Settings dialog.
3. **Search bar positioning.** The scrollback search overlay is anchored to the top-right of
   the terminal area, but should be anchored to the top-right of the _focused pane_ that
   initiated the search. In single-pane mode this looks the same, but in multi-pane splits
   the search should appear inside the active pane, not spanning the full window width.
   Additionally, the search overlay uses a hardcoded egui `Area` ID (`"search_overlay"`)
   which conflicts across multiple panes.
4. **Settings dialog as independent window.** Currently the Settings dialog is an egui
   `Window` inside the parent window. It should be its own independent OS window (using
   `freminal-windowing` window creation), allowing it to be moved, resized, and positioned
   independently.

### 69 Subtasks

1. **69.1 — Fix close/dismiss button glyphs**
   Replace `"✕"` (U+2715) and `"\u{2715}"` in the GUI with a reliable alternative.
   Options: plain `"X"`, or use egui's built-in `"🗙"` / `RichText` with a fallback.
   Audit all GUI files for any other Unicode glyphs that may render as squares on
   systems with limited font coverage. Known locations:
   - `freminal/src/gui/search.rs:403` — search bar close button
   - `freminal/src/gui/settings.rs:1067` — keybinding unbind button
   - Check for `◀` / `▶` navigation buttons in search.rs — verify these render correctly

2. **69.2 — Add background image settings to Settings dialog**
   Add controls to the UI tab of the Settings dialog for:
   - `background_image`: file path text field with a "Browse..." button (file picker)
   - `background_image_mode`: ComboBox with Fill / Fit / Cover / Tile options
   - `background_image_opacity`: Slider (0.0–1.0, step 0.05)
     These should appear in the existing UI tab near the existing `background_opacity` slider.
     Changes must write back to the config and take effect immediately (hot-reload).

3. **69.3 — Add shader settings to Settings dialog**
   Add a "Shaders" section to the UI tab (or a new dedicated tab) in the Settings dialog:
   - `shader.path`: file path text field with a "Browse..." button
   - `shader.hot_reload`: Checkbox
     Include a note/label explaining that custom shaders are GLSL fragment shaders.
     Changes must write back to config and take effect immediately.

4. **69.4 — Audit config.toml coverage**
   Systematically compare every field in `config_example.toml` against the Settings dialog.
   Verify that after 69.2 and 69.3, every user-facing config option has a corresponding
   Settings UI control. Document any remaining gaps and either add controls or justify
   why a field is config-only (e.g., `managed_by` is intentionally not user-editable).

5. **69.5 — Per-pane search overlay positioning**
   Move the search overlay to be anchored to the top-right of the _pane_ that had focus
   when the search was initiated, not the overall terminal area.
   - Make the `Area` ID unique per pane: `egui::Id::new(("search_overlay", pane_id))`
   - Use the individual pane's `terminal_rect` (which is already passed correctly) for
     anchor positioning
   - Ensure the search bar does not overflow outside the pane rect — if the pane is too
     narrow (< 260px), allow the search bar to extend left past the pane boundary but
     clip to the window
   - Test with multiple panes: search should only appear on the focused pane, not on
     all panes or at the window level
   - Verify that opening search in one pane and switching focus to another pane behaves
     correctly (search stays open on the original pane, or closes — decide and implement
     consistently)

6. **69.6 — Settings dialog as independent window**
   Convert the Settings dialog from an egui `Window` (modal inside the parent) to a
   standalone OS window via `freminal-windowing::WindowHandle::create_window()`.
   - The settings window should have a reasonable default size (~600x500) and be resizable
   - Only one settings window should exist at a time (opening settings when already open
     should focus the existing window)
   - The settings window reads from and writes to the shared `Config` — changes are
     applied immediately to all terminal windows
   - Closing the settings window does not close any terminal windows
   - The settings window does not appear in the tab bar or have its own terminal
   - The `App::update()` method must detect whether a `WindowId` is the settings window
     and render the settings UI instead of the terminal UI
   - Keyboard shortcut for opening settings (existing `KeyAction::OpenSettings`) should
     work from any terminal window

7. **69.7 — Menu shortcut labels**
   Menu items that have a corresponding keybinding should display the shortcut to the right
   of the menu label, using platform-canonical modifier symbols:
   - macOS: `⌘` for Command, `⌥` for Option/Alt, `⇧` for Shift, `⌃` for Control
   - Linux/Windows: `Ctrl+`, `Alt+`, `Shift+`
     Look up each menu action's `KeyAction` in the active `BindingMap` to find the bound
     combo. Format the combo using platform-aware display (detect OS at runtime or compile
     time). If an action has no binding, show no shortcut text. If the user has rebound or
     unbound a shortcut, the menu must reflect the _current_ binding, not the default.
     egui's `Button::shortcut_text()` or manual `ui.with_layout(right_to_left)` label can
     be used for right-aligned shortcut display.

8. **69.8 — Tests and verification**
   - Verify all config options have settings UI controls
   - Verify close/dismiss buttons render correctly (no squares)
   - Verify search overlay is per-pane positioned in multi-pane layouts
   - Verify settings window opens as independent OS window
   - Verify menu items show correct keybinding labels
   - Verify keybinding labels update when user rebinds shortcuts
   - Run full verification suite

### 69 Primary Files

- `freminal/src/gui/search.rs` (search overlay positioning, close button glyph)
- `freminal/src/gui/settings.rs` (new config controls, window conversion)
- `freminal/src/gui/menu.rs` (shortcut label display)
- `freminal/src/gui/terminal/widget.rs` (search overlay call site)
- `freminal/src/gui/mod.rs` (settings window management, App::update dispatch)
- `freminal/src/gui/window.rs` (per-window state — settings window flag)

### 69 Design Decisions

1. **ASCII close buttons over Unicode glyphs.** Unicode symbols like ✕ (U+2715) are not
   guaranteed to be in egui's default font on all platforms. A plain `"X"` or egui's
   built-in close icon is universally reliable. Visual polish is not worth broken rendering.

2. **Per-pane search ID.** The current shared `"search_overlay"` Area ID means egui treats
   all panes' search bars as the same widget. With unique IDs per pane, each search bar has
   independent state and positioning.

3. **Settings as independent window.** An in-window modal (a) blocks interaction with the
   terminal behind it, (b) cannot be moved to a second monitor, (c) is coupled to the
   parent window's lifecycle. A standalone window solves all three. It follows the
   peer-window model established in Task 64.

4. **Platform-native modifier symbols in menus.** Users expect `⌘` on macOS and `Ctrl` on
   Linux/Windows. Using platform-canonical display follows OS conventions and makes
   shortcuts immediately recognizable. Menu labels must reflect the _current_ binding
   (not hardcoded defaults) so that user customizations are always visible.

---

## Dependency Graph

```text
Task 58 (Built-in Muxing) ──► Task 59 (FREC v2 Recording)

Task 36 (Tabs) ──► Task 61 (Saved Layouts)
Task 58 (Built-in Muxing) ──► Task 61 (Saved Layouts)

Task 68 (Platform Performance) ── independent, no dependencies
Task 69 (UI Polish) ── independent, no dependencies
```

**Recommended order:**

All four tasks can start in parallel (independent of each other).

```text
v0.7.0 Execution:
  ┌── Task 59 (FREC v2 Recording)
  │
  ├── Task 61 (Saved Layouts)       [parallel with 59]
  │
  ├── Task 68 (Platform Perf)       [parallel with 59 and 61]
  │
  └── Task 69 (UI Polish)           [parallel with all]
```

---

## Cross-Cutting Concerns

### Recording + Layouts Interaction

Task 59 (recording) captures topology events including window geometry. Task 61 (layouts)
defines topology with window geometry. A recording's initial state can conceptually be
exported as a layout file (diagnostic utility). This interaction is a future enhancement,
not a v0.7.0 requirement, but the format designs should not preclude it.

### Config Schema Extensions

Task 61 extends the config with `[startup]` fields:

```toml
[startup]
layout = "dev"                    # name or path of layout to load on startup
restore_last_session = false      # auto-save/restore
```

This must be propagated to `config.rs`, `config_example.toml`, the home-manager module, and
the Settings Modal.

### `sequence_decoder.py` as Canonical Analysis Tool

`sequence_decoder.py` is the canonical tool for analyzing FREC recordings. Agents working
with recording files MUST use this tool rather than writing ad-hoc parsers. See the
instruction in `agents.md` under "FREC Recording Analysis".

---

## Completion Criteria

Per `agents.md`, each task is complete when:

1. All subtasks marked complete
2. `cargo test --all` passes
3. `cargo clippy --all-targets --all-features -- -D warnings` passes
4. `cargo-machete` passes
5. Benchmarks show no unexplained regressions for render/buffer changes
6. Config schema additions propagated to config.rs, config_example.toml, home-manager, settings
