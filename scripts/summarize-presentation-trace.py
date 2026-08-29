#!/usr/bin/env python3
"""Summarize fn64's joined host A/V presentation trace.

The reported API-boundary residual compares two mappings of typed emulated time
onto one host epoch. A negative value means the VI present call returned earlier
than the audio anchor predicts playback for the same emulated cycle. It is not a
measurement of display scanout, acoustic output latency, or content A/V phase.

The whole-trace relative rate is diagnostic. A pace verdict requires four
content-neutral equal partitions of the observed common emulated interval,
enough samples to fit each lane, agreement after any first-partition debt
recovery, one audio continuity generation, complete diagnostic transport,
and at least sixty common emulated seconds. Sampled-away audio-DMA anchors and
ready-but-unsubmitted VI fields remain explicit coverage facts, not transport
loss and not automatic refusal reasons.
"""

from __future__ import annotations

import argparse
import bisect
import json
import statistics
from pathlib import Path
from typing import Any


SCHEMA = "fn64.host-presentation.v8"
PRESENTATION_STAGES = frozenset({"source", "post_vi"})
AUDIO_UNDERRUN_REASONS = frozenset(
    {"ring_empty", "ring_short", "producer_contention"}
)
ACTIVE_PHASES = frozenset(
    {"waiting", "guest_step", "device_advance", "vi_scanout", "window_present"}
)
PACE_WINDOW_COUNT = 4
PACE_MINIMUM_SAMPLES_PER_LANE_WINDOW = 3
PACE_MINIMUM_COMMON_SECONDS = 60
PACE_CONFIDENCE_Z = 1.96


def _integer(record: dict[str, Any], key: str) -> int:
    value = record.get(key)
    if not isinstance(value, int) or isinstance(value, bool):
        raise ValueError(f"{record.get('record', 'record')}.{key} must be an integer")
    return value


def _nonnegative_integer(record: dict[str, Any], key: str) -> int:
    value = _integer(record, key)
    if value < 0:
        raise ValueError(f"{record.get('record', 'record')}.{key} must be nonnegative")
    return value


def _nullable_nonnegative_integer(record: dict[str, Any], key: str) -> int | None:
    if key not in record:
        raise ValueError(f"{record.get('record', 'record')}.{key} must be present")
    value = record.get(key)
    if value is None:
        return None
    return _nonnegative_integer(record, key)


def _boolean(record: dict[str, Any], key: str) -> bool:
    value = record.get(key)
    if not isinstance(value, bool):
        raise ValueError(f"{record.get('record', 'record')}.{key} must be a boolean")
    return value


def _presentation_stage(record: dict[str, Any]) -> str:
    value = record.get("stage")
    if value not in PRESENTATION_STAGES:
        raise ValueError(
            f"{record.get('record', 'record')}.stage must be source or post_vi"
        )
    return value


def _hash64(record: dict[str, Any], key: str) -> str:
    value = record.get(key)
    if not isinstance(value, str) or len(value) != 16 or any(
        character not in "0123456789abcdef" for character in value
    ):
        raise ValueError(
            f"{record.get('record', 'record')}.{key} must be 16 lowercase hex digits"
        )
    return value


def load_trace(path: Path) -> tuple[dict[str, Any], list[dict[str, Any]]]:
    records = []
    with path.open(encoding="utf-8") as stream:
        for line_number, line in enumerate(stream, 1):
            try:
                record = json.loads(line)
            except json.JSONDecodeError as error:
                raise ValueError(f"line {line_number}: {error}") from error
            if not isinstance(record, dict):
                raise ValueError(f"line {line_number}: record must be an object")
            records.append(record)
    if len(records) < 2:
        raise ValueError("trace must contain header and end records")
    header, end = records[0], records[-1]
    if header.get("record") != "header" or header.get("schema") != SCHEMA:
        raise ValueError(f"expected {SCHEMA} header")
    if end.get("record") != "end":
        raise ValueError("trace is not sealed with an end record")
    data = records[1:-1]
    if _integer(end, "data_records") != len(data):
        raise ValueError("end.data_records does not match the sealed trace")
    if _integer(header, "emulated_hz") <= 0:
        raise ValueError("header.emulated_hz must be positive")
    return header, data


def _nearest_anchor(
    anchors: list[dict[str, Any]], cycles: list[int], cycle: int
) -> dict[str, Any]:
    index = bisect.bisect_left(cycles, cycle)
    candidates = anchors[max(0, index - 1) : min(len(anchors), index + 1)]
    return min(candidates, key=lambda anchor: abs(_integer(anchor, "emulated_cycle") - cycle))


def _percentile(values: list[float], percentile: float) -> float:
    ordered = sorted(values)
    index = max(0, min(len(ordered) - 1, int(len(ordered) * percentile + 0.999999) - 1))
    return ordered[index]


def _duration_summary_ms(values_ns: list[int]) -> dict[str, float] | None:
    if not values_ns:
        return None
    values_ms = [value / 1_000_000 for value in values_ns]
    return {
        "median": statistics.median(values_ms),
        "p95": _percentile(values_ms, 0.95),
        "maximum": max(values_ms),
    }


def _render_summary(data: list[dict[str, Any]]) -> dict[str, Any]:
    batches = [record for record in data if record.get("record") == "render_batch"]
    incomplete = [
        record for record in data if record.get("record") == "render_batch_incomplete"
    ]
    dispatched = [
        record
        for record in data
        if record.get("record") in {"render_batch", "render_batch_incomplete"}
    ]
    for expected_id, batch in enumerate(dispatched):
        batch_id = _nonnegative_integer(batch, "batch_id")
        if batch_id != expected_id:
            raise ValueError(
                "renderer batch IDs must be unique, contiguous, and monotonic from zero"
            )
    worker = [record for record in batches if record.get("execution_mode") == "worker"]
    local = [record for record in batches if record.get("execution_mode") == "local"]
    if len(worker) + len(local) != len(batches):
        raise ValueError("render_batch.execution_mode must be worker or local")
    join_causes: dict[str, int] = {}
    worker_ns = []
    worker_thread_cpu_ns = []
    worker_non_cpu_wall_ns = []
    join_wait_ns = []
    guest_overlap_ns = []
    cpu_finish_ns = []
    incomplete_reasons: dict[str, int] = {}
    for batch in incomplete:
        if _nonnegative_integer(batch, "members") == 0:
            raise ValueError("render_batch_incomplete.members must be positive")
        _nonnegative_integer(batch, "dispatch_cycle")
        _nonnegative_integer(batch, "dispatch_host_ns")
        reason = batch.get("reason")
        if reason != "process_exit_before_completion":
            raise ValueError("render_batch_incomplete.reason is invalid")
        incomplete_reasons[reason] = incomplete_reasons.get(reason, 0) + 1
    for batch in batches:
        if "worker_thread_cpu_ns" not in batch:
            raise ValueError("render_batch.worker_thread_cpu_ns must be present")
        batch_id = _nonnegative_integer(batch, "batch_id")
        member_count = _nonnegative_integer(batch, "members")
        if member_count == 0:
            raise ValueError("render_batch.members must be positive")
        if batch.get("queue_kind") != "raw_dpc_task_batch":
            raise ValueError("render_batch.queue_kind is invalid")
        if _nonnegative_integer(batch, "queue_id") != batch_id:
            raise ValueError("render_batch.queue_id must equal batch_id")
        if batch.get("cpu_dispatch_lane") not in {
            "canonical_block_program",
            "abi_function_unattributed",
        }:
            raise ValueError("render_batch.cpu_dispatch_lane is invalid")
        if batch.get("rsp_dispatch_lane") != "interpreted":
            raise ValueError("render_batch.rsp_dispatch_lane is invalid")
        rdp_lane = batch.get("rdp_lane")
        if rdp_lane not in {"cpu", "compute", "mixed", "unavailable"}:
            raise ValueError("render_batch.rdp_lane is invalid")
        cpu_members = batch.get("rdp_cpu_members")
        compute_members = batch.get("rdp_compute_members")
        if rdp_lane == "unavailable":
            if cpu_members is not None or compute_members is not None:
                raise ValueError("unavailable RDP lane must not claim member counts")
        else:
            cpu_members = _nonnegative_integer(batch, "rdp_cpu_members")
            compute_members = _nonnegative_integer(batch, "rdp_compute_members")
            if cpu_members + compute_members != member_count:
                raise ValueError("RDP mechanism counts must equal batch members")
            expected_lane = (
                "compute" if cpu_members == 0 else "cpu" if compute_members == 0 else "mixed"
            )
            if rdp_lane != expected_lane:
                raise ValueError("RDP lane disagrees with mechanism counts")
        expected_thread = "rdp_worker" if batch in worker else "emulation"
        if batch.get("host_thread") != expected_thread:
            raise ValueError("render_batch.host_thread disagrees with execution_mode")
        cause = batch.get("join_cause")
        if cause is not None:
            if cause not in {
                "vi_visibility",
                "later_graphics",
                "dmem_dependency",
                "later_graphics_and_dmem_dependency",
            }:
                raise ValueError("render_batch.join_cause is invalid")
            join_causes[cause] = join_causes.get(cause, 0) + 1
        if batch.get("coherence_reason") != cause:
            raise ValueError("render_batch.coherence_reason must equal join_cause")
        dispatch_cycle = _nonnegative_integer(batch, "dispatch_cycle")
        completion_cycle = _nonnegative_integer(batch, "completion_cycle")
        if completion_cycle < dispatch_cycle:
            raise ValueError("render batch completed before it was dispatched")
        dispatch_host = _nonnegative_integer(batch, "dispatch_host_ns")
        completion_host = _nonnegative_integer(batch, "completion_host_ns")
        if completion_host < dispatch_host:
            raise ValueError("render batch host completion preceded dispatch")
        for key in (
            "staged_writes_ns",
            "commit_ns",
            "copyback_ns",
            "publication_ns",
        ):
            _nonnegative_integer(batch, key)
        cpu_finish_ns.append(
            sum(
                _integer(batch, key)
                for key in (
                    "staged_writes_ns",
                    "commit_ns",
                    "copyback_ns",
                    "publication_ns",
                )
            )
        )
        if batch in worker:
            start = _nonnegative_integer(batch, "worker_start_host_ns")
            finish = _nonnegative_integer(batch, "worker_finish_host_ns")
            if start < dispatch_host:
                raise ValueError("render worker started before batch dispatch")
            if finish < start:
                raise ValueError("render worker finished before it started")
            worker_ns.append(finish - start)
            cpu = batch["worker_thread_cpu_ns"]
            if cpu is not None:
                cpu = _nonnegative_integer(batch, "worker_thread_cpu_ns")
                if cpu > finish - start:
                    raise ValueError("render worker CPU time exceeds its wall span")
                worker_thread_cpu_ns.append(cpu)
                worker_non_cpu_wall_ns.append(finish - start - cpu)
            request = batch.get("join_request_host_ns")
            returned = batch.get("join_return_host_ns")
            if cause is None or request is None or returned is None:
                raise ValueError("render worker join fields must be present together")
            request = _nonnegative_integer(batch, "join_request_host_ns")
            returned = _nonnegative_integer(batch, "join_return_host_ns")
            if request < dispatch_host:
                raise ValueError("render join was requested before batch dispatch")
            if returned < request:
                raise ValueError("render join returned before it was requested")
            if returned < finish:
                raise ValueError("render join returned before worker completion")
            join_wait_ns.append(returned - request)
            guest_overlap_ns.append(max(0, min(request, finish) - start))
        elif any(
            batch.get(key) is not None
            for key in (
                "worker_start_host_ns",
                "worker_finish_host_ns",
                "worker_thread_cpu_ns",
                "join_cause",
                "join_request_host_ns",
                "join_return_host_ns",
            )
        ):
            raise ValueError("local render batch cannot carry worker or join fields")
    if worker_thread_cpu_ns and len(worker_thread_cpu_ns) != len(worker):
        raise ValueError("render worker CPU clock availability changed within the trace")
    return {
        "batches": len(batches),
        "dispatched_batches": len(dispatched),
        "incomplete_batches": len(incomplete),
        "incomplete_reasons": incomplete_reasons,
        "performance_complete": not incomplete,
        "members": sum(_integer(batch, "members") for batch in batches),
        "worker_batches": len(worker),
        "local_batches": len(local),
        "join_causes": join_causes,
        "worker_execute_ms": _duration_summary_ms(worker_ns),
        "worker_cpu_observed_batches": len(worker_thread_cpu_ns),
        "worker_cpu_unavailable_batches": len(worker) - len(worker_thread_cpu_ns),
        "worker_thread_cpu_ms": _duration_summary_ms(worker_thread_cpu_ns),
        "worker_non_cpu_wall_ms": _duration_summary_ms(worker_non_cpu_wall_ns),
        "guest_overlap_before_join_ms": _duration_summary_ms(guest_overlap_ns),
        "architectural_join_wait_ms": _duration_summary_ms(join_wait_ns),
        "emulation_finish_phases_ms": _duration_summary_ms(cpu_finish_ns),
    }


def _exact_cue_summary(
    header: dict[str, Any], data: list[dict[str, Any]]
) -> dict[str, Any]:
    cue_id = header.get("cue_id")
    exact = [
        record
        for record in data
        if record.get("record") in {"av_cue_audio", "av_cue_video", "av_cue_pair"}
    ]
    if cue_id is None:
        if exact:
            raise ValueError("exact cue records require header.cue_id")
        return {"requested": False, "valid": False, "reason": "not_requested"}
    if not isinstance(cue_id, str) or not cue_id or not all(
        character.isascii() and (character.isalnum() or character in "._-:")
        for character in cue_id
    ):
        raise ValueError("header.cue_id is invalid")
    if any(record.get("cue_id") != cue_id for record in exact):
        raise ValueError("exact cue record does not match header.cue_id")
    audio = [record for record in exact if record.get("record") == "av_cue_audio"]
    video = [record for record in exact if record.get("record") == "av_cue_video"]
    pairs = [record for record in exact if record.get("record") == "av_cue_pair"]
    missing = []
    if len(audio) != 1:
        missing.append("audio" if not audio else "unique_audio")
    if len(video) != 1:
        missing.append("video" if not video else "unique_video")
    if missing:
        if pairs:
            raise ValueError("av_cue_pair cannot exist without unique cue halves")
        return {
            "requested": True,
            "cue_id": cue_id,
            "valid": False,
            "reason": ",".join(missing),
        }
    audio_record = audio[0]
    video_record = video[0]
    valid = audio_record.get("valid")
    if not isinstance(valid, bool):
        raise ValueError("av_cue_audio.valid must be a boolean")
    if not valid:
        if pairs:
            raise ValueError("invalid av_cue_audio cannot have an av_cue_pair")
        reason = audio_record.get("invalid_reason")
        if not isinstance(reason, str) or not reason:
            raise ValueError("invalid av_cue_audio requires invalid_reason")
        return {
            "requested": True,
            "cue_id": cue_id,
            "valid": False,
            "reason": reason,
        }
    if audio_record.get("invalid_reason") is not None:
        raise ValueError("valid av_cue_audio cannot carry invalid_reason")
    if _integer(audio_record, "landmark_generation") != _integer(
        audio_record, "current_generation"
    ):
        raise ValueError("valid av_cue_audio generations do not match")
    if len(pairs) != 1:
        return {
            "requested": True,
            "cue_id": cue_id,
            "valid": False,
            "reason": "missing_pair" if not pairs else "nonunique_pair",
        }
    pair = pairs[0]
    _presentation_stage(video_record)
    if (
        _integer(pair, "audio_dma_id") != _integer(audio_record, "dma_id")
        or _integer(pair, "audio_guest_frame_offset")
        != _integer(audio_record, "guest_frame_offset")
        or _integer(pair, "audio_generation")
        != _integer(audio_record, "landmark_generation")
        or _hash64(pair, "video_hash") != _hash64(video_record, "rgba_hash")
        or _integer(pair, "video_occurrence") != _integer(video_record, "occurrence")
        or pair.get("video_stage") != _presentation_stage(video_record)
        or _integer(pair, "video_presentation_generation")
        != _integer(video_record, "presentation_generation")
        or _integer(pair, "video_retrace_cycle")
        != _integer(video_record, "retrace_cycle")
    ):
        raise ValueError("av_cue_pair does not identify its exact cue halves")
    denominator = _integer(pair, "cycle_denominator")
    if denominator <= 0:
        raise ValueError("av_cue_pair.cycle_denominator must be positive")
    if denominator != _integer(audio_record, "ai_clock_hz"):
        raise ValueError("av_cue_pair cycle denominator does not match audio clock")
    audio_cycle_numerator = _integer(pair, "audio_cycle_numerator")
    expected_audio_cycle_numerator = (
        _integer(audio_record, "dma_start_cycle") * denominator
        + _integer(audio_record, "guest_frame_offset")
        * _integer(header, "emulated_hz")
        * (_integer(audio_record, "start_dacrate") + 1)
    )
    if audio_cycle_numerator != expected_audio_cycle_numerator:
        raise ValueError("av_cue_pair audio cycle does not close")
    host_phase = _integer(pair, "video_minus_audio_host_ns")
    audio_host = _integer(pair, "audio_predicted_playback_host_ns")
    video_host = _integer(pair, "video_present_return_host_ns")
    if audio_host != _integer(audio_record, "predicted_playback_host_ns"):
        raise ValueError("av_cue_pair audio host instant does not match cue half")
    if video_host != _integer(video_record, "present_return_host_ns"):
        raise ValueError("av_cue_pair video host instant does not match cue half")
    if video_host - audio_host != host_phase:
        raise ValueError("av_cue_pair host phase does not close")
    guest_numerator = _integer(pair, "video_minus_audio_guest_numerator")
    if (
        _integer(pair, "video_retrace_cycle") * denominator
        - audio_cycle_numerator
        != guest_numerator
    ):
        raise ValueError("av_cue_pair guest phase does not close")
    return {
        "requested": True,
        "cue_id": cue_id,
        "valid": True,
        "audio_dma_id": _integer(pair, "audio_dma_id"),
        "audio_guest_frame_offset": _integer(pair, "audio_guest_frame_offset"),
        "audio_generation": _integer(pair, "audio_generation"),
        "video_hash": pair.get("video_hash"),
        "video_occurrence": _integer(pair, "video_occurrence"),
        "video_stage": pair.get("video_stage"),
        "video_presentation_generation": _integer(
            pair, "video_presentation_generation"
        ),
        "video_minus_audio_host_ms": host_phase / 1_000_000,
        "video_minus_audio_guest_cycles": guest_numerator / denominator,
        "guest_phase_numerator": guest_numerator,
        "guest_phase_denominator": denominator,
    }


def _audio_stream_start_summary(data: list[dict[str, Any]]) -> dict[str, Any]:
    records = [
        record for record in data if record.get("record") == "audio_stream_start"
    ]
    if not records:
        return {"complete": False, "reason": "missing"}
    if len(records) != 1:
        raise ValueError("audio_stream_start must be unique")
    record = records[0]
    dma_id = _integer(record, "dma_id")
    if dma_id <= 0:
        raise ValueError("audio_stream_start.dma_id must be positive")
    payload = _integer(record, "payload_queued_host_ns")
    play = _integer(record, "play_returned_host_ns")
    callback = _integer(record, "first_callback_host_ns")
    if play < payload:
        raise ValueError("audio stream play returned before its payload was queued")
    if callback < payload:
        raise ValueError("audio callback preceded its payload queue")
    return {
        "complete": True,
        "dma_id": dma_id,
        "dma_started_cycle": _nonnegative_integer(record, "dma_started_cycle"),
        "payload_to_play_ms": (play - payload) / 1_000_000,
        "first_callback_minus_play_return_ms": (callback - play) / 1_000_000,
        "payload_to_first_callback_ms": (callback - payload) / 1_000_000,
    }


def _telemetry_loss_summary(data: list[dict[str, Any]]) -> dict[str, Any]:
    records = [record for record in data if record.get("record") == "telemetry_loss"]
    dropped = 0
    for record in records:
        if record.get("source") != "audio_underrun":
            raise ValueError("telemetry_loss.source must be audio_underrun")
        count = _nonnegative_integer(record, "dropped_observations")
        if count == 0:
            raise ValueError("telemetry_loss.dropped_observations must be positive")
        dropped += count
    return {
        "records": len(records),
        "audio_underrun_dropped_observations": dropped,
        "complete": dropped == 0,
    }


def _audio_underrun_summary(
    data: list[dict[str, Any]], telemetry_loss: dict[str, Any]
) -> dict[str, Any]:
    records = [record for record in data if record.get("record") == "audio_underrun"]
    reasons: dict[str, int] = {}
    active_phases: dict[str, int] = {}
    requested_total = 0
    delivered_total = 0
    underrun_total = 0
    ring_depth_observed = 0
    previous_sequence = None
    missing_sequence_ids = 0
    for record in records:
        sequence = _nonnegative_integer(record, "sequence")
        if sequence == 0:
            raise ValueError("audio_underrun.sequence must be positive")
        callback = _integer(record, "callback_host_ns")
        if previous_sequence is not None and sequence <= previous_sequence:
            raise ValueError("audio_underrun.sequence must be unique and monotonic")
        if previous_sequence is None:
            missing_sequence_ids += sequence - 1
        else:
            missing_sequence_ids += sequence - previous_sequence - 1
        previous_sequence = sequence

        reason = record.get("reason")
        if reason not in AUDIO_UNDERRUN_REASONS:
            raise ValueError("audio_underrun.reason is invalid")
        active_phase = record.get("active_phase")
        if active_phase not in ACTIVE_PHASES:
            raise ValueError("audio_underrun.active_phase is invalid")
        requested = _nonnegative_integer(record, "requested_sample_slots")
        delivered = _nonnegative_integer(record, "delivered_sample_slots")
        underrun = _nonnegative_integer(record, "underrun_sample_slots")
        ring_before = _nullable_nonnegative_integer(record, "ring_sample_slots_before")
        if requested == 0:
            raise ValueError("audio_underrun.requested_sample_slots must be positive")
        if delivered > requested or underrun != requested - delivered or underrun == 0:
            raise ValueError("audio_underrun sample-slot accounting does not close")
        if reason == "ring_empty":
            if delivered != 0 or ring_before != 0:
                raise ValueError("ring_empty underrun requires zero delivery and ring depth")
        elif reason == "ring_short":
            if delivered == 0 or ring_before is None or ring_before != delivered:
                raise ValueError(
                    "ring_short underrun requires a partial delivery matching ring depth"
                )
        elif delivered != 0 or ring_before is not None:
            raise ValueError(
                "producer_contention underrun requires zero delivery and unknown ring depth"
            )
        reasons[reason] = reasons.get(reason, 0) + 1
        active_phases[active_phase] = active_phases.get(active_phase, 0) + 1
        requested_total += requested
        delivered_total += delivered
        underrun_total += underrun
        ring_depth_observed += ring_before is not None

    dropped = telemetry_loss["audio_underrun_dropped_observations"]
    if missing_sequence_ids > dropped:
        raise ValueError("audio_underrun sequence gaps exceed telemetry loss")
    return {
        "events": len(records),
        "telemetry_complete": telemetry_loss["complete"],
        "dropped_observations": dropped,
        "missing_sequence_ids": missing_sequence_ids,
        "unlocated_or_tail_dropped_observations": dropped - missing_sequence_ids,
        "reasons": reasons,
        "active_phases": active_phases,
        "requested_sample_slots": requested_total,
        "delivered_sample_slots": delivered_total,
        "underrun_sample_slots": underrun_total,
        "ring_depth_observed_events": ring_depth_observed,
        "ring_depth_unavailable_events": len(records) - ring_depth_observed,
        "first_sequence": records[0]["sequence"] if records else None,
        "last_sequence": records[-1]["sequence"] if records else None,
    }


def _presentation_span_summary(data: list[dict[str, Any]]) -> dict[str, Any]:
    scanouts = [record for record in data if record.get("record") == "vi_scanout_span"]
    windows = [record for record in data if record.get("record") == "window_present_span"]
    fields = [record for record in data if record.get("record") == "vi_present"]
    scanout_identities: dict[tuple[str, int, int], tuple[bool, int, int]] = {}
    scanout_cycles: set[int] = set()
    ready_scanout_ns = []
    unavailable_scanout_ns = []
    successful_window_ns = []
    failed_window_ns = []
    successful_window_identities: dict[tuple[str, int, int], tuple[int, int]] = {}
    presented_identities: dict[tuple[str, int, int], int] = {}

    for record in scanouts:
        cycle = _nonnegative_integer(record, "retrace_cycle")
        source_generation = _nonnegative_integer(record, "source_generation")
        post_vi_generation = _nonnegative_integer(record, "post_vi_generation")
        if source_generation == 0 or post_vi_generation == 0:
            raise ValueError("vi_scanout_span generations must be positive")
        source_ready = _boolean(record, "source_ready")
        post_vi_ready = _boolean(record, "post_vi_ready")
        if source_ready and post_vi_ready:
            raise ValueError("one VI scanout operation cannot ready both presentation stages")
        start = _integer(record, "start_host_ns")
        finish = _integer(record, "finish_host_ns")
        if finish < start:
            raise ValueError("vi_scanout_span finished before it started")
        if cycle in scanout_cycles:
            raise ValueError("vi_scanout_span retrace cycle must be unique")
        scanout_cycles.add(cycle)
        for identity, ready in (
            (("source", source_generation, cycle), source_ready),
            (("post_vi", post_vi_generation, cycle), post_vi_ready),
        ):
            if identity in scanout_identities:
                raise ValueError("vi_scanout_span presentation identity must be unique")
            scanout_identities[identity] = (ready, start, finish)
        (ready_scanout_ns if source_ready or post_vi_ready else unavailable_scanout_ns).append(
            finish - start
        )

    for record in fields:
        identity = (
            _presentation_stage(record),
            _nonnegative_integer(record, "presentation_generation"),
            _nonnegative_integer(record, "retrace_cycle"),
        )
        if identity in presented_identities:
            raise ValueError("vi_present presentation identity must be unique")
        presented_identities[identity] = _integer(record, "present_return_host_ns")

    for record in windows:
        outcome = record.get("outcome")
        if outcome not in {"success", "render_failed"}:
            raise ValueError("window_present_span.outcome is invalid")
        start = _integer(record, "start_host_ns")
        finish = _integer(record, "finish_host_ns")
        if finish < start:
            raise ValueError("window_present_span finished before it started")
        if "stage" not in record:
            raise ValueError("window_present_span.stage must be present")
        values = (
            record.get("stage"),
            record.get("presentation_generation"),
            record.get("retrace_cycle"),
        )
        if all(value is None for value in values):
            identity = None
        elif any(value is None for value in values):
            raise ValueError(
                "window_present_span presentation identity must be wholly present or null"
            )
        else:
            identity = (
                _presentation_stage(record),
                _nonnegative_integer(record, "presentation_generation"),
                _nonnegative_integer(record, "retrace_cycle"),
            )
        if outcome == "success":
            successful_window_ns.append(finish - start)
            if identity is not None:
                if identity in successful_window_identities:
                    raise ValueError(
                        "successful window presentation identity must be unique"
                    )
                successful_window_identities[identity] = (start, finish)
        else:
            failed_window_ns.append(finish - start)

    for identity, (window_start, window_finish) in successful_window_identities.items():
        scanout = scanout_identities.get(identity)
        if scanout is None or not scanout[0]:
            raise ValueError(
                "successful identified window presentation has no ready VI scanout"
            )
        if scanout[2] > window_start:
            raise ValueError("window presentation began before VI scanout finished")
        if presented_identities.get(identity) != window_finish:
            raise ValueError(
                "successful identified window presentation has no exact vi_present return"
            )
    for identity in presented_identities:
        if identity not in successful_window_identities:
            raise ValueError("vi_present has no successful matching window presentation")

    ready_identities = {
        identity for identity, (ready, _, _) in scanout_identities.items() if ready
    }
    unsubmitted_ready = ready_identities - successful_window_identities.keys()
    return {
        "vi_scanouts": len(scanouts),
        "ready_vi_scanouts": len(ready_scanout_ns),
        "unavailable_vi_scanouts": len(unavailable_scanout_ns),
        "ready_presentation_stages": len(ready_identities),
        "unsubmitted_ready_vi_scanouts": len(unsubmitted_ready),
        "ready_vi_scanout_ms": _duration_summary_ms(ready_scanout_ns),
        "unavailable_vi_scanout_ms": _duration_summary_ms(unavailable_scanout_ns),
        "window_presents": len(windows),
        "successful_window_presents": len(successful_window_ns),
        "failed_window_presents": len(failed_window_ns),
        "successful_window_present_ms": _duration_summary_ms(successful_window_ns),
        "failed_window_present_ms": _duration_summary_ms(failed_window_ns),
        "joined_presentations": len(successful_window_identities),
    }


def _guest_task_summary(data: list[dict[str, Any]]) -> dict[str, Any]:
    tasks = [record for record in data if record.get("record") == "guest_task"]
    batches: dict[int, dict[str, Any]] = {}
    for record in data:
        if record.get("record") not in {"render_batch", "render_batch_incomplete"}:
            continue
        batch_id = _nonnegative_integer(record, "batch_id")
        if batch_id in batches:
            raise ValueError("render batch identity is ambiguous for guest-task join")
        batches[batch_id] = record
    keys: set[tuple[int, int]] = set()
    outcomes: dict[str, int] = {}
    rsp_lanes: dict[str, int] = {}
    rdp_lanes: dict[str, int] = {}
    for task in tasks:
        task_offset = _nonnegative_integer(task, "task_offset")
        generation = _nonnegative_integer(task, "admission_generation")
        if generation == 0 or (task_offset, generation) in keys:
            raise ValueError("guest_task key must be unique with a nonzero generation")
        keys.add((task_offset, generation))
        resumed_from = task.get("resumed_from_admission_generation")
        if resumed_from is not None and (
            not isinstance(resumed_from, int)
            or isinstance(resumed_from, bool)
            or resumed_from <= 0
            or resumed_from >= generation
        ):
            raise ValueError("guest_task resumed generation must be positive and earlier")
        kind = task.get("kind")
        if kind not in {"graphics", "audio", "other"}:
            raise ValueError("guest_task.kind is invalid")
        outcome = task.get("outcome")
        if outcome not in {"completed", "yielded", "abandoned_at_process_exit"}:
            raise ValueError("guest_task.outcome is invalid")
        outcomes[outcome] = outcomes.get(outcome, 0) + 1
        if task.get("cpu_dispatch_lane") not in {
            "canonical_block_program",
            "abi_function_unattributed",
        }:
            raise ValueError("guest_task.cpu_dispatch_lane is invalid")
        thread_kind = task.get("dispatch_thread_kind")
        thread_id = task.get("dispatch_thread_id")
        if thread_kind == "executor":
            _nonnegative_integer(task, "dispatch_thread_id")
        elif thread_kind == "unattributed":
            if thread_id is not None:
                raise ValueError("unattributed guest task thread must have null ID")
        else:
            raise ValueError("guest_task.dispatch_thread_kind is invalid")
        rsp_lane = task.get("rsp_dispatch_lane")
        if rsp_lane not in {"interpreted", "translated", "unavailable"}:
            raise ValueError("guest_task.rsp_dispatch_lane is invalid")
        rsp_lanes[rsp_lane] = rsp_lanes.get(rsp_lane, 0) + 1
        rdp_lane = task.get("rdp_lane")
        if rdp_lane not in {"cpu", "compute", "mixed", "unavailable", "not_applicable"}:
            raise ValueError("guest_task.rdp_lane is invalid")
        rdp_lanes[rdp_lane] = rdp_lanes.get(rdp_lane, 0) + 1
        cpu_members = task.get("rdp_cpu_members")
        compute_members = task.get("rdp_compute_members")
        if rdp_lane in {"unavailable", "not_applicable"}:
            if cpu_members is not None or compute_members is not None:
                raise ValueError("unavailable/not-applicable guest RDP lane has member counts")
        else:
            cpu_members = _nonnegative_integer(task, "rdp_cpu_members")
            compute_members = _nonnegative_integer(task, "rdp_compute_members")
            expected = "compute" if cpu_members == 0 else "cpu" if compute_members == 0 else "mixed"
            if cpu_members + compute_members == 0 or rdp_lane != expected:
                raise ValueError("guest task RDP lane disagrees with its member counts")
        if (kind == "audio") != (rdp_lane == "not_applicable"):
            raise ValueError("only audio guest tasks have a not-applicable RDP lane")
        dispatch_cycle = _nonnegative_integer(task, "dispatch_cycle")
        completion_cycle = _nonnegative_integer(task, "completion_cycle")
        dispatch_host = _nonnegative_integer(task, "dispatch_host_ns")
        completion_host = _nonnegative_integer(task, "completion_host_ns")
        if completion_cycle < dispatch_cycle or completion_host < dispatch_host:
            raise ValueError("guest task completed before dispatch")
        queue_kind = task.get("queue_kind")
        queue_id = task.get("queue_id")
        if queue_kind == "not_applicable":
            if queue_id is not None or task.get("host_thread") != "emulation" or task.get("coherence_reason") is not None:
                raise ValueError("nonqueued guest task has queue/thread/coherence claims")
        elif queue_kind == "raw_dpc_task_batch":
            queue_id = _nonnegative_integer(task, "queue_id")
            batch = batches.get(queue_id)
            if batch is None:
                raise ValueError("guest task raw-DPC queue does not name a retained batch")
            if batch.get("record") == "render_batch":
                for field in ("rdp_lane", "rdp_cpu_members", "rdp_compute_members", "host_thread", "coherence_reason"):
                    if task.get(field) != batch.get(field):
                        raise ValueError(f"guest task {field} disagrees with actual batch evidence")
            elif not (
                outcome == "abandoned_at_process_exit"
                and rdp_lane == "unavailable"
                and task.get("host_thread") == "rdp_worker"
                and task.get("coherence_reason") is None
            ):
                raise ValueError("incomplete raw-DPC task must remain explicitly unavailable")
        else:
            raise ValueError("guest_task.queue_kind is invalid")
    return {
        "tasks": len(tasks),
        "outcomes": outcomes,
        "rsp_lanes": rsp_lanes,
        "rdp_lanes": rdp_lanes,
    }


def _least_squares_rate(
    records: list[dict[str, Any]], cycle_key: str, host_ns_key: str, hz: int
) -> float | None:
    if len(records) < 2:
        return None
    points = [
        (
            _integer(record, cycle_key) / hz,
            _integer(record, host_ns_key) / 1_000_000_000,
        )
        for record in records
    ]
    mean_emulated = statistics.fmean(point[0] for point in points)
    mean_host = statistics.fmean(point[1] for point in points)
    variance = sum((emulated - mean_emulated) ** 2 for emulated, _ in points)
    if variance == 0:
        return None
    covariance = sum(
        (emulated - mean_emulated) * (host - mean_host)
        for emulated, host in points
    )
    return covariance / variance


def _least_squares_rate_and_standard_error(
    records: list[dict[str, Any]], cycle_key: str, host_ns_key: str, hz: int
) -> tuple[float, float] | None:
    if len(records) < PACE_MINIMUM_SAMPLES_PER_LANE_WINDOW:
        return None
    points = [
        (
            _integer(record, cycle_key) / hz,
            _integer(record, host_ns_key) / 1_000_000_000,
        )
        for record in records
    ]
    mean_emulated = statistics.fmean(point[0] for point in points)
    mean_host = statistics.fmean(point[1] for point in points)
    variance = sum((emulated - mean_emulated) ** 2 for emulated, _ in points)
    if variance == 0:
        return None
    rate = sum(
        (emulated - mean_emulated) * (host - mean_host)
        for emulated, host in points
    ) / variance
    residual_sum_squares = sum(
        (host - (mean_host + rate * (emulated - mean_emulated))) ** 2
        for emulated, host in points
    )
    standard_error = (
        residual_sum_squares / (len(points) - 2) / variance
    ) ** 0.5
    return rate, standard_error


def _relative_rate_estimate(
    anchors: list[dict[str, Any]],
    fields: list[dict[str, Any]],
    hz: int,
) -> dict[str, float] | None:
    audio = _least_squares_rate_and_standard_error(
        anchors, "emulated_cycle", "predicted_playback_host_ns", hz
    )
    video = _least_squares_rate_and_standard_error(
        fields, "retrace_cycle", "present_return_host_ns", hz
    )
    if audio is None or video is None or audio[0] <= 0:
        return None
    audio_rate, audio_standard_error = audio
    video_rate, video_standard_error = video
    ratio = video_rate / audio_rate
    ratio_standard_error = (
        (video_standard_error / audio_rate) ** 2
        + (video_rate * audio_standard_error / audio_rate**2) ** 2
    ) ** 0.5
    ppm = (ratio - 1) * 1_000_000
    ppm_margin = PACE_CONFIDENCE_Z * ratio_standard_error * 1_000_000
    return {
        "audio_host_seconds_per_emulated_second": audio_rate,
        "video_host_seconds_per_emulated_second": video_rate,
        "video_vs_audio_rate_ppm": ppm,
        "video_vs_audio_rate_ppm_ci95_low": ppm - ppm_margin,
        "video_vs_audio_rate_ppm_ci95_high": ppm + ppm_margin,
        "video_minus_audio_drift_ms_per_minute": (video_rate - audio_rate)
        * 60_000,
    }


def _confidence_intervals_overlap(
    left: dict[str, Any], right: dict[str, Any]
) -> bool:
    return max(
        left["video_vs_audio_rate_ppm_ci95_low"],
        right["video_vs_audio_rate_ppm_ci95_low"],
    ) <= min(
        left["video_vs_audio_rate_ppm_ci95_high"],
        right["video_vs_audio_rate_ppm_ci95_high"],
    )


def _missing_positive_ids(values: list[int]) -> list[list[int]]:
    if not values:
        return []
    ranges = []
    previous = 0
    for value in values:
        if value <= previous:
            raise ValueError("audio_anchor.dma_id must be unique and monotonic")
        if value > previous + 1:
            ranges.append([previous + 1, value - 1])
        previous = value
    return ranges


def _relative_pace_summary(
    anchors: list[dict[str, Any]],
    fields: list[dict[str, Any]],
    hz: int,
    telemetry_loss: dict[str, Any],
    presentation_spans: dict[str, Any],
    audio_continuity_invalidated: bool,
) -> dict[str, Any] | None:
    overlap_start = max(
        _integer(anchors[0], "emulated_cycle"),
        _integer(fields[0], "retrace_cycle"),
    )
    overlap_end = min(
        _integer(anchors[-1], "emulated_cycle"),
        _integer(fields[-1], "retrace_cycle"),
    )
    pace_anchors = [
        anchor
        for anchor in anchors
        if overlap_start <= _integer(anchor, "emulated_cycle") <= overlap_end
    ]
    pace_fields = [
        field
        for field in fields
        if overlap_start <= _integer(field, "retrace_cycle") <= overlap_end
    ]
    audio_rate = _least_squares_rate(
        pace_anchors, "emulated_cycle", "predicted_playback_host_ns", hz
    )
    video_rate = _least_squares_rate(
        pace_fields, "retrace_cycle", "present_return_host_ns", hz
    )
    if audio_rate is None or video_rate is None or audio_rate <= 0:
        return None

    dma_ids = [_nonnegative_integer(anchor, "dma_id") for anchor in anchors]
    if any(dma_id == 0 for dma_id in dma_ids):
        raise ValueError("audio_anchor.dma_id must be positive")
    missing_dma_id_ranges = _missing_positive_ids(dma_ids)
    missing_dma_ids = sum(end - start + 1 for start, end in missing_dma_id_ranges)
    continuity_generations = sorted(
        {_nonnegative_integer(anchor, "generation") for anchor in anchors}
    )
    if any(generation == 0 for generation in continuity_generations):
        raise ValueError("audio_anchor.generation must be positive")

    windows = []
    span = overlap_end - overlap_start
    if span > 0:
        for index in range(PACE_WINDOW_COUNT):
            start = overlap_start + span * index // PACE_WINDOW_COUNT
            finish = overlap_start + span * (index + 1) // PACE_WINDOW_COUNT
            window_anchors = [
                anchor
                for anchor in pace_anchors
                if start <= _integer(anchor, "emulated_cycle")
                and (
                    _integer(anchor, "emulated_cycle") <= finish
                    if index == PACE_WINDOW_COUNT - 1
                    else _integer(anchor, "emulated_cycle") < finish
                )
            ]
            window_fields = [
                field
                for field in pace_fields
                if start <= _integer(field, "retrace_cycle")
                and (
                    _integer(field, "retrace_cycle") <= finish
                    if index == PACE_WINDOW_COUNT - 1
                    else _integer(field, "retrace_cycle") < finish
                )
            ]
            estimate = _relative_rate_estimate(window_anchors, window_fields, hz)
            windows.append(
                {
                    "index": index,
                    "start_cycle": start,
                    "end_cycle": finish,
                    "audio_samples": len(window_anchors),
                    "video_samples": len(window_fields),
                    "estimate": estimate,
                }
            )

    complete_windows = len(windows) == PACE_WINDOW_COUNT and all(
        window["estimate"] is not None for window in windows
    )
    startup_or_debt_recovery = False
    window_disagreement = False
    steady_start_cycle = overlap_start
    steady_anchors = pace_anchors
    steady_fields = pace_fields
    if complete_windows:
        first = windows[0]["estimate"]
        later = [window["estimate"] for window in windows[1:]]
        startup_or_debt_recovery = all(
            not _confidence_intervals_overlap(first, estimate) for estimate in later
        )
        steady_windows = later if startup_or_debt_recovery else [
            window["estimate"] for window in windows
        ]
        window_disagreement = any(
            not _confidence_intervals_overlap(left, right)
            for index, left in enumerate(steady_windows)
            for right in steady_windows[index + 1 :]
        )
        if startup_or_debt_recovery:
            steady_start_cycle = windows[1]["start_cycle"]
            steady_anchors = [
                anchor
                for anchor in pace_anchors
                if _integer(anchor, "emulated_cycle") >= steady_start_cycle
            ]
            steady_fields = [
                field
                for field in pace_fields
                if _integer(field, "retrace_cycle") >= steady_start_cycle
            ]

    refusal_reasons = []
    if span < PACE_MINIMUM_COMMON_SECONDS * hz or not complete_windows:
        refusal_reasons.append("too_short")
    if window_disagreement:
        refusal_reasons.append("window_disagreement")
    if len(continuity_generations) != 1 or audio_continuity_invalidated:
        refusal_reasons.append("audio_continuity_generation_changed")
    if not telemetry_loss["complete"]:
        refusal_reasons.append("telemetry_loss")

    steady_estimate = None
    if not refusal_reasons:
        steady_estimate = _relative_rate_estimate(steady_anchors, steady_fields, hz)
        if steady_estimate is None:
            refusal_reasons.append("too_short")

    return {
        "status": (
            "refused"
            if refusal_reasons
            else "accepted_after_startup_or_debt_recovery"
            if startup_or_debt_recovery
            else "accepted"
        ),
        "refusal_reasons": refusal_reasons,
        "startup_or_debt_recovery_detected": startup_or_debt_recovery,
        "overlap_start_cycle": overlap_start,
        "overlap_end_cycle": overlap_end,
        "overlap_seconds": span / hz,
        "audio_actual_start_cycle": _integer(pace_anchors[0], "emulated_cycle"),
        "audio_actual_end_cycle": _integer(pace_anchors[-1], "emulated_cycle"),
        "video_actual_start_cycle": _integer(pace_fields[0], "retrace_cycle"),
        "video_actual_end_cycle": _integer(pace_fields[-1], "retrace_cycle"),
        "audio_samples": len(pace_anchors),
        "video_samples": len(pace_fields),
        "audio_dma_id_first": dma_ids[0],
        "audio_dma_id_last": dma_ids[-1],
        "missing_audio_dma_anchor_ids": missing_dma_ids,
        "missing_audio_dma_anchor_id_ranges": missing_dma_id_ranges,
        "audio_continuity_generations": continuity_generations,
        "audio_continuity_complete": (
            len(continuity_generations) == 1 and not audio_continuity_invalidated
        ),
        "unsubmitted_ready_vi_scanouts": presentation_spans[
            "unsubmitted_ready_vi_scanouts"
        ],
        "telemetry_complete": telemetry_loss["complete"],
        "verdict_inputs_complete": not any(
            reason
            in {
                "audio_continuity_generation_changed",
                "telemetry_loss",
            }
            for reason in refusal_reasons
        ),
        "sample_coverage_complete": (
            missing_dma_ids == 0
            and presentation_spans["unsubmitted_ready_vi_scanouts"] == 0
        ),
        "whole_trace_audio_host_seconds_per_emulated_second": audio_rate,
        "whole_trace_video_host_seconds_per_emulated_second": video_rate,
        "whole_trace_video_vs_audio_rate_ppm": (video_rate / audio_rate - 1)
        * 1_000_000,
        "whole_trace_video_minus_audio_drift_ms_per_minute": (
            video_rate - audio_rate
        )
        * 60_000,
        "steady_start_cycle": steady_start_cycle,
        "steady_audio_samples": len(steady_anchors),
        "steady_video_samples": len(steady_fields),
        "steady_estimate": steady_estimate,
        "windows": windows,
    }


def summarize(
    header: dict[str, Any], data: list[dict[str, Any]], tolerance_ms: float
) -> dict[str, Any]:
    hz = _integer(header, "emulated_hz")
    anchors = [record for record in data if record.get("record") == "audio_anchor"]
    fields = [record for record in data if record.get("record") == "vi_present"]
    if not anchors:
        raise ValueError("trace contains no complete audio anchors")
    if not fields:
        raise ValueError("trace contains no VI presents")
    for previous, current in zip(anchors, anchors[1:]):
        if (
            _integer(current, "emulated_cycle")
            <= _integer(previous, "emulated_cycle")
            or _integer(current, "predicted_playback_host_ns")
            <= _integer(previous, "predicted_playback_host_ns")
        ):
            raise ValueError("audio anchors must be monotonic in cycle and host time")
    for field in fields:
        _presentation_stage(field)
        _integer(field, "presentation_generation")
    for previous, current in zip(fields, fields[1:]):
        if (
            _integer(current, "retrace_cycle")
            <= _integer(previous, "retrace_cycle")
            or _integer(current, "present_return_host_ns")
            < _integer(previous, "present_return_host_ns")
        ):
            raise ValueError("VI presents must be monotonic in cycle and host time")
    anchors.sort(key=lambda record: _integer(record, "emulated_cycle"))
    anchor_cycles = [_integer(record, "emulated_cycle") for record in anchors]
    comparable_fields = [
        field
        for field in fields
        if _integer(field, "retrace_cycle") >= anchor_cycles[0]
    ]
    if not comparable_fields:
        raise ValueError("trace contains no VI presents at or after its first audio anchor")

    comparisons = []
    for field in comparable_fields:
        vi_cycle = _integer(field, "retrace_cycle")
        anchor = _nearest_anchor(anchors, anchor_cycles, vi_cycle)
        audio_offset_ns = _integer(anchor, "predicted_playback_host_ns") - (
            _integer(anchor, "emulated_cycle") * 1_000_000_000 / hz
        )
        video_offset_ns = _integer(field, "present_return_host_ns") - (
            vi_cycle * 1_000_000_000 / hz
        )
        comparisons.append(
            {
                "stage": _presentation_stage(field),
                "presentation_generation": _integer(
                    field, "presentation_generation"
                ),
                "retrace_cycle": vi_cycle,
                "swap_count": _integer(field, "swap_count"),
                "audio_generation": _integer(anchor, "generation"),
                "audio_dma_id": _integer(anchor, "dma_id"),
                "audio_offset_ms": audio_offset_ns / 1_000_000,
                "video_offset_ms": video_offset_ns / 1_000_000,
                "api_boundary_video_minus_audio_ms": (video_offset_ns - audio_offset_ns)
                / 1_000_000,
            }
        )

    residuals = [item["api_boundary_video_minus_audio_ms"] for item in comparisons]
    violating = next(
        (
            item
            for item in comparisons
            if abs(item["api_boundary_video_minus_audio_ms"]) > tolerance_ms
        ),
        None,
    )
    telemetry_loss = _telemetry_loss_summary(data)
    presentation_spans = _presentation_span_summary(data)
    first_anchor_index = next(
        index for index, record in enumerate(data) if record.get("record") == "audio_anchor"
    )
    audio_continuity_invalidated = False
    for index, record in enumerate(data):
        if record.get("record") != "audio_generation":
            continue
        generation = _nonnegative_integer(record, "generation")
        if generation == 0:
            raise ValueError("audio_generation.generation must be positive")
        _integer(record, "observed_host_ns")
        if index > first_anchor_index and not _boolean(record, "anchor_valid"):
            audio_continuity_invalidated = True
    relative_pace = _relative_pace_summary(
        anchors,
        fields,
        hz,
        telemetry_loss,
        presentation_spans,
        audio_continuity_invalidated,
    )
    return {
        "schema": SCHEMA,
        "trace_id": header.get("trace_id"),
        "audio_anchors": len(anchors),
        "vi_presents": len(fields),
        "comparisons": len(comparisons),
        "vi_before_first_audio_anchor": len(fields) - len(comparable_fields),
        "tolerance_ms": tolerance_ms,
        "api_boundary_video_minus_audio_ms": {
            "median": statistics.median(residuals),
            "p05": _percentile(residuals, 0.05),
            "p95": _percentile(residuals, 0.95),
            "minimum": min(residuals),
            "maximum": max(residuals),
        },
        "relative_pace": relative_pace,
        "exact_cue": _exact_cue_summary(header, data),
        "audio_stream_start": _audio_stream_start_summary(data),
        "audio_underruns": _audio_underrun_summary(data, telemetry_loss),
        "presentation_spans": presentation_spans,
        "telemetry_loss": telemetry_loss,
        "renderer": _render_summary(data),
        "guest_tasks": _guest_task_summary(data),
        "first_outside_tolerance": violating,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("trace", type=Path)
    parser.add_argument("--tolerance-ms", type=float, default=5.0)
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args()
    if args.tolerance_ms < 0:
        parser.error("--tolerance-ms must be nonnegative")
    try:
        header, data = load_trace(args.trace)
        summary = summarize(header, data, args.tolerance_ms)
    except (OSError, ValueError) as error:
        parser.error(str(error))
    if args.json:
        print(json.dumps(summary, sort_keys=True))
    else:
        phase = summary["api_boundary_video_minus_audio_ms"]
        print(
            f"trace={summary['trace_id']} anchors={summary['audio_anchors']} "
            f"vi={summary['vi_presents']} comparisons={summary['comparisons']}"
        )
        print(
            "API-boundary video-minus-audio ms: "
            f"median={phase['median']:.3f} p05={phase['p05']:.3f} "
            f"p95={phase['p95']:.3f} range={phase['minimum']:.3f}..{phase['maximum']:.3f}"
        )
        pace = summary["relative_pace"]
        if pace is None:
            print("relative pace: unavailable (need two audio and two video samples in overlap)")
        else:
            print(
                "relative pace whole-trace diagnostic: "
                f"video_vs_audio={pace['whole_trace_video_vs_audio_rate_ppm']:+.1f} ppm "
                "phase_drift="
                f"{pace['whole_trace_video_minus_audio_drift_ms_per_minute']:+.3f} ms/min "
                f"(audio_n={pace['audio_samples']} video_n={pace['video_samples']})"
            )
            if pace["steady_estimate"] is None:
                print(
                    "relative pace verdict: refused "
                    f"({','.join(pace['refusal_reasons'])})"
                )
            else:
                steady = pace["steady_estimate"]
                print(
                    f"relative pace verdict: {pace['status']} "
                    f"video_vs_audio={steady['video_vs_audio_rate_ppm']:+.1f} ppm "
                    "ci95="
                    f"{steady['video_vs_audio_rate_ppm_ci95_low']:+.1f}.."
                    f"{steady['video_vs_audio_rate_ppm_ci95_high']:+.1f} ppm"
                )
            print(
                "  pace completeness: "
                f"verdict_inputs={pace['verdict_inputs_complete']} "
                f"sample_coverage={pace['sample_coverage_complete']} "
                f"missing_audio_dma_anchors={pace['missing_audio_dma_anchor_ids']} "
                f"unsubmitted_ready_vi={pace['unsubmitted_ready_vi_scanouts']} "
                "startup_or_debt_recovery="
                f"{pace['startup_or_debt_recovery_detected']}"
            )
            print(
                "  pace interval: "
                f"common={pace['overlap_seconds']:.3f} s "
                f"declared={pace['overlap_start_cycle']}..{pace['overlap_end_cycle']} "
                "audio_actual="
                f"{pace['audio_actual_start_cycle']}..{pace['audio_actual_end_cycle']} "
                "video_actual="
                f"{pace['video_actual_start_cycle']}..{pace['video_actual_end_cycle']}"
            )
        cue = summary["exact_cue"]
        if not cue["requested"]:
            print("exact cue: not requested")
        elif not cue["valid"]:
            print(f"exact cue {cue['cue_id']}: invalid ({cue['reason']})")
        else:
            print(
                f"exact cue {cue['cue_id']}: "
                f"video-minus-audio host={cue['video_minus_audio_host_ms']:+.3f} ms "
                f"guest={cue['video_minus_audio_guest_cycles']:+.3f} cycles"
            )
        startup = summary["audio_stream_start"]
        if not startup["complete"]:
            print(f"audio stream start: incomplete ({startup['reason']})")
        else:
            print(
                "audio stream start: "
                f"dma={startup['dma_id']} "
                f"payload-to-play={startup['payload_to_play_ms']:.3f} ms "
                "callback-minus-play-return="
                f"{startup['first_callback_minus_play_return_ms']:+.3f} ms"
            )
        renderer = summary["renderer"]
        print(
            "renderer: "
            f"dispatched={renderer['dispatched_batches']} complete={renderer['batches']} "
            f"incomplete={renderer['incomplete_batches']} members={renderer['members']} "
            f"worker={renderer['worker_batches']} local={renderer['local_batches']} "
            f"joins={renderer['join_causes']}"
        )
        if not renderer["performance_complete"]:
            print(
                "  renderer performance census is incomplete: "
                f"reasons={renderer['incomplete_reasons']}"
            )
        for label, key in (
            ("worker execute", "worker_execute_ms"),
            ("worker thread CPU", "worker_thread_cpu_ms"),
            ("worker non-CPU wall", "worker_non_cpu_wall_ms"),
            ("guest overlap before join", "guest_overlap_before_join_ms"),
            ("architectural join wait", "architectural_join_wait_ms"),
            ("emulation finish phases", "emulation_finish_phases_ms"),
        ):
            values = renderer[key]
            if values is not None:
                print(
                    f"  {label} ms: median={values['median']:.3f} "
                    f"p95={values['p95']:.3f} max={values['maximum']:.3f}"
                )
        underruns = summary["audio_underruns"]
        print(
            "audio underruns: "
            f"events={underruns['events']} dropped={underruns['dropped_observations']} "
            f"sample_slots={underruns['underrun_sample_slots']} "
            f"reasons={underruns['reasons']} phases={underruns['active_phases']}"
        )
        spans = summary["presentation_spans"]
        print(
            "presentation spans: "
            f"vi={spans['vi_scanouts']} ready={spans['ready_vi_scanouts']} "
            f"unavailable={spans['unavailable_vi_scanouts']} "
            f"unsubmitted_ready={spans['unsubmitted_ready_vi_scanouts']} "
            f"window={spans['window_presents']} success={spans['successful_window_presents']} "
            f"failed={spans['failed_window_presents']} joined={spans['joined_presentations']}"
        )
        first = summary["first_outside_tolerance"]
        if first is None:
            print(f"all fields within {summary['tolerance_ms']:.3f} ms")
        else:
            print(
                f"first outside {summary['tolerance_ms']:.3f} ms: "
                f"stage={first['stage']} "
                f"presentation_generation={first['presentation_generation']} "
                f"retrace_cycle={first['retrace_cycle']} swap={first['swap_count']} "
                f"audio_generation={first['audio_generation']} dma={first['audio_dma_id']} "
                "API-boundary residual="
                f"{first['api_boundary_video_minus_audio_ms']:.3f} ms"
            )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
