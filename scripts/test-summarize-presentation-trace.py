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
                "source_generation": 8,
                "retrace_cycle": 90,
                "swap_count": 10,
                "present_return_host_ns": 90,
            },
            {
                "record": "vi_present",
                "source_generation": 9,
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


if __name__ == "__main__":
    unittest.main()
