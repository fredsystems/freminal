//! FREC v2 recording format types.
//!
//! This module defines the binary recording format for multi-window, multi-pane terminal
//! sessions. The format captures PTY I/O, topology changes, user input, and window lifecycle
//! events with microsecond timestamps.
//!
//! ## File Structure
//!
//! ```text
//! [File Header]  magic + version + flags + metadata_len + metadata (MessagePack)
//! [Event Stream] sequential variable-length records
//! [Seek Index]   entry_count + (timestamp_us, file_offset) pairs
//! [Footer]       seek_index_offset + total_duration_us + total_events + magic
//! ```
//!
//! ## Serialization
//!
//! Metadata and topology snapshots use `MessagePack` via `rmp_serde` for compact, self-describing
//! serialization. Event records use a fixed binary header (timestamp + type + length) with
//! `MessagePack` payloads. `MessagePack` was chosen over bincode for forward-compatible decoding
//! (field names preserved) and easy consumption from Python (the `msgpack` package) in
//! the `sequence_decoder` script.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// File magic bytes identifying FREC files.
pub const FREC_MAGIC: &[u8; 4] = b"FREC";

/// Format version for FREC v2.
pub const FREC_VERSION: u8 = 0x02;

/// Size of the file header before the metadata blob:
/// magic (4) + version (1) + flags (4) + `metadata_length` (4) = 13 bytes.
pub const HEADER_FIXED_SIZE: usize = 13;

/// Size of each seek index entry: `timestamp_us` (8) + `file_offset` (8) = 16 bytes.
pub const SEEK_INDEX_ENTRY_SIZE: usize = 16;

/// Size of the file footer:
/// `seek_index_offset` (8) + `total_duration_us` (8) + `total_events` (8) + magic (4) = 28 bytes.
pub const FOOTER_SIZE: usize = 28;

/// Size of each event record header:
/// `timestamp_us` (8) + `event_type` (1) + `payload_length` (4) = 13 bytes.
pub const EVENT_HEADER_SIZE: usize = 13;

// ---------------------------------------------------------------------------
// Metadata
// ---------------------------------------------------------------------------

/// Top-level metadata block written in the file header.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RecordingMetadata {
    /// Freminal version string (e.g. "0.7.0").
    pub freminal_version: String,
    /// Unix epoch seconds when recording started.
    pub created_at: u64,
    /// `$TERM` value (e.g. "xterm-256color").
    pub term: String,
    /// Full window/tab/pane tree at recording start.
    pub initial_topology: TopologySnapshot,
    /// Scrollback line limit.
    pub scrollback_limit: u32,
}

// ---------------------------------------------------------------------------
// Topology snapshots
// ---------------------------------------------------------------------------

/// Complete snapshot of all windows at a point in time.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TopologySnapshot {
    /// All open windows.
    pub windows: Vec<WindowSnapshot>,
}

/// Snapshot of a single window.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WindowSnapshot {
    /// Recording-local window ID (monotonically assigned, starting at 0).
    pub window_id: u32,
    /// Window position in pixels. `None` on Wayland where position is compositor-managed.
    pub position: Option<(i32, i32)>,
    /// Window size in pixels (width, height).
    pub size: (u32, u32),
    /// Tabs in this window.
    pub tabs: Vec<TabSnapshot>,
    /// ID of the active (focused) tab.
    pub active_tab: u32,
}

/// Snapshot of a single tab.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TabSnapshot {
    /// Recording-local tab ID.
    pub tab_id: u32,
    /// Parent window ID.
    pub window_id: u32,
    /// Pane layout tree.
    pub pane_tree: PaneTreeSnapshot,
    /// ID of the active (focused) pane.
    pub active_pane: u32,
    /// Pane ID if a pane is zoomed, `None` otherwise.
    pub zoomed_pane: Option<u32>,
}

/// Root of a pane layout tree.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PaneTreeSnapshot {
    /// Root node of the tree.
    pub node: PaneNodeSnapshot,
}

/// Node in the pane layout tree — either a leaf pane or a split.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum PaneNodeSnapshot {
    /// A terminal pane.
    Leaf {
        /// Recording-local pane ID.
        pane_id: u32,
        /// Terminal columns.
        cols: u32,
        /// Terminal rows.
        rows: u32,
        /// Current working directory (best-effort via `/proc`).
        cwd: Option<String>,
        /// Shell command.
        shell: Option<String>,
        /// Pane title.
        title: String,
    },
    /// A split containing two children.
    Split {
        /// Split orientation.
        direction: RecordingSplitDirection,
        /// Ratio of first child (0.0..1.0).
        ratio: f32,
        /// First child (left or top).
        first: Box<Self>,
        /// Second child (right or bottom).
        second: Box<Self>,
    },
}

/// Split direction for recording format (independent of GUI `SplitDirection` to keep the
/// recording format stable across GUI refactors).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum RecordingSplitDirection {
    /// Left | Right — the divider is a vertical line.
    Horizontal,
    /// Top / Bottom — the divider is a horizontal line.
    Vertical,
}

// ---------------------------------------------------------------------------
// Event types
// ---------------------------------------------------------------------------

/// Event type discriminant. Encoded as a single `u8` in the event header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum EventType {
    /// PTY output (terminal → screen).
    PtyOutput = 0x01,
    /// PTY input (keyboard → terminal).
    PtyInput = 0x02,
    /// Pane resize.
    PaneResize = 0x03,
    /// Window resize.
    WindowResize = 0x04,
    /// New tab created.
    TabCreate = 0x05,
    /// Tab closed.
    TabClose = 0x06,
    /// Pane split.
    PaneSplit = 0x07,
    /// Pane closed.
    PaneClose = 0x08,
    /// Focus changed to a different pane.
    FocusChange = 0x09,
    /// Pane zoom toggled.
    ZoomToggle = 0x0A,
    /// Switched to a different tab.
    TabSwitch = 0x0B,
    /// Color theme changed.
    ThemeChange = 0x0C,
    /// Keyboard input event.
    KeyboardInput = 0x0D,
    /// Mouse move (debounced).
    MouseMove = 0x0E,
    /// Mouse button press/release.
    MouseButton = 0x0F,
    /// Mouse scroll.
    MouseScroll = 0x10,
    /// New window created.
    WindowCreate = 0x11,
    /// Window closed.
    WindowClose = 0x12,
    /// Window focus gained/lost.
    WindowFocus = 0x13,
    /// Clipboard paste into pane.
    ClipboardPaste = 0x14,
    /// Bell (audible/visual).
    BellEvent = 0x15,
    /// Text selection changed.
    SelectionEvent = 0x16,
    /// Window moved.
    WindowMove = 0x17,
}

impl EventType {
    /// Convert from raw `u8` discriminant.
    #[must_use]
    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            0x01 => Some(Self::PtyOutput),
            0x02 => Some(Self::PtyInput),
            0x03 => Some(Self::PaneResize),
            0x04 => Some(Self::WindowResize),
            0x05 => Some(Self::TabCreate),
            0x06 => Some(Self::TabClose),
            0x07 => Some(Self::PaneSplit),
            0x08 => Some(Self::PaneClose),
            0x09 => Some(Self::FocusChange),
            0x0A => Some(Self::ZoomToggle),
            0x0B => Some(Self::TabSwitch),
            0x0C => Some(Self::ThemeChange),
            0x0D => Some(Self::KeyboardInput),
            0x0E => Some(Self::MouseMove),
            0x0F => Some(Self::MouseButton),
            0x10 => Some(Self::MouseScroll),
            0x11 => Some(Self::WindowCreate),
            0x12 => Some(Self::WindowClose),
            0x13 => Some(Self::WindowFocus),
            0x14 => Some(Self::ClipboardPaste),
            0x15 => Some(Self::BellEvent),
            0x16 => Some(Self::SelectionEvent),
            0x17 => Some(Self::WindowMove),
            _ => None,
        }
    }

    /// Convert to raw `u8` discriminant.
    #[must_use]
    pub const fn to_u8(self) -> u8 {
        self as u8
    }
}

// ---------------------------------------------------------------------------
// Event payloads
// ---------------------------------------------------------------------------

/// A single recording event with timestamp and typed payload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RecordingEvent {
    /// Microseconds elapsed since recording start.
    pub timestamp_us: u64,
    /// Event payload.
    pub payload: EventPayload,
}

/// Typed event payload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum EventPayload {
    /// PTY output data (terminal → screen).
    PtyOutput {
        /// Target pane.
        pane_id: u32,
        /// Raw PTY output bytes.
        data: Vec<u8>,
    },
    /// PTY input data (keyboard → terminal).
    PtyInput {
        /// Target pane.
        pane_id: u32,
        /// Raw bytes sent to PTY.
        data: Vec<u8>,
    },
    /// Pane terminal size changed.
    PaneResize {
        /// Target pane.
        pane_id: u32,
        /// New column count.
        cols: u32,
        /// New row count.
        rows: u32,
    },
    /// Window pixel size changed.
    WindowResize {
        /// Target window.
        window_id: u32,
        /// New width in pixels.
        width_px: u32,
        /// New height in pixels.
        height_px: u32,
    },
    /// New tab created.
    TabCreate {
        /// Parent window.
        window_id: u32,
        /// New tab ID.
        tab_id: u32,
        /// Initial pane ID.
        pane_id: u32,
        /// Initial columns.
        cols: u32,
        /// Initial rows.
        rows: u32,
    },
    /// Tab closed.
    TabClose {
        /// Parent window.
        window_id: u32,
        /// Closed tab ID.
        tab_id: u32,
    },
    /// Pane split.
    PaneSplit {
        /// Parent window.
        window_id: u32,
        /// Pane that was split.
        parent_pane: u32,
        /// Newly created pane.
        new_pane: u32,
        /// Split direction.
        direction: RecordingSplitDirection,
        /// Split ratio.
        ratio: f32,
        /// New pane columns.
        cols: u32,
        /// New pane rows.
        rows: u32,
    },
    /// Pane closed.
    PaneClose {
        /// Closed pane.
        pane_id: u32,
    },
    /// Focus moved to a different pane.
    FocusChange {
        /// Window containing the focused pane.
        window_id: u32,
        /// Tab containing the focused pane.
        tab_id: u32,
        /// Newly focused pane.
        pane_id: u32,
    },
    /// Pane zoom state toggled.
    ZoomToggle {
        /// Window.
        window_id: u32,
        /// Tab.
        tab_id: u32,
        /// Pane.
        pane_id: u32,
        /// `true` if pane is now zoomed.
        zoomed: bool,
    },
    /// Active tab switched.
    TabSwitch {
        /// Window.
        window_id: u32,
        /// Newly active tab.
        tab_id: u32,
    },
    /// Color theme changed.
    ThemeChange {
        /// New theme name.
        theme_name: String,
    },
    /// Keyboard input.
    KeyboardInput {
        /// Window.
        window_id: u32,
        /// Target pane.
        pane_id: u32,
        /// Human-readable key name (e.g. "Ctrl+C").
        key_name: String,
        /// Modifier flags (bit 0=shift, 1=ctrl, 2=alt, 3=super).
        modifiers: u8,
        /// Raw bytes that were sent to the PTY.
        encoded: Vec<u8>,
    },
    /// Mouse move (debounced).
    MouseMove {
        /// Window.
        window_id: u32,
        /// Pane under cursor.
        pane_id: u32,
        /// Cell column.
        x: u32,
        /// Cell row.
        y: u32,
        /// Number of raw events coalesced into this one.
        coalesced_count: u32,
    },
    /// Mouse button press or release.
    MouseButton {
        /// Window.
        window_id: u32,
        /// Pane under cursor.
        pane_id: u32,
        /// Button number (0=left, 1=middle, 2=right).
        button: u8,
        /// `true` if pressed, `false` if released.
        pressed: bool,
        /// Cell column.
        x: u32,
        /// Cell row.
        y: u32,
    },
    /// Mouse scroll.
    MouseScroll {
        /// Window.
        window_id: u32,
        /// Pane under cursor.
        pane_id: u32,
        /// Horizontal scroll delta.
        delta_x: f32,
        /// Vertical scroll delta.
        delta_y: f32,
    },
    /// New window created.
    WindowCreate {
        /// New window ID.
        window_id: u32,
        /// Width in pixels.
        width_px: u32,
        /// Height in pixels.
        height_px: u32,
        /// X position.
        x: i32,
        /// Y position.
        y: i32,
    },
    /// Window closed.
    WindowClose {
        /// Closed window ID.
        window_id: u32,
    },
    /// Window focus gained or lost.
    WindowFocus {
        /// Window.
        window_id: u32,
        /// `true` if focused, `false` if unfocused.
        focused: bool,
    },
    /// Clipboard paste.
    ClipboardPaste {
        /// Target pane.
        pane_id: u32,
        /// Pasted content.
        data: Vec<u8>,
    },
    /// Bell event.
    BellEvent {
        /// Pane that triggered the bell.
        pane_id: u32,
        /// Bell type (0=audible, 1=visual).
        bell_type: u8,
    },
    /// Text selection.
    SelectionEvent {
        /// Pane.
        pane_id: u32,
        /// Selection start row.
        start_row: u32,
        /// Selection start column.
        start_col: u32,
        /// Selection end row.
        end_row: u32,
        /// Selection end column.
        end_col: u32,
        /// `true` if block/rectangular selection.
        is_block: bool,
    },
    /// Window moved.
    WindowMove {
        /// Window.
        window_id: u32,
        /// New X position.
        x: i32,
        /// New Y position.
        y: i32,
    },
}

impl EventPayload {
    /// Returns the [`EventType`] discriminant for this payload.
    #[must_use]
    pub const fn event_type(&self) -> EventType {
        match self {
            Self::PtyOutput { .. } => EventType::PtyOutput,
            Self::PtyInput { .. } => EventType::PtyInput,
            Self::PaneResize { .. } => EventType::PaneResize,
            Self::WindowResize { .. } => EventType::WindowResize,
            Self::TabCreate { .. } => EventType::TabCreate,
            Self::TabClose { .. } => EventType::TabClose,
            Self::PaneSplit { .. } => EventType::PaneSplit,
            Self::PaneClose { .. } => EventType::PaneClose,
            Self::FocusChange { .. } => EventType::FocusChange,
            Self::ZoomToggle { .. } => EventType::ZoomToggle,
            Self::TabSwitch { .. } => EventType::TabSwitch,
            Self::ThemeChange { .. } => EventType::ThemeChange,
            Self::KeyboardInput { .. } => EventType::KeyboardInput,
            Self::MouseMove { .. } => EventType::MouseMove,
            Self::MouseButton { .. } => EventType::MouseButton,
            Self::MouseScroll { .. } => EventType::MouseScroll,
            Self::WindowCreate { .. } => EventType::WindowCreate,
            Self::WindowClose { .. } => EventType::WindowClose,
            Self::WindowFocus { .. } => EventType::WindowFocus,
            Self::ClipboardPaste { .. } => EventType::ClipboardPaste,
            Self::BellEvent { .. } => EventType::BellEvent,
            Self::SelectionEvent { .. } => EventType::SelectionEvent,
            Self::WindowMove { .. } => EventType::WindowMove,
        }
    }
}

// ---------------------------------------------------------------------------
// Seek index
// ---------------------------------------------------------------------------

/// A single entry in the seek index.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct SeekIndexEntry {
    /// Microseconds elapsed since recording start.
    pub timestamp_us: u64,
    /// Byte offset from start of file to the event record.
    pub file_offset: u64,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// Helper: round-trip a value through `MessagePack` serialization.
    fn round_trip<T: Serialize + for<'de> Deserialize<'de> + PartialEq + std::fmt::Debug>(
        value: &T,
    ) {
        let bytes = rmp_serde::to_vec(value).unwrap();
        let decoded: T = rmp_serde::from_slice(&bytes).unwrap();
        assert_eq!(value, &decoded);
    }

    #[test]
    fn round_trip_metadata() {
        let metadata = RecordingMetadata {
            freminal_version: "0.7.0".to_string(),
            created_at: 1_713_000_000,
            term: "xterm-256color".to_string(),
            initial_topology: TopologySnapshot {
                windows: vec![WindowSnapshot {
                    window_id: 0,
                    position: Some((100, 200)),
                    size: (1920, 1080),
                    tabs: vec![TabSnapshot {
                        tab_id: 0,
                        window_id: 0,
                        pane_tree: PaneTreeSnapshot {
                            node: PaneNodeSnapshot::Leaf {
                                pane_id: 0,
                                cols: 80,
                                rows: 24,
                                cwd: Some("/home/user".to_string()),
                                shell: Some("/bin/zsh".to_string()),
                                title: "zsh".to_string(),
                            },
                        },
                        active_pane: 0,
                        zoomed_pane: None,
                    }],
                    active_tab: 0,
                }],
            },
            scrollback_limit: 10_000,
        };
        round_trip(&metadata);
    }

    #[test]
    fn round_trip_split_topology() {
        let tree = PaneTreeSnapshot {
            node: PaneNodeSnapshot::Split {
                direction: RecordingSplitDirection::Horizontal,
                ratio: 0.5,
                first: Box::new(PaneNodeSnapshot::Leaf {
                    pane_id: 0,
                    cols: 40,
                    rows: 24,
                    cwd: None,
                    shell: None,
                    title: "left".to_string(),
                }),
                second: Box::new(PaneNodeSnapshot::Split {
                    direction: RecordingSplitDirection::Vertical,
                    ratio: 0.6,
                    first: Box::new(PaneNodeSnapshot::Leaf {
                        pane_id: 1,
                        cols: 40,
                        rows: 14,
                        cwd: Some("/tmp".to_string()),
                        shell: Some("/bin/bash".to_string()),
                        title: "top-right".to_string(),
                    }),
                    second: Box::new(PaneNodeSnapshot::Leaf {
                        pane_id: 2,
                        cols: 40,
                        rows: 10,
                        cwd: None,
                        shell: None,
                        title: "bottom-right".to_string(),
                    }),
                }),
            },
        };
        round_trip(&tree);
    }

    #[test]
    fn round_trip_seek_index_entry() {
        let entry = SeekIndexEntry {
            timestamp_us: 5_000_000,
            file_offset: 1024,
        };
        round_trip(&entry);
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn round_trip_all_event_payloads() {
        let events = vec![
            RecordingEvent {
                timestamp_us: 100,
                payload: EventPayload::PtyOutput {
                    pane_id: 0,
                    data: b"hello world\r\n".to_vec(),
                },
            },
            RecordingEvent {
                timestamp_us: 200,
                payload: EventPayload::PtyInput {
                    pane_id: 0,
                    data: b"ls\r".to_vec(),
                },
            },
            RecordingEvent {
                timestamp_us: 300,
                payload: EventPayload::PaneResize {
                    pane_id: 0,
                    cols: 120,
                    rows: 40,
                },
            },
            RecordingEvent {
                timestamp_us: 400,
                payload: EventPayload::WindowResize {
                    window_id: 0,
                    width_px: 1920,
                    height_px: 1080,
                },
            },
            RecordingEvent {
                timestamp_us: 500,
                payload: EventPayload::TabCreate {
                    window_id: 0,
                    tab_id: 1,
                    pane_id: 1,
                    cols: 80,
                    rows: 24,
                },
            },
            RecordingEvent {
                timestamp_us: 600,
                payload: EventPayload::TabClose {
                    window_id: 0,
                    tab_id: 1,
                },
            },
            RecordingEvent {
                timestamp_us: 700,
                payload: EventPayload::PaneSplit {
                    window_id: 0,
                    parent_pane: 0,
                    new_pane: 2,
                    direction: RecordingSplitDirection::Horizontal,
                    ratio: 0.5,
                    cols: 40,
                    rows: 24,
                },
            },
            RecordingEvent {
                timestamp_us: 800,
                payload: EventPayload::PaneClose { pane_id: 2 },
            },
            RecordingEvent {
                timestamp_us: 900,
                payload: EventPayload::FocusChange {
                    window_id: 0,
                    tab_id: 0,
                    pane_id: 0,
                },
            },
            RecordingEvent {
                timestamp_us: 1000,
                payload: EventPayload::ZoomToggle {
                    window_id: 0,
                    tab_id: 0,
                    pane_id: 0,
                    zoomed: true,
                },
            },
            RecordingEvent {
                timestamp_us: 1100,
                payload: EventPayload::TabSwitch {
                    window_id: 0,
                    tab_id: 1,
                },
            },
            RecordingEvent {
                timestamp_us: 1200,
                payload: EventPayload::ThemeChange {
                    theme_name: "Catppuccin Mocha".to_string(),
                },
            },
            RecordingEvent {
                timestamp_us: 1300,
                payload: EventPayload::KeyboardInput {
                    window_id: 0,
                    pane_id: 0,
                    key_name: "Ctrl+C".to_string(),
                    modifiers: 0x02,
                    encoded: vec![0x03],
                },
            },
            RecordingEvent {
                timestamp_us: 1400,
                payload: EventPayload::MouseMove {
                    window_id: 0,
                    pane_id: 0,
                    x: 10,
                    y: 5,
                    coalesced_count: 3,
                },
            },
            RecordingEvent {
                timestamp_us: 1500,
                payload: EventPayload::MouseButton {
                    window_id: 0,
                    pane_id: 0,
                    button: 0,
                    pressed: true,
                    x: 10,
                    y: 5,
                },
            },
            RecordingEvent {
                timestamp_us: 1600,
                payload: EventPayload::MouseScroll {
                    window_id: 0,
                    pane_id: 0,
                    delta_x: 0.0,
                    delta_y: -3.0,
                },
            },
            RecordingEvent {
                timestamp_us: 1700,
                payload: EventPayload::WindowCreate {
                    window_id: 1,
                    width_px: 800,
                    height_px: 600,
                    x: 50,
                    y: 50,
                },
            },
            RecordingEvent {
                timestamp_us: 1800,
                payload: EventPayload::WindowClose { window_id: 1 },
            },
            RecordingEvent {
                timestamp_us: 1900,
                payload: EventPayload::WindowFocus {
                    window_id: 0,
                    focused: true,
                },
            },
            RecordingEvent {
                timestamp_us: 2000,
                payload: EventPayload::ClipboardPaste {
                    pane_id: 0,
                    data: b"pasted text".to_vec(),
                },
            },
            RecordingEvent {
                timestamp_us: 2100,
                payload: EventPayload::BellEvent {
                    pane_id: 0,
                    bell_type: 0,
                },
            },
            RecordingEvent {
                timestamp_us: 2200,
                payload: EventPayload::SelectionEvent {
                    pane_id: 0,
                    start_row: 0,
                    start_col: 0,
                    end_row: 0,
                    end_col: 10,
                    is_block: false,
                },
            },
            RecordingEvent {
                timestamp_us: 2300,
                payload: EventPayload::WindowMove {
                    window_id: 0,
                    x: 200,
                    y: 300,
                },
            },
        ];

        for event in &events {
            round_trip(event);
        }
    }

    #[test]
    fn event_type_from_u8_round_trip() {
        for id in 0x01..=0x17u8 {
            let et = EventType::from_u8(id).unwrap_or_else(|| panic!("unknown event type {id:#x}"));
            assert_eq!(et.to_u8(), id);
        }
    }

    #[test]
    fn event_type_from_u8_unknown() {
        assert_eq!(EventType::from_u8(0x00), None);
        assert_eq!(EventType::from_u8(0x18), None);
        assert_eq!(EventType::from_u8(0xFF), None);
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn event_payload_event_type_matches() {
        let cases: Vec<(EventPayload, EventType)> = vec![
            (
                EventPayload::PtyOutput {
                    pane_id: 0,
                    data: vec![],
                },
                EventType::PtyOutput,
            ),
            (
                EventPayload::PtyInput {
                    pane_id: 0,
                    data: vec![],
                },
                EventType::PtyInput,
            ),
            (
                EventPayload::PaneResize {
                    pane_id: 0,
                    cols: 80,
                    rows: 24,
                },
                EventType::PaneResize,
            ),
            (
                EventPayload::WindowResize {
                    window_id: 0,
                    width_px: 800,
                    height_px: 600,
                },
                EventType::WindowResize,
            ),
            (
                EventPayload::TabCreate {
                    window_id: 0,
                    tab_id: 0,
                    pane_id: 0,
                    cols: 80,
                    rows: 24,
                },
                EventType::TabCreate,
            ),
            (
                EventPayload::TabClose {
                    window_id: 0,
                    tab_id: 0,
                },
                EventType::TabClose,
            ),
            (
                EventPayload::PaneSplit {
                    window_id: 0,
                    parent_pane: 0,
                    new_pane: 1,
                    direction: RecordingSplitDirection::Vertical,
                    ratio: 0.5,
                    cols: 40,
                    rows: 24,
                },
                EventType::PaneSplit,
            ),
            (EventPayload::PaneClose { pane_id: 0 }, EventType::PaneClose),
            (
                EventPayload::FocusChange {
                    window_id: 0,
                    tab_id: 0,
                    pane_id: 0,
                },
                EventType::FocusChange,
            ),
            (
                EventPayload::ZoomToggle {
                    window_id: 0,
                    tab_id: 0,
                    pane_id: 0,
                    zoomed: false,
                },
                EventType::ZoomToggle,
            ),
            (
                EventPayload::TabSwitch {
                    window_id: 0,
                    tab_id: 0,
                },
                EventType::TabSwitch,
            ),
            (
                EventPayload::ThemeChange {
                    theme_name: String::new(),
                },
                EventType::ThemeChange,
            ),
            (
                EventPayload::KeyboardInput {
                    window_id: 0,
                    pane_id: 0,
                    key_name: String::new(),
                    modifiers: 0,
                    encoded: vec![],
                },
                EventType::KeyboardInput,
            ),
            (
                EventPayload::MouseMove {
                    window_id: 0,
                    pane_id: 0,
                    x: 0,
                    y: 0,
                    coalesced_count: 0,
                },
                EventType::MouseMove,
            ),
            (
                EventPayload::MouseButton {
                    window_id: 0,
                    pane_id: 0,
                    button: 0,
                    pressed: false,
                    x: 0,
                    y: 0,
                },
                EventType::MouseButton,
            ),
            (
                EventPayload::MouseScroll {
                    window_id: 0,
                    pane_id: 0,
                    delta_x: 0.0,
                    delta_y: 0.0,
                },
                EventType::MouseScroll,
            ),
            (
                EventPayload::WindowCreate {
                    window_id: 0,
                    width_px: 800,
                    height_px: 600,
                    x: 0,
                    y: 0,
                },
                EventType::WindowCreate,
            ),
            (
                EventPayload::WindowClose { window_id: 0 },
                EventType::WindowClose,
            ),
            (
                EventPayload::WindowFocus {
                    window_id: 0,
                    focused: true,
                },
                EventType::WindowFocus,
            ),
            (
                EventPayload::ClipboardPaste {
                    pane_id: 0,
                    data: vec![],
                },
                EventType::ClipboardPaste,
            ),
            (
                EventPayload::BellEvent {
                    pane_id: 0,
                    bell_type: 0,
                },
                EventType::BellEvent,
            ),
            (
                EventPayload::SelectionEvent {
                    pane_id: 0,
                    start_row: 0,
                    start_col: 0,
                    end_row: 0,
                    end_col: 0,
                    is_block: false,
                },
                EventType::SelectionEvent,
            ),
            (
                EventPayload::WindowMove {
                    window_id: 0,
                    x: 0,
                    y: 0,
                },
                EventType::WindowMove,
            ),
        ];

        for (payload, expected) in &cases {
            assert_eq!(payload.event_type(), *expected);
        }
    }

    #[test]
    fn metadata_none_position() {
        let snap = WindowSnapshot {
            window_id: 0,
            position: None,
            size: (800, 600),
            tabs: vec![],
            active_tab: 0,
        };
        round_trip(&snap);
    }

    #[test]
    fn empty_topology() {
        let topo = TopologySnapshot { windows: vec![] };
        round_trip(&topo);
    }
}
