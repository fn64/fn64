#!/usr/bin/env python3
"""ROM-free regression tests for cold-coverage-panel.py."""

from __future__ import annotations

import errno
import hashlib
import importlib.util
import json
import os
import subprocess
import sys
import tempfile
import time
import unittest
from unittest import mock
from pathlib import Path


SCRIPT = Path(__file__).resolve().with_name("cold-coverage-panel.py")
SPEC = importlib.util.spec_from_file_location("cold_coverage_panel", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
PANEL = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = PANEL
SPEC.loader.exec_module(PANEL)


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def encoded(value: object) -> bytes:
    return (json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n").encode()


class PanelFixture:
    def __init__(self, root: Path):
        self.root = root
        self.roms = {name: root / f"{name}.z64" for name in ("alpha", "beta")}
        self.roms["alpha"].write_text("ok")
        self.roms["beta"].write_text("ok")
        self.digests = {"alpha": "1" * 64, "beta": "2" * 64}
        self.pid_path = root / "grandchild.pid"
        self.counter_path = root / "counter"
        self.binary = root / "fn64-discover"
        self.binary.write_text(self.dispatcher())
        self.binary.chmod(0o700)
        self.manifest = root / "manifest.json"
        self.write_manifest(["alpha"])

    def dispatcher(self) -> str:
        return f"""#!{sys.executable}
import hashlib,json,os,subprocess,sys,time
from pathlib import Path
assert sys.argv[1]=='__cold-rom-child' and len(sys.argv)==4
mode=Path(sys.argv[2]).read_text(); expected=sys.argv[3]
pid_path=Path({str(self.pid_path)!r}); counter_path=Path({str(self.counter_path)!r})
if mode=='fail': sys.exit(9)
if mode=='hang':
 child=subprocess.Popen([sys.executable,'-c','import time; time.sleep(60)']); pid_path.write_text(str(child.pid)); time.sleep(60)
if mode=='output':
 sys.stdout.write('x'*({PANEL.MAX_SUBPROCESS_OUTPUT_BYTES}+1)); sys.stdout.flush(); time.sleep(60)
if mode=='rss':
 child=subprocess.Popen([sys.executable,'-c','import time; allocation=bytearray(64*1024*1024); time.sleep(60)']); pid_path.write_text(str(child.pid)); time.sleep(60)
if mode=='survivor':
 child=subprocess.Popen([sys.executable,'-c','import time; time.sleep(60)']); pid_path.write_text(str(child.pid))
schema='fn64.cold-rom-measurement.v1' if mode=='schema' else 'fn64.cold-rom-measurement.v2'
limits={{'max_rom_input_bytes':67108864,'max_decoded_vrom_file_bytes':67108864,'max_banks':4096,'max_aggregate_materialized_bytes':67108864,'max_projected_fact_rows':4000000,'max_projected_fact_bytes':268435456,'max_cross_bank_authority_records':1048576}}
outcome={{'strategy':'boot_bank_open','candidate_tables':0,'admitted_tables':0,'admitted_intervals':0,'decoded_file_limit_hits':0,'proven_mappings':0,'supported_mappings':0,'request_dma_open_rows':0,'request_dma_incomplete':False,'request_dma_input_limit_hit':False,'physical_wrapper_candidates_examined':0,'wrapper_semantic_proof_unavailable':0,'physical_wrapper_candidate_limit_hit':False}}
measurement={{'schema':schema,'limits':limits,'normalized_rom_sha256':expected,'selected_strategy':'boot_bank_open','strategy_outcomes':[outcome],'fact_count':0,'overlay_relocation_fact_count':0,'proven_bank_count':0,'closure':{{'status':'open','blocker':'no_proven_mappings'}},'stage1_effects':{{'status':'open','blocker':'composition_unavailable'}},'ledger_total_bytes':0,'ledger_code_like_floor_bytes':0,'ledger_bytes_by_class':{{}}}}
if mode=='wrong-digest': measurement['normalized_rom_sha256']='f'*64
if mode=='nested': measurement['limits']={{}}
if mode=='bad-bool': measurement['strategy_outcomes'][0]['request_dma_incomplete']=0
if mode=='bad-ledger': measurement['ledger_total_bytes']=1
if mode=='bad-closure': measurement['closure']={{'status':'measured','scoreboard':{{'total_destinations':1,'per_class':{{'exact_aot':{{'destinations':0,'bytes':0}},'block_aot':{{'destinations':0,'bytes':0}},'dynamic_mips':{{'destinations':0,'bytes':0}},'unsupported':{{'destinations':0,'bytes':0}}}},'per_reason':{{'in_exact_owner':0,'in_proven_block':0,'open_indirect_site':0,'bounded_indirect_site':0,'mapped_not_proven_code':0,'proven_code_no_owner':0,'into_proven_data':0,'outside_all_mappings':0}},'unsupported':0,'dynamic_mips':0}}}}
if mode=='nondeterministic':
 count=int(counter_path.read_text())+1 if counter_path.exists() else 1; counter_path.write_text(str(count)); measurement['fact_count']=count
claimed=hashlib.sha256(json.dumps(measurement,separators=(',',':')).encode()).hexdigest()
if mode=='tamper': claimed='0'*64
receipt={{'measurement':measurement,'receipt_sha256':claimed}}
if mode=='trailing': print(json.dumps(receipt,separators=(',',':'))+' trailing')
else: print(json.dumps(receipt,separators=(',',':')))
"""

    def write_manifest(self, names: list[str], **outer: object) -> None:
        entries = [
            {
                "stable_id": name,
                "rom_path": str(self.roms[name]),
                "expected_normalized_rom_sha256": self.digests[name],
            }
            for name in names
        ]
        value = {
            "schema": PANEL.MANIFEST_SCHEMA,
            "schema_version": 1,
            "entries": entries,
            **outer,
        }
        self.manifest.write_bytes(encoded(value))

    def run(self, *, repetitions: int = 2, ok: bool = True, extra: list[str] | None = None):
        command = [
            sys.executable,
            str(SCRIPT),
            "--manifest",
            str(self.manifest),
            "--binary",
            str(self.binary),
            "--repetitions",
            str(repetitions),
            "--timeout-seconds",
            "5",
            "--max-rss-mib",
            "512",
            "--min-free-percent",
            "0",
            "--poll-milliseconds",
            "25",
            *(extra or []),
        ]
        result = subprocess.run(command, capture_output=True, text=True, timeout=20)
        if ok and result.returncode != 0:
            raise AssertionError(result.stderr)
        if not ok and result.returncode == 0:
            raise AssertionError(result.stdout)
        return result


class ColdCoveragePanelTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="fn64-cold-panel-test-")
        self.fixture = PanelFixture(Path(self.temp.name).resolve())

    def tearDown(self):
        self.temp.cleanup()

    def assert_process_gone(self, pid: int) -> None:
        for _ in range(100):
            try:
                os.kill(pid, 0)
            except ProcessLookupError:
                return
            time.sleep(0.02)
        self.fail(f"process {pid} survived its bounded child")

    def test_success_buffers_deterministic_repetitions_and_distributions(self):
        result = self.fixture.run(repetitions=3)
        records = [json.loads(line) for line in result.stdout.splitlines()]
        self.assertEqual(len(records), 4)
        self.assertEqual([record["run_index"] for record in records[:-1]], [1, 2, 3])
        self.assertTrue(all(record["stable_id"] == "alpha" for record in records[:-1]))
        final = records[-1]
        self.assertEqual(final["schema"], PANEL.RESULT_SCHEMA)
        self.assertEqual(final["repetitions"], 3)
        executable_sha256 = digest(self.fixture.binary.read_bytes())
        self.assertEqual(final["fn64_discover_sha256"], executable_sha256)
        self.assertEqual(len(final["entries"][0]["wall_ms_distribution"]), 3)
        self.assertEqual(len(final["entries"][0]["peak_rss_bytes_distribution"]), 3)
        self.assertRegex(final["panel_sha256"], r"^[0-9a-f]{64}$")
        self.assertEqual(
            final["subprocess_limits"]["output_enforcement"], "rlimit_fsize"
        )
        self.assertEqual(
            final["subprocess_limits"]["rss_enforcement"],
            "sampled_process_group_watchdog",
        )
        retained = final["entries"][0]
        PANEL.parse_receipt(
            json.dumps(retained["receipt"], sort_keys=True).encode(),
            retained["normalized_rom_sha256"],
        )
        deterministic = {
            "schema": "fn64.cold-coverage-panel-deterministic.v1",
            "schema_version": 1,
            "repetitions": 3,
            "fn64_discover_sha256": executable_sha256,
            "entries": [
                {
                    "stable_id": retained["stable_id"],
                    "normalized_rom_sha256": retained["normalized_rom_sha256"],
                    "receipt_sha256": retained["receipt_sha256"],
                    "receipt": retained["receipt"],
                }
            ],
        }
        self.assertEqual(
            final["panel_sha256"], digest(PANEL.canonical_sorted(deterministic))
        )
        self.assertNotIn(str(self.fixture.root), result.stdout)
        self.assertNotIn(".z64", result.stdout)

    def test_multi_rom_aggregate_distributions_are_per_panel_repetition(self):
        self.fixture.write_manifest(["alpha", "beta"])
        result = self.fixture.run(repetitions=3)
        records = [json.loads(line) for line in result.stdout.splitlines()]
        observations = records[:-1]
        final = records[-1]
        self.assertEqual(len(observations), 6)
        wall_by_run = [
            sum(record["wall_ms"] for record in observations if record["run_index"] == run)
            for run in (1, 2, 3)
        ]
        peak_by_run = [
            max(
                record["peak_rss_bytes"]
                for record in observations
                if record["run_index"] == run
            )
            for run in (1, 2, 3)
        ]
        self.assertEqual(final["aggregate_wall_ms_distribution"], sorted(wall_by_run))
        self.assertEqual(
            final["aggregate_peak_rss_bytes_distribution"], sorted(peak_by_run)
        )
        self.assertEqual(len(final["aggregate_wall_ms_distribution"]), 3)

    def test_manifest_is_strict_sorted_digest_bound_and_no_symlink(self):
        cases = []
        self.fixture.write_manifest(["alpha"], unknown=True)
        cases.append(self.fixture.manifest.read_bytes())
        self.fixture.write_manifest(["beta", "alpha"])
        cases.append(self.fixture.manifest.read_bytes())
        self.fixture.write_manifest(["alpha", "beta"])
        value = json.loads(self.fixture.manifest.read_text())
        value["entries"][1]["expected_normalized_rom_sha256"] = self.fixture.digests[
            "alpha"
        ]
        cases.append(encoded(value))
        for case in cases:
            self.fixture.manifest.write_bytes(case)
            self.assertEqual(self.fixture.run(ok=False).stdout, "")

        link = self.fixture.root / "linked.z64"
        link.symlink_to(self.fixture.roms["alpha"])
        value = {
            "schema": PANEL.MANIFEST_SCHEMA,
            "schema_version": 1,
            "entries": [{
                "stable_id": "alpha",
                "rom_path": str(link),
                "expected_normalized_rom_sha256": self.fixture.digests["alpha"],
            }],
        }
        self.fixture.manifest.write_bytes(encoded(value))
        self.assertEqual(self.fixture.run(ok=False).stdout, "")

    def test_manifest_rejects_oversized_rom_before_launch(self):
        oversized = self.fixture.root / "oversized.z64"
        with oversized.open("wb") as output:
            output.truncate(PANEL.MAX_ROM_BYTES + 1)
        value = {
            "schema": PANEL.MANIFEST_SCHEMA,
            "schema_version": 1,
            "entries": [{
                "stable_id": "oversized",
                "rom_path": str(oversized),
                "expected_normalized_rom_sha256": "3" * 64,
            }],
        }
        self.fixture.manifest.write_bytes(encoded(value))
        self.assertEqual(self.fixture.run(ok=False).stdout, "")

    def test_receipt_schema_digest_trailing_and_nondeterminism_fail_atomically(self):
        for mode in (
            "schema",
            "wrong-digest",
            "tamper",
            "trailing",
            "nondeterministic",
            "nested",
            "bad-bool",
            "bad-ledger",
            "bad-closure",
        ):
            self.fixture.roms["alpha"].write_text(mode)
            result = self.fixture.run(repetitions=2, ok=False)
            self.assertEqual(result.stdout, "", mode)

    def test_later_child_failure_emits_no_partial_stdout(self):
        self.fixture.write_manifest(["alpha", "beta"])
        self.fixture.roms["beta"].write_text("fail")
        output = self.fixture.root / "failed-panel.jsonl"
        result = self.fixture.run(
            repetitions=1, ok=False, extra=["--output", str(output)]
        )
        self.assertEqual(result.stdout, "")
        self.assertIn("beta failed: child_exit_9", result.stderr)
        self.assertFalse(output.exists())

    def test_durable_output_is_atomic_private_and_leaves_no_temporary(self):
        output = self.fixture.root / "panel.jsonl"
        result = self.fixture.run(
            repetitions=2, extra=["--output", str(output)]
        )
        self.assertEqual(result.stdout, "")
        records = [json.loads(line) for line in output.read_text().splitlines()]
        self.assertEqual(len(records), 3)
        self.assertEqual(records[-1]["schema"], PANEL.RESULT_SCHEMA)
        self.assertEqual(output.stat().st_mode & 0o777, 0o600)
        self.assertEqual(
            list(self.fixture.root.glob(f".{output.name}.tmp-*")), []
        )

    def test_durable_output_refuses_overwrite_and_symlink_parent(self):
        output = self.fixture.root / "existing.jsonl"
        output.write_text("owner-data")
        result = self.fixture.run(
            repetitions=1, ok=False, extra=["--output", str(output)]
        )
        self.assertEqual(result.stdout, "")
        self.assertEqual(output.read_text(), "owner-data")

        real_parent = self.fixture.root / "real-output"
        real_parent.mkdir()
        linked_parent = self.fixture.root / "linked-output"
        linked_parent.symlink_to(real_parent, target_is_directory=True)
        linked_output = linked_parent / "panel.jsonl"
        result = self.fixture.run(
            repetitions=1, ok=False, extra=["--output", str(linked_output)]
        )
        self.assertEqual(result.stdout, "")
        self.assertFalse((real_parent / "panel.jsonl").exists())

    def test_post_link_durability_failure_removes_only_our_artifact(self):
        output = self.fixture.root / "fsync-failure.jsonl"
        records = [{"schema": "test-only", "schema_version": 1}]
        with mock.patch.object(
            PANEL.os,
            "fsync",
            side_effect=[None, OSError(errno.EIO, "injected directory fsync failure")],
        ):
            with self.assertRaisesRegex(PANEL.PanelError, "publishing durable"):
                PANEL.publish_records(output, records)
        self.assertFalse(output.exists())
        self.assertEqual(
            list(self.fixture.root.glob(f".{output.name}.tmp-*")), []
        )

    def bounded_direct(self, mode: str, **changes: object):
        self.fixture.roms["alpha"].write_text(mode)
        scratch = self.fixture.root / f"scratch-{mode}"
        scratch.mkdir(mode=0o700)
        values = {
            "timeout_seconds": 2.0,
            "max_rss_bytes": 512 * 1024 * 1024,
            "min_free_percent": 0,
            "poll_milliseconds": 25,
            **changes,
        }
        return PANEL.run_bounded(
            [
                str(self.fixture.binary),
                "__cold-rom-child",
                str(self.fixture.roms["alpha"]),
                self.fixture.digests["alpha"],
            ],
            stable_id="alpha",
            scratch=scratch,
            **values,
        )

    def test_timeout_kills_complete_process_group(self):
        with self.assertRaisesRegex(PANEL.PanelError, "timeout"):
            self.bounded_direct("hang", timeout_seconds=0.25)
        pid = int(self.fixture.pid_path.read_text())
        self.assert_process_gone(pid)

    def test_output_bound_kills_child(self):
        with self.assertRaisesRegex(PANEL.PanelError, "output_limit"):
            self.bounded_direct("output")

    def test_kernel_file_limit_bounds_output_overshoot(self):
        output = self.fixture.root / "kernel-limited-output"
        with output.open("wb") as stream:
            result = subprocess.run(
                [
                    sys.executable,
                    "-c",
                    "import os; chunk=b'x'*1048576; [os.write(1, chunk) for _ in range(8)]",
                ],
                stdin=subprocess.DEVNULL,
                stdout=stream,
                stderr=subprocess.DEVNULL,
                preexec_fn=lambda: PANEL.apply_child_resource_limits(4096, "none", None),
                timeout=5,
                check=False,
            )
        self.assertNotEqual(result.returncode, 0)
        self.assertLessEqual(output.stat().st_size, 4096)

    def test_inherited_hard_memory_backstop_rejects_large_allocation(self):
        kind, limit = PANEL.hard_memory_limit(1)
        if kind == "none":
            self.skipTest("platform has no supported inherited hard memory backstop")
        assert limit is not None
        result = subprocess.run(
            [sys.executable, "-c", f"allocation=bytearray({limit * 2})"],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            preexec_fn=lambda: PANEL.apply_child_resource_limits(
                PANEL.MAX_SUBPROCESS_OUTPUT_BYTES, kind, limit
            ),
            timeout=5,
            check=False,
        )
        self.assertNotEqual(result.returncode, 0)

    def test_aggregate_rss_bound_kills_allocating_descendant(self):
        with self.assertRaisesRegex(PANEL.PanelError, "memory_rss_limit"):
            self.bounded_direct("rss", max_rss_bytes=24 * 1024 * 1024)
        pid = int(self.fixture.pid_path.read_text())
        self.assert_process_gone(pid)

    def test_successful_leader_cannot_leave_group_survivor(self):
        with self.assertRaisesRegex(PANEL.PanelError, "child_survivors"):
            self.bounded_direct("survivor")
        pid = int(self.fixture.pid_path.read_text())
        self.assert_process_gone(pid)

    def test_group_teardown_wait_is_bounded(self):
        process = mock.Mock()
        process.pid = 12345
        process.wait.side_effect = subprocess.TimeoutExpired(
            cmd=["test-only"], timeout=PANEL.TEARDOWN_TIMEOUT_SECONDS
        )
        with mock.patch.object(PANEL.os, "killpg"):
            self.assertFalse(PANEL.kill_group(process))
        process.wait.assert_called_once_with(timeout=PANEL.TEARDOWN_TIMEOUT_SECONDS)


if __name__ == "__main__":
    unittest.main()
