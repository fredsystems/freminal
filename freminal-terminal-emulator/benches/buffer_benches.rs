// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

// The legacy flat-buffer benchmarks have been removed along with the old buffer
// implementation in Phase 6. This file exists solely to satisfy Criterion's
// requirement for a valid `criterion_main!` entry point.

use criterion::Criterion;
use criterion::criterion_group;
use criterion::criterion_main;

fn noop_bench(_bench: &mut Criterion) {
    // Nothing to benchmark here — the old Buffer type has been removed.
}

criterion_group!(benches, noop_bench);
criterion_main!(benches);
