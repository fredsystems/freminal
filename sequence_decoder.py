#!/usr/bin/python3

# a helper script to evaluate terminal session recordings
#
# Reads a raw binary recording file (as produced by --recording-path)
# and decodes the bytes into readable text.
#
# Usage:
#   python3 sequence_decoder.py --recording-path=path/to/file
#   python3 sequence_decoder.py --recording-path=path/to/file --convert-escape
#   python3 sequence_decoder.py --recording-path=path/to/file --convert-escape --split-commands

import sys

filename = "sequence.bin"
convert_escape = False
split_commands = False

for arg in sys.argv[1:]:
    if arg.startswith("--recording-path"):
        filename = arg.split("=")[1]
    elif arg == "--convert-escape":
        convert_escape = True
    elif arg == "--split-commands":
        split_commands = True

try:
    with open(filename, "rb") as f:
        data = f.read()
except FileNotFoundError:
    print(f"File {filename} not found.")
    sys.exit(1)
except Exception as e:
    print(f"An error occurred: {e}")
    sys.exit(1)

# Decode bytes to a string, replacing invalid UTF-8 with the replacement character
decoded_string = data.decode("utf-8", errors="replace")

if convert_escape:
    decoded_string = decoded_string.replace("\x1b", "ESC")

if split_commands:
    commands = decoded_string.split("ESC")
    for i, command in enumerate(commands):
        print(f"N{i} ESC " + repr(command))
else:
    print(repr(decoded_string))
