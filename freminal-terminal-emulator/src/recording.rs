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

use conv2::ValueFrom;
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
// Writer
// ---------------------------------------------------------------------------

/// Errors that can occur during recording.
#[derive(Debug, thiserror::Error)]
pub enum RecordingError {
    /// I/O error during file write.
    #[error("recording I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// Serialization error.
    #[error("recording serialization error: {0}")]
    Serialize(#[from] rmp_serde::encode::Error),
}

/// Per-pane recording context passed into input handlers.
///
/// Bundles the recording handle with the window and pane IDs needed to
/// emit events. Avoids threading three extra parameters through deep call
/// chains.
pub struct RecordingContext<'a> {
    /// The recording handle.
    pub handle: &'a RecordingHandle,
    /// Recording-local window identifier.
    pub window_id: u32,
    /// Recording-local pane identifier.
    pub pane_id: u32,
}

/// Handle for sending recording events from any thread.
///
/// Cheaply cloneable. Dropping all clones signals the writer thread to finalize
/// and flush.
#[derive(Clone)]
pub struct RecordingHandle {
    tx: crossbeam_channel::Sender<RecordingEvent>,
    start: std::time::Instant,
}

impl RecordingHandle {
    /// Send an event to the recording writer thread.
    ///
    /// Events are silently dropped on a full channel to avoid
    /// blocking the PTY/GUI threads (uses `try_send`).
    pub fn send(&self, event: RecordingEvent) {
        // Best-effort: never block production threads.
        let _: Result<(), _> = self.tx.try_send(event);
    }

    /// Compute the current timestamp in microseconds since recording start.
    #[must_use]
    pub fn timestamp_us(&self) -> u64 {
        let elapsed = self.start.elapsed();
        // Truncation is acceptable: u64 microseconds covers ~584,942 years.
        #[allow(clippy::cast_possible_truncation)]
        let us = elapsed.as_micros() as u64;
        us
    }

    /// Build and send an event with the current timestamp.
    pub fn emit(&self, payload: EventPayload) {
        self.send(RecordingEvent {
            timestamp_us: self.timestamp_us(),
            payload,
        });
    }
}

/// Dedicated writer thread state. Not public — created via [`start_recording`].
struct WriterThread {
    writer: std::io::BufWriter<std::fs::File>,
    rx: crossbeam_channel::Receiver<RecordingEvent>,
    seek_entries: Vec<SeekIndexEntry>,
    last_seek_timestamp_us: u64,
    events_written: u64,
    last_timestamp_us: u64,
}

/// Seek index interval: one entry per second of recording time.
const SEEK_INDEX_INTERVAL_US: u64 = 1_000_000;

impl WriterThread {
    /// Write the fixed file header and serialized metadata.
    fn write_header(&mut self, metadata: &RecordingMetadata) -> Result<(), RecordingError> {
        use std::io::Write;

        // Magic
        self.writer.write_all(FREC_MAGIC)?;
        // Version
        self.writer.write_all(&[FREC_VERSION])?;
        // Flags (reserved, all zero)
        self.writer.write_all(&0u32.to_le_bytes())?;
        // Serialize metadata
        let metadata_bytes = rmp_serde::to_vec(metadata)?;
        let metadata_len = u32::try_from(metadata_bytes.len()).unwrap_or(u32::MAX);
        self.writer.write_all(&metadata_len.to_le_bytes())?;
        self.writer.write_all(&metadata_bytes)?;
        self.writer.flush()?;
        Ok(())
    }

    /// Write a single event record and update seek index.
    fn write_event(&mut self, event: &RecordingEvent) -> Result<(), RecordingError> {
        use std::io::{Seek, Write};

        // Record file offset for seek index (before writing this event).
        let file_offset = self.writer.stream_position()?;

        // Check if we need a seek index entry (~1 second intervals).
        if event.timestamp_us >= self.last_seek_timestamp_us + SEEK_INDEX_INTERVAL_US
            || self.seek_entries.is_empty()
        {
            self.seek_entries.push(SeekIndexEntry {
                timestamp_us: event.timestamp_us,
                file_offset,
            });
            self.last_seek_timestamp_us = event.timestamp_us;
        }

        // Serialize payload via MessagePack.
        let payload_bytes = rmp_serde::to_vec(&event.payload)?;
        let payload_len = u32::try_from(payload_bytes.len()).unwrap_or(u32::MAX);

        // Event header: timestamp_us (8) + event_type (1) + payload_length (4)
        self.writer.write_all(&event.timestamp_us.to_le_bytes())?;
        self.writer
            .write_all(&[event.payload.event_type().to_u8()])?;
        self.writer.write_all(&payload_len.to_le_bytes())?;
        // Payload
        self.writer.write_all(&payload_bytes)?;

        self.events_written += 1;
        self.last_timestamp_us = event.timestamp_us;

        Ok(())
    }

    /// Write the seek index and footer, finalizing the file.
    fn finalize(&mut self) -> Result<(), RecordingError> {
        use std::io::{Seek, Write};

        // Record offset of seek index.
        let seek_index_offset = self.writer.stream_position()?;

        // Seek index: entry count + entries.
        let entry_count = u64::try_from(self.seek_entries.len()).unwrap_or(u64::MAX);
        self.writer.write_all(&entry_count.to_le_bytes())?;
        for entry in &self.seek_entries {
            self.writer.write_all(&entry.timestamp_us.to_le_bytes())?;
            self.writer.write_all(&entry.file_offset.to_le_bytes())?;
        }

        // Footer: seek_index_offset + total_duration + total_events + magic.
        self.writer.write_all(&seek_index_offset.to_le_bytes())?;
        self.writer
            .write_all(&self.last_timestamp_us.to_le_bytes())?;
        self.writer.write_all(&self.events_written.to_le_bytes())?;
        self.writer.write_all(FREC_MAGIC)?;

        self.writer.flush()?;
        Ok(())
    }

    /// Run the writer loop: drain events from channel, write, finalize on close.
    fn run(mut self, metadata: &RecordingMetadata) {
        if let Err(e) = self.write_header(metadata) {
            error!("FREC recording: failed to write header: {e}");
            return;
        }

        // Take rx out so we don't hold an immutable borrow on self while writing.
        let rx = self.rx.clone();

        // Drain events until all senders are dropped.
        for event in &rx {
            if let Err(e) = self.write_event(&event) {
                error!("FREC recording: failed to write event: {e}");
            }
        }

        if let Err(e) = self.finalize() {
            error!("FREC recording: failed to finalize: {e}");
        }
    }
}

/// Start a recording session.
///
/// Creates the output file, spawns a dedicated writer thread, and returns a
/// [`RecordingHandle`] that can be cloned and sent to any thread. Dropping all
/// handles causes the writer to finalize the file and exit.
///
/// The channel is bounded to `channel_capacity` events. If the channel is full,
/// events are silently dropped (the writer is I/O-bound, not the PTY thread).
///
/// # Errors
///
/// Returns an error if the output file cannot be created.
pub fn start_recording(
    path: &std::path::Path,
    metadata: RecordingMetadata,
    channel_capacity: usize,
) -> Result<RecordingHandle, RecordingError> {
    let file = std::fs::File::create(path)?;
    let writer = std::io::BufWriter::new(file);
    let (tx, rx) = crossbeam_channel::bounded(channel_capacity);

    let thread = WriterThread {
        writer,
        rx,
        seek_entries: Vec::new(),
        last_seek_timestamp_us: 0,
        events_written: 0,
        last_timestamp_us: 0,
    };

    std::thread::Builder::new()
        .name("frec-writer".to_string())
        .spawn(move || thread.run(&metadata))
        .map_err(|e| RecordingError::Io(std::io::Error::other(e)))?;

    Ok(RecordingHandle {
        tx,
        start: std::time::Instant::now(),
    })
}

// ---------------------------------------------------------------------------
// Reader / Parser
// ---------------------------------------------------------------------------

/// Parsed contents of a FREC v2 file.
#[derive(Debug, Clone)]
pub struct ParsedRecording {
    /// Metadata from the file header.
    pub metadata: RecordingMetadata,
    /// All events in order.
    pub events: Vec<RecordingEvent>,
    /// Seek index entries.
    pub seek_index: Vec<SeekIndexEntry>,
    /// Total recording duration in microseconds (from footer).
    pub total_duration_us: u64,
    /// Total event count (from footer).
    pub total_events: u64,
}

/// Errors that can occur while parsing a FREC file.
#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    /// I/O error.
    #[error("parse I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// Deserialization error.
    #[error("parse deserialization error: {0}")]
    Deserialize(#[from] rmp_serde::decode::Error),
    /// Invalid file format.
    #[error("invalid FREC file: {0}")]
    InvalidFormat(String),
}

/// Parse a FREC v2 file into memory (full load mode).
///
/// Reads the entire file and returns all metadata, events, and the seek index.
/// Suitable for small-to-medium files and integration tests.
///
/// # Errors
///
/// Returns [`ParseError`] if the file is not a valid FREC v2 file.
pub fn parse_recording(path: &std::path::Path) -> Result<ParsedRecording, ParseError> {
    let data = std::fs::read(path)?;
    parse_recording_from_bytes(&data)
}

/// Parse FREC v2 from an in-memory byte slice.
///
/// # Errors
///
/// Returns [`ParseError`] if the data is not a valid FREC v2 file.
pub fn parse_recording_from_bytes(data: &[u8]) -> Result<ParsedRecording, ParseError> {
    if data.len() < HEADER_FIXED_SIZE + FOOTER_SIZE {
        return Err(ParseError::InvalidFormat(
            "file too small for header + footer".to_string(),
        ));
    }

    let mut pos = 0;

    // --- Header ---
    if &data[pos..pos + 4] != FREC_MAGIC {
        return Err(ParseError::InvalidFormat("bad magic".to_string()));
    }
    pos += 4;

    if data[pos] != FREC_VERSION {
        return Err(ParseError::InvalidFormat(format!(
            "unsupported version: {:#04x}",
            data[pos]
        )));
    }
    pos += 1;

    // Flags (reserved).
    pos += 4;

    let meta_len = read_u32_le(data, pos) as usize;
    pos += 4;

    if pos + meta_len > data.len() {
        return Err(ParseError::InvalidFormat(
            "metadata length exceeds file size".to_string(),
        ));
    }
    let metadata: RecordingMetadata = rmp_serde::from_slice(&data[pos..pos + meta_len])?;
    pos += meta_len;

    // --- Footer ---
    let footer_start = data.len() - FOOTER_SIZE;
    if &data[footer_start + 24..footer_start + 28] != FREC_MAGIC {
        return Err(ParseError::InvalidFormat("bad footer magic".to_string()));
    }
    let seek_index_offset = usize::value_from(read_u64_le(data, footer_start))
        .map_err(|_| ParseError::InvalidFormat("seek_index_offset overflows usize".to_string()))?;
    let total_duration_us = read_u64_le(data, footer_start + 8);
    let total_events = read_u64_le(data, footer_start + 16);

    // --- Events ---
    let mut events = Vec::new();
    let mut event_pos = pos;
    while event_pos < seek_index_offset {
        if event_pos + EVENT_HEADER_SIZE > data.len() {
            return Err(ParseError::InvalidFormat(
                "truncated event header".to_string(),
            ));
        }
        let timestamp_us = read_u64_le(data, event_pos);
        // Skip event_type byte (we deserialize payload which includes the variant).
        let payload_len = read_u32_le(data, event_pos + 9) as usize;
        event_pos += EVENT_HEADER_SIZE;

        if event_pos + payload_len > data.len() {
            return Err(ParseError::InvalidFormat(
                "truncated event payload".to_string(),
            ));
        }
        let payload: EventPayload =
            rmp_serde::from_slice(&data[event_pos..event_pos + payload_len])?;
        events.push(RecordingEvent {
            timestamp_us,
            payload,
        });
        event_pos += payload_len;
    }

    // --- Seek index ---
    let mut idx_pos = seek_index_offset;
    if idx_pos + 8 > footer_start {
        return Err(ParseError::InvalidFormat(
            "seek index overlaps footer".to_string(),
        ));
    }
    let entry_count = usize::value_from(read_u64_le(data, idx_pos)).map_err(|_| {
        ParseError::InvalidFormat("seek index entry_count overflows usize".to_string())
    })?;
    idx_pos += 8;

    let mut seek_index = Vec::with_capacity(entry_count);
    for _ in 0..entry_count {
        if idx_pos + SEEK_INDEX_ENTRY_SIZE > footer_start {
            return Err(ParseError::InvalidFormat(
                "truncated seek index".to_string(),
            ));
        }
        seek_index.push(SeekIndexEntry {
            timestamp_us: read_u64_le(data, idx_pos),
            file_offset: read_u64_le(data, idx_pos + 8),
        });
        idx_pos += SEEK_INDEX_ENTRY_SIZE;
    }

    Ok(ParsedRecording {
        metadata,
        events,
        seek_index,
        total_duration_us,
        total_events,
    })
}

/// Read a little-endian `u32` from a byte slice at the given offset.
fn read_u32_le(data: &[u8], offset: usize) -> u32 {
    let bytes: [u8; 4] = [
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ];
    u32::from_le_bytes(bytes)
}

/// Read a little-endian `u64` from a byte slice at the given offset.
fn read_u64_le(data: &[u8], offset: usize) -> u64 {
    let bytes: [u8; 8] = [
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
        data[offset + 4],
        data[offset + 5],
        data[offset + 6],
        data[offset + 7],
    ];
    u64::from_le_bytes(bytes)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::cast_possible_truncation,
    clippy::used_underscore_binding
)]
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

    /// Create a minimal test metadata value.
    fn test_metadata() -> RecordingMetadata {
        RecordingMetadata {
            freminal_version: "0.7.0".to_string(),
            created_at: 1_700_000_000,
            term: "xterm-256color".to_string(),
            initial_topology: TopologySnapshot { windows: vec![] },
            scrollback_limit: 10_000,
        }
    }

    /// Read back a FREC file using the production parser.
    fn read_frec_file(path: &std::path::Path) -> ParsedRecording {
        parse_recording(path).unwrap()
    }

    #[test]
    fn writer_empty_recording() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.frec");
        let metadata = test_metadata();

        let handle = start_recording(&path, metadata.clone(), 64).unwrap();
        drop(handle); // Signal writer to finalize.

        // Give writer thread time to finish.
        std::thread::sleep(std::time::Duration::from_millis(100));

        let parsed = read_frec_file(&path);

        assert_eq!(parsed.metadata, metadata);
        assert!(parsed.events.is_empty());
        assert!(parsed.seek_index.is_empty());
        assert_eq!(parsed.total_duration_us, 0);
        assert_eq!(parsed.total_events, 0);
    }

    #[test]
    fn writer_round_trip_events() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.frec");
        let metadata = test_metadata();

        let handle = start_recording(&path, metadata.clone(), 64).unwrap();

        handle.send(RecordingEvent {
            timestamp_us: 1000,
            payload: EventPayload::PtyOutput {
                pane_id: 0,
                data: b"hello".to_vec(),
            },
        });
        handle.send(RecordingEvent {
            timestamp_us: 2000,
            payload: EventPayload::PaneResize {
                pane_id: 0,
                cols: 120,
                rows: 40,
            },
        });
        handle.send(RecordingEvent {
            timestamp_us: 3000,
            payload: EventPayload::WindowClose { window_id: 0 },
        });

        drop(handle);
        std::thread::sleep(std::time::Duration::from_millis(100));

        let parsed = read_frec_file(&path);

        assert_eq!(parsed.metadata, metadata);
        assert_eq!(parsed.events.len(), 3);
        assert_eq!(parsed.total_events, 3);
        assert_eq!(parsed.total_duration_us, 3000);

        // Verify event contents.
        assert_eq!(parsed.events[0].timestamp_us, 1000);
        assert_eq!(
            parsed.events[0].payload,
            EventPayload::PtyOutput {
                pane_id: 0,
                data: b"hello".to_vec()
            }
        );
        assert_eq!(parsed.events[1].timestamp_us, 2000);
        assert_eq!(parsed.events[2].timestamp_us, 3000);

        // First event should create a seek entry.
        assert!(!parsed.seek_index.is_empty());
        assert_eq!(parsed.seek_index[0].timestamp_us, 1000);
    }

    #[test]
    fn writer_seek_index_intervals() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.frec");
        let metadata = test_metadata();

        let handle = start_recording(&path, metadata, 256).unwrap();

        // Write events spanning 3.5 seconds — should produce ~4 seek entries.
        for i in 0..35 {
            handle.send(RecordingEvent {
                timestamp_us: i * 100_000, // 0, 100ms, 200ms, ..., 3400ms
                payload: EventPayload::PtyOutput {
                    pane_id: 0,
                    data: vec![b'A' + (i % 26) as u8],
                },
            });
        }

        drop(handle);
        std::thread::sleep(std::time::Duration::from_millis(100));

        let parsed = read_frec_file(&path);

        assert_eq!(parsed.events.len(), 35);
        // First entry at t=0, then at t>=1s, t>=2s, t>=3s → 4 entries.
        assert_eq!(parsed.seek_index.len(), 4);
        assert_eq!(parsed.seek_index[0].timestamp_us, 0);
        assert!(parsed.seek_index[1].timestamp_us >= 1_000_000);
        assert!(parsed.seek_index[2].timestamp_us >= 2_000_000);
        assert!(parsed.seek_index[3].timestamp_us >= 3_000_000);
    }

    #[test]
    fn writer_event_ordering_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.frec");
        let metadata = test_metadata();

        let handle = start_recording(&path, metadata, 256).unwrap();

        for i in 0..100u64 {
            handle.send(RecordingEvent {
                timestamp_us: i * 10,
                payload: EventPayload::PtyInput {
                    pane_id: 0,
                    data: vec![i as u8],
                },
            });
        }

        drop(handle);
        std::thread::sleep(std::time::Duration::from_millis(100));

        let parsed = read_frec_file(&path);

        assert_eq!(parsed.events.len(), 100);
        assert_eq!(parsed.total_events, 100);
        for (i, event) in parsed.events.iter().enumerate() {
            assert_eq!(event.timestamp_us, (i as u64) * 10);
        }
    }

    /// Integration test: exercises the `RecordingHandle::emit()` convenience
    /// method with a realistic mix of event types, then verifies the full
    /// round-trip through the writer thread and parser.
    #[test]
    fn integration_mixed_event_types() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mixed.frec");
        let metadata = test_metadata();

        let handle = start_recording(&path, metadata, 256).unwrap();

        // Simulate a realistic session: window create, PTY output, keyboard input,
        // mouse events, clipboard paste, selection, pane close, window close.
        handle.emit(EventPayload::WindowCreate {
            window_id: 0,
            width_px: 1920,
            height_px: 1080,
            x: 0,
            y: 0,
        });
        handle.emit(EventPayload::PtyOutput {
            pane_id: 0,
            data: b"$ ls\r\n".to_vec(),
        });
        handle.emit(EventPayload::KeyboardInput {
            window_id: 0,
            pane_id: 0,
            key_name: "l".to_string(),
            modifiers: 0,
            encoded: b"l".to_vec(),
        });
        handle.emit(EventPayload::MouseMove {
            window_id: 0,
            pane_id: 0,
            x: 10,
            y: 5,
            coalesced_count: 1,
        });
        handle.emit(EventPayload::MouseButton {
            window_id: 0,
            pane_id: 0,
            button: 0,
            pressed: true,
            x: 10,
            y: 5,
        });
        handle.emit(EventPayload::MouseScroll {
            window_id: 0,
            pane_id: 0,
            delta_x: 0.0,
            delta_y: -3.0,
        });
        handle.emit(EventPayload::ClipboardPaste {
            pane_id: 0,
            data: b"pasted text".to_vec(),
        });
        handle.emit(EventPayload::SelectionEvent {
            pane_id: 0,
            start_row: 0,
            start_col: 0,
            end_row: 0,
            end_col: 10,
            is_block: false,
        });
        handle.emit(EventPayload::PaneClose { pane_id: 0 });
        handle.emit(EventPayload::WindowClose { window_id: 0 });

        // Drop handle to trigger finalization.
        drop(handle);
        std::thread::sleep(std::time::Duration::from_millis(100));

        let parsed = read_frec_file(&path);

        assert_eq!(parsed.events.len(), 10);
        assert_eq!(parsed.total_events, 10);

        // Verify event types in order.
        let expected_types = [
            EventType::WindowCreate,
            EventType::PtyOutput,
            EventType::KeyboardInput,
            EventType::MouseMove,
            EventType::MouseButton,
            EventType::MouseScroll,
            EventType::ClipboardPaste,
            EventType::SelectionEvent,
            EventType::PaneClose,
            EventType::WindowClose,
        ];
        for (i, expected) in expected_types.iter().enumerate() {
            assert_eq!(
                parsed.events[i].payload.event_type(),
                *expected,
                "Event {i} type mismatch"
            );
        }

        // Verify timestamps are monotonically non-decreasing.
        for window in parsed.events.windows(2) {
            assert!(
                window[1].timestamp_us >= window[0].timestamp_us,
                "Timestamps not monotonic"
            );
        }

        // Verify footer duration covers the event span.
        assert!(parsed.total_duration_us >= parsed.events.last().unwrap().timestamp_us);
    }
}
