# PLAN_VERSION_130_1.md — v0.13.1 "Shipped-Protocol Conformance"

Part of v0.13.x "Kitty: Full Protocol Compliance". The compliance audit, common-work
analysis, reference specs and cross-milestone design decisions live in the overview,
`PLAN_VERSION_130.md`. This document holds the v0.13.1 tasks only.

**Status:** Planned.

The heavy conformance work on protocols freminal already ships: Unicode width and
segmentation, graphics, the graphics placement model, keyboard, and macOS Option as Alt. Each
task is an enriched stub (goal, audit findings, durable decisions, open questions) and is
decomposed at activation against the code as it then exists.

## Tasks

| #   | Task                                     | Milestone | Scope | Status  | Depends on                      |
| --- | ---------------------------------------- | --------- | ----- | ------- | ------------------------------- |
| 134 | Unicode Width & Segmentation Conformance | v0.13.1   | XL    | Planned | None                            |
| 135 | Graphics Protocol Conformance            | v0.13.1   | L     | Planned | 126.2, 129, 130, 131            |
| 136 | Graphics Placement Model & Z-Layers      | v0.13.1   | XL    | Planned | 135                             |
| 137 | Keyboard Protocol Conformance            | v0.13.1   | XL    | Planned | 128, 131; architecture sign-off |
| 152 | macOS Option as Alt & Per-Side Alt       | v0.13.1   | M     | Planned | 147; 137 coordination           |

---

## Task 134 — Unicode Width & Segmentation Conformance (stub, v0.13.1)

**Goal.** Ordinary text is split into cells exactly per kitty's algorithm ("The algorithm for
splitting text into cells", text-sizing spec). The buffer's stored cell width becomes the
single authority. This fixes ordinary-text bugs, and it is the foundation Task 104 needs.

**Audit findings (divergences D1–D12).**

- **D1, segmentation is per data chunk.** Segmentation runs inside one `TerminalOutput::Data`
  chunk (`tchar.rs:108-119`). A combining mark, VS15/VS16, ZWJ continuation or SARA AM
  arriving in a later read or after an SGR starts its own cell.
- **D2, zero-width graphemes take a column** (`row.rs:818` `.max(1)`).
- **D3, invalid UTF-8 drops the whole chunk** (`mod.rs:802-804`); kitty substitutes U+FFFD
  per maximal subpart. A grapheme over 16 bytes (`TChar` cap) drops the chunk the same way.
- **D4, a lone Regional Indicator gets width 1**; kitty gives 2.
- **D5, a standalone emoji modifier gets width 2**; kitty treats it as a zero-width mark.
- **D6, non-RGI ZWJ sequences** may sum to width 3.
- **D7, width 3 exists** (U+17D8); kitty never exceeds 2.
- **D8, `unicode-width` script ligature special cases** diverge.
- **D9, emoji wide-set edge differences.**
- **D10, explicit width cannot be expressed** (needed by OSC 66 `w=`).
- **D11, unlisted C0 controls and noncharacters become cells.**
- **D12, with DECAWM off, all remaining text is discarded** instead of overwriting the last
  column (`buffer/mod.rs:879-888`, pinned by `insert_text_no_auto_wrap_discards_excess`).
- **Width is recomputed from `TChar` at about 14 sites in two crates.** Three independent
  width implementations exist in the GUI crate alone.

**Scope sketch.**

- A stateful previous-cell attach in the buffer insert path.
- U+FFFD substitution; control and noncharacter filtering.
- Width stored on the cell (natural vs explicit, as an enum).
- Remove the recompute sites (`coords.rs`, `view_state.rs`, `search.rs`, `shaping.rs`).
- A vendored `GraphemeBreakTest.txt` plus a differential test.
- Parser, buffer and `build_snapshot` benchmarks before and after.

**Durable decisions.**

- The width algorithm lives in **one replaceable module**, because the upstream algorithm is
  volatile (#8533). Do not hand-copy kitty's tables without a conformance test.
- The cell's wide-char role becomes an enum: it replaces the `is_wide_head` /
  `is_wide_continuation` bool pair, which can currently represent an impossible state
  (`cell.rs:208`).

**Open questions.**

- Wrap `unicode-width` with overrides, or generate a kitty-identical table.
- Fix D12 (DECAWM-off overwrite) in this task: it changes ordinary-text behaviour and a test
  pins the current behaviour.
- The Unicode version pin.

## Task 135 — Graphics Protocol Conformance (stub, v0.13.1)

**Goal.** Fix every protocol-level graphics deviation that does not need the placement-model
rewrite.

**Audit findings (items not covered by 126.2 or Task 136).**

- **Responses.** Freminal replies to commands carrying neither `i` nor `I`; kitty is silent
  (`graphics_kitty.rs:1450-1454, 1526-1538, 2528-2539`). Parse errors get no response.
  `i`+`I` together is not rejected with `EINVAL`. `r=` is not echoed on `a=f`/`a=a`. The
  last chunk's `q` is ignored. The error codes `EPERM`, `EIO` and `ENOTSUP` are invented,
  while `EBADF`, `EFBIG`, `ENOSPC`, `ENOMEM` and `EILSEQ` are missing.
- **Ids.** Auto-assigned ids come from a process-global counter shared with sixel, iTerm2 and
  all panes, so they can collide with client ids (`image_store.rs:21,49`). Re-transmitting an
  existing id does not delete its placements.
- **Transmission.**
  - `a=q` checks only the format and never tries the medium.
  - `S=`/`O=` are ignored for files.
  - A shm name without a leading `/` is accepted.
  - Limits are absent: unbounded inflate, unbounded chunk accumulation, a client-supplied
    `S` trusted as capacity, and no 10000 px / 400 MB limits.
  - `o=z` inflation is capped at `MAX_KITTY_DATA_BYTES` only (Task 129), not at the size
    implied by `s`/`v`/`f` (129 review; written here by Task 147).
  - A chunked `a=f` continuation carrying `a=f` (required by spec) restarts the transfer
    (`graphics_kitty.rs:271-285`; the test uses a non-conformant shape).
- **Animation.**
  - An `a=f,r=N` edit does not use the existing frame as its canvas.
  - `a=a`/`a=c` ignore `I=`.
  - A forced current frame (`c=`) re-snaps every tick.
  - Gapless frames show for 40 ms.
  - `s=1` doesn't reset loops.
- **Deletes.**
  - `d=a/A` wipes virtual placements and all relative-placement records.
  - `d=n/N` ignores `p` and the "newest only" rule.
  - `d=f/F` deletes all frames instead of frame `r`.
- **Window size.**
  - `CSI 14 t` reports the OS window's outer rectangle (`rendering.rs:412-434`;
    `window_manipulation.rs:236-237` maps `14` to the window rather than the text area).
  - TIOCGWINSZ pixel size is 0 at first spawn and after DECCOLM.
- **Tests that enshrine deviations** must be rewritten: `kitty_delete_all_clears_virtual_placements`,
  `kitty_animation_frame_chunked_transfer_reassembles`, and
  `kitty_query_*` (format only).

**Scope sketch.** Work items G1, G2 (the remainder after 126.2), G3, G7, G8, G10 and G11 from
the audit:

- a typed `KittyGraphicsError` with a wire-code `Display`;
- id-model separation;
- `a=q` running the full load path without storing;
- limits via the Task 129 assembler;
- animation and delete parity;
- `CSI 14 t` semantics;
- a conformance suite ported from kitty's `kitty_tests/graphics.py`.

**Open questions.**

- Silence for id-less commands: follow kitty's code (silent) or the spec's "when specifying
  an id" reading; the two agree.
- `c`+`r` given together: stretch (kitty code; freminal today) or letterbox (spec prose).

**Routed from 147.14 (2026-11-20).** Decide, against kitty's source, whether a marked `t=t`
file is deleted when the read fails after the open (over the size cap, read error). Today
Windows deletes it (delete-on-close is requested at open) and unix keeps it. Align both.

**Routed from the Task 147 review (2026-11-20), A13.** On Windows, the delete-on-close open
has no check that the opened object is the canonicalised file (unix binds `(dev, ino)`); a
path swapped for a reparse point between `canonicalize` and the open could be deleted. The
std file-ID accessors are unstable; decide together with the read-failure question above.

## Task 136 — Graphics Placement Model & Z-Layers (stub, v0.13.1)

**Goal.** Give kitty placements a first-class model, independent of text cells, so that
text and images coexist, erases leave images alone, z-index layering works and placements
size themselves per placement.

**Audit findings.**

- **Text and erase destroy tiles.** Writing text over a kitty tile removes it, and
  EL/ED0/ED1/ECH/ICH/DCH erase tiles (`buffer/mod.rs:925-952`, `erase.rs`, `lines.rs`). The
  spec says other erase commands must not affect graphics.
- **Placement pre-clears unrelated images.** Placing an image clears image cells in the
  overlapping columns of **every row below the cursor** (`images.rs:219-244`).
- **No z-layering.** Negative z is not drawn under text, and `z < -1,073,741,824` is not
  drawn under backgrounds: images are always drawn last (`gpu.rs:767-787`).
- **Overlapping placements cannot coexist**: there is one `ImagePlacement` per cell.
- **Display size and mode are shared per image**, not per placement
  (`image_store.rs:158-168`).
- **Missing `c` or `r` is not derived from the aspect ratio.**
- **Cursor after placement** goes to column 0 of the row below. The spec and kitty move it
  right by the image's columns and down by rows−1.
- **Relative placements** only partially follow their parent:
  - real-parent children are stamped once and don't follow parent moves;
  - lowercase deletes don't cascade;
  - cascades work per image id rather than per placement;
  - re-putting a child leaves stale cells.
- **Unicode placeholders.**
  - The image is stretched rather than aspect-fit and centred.
  - Placement id 0 does not match any virtual placement.
  - 16-colour ids are not accepted.
  - Inheritance resolves from the last placeholder processed
    (`TerminalHandler::prev_placeholder`, `resolve_placeholder_diacritics`), not from the
    cell to the left on the same row as the kitty spec says, and it is not cleared on
    cursor motion: CR, LF and CUP leave it in place (`scroll_ops.rs` `handle_newline` /
    `handle_carriage_return`). A placeholder after CR LF or a cursor move inherits from an
    unrelated cell. Reported by frext plan 08.11, whose kitty icons write explicit row and
    column diacritics on every cell and so do not depend on the fix.

**Scope sketch.** Work items G4, G5, G6 and G9:

- a placement store with stable `RowNumber` anchors (Task 125 `RowStore`);
- a snapshot placement list (sparse);
- `build_image_verts` consuming placements;
- three render passes (below-background, below-text, above-text);
- margin-aware scroll containment, including the room an image placement makes below itself:
  today `Buffer::place_image` scrolls the whole screen (primary and alternate) whatever the
  DECSTBM margins (routed from Task 131 adversarial-review finding 3 / 131.C5).

**Durable decisions.**

- The snapshot carries a **sparse placement list**, not a per-cell parallel `Arc<Vec<…>>`.
- Every placement mutation bumps `row_epochs` for every row the placement covers. Damage is
  classified per `freminal-damage-model`; a placement change is BOUNDABLE-NOW over its
  covered rows.
- Mandatory benchmarks: `build_snapshot` and vertex build.

**Open questions.**

- Whether sixel and iTerm2 images move onto the same placement model or stay cell tiles.
- Reflow behaviour of placements.

**Routed from the Task 147 review (2026-11-20).** B9: at `background_opacity < 1`,
explicit-background cells are composited over the translucent pane base fill (under DECSCNM
since 147.27, under an OSC 11 override since 132.11), so they are tinted and their alpha is
too high; characterisation tests in `pixel_harness.rs` (`translucent_base_fill_pixel_tests`)
pin today's values. Also: an `Inline` image placement clears same-column image cells below
the new image's own rows when the cursor is mid-screen.

## Task 137 — Keyboard Protocol Conformance (stub, v0.13.1)

**Goal.** The bytes emitted for every key event match kitty's `key_encoding.c` for every flag
combination, in legacy mode as well.

**Audit findings (B1–B18).**

- **B1 (high), Alt and Super never apply to text keys** (egui delivers `Event::Text`).
  Legacy Alt+text (ESC prefix, `?1036`/`?1039`, macOS Option as Alt) moved to Task 147.24.
  Super on text keys, Alt+Backspace (B3) and the kitty-protocol encoding remain here.
- **B2 (high), modified Enter is dropped entirely** (`gui/terminal/input.rs:1894-1899`).
- **B3 (high), Enter/Tab/Backspace/Escape carry no modifiers.** Shift+Tab sends HT instead of
  `CSI Z`. The terminfo entry declares `kcbt=\E[Z` (Task 151), so fixing B3 makes it
  true; the Task 151 table-of-truth test then moves `kcbt` out of `DECLARED_NOT_SENT`.
- **B4 (high), key identity comes from typed text.**
  - Non-ASCII text is emitted as one `CSI u` per UTF-8 byte. (Fixed for flag 8 by the Task 147
    review, A5: one `CSI codepoint u` per character. The flag-4 shifted key for non-ASCII is
    still missing.)
  - Shifted symbols lose Shift and use the wrong code.
  - Caps Lock is inferred as Shift.
  - Text-key repeat is reported as press.
- **B5, the flag-4 shifted key is always emitted.** The base-layout key is faked, and the
  shifted table is hard-coded US-QWERTY. Tests pin the wrong output.
- **B6, `Ctrl(c)` hard-codes modifier 5.**
- **B7, Ctrl+digit and Ctrl+punctuation** emit raw C0 bytes under flag 1, or are dropped.
- **B8, unmodified F1–F4 and DECCKM arrows** keep SS3 under flags 1 and 8.
- **B9, Enter/Tab/Backspace emit release/repeat without flag 8.**
- **B10, flag 2 alone is inert.**
- **B11, keypad regressions** from the raw intercept: NumLock-off navigation keys and
  text-producing keypad keys sent as `CSI u`. The "DECKPAM ignored" half moved to Task 147.5
  (legacy application keypad sends SS3).
- **B12, F13–F35 are unreachable from the GUI.**
- **B13, legacy modified F3 is sent as `CSI 1;mR`.**
- **B14, modifier-key events** likely carry stale modifier bits on Wayland.
- **B15, double alt-screen entry loses the main stack** (fixed in Task 131).
- **B16, the release path reconstructs inputs lossily.**
- **Missing keys:** hyper/meta bits and keys, AltGr (ISO_LEVEL3), and six media keys.
  These are available as winit logical keys.
- **Lock bits remain reverted** (114.11). winit still exposes no lock state (PR #4244 is
  open).

**Scope sketch.** Work items K0–K14:

- K0, a characterisation harness first: synthetic egui events across a flag matrix, with
  golden vectors from `key_encoding.c`;
- K1, a key-event model;
- K2–K11, fixes to the encoder and the GUI layer;
- K12, docs;
- K13, IME, coordinated with Task 88;
- K14, lock-key events (the event half only).

**Hard gate: architecture sign-off required before decomposition.** The fix needs key
identity at the GUI boundary (logical key without modifiers, text with all modifiers,
location, physical key). There are two options:

- (a) widen the Task 114 raw-winit intercept to all key events, reversing a Task 114 durable
  decision;
- (b) stay on egui 0.36 (`Event::Key.physical_key`) plus a small windowing side-channel.

This is a `freminal-architecture` and `autonomy-boundaries` stop.

**Open questions.**

- (a) vs (b) above.
- Ctrl+`-` maps to 0x1F (xterm) or `-` (kitty spec).
- Shipping CapsLock/NumLock/ScrollLock key events under flag 8 without the lock bits.

**Routed from the Task 147 review (2026-11-20).** A12/B5: the application keypad sends
`SS3 <final>` with no modifier parameter (xterm adds one, `input.c:1394-1396`), and VT52
keypad keys need `ESC ? <final>` (`input.c:855-859`). B10: under `modifyOtherKeys`, xterm sends
some Alt+letter combinations as `CSI 27 ; mod ; code ~`; freminal always uses the ESC prefix.

**Routed from PR #589 review (R33, 2026-11-20):** broadcast input encodes a key once, with the
active pane's modes, and sends those bytes to every target pane. A target in a different mode
(DECCKM, keypad, kitty flags, and since 147.4 S8C1T framing) gets bytes for the wrong mode.
The fix is to send the logical key and encode it per destination, which is the same
key-identity boundary this task decides.

**Also routed (2026-11-20):** under kitty flags 1 and 8, Alt+text is sent as `CSI <cp> u`
without the Alt modifier (`;3`). Under flags 8 and 16, `associated_text` is the whole `Event::Text`
(`gui/terminal/input.rs`), so a multi-character commit (`ab`, an IME string) repeats the full
text on every per-character `CSI u`.

## Task 152 — macOS Option as Alt & Per-Side Alt (stub, v0.13.1)

**Goal.** On macOS, Option can act as Alt for terminal input, so Option+b sends `ESC b`
under `?1036` instead of composing `∫`, with a per-side choice: a new
`[keyboard] macos_option_as_alt = "none" | "left" | "right" | "both"` (default `none`), as kitty
(`macos_option_as_alt`) and Ghostty (`macos-option-as-alt`) offer. Created 2026-11-19 by
maintainer decision, when 147.24 shipped `?1036` without the macOS half.

**Findings (147.24).**

- On macOS, egui-winit fills `Event::Text` from winit's `text`, which is the character Option
  has already composed. Sending the base key needs winit's macOS-only
  `KeyEventExtModifierSupplement::text_with_all_modifiers` / `key_without_modifiers`, which
  freminal does not read today; it would have to be carried from `freminal-windowing` to the
  input encoder.
- Per-side Option: `freminal-windowing`'s `RawKeyMods` carries one `alt` flag and the
  raw-key path only covers egui-blocked keys, so it carries no text. egui delivers
  `Event::Key { AltLeft / AltRight }`, so per-side state is available, but it needs a
  cross-frame `AltKeyState` (like `SuperKeyState`) through `InputCarryState`,
  `PaneRenderCache` and `widget.rs`.
- 147.24 left the decision as a pure function (`gui/terminal/alt_text.rs`, parameterised over
  `HostPlatform`), with macOS Option always composing, so this task extends that function
  rather than adding a second path.

**Approach (maintainer decision, 2026-11-19: carry the base key through the windowing
layer).** A windowing-side channel delivers, for each text-producing key event on macOS, the
key without modifiers and which Option key (left/right) is held; the GUI uses it in place of
`Event::Text` when the configured side is held, so the base character gets the Alt prefix.
Full `freminal-config-options` checklist for the new option. AltGr rules from 147.24 stay
(Ctrl+Alt is never a prefix). Kitty keyboard encodings stay Task 137's.

**Coordination with Task 137.** Task 137 needs key identity at the GUI boundary and is gated
on an architecture choice between widening the Task 114 raw-winit intercept and an egui
`physical_key` plus a windowing side channel. This task's channel is the same seam, so it is
designed against that choice: either Task 137's decision is made first and this task uses it,
or this task lands the side channel in a form Task 137 can extend. Decided at activation with
the maintainer; `freminal-architecture` applies (windowing must not grow GUI policy).

**Open questions.**

- Whether `left`/`right` need the base key from the physical key (layout-independent) or the
  logical key without modifiers (layout-aware); kitty and Ghostty both use the latter.
- Dead keys and IME composition on macOS while Option is Alt (they must not be broken for the
  side that is not Alt).
- Whether other platforms get a per-side Alt choice too (today Alt is always the prefix there).
