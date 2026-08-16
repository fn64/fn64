#!/usr/bin/env python3
"""Hostile tests for the RT64 port parity authority checker."""

from __future__ import annotations

import hashlib
import importlib.util
import json
import os
import subprocess
import sys
import tempfile
import threading
import unittest
from unittest import mock
from pathlib import Path


CHECKER_PATH = Path(__file__).with_name("check_rt64_port_parity.py")
SPEC = importlib.util.spec_from_file_location("check_rt64_port_parity", CHECKER_PATH)
assert SPEC is not None and SPEC.loader is not None
CHECKER = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = CHECKER
SPEC.loader.exec_module(CHECKER)


class ArtifactAuthorityTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.artifacts = self.root / "evidence" / "rt64-port" / "artifacts"
        self.artifacts.mkdir(parents=True)

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def write(self, name: str, contents: bytes) -> dict:
        path = self.artifacts / name
        path.write_bytes(contents)
        return {
            "path": path.relative_to(self.root).as_posix(),
            "sha256": hashlib.sha256(contents).hexdigest(),
        }

    def test_regular_artifact_is_rehashed(self) -> None:
        reference = self.write("report.json", b"{}")
        artifact = CHECKER.ArtifactRegistry(self.root).load(
            reference, prefix=("evidence", "rt64-port", "artifacts")
        )
        self.assertEqual(artifact.bytes, b"{}")
        self.assertEqual(artifact.sha256, reference["sha256"])

    def test_declared_digest_is_not_trusted(self) -> None:
        reference = self.write("report.json", b"{}")
        reference["sha256"] = "0" * 64
        with self.assertRaisesRegex(CHECKER.ParityError, "SHA-256 mismatch"):
            CHECKER.ArtifactRegistry(self.root).load(
                reference, prefix=("evidence", "rt64-port", "artifacts")
            )

    @unittest.skipUnless(hasattr(os, "symlink"), "platform has no symlink support")
    def test_symlinked_artifact_cannot_enter_evidence(self) -> None:
        target = self.artifacts / "target.json"
        target.write_bytes(b"{}")
        link = self.artifacts / "link.json"
        link.symlink_to(target.name)
        reference = {
            "path": link.relative_to(self.root).as_posix(),
            "sha256": hashlib.sha256(b"{}").hexdigest(),
        }
        with self.assertRaisesRegex(CHECKER.ParityError, "symlinked artifact path"):
            CHECKER.ArtifactRegistry(self.root).load(
                reference, prefix=("evidence", "rt64-port", "artifacts")
            )

    @unittest.skipUnless(hasattr(os, "link"), "platform has no hard-link support")
    def test_hard_link_alias_cannot_supply_two_runs(self) -> None:
        first = self.write("first.json", b"{}")
        first_path = self.root / first["path"]
        alias_path = self.artifacts / "alias.json"
        os.link(first_path, alias_path)
        alias = {
            "path": alias_path.relative_to(self.root).as_posix(),
            "sha256": first["sha256"],
        }
        registry = CHECKER.ArtifactRegistry(self.root)
        registry.load(first, prefix=("evidence", "rt64-port", "artifacts"))
        with self.assertRaisesRegex(CHECKER.ParityError, "aliases retained artifact"):
            registry.load(alias, prefix=("evidence", "rt64-port", "artifacts"))


class ProductionControlTests(unittest.TestCase):
    def test_structural_cli_does_not_execute_registered_evidence(self) -> None:
        completed = subprocess.run(
            [sys.executable, str(CHECKER_PATH), "--structural-only"],
            cwd=CHECKER_PATH.parent.parent,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=True,
        )
        self.assertIn("structurally clean", completed.stdout)

    def test_rejection_guards_are_structural_only(self) -> None:
        root = CHECKER_PATH.parent.parent
        manifest = CHECKER.load_json(root / "docs/rt64-port-parity.json")
        real_validate = CHECKER.validate_manifest
        calls = 0

        def structural(candidate, candidate_root, *, execute_evidence=True):
            nonlocal calls
            calls += 1
            self.assertFalse(execute_evidence)
            return real_validate(
                candidate,
                candidate_root,
                execute_evidence=False,
            )

        with mock.patch.object(CHECKER, "validate_manifest", side_effect=structural):
            CHECKER.rejection_guards(manifest, root)
        self.assertGreater(calls, 50)

    def test_qualification_report_retains_fresh_process_evidence(self) -> None:
        root = CHECKER_PATH.parent.parent
        manifest = CHECKER.load_json(root / "docs/rt64-port-parity.json")
        row = next(
            row
            for row in manifest["rows"]
            if row["id"] == "feature::deferred-frame-history"
        )
        series = CHECKER.ExecutedSeries(
            semantic_identity="1" * 64,
            process_identities=tuple(f"{value:064x}" for value in range(10)),
            run_identities=tuple(f"{value + 10:064x}" for value in range(10)),
            series_identity="2" * 64,
            child_pids=tuple(range(100, 110)),
            challenges=tuple(f"{value + 20:064x}" for value in range(10)),
        )
        report = CHECKER.qualification_report([(row, "RT64_PASS", series)])
        retained = report["rows"][0]
        self.assertEqual(report["schema"], "fn64.render-conformance.qualification-report.v1")
        self.assertEqual(retained["required_run_count"], 10)
        self.assertEqual(len(set(retained["child_pids"])), 10)
        self.assertEqual(len(set(retained["challenges"])), 10)


class ClosedEvidenceTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.repository = CHECKER_PATH.parent.parent
        subprocess.run(
            ["cargo", "build", "-p", "fn64-render-conformance", "--features", "conformance-test-runner", "--bins"],
            cwd=cls.repository, check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        )
        cls.verifier = cls.repository / "target" / "debug" / "fn64-render-conformance-verifier"
        cls.runner = cls.repository / "target" / "debug" / "fn64-render-conformance-test-runner"

    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.artifacts = self.root / "evidence" / "rt64-port" / "artifacts"
        self.artifacts.mkdir(parents=True)

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def artifact(self, name: str, contents: bytes, *, executable: bool = False) -> dict:
        path = self.artifacts / name
        path.write_bytes(contents)
        if executable:
            path.chmod(0o755)
        return {
            "path": path.relative_to(self.root).as_posix(),
            "sha256": hashlib.sha256(contents).hexdigest(),
        }

    def row(self, *, guest: bool = False) -> dict:
        return {
            "id": "base::rdp-command-state-order",
            "earliest_observable": (
                "resource_journal_guest_memory_effects"
                if guest
                else "admitted_commands_state"
            ),
            "observable_layers": (
                ["resource_journal_guest_memory_effects"]
                if guest
                else ["admitted_commands_state"]
            ),
        }

    def fixture_bundle(self, row: dict, guest: bool) -> tuple[dict, dict]:
        completed = subprocess.run(
            [str(self.runner), "emit-test-fixture"],
            input=CHECKER.canonical_bytes({"row_id": row["id"], "capture_layer": row["earliest_observable"], "guest_visible": guest}),
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=True,
        )
        bundle = json.loads(completed.stdout)
        return bundle["replay"], bundle["authority"]

    def execution(self, behavior: str = "honest", *, guest: bool = False):
        row = self.row(guest=guest)
        runner = self.artifact("runner", self.runner.read_bytes(), executable=True)
        verifier = self.artifact("verifier", self.verifier.read_bytes(), executable=True)
        sources = [
            self.artifact("wire.rs", (self.repository / "crates/fn64-render-conformance/src/wire.rs").read_bytes()),
            self.artifact("lib.rs", (self.repository / "crates/fn64-render-conformance/src/lib.rs").read_bytes()),
            self.artifact("verifier.rs", (self.repository / "crates/fn64-render-conformance/src/bin/fn64-render-conformance-verifier.rs").read_bytes()),
            self.artifact("fn64-render-conformance-test-runner.rs", (self.repository / "crates/fn64-render-conformance/src/bin/fn64-render-conformance-test-runner.rs").read_bytes()),
        ]
        build_inputs = [
            self.artifact("Cargo.toml", (self.repository / "crates/fn64-render-conformance/Cargo.toml").read_bytes()),
            self.artifact("Cargo.lock", (self.repository / "Cargo.lock").read_bytes()),
        ]
        replay_value, authority_value = self.fixture_bundle(row, guest)
        replay = self.artifact("replay.json", CHECKER.canonical_bytes(replay_value))
        authority = self.artifact("authority.json", CHECKER.canonical_bytes(authority_value))
        toolchain = subprocess.run(["rustc", "-vV"], text=True, stdout=subprocess.PIPE, check=True).stdout
        closure = {
            "runner_sha256": runner["sha256"], "verifier_sha256": verifier["sha256"],
            "source_inputs": sources, "build_inputs": build_inputs,
            "toolchain": {"rustc_vv": toolchain}, "enabled_features": [],
            "rt64_source_identity": None,
        }
        receipt_value = {
            "schema": "fn64.render-conformance.build-receipt.v2", "runner": runner,
            "verifier": verifier,
            "source_inputs": sources, "build_inputs": build_inputs, "toolchain": {"rustc_vv": toolchain},
            "enabled_features": [], "rt64_source_identity": None,
            "closure_identity": CHECKER.canonical_digest(closure),
        }
        receipt = self.artifact("build-receipt.json", CHECKER.canonical_bytes(receipt_value))
        evidence = {
            "availability": "qualified",
            "execution": {
                "runner_id": "reviewed-runner",
                "runner_artifact": runner,
                "verifier_artifact": verifier,
                "build_receipt": receipt,
                "replay_artifact": replay,
                "authority_artifact": authority,
            },
        }
        policies = {
            "reviewed-runner": CHECKER.RunnerPolicy(
                "rust_port", runner["path"], runner["sha256"], receipt["path"], receipt["sha256"],
                verifier["path"], verifier["sha256"],
                authority["path"], authority["sha256"],
                ("run", behavior),
                test_only=True,
            )
        }
        return row, evidence, policies, replay_value, authority_value

    def test_verifier_launches_ten_fresh_processes(self) -> None:
        row, evidence, policies, _, _ = self.execution()
        series = CHECKER.execute_qualified(
            self.root, evidence, row, "rust_port", "RUST_PASS",
            runner_registry=policies,
        )
        self.assertEqual(len(set(series.process_identities)), CHECKER.REQUIRED_RUNS)
        self.assertEqual(len(set(series.run_identities)), CHECKER.REQUIRED_RUNS)

    def test_wrong_pid_is_rejected(self) -> None:
        row, evidence, policies, _, _ = self.execution("wrong-pid")
        with self.assertRaisesRegex(CHECKER.ParityError, "launched child PID"):
            CHECKER.execute_qualified(
                self.root, evidence, row, "rust_port", "RUST_PASS",
                runner_registry=policies,
            )

    def test_stdout_hostility_is_rejected(self) -> None:
        row, evidence, policies, _, _ = self.execution("stdout-hostile")
        with self.assertRaisesRegex(CHECKER.ParityError, "runner emitted invalid JSON"):
            CHECKER.execute_qualified(
                self.root, evidence, row, "rust_port", "RUST_PASS",
                runner_registry=policies,
            )

    def test_ambient_loader_interpreter_and_plugin_injection_is_rejected(self) -> None:
        row, evidence, policies, _, _ = self.execution()
        for name in (
            "DYLD_INSERT_LIBRARIES",
            "DYLD_LIBRARY_PATH",
            "LD_PRELOAD",
            "LD_LIBRARY_PATH",
            "PYTHONPATH",
            "NODE_OPTIONS",
            "QT_PLUGIN_PATH",
            "VK_LAYER_PATH",
            "SDL_DYNAMIC_API",
            "dyld_insert_libraries",
        ):
            with self.subTest(name=name), mock.patch.dict(
                os.environ, {name: "/tmp/fn64-injection-sentinel"}
            ):
                with self.assertRaisesRegex(
                    CHECKER.ParityError,
                    "ambient loader/interpreter/plugin injection environment",
                ):
                    CHECKER.execute_qualified(
                        self.root, evidence, row, "rust_port", "RUST_PASS",
                        runner_registry=policies,
                    )

    def test_runner_and_verifier_receive_an_empty_environment(self) -> None:
        row, evidence, policies, _, _ = self.execution("environment-sentinel")
        real_run = CHECKER.subprocess.run
        real_popen = CHECKER.subprocess.Popen
        verifier_calls = 0
        runner_calls = 0

        def checked_run(*args, **kwargs):
            nonlocal verifier_calls
            command = args[0]
            if Path(command[0]).name == "verifier":
                verifier_calls += 1
                self.assertEqual(kwargs.get("env"), {})
            return real_run(*args, **kwargs)

        def checked_popen(*args, **kwargs):
            nonlocal runner_calls
            command = args[0]
            if Path(command[0]).name in {"runner", "verifier"}:
                self.assertEqual(kwargs.get("env"), {})
            if Path(command[0]).name == "runner":
                runner_calls += 1
            return real_popen(*args, **kwargs)

        with mock.patch.dict(
            os.environ, {"FN64_CONFORMANCE_ENV_SENTINEL": "must-not-propagate"}
        ), mock.patch.object(
            CHECKER.subprocess, "run", side_effect=checked_run
        ), mock.patch.object(
            CHECKER.subprocess, "Popen", side_effect=checked_popen
        ):
            CHECKER.execute_qualified(
                self.root, evidence, row, "rust_port", "RUST_PASS",
                runner_registry=policies,
            )
        self.assertEqual(verifier_calls, CHECKER.REQUIRED_RUNS + 1)
        self.assertEqual(runner_calls, CHECKER.REQUIRED_RUNS)

    def test_reviewed_cooldown_applies_only_between_runs(self) -> None:
        row, evidence, policies, _, _ = self.execution()
        policy = policies["reviewed-runner"]
        policies["reviewed-runner"] = CHECKER.RunnerPolicy(
            policy.delegate_kind,
            policy.runner_path,
            policy.runner_sha256,
            policy.build_receipt_path,
            policy.build_receipt_sha256,
            policy.verifier_path,
            policy.verifier_sha256,
            policy.authority_path,
            policy.authority_sha256,
            policy.runner_args,
            test_only=True,
            cooldown_milliseconds=1,
        )
        with mock.patch.object(CHECKER, "cooldown_sleep") as sleep:
            CHECKER.execute_qualified(
                self.root, evidence, row, "rust_port", "RUST_PASS",
                runner_registry=policies,
            )
        self.assertEqual(
            sleep.call_args_list,
            [mock.call(0.001)] * (CHECKER.REQUIRED_RUNS - 1),
        )

    def test_reviewed_cooldown_ignores_unrelated_concurrent_sleeps(self) -> None:
        # Hostile: a background thread hammers the real, process-global
        # time.sleep concurrently with the checker's run loop (simulating
        # unrelated activity elsewhere in the test process, e.g. docs lint
        # or other parallel tests). Patching CHECKER.cooldown_sleep — a name
        # private to this module — must not observe any of that noise: only
        # calls this module itself makes should ever appear in the mock.
        row, evidence, policies, _, _ = self.execution()
        policy = policies["reviewed-runner"]
        policies["reviewed-runner"] = CHECKER.RunnerPolicy(
            policy.delegate_kind,
            policy.runner_path,
            policy.runner_sha256,
            policy.build_receipt_path,
            policy.build_receipt_sha256,
            policy.verifier_path,
            policy.verifier_sha256,
            policy.authority_path,
            policy.authority_sha256,
            policy.runner_args,
            test_only=True,
            cooldown_milliseconds=1,
        )
        stop = threading.Event()

        def hammer_real_sleep() -> None:
            while not stop.is_set():
                CHECKER.time.sleep(0.001)

        noise = threading.Thread(target=hammer_real_sleep, daemon=True)
        noise.start()
        try:
            with mock.patch.object(CHECKER, "cooldown_sleep") as sleep:
                CHECKER.execute_qualified(
                    self.root, evidence, row, "rust_port", "RUST_PASS",
                    runner_registry=policies,
                )
        finally:
            stop.set()
            noise.join(timeout=5)
        self.assertEqual(
            sleep.call_args_list,
            [mock.call(0.001)] * (CHECKER.REQUIRED_RUNS - 1),
        )

    def test_unreviewed_cooldown_is_rejected(self) -> None:
        row, evidence, policies, _, _ = self.execution()
        policy = policies["reviewed-runner"]
        policies["reviewed-runner"] = CHECKER.RunnerPolicy(
            policy.delegate_kind,
            policy.runner_path,
            policy.runner_sha256,
            policy.build_receipt_path,
            policy.build_receipt_sha256,
            policy.verifier_path,
            policy.verifier_sha256,
            policy.authority_path,
            policy.authority_sha256,
            policy.runner_args,
            test_only=True,
            cooldown_milliseconds=CHECKER.MAX_RUNNER_COOLDOWN_MILLISECONDS + 1,
        )
        with self.assertRaisesRegex(CHECKER.ParityError, "cooldown is outside"):
            CHECKER.execute_qualified(
                self.root, evidence, row, "rust_port", "RUST_PASS",
                runner_registry=policies,
            )

    def test_synthetic_runner_cannot_enter_production_registry(self) -> None:
        row, evidence, policies, _, _ = self.execution()
        with mock.patch.object(CHECKER, "REGISTERED_RUNNERS", policies):
            with self.assertRaisesRegex(
                CHECKER.ParityError,
                "synthetic test runner cannot enter the production registry",
            ):
                CHECKER.execute_qualified(
                    self.root, evidence, row, "rust_port", "RUST_PASS"
                )

    def test_guest_visible_result_requires_typed_commit_result(self) -> None:
        row, evidence, policies, _, _ = self.execution(guest=True)
        series = CHECKER.execute_qualified(
            self.root, evidence, row, "rust_port", "RUST_PASS",
            runner_registry=policies,
        )
        self.assertTrue(CHECKER.is_digest(series.semantic_identity))

    def test_hand_authored_series_cannot_supply_process_tokens_or_leaves(self) -> None:
        row, evidence, policies, _, _ = self.execution()
        evidence["execution"]["runs"] = [
            {"process_token": f"{ordinal:064x}", "semantic": "7" * 64}
            for ordinal in range(CHECKER.REQUIRED_RUNS)
        ]
        with self.assertRaisesRegex(CHECKER.ParityError, "execution request fields drifted"):
            CHECKER.execute_qualified(
                self.root, evidence, row, "rust_port", "RUST_PASS",
                runner_registry=policies,
            )

    def test_one_execution_cloned_ten_times_is_rejected(self) -> None:
        row, evidence, policies, _, _ = self.execution()
        clone_path = self.artifacts / "clone-state.json"
        policy = policies["reviewed-runner"]
        policies["reviewed-runner"] = CHECKER.RunnerPolicy(
            policy.delegate_kind, policy.runner_path, policy.runner_sha256,
            policy.build_receipt_path, policy.build_receipt_sha256,
            policy.verifier_path, policy.verifier_sha256,
            policy.authority_path, policy.authority_sha256,
            ("run", "clone", str(clone_path)),
            test_only=True,
        )
        with self.assertRaisesRegex(
            CHECKER.ParityError,
            "did not answer this verifier challenge|launched child PID",
        ):
            CHECKER.execute_qualified(
                self.root, evidence, row, "rust_port", "RUST_PASS",
                runner_registry=policies,
            )

    def test_malicious_echo_runner_never_receives_private_expectation(self) -> None:
        row, evidence, policies, replay, authority = self.execution("echo")
        request_keys = {"schema", "ordinal", "challenge", "replay"}
        self.assertNotIn("expected_observation", replay)
        self.assertIn("expected_observation", authority)
        self.assertEqual(request_keys, {"schema", "ordinal", "challenge", "replay"})
        with self.assertRaisesRegex(CHECKER.ParityError, "does not satisfy RUST_PASS"):
            CHECKER.execute_qualified(self.root, evidence, row, "rust_port", "RUST_PASS", runner_registry=policies)

    def test_invalid_record_is_rejected_by_rust_decoder(self) -> None:
        row, evidence, policies, replay, _ = self.execution()
        replay["record_hex"] = "00"
        evidence["execution"]["replay_artifact"] = self.artifact("invalid-replay.json", CHECKER.canonical_bytes(replay))
        with self.assertRaisesRegex(CHECKER.ParityError, "Rust verifier rejected evidence"):
            CHECKER.execute_qualified(self.root, evidence, row, "rust_port", "RUST_PASS", runner_registry=policies)

    def test_missing_and_wrong_payload_are_rejected(self) -> None:
        for mutation in (lambda value: value.update(payload_streams_hex=[]), lambda value: value["payload_streams_hex"].__setitem__(0, "00" * 8)):
            with self.subTest(mutation=mutation):
                row, evidence, policies, replay, _ = self.execution()
                mutation(replay)
                evidence["execution"]["replay_artifact"] = self.artifact(f"bad-replay-{len(list(self.artifacts.iterdir()))}.json", CHECKER.canonical_bytes(replay))
                with self.assertRaisesRegex(CHECKER.ParityError, "Rust verifier rejected evidence"):
                    CHECKER.execute_qualified(self.root, evidence, row, "rust_port", "RUST_PASS", runner_registry=policies)

    def test_arbitrary_effects_fail_private_authority(self) -> None:
        row, evidence, policies, _, _ = self.execution("arbitrary-effects", guest=True)
        with self.assertRaisesRegex(CHECKER.ParityError, "Rust verifier rejected evidence|does not satisfy"):
            CHECKER.execute_qualified(self.root, evidence, row, "rust_port", "RUST_PASS", runner_registry=policies)

    def test_caller_authored_authority_is_not_registered(self) -> None:
        row, evidence, policies, _, authority = self.execution()
        authority["expected_observation"]["bytes_hex"] = "00"
        replacement = self.artifact("caller-authority.json", CHECKER.canonical_bytes(authority))
        evidence["execution"]["authority_artifact"] = replacement
        with self.assertRaisesRegex(CHECKER.ParityError, "verifier-private authority artifact is not registered"):
            CHECKER.execute_qualified(self.root, evidence, row, "rust_port", "RUST_PASS", runner_registry=policies)

    def test_fake_guest_label_fails_typed_proof(self) -> None:
        row, evidence, policies, _, _ = self.execution("fake-guest", guest=True)
        with self.assertRaisesRegex(CHECKER.ParityError, "Rust verifier rejected evidence"):
            CHECKER.execute_qualified(self.root, evidence, row, "rust_port", "RUST_PASS", runner_registry=policies)

    def test_self_consistent_guest_proof_from_an_old_challenge_is_rejected(self) -> None:
        row, evidence, policies, _, _ = self.execution(
            "stale-guest-challenge", guest=True
        )
        with self.assertRaisesRegex(CHECKER.ParityError, "Rust verifier rejected evidence"):
            CHECKER.execute_qualified(
                self.root,
                evidence,
                row,
                "rust_port",
                "RUST_PASS",
                runner_registry=policies,
            )

    def test_mutated_source_and_decorative_build_receipt_fail(self) -> None:
        row, evidence, policies, _, _ = self.execution()
        (self.root / "evidence/rt64-port/artifacts/wire.rs").write_bytes(b"mutated")
        with self.assertRaisesRegex(CHECKER.ParityError, "SHA-256 mismatch"):
            CHECKER.execute_qualified(self.root, evidence, row, "rust_port", "RUST_PASS", runner_registry=policies)
        row, evidence, policies, _, _ = self.execution()
        receipt_path = self.root / evidence["execution"]["build_receipt"]["path"]
        receipt = json.loads(receipt_path.read_text())
        receipt["closure_identity"] = "00" * 32
        encoded = CHECKER.canonical_bytes(receipt)
        receipt_path.write_bytes(encoded)
        evidence["execution"]["build_receipt"]["sha256"] = hashlib.sha256(encoded).hexdigest()
        policy = policies["reviewed-runner"]
        policies["reviewed-runner"] = CHECKER.RunnerPolicy(
            policy.delegate_kind,
            policy.runner_path,
            policy.runner_sha256,
            policy.build_receipt_path,
            evidence["execution"]["build_receipt"]["sha256"],
            policy.verifier_path,
            policy.verifier_sha256,
            policy.authority_path,
            policy.authority_sha256,
            policy.runner_args,
            test_only=True,
        )
        with self.assertRaisesRegex(CHECKER.ParityError, "closure identity mismatch"):
            CHECKER.execute_qualified(self.root, evidence, row, "rust_port", "RUST_PASS", runner_registry=policies)

    def test_cross_language_replay_identity_golden(self) -> None:
        row, _, _, replay, _ = self.execution()
        completed = subprocess.run([str(self.verifier), "inspect"], input=CHECKER.canonical_bytes(replay), stdout=subprocess.PIPE, check=True)
        inspection = json.loads(completed.stdout)
        fields = [row["id"].encode(), bytes.fromhex(replay["record_hex"]), bytes([CHECKER.OBSERVABLES.index(replay["capture_layer"])])]
        fields.extend(bytes.fromhex(value) for value in replay["payload_streams_hex"])
        python_identity = CHECKER.framed_digest(b"fn64.render-conformance.replay.v1\0", fields)
        self.assertEqual(inspection["replay_identity"], python_identity)
        self.assertEqual(python_identity, "af47594d05af27126fbc9b8936b6c8f2ce555490293eee41d3530efe8983deea")

    def test_rt64_source_identity_changes_receipt_hash(self) -> None:
        common = [b"row", bytes.fromhex("1" * 64)]
        first = CHECKER.framed_digest(
            b"fn64.render-conformance.receipt.v4\0",
            common + [b"\1", bytes.fromhex("2" * 64)],
        )
        second = CHECKER.framed_digest(
            b"fn64.render-conformance.receipt.v4\0",
            common + [b"\1", bytes.fromhex("3" * 64)],
        )
        self.assertNotEqual(first, second)

    def test_rt64_source_identity_hashes_the_pinned_source_id(self) -> None:
        root = CHECKER_PATH.parent.parent
        source_id = json.loads(
            (root / "docs" / "rt64-port-authority.json").read_text(encoding="utf-8")
        )["oracle"]["source_id"]
        expected = CHECKER.framed_digest(
            b"fn64.render-conformance.rt64-source.v1\0",
            [source_id.encode()],
        )
        self.assertEqual(CHECKER.rt64_source_identity(root), expected)


if __name__ == "__main__":
    unittest.main()
