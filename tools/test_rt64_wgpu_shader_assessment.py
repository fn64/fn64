#!/usr/bin/env python3

from __future__ import annotations

import argparse
import copy
import json
import subprocess
import struct
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))
import rt64_shader_artifacts as base
import rt64_wgpu_shader_assessment as subject


def inventory(blocked: bool = False) -> dict:
    value = {
        "schema": "fn64.spirv-semantic-inventory.v1",
        "word_count": 20,
        "id_bound": 4,
        "capabilities": ([
            {"name": "Shader", "value": 1, "word_offset": 5},
            {"name": "ShaderNonUniform", "value": 5301, "word_offset": 7},
        ] if blocked else [{"name": "Shader", "value": 1, "word_offset": 5}]),
        "extensions": ([{"name": "SPV_EXT_descriptor_indexing", "word_offset": 9}] if blocked else []),
        "non_uniform_decorations": ([{"target_id": 2, "word_offset": 12}] if blocked else []),
    }
    value["inventory_sha256"] = base.digest_bytes(base.canonical_json(value))
    return value


def reference_row(blocked: bool = False, index: int = 0) -> dict:
    return {
        "id": f"shader-{index}", "source": f"src/shader{index}.hlsl", "stage": "compute",
        "entry": "CSMain", "spirv_artifact": f"spirv/shader{index}.spv",
        "spirv_sha256": "a" * 64, "spirv_bytes": 20,
        "semantic_inventory": inventory(blocked),
    }


def success_bytes(row: dict, module_bytes: int = 20) -> bytes:
    return (
        '{"schema":"fn64.wgpu-shader-validation.v1","status":"passed",'
        f'"wgpu_major":30,"stage":"{row["stage"]}","entry":"{row["entry"]}",'
        f'"module_bytes":{module_bytes}}}\n'
    ).encode()


def spirv_fixture(*, descriptor_set: int = 0, binding: int = 2, member_index: int = 7,
                  vector_length: int = 3, member_offset: int = 92,
                  buffer_block_decoration: int = 3) -> bytes:
    words = [0x07230203, 0x00010000, 0, 8, 0]

    def instruction(opcode: int, *operands: int) -> None:
        words.append(((len(operands) + 1) << 16) | opcode)
        words.extend(operands)

    def literal(value: str) -> list[int]:
        data = value.encode() + b"\0"
        data += b"\0" * (-len(data) % 4)
        return list(struct.unpack(f"<{len(data) // 4}I", data))

    instruction(5, 7, *literal("instanceRDPParams"))
    instruction(5, 5, *literal("type.StructuredBuffer.RDPParams"))
    instruction(5, 3, *literal("RDPParams"))
    instruction(6, 3, member_index, *literal("keyScale"))
    instruction(71, 7, 34, descriptor_set)
    instruction(71, 7, 33, binding)
    instruction(71, 5, buffer_block_decoration)
    instruction(71, 4, 6, 128)
    instruction(72, 5, 0, 35, 0)
    instruction(72, 5, 0, 24)
    instruction(72, 3, member_index, 35, member_offset)
    instruction(22, 1, 32)
    instruction(23, 2, 1, vector_length)
    instruction(30, 3, 1, 1, 1, 1, 1, 1, 1, 2)
    instruction(29, 4, 3)
    instruction(30, 5, 4)
    instruction(32, 6, 2, 5)
    instruction(59, 6, 7, 2)
    return struct.pack(f"<{len(words)}I", *words)


EMPTY_SPIRV = struct.pack("<5I", 0x07230203, 0x00010000, 0, 1, 0)


class PolicyTests(unittest.TestCase):
    def test_policy_is_exact_and_selftest_passes(self) -> None:
        self.assertEqual(subject.load_policy()["reference_corpus"]["row_count"], 56)
        subject.selftest()

    def test_parser_has_four_commands(self) -> None:
        parser = subject.parser()
        self.assertEqual(parser.parse_args(["selftest"]).command, "selftest")
        with self.assertRaises(SystemExit):
            parser.parse_args(["assess"])

    def test_policy_rejects_extra_top_level_key(self) -> None:
        policy = base.load_json(subject.POLICY_PATH)
        policy["rogue"] = True
        with mock.patch.object(base, "load_json", return_value=policy), self.assertRaises(base.ArtifactError):
            subject.load_policy()

    def test_policy_rejects_relaxed_validator_argv(self) -> None:
        policy = base.load_json(subject.POLICY_PATH)
        policy["wgpu_validator"]["arguments"].append("--relaxed")
        with mock.patch.object(base, "load_json", return_value=policy), self.assertRaises(base.ArtifactError):
            subject.load_policy()

    def test_policy_rejects_runtime_ready_true(self) -> None:
        policy = base.load_json(subject.POLICY_PATH)
        policy["runtime_readiness"]["runtime_ready"] = True
        with mock.patch.object(base, "load_json", return_value=policy), self.assertRaises(base.ArtifactError):
            subject.load_policy()


class ReferenceReceiptSerializationTests(unittest.TestCase):
    def setUp(self) -> None:
        self.value = {"schema": "fixture.v1", "nested": {"b": 2, "a": 1}}

    def test_accepts_exact_pretty_json(self) -> None:
        self.assertEqual(subject.load_exact_pretty_json_bytes(base.pretty_json(self.value), "fixture"), self.value)

    def test_rejects_compact_canonical_json(self) -> None:
        with self.assertRaisesRegex(base.ArtifactError, "not exact pretty JSON"):
            subject.load_exact_pretty_json_bytes(base.canonical_json(self.value), "fixture")

    def test_rejects_whitespace_mutation(self) -> None:
        mutated = base.pretty_json(self.value).replace(b"  ", b"    ", 1)
        with self.assertRaisesRegex(base.ArtifactError, "not exact pretty JSON"):
            subject.load_exact_pretty_json_bytes(mutated, "fixture")

    def test_rejects_field_order_mutation(self) -> None:
        mutated = (json.dumps(self.value, indent=2, sort_keys=False) + "\n").encode()
        self.assertNotEqual(mutated, base.pretty_json(self.value))
        with self.assertRaisesRegex(base.ArtifactError, "not exact pretty JSON"):
            subject.load_exact_pretty_json_bytes(mutated, "fixture")


class ClassificationTests(unittest.TestCase):
    def setUp(self) -> None:
        self.policy = subject.load_policy()

    def completed(self, code: int, stdout: bytes, stderr: bytes) -> subprocess.CompletedProcess[bytes]:
        return subprocess.CompletedProcess(["validator"], code, stdout, stderr)

    def test_ingestible_exact_bytes(self) -> None:
        row = reference_row()
        result = subject.classify_result(self.completed(0, success_bytes(row), b""), row, EMPTY_SPIRV, self.policy)
        self.assertEqual(result[0], "ingestible")

    def test_ingestible_rejects_field_order_change(self) -> None:
        row = reference_row()
        altered = (json.dumps(json.loads(success_bytes(row)), sort_keys=True, separators=(",", ":")) + "\n").encode()
        with self.assertRaises(base.ArtifactError):
            subject.classify_result(self.completed(0, altered, b""), row, EMPTY_SPIRV, self.policy)

    def test_ingestible_rejects_extra_newline(self) -> None:
        row = reference_row()
        with self.assertRaises(base.ArtifactError):
            subject.classify_result(self.completed(0, success_bytes(row) + b"\n", b""), row, EMPTY_SPIRV, self.policy)

    def test_ingestible_rejects_stderr(self) -> None:
        row = reference_row()
        with self.assertRaises(base.ArtifactError):
            subject.classify_result(self.completed(0, success_bytes(row), b"warning\n"), row, EMPTY_SPIRV, self.policy)

    def test_shader_nonuniform_unexpected_success_is_fatal(self) -> None:
        row = reference_row(True)
        with self.assertRaisesRegex(base.ArtifactError, "unexpectedly passed"):
            subject.classify_result(self.completed(0, success_bytes(row), b""), row, EMPTY_SPIRV, self.policy)

    def test_blocked_known_requires_complete_exact_witness(self) -> None:
        row = reference_row(True)
        outcome, reason, record, witness = subject.classify_result(self.completed(2, b"", subject.KNOWN_STDERR), row, EMPTY_SPIRV, self.policy)
        self.assertEqual((outcome, reason, record, witness), ("blocked-known", self.policy["outcomes"]["blocked_known_shader_nonuniform"]["reason_code"], None, None))

    def test_blocked_known_rejects_stderr_prefix(self) -> None:
        row = reference_row(True)
        with self.assertRaises(base.ArtifactError):
            subject.classify_result(self.completed(2, b"", subject.KNOWN_STDERR[:-1]), row, EMPTY_SPIRV, self.policy)

    def test_blocked_known_rejects_stdout(self) -> None:
        row = reference_row(True)
        with self.assertRaises(base.ArtifactError):
            subject.classify_result(self.completed(2, b"{}\n", subject.KNOWN_STDERR), row, EMPTY_SPIRV, self.policy)

    def test_blocked_known_rejects_missing_capability(self) -> None:
        row = reference_row(False)
        with self.assertRaises(base.ArtifactError):
            subject.classify_result(self.completed(2, b"", subject.KNOWN_STDERR), row, EMPTY_SPIRV, self.policy)

    def test_blocked_known_rejects_missing_extension(self) -> None:
        row = reference_row(True)
        row["semantic_inventory"]["extensions"] = []
        with self.assertRaises(base.ArtifactError):
            subject.classify_result(self.completed(2, b"", subject.KNOWN_STDERR), row, EMPTY_SPIRV, self.policy)

    def test_blocked_known_rejects_missing_direct_decoration(self) -> None:
        row = reference_row(True)
        row["semantic_inventory"]["non_uniform_decorations"] = []
        with self.assertRaises(base.ArtifactError):
            subject.classify_result(self.completed(2, b"", subject.KNOWN_STDERR), row, EMPTY_SPIRV, self.policy)

    def test_unknown_exit_is_fatal(self) -> None:
        with self.assertRaises(base.ArtifactError):
            subject.classify_result(self.completed(1, b"", b"unknown\n"), reference_row(), EMPTY_SPIRV, self.policy)


class ScalarLayoutClassificationTests(unittest.TestCase):
    def setUp(self) -> None:
        self.policy = subject.load_policy()
        self.row = reference_row(False)

    def completed(self, code: int, stdout: bytes, stderr: bytes) -> subprocess.CompletedProcess[bytes]:
        return subprocess.CompletedProcess(["validator"], code, stdout, stderr)

    def test_exact_scalar_layout_witness_and_error_are_blocked_known(self) -> None:
        module = spirv_fixture()
        outcome, reason, record, witness = subject.classify_result(
            self.completed(2, b"", subject.SCALAR_LAYOUT_STDERR), self.row, module, self.policy
        )
        self.assertEqual(outcome, "blocked-known")
        self.assertEqual(reason, self.policy["outcomes"]["blocked_known_scalar_layout"]["reason_code"])
        self.assertIsNone(record)
        subject.validate_scalar_witness_record(witness, self.policy, "fixture")

    def test_scalar_layout_witness_unexpected_success_is_fatal(self) -> None:
        module = spirv_fixture()
        with self.assertRaisesRegex(base.ArtifactError, "unexpectedly passed"):
            subject.classify_result(
                self.completed(0, success_bytes(self.row, len(module)), b""), self.row, module, self.policy
            )

    def test_scalar_layout_stderr_near_match_is_fatal(self) -> None:
        with self.assertRaises(base.ArtifactError):
            subject.classify_result(
                self.completed(2, b"", subject.SCALAR_LAYOUT_STDERR[:-1]),
                self.row, spirv_fixture(), self.policy,
            )

    def test_scalar_layout_requires_empty_stdout(self) -> None:
        with self.assertRaises(base.ArtifactError):
            subject.classify_result(
                self.completed(2, b"{}\n", subject.SCALAR_LAYOUT_STDERR),
                self.row, spirv_fixture(), self.policy,
            )

    def test_scalar_layout_rejects_descriptor_set_drift(self) -> None:
        with self.assertRaisesRegex(base.ArtifactError, "descriptor set"):
            subject.scalar_layout_witness(spirv_fixture(descriptor_set=1), self.policy)

    def test_scalar_layout_rejects_binding_drift(self) -> None:
        with self.assertRaisesRegex(base.ArtifactError, "binding"):
            subject.scalar_layout_witness(spirv_fixture(binding=3), self.policy)

    def test_scalar_layout_rejects_buffer_block_drift(self) -> None:
        with self.assertRaisesRegex(base.ArtifactError, "BufferBlock"):
            subject.scalar_layout_witness(spirv_fixture(buffer_block_decoration=2), self.policy)

    def test_scalar_layout_rejects_member_drift(self) -> None:
        with self.assertRaisesRegex(base.ArtifactError, "member"):
            subject.scalar_layout_witness(spirv_fixture(member_index=6), self.policy)

    def test_scalar_layout_rejects_type_drift(self) -> None:
        with self.assertRaisesRegex(base.ArtifactError, "member type"):
            subject.scalar_layout_witness(spirv_fixture(vector_length=4), self.policy)

    def test_scalar_layout_rejects_offset_drift(self) -> None:
        with self.assertRaisesRegex(base.ArtifactError, "member offset"):
            subject.scalar_layout_witness(spirv_fixture(member_offset=96), self.policy)

    def test_scalar_layout_rejects_alignment_drift(self) -> None:
        witness = subject.scalar_layout_witness(spirv_fixture(), self.policy)
        witness["required_alignment"] = 4
        unhashed = copy.deepcopy(witness)
        unhashed.pop("witness_sha256")
        witness["witness_sha256"] = base.digest_bytes(base.canonical_json(unhashed))
        with self.assertRaisesRegex(base.ArtifactError, "fields changed"):
            subject.validate_scalar_witness_record(witness, self.policy, "fixture")


class InventoryTests(unittest.TestCase):
    def test_valid_inventory(self) -> None:
        subject.validate_inventory_record(inventory(True), "fixture")

    def test_inventory_hash_mutation(self) -> None:
        value = inventory(True)
        value["word_count"] += 1
        with self.assertRaises(base.ArtifactError):
            subject.validate_inventory_record(value, "fixture")

    def test_inventory_extra_nested_key(self) -> None:
        value = inventory(True)
        value["capabilities"][0]["extra"] = 1
        with self.assertRaises(base.ArtifactError):
            subject.validate_inventory_record(value, "fixture")

    def test_inventory_target_must_be_below_bound(self) -> None:
        value = inventory(True)
        value["non_uniform_decorations"][0]["target_id"] = value["id_bound"]
        unhashed = copy.deepcopy(value)
        unhashed.pop("inventory_sha256")
        value["inventory_sha256"] = base.digest_bytes(base.canonical_json(unhashed))
        with self.assertRaises(base.ArtifactError):
            subject.validate_inventory_record(value, "fixture")


class ReceiptTests(unittest.TestCase):
    def setUp(self) -> None:
        self.policy = copy.deepcopy(subject.load_policy())
        self.policy["reference_corpus"]["entry_order_sha256"] = base.digest_bytes(
            base.canonical_json([f"shader-{index}" for index in range(56)])
        )

    def make_entry(self, index: int, blocked: bool) -> dict:
        row = reference_row(blocked, index)
        if blocked:
            validation = {
                "arguments": ["--shader", "<private-staged-spv>", "--stage", "compute", "--entry", "CSMain"],
                "exit_code": 2, "stdout_sha256": subject.EMPTY_SHA256, "stdout_bytes": 0,
                "stderr_sha256": base.digest_bytes(subject.KNOWN_STDERR), "stderr_bytes": len(subject.KNOWN_STDERR),
            }
            result = None
            reason = self.policy["outcomes"]["blocked_known_shader_nonuniform"]["reason_code"]
        else:
            output = success_bytes(row)
            validation = {
                "arguments": ["--shader", "<private-staged-spv>", "--stage", "compute", "--entry", "CSMain"],
                "exit_code": 0, "stdout_sha256": base.digest_bytes(output), "stdout_bytes": len(output),
                "stderr_sha256": subject.EMPTY_SHA256, "stderr_bytes": 0,
            }
            result = {"schema": "fn64.wgpu-shader-validation.v1", "status": "passed", "wgpu_major": 30, "stage": "compute", "entry": "CSMain", "module_bytes": 20}
            reason = None
        return {
            **row, "scalar_layout_witness": None,
            "outcome": "blocked-known" if blocked else "ingestible", "reason_code": reason,
            "validation": validation, "validation_record": result,
        }

    def make_receipt(self) -> dict:
        entries = [self.make_entry(index, index == 0) for index in range(56)]
        ref = self.policy["reference_corpus"]
        validator = self.policy["wgpu_validator"]
        return base.add_receipt_hash({
            "schema": self.policy["receipt_schema"], "status": "complete",
            "producer_sha256": base.digest_file(subject.TOOL_PATH), "policy_sha256": base.digest_file(subject.POLICY_PATH),
            "reference_corpus": {key: value for key, value in ref.items() if key not in {"receipt_schema"}},
            "wgpu_validator": {
                "build_receipt_sha256": validator["build_receipt_sha256"], "binary_sha256": validator["binary_sha256"],
                "source_set_sha256": validator["source_set_sha256"], "cargo_lock_sha256": validator["cargo_lock_sha256"],
                "dependency_set_sha256": validator["dependency_set_sha256"], "identity": validator["identity"],
            },
            "assessment_contract": {
                "strict_capabilities": True, "noop_checked_shader_module": True,
                "arguments": validator["arguments"], "controlled_environment": validator["controlled_environment"],
                "outcome_order": self.policy["outcomes"]["order"],
            },
            "entries": entries, "outcome_counts": {"ingestible": 55, "blocked-known": 1},
            "assessment_set_sha256": base.digest_bytes(base.canonical_json(entries)),
            "runtime_readiness": subject.runtime_readiness(entries, self.policy),
            "claim_boundary": self.policy["claim_boundary"],
        })

    def rehash(self, receipt: dict) -> dict:
        return base.add_receipt_hash(receipt)

    def test_valid_receipt(self) -> None:
        subject.validate_assessment_receipt(self.make_receipt(), self.policy)

    def test_valid_scalar_layout_blocked_receipt(self) -> None:
        receipt = self.make_receipt()
        row = receipt["entries"][0]
        scalar = self.policy["outcomes"]["blocked_known_scalar_layout"]
        row["reason_code"] = scalar["reason_code"]
        row["scalar_layout_witness"] = subject.scalar_layout_witness(spirv_fixture(), self.policy)
        row["validation"].update({
            "exit_code": 2, "stdout_sha256": subject.EMPTY_SHA256, "stdout_bytes": 0,
            "stderr_sha256": base.digest_bytes(subject.SCALAR_LAYOUT_STDERR),
            "stderr_bytes": len(subject.SCALAR_LAYOUT_STDERR),
        })
        receipt["assessment_set_sha256"] = base.digest_bytes(base.canonical_json(receipt["entries"]))
        subject.validate_assessment_receipt(self.rehash(receipt), self.policy)

    def test_receipt_hash_mutation(self) -> None:
        receipt = self.make_receipt()
        receipt["status"] = "partial"
        with self.assertRaises(base.ArtifactError):
            subject.validate_assessment_receipt(receipt, self.policy)

    def test_receipt_rejects_runtime_ready_true(self) -> None:
        receipt = self.make_receipt()
        receipt["runtime_readiness"]["runtime_ready"] = True
        with self.assertRaises(base.ArtifactError):
            subject.validate_assessment_receipt(self.rehash(receipt), self.policy)

    def test_receipt_rejects_adapter_evidence_extra_field(self) -> None:
        receipt = self.make_receipt()
        receipt["runtime_readiness"]["adapter"] = {"features": ["anything"]}
        with self.assertRaises(base.ArtifactError):
            subject.validate_assessment_receipt(self.rehash(receipt), self.policy)

    def test_receipt_rejects_reordered_rows(self) -> None:
        receipt = self.make_receipt()
        receipt["entries"].reverse()
        receipt["assessment_set_sha256"] = base.digest_bytes(base.canonical_json(receipt["entries"]))
        with self.assertRaises(base.ArtifactError):
            subject.validate_assessment_receipt(self.rehash(receipt), self.policy)

    def test_receipt_rejects_duplicate_row(self) -> None:
        receipt = self.make_receipt()
        receipt["entries"][1]["id"] = receipt["entries"][0]["id"]
        receipt["assessment_set_sha256"] = base.digest_bytes(base.canonical_json(receipt["entries"]))
        with self.assertRaises(base.ArtifactError):
            subject.validate_assessment_receipt(self.rehash(receipt), self.policy)

    def test_receipt_rejects_inventory_mutation(self) -> None:
        receipt = self.make_receipt()
        receipt["entries"][0]["semantic_inventory"]["capabilities"].reverse()
        receipt["assessment_set_sha256"] = base.digest_bytes(base.canonical_json(receipt["entries"]))
        with self.assertRaises(base.ArtifactError):
            subject.validate_assessment_receipt(self.rehash(receipt), self.policy)

    def test_receipt_rejects_known_error_digest_mutation(self) -> None:
        receipt = self.make_receipt()
        receipt["entries"][0]["validation"]["stderr_sha256"] = "0" * 64
        receipt["assessment_set_sha256"] = base.digest_bytes(base.canonical_json(receipt["entries"]))
        with self.assertRaises(base.ArtifactError):
            subject.validate_assessment_receipt(self.rehash(receipt), self.policy)

    def test_receipt_rejects_validator_identity_mutation(self) -> None:
        receipt = self.make_receipt()
        receipt["wgpu_validator"]["identity"]["backend"] = "vulkan"
        with self.assertRaises(base.ArtifactError):
            subject.validate_assessment_receipt(self.rehash(receipt), self.policy)

    def test_receipt_rejects_reference_file_identity_mutation(self) -> None:
        receipt = self.make_receipt()
        receipt["reference_corpus"]["receipt_file_sha256"] = "0" * 64
        with self.assertRaises(base.ArtifactError):
            subject.validate_assessment_receipt(self.rehash(receipt), self.policy)


class OutputTests(unittest.TestCase):
    def test_failure_does_not_create_output(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary) / "output"
            args = argparse.Namespace(output_dir=str(output))
            with mock.patch.object(subject, "build_assessment", side_effect=base.ArtifactError("stop")), self.assertRaises(base.ArtifactError):
                subject.write_assessment(args)
            self.assertFalse(output.exists())

    def test_safe_relative_rejects_traversal_and_absolute(self) -> None:
        for value in ("../x.spv", "/tmp/x.spv", ""):
            with self.subTest(value=value), self.assertRaises(base.ArtifactError):
                subject.safe_relative(value, "fixture")

    def test_runtime_readiness_reason_order(self) -> None:
        policy = subject.load_policy()
        blocked = [{"outcome": "blocked-known"}]
        passed = [{"outcome": "ingestible"}]
        self.assertEqual(subject.runtime_readiness(blocked, policy)["reasons"], policy["runtime_readiness"]["reason_order"])
        self.assertEqual(subject.runtime_readiness(passed, policy)["reasons"], policy["runtime_readiness"]["reason_order"][1:])


if __name__ == "__main__":
    unittest.main()
