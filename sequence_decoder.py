#!/usr/bin/python3

# A helper script to evaluate terminal session recordings.
#
# Supports FREC v1 (legacy) and FREC v2 (current) formats.
#
# FREC v1 format:
#   Header: b"FREC" + version byte (0x01)
#   Frame:  [u64 LE timestamp_us] [u32 LE data_length] [data bytes]
#
# FREC v2 format:
#   Header: b"FREC" + version (0x02) + flags (u32 LE) + metadata_len (u32 LE) + metadata (msgpack)
#   Events: [u64 LE timestamp_us] [u8 event_type] [u32 LE payload_len] [payload (msgpack)]
#   Seek index: [u32 LE entry_count] + entries (16 bytes each)
#   Footer: [u64 LE seek_index_offset] [u64 LE total_duration_us] [u64 LE total_events] [4B magic]
#
# Usage:
#   python3 sequence_decoder.py --recording-path=path/to/file
#   python3 sequence_decoder.py --recording-path=path/to/file --convert-escape
#   python3 sequence_decoder.py --recording-path=path/to/file --convert-escape --split-commands
#   python3 sequence_decoder.py --recording-path=path/to/file --show-timing
#   python3 sequence_decoder.py --recording-path=path/to/file --summary          # v2 only
#   python3 sequence_decoder.py --recording-path=path/to/file --pane 0           # v2 only
#   python3 sequence_decoder.py --recording-path=path/to/file --events-only      # v2 only

import struct
import sys
import json

# ---------------------------------------------------------------------------
# CLI argument parsing
# ---------------------------------------------------------------------------

filename = "sequence.bin"
convert_escape = False
split_commands = False
show_timing = False
summary_only = False
filter_pane = None
events_only = False

for arg in sys.argv[1:]:
    if arg.startswith("--recording-path"):
        filename = arg.split("=", 1)[1]
    elif arg == "--convert-escape":
        convert_escape = True
    elif arg == "--split-commands":
        split_commands = True
    elif arg == "--show-timing":
        show_timing = True
    elif arg == "--summary":
        summary_only = True
    elif arg.startswith("--pane"):
        filter_pane = int(arg.split("=", 1)[1] if "=" in arg else sys.argv[sys.argv.index(arg) + 1])
    elif arg == "--events-only":
        events_only = True

# ---------------------------------------------------------------------------
# Read file
# ---------------------------------------------------------------------------

try:
    with open(filename, "rb") as f:
        data = f.read()
except FileNotFoundError:
    print(f"File {filename} not found.")
    sys.exit(1)
except Exception as e:
    print(f"An error occurred: {e}")
    sys.exit(1)

if len(data) < 5:
    print(f"File too short for header (got {len(data)} bytes)")
    sys.exit(1)

magic = data[0:4]
if magic != b"FREC":
    print(f"Invalid magic bytes: {magic!r} (expected b'FREC')")
    sys.exit(1)

version = data[4]

# ---------------------------------------------------------------------------
# Event type names (v2)
# ---------------------------------------------------------------------------

EVENT_TYPES = {
    0x01: "PtyOutput",
    0x02: "PtyInput",
    0x03: "PaneResize",
    0x04: "TabCreate",
    0x05: "TabClose",
    0x06: "PaneSplit",
    0x07: "TabSwitch",
    0x08: "PaneClose",
    0x09: "FocusChange",
    0x0A: "ZoomToggle",
    0x0B: "ThemeChange",
    0x0C: "BellEvent",
    0x0D: "KeyboardInput",
    0x0E: "MouseMove",
    0x0F: "MouseButton",
    0x10: "MouseScroll",
    0x11: "WindowCreate",
    0x12: "WindowClose",
    0x13: "WindowFocus",
    0x14: "ClipboardPaste",
    0x15: "WindowMove",
    0x16: "SelectionEvent",
    0x17: "WindowResize",
}


def format_payload(event_type_name, payload_dict, convert_esc=False):
    """Format a decoded msgpack payload for display."""
    if event_type_name in ("PtyOutput", "PtyInput"):
        raw = payload_dict.get("data", b"")
        if isinstance(raw, list):
            raw = bytes(raw)
        if isinstance(raw, bytes):
            text = raw.decode("utf-8", errors="replace")
        else:
            text = str(raw)
        if convert_esc:
            text = text.replace("\x1b", "ESC")
        return repr(text)

    # For other event types, show the payload as key=value pairs.
    parts = []
    for k, v in payload_dict.items():
        if isinstance(v, list) and all(isinstance(x, int) for x in v):
            v = bytes(v).decode("utf-8", errors="replace")
        elif isinstance(v, bytes):
            v = v.decode("utf-8", errors="replace")
        parts.append(f"{k}={v!r}")
    return " ".join(parts)


def pane_id_from_payload(event_type_name, payload_dict):
    """Extract pane_id from a payload dict, if present."""
    return payload_dict.get("pane_id")


# ---------------------------------------------------------------------------
# V1 decoder
# ---------------------------------------------------------------------------

if version == 1:
    if events_only or summary_only or filter_pane is not None:
        print("--events-only, --summary, and --pane are only supported for FREC v2 files.")
        sys.exit(1)

    pos = 5
    frame_num = 0

    while pos < len(data):
        if pos + 12 > len(data):
            print(f"Truncated frame header at offset {pos}")
            sys.exit(1)

        timestamp_us = struct.unpack_from("<Q", data, pos)[0]
        data_len = struct.unpack_from("<I", data, pos + 8)[0]
        pos += 12

        if pos + data_len > len(data):
            print(f"Truncated frame data at offset {pos - 12}: need {data_len} bytes, have {len(data) - pos}")
            sys.exit(1)

        frame_data = data[pos:pos + data_len]
        pos += data_len

        decoded_string = frame_data.decode("utf-8", errors="replace")

        if convert_escape:
            decoded_string = decoded_string.replace("\x1b", "ESC")

        timestamp_s = timestamp_us / 1_000_000.0
        prefix = f"[{timestamp_s:8.3f}s] " if show_timing else ""

        if split_commands:
            commands = decoded_string.split("ESC")
            for i, command in enumerate(commands):
                print(f"{prefix}F{frame_num} N{i} ESC " + repr(command))
        else:
            print(f"{prefix}F{frame_num}: " + repr(decoded_string))

        frame_num += 1

    print(f"\nTotal frames: {frame_num}")
    sys.exit(0)

# ---------------------------------------------------------------------------
# V2 decoder
# ---------------------------------------------------------------------------

if version != 2:
    print(f"Unsupported version: {version}")
    sys.exit(1)

try:
    import msgpack
except ImportError:
    print("The 'msgpack' package is required for FREC v2 decoding.")
    print("Install it with: pip install msgpack")
    sys.exit(1)

# Header: magic(4) + version(1) + flags(4) + metadata_len(4) = 13
HEADER_FIXED = 13
if len(data) < HEADER_FIXED:
    print(f"File too short for v2 header (got {len(data)} bytes)")
    sys.exit(1)

flags = struct.unpack_from("<I", data, 5)[0]
metadata_len = struct.unpack_from("<I", data, 9)[0]

if len(data) < HEADER_FIXED + metadata_len:
    print(f"File too short for metadata blob (need {HEADER_FIXED + metadata_len}, got {len(data)})")
    sys.exit(1)

metadata_blob = data[HEADER_FIXED:HEADER_FIXED + metadata_len]
metadata = msgpack.unpackb(metadata_blob, raw=False)

# Footer: last 28 bytes
FOOTER_SIZE = 28
footer_start = len(data) - FOOTER_SIZE
if footer_start < HEADER_FIXED + metadata_len:
    print("File too short for footer.")
    sys.exit(1)

footer_magic = data[-4:]
if footer_magic != b"FREC":
    print(f"Invalid footer magic: {footer_magic!r}")
    # Continue anyway — might be a truncated recording.

seek_index_offset = struct.unpack_from("<Q", data, footer_start)[0]
total_duration_us = struct.unpack_from("<Q", data, footer_start + 8)[0]
total_events = struct.unpack_from("<Q", data, footer_start + 16)[0]

# ---------------------------------------------------------------------------
# Summary mode
# ---------------------------------------------------------------------------

if summary_only:
    print("=== FREC v2 Recording Summary ===")
    print(f"File:            {filename}")
    print(f"Version:         {version}")
    print(f"Flags:           0x{flags:08X}")
    print(f"Duration:        {total_duration_us / 1_000_000:.3f}s")
    print(f"Total events:    {total_events}")
    print()
    print("--- Metadata ---")
    print(f"  Freminal:      {metadata.get('freminal_version', '?')}")
    print(f"  Created at:    {metadata.get('created_at', '?')}")
    print(f"  $TERM:         {metadata.get('term', '?')}")
    print(f"  Scrollback:    {metadata.get('scrollback_limit', '?')}")

    topo = metadata.get("initial_topology", {})
    windows = topo.get("windows", [])
    print(f"  Windows:       {len(windows)}")
    for w in windows:
        wid = w.get("window_id", "?")
        size = w.get("size", ("?", "?"))
        tabs = w.get("tabs", [])
        print(f"    Window {wid}: {size[0]}x{size[1]}, {len(tabs)} tab(s)")
        for t in tabs:
            tid = t.get("tab_id", "?")
            panes = t.get("panes", {})
            print(f"      Tab {tid}: pane tree = {json.dumps(panes, default=str)[:100]}")
    print()

    # Count event types
    pos = HEADER_FIXED + metadata_len
    end = footer_start
    if seek_index_offset > 0 and seek_index_offset < len(data):
        end = min(end, seek_index_offset)

    type_counts = {}
    while pos + 13 <= end:
        _ts = struct.unpack_from("<Q", data, pos)[0]
        etype = data[pos + 8]
        plen = struct.unpack_from("<I", data, pos + 9)[0]
        pos += 13 + plen
        name = EVENT_TYPES.get(etype, f"Unknown(0x{etype:02X})")
        type_counts[name] = type_counts.get(name, 0) + 1

    print("--- Event Counts ---")
    for name in sorted(type_counts.keys()):
        print(f"  {name:20s}: {type_counts[name]}")

    sys.exit(0)

# ---------------------------------------------------------------------------
# Event stream decoding
# ---------------------------------------------------------------------------

pos = HEADER_FIXED + metadata_len
end = footer_start
if seek_index_offset > 0 and seek_index_offset < len(data):
    end = min(end, seek_index_offset)

event_num = 0

# Print header info
if not events_only:
    print(f"=== FREC v2 | {metadata.get('freminal_version', '?')} | "
          f"duration {total_duration_us / 1_000_000:.3f}s | "
          f"{total_events} events ===")
    print()

while pos + 13 <= end:
    timestamp_us = struct.unpack_from("<Q", data, pos)[0]
    event_type_byte = data[pos + 8]
    payload_len = struct.unpack_from("<I", data, pos + 9)[0]
    pos += 13

    if pos + payload_len > end:
        print(f"Truncated event payload at offset {pos - 13}")
        break

    payload_raw = data[pos:pos + payload_len]
    pos += payload_len

    event_type_name = EVENT_TYPES.get(event_type_byte, f"Unknown(0x{event_type_byte:02X})")

    # Decode msgpack payload
    try:
        payload_dict = msgpack.unpackb(payload_raw, raw=False)
        # rmp_serde serializes Rust enum variants as {variant_name: {fields...}}.
        # Unwrap the single-key wrapper to get the inner field map.
        if isinstance(payload_dict, dict) and len(payload_dict) == 1:
            inner = next(iter(payload_dict.values()))
            if isinstance(inner, dict):
                payload_dict = inner
    except Exception:
        payload_dict = {"_raw": payload_raw.hex()}

    # Apply filters
    if filter_pane is not None:
        pid = pane_id_from_payload(event_type_name, payload_dict)
        if pid is not None and pid != filter_pane:
            event_num += 1
            continue

    if events_only:
        # Only show topology/lifecycle events, not PTY data or mouse moves
        if event_type_name in ("PtyOutput", "PtyInput", "MouseMove"):
            event_num += 1
            continue

    # Format output
    timestamp_s = timestamp_us / 1_000_000.0
    prefix = f"[{timestamp_s:8.3f}s] " if show_timing else ""

    payload_str = format_payload(event_type_name, payload_dict, convert_escape)

    if split_commands and event_type_name in ("PtyOutput", "PtyInput"):
        raw = payload_dict.get("data", b"")
        if isinstance(raw, bytes):
            text = raw.decode("utf-8", errors="replace")
        else:
            text = str(raw)
        if convert_escape:
            text = text.replace("\x1b", "ESC")
        commands = text.split("ESC")
        for i, command in enumerate(commands):
            print(f"{prefix}E{event_num} {event_type_name} N{i} ESC " + repr(command))
    else:
        print(f"{prefix}E{event_num} {event_type_name}: {payload_str}")

    event_num += 1

print(f"\nTotal events decoded: {event_num}")
