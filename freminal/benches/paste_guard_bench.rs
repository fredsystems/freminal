// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Smart paste guard analyzer benchmarks (Task 77).
//!
//! [`PasteGuard::analyze`] runs on the GUI thread for every paste, so a
//! pathological payload (a very large clipboard with every trigger enabled and
//! many regex patterns) must not stall the UI. The plan's budget is **< 50 ms
//! for a 1 MB paste with all triggers and 20 patterns**.
//!
//! These benchmarks measure the analyzer on:
//!
//! 1. `analyze_1mb_all_triggers` — the worst case: 1 MB, multi-line, embedded
//!    control characters, and 20 dangerous-command patterns.
//! 2. `analyze_typical_paste` — a realistic ~2 KB multi-line snippet.
//! 3. `rebuild_patterns` — recompiling the 20-pattern cache (config apply /
//!    hot-reload cost).
//!
//! Pattern compilation is excluded from the per-paste path: the `analyze`
//! benches build the `PasteGuard` once outside the timed closure.

use std::time::Duration;

use criterion::{Criterion, criterion_group, criterion_main};
use freminal::gui::paste_guard::PasteGuard;
use freminal_common::config::PasteGuardConfig;
use std::hint::black_box;

fn configure() -> Criterion {
    Criterion::default()
        .sample_size(20)
        .warm_up_time(Duration::from_millis(300))
        .measurement_time(Duration::from_secs(3))
        .with_plots()
}

/// A config with all triggers on and 20 dangerous-command patterns (the 7
/// defaults plus 13 more), matching the plan's worst-case budget.
fn config_20_patterns() -> PasteGuardConfig {
    // The default config already has every trigger enabled and the 7 default
    // patterns, so we only need to pad the pattern list out to 20.
    let mut cfg = PasteGuardConfig::default();
    debug_assert!(cfg.enabled && cfg.multiline && cfg.control_chars && cfg.patterns);
    // Start from the 7 defaults, then pad to 20 with additional realistic
    // dangerous-command patterns.
    cfg.pattern_list.extend(
        [
            r"\bchmod\s+-R?\s*777\b",
            r"\bchown\s+-R\b",
            r"\bgit\s+push\s+--force\b",
            r"\bgit\s+reset\s+--hard\b",
            r"\bkill\s+-9\b",
            r"\bshutdown\b",
            r"\breboot\b",
            r"\bmkfs\b",
            r"\bfdisk\b",
            r"\bparted\b",
            r"\bnpm\s+publish\b",
            r"\bdocker\s+system\s+prune\b",
            r"\bterraform\s+destroy\b",
        ]
        .iter()
        .map(|s| (*s).to_owned()),
    );
    debug_assert_eq!(cfg.pattern_list.len(), 20);
    cfg
}

/// Build a payload of approximately `target_bytes` bytes: many lines of shell-
/// like text, a couple of embedded control characters, and a dangerous command
/// near the end so the pattern scan does not short-circuit early.
fn payload_of_size(target_bytes: usize) -> String {
    let mut out = String::with_capacity(target_bytes + 64);
    let line = "echo benchmark output line with some lorem ipsum filler text\n";
    while out.len() < target_bytes {
        out.push_str(line);
    }
    // An embedded control char (BEL) so the control-char trigger fires.
    out.push('\u{0007}');
    // A dangerous command at the very end so every pattern is scanned.
    out.push_str("\nsudo rm -rf /tmp/example\n");
    out
}

fn bench_analyze_1mb_all_triggers(c: &mut Criterion) {
    let cfg = config_20_patterns();
    let guard = PasteGuard::new(&cfg);
    let payload = payload_of_size(1024 * 1024);

    c.bench_function("analyze_1mb_all_triggers", |b| {
        b.iter(|| {
            let result = guard.analyze(black_box(&payload), black_box(&cfg));
            black_box(result);
        });
    });
}

fn bench_analyze_typical_paste(c: &mut Criterion) {
    let cfg = config_20_patterns();
    let guard = PasteGuard::new(&cfg);
    let payload = payload_of_size(2 * 1024);

    c.bench_function("analyze_typical_paste", |b| {
        b.iter(|| {
            let result = guard.analyze(black_box(&payload), black_box(&cfg));
            black_box(result);
        });
    });
}

fn bench_rebuild_patterns(c: &mut Criterion) {
    let cfg = config_20_patterns();

    c.bench_function("rebuild_patterns", |b| {
        b.iter(|| {
            let mut guard = PasteGuard::default();
            let errors = guard.rebuild(black_box(&cfg));
            black_box(errors);
            black_box(guard);
        });
    });
}

criterion_group!(
    name = benches;
    config = configure();
    targets =
        bench_analyze_1mb_all_triggers,
        bench_analyze_typical_paste,
        bench_rebuild_patterns,
);

criterion_main!(benches);
