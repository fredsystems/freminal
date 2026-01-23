// Copyright (C) 2024-2025 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use criterion::criterion_group;
use criterion::criterion_main;
use criterion::BenchmarkId;
use criterion::Criterion;

use freminal_common::buffer_states::buffer_type::BufferType;
use freminal_terminal_emulator::ansi_components::modes::decawm::Decawm;
use freminal_terminal_emulator::state::internal::Buffer;
use std::io::Read;

fn load_random_file() -> Vec<u8> {
    // load random_crap.txt from ../speed_tests/random_crap.txt
    let path = std::path::Path::new("../speed_tests/10000_lines.txt");
    let file = std::fs::File::open(path).unwrap();

    let mut reader = std::io::BufReader::new(file);
    let mut buffer = Vec::new();
    reader.read_to_end(&mut buffer).unwrap();

    buffer
}

fn bench_display_vec_tchar_large_chunk(bench: &mut Criterion) {
    let data = load_random_file();

    // create a Buffer
    let mut group = bench.benchmark_group("display_vec_tchar_large_chunk");
    group.bench_with_input(BenchmarkId::from_parameter("test"), &data, |b, data| {
        b.iter(|| {
            let mut buf = Buffer::new(100, 80, BufferType::Primary);

            let response = buf
                .terminal_buffer
                .insert_data(&buf.cursor_state.pos, data, &Decawm::AutoWrap)
                .unwrap(); // insert data into the buffer

            buf.format_tracker
                .push_range_adjustment(response.insertion_range);
            buf.format_tracker
                .push_range(&buf.cursor_state, response.written_range);
            buf.cursor_state.pos = response.new_cursor_pos;
        });
    });

    group.finish();
}

fn bench_display_vec_tchar_chunked(bench: &mut Criterion) {
    let data = load_random_file();
    // split data into chunks of 1000 bytes
    let data: Vec<&[u8]> = data.chunks(1000).collect();

    // create a Buffer
    let mut group = bench.benchmark_group("display_vec_tchar_chunked");
    group.bench_with_input(BenchmarkId::from_parameter("test"), &data, |b, data| {
        b.iter(|| {
            let mut buf = Buffer::new(100, 80, BufferType::Primary);

            for chunk in data {
                let response = buf
                    .terminal_buffer
                    .insert_data(&buf.cursor_state.pos, chunk, &Decawm::AutoWrap)
                    .unwrap(); // insert data into the buffer

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
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_display_vec_tchar_large_chunk,
    bench_display_vec_tchar_chunked
);
criterion_main!(benches);
