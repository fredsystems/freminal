#!/usr/bin/env python3
"""Deterministic tests for summarize.py.

Run with: python3 -m unittest assets/profiling/task125/test_summarize.py
(or `python3 test_summarize.py` from this directory).
"""

from __future__ import annotations

import random
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import summarize

WALL_SECONDS = 20.0


def sample(
    repeat: int, workload: str, terminal: str, ms_per_second: float
) -> summarize.Sample:
    """A sample whose task-clock rate is exactly `ms_per_second`."""
    return summarize.Sample(
        repeat=repeat,
        workload=workload,
        terminal=terminal,
        wall_seconds=WALL_SECONDS,
        task_clock_ms=ms_per_second * WALL_SECONDS,
    )


def series(
    workload: str, freminal: list[float], wezterm: list[float], ghostty: list[float]
) -> list[summarize.Sample]:
    samples: list[summarize.Sample] = []
    for terminal, values in (
        ("freminal", freminal),
        ("wezterm", wezterm),
        ("ghostty", ghostty),
    ):
        samples.extend(
            sample(repeat, workload, terminal, value)
            for repeat, value in enumerate(values, start=1)
        )
    return samples


def only(results: list[dict[str, object]]) -> dict[str, object]:
    assert len(results) == 1
    return results[0]


class PairedDeltaTests(unittest.TestCase):
    def test_delta_is_freminal_minus_peer_paired_by_repeat(self) -> None:
        # Pairing by repeat matters: the unpaired medians (10 and 8) would give
        # 2.0, but the paired deltas are [5, 0, 1] whose median is 1.0.
        result = only(
            summarize.summarize(
                series(
                    "w",
                    freminal=[12.0, 10.0, 9.0],
                    wezterm=[7.0, 10.0, 8.0],
                    ghostty=[1.0, 1.0, 1.0],
                ),
                expected_repeats=3,
            )
        )
        self.assertEqual(result["peer"], "wezterm")
        self.assertEqual(result["n_pairs"], 3)
        self.assertAlmostEqual(result["median_delta_ms_per_second"], 1.0)
        # Resampling three values can only return values from the data, so the
        # bounds are the extreme paired deltas.
        self.assertAlmostEqual(result["ci95_low"], 0.0)
        self.assertAlmostEqual(result["ci95_high"], 5.0)

    def test_pairing_ignores_row_order(self) -> None:
        forward = series("w", [5.0, 6.0, 9.0], [4.0, 4.0, 4.0], [1.0, 1.0, 1.0])
        shuffled = list(reversed(forward))
        self.assertEqual(
            summarize.summarize(forward, 3), summarize.summarize(shuffled, 3)
        )

    def test_negative_delta_is_reported_negative(self) -> None:
        result = only(
            summarize.summarize(
                series("w", [1.0] * 3, [3.0] * 3, [2.0] * 3), expected_repeats=3
            )
        )
        self.assertAlmostEqual(result["median_delta_ms_per_second"], -2.0)


class SlowerPeerSelectionTests(unittest.TestCase):
    def test_peer_is_the_one_with_the_higher_median_rate(self) -> None:
        result = only(
            summarize.summarize(
                series(
                    "w",
                    freminal=[10.0] * 3,
                    wezterm=[4.0] * 3,
                    ghostty=[6.0] * 3,
                ),
                expected_repeats=3,
            )
        )
        self.assertEqual(result["peer"], "ghostty")
        self.assertAlmostEqual(result["median_delta_ms_per_second"], 4.0)

    def test_selection_uses_the_median_not_the_mean(self) -> None:
        # wezterm has the higher mean (one outlier) but the lower median.
        result = only(
            summarize.summarize(
                series(
                    "w",
                    freminal=[10.0] * 3,
                    wezterm=[1.0, 1.0, 100.0],
                    ghostty=[5.0, 5.0, 5.0],
                ),
                expected_repeats=3,
            )
        )
        self.assertEqual(result["peer"], "ghostty")

    def test_peer_is_chosen_per_workload(self) -> None:
        samples = series("a", [9.0] * 3, [8.0] * 3, [1.0] * 3) + series(
            "b", [9.0] * 3, [1.0] * 3, [8.0] * 3
        )
        peers = {r["workload"]: r["peer"] for r in summarize.summarize(samples, 3)}
        self.assertEqual(peers, {"a": "wezterm", "b": "ghostty"})


class BootstrapTests(unittest.TestCase):
    def test_seed_is_125(self) -> None:
        self.assertEqual(summarize.BOOTSTRAP_SEED, 125)
        self.assertEqual(summarize.BOOTSTRAP_RESAMPLES, 10_000)

    def test_golden_intervals(self) -> None:
        # Pinned outputs of the seed-125, 10,000-resample bootstrap. A change
        # here means the statistic or its RNG stream changed, which silently
        # changes every past and future verdict.
        self.assertEqual(
            summarize.bootstrap_median_interval([1.0, 2.0, 4.0, 8.0, 16.0]),
            (1.0, 16.0),
        )
        self.assertEqual(
            summarize.bootstrap_median_interval([0.2, 0.9, 1.4]), (0.2, 1.4)
        )
        self.assertEqual(
            summarize.bootstrap_median_interval([0.5, 0.7, 0.6, 0.9, 1.1, 0.4, 0.8]),
            (0.5, 0.9),
        )

    def test_repeatable_and_independent_of_global_random_state(self) -> None:
        values = [0.5, 0.7, 0.6, 0.9, 1.1, 0.4, 0.8]
        random.seed(1)
        first = summarize.bootstrap_median_interval(values)
        random.seed(2)
        random.random()
        second = summarize.bootstrap_median_interval(values)
        self.assertEqual(first, second)

    def test_constant_deltas_collapse_to_a_point(self) -> None:
        self.assertEqual(summarize.bootstrap_median_interval([0.7] * 5), (0.7, 0.7))

    def test_interval_stays_inside_the_data_and_brackets_the_median(self) -> None:
        values = [0.3, 2.5, 1.1, 0.9, 4.0, 1.7, 2.2]
        low, high = summarize.bootstrap_median_interval(values)
        self.assertGreaterEqual(low, min(values))
        self.assertLessEqual(high, max(values))
        self.assertLessEqual(low, sorted(values)[len(values) // 2])
        self.assertGreaterEqual(high, sorted(values)[len(values) // 2])


class MaterialRuleTests(unittest.TestCase):
    """material = (interval low > 0) and (median delta >= 0.5 ms/s)."""

    def material(self, deltas: list[float]) -> bool:
        peer = [10.0] * len(deltas)
        freminal = [10.0 + d for d in deltas]
        result = only(
            summarize.summarize(
                series("w", freminal, peer, [0.0] * len(deltas)),
                expected_repeats=len(deltas),
            )
        )
        return bool(result["material"])

    def test_floor_is_half_a_millisecond_per_second(self) -> None:
        self.assertEqual(summarize.MATERIAL_FLOOR_MS_PER_SECOND, 0.5)

    def test_clear_positive_gap_above_floor_is_material(self) -> None:
        self.assertTrue(self.material([0.6] * 7))

    def test_exactly_at_the_floor_is_material(self) -> None:
        self.assertTrue(self.material([0.5] * 7))

    def test_positive_but_below_floor_is_not_material(self) -> None:
        self.assertFalse(self.material([0.4] * 7))

    def test_interval_touching_zero_is_not_material(self) -> None:
        # Median is 1.0 (above the floor) but three of seven pairs are 0.0, so
        # resampled medians reach 0.0 and the interval low bound is not
        # strictly above zero.
        self.assertFalse(self.material([0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0]))

    def test_interval_spanning_zero_is_not_material(self) -> None:
        self.assertFalse(self.material([-1.0, -0.5, 0.2, 1.0, 1.5, 2.0, 2.5]))

    def test_freminal_materially_faster_is_not_flagged(self) -> None:
        self.assertFalse(self.material([-3.0] * 7))


class IntervalLabelTests(unittest.TestCase):
    def results(self, repeats: int) -> dict[str, object]:
        return only(
            summarize.summarize(
                series("w", [11.0] * repeats, [10.0] * repeats, [1.0] * repeats),
                expected_repeats=repeats,
            )
        )

    def test_screen_sized_samples_are_a_range(self) -> None:
        result = self.results(3)
        self.assertEqual(result["interval_kind"], "range")
        line = summarize.format_result(result)
        self.assertIn("range=[", line)
        self.assertNotIn("ci95=[", line)
        self.assertIn("n=3", line)

    def test_four_samples_are_still_a_range(self) -> None:
        self.assertEqual(self.results(4)["interval_kind"], "range")

    def test_five_or_more_samples_are_a_ci95(self) -> None:
        for repeats in (5, 7):
            result = self.results(repeats)
            self.assertEqual(result["interval_kind"], "ci95")
            line = summarize.format_result(result)
            self.assertIn("ci95=[", line)
            self.assertNotIn("range=[", line)

    def test_machine_readable_field_names_are_stable(self) -> None:
        self.assertEqual(
            set(self.results(3)),
            {
                "workload",
                "peer",
                "n_pairs",
                "median_delta_ms_per_second",
                "interval_kind",
                "ci95_low",
                "ci95_high",
                "material",
            },
        )

    def test_material_screen_result_is_flagged_as_screen_only(self) -> None:
        line = summarize.format_result(self.results(3))
        self.assertIn("material=True", line)
        self.assertIn("screen only", line)
        self.assertNotIn("screen only", summarize.format_result(self.results(7)))


class ValidationTests(unittest.TestCase):
    def test_wrong_repeat_count_is_rejected(self) -> None:
        with self.assertRaisesRegex(ValueError, "freminal has 3 repeats, expected 7"):
            summarize.summarize(series("w", [1.0] * 3, [1.0] * 3, [1.0] * 3), 7)

    def test_short_terminal_that_is_not_the_peer_is_rejected(self) -> None:
        # ghostty is the faster terminal, so wezterm is the chosen peer; the
        # old Freminal/peer-only check never looked at ghostty's short series.
        with self.assertRaisesRegex(ValueError, "ghostty has 2 repeats, expected 3"):
            summarize.summarize(
                series("w", [9.0] * 3, [8.0] * 3, [1.0] * 2), expected_repeats=3
            )

    def test_short_peer_terminal_is_rejected(self) -> None:
        with self.assertRaisesRegex(ValueError, "wezterm has 2 repeats, expected 3"):
            summarize.summarize(
                series("w", [9.0] * 3, [8.0] * 2, [1.0] * 3), expected_repeats=3
            )

    def test_extra_repeats_on_one_terminal_are_rejected(self) -> None:
        with self.assertRaisesRegex(ValueError, "ghostty has 4 repeats, expected 3"):
            summarize.summarize(
                series("w", [9.0] * 3, [8.0] * 3, [1.0] * 4), expected_repeats=3
            )

    def test_right_count_but_different_repeat_ids_is_rejected(self) -> None:
        samples = [
            s
            for s in series("w", [9.0] * 3, [8.0] * 3, [1.0] * 3)
            if not (s.terminal == "ghostty" and s.repeat == 3)
        ]
        samples.append(sample(4, "w", "ghostty", 1.0))
        with self.assertRaisesRegex(ValueError, "repeat IDs differ"):
            summarize.summarize(samples, expected_repeats=3)

    def test_validation_is_per_workload(self) -> None:
        samples = series("a", [9.0] * 3, [8.0] * 3, [1.0] * 3) + series(
            "b", [9.0] * 3, [8.0] * 3, [1.0] * 2
        )
        with self.assertRaisesRegex(ValueError, "^b: ghostty has 2 repeats"):
            summarize.summarize(samples, expected_repeats=3)

    def test_missing_terminal_is_rejected(self) -> None:
        samples = [
            s
            for s in series("w", [1.0] * 3, [1.0] * 3, [1.0] * 3)
            if s.terminal != "ghostty"
        ]
        with self.assertRaisesRegex(ValueError, "missing terminals"):
            summarize.summarize(samples, 3)

    def test_duplicate_sample_is_rejected(self) -> None:
        samples = series("w", [1.0] * 3, [1.0] * 3, [1.0] * 3)
        samples.append(sample(1, "w", "freminal", 2.0))
        with self.assertRaisesRegex(ValueError, "duplicate sample"):
            summarize.summarize(samples, 3)


class CsvCompatibilityTests(unittest.TestCase):
    HEADER = (
        "repeat,workload,terminal,wall_seconds,task_clock_ms,user_task_clock_ms,"
        "kernel_task_clock_ms,cycles,instructions,context_switches,wakeups,"
        "gpu_gfx_ns,grid_rows,grid_cols,gpu_status,exited_tids"
    )

    def test_extra_capture_columns_and_blank_gpu_cell_are_accepted(self) -> None:
        rows = [
            # repeat,workload,terminal,wall,task_clock, user,kernel,cycles,instr,
            # switches,wakeups,gpu,rows,cols,gpu_status,exited_tids
            "1,idle-blink,freminal,20,400,300,100,1,2,3,4,,31,124,unavailable,0",
            "1,idle-blink,wezterm,20,200,150,50,1,2,3,4,12345,31,120,available,2",
            "1,idle-blink,ghostty,20,100,75,25,1,2,3,4,999,30,119,available,1",
        ]
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "samples.csv"
            path.write_text("\n".join([self.HEADER, *rows]) + "\n", encoding="utf-8")
            samples = summarize.read_samples(path)
        self.assertEqual(len(samples), 3)
        by_terminal = {s.terminal: s.task_clock_ms_per_second for s in samples}
        self.assertEqual(
            by_terminal, {"freminal": 20.0, "wezterm": 10.0, "ghostty": 5.0}
        )
        result = only(summarize.summarize(samples, expected_repeats=1))
        self.assertEqual(result["peer"], "wezterm")
        self.assertAlmostEqual(result["median_delta_ms_per_second"], 10.0)


if __name__ == "__main__":
    unittest.main()
