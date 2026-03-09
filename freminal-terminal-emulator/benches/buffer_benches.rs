// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

// These benchmarks exercise the legacy flat-buffer implementation.
// They are compiled and run only when the `new-buffer` feature is NOT active.
// Under the default configuration (`new-buffer` enabled) Criterion still needs
// a valid `criterion_main!` entry point, so we provide a no-op group.

#[cfg(not(feature = "new-buffer"))]
use freminal_common::buffer_states::buffer_type::BufferType;
#[cfg(not(feature = "new-buffer"))]
use freminal_common::buffer_states::modes::decawm::Decawm;
#[cfg(not(feature = "new-buffer"))]
use freminal_terminal_emulator::state::internal::Buffer;
#[cfg(not(feature = "new-buffer"))]
use std::io::Read;

use criterion::Criterion;
use criterion::criterion_group;
use criterion::criterion_main;

// ---------------------------------------------------------------------------
// Legacy buffer benchmarks (old-buffer path only)
// ---------------------------------------------------------------------------

#[cfg(not(feature = "new-buffer"))]
fn load_random_file() -> Vec<u8> {
    let path = std::path::Path::new("../speed_tests/10000_lines.txt");
    let file = std::fs::File::open(path).unwrap();
    let mut reader = std::io::BufReader::new(file);
    let mut buffer = Vec::new();
    reader.read_to_end(&mut buffer).unwrap();
    buffer
}

#[cfg(not(feature = "new-buffer"))]
fn bench_display_vec_tchar_large_chunk(bench: &mut Criterion) {
    let data = load_random_file();

    let mut group = bench.benchmark_group("display_vec_tchar_large_chunk");
    group.bench_with_input(
        criterion::BenchmarkId::from_parameter("test"),
        &data,
        |b, data| {
            b.iter(|| {
                let mut buf = Buffer::new(100, 80, BufferType::Primary);

                let response = buf
                    .terminal_buffer
                    .insert_data(&buf.cursor_state.pos, data, &Decawm::AutoWrap)
                    .unwrap();

                buf.format_tracker
                    .push_range_adjustment(response.insertion_range);
                buf.format_tracker
                    .push_range(&buf.cursor_state, response.written_range);
                buf.cursor_state.pos = response.new_cursor_pos;
            });
        },
    );

    group.finish();
}

#[cfg(not(feature = "new-buffer"))]
fn bench_display_vec_tchar_chunked(bench: &mut Criterion) {
    let data = load_random_file();
    let data: Vec<&[u8]> = data.chunks(1000).collect();

    let mut group = bench.benchmark_group("display_vec_tchar_chunked");
    group.bench_with_input(
        criterion::BenchmarkId::from_parameter("test"),
        &data,
        |b, data| {
            b.iter(|| {
                let mut buf = Buffer::new(100, 80, BufferType::Primary);

                for chunk in data {
                    let response = buf
                        .terminal_buffer
                        .insert_data(&buf.cursor_state.pos, chunk, &Decawm::AutoWrap)
                        .unwrap();

                    buf.format_tracker
                        .push_range_adjustment(response.insertion_range);
                    buf.format_tracker
                        .push_range(&buf.cursor_state, response.written_range);
                    buf.cursor_state.pos = response.new_cursor_pos;

                    if let Some(range) = buf.terminal_buffer.clip_lines_for_primary_buffer() {
                        buf.format_tracker.delete_range(range).unwrap();
                    }
                }
            });
        },
    );

    group.finish();
}

// ---------------------------------------------------------------------------
// No-op benchmark used when the new-buffer feature is active so that
// `criterion_main!` always has at least one group to register.
// ---------------------------------------------------------------------------

#[cfg(feature = "new-buffer")]
fn noop_bench(_bench: &mut Criterion) {
    // Nothing to benchmark here — the old Buffer type is compiled out.
}

// ---------------------------------------------------------------------------
// Criterion wiring
// ---------------------------------------------------------------------------

#[cfg(not(feature = "new-buffer"))]
criterion_group!(
    benches,
    bench_display_vec_tchar_large_chunk,
    bench_display_vec_tchar_chunked
);

#[cfg(feature = "new-buffer")]
criterion_group!(benches, noop_bench);

criterion_main!(benches);
