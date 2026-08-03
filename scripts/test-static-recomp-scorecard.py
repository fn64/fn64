#!/usr/bin/env python3
"""Shell-free fixture tests for static-recomp-scorecard.py."""

from __future__ import annotations

import importlib.util
import hashlib
import json
import tempfile
from contextlib import redirect_stderr, redirect_stdout
from io import StringIO
from pathlib import Path


SCRIPT = Path(__file__).with_name("static-recomp-scorecard.py")
SPEC = importlib.util.spec_from_file_location("static_recomp_scorecard", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
scorecard = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(scorecard)
SHA = "1" * 64


def closure() -> dict:
    return {
        "schema": scorecard.CLOSURE_SCHEMA,
        "normalized_rom_sha256": SHA,
        "snapshot_schema_versions": [5],
        "classification_authority": "union_of_proven_rom_mapping_va_intervals",
        "authorities_not_consulted": ["host", "vector", "generation", "tlb"],
        "composed_bank_inputs": [],
        "proven_mapping_geometry": [],
        "scoreboard": {
            "total_destinations": 4,
            "per_class": {
                "exact_aot": {"destinations": 1, "bytes": 4},
                "block_aot": {"destinations": 1, "bytes": 4},
                "dynamic_mips": {"destinations": 1, "bytes": 0},
                "unsupported": {"destinations": 1, "bytes": 4},
            },
            "per_reason": {"open_indirect_site": 1, "outside_all_mappings": 1},
            "unsupported": 1,
            "dynamic_mips": 1,
        },
        "dynamic_concrete": [],
        "dynamic_indirect": [
            {
                "bank": "resident",
                "site_pc": 0x80000020,
                "via_call": False,
                "state": "Open",
                "kind": None,
                "targets": [],
                "memory_sources": [],
            }
        ],
        "unsupported": [
            {
                "destination_va": 0x80001000,
                "reason": "outside_all_mappings",
                "incoming": [{
                    "bank": "resident",
                    "block_start_va": 0x80000000,
                    "block_end_va": 0x80000008,
                    "source_site_va": 0x80000000,
                    "kind": "call",
                }],
            }
        ],
    }


def source() -> dict:
    arrays = {
        "dense_generations": [],
        "external_images": [],
        "exception_vectors": [],
        "host_bindings": [],
        "cache_sites": [],
        "direct_dma_findings": [],
        "direct_dma_blockers": [],
        "raw_pi_primitives": [],
        "cpu_store_watched_destinations": [],
        "cpu_store_scans": [],
        "cop0_status_scans": [],
        "external_cop0_status_scans": [],
        "conditional_cpu_word_stores": [],
        "open_cpu_word_stores": [],
        "open_writer_classes": [],
    }
    return {
        "schema": scorecard.SOURCE_SCHEMA,
        "producer": "fixture",
        "normalized_rom_sha256": SHA,
        "dense_aot_pack_sha256": SHA,
        "initial_cop0_status": {"authority": "missing"},
        **arrays,
        "transfer_scan": {
            "coverage": "bounded",
            "summary": {
                "direct_total": 1,
                "direct_guest": 1,
                "direct_host": 0,
                "direct_open": 0,
                "indirect_closed": 0,
                "indirect_bounded": 0,
                "indirect_open": 1,
            },
            "inventory": "open",
            "direct": [],
            "indirect_frontier": [],
            "blockers": ["fixture blocker"],
        },
    }


def writers() -> dict:
    return {
        "schema": scorecard.WRITER_SCHEMA,
        "producer": "fixture",
        "program_model_sha256": SHA,
        "channels": [
            {
                "channel": channel,
                "state": "open",
                "blockers": [{"code": "coverage_open", "evidence": "fixture blocker"}],
            }
            for channel in scorecard.CHANNELS
        ],
    }


def completed_writers() -> dict:
    value = writers()
    for row in value["channels"]:
        row.pop("blockers")
        row["state"] = "complete"
        row["receipt"] = {
            "validator": "writer_audit_bundle",
            "receipt": {
                "bundle_validator_schema": scorecard.WRITER_AUDIT_BUNDLE_SCHEMA,
                "bundle_authority_sha256": "2" * 64,
                "channel_series_authority_sha256": hashlib.sha256(
                    row["channel"].encode()
                ).hexdigest(),
            },
        }
    return value


def writer_audit(denominator_payload: bytes) -> dict:
    digest_fields = scorecard.WRITER_AUDIT_KEYS - {
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
    value = {field: "3" * 64 for field in digest_fields}
    value.update({
        "schema": scorecard.WRITER_AUDIT_SCHEMA,
        "exact_runs_per_channel": 10,
        "channel_count": 8,
        "completed_channel_bitmap": 255,
        "build_schema": scorecard.VERIFIED_BUILD_SCHEMA,
        "selected_build_cargo_jobs": 2,
        "build_max_rss_mib": 4096,
        "build_min_free_percent": 40,
        "bundle_schema": scorecard.WRITER_AUDIT_BUNDLE_SCHEMA,
        "normalized_rom_sha256": SHA,
        "program_model_sha256": SHA,
        "bundle_authority_sha256": "2" * 64,
        "writer_denominator_sha256": hashlib.sha256(denominator_payload).hexdigest(),
    })
    return value


def invoke(root: Path, *extra: str) -> tuple[int, str, str]:
    paths = {}
    for name, value in (("closure", closure()), ("source", source()), ("writers", writers())):
        path = root / f"{name}.json"
        path.write_text(json.dumps(value))
        paths[name] = path
    out, err = StringIO(), StringIO()
    argv = [
        "--closure-audit", str(paths["closure"]),
        "--source-frontier", str(paths["source"]),
        "--writer-denominator", str(paths["writers"]),
        *extra,
    ]
    try:
        with redirect_stdout(out), redirect_stderr(err):
            result = scorecard.main(argv)
    except SystemExit as error:
        result = int(error.code)
    return result, out.getvalue(), err.getvalue()


def invoke_completed(
    root: Path, *, mutate_audit=None, mutate_writers=None, include_audit: bool = True
):
    values = {"closure": closure(), "source": source(), "writers": completed_writers()}
    if mutate_writers is not None:
        mutate_writers(values["writers"])
    paths = {}
    for name, value in values.items():
        path = root / f"completed-{name}.json"
        path.write_text(json.dumps(value, separators=(",", ":")))
        paths[name] = path
    denominator_payload = paths["writers"].read_bytes()
    audit = writer_audit(denominator_payload)
    if mutate_audit is not None:
        mutate_audit(audit)
    audit_path = root / "completed-writer-audit.json"
    audit_path.write_text(json.dumps(audit, separators=(",", ":")))
    argv = [
        "--closure-audit", str(paths["closure"]),
        "--source-frontier", str(paths["source"]),
        "--writer-denominator", str(paths["writers"]),
        "--evidence-label", "current",
        "--ack-current-is-caller-attested",
        "--format", "json",
    ]
    if include_audit:
        argv.extend(("--writer-audit", str(audit_path)))
    out, err = StringIO(), StringIO()
    with redirect_stdout(out), redirect_stderr(err):
        result = scorecard.main(argv)
    return result, out.getvalue(), err.getvalue()


def main() -> int:
    with tempfile.TemporaryDirectory(prefix="fn64-scorecard-test.") as temporary:
        root = Path(temporary)
        result, output, _ = invoke(root, "--evidence-label", "historical", "--format", "json")
        assert result == 0
        report = json.loads(output)
        assert report["authority"] == "diagnostic_aggregation_only"
        assert report["completion_claim"] is False
        assert report["closure"]["aot_percent_of_concrete_destination_bytes"] == 66.666667
        assert report["writer_denominator"]["complete_channels"] == 0
        assert report["source_frontier"]["known_open_findings"] is True

        result, _, error = invoke(root, "--evidence-label", "current")
        assert result == 2 and "caller-attested" in error
        result, output, _ = invoke(
            root,
            "--evidence-label", "current",
            "--ack-current-is-caller-attested",
            "--format", "json",
        )
        assert result == 0
        assert json.loads(output)["current_label_basis"] == "caller_attested_not_worktree_bound"

        stale = closure()
        stale["snapshot_schema_versions"] = [4]
        (root / "closure.json").write_text(json.dumps(stale))
        try:
            scorecard.validate_closure(root / "closure.json", "current")
        except scorecard.InvalidReceipt as error:
            assert "snapshot schema v5 only" in str(error)
        else:
            raise AssertionError("stale snapshot schema was accepted as current")

        bad = closure()
        bad["unexpected"] = True
        (root / "closure.json").write_text(json.dumps(bad))
        # invoke rewrites fixtures, so exercise the validator directly.
        try:
            scorecard.validate_closure(root / "closure.json", "historical")
        except scorecard.InvalidReceipt as error:
            assert "unexpected=['unexpected']" in str(error)
        else:
            raise AssertionError("unknown closure field was accepted")

        bad_writer = writers()
        bad_writer["channels"][-1]["channel"] = scorecard.CHANNELS[0]
        (root / "writers.json").write_text(json.dumps(bad_writer))
        try:
            scorecard.validate_writer(root / "writers.json")
        except scorecard.InvalidReceipt as error:
            assert "duplicate" in str(error)
        else:
            raise AssertionError("duplicate writer channel was accepted")

        completed_writer = writers()
        completed_writer["channels"][2] = {
            "channel": "si_dma",
            "state": "complete",
            "receipt": {
                "validator": "si_dma",
                "receipt": {"validator_schema": "fixture.si.v1", "series_authority_sha256": SHA},
            },
        }
        completed_writer["channels"][7] = {
            "channel": "bootstrap_or_import",
            "state": "complete",
            "receipt": {
                "validator": "writer_audit_bundle",
                "receipt": {
                    "bundle_validator_schema": "fixture.bundle.v1",
                    "bundle_authority_sha256": SHA,
                    "channel_series_authority_sha256": SHA,
                },
            },
        }
        (root / "writers.json").write_text(json.dumps(completed_writer))
        assert scorecard.validate_writer(root / "writers.json")["complete_channels"] == 2

        cpu_bundle_writer = writers()
        cpu_bundle_writer["channels"][0] = {
            "channel": "cpu_instruction_store",
            "state": "complete",
            "receipt": {
                "validator": "writer_audit_bundle",
                "receipt": {
                    "bundle_validator_schema": "fixture.bundle.v1",
                    "bundle_authority_sha256": SHA,
                    "channel_series_authority_sha256": SHA,
                },
            },
        }
        (root / "writers.json").write_text(json.dumps(cpu_bundle_writer))
        assert scorecard.validate_writer(root / "writers.json")["complete_channels"] == 1

        malformed_cpu_bundle_writer = json.loads(json.dumps(cpu_bundle_writer))
        malformed_cpu_bundle_writer["channels"][0]["receipt"]["receipt"]["unexpected"] = True
        (root / "writers.json").write_text(json.dumps(malformed_cpu_bundle_writer))
        try:
            scorecard.validate_writer(root / "writers.json")
        except scorecard.InvalidReceipt as error:
            assert "unexpected=['unexpected']" in str(error)
        else:
            raise AssertionError("writer-audit CPU bundle accepted an unknown receipt field")

        pi_bundle_writer = writers()
        pi_bundle_writer["channels"][1] = {
            "channel": "pi_dma",
            "state": "complete",
            "receipt": cpu_bundle_writer["channels"][0]["receipt"],
        }
        (root / "writers.json").write_text(json.dumps(pi_bundle_writer))
        assert scorecard.validate_writer(root / "writers.json")["complete_channels"] == 1

        malformed_pi_bundle_writer = json.loads(json.dumps(pi_bundle_writer))
        malformed_pi_bundle_writer["channels"][1]["receipt"]["receipt"].pop(
            "channel_series_authority_sha256"
        )
        (root / "writers.json").write_text(json.dumps(malformed_pi_bundle_writer))
        try:
            scorecard.validate_writer(root / "writers.json")
        except scorecard.InvalidReceipt as error:
            assert "missing=['channel_series_authority_sha256']" in str(error)
        else:
            raise AssertionError("writer-audit PI bundle accepted a missing receipt field")

        host_abi_bundle_writer = writers()
        host_abi_bundle_writer["channels"][6] = {
            "channel": "host_abi",
            "state": "complete",
            "receipt": cpu_bundle_writer["channels"][0]["receipt"],
        }
        (root / "writers.json").write_text(json.dumps(host_abi_bundle_writer))
        assert scorecard.validate_writer(root / "writers.json")["complete_channels"] == 1

        malformed_host_abi_bundle_writer = json.loads(json.dumps(host_abi_bundle_writer))
        malformed_host_abi_bundle_writer["channels"][6]["receipt"]["receipt"].pop(
            "bundle_authority_sha256"
        )
        (root / "writers.json").write_text(json.dumps(malformed_host_abi_bundle_writer))
        try:
            scorecard.validate_writer(root / "writers.json")
        except scorecard.InvalidReceipt as error:
            assert "missing=['bundle_authority_sha256']" in str(error)
        else:
            raise AssertionError("writer-audit HostAbi bundle accepted a missing receipt field")

        standalone_host_abi_writer = writers()
        standalone_host_abi_writer["channels"][6] = {
            "channel": "host_abi",
            "state": "complete",
            "receipt": {
                "validator": "host_abi",
                "receipt": {
                    "validator_schema": "fixture.host-abi.v1",
                    "evidence_sha256": SHA,
                },
            },
        }
        (root / "writers.json").write_text(json.dumps(standalone_host_abi_writer))
        try:
            scorecard.validate_writer(root / "writers.json")
        except scorecard.InvalidReceipt as error:
            assert "unknown validator variant 'host_abi'" in str(error)
        else:
            raise AssertionError("standalone HostAbi receipt bypassed writer-audit bundle")

        rdp_renderer_bundle_writer = writers()
        rdp_renderer_bundle_writer["channels"][5] = {
            "channel": "rdp_renderer",
            "state": "complete",
            "receipt": cpu_bundle_writer["channels"][0]["receipt"],
        }
        (root / "writers.json").write_text(json.dumps(rdp_renderer_bundle_writer))
        assert scorecard.validate_writer(root / "writers.json")["complete_channels"] == 1

        malformed_rdp_renderer_bundle_writer = json.loads(json.dumps(rdp_renderer_bundle_writer))
        malformed_rdp_renderer_bundle_writer["channels"][5]["receipt"]["receipt"].pop(
            "channel_series_authority_sha256"
        )
        (root / "writers.json").write_text(json.dumps(malformed_rdp_renderer_bundle_writer))
        try:
            scorecard.validate_writer(root / "writers.json")
        except scorecard.InvalidReceipt as error:
            assert "missing=['channel_series_authority_sha256']" in str(error)
        else:
            raise AssertionError("writer-audit RDP bundle accepted a missing receipt field")

        standalone_rdp_renderer_writer = writers()
        standalone_rdp_renderer_writer["channels"][5] = {
            "channel": "rdp_renderer",
            "state": "complete",
            "receipt": {
                "validator": "rdp_renderer",
                "receipt": {
                    "validator_schema": "fixture.rdp-renderer.v1",
                    "evidence_sha256": SHA,
                },
            },
        }
        (root / "writers.json").write_text(json.dumps(standalone_rdp_renderer_writer))
        try:
            scorecard.validate_writer(root / "writers.json")
        except scorecard.InvalidReceipt as error:
            assert "unknown validator variant 'rdp_renderer'" in str(error)
        else:
            raise AssertionError("standalone RDP receipt bypassed writer-audit bundle")

        rsp_bundle_writer = writers()
        rsp_bundle_writer["channels"][4] = {
            "channel": "rsp_execution_or_hle_writeback",
            "state": "complete",
            "receipt": cpu_bundle_writer["channels"][0]["receipt"],
        }
        (root / "writers.json").write_text(json.dumps(rsp_bundle_writer))
        assert scorecard.validate_writer(root / "writers.json")["complete_channels"] == 1

        malformed_rsp_bundle_writer = json.loads(json.dumps(rsp_bundle_writer))
        malformed_rsp_bundle_writer["channels"][4]["receipt"]["receipt"].pop(
            "bundle_validator_schema"
        )
        (root / "writers.json").write_text(json.dumps(malformed_rsp_bundle_writer))
        try:
            scorecard.validate_writer(root / "writers.json")
        except scorecard.InvalidReceipt as error:
            assert "missing=['bundle_validator_schema']" in str(error)
        else:
            raise AssertionError("writer-audit RSP bundle accepted a missing receipt field")

        standalone_rsp_writer = writers()
        standalone_rsp_writer["channels"][4] = {
            "channel": "rsp_execution_or_hle_writeback",
            "state": "complete",
            "receipt": {
                "validator": "rsp_execution_or_hle_writeback",
                "receipt": {
                    "validator_schema": "fixture.rsp.v1",
                    "evidence_sha256": SHA,
                },
            },
        }
        (root / "writers.json").write_text(json.dumps(standalone_rsp_writer))
        try:
            scorecard.validate_writer(root / "writers.json")
        except scorecard.InvalidReceipt as error:
            assert "unknown validator variant 'rsp_execution_or_hle_writeback'" in str(error)
        else:
            raise AssertionError("standalone RSP receipt bypassed writer-audit bundle")

        bad_board = closure()
        bad_board["scoreboard"]["unsupported"] = 0
        (root / "closure.json").write_text(json.dumps(bad_board))
        try:
            scorecard.validate_closure(root / "closure.json", "historical")
        except scorecard.InvalidReceipt as error:
            assert "unsupported headline disagrees" in str(error)
        else:
            raise AssertionError("inconsistent closure headline was accepted")

        result, output, error = invoke_completed(root)
        assert result == 0, error
        completed_report = json.loads(output)
        assert completed_report["writer_denominator"]["complete_channels"] == 8
        assert completed_report["writer_audit"]["exact_runs_per_channel"] == 10
        assert completed_report["writer_audit"]["completed_channel_bitmap"] == 255

        result, _, error = invoke_completed(root, include_audit=False)
        assert result == 1 and "require --writer-audit" in error

        def make_partial_writer(denominator):
            for index in (1, 4):
                denominator["channels"][index].pop("receipt")
                denominator["channels"][index]["state"] = "open"
                denominator["channels"][index]["blockers"] = [
                    {"code": "validator_unavailable", "evidence": "series failed"}
                ]

        result, _, error = invoke_completed(
            root, mutate_writers=make_partial_writer, include_audit=False
        )
        assert result == 1 and "require --writer-audit" in error

        result, _, error = invoke_completed(
            root,
            mutate_writers=make_partial_writer,
            mutate_audit=lambda audit: audit.__setitem__(
                "schema", "fn64.wm-selected-build-writer-audit-partial-diagnostic.v1"
            ),
        )
        assert result == 1 and "unsupported schema" in error

        for field, invalid in (
            ("exact_runs_per_channel", 9),
            ("channel_count", 7),
            ("completed_channel_bitmap", 254),
            ("selected_build_cargo_jobs", 1),
            ("build_max_rss_mib", 2048),
            ("build_min_free_percent", 25),
        ):
            result, _, error = invoke_completed(
                root, mutate_audit=lambda audit, f=field, v=invalid: audit.__setitem__(f, v)
            )
            assert result == 1 and f"{field} must be exactly" in error

        result, _, error = invoke_completed(
            root,
            mutate_audit=lambda audit: audit.__setitem__(
                "build_schema", "fn64.verified-generated-runner-build.v4"
            ),
        )
        assert result == 1 and "unsupported build schema" in error

        for field, message in (
            ("normalized_rom_sha256", "ROM digests differ"),
            ("program_model_sha256", "program-model digests differ"),
            ("writer_denominator_sha256", "exact writer denominator"),
            ("bundle_authority_sha256", "bundle authorities differ"),
        ):
            result, _, error = invoke_completed(
                root, mutate_audit=lambda audit, f=field: audit.__setitem__(f, "4" * 64)
            )
            assert result == 1 and message in error

        mixed_bundle = completed_writers()
        mixed_bundle["channels"][0]["receipt"]["receipt"]["bundle_authority_sha256"] = "5" * 64
        (root / "mixed-writers.json").write_text(json.dumps(mixed_bundle))
        try:
            scorecard.validate_writer(root / "mixed-writers.json")
        except scorecard.InvalidReceipt as error:
            assert "do not share one bundle" in str(error)
        else:
            raise AssertionError("writer rows from different bundles were accepted")

    print("static recomp scorecard selftest: 32/32")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
