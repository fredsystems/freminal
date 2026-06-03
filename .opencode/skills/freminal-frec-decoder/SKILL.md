---
name: freminal-frec-decoder
description: Use ONLY when working in the freminal repository AND analyzing files produced by `--recording-path` (the `.frec` or `.bin` files emitted by the FREC recorder). Forbids writing ad-hoc parsers or one-off python scripts for binary frec files; mandates use of `sequence_decoder.py` at the repo root, which supports filtering, escape decoding, timing, and topology events.
---

# Freminal: FREC recording analysis uses `sequence_decoder.py`

When analyzing FREC recording files (`.frec`, `.bin`, or anything
produced by `--recording-path`), agents MUST use
`sequence_decoder.py` at the repo root.

**Do NOT** write ad-hoc python parsers, throwaway shell scripts, or
inline binary parsing code to read FREC files. The decoder already
exists, is maintained, and supports everything you'll need.

## What the decoder does

- Decodes both v1 (legacy) and v2 (current) FREC formats.
- Filters by pane, window, or event type.
- Converts escape sequences to human-readable form
  (`--convert-escape`).
- Shows per-event timing (`--show-timing`).
- Lifts topology / lifecycle events out separately
  (`--events-only`, v2).
- Prints a recording summary (`--summary`, v2).

## Usage

```sh
# Basic decode
python3 sequence_decoder.py --recording-path=path/to/file

# With escape sequence conversion and timing
python3 sequence_decoder.py --recording-path=path/to/file --convert-escape --show-timing

# Filter to a specific pane (v2 only)
python3 sequence_decoder.py --recording-path=path/to/file --pane 0

# Show only topology / lifecycle events (v2 only)
python3 sequence_decoder.py --recording-path=path/to/file --events-only

# Show recording summary (v2 only)
python3 sequence_decoder.py --recording-path=path/to/file --summary
```

## If the decoder lacks something you need

Extend the decoder. Do not work around it with a one-off script.

The decoder is the canonical FREC tool for this repo; any parsing
logic that grows up in shell scripts will rot, lie about edge
cases, and create silent disagreement about what a recording
"means". A real feature in `sequence_decoder.py` is reviewable and
testable. A side-script isn't.

## When to stop and ask

- The decoder can't represent something a recording contains
  (e.g. a brand-new event type the FREC writer just started
  emitting). Stop, propose the decoder extension you'd need, and
  confirm scope.
- The recording itself looks malformed. Don't try to parse it
  defensively -- surface the malformation, since it likely
  indicates a writer bug.
