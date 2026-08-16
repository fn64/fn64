#!/usr/bin/env python3
"""Check fn64's future RT64 render-measurement wire contract.

This repository does not yet contain the Rust raw-sample ABI or shell JSON
emitter required by M0.3. Consequently this tool does not execute a binary,
collect samples, aggregate reports, or manufacture a comparison-ready fixture.
It validates the schema, generated documentation, and externally supplied
single reports. Cohort acceptance is deferred because v1 does not encode the
pair ordinals or requested/observed workload boundaries required to prove it.
"""

from __future__ import annotations

import argparse
import copy
import json
import math
import re
import sys
from pathlib import Path
from typing import Callable

ROOT = Path(__file__).resolve().parent.parent
SCHEMA_PATH = ROOT / "docs/rt64-render-measurement-schema.json"
DOC_PATH = ROOT / "docs/RT64-RENDER-MEASUREMENT.md"

REPORT_SCHEMA = "fn64.rt64-render-measurement-report.v1"
SCENARIO_RE = re.compile(r"^[a-z0-9][a-z0-9._-]{0,127}$")
LABEL_RE = re.compile(r"^[a-z0-9][a-z0-9._-]{0,63}$")
REASON_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9 .,_:()'-]{0,255}$")
HEX64_RE = re.compile(r"^[0-9a-f]{64}$")
HEX40_RE = re.compile(r"^[0-9a-f]{40}$")

TOP_LEVEL_KEYS = (
    "schema",
    "tier",
    "scenario",
    "process_role",
    "route",
    "identity",
    "horizon",
    "census",
    "metrics",
)
IDENTITY_KEYS = ("program", "build", "host", "gpu_query")
PROGRAM_KEYS = (
    "source_state",
    "generated_code_archive_sha256",
    "section_bridge_archive_sha256",
    "dispatch_source_sha256",
)
PROGRAM_DIGEST_KEYS = PROGRAM_KEYS[1:]
BUILD_KEYS = (
    "cargo_profile",
    "cargo_features",
    "rustc_vv_sha256",
    "git_head",
    "git_clean",
)
HOST_KEYS = ("os", "arch", "cpu_model")
GPU_QUERY_KEYS = ("state", "graphics_api", "adapter_name", "driver_version")
HORIZON_KEYS = (
    "warmup_gfx_submits",
    "steady_began_at_field",
    "steady_began_at_gfx_submits",
    "rebased",
)
CENSUS_KEYS = (
    "total_fields",
    "transient_fields",
    "truncated_advances",
    "warmup_gfx",
    "steady_began_at_field",
    "steady_began_at_gfx",
    "counters_armed",
    "samples",
)
SAMPLE_KEYS = (
    "advance_index",
    "wall_ns",
    "committed_vi_fields",
    "guest_cycles",
    "gfx_submits",
    "counters",
)
COUNTER_KEYS = (
    "present_count",
    "executor_ns",
    "gfx_total_ns",
    "rsp_ns",
    "rdp_submit_ns",
    "vi_present_ns",
    "dpc_alloc_ns",
    "dpc_copy_in_ns",
    "dpc_copy_back_ns",
)
METRIC_KEYS = (
    "sample_count",
    "cpu_field_latency",
    "gpu_interval",
    "copy_upload_readback_bytes",
    "queue_wait",
    "allocation_bytes",
    "shader_pso_compile",
    "full_gpu_pass",
    "total_vram_bytes",
    "physical_presentation",
)
CPU_LATENCY_KEYS = ("state", "p50_ms", "p95_ms", "p99_ms", "max_ms", "mean_ms")
STATEFUL_METRICS = METRIC_KEYS[2:]
INTEGER_METRICS = {
    "copy_upload_readback_bytes",
    "allocation_bytes",
    "shader_pso_compile",
    "total_vram_bytes",
    "physical_presentation",
}
ALL_METRIC_STATES = ("armed", "unavailable", "armed_not_reached")
CROSS_FIELD_RULES = (
    "horizon values equal their census counterparts",
    "control reports have counters_armed false; instrumented reports have counters_armed true",
    "sample advance_index values are contiguous from zero",
    "sample_count equals the sample array length",
    "cpu_field_latency percentiles use each advance wall_ns divided by committed_vi_fields",
    "cpu_field_latency mean_ms equals sum wall_ns divided by sum committed_vi_fields",
    "total_fields covers transient plus every retained committed VI field",
    "headless_pump_one_frame requires physical_presentation unavailable",
    "comparison_ready requires a receipt, clean build, queried GPU, rebased horizon, no truncation, and every metric armed",
)
COHORT_RULES = (
    "v1 cohort acceptance is deferred because reports do not encode the required pair and boundary fields",
    "a future cohort schema must require exactly five control/instrumented pairs",
    "a future cohort schema must encode explicit pair and repetition ordinals",
    "a future cohort schema must encode alternating control then instrumented and instrumented then control pair order",
    "a future cohort schema must require equal identity and requested and observed horizon and workload boundaries within each pair",
)
METRIC_SPECS = {
    "sample_count": {
        "value_type": "integer",
        "unit": "samples",
        "allowed_states": ["armed"],
    },
    "cpu_field_latency": {
        "value_type": "distribution",
        "unit": "milliseconds_per_committed_vi_field",
        "allowed_states": ["armed"],
        "exact_keys": list(CPU_LATENCY_KEYS),
    },
    "gpu_interval": {
        "value_type": "number",
        "unit": "nanoseconds",
        "allowed_states": list(ALL_METRIC_STATES),
    },
    "copy_upload_readback_bytes": {
        "value_type": "integer",
        "unit": "bytes",
        "allowed_states": list(ALL_METRIC_STATES),
    },
    "queue_wait": {
        "value_type": "number",
        "unit": "nanoseconds",
        "allowed_states": list(ALL_METRIC_STATES),
    },
    "allocation_bytes": {
        "value_type": "integer",
        "unit": "bytes",
        "allowed_states": list(ALL_METRIC_STATES),
    },
    "shader_pso_compile": {
        "value_type": "integer",
        "unit": "compile_events",
        "allowed_states": list(ALL_METRIC_STATES),
    },
    "full_gpu_pass": {
        "value_type": "number",
        "unit": "nanoseconds",
        "allowed_states": list(ALL_METRIC_STATES),
    },
    "total_vram_bytes": {
        "value_type": "integer",
        "unit": "bytes",
        "allowed_states": list(ALL_METRIC_STATES),
    },
    "physical_presentation": {
        "value_type": "integer",
        "unit": "present_events",
        "allowed_states": list(ALL_METRIC_STATES),
    },
}


class MeasurementError(RuntimeError):
    pass


def require(condition: bool, message: str) -> None:
    if not condition:
        raise MeasurementError(message)


def _object(value: object, keys: tuple[str, ...], at: str) -> dict:
    require(isinstance(value, dict), f"{at} must be an object")
    actual = set(value.keys())
    expected = set(keys)
    require(
        actual == expected,
        f"{at} keys must be exactly {list(keys)!r}; "
        f"missing={sorted(expected - actual)!r}, extra={sorted(actual - expected)!r}",
    )
    return value


def _integer(value: object, at: str, minimum: int = 0) -> int:
    require(type(value) is int, f"{at} must be an integer")
    require(value >= minimum, f"{at} must be >= {minimum}")
    require(value <= (1 << 64) - 1, f"{at} must fit in an unsigned 64-bit integer")
    return value


def _number(value: object, at: str, minimum: float = 0.0) -> float:
    require(type(value) in (int, float), f"{at} must be a number")
    numeric = float(value)
    require(math.isfinite(numeric), f"{at} must be finite")
    require(numeric >= minimum, f"{at} must be >= {minimum}")
    return numeric


def _string(value: object, at: str, maximum: int = 256) -> str:
    require(isinstance(value, str), f"{at} must be a string")
    require(1 <= len(value) <= maximum, f"{at} length must be in 1..={maximum}")
    return value


def _require_path_free(value: object, at: str = "$") -> None:
    if isinstance(value, str):
        require("/" not in value and "\\" not in value, f"{at} must not contain a path separator")
        require("\x00" not in value and "\n" not in value and "\r" not in value, f"{at} must be one path-free line")
    elif isinstance(value, dict):
        for key, nested in value.items():
            _require_path_free(nested, f"{at}.{key}")
    elif isinstance(value, list):
        for index, nested in enumerate(value):
            _require_path_free(nested, f"{at}[{index}]")


def load_schema() -> dict:
    try:
        schema = json.loads(SCHEMA_PATH.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise MeasurementError(f"cannot load {SCHEMA_PATH}: {error}") from error
    require(isinstance(schema, dict), "schema root must be an object")
    return schema


def validate_schema_shape(schema: object) -> None:
    root = _object(
        schema,
        (
            "schema",
            "description",
            "availability",
            "tiers",
            "metric_value_shapes",
            "required_top_level_fields",
            "fields",
            "cross_field_rules",
            "cohort_rules",
        ),
        "schema",
    )
    require(root["schema"] == REPORT_SCHEMA, "schema identity drift")
    _string(root["description"], "schema.description", 512)

    availability = _object(
        root["availability"],
        ("report_emitter", "cohort_validator", "missing", "run_command"),
        "schema.availability",
    )
    require(availability["report_emitter"] == "unavailable", "report emitter must remain unavailable until its Rust seam lands")
    require(availability["cohort_validator"] == "deferred", "cohort validation must remain deferred until v1 grows pair and boundary fields")
    require(availability["run_command"] is None, "run_command must be null while the emitter is unavailable")
    require(
        availability["missing"]
        == [
            "fn64-abi raw per-advance measurement snapshot",
            "post-warmup measurement-window rebase",
            "fn64-shell path-free JSON report emitter",
            "native metric instruments and presentation-capable route",
        ],
        "availability.missing frontier drift",
    )

    tiers = _object(root["tiers"], ("development", "comparison_ready"), "schema.tiers")
    for name, may_compare in (("development", False), ("comparison_ready", True)):
        tier = _object(tiers[name], ("meaning", "may_compare"), f"schema.tiers.{name}")
        _string(tier["meaning"], f"schema.tiers.{name}.meaning", 256)
        require(tier["may_compare"] is may_compare, f"schema.tiers.{name}.may_compare drift")

    shapes = _object(
        root["metric_value_shapes"], ALL_METRIC_STATES, "schema.metric_value_shapes"
    )
    require(
        shapes["armed"] == {"exact_keys": ["state", "value"], "value_minimum": 0},
        "armed metric value shape drift",
    )
    require(
        shapes["unavailable"]
        == {
            "exact_keys": ["state", "reason"],
            "reason_pattern": REASON_RE.pattern,
        },
        "unavailable metric value shape drift",
    )
    require(
        shapes["armed_not_reached"]
        == {"exact_keys": ["state", "value"], "value_const": 0},
        "armed_not_reached metric value shape drift",
    )

    require(root["required_top_level_fields"] == list(TOP_LEVEL_KEYS), "top-level report keys/order drift")
    fields = _object(root["fields"], TOP_LEVEL_KEYS, "schema.fields")
    require(fields["schema"] == {"type": "string", "const": REPORT_SCHEMA}, "fields.schema drift")
    require(fields["tier"] == {"type": "string", "enum": ["development", "comparison_ready"]}, "fields.tier drift")
    require(fields["scenario"] == {"type": "string", "pattern": SCENARIO_RE.pattern}, "fields.scenario drift")
    require(fields["process_role"] == {"type": "string", "enum": ["control", "instrumented"]}, "fields.process_role drift")
    require(fields["route"] == {"type": "string", "enum": ["headless_pump_one_frame"]}, "fields.route drift")

    identity = _object(fields["identity"], ("type", "exact_keys", "properties"), "schema.fields.identity")
    require(identity["type"] == "object" and identity["exact_keys"] == list(IDENTITY_KEYS), "identity declaration drift")
    identity_props = _object(identity["properties"], IDENTITY_KEYS, "schema.fields.identity.properties")
    for name, keys in (
        ("program", PROGRAM_KEYS),
        ("build", BUILD_KEYS),
        ("host", HOST_KEYS),
        ("gpu_query", GPU_QUERY_KEYS),
    ):
        require(
            identity_props[name] == {"type": "object", "exact_keys": list(keys)},
            f"identity.{name} declaration drift",
        )

    horizon = _object(fields["horizon"], ("type", "exact_keys", "integer_minimum"), "schema.fields.horizon")
    require(horizon == {"type": "object", "exact_keys": list(HORIZON_KEYS), "integer_minimum": 0}, "horizon declaration drift")
    census = _object(
        fields["census"],
        ("type", "exact_keys", "sample_exact_keys", "counter_exact_keys"),
        "schema.fields.census",
    )
    require(census["type"] == "object", "census type drift")
    require(census["exact_keys"] == list(CENSUS_KEYS), "census keys/order drift")
    require(census["sample_exact_keys"] == list(SAMPLE_KEYS), "sample keys/order drift")
    require(census["counter_exact_keys"] == list(COUNTER_KEYS), "counter keys/order drift")

    metrics = _object(fields["metrics"], ("type", "exact_keys", "properties"), "schema.fields.metrics")
    require(metrics["type"] == "object" and metrics["exact_keys"] == list(METRIC_KEYS), "metric keys/order drift")
    properties = _object(metrics["properties"], METRIC_KEYS, "schema.fields.metrics.properties")
    require(
        properties == METRIC_SPECS,
        "v1 metric type/unit/state specifications drift",
    )
    require(
        root["cross_field_rules"] == list(CROSS_FIELD_RULES),
        "cross_field_rules semantic contract drift",
    )
    require(
        root["cohort_rules"] == list(COHORT_RULES),
        "cohort_rules semantic contract drift",
    )


def _validate_program(value: object) -> dict:
    program = _object(value, PROGRAM_KEYS, "$.identity.program")
    state = program["source_state"]
    require(state in ("issued_receipt", "content_free_build_no_receipt"), "$.identity.program.source_state invalid")
    for key in PROGRAM_DIGEST_KEYS:
        digest = program[key]
        if state == "issued_receipt":
            require(isinstance(digest, str) and HEX64_RE.fullmatch(digest) is not None, f"$.identity.program.{key} must be sha256 for issued_receipt")
        else:
            require(digest is None, f"$.identity.program.{key} must be null without a receipt")
    return program


def _validate_build(value: object) -> dict:
    build = _object(value, BUILD_KEYS, "$.identity.build")
    profile = _string(build["cargo_profile"], "$.identity.build.cargo_profile", 64)
    require(LABEL_RE.fullmatch(profile) is not None, "$.identity.build.cargo_profile must be a path-free label")
    features = build["cargo_features"]
    require(isinstance(features, list), "$.identity.build.cargo_features must be an array")
    require(len(features) <= 128, "$.identity.build.cargo_features exceeds 128 entries")
    for index, feature in enumerate(features):
        feature = _string(feature, f"$.identity.build.cargo_features[{index}]", 64)
        require(LABEL_RE.fullmatch(feature) is not None, f"$.identity.build.cargo_features[{index}] must be a label")
    require(features == sorted(set(features)), "$.identity.build.cargo_features must be sorted and unique")
    require(isinstance(build["rustc_vv_sha256"], str) and HEX64_RE.fullmatch(build["rustc_vv_sha256"]) is not None, "$.identity.build.rustc_vv_sha256 must be sha256")
    require(isinstance(build["git_head"], str) and HEX40_RE.fullmatch(build["git_head"]) is not None, "$.identity.build.git_head must be sha1")
    require(type(build["git_clean"]) is bool, "$.identity.build.git_clean must be boolean")
    return build


def _validate_host(value: object) -> dict:
    host = _object(value, HOST_KEYS, "$.identity.host")
    _string(host["os"], "$.identity.host.os", 128)
    _string(host["arch"], "$.identity.host.arch", 128)
    if host["cpu_model"] is not None:
        _string(host["cpu_model"], "$.identity.host.cpu_model", 256)
    return host


def _validate_gpu_query(value: object) -> dict:
    gpu = _object(value, GPU_QUERY_KEYS, "$.identity.gpu_query")
    require(gpu["state"] in ("queried", "unavailable"), "$.identity.gpu_query.state invalid")
    details = GPU_QUERY_KEYS[1:]
    if gpu["state"] == "queried":
        for key in details:
            _string(gpu[key], f"$.identity.gpu_query.{key}", 256)
    else:
        for key in details:
            require(gpu[key] is None, f"$.identity.gpu_query.{key} must be null when unavailable")
    return gpu


def _validate_horizon(value: object) -> dict:
    horizon = _object(value, HORIZON_KEYS, "$.horizon")
    for key in HORIZON_KEYS[:3]:
        _integer(horizon[key], f"$.horizon.{key}")
    require(type(horizon["rebased"]) is bool, "$.horizon.rebased must be boolean")
    return horizon


def _validate_census(
    value: object, process_role: str, horizon: dict
) -> tuple[dict, list[float], float]:
    census = _object(value, CENSUS_KEYS, "$.census")
    for key in CENSUS_KEYS[:6]:
        _integer(census[key], f"$.census.{key}")
    require(type(census["counters_armed"]) is bool, "$.census.counters_armed must be boolean")
    require(
        census["counters_armed"] is (process_role == "instrumented"),
        "$.census.counters_armed must be false for control and true for instrumented",
    )
    samples = census["samples"]
    require(isinstance(samples, list), "$.census.samples must be an array")
    require(1 <= len(samples) <= 200_000, "$.census.samples length must be in 1..=200000")
    latencies: list[float] = []
    retained_fields = 0
    retained_wall_ns = 0
    for index, value in enumerate(samples):
        sample = _object(value, SAMPLE_KEYS, f"$.census.samples[{index}]")
        require(_integer(sample["advance_index"], f"$.census.samples[{index}].advance_index") == index, f"$.census.samples[{index}].advance_index must equal its zero-based array index")
        wall_ns = _integer(sample["wall_ns"], f"$.census.samples[{index}].wall_ns")
        committed = _integer(sample["committed_vi_fields"], f"$.census.samples[{index}].committed_vi_fields", 1)
        _integer(sample["guest_cycles"], f"$.census.samples[{index}].guest_cycles")
        _integer(sample["gfx_submits"], f"$.census.samples[{index}].gfx_submits")
        retained_fields += committed
        retained_wall_ns += wall_ns
        latencies.append(wall_ns / committed / 1_000_000.0)
        counters = sample["counters"]
        if census["counters_armed"]:
            counters = _object(counters, COUNTER_KEYS, f"$.census.samples[{index}].counters")
            for key in COUNTER_KEYS:
                _integer(counters[key], f"$.census.samples[{index}].counters.{key}")
        else:
            require(counters is None, f"$.census.samples[{index}].counters must be null when counters are unarmed")

    require(horizon["warmup_gfx_submits"] == census["warmup_gfx"], "$.horizon.warmup_gfx_submits must equal $.census.warmup_gfx")
    require(horizon["steady_began_at_field"] == census["steady_began_at_field"], "$.horizon.steady_began_at_field must equal $.census.steady_began_at_field")
    require(horizon["steady_began_at_gfx_submits"] == census["steady_began_at_gfx"], "$.horizon.steady_began_at_gfx_submits must equal $.census.steady_began_at_gfx")
    require(census["total_fields"] >= census["transient_fields"] + retained_fields, "$.census.total_fields must cover transient plus retained committed fields")
    require(census["total_fields"] >= census["steady_began_at_field"] + retained_fields, "$.census.total_fields must cover the steady boundary plus retained committed fields")
    mean_ms = retained_wall_ns / retained_fields / 1_000_000.0
    return census, latencies, mean_ms


def _nearest_rank(values: list[float], percentile: int) -> float:
    ordered = sorted(values)
    rank = max(math.ceil(percentile * len(ordered) / 100), 1)
    return ordered[rank - 1]


def _validate_cpu_latency(value: object, latencies: list[float], mean_ms: float) -> None:
    metric = _object(value, CPU_LATENCY_KEYS, "$.metrics.cpu_field_latency")
    require(metric["state"] == "armed", "$.metrics.cpu_field_latency.state must be armed")
    expected = {
        "p50_ms": _nearest_rank(latencies, 50),
        "p95_ms": _nearest_rank(latencies, 95),
        "p99_ms": _nearest_rank(latencies, 99),
        "max_ms": max(latencies),
        "mean_ms": mean_ms,
    }
    for key, expected_value in expected.items():
        actual = _number(metric[key], f"$.metrics.cpu_field_latency.{key}")
        require(
            math.isclose(actual, expected_value, rel_tol=1e-12, abs_tol=1e-12),
            f"$.metrics.cpu_field_latency.{key} must equal the sample-derived value {expected_value!r}",
        )


def _validate_stateful_metric(name: str, value: object) -> str:
    require(isinstance(value, dict), f"$.metrics.{name} must be an object")
    state = value.get("state")
    require(state in ALL_METRIC_STATES, f"$.metrics.{name}.state invalid")
    if state == "unavailable":
        metric = _object(value, ("state", "reason"), f"$.metrics.{name}")
        reason = _string(metric["reason"], f"$.metrics.{name}.reason", 256)
        require(REASON_RE.fullmatch(reason) is not None, f"$.metrics.{name}.reason fails the content-free reason pattern")
        return state
    metric = _object(value, ("state", "value"), f"$.metrics.{name}")
    if name in INTEGER_METRICS:
        numeric = _integer(metric["value"], f"$.metrics.{name}.value")
    else:
        numeric = _number(metric["value"], f"$.metrics.{name}.value")
    if state == "armed_not_reached":
        require(numeric == 0, f"$.metrics.{name}.value must be zero for armed_not_reached")
    return state


def _derive_tier(report: dict, states: dict[str, str]) -> str:
    ready = (
        report["identity"]["program"]["source_state"] == "issued_receipt"
        and report["identity"]["build"]["git_clean"] is True
        and report["identity"]["gpu_query"]["state"] == "queried"
        and report["horizon"]["rebased"] is True
        and report["census"]["truncated_advances"] == 0
        and all(state == "armed" for state in states.values())
    )
    return "comparison_ready" if ready else "development"


def validate_report(report: object) -> str:
    report = _object(report, TOP_LEVEL_KEYS, "$")
    require(report["schema"] == REPORT_SCHEMA, "$.schema identity drift")
    require(report["tier"] in ("development", "comparison_ready"), "$.tier invalid")
    scenario = _string(report["scenario"], "$.scenario", 128)
    require(SCENARIO_RE.fullmatch(scenario) is not None, "$.scenario fails the content-free label pattern")
    require(HEX64_RE.fullmatch(scenario) is None, "$.scenario must not be a raw sha256")
    require(report["process_role"] in ("control", "instrumented"), "$.process_role invalid")
    require(report["route"] == "headless_pump_one_frame", "$.route invalid")

    identity = _object(report["identity"], IDENTITY_KEYS, "$.identity")
    _validate_program(identity["program"])
    _validate_build(identity["build"])
    _validate_host(identity["host"])
    _validate_gpu_query(identity["gpu_query"])
    _require_path_free(report)

    horizon = _validate_horizon(report["horizon"])
    census, latencies, mean_ms = _validate_census(
        report["census"], report["process_role"], horizon
    )
    metrics = _object(report["metrics"], METRIC_KEYS, "$.metrics")
    sample_count = _object(metrics["sample_count"], ("state", "value"), "$.metrics.sample_count")
    require(sample_count["state"] == "armed", "$.metrics.sample_count.state must be armed")
    require(_integer(sample_count["value"], "$.metrics.sample_count.value", 1) == len(census["samples"]), "$.metrics.sample_count.value must equal len($.census.samples)")
    _validate_cpu_latency(metrics["cpu_field_latency"], latencies, mean_ms)
    states = {name: _validate_stateful_metric(name, metrics[name]) for name in STATEFUL_METRICS}

    require(
        states["physical_presentation"] == "unavailable",
        "headless_pump_one_frame cannot arm physical_presentation",
    )
    derived = _derive_tier(report, states)
    require(report["tier"] == derived, f"$.tier claims {report['tier']!r}, derived tier is {derived!r}")
    return derived


def render_doc(schema: dict) -> str:
    missing = schema["availability"]["missing"]
    metric_specs = schema["fields"]["metrics"]["properties"]
    lines = [
        "# RT64 render measurement report contract",
        "",
        "Generated by `tools/run_rt64_render_baseline.py` from",
        "`docs/rt64-render-measurement-schema.json`; do not edit this file by hand.",
        "",
        f"Wire schema: `{schema['schema']}`. {schema['description']}",
        "",
        "## Current frontier",
        "",
        "Report production is **unavailable**. The checker has no `--run` mode and",
        "does not launch a native binary. These implementation seams are missing:",
        "",
    ]
    lines.extend(f"- {item}." for item in missing)
    lines.extend(
        [
            "",
            "The schema/checker mechanism is closed; M0.3 baseline capture is not.",
            "No private linked build was run and no measurement numbers exist.",
            "",
            "## Tiers",
            "",
            "| Tier | Meaning | May compare |",
            "|---|---|---|",
        ]
    )
    for name, tier in schema["tiers"].items():
        lines.append(f"| `{name}` | {tier['meaning']} | {tier['may_compare']} |")
    lines.extend(
        [
            "",
            "The sole declared route, `headless_pump_one_frame`, has no physical",
            "presentation. Its `physical_presentation` metric must therefore be",
            "`unavailable`, which mechanically prevents `comparison_ready`. Adding a",
            "presentation-capable route and its Rust emitter is a later schema change.",
            "",
            "## Exact metric value shapes",
            "",
            "| State | Exact keys | Constraint |",
            "|---|---|---|",
            "| `armed` | `state`, `value` | finite value at least zero |",
            "| `unavailable` | `state`, `reason` | nonempty path-free reason; no value |",
            "| `armed_not_reached` | `state`, `value` | value exactly zero |",
            "",
            "`sample_count` and `cpu_field_latency` are always armed and have their own",
            "exact shapes. All other metrics admit the three shapes above.",
            "",
            "## Metric denominator",
            "",
            "| Metric | Value kind | Unit |",
            "|---|---|---|",
        ]
    )
    for name in METRIC_KEYS:
        spec = metric_specs[name]
        lines.append(f"| `{name}` | `{spec['value_type']}` | `{spec['unit']}` |")
    lines.extend(
        [
            "",
            "## Mechanical checks",
            "",
            "The checker requires exact key sets at every object level; exact scalar",
            "types (booleans never count as integers); nonnegative, finite values;",
            "contiguous sample order; matching horizon/census boundaries; exact",
            "sample-count, percentile, and weighted-mean latency derivation;",
            "role/counter agreement; and no path",
            "separator in any report string.",
            "",
            "## Cohort frontier",
            "",
            "This v1 report does not encode pair/repetition ordinals or both requested",
            "and observed horizon/workload boundaries. Cohort validation is therefore",
            "deferred, not approximated. A later schema must require exactly five pairs,",
            "explicit ordinals, alternating control/instrumented and instrumented/control",
            "order, and equal identity plus requested/observed boundaries within each pair.",
            "",
            "## Validation",
            "",
            "```sh",
            "python3 tools/run_rt64_render_baseline.py --validate-schema",
            "python3 tools/run_rt64_render_baseline.py --check-doc",
            "python3 tools/run_rt64_render_baseline.py --validate-report /path/to/report.json",
            "python3 tools/run_rt64_render_baseline.py --selftest",
            "```",
            "",
        ]
    )
    return "\n".join(lines)


def check_doc(schema: dict) -> None:
    expected = render_doc(schema)
    try:
        actual = DOC_PATH.read_text(encoding="utf-8")
    except OSError as error:
        raise MeasurementError(f"cannot read {DOC_PATH}: {error}") from error
    require(actual == expected, f"{DOC_PATH.relative_to(ROOT)} is stale; regenerate it from the schema")


def _counters() -> dict:
    return {key: 0 for key in COUNTER_KEYS}


def _development_report(role: str = "control") -> dict:
    armed = role == "instrumented"
    return {
        "schema": REPORT_SCHEMA,
        "tier": "development",
        "scenario": "unit-test",
        "process_role": role,
        "route": "headless_pump_one_frame",
        "identity": {
            "program": {
                "source_state": "content_free_build_no_receipt",
                "generated_code_archive_sha256": None,
                "section_bridge_archive_sha256": None,
                "dispatch_source_sha256": None,
            },
            "build": {
                "cargo_profile": "release",
                "cargo_features": [],
                "rustc_vv_sha256": "a" * 64,
                "git_head": "b" * 40,
                "git_clean": True,
            },
            "host": {"os": "macos", "arch": "aarch64", "cpu_model": None},
            "gpu_query": {
                "state": "unavailable",
                "graphics_api": None,
                "adapter_name": None,
                "driver_version": None,
            },
        },
        "horizon": {
            "warmup_gfx_submits": 0,
            "steady_began_at_field": 0,
            "steady_began_at_gfx_submits": 0,
            "rebased": False,
        },
        "census": {
            "total_fields": 1,
            "transient_fields": 0,
            "truncated_advances": 0,
            "warmup_gfx": 0,
            "steady_began_at_field": 0,
            "steady_began_at_gfx": 0,
            "counters_armed": armed,
            "samples": [
                {
                    "advance_index": 0,
                    "wall_ns": 16_000_000,
                    "committed_vi_fields": 1,
                    "guest_cycles": 1_500_000,
                    "gfx_submits": 1,
                    "counters": _counters() if armed else None,
                }
            ],
        },
        "metrics": {
            "sample_count": {"state": "armed", "value": 1},
            "cpu_field_latency": {
                "state": "armed",
                "p50_ms": 16.0,
                "p95_ms": 16.0,
                "p99_ms": 16.0,
                "max_ms": 16.0,
                "mean_ms": 16.0,
            },
            **{
                name: {"state": "unavailable", "reason": "instrument not implemented"}
                for name in STATEFUL_METRICS
            },
        },
    }


def _unequal_multifield_report() -> dict:
    report = _development_report()
    report["census"]["total_fields"] = 4
    report["census"]["samples"] = [
        {
            "advance_index": 0,
            "wall_ns": 10_000_000,
            "committed_vi_fields": 1,
            "guest_cycles": 1_000_000,
            "gfx_submits": 1,
            "counters": None,
        },
        {
            "advance_index": 1,
            "wall_ns": 60_000_000,
            "committed_vi_fields": 3,
            "guest_cycles": 3_000_000,
            "gfx_submits": 1,
            "counters": None,
        },
    ]
    report["metrics"]["sample_count"]["value"] = 2
    report["metrics"]["cpu_field_latency"].update(
        {
            "p50_ms": 10.0,
            "p95_ms": 20.0,
            "p99_ms": 20.0,
            "max_ms": 20.0,
            "mean_ms": 17.5,
        }
    )
    return report


def _expect_rejected(label: str, action: Callable[[], None]) -> None:
    try:
        action()
    except MeasurementError:
        return
    raise MeasurementError(f"hostile matrix failed to reject: {label}")


def selftest() -> int:
    schema = load_schema()
    validate_schema_shape(schema)
    require(validate_report(_development_report("control")) == "development", "control positive fixture failed")
    require(validate_report(_development_report("instrumented")) == "development", "instrumented positive fixture failed")
    require(
        validate_report(_unequal_multifield_report()) == "development",
        "unequal multi-field weighted-mean fixture failed",
    )

    hostile: list[tuple[str, Callable[[], None]]] = []

    def report_case(label: str, mutate: Callable[[dict], None]) -> None:
        report = _development_report()
        mutate(report)
        hostile.append((label, lambda report=report: validate_report(report)))

    report_case("top-level missing", lambda r: r.pop("route"))
    report_case("top-level extra", lambda r: r.__setitem__("extra", 1))
    report_case("wrong schema", lambda r: r.__setitem__("schema", "wrong"))
    report_case("tier wrong type", lambda r: r.__setitem__("tier", 1))
    report_case("fabricated comparison tier", lambda r: r.__setitem__("tier", "comparison_ready"))
    report_case("scenario slash", lambda r: r.__setitem__("scenario", "unit/test"))
    report_case("scenario raw hash", lambda r: r.__setitem__("scenario", "a" * 64))
    report_case("scenario empty", lambda r: r.__setitem__("scenario", ""))
    report_case("role invalid", lambda r: r.__setitem__("process_role", "other"))
    report_case("route invalid", lambda r: r.__setitem__("route", "windowed"))
    report_case("identity extra", lambda r: r["identity"].__setitem__("hostname", "secret"))
    report_case("program missing", lambda r: r["identity"]["program"].pop("dispatch_source_sha256"))
    report_case("program state invalid", lambda r: r["identity"]["program"].__setitem__("source_state", "guessed"))
    report_case("receipt missing digests", lambda r: r["identity"]["program"].__setitem__("source_state", "issued_receipt"))
    report_case("receipt digest malformed", lambda r: (r["identity"]["program"].update({"source_state": "issued_receipt", "generated_code_archive_sha256": "x", "section_bridge_archive_sha256": "d" * 64, "dispatch_source_sha256": "e" * 64})))
    report_case("content-free digest present", lambda r: r["identity"]["program"].__setitem__("dispatch_source_sha256", "e" * 64))
    report_case("build extra", lambda r: r["identity"]["build"].__setitem__("target", "host"))
    report_case("profile slash", lambda r: r["identity"]["build"].__setitem__("cargo_profile", "target/release"))
    report_case("features wrong type", lambda r: r["identity"]["build"].__setitem__("cargo_features", "rt64"))
    report_case("features unsorted", lambda r: r["identity"]["build"].__setitem__("cargo_features", ["z", "a"]))
    report_case("features duplicate", lambda r: r["identity"]["build"].__setitem__("cargo_features", ["a", "a"]))
    report_case("feature path", lambda r: r["identity"]["build"].__setitem__("cargo_features", ["a/b"]))
    report_case("rustc hash malformed", lambda r: r["identity"]["build"].__setitem__("rustc_vv_sha256", "a" * 63))
    report_case("git head malformed", lambda r: r["identity"]["build"].__setitem__("git_head", "b" * 39))
    report_case("git clean integer", lambda r: r["identity"]["build"].__setitem__("git_clean", 1))
    report_case("host missing", lambda r: r["identity"]["host"].pop("cpu_model"))
    report_case("host path", lambda r: r["identity"]["host"].__setitem__("cpu_model", "/private/model"))
    report_case("host windows path", lambda r: r["identity"]["host"].__setitem__("cpu_model", "C:\\private\\model"))
    report_case("gpu unavailable with detail", lambda r: r["identity"]["gpu_query"].__setitem__("graphics_api", "metal"))
    report_case("gpu queried without detail", lambda r: r["identity"]["gpu_query"].__setitem__("state", "queried"))
    report_case("horizon extra", lambda r: r["horizon"].__setitem__("end", 1))
    report_case("horizon bool integer", lambda r: r["horizon"].__setitem__("warmup_gfx_submits", True))
    report_case("horizon negative", lambda r: r["horizon"].__setitem__("steady_began_at_field", -1))
    report_case("horizon rebase integer", lambda r: r["horizon"].__setitem__("rebased", 1))
    report_case("census extra", lambda r: r["census"].__setitem__("other", 0))
    report_case("census negative", lambda r: r["census"].__setitem__("total_fields", -1))
    report_case("census bool integer", lambda r: r["census"].__setitem__("total_fields", False))
    report_case("control counters armed", lambda r: r["census"].__setitem__("counters_armed", True))
    report_case("samples wrong type", lambda r: r["census"].__setitem__("samples", {}))
    report_case("samples empty", lambda r: r["census"].__setitem__("samples", []))
    report_case("sample extra", lambda r: r["census"]["samples"][0].__setitem__("extra", 0))
    report_case("advance index wrong", lambda r: r["census"]["samples"][0].__setitem__("advance_index", 1))
    report_case("wall bool", lambda r: r["census"]["samples"][0].__setitem__("wall_ns", True))
    report_case("committed zero", lambda r: r["census"]["samples"][0].__setitem__("committed_vi_fields", 0))
    report_case("guest cycles negative", lambda r: r["census"]["samples"][0].__setitem__("guest_cycles", -1))
    report_case("control counters object", lambda r: r["census"]["samples"][0].__setitem__("counters", _counters()))
    report_case("horizon warmup mismatch", lambda r: r["horizon"].__setitem__("warmup_gfx_submits", 1))
    report_case("horizon field mismatch", lambda r: r["horizon"].__setitem__("steady_began_at_field", 1))
    report_case("horizon gfx mismatch", lambda r: r["horizon"].__setitem__("steady_began_at_gfx_submits", 1))
    report_case("total fields undercount", lambda r: r["census"].__setitem__("total_fields", 0))
    report_case("metrics extra", lambda r: r["metrics"].__setitem__("other", {"state": "armed", "value": 0}))
    report_case("sample count unavailable", lambda r: r["metrics"].__setitem__("sample_count", {"state": "unavailable", "reason": "missing"}))
    report_case("sample count bool", lambda r: r["metrics"]["sample_count"].__setitem__("value", True))
    report_case("sample count mismatch", lambda r: r["metrics"]["sample_count"].__setitem__("value", 2))
    report_case("latency missing", lambda r: r["metrics"]["cpu_field_latency"].pop("mean_ms"))
    report_case("latency nan", lambda r: r["metrics"]["cpu_field_latency"].__setitem__("mean_ms", float("nan")))
    report_case("latency infinity", lambda r: r["metrics"]["cpu_field_latency"].__setitem__("max_ms", float("inf")))
    report_case("latency negative", lambda r: r["metrics"]["cpu_field_latency"].__setitem__("p50_ms", -1.0))
    report_case("latency derivation mismatch", lambda r: r["metrics"]["cpu_field_latency"].__setitem__("p99_ms", 15.0))
    report_case("stateful unknown state", lambda r: r["metrics"].__setitem__("queue_wait", {"state": "unknown"}))
    report_case("unavailable with value", lambda r: r["metrics"].__setitem__("queue_wait", {"state": "unavailable", "reason": "missing", "value": 0}))
    report_case("unavailable no reason", lambda r: r["metrics"].__setitem__("queue_wait", {"state": "unavailable"}))
    report_case("unavailable path reason", lambda r: r["metrics"].__setitem__("queue_wait", {"state": "unavailable", "reason": "missing /tmp/query"}))
    report_case("unavailable newline reason", lambda r: r["metrics"].__setitem__("queue_wait", {"state": "unavailable", "reason": "missing\nquery"}))
    report_case("armed no value", lambda r: r["metrics"].__setitem__("queue_wait", {"state": "armed"}))
    report_case("armed with reason", lambda r: r["metrics"].__setitem__("queue_wait", {"state": "armed", "value": 0, "reason": "why"}))
    report_case("armed nan", lambda r: r["metrics"].__setitem__("queue_wait", {"state": "armed", "value": float("nan")}))
    report_case("armed negative", lambda r: r["metrics"].__setitem__("queue_wait", {"state": "armed", "value": -1}))
    report_case("integer metric float", lambda r: r["metrics"].__setitem__("allocation_bytes", {"state": "armed", "value": 1.5}))
    report_case("not reached nonzero", lambda r: r["metrics"].__setitem__("queue_wait", {"state": "armed_not_reached", "value": 1}))
    report_case("not reached reason", lambda r: r["metrics"].__setitem__("queue_wait", {"state": "armed_not_reached", "reason": "none"}))
    report_case("headless presentation armed", lambda r: r["metrics"].__setitem__("physical_presentation", {"state": "armed", "value": 1}))
    report_case("headless presentation not reached", lambda r: r["metrics"].__setitem__("physical_presentation", {"state": "armed_not_reached", "value": 0}))

    instrumented = _development_report("instrumented")
    instrumented["census"]["samples"][0]["counters"].pop("rsp_ns")
    hostile.append(("instrumented counter missing", lambda: validate_report(instrumented)))
    instrumented_extra = _development_report("instrumented")
    instrumented_extra["census"]["samples"][0]["counters"]["other"] = 0
    hostile.append(("instrumented counter extra", lambda: validate_report(instrumented_extra)))
    instrumented_negative = _development_report("instrumented")
    instrumented_negative["census"]["samples"][0]["counters"]["rsp_ns"] = -1
    hostile.append(("instrumented counter negative", lambda: validate_report(instrumented_negative)))

    old_mean = _unequal_multifield_report()
    old_mean["metrics"]["cpu_field_latency"]["mean_ms"] = 15.0
    hostile.append(("unweighted per-advance mean", lambda: validate_report(old_mean)))

    schema_hostile = copy.deepcopy(schema)
    schema_hostile["availability"]["run_command"] = "run-now"
    hostile.append(("schema cannot advertise runner", lambda value=schema_hostile: validate_schema_shape(value)))
    schema_hostile = copy.deepcopy(schema)
    schema_hostile["fields"]["metrics"]["exact_keys"].pop()
    hostile.append(("schema metric denominator cannot shrink", lambda value=schema_hostile: validate_schema_shape(value)))
    schema_hostile = copy.deepcopy(schema)
    schema_hostile["metric_value_shapes"]["armed_not_reached"]["value_const"] = 1
    hostile.append(("schema not-reached value cannot drift", lambda value=schema_hostile: validate_schema_shape(value)))
    schema_hostile = copy.deepcopy(schema)
    schema_hostile["fields"]["metrics"]["properties"]["queue_wait"]["unit"] = "milliseconds"
    hostile.append(("schema metric unit semantic mutation", lambda value=schema_hostile: validate_schema_shape(value)))
    schema_hostile = copy.deepcopy(schema)
    schema_hostile["cross_field_rules"][0] = "same length but different cross-field rule"
    hostile.append(("schema cross-field same-length mutation", lambda value=schema_hostile: validate_schema_shape(value)))
    schema_hostile = copy.deepcopy(schema)
    schema_hostile["cohort_rules"][0] = "same length but false cohort rule"
    hostile.append(("schema cohort same-length mutation", lambda value=schema_hostile: validate_schema_shape(value)))

    for label, action in hostile:
        _expect_rejected(label, action)

    require(render_doc(schema) == render_doc(schema), "doc rendering must be deterministic")
    check_doc(schema)
    print(
        f"rt64-render-measurement selftest: 3 development reports accepted, "
        f"{len(hostile)} hostile cases rejected, weighted mean checked, cohort deferred, "
        "comparison-ready fabrication rejected, generated doc exact"
    )
    return 0


def _load_json(path: Path, label: str) -> object:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise MeasurementError(f"cannot read {label} {path}: {error}") from error


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    actions = parser.add_mutually_exclusive_group(required=True)
    actions.add_argument("--validate-schema", action="store_true")
    actions.add_argument("--print-doc", action="store_true")
    actions.add_argument("--check-doc", action="store_true")
    actions.add_argument("--validate-report", type=Path)
    actions.add_argument("--selftest", action="store_true")
    args = parser.parse_args()

    try:
        if args.selftest:
            return selftest()
        schema = load_schema()
        validate_schema_shape(schema)
        if args.validate_schema:
            print(f"rt64-render-measurement: {SCHEMA_PATH.relative_to(ROOT)} is valid")
        elif args.print_doc:
            sys.stdout.write(render_doc(schema))
        elif args.check_doc:
            check_doc(schema)
            print("rt64-render-measurement: schema and generated document agree")
        elif args.validate_report is not None:
            report = _load_json(args.validate_report, "report")
            tier = validate_report(report)
            print(f"rt64-render-measurement: report is valid, tier={tier}")
        return 0
    except MeasurementError as error:
        print(f"rt64-render-measurement: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
