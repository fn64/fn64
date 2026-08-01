#!/usr/bin/env python3
"""Run a digest-bound cold-ROM panel under explicit process resource limits.

The input manifest and ROMs are private local capabilities.  Successful
stdout is buffered until every child and repetition validates, and contains
only stable identifiers, digests, measurements, and resource observations.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import resource
import secrets
import shutil
import signal
import stat
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any


MANIFEST_SCHEMA = "fn64.cold-coverage-panel-input.v1"
RECEIPT_SCHEMA = "fn64.cold-rom-measurement.v2"
MEASUREMENT_FIELDS = {
    "schema",
    "limits",
    "normalized_rom_sha256",
    "selected_strategy",
    "strategy_outcomes",
    "fact_count",
    "overlay_relocation_fact_count",
    "proven_bank_count",
    "closure",
    "stage1_effects",
    "ledger_total_bytes",
    "ledger_code_like_floor_bytes",
    "ledger_bytes_by_class",
}
OBSERVATION_SCHEMA = "fn64.cold-coverage-observation.v1"
RESULT_SCHEMA = "fn64.cold-coverage-panel-result.v1"
MAX_MANIFEST_BYTES = 1024 * 1024
MAX_ROM_BYTES = 64 * 1024 * 1024
MAX_EXECUTABLE_BYTES = 256 * 1024 * 1024
MAX_SUBPROCESS_OUTPUT_BYTES = 1024 * 1024
MAX_PANEL_ENTRIES = 64
DEFAULT_REPETITIONS = 10
DEFAULT_TIMEOUT_SECONDS = 600
DEFAULT_MAX_RSS_MIB = 2048
DEFAULT_MIN_FREE_PERCENT = 40
DEFAULT_POLL_MILLISECONDS = 250
TEARDOWN_TIMEOUT_SECONDS = 5
MEMORY_LIMIT_STARTUP_HEADROOM_BYTES = 256 * 1024 * 1024
U64_MAX = (1 << 64) - 1
HEX256 = re.compile(r"[0-9a-f]{64}\Z")
STABLE_ID = re.compile(r"[a-z0-9][a-z0-9_-]{0,63}\Z")

FIXED_MEASUREMENT_LIMITS = {
    "max_rom_input_bytes": 64 * 1024 * 1024,
    "max_decoded_vrom_file_bytes": 64 * 1024 * 1024,
    "max_banks": 4096,
    "max_aggregate_materialized_bytes": 64 * 1024 * 1024,
    "max_projected_fact_rows": 4_000_000,
    "max_projected_fact_bytes": 256 * 1024 * 1024,
    "max_cross_bank_authority_records": 1_048_576,
}
DISCOVERY_STRATEGIES = {
    "boot_bank_open",
    "boot_bank_only",
    "recovered_vrom",
    "recovered_overlays",
    "untabled_delta_vote",
}
STRATEGY_OUTCOME_FIELDS = {
    "strategy",
    "candidate_tables",
    "admitted_tables",
    "admitted_intervals",
    "decoded_file_limit_hits",
    "proven_mappings",
    "supported_mappings",
    "request_dma_open_rows",
    "request_dma_incomplete",
    "request_dma_input_limit_hit",
    "physical_wrapper_candidates_examined",
    "wrapper_semantic_proof_unavailable",
    "physical_wrapper_candidate_limit_hit",
}
STRATEGY_OUTCOME_COUNT_FIELDS = STRATEGY_OUTCOME_FIELDS - {
    "strategy",
    "request_dma_incomplete",
    "request_dma_input_limit_hit",
    "physical_wrapper_candidate_limit_hit",
}
DESTINATION_CLASSES = {
    "exact_aot",
    "block_aot",
    "dynamic_mips",
    "unsupported",
}
DESTINATION_REASONS = {
    "in_exact_owner",
    "in_proven_block",
    "open_indirect_site",
    "bounded_indirect_site",
    "mapped_not_proven_code",
    "proven_code_no_owner",
    "into_proven_data",
    "outside_all_mappings",
}
LEDGER_CLASSES = {
    "mapped",
    "header_and_ipl3",
    "padding",
    "container",
    "code_like",
    "high_entropy",
    "structured_data",
    "unclassified",
}
INTRINSIC_EFFECT_KINDS = {
    "cache",
    "sync",
    "cop0_read",
    "cop0_write",
    "cop0_control",
    "syscall",
    "break",
}
DIRECT_PHYSICAL_REGIONS = {"rdram", "rcp", "pif", "other"}
STAGE1_SUMMARY_FIELDS = {
    "bank_count",
    "aligned_word_count",
    "reachable_intrinsic_by_kind",
    "nondeterministic_cop0_read_count",
    "proven_data_intrinsic_count",
    "unclassified_intrinsic_count",
    "reachable_memory_read_count",
    "reachable_memory_write_count",
    "exact_direct_memory_by_region",
    "exact_tlb_translated_memory_count",
    "open_memory_address_count",
    "obvious_external_effect_count",
}


class PanelError(RuntimeError):
    pass


@dataclass(frozen=True)
class ManifestEntry:
    stable_id: str
    rom_path: Path
    expected_normalized_rom_sha256: str


@dataclass(frozen=True)
class ChildResult:
    stdout: bytes
    wall_ms: int
    peak_rss_bytes: int


def exact_keys(value: dict[str, Any], required: set[str], label: str) -> None:
    missing = required - set(value)
    unknown = set(value) - required
    if missing or unknown:
        raise PanelError(
            f"{label} fields differ: missing={sorted(missing)} unknown={sorted(unknown)}"
        )


def object_value(value: Any, label: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise PanelError(f"{label} must be a JSON object")
    return value


def reject_duplicate_pairs(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise PanelError(f"JSON contains duplicate field {key!r}")
        result[key] = value
    return result


def reject_nonfinite(value: str) -> Any:
    raise PanelError(f"JSON contains non-finite number {value}")


def decode_json(data: bytes, label: str) -> Any:
    try:
        text = data.decode("utf-8")
    except UnicodeDecodeError as error:
        raise PanelError(f"{label} is not UTF-8") from error
    try:
        return json.loads(
            text,
            object_pairs_hook=reject_duplicate_pairs,
            parse_constant=reject_nonfinite,
        )
    except (json.JSONDecodeError, PanelError) as error:
        if isinstance(error, PanelError):
            raise
        raise PanelError(f"{label} is not valid JSON") from error


def canonical_sorted(value: Any) -> bytes:
    try:
        return json.dumps(
            value,
            sort_keys=True,
            separators=(",", ":"),
            ensure_ascii=False,
            allow_nan=False,
        ).encode("utf-8")
    except (TypeError, ValueError) as error:
        raise PanelError("value cannot be encoded as canonical JSON") from error


def rust_struct_json(measurement: dict[str, Any]) -> bytes:
    """Reconstruct serde_json's V2 struct order, independent of input order."""
    limits = measurement["limits"]
    outcomes = measurement["strategy_outcomes"]
    closure = measurement["closure"]
    effects = measurement["stage1_effects"]

    ordered_limits = {
        field: limits[field]
        for field in (
            "max_rom_input_bytes",
            "max_decoded_vrom_file_bytes",
            "max_banks",
            "max_aggregate_materialized_bytes",
            "max_projected_fact_rows",
            "max_projected_fact_bytes",
            "max_cross_bank_authority_records",
        )
    }
    ordered_outcomes = [
        {
            field: outcome[field]
            for field in (
                "strategy",
                "candidate_tables",
                "admitted_tables",
                "admitted_intervals",
                "decoded_file_limit_hits",
                "proven_mappings",
                "supported_mappings",
                "request_dma_open_rows",
                "request_dma_incomplete",
                "request_dma_input_limit_hit",
                "physical_wrapper_candidates_examined",
                "wrapper_semantic_proof_unavailable",
                "physical_wrapper_candidate_limit_hit",
            )
        }
        for outcome in outcomes
    ]
    if closure["status"] == "open":
        ordered_closure = {"status": "open", "blocker": closure["blocker"]}
    else:
        scoreboard = closure["scoreboard"]
        ordered_closure = {
            "status": "measured",
            "scoreboard": {
                "total_destinations": scoreboard["total_destinations"],
                "per_class": {
                    name: {
                        "destinations": scoreboard["per_class"][name]["destinations"],
                        "bytes": scoreboard["per_class"][name]["bytes"],
                    }
                    for name in sorted(scoreboard["per_class"])
                },
                "per_reason": {
                    name: scoreboard["per_reason"][name]
                    for name in sorted(scoreboard["per_reason"])
                },
                "unsupported": scoreboard["unsupported"],
                "dynamic_mips": scoreboard["dynamic_mips"],
            },
        }
    if effects["status"] == "open":
        ordered_effects = {"status": "open", "blocker": effects["blocker"]}
    else:
        summary = effects["summary"]
        ordered_effects = {
            "status": "measured",
            "summary": {
                "bank_count": summary["bank_count"],
                "aligned_word_count": summary["aligned_word_count"],
                "reachable_intrinsic_by_kind": {
                    name: summary["reachable_intrinsic_by_kind"][name]
                    for name in sorted(summary["reachable_intrinsic_by_kind"])
                },
                "nondeterministic_cop0_read_count": summary[
                    "nondeterministic_cop0_read_count"
                ],
                "proven_data_intrinsic_count": summary["proven_data_intrinsic_count"],
                "unclassified_intrinsic_count": summary["unclassified_intrinsic_count"],
                "reachable_memory_read_count": summary["reachable_memory_read_count"],
                "reachable_memory_write_count": summary["reachable_memory_write_count"],
                "exact_direct_memory_by_region": {
                    name: summary["exact_direct_memory_by_region"][name]
                    for name in sorted(summary["exact_direct_memory_by_region"])
                },
                "exact_tlb_translated_memory_count": summary[
                    "exact_tlb_translated_memory_count"
                ],
                "open_memory_address_count": summary["open_memory_address_count"],
                "obvious_external_effect_count": summary["obvious_external_effect_count"],
            },
        }
    ordered = {
        "schema": measurement["schema"],
        "limits": ordered_limits,
        "normalized_rom_sha256": measurement["normalized_rom_sha256"],
        "selected_strategy": measurement["selected_strategy"],
        "strategy_outcomes": ordered_outcomes,
        "fact_count": measurement["fact_count"],
        "overlay_relocation_fact_count": measurement["overlay_relocation_fact_count"],
        "proven_bank_count": measurement["proven_bank_count"],
        "closure": ordered_closure,
        "stage1_effects": ordered_effects,
        "ledger_total_bytes": measurement["ledger_total_bytes"],
        "ledger_code_like_floor_bytes": measurement["ledger_code_like_floor_bytes"],
        "ledger_bytes_by_class": {
            name: measurement["ledger_bytes_by_class"][name]
            for name in sorted(measurement["ledger_bytes_by_class"])
        },
    }
    try:
        return json.dumps(
            ordered,
            separators=(",", ":"),
            ensure_ascii=False,
            allow_nan=False,
        ).encode("utf-8")
    except (TypeError, ValueError) as error:
        raise PanelError("measurement cannot be re-encoded as JSON") from error


def require_digest(value: Any, label: str) -> str:
    if not isinstance(value, str) or not HEX256.fullmatch(value):
        raise PanelError(f"{label} must be a lowercase SHA-256")
    return value


def require_stable_id(value: Any) -> str:
    if not isinstance(value, str) or not STABLE_ID.fullmatch(value):
        raise PanelError("stable_id must be a path-free lowercase identifier")
    return value


def require_u64(value: Any, label: str) -> int:
    if type(value) is not int or value < 0 or value > U64_MAX:
        raise PanelError(f"{label} must be an unsigned 64-bit integer")
    return value


def require_bool(value: Any, label: str) -> bool:
    if type(value) is not bool:
        raise PanelError(f"{label} must be a boolean")
    return value


def require_enum(value: Any, choices: set[str], label: str) -> str:
    if not isinstance(value, str) or value not in choices:
        raise PanelError(f"{label} has an unsupported value")
    return value


def validate_count_map(
    value: Any,
    *,
    allowed: set[str],
    label: str,
    require_all: bool = False,
) -> dict[str, Any]:
    counts = object_value(value, label)
    unknown = set(counts) - allowed
    missing = allowed - set(counts) if require_all else set()
    if unknown or missing:
        raise PanelError(
            f"{label} fields differ: missing={sorted(missing)} unknown={sorted(unknown)}"
        )
    for key, count in counts.items():
        require_u64(count, f"{label}.{key}")
    return counts


def validate_strategy_outcomes(value: Any, selected_strategy: str) -> None:
    if not isinstance(value, list) or not value:
        raise PanelError("cold-ROM strategy_outcomes must be a nonempty array")
    seen: set[str] = set()
    for index, raw in enumerate(value):
        outcome = object_value(raw, f"cold-ROM strategy outcome {index + 1}")
        exact_keys(
            outcome,
            STRATEGY_OUTCOME_FIELDS,
            f"cold-ROM strategy outcome {index + 1}",
        )
        strategy = require_enum(
            outcome["strategy"],
            DISCOVERY_STRATEGIES,
            f"cold-ROM strategy outcome {index + 1} strategy",
        )
        if strategy in seen:
            raise PanelError("cold-ROM strategy_outcomes contains a duplicate strategy")
        seen.add(strategy)
        for field in STRATEGY_OUTCOME_COUNT_FIELDS:
            require_u64(
                outcome[field], f"cold-ROM strategy outcome {index + 1}.{field}"
            )
        for field in (
            "request_dma_incomplete",
            "request_dma_input_limit_hit",
            "physical_wrapper_candidate_limit_hit",
        ):
            require_bool(
                outcome[field], f"cold-ROM strategy outcome {index + 1}.{field}"
            )
    if selected_strategy not in seen:
        raise PanelError("selected cold-ROM strategy has no strategy outcome")


def validate_closure(value: Any) -> None:
    closure = object_value(value, "cold-ROM closure")
    status = closure.get("status")
    if status == "open":
        exact_keys(closure, {"status", "blocker"}, "cold-ROM open closure")
        require_enum(
            closure["blocker"],
            {
                "no_proven_mappings",
                "bank_preparation_rejected",
                "snapshot_composition_rejected",
            },
            "cold-ROM closure blocker",
        )
        return
    if status != "measured":
        raise PanelError("cold-ROM closure status has an unsupported value")
    exact_keys(closure, {"status", "scoreboard"}, "cold-ROM measured closure")
    scoreboard = object_value(closure["scoreboard"], "cold-ROM closure scoreboard")
    exact_keys(
        scoreboard,
        {"total_destinations", "per_class", "per_reason", "unsupported", "dynamic_mips"},
        "cold-ROM closure scoreboard",
    )
    per_class = object_value(scoreboard["per_class"], "cold-ROM closure per_class")
    exact_keys(per_class, DESTINATION_CLASSES, "cold-ROM closure per_class")
    destinations: dict[str, int] = {}
    for class_name, raw in per_class.items():
        tally = object_value(raw, f"cold-ROM closure class {class_name}")
        exact_keys(tally, {"destinations", "bytes"}, f"cold-ROM closure class {class_name}")
        destinations[class_name] = require_u64(
            tally["destinations"], f"cold-ROM closure class {class_name}.destinations"
        )
        require_u64(tally["bytes"], f"cold-ROM closure class {class_name}.bytes")
    reasons = validate_count_map(
        scoreboard["per_reason"],
        allowed=DESTINATION_REASONS,
        label="cold-ROM closure per_reason",
        require_all=True,
    )
    total = require_u64(scoreboard["total_destinations"], "cold-ROM closure total")
    unsupported = require_u64(scoreboard["unsupported"], "cold-ROM unsupported count")
    dynamic = require_u64(scoreboard["dynamic_mips"], "cold-ROM dynamic-MIPS count")
    reason_classes = {
        "exact_aot": reasons["in_exact_owner"],
        "block_aot": reasons["in_proven_block"],
        "dynamic_mips": sum(
            reasons[name]
            for name in (
                "open_indirect_site",
                "bounded_indirect_site",
                "mapped_not_proven_code",
                "proven_code_no_owner",
            )
        ),
        "unsupported": reasons["into_proven_data"] + reasons["outside_all_mappings"],
    }
    if (
        sum(destinations.values()) != total
        or sum(reasons.values()) != total
        or destinations != reason_classes
        or unsupported != destinations["unsupported"]
        or dynamic != destinations["dynamic_mips"]
    ):
        raise PanelError("cold-ROM closure totals are internally inconsistent")


def validate_stage1_effects(value: Any) -> None:
    effects = object_value(value, "cold-ROM stage1_effects")
    status = effects.get("status")
    if status == "open":
        exact_keys(effects, {"status", "blocker"}, "cold-ROM open stage1_effects")
        require_enum(
            effects["blocker"],
            {"composition_unavailable", "snapshot_bank_missing", "scan_rejected"},
            "cold-ROM stage1_effects blocker",
        )
        return
    if status != "measured":
        raise PanelError("cold-ROM stage1_effects status has an unsupported value")
    exact_keys(effects, {"status", "summary"}, "cold-ROM measured stage1_effects")
    summary = object_value(effects["summary"], "cold-ROM stage1_effect summary")
    exact_keys(summary, STAGE1_SUMMARY_FIELDS, "cold-ROM stage1_effect summary")
    for field in STAGE1_SUMMARY_FIELDS - {
        "reachable_intrinsic_by_kind",
        "exact_direct_memory_by_region",
    }:
        require_u64(summary[field], f"cold-ROM stage1_effect summary.{field}")
    validate_count_map(
        summary["reachable_intrinsic_by_kind"],
        allowed=INTRINSIC_EFFECT_KINDS,
        label="cold-ROM reachable intrinsic effects",
    )
    validate_count_map(
        summary["exact_direct_memory_by_region"],
        allowed=DIRECT_PHYSICAL_REGIONS,
        label="cold-ROM exact direct-memory regions",
    )


def validate_measurement(measurement: dict[str, Any], expected_digest: str) -> None:
    exact_keys(measurement, MEASUREMENT_FIELDS, "cold-ROM measurement")
    if measurement["schema"] != RECEIPT_SCHEMA:
        raise PanelError("cold-ROM receipt schema mismatch")
    if measurement["limits"] != FIXED_MEASUREMENT_LIMITS:
        raise PanelError("cold-ROM receipt resource envelope differs from schema v2")
    if measurement["normalized_rom_sha256"] != expected_digest:
        raise PanelError("cold-ROM receipt normalized digest mismatch")
    selected = require_enum(
        measurement["selected_strategy"], DISCOVERY_STRATEGIES, "cold-ROM selected strategy"
    )
    validate_strategy_outcomes(measurement["strategy_outcomes"], selected)
    for field in ("fact_count", "overlay_relocation_fact_count", "proven_bank_count"):
        require_u64(measurement[field], f"cold-ROM measurement.{field}")
    if measurement["overlay_relocation_fact_count"] > measurement["fact_count"]:
        raise PanelError("cold-ROM overlay relocation facts exceed all facts")
    validate_closure(measurement["closure"])
    validate_stage1_effects(measurement["stage1_effects"])
    total = require_u64(measurement["ledger_total_bytes"], "cold-ROM ledger total")
    floor = require_u64(
        measurement["ledger_code_like_floor_bytes"], "cold-ROM code-like floor"
    )
    ledger = validate_count_map(
        measurement["ledger_bytes_by_class"],
        allowed=LEDGER_CLASSES,
        label="cold-ROM ledger classes",
    )
    if sum(ledger.values()) != total:
        raise PanelError("cold-ROM ledger classes do not sum to total bytes")
    if floor != ledger.get("code_like", 0):
        raise PanelError("cold-ROM code-like floor disagrees with ledger")


def inspect_regular(path: Path, label: str, limit: int | None) -> os.stat_result:
    if not path.is_absolute() or ".." in path.parts:
        raise PanelError(f"{label} must be an absolute path without '..'")
    try:
        if path.resolve(strict=True) != path:
            raise PanelError(f"{label} must be canonical and contain no symlinks")
        initial = path.lstat()
    except OSError as error:
        raise PanelError(f"cannot inspect {label}") from error
    if not stat.S_ISREG(initial.st_mode):
        raise PanelError(f"{label} must be a regular file")
    if limit is not None and initial.st_size > limit:
        raise PanelError(f"{label} exceeds its {limit}-byte bound")
    return initial


def stable_read(path: Path, label: str, limit: int) -> bytes:
    initial = inspect_regular(path, label, limit)

    flags = os.O_RDONLY
    if hasattr(os, "O_NOFOLLOW"):
        flags |= os.O_NOFOLLOW
    descriptor = -1
    try:
        descriptor = os.open(path, flags)
        opened = os.fstat(descriptor)
        if not stat.S_ISREG(opened.st_mode) or not same_file_snapshot(initial, opened):
            raise PanelError(f"{label} identity changed while opening")
        chunks: list[bytes] = []
        retained = 0
        while retained <= limit:
            chunk = os.read(descriptor, min(1024 * 1024, limit + 1 - retained))
            if not chunk:
                break
            chunks.append(chunk)
            retained += len(chunk)
        if retained > limit:
            raise PanelError(f"{label} exceeds its {limit}-byte bound while reading")
        after_open = os.fstat(descriptor)
        after_path = path.lstat()
        if not same_file_snapshot(opened, after_open) or not same_file_snapshot(
            opened, after_path
        ):
            raise PanelError(f"{label} changed while reading")
        data = b"".join(chunks)
        if len(data) != opened.st_size:
            raise PanelError(f"{label} changed length while reading")
        return data
    except OSError as error:
        raise PanelError(f"cannot read {label}") from error
    finally:
        if descriptor >= 0:
            os.close(descriptor)


def stable_file_sha256(
    path: Path, label: str, limit: int
) -> tuple[str, os.stat_result]:
    initial = inspect_regular(path, label, limit)
    flags = os.O_RDONLY
    if hasattr(os, "O_NOFOLLOW"):
        flags |= os.O_NOFOLLOW
    descriptor = -1
    try:
        descriptor = os.open(path, flags)
        opened = os.fstat(descriptor)
        if not stat.S_ISREG(opened.st_mode) or not same_file_snapshot(initial, opened):
            raise PanelError(f"{label} identity changed while opening")
        digest = hashlib.sha256()
        retained = 0
        while retained <= limit:
            chunk = os.read(descriptor, min(1024 * 1024, limit + 1 - retained))
            if not chunk:
                break
            digest.update(chunk)
            retained += len(chunk)
        if retained > limit:
            raise PanelError(f"{label} exceeds its {limit}-byte bound while hashing")
        after_open = os.fstat(descriptor)
        after_path = path.lstat()
        if not same_file_snapshot(opened, after_open) or not same_file_snapshot(
            opened, after_path
        ):
            raise PanelError(f"{label} changed while hashing")
        if retained != opened.st_size:
            raise PanelError(f"{label} changed length while hashing")
        return digest.hexdigest(), opened
    except OSError as error:
        raise PanelError(f"cannot hash {label}") from error
    finally:
        if descriptor >= 0:
            os.close(descriptor)


def same_file_snapshot(left: os.stat_result, right: os.stat_result) -> bool:
    return (
        left.st_dev == right.st_dev
        and left.st_ino == right.st_ino
        and left.st_size == right.st_size
        and left.st_mtime_ns == right.st_mtime_ns
    )


def validate_executable(path_text: str) -> Path:
    path = Path(path_text)
    inspect_regular(path, "fn64-discover executable", MAX_EXECUTABLE_BYTES)
    if not os.access(path, os.X_OK):
        raise PanelError("fn64-discover executable is not executable")
    return path


def validate_output_destination(path_text: str) -> Path:
    path = Path(path_text)
    if not path.is_absolute() or ".." in path.parts or path.name in ("", ".", ".."):
        raise PanelError("output must be an absolute new file path without '..'")
    parent = path.parent
    try:
        if parent.resolve(strict=True) != parent:
            raise PanelError("output parent must be canonical and contain no symlinks")
        parent_info = parent.lstat()
    except OSError as error:
        raise PanelError("cannot inspect output parent") from error
    if not stat.S_ISDIR(parent_info.st_mode):
        raise PanelError("output parent must be an existing directory")
    try:
        path.lstat()
    except FileNotFoundError:
        return path
    except OSError as error:
        raise PanelError("cannot inspect output destination") from error
    raise PanelError("refusing to overwrite existing output destination")


def encoded_record(record: dict[str, Any]) -> bytes:
    return canonical_sorted(record) + b"\n"


def publish_records(path: Path, records: list[dict[str, Any]]) -> None:
    """Publish complete JSONL through a same-directory no-clobber hard link."""
    # Revalidate immediately before creating a sibling. `link` below is the
    # atomic no-clobber decision if another writer wins after this check.
    validate_output_destination(str(path))
    temporary = path.parent / (
        f".{path.name}.tmp-{os.getpid()}-{secrets.token_hex(8)}"
    )
    descriptor = -1
    published = False
    published_identity: tuple[int, int] | None = None
    publication_complete = False
    try:
        flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL
        if hasattr(os, "O_NOFOLLOW"):
            flags |= os.O_NOFOLLOW
        descriptor = os.open(temporary, flags, 0o600)
        for record in records:
            data = encoded_record(record)
            offset = 0
            while offset < len(data):
                written = os.write(descriptor, data[offset:])
                if written <= 0:
                    raise PanelError("writing durable panel output made no progress")
                offset += written
        os.fsync(descriptor)
        os.link(temporary, path, follow_symlinks=False)
        published = True
        linked = os.fstat(descriptor)
        published_identity = (linked.st_dev, linked.st_ino)
        os.unlink(temporary)
        directory_flags = os.O_RDONLY
        if hasattr(os, "O_DIRECTORY"):
            directory_flags |= os.O_DIRECTORY
        directory = os.open(path.parent, directory_flags)
        try:
            os.fsync(directory)
        finally:
            os.close(directory)
        publication_complete = True
    except FileExistsError as error:
        raise PanelError("refusing to overwrite existing output destination") from error
    except OSError as error:
        raise PanelError(f"publishing durable panel output failed ({error.errno})") from error
    finally:
        if descriptor >= 0:
            os.close(descriptor)
        temporary.unlink(missing_ok=True)
        if published and not publication_complete:
            # A post-link durability failure must not turn a failed command into
            # an apparently valid artifact. Remove only the inode we published.
            try:
                destination = path.lstat()
                if published_identity == (destination.st_dev, destination.st_ino):
                    path.unlink()
            except OSError:
                pass


def parse_manifest(path_text: str) -> list[ManifestEntry]:
    data = stable_read(Path(path_text), "panel manifest", MAX_MANIFEST_BYTES)
    manifest = object_value(decode_json(data, "panel manifest"), "panel manifest")
    exact_keys(manifest, {"schema", "schema_version", "entries"}, "panel manifest")
    if manifest["schema"] != MANIFEST_SCHEMA or manifest["schema_version"] != 1:
        raise PanelError("unsupported panel manifest schema")
    if not isinstance(manifest["entries"], list) or not manifest["entries"]:
        raise PanelError("panel manifest entries must be a nonempty array")
    if len(manifest["entries"]) > MAX_PANEL_ENTRIES:
        raise PanelError(f"panel manifest exceeds its {MAX_PANEL_ENTRIES}-entry bound")

    entries: list[ManifestEntry] = []
    for index, raw in enumerate(manifest["entries"]):
        entry = object_value(raw, f"manifest entry {index + 1}")
        exact_keys(
            entry,
            {"stable_id", "rom_path", "expected_normalized_rom_sha256"},
            f"manifest entry {index + 1}",
        )
        stable_id = require_stable_id(entry["stable_id"])
        if not isinstance(entry["rom_path"], str) or not entry["rom_path"]:
            raise PanelError(f"manifest entry {stable_id} rom_path must be a string")
        rom_path = Path(entry["rom_path"])
        inspect_regular(rom_path, f"manifest entry {stable_id} ROM", MAX_ROM_BYTES)
        entries.append(
            ManifestEntry(
                stable_id,
                rom_path,
                require_digest(
                    entry["expected_normalized_rom_sha256"],
                    f"manifest entry {stable_id} normalized digest",
                ),
            )
        )

    ids = [entry.stable_id for entry in entries]
    if ids != sorted(set(ids)):
        raise PanelError("manifest entries must have unique, sorted stable_id values")
    digests = [entry.expected_normalized_rom_sha256 for entry in entries]
    if len(digests) != len(set(digests)):
        raise PanelError("manifest entries must have unique normalized ROM digests")
    return entries


def command_output(argv: list[str], label: str) -> str:
    try:
        result = subprocess.run(
            argv,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            env={},
            timeout=2,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        raise PanelError(f"resource sampling failed: {label}") from error
    if result.returncode != 0 or len(result.stdout) > MAX_SUBPROCESS_OUTPUT_BYTES:
        raise PanelError(f"resource sampling failed: {label}")
    try:
        return result.stdout.decode("ascii")
    except UnicodeDecodeError as error:
        raise PanelError(f"resource sampling failed: {label}") from error


def free_memory_percent() -> int:
    if sys.platform == "darwin":
        pressure = command_output(["/usr/bin/memory_pressure", "-Q"], "free_memory")
        prefix = "System-wide memory free percentage: "
        values = [
            line[len(prefix) : -1]
            for line in pressure.splitlines()
            if line.startswith(prefix) and line.endswith("%")
        ]
        if len(values) != 1 or not values[0].isascii() or not values[0].isdecimal():
            raise PanelError("resource sampling failed: free_memory")
        percent = int(values[0])
    elif sys.platform.startswith("linux"):
        fields: dict[str, int] = {}
        try:
            for line in Path("/proc/meminfo").read_text().splitlines():
                key, separator, tail = line.partition(":")
                parts = tail.split()
                if separator and len(parts) >= 1 and parts[0].isdecimal():
                    fields[key] = int(parts[0])
        except OSError as error:
            raise PanelError("resource sampling failed: free_memory") from error
        total = fields.get("MemTotal", 0)
        available = fields.get("MemAvailable", 0)
        if total <= 0 or available < 0 or available > total:
            raise PanelError("resource sampling failed: free_memory")
        percent = available * 100 // total
    else:
        raise PanelError("resource sampling failed: unsupported_platform")
    if percent < 0 or percent > 100:
        raise PanelError("resource sampling failed: free_memory")
    return percent


def sample_resources(pgid: int | None) -> tuple[int, int, int]:
    process_table = command_output(["/bin/ps", "-axo", "pid=,pgid=,rss="], "process_table")
    rss_bytes = 0
    members = 0
    for line in process_table.splitlines():
        fields = line.split()
        if len(fields) != 3 or any(
            not field.isascii() or not field.isdecimal() for field in fields
        ):
            raise PanelError("resource sampling failed: process_table")
        _pid, process_pgid, process_rss_kib = (int(field) for field in fields)
        if pgid is not None and process_pgid == pgid:
            members += 1
            rss_bytes += process_rss_kib * 1024
    return rss_bytes, free_memory_percent(), members


def hard_memory_limit(max_rss_bytes: int) -> tuple[str, int | None]:
    """Return the strongest inherited per-process limit available here.

    This is a backstop, not an aggregate-RSS limit. The extra address-space or
    headroom avoids treating loader/runtime mappings as resident heap while
    the process-group RSS watchdog retains the requested threshold. Darwin's
    exposed RSS/address-space rlimits are not enforced, so macOS reports none.
    """
    limit = max_rss_bytes + MEMORY_LIMIT_STARTUP_HEADROOM_BYTES
    if sys.platform.startswith("linux") and hasattr(resource, "RLIMIT_AS"):
        return "rlimit_as_per_process", limit
    return "none", None


def apply_child_resource_limits(
    output_bytes: int, memory_kind: str, memory_bytes: int | None
) -> None:
    resource.setrlimit(resource.RLIMIT_FSIZE, (output_bytes, output_bytes))
    if memory_kind == "rlimit_as_per_process":
        assert memory_bytes is not None
        resource.setrlimit(resource.RLIMIT_AS, (memory_bytes, memory_bytes))


def kill_group(process: subprocess.Popen[bytes]) -> bool:
    if process.pid <= 1 or process.pid == os.getpgrp():
        raise PanelError("refusing to signal a non-isolated process group")
    try:
        os.killpg(process.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    try:
        process.wait(timeout=TEARDOWN_TIMEOUT_SECONDS)
        return True
    except subprocess.TimeoutExpired:
        return False


def run_bounded(
    argv: list[str],
    *,
    stable_id: str,
    scratch: Path,
    timeout_seconds: float,
    max_rss_bytes: int,
    min_free_percent: int,
    poll_milliseconds: int,
) -> ChildResult:
    if timeout_seconds <= 0 or timeout_seconds > 7200:
        raise PanelError("timeout must be greater than zero and at most 7200 seconds")
    if max_rss_bytes <= 0:
        raise PanelError("maximum RSS must be positive")
    if min_free_percent < 0 or min_free_percent > 100:
        raise PanelError("minimum free percentage must be between 0 and 100")
    if poll_milliseconds < 10 or poll_milliseconds > 5000:
        raise PanelError("poll interval must be between 10 and 5000 milliseconds")

    nonce = time.monotonic_ns()
    stdout_path = scratch / f"child-{os.getpid()}-{nonce}.stdout"
    stderr_path = scratch / f"child-{os.getpid()}-{nonce}.stderr"
    stdout_fd = -1
    stderr_fd = -1
    process: subprocess.Popen[bytes] | None = None
    peak_rss_bytes = 0
    started_ns = 0
    termination_attempted = False
    memory_kind, memory_bytes = hard_memory_limit(max_rss_bytes)

    def terminate_child() -> None:
        nonlocal termination_attempted
        termination_attempted = True
        if process is not None and not kill_group(process):
            raise PanelError(f"{stable_id} failed: teardown_timeout")

    try:
        stdout_fd = os.open(stdout_path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        stderr_fd = os.open(stderr_path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        try:
            _rss, free_percent, _members = sample_resources(None)
        except PanelError:
            raise PanelError(f"{stable_id} failed: resource_sampling") from None
        if free_percent < min_free_percent:
            raise PanelError(f"{stable_id} failed: memory_free_floor")
        try:
            process = subprocess.Popen(
                argv,
                stdin=subprocess.DEVNULL,
                stdout=stdout_fd,
                stderr=stderr_fd,
                cwd=scratch,
                env={},
                start_new_session=True,
                preexec_fn=lambda: apply_child_resource_limits(
                    MAX_SUBPROCESS_OUTPUT_BYTES, memory_kind, memory_bytes
                ),
            )
        except (OSError, subprocess.SubprocessError) as error:
            raise PanelError(f"{stable_id} failed: launch") from error

        started_ns = time.monotonic_ns()
        next_sample_ns = started_ns
        poll_ns = poll_milliseconds * 1_000_000
        while True:
            now_ns = time.monotonic_ns()
            if now_ns - started_ns > int(timeout_seconds * 1_000_000_000):
                terminate_child()
                raise PanelError(f"{stable_id} failed: timeout")
            if (
                os.fstat(stdout_fd).st_size > MAX_SUBPROCESS_OUTPUT_BYTES
                or os.fstat(stderr_fd).st_size > MAX_SUBPROCESS_OUTPUT_BYTES
            ):
                terminate_child()
                raise PanelError(f"{stable_id} failed: output_limit")
            if now_ns >= next_sample_ns:
                try:
                    rss_bytes, free_percent, members = sample_resources(process.pid)
                except PanelError:
                    terminate_child()
                    raise PanelError(f"{stable_id} failed: resource_sampling") from None
                peak_rss_bytes = max(peak_rss_bytes, rss_bytes)
                if rss_bytes > max_rss_bytes:
                    terminate_child()
                    raise PanelError(f"{stable_id} failed: memory_rss_limit")
                if free_percent < min_free_percent:
                    terminate_child()
                    raise PanelError(f"{stable_id} failed: memory_free_floor")
                if process.poll() is None and members == 0:
                    terminate_child()
                    raise PanelError(f"{stable_id} failed: resource_sampling")
                next_sample_ns = now_ns + poll_ns

            returncode = process.poll()
            if returncode is not None:
                try:
                    rss_bytes, free_percent, members = sample_resources(process.pid)
                except PanelError:
                    terminate_child()
                    raise PanelError(f"{stable_id} failed: resource_sampling") from None
                peak_rss_bytes = max(peak_rss_bytes, rss_bytes)
                if rss_bytes > max_rss_bytes:
                    terminate_child()
                    raise PanelError(f"{stable_id} failed: memory_rss_limit")
                if free_percent < min_free_percent:
                    terminate_child()
                    raise PanelError(f"{stable_id} failed: memory_free_floor")
                if members != 0:
                    terminate_child()
                    raise PanelError(f"{stable_id} failed: child_survivors")
                if returncode != 0:
                    if (
                        os.fstat(stdout_fd).st_size >= MAX_SUBPROCESS_OUTPUT_BYTES
                        or os.fstat(stderr_fd).st_size >= MAX_SUBPROCESS_OUTPUT_BYTES
                    ):
                        raise PanelError(f"{stable_id} failed: output_limit")
                    raise PanelError(f"{stable_id} failed: child_exit_{returncode}")
                break
            sleep_seconds = min(0.05, max(0.0, (next_sample_ns - time.monotonic_ns()) / 1e9))
            time.sleep(sleep_seconds)

        if (
            os.fstat(stdout_fd).st_size > MAX_SUBPROCESS_OUTPUT_BYTES
            or os.fstat(stderr_fd).st_size > MAX_SUBPROCESS_OUTPUT_BYTES
        ):
            raise PanelError(f"{stable_id} failed: output_limit")
        os.close(stdout_fd)
        stdout_fd = -1
        output = stable_read(stdout_path, "child stdout", MAX_SUBPROCESS_OUTPUT_BYTES)
        wall_ms = (time.monotonic_ns() - started_ns + 999_999) // 1_000_000
        return ChildResult(output, wall_ms, peak_rss_bytes)
    except BaseException:
        if (
            process is not None
            and process.poll() is None
            and not termination_attempted
        ):
            terminate_child()
        raise
    finally:
        if stdout_fd >= 0:
            os.close(stdout_fd)
        if stderr_fd >= 0:
            os.close(stderr_fd)
        stdout_path.unlink(missing_ok=True)
        stderr_path.unlink(missing_ok=True)


def parse_receipt(data: bytes, expected_digest: str) -> dict[str, Any]:
    receipt = object_value(decode_json(data, "cold-ROM receipt"), "cold-ROM receipt")
    exact_keys(receipt, {"measurement", "receipt_sha256"}, "cold-ROM receipt")
    measurement = object_value(receipt["measurement"], "cold-ROM measurement")
    validate_measurement(measurement, expected_digest)
    claimed = require_digest(receipt["receipt_sha256"], "cold-ROM receipt digest")
    actual = hashlib.sha256(rust_struct_json(measurement)).hexdigest()
    if claimed != actual:
        raise PanelError("cold-ROM receipt digest mismatch")
    return receipt


def validate_positive_integer(value: int, label: str, maximum: int) -> int:
    if isinstance(value, bool) or value < 1 or value > maximum:
        raise PanelError(f"{label} must be between 1 and {maximum}")
    return value


def run_panel(args: argparse.Namespace) -> list[dict[str, Any]]:
    entries = parse_manifest(args.manifest)
    executable = validate_executable(args.binary)
    executable_sha256, executable_snapshot = stable_file_sha256(
        executable, "fn64-discover executable", MAX_EXECUTABLE_BYTES
    )
    repetitions = validate_positive_integer(args.repetitions, "repetitions", 100)
    timeout_seconds = validate_positive_integer(args.timeout_seconds, "timeout", 7200)
    max_rss_mib = validate_positive_integer(args.max_rss_mib, "maximum RSS MiB", 1024 * 1024)
    if args.min_free_percent < 0 or args.min_free_percent > 100:
        raise PanelError("minimum free percentage must be between 0 and 100")
    if args.poll_milliseconds < 10 or args.poll_milliseconds > 5000:
        raise PanelError("poll interval must be between 10 and 5000 milliseconds")

    scratch = Path(tempfile.mkdtemp(prefix="fn64-cold-panel-")).resolve()
    scratch.chmod(0o700)
    observations: list[dict[str, Any]] = []
    retained: dict[str, dict[str, Any]] = {}
    panel_walls = [0] * repetitions
    panel_peaks = [0] * repetitions
    try:
        for entry in entries:
            first_receipt_bytes: bytes | None = None
            first_receipt: dict[str, Any] | None = None
            walls: list[int] = []
            peaks: list[int] = []
            for run_index in range(1, repetitions + 1):
                current_executable = inspect_regular(
                    executable, "fn64-discover executable", MAX_EXECUTABLE_BYTES
                )
                if not same_file_snapshot(executable_snapshot, current_executable):
                    raise PanelError("fn64-discover executable changed during panel")
                result = run_bounded(
                    [
                        str(executable),
                        "__cold-rom-child",
                        str(entry.rom_path),
                        entry.expected_normalized_rom_sha256,
                    ],
                    stable_id=entry.stable_id,
                    scratch=scratch,
                    timeout_seconds=timeout_seconds,
                    max_rss_bytes=max_rss_mib * 1024 * 1024,
                    min_free_percent=args.min_free_percent,
                    poll_milliseconds=args.poll_milliseconds,
                )
                current_executable = inspect_regular(
                    executable, "fn64-discover executable", MAX_EXECUTABLE_BYTES
                )
                if not same_file_snapshot(executable_snapshot, current_executable):
                    raise PanelError("fn64-discover executable changed during panel")
                receipt = parse_receipt(result.stdout, entry.expected_normalized_rom_sha256)
                receipt_bytes = canonical_sorted(receipt)
                if first_receipt_bytes is None:
                    first_receipt_bytes = receipt_bytes
                    first_receipt = receipt
                elif receipt_bytes != first_receipt_bytes:
                    raise PanelError(f"{entry.stable_id} failed: nondeterministic_receipt")
                walls.append(result.wall_ms)
                peaks.append(result.peak_rss_bytes)
                panel_walls[run_index - 1] += result.wall_ms
                panel_peaks[run_index - 1] = max(
                    panel_peaks[run_index - 1], result.peak_rss_bytes
                )
                observations.append(
                    {
                        "schema": OBSERVATION_SCHEMA,
                        "schema_version": 1,
                        "stable_id": entry.stable_id,
                        "normalized_rom_sha256": entry.expected_normalized_rom_sha256,
                        "run_index": run_index,
                        "wall_ms": result.wall_ms,
                        "peak_rss_bytes": result.peak_rss_bytes,
                        "receipt_sha256": receipt["receipt_sha256"],
                    }
                )
            assert first_receipt is not None
            retained[entry.stable_id] = {
                "stable_id": entry.stable_id,
                "normalized_rom_sha256": entry.expected_normalized_rom_sha256,
                "receipt_sha256": first_receipt["receipt_sha256"],
                "receipt": first_receipt,
                "wall_ms_distribution": sorted(walls),
                "peak_rss_bytes_distribution": sorted(peaks),
            }
    finally:
        shutil.rmtree(scratch, ignore_errors=True)

    final_executable_sha256, final_executable_snapshot = stable_file_sha256(
        executable, "fn64-discover executable", MAX_EXECUTABLE_BYTES
    )
    if (
        final_executable_sha256 != executable_sha256
        or not same_file_snapshot(executable_snapshot, final_executable_snapshot)
    ):
        raise PanelError("fn64-discover executable changed during panel")

    deterministic = {
        "schema": "fn64.cold-coverage-panel-deterministic.v1",
        "schema_version": 1,
        "repetitions": repetitions,
        "fn64_discover_sha256": executable_sha256,
        "entries": [
            {
                "stable_id": retained[entry.stable_id]["stable_id"],
                "normalized_rom_sha256": retained[entry.stable_id][
                    "normalized_rom_sha256"
                ],
                "receipt_sha256": retained[entry.stable_id]["receipt_sha256"],
                "receipt": retained[entry.stable_id]["receipt"],
            }
            for entry in entries
        ],
    }
    memory_kind, memory_bytes = hard_memory_limit(max_rss_mib * 1024 * 1024)
    final = {
        "schema": RESULT_SCHEMA,
        "schema_version": 1,
        "repetitions": repetitions,
        "entry_count": len(entries),
        "fn64_discover_sha256": executable_sha256,
        "panel_sha256": hashlib.sha256(canonical_sorted(deterministic)).hexdigest(),
        "subprocess_limits": {
            "timeout_seconds": timeout_seconds,
            "sampled_process_group_rss_threshold_bytes": max_rss_mib * 1024 * 1024,
            "rss_enforcement": "sampled_process_group_watchdog",
            "hard_memory_enforcement": memory_kind,
            "hard_memory_limit_per_process_bytes": memory_bytes,
            "min_system_free_percent": args.min_free_percent,
            "poll_milliseconds": args.poll_milliseconds,
            "max_stdout_bytes": MAX_SUBPROCESS_OUTPUT_BYTES,
            "max_stderr_bytes": MAX_SUBPROCESS_OUTPUT_BYTES,
            "output_enforcement": "rlimit_fsize",
            "teardown_timeout_seconds": TEARDOWN_TIMEOUT_SECONDS,
        },
        "entries": [retained[entry.stable_id] for entry in entries],
        "aggregate_wall_ms_distribution": sorted(panel_walls),
        "aggregate_peak_rss_bytes_distribution": sorted(panel_peaks),
    }
    return [*observations, final]


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(description=__doc__)
    result.add_argument("--manifest", required=True)
    result.add_argument("--binary", required=True)
    result.add_argument("--output")
    result.add_argument("--repetitions", type=int, default=DEFAULT_REPETITIONS)
    result.add_argument("--timeout-seconds", type=int, default=DEFAULT_TIMEOUT_SECONDS)
    result.add_argument("--max-rss-mib", type=int, default=DEFAULT_MAX_RSS_MIB)
    result.add_argument("--min-free-percent", type=int, default=DEFAULT_MIN_FREE_PERCENT)
    result.add_argument("--poll-milliseconds", type=int, default=DEFAULT_POLL_MILLISECONDS)
    return result


def main() -> int:
    try:
        args = parser().parse_args()
        output_path = validate_output_destination(args.output) if args.output else None
        records = run_panel(args)
        # No successful byte reaches stdout until the complete panel is sealed.
        if output_path is None:
            for record in records:
                sys.stdout.buffer.write(encoded_record(record))
        else:
            publish_records(output_path, records)
        return 0
    except PanelError as error:
        print(f"cold-coverage-panel: {error}", file=sys.stderr)
        return 1
    except OSError as error:
        print(f"cold-coverage-panel: operating-system error ({error.errno})", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
