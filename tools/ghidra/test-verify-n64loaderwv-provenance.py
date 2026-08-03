#!/usr/bin/env python3

from __future__ import annotations

import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
import zipfile


ROOT = Path(__file__).resolve().parents[2]
VERIFIER = ROOT / "tools/ghidra/verify-n64loaderwv-provenance.py"
REPOSITORY = "https://github.com/fn64/N64LoaderWV"
HEX64 = "12" * 32


class ProvenanceTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory(prefix="fn64-n64loaderwv-provenance-")
        self.root = Path(self.temporary.name).resolve()
        self.checkout = self.root / "checkout"
        self.checkout.mkdir()
        self.git("init")
        self.git("config", "user.name", "fn64 test")
        self.git("config", "user.email", "fn64@example.invalid")
        (self.checkout / "source.txt").write_text("approved source\n", encoding="utf-8")
        self.git("add", "source.txt")
        self.git("commit", "-m", "approved")
        self.commit = self.git("rev-parse", "HEAD")
        self.tree = self.git("rev-parse", "HEAD^{tree}")
        self.git("remote", "add", "origin", "git@github.com:fn64/N64LoaderWV.git")
        self.git("update-ref", "refs/remotes/origin/fn64/analysis-export", self.commit)
        self.policy = self.root / "policy.json"
        self.write_policy()
        self.policy_sha = hashlib.sha256(self.policy.read_bytes()).hexdigest()
        self.extension = self.root / "extension.zip"
        with zipfile.ZipFile(self.extension, "w") as archive:
            archive.writestr("N64LoaderWV/lib/N64LoaderWV.jar", b"synthetic")
        self.extension_sha = hashlib.sha256(self.extension.read_bytes()).hexdigest()
        self.receipt = self.root / "receipt.txt"
        self.write_receipt()
        self.artifact_policy = self.root / "artifact-policy.json"
        self.write_artifact_policy()

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def git(self, *arguments: str) -> str:
        result = subprocess.run(
            ["git", "-C", str(self.checkout), *arguments],
            check=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        return result.stdout.strip()

    def write_policy(self, **changes: object) -> None:
        value: dict[str, object] = {
            "approved_commit": getattr(self, "commit", "0" * 40),
            "approved_tree": getattr(self, "tree", "0" * 40),
            "repository": REPOSITORY,
            "schema": "fn64.n64loaderwv-source-policy",
            "schema_version": 1,
        }
        value.update(changes)
        self.policy.write_text(
            json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n",
            encoding="utf-8",
        )

    def receipt_fields(self) -> dict[str, str]:
        fields = {
            "schema": "fn64.n64loaderwv-conformance.v2",
            "conformance_mode": "approved",
            "n64loaderwv_repository": REPOSITORY,
            "n64loaderwv_policy_sha256": self.policy_sha,
            "n64loaderwv_commit": self.commit,
            "n64loaderwv_tree": self.tree,
            "n64loaderwv_source_archive_sha256": HEX64,
            "n64loaderwv_extension_sha256": self.extension_sha,
            "ghidra_version": "12.1.2",
        }
        for key in (
            "ghidra_build_sha256",
            "export_script_sha256",
            "rom_sha256",
            "rdram_sha256",
            "bank_sha256",
            "mapping_sha256",
            "configuration_sha256",
            "evidence_sha256",
            "provider_jsonl_sha256",
            "build_memory_guard_sha256",
            "analysis_memory_guard_sha256",
            "gate_memory_guard_sha256",
        ):
            fields[key] = HEX64
        return fields

    def write_receipt(self, **changes: str) -> None:
        fields = self.receipt_fields()
        fields.update(changes)
        self.receipt.write_text(
            "\n".join(f"{key}={value}" for key, value in fields.items()) + "\n",
            encoding="utf-8",
        )

    def write_artifact_policy(self, **changes: object) -> None:
        value: dict[str, object] = {
            "approved_conformance_receipt_sha256": hashlib.sha256(
                self.receipt.read_bytes()
            ).hexdigest(),
            "approved_extension_sha256": self.extension_sha,
            "schema": "fn64.n64loaderwv-artifact-policy",
            "schema_version": 1,
            "source_policy_sha256": self.policy_sha,
        }
        value.update(changes)
        self.artifact_policy.write_text(
            json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n",
            encoding="utf-8",
        )

    def verify(self, mode: str, *paths: object) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [sys.executable, str(VERIFIER), mode, *(str(path) for path in paths)],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )

    def assert_rejected(self, mode: str, *paths: object) -> None:
        result = self.verify(mode, *paths)
        self.assertNotEqual(result.returncode, 0, (result.stdout, result.stderr))

    def test_checkout_accepts_exact_fn64_origin_commit_tree(self) -> None:
        result = self.verify("checkout", self.policy, self.checkout, self.commit)
        self.assertEqual(result.returncode, 0, result.stderr)
        value = json.loads(result.stdout)
        self.assertEqual(value["repository"], REPOSITORY)
        self.assertEqual(value["commit"], self.commit)
        self.assertEqual(value["tree"], self.tree)
        self.assertEqual(value["policy_sha256"], self.policy_sha)

    def test_checkout_rejects_non_fn64_origin(self) -> None:
        self.git("remote", "set-url", "origin", "git@github.com:jeremyw/N64LoaderWV.git")
        self.assert_rejected("checkout", self.policy, self.checkout, self.commit)

    def test_checkout_rejects_commit_not_in_origin_refs(self) -> None:
        self.git("update-ref", "-d", "refs/remotes/origin/fn64/analysis-export")
        self.assert_rejected("checkout", self.policy, self.checkout, self.commit)

    def test_checkout_rejects_wrong_tree_policy(self) -> None:
        self.write_policy(approved_tree="f" * 40)
        self.assert_rejected("checkout", self.policy, self.checkout, self.commit)

    def test_policy_rejects_duplicate_and_unknown_fields(self) -> None:
        duplicate = self.root / "duplicate-policy.json"
        duplicate.write_text(
            '{"schema":"fn64.n64loaderwv-source-policy","schema":"duplicate",'
            '"schema_version":1,"repository":"https://github.com/fn64/N64LoaderWV",'
            f'"approved_commit":"{self.commit}","approved_tree":"{self.tree}"}}\n',
            encoding="utf-8",
        )
        self.assert_rejected("checkout", duplicate, self.checkout, self.commit)
        self.write_policy(unknown="value")
        self.assert_rejected("checkout", self.policy, self.checkout, self.commit)

    def test_artifact_accepts_receipt_bound_zip(self) -> None:
        result = self.verify(
            "artifact", self.artifact_policy, self.policy, self.receipt, self.extension
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        value = json.loads(result.stdout)
        self.assertEqual(value["extension_sha256"], self.extension_sha)
        self.assertEqual(value["commit"], self.commit)
        self.assertEqual(
            value["conformance_receipt_sha256"],
            hashlib.sha256(self.receipt.read_bytes()).hexdigest(),
        )

    def test_artifact_rejects_changed_zip(self) -> None:
        self.extension.write_bytes(self.extension.read_bytes() + b"changed")
        self.assert_rejected(
            "artifact", self.artifact_policy, self.policy, self.receipt, self.extension
        )

    def test_artifact_rejects_forged_receipt_bound_to_arbitrary_zip(self) -> None:
        with zipfile.ZipFile(self.extension, "w") as archive:
            archive.writestr("N64LoaderWV/lib/N64LoaderWV.jar", b"attacker-selected")
        forged_extension_sha = hashlib.sha256(self.extension.read_bytes()).hexdigest()
        self.write_receipt(n64loaderwv_extension_sha256=forged_extension_sha)
        integrity = self.verify("candidate-integrity", self.receipt, self.extension)
        self.assertEqual(integrity.returncode, 0, integrity.stderr)
        self.assert_rejected(
            "artifact", self.artifact_policy, self.policy, self.receipt, self.extension
        )

    def test_artifact_rejects_wrong_repository_commit_or_policy(self) -> None:
        for field, value in (
            ("n64loaderwv_repository", "https://github.com/jeremyw/N64LoaderWV"),
            ("n64loaderwv_commit", "f" * 40),
            ("n64loaderwv_policy_sha256", "f" * 64),
        ):
            with self.subTest(field=field):
                self.write_receipt(**{field: value})
                self.assert_rejected(
                    "artifact",
                    self.artifact_policy,
                    self.policy,
                    self.receipt,
                    self.extension,
                )

    def test_artifact_rejects_development_receipt(self) -> None:
        self.write_receipt(conformance_mode="development")
        self.assert_rejected(
            "artifact", self.artifact_policy, self.policy, self.receipt, self.extension
        )

    def test_artifact_rejects_duplicate_unknown_and_missing_fields(self) -> None:
        original = self.receipt.read_text(encoding="utf-8")
        self.receipt.write_text(original + f"rom_sha256={HEX64}\n", encoding="utf-8")
        self.assert_rejected(
            "artifact", self.artifact_policy, self.policy, self.receipt, self.extension
        )
        self.receipt.write_text(original + "unknown=value\n", encoding="utf-8")
        self.assert_rejected(
            "artifact", self.artifact_policy, self.policy, self.receipt, self.extension
        )
        self.receipt.write_text(
            "\n".join(line for line in original.splitlines() if not line.startswith("rom_sha256=")) + "\n",
            encoding="utf-8",
        )
        self.assert_rejected(
            "artifact", self.artifact_policy, self.policy, self.receipt, self.extension
        )

    def test_artifact_rejects_unpinned_source_policy(self) -> None:
        self.write_policy(approved_tree="f" * 40)
        self.assert_rejected(
            "artifact", self.artifact_policy, self.policy, self.receipt, self.extension
        )

    def test_artifact_policy_rejects_duplicate_unknown_and_invalid_digests(self) -> None:
        original = self.artifact_policy.read_text(encoding="utf-8")
        self.artifact_policy.write_text(
            original.rstrip()[:-1] + ',"schema":"duplicate"}\n', encoding="utf-8"
        )
        self.assert_rejected(
            "artifact", self.artifact_policy, self.policy, self.receipt, self.extension
        )
        self.write_artifact_policy(unknown="value")
        self.assert_rejected(
            "artifact", self.artifact_policy, self.policy, self.receipt, self.extension
        )
        self.write_artifact_policy(approved_extension_sha256="A" * 64)
        self.assert_rejected(
            "artifact", self.artifact_policy, self.policy, self.receipt, self.extension
        )

    def test_candidate_integrity_accepts_self_consistent_unpinned_pair(self) -> None:
        result = self.verify("candidate-integrity", self.receipt, self.extension)
        self.assertEqual(result.returncode, 0, result.stderr)
        value = json.loads(result.stdout)
        self.assertEqual(value["verification"], "candidate_integrity_only")
        self.assertEqual(value["extension_sha256"], self.extension_sha)

    def test_every_vw_headless_invocation_has_a_provenance_gate(self) -> None:
        ghidra_tools = ROOT / "tools/ghidra"
        invokers = {
            path.name
            for path in ghidra_tools.glob("*.sh")
            if "-loader N64LoaderWVLoader" in path.read_text(encoding="utf-8")
        }
        self.assertEqual(
            invokers,
            {
                "run-n64loaderwv-conformance.sh",
                "run-n64loaderwv-first-contact.sh",
                "run-snapshot-loader-ab.sh",
            },
        )
        conformance = (ghidra_tools / "run-n64loaderwv-conformance.sh").read_text(
            encoding="utf-8"
        )
        first_contact = (ghidra_tools / "run-n64loaderwv-first-contact.sh").read_text(
            encoding="utf-8"
        )
        loader_ab = (ghidra_tools / "run-snapshot-loader-ab.sh").read_text(
            encoding="utf-8"
        )
        self.assertIn('"$provenance_verifier" checkout', conformance)
        self.assertIn('"$provenance_verifier" candidate-integrity', conformance)
        self.assertIn('"$provenance_verifier" artifact', first_contact)
        self.assertIn('"$verifier_source" artifact', loader_ab)
        self.assertIn('"$bound_verifier" artifact', loader_ab)


if __name__ == "__main__":
    unittest.main()
