# PLAN_VERSION_131.md — v0.13.1 "Scrollback Compression"

> **STATUS: ENRICHED STUB.** Durable design decisions are captured below;
> per-subtask decomposition happens at activation in a dedicated session,
> against the code as it then exists (see the `freminal-version-activation`
> skill). Do not invent subtasks early.

## Goal

Add LZ4 compression of idle scrollback as a memory multiplier on top of the compact cell
representation shipped in Task 118 (v0.12.0). This is **phase two** of the two-phase
scrollback-memory effort. Phase one (compact representation) delivers the large, guaranteed,
zero-runtime-cost win (~8–12× smaller stored rows); phase two adds an incremental multiplier
by compressing scrollback that has been idle, and decompressing it on demand when the user
scrolls into it — the strategy popularised by Ghostty.

**Depends on Task 118** (compact representation). Compression operates on the flat, compact,
pointer-free byte form that Task 118 introduces; without it there is nothing sensible to
compress (raw `Cell` structs contain `Arc`/`Box` pointers that must not be byte-compressed).

Point release after v0.13.0 (last kitty version), inserted like v0.11.1 was, so no existing
version renumbers.

---

## Task Summary

| #   | Feature                      | Scope | Status | Depends On |
| --- | ---------------------------- | ----- | ------ | ---------- |
| 119 | Scrollback Compression (LZ4) | Large | Stub   | Task 118   |

---

## Task 119 — Scrollback Compression (LZ4)

Compress blocks of idle scrollback (in the Task-118 compact form) with LZ4, decompress on
demand when scrolled into view, keep decompressed while visible, and recompress when the
region scrolls back out. The aim, stated as the user-facing outcome: **make a very large
scrollback affordable so the default can be raised further, especially for users running
many tabs/panes at once** — the aggregate memory across many buffers is what actually hurts,
not one buffer.

### Measured motivation (feasibility spike)

Spike ran against 100k-line corpora with realistic "stable-structure + unique-content"
data (repeating prompt/log/ls skeletons + genuinely unique per-line payload), plus a
pessimistic high-entropy bracket. All ratios are **on top of** the Task-118 flat compact
form:

| Corpus                    | Flat (Task 118) | flat + LZ4      | flat + zstd-3   |
| ------------------------- | --------------- | --------------- | --------------- |
| Shell session (typical)   | ~345 B/line     | ~106 B/line     | ~32 B/line      |
| Source / logs             | ~310 B/line     | ~120 B/line     | ~37 B/line      |
| High-entropy colored (WC) | ~732 B/line     | ~625 B/line     | ~429 B/line     |

Total multiplier vs. **today's** 72-byte-cell in-memory representation: flat alone ~8–12×;
flat + LZ4 ~9× (worst case) to ~31–39× (typical); flat + zstd-3 ~14× (worst) to ~100–131×
(typical). Throughput: LZ4 decompress ~2,600 MB/s, zstd-3 decompress ~1,300 MB/s — both far
above any plausible scroll/reflow rate.

### The decompression-on-the-fly tradeoff (durable analysis)

The bulk throughput number (~2,600 MB/s) is **not** the number that governs on-the-fly cost.
The real costs are:

1. **Per-call fixed overhead** — favours LZ4 (low) over zstd (higher, especially with a
   dictionary). This is why per-*line* compression is wrong: a ~40-byte line gives a terrible
   ratio and pays fixed overhead every access.
2. **Block granularity = decompress amplification** — to read one line you decompress its
   whole block. A 256-line block ≈ ~88 KB flat ≈ ~34 µs at LZ4 speed — well under one 16.6 ms
   frame. Compress in **blocks** (≈128–256 lines), never per line.
3. **Allocation churn** — naive per-access output allocation causes the jank, not the codec.
   Mitigate with a reusable scratch buffer + an LRU cache of recently-decompressed blocks
   (decompress once, keep live while visible, recompress/evict on scroll-out). Steady-state
   scrolling within a cached region does zero decompression.
4. **Reflow is the genuinely expensive case, not scroll.** Full reflow of 100k lines
   decompresses everything (~13 ms at LZ4 speed) on top of the re-wrap CPU cost that already
   exists. Mitigation: band-decompress the viewport region first, reflow + publish a snapshot,
   then finish the rest asynchronously.

### Design decisions (durable)

- **LZ4 is the default codec, not zstd.** The "fast over ratio" preference plus the on-the-fly
  decompression profile (frequent, small, hot-path block reads) point at LZ4's low per-call
  overhead and ~2,600 MB/s decompress. zstd-low may be offered as an opt-in "maximum savings"
  tier (its ratio is markedly better), but the live/hot path is LZ4. A future refinement could
  use LZ4 for recently-idle blocks and zstd for deep-cold blocks; that is not day one.
- **Compress in blocks of idle scrollback, never per line.** Per-line compression gives a
  terrible ratio and pays fixed overhead per access. Block size ≈128–256 lines, tuned at
  activation.
- **Never compress the active/visible region.** Only scrollback that has been idle past a
  threshold (Ghostty uses ~250 ms) is compressed. The visible `height` rows and the
  Task-118 compact-but-uncompressed scrollback near the viewport stay directly readable.
- **LRU cache of decompressed blocks.** Decompress on scroll-into-view, keep decompressed
  while visible, recompress/evict on scroll-out. A reusable scratch buffer avoids per-access
  allocation churn.
- **Reflow uses band-decompression.** On resize, decompress only the blocks around the
  viewport, reflow that band, publish a snapshot, then finish the remaining reflow async, so
  the user never waits on a full-scrollback decompress. This is the trickiest part of the
  task and the main reason it is a separate version from Task 118.
- **Lives in `freminal-buffer`, below the snapshot line.** Compression is internal to the
  buffer; `build_snapshot()`, the terminal-emulator, and the GUI are unaffected (they read
  decompacted/decompressed rows through the existing flatten accessors). Respects the crate
  dependency boundaries in `freminal-architecture`.
- **New dependency (`lz4_flex`) added via `flake.nix` + `Cargo.toml`** per
  `flake-dev-shell-discipline` — pure-Rust LZ4, no C toolchain, Windows-clean. `freminal-buffer`
  currently has zero serialization/compression dependencies; this is the first. (zstd, if the
  opt-in tier is built, pulls a C dependency and must be weighed against the Windows
  cross-check.)
- **Phase one already delivers most of the prize.** ~8–12× comes from Task 118 alone with no
  codec and no decompression cost. This task is the *incremental* multiplier (another ~3× LZ4
  / ~10× zstd typical) that carries essentially all the complexity (blocks, cache, reflow
  band-decompression). It is worth doing — the many-tabs-many-panes aggregate-memory case is
  real — but it is explicitly the smaller, riskier half of the effort.

### Open questions (decide at activation)

- Block size (128 vs 256 lines) and idle threshold (250 ms?) — tune against measured
  scroll/reflow behaviour at activation.
- LRU cache sizing (how many decompressed blocks kept live) and eviction policy.
- zstd opt-in "max savings" tier: ship in this version or defer? (Weigh the C-dependency /
  Windows cross-check cost against the ratio gain.)
- Async-reflow-tail mechanism: which thread performs the deferred full reflow, and how the
  partially-reflowed snapshot is represented without violating the lock-free snapshot model
  (`freminal-architecture`).
- Interaction with the recording/FREC path and with the `row_cache` (Task 118 already evicts
  cache for compact scrollback rows; compressed blocks should likewise hold no cache entry).

---

## Design Decisions (provisional)

- **Two-phase, compact-first.** Compact representation (Task 118) ships in v0.12.0 and carries
  the guaranteed win with zero runtime cost; compression (this task) is the incremental,
  higher-complexity multiplier layered on top. They are deliberately separate versions.
- **LZ4 over zstd for the hot path**, driven by the decompression-on-the-fly tradeoff, not
  bulk throughput.
- **Correctness over ratio.** Any compressed block must round-trip losslessly to the
  Task-118 compact form and thence to `Row`/`Cell`. A wrong scrollback line is worse than a
  larger one.
