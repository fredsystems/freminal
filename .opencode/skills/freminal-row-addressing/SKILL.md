---
name: freminal-row-addressing
description: Use ONLY when working in the freminal repository AND storing, comparing, translating or persisting a reference to a buffer row or cell across time -- a selection or search anchor, a prompt mark or command block, an image or kitty placement origin, a saved cursor, a damage or hover cache, a snapshot field naming rows, a GUI click or context-menu target -- or touching RowStore, RowNumber, scrollback eviction, ED 3, RIS, alternate-screen switching or width-changing reflow in freminal-buffer. Codifies logical RowNumber versus physical screen position, the never-reused number space and the alternate namespace, the ReflowRemap rule for anything that must survive a reflow, and the translation-before-use rule for snapshot coordinates.
---

# Freminal: how to refer to a row

Before Task 125, rows were addressed by their position in a `Vec`, so evicting scrollback
shifted every stored index and selections, saved cursors and kitty placements silently drifted.
The `RowStore` fixed that by giving each row a stable number. The rationale and measurements are
in `Documents/DESIGN_DECISIONS.md`, "Performance Parity and the `RowStore` (Task 125)".

## Two kinds of row reference

| Kind                          | Type / source                                 | Use for                                                                                                                    |
| ----------------------------- | --------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------- |
| Logical (attached to text)    | `RowNumber` (`freminal_common`), `row_base()` | prompt marks, command-block rows, image origins, kitty placements, GUI selection/search/click/context-menu anchors, epochs |
| Physical (attached to screen) | cursor position, `scroll_offset`, screen y    | the cursor, the DECSC slot between resizes, anything defined by the visible grid                                           |

Pick one deliberately. A value that should stay on the same text uses `RowNumber`; a value that
should stay at the same screen position uses a screen coordinate. Never store a raw `usize`
index into `rows` across an operation that can evict or reflow.

## The number space

- `RowNumber(u64)` is never reused. Eviction advances `row_base`; RIS and ED 3 advance it too
  rather than resetting it. A number below `row_base` is detectably gone, never aliased.
- The alternate screen has its own namespace starting at `RowNumber::ALTERNATE_BASE`
  (`1 << 63`), so a primary-screen mark can never resolve on the alternate screen.
- Staleness checks compare a `BufferExtent { row_base, total_rows }`, not a row count: a count
  can be unchanged across ED 3 while every row is new.

## Reflow

A width-changing resize renumbers rows. `Buffer::take_reflow_remap()` returns the
`ReflowRemap` for the reflow; everything logical that must survive it is translated through the
remap (prompts, command blocks, image extents, kitty placements, the saved cursor via
`pin_saved_cursor` / `settle_saved_cursor`). A new kind of row reference must be added to that
remap in the same change, or it falls below `row_base` and is dropped. Test it with a narrowing
and a widening reflow that moves the referenced text.

## Snapshot and GUI coordinates

Snapshots carry rows in snapshot (visible-window) space plus `row_base`. Anything the GUI draws
or hit-tests from buffer rows is translated to the visible window first
(`visible_window_start`, `translate_ranges_to_snapshot`). Feeding buffer rows into a
snapshot-space structure works for the first screenful and silently fails once scrollback
exists. Damage and hover caches live in screen-row space, because buffer and snapshot spaces
alias when the window moves (`freminal-damage-model`).

## Tests a row-reference change needs

- The reference survives eviction at full scrollback capacity.
- It survives a narrowing and a widening reflow.
- It is invalid (not aliased) after ED 3, RIS, and on the other screen.
- It works with scrollback present, not only on the first screenful.

## When to stop and ask

- It is unclear whether a value should follow the text or the screen (DECSC follows the screen
  per xterm, for example). That is a terminal-semantics question; check xterm and kitty first.
- A new reference cannot be expressed through `ReflowRemap`.
