#!/usr/bin/env python3
"""No-ROM regression tests for cold-training-fold.py."""

from __future__ import annotations

import hashlib
import importlib.util
import json
import os
import stat
import subprocess
import sys
import tempfile
import unittest
from unittest import mock
from pathlib import Path

SCRIPT = Path(__file__).resolve().with_name("cold-training-fold.py")
SPEC = importlib.util.spec_from_file_location("cold_training_fold", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
FOLD_MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(FOLD_MODULE)


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def canonical(value: object) -> bytes:
    return (json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n").encode()


class FoldFixture:
    def __init__(self, root: Path):
        self.root = root
        self.bin = root / "bin"
        self.bin.mkdir(mode=0o700)
        self.log = root / "calls.jsonl"
        self.roms = {}
        self.dumps = {}
        for game in ("held", "train"):
            rom = root / f"{game}.z64"
            dump = root / f"{game}.toml"
            rom.write_bytes((game + "-rom").encode())
            dump.write_bytes((game + "-dump").encode())
            self.roms[game] = rom
            self.dumps[game] = dump
        dispatcher = self._dispatcher()
        for name in ("rom_identity", "produce_snapshot_workspace", "validate_training_workspace", "attribute_known_functions"):
            path = self.bin / name
            path.write_text(dispatcher)
            path.chmod(0o700)
        self.manifest = root / "prepare.json"
        self.write_manifest()

    def _dispatcher(self) -> str:
        return f"""#!{sys.executable}
import hashlib,json,os,subprocess,sys,time
from pathlib import Path
name=Path(sys.argv[0]).name
with open({str(self.log)!r},'a') as log: log.write(json.dumps({{'name':name,'args':sys.argv[1:]}})+'\\n')
sha=lambda b:hashlib.sha256(b).hexdigest()
if name=='rom_identity':
 p=Path(sys.argv[1]); print(json.dumps({{'schema':'fn64.rom-identity','schema_version':1,'normalized_rom_sha256':sha(p.read_bytes()),'source_byte_order':'z64','byte_length':p.stat().st_size,'entry_point':0}}))
elif name=='produce_snapshot_workspace':
 _,rom,out=sys.argv[1:]; out=Path(out); rd=sha(Path(rom).read_bytes()); candidate='1'*64
 manifest={{'schema':'fn64.snapshot-workspace','schema_version':4,'state':'open','open_reason':'no_proven_banks','normalized_rom_sha256':rd,'discovery':{{}},'limits':{{}},'snapshot_wire':{{'schema_version':6,'authority':'diagnostic_only','duplicates_fact_db_per_bank':False,'remaining_large_rom_frontier':'streaming_v6'}},'aggregate_snapshot_artifact_bytes':0,'rom_recompilation_complete':False,'remaining_recompilation_frontier':'proven_bank_and_callable_owner_closure','intended_use':'sealed_cold_function_training_input','cold_training':{{'schema_version':3,'algorithm':'fn64.cold-function-training.v3','answer_key_present':False,'candidate_artifact':'cold-candidates.json','candidate_artifact_byte_length':0,'candidate_artifact_sha256':sha(b''),'scoped_candidate_identities_v3_sha256':candidate}},'banks':[]}}
 (out/'snapshot-workspace.json').write_text(json.dumps(manifest)); os.chmod(out/'snapshot-workspace.json',0o600)
elif name=='validate_training_workspace':
 if Path({str(self.root / 'survivor-validator')!r}).exists():
  child=subprocess.Popen([sys.executable,'-c','import time; time.sleep(60)'])
  Path({str(self.root / 'grandchild.pid')!r}).write_text(str(child.pid))
 elif Path({str(self.root / 'allocate-validator')!r}).exists():
  child=subprocess.Popen([sys.executable,'-c','import time; allocation=bytearray(64*1024*1024); time.sleep(60)'])
  Path({str(self.root / 'grandchild.pid')!r}).write_text(str(child.pid))
  time.sleep(60)
 elif Path({str(self.root / 'hang-validator')!r}).exists():
  child=subprocess.Popen([sys.executable,'-c','import time; time.sleep(60)'])
  Path({str(self.root / 'grandchild.pid')!r}).write_text(str(child.pid))
  time.sleep(60)
 print('validated')
elif name=='attribute_known_functions':
 if sys.argv[1]=='--validate-report':
  _,path,cold,dump,rom_sha,cold_sha,candidate_sha,dump_sha=sys.argv[1:]
  report=json.loads(Path(path).read_text())
  assert sha((Path(cold)/'snapshot-workspace.json').read_bytes())==cold_sha
  assert sha(Path(dump).read_bytes())==dump_sha
  assert report['normalized_rom_sha256']==rom_sha and report['cold_workspace_manifest_sha256']==cold_sha
  assert report['cold_candidate_identities_v3_sha256']==candidate_sha and report['answer_key_sha256']==dump_sha
  assert report['schema_version']==2 and report['algorithm']=='fn64.known-function-attribution.v2'
  print(json.dumps({{'schema':'fn64.known-function-attribution-validation.v1','schema_version':1,'report_sha256':sha(Path(path).read_bytes())}},separators=(',',':')))
 else:
  cold,dump,rom_sha,dump_sha,out=sys.argv[1:]; out=Path(out); m=(Path(cold)/'snapshot-workspace.json').read_bytes(); manifest=json.loads(m)
  candidate_totals={{key:0 for key in ('denominator','gradable','ungradable','combined','per_detector_only','candidate_matched','ambiguous_answer_mapping','interior','gap','outside')}}
  totals={{key:0 for key in ('raw_rows','nonzero_rows','distinct_bodies','alias_rows','marker_rows','candidate_matched_rows','missed_rows')}}
  inner={{'schema_version':1,'sections':[],'rows':[],'candidate_statuses':[],'candidate_totals':candidate_totals,'totals':totals,'per_domain':[]}}
  inner['canonical_sha256']=sha(json.dumps(inner,separators=(',',':')).encode())
  report={{'schema_version':2,'algorithm':'fn64.known-function-attribution.v2','normalized_rom_sha256':rom_sha,'cold_workspace_manifest_sha256':sha(m),'cold_candidate_identities_v3_sha256':manifest['cold_training']['scoped_candidate_identities_v3_sha256'],'answer_key_sha256':dump_sha,'answer_key_execution_domain':'unknown','report':inner}}
  if Path({str(self.root / 'bad-report')!r}).exists(): report['algorithm']='not-the-real-algorithm'
  (out/'known-function-attribution.json').write_text(json.dumps(report)); os.chmod(out/'known-function-attribution.json',0o600)
"""

    def write_manifest(self, *, held_key: bool = False, extra: bool = False) -> None:
        answer = {"format": "fn64.dump-toml.v1", "path": str(self.dumps["train"]), "expected_sha256": digest(self.dumps["train"].read_bytes()), "license_disposition": "test-only"}
        held = {"id": "held", "family": "family_b", "rom_path": str(self.roms["held"]), "expected_normalized_rom_sha256": digest(self.roms["held"].read_bytes())}
        if held_key:
            held["answer_key"] = {**answer, "path": str(self.dumps["held"]), "expected_sha256": digest(self.dumps["held"].read_bytes())}
        value = {"schema": "fn64.loo-prepare-input.v1", "schema_version": 1, "fold_id": "fold_a", "training_ids": ["train"], "held_out_id": "held", "entries": [held, {"id": "train", "family": "family_a", "rom_path": str(self.roms["train"]), "expected_normalized_rom_sha256": digest(self.roms["train"].read_bytes()), "answer_key": answer}]}
        if extra:
            value["unknown"] = True
        self.manifest.write_bytes(canonical(value))

    def run_command(self, *args: str, ok: bool = True) -> subprocess.CompletedProcess[str]:
        result = subprocess.run([sys.executable, str(SCRIPT), *args], text=True, capture_output=True)
        if ok and result.returncode != 0:
            raise AssertionError(result.stderr)
        if not ok and result.returncode == 0:
            raise AssertionError(result.stdout)
        return result

    def prepare(self, run: Path, ok: bool = True):
        return self.run_command("prepare", "--manifest", str(self.manifest), "--run", str(run), "--bin-dir", str(self.bin), "--timeout-seconds", "10", ok=ok)


class ColdTrainingFoldTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="fn64-fold-test-")
        self.fixture = FoldFixture(Path(self.temp.name).resolve())

    def tearDown(self):
        self.temp.cleanup()

    def test_two_phase_order_exclusion_and_create_new(self):
        run = self.fixture.root / "run"
        self.fixture.prepare(run)
        calls = [json.loads(line) for line in self.fixture.log.read_text().splitlines()]
        attributes = [call for call in calls if call["name"] == "attribute_known_functions" and call["args"][0] != "--validate-report"]
        self.assertEqual(len(attributes), 1)
        self.assertEqual(Path(attributes[0]["args"][1]).name, "train.dump.toml")
        self.assertNotEqual(attributes[0]["args"][1], str(self.fixture.dumps["train"]))
        self.assertNotIn(str(self.fixture.dumps["held"]), self.fixture.log.read_text())
        training = run / "fold" / "training-receipt.json"
        self.assertEqual(stat.S_IMODE(training.stat().st_mode), 0o600)
        training_sha = digest(training.read_bytes())
        mechanism = self.fixture.root / "mechanism.json"
        mechanism.write_bytes(canonical({"schema": "fn64.discovery-mechanism.v1", "schema_version": 1, "algorithm": "test", "source_revision_or_patch_digest": "2" * 64, "parameter_digest": "3" * 64, "training_receipt_sha256": training_sha, "training_ids": ["train"], "held_out_id": "held"}))
        self.fixture.run_command("freeze", "--run", str(run), "--mechanism", str(mechanism), "--expected-training-receipt-sha256", training_sha)
        freeze = run / "fold" / "freeze.json"
        freeze_sha = digest(freeze.read_bytes())
        admission = self.fixture.root / "admission.json"
        admission_value = {"schema": "fn64.loo-heldout-key-admission.v1", "schema_version": 1, "fold_id": "fold_a", "held_out_id": "held", "dump_path": str(self.fixture.dumps["held"]), "expected_dump_sha256": digest(self.fixture.dumps["held"].read_bytes()), "expected_rom_sha256": digest(self.fixture.roms["held"].read_bytes()), "expected_freeze_sha256": freeze_sha}
        admission.write_bytes(canonical(admission_value))
        self.fixture.run_command("grade-heldout", "--run", str(run), "--admission", str(admission), "--expected-freeze-sha256", "0" * 64, "--bin-dir", str(self.fixture.bin), ok=False)
        admission_value["expected_dump_sha256"] = "0" * 64
        admission.write_bytes(canonical(admission_value))
        self.fixture.run_command("grade-heldout", "--run", str(run), "--admission", str(admission), "--expected-freeze-sha256", freeze_sha, "--bin-dir", str(self.fixture.bin), ok=False)
        admission_value["expected_dump_sha256"] = digest(self.fixture.dumps["held"].read_bytes())
        admission.write_bytes(canonical(admission_value))
        loosened = self.fixture.run_command("grade-heldout", "--run", str(run), "--admission", str(admission),
                                            "--expected-freeze-sha256", freeze_sha, "--bin-dir", str(self.fixture.bin),
                                            "--max-rss-mib", "4096", ok=False)
        self.assertIn("subprocess limits differ", loosened.stderr)
        self.fixture.run_command("grade-heldout", "--run", str(run), "--admission", str(admission), "--expected-freeze-sha256", freeze_sha, "--bin-dir", str(self.fixture.bin), "--timeout-seconds", "10")
        calls = [json.loads(line) for line in self.fixture.log.read_text().splitlines()]
        self.assertEqual([Path(call["args"][1]).name for call in calls if call["name"] == "attribute_known_functions" and call["args"][0] != "--validate-report"], ["train.dump.toml", "answer-key.toml"])
        held = json.loads((run / "heldout" / "held-out-receipt.json").read_text())
        self.assertEqual([event["event"] for event in held["events"]], ["freeze_validated", "held_out_key_admitted_by_orchestrator", "held_out_report_validated"])
        self.fixture.prepare(run, ok=False)
        self.fixture.run_command("freeze", "--run", str(run), "--mechanism", str(mechanism), "--expected-training-receipt-sha256", training_sha, ok=False)
        self.fixture.run_command("grade-heldout", "--run", str(run), "--admission", str(admission), "--expected-freeze-sha256", freeze_sha, "--bin-dir", str(self.fixture.bin), ok=False)

    def test_failure_is_transactional_and_retryable(self):
        run = self.fixture.root / "transactional"
        marker = self.fixture.root / "bad-report"
        marker.touch()
        self.fixture.prepare(run, ok=False)
        self.assertFalse(run.exists())
        marker.unlink()
        self.fixture.prepare(run)
        self.assertTrue((run / "fold" / "training-receipt.json").is_file())

    def test_executable_identity_is_frozen(self):
        run = self.fixture.root / "identity"
        self.fixture.prepare(run)
        training = run / "fold" / "training-receipt.json"
        training_sha = digest(training.read_bytes())
        mechanism = self.fixture.root / "identity-mechanism.json"
        mechanism.write_bytes(canonical({"schema": "fn64.discovery-mechanism.v1", "schema_version": 1, "algorithm": "test", "source_revision_or_patch_digest": "2" * 64, "parameter_digest": "3" * 64, "training_receipt_sha256": training_sha, "training_ids": ["train"], "held_out_id": "held"}))
        self.fixture.run_command("freeze", "--run", str(run), "--mechanism", str(mechanism), "--expected-training-receipt-sha256", training_sha)
        freeze = run / "fold" / "freeze.json"
        admission = self.fixture.root / "identity-admission.json"
        admission.write_bytes(canonical({"schema": "fn64.loo-heldout-key-admission.v1", "schema_version": 1, "fold_id": "fold_a", "held_out_id": "held", "dump_path": str(self.fixture.dumps["held"]), "expected_dump_sha256": digest(self.fixture.dumps["held"].read_bytes()), "expected_rom_sha256": digest(self.fixture.roms["held"].read_bytes()), "expected_freeze_sha256": digest(freeze.read_bytes())}))
        binary = self.fixture.bin / "validate_training_workspace"
        binary.write_text(binary.read_text() + "\n# mutation\n")
        result = self.fixture.run_command("grade-heldout", "--run", str(run), "--admission", str(admission), "--expected-freeze-sha256", digest(freeze.read_bytes()), "--bin-dir", str(self.fixture.bin), ok=False)
        self.assertIn("executable identities", result.stderr)
        self.assertFalse((run / "heldout").exists())

    def test_timeout_kills_process_group_and_leaves_no_run(self):
        run = self.fixture.root / "timeout"
        (self.fixture.root / "hang-validator").touch()
        result = self.fixture.run_command("prepare", "--manifest", str(self.fixture.manifest), "--run", str(run), "--bin-dir", str(self.fixture.bin), "--timeout-seconds", "3", ok=False)
        self.assertIn("timeout", result.stderr)
        self.assertFalse(run.exists())
        pid = int((self.fixture.root / "grandchild.pid").read_text())
        for _ in range(50):
            try:
                os.kill(pid, 0)
            except ProcessLookupError:
                break
            import time
            time.sleep(0.02)
        else:
            self.fail("grandchild survived the timed-out process group")

    def test_aggregate_rss_cap_kills_allocating_descendant(self):
        run = self.fixture.root / "rss-cap"
        (self.fixture.root / "allocate-validator").touch()
        result = self.fixture.run_command("prepare", "--manifest", str(self.fixture.manifest), "--run", str(run),
                                          "--bin-dir", str(self.fixture.bin), "--timeout-seconds", "10",
                                          "--max-rss-mib", "32", ok=False)
        self.assertIn("memory_rss_limit", result.stderr)
        self.assertFalse(run.exists())
        pid = int((self.fixture.root / "grandchild.pid").read_text())
        for _ in range(50):
            try:
                os.kill(pid, 0)
            except ProcessLookupError:
                break
            import time
            time.sleep(0.02)
        else:
            self.fail("allocating grandchild survived the RSS breach")

    def test_successful_leader_cannot_leave_same_group_survivor(self):
        run = self.fixture.root / "survivor"
        (self.fixture.root / "survivor-validator").touch()
        result = self.fixture.prepare(run, ok=False)
        self.assertIn("child_survivors", result.stderr)
        self.assertFalse(run.exists())
        pid = int((self.fixture.root / "grandchild.pid").read_text())
        for _ in range(50):
            try:
                os.kill(pid, 0)
            except ProcessLookupError:
                break
            import time
            time.sleep(0.02)
        else:
            self.fail("same-PGID survivor was not killed")

    def test_receipt_publication_writes_all_bytes_after_short_writes(self):
        directory = self.fixture.root / "short-write"
        directory.mkdir(mode=0o700)
        destination = directory / "receipt.json"
        data = b'{"payload":"longer-than-one-short-write"}\n'
        real_write = os.write

        def short_write(descriptor: int, value: bytes | memoryview) -> int:
            return real_write(descriptor, value[:3])

        with mock.patch.object(FOLD_MODULE.os, "write", side_effect=short_write):
            actual_digest = FOLD_MODULE.publish_new(destination, data)
        self.assertEqual(destination.read_bytes(), data)
        self.assertEqual(actual_digest, digest(data))

    def test_stable_copy_rejects_zero_progress_write(self):
        source = self.fixture.root / "copy-source"
        destination = self.fixture.root / "copy-destination"
        source.write_bytes(b"must-not-silently-truncate")
        with mock.patch.object(FOLD_MODULE.os, "write", return_value=0):
            with self.assertRaisesRegex(FOLD_MODULE.FoldError, "no write progress"):
                FOLD_MODULE.copy_stable_input(source, destination, max_bytes=1024)

    def test_directory_publication_excludes_racing_destination(self):
        stage = self.fixture.root / "publish-stage"
        final = self.fixture.root / "publish-final"
        stage.mkdir(mode=0o700)
        real_rename = FOLD_MODULE.rename_directory_exclusive

        def race_then_rename(source: Path, destination: Path) -> None:
            destination.mkdir(mode=0o700)
            (destination / "racer-owned").write_text("preserve")
            real_rename(source, destination)

        with mock.patch.object(FOLD_MODULE, "rename_directory_exclusive", side_effect=race_then_rename):
            with self.assertRaisesRegex(FOLD_MODULE.FoldError, "refusing to overwrite"):
                FOLD_MODULE.publish_directory(stage, final)
        self.assertEqual((final / "racer-owned").read_text(), "preserve")
        self.assertTrue(stage.is_dir())

    def test_free_memory_preflight_refuses_to_launch(self):
        scratch = self.fixture.root / "free-floor"
        scratch.mkdir(mode=0o700)
        limits = FOLD_MODULE.subprocess_limits(2048, 40)
        with mock.patch.object(FOLD_MODULE, "sample_resources", return_value=(0, 39, 0)), \
             mock.patch.object(FOLD_MODULE.subprocess, "Popen") as launch:
            with self.assertRaisesRegex(FOLD_MODULE.FoldError, "memory_free_floor"):
                FOLD_MODULE.run_bounded(["/does/not/launch"], 10, "preflight", scratch, limits)
        launch.assert_not_called()

    def test_validator_summary_digest_must_match_stable_report(self):
        report = self.fixture.root / "validated-report.json"
        report.write_bytes(b'{"report":"current"}\n')
        summary = json.dumps({"schema": "fn64.known-function-attribution-validation.v1", "schema_version": 1,
                              "report_sha256": "0" * 64}, separators=(",", ":")) + "\n"
        cold = {"normalized_rom_sha256": "1" * 64, "cold_manifest_sha256": "2" * 64,
                "candidate_identity_v3_sha256": "3" * 64}
        with mock.patch.object(FOLD_MODULE, "run_bounded", return_value=summary):
            with self.assertRaisesRegex(FOLD_MODULE.FoldError, "report_changed_after_validation"):
                FOLD_MODULE.validate_attribution(Path("/unused/binary"), report, Path("/unused/cold"),
                                                 Path("/unused/dump"), cold, "4" * 64, 10,
                                                 self.fixture.root, FOLD_MODULE.subprocess_limits(2048, 40), "validator")

    def test_unknown_fields_heldout_key_and_digest_mismatch_fail(self):
        self.fixture.write_manifest(extra=True)
        self.fixture.prepare(self.fixture.root / "unknown", ok=False)
        self.fixture.write_manifest(held_key=True)
        self.fixture.prepare(self.fixture.root / "keyed", ok=False)
        self.fixture.write_manifest()
        value = json.loads(self.fixture.manifest.read_text())
        value["entries"][1]["expected_normalized_rom_sha256"] = "0" * 64
        self.fixture.manifest.write_bytes(canonical(value))
        self.fixture.prepare(self.fixture.root / "bad-digest", ok=False)

    def test_symlink_noncanonical_and_mode_rejection(self):
        self.fixture.run_command("prepare", "--manifest", "prepare.json", "--run", str(self.fixture.root / "relative"), "--bin-dir", str(self.fixture.bin), ok=False)
        manifest_link = self.fixture.root / "manifest-link.json"
        manifest_link.symlink_to(self.fixture.manifest)
        result = self.fixture.run_command("prepare", "--manifest", str(manifest_link), "--run", str(self.fixture.root / "linked"), "--bin-dir", str(self.fixture.bin), ok=False)
        self.assertIn("no-symlink", result.stderr)
        value = json.loads(self.fixture.manifest.read_text())
        rom_link = self.fixture.root / "rom-link.z64"
        rom_link.symlink_to(self.fixture.roms["held"])
        value["entries"][0]["rom_path"] = str(rom_link)
        self.fixture.manifest.write_bytes(canonical(value))
        self.fixture.prepare(self.fixture.root / "rom-linked", ok=False)
        self.fixture.write_manifest()
        run = self.fixture.root / "run-mode"
        self.fixture.prepare(run)
        run.chmod(0o755)
        result = self.fixture.run_command("freeze", "--run", str(run), "--mechanism", str(self.fixture.manifest), "--expected-training-receipt-sha256", "0" * 64, ok=False)
        self.assertIn("mode 0700", result.stderr)


if __name__ == "__main__":
    unittest.main()
