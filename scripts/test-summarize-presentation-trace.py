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


def presentation_span_records(
    stage: str,
    generation: int,
    retrace_cycle: int,
    present_return_host_ns: int,
) -> list[dict[str, object]]:
    source_generation = generation if stage == "source" else max(1, generation - 1)
    post_vi_generation = generation if stage == "post_vi" else generation + 1
    return [
        {
            "record": "vi_scanout_span",
            "retrace_cycle": retrace_cycle,
            "source_generation": source_generation,
            "source_ready": stage == "source",
            "post_vi_generation": post_vi_generation,
            "post_vi_ready": stage == "post_vi",
            "start_host_ns": present_return_host_ns - 3,
            "finish_host_ns": present_return_host_ns - 2,
        },
        {
            "record": "window_present_span",
            "stage": stage,
            "presentation_generation": generation,
            "retrace_cycle": retrace_cycle,
            "outcome": "success",
            "start_host_ns": present_return_host_ns - 1,
            "finish_host_ns": present_return_host_ns,
        },
    ]


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
            *presentation_span_records("source", 8, 90, 90),
            {
                "record": "vi_present",
                "stage": "post_vi",
                "presentation_generation": 9,
                "retrace_cycle": 200,
                "swap_count": 11,
                "present_return_host_ns": 200,
            },
            *presentation_span_records("post_vi", 9, 200, 200),
            {"record": "end", "data_records": 7},
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
            *presentation_span_records("post_vi", 1, 0, 50_000_000),
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
            *presentation_span_records(
                "post_vi", 2, 1_000_000_000, 1_049_000_000
            ),
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
            *presentation_span_records("source", 1, 0, 0),
            {
                "record": "vi_present",
                "stage": "post_vi",
                "presentation_generation": 2,
                "retrace_cycle": 1_000_000_000,
                "swap_count": 1,
                "present_return_host_ns": 1_000_000_000,
            },
            *presentation_span_records(
                "post_vi", 2, 1_000_000_000, 1_000_000_000
            ),
            {
                "record": "render_batch",
                "batch_id": 0,
                "queue_kind": "raw_dpc_task_batch",
                "queue_id": 0,
                "members": 4,
                "cpu_dispatch_lane": "canonical_block_program",
                "rsp_dispatch_lane": "interpreted",
                "rdp_lane": "mixed",
                "rdp_cpu_members": 1,
                "rdp_compute_members": 3,
                "host_thread": "rdp_worker",
                "execution_mode": "worker",
                "dispatch_cycle": 100,
                "completion_cycle": 200,
                "dispatch_host_ns": 10_000_000,
                "completion_host_ns": 21_000_000,
                "worker_start_host_ns": 11_000_000,
                "worker_finish_host_ns": 21_000_000,
                "worker_thread_cpu_ns": 7_000_000,
                "join_cause": "vi_visibility",
                "coherence_reason": "vi_visibility",
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
        self.assertEqual(renderer["worker_cpu_observed_batches"], 1)
        self.assertEqual(renderer["worker_cpu_unavailable_batches"], 0)
        self.assertEqual(renderer["worker_thread_cpu_ms"]["median"], 7)
        self.assertEqual(renderer["worker_non_cpu_wall_ms"]["median"], 3)
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

    def test_audio_stream_start_closes_payload_play_and_callback_order(self) -> None:
        record = {
            "record": "audio_stream_start",
            "dma_id": 1,
            "payload_queued_host_ns": 10,
            "dma_started_cycle": 100,
            "play_returned_host_ns": 20,
            "first_callback_host_ns": 30,
        }
        summary = SUMMARY._audio_stream_start_summary([record])
        self.assertTrue(summary["complete"])
        self.assertEqual(summary["dma_started_cycle"], 100)
        self.assertEqual(summary["payload_to_play_ms"], 0.00001)
        self.assertEqual(summary["first_callback_minus_play_return_ms"], 0.00001)
        self.assertFalse(SUMMARY._audio_stream_start_summary([])["complete"])

        with self.assertRaisesRegex(ValueError, "play returned before"):
            SUMMARY._audio_stream_start_summary(
                [{**record, "play_returned_host_ns": 9}]
            )
        raced = SUMMARY._audio_stream_start_summary(
            [{**record, "first_callback_host_ns": 19}]
        )
        self.assertAlmostEqual(
            raced["first_callback_minus_play_return_ms"], -0.000001
        )
        with self.assertRaisesRegex(ValueError, "callback preceded"):
            SUMMARY._audio_stream_start_summary(
                [{**record, "first_callback_host_ns": 9}]
            )
        with self.assertRaisesRegex(ValueError, "must be unique"):
            SUMMARY._audio_stream_start_summary([record, record])

    def test_audio_underruns_close_slots_sequences_and_telemetry_loss(self) -> None:
        records = [
            {
                "record": "audio_underrun",
                "sequence": 1,
                "callback_host_ns": -10,
                "reason": "ring_empty",
                "requested_sample_slots": 8,
                "delivered_sample_slots": 0,
                "underrun_sample_slots": 8,
                "ring_sample_slots_before": 0,
                "active_phase": "waiting",
            },
            {
                "record": "audio_underrun",
                "sequence": 2,
                "callback_host_ns": 20,
                "reason": "ring_short",
                "requested_sample_slots": 8,
                "delivered_sample_slots": 3,
                "underrun_sample_slots": 5,
                "ring_sample_slots_before": 3,
                "active_phase": "vi_scanout",
            },
            {
                "record": "audio_underrun",
                "sequence": 4,
                "callback_host_ns": 30,
                "reason": "producer_contention",
                "requested_sample_slots": 8,
                "delivered_sample_slots": 0,
                "underrun_sample_slots": 8,
                "ring_sample_slots_before": None,
                "active_phase": "window_present",
            },
            {
                "record": "telemetry_loss",
                "source": "audio_underrun",
                "dropped_observations": 2,
            },
        ]
        loss = SUMMARY._telemetry_loss_summary(records)
        summary = SUMMARY._audio_underrun_summary(records, loss)
        self.assertFalse(loss["complete"])
        self.assertEqual(summary["events"], 3)
        self.assertEqual(summary["dropped_observations"], 2)
        self.assertEqual(summary["missing_sequence_ids"], 1)
        self.assertEqual(summary["unlocated_or_tail_dropped_observations"], 1)
        self.assertEqual(summary["requested_sample_slots"], 24)
        self.assertEqual(summary["delivered_sample_slots"], 3)
        self.assertEqual(summary["underrun_sample_slots"], 21)
        self.assertEqual(summary["ring_depth_observed_events"], 2)
        self.assertEqual(
            summary["reasons"],
            {"ring_empty": 1, "ring_short": 1, "producer_contention": 1},
        )
        self.assertEqual(summary["first_sequence"], 1)
        self.assertEqual(summary["last_sequence"], 4)

        with self.assertRaisesRegex(ValueError, "sequence gaps exceed"):
            SUMMARY._audio_underrun_summary(
                records[:-1],
                {"complete": True, "audio_underrun_dropped_observations": 0},
            )
        with self.assertRaisesRegex(ValueError, "must be positive"):
            SUMMARY._audio_underrun_summary(
                [{**records[0], "sequence": 0}],
                {"complete": True, "audio_underrun_dropped_observations": 0},
            )
        with self.assertRaisesRegex(ValueError, "unique and monotonic"):
            SUMMARY._audio_underrun_summary(
                [records[1], records[0]],
                {"complete": True, "audio_underrun_dropped_observations": 0},
            )
        overlapping = SUMMARY._audio_underrun_summary(
            [records[0], {**records[1], "callback_host_ns": -11}],
            {"complete": True, "audio_underrun_dropped_observations": 0},
        )
        self.assertEqual(overlapping["events"], 2)
        with self.assertRaisesRegex(ValueError, "sample-slot accounting"):
            SUMMARY._audio_underrun_summary(
                [{**records[0], "underrun_sample_slots": 7}],
                {"complete": True, "audio_underrun_dropped_observations": 0},
            )
        with self.assertRaisesRegex(ValueError, "ring_short underrun"):
            SUMMARY._audio_underrun_summary(
                [{**records[1], "sequence": 1, "ring_sample_slots_before": None}],
                {"complete": True, "audio_underrun_dropped_observations": 0},
            )
        with self.assertRaisesRegex(ValueError, "producer_contention underrun"):
            SUMMARY._audio_underrun_summary(
                [{**records[2], "sequence": 1, "ring_sample_slots_before": 0}],
                {"complete": True, "audio_underrun_dropped_observations": 0},
            )
        with self.assertRaisesRegex(ValueError, "active_phase is invalid"):
            SUMMARY._audio_underrun_summary(
                [{**records[0], "active_phase": "unknown"}],
                {"complete": True, "audio_underrun_dropped_observations": 0},
            )

    def test_telemetry_loss_is_typed_and_positive(self) -> None:
        self.assertTrue(SUMMARY._telemetry_loss_summary([])["complete"])
        with self.assertRaisesRegex(ValueError, "source must be audio_underrun"):
            SUMMARY._telemetry_loss_summary(
                [{"record": "telemetry_loss", "source": "other", "dropped_observations": 1}]
            )
        with self.assertRaisesRegex(ValueError, "must be positive"):
            SUMMARY._telemetry_loss_summary(
                [{"record": "telemetry_loss", "source": "audio_underrun", "dropped_observations": 0}]
            )

    def test_presentation_spans_join_ready_scanout_to_successful_window(self) -> None:
        ready = {
            "record": "vi_scanout_span",
            "retrace_cycle": 100,
            "source_generation": 3,
            "source_ready": False,
            "post_vi_generation": 4,
            "post_vi_ready": True,
            "start_host_ns": -5,
            "finish_host_ns": 995,
        }
        unavailable = {
            "record": "vi_scanout_span",
            "retrace_cycle": 200,
            "source_generation": 5,
            "source_ready": False,
            "post_vi_generation": 6,
            "post_vi_ready": False,
            "start_host_ns": 1_000,
            "finish_host_ns": 1_500,
        }
        success = {
            "record": "window_present_span",
            "stage": "post_vi",
            "presentation_generation": 4,
            "retrace_cycle": 100,
            "outcome": "success",
            "start_host_ns": 2_000,
            "finish_host_ns": 5_000,
        }
        failed = {
            "record": "window_present_span",
            "stage": None,
            "presentation_generation": None,
            "retrace_cycle": None,
            "outcome": "render_failed",
            "start_host_ns": 6_000,
            "finish_host_ns": 10_000,
        }
        presented = {
            "record": "vi_present",
            "stage": "post_vi",
            "presentation_generation": 4,
            "retrace_cycle": 100,
            "swap_count": 1,
            "present_return_host_ns": 5_000,
        }
        summary = SUMMARY._presentation_span_summary(
            [ready, unavailable, success, failed, presented]
        )
        self.assertEqual(summary["ready_vi_scanouts"], 1)
        self.assertEqual(summary["unavailable_vi_scanouts"], 1)
        self.assertEqual(summary["joined_presentations"], 1)
        self.assertEqual(summary["ready_vi_scanout_ms"]["median"], 0.001)
        self.assertEqual(summary["successful_window_present_ms"]["median"], 0.003)
        self.assertEqual(summary["failed_window_present_ms"]["median"], 0.004)

        suppressed = SUMMARY._presentation_span_summary([ready])
        self.assertEqual(suppressed["unsubmitted_ready_vi_scanouts"], 1)
        with self.assertRaisesRegex(ValueError, "no ready VI scanout"):
            SUMMARY._presentation_span_summary([success, presented])
        with self.assertRaisesRegex(ValueError, "no exact vi_present return"):
            SUMMARY._presentation_span_summary([ready, success])
        with self.assertRaisesRegex(ValueError, "no successful matching"):
            SUMMARY._presentation_span_summary([presented])
        with self.assertRaisesRegex(ValueError, "before VI scanout finished"):
            SUMMARY._presentation_span_summary(
                [ready, {**success, "start_host_ns": 994}, presented]
            )
        with self.assertRaisesRegex(ValueError, "no exact vi_present return"):
            SUMMARY._presentation_span_summary(
                [ready, success, {**presented, "present_return_host_ns": 4_999}]
            )
        with self.assertRaisesRegex(ValueError, "wholly present or null"):
            SUMMARY._presentation_span_summary([{**failed, "stage": "post_vi"}])
        with self.assertRaisesRegex(ValueError, "must be a boolean"):
            SUMMARY._presentation_span_summary([{**unavailable, "source_ready": 0}])
        with self.assertRaisesRegex(ValueError, "finished before"):
            SUMMARY._presentation_span_summary(
                [{**unavailable, "finish_host_ns": 999}]
            )
        with self.assertRaisesRegex(ValueError, "outcome is invalid"):
            SUMMARY._presentation_span_summary([{**failed, "outcome": "skipped"}])

    def test_v8_summary_exposes_underrun_spans_and_loss(self) -> None:
        header = {
            "record": "header",
            "schema": SUMMARY.SCHEMA,
            "trace_id": "v8",
            "emulated_hz": 1_000_000_000,
        }
        data = [
            {
                "record": "audio_anchor",
                "generation": 1,
                "dma_id": 1,
                "emulated_cycle": 100,
                "predicted_playback_host_ns": 100,
            },
            {
                "record": "vi_present",
                "stage": "post_vi",
                "presentation_generation": 2,
                "retrace_cycle": 100,
                "swap_count": 1,
                "present_return_host_ns": 100,
            },
            {
                "record": "audio_underrun",
                "sequence": 1,
                "callback_host_ns": 110,
                "reason": "ring_empty",
                "requested_sample_slots": 4,
                "delivered_sample_slots": 0,
                "underrun_sample_slots": 4,
                "ring_sample_slots_before": 0,
                "active_phase": "device_advance",
            },
            {
                "record": "vi_scanout_span",
                "retrace_cycle": 100,
                "source_generation": 1,
                "source_ready": False,
                "post_vi_generation": 2,
                "post_vi_ready": True,
                "start_host_ns": 80,
                "finish_host_ns": 90,
            },
            {
                "record": "window_present_span",
                "stage": "post_vi",
                "presentation_generation": 2,
                "retrace_cycle": 100,
                "outcome": "success",
                "start_host_ns": 90,
                "finish_host_ns": 100,
            },
        ]
        summary = SUMMARY.summarize(header, data, tolerance_ms=1)
        self.assertEqual(summary["schema"], "fn64.host-presentation.v8")
        self.assertEqual(summary["audio_underruns"]["events"], 1)
        self.assertTrue(summary["audio_underruns"]["telemetry_complete"])
        self.assertEqual(summary["presentation_spans"]["joined_presentations"], 1)
        self.assertTrue(summary["telemetry_loss"]["complete"])

    def test_rejects_malformed_render_batch_authority(self) -> None:
        valid = {
            "record": "render_batch",
            "batch_id": 0,
            "queue_kind": "raw_dpc_task_batch",
            "queue_id": 0,
            "members": 1,
            "cpu_dispatch_lane": "canonical_block_program",
            "rsp_dispatch_lane": "interpreted",
            "rdp_lane": "cpu",
            "rdp_cpu_members": 1,
            "rdp_compute_members": 0,
            "host_thread": "rdp_worker",
            "execution_mode": "worker",
            "dispatch_cycle": 10,
            "completion_cycle": 20,
            "dispatch_host_ns": 100,
            "completion_host_ns": 140,
            "worker_start_host_ns": 110,
            "worker_finish_host_ns": 140,
            "worker_thread_cpu_ns": 20,
            "join_cause": "vi_visibility",
            "coherence_reason": "vi_visibility",
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
            ({key: value for key, value in valid.items() if key != "worker_thread_cpu_ns"}, "must be present"),
            ({**valid, "worker_thread_cpu_ns": -1}, "must be nonnegative"),
            ({**valid, "worker_thread_cpu_ns": True}, "must be an integer"),
            ({**valid, "worker_thread_cpu_ns": 31}, "CPU time exceeds"),
            ({**valid, "join_request_host_ns": None}, "present together"),
            ({**valid, "join_return_host_ns": 139}, "before worker completion"),
        )
        for mutation, message in mutations:
            with self.subTest(message=message), self.assertRaisesRegex(ValueError, message):
                SUMMARY._render_summary([mutation])

        unavailable = {**valid, "worker_thread_cpu_ns": None}
        unavailable_summary = SUMMARY._render_summary([unavailable])
        self.assertEqual(unavailable_summary["worker_cpu_observed_batches"], 0)
        self.assertEqual(unavailable_summary["worker_cpu_unavailable_batches"], 1)
        self.assertIsNone(unavailable_summary["worker_thread_cpu_ms"])
        with self.assertRaisesRegex(ValueError, "availability changed"):
            SUMMARY._render_summary(
                [valid, {**unavailable, "batch_id": 1, "queue_id": 1}]
            )

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
            "fn64.host-presentation.v4",
            "fn64.host-presentation.v5",
            "fn64.host-presentation.v6",
            "fn64.host-presentation.v7",
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

    def test_guest_tasks_join_exact_batch_mechanism_and_reject_aliases(self) -> None:
        batch = {
            "record": "render_batch",
            "batch_id": 0,
            "rdp_lane": "mixed",
            "rdp_cpu_members": 1,
            "rdp_compute_members": 2,
            "host_thread": "rdp_worker",
            "coherence_reason": "vi_visibility",
        }
        task = {
            "record": "guest_task",
            "task_offset": 0x140,
            "admission_generation": 8,
            "resumed_from_admission_generation": 7,
            "kind": "graphics",
            "outcome": "completed",
            "cpu_dispatch_lane": "canonical_block_program",
            "dispatch_thread_kind": "executor",
            "dispatch_thread_id": 3,
            "rsp_dispatch_lane": "interpreted",
            "rdp_lane": "mixed",
            "rdp_cpu_members": 1,
            "rdp_compute_members": 2,
            "queue_kind": "raw_dpc_task_batch",
            "queue_id": 0,
            "host_thread": "rdp_worker",
            "coherence_reason": "vi_visibility",
            "dispatch_cycle": 10,
            "completion_cycle": 20,
            "dispatch_host_ns": 100,
            "completion_host_ns": 200,
        }
        summary = SUMMARY._guest_task_summary([batch, task])
        self.assertEqual(summary["tasks"], 1)
        self.assertEqual(summary["rsp_lanes"], {"interpreted": 1})
        self.assertEqual(summary["rdp_lanes"], {"mixed": 1})

        with self.assertRaisesRegex(ValueError, "actual batch evidence"):
            SUMMARY._guest_task_summary([batch, {**task, "rdp_cpu_members": 2}])
        with self.assertRaisesRegex(ValueError, "key must be unique"):
            SUMMARY._guest_task_summary([batch, task, task])
        with self.assertRaisesRegex(ValueError, "batch identity is ambiguous"):
            SUMMARY._guest_task_summary([batch, {**batch, "record": "render_batch_incomplete"}])
        with self.assertRaisesRegex(ValueError, "only audio"):
            SUMMARY._guest_task_summary(
                [
                    {
                        **task,
                        "queue_kind": "not_applicable",
                        "queue_id": None,
                        "host_thread": "emulation",
                        "coherence_reason": None,
                        "rdp_lane": "not_applicable",
                        "rdp_cpu_members": None,
                        "rdp_compute_members": None,
                    }
                ]
            )


if __name__ == "__main__":
    unittest.main()
