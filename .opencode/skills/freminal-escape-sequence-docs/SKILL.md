---
name: freminal-escape-sequence-docs
description: Use ONLY when working in the freminal repository AND changing the ANSI parser, terminal handler, or renderer in a way that adds, removes, or alters support for an escape sequence (C0/C1, ESC, CSI, OSC, DCS, APC, DEC mode, standard mode, charset, or renderer-side consumption). Mandates the dual-document update: ESCAPE_SEQUENCE_COVERAGE.md (status table) and ESCAPE_SEQUENCE_GAPS.md (gap roadmap), with the "Last updated" header and Planned column maintenance.
---

# Freminal: escape sequence documentation is part of the change

Any change to the parser, terminal handler, or renderer that adds,
removes, or alters support for an escape sequence (C0/C1, ESC, CSI,
OSC, DCS, APC, DEC mode, standard mode, charset, or renderer-side
consumption of a sequence) MUST update both:

- `Documents/ESCAPE_SEQUENCE_COVERAGE.md` -- the authoritative
  coverage table.
- `Documents/ESCAPE_SEQUENCE_GAPS.md` -- the gap / roadmap document.

These two documents are the source of truth for outside contributors
judging terminal compatibility. Letting them drift is a correctness
bug in the documentation itself.

## Checklist for every escape-sequence change

1. **Update `ESCAPE_SEQUENCE_COVERAGE.md`** -- the relevant row's
   status icon, notes, and task reference.
2. **Add or remove the row in `ESCAPE_SEQUENCE_GAPS.md`**:
   - If newly implemented: **remove** the gap entry entirely.
   - If newly discovered as unsupported: **add** it with
     Importance, Type, and Planned columns populated.
3. **Populate the `Planned` column** in `ESCAPE_SEQUENCE_GAPS.md`
   with the version/task that will close the gap (e.g.
   `v0.9.0 Task 72`) or `-` if unscheduled.
4. **Update the "Last updated" header line** in both files with the
   current date and a brief note on which task(s) prompted the
   update.
5. **If the change affects the Specification Coverage Summary table
   in `ESCAPE_SEQUENCE_COVERAGE.md`**, update that row as well.

## What counts as "an escape sequence change"

Anything that changes whether or how freminal interprets one of:

- C0 control codes (BEL, BS, HT, LF, VT, FF, CR, ...)
- C1 control codes
- ESC-introduced sequences (e.g. `ESC c`, `ESC =`, charset selection)
- CSI (`ESC [ ...`): SGR, cursor movement, mode set/reset,
  scrolling region, erase, ...
- OSC (`ESC ] ...`): hyperlinks, color queries, window title, ...
- DCS / APC / PM / SOS
- DEC private modes (`?` prefixed)
- Standard modes
- Renderer-side consumption (e.g. when the parser hands a sequence
  through but the renderer is what actually realizes it visually)

If you're unsure whether a change counts, treat it as if it does --
the documents are cheap to update and expensive to let drift.

## When to stop and ask

- A change adds partial support (e.g. parses the sequence but
  ignores some parameters). The coverage table needs a non-binary
  status -- decide with the user what status icon represents
  "partial" before making one up.
- The change implements a sequence that isn't currently in either
  document at all (a brand-new addition not previously tracked).
  Add the row to coverage; do NOT also add it to gaps (the gap is
  closed by the same change). Confirm the right Spec / Type
  classification.
- The "Last updated" header conflicts because someone else also
  updated the file today. Merge the notes; don't overwrite.
