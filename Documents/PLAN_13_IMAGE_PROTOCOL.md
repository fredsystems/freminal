# PLAN_13 — Inline Image Protocol Support

## Overview

Add support for displaying inline images in the terminal. This enables tools like `imgcat`,
yazi (file manager with image preview), and other image-aware terminal programs to display
images directly in the Freminal window.

**Dependencies:** Task 1 (Custom Terminal Renderer) — complete; provides the GPU rendering
pipeline needed to display image textures.
**Dependents:** None
**Primary crates:** `freminal-terminal-emulator` (parser), `freminal-buffer` (image storage),
`freminal` (GPU texture upload and rendering)
**Estimated scope:** Large (new feature, multiple protocols)

---

## Protocol Landscape

Three major inline image protocols exist. They are not mutually exclusive — most modern
terminals implement more than one.

### Sixel (DEC, 1983)

**Format:** DCS (Device Control String) sequence containing a palette-indexed bitmap.

```text
ESC P <params> q <sixel data> ESC \
```

- Data is a stream of 6-pixel-tall column strips encoded as printable ASCII characters (each
  character represents a 1×6 pixel column, with bits 0-5 mapping to pixels top-to-bottom).
- Color palette defined inline via `#<index>;2;<r>;<g>;<b>` (0-100 range per channel).
- Terminal decodes the pixel data and renders it.
- **No base64. No compression.** Raw palette-indexed bitmap in ASCII. Very bandwidth-inefficient
  for large images.

**Pros:** Oldest protocol; broad support (foot, Windows Terminal, WezTerm, xterm -ti vt340,
libsixel ecosystem). Works through tmux (with `--enable-sixel` build flag).

**Cons:** Bandwidth-inefficient (large payloads for hi-res images). Limited to 256 colors per
palette (though modern implementations allow more). No transparency in the original spec
(some terminals extend it). No image placement control — images are emitted at the cursor
and scroll with text. No animation. No shared memory or file-path transfer.

**Complexity:** Medium. Requires a DCS parser, sixel pixel decoder, palette management, and
texture upload. The parser is non-trivial but well-documented.

**Terminals:** foot, Windows Terminal, WezTerm, Black Box, xterm (with -ti vt340), st (with
patch), mlterm, contour, Zellij (buggy).

### iTerm2 Inline Images Protocol (iTerm2, ~2014)

**Format:** OSC 1337 sequence with base64-encoded file data.

```text
ESC ] 1337 ; File = [args] : <base64 data> BEL
```

Arguments: `name=<b64name>`, `size=<bytes>`, `width=<spec>`, `height=<spec>`,
`preserveAspectRatio=<0|1>`, `inline=<0|1>`.

Width/height specs: `N` (cells), `Npx` (pixels), `N%` (percentage), `auto`.

- Terminal decodes the base64 payload, identifies the image format (PNG, JPEG, GIF, etc.),
  and renders it at the specified size.
- Supports animated GIFs.
- Also supports file downloads (when `inline=0`).
- Multipart variant (iTerm2 3.5+): `MultipartFile`, `FilePart`, `FileEnd` sequences for
  tmux compatibility (avoids single giant OSC).

**Pros:** Simple protocol. Widely adopted as a de facto standard by terminals that aren't kitty.
Used by: WezTerm, VSCode terminal, Warp, Tabby, Rio, Bobcat, yazi, imgcat.

**Cons:** Entire file is base64-encoded in the escape sequence — ~33% bandwidth overhead.
No shared memory or file-path transfer. No fine-grained placement control (image appears at
cursor, sized in cells/pixels/percentage). No animation control beyond animated GIF support.
Large images may hit terminal OSC size limits.

**Complexity:** Low-Medium. Requires OSC 1337 parsing (already partially handled for other
iTerm2 extensions), base64 decoding, image format decoding (can use the `image` crate),
and texture upload.

**Terminals:** iTerm2, WezTerm, VSCode, Warp, Tabby, Rio, Bobcat, Mintty (partial).

### Kitty Graphics Protocol (kitty, 2017)

**Format:** APC (Application Program Command) escape sequence with key=value control data
and optional base64 payload.

```text
ESC _ G <control data> ; <base64 payload> ESC \
```

Control data is `key=value` pairs separated by commas. Key capabilities:

- **Transfer methods:** Direct (base64 in escape), file path, shared memory, temp file.
- **Image formats:** RGB/RGBA raw pixels, PNG, or compressed (zlib) raw pixels.
- **Placement:** Images are assigned numeric IDs. Placements reference an image ID and specify
  position (cells, pixels, or relative), z-layer, and crop region.
- **Unicode placeholders:** Virtual characters that reference image placements, allowing images
  to participate in text reflow and be positioned via normal cursor movement. This is the
  modern preferred approach.
- **Animation:** Frame-based animation with composition modes (replace, overlay, blend),
  frame durations, and loop control.
- **Deletion:** Explicit commands to delete images by ID, position, or z-layer.
- **Query:** Terminal can be queried for protocol support.
- **Storage quotas:** Terminal manages GPU memory and evicts old images under pressure.

**Pros:** Most capable protocol. Fine-grained placement, animation, multiple transfer methods
(shared memory is zero-copy for local use), image IDs allow reuse without retransmission.
Unicode placeholders work through tmux and multiplexers.

**Cons:** Most complex to implement. The full protocol is very large (transfer, display,
animation, deletion, queries, Unicode placeholders). File-path and shared-memory transfer
modes are local-only (not useful over SSH without extra tooling).

**Complexity:** High for the full protocol. A minimal implementation (direct base64 transfer +
simple display + Unicode placeholders) is Medium.

**Terminals:** kitty, Ghostty, Konsole (partial — "old" protocol variant), WezTerm (partial).

---

## Protocol Priority for Freminal

Based on ecosystem adoption and implementation complexity:

### Phase 1: iTerm2 Inline Images Protocol (Recommended first)

**Why first:**

- Simplest to implement (OSC parsing + base64 + image decode + texture).
- Broadest adoption as a "common denominator" protocol.
- Immediately enables yazi image preview, imgcat, and many other tools.
- WezTerm, VSCode, and most non-kitty terminals chose this as their first protocol.

### Phase 2: Kitty Graphics Protocol (Minimal subset)

**Why second:**

- Required for kitty-specific tools and for yazi's preferred "Kgp" adapter (Unicode
  placeholders).
- A minimal implementation (direct transfer + display + Unicode placeholders + query) covers
  the majority of real-world usage.
- Animation and shared-memory transfer can be deferred.

### Phase 3: Sixel (Optional, if demand exists)

**Why last:**

- Less efficient than iTerm2 or Kitty protocols.
- Primary use case is legacy tools and multiplexer passthrough.
- foot and Windows Terminal are the main Sixel-first terminals; Freminal already has a more
  modern rendering pipeline that better suits the other protocols.
- Can be added later if users request it or for Zellij compatibility.

---

## Architecture

### Image Storage (freminal-buffer)

Images need a representation in the buffer model so they can scroll with text and survive
reflow.

```rust
/// An inline image stored in the terminal buffer.
pub struct InlineImage {
    /// Unique image ID (auto-assigned or from Kitty protocol).
    pub id: u64,
    /// Decoded pixel data (RGBA).
    pub pixels: Arc<Vec<u8>>,
    /// Image dimensions in pixels.
    pub width_px: u32,
    pub height_px: u32,
    /// Display size in terminal cells.
    pub display_cols: usize,
    pub display_rows: usize,
}

/// A reference to an image placement within a Cell.
pub struct ImagePlacement {
    pub image_id: u64,
    /// Which portion of the image this cell covers (for multi-cell images).
    pub col_in_image: usize,
    pub row_in_image: usize,
}
```

Cells that contain image data carry an `Option<ImagePlacement>`. The actual pixel data is
stored in a separate `HashMap<u64, InlineImage>` on the `Buffer` (or a dedicated image store).

### Parser (freminal-terminal-emulator)

- **iTerm2:** Extend OSC handler to recognize `1337;File=...` and `1337;MultipartFile`/
  `FilePart`/`FileEnd`. Parse arguments, accumulate base64 data, decode image on completion.
- **Kitty:** Add APC sequence handler. Parse `_G` control data key=value pairs. Handle
  `a=t` (transmit), `a=T` (transmit+display), `a=p` (display placement), `a=d` (delete),
  `a=q` (query). Accumulate chunked payloads (kitty splits large images across multiple
  APC sequences using `m=1` continuation flag).

### Rendering (freminal — GUI)

Images are uploaded as GL textures. The custom renderer already uses glow — adding texture
quads for image regions is straightforward:

1. When `build_snapshot` encounters cells with `ImagePlacement`, include the image data
   (or texture ID) in the snapshot.
2. The renderer uploads new images as GL textures (cached by image ID).
3. During the draw pass, image cells emit textured quads instead of glyph quads.
4. Images that scroll off-screen have their textures evicted after a configurable limit.

---

## Implementation Checklist

> **Agent instructions:** Follow the Multi-Step Task Protocol from `agents.md`.

### Phase 1 — iTerm2 Inline Images

---

- [x] **13.1 — Add image storage types to freminal-buffer**
  - Define `InlineImage`, `ImagePlacement`, and the image store (`HashMap<u64, InlineImage>`).
  - Add `Option<ImagePlacement>` to `Cell` (or a parallel structure to avoid bloating every
    cell — investigate the performance tradeoff).
  - Add methods to insert an image at a cursor position, spanning multiple cells.
  - Add tests for image insertion, scrolling, and reflow behavior.
  - **Verify:** `cargo test --all` passes. No existing test regressions.
  - **Completed 2026-03-12.** Created `freminal-buffer/src/image_store.rs` with `InlineImage`
    (Arc pixel data), `ImagePlacement`, `ImageStore` (HashMap + retain_referenced GC).
    Added `Option<Box<ImagePlacement>>` to `Cell` (8 bytes null for non-image cells).
    Added `place_image()` to `Buffer` with width clipping and scroll support.
    Image store saved/restored across alternate screen, cleared on full_reset, GC'd
    in `enforce_scrollback_limit()`. 23 unit tests covering placement, scrolling, GC,
    alternate screen, cell accessors, and store operations. Fixed infinite loop bug
    in place_image (now uses push_row + enforce_scrollback_limit matching handle_lf).
    Commit: `4309da7`.

---

- [ ] **13.2 — Parse iTerm2 OSC 1337 File sequences**
  - Extend the OSC handler in `freminal-terminal-emulator` to recognize `1337;File=`.
  - Parse arguments: `name`, `size`, `width`, `height`, `preserveAspectRatio`, `inline`.
  - Accumulate base64 payload, decode on BEL/ST.
  - Decode image using the `image` crate (add as dependency to `freminal-terminal-emulator`
    or `freminal-buffer`).
  - Convert decoded image to RGBA pixel data.
  - Call into the buffer to place the image at the current cursor position.
  - Add tests with sample base64-encoded PNG/JPEG payloads.
  - **Verify:** `cargo test --all` passes. Parser correctly extracts image data from
    OSC 1337 sequences.

---

- [ ] **13.3 — Add iTerm2 MultipartFile support**
  - Handle `1337;MultipartFile=`, `1337;FilePart=`, and `1337;FileEnd` sequences.
  - Accumulate parts into a single payload buffer, then decode as in 13.2.
  - Add tests for multi-part image transfer.
  - **Verify:** `cargo test --all` passes.

---

- [ ] **13.4 — Render inline images via GL textures**
  - In `freminal/src/gui/renderer.rs`, add support for textured quads.
  - Upload `InlineImage` pixel data as GL textures (cached by image ID).
  - During the draw pass, emit textured quads for cells with `ImagePlacement`.
  - Handle image eviction when images scroll out of the scrollback limit.
  - Include image data in `TerminalSnapshot` (via `Arc` to avoid copies).
  - **Verify:** Manual test: `imgcat` displays an image inline. `cargo test --all` passes.

---

- [ ] **13.5 — Smoke test with yazi and imgcat**
  - Test with `imgcat` (iTerm2's reference tool).
  - Test with yazi file manager — confirm image preview works when yazi detects the iTerm2
    protocol.
  - Document any issues or limitations.
  - **Verify:** Both tools display images correctly.

---

### Phase 2 — Kitty Graphics Protocol (Minimal)

---

- [ ] **13.6 — Parse Kitty APC graphics sequences**
  - Add APC sequence handler to the parser.
  - Parse `_G` control data: `a` (action), `f` (format), `t` (transmission), `s`/`v` (size),
    `i` (image ID), `p` (placement ID), `m` (more data flag), `q` (quiet mode).
  - Handle chunked transfer (`m=1` continuation, `m=0` final chunk).
  - Handle `a=q` (query) — respond with OK/error.
  - Add tests for APC parsing and chunked reassembly.
  - **Verify:** `cargo test --all` passes.

---

- [ ] **13.7 — Implement Kitty direct transfer and display**
  - Handle `a=t` (transmit) and `a=T` (transmit + display) actions.
  - Support `f=32` (RGBA), `f=24` (RGB), and `f=100` (PNG) formats.
  - Support `t=d` (direct/base64) transmission medium.
  - Decode payload and store as `InlineImage`.
  - Handle `a=p` (place) and `a=d` (delete) actions.
  - Reuse the GL texture rendering from 13.4.
  - **Verify:** `cargo test --all` passes. `kitty icat` displays images (if testing against
    kitty tools).

---

- [ ] **13.8 — Implement Kitty Unicode placeholders**
  - Handle the Unicode placeholder virtual character (U+10EEEE) in the buffer.
  - Map placeholder characters to image placements.
  - Render placeholder cells as image texture quads.
  - This is required for yazi's preferred "Kgp" adapter and for tmux passthrough.
  - **Verify:** `cargo test --all` passes. yazi detects and uses the Kitty protocol.

---

### Phase 3 — Sixel (Deferred)

- [ ] **13.9 — Sixel parser and renderer**
  - DCS sequence handler for sixel data.
  - Sixel pixel decoder (palette management, 6-pixel column strips).
  - Texture upload and rendering.
  - Deferred until demand exists. Not blocking any current use case.

---

## Affected Files

| File                                               | Change Type                                              |
| -------------------------------------------------- | -------------------------------------------------------- |
| `freminal-buffer/src/cell.rs`                      | Add `Option<ImagePlacement>` or parallel image cell data |
| `freminal-buffer/src/buffer.rs`                    | Add image store, image insertion, image-aware reflow     |
| `freminal-buffer/Cargo.toml`                       | Add `image` crate dependency (if decoding happens here)  |
| `freminal-terminal-emulator/src/ansi_components/`  | OSC 1337 and APC handlers                                |
| `freminal-terminal-emulator/src/state/internal.rs` | Image placement dispatch                                 |
| `freminal-terminal-emulator/src/snapshot.rs`       | Include image data in snapshot                           |
| `freminal/src/gui/renderer.rs`                     | Textured quad rendering, texture cache                   |
| `freminal/src/gui/shaping.rs`                      | Image cell handling in shaped output                     |

---

## Dependencies (Crate)

| Crate                                             | New Dependency | Purpose                                    |
| ------------------------------------------------- | -------------- | ------------------------------------------ |
| `freminal-buffer` or `freminal-terminal-emulator` | `image`        | PNG/JPEG/GIF decoding                      |
| `freminal-buffer` or `freminal-terminal-emulator` | `base64`       | Base64 decoding (may already be available) |

---

## Risk Assessment

| Risk                                         | Likelihood | Impact | Mitigation                                             |
| -------------------------------------------- | ---------- | ------ | ------------------------------------------------------ |
| Cell bloat from `Option<ImagePlacement>`     | Medium     | Medium | Use separate sparse storage instead of per-cell Option |
| Large image payloads causing memory pressure | Medium     | Medium | Implement image eviction and storage quotas            |
| Image crate adds compile time                | Low        | Low    | Feature-gate if needed                                 |
| Protocol detection by tools (yazi)           | Medium     | Low    | Ensure DA and XTGETTCAP responses advertise support    |
| Performance regression from texture uploads  | Low        | Medium | Upload only on new/changed images; evict off-screen    |

---

## How yazi Detects Protocol Support

yazi checks `$TERM`, `$TERM_PROGRAM`, and `$XDG_SESSION_TYPE` environment variables. Its
priority order:

1. Kitty Unicode placeholders (Kgp) — if TERM contains "kitty" or TERM_PROGRAM is "ghostty"
2. Kitty old protocol (KgpOld) — if TERM_PROGRAM is "konsole"
3. iTerm2 (Iip) — if TERM_PROGRAM is "iTerm.app", "WezTerm", "vscode", etc.
4. Sixel — if terminal reports sixel support via DA
5. X11/Wayland (Überzug++) — if `$XDG_SESSION_TYPE` is "x11" or "wayland"
6. Chafa — ASCII art fallback

For Freminal to be detected by yazi, we should either:

- Set `TERM_PROGRAM=Freminal` and get yazi to add detection (upstream PR), OR
- Respond to DA queries with sixel/kitty graphics capability bits, OR
- Support the iTerm2 protocol and set `TERM_PROGRAM` to a value yazi recognizes (e.g.,
  add Freminal to yazi's detection list).

The recommended approach is: implement iTerm2 protocol first, set
`TERM_PROGRAM=Freminal`, and submit a PR to yazi adding Freminal detection.
