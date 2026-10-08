---
name: freminal-bench-table
description: Use ONLY when working in the freminal repository AND touching the rendering pipeline, PTY I/O, buffer operations, ANSI parser, or `build_snapshot()`. Names exactly which benchmark file and benchmark IDs cover each performance-sensitive area of freminal. The generic before/after procedure, regression threshold, and recording format live in the shared `performance-benchmarks` skill — this skill is the freminal-specific catalog that skill points back to.
---

# Freminal: benchmark catalog

This skill is the **freminal-specific lookup table** for the generic
`performance-benchmarks` policy. When a change touches the
**rendering pipeline**, **PTY I/O**, **buffer operations**, the
**ANSI parser**, **vertex-instance building**, **image handling**, or
**scrollback compaction/compression**, find the relevant benchmark in
the tables below and follow the procedure in `performance-benchmarks`.

If no appropriate benchmark exists for the code being changed, the
agent MUST create a new benchmark as part of the task **before**
proceeding with the change (see "When no benchmark exists" in the
shared skill).

Each row below lists the **group ID** — the string passed to
`cargo bench <group-id>` (a criterion `benchmark_group` id, or a bare
`bench_function` id when a benchmark has no explicit group) — and the
**defining function** in the bench source file, since the two often
differ. Always run benchmarks by group ID, not by function name.

## #405 Part C measurement surface: partial-dirty benchmarks

Issue #405 Part C is about quantifying the cost of a single-row edit
(the common "1 of N visible rows changed" case) versus a full-screen
rebuild, across every stage of the pipeline that is NOT yet
per-row-incremental. These are the load-bearing benchmarks for that
work:

| Stage                          | Group ID                           | Defining function                  | What it isolates                                                                                                            |
| ------------------------------ | ---------------------------------- | ---------------------------------- | --------------------------------------------------------------------------------------------------------------------------- |
| Snapshot build (flatten/merge) | `bench_build_snapshot`             | `bench_build_snapshot`             | `build_snapshot_80x24_partial_dirty` sub-bench: 1-of-24-rows dirty vs. clean/full-dirty                                     |
| Glyph shaping                  | `shaping_ligatures`                | `bench_shaping_ligatures`          | `shape_visible_partial_dirty_200x50` sub-bench: `ShapingCache` IS per-row content-hashed, so only the changed row re-shapes |
| Background vertex instances    | `instanced_bg_partial_dirty`       | `bench_bg_instances_partial_dirty` | All-default, 10%-sparse, and dense corpora; all 50 rows vs. one middle row with preallocated outputs                        |
| Foreground vertex instances    | `instanced_fg_partial_dirty`       | `bench_fg_instances_partial_dirty` | Warm-atlas steady-state construction for all 50 rows vs. one middle row; NOT itself incremental                             |
| Foreground atlas rasterization | `instanced_fg_atlas_rasterization` | `bench_fg_atlas_rasterization`     | Cold-atlas full-screen foreground build, separate from steady-state vertex construction                                     |

The plain (non-`_partial_dirty`) `instanced_bg` / `instanced_fg`
groups (functions `bench_bg_instances` / `bench_fg_instances`, below)
quantify the raw vertex-instance-build cost at 80x24 and 200x50 — the
baseline the partial-dirty headroom benches are measured against.
Both `build_background_instances` and `build_foreground_instances`
`clear()` their output buffers and walk every visible row
unconditionally: there is no per-row incremental vertex path today.

## freminal-buffer/benches/buffer_row_bench.rs

| Change area                                                                                                                           | Group ID                                          | Defining function                     |
| ------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------- | ------------------------------------- |
| Buffer insert (bulk / chunked)                                                                                                        | `buffer_insert_large_line`                        | `bench_insert_large_line`             |
| Buffer insert (bulk / chunked)                                                                                                        | `buffer_insert_chunks`                            | `bench_insert_chunks`                 |
| Buffer insert with format-tag churn                                                                                                   | `bench_insert_with_color_changes`                 | `bench_insert_with_color_changes`     |
| Cursor ops (CUP + data, TUI redraw)                                                                                                   | `bench_cursor_ops`                                | `bench_cursor_ops`                    |
| Relative cursor motion (CUU/CUD/CUF/CUB)                                                                                              | `bench_move_cursor_relative`                      | `bench_move_cursor_relative`          |
| LF-heavy scroll / scrollback limit                                                                                                    | `bench_lf_heavy`                                  | `bench_lf_heavy`                      |
| LF-heavy scroll with BCE background                                                                                                   | `bench_lf_heavy_bce`                              | `bench_lf_heavy_bce`                  |
| Buffer resize / reflow                                                                                                                | `buffer_resize`                                   | `bench_resize`                        |
| Extreme softwrap                                                                                                                      | `softwrap_heavy`                                  | `bench_softwrap_heavy`                |
| Visible-window flatten                                                                                                                | `bench_visible_flatten`                           | `bench_visible_flatten`               |
| Scrollback flatten                                                                                                                    | `bench_scrollback_flatten`                        | `bench_scrollback_flatten`            |
| Scrollback render at various offsets                                                                                                  | `bench_scrollback_render`                         | `bench_scrollback_render`             |
| URL auto-detection flatten (one URL/row)                                                                                              | `bench_flatten_url_heavy`                         | `bench_flatten_url_heavy`             |
| URL auto-detection flatten (soft-wrapped)                                                                                             | `bench_flatten_wrapped_url_heavy`                 | `bench_flatten_wrapped_url_heavy`     |
| Alternate screen switch (buffer-level only, no parser/snapshot — see `bench_alt_screen_transition_e2e` below for the end-to-end cost) | `bench_alternate_screen_switch`                   | `bench_alternate_screen_switch`       |
| Erase display (ED, to end)                                                                                                            | `bench_erase_display`                             | `bench_erase_display`                 |
| Erase display (ED, full — Ps=2)                                                                                                       | `bench_erase_display_full`                        | `bench_erase_display_full`            |
| Erase display with BCE background                                                                                                     | `bench_erase_display_bce`                         | `bench_erase_display_bce`             |
| Command block record/finish cycle                                                                                                     | `command_block_record_10k` (bare id, no group)    | `bench_command_block_record`          |
| Kitty image store insert + quota scan                                                                                                 | `image_store_insert_at_quota` (bare id, no group) | `bench_image_store_insert_at_quota`   |
| Compressed scrollback block round trip                                                                                                | `bench_compressed_block_round_trip`               | `bench_compressed_block_round_trip`   |
| Scroll into a compressed scrollback region                                                                                            | `bench_scroll_into_compressed_region`             | `bench_scroll_into_compressed_region` |
| Idle-tick scrollback compaction (Task 118)                                                                                            | `bench_idle_compaction_tick`                      | `bench_idle_compaction_tick`          |
| Idle-tick scrollback compression (Task 119)                                                                                           | `bench_idle_compression_tick`                     | `bench_idle_compression_tick`         |
| LF eviction at default 10,000-row capacity: `plain` / `compressed` / `prompts` / `image` IDs (Task 125.12)                            | `bench_lf_eviction_at_capacity`                   | `bench_lf_eviction_at_capacity`       |
| LF eviction retained-row sweep, limits 1,000 / 10,000 / 50,000 (Task 125.12)                                                          | `bench_lf_eviction_scaling`                       | `bench_lf_eviction_scaling`           |
| Full-depth width-change reflow, 10k scrollback: `{live,compacted,compressed}/{widen,narrow,one_col}` IDs (Task 120.1)                 | `reflow_full_depth`                               | `bench_reflow_full_depth`             |
| Full-depth reflow, 100k scrollback, `{live,compacted,compressed}/one_col` (opt-in: runs only when the filter names `100k`)            | `reflow_full_depth_100k`                          | `bench_reflow_full_depth_100k`        |

## freminal-terminal-emulator/benches/buffer_benches.rs

Group IDs in this file match their defining function names 1:1 (no
mismatches here).

| Change area                                                                                                                        | Group ID / function                                                                                                                                             |
| ---------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| ANSI parser — plain text                                                                                                           | `bench_parse_plain_text`                                                                                                                                        |
| ANSI parser — SGR-heavy                                                                                                            | `bench_parse_sgr_heavy`                                                                                                                                         |
| ANSI parser + handler — CUP writes (TUI redraw)                                                                                    | `bench_parse_cup_writes`                                                                                                                                        |
| ANSI parser + handler — bursty PTY chunking                                                                                        | `bench_parse_bursty`                                                                                                                                            |
| `handle_incoming_data` (UTF-8 reassembly + parse)                                                                                  | `bench_handle_incoming_data`                                                                                                                                    |
| Data flatten for GUI (`data_and_format_data_for_gui`)                                                                              | `bench_data_and_format_for_gui`                                                                                                                                 |
| `build_snapshot()` — dirty/clean/partial-dirty paths                                                                               | `bench_build_snapshot` (see #405 Part C table above for `build_snapshot_80x24_partial_dirty`)                                                                   |
| `build_snapshot()` with 10k-row scrollback                                                                                         | `bench_build_snapshot_with_scrollback`                                                                                                                          |
| `build_snapshot()` with 0 / 500 / 10,000 command blocks (clean and text-change paths; the `Arc<[CommandBlock]>` generation cache)  | `bench_build_snapshot_command_blocks`                                                                                                                           |
| Alternate-screen transition, end-to-end (parser -> handler -> `build_snapshot`, cache-invalidation tax on `previous_visible_snap`) | `bench_alt_screen_transition_e2e`                                                                                                                               |
| Real-world scrollback memory (bytes/line, colored corpora)                                                                         | `scrollback_memory_realworld_build_output` / `scrollback_memory_realworld_ls_color` (bare ids, no group; defining function `bench_scrollback_memory_realworld`) |
| Sustained output at default 10,000-row capacity (`seq 1 200` burst through `handle_incoming_data`, Task 125.12)                    | `bench_sustained_output_at_capacity` (ID `seq_200_burst`)                                                                                                       |

## freminal/benches/render_loop_bench.rs

Group IDs in this file frequently do NOT match the defining function
name (the historical `feed_data_*` / `build_snapshot_*` naming in the
old catalog referred to `BenchmarkId` labels or stale names, not the
actual group IDs — corrected below).

| Change area                                                             | Group ID                           | Defining function                  |
| ----------------------------------------------------------------------- | ---------------------------------- | ---------------------------------- |
| Data-feed, plain-text incremental (scrolling shell)                     | `render_terminal_text`             | `bench_feed_data_incremental`      |
| Data-feed, ANSI/SGR-heavy (dense TUI)                                   | `render_terminal_text_ansi_heavy`  | `bench_feed_data_ansi_heavy`       |
| Data-feed, bursty chunking pattern                                      | `render_terminal_text_bursty`      | `bench_feed_data_bursty`           |
| `build_snapshot()` after an ANSI-heavy feed                             | `render_terminal_text_snapshot`    | `bench_build_snapshot_after_feed`  |
| ArcSwap store/load (snapshot transport)                                 | `render_terminal_text_arcswap`     | `bench_arcswap_roundtrip`          |
| Glyph shaping, ligatures on/off, cache hit, partial-dirty               | `shaping_ligatures`                | `bench_shaping_ligatures`          |
| Fold-placeholder line shaping (Task 72.10)                              | `shape_placeholder_line`           | `bench_shape_placeholder_line`     |
| Background vertex-instance build (80x24, 200x50)                        | `instanced_bg`                     | `bench_bg_instances`               |
| Foreground vertex-instance build (80x24, 200x50)                        | `instanced_fg`                     | `bench_fg_instances`               |
| Background vertex instances, all-rows-vs-one-row headroom (#405 Part C) | `instanced_bg_partial_dirty`       | `bench_bg_instances_partial_dirty` |
| Foreground vertex instances, all-rows-vs-one-row headroom (#405 Part C) | `instanced_fg_partial_dirty`       | `bench_fg_instances_partial_dirty` |
| Foreground cold-atlas rasterization                                     | `instanced_fg_atlas_rasterization` | `bench_fg_atlas_rasterization`     |
| Chrome style build (`build_visuals`, theme/profile switch cost)         | `build_visuals`                    | `bench_build_visuals`              |
| Kitty image animation frame-tick selection                              | `image_animation_tick`             | `bench_image_animation_tick`       |
| Kitty image-quad vertex generation                                      | `build_image_verts`                | `bench_build_image_verts`          |

## Scrollback eviction at capacity (Task 125 `RowStore` remediation)

Once scrollback is full, every line feed runs
`Buffer::enforce_scrollback_limit`, which front-drains the row storage and
scans for unreferenced blocks and images. Task 125.10 found this is the whole
`sustained-output` CPU gap, and Tasks 125.13-125.16 (`RowStore`, logical row
numbers, O(evicted) eviction) must be measured against it. The groups below run
at the real default 10,000-row limit on a 124x31 grid. The timed unit is one
burst of 200 lines (a short text insert + CR + LF each); every LF evicts one
row. Each iteration builds a fresh at-capacity buffer in untimed setup, so the
scenario state (compressed blocks, prompt marks, image) cannot scroll off over
repeated bursts. They use only public `Buffer` / emulator API and never touch
`rows` / `row_cache` / `row_block_map`, so they compile across the refactor.
Baseline name: `before_125_rowstore`.

| Group ID                              | ID(s)                                              | What it isolates                                                                                                                                                                    |
| ------------------------------------- | -------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `bench_lf_eviction_at_capacity`       | `plain`                                            | Baseline eviction: front-drain of `rows` / `row_cache` / `row_block_map` and the `merge_cache` reset                                                                                |
| `bench_lf_eviction_at_capacity`       | `compressed`                                       | Whole scrollback compacted + LZ4-compressed first (the idle-tick entry points), so eviction hits compressed blocks (`gc_unreferenced_blocks`)                                       |
| `bench_lf_eviction_at_capacity`       | `prompts`                                          | OSC 133-style prompt mark + command block every 20 lines (`prune_evicted_marks` over ~500 marks)                                                                                    |
| `bench_lf_eviction_at_capacity`       | `image`                                            | One inline image mid-scrollback (`image_store.retain_referenced` rescans every live cell per eviction)                                                                              |
| `bench_lf_eviction_scaling`           | `plain/1000`, `plain/10000`, `plain/50000`         | Scrollback-limit sweep: O(retained rows) vs O(evicted rows). Flat across limits is the 125.15 acceptance criterion                                                                  |
| `bench_sustained_output_at_capacity`  | `seq_200_burst`                                    | End to end: `seq 1 200` bytes (`\r\n`) through `handle_incoming_data` on an at-capacity emulator; the Task 125.10 `sustained-output` path                                           |
| `bench_lf_eviction_long_run`          | `plain/10000`                                      | One buffer across 2x the live row count of line feeds, so `RowStore` compaction (every ~5,000 evictions) is inside the measurement                                                  |
| `bench_build_snapshot_command_blocks` | `clean/{0,500,10000}`, `text_change/{0,500,10000}` | `build_snapshot` with N OSC 133 command blocks (one per row). The `Arc<[CommandBlock]>` is cached behind a buffer generation counter: `clean` and `text_change` must stay flat in N |

The at-capacity bursts never run long enough to reach `RowStore` compaction
(`max(live / 2, 64)` dead slots, about 5,000 evictions at the default limit),
so `bench_lf_eviction_long_run` exists to put it inside a measurement. Its
Criterion mean is the amortised cost per line feed; compaction is a stall inside
a single line feed, which a mean hides. Running the group therefore also prints
one `[burst-latency limit=N]` line per limit (10,000 and 50,000) with the mean,
median, p99 and worst 200-line burst, measured by the bench itself (a probe, not
a test: it asserts nothing). The probe only runs when the filter is empty or
names `long_run`. Read the worst-burst figure, not the mean, when judging a
change to compaction or to the `RowStore` layout.

Note: `bench_lf_heavy` / `bench_lf_heavy_bce` (4,100 LFs) no longer reach
capacity since the default scrollback rose to 10,000 (Task 118); they measure
buffer growth, not eviction. Use the groups above for eviction.

## Where the rest of the policy lives

The before/after capture procedure, the 15% regression threshold,
the recording-format table, the "add a benchmark first if none
exists" rule, and the stop-and-ask cases all live in the shared
`performance-benchmarks` skill. This skill exists only to map
freminal code areas to bench files; the policy on what to do with
that information is generic.
