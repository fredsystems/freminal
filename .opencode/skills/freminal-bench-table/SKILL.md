---
name: freminal-bench-table
description: Use ONLY when working in the freminal repository AND touching the rendering pipeline, PTY I/O, buffer operations, ANSI parser, or `build_snapshot()`. Names exactly which benchmark file and benchmark IDs cover each performance-sensitive area of freminal. The generic before/after procedure, regression threshold, and recording format live in the shared `performance-benchmarks` skill — this skill is the freminal-specific catalog that skill points back to.
---

# Freminal: benchmark catalog

This skill is the **freminal-specific lookup table** for the generic
`performance-benchmarks` policy. When a change touches the
**rendering pipeline**, **PTY I/O**, **buffer operations**, the
**ANSI parser**, or `build_snapshot()`, find the relevant benchmark
in the table below and follow the procedure in `performance-benchmarks`.

If no appropriate benchmark exists for the code being changed, the
agent MUST create a new benchmark as part of the task **before**
proceeding with the change (see "When no benchmark exists" in the
shared skill).

## Benchmark lookup table

| Change area                             | Benchmark file                                         | Key benchmarks                                                                 |
| --------------------------------------- | ------------------------------------------------------ | ------------------------------------------------------------------------------ |
| Buffer insert / cell ops                | `freminal-buffer/benches/buffer_row_bench.rs`          | `bench_insert_*`, `bench_cursor_ops`, `bench_lf_heavy`                         |
| Buffer resize / reflow                  | `freminal-buffer/benches/buffer_row_bench.rs`          | `buffer_resize`, `softwrap_heavy`                                              |
| Buffer flatten / visible rows           | `freminal-buffer/benches/buffer_row_bench.rs`          | `bench_visible_flatten`, `bench_scrollback_flatten`, `bench_scrollback_render` |
| Alternate screen                        | `freminal-buffer/benches/buffer_row_bench.rs`          | `bench_alternate_screen_switch`                                                |
| Erase operations                        | `freminal-buffer/benches/buffer_row_bench.rs`          | `bench_erase_display`, `bench_erase_display_full`                              |
| ANSI parser                             | `freminal-terminal-emulator/benches/buffer_benches.rs` | `bench_parse_plain_text`, `bench_parse_sgr_heavy`, `bench_parse_cup_writes`    |
| `handle_incoming_data` / UTF-8 assembly | `freminal-terminal-emulator/benches/buffer_benches.rs` | `bench_handle_incoming_data`, `bench_parse_bursty`                             |
| Snapshot building                       | `freminal-terminal-emulator/benches/buffer_benches.rs` | `bench_build_snapshot`, `bench_build_snapshot_with_scrollback`                 |
| Data flatten for GUI                    | `freminal-terminal-emulator/benches/buffer_benches.rs` | `bench_data_and_format_for_gui`                                                |
| Rendering pipeline / shaping            | `freminal/benches/render_loop_bench.rs`                | `feed_data_*`, `build_snapshot_*`, `shaping_ligatures`                         |
| ArcSwap / snapshot transport            | `freminal/benches/render_loop_bench.rs`                | `render_terminal_text_arcswap`                                                 |

## Where the rest of the policy lives

The before/after capture procedure, the 15% regression threshold,
the recording-format table, the "add a benchmark first if none
exists" rule, and the stop-and-ask cases all live in the shared
`performance-benchmarks` skill. This skill exists only to map
freminal code areas to bench files; the policy on what to do with
that information is generic.
