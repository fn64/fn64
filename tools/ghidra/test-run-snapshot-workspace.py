#!/usr/bin/env python3
"""Adversarial tests for the bounded snapshot-workspace queue."""

from __future__ import annotations

import hashlib
import importlib.util
import json
import os
import signal
import stat
import sys
import tempfile
import time
import unittest
from contextlib import contextmanager
from pathlib import Path
from unittest.mock import patch


MODULE_PATH = Path(__file__).with_name("run-snapshot-workspace.py")
SPEC = importlib.util.spec_from_file_location("fn64_run_snapshot_workspace", MODULE_PATH)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"cannot load {MODULE_PATH}")
QUEUE = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = QUEUE
SPEC.loader.exec_module(QUEUE)


def encoded_json(value: object) -> bytes:
    return (json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n").encode()


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def private_directory(path: Path) -> None:
    path.mkdir(mode=0o700, parents=True, exist_ok=True)
    path.chmod(0o700)


def private_file(path: Path, data: bytes, *, executable: bool = False) -> None:
    private_directory(path.parent)
    path.write_bytes(data)
    path.chmod(0o700 if executable else 0o600)


def make_snapshot_workspace(
    root: Path,
    bank_count: int = 2,
    *,
    seed_modes: list[str] | None = None,
) -> Path:
    workspace = root / "source"
    private_directory(workspace)
    if seed_modes is None:
        seed_modes = ["discovery_only"] * bank_count
    if len(seed_modes) != bank_count:
        raise ValueError("seed mode count must match bank count")
    banks = []
    aggregate_snapshots = 0
    rom_cursor = 0
    for index, seed_mode in enumerate(seed_modes):
        bank_length = 8 if seed_mode == "paired" else 4
        bank_bytes = bytes(0x41 + (index + offset) % 20 for offset in range(bank_length))
        va_start = 0x80000000 + rom_cursor
        snapshot_bytes = encoded_json(
            {
                "bank": index,
                "rom_start": rom_cursor,
                "va_start": va_start,
                "byte_length": bank_length,
            }
        )
        bank_artifact = f"bank-{index:06}.bin"
        snapshot_artifact = f"bank-{index:06}.snapshot.json"
        private_file(workspace / bank_artifact, bank_bytes)
        private_file(workspace / snapshot_artifact, snapshot_bytes)
        program_sha = hashlib.sha256(
            b"fn64.program-snapshot.v3\0" + snapshot_bytes[:-1]
        ).hexdigest()
        if seed_mode == "discovery_only":
            ghidra_seeds = {"mode": "discovery_only", "role": "candidate_only"}
        elif seed_mode == "base_only":
            ghidra_seeds = {
                "mode": "base_only",
                "base_seed": va_start,
                "base_seed_role": "proven_owner",
            }
        elif seed_mode == "paired":
            ghidra_seeds = {
                "mode": "paired",
                "base_seed": va_start,
                "base_seed_role": "proven_owner",
                "snapshot_seed": va_start + 4,
                "snapshot_seed_role": "assessed_owner",
                "snapshot_seed_assessment": "candidate",
            }
        else:
            raise ValueError(f"unsupported fixture seed mode {seed_mode!r}")
        banks.append(
            {
                "index": index,
                "bank": f"bank-{index}",
                "backing": {
                    "kind": "rom_affine",
                    "rom_space": "Physical",
                    "rom_start": rom_cursor,
                    "rom_end": rom_cursor + bank_length,
                },
                "va_start": va_start,
                "va_end": va_start + bank_length,
                "byte_length": bank_length,
                "backing_evidence_fact_indices": [],
                "bank_sha256": sha256(bank_bytes),
                "bank_artifact": bank_artifact,
                "snapshot_artifact": snapshot_artifact,
                "snapshot_artifact_byte_length": len(snapshot_bytes),
                "snapshot_artifact_sha256": sha256(snapshot_bytes),
                "program_snapshot_sha256": program_sha,
                "ghidra_seeds": ghidra_seeds,
            }
        )
        aggregate_snapshots += len(snapshot_bytes)
        rom_cursor += bank_length
    strategies = [
        "boot_bank_open",
        "boot_bank_only",
        "recovered_vrom",
        "recovered_overlays",
        "untabled_delta_vote",
    ]
    manifest = {
        "schema": "fn64.snapshot-workspace",
        "schema_version": 4,
        "state": "composed",
        "open_reason": None,
        "normalized_rom_sha256": "11" * 32,
        "discovery": {
            "selected": "boot_bank_only",
            "outcomes": [
                {
                    "strategy": strategy,
                    "candidate_tables": 0,
                    "admitted_tables": 0,
                    "admitted_intervals": 0,
                    "proven_mappings": 1 if strategy == "boot_bank_only" else 0,
                    "supported_mappings": 0,
                    "decoded_file_limit_hits": 0,
                    "request_dma_open_rows": 0,
                    "request_dma_incomplete": False,
                    "request_dma_input_limit_hit": False,
                    "physical_wrapper_candidates_examined": 0,
                    "wrapper_semantic_proof_unavailable": 0,
                    "physical_wrapper_candidate_limit_hit": False,
                }
                for strategy in strategies
            ],
        },
        "limits": {
            "max_rom_bytes": 64 * 1024 * 1024,
            "max_banks": 4096,
            "max_snapshot_artifact_bytes": 128 * 1024 * 1024,
            "max_aggregate_snapshot_artifact_bytes": 1024 * 1024 * 1024,
            "max_discovery_decoded_vrom_file_bytes": 64 * 1024 * 1024,
            "max_preparation_decoded_vrom_file_bytes": 64 * 1024 * 1024,
            "max_projected_fact_rows": 4_000_000,
            "max_projected_fact_bytes": 256 * 1024 * 1024,
            "max_aggregate_materialized_bytes": 256 * 1024 * 1024,
            "max_cross_bank_authority_records": 1_048_576,
        },
        "snapshot_wire": {
            "schema_version": 6,
            "authority": "diagnostic_only",
            "duplicates_fact_db_per_bank": False,
            "remaining_large_rom_frontier": "streaming_v6",
        },
        "aggregate_snapshot_artifact_bytes": aggregate_snapshots,
        "rom_recompilation_complete": False,
        "remaining_recompilation_frontier": "proven_bank_and_callable_owner_closure",
        "intended_use": "candidate_ghidra_only",
        "banks": banks,
    }
    private_file(workspace / "snapshot-workspace.json", encoded_json(manifest))
    return workspace


FAKE_RUNNER = r'''#!/usr/bin/env python3
import fcntl
import hashlib
import json
import os
import signal
import subprocess
import sys
import time
from pathlib import Path

os.umask(0o077)

def enc(value):
    return (json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n").encode()

def digest(data):
    return hashlib.sha256(data).hexdigest()

def put(path, data):
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    path.parent.chmod(0o700)
    path.write_bytes(data)
    path.chmod(0o600)

def check_inherited_lock():
    target = os.stat(os.environ["FN64_TEST_LOCK"])
    matches = []
    try:
        names = os.listdir("/dev/fd")
    except OSError:
        names = [str(fd) for fd in range(3, 1024)]
    for name in names:
        if not name.isdigit() or int(name) <= 2:
            continue
        try:
            opened = os.fstat(int(name))
        except OSError:
            continue
        if (opened.st_dev, opened.st_ino) == (target.st_dev, target.st_ino):
            matches.append(int(name))
    if not matches:
        raise RuntimeError("queue lock descriptor was not inherited")
    put(Path(os.environ["FN64_TEST_LOCK_WITNESS"]), (str(matches[0]) + "\n").encode())

def emit_success(workspace, bank_name, snapshot_path):
    retained = workspace / "ghidra-snapshot-bank.fake"
    contents = {
        "out/tool-claims.json": b"{}\n",
        "modes/unseeded/out/provider.jsonl": b"{}\n",
        "diagnostics/ghidra-distribution-scan.log": b"scan\n",
        "diagnostics/ghidra-distribution-scan-memory.jsonl": b"{}\n",
        "diagnostics/ghidra-distribution-verify.log": b"verify\n",
        "diagnostics/ghidra-distribution-verify-memory.jsonl": b"{}\n",
        "diagnostics/stage-memory.jsonl": b"{}\n",
        "modes/unseeded/diagnostics/memory.jsonl": b"{}\n",
        "diagnostics/ingest-memory.jsonl": b"{}\n",
        "tool-artifacts/Fn64ExportCandidates.java": b"candidate\n",
        "tool-artifacts/analyzeHeadless": b"headless\n",
        "tool-artifacts/application.properties": b"version=test\n",
        "tool-artifacts/java": b"java\n",
        "tool-artifacts/ingest_tool_claims": b"ingest\n",
        "tool-artifacts/manifest-ghidra-distribution.py": b"manifest\n",
        "tool-artifacts/memory-guard.zsh": b"guard\n",
        "tool-artifacts/run-snapshot-bank.sh": b"runner\n",
        "tool-artifacts/stage_snapshot_bank": b"stage\n",
    }
    if not is_discovery:
        contents["tool-artifacts/Fn64SeedFunctions.java"] = b"seed\n"
    distribution = enc({
        "schema": "fn64.ghidra-distribution-manifest",
        "schema_version": 1,
        "files": [{"fixture": True}],
    })
    contents["tool-artifacts/ghidra-distribution.json"] = distribution
    distribution_sha = digest(distribution)
    put(
        workspace
        / ".fn64-ghidra-distribution-manifests"
        / f"{distribution_sha}.json",
        distribution,
    )
    for relative, data in contents.items():
        put(retained / relative, data)

    orchestration_paths = [
        "tool-artifacts/ingest_tool_claims",
        "tool-artifacts/manifest-ghidra-distribution.py",
        "tool-artifacts/memory-guard.zsh",
        "tool-artifacts/run-snapshot-bank.sh",
        "tool-artifacts/stage_snapshot_bank",
    ]
    orchestration = enc({
        "schema": "fn64.ghidra-orchestration-artifacts",
        "schema_version": 1,
        "artifacts": [
            {"path": path, "byte_length": len(contents[path]), "sha256": digest(contents[path])}
            for path in orchestration_paths
        ],
    })
    put(retained / "tool-artifacts/orchestration.json", orchestration)
    contents["tool-artifacts/orchestration.json"] = orchestration

    tool_paths = [
        "tool-artifacts/Fn64ExportCandidates.java",
        "tool-artifacts/analyzeHeadless",
        "tool-artifacts/application.properties",
        "tool-artifacts/ghidra-distribution.json",
        "tool-artifacts/java",
        "tool-artifacts/orchestration.json",
    ]
    if not is_discovery:
        tool_paths.insert(1, "tool-artifacts/Fn64SeedFunctions.java")
    tool_manifest = enc({
        "schema": "fn64.tool-artifact-manifest",
        "schema_version": 1,
        "tool_name": "ghidra-headless-unseeded",
        "tool_version": "fixture-v1",
        "artifacts": [
            {"path": path, "byte_length": len(contents[path]), "sha256": digest(contents[path])}
            for path in tool_paths
        ],
    })
    put(retained / "tool-unseeded.json", tool_manifest)

    snapshot_bytes = snapshot_path.read_bytes()
    snapshot = json.loads(snapshot_bytes)
    program_sha = hashlib.sha256(b"fn64.program-snapshot.v3\0" + snapshot_bytes[:-1]).hexdigest()
    bank_bytes = bank_path.read_bytes()
    put(retained / "inputs/bank.bin", bank_bytes)
    put(retained / "inputs/program-snapshot.json", snapshot_bytes)
    va_start = snapshot["va_start"]
    va_end = va_start + len(bank_bytes)
    evidence_seeds = (
        {"mode": "discovery_only", "role": "candidate_only"}
        if is_discovery
        else {"mode": "base_only", "base_seed": base_seed}
    )
    evidence = enc({
        "schema": "fn64.snapshot-bank-evidence",
        "schema_version": 3 if is_discovery else 2,
        "program_snapshot_sha256": program_sha,
        "input": {
            "normalized_rom_sha256": "11" * 32,
            "bank": bank_name,
            "bank_bytes_sha256": digest(bank_bytes),
            "mapping_sha256": "22" * 32,
            "va_start": va_start,
            "va_end": va_end,
        },
        "backing": {
            "rom_space": "Physical",
            "rom_start": snapshot["rom_start"],
            "rom_end": snapshot["rom_start"] + len(bank_bytes),
        },
        "artifact": {"byte_length": len(bank_bytes), "sha256": digest(bank_bytes)},
        "seeds": evidence_seeds,
    })
    contents["raw/evidence.json"] = evidence
    put(retained / "raw/evidence.json", evidence)
    config_value = {
        "schema": "fn64.ghidra-bank-config",
        "schema_version": 1,
        "mode": "unseeded",
        "bank": bank_name,
        "va_start": va_start,
        "va_end": va_end,
        "base_seed": None if is_discovery else base_seed,
        "snapshot_seed": None,
        "loader": "BinaryLoader",
        "processor": "MIPS:BE:64:64-32addr",
        "cspec": "o32",
        "ghidra_version": "fixture-v1",
        "analysis_timeout_seconds": 120,
        "max_cpu": 1,
        "heap_mib": 1024,
        "rss_mib": 2048,
        "min_free_percent": 40,
        "wall_seconds": 180,
        "tool_manifest_sha256": digest(tool_manifest),
    }
    if is_discovery:
        config_value["role"] = "candidate_only"
    config = enc(config_value)
    contents["config/unseeded.json"] = config
    put(retained / "config/unseeded.json", config)
    request = enc({
        "schema": "fn64.tool-ingest-request",
        "schema_version": 1,
        "runs": [{
            "bank": bank_name,
            "jsonl": "modes/unseeded/out/provider.jsonl",
            "tool": {
                "name": "ghidra-headless-unseeded",
                "version": "fixture-v1",
                "build_sha256": digest(tool_manifest),
            },
            "tool_artifact_manifest": "tool-unseeded.json",
            "role": "function_boundary_candidates",
            "lineage_artifacts": [
                {"role": "tool_configuration", "path": "config/unseeded.json"},
                {"role": "evidence_manifest", "path": "raw/evidence.json"},
            ],
        }],
    })
    contents["request.json"] = request
    put(retained / "request.json", request)
    resources = {
        "ghidra_distribution_scan_log": "diagnostics/ghidra-distribution-scan.log",
        "ghidra_distribution_scan": "diagnostics/ghidra-distribution-scan-memory.jsonl",
        "ghidra_distribution_verify_log": "diagnostics/ghidra-distribution-verify.log",
        "ghidra_distribution_verify": "diagnostics/ghidra-distribution-verify-memory.jsonl",
        "stage": "diagnostics/stage-memory.jsonl",
        "unseeded": "modes/unseeded/diagnostics/memory.jsonl",
        "ingest": "diagnostics/ingest-memory.jsonl",
    }
    receipt = {
        "schema": "fn64.ghidra-snapshot-bank-receipt",
        "schema_version": 1,
        "execution_mode": "discovery-only" if is_discovery else "unseeded-only",
        "paired_comparison_complete": False,
        "completed_modes": ["unseeded"],
        "program_snapshot_sha256": program_sha,
        "bank": bank_name,
        "seeds": evidence_seeds,
        "evidence_sha256": digest(contents["raw/evidence.json"]),
        "request_sha256": digest(contents["request.json"]),
        "unseeded_tool_manifest_sha256": digest(tool_manifest),
        "tool_claims_sha256": digest(contents["out/tool-claims.json"]),
        "ghidra_distribution_manifest_complete": True,
        "ghidra_distribution_manifest_sha256": distribution_sha,
        "ghidra_distribution_file_count": 1,
        "tool_artifact_scope": "all-ghidra-install-regular-files,jdk-java,fn64-analysis-scripts,and-bound-orchestration-helpers",
        "configuration_sha256": {"unseeded": digest(contents["config/unseeded.json"])},
        "provider_jsonl_sha256": {"unseeded": digest(contents["modes/unseeded/out/provider.jsonl"])},
        "resource_evidence_sha256": {
            key: digest(contents[path]) for key, path in resources.items()
        },
    }
    put(retained / "out/receipt.json", enc(receipt))
    return retained / "out/receipt.json"

check_inherited_lock()
mode = os.environ.get("FN64_TEST_MODE", "success")
is_discovery = sys.argv[1] == "--discovery-only"
if not is_discovery and sys.argv[1] != "--unseeded-only":
    raise RuntimeError("unknown runner execution mode")
snapshot_path = Path(sys.argv[2])
bank_name = sys.argv[3]
bank_path = Path(sys.argv[4])
workspace = Path(sys.argv[5])
base_seed = None if is_discovery else int(sys.argv[6], 0)
active = Path(os.environ["FN64_TEST_ACTIVE"])
try:
    descriptor = os.open(active, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
except FileExistsError:
    raise RuntimeError("overlapping queue launch")
else:
    os.close(descriptor)
with open(os.environ["FN64_TEST_LAUNCHES"], "a", encoding="utf-8") as stream:
    stream.write(bank_name + "\n")
with open(os.environ["FN64_TEST_ARGUMENTS"], "a", encoding="utf-8") as stream:
    stream.write(json.dumps(sys.argv[1:], separators=(",", ":")) + "\n")

if mode == "flood":
    signal.signal(signal.SIGTERM, signal.SIG_IGN)
    child = subprocess.Popen(
        [sys.executable, "-c", "import signal,time; signal.signal(signal.SIGTERM, signal.SIG_IGN); time.sleep(30)"],
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    put(Path(os.environ["FN64_TEST_DESCENDANT"]), (str(child.pid) + "\n").encode())
    while True:
        os.write(1, b"x" * 65536)

if mode == "orphan_pipes":
    child_code = (
        "import os,signal,time; "
        "signal.signal(signal.SIGTERM, signal.SIG_IGN); "
        "time.sleep(.2); end=time.monotonic()+2.5; "
        "exec('while time.monotonic() < end:\\n os.write(1, b\\\"x\\\" * 65536)')"
    )
    child = subprocess.Popen([sys.executable, "-c", child_code])
    put(Path(os.environ["FN64_TEST_DESCENDANT"]), (str(child.pid) + "\n").encode())
    active.unlink(missing_ok=True)
    raise SystemExit(0)

try:
    receipt_path = emit_success(workspace, bank_name, snapshot_path)
    if mode == "malformed":
        receipt = json.loads(receipt_path.read_bytes())
        receipt["forged"] = True
        put(receipt_path, enc(receipt))
    elif mode == "mutate":
        with bank_path.open("ab") as stream:
            stream.write(b"X")
            stream.flush()
            os.fsync(stream.fileno())
    elif mode in {"extra_cache", "cache_tamper", "cache_symlink"}:
        receipt = json.loads(receipt_path.read_bytes())
        cache = workspace / ".fn64-ghidra-distribution-manifests"
        expected = cache / f"{receipt['ghidra_distribution_manifest_sha256']}.json"
        if mode == "extra_cache":
            put(cache / (("00" * 32) + ".json"), b"{}\n")
        elif mode == "cache_tamper":
            put(expected, b"{}\n")
        else:
            expected.unlink()
            os.symlink(
                receipt_path.parents[1] / "tool-artifacts/ghidra-distribution.json",
                expected,
            )
    time.sleep(0.03)
finally:
    active.unlink(missing_ok=True)
'''


class SnapshotWorkspaceQueueTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory(prefix="fn64-queue-test-")
        self.root = Path(self.temporary.name).resolve()
        self.root.chmod(0o700)
        self.source = make_snapshot_workspace(self.root)
        self.output = self.root / "output"
        private_directory(self.output)
        self.runner = self.root / "fake-runner.py"
        private_file(self.runner, FAKE_RUNNER.encode(), executable=True)
        self.stage = self.root / "stage"
        self.ingest = self.root / "ingest"
        private_file(self.stage, b"#!/bin/sh\nexit 0\n", executable=True)
        private_file(self.ingest, b"#!/bin/sh\nexit 0\n", executable=True)
        self.lock = self.root / "queue.lock"
        self.active = self.root / "active"
        self.launches = self.root / "launches.log"
        self.arguments = self.root / "arguments.jsonl"
        self.descendant = self.root / "descendant.pid"
        self.lock_witness = self.root / "lock-witness"
        self.base_environment = {
            "FN64_STAGE_SNAPSHOT_BANK": str(self.stage),
            "FN64_INGEST_TOOL_CLAIMS": str(self.ingest),
            "FN64_TEST_LOCK": str(self.lock),
            "FN64_TEST_ACTIVE": str(self.active),
            "FN64_TEST_LAUNCHES": str(self.launches),
            "FN64_TEST_ARGUMENTS": str(self.arguments),
            "FN64_TEST_DESCENDANT": str(self.descendant),
            "FN64_TEST_LOCK_WITNESS": str(self.lock_witness),
        }

    def tearDown(self) -> None:
        if self.descendant.exists():
            try:
                pid = int(self.descendant.read_text().strip())
                os.kill(pid, signal.SIGKILL)
            except (OSError, ValueError):
                pass
        self.temporary.cleanup()

    @contextmanager
    def environment(self, mode: str = "success"):
        values = {**self.base_environment, "FN64_TEST_MODE": mode}
        with patch.dict(os.environ, values, clear=False):
            yield

    def limits(self, **changes):
        values = {
            "max_launches": 64,
            "max_wall_seconds": 10,
            "max_attempts_per_bank": 3,
            "max_ordinary_failures": 8,
            "max_log_bytes": 1024 * 1024,
            "max_attempt_bytes": 8 * 1024 * 1024,
            "min_free_disk_bytes": 1,
            "termination_grace_seconds": 1,
        }
        values.update(changes)
        return QUEUE.QueueLimits(**values)

    def run_queue(self, *, mode: str = "success", limits=None) -> int:
        with self.environment(mode):
            return QUEUE.run_queue(
                self.source,
                self.output,
                limits=limits or self.limits(),
                lock_path=self.lock,
                runner_path=self.runner,
            )

    def test_two_banks_are_sequential_lock_is_inherited_and_resume_does_not_relaunch(self) -> None:
        self.assertEqual(self.run_queue(), 0)
        self.assertEqual(self.launches.read_text().splitlines(), ["bank-0", "bank-1"])
        self.assertGreaterEqual(int(self.lock_witness.read_text()), 3)
        self.assertFalse(self.active.exists())
        self.assertEqual(self.run_queue(), 0)
        self.assertEqual(self.launches.read_text().splitlines(), ["bank-0", "bank-1"])
        receipt = json.loads((self.output / "queue-receipt.json").read_bytes())
        request = json.loads((self.output / "queue-request.json").read_bytes())
        queue_bytes = MODULE_PATH.read_bytes()
        self.assertEqual(
            request["tools"]["queue"],
            {"byte_length": len(queue_bytes), "sha256": sha256(queue_bytes)},
        )
        self.assertEqual(receipt["state"], "candidate_queue_complete")
        self.assertEqual([bank["state"] for bank in receipt["banks"]], ["success", "success"])

    def test_singleton_lock_rejects_a_second_owner(self) -> None:
        descriptor = QUEUE.acquire_lock(self.lock)
        try:
            with self.assertRaisesRegex(QUEUE.QueueError, "another .* queue is active"):
                QUEUE.acquire_lock(self.lock)
        finally:
            os.close(descriptor)

    def test_base_only_and_paired_dispatch_only_the_proven_base_seed(self) -> None:
        self.source = make_snapshot_workspace(
            self.root / "seeded",
            bank_count=2,
            seed_modes=["base_only", "paired"],
        )
        manifest = json.loads((self.source / "snapshot-workspace.json").read_bytes())
        self.assertEqual(self.run_queue(), 0)
        arguments = [json.loads(line) for line in self.arguments.read_text().splitlines()]
        self.assertEqual(len(arguments), 2)
        for index, actual in enumerate(arguments):
            bank = manifest["banks"][index]
            self.assertEqual(
                actual,
                [
                    "--unseeded-only",
                    str(self.source / bank["snapshot_artifact"]),
                    bank["bank"],
                    str(self.source / bank["bank_artifact"]),
                    str(
                        self.output
                        / f"banks/{index:06}/attempts/000001/runner-workspace"
                    ),
                    f"0x{bank['ghidra_seeds']['base_seed']:08x}",
                ],
            )
        paired_snapshot_seed = manifest["banks"][1]["ghidra_seeds"]["snapshot_seed"]
        self.assertNotIn(f"0x{paired_snapshot_seed:08x}", arguments[1])
        for index in range(2):
            retained = (
                self.output
                / f"banks/{index:06}/attempts/000001/runner-workspace/ghidra-snapshot-bank.fake"
            )
            receipt = json.loads((retained / "out/receipt.json").read_bytes())
            evidence = json.loads((retained / "raw/evidence.json").read_bytes())
            config = json.loads((retained / "config/unseeded.json").read_bytes())
            expected_seed = manifest["banks"][index]["ghidra_seeds"]["base_seed"]
            expected_seeds = {"mode": "base_only", "base_seed": expected_seed}
            self.assertEqual(receipt["execution_mode"], "unseeded-only")
            self.assertEqual(receipt["seeds"], expected_seeds)
            self.assertEqual(evidence["schema_version"], 2)
            self.assertEqual(evidence["seeds"], expected_seeds)
            self.assertEqual(config["base_seed"], expected_seed)
            self.assertNotIn("role", config)

    def test_malformed_current_producer_schema_is_rejected_before_launch(self) -> None:
        manifest_path = self.source / "snapshot-workspace.json"
        manifest = json.loads(manifest_path.read_bytes())
        manifest["unrecognized_producer_field"] = True
        private_file(manifest_path, encoded_json(manifest))
        with self.assertRaisesRegex(QUEUE.QueueError, "snapshot manifest has wrong fields"):
            self.run_queue()
        self.assertFalse(self.launches.exists())
        self.assertFalse((self.output / "queue-request.json").exists())

    def test_legacy_replicated_fact_limits_are_not_reinterpreted(self) -> None:
        manifest_path = self.source / "snapshot-workspace.json"
        manifest = json.loads(manifest_path.read_bytes())
        limits = manifest["limits"]
        limits["max_bank_fact_product"] = limits.pop("max_projected_fact_rows")
        limits["max_replicated_base_fact_bytes"] = limits.pop(
            "max_projected_fact_bytes"
        )
        private_file(manifest_path, encoded_json(manifest))
        with self.assertRaisesRegex(QUEUE.QueueError, "producer limits has wrong fields"):
            self.run_queue()
        self.assertFalse(self.launches.exists())
        self.assertFalse((self.output / "queue-request.json").exists())

    def test_legacy_duplicated_snapshot_wire_is_not_reinterpreted(self) -> None:
        manifest_path = self.source / "snapshot-workspace.json"
        manifest = json.loads(manifest_path.read_bytes())
        manifest["snapshot_wire"]["schema_version"] = 2
        manifest["snapshot_wire"]["duplicates_fact_db_per_bank"] = True
        private_file(manifest_path, encoded_json(manifest))
        with self.assertRaisesRegex(
            QUEUE.QueueError, "snapshot wire is not admitted projected diagnostic v6"
        ):
            self.run_queue()
        self.assertFalse(self.launches.exists())
        self.assertFalse((self.output / "queue-request.json").exists())

    def test_virtual_affine_backing_requires_exactly_one_evidence_index(self) -> None:
        manifest_path = self.source / "snapshot-workspace.json"
        manifest = json.loads(manifest_path.read_bytes())
        manifest["banks"][0]["backing"]["rom_space"] = "Virtual"
        manifest["banks"][0]["backing_evidence_fact_indices"] = [7]
        private_file(manifest_path, encoded_json(manifest))

        _, _, _, banks = QUEUE.validate_manifest(self.source)
        self.assertEqual(banks[0].rom_space, "Virtual")
        self.assertEqual(banks[0].rom_start, 0)
        self.assertEqual(banks[0].rom_end, 4)

        manifest["banks"][0]["backing_evidence_fact_indices"] = []
        private_file(manifest_path, encoded_json(manifest))
        with self.assertRaisesRegex(QUEUE.QueueError, "backing evidence does not match"):
            QUEUE.validate_manifest(self.source)

    def test_materialized_backing_is_rejected_without_fabricating_rom_coordinates(self) -> None:
        manifest_path = self.source / "snapshot-workspace.json"
        manifest = json.loads(manifest_path.read_bytes())
        manifest["banks"][0]["backing"] = {
            "kind": "materialized",
            "receipt_sha256": "22" * 32,
            "output_start": 0,
            "output_end": manifest["banks"][0]["byte_length"],
        }
        private_file(manifest_path, encoded_json(manifest))

        with self.assertRaisesRegex(QUEUE.QueueError, "materialized backing is unsupported"):
            QUEUE.validate_manifest(self.source)

    def test_legacy_ineligible_seed_mode_is_not_admitted_by_current_producer(self) -> None:
        manifest_path = self.source / "snapshot-workspace.json"
        manifest = json.loads(manifest_path.read_bytes())
        manifest["banks"][0]["ghidra_seeds"] = {
            "mode": "ineligible",
            "reason": "no_proven_owner",
        }
        private_file(manifest_path, encoded_json(manifest))
        with self.assertRaisesRegex(QUEUE.QueueError, "unknown seed mode"):
            self.run_queue()
        self.assertFalse(self.launches.exists())
        self.assertFalse((self.output / "queue-request.json").exists())

    def test_malformed_runner_receipt_is_retained_as_failure(self) -> None:
        self.source = make_snapshot_workspace(self.root / "one-bank", bank_count=1)
        self.assertEqual(self.run_queue(mode="malformed"), 1)
        result = json.loads(
            (self.output / "banks/000000/attempts/000001/result.json").read_bytes()
        )
        self.assertEqual(result["state"], "failure")
        self.assertEqual(result["failure_class"], "invalid_runner_completion")
        self.assertFalse((self.output / "queue-receipt.json").exists())

    def test_extra_distribution_cache_entry_is_rejected(self) -> None:
        self.source = make_snapshot_workspace(self.root / "extra-cache", bank_count=1)
        self.assertEqual(self.run_queue(mode="extra_cache"), 1)
        result = json.loads(
            (self.output / "banks/000000/attempts/000001/result.json").read_bytes()
        )
        self.assertEqual(result["failure_class"], "invalid_runner_completion")
        self.assertFalse((self.output / "queue-receipt.json").exists())

    def test_tampered_distribution_cache_entry_is_rejected(self) -> None:
        self.source = make_snapshot_workspace(self.root / "tampered-cache", bank_count=1)
        self.assertEqual(self.run_queue(mode="cache_tamper"), 1)
        result = json.loads(
            (self.output / "banks/000000/attempts/000001/result.json").read_bytes()
        )
        self.assertEqual(result["failure_class"], "invalid_runner_completion")
        self.assertFalse((self.output / "queue-receipt.json").exists())

    def test_symlinked_distribution_cache_entry_is_rejected(self) -> None:
        self.source = make_snapshot_workspace(self.root / "symlink-cache", bank_count=1)
        with self.assertRaisesRegex(QUEUE.QueueError, "symlink"):
            self.run_queue(mode="cache_symlink")
        self.assertFalse((self.output / "queue-receipt.json").exists())

    def test_forged_existing_final_receipt_is_rejected(self) -> None:
        self.assertEqual(self.run_queue(), 0)
        receipt_path = self.output / "queue-receipt.json"
        receipt = json.loads(receipt_path.read_bytes())
        receipt["state"] = "forged"
        private_file(receipt_path, encoded_json(receipt))
        with self.assertRaisesRegex(QUEUE.QueueError, "queue receipt does not match"):
            self.run_queue()
        self.assertEqual(self.launches.read_text().splitlines(), ["bank-0", "bank-1"])

    def test_source_mutation_while_runner_is_active_is_rejected(self) -> None:
        self.source = make_snapshot_workspace(self.root / "mutated", bank_count=1)
        with self.assertRaisesRegex(QUEUE.QueueError, "input changed while its runner was active"):
            self.run_queue(mode="mutate")
        self.assertFalse(
            (self.output / "banks/000000/attempts/000001/result.json").exists()
        )

    def test_log_cap_kills_term_ignoring_process_group(self) -> None:
        self.source = make_snapshot_workspace(self.root / "flood", bank_count=1)
        started = time.monotonic()
        self.assertEqual(
            self.run_queue(
                mode="flood",
                limits=self.limits(
                    max_log_bytes=1024,
                    termination_grace_seconds=1,
                ),
            ),
            1,
        )
        self.assertLess(time.monotonic() - started, 5)
        result = json.loads(
            (self.output / "banks/000000/attempts/000001/result.json").read_bytes()
        )
        self.assertEqual(result["failure_class"], "log_cap")
        self.assertTrue(result["stop_scheduling"])
        self.assertEqual((self.output / "banks/000000/attempts/000001/stdout.log").stat().st_size, 1024)
        descendant_pid = int(self.descendant.read_text().strip())
        deadline = time.monotonic() + 2
        while time.monotonic() < deadline:
            try:
                os.kill(descendant_pid, 0)
            except ProcessLookupError:
                break
            time.sleep(0.02)
        else:
            self.fail(f"TERM-ignoring runner descendant {descendant_pid} survived group kill")

    def test_log_cap_kills_descendant_that_holds_pipes_after_parent_exit(self) -> None:
        self.source = make_snapshot_workspace(self.root / "orphan-pipes", bank_count=1)
        started = time.monotonic()
        self.assertEqual(
            self.run_queue(
                mode="orphan_pipes",
                limits=self.limits(max_log_bytes=1024, termination_grace_seconds=1),
            ),
            1,
        )
        # One graceful second plus the queue's bounded forced pipe-drain phase;
        # allow scheduler contention without weakening the child-gone oracle.
        self.assertLess(time.monotonic() - started, 8)
        result = json.loads(
            (self.output / "banks/000000/attempts/000001/result.json").read_bytes()
        )
        self.assertEqual(result["failure_class"], "log_cap")
        descendant_pid = int(self.descendant.read_text().strip())
        deadline = time.monotonic() + 1
        while time.monotonic() < deadline:
            try:
                os.kill(descendant_pid, 0)
            except ProcessLookupError:
                break
            time.sleep(0.02)
        else:
            self.fail(
                f"runner descendant {descendant_pid} survived after its parent exited"
            )


if __name__ == "__main__":
    unittest.main()
