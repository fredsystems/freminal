// Copyright ...
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

use freminal_buffer::buffer::Buffer;
use freminal_common::buffer_states::tchar::TChar;

use std::fs::File;
use std::io::Read;
use std::time::Duration;

// ---------------------------------------------------------------
// Criterion configuration: FAST RUNS
// ---------------------------------------------------------------
fn configure() -> Criterion {
    Criterion::default()
        .sample_size(10) // small, fast samples
        .warm_up_time(Duration::from_millis(300)) // fast warmup
        .measurement_time(Duration::from_secs(1)) // short measure
        .with_plots() // keep report output
}

// ---------------------------------------------------------------
// Helper: load input file fully into Vec<TChar>
// ---------------------------------------------------------------
fn load_tchars(path: &str) -> Vec<TChar> {
    let mut file = File::open(path).expect("benchmark input missing");
    let mut buf = Vec::new();
    file.read_to_end(&mut buf).unwrap();

    buf.into_iter().map(TChar::from).collect()
}

// ---------------------------------------------------------------
// Benchmark: inserting a large Vec<TChar> in one go
// ---------------------------------------------------------------
fn bench_insert_large_line(c: &mut Criterion) {
    let data = load_tchars("../speed_tests/10000_lines.txt");

    let mut group = c.benchmark_group("buffer_insert_large_line");
    group.throughput(Throughput::Elements(data.len() as u64));

    group.bench_function(BenchmarkId::new("insert_full", data.len()), |b| {
        b.iter(|| {
            let mut buf = Buffer::new(100, 80);
            buf.insert_text(&data);
        })
    });

    group.finish();
}

// ---------------------------------------------------------------
// Benchmark: inserting in chunks
// ---------------------------------------------------------------
fn bench_insert_chunks(c: &mut Criterion) {
    let data = load_tchars("../speed_tests/10000_lines.txt");
    let chunks: Vec<&[TChar]> = data.chunks(1000).collect();

    let mut group = c.benchmark_group("buffer_insert_chunks");
    group.throughput(Throughput::Elements(data.len() as u64));

    group.bench_function(BenchmarkId::new("insert_chunks_1000", chunks.len()), |b| {
        b.iter(|| {
            let mut buf = Buffer::new(100, 80);
            for chunk in &chunks {
                buf.insert_text(chunk);
            }
        })
    });

    group.finish();
}

// ---------------------------------------------------------------
// Benchmark: resizing
// ---------------------------------------------------------------
fn bench_resize(c: &mut Criterion) {
    let data = load_tchars("../speed_tests/10000_lines.txt");

    let mut group = c.benchmark_group("buffer_resize");

    group.bench_with_input(BenchmarkId::new("reflow_width", 40), &data, |b, data| {
        b.iter(|| {
            let mut buf = Buffer::new(100, 80);
            buf.insert_text(data);
            buf.set_size(40, 80);
        })
    });

    group.bench_with_input(BenchmarkId::new("shrink_height", 20), &data, |b, data| {
        b.iter(|| {
            let mut buf = Buffer::new(100, 200);
            buf.insert_text(data);
            buf.set_size(100, 20);
        })
    });

    group.finish();
}

// ---------------------------------------------------------------
// Benchmark: extreme softwrap behavior
// ---------------------------------------------------------------
fn bench_softwrap_heavy(c: &mut Criterion) {
    let long_line = "a".repeat(5000);
    let data: Vec<TChar> = long_line.chars().map(TChar::from).collect();

    let mut group = c.benchmark_group("softwrap_heavy");

    group.bench_function("wrap_long_line_to_width_10", |b| {
        b.iter(|| {
            let mut buf = Buffer::new(100, 80);
            buf.insert_text(&data);
            buf.set_size(10, 80);
        });
    });

    group.finish();
}

// ---------------------------------------------------------------
// Criterion bootstrap
// ---------------------------------------------------------------
criterion_group!(
    name = benches;
    config = configure();
    targets =
        bench_insert_large_line,
        bench_insert_chunks,
        bench_resize,
        bench_softwrap_heavy,
);

criterion_main!(benches);
