#!/usr/bin/env python3
"""ROM-free tests for summarize-presentation-trace.py."""

from __future__ import annotations

import importlib.util
import json
import sys
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).resolve().with_name("summarize-presentation-trace.py")
SPEC = importlib.util.spec_from_file_location("summarize_presentation_trace", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
SUMMARY = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = SUMMARY
SPEC.loader.exec_module(SUMMARY)


class PresentationTraceSummaryTests(unittest.TestCase):
    def test_reports_first_exact_field_with_phase_residual(self) -> None:
        records = [
            {
                "record": "header",
                "schema": SUMMARY.SCHEMA,
                "trace_id": "synthetic",
                "emulated_hz": 1_000_000_000,
            },
            {
                "record": "audio_anchor",
                "generation": 2,
                "dma_id": 7,
                "emulated_cycle": 100,
                "predicted_playback_host_ns": 150,
            },
            {
                "record": "vi_present",
                "stage": "source",
                "presentation_generation": 8,
                "retrace_cycle": 90,
                "swap_count": 10,
                "present_return_host_ns": 90,
            },
            {
                "record": "vi_present",
                "stage": "post_vi",
                "presentation_generation": 9,
                "retrace_cycle": 200,
                "swap_count": 11,
                "present_return_host_ns": 200,
            },
            {"record": "end", "data_records": 3},
        ]
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "trace.jsonl"
            path.write_text(
                "".join(json.dumps(record) + "\n" for record in records),
                encoding="utf-8",
            )
            header, data = SUMMARY.load_trace(path)
        result = SUMMARY.summarize(header, data, tolerance_ms=0.00004)
        self.assertEqual(result["vi_before_first_audio_anchor"], 1)
        self.assertEqual(result["video_minus_audio_ms"]["median"], -0.00005)
        self.assertEqual(result["first_outside_tolerance"]["retrace_cycle"], 200)
        self.assertEqual(result["first_outside_tolerance"]["audio_dma_id"], 7)
        self.assertEqual(result["first_outside_tolerance"]["stage"], "post_vi")
        self.assertEqual(
            result["first_outside_tolerance"]["presentation_generation"], 9
        )

    def test_rejects_unsealed_and_miscounted_traces(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "trace.jsonl"
            path.write_text(
                json.dumps(
                    {
                        "record": "header",
                        "schema": SUMMARY.SCHEMA,
                        "emulated_hz": 93_750_000,
                    }
                )
                + "\n"
                + json.dumps({"record": "end", "data_records": 1})
                + "\n",
                encoding="utf-8",
            )
            with self.assertRaisesRegex(ValueError, "data_records"):
                SUMMARY.load_trace(path)

    def test_separates_fixed_phase_from_relative_pace(self) -> None:
        header = {
            "record": "header",
            "schema": SUMMARY.SCHEMA,
            "trace_id": "pace",
            "emulated_hz": 1_000_000_000,
        }
        data = [
            {
                "record": "audio_anchor",
                "generation": 1,
                "dma_id": 1,
                "emulated_cycle": 0,
                "predicted_playback_host_ns": 100_000_000,
            },
            {
                "record": "vi_present",
                "stage": "post_vi",
                "presentation_generation": 1,
                "retrace_cycle": 0,
                "swap_count": 0,
                "present_return_host_ns": 50_000_000,
            },
            {
                "record": "audio_anchor",
                "generation": 1,
                "dma_id": 2,
                "emulated_cycle": 1_000_000_000,
                "predicted_playback_host_ns": 1_100_000_000,
            },
            {
                "record": "vi_present",
                "stage": "post_vi",
                "presentation_generation": 2,
                "retrace_cycle": 1_000_000_000,
                "swap_count": 1,
                "present_return_host_ns": 1_049_000_000,
            },
        ]
        result = SUMMARY.summarize(header, data, tolerance_ms=5)
        pace = result["relative_pace"]
        self.assertIsNotNone(pace)
        self.assertAlmostEqual(pace["video_vs_audio_rate_ppm"], -1_000)
        self.assertAlmostEqual(pace["video_minus_audio_drift_ms_per_minute"], -60)
        self.assertEqual(pace["audio_samples"], 2)
        self.assertEqual(pace["video_samples"], 2)

    def test_rejects_unlabeled_or_legacy_stage_evidence(self) -> None:
        header = {
            "record": "header",
            "schema": SUMMARY.SCHEMA,
            "trace_id": "stage-gate",
            "emulated_hz": 1_000_000_000,
        }
        audio = {
            "record": "audio_anchor",
            "generation": 1,
            "dma_id": 1,
            "emulated_cycle": 0,
            "predicted_playback_host_ns": 0,
        }
        legacy_video = {
            "record": "vi_present",
            "source_generation": 1,
            "retrace_cycle": 0,
            "swap_count": 0,
            "present_return_host_ns": 0,
        }
        with self.assertRaisesRegex(ValueError, "vi_present.stage"):
            SUMMARY.summarize(header, [audio, legacy_video], tolerance_ms=0)

        mislabeled_video = {
            **legacy_video,
            "stage": "filtered_maybe",
            "presentation_generation": 1,
        }
        with self.assertRaisesRegex(ValueError, "source or post_vi"):
            SUMMARY.summarize(header, [audio, mislabeled_video], tolerance_ms=0)

        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "legacy.jsonl"
            path.write_text(
                json.dumps({**header, "schema": "fn64.host-presentation.v1"})
                + "\n"
                + json.dumps({"record": "end", "data_records": 0})
                + "\n",
                encoding="utf-8",
            )
            with self.assertRaisesRegex(ValueError, SUMMARY.SCHEMA):
                SUMMARY.load_trace(path)


if __name__ == "__main__":
    unittest.main()
