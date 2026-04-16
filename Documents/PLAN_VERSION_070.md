# PLAN_VERSION_070.md — v0.7.0 "Replay & Layouts"

## Goal

Transform Freminal's recording/playback system from a single-stream byte dump into a full
session reconstruction engine, and introduce a layout system that lets users define, save,
and restore complete multi-tab, multi-pane workspace configurations.

---

## Task Summary

| #   | Feature                           | Scope | Status  | Dependencies     |
| --- | --------------------------------- | ----- | ------- | ---------------- |
| 59  | FREC v2: Multi-Pane Recording     | Large | Pending | Task 32, Task 58 |
| 60  | Playback v2: Multi-Pane Replay    | Large | Pending | Task 59          |
| 61  | Saved Layouts (Session Templates) | Large | Pending | Task 36, Task 58 |

---

## Task 59 — FREC v2: Multi-Pane Recording Format

### 59 Overview

The current FREC v1 format is a flat sequence of timestamped PTY read chunks — no metadata,
no terminal dimensions, no input recording, and no awareness of tabs or panes. It was designed
for a single-emulator world. With tabs (Task 36) and split panes (Task 58) now in place, the
format is fundamentally inadequate: it cannot record multi-pane sessions at all, and even for
single-pane sessions it loses critical information (window size, user input, resize events).

FREC v2 is a complete redesign. The goals:

1. **Per-stream isolation.** Each pane's PTY output is recorded independently — no byte
   interleaving. A recording of a 4-pane session contains 4 distinct output streams.
2. **Bidirectional capture.** Every byte Freminal sends TO the PTY (keyboard input, paste,
   bracketed paste sequences, report responses) is recorded per-pane. This is not replayed
   during playback — it exists purely for diagnostics ("what did the user type to produce
   this state?").
3. **Topology events.** Tab create/close, pane split/close, focus changes, and zoom
   toggle are recorded as timestamped events. Combined with the per-pane streams, this
   allows exact reconstruction of Freminal's state at any point in time.
4. **Size tracking.** Initial window dimensions and every resize event (both window-level
   and per-pane) are recorded, so playback can reconstruct the correct terminal geometry.
5. **Rich metadata.** Recording version, Freminal version, initial tab/pane topology,
   per-pane dimensions, TERM value, scrollback limit, shell, CWD at recording start.
6. **Seekability.** A frame index table at the end of the file enables random access and
   backward seeking without scanning the entire file.

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
│   Record 0: { timestamp_us, event_type, pane_id, ... }│
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

#### Metadata Block

Serialized as a structured binary format (MessagePack or bincode — decided during
implementation based on dependency weight). Contains:

```rust
struct RecordingMetadata {
    freminal_version: String,       // e.g. "0.6.0"
    created_at: u64,                // Unix epoch seconds
    term: String,                   // e.g. "xterm-256color"
    initial_window_size: (u32, u32), // pixels
    initial_topology: TopologySnapshot, // full tab/pane tree at recording start
    scrollback_limit: u32,
}

struct TopologySnapshot {
    tabs: Vec<TabSnapshot>,
    active_tab: TabId,
}

struct TabSnapshot {
    id: TabId,
    pane_tree: PaneTreeSnapshot,
    active_pane: PaneId,
    zoomed_pane: Option<PaneId>,
}

struct PaneTreeSnapshot {
    // Mirrors PaneTree enum but with serializable IDs and metadata
    node: PaneNodeSnapshot,
}

enum PaneNodeSnapshot {
    Leaf {
        pane_id: PaneId,
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

| Type ID | Name         | Payload                                                                          |
| ------- | ------------ | -------------------------------------------------------------------------------- |
| 0x01    | PtyOutput    | pane_id: u32, data: [u8]                                                         |
| 0x02    | PtyInput     | pane_id: u32, data: [u8]                                                         |
| 0x03    | PaneResize   | pane_id: u32, cols: u32, rows: u32                                               |
| 0x04    | WindowResize | width_px: u32, height_px: u32                                                    |
| 0x05    | TabCreate    | tab_id: u32, pane_id: u32, cols: u32, rows: u32                                  |
| 0x06    | TabClose     | tab_id: u32                                                                      |
| 0x07    | PaneSplit    | parent_pane: u32, new_pane: u32, direction: u8, ratio: f32, cols: u32, rows: u32 |
| 0x08    | PaneClose    | pane_id: u32                                                                     |
| 0x09    | FocusChange  | tab_id: u32, pane_id: u32                                                        |
| 0x0A    | ZoomToggle   | tab_id: u32, pane_id: u32, zoomed: u8                                            |
| 0x0B    | TabSwitch    | tab_id: u32                                                                      |
| 0x0C    | ThemeChange  | theme_name: String (length-prefixed)                                             |

**PtyOutput (0x01)** is the primary data event — equivalent to v1 frames but tagged with a
pane ID. **PtyInput (0x02)** records what Freminal sent to the PTY — keyboard input, paste
content, report responses. This is write-only (diagnostics) and is skipped during normal
playback.

#### Backward Compatibility

- Files starting with `b"FREC" 0x01` are v1 and handled by the existing parser.
- Files starting with `b"FREC" 0x02` are v2 and handled by the new parser.
- `parse_recording()` dispatches on the version byte.
- The `--with-playback-file` flag accepts both formats. v1 files play back in single-pane
  mode exactly as today.

### 59 Subtasks

1. **59.1 — Define FREC v2 format types**
   Create the Rust types for the v2 format: `RecordingMetadataV2`, `RecordingEvent`,
   `EventType` enum, `TopologySnapshot`, `PaneNodeSnapshot`, `SeekIndexEntry`. Place in
   `freminal-terminal-emulator/src/recording.rs` (or a new `recording/` module directory
   if the file grows too large). All types must be serializable. Choose and justify the
   serialization format (MessagePack via `rmp-serde`, bincode, or raw manual encoding).
   Add unit tests for round-trip serialization of all types.

2. **59.2 — v2 file writer: header and metadata**
   Implement `write_header_v2()` that writes the magic, version, flags, and serialized
   metadata block. The writer takes a `RecordingMetadataV2` (constructed from the current
   `TabManager` topology at recording start). Unit tests: write + read back, verify
   metadata fields survive the round trip.

3. **59.3 — v2 file writer: event stream**
   Implement `write_event()` that appends a single event record to the file. Called from
   the PTY reader thread (for PtyOutput/PtyInput) and the GUI thread (for topology and
   resize events). Use a channel to funnel events from multiple threads to a single writer
   thread (avoids interleaving and file locking). Unit tests: write a sequence of events,
   read back, verify ordering and content.

4. **59.4 — v2 file writer: seek index and footer**
   On recording stop (or application exit), the writer thread computes the seek index
   (one entry per ~1 second of recording time, or per N events), writes it, and writes
   the footer with the index offset, total duration, and event count. Implement graceful
   finalization on both clean exit and SIGTERM/crash (write what we have). Unit tests:
   verify index entries point to correct file offsets, verify footer fields.

5. **59.5 — v2 file parser**
   Implement `parse_recording_v2()` that reads the header, metadata, and loads the event
   stream. Two modes: full load (all events into memory, for small files) and indexed
   streaming (use the seek index for random access, for large files). The indexed mode
   reads events on demand by seeking to the nearest index entry. Dispatch from the existing
   `parse_recording()` based on version byte. Unit tests: parse files written by 59.2–59.4,
   verify all fields. Test backward compat: v1 files still parse correctly.

6. **59.6 — Hook PTY output recording**
   In the PTY reader thread (`pty.rs`), replace the v1 `write_frame()` call with
   `write_event(PtyOutput { pane_id, data })`. Each pane's PTY reader knows its pane ID
   (threaded through at spawn time). This is the minimal change to start producing v2 files.

7. **59.7 — Hook PTY input recording**
   Every byte sent TO the PTY (keyboard input via `PtyWrite::Write`, paste via
   `PtyWrite::Write`, resize via `PtyWrite::Resize`, and report responses) is captured as
   a `PtyInput` event. The input channel already passes through a known point — tap it
   there. `PtyWrite::Resize` additionally generates a `PaneResize` event.

8. **59.8 — Hook topology events**
   In the GUI thread, emit topology events when:
   - A tab is created (`TabCreate`) or closed (`TabClose`)
   - A pane is split (`PaneSplit`) or closed (`PaneClose`)
   - Focus changes (`FocusChange`)
   - Zoom toggles (`ZoomToggle`)
   - The active tab switches (`TabSwitch`)
   - The window resizes (`WindowResize`)
     These are sent through the event channel to the writer thread.

9. **59.9 — Recording CLI and config updates**
   Update `--recording-path` to produce v2 files by default. Add `--recording-format v1|v2`
   flag for backward compatibility. Update `config_example.toml` if any config-level
   recording options are added. Update the `playback` feature flag gating to cover the
   new types.

10. **59.10 — Tests and integration**
    End-to-end test: start a headless multi-pane session, feed input, split panes, resize,
    close panes, stop recording. Parse the resulting file and verify: correct event ordering,
    per-pane data isolation (no interleaving), topology events at correct timestamps, seek
    index validity. Test v1 backward compatibility. Test graceful finalization on abrupt stop.

### 59 Primary Files

- `freminal-terminal-emulator/src/recording.rs` (or `recording/` module — format types, writer, parser)
- `freminal-terminal-emulator/src/io/pty.rs` (PTY output and input hooks)
- `freminal/src/gui/mod.rs` (topology event emission)
- `freminal/src/gui/panes/mod.rs` (pane ID threading)
- `freminal/src/gui/tabs.rs` (tab lifecycle events)
- `freminal-common/src/args.rs` (CLI flag updates)

### 59 Design Decisions

1. **Single writer thread via channel.** Events from multiple PTY reader threads and the
   GUI thread are funneled through a bounded crossbeam channel to a dedicated writer thread.
   This avoids file-level locking, guarantees chronological ordering (events are timestamped
   at the source and sorted if needed), and keeps I/O off the hot paths.

2. **Seek index at file end.** Writing the index at finalization (rather than maintaining it
   inline) simplifies the writer — it just appends events sequentially. The footer's index
   offset allows the parser to jump straight to the index. Crash resilience: if the file is
   not finalized (crash before footer), the parser falls back to sequential scanning.

3. **PtyInput is write-only.** Input events are never replayed during playback — they exist
   for diagnostic reconstruction. This means the playback engine can skip them entirely,
   and the recording overhead of capturing input is negligible (input volume is tiny compared
   to output).

4. **Topology snapshots, not diffs.** The metadata block stores the full initial topology.
   Subsequent topology changes are recorded as discrete events (split, close, etc.). The
   playback engine reconstructs the topology by applying events sequentially from the initial
   snapshot. This is simpler and more debuggable than differential encoding.

---

## Task 60 — Playback v2: Multi-Pane Replay with Size Adaptation

### 60 Overview

The current playback engine (`freminal/src/playback.rs`) drives a single headless
`TerminalEmulator`, feeding it frames sequentially. It cannot:

- Replay multi-pane or multi-tab sessions
- Handle window size mismatch between recording and playback
- Seek backward or jump to arbitrary positions
- Isolate a single pane's stream

Task 60 rebuilds the playback engine to consume FREC v2 files, reconstruct the full tab/pane
topology, and introduce size adaptation strategies for tiling WM users who cannot freely
resize their window.

### 60 Design

#### Multi-Pane Reconstruction

The playback engine creates a headless `TerminalEmulator` per pane (just as live mode does).
On startup, it reads the `TopologySnapshot` from the FREC v2 metadata and constructs the
initial tab/pane tree:

```text
1. Read metadata.initial_topology
2. For each tab:
   a. For each leaf pane in the tree: create TerminalEmulator::new_headless(cols, rows)
   b. Construct the PaneTree (mirrors the recorded topology)
   c. Set active_pane and zoomed_pane from the snapshot
3. Publish initial snapshots for all panes
```

As topology events arrive during playback (TabCreate, PaneSplit, PaneClose, etc.), the engine
modifies the tab/pane tree accordingly — creating or destroying emulators as needed.

PtyOutput events are routed to the correct emulator by `pane_id`. PtyInput events are skipped
(logged to a diagnostic sidebar if one is ever added, but not fed to emulators).

#### Size Adaptation

The recording captures the exact terminal dimensions at every point. The playback window may
be a different size — especially on tiling WMs where the user cannot resize freely. Three
strategies, selectable via a toolbar toggle:

**1. Letterbox (default):**
Render each pane at its _recorded_ dimensions. If the playback window is larger, pad with
background color. If smaller, scroll/clip. This guarantees pixel-perfect fidelity — the
terminal content is identical to what was recorded. The pane tree layout uses the recorded
split ratios applied to the recorded window size, then the entire layout is centered in the
actual window.

**2. Reflow:**
Create each headless emulator at the _playback_ window dimensions (or the pane's proportional
share of the playback window). The same byte stream is fed, but the terminal re-wraps content
to fit the new width. This changes line breaks and may alter visual layout, but is more
readable when the size mismatch is large. Resize events in the recording are translated
proportionally.

**3. Scale:**
Render at the recorded dimensions (like letterbox) but scale the output to fill the playback
window. Preserves layout fidelity while using available space. May look fuzzy if scaling up
significantly. Implemented via an intermediate framebuffer rendered at recorded size, then
scaled to the display rect (leveraging the existing custom GL renderer).

**Default: Letterbox.** It is the only mode that guarantees the playback is visually identical
to the recording. The user can switch modes during playback via the toolbar.

#### Seek and Rewind

The FREC v2 seek index enables jumping to arbitrary positions:

**Forward seek:** Skip events until the target timestamp. Feed PtyOutput events to emulators
in fast-forward (no timing delays). Apply topology events normally.

**Backward seek / rewind:** Terminal emulators are not reversible — you cannot "undo" bytes
fed to them. To seek backward:

1. Find the nearest seek index entry at or before the target timestamp.
2. Reset all emulators to their initial state (or the state at the index entry — see
   checkpoint strategy below).
3. Replay all events from that point to the target timestamp in fast-forward.

**Checkpoint strategy (optimization):** During forward playback, periodically snapshot the
full emulator state (every ~5 seconds or at each seek index entry). Store these snapshots
in memory. When seeking backward, find the nearest checkpoint before the target and replay
from there instead of from the beginning. Memory budget: cap at ~100 checkpoints (each
snapshot is a few hundred KB), evicting the oldest when the cap is reached.

#### Per-Pane Solo Mode

The playback toolbar offers a pane selector. When a pane is "soloed," only that pane's
PtyOutput events are fed to its emulator and rendered. Other panes are paused (their events
are skipped). This is useful for focusing on one pane's output in a busy multi-pane recording.
Unsoloing resumes all panes from the current timestamp (events that were skipped are not
replayed — the other panes jump to their state at the current time via fast-forward).

#### Playback Toolbar Updates

The current toolbar has: mode selector (Instant/RealTime/FrameStep), play/pause, frame
counter. v2 adds:

- **Timeline scrubber:** drag to seek to any timestamp. Shows total duration and current
  position.
- **Size mode toggle:** Letterbox / Reflow / Scale.
- **Pane selector:** dropdown or clickable pane labels for solo mode.
- **Speed control:** 0.5x, 1x, 2x, 4x, 8x playback speed (multiplier on inter-frame delays).
- **Diagnostic toggle:** show/hide PtyInput events in a side panel (stretch goal).

### 60 Subtasks

1. **60.1 — Multi-emulator playback engine**
   Refactor `run_playback_thread` (or replace it) to manage multiple headless
   `TerminalEmulator` instances, one per pane. Read the `TopologySnapshot` from the v2
   metadata and construct the initial emulator set. Route `PtyOutput` events by pane ID.
   Handle topology events (create/destroy emulators as panes split/close). Publish per-pane
   snapshots via per-pane `ArcSwap`. Fall back to single-emulator mode for v1 files.

2. **60.2 — Playback tab/pane tree reconstruction**
   On playback start, construct a `TabManager` with the recorded topology. The GUI renders
   tabs and panes using the same layout code as live mode. Topology events during playback
   modify the tree (split, close, tab create/close). The GUI must handle dynamic topology
   changes during playback just as it does in live mode.

3. **60.3 — Letterbox size adaptation**
   Implement the letterbox strategy: each pane's emulator runs at its recorded dimensions
   regardless of the playback window size. The layout engine uses recorded dimensions for
   the pane tree, centers the result in the available window, and pads with background color.
   If the playback window is smaller than the recorded layout, clip or add scrollbars to the
   outer frame (not per-pane — the entire layout scrolls as a unit).

4. **60.4 — Reflow size adaptation**
   Implement the reflow strategy: each pane's emulator is created at the playback window's
   proportional dimensions. Resize events in the recording are translated proportionally.
   The pane tree uses the playback window dimensions with recorded split ratios.

5. **60.5 — Scale size adaptation**
   Implement the scale strategy: render the terminal layout at recorded dimensions into an
   offscreen framebuffer (FBO), then draw a single textured quad scaled to fill the playback
   window. This leverages the existing GL renderer. If Task 55 (Custom Shaders) is complete,
   the scale pass is just another post-processing step. If not, a minimal FBO + blit shader
   is needed (subset of 55.1).

6. **60.6 — Forward seek**
   Implement seek-forward: given a target timestamp, skip events and feed them to emulators
   in fast-forward (no timing delays). Update the timeline scrubber position. Handle topology
   events during fast-forward.

7. **60.7 — Backward seek with checkpoints**
   Implement the checkpoint system: during forward playback, snapshot emulator state at
   regular intervals. On backward seek, find the nearest checkpoint, restore emulator state,
   and fast-forward to the target. Cap checkpoint memory at a configurable budget. If no
   checkpoint exists (e.g., seeking to near the beginning), reset to initial state and
   replay from the start.

8. **60.8 — Per-pane solo mode**
   Add pane solo/unsolo to the playback toolbar. When soloed, only the selected pane's
   PtyOutput events are processed. Other panes freeze at their current state. Unsoloing
   fast-forwards the unsoloed panes to the current timestamp.

9. **60.9 — Speed control**
   Add speed multiplier to the playback toolbar (0.5x, 1x, 2x, 4x, 8x). The multiplier
   divides inter-frame delays in RealTime mode. In Instant mode, speed is already infinite.
   In FrameStep mode, speed is irrelevant.

10. **60.10 — Timeline scrubber UI**
    Add a horizontal scrubber bar to the playback toolbar showing total duration and current
    position. Clicking/dragging seeks to the target timestamp (using 60.6/60.7). Show
    timestamps in human-readable format (MM:SS or HH:MM:SS).

11. **60.11 — v1 backward compatibility**
    Ensure v1 FREC files continue to play back correctly. The v1 path remains single-pane,
    forward-only, with the existing playback modes. The new toolbar features (seek, speed,
    size mode) gracefully degrade: seek is not available (no index), size mode uses letterbox
    at the default 80x24, speed control works normally.

12. **60.12 — Tests and integration**
    End-to-end test: record a multi-pane session (using 59.10's test infrastructure), play
    it back, verify: pane topology matches, per-pane content matches frame-by-frame, seek
    forward/backward produces correct state, size adaptation modes render without crashes.
    Test v1 backward compatibility. Benchmark: measure playback throughput for large
    recordings (e.g., 1M events).

### 60 Primary Files

- `freminal/src/playback.rs` (rebuilt playback engine)
- `freminal/src/gui/mod.rs` (playback toolbar updates, multi-pane playback rendering)
- `freminal/src/gui/tabs.rs` (playback tab/pane tree construction)
- `freminal/src/gui/panes/mod.rs` (playback-mode pane management)
- `freminal/src/gui/terminal/widget.rs` (size adaptation rendering)
- `freminal/src/gui/renderer/gpu.rs` (FBO for scale mode, if Task 55 not done)
- `freminal/src/main.rs` (v2 playback startup path)

### 60 Design Decisions

1. **Letterbox as default.** A tiling WM user cannot resize freely. Reflow changes the
   content layout. Scale can be fuzzy. Letterbox is the only mode that guarantees the
   playback is visually identical to the recording, regardless of window size. The user
   can switch to reflow or scale if they prefer readability over fidelity.

2. **Checkpoints for backward seek.** Terminal emulators are state machines with no reverse
   operation. The alternative to checkpoints is replaying from the beginning on every
   backward seek, which is O(N) in recording length. Checkpoints make it O(checkpoint_interval).
   The memory cost is bounded by the checkpoint cap.

3. **Per-pane solo is skip-based, not filter-based.** Solo mode skips events for non-soloed
   panes rather than filtering them out of the event stream. This means the timeline stays
   consistent (the scrubber position reflects real recording time, not just the soloed pane's
   activity). Unsoloing requires fast-forward to sync the other panes.

4. **Speed control as delay multiplier.** The simplest correct approach. 2x speed = half the
   inter-frame delay. This preserves relative timing between events (a burst of output still
   looks like a burst, just faster). The alternative — frame batching — would change the
   visual cadence.

---

## Task 61 — Saved Layouts (Session Templates)

### 61 Overview

A layout is a complete description of a Freminal workspace: how many tabs, how each tab's
panes are arranged, and what runs in each pane. Layouts enable:

- **Startup configuration:** launch Freminal with a predefined multi-tab, multi-pane
  workspace tailored to a project
- **Save current state:** capture the running session's topology, working directories, and
  programs into a reusable layout file
- **Layout library:** a collection of named layouts in `~/.config/freminal/layouts/`,
  selectable from the menu or Settings Modal
- **Partial application:** load a layout into a new tab without replacing the entire session
- **Auto-restore:** optionally save the layout on exit and restore it on next launch

This task subsumes and expands Task 56 (Session Restore / Startup Commands), which was limited
to flat tab lists with no pane tree support.

### 61 Design

#### Layout File Format

Layouts are TOML files. TOML's nesting is limited, but for tree structures we use a
flattened representation with explicit parent references. The format is human-readable
and hand-editable — a key design goal.

```toml
# ~/.config/freminal/layouts/dev.toml

[layout]
name = "Development"
description = "Standard dev workspace: editor, server, logs"

# Variables — can be overridden from CLI: freminal --layout dev.toml ~/projects/myapp
# $1, $2, etc. are positional args. Named vars use ${VAR_NAME}.
[layout.variables]
project_dir = "~/projects/default"  # default value, overridden by $1 if provided

# --- Tab definitions ---

[[tabs]]
title = "Editor"
active = true  # this tab is focused on launch

  # Pane tree for this tab. The tree is defined as a flat list of nodes.
  # Each node has an "id" (local to this tab) and optionally a "parent" + "position".
  # The root node has no parent.

  [[tabs.panes]]
  id = "root"
  split = "vertical"   # "vertical" (left/right) or "horizontal" (top/bottom)
  ratio = 0.65

  [[tabs.panes]]
  id = "editor"
  parent = "root"
  position = "first"   # "first" (left/top) or "second" (right/bottom)
  directory = "${project_dir}"
  command = "nvim ."
  active = true         # this pane has focus within the tab

  [[tabs.panes]]
  id = "sidebar"
  parent = "root"
  position = "second"
  split = "horizontal"
  ratio = 0.5

  [[tabs.panes]]
  id = "terminal"
  parent = "sidebar"
  position = "first"
  directory = "${project_dir}"

  [[tabs.panes]]
  id = "git"
  parent = "sidebar"
  position = "second"
  directory = "${project_dir}"
  command = "lazygit"

[[tabs]]
title = "Server"

  [[tabs.panes]]
  id = "server"
  directory = "${project_dir}"
  command = "cargo watch -x run"

[[tabs]]
title = "Logs"

  [[tabs.panes]]
  id = "logs"
  directory = "/var/log"
  command = "tail -f syslog"
```

#### Pane Node Properties

Each `[[tabs.panes]]` entry is either a **split node** (has `split` and `ratio`) or a
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

A tab with a single pane omits `parent`, `position`, and `split` — just one `[[tabs.panes]]`
entry with the leaf properties.

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

- Tab count and order
- Per-tab pane tree structure (split directions, ratios)
- Per-pane working directory (read from `/proc/<pid>/cwd` on Linux)
- Per-pane foreground process (read from `/proc/<pid>/cmdline` on Linux — best-effort,
  may be empty or show the shell)
- Per-pane title (current OSC title)

The output is a valid layout TOML file. Directories are written as absolute paths; the user
can edit the file to use variables afterward.

**Platform note:** CWD and process detection via `/proc` is Linux-specific. On other
platforms (if Freminal is ever ported), these fields are omitted and the saved layout
captures topology only.

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
topology and CWDs but not running programs (since those have exited).

#### Layout Library

Layouts in `~/.config/freminal/layouts/` are discovered automatically. The menu bar shows
them under "Layouts" with their `layout.name` field. The Settings Modal's "Layouts" tab
(new tab) lists all discovered layouts with name, description, and a preview of the
topology.

### 61 Subtasks

1. **61.1 — Layout file format and parser**
   Define the `Layout`, `LayoutTab`, `LayoutPane`, `LayoutVariables` types. Implement
   TOML parsing with validation: detect orphan nodes (parent references a non-existent ID),
   multiple roots, cycles, missing position on non-root nodes. Implement variable
   substitution (`$1`, `${name}`, `$ENV{...}`, `~`). Unit tests: parse valid layouts,
   reject malformed ones, verify variable substitution.

2. **61.2 — Layout application engine**
   Given a parsed `Layout`, create the tab/pane tree using `spawn_pty_tab` for each leaf
   pane. Pass `directory` to PTY creation (set CWD before exec). Queue `command` for
   injection after shell ready (reuse the startup command injection mechanism). Set initial
   titles. Set active tab and active pane per the layout spec.

3. **61.3 — Startup command injection**
   Implement command injection: after a pane's shell is ready (detect via a short delay or
   by watching for the first prompt), send the `command` string as PTY input followed by
   a newline. Handle the "no command" case (just open a shell). This is the core of the
   original Task 56.3.

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
   struct and serializes it to TOML. Read CWDs via `/proc/<pid>/cwd`. Read foreground
   processes via `/proc/<pid>/cmdline` (best-effort). Write to a user-chosen path
   (file dialog or prompt). Unit tests: round-trip save/load produces equivalent topology.

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
   Implement `startup.restore_last_session`: on exit, save to `_last_session.toml`. On
   startup (if no `--layout` and `restore_last_session = true`), load from
   `_last_session.toml`. The saved layout includes topology and CWDs but not commands
   (since processes have exited — just open shells in the saved directories).

10. **61.10 — Config and settings integration**
    Add `[startup]` section to config: `layout`, `restore_last_session`. Add a "Layouts"
    tab to the Settings Modal showing discovered layouts with previews. Update
    `config_example.toml` and home-manager module.

11. **61.11 — Tests and integration**
    End-to-end: write a layout file, launch with `--layout`, verify correct tab/pane
    topology, correct CWDs, correct command injection. Test variable substitution with
    CLI overrides. Test save/load round-trip. Test auto-restore. Test malformed layouts
    produce clear error messages. Test partial application (append mode).

### 61 Primary Files

- `freminal-common/src/config.rs` (`StartupConfig`, `LayoutConfig`)
- `freminal-common/src/layout.rs` (new — layout types, parser, variable substitution)
- `freminal/src/gui/mod.rs` (layout application, menu integration)
- `freminal/src/gui/tabs.rs` (layout-driven tab creation)
- `freminal/src/gui/panes/mod.rs` (layout-driven pane tree construction)
- `freminal/src/main.rs` (startup layout loading)
- `freminal-common/src/args.rs` (`--layout`, `--var` flags)
- `freminal-common/src/keybindings.rs` (layout key actions)
- `config_example.toml`
- `nix/home-manager-module.nix`

### 61 Design Decisions

1. **TOML over JSON.** Consistent with the existing config format. TOML's nesting limitations
   are overcome by the flat node list with parent references — this is more readable than
   deeply nested inline tables would be, and allows the user to define nodes in any order.

2. **Flat node list with parent references.** A tree can be represented in TOML either as
   deeply nested inline tables (ugly, hard to edit) or as a flat list with explicit
   relationships. The flat approach is how many configuration systems handle tree structures
   (e.g., Terraform resources, Kubernetes manifests). Each node is self-contained and
   easy to add/remove.

3. **Variables for reusability.** Without variables, layouts are project-specific (hardcoded
   paths). Variables make a single layout file work across projects. The `$1` positional
   convention is familiar from shell scripting.

4. **Save captures topology + CWD, not running programs.** We cannot reliably restart an
   arbitrary program the user was running. Saving CWDs means the user gets shells in the
   right directories, which covers 90% of the restore value. If `command` was specified
   in the original layout, the saved layout preserves it — but the "detected" foreground
   process from `/proc` is stored as a comment, not as an auto-run command, to avoid
   surprising behavior.

5. **Subsumes Task 56 entirely.** Task 56's `[startup.tabs]` flat list is a strict subset
   of the layout system. Rather than implementing both, Task 61 provides a superset that
   handles both simple (`[[tabs]]` with one pane each) and complex (multi-tab, multi-pane,
   variables) cases. The `[startup]` config section is unified under the layout system.

---

## Dependency Graph

```text
Task 32 (Playback Feature Flag) ──► Task 59 (FREC v2 Format)
Task 58 (Built-in Muxing) ──────► Task 59 (FREC v2 Format)
                                     │
                                     ▼
                                  Task 60 (Playback v2)

Task 36 (Tabs) ──► Task 61 (Saved Layouts)
Task 58 (Built-in Muxing) ──► Task 61 (Saved Layouts)

Task 55 (Custom Shaders) ─ ─ ► Task 60.5 (Scale mode — soft dependency, can use minimal FBO)

Tasks 59 and 61 are independent of each other.
Task 60 depends on Task 59 (needs v2 format to replay).
Task 61 can start before, during, or after Task 59/60.
```

**Recommended order:**

1. Tasks 59 and 61 can start in parallel (independent).
2. Task 60 starts after Task 59 is complete.
3. Within Task 60, subtask 60.5 (Scale mode) benefits from Task 55 (Custom Shaders) if
   complete, but can be implemented with a minimal FBO otherwise.

```text
v0.7.0 Execution:
  ┌── Task 59 (FREC v2 Format) ──► Task 60 (Playback v2)
  │
  └── Task 61 (Saved Layouts)      [parallel with 59, independent]
```

---

## Cross-Cutting Concerns

### Recording + Layouts Interaction

Task 59 (recording) captures topology events. Task 61 (layouts) defines topology. When
recording starts, the initial topology snapshot in the FREC v2 metadata IS effectively a
layout. This means:

- A recording's initial state can be exported as a layout file (diagnostic utility).
- A layout file can be used to set up the initial state for a recording session.

This interaction is a future enhancement, not a v0.7.0 requirement, but the format designs
should not preclude it.

### Config Schema Extensions

Task 61 extends the config with `[startup]` fields:

```toml
[startup]
layout = "dev"                    # name or path of layout to load on startup
restore_last_session = false      # auto-save/restore
```

This must be propagated to `config.rs`, `config_example.toml`, the home-manager module, and
the Settings Modal.

### Feature Flag Scope

Tasks 59 and 60 are gated behind the `playback` feature flag (extending the existing gating
from Task 32). Task 61 (layouts) is NOT feature-gated — it is always available, as it is a
core workflow feature independent of recording/playback.

---

## Completion Criteria

Per `agents.md`, each task is complete when:

1. All subtasks marked complete
2. `cargo test --all` passes
3. `cargo clippy --all-targets --all-features -- -D warnings` passes
4. `cargo-machete` passes
5. Benchmarks show no unexplained regressions for render/buffer changes
6. Config schema additions propagated to config.rs, config_example.toml, home-manager, settings
