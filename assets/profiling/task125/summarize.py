#!/usr/bin/env python3
"""Summarize paired Task 125 terminal measurements deterministically."""

from __future__ import annotations

import argparse
import csv
import random
import statistics
from collections import defaultdict
from dataclasses import dataclass
from pathlib import Path

BOOTSTRAP_SEED = 125
BOOTSTRAP_RESAMPLES = 10_000
MATERIAL_FLOOR_MS_PER_SECOND = 0.5


@dataclass(frozen=True)
class Sample:
    repeat: int
    workload: str
    terminal: str
    wall_seconds: float
    task_clock_ms: float

    @property
    def task_clock_ms_per_second(self) -> float:
        return self.task_clock_ms / self.wall_seconds


def read_samples(path: Path) -> list[Sample]:
    with path.open(newline="", encoding="utf-8") as handle:
        rows = csv.DictReader(handle)
        return [
            Sample(
                repeat=int(row["repeat"]),
                workload=row["workload"],
                terminal=row["terminal"],
                wall_seconds=float(row["wall_seconds"]),
                task_clock_ms=float(row["task_clock_ms"]),
            )
            for row in rows
        ]


def percentile(sorted_values: list[float], fraction: float) -> float:
    index = round((len(sorted_values) - 1) * fraction)
    return sorted_values[index]


def bootstrap_median_interval(values: list[float]) -> tuple[float, float]:
    rng = random.Random(BOOTSTRAP_SEED)
    medians = sorted(
        statistics.median(rng.choices(values, k=len(values)))
        for _ in range(BOOTSTRAP_RESAMPLES)
    )
    return percentile(medians, 0.025), percentile(medians, 0.975)


def summarize(samples: list[Sample], expected_repeats: int) -> list[dict[str, object]]:
    grouped: dict[str, dict[str, dict[int, float]]] = defaultdict(
        lambda: defaultdict(dict)
    )
    for sample in samples:
        if sample.repeat in grouped[sample.workload][sample.terminal]:
            raise ValueError(
                f"duplicate sample: {sample.workload}/{sample.terminal}/{sample.repeat}"
            )
        grouped[sample.workload][sample.terminal][sample.repeat] = (
            sample.task_clock_ms_per_second
        )

    results: list[dict[str, object]] = []
    for workload, terminals in sorted(grouped.items()):
        required = {"freminal", "wezterm", "ghostty"}
        if set(terminals) != required:
            missing = sorted(required - set(terminals))
            unexpected = sorted(set(terminals) - required)
            raise ValueError(
                f"{workload}: missing terminals: {missing}; unexpected: {unexpected}"
            )
        peer = max(
            ("wezterm", "ghostty"),
            key=lambda name: statistics.median(terminals[name].values()),
        )
        repeats = sorted(set(terminals["freminal"]) & set(terminals[peer]))
        if len(repeats) != expected_repeats:
            raise ValueError(
                f"{workload}: expected {expected_repeats} paired repeats, got {len(repeats)}"
            )
        deltas = [
            terminals["freminal"][repeat] - terminals[peer][repeat]
            for repeat in repeats
        ]
        median_delta = statistics.median(deltas)
        low, high = bootstrap_median_interval(deltas)
        results.append(
            {
                "workload": workload,
                "peer": peer,
                "median_delta_ms_per_second": median_delta,
                "ci95_low": low,
                "ci95_high": high,
                # This remediation gate is intentionally one-sided: Freminal
                # being materially faster does not require a fix.
                "material": low > 0.0
                and median_delta >= MATERIAL_FLOOR_MS_PER_SECOND,
            }
        )
    return results


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("csv_path", type=Path)
    parser.add_argument("--expected-repeats", type=int, required=True)
    args = parser.parse_args()
    for result in summarize(read_samples(args.csv_path), args.expected_repeats):
        print(
            "{workload}: peer={peer} delta={median_delta_ms_per_second:.6f} "
            "ms/s ci95=[{ci95_low:.6f},{ci95_high:.6f}] material={material}".format(
                **result
            )
        )


if __name__ == "__main__":
    main()
