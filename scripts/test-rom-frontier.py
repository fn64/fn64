#!/usr/bin/env python3
"""ROM-free regression tests for rom-frontier.py."""

from __future__ import annotations

import importlib.util
import contextlib
import io
import json
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock


SCRIPT = Path(__file__).resolve().with_name("rom-frontier.py")
SPEC = importlib.util.spec_from_file_location("rom_frontier", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
FRONTIER = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = FRONTIER
SPEC.loader.exec_module(FRONTIER)


def catalog_record(**overrides: object) -> dict:
    record = {
        "schema": FRONTIER.CATALOG_SCHEMA,
        "normalized_rom_sha256": "a" * 64,
        "stable_id": "test-rom",
        "internal_name": "TEST",
        "ipl3_group": "cic_6102_7101",
        "distinct_jal_targets": 1000,
        "loader_stub_ratio": 0.8,
        "code_run_share": 0.95,
        "boot_entropy": 5.6,
        "unaligned_mem": 10,
        "cache_ops": 2,
        "branch_likely": 30,
    }
    record.update(overrides)
    return record


def summary(digest: str = "a" * 64, **overrides: object) -> dict:
    states = {"Proven": 1, "Candidate": 500, "Supported": 20}
    coverage = {
        "mapped_banks": 1,
        "executable_bytes": 0,
        "function_entries_by_state": states,
    }
    record = {
        "schema_version": 1,
        "normalized_rom_sha256": digest,
        "selected_strategy": "boot_bank_only",
        "coverage": coverage,
        "strategy_outcomes": [
            outcome(),
            outcome(strategy="recovered_vrom"),
        ],
    }
    record.update(overrides)
    return record


def owner_proof() -> dict:
    marginal = {
        "kind": "unresolved_indirect",
        "affected_assessments": 7,
        "occurrences": 12,
        "sole_blocker_assessments": 3,
    }
    combination = {
        "assessment_state": "candidate",
        "kinds": ["entry_not_authoritative", "unresolved_indirect"],
        "assessments": 9,
    }
    ranges = {
        "proven_range_count": 1,
        "proven_bytes": 64,
        "provenance": [{"rule": "block_closure", "range_count": 1, "bytes": 64}],
    }
    indirect = {
        "total_sites": 8,
        "exhaustive_sites": 2,
        "bounded_sites": 1,
        "open_sites": 5,
        "via_call_sites": 3,
        "via_jump_sites": 5,
        "resolution_kinds": [
            {"kind": "constant", "sites": 2},
            {"kind": "unresolved", "sites": 6},
        ],
        "target_count_distribution": [
            {"target_count": 0, "sites": 5},
            {"target_count": 1, "sites": 3},
        ],
    }
    return {
        "coverage_blocker_payloads_omitted": True,
        "banks": [
            {
                "bank": "boot",
                "assessed_entries": 10,
                "exact_owners": 1,
                "candidate_owners": 9,
                "ambiguous_owners": 0,
                "exact_owner_bytes": 64,
                "blocker_marginals": [marginal],
                "blocker_combinations": [combination],
                "executable_ranges": ranges,
                "indirect_transfers": indirect,
            }
        ],
        "blocker_marginals": [marginal],
        "blocker_combinations": [combination],
        "executable_ranges": ranges,
        "indirect_transfers": indirect,
    }


class ClassifyTests(unittest.TestCase):
    def test_compressed_boot_wins_over_every_other_signal(self) -> None:
        # A packed boot image is a decompression problem, not a geometry
        # problem, however its other measures read.
        record = catalog_record(boot_entropy=7.86, code_run_share=0.005)
        self.assertEqual(FRONTIER.classify(record), "compressed_boot")

    def test_loader_stub_is_separated_from_resident_code(self) -> None:
        self.assertEqual(
            FRONTIER.classify(catalog_record(loader_stub_ratio=49.9)), "loader_stub"
        )
        self.assertEqual(
            FRONTIER.classify(catalog_record(loader_stub_ratio=0.6)), "resident_code"
        )

    def test_sparse_boot_is_not_mistaken_for_compressed(self) -> None:
        # Low code share with LOW entropy is sparse or relocated, which entropy
        # alone would misfile as healthy.
        record = catalog_record(code_run_share=0.064, boot_entropy=5.72)
        self.assertEqual(FRONTIER.classify(record), "sparse_boot")


def outcome(strategy: str = "recovered_overlays", **overrides: object) -> dict:
    record = {
        "strategy": strategy,
        "candidate_tables": 0,
        "admitted_tables": 0,
        "admitted_intervals": 0,
        "decoded_file_limit_hits": 0,
        "proven_mappings": 1,
        "supported_mappings": 0,
        "request_dma_open_rows": 0,
        "request_dma_incomplete": False,
        "request_dma_input_limit_hit": False,
        "physical_wrapper_candidates_examined": 0,
        "wrapper_semantic_proof_unavailable": 0,
        "physical_wrapper_candidate_limit_hit": False,
    }
    record.update(overrides)
    return record


class GeometryFailureTests(unittest.TestCase):
    def test_multi_bank_mapping_is_recovered(self) -> None:
        # Mega Man 64's shape: one table, 29 intervals, 28 proven mappings.
        outcomes = [
            outcome(candidate_tables=1, admitted_tables=1, admitted_intervals=29, proven_mappings=28)
        ]
        self.assertEqual(FRONTIER.geometry_failure(outcomes), "recovered")

    def test_no_candidate_table_is_distinguished_from_rejection(self) -> None:
        self.assertEqual(
            FRONTIER.geometry_failure([outcome()]), "no_candidate_table_found"
        )
        self.assertEqual(
            FRONTIER.geometry_failure([outcome(candidate_tables=3)]),
            "candidate_table_under_mapped",
        )

    def test_wrapper_rejection_is_not_reported_as_a_failure_reason(self) -> None:
        # Measured: Mega Man 64 rejects 631 of 632 wrapper candidates and still
        # recovers 28 banks via descriptor tables. Rejection is the normal
        # state, so it must not mask the real frontier -- no candidate table.
        outcomes = [outcome(strategy="recovered_vrom", physical_wrapper_candidates_examined=1009)]
        self.assertEqual(FRONTIER.geometry_failure(outcomes), "no_candidate_table_found")

    def test_wrapper_shapes_awaiting_proof_are_named(self) -> None:
        self.assertEqual(
            FRONTIER.geometry_failure(
                [outcome(strategy="recovered_vrom", wrapper_semantic_proof_unavailable=4)]
            ),
            "wrapper_shape_awaiting_proof",
        )

    def test_resource_ceilings_outrank_emptier_verdicts(self) -> None:
        # A truncated search is a frontier, not proven absence, so it must not
        # be reported as "no candidate table found".
        self.assertEqual(
            FRONTIER.geometry_failure([outcome(decoded_file_limit_hits=2)]),
            "decode_limit_hit",
        )
        self.assertEqual(
            FRONTIER.geometry_failure([outcome(physical_wrapper_candidate_limit_hit=True)]),
            "wrapper_limit_hit",
        )

    def test_missing_geometry_strategies_are_loud_not_silent(self) -> None:
        self.assertEqual(
            FRONTIER.geometry_failure([outcome(strategy="boot_bank_only")]),
            "no_geometry_strategy_ran",
        )
        self.assertEqual(FRONTIER.geometry_failure([]), "no_geometry_strategy_ran")

    def test_recovered_wins_over_a_concurrent_resource_ceiling(self) -> None:
        outcomes = [
            outcome(proven_mappings=28, admitted_intervals=29, admitted_tables=1),
            outcome(strategy="recovered_vrom", decoded_file_limit_hits=5),
        ]
        self.assertEqual(FRONTIER.geometry_failure(outcomes), "recovered")


class JoinTests(unittest.TestCase):
    def test_join_reports_the_proven_target_gap(self) -> None:
        rows = FRONTIER.join([catalog_record()], {"a" * 64: summary()})
        self.assertEqual(len(rows), 1)
        self.assertEqual(rows[0]["proven_entries"], 1)
        self.assertEqual(rows[0]["distinct_jal_targets"], 1000)
        self.assertEqual(rows[0]["proven_target_share"], 0.001)

    def test_unmatched_catalog_rows_are_dropped_not_guessed(self) -> None:
        with self.assertRaises(FRONTIER.FrontierError):
            FRONTIER.join([catalog_record()], {"b" * 64: summary("b" * 64)})

    def test_zero_targets_does_not_divide_by_zero(self) -> None:
        rows = FRONTIER.join(
            [catalog_record(distinct_jal_targets=0)], {"a" * 64: summary()}
        )
        self.assertEqual(rows[0]["proven_target_share"], 0.0)

    def test_owner_diagnostics_and_measurement_are_retained_without_renaming(self) -> None:
        proof = owner_proof()
        rows = FRONTIER.join(
            [catalog_record()],
            {"a" * 64: summary(owner_proof=proof)},
            {
                "a" * 64: {
                    "wall_seconds": 1.25,
                    "sampled_peak_rss_bytes": 4096,
                    "discovery_binary_sha256": "b" * 64,
                }
            },
        )
        self.assertEqual(rows[0]["owner_proof"], proof)
        self.assertEqual(rows[0]["wall_seconds"], 1.25)
        self.assertEqual(rows[0]["sampled_peak_rss_bytes"], 4096)
        self.assertEqual(rows[0]["discovery_binary_sha256"], "b" * 64)
        self.assertEqual(rows[0]["rss_scope"], "direct_process")
        self.assertEqual(rows[0]["rss_sample_interval_seconds"], 1.0)
        self.assertEqual(
            rows[0]["owner_proof"]["blocker_marginals"][0]["sole_blocker_assessments"],
            3,
        )

    def test_v1_summary_without_owner_diagnostics_remains_supported(self) -> None:
        row = FRONTIER.join([catalog_record()], {"a" * 64: summary()})[0]
        self.assertNotIn("owner_proof", row)
        self.assertNotIn("wall_seconds", row)

    def test_malformed_owner_diagnostics_are_loud(self) -> None:
        with self.assertRaises(FRONTIER.FrontierError):
            FRONTIER.join(
                [catalog_record()],
                {"a" * 64: summary(owner_proof=[])},
            )
        malformed = owner_proof()
        malformed["blocker_combinations"][0]["kinds"] = [
            "unresolved_indirect",
            "entry_not_authoritative",
        ]
        with self.assertRaises(FRONTIER.FrontierError):
            FRONTIER.join(
                [catalog_record()],
                {"a" * 64: summary(owner_proof=malformed)},
            )

    def test_report_exposes_sole_payoff_and_exact_combinations(self) -> None:
        row = FRONTIER.join(
            [catalog_record()],
            {"a" * 64: summary(owner_proof=owner_proof())},
        )[0]
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            FRONTIER.report([row])
        rendered = output.getvalue()
        self.assertIn("sole immediate payoff", rendered)
        self.assertIn("unresolved_indirect", rendered)
        self.assertIn("entry_not_authoritative + unresolved_indirect", rendered)
        self.assertIn("indirect sites:", rendered)


class DiscoveryRunTests(unittest.TestCase):
    def test_owner_proof_parallelism_default_is_memory_bounded(self) -> None:
        args = FRONTIER.parser().parse_args(["--catalog", "catalog", "--binary", "binary"])
        self.assertEqual(args.jobs, min(2, FRONTIER.os.cpu_count() or 1))

    def test_owner_proof_is_requested_and_cost_is_measured(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            arguments = root / "arguments"
            executable = root / "discover"
            emitted_summary = {
                "schema_version": 2,
                "normalized_rom_sha256": "a" * 64,
                "owner_proof": owner_proof(),
            }
            encoded_summary = json.dumps(
                emitted_summary, separators=(",", ":"), ensure_ascii=False
            )
            receipt = json.dumps(
                {
                    "summary": emitted_summary,
                    "receipt_sha256": __import__("hashlib")
                    .sha256(encoded_summary.encode())
                    .hexdigest(),
                },
                separators=(",", ":"),
            )
            executable.write_text(
                "#!/bin/sh\n"
                f"printf '%s\\n' \"$@\" > '{arguments}'\n"
                f"printf '%s\\n' '{receipt}'\n"
            )
            executable.chmod(0o755)
            result = FRONTIER.run_discovery(executable, root / "sample.z64", 5)
            self.assertEqual(
                arguments.read_text().splitlines(),
                [str(root / "sample.z64"), "--summary", "--prove-owners"],
            )
            self.assertGreaterEqual(result["wall_seconds"], 0)
            self.assertIn("peak_rss_bytes", result)

    def test_tampered_summary_receipt_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            executable = root / "discover"
            emitted_summary = {
                "schema_version": 2,
                "normalized_rom_sha256": "a" * 64,
                "owner_proof": owner_proof(),
            }
            receipt = json.dumps(
                {"summary": emitted_summary, "receipt_sha256": "0" * 64},
                separators=(",", ":"),
            )
            executable.write_text(f"#!/bin/sh\nprintf '%s\\n' '{receipt}'\n")
            executable.chmod(0o755)
            with self.assertRaisesRegex(FRONTIER.FrontierError, "hash does not match"):
                FRONTIER.run_discovery(executable, root / "sample.z64", 5)


class ParallelismTests(unittest.TestCase):
    def test_summaries_key_on_digest_so_completion_order_cannot_reorder_output(self) -> None:
        # Discovery fans out across cores, so results arrive in completion
        # order rather than ROM order. Rows are keyed by normalized digest and
        # the join walks the catalog, so output order follows the catalog and
        # is unaffected by which subprocess finishes first.
        catalog = [
            catalog_record(normalized_rom_sha256="a" * 64, internal_name="FIRST"),
            catalog_record(normalized_rom_sha256="b" * 64, internal_name="SECOND"),
        ]
        forward = {"a" * 64: summary("a" * 64), "b" * 64: summary("b" * 64)}
        reversed_completion = {"b" * 64: summary("b" * 64), "a" * 64: summary("a" * 64)}
        self.assertEqual(
            [row["internal_name"] for row in FRONTIER.join(catalog, forward)],
            [row["internal_name"] for row in FRONTIER.join(catalog, reversed_completion)],
        )


class CatalogLoadTests(unittest.TestCase):
    def test_binary_digest_is_streamed_from_exact_bytes(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "discover"
            path.write_bytes(b"exact executable bytes")
            self.assertEqual(
                FRONTIER.sha256_file(path),
                "135a9af43260004bfc617b97f806fcc6600e211fe0219d6c4311ef8cc6d59b48",
            )

    def test_wrong_schema_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "catalog.jsonl"
            path.write_text(json.dumps({"schema": "fn64.something-else.v1"}) + "\n")
            with self.assertRaises(FRONTIER.FrontierError):
                FRONTIER.load_catalog(path)

    def test_empty_catalog_is_loud(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "catalog.jsonl"
            path.write_text("\n")
            with self.assertRaises(FRONTIER.FrontierError):
                FRONTIER.load_catalog(path)

    def test_blank_lines_are_tolerated(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "catalog.jsonl"
            path.write_text(json.dumps(catalog_record()) + "\n\n")
            self.assertEqual(len(FRONTIER.load_catalog(path)), 1)


class OutputTests(unittest.TestCase):
    def test_relative_destination_is_refused(self) -> None:
        with self.assertRaises(FRONTIER.FrontierError):
            FRONTIER.validate_output_destination("relative.jsonl")

    def test_publish_writes_canonical_jsonl(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory).resolve() / "frontier.jsonl"
            FRONTIER.publish_records(path, [{"b": 2, "a": 1}])
            self.assertEqual(path.read_bytes(), b'{"a":1,"b":2}\n')

    def test_empty_failure_output_is_supported(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory).resolve() / "failures.jsonl"
            FRONTIER.publish_records(path, [])
            self.assertEqual(path.read_bytes(), b"")

    def test_all_failed_roms_are_published_only_to_explicit_failure_output(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            catalog = root / "catalog.jsonl"
            catalog.write_text(json.dumps(catalog_record()) + "\n")
            rom_dir = root / "roms"
            rom_dir.mkdir()
            (rom_dir / "failed.z64").write_bytes(b"")
            executable = root / "discover"
            executable.write_text("#!/bin/sh\nexit 3\n")
            executable.chmod(0o755)
            failures = root / "failures.jsonl"
            output = root / "frontier.jsonl"
            argv = [
                str(SCRIPT),
                "--catalog",
                str(catalog),
                "--binary",
                str(executable),
                "--rom-dir",
                str(rom_dir),
                "--output",
                str(output),
                "--failures-output",
                str(failures),
                "--jobs",
                "1",
            ]
            with mock.patch.object(sys, "argv", argv):
                self.assertEqual(FRONTIER.main(), 1)
            self.assertFalse(output.exists())
            record = json.loads(failures.read_text())
            self.assertEqual(record["schema"], FRONTIER.FAILURE_SCHEMA)
            self.assertEqual(record["input_name"], "failed.z64")
            self.assertEqual(
                record["identity_scope"], "path_free_basename_not_verified_digest"
            )


if __name__ == "__main__":
    unittest.main()
