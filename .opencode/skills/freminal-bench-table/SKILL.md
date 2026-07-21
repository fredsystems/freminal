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

| Stage                          | Group ID                     | Defining function                  | What it isolates                                                                                                            |
| ------------------------------ | ---------------------------- | ---------------------------------- | --------------------------------------------------------------------------------------------------------------------------- |
| Snapshot build (flatten/merge) | `bench_build_snapshot`       | `bench_build_snapshot`             | `build_snapshot_80x24_partial_dirty` sub-bench: 1-of-24-rows dirty vs. clean/full-dirty                                     |
| Glyph shaping                  | `shaping_ligatures`          | `bench_shaping_ligatures`          | `shape_visible_partial_dirty_200x50` sub-bench: `ShapingCache` IS per-row content-hashed, so only the changed row re-shapes |
| Background vertex instances    | `instanced_bg_partial_dirty` | `bench_bg_instances_partial_dirty` | `build_bg_instances_all_rows` vs. `build_bg_instances_one_row` — quantifies recoverable headroom; NOT itself incremental    |
| Foreground vertex instances    | `instanced_fg_partial_dirty` | `bench_fg_instances_partial_dirty` | `build_fg_instances_all_rows` vs. `build_fg_instances_one_row` — same headroom framing for `build_foreground_instances`     |

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
| Alternate-screen transition, end-to-end (parser -> handler -> `build_snapshot`, cache-invalidation tax on `previous_visible_snap`) | `bench_alt_screen_transition_e2e`                                                                                                                               |
| Real-world scrollback memory (bytes/line, colored corpora)                                                                         | `scrollback_memory_realworld_build_output` / `scrollback_memory_realworld_ls_color` (bare ids, no group; defining function `bench_scrollback_memory_realworld`) |

## freminal/benches/render_loop_bench.rs

Group IDs in this file frequently do NOT match the defining function
name (the historical `feed_data_*` / `build_snapshot_*` naming in the
old catalog referred to `BenchmarkId` labels or stale names, not the
actual group IDs — corrected below).

| Change area                                                             | Group ID                          | Defining function                  |
| ----------------------------------------------------------------------- | --------------------------------- | ---------------------------------- |
| Data-feed, plain-text incremental (scrolling shell)                     | `render_terminal_text`            | `bench_feed_data_incremental`      |
| Data-feed, ANSI/SGR-heavy (dense TUI)                                   | `render_terminal_text_ansi_heavy` | `bench_feed_data_ansi_heavy`       |
| Data-feed, bursty chunking pattern                                      | `render_terminal_text_bursty`     | `bench_feed_data_bursty`           |
| `build_snapshot()` after an ANSI-heavy feed                             | `render_terminal_text_snapshot`   | `bench_build_snapshot_after_feed`  |
| ArcSwap store/load (snapshot transport)                                 | `render_terminal_text_arcswap`    | `bench_arcswap_roundtrip`          |
| Glyph shaping, ligatures on/off, cache hit, partial-dirty               | `shaping_ligatures`               | `bench_shaping_ligatures`          |
| Fold-placeholder line shaping (Task 72.10)                              | `shape_placeholder_line`          | `bench_shape_placeholder_line`     |
| Background vertex-instance build (80x24, 200x50)                        | `instanced_bg`                    | `bench_bg_instances`               |
| Foreground vertex-instance build (80x24, 200x50)                        | `instanced_fg`                    | `bench_fg_instances`               |
| Background vertex instances, all-rows-vs-one-row headroom (#405 Part C) | `instanced_bg_partial_dirty`      | `bench_bg_instances_partial_dirty` |
| Foreground vertex instances, all-rows-vs-one-row headroom (#405 Part C) | `instanced_fg_partial_dirty`      | `bench_fg_instances_partial_dirty` |
| Chrome style build (`build_visuals`, theme/profile switch cost)         | `build_visuals`                   | `bench_build_visuals`              |
| Kitty image animation frame-tick selection                              | `image_animation_tick`            | `bench_image_animation_tick`       |
| Kitty image-quad vertex generation                                      | `build_image_verts`               | `bench_build_image_verts`          |

## Where the rest of the policy lives

The before/after capture procedure, the 15% regression threshold,
the recording-format table, the "add a benchmark first if none
exists" rule, and the stop-and-ask cases all live in the shared
`performance-benchmarks` skill. This skill exists only to map
freminal code areas to bench files; the policy on what to do with
that information is generic.
