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

    def test_summarizes_worker_overlap_join_and_finish_phases(self) -> None:
        header = {
            "record": "header",
            "schema": SUMMARY.SCHEMA,
            "trace_id": "worker",
            "emulated_hz": 1_000_000_000,
        }
        data = [
            {
                "record": "audio_anchor",
                "generation": 1,
                "dma_id": 1,
                "emulated_cycle": 0,
                "predicted_playback_host_ns": 0,
            },
            {
                "record": "audio_anchor",
                "generation": 1,
                "dma_id": 2,
                "emulated_cycle": 1_000_000_000,
                "predicted_playback_host_ns": 1_000_000_000,
            },
            {
                "record": "vi_present",
                "stage": "source",
                "presentation_generation": 1,
                "retrace_cycle": 0,
                "swap_count": 0,
                "present_return_host_ns": 0,
            },
            {
                "record": "vi_present",
                "stage": "post_vi",
                "presentation_generation": 2,
                "retrace_cycle": 1_000_000_000,
                "swap_count": 1,
                "present_return_host_ns": 1_000_000_000,
            },
            {
                "record": "render_batch",
                "batch_id": 0,
                "members": 4,
                "execution_mode": "worker",
                "dispatch_cycle": 100,
                "completion_cycle": 200,
                "dispatch_host_ns": 10_000_000,
                "worker_start_host_ns": 11_000_000,
                "worker_finish_host_ns": 21_000_000,
                "join_cause": "vi_visibility",
                "join_request_host_ns": 19_000_000,
                "join_return_host_ns": 25_000_000,
                "staged_writes_ns": 1_000_000,
                "commit_ns": 2_000_000,
                "copyback_ns": 3_000_000,
                "publication_ns": 4_000_000,
            },
        ]
        renderer = SUMMARY.summarize(header, data, tolerance_ms=1)["renderer"]
        self.assertEqual(renderer["batches"], 1)
        self.assertEqual(renderer["members"], 4)
        self.assertEqual(renderer["join_causes"], {"vi_visibility": 1})
        self.assertEqual(renderer["worker_execute_ms"]["median"], 10)
        self.assertEqual(renderer["guest_overlap_before_join_ms"]["median"], 8)
        self.assertEqual(renderer["architectural_join_wait_ms"]["median"], 6)
        self.assertEqual(renderer["emulation_finish_phases_ms"]["median"], 10)
        self.assertTrue(renderer["performance_complete"])

    def test_reports_terminal_incomplete_batch_and_marks_performance_partial(self) -> None:
        renderer = SUMMARY._render_summary(
            [
                {
                    "record": "render_batch_incomplete",
                    "batch_id": 0,
                    "members": 3,
                    "dispatch_cycle": 20,
                    "dispatch_host_ns": 40,
                    "reason": "process_exit_before_completion",
                }
            ]
        )
        self.assertEqual(renderer["dispatched_batches"], 1)
        self.assertEqual(renderer["batches"], 0)
        self.assertEqual(renderer["incomplete_batches"], 1)
        self.assertEqual(
            renderer["incomplete_reasons"], {"process_exit_before_completion": 1}
        )
        self.assertFalse(renderer["performance_complete"])

    def test_exact_cue_pair_reports_direct_host_and_guest_phase(self) -> None:
        header = {
            "record": "header",
            "schema": SUMMARY.SCHEMA,
            "trace_id": "cue",
            "cue_id": "cue-1",
            "emulated_hz": 10,
        }
        audio = {
            "record": "av_cue_audio",
            "cue_id": "cue-1",
            "dma_id": 7,
            "guest_frame_offset": 3,
            "dma_start_cycle": 10,
            "start_dacrate": 1,
            "ai_clock_hz": 10,
            "predicted_playback_host_ns": 2_000_000,
            "landmark_generation": 4,
            "current_generation": 4,
            "valid": True,
            "invalid_reason": None,
        }
        video = {
            "record": "av_cue_video",
            "cue_id": "cue-1",
            "rgba_hash": "0000000000001234",
            "occurrence": 2,
            "stage": "post_vi",
            "presentation_generation": 9,
            "retrace_cycle": 18,
            "present_return_host_ns": 2_500_000,
        }
        pair = {
            "record": "av_cue_pair",
            "cue_id": "cue-1",
            "audio_dma_id": 7,
            "audio_guest_frame_offset": 3,
            "audio_generation": 4,
            "audio_cycle_numerator": 160,
            "video_hash": "0000000000001234",
            "video_occurrence": 2,
            "video_stage": "post_vi",
            "video_presentation_generation": 9,
            "video_retrace_cycle": 18,
            "cycle_denominator": 10,
            "video_minus_audio_guest_numerator": 20,
            "audio_predicted_playback_host_ns": 2_000_000,
            "video_present_return_host_ns": 2_500_000,
            "video_minus_audio_host_ns": 500_000,
        }
        result = SUMMARY._exact_cue_summary(header, [audio, video, pair])
        self.assertTrue(result["valid"])
        self.assertEqual(result["video_minus_audio_host_ms"], 0.5)
        self.assertEqual(result["video_minus_audio_guest_cycles"], 2.0)

        with self.assertRaisesRegex(ValueError, "host phase does not close"):
            SUMMARY._exact_cue_summary(
                header,
                [audio, video, {**pair, "video_minus_audio_host_ns": 499_999}],
            )
        with self.assertRaisesRegex(ValueError, "audio cycle does not close"):
            SUMMARY._exact_cue_summary(
                header,
                [audio, video, {**pair, "audio_cycle_numerator": 159}],
            )
        with self.assertRaisesRegex(ValueError, "guest phase does not close"):
            SUMMARY._exact_cue_summary(
                header,
                [audio, video, {**pair, "video_minus_audio_guest_numerator": 19}],
            )
        with self.assertRaisesRegex(ValueError, "audio host instant"):
            SUMMARY._exact_cue_summary(
                header,
                [audio, video, {**pair, "audio_predicted_playback_host_ns": 1}],
            )

    def test_exact_cue_fails_closed_on_continuity_or_missing_halves(self) -> None:
        header = {
            "record": "header",
            "schema": SUMMARY.SCHEMA,
            "trace_id": "cue",
            "cue_id": "cue-2",
            "emulated_hz": 1_000_000_000,
        }
        invalid_audio = {
            "record": "av_cue_audio",
            "cue_id": "cue-2",
            "dma_id": 8,
            "guest_frame_offset": 0,
            "dma_start_cycle": 100,
            "start_dacrate": 1_519,
            "ai_clock_hz": 48_681_812,
            "predicted_playback_host_ns": 200,
            "landmark_generation": 4,
            "current_generation": 5,
            "valid": False,
            "invalid_reason": "continuity_generation_changed",
        }
        video = {
            "record": "av_cue_video",
            "cue_id": "cue-2",
            "rgba_hash": "0000000000005678",
            "occurrence": 1,
            "stage": "post_vi",
            "presentation_generation": 10,
            "retrace_cycle": 400,
            "present_return_host_ns": 250,
        }
        result = SUMMARY._exact_cue_summary(header, [invalid_audio, video])
        self.assertFalse(result["valid"])
        self.assertEqual(result["reason"], "continuity_generation_changed")
        missing = SUMMARY._exact_cue_summary(header, [invalid_audio])
        self.assertFalse(missing["valid"])
        self.assertEqual(missing["reason"], "video")

    def test_rejects_malformed_render_batch_authority(self) -> None:
        valid = {
            "record": "render_batch",
            "batch_id": 0,
            "members": 1,
            "execution_mode": "worker",
            "dispatch_cycle": 10,
            "completion_cycle": 20,
            "dispatch_host_ns": 100,
            "worker_start_host_ns": 110,
            "worker_finish_host_ns": 140,
            "join_cause": "vi_visibility",
            "join_request_host_ns": 130,
            "join_return_host_ns": 150,
            "staged_writes_ns": 1,
            "commit_ns": 2,
            "copyback_ns": 3,
            "publication_ns": 4,
        }
        mutations = (
            ({**valid, "members": 0}, "members must be positive"),
            ({**valid, "completion_cycle": 9}, "completed before"),
            ({**valid, "worker_start_host_ns": 99}, "started before batch dispatch"),
            ({**valid, "join_request_host_ns": None}, "present together"),
            ({**valid, "join_return_host_ns": 139}, "before worker completion"),
        )
        for mutation, message in mutations:
            with self.subTest(message=message), self.assertRaisesRegex(ValueError, message):
                SUMMARY._render_summary([mutation])

        with self.assertRaisesRegex(ValueError, "unique, contiguous, and monotonic"):
            SUMMARY._render_summary(
                [
                    valid,
                    {
                        "record": "render_batch_incomplete",
                        "batch_id": 0,
                        "members": 1,
                        "dispatch_cycle": 21,
                        "dispatch_host_ns": 151,
                        "reason": "process_exit_before_completion",
                    },
                ]
            )
        with self.assertRaisesRegex(ValueError, "unique, contiguous, and monotonic"):
            SUMMARY._render_summary([{**valid, "batch_id": 1}])
        with self.assertRaisesRegex(ValueError, "reason is invalid"):
            SUMMARY._render_summary(
                [
                    {
                        "record": "render_batch_incomplete",
                        "batch_id": 0,
                        "members": 1,
                        "dispatch_cycle": 21,
                        "dispatch_host_ns": 151,
                        "reason": "unknown",
                    }
                ]
            )

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

        for legacy_schema in (
            "fn64.host-presentation.v1",
            "fn64.host-presentation.v2",
            "fn64.host-presentation.v3",
        ):
            with self.subTest(schema=legacy_schema), tempfile.TemporaryDirectory() as directory:
                path = Path(directory) / "legacy.jsonl"
                path.write_text(
                    json.dumps({**header, "schema": legacy_schema})
                    + "\n"
                    + json.dumps({"record": "end", "data_records": 0})
                    + "\n",
                    encoding="utf-8",
                )
                with self.assertRaisesRegex(ValueError, SUMMARY.SCHEMA):
                    SUMMARY.load_trace(path)


if __name__ == "__main__":
    unittest.main()
