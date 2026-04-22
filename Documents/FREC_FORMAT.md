# FREC v2 Recording Format

Reference for the Freminal recording format (FREC v2), produced by
`freminal --recording-path <file>` and consumed by `sequence_decoder.py` and any
third-party replay/analysis tooling.

FREC v1 has been removed. There is no backward compatibility layer — v2 is the
only supported format.

Recording is always compiled into the binary (no feature gate); it is activated
at runtime via `--recording-path`.

---

## File Structure

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

All integers are little-endian.

The seek index enables external replay and analysis tools to jump to arbitrary
positions without scanning the entire file. Freminal itself does not replay
recordings, but the index is cheap to produce and valuable for
`sequence_decoder.py` and third-party tooling. The footer magic permits reverse
scanning to locate the index when file length is known but contents are
streaming.

---

## Metadata Block

Serialized as a structured binary format (MessagePack via `rmp-serde` or
bincode — chosen during implementation based on dependency weight). Present
immediately after the fixed header.

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

The initial topology captures the full window/tab/pane tree at recording start.
All subsequent topology changes are captured as events (see `WindowCreate`,
`TabCreate`, `PaneSplit`, etc.).

---

## Event Stream

Each event record has a common fixed-size header followed by a
variable-length payload:

```text
[0..8]   u64 LE   timestamp_us (elapsed from recording start)
[8]      u8       event_type
[9..13]  u32 LE   payload_length
[13..]   payload  (event_type-specific)
```

### Event Types

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

### Event Design Notes

- **PtyOutput (0x01)** is the primary data event — PTY read data tagged with a
  pane ID. All other events are metadata/context around the PTY byte stream.
- **PtyInput (0x02)** records bytes sent TO the PTY. Exists for diagnostics —
  correlating what the user typed with what the terminal produced.
- **KeyboardInput (0x0D)** records the human-readable key name (e.g. `Ctrl+C`,
  `Enter`, `Alt+F4`) plus the raw encoded bytes sent to the PTY. This enables
  correlating a `PtyInput` event with the keypress that generated it — the
  `key_name` field is the human-readable form, `encoded` is the raw bytes (which
  will also appear as a `PtyInput` event for the target pane).
- **MouseMove (0x0E)** is debounced to ~10 writes/second. The `coalesced_count`
  field records how many raw mouse-move events occurred since the last
  `MouseMove` write, preserving the information that the mouse was actively
  moving without flooding the recording. Recording every raw mouse move wastes
  space and provides no diagnostic value.
- **WindowCreate/WindowClose (0x11/0x12)** track window lifecycle. Tab and pane
  events include `window_id` to scope them to the correct window.
- **SelectionEvent (0x16)** records text selection for clipboard copy.
- **WindowMove (0x17)** records window position changes (no-op on Wayland where
  position is compositor-managed, but recorded on X11/macOS/Windows).

---

## ID Assignment

`winit::window::WindowId` is opaque and not serializable. The recording system
assigns its own monotonically increasing `u32` window IDs at recording time. A
mapping from `winit::window::WindowId` → `u32` is maintained by the recording
writer. The first window gets ID 0, subsequent windows get incrementing IDs.

`TabId` and `PaneId` are `u64` internally but are recorded as `u32` in FREC v2
(sufficient for any practical session — 4 billion panes).

---

## Analysis Tool

`sequence_decoder.py` at the repository root is the canonical tool for
analyzing FREC recordings. Agents working with recording files MUST use this
tool rather than writing ad-hoc parsers. See `agents.md` under "FREC Recording
Analysis" for usage.

If the decoder lacks a feature needed for the current task, extend the decoder
rather than working around it.
