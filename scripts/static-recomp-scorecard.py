#!/usr/bin/env python3
"""Aggregate static-recompilation receipts without promoting their authority.

This script does no discovery and loads no capability.  It only checks and
summarizes explicitly named JSON artifacts emitted by the existing gates.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
from pathlib import Path
from typing import Any, NoReturn


CLOSURE_SCHEMA = "fn64.execution-closure-audit.v3"
SOURCE_SCHEMA = "fn64.executable-source-frontier.v1"
WRITER_SCHEMA = "fn64.executable-writer-channel-denominator.v2"
WRITER_AUDIT_SCHEMA = "fn64.wm-selected-build-writer-audit.v3"
WRITER_AUDIT_BUNDLE_SCHEMA = "fn64.verified-generated-runner-writer-audit-bundle.v1"
VERIFIED_BUILD_SCHEMA = "fn64.verified-generated-runner-build.v5"
SCORECARD_SCHEMA = "fn64.static-recomp-scorecard.v1"

CLASSES = ("exact_aot", "block_aot", "dynamic_mips", "unsupported")
CHANNELS = (
    "cpu_instruction_store",
    "pi_dma",
    "si_dma",
    "sp_dma",
    "rsp_execution_or_hle_writeback",
    "rdp_renderer",
    "host_abi",
    "bootstrap_or_import",
)

CLOSURE_KEYS = {
    "schema",
    "normalized_rom_sha256",
    "snapshot_schema_versions",
    "classification_authority",
    "authorities_not_consulted",
    "composed_bank_inputs",
    "proven_mapping_geometry",
    "scoreboard",
    "dynamic_concrete",
    "dynamic_indirect",
    "unsupported",
}
SOURCE_KEYS = {
    "schema",
    "producer",
    "normalized_rom_sha256",
    "dense_aot_pack_sha256",
    "initial_cop0_status",
    "dense_generations",
    "external_images",
    "exception_vectors",
    "host_bindings",
    "cache_sites",
    "direct_dma_findings",
    "direct_dma_blockers",
    "raw_pi_primitives",
    "cpu_store_watched_destinations",
    "cpu_store_scans",
    "cop0_status_scans",
    "external_cop0_status_scans",
    "conditional_cpu_word_stores",
    "open_cpu_word_stores",
    "transfer_scan",
    "open_writer_classes",
}
TRANSFER_KEYS_REQUIRED = {
    "coverage",
    "summary",
    "inventory",
    "direct",
    "indirect_frontier",
    "blockers",
}
TRANSFER_KEYS_OPTIONAL = {"catalog_guarded", "catalog_total_authority"}
TRANSFER_SUMMARY_KEYS = {
    "direct_total",
    "direct_guest",
    "direct_host",
    "direct_open",
    "indirect_closed",
    "indirect_bounded",
    "indirect_open",
}
WRITER_KEYS = {"schema", "producer", "program_model_sha256", "channels"}
WRITER_AUDIT_KEYS = {
    "schema",
    "exact_runs_per_channel",
    "channel_count",
    "completed_channel_bitmap",
    "build_schema",
    "build_authority_sha256",
    "selected_binary_sha256",
    "private_build_inputs_sha256",
    "cargo_graph_sha256",
    "cargo_source_sha256",
    "build_environment_sha256",
    "builder_cargo_sha256",
    "builder_rustc_sha256",
    "cargo_config_sha256",
    "memory_guard_sha256",
    "selected_build_cargo_jobs",
    "build_max_rss_mib",
    "build_min_free_percent",
    "program_identity_sha256",
    "normalized_rom_sha256",
    "manifest_sha256",
    "lock_sha256",
    "root_adapter_source_sha256",
    "shard_cargo_source_tree_sha256",
    "emitter_source_sha256",
    "runtime_source_sha256",
    "prepared_tree_sha256",
    "producer_cargo_source_sha256",
    "producer_binary_sha256",
    "bundle_schema",
    "bundle_authority_sha256",
    "program_model_sha256",
    "writer_denominator_sha256",
}


class InvalidReceipt(ValueError):
    pass


def fail(message: str) -> NoReturn:
    raise InvalidReceipt(message)


def exact_keys(value: Any, expected: set[str], where: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        fail(f"{where}: expected object")
    actual = set(value)
    if actual != expected:
        fail(
            f"{where}: fields differ: missing={sorted(expected - actual)} "
            f"unexpected={sorted(actual - expected)}"
        )
    return value


def keys_with_optional(
    value: Any, required: set[str], optional: set[str], where: str
) -> dict[str, Any]:
    if not isinstance(value, dict):
        fail(f"{where}: expected object")
    actual = set(value)
    missing = required - actual
    unexpected = actual - required - optional
    if missing or unexpected:
        fail(
            f"{where}: fields differ: missing={sorted(missing)} "
            f"unexpected={sorted(unexpected)}"
        )
    return value


def require_list(value: Any, where: str) -> list[Any]:
    if not isinstance(value, list):
        fail(f"{where}: expected array")
    return value


def require_string(value: Any, where: str, *, nonempty: bool = True) -> str:
    if not isinstance(value, str) or (nonempty and not value.strip()):
        fail(f"{where}: expected nonempty string")
    return value


def require_uint(value: Any, where: str) -> int:
    if not isinstance(value, int) or isinstance(value, bool) or value < 0:
        fail(f"{where}: expected unsigned integer")
    return value


def require_sha256(value: Any, where: str) -> str:
    text = require_string(value, where)
    if len(text) != 64 or any(character not in "0123456789abcdef" for character in text):
        fail(f"{where}: expected lowercase SHA-256")
    return text


def load_receipt(path: Path, where: str) -> tuple[dict[str, Any], str, bytes]:
    try:
        raw = path.read_bytes()
    except OSError as error:
        fail(f"{where}: reading {path}: {error}")
    try:
        value = json.loads(raw)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        fail(f"{where}: invalid JSON in {path}: {error}")
    if not isinstance(value, dict):
        fail(f"{where}: top level must be an object")
    return value, hashlib.sha256(raw).hexdigest(), raw


def validate_closure(path: Path, label: str) -> dict[str, Any]:
    receipt, receipt_sha, _ = load_receipt(path, "closure audit")
    exact_keys(receipt, CLOSURE_KEYS, "closure audit")
    if receipt["schema"] != CLOSURE_SCHEMA:
        fail(f"closure audit: unsupported schema {receipt['schema']!r}")
    rom_sha = require_sha256(receipt["normalized_rom_sha256"], "closure ROM digest")
    versions = require_list(receipt["snapshot_schema_versions"], "snapshot versions")
    if not versions or any(require_uint(item, "snapshot version") == 0 for item in versions):
        fail("closure audit: snapshot versions must be nonempty positive integers")
    if versions != sorted(set(versions)):
        fail("closure audit: snapshot versions are not canonical")
    if label == "current" and versions != [5]:
        fail("closure audit: current label requires snapshot schema v5 only")
    require_string(receipt["classification_authority"], "classification authority")
    omitted = require_list(receipt["authorities_not_consulted"], "omitted authorities")
    if len(omitted) != 4 or any(not isinstance(item, str) or not item for item in omitted):
        fail("closure audit: expected four named authorities not consulted")
    bank_inputs = require_list(receipt["composed_bank_inputs"], "composed bank inputs")
    for index, bank in enumerate(bank_inputs):
        bank = exact_keys(
            bank,
            {"bank", "va_start", "va_end", "rom_space", "rom_start", "rom_end", "bytes_sha256"},
            f"composed bank[{index}]",
        )
        require_string(bank["bank"], f"composed bank[{index}] name")
        require_sha256(bank["bytes_sha256"], f"composed bank[{index}] digest")
        for key in ("va_start", "va_end", "rom_start", "rom_end"):
            require_uint(bank[key], f"composed bank[{index}] {key}")
    mappings = require_list(receipt["proven_mapping_geometry"], "mapping geometry")
    for index, mapping in enumerate(mappings):
        mapping = exact_keys(
            mapping,
            {"bank", "rom_space", "rom_start", "rom_end", "va_start", "va_end"},
            f"mapping[{index}]",
        )
        require_string(mapping["bank"], f"mapping[{index}] bank")
        for key in ("rom_start", "rom_end", "va_start", "va_end"):
            require_uint(mapping[key], f"mapping[{index}] {key}")

    board = exact_keys(
        receipt["scoreboard"],
        {"total_destinations", "per_class", "per_reason", "unsupported", "dynamic_mips"},
        "closure scoreboard",
    )
    total_destinations = require_uint(board["total_destinations"], "total destinations")
    per_class = board["per_class"]
    if not isinstance(per_class, dict) or not set(per_class).issubset(CLASSES):
        fail("closure scoreboard: invalid per_class map")
    tallies: dict[str, dict[str, int]] = {}
    for class_name in CLASSES:
        value = per_class.get(class_name, {"destinations": 0, "bytes": 0})
        exact_keys(value, {"destinations", "bytes"}, f"closure class {class_name}")
        destinations = require_uint(value["destinations"], f"{class_name} destinations")
        byte_count = require_uint(value["bytes"], f"{class_name} bytes")
        if byte_count % 4:
            fail(f"closure class {class_name}: byte count is not word aligned")
        tallies[class_name] = {"destinations": destinations, "bytes": byte_count}
    if sum(row["destinations"] for row in tallies.values()) != total_destinations:
        fail("closure scoreboard: per-class destinations do not sum to total")
    unsupported_count = require_uint(board["unsupported"], "unsupported headline")
    dynamic_count = require_uint(board["dynamic_mips"], "dynamic_mips headline")
    if unsupported_count != tallies["unsupported"]["destinations"]:
        fail("closure scoreboard: unsupported headline disagrees with class tally")
    if dynamic_count != tallies["dynamic_mips"]["destinations"]:
        fail("closure scoreboard: dynamic_mips headline disagrees with class tally")
    if not isinstance(board["per_reason"], dict) or any(
        not isinstance(key, str) or require_uint(value, f"reason {key}") < 0
        for key, value in board["per_reason"].items()
    ):
        fail("closure scoreboard: invalid reason histogram")

    def validate_incoming(value: Any, where: str) -> None:
        incoming = require_list(value, f"{where} incoming")
        if not incoming:
            fail(f"{where}: incoming edges must be nonempty")
        for edge_index, edge in enumerate(incoming):
            edge = exact_keys(
                edge,
                {"bank", "block_start_va", "block_end_va", "source_site_va", "kind"},
                f"{where} incoming[{edge_index}]",
            )
            require_string(edge["bank"], f"{where} incoming[{edge_index}] bank")
            for key in ("block_start_va", "block_end_va", "source_site_va"):
                require_uint(edge[key], f"{where} incoming[{edge_index}] {key}")
            if not isinstance(edge["kind"], (str, dict)):
                fail(f"{where} incoming[{edge_index}] kind: invalid enum shape")

    dynamic_concrete = require_list(receipt["dynamic_concrete"], "dynamic concrete")
    dynamic_addresses: set[int] = set()
    dynamic_reasons: dict[str, int] = {}
    for index, item in enumerate(dynamic_concrete):
        item = exact_keys(
            item,
            {"destination_va", "reason", "incoming", "block_proof", "owner_proof"},
            f"dynamic_concrete[{index}]",
        )
        address = require_uint(item["destination_va"], f"dynamic_concrete[{index}] address")
        if address in dynamic_addresses:
            fail("closure audit: duplicate dynamic concrete destination")
        dynamic_addresses.add(address)
        reason = item["reason"]
        if reason not in ("mapped_not_proven_code", "proven_code_no_owner"):
            fail(f"dynamic_concrete[{index}]: invalid concrete dynamic reason")
        dynamic_reasons[reason] = dynamic_reasons.get(reason, 0) + 1
        validate_incoming(item["incoming"], f"dynamic_concrete[{index}]")
        block_proof = require_list(item["block_proof"], f"dynamic_concrete[{index}] block proof")
        if reason == "proven_code_no_owner" and not block_proof:
            fail(f"dynamic_concrete[{index}]: proven code lacks block-proof metadata")
        for proof_index, proof in enumerate(block_proof):
            proof = exact_keys(
                proof,
                {"bank", "block_start_va", "block_end_va", "blocker_kinds"},
                f"dynamic_concrete[{index}] block_proof[{proof_index}]",
            )
            require_string(proof["bank"], f"dynamic_concrete[{index}] block proof bank")
            require_uint(proof["block_start_va"], f"dynamic_concrete[{index}] block start")
            require_uint(proof["block_end_va"], f"dynamic_concrete[{index}] block end")
            blocker_kinds = require_list(
                proof["blocker_kinds"], f"dynamic_concrete[{index}] block blocker kinds"
            )
            if not blocker_kinds or any(not isinstance(kind, str) or not kind for kind in blocker_kinds):
                fail(f"dynamic_concrete[{index}]: invalid block blocker kinds")
        owner_proof = require_list(item["owner_proof"], f"dynamic_concrete[{index}] owner proof")
        for proof_index, proof in enumerate(owner_proof):
            proof = exact_keys(
                proof,
                {"bank", "entry_va", "state", "proposed_va_end", "blocker_kinds"},
                f"dynamic_concrete[{index}] owner_proof[{proof_index}]",
            )
            require_string(proof["bank"], f"dynamic_concrete[{index}] owner proof bank")
            require_uint(proof["entry_va"], f"dynamic_concrete[{index}] owner entry")
            if proof["state"] not in ("candidate", "ambiguous"):
                fail(f"dynamic_concrete[{index}]: invalid owner assessment state")
            if proof["proposed_va_end"] is not None:
                require_uint(proof["proposed_va_end"], f"dynamic_concrete[{index}] owner end")
            blocker_kinds = require_list(
                proof["blocker_kinds"], f"dynamic_concrete[{index}] owner blocker kinds"
            )
            if not blocker_kinds or any(not isinstance(kind, str) or not kind for kind in blocker_kinds):
                fail(f"dynamic_concrete[{index}]: invalid owner blocker kinds")

    dynamic_indirect = require_list(receipt["dynamic_indirect"], "dynamic indirect")
    prior_indirect_key: tuple[str, int, bool] | None = None
    for index, item in enumerate(dynamic_indirect):
        item = exact_keys(
            item,
            {"bank", "site_pc", "via_call", "state", "kind", "targets", "memory_sources"},
            f"dynamic_indirect[{index}]",
        )
        bank = require_string(item["bank"], f"dynamic_indirect[{index}] bank")
        site_pc = require_uint(item["site_pc"], f"dynamic_indirect[{index}] site")
        if not isinstance(item["via_call"], bool):
            fail(f"dynamic_indirect[{index}]: via_call must be boolean")
        key = (bank, site_pc, item["via_call"])
        if prior_indirect_key is not None and key < prior_indirect_key:
            fail("closure audit: dynamic indirect sites are not canonical")
        prior_indirect_key = key
        if item["state"] not in ("Open", "Bounded"):
            fail(f"dynamic_indirect[{index}]: invalid dynamic proof state")
        expected_reason = "open_indirect_site" if item["state"] == "Open" else "bounded_indirect_site"
        dynamic_reasons[expected_reason] = dynamic_reasons.get(expected_reason, 0) + 1
        if item["kind"] not in (None, "Constant", "MemoryValueSet", "JumpTable"):
            fail(f"dynamic_indirect[{index}]: invalid resolution kind")
        for field in ("targets", "memory_sources"):
            values = require_list(item[field], f"dynamic_indirect[{index}] {field}")
            if any(not isinstance(value, int) or isinstance(value, bool) or value < 0 for value in values):
                fail(f"dynamic_indirect[{index}]: invalid {field}")
            if values != sorted(set(values)):
                fail(f"dynamic_indirect[{index}]: {field} is not canonical")

    if len(dynamic_concrete) * 4 != tallies["dynamic_mips"]["bytes"]:
        fail("closure audit: dynamic concrete list disagrees with dynamic byte tally")
    if len(dynamic_concrete) + len(dynamic_indirect) != dynamic_count:
        fail("closure audit: dynamic detail lists disagree with headline")
    for reason, count in dynamic_reasons.items():
        if board["per_reason"].get(reason, 0) != count:
            fail(f"closure audit: dynamic {reason} details disagree with reason histogram")

    unsupported = require_list(receipt["unsupported"], "unsupported destinations")
    if len(unsupported) != unsupported_count:
        fail("closure audit: unsupported address list disagrees with headline")
    addresses: set[int] = set()
    for index, item in enumerate(unsupported):
        item = exact_keys(
            item, {"destination_va", "reason", "incoming"}, f"unsupported[{index}]"
        )
        address = require_uint(item["destination_va"], f"unsupported[{index}] address")
        if address in addresses:
            fail("closure audit: duplicate unsupported destination")
        addresses.add(address)
        if item["reason"] not in ("into_proven_data", "outside_all_mappings"):
            fail(f"unsupported[{index}]: invalid unsupported reason")
        validate_incoming(item["incoming"], f"unsupported[{index}]")

    concrete_bytes = sum(row["bytes"] for row in tallies.values())
    aot_bytes = tallies["exact_aot"]["bytes"] + tallies["block_aot"]["bytes"]
    return {
        "receipt_sha256": receipt_sha,
        "normalized_rom_sha256": rom_sha,
        "snapshot_schema_versions": versions,
        "classification_authority": receipt["classification_authority"],
        "authorities_not_consulted": omitted,
        "total_destinations": total_destinations,
        "per_class": tallies,
        "concrete_destination_bytes": concrete_bytes,
        "aot_concrete_destination_bytes": aot_bytes,
        "aot_percent_of_concrete_destination_bytes": (
            round(100.0 * aot_bytes / concrete_bytes, 6) if concrete_bytes else None
        ),
        "unsupported_zero_required": unsupported_count,
        "dynamic_mips_zero_required_for_pure_static": dynamic_count,
    }


def validate_source(path: Path) -> dict[str, Any]:
    receipt, receipt_sha, _ = load_receipt(path, "source frontier")
    exact_keys(receipt, SOURCE_KEYS, "source frontier")
    if receipt["schema"] != SOURCE_SCHEMA:
        fail(f"source frontier: unsupported schema {receipt['schema']!r}")
    require_string(receipt["producer"], "source producer")
    rom_sha = require_sha256(receipt["normalized_rom_sha256"], "source ROM digest")
    pack_sha = require_sha256(receipt["dense_aot_pack_sha256"], "dense pack digest")
    for key in SOURCE_KEYS - {
        "schema",
        "producer",
        "normalized_rom_sha256",
        "dense_aot_pack_sha256",
        "initial_cop0_status",
        "transfer_scan",
    }:
        require_list(receipt[key], f"source {key}")
    initial_status = receipt["initial_cop0_status"]
    if not isinstance(initial_status, dict) or initial_status.get("authority") not in (
        "missing",
        "boot_context",
    ):
        fail("source initial_cop0_status: invalid tagged authority")

    transfer = keys_with_optional(
        receipt["transfer_scan"],
        TRANSFER_KEYS_REQUIRED,
        TRANSFER_KEYS_OPTIONAL,
        "transfer scan",
    )
    if transfer["inventory"] not in ("complete", "open"):
        fail("transfer scan: inventory must be complete or open")
    summary = exact_keys(transfer["summary"], TRANSFER_SUMMARY_KEYS, "transfer summary")
    summary = {key: require_uint(value, f"transfer summary {key}") for key, value in summary.items()}
    if summary["direct_guest"] + summary["direct_host"] + summary["direct_open"] != summary["direct_total"]:
        fail("transfer summary: direct dispositions do not sum to direct_total")
    for key in ("direct", "indirect_frontier", "blockers"):
        require_list(transfer[key], f"transfer scan {key}")
    if "catalog_guarded" in transfer:
        require_list(transfer["catalog_guarded"], "transfer scan catalog_guarded")

    exception_vectors = require_list(receipt["exception_vectors"], "exception vectors")
    open_vectors = 0
    for index, vector in enumerate(exception_vectors):
        vector = exact_keys(vector, {"destination", "disposition"}, f"exception vector[{index}]")
        require_uint(vector["destination"], f"exception vector[{index}] destination")
        disposition = vector["disposition"]
        if isinstance(disposition, str):
            if disposition != "bev_clear_invariant":
                fail(f"exception vector[{index}]: invalid unit disposition")
        elif isinstance(disposition, dict) and len(disposition) == 1:
            variant = next(iter(disposition))
            if variant not in (
                "exact_code_owner",
                "machine_checked_unreachability",
                "open",
            ):
                fail(f"exception vector[{index}]: invalid disposition variant")
            if not isinstance(disposition[variant], dict):
                fail(f"exception vector[{index}]: invalid disposition payload")
            open_vectors += variant == "open"
        else:
            fail(f"exception vector[{index}]: invalid disposition shape")
    cache_sites = require_list(receipt["cache_sites"], "cache sites")
    unclassified_cache = 0
    for index, site in enumerate(cache_sites):
        site = exact_keys(
            site,
            {
                "bank",
                "guest_pc",
                "raw_word",
                "decoded_op",
                "base_register",
                "offset",
                "word_class",
                "disposition",
                "evidence",
            },
            f"cache site[{index}]",
        )
        if site["disposition"] not in ("reachable_instruction", "proven_data", "unclassified"):
            fail(f"cache site[{index}]: invalid disposition")
        unclassified_cache += site["disposition"] == "unclassified"
    explicit_open = any(
        (
            transfer["inventory"] == "open",
            initial_status["authority"] == "missing",
            summary["direct_open"] != 0,
            summary["indirect_bounded"] != 0,
            summary["indirect_open"] != 0,
            bool(transfer["blockers"]),
            open_vectors != 0,
            bool(receipt["open_writer_classes"]),
            bool(receipt["direct_dma_blockers"]),
            bool(receipt["conditional_cpu_word_stores"]),
            bool(receipt["open_cpu_word_stores"]),
            unclassified_cache != 0,
        )
    )
    return {
        "receipt_sha256": receipt_sha,
        "normalized_rom_sha256": rom_sha,
        "dense_aot_pack_sha256": pack_sha,
        "transfer_inventory": transfer["inventory"],
        "transfer_summary": summary,
        "transfer_blockers": len(transfer["blockers"]),
        "open_exception_vectors": open_vectors,
        "open_writer_classes": len(receipt["open_writer_classes"]),
        "unclassified_cache_sites": unclassified_cache,
        "direct_dma_blockers": len(receipt["direct_dma_blockers"]),
        "conditional_cpu_word_stores": len(receipt["conditional_cpu_word_stores"]),
        "open_cpu_word_stores": len(receipt["open_cpu_word_stores"]),
        "known_open_findings": explicit_open,
        "catalog_total_authority_claimed": False,
        "note": "schema v1 is inventory and exposes no exhaustiveness authority",
    }


def validate_writer(path: Path) -> dict[str, Any]:
    receipt, receipt_sha, raw = load_receipt(path, "writer denominator")
    exact_keys(receipt, WRITER_KEYS, "writer denominator")
    if receipt["schema"] != WRITER_SCHEMA:
        fail(f"writer denominator: unsupported schema {receipt['schema']!r}")
    require_string(receipt["producer"], "writer producer")
    model_sha = require_sha256(receipt["program_model_sha256"], "writer program model")
    rows = require_list(receipt["channels"], "writer channels")
    if len(rows) != len(CHANNELS):
        fail("writer denominator: expected exactly eight channels")
    states: dict[str, str] = {}
    blockers: dict[str, int] = {}
    bundle_rows: dict[str, dict[str, str]] = {}
    for index, row in enumerate(rows):
        if not isinstance(row, dict):
            fail(f"writer channel[{index}]: expected object")
        if set(row) not in ({"channel", "state", "blockers"}, {"channel", "state", "receipt"}):
            fail(f"writer channel[{index}]: invalid state shape")
        channel = row.get("channel")
        if channel not in CHANNELS or channel in states:
            fail(f"writer channel[{index}]: unknown or duplicate channel {channel!r}")
        state = row.get("state")
        if state == "open" and set(row) == {"channel", "state", "blockers"}:
            entries = require_list(row["blockers"], f"writer channel {channel} blockers")
            if not entries:
                fail(f"writer channel {channel}: open state requires blockers")
            for blocker in entries:
                exact_keys(blocker, {"code", "evidence"}, f"writer channel {channel} blocker")
                require_string(blocker["code"], f"writer channel {channel} blocker code")
                require_string(blocker["evidence"], f"writer channel {channel} evidence")
            blockers[channel] = len(entries)
        elif state == "complete" and set(row) == {"channel", "state", "receipt"}:
            evidence = exact_keys(
                row["receipt"], {"validator", "receipt"}, f"writer channel {channel} receipt"
            )
            validator = evidence["validator"]
            if validator == channel and validator not in (
                "si_dma",
                "sp_dma",
                "host_abi",
                "rdp_renderer",
                "rsp_execution_or_hle_writeback",
            ):
                inner = exact_keys(
                    evidence["receipt"],
                    {"validator_schema", "evidence_sha256"},
                    f"writer channel {channel} receipt evidence",
                )
                require_string(
                    inner["validator_schema"], f"writer channel {channel} validator schema"
                )
                require_sha256(inner["evidence_sha256"], f"writer channel {channel} evidence digest")
            elif validator in ("si_dma", "sp_dma"):
                expected_channel = validator
                if channel != expected_channel:
                    fail(f"writer channel {channel}: {validator} receipt is for {expected_channel}")
                inner = exact_keys(
                    evidence["receipt"],
                    {"validator_schema", "series_authority_sha256"},
                    f"writer channel {channel} series receipt evidence",
                )
                require_string(
                    inner["validator_schema"], f"writer channel {channel} validator schema"
                )
                require_sha256(
                    inner["series_authority_sha256"],
                    f"writer channel {channel} series authority digest",
                )
            elif validator == "writer_audit_bundle":
                if channel not in (
                    "bootstrap_or_import",
                    "cpu_instruction_store",
                    "host_abi",
                    "pi_dma",
                    "rdp_renderer",
                    "rsp_execution_or_hle_writeback",
                    "si_dma",
                    "sp_dma",
                ):
                    fail(f"writer channel {channel}: writer-audit bundle cannot complete this channel")
                inner = exact_keys(
                    evidence["receipt"],
                    {
                        "bundle_validator_schema",
                        "bundle_authority_sha256",
                        "channel_series_authority_sha256",
                    },
                    f"writer channel {channel} bundle receipt evidence",
                )
                require_string(
                    inner["bundle_validator_schema"],
                    f"writer channel {channel} bundle validator schema",
                )
                require_sha256(
                    inner["bundle_authority_sha256"],
                    f"writer channel {channel} bundle authority digest",
                )
                require_sha256(
                    inner["channel_series_authority_sha256"],
                    f"writer channel {channel} channel-series authority digest",
                )
                bundle_rows[channel] = inner
            else:
                fail(f"writer channel {channel}: unknown validator variant {validator!r}")
            blockers[channel] = 0
        else:
            fail(f"writer channel {channel}: state and fields disagree")
        states[channel] = state
    if set(states) != set(CHANNELS):
        fail("writer denominator: fixed channel set is incomplete")
    complete = sum(state == "complete" for state in states.values())
    bundle_schemas = {row["bundle_validator_schema"] for row in bundle_rows.values()}
    bundle_authorities = {row["bundle_authority_sha256"] for row in bundle_rows.values()}
    if len(bundle_schemas) > 1 or len(bundle_authorities) > 1:
        fail("writer denominator: writer-audit rows do not share one bundle")
    return {
        "receipt_sha256": receipt_sha,
        "published_payload_sha256": hashlib.sha256(
            raw[:-1] if raw.endswith(b"\n") else raw
        ).hexdigest(),
        "program_model_sha256": model_sha,
        "complete_channels": complete,
        "open_channels": len(CHANNELS) - complete,
        "writer_audit_bundle_channels": len(bundle_rows),
        "writer_audit_bundle_schema": next(iter(bundle_schemas), None),
        "writer_audit_bundle_authority_sha256": next(iter(bundle_authorities), None),
        "states": {channel: states[channel] for channel in CHANNELS},
        "blocker_counts": {channel: blockers[channel] for channel in CHANNELS},
    }


def validate_writer_audit(path: Path) -> dict[str, Any]:
    receipt, receipt_sha, _ = load_receipt(path, "writer audit")
    exact_keys(receipt, WRITER_AUDIT_KEYS, "writer audit")
    if receipt["schema"] != WRITER_AUDIT_SCHEMA:
        fail(f"writer audit: unsupported schema {receipt['schema']!r}")
    exact_values = {
        "exact_runs_per_channel": 10,
        "channel_count": 8,
        "completed_channel_bitmap": 255,
        "selected_build_cargo_jobs": 2,
        "build_max_rss_mib": 4096,
        "build_min_free_percent": 40,
    }
    for field, expected in exact_values.items():
        actual = require_uint(receipt[field], f"writer audit {field}")
        if actual != expected:
            fail(f"writer audit: {field} must be exactly {expected}, got {actual}")
    if receipt["build_schema"] != VERIFIED_BUILD_SCHEMA:
        fail(f"writer audit: unsupported build schema {receipt['build_schema']!r}")
    if receipt["bundle_schema"] != WRITER_AUDIT_BUNDLE_SCHEMA:
        fail(f"writer audit: unsupported bundle schema {receipt['bundle_schema']!r}")
    digest_fields = WRITER_AUDIT_KEYS - {
        "schema",
        "exact_runs_per_channel",
        "channel_count",
        "completed_channel_bitmap",
        "build_schema",
        "selected_build_cargo_jobs",
        "build_max_rss_mib",
        "build_min_free_percent",
        "bundle_schema",
    }
    for field in digest_fields:
        require_sha256(receipt[field], f"writer audit {field}")
    return {
        "receipt_sha256": receipt_sha,
        "schema": receipt["schema"],
        "exact_runs_per_channel": receipt["exact_runs_per_channel"],
        "channel_count": receipt["channel_count"],
        "completed_channel_bitmap": receipt["completed_channel_bitmap"],
        "selected_build_cargo_jobs": receipt["selected_build_cargo_jobs"],
        "build_max_rss_mib": receipt["build_max_rss_mib"],
        "build_min_free_percent": receipt["build_min_free_percent"],
        "normalized_rom_sha256": receipt["normalized_rom_sha256"],
        "program_identity_sha256": receipt["program_identity_sha256"],
        "program_model_sha256": receipt["program_model_sha256"],
        "bundle_schema": receipt["bundle_schema"],
        "bundle_authority_sha256": receipt["bundle_authority_sha256"],
        "writer_denominator_sha256": receipt["writer_denominator_sha256"],
    }


def render_text(scorecard: dict[str, Any]) -> str:
    closure = scorecard["closure"]
    source = scorecard["source_frontier"]
    writer = scorecard["writer_denominator"]
    return "\n".join(
        (
            f"evidence={scorecard['evidence_label']} authority=diagnostic_only",
            f"transfer_closure unsupported={closure['unsupported_zero_required']} "
            f"dynamic_mips={closure['dynamic_mips_zero_required_for_pure_static']} "
            f"aot_concrete_bytes={closure['aot_concrete_destination_bytes']}/"
            f"{closure['concrete_destination_bytes']}",
            f"source_catalog transfer_inventory={source['transfer_inventory']} "
            f"known_open_findings={str(source['known_open_findings']).lower()} "
            f"catalog_total_authority=false",
            f"writer_authority complete={writer['complete_channels']}/8 "
            f"open={writer['open_channels']}",
            "completion_claim=false (scorecard aggregation cannot mint authority)",
        )
    )


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--closure-audit", required=True, type=Path)
    parser.add_argument("--source-frontier", required=True, type=Path)
    parser.add_argument("--writer-denominator", required=True, type=Path)
    parser.add_argument(
        "--writer-audit",
        type=Path,
        help="required authority companion for a writer-audit-bundle denominator",
    )
    parser.add_argument("--evidence-label", required=True, choices=("historical", "current"))
    parser.add_argument(
        "--ack-current-is-caller-attested",
        action="store_true",
        help="required for current: receipts do not bind the current Git worktree",
    )
    parser.add_argument("--format", choices=("json", "text"), default="text")
    args = parser.parse_args(argv)
    if args.evidence_label == "current" and not args.ack_current_is_caller_attested:
        parser.error(
            "--evidence-label current requires --ack-current-is-caller-attested; "
            "these receipts do not bind a Git worktree identity"
        )
    if args.evidence_label == "historical" and args.ack_current_is_caller_attested:
        parser.error("--ack-current-is-caller-attested is invalid for historical evidence")
    return args


def main(argv: list[str] | None = None) -> int:
    args = parse_args(sys.argv[1:] if argv is None else argv)
    try:
        closure = validate_closure(args.closure_audit, args.evidence_label)
        source = validate_source(args.source_frontier)
        writer = validate_writer(args.writer_denominator)
        writer_audit = validate_writer_audit(args.writer_audit) if args.writer_audit else None
        if closure["normalized_rom_sha256"] != source["normalized_rom_sha256"]:
            fail("receipt binding: closure and source ROM digests differ")
        if source["dense_aot_pack_sha256"] != writer["program_model_sha256"]:
            fail("receipt binding: source dense-pack and writer program-model digests differ")
        bundle_channels = writer["writer_audit_bundle_channels"]
        if bundle_channels and writer_audit is None:
            fail("receipt binding: writer-audit bundle rows require --writer-audit")
        if writer_audit is not None:
            if bundle_channels != len(CHANNELS) or writer["complete_channels"] != len(CHANNELS):
                fail("receipt binding: writer audit requires exactly eight completed bundle rows")
            if writer_audit["normalized_rom_sha256"] != closure["normalized_rom_sha256"]:
                fail("receipt binding: writer audit and closure/source ROM digests differ")
            if writer_audit["program_model_sha256"] != writer["program_model_sha256"]:
                fail("receipt binding: writer audit and denominator program-model digests differ")
            if writer_audit["writer_denominator_sha256"] != writer["published_payload_sha256"]:
                fail("receipt binding: writer audit does not bind the exact writer denominator")
            if writer_audit["bundle_schema"] != writer["writer_audit_bundle_schema"]:
                fail("receipt binding: writer audit and denominator bundle schemas differ")
            if (
                writer_audit["bundle_authority_sha256"]
                != writer["writer_audit_bundle_authority_sha256"]
            ):
                fail("receipt binding: writer audit and denominator bundle authorities differ")
        scorecard = {
            "schema": SCORECARD_SCHEMA,
            "evidence_label": args.evidence_label,
            "current_label_basis": (
                "caller_attested_not_worktree_bound"
                if args.evidence_label == "current"
                else "explicit_historical"
            ),
            "authority": "diagnostic_aggregation_only",
            "can_mint_or_restore_capability": False,
            "completion_claim": False,
            "closure": closure,
            "source_frontier": source,
            "writer_denominator": writer,
            "writer_audit": writer_audit,
        }
    except InvalidReceipt as error:
        print(f"static-recomp-scorecard: {error}", file=sys.stderr)
        return 1
    if args.format == "json":
        print(json.dumps(scorecard, sort_keys=True, separators=(",", ":")))
    else:
        print(render_text(scorecard))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
