#!/usr/bin/python3

# a helper script to evaluate terminal session recordings
#
# Reads a FREC-format recording file (as produced by --recording-path)
# and decodes the frames into readable text.
#
# Format:
#   Header: b"FREC" + version byte (0x01)
#   Frame:  [u64 LE timestamp_us] [u32 LE data_length] [data bytes]
#
# Usage:
#   python3 sequence_decoder.py --recording-path=path/to/file
#   python3 sequence_decoder.py --recording-path=path/to/file --convert-escape
#   python3 sequence_decoder.py --recording-path=path/to/file --convert-escape --split-commands
#   python3 sequence_decoder.py --recording-path=path/to/file --show-timing

import struct
import sys

filename = "sequence.bin"
convert_escape = False
split_commands = False
show_timing = False

for arg in sys.argv[1:]:
    if arg.startswith("--recording-path"):
        filename = arg.split("=")[1]
    elif arg == "--convert-escape":
        convert_escape = True
    elif arg == "--split-commands":
        split_commands = True
    elif arg == "--show-timing":
        show_timing = True

try:
    with open(filename, "rb") as f:
        data = f.read()
except FileNotFoundError:
    print(f"File {filename} not found.")
    sys.exit(1)
except Exception as e:
    print(f"An error occurred: {e}")
    sys.exit(1)

# Parse header
if len(data) < 5:
    print(f"File too short for header (got {len(data)} bytes)")
    sys.exit(1)

magic = data[0:4]
if magic != b"FREC":
    print(f"Invalid magic bytes: {magic!r} (expected b'FREC')")
    sys.exit(1)

version = data[4]
if version != 1:
    print(f"Unsupported version: {version}")
    sys.exit(1)

# Parse frames
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
