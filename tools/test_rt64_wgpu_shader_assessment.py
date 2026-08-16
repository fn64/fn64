#!/usr/bin/env python3

from __future__ import annotations

import argparse
import copy
import json
import os
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


def success_bytes(row: dict, module_bytes: int = 20, profile: str = "baseline") -> bytes:
    return subject.validator_success_bytes(profile, row["stage"], row["entry"], module_bytes)


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


def combined_scalar_and_fragment_fixture() -> bytes:
    """A single module that structurally matches both the scalar-layout and
    fragment-interface witness shapes at once, reproducing the real M2.5a
    ShaderNonUniform rows: both `instanceRDPParams` and a direct `PSMain`
    `SV_TARGET0` interface member exist in the same shader regardless of
    which diagnostic actually blocked it."""
    scalar_words = list(struct.unpack("<{}I".format(len(spirv_fixture()) // 4), spirv_fixture()))
    fragment_words = list(struct.unpack("<{}I".format(len(fragment_interface_fixture()) // 4), fragment_interface_fixture()))
    # scalar_fixture uses result ids 1-7; shift the fragment fixture's 1-9 by +100 to avoid collision.
    shift = 100

    def remap_fragment_stream(words: list[int]) -> list[int]:
        offset = 5
        out = list(words[:5])
        while offset < len(words):
            first = words[offset]
            word_count = first >> 16
            opcode = first & 0xFFFF
            operands = list(words[offset + 1: offset + word_count])
            if opcode == 15:  # OpEntryPoint: execution_model, entry_id, name..., interface...
                operands[1] += shift
                # name is nul-terminated ASCII words; interface ids follow, but here it's exactly one word (id 9).
                operands[-1] += shift
            elif opcode in (5, 71):  # OpName / OpDecorate: target is operands[0]; other operands are literals.
                operands[0] += shift
            elif opcode in (22, 21):  # OpTypeFloat/OpTypeInt: operands[0] is the result id; rest are width/sign literals.
                operands[0] += shift
            elif opcode == 23:  # OpTypeVector: result id, component type id, literal count.
                operands[0] += shift
                operands[1] += shift
            elif opcode == 32:  # OpTypePointer: result id, storage class literal, pointee type id.
                operands[0] += shift
                operands[2] += shift
            elif opcode == 59:  # OpVariable: result type id, result id, storage class literal.
                operands[0] += shift
                operands[1] += shift
            out.append(((len(operands) + 1) << 16) | opcode)
            out.extend(operands)
            offset += word_count
        return out

    merged = list(scalar_words[:4]) + [0]
    merged[3] = max(scalar_words[3], shift + fragment_words[3])
    merged += scalar_words[5:]
    merged += remap_fragment_stream(fragment_words)[5:]
    return struct.pack(f"<{len(merged)}I", *merged)


def sampled_buffer_fixture(*, capability: int = 46, duplicate: bool = False, extra_capability: int | None = None) -> bytes:
    words = [0x07230203, 0x00010000, 0, 2, 0]

    def instruction(opcode: int, *operands: int) -> None:
        words.append(((len(operands) + 1) << 16) | opcode)
        words.extend(operands)

    instruction(17, capability)  # OpCapability
    if duplicate:
        instruction(17, capability)
    if extra_capability is not None:
        instruction(17, extra_capability)
    return struct.pack(f"<{len(words)}I", *words)


def fragment_interface_fixture(
    *,
    stage: str = "fragment",
    entry: str = "PSMain",
    variable_name: str = "out.var.SV_TARGET0",
    storage_class: int = 3,
    direct_interface: bool = True,
    vector_length: int = 4,
    component_type: str = "float",
    location: int = 0,
    index: int = 0,
    missing_location: bool = False,
    missing_index: bool = False,
    index_decoration_kind: int = subject.SPIRV_DECORATION_INDEX,
    execution_model: int = 4,
) -> bytes:
    words = [0x07230203, 0x00010000, 0, 10, 0]

    def instruction(opcode: int, *operands: int) -> None:
        words.append(((len(operands) + 1) << 16) | opcode)
        words.extend(operands)

    def literal(value: str) -> list[int]:
        data = value.encode() + b"\0"
        data += b"\0" * (-len(data) % 4)
        return list(struct.unpack(f"<{len(data) // 4}I", data))

    interface_ids = [9] if direct_interface else []
    instruction(15, execution_model, 8, *literal(entry), *interface_ids)  # OpEntryPoint
    instruction(5, 9, *literal(variable_name))  # OpName
    if not missing_location:
        instruction(71, 9, subject.SPIRV_DECORATION_LOCATION, location)  # Decorate Location
    if not missing_index:
        instruction(71, 9, index_decoration_kind, index)  # Decorate Index (or a hostile substitute kind)
    type_ids = {"float": 1, "uint": 2, "int": 3}
    instruction(22, 1, 32)  # OpTypeFloat 32-bit
    instruction(21, 2, 32, 0)  # OpTypeInt uint
    instruction(21, 3, 32, 1)  # OpTypeInt int
    instruction(23, 4, type_ids[component_type], vector_length)  # OpTypeVector
    instruction(32, 5, storage_class, 4)  # OpTypePointer
    instruction(59, 5, 9, storage_class)  # OpVariable
    return struct.pack(f"<{len(words)}I", *words)


def push_constant_fixture(
    member_types: tuple[str, ...] = ("f32",),
    offsets: tuple[int, ...] = (0,),
    *,
    block: bool = True,
    missing_offset: int | None = None,
    duplicate_offset: int | None = None,
    multiple_globals: bool = False,
    group_decoration: bool = False,
    missing_variable_name: bool = False,
    missing_struct_name: bool = False,
    missing_member_name: int | None = None,
) -> bytes:
    words = [0x07230203, 0x00010000, 0, 32, 0]

    def instruction(opcode: int, *operands: int) -> None:
        words.append(((len(operands) + 1) << 16) | opcode)
        words.extend(operands)

    def literal(value: str) -> list[int]:
        data = value.encode() + b"\0"
        data += b"\0" * (-len(data) % 4)
        return list(struct.unpack(f"<{len(data) // 4}I", data))

    type_ids = {
        "f32": 1, "u32": 2, "i32": 3, "vec2f": 4, "vec3f": 5, "vec4u": 6,
        "bool": 10, "f64": 11, "vec1f": 12, "vec5f": 13, "matrix": 14,
        "array": 15, "runtime-array": 16, "nested": 17, "unknown": 18,
        "recursive": 7,
    }
    if not missing_variable_name:
        instruction(5, 9, *literal("pc"))
    if not missing_struct_name:
        instruction(5, 7, *literal("PushConstants"))
    for index in range(len(member_types)):
        if index != missing_member_name:
            instruction(6, 7, index, *literal(f"member{index}"))
    if block:
        instruction(71, 7, 2)
    for index, member_offset in enumerate(offsets):
        if index != missing_offset:
            instruction(72, 7, index, 35, member_offset)
        if index == duplicate_offset:
            instruction(72, 7, index, 35, member_offset)
    if group_decoration:
        instruction(73, 19)
    instruction(22, 1, 32)
    instruction(21, 2, 32, 0)
    instruction(21, 3, 32, 1)
    instruction(23, 4, 1, 2)
    instruction(23, 5, 1, 3)
    instruction(23, 6, 2, 4)
    instruction(20, 10)
    instruction(22, 11, 64)
    instruction(23, 12, 1, 1)
    instruction(23, 13, 1, 5)
    instruction(24, 14, 4, 2)
    instruction(28, 15, 1, 20)
    instruction(29, 16, 1)
    instruction(30, 17, 1)
    instruction(30, 7, *(type_ids[name] for name in member_types))
    instruction(32, 8, 9, 7)
    instruction(59, 8, 9, 9)
    if multiple_globals:
        instruction(59, 8, 19, 9)
    return struct.pack(f"<{len(words)}I", *words)


class PolicyTests(unittest.TestCase):
    def test_policy_is_exact_and_selftest_passes(self) -> None:
        self.assertEqual(subject.load_policy()["reference_corpus"]["row_count"], 56)
        subject.selftest()

    def test_parser_has_five_commands(self) -> None:
        parser = subject.parser()
        self.assertEqual(parser.parse_args(["selftest"]).command, "selftest")
        census = parser.parse_args([
            "diagnostic-census", "--reference-artifact-dir", "/corpus",
            "--wgpu-validator-build-dir", "/validator",
        ])
        self.assertEqual(census.command, "diagnostic-census")
        self.assertFalse(hasattr(census, "output_dir"))
        self.assertFalse(hasattr(census, "assessment_dir"))
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

    def test_policy_rejects_guessed_validator_hash_while_pending(self) -> None:
        policy = base.load_json(subject.POLICY_PATH)
        policy["wgpu_validator"].update({
            "build_receipt_sha256": None,
            "binary_sha256": None,
            "source_set_sha256": None,
            "cargo_lock_sha256": None,
            "dependency_set_sha256": None,
            "artifact_identity_status": "pending-m2.4-v2-integration",
        })
        policy["wgpu_validator"]["binary_sha256"] = "0" * 64
        with mock.patch.object(base, "load_json", return_value=policy), self.assertRaisesRegex(base.ArtifactError, "guessed hashes"):
            subject.load_policy()


class DecorationConstantImportGuardTests(unittest.TestCase):
    """Proves the Location/Index/InputAttachmentIndex pin fires at import time and
    survives `python -O`, where a bare `assert` would silently vanish."""

    def run_tampered_import(self, replacement_index: int, *, optimized: bool) -> subprocess.CompletedProcess[bytes]:
        source = subject.__file__
        original = Path(source).read_text()
        # Tamper with a value that is still pairwise-distinct from the other two, so a
        # weaker pairwise-only guard (the pre-repair form) would have let this through.
        self.assertNotEqual(replacement_index, subject.SPIRV_DECORATION_INPUT_ATTACHMENT_INDEX)
        self.assertNotEqual(replacement_index, subject.SPIRV_DECORATION_LOCATION)
        tampered = original.replace(
            "SPIRV_DECORATION_INDEX = 32", f"SPIRV_DECORATION_INDEX = {replacement_index}", 1,
        )
        self.assertNotEqual(tampered, original, "tamper target line not found")
        with tempfile.TemporaryDirectory() as tmp:
            tampered_path = Path(tmp) / "rt64_wgpu_shader_assessment.py"
            tampered_path.write_text(tampered)
            args = [sys.executable]
            if optimized:
                args.append("-O")
            args += ["-c", "import rt64_wgpu_shader_assessment"]
            env = {**os.environ, "PYTHONPATH": f"{tmp}:{Path(source).resolve().parent}"}
            return subprocess.run(args, env=env, capture_output=True)

    def test_tampered_index_constant_fails_import_normally(self) -> None:
        result = self.run_tampered_import(33, optimized=False)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(b"pinned Location/Index/InputAttachmentIndex", result.stderr)

    def test_tampered_index_constant_fails_import_under_dash_o(self) -> None:
        result = self.run_tampered_import(33, optimized=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(b"pinned Location/Index/InputAttachmentIndex", result.stderr)


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


class ValidatorProfileDerivationTests(unittest.TestCase):
    def setUp(self) -> None:
        self.policy = subject.load_policy()

    def derive(self, value: bytes) -> dict:
        return subject.derive_validator_profile(value, self.policy)

    def test_baseline_requires_exact_absence_witness(self) -> None:
        derived = self.derive(EMPTY_SPIRV)
        self.assertIsNone(derived["immediate_witness"])
        self.assertEqual(derived["profile"], subject.validator_profile("baseline"))
        subject.validate_profile_derivation_record(derived, "fixture")

    def test_every_closed_extent_selects_exact_minimum_profile(self) -> None:
        cases = {
            4: (("f32",), (0,)),
            8: (("vec2f",), (0,)),
            16: (("vec3f",), (0,)),
            20: (("f32",) * 5, (0, 4, 8, 12, 16)),
            24: (("vec2f", "f32", "f32", "f32"), (0, 8, 12, 16)),
            32: (("vec4u", "vec4u"), (0, 16)),
            40: (("vec2f",) + ("f32",) * 7, (0, 8, 12, 16, 20, 24, 28, 32)),
            56: (("vec2f",) + ("f32",) * 11, (0, 8, 12, 16, 20, 24, 28, 32, 36, 40, 44, 48)),
        }
        for extent, (types, offsets) in cases.items():
            with self.subTest(extent=extent):
                derived = self.derive(push_constant_fixture(types, offsets))
                self.assertEqual(derived["immediate_witness"]["required_max_immediate_size"], extent)
                self.assertEqual(derived["profile"], subject.validator_profile(f"immediates-{extent}"))
                subject.validate_profile_derivation_record(derived, "fixture")

    def test_witness_binds_ids_names_types_offsets_and_sizes(self) -> None:
        derived = self.derive(push_constant_fixture(("u32", "vec3f"), (0, 4)))
        witness = derived["immediate_witness"]
        self.assertEqual(witness["variable_id"], 9)
        self.assertEqual(witness["pointer_type_id"], 8)
        self.assertEqual(witness["struct_type_id"], 7)
        self.assertEqual(witness["members"], [
            {"index": 0, "name": "member0", "type": "uint", "offset": 0, "size": 4},
            {"index": 1, "name": "member1", "type": "float3", "offset": 4, "size": 12},
        ])
        mutated = copy.deepcopy(derived)
        mutated["immediate_witness"]["members"][0]["offset"] = 4
        with self.assertRaisesRegex(base.ArtifactError, "overlap"):
            subject.validate_profile_derivation_record(mutated, "fixture")

    def test_mixed_alignment_rounds_raw_content_extent_like_naga(self) -> None:
        cases = (
            (("vec2f", "vec2f", "f32"), (0, 8, 16), 20, 24),
            (("vec2f",) + ("f32",) * 7, (0, 8, 12, 16, 20, 24, 28, 32), 36, 40),
            (("vec2f",) + ("f32",) * 11, (0, 8, 12, 16, 20, 24, 28, 32, 36, 40, 44, 48), 52, 56),
            (("vec3f", "f32", "f32"), (0, 12, 16), 20, 32),
        )
        for types, offsets, raw_extent, rounded_span in cases:
            with self.subTest(raw_extent=raw_extent, rounded_span=rounded_span):
                derived = self.derive(push_constant_fixture(types, offsets))
                self.assertEqual(offsets[-1] + derived["immediate_witness"]["members"][-1]["size"], raw_extent)
                self.assertEqual(derived["immediate_witness"]["required_max_immediate_size"], rounded_span)
                self.assertEqual(derived["profile"], subject.validator_profile(f"immediates-{rounded_span}"))

    def test_rejects_multiple_push_constant_globals(self) -> None:
        with self.assertRaisesRegex(base.ArtifactError, "multiple PushConstant"):
            self.derive(push_constant_fixture(multiple_globals=True))

    def test_rejects_group_decorations(self) -> None:
        with self.assertRaisesRegex(base.ArtifactError, "group decoration"):
            self.derive(push_constant_fixture(group_decoration=True))

    def test_rejects_missing_or_duplicate_offsets(self) -> None:
        with self.assertRaisesRegex(base.ArtifactError, "lacks one exact Offset"):
            self.derive(push_constant_fixture(missing_offset=0))
        with self.assertRaisesRegex(base.ArtifactError, "duplicate profile OpMemberDecorate"):
            self.derive(push_constant_fixture(duplicate_offset=0))

    def test_rejects_missing_names_or_block(self) -> None:
        for kwargs, message in (
            ({"missing_variable_name": True}, "variable name"),
            ({"missing_struct_name": True}, "struct name"),
            ({"missing_member_name": 0}, "member 0 name"),
            ({"block": False}, "Block decoration"),
        ):
            with self.subTest(kwargs=kwargs), self.assertRaisesRegex(base.ArtifactError, message):
                self.derive(push_constant_fixture(**kwargs))

    def test_rejects_unsupported_widths_shapes_and_composites(self) -> None:
        for kind in ("bool", "f64", "vec1f", "vec5f", "matrix", "array", "runtime-array", "nested", "recursive", "unknown"):
            with self.subTest(kind=kind), self.assertRaises(base.ArtifactError):
                self.derive(push_constant_fixture((kind,), (0,)))

    def test_rejects_overlap_overflow_and_unreviewed_extent(self) -> None:
        with self.assertRaisesRegex(base.ArtifactError, "overlap"):
            self.derive(push_constant_fixture(("vec4u", "f32"), (0, 12)))
        with self.assertRaisesRegex(base.ArtifactError, "overflows"):
            self.derive(push_constant_fixture(("vec2f",), (0xFFFFFFFC,)))
        with self.assertRaisesRegex(base.ArtifactError, "round-up overflows"):
            self.derive(push_constant_fixture(("vec4u",), (0xFFFFFFEC,)))
        with self.assertRaisesRegex(base.ArtifactError, "unreviewed"):
            self.derive(push_constant_fixture(("f32",), (8,)))
        for raw_size in (36, 52):
            with self.subTest(raw_size=raw_size), self.assertRaisesRegex(base.ArtifactError, "unreviewed"):
                self.derive(push_constant_fixture(("f32",) * (raw_size // 4), tuple(range(0, raw_size, 4))))


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
        outcome, reason, record, scalar_witness, buffer_witness, fragment_witness = subject.classify_result(self.completed(2, b"", subject.KNOWN_STDERR), row, EMPTY_SPIRV, self.policy)
        self.assertEqual(
            (outcome, reason, record, scalar_witness, buffer_witness, fragment_witness),
            ("blocked-known", self.policy["outcomes"]["blocked_known_shader_nonuniform"]["reason_code"], None, None, None, None),
        )

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

    def test_shader_nonuniform_serializes_no_witness_when_module_also_matches_scalar_layout(self) -> None:
        # Regression for the v5 receipt bug: a ShaderNonUniform-blocked row whose SPIR-V
        # also structurally matches the scalar-layout witness must not leak that witness.
        row = reference_row(True)
        row["stage"], row["entry"] = "fragment", "PSMain"
        module = spirv_fixture()
        outcome, reason, record, scalar_witness, buffer_witness, fragment_witness = subject.classify_result(
            self.completed(2, b"", subject.KNOWN_STDERR), row, module, self.policy
        )
        self.assertEqual(
            (outcome, reason, record, scalar_witness, buffer_witness, fragment_witness),
            ("blocked-known", self.policy["outcomes"]["blocked_known_shader_nonuniform"]["reason_code"], None, None, None, None),
        )

    def test_shader_nonuniform_serializes_no_witness_when_module_also_matches_fragment_interface(self) -> None:
        row = reference_row(True)
        row["stage"], row["entry"] = "fragment", "PSMain"
        module = fragment_interface_fixture()
        outcome, reason, record, scalar_witness, buffer_witness, fragment_witness = subject.classify_result(
            self.completed(2, b"", subject.KNOWN_STDERR), row, module, self.policy
        )
        self.assertEqual(
            (outcome, reason, record, scalar_witness, buffer_witness, fragment_witness),
            ("blocked-known", self.policy["outcomes"]["blocked_known_shader_nonuniform"]["reason_code"], None, None, None, None),
        )

    def test_shader_nonuniform_serializes_no_witness_when_module_matches_both_scalar_and_fragment(self) -> None:
        # Exact reproduction shape of all six real M2.5a ShaderNonUniform rows
        # (RasterPSDynamic, RasterPSDynamicMS, RasterPSSpecConstant,
        # RasterPSSpecConstantMS, RasterPSSpecConstantFlat, RasterPSSpecConstantFlatMS):
        # instanceRDPParams and a direct PSMain SV_TARGET0 interface member both present.
        row = reference_row(True)
        row["stage"], row["entry"] = "fragment", "PSMain"
        module = combined_scalar_and_fragment_fixture()
        outcome, reason, record, scalar_witness, buffer_witness, fragment_witness = subject.classify_result(
            self.completed(2, b"", subject.KNOWN_STDERR), row, module, self.policy
        )
        self.assertEqual(
            (outcome, reason, record, scalar_witness, buffer_witness, fragment_witness),
            ("blocked-known", self.policy["outcomes"]["blocked_known_shader_nonuniform"]["reason_code"], None, None, None, None),
        )


class ScalarLayoutClassificationTests(unittest.TestCase):
    def setUp(self) -> None:
        self.policy = subject.load_policy()
        self.row = reference_row(False)

    def completed(self, code: int, stdout: bytes, stderr: bytes) -> subprocess.CompletedProcess[bytes]:
        return subprocess.CompletedProcess(["validator"], code, stdout, stderr)

    def test_exact_scalar_layout_witness_and_error_are_blocked_known(self) -> None:
        module = spirv_fixture()
        outcome, reason, record, scalar_witness, buffer_witness, fragment_witness = subject.classify_result(
            self.completed(2, b"", subject.SCALAR_LAYOUT_STDERR), self.row, module, self.policy
        )
        self.assertEqual(outcome, "blocked-known")
        self.assertEqual(reason, self.policy["outcomes"]["blocked_known_scalar_layout"]["reason_code"])
        self.assertIsNone(record)
        self.assertIsNone(buffer_witness)
        self.assertIsNone(fragment_witness)
        subject.validate_scalar_witness_record(scalar_witness, self.policy, "fixture")

    def test_scalar_layout_row_drops_coincidental_fragment_interface_match(self) -> None:
        # A scalar-layout-blocked row whose module also structurally matches the
        # fragment-interface shape must serialize only its own witness.
        row = reference_row(False)
        row["stage"], row["entry"] = "fragment", "PSMain"
        module = combined_scalar_and_fragment_fixture()
        outcome, reason, record, scalar_witness, buffer_witness, fragment_witness = subject.classify_result(
            self.completed(2, b"", subject.SCALAR_LAYOUT_STDERR), row, module, self.policy
        )
        self.assertEqual(outcome, "blocked-known")
        self.assertEqual(reason, self.policy["outcomes"]["blocked_known_scalar_layout"]["reason_code"])
        self.assertIsNone(buffer_witness)
        self.assertIsNone(fragment_witness)
        subject.validate_scalar_witness_record(scalar_witness, self.policy, "fixture")

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


class SampledBufferClassificationTests(unittest.TestCase):
    def setUp(self) -> None:
        self.policy = subject.load_policy()
        self.row = reference_row(False)

    def completed(self, code: int, stdout: bytes, stderr: bytes) -> subprocess.CompletedProcess[bytes]:
        return subprocess.CompletedProcess(["validator"], code, stdout, stderr)

    def test_exact_sampled_buffer_witness_and_error_are_blocked_known(self) -> None:
        module = sampled_buffer_fixture()
        outcome, reason, record, scalar_witness, buffer_witness, fragment_witness = subject.classify_result(
            self.completed(2, b"", subject.SAMPLED_BUFFER_STDERR), self.row, module, self.policy
        )
        self.assertEqual(outcome, "blocked-known")
        self.assertEqual(reason, self.policy["outcomes"]["blocked_known_sampled_buffer"]["reason_code"])
        self.assertIsNone(record)
        self.assertIsNone(scalar_witness)
        self.assertIsNone(fragment_witness)
        subject.validate_sampled_buffer_witness_record(buffer_witness, self.policy, "fixture")

    def test_sampled_buffer_witness_unexpected_success_is_fatal(self) -> None:
        module = sampled_buffer_fixture()
        with self.assertRaisesRegex(base.ArtifactError, "unexpectedly passed"):
            subject.classify_result(
                self.completed(0, success_bytes(self.row, len(module)), b""), self.row, module, self.policy
            )

    def test_sampled_buffer_stderr_near_match_is_fatal(self) -> None:
        with self.assertRaises(base.ArtifactError):
            subject.classify_result(
                self.completed(2, b"", subject.SAMPLED_BUFFER_STDERR[:-1]),
                self.row, sampled_buffer_fixture(), self.policy,
            )

    def test_sampled_buffer_requires_empty_stdout(self) -> None:
        with self.assertRaises(base.ArtifactError):
            subject.classify_result(
                self.completed(2, b"{}\n", subject.SAMPLED_BUFFER_STDERR),
                self.row, sampled_buffer_fixture(), self.policy,
            )

    def test_sampled_buffer_requires_exact_exit_code(self) -> None:
        with self.assertRaises(base.ArtifactError):
            subject.classify_result(
                self.completed(1, b"", subject.SAMPLED_BUFFER_STDERR),
                self.row, sampled_buffer_fixture(), self.policy,
            )

    def test_sampled_buffer_pass_shaped_result_without_witness_is_fatal(self) -> None:
        with self.assertRaisesRegex(base.ArtifactError, "unexpectedly passed|unclassified"):
            subject.classify_result(
                self.completed(0, success_bytes(self.row), b""),
                self.row, sampled_buffer_fixture(), self.policy,
            )

    def test_sampled_buffer_witness_is_none_when_capability_absent(self) -> None:
        self.assertIsNone(subject.sampled_buffer_witness(EMPTY_SPIRV, self.policy))

    def test_sampled_buffer_witness_rejects_wrong_capability_value(self) -> None:
        self.assertIsNone(subject.sampled_buffer_witness(sampled_buffer_fixture(capability=45), self.policy))

    def test_sampled_buffer_witness_rejects_duplicate_capability(self) -> None:
        with self.assertRaisesRegex(base.ArtifactError, "exactly once"):
            subject.sampled_buffer_witness(sampled_buffer_fixture(duplicate=True), self.policy)

    def test_sampled_buffer_witness_ignores_unrelated_extra_capability(self) -> None:
        witness = subject.sampled_buffer_witness(sampled_buffer_fixture(extra_capability=1), self.policy)
        self.assertIsNotNone(witness)
        self.assertEqual(witness["capability"], {"name": "SampledBuffer", "value": 46})

    def test_sampled_buffer_witness_records_exact_word_offset(self) -> None:
        module = sampled_buffer_fixture(extra_capability=1)
        witness = subject.sampled_buffer_witness(module, self.policy)
        self.assertEqual(witness["word_offset"], 5)

    def test_sampled_buffer_witness_rejects_group_decoration(self) -> None:
        words = [0x07230203, 0x00010000, 0, 20, 0, (2 << 16) | 17, 46, (2 << 16) | 73, 19]
        module = struct.pack(f"<{len(words)}I", *words)
        with self.assertRaisesRegex(base.ArtifactError, "not implemented"):
            subject.sampled_buffer_witness(module, self.policy)

    def test_validate_sampled_buffer_witness_record_rejects_field_mutation(self) -> None:
        witness = subject.sampled_buffer_witness(sampled_buffer_fixture(), self.policy)
        for key, value in (("capability", {"name": "SampledBuffer", "value": 47}), ("word_offset", 9999)):
            with self.subTest(key=key):
                mutated = copy.deepcopy(witness)
                mutated[key] = value
                with self.assertRaises(base.ArtifactError):
                    subject.validate_sampled_buffer_witness_record(mutated, self.policy, "fixture")

    def test_validate_sampled_buffer_witness_record_rejects_hash_mutation(self) -> None:
        witness = subject.sampled_buffer_witness(sampled_buffer_fixture(), self.policy)
        witness["witness_sha256"] = "0" * 64
        with self.assertRaisesRegex(base.ArtifactError, "identity changed"):
            subject.validate_sampled_buffer_witness_record(witness, self.policy, "fixture")

    def test_validate_sampled_buffer_witness_record_rejects_extra_key(self) -> None:
        witness = subject.sampled_buffer_witness(sampled_buffer_fixture(), self.policy)
        witness["extra"] = True
        unhashed = copy.deepcopy(witness)
        unhashed.pop("witness_sha256")
        witness["witness_sha256"] = base.digest_bytes(base.canonical_json(unhashed))
        with self.assertRaises(base.ArtifactError):
            subject.validate_sampled_buffer_witness_record(witness, self.policy, "fixture")


class FragmentInterfaceClassificationTests(unittest.TestCase):
    def setUp(self) -> None:
        self.policy = subject.load_policy()
        self.row = {**reference_row(False), "stage": "fragment", "entry": "PSMain"}

    def completed(self, code: int, stdout: bytes, stderr: bytes) -> subprocess.CompletedProcess[bytes]:
        return subprocess.CompletedProcess(["validator"], code, stdout, stderr)

    def test_exact_fragment_interface_witness_and_error_are_blocked_known(self) -> None:
        module = fragment_interface_fixture()
        outcome, reason, record, scalar_witness, buffer_witness, fragment_witness = subject.classify_result(
            self.completed(2, b"", subject.FRAGMENT_INTERFACE_STDERR), self.row, module, self.policy
        )
        self.assertEqual(outcome, "blocked-known")
        self.assertEqual(reason, self.policy["outcomes"]["blocked_known_fragment_direct_blend_src_index_output"]["reason_code"])
        self.assertIsNone(record)
        self.assertIsNone(scalar_witness)
        self.assertIsNone(buffer_witness)
        subject.validate_fragment_interface_witness_record(fragment_witness, self.policy, "fixture")

    def test_fragment_interface_row_drops_coincidental_scalar_layout_match(self) -> None:
        # A fragment-interface-blocked row whose module also structurally matches
        # the scalar-layout shape must serialize only its own witness.
        module = combined_scalar_and_fragment_fixture()
        outcome, reason, record, scalar_witness, buffer_witness, fragment_witness = subject.classify_result(
            self.completed(2, b"", subject.FRAGMENT_INTERFACE_STDERR), self.row, module, self.policy
        )
        self.assertEqual(outcome, "blocked-known")
        self.assertEqual(reason, self.policy["outcomes"]["blocked_known_fragment_direct_blend_src_index_output"]["reason_code"])
        self.assertIsNone(scalar_witness)
        self.assertIsNone(buffer_witness)
        subject.validate_fragment_interface_witness_record(fragment_witness, self.policy, "fixture")

    def test_fragment_interface_witness_unexpected_success_is_fatal(self) -> None:
        module = fragment_interface_fixture()
        with self.assertRaisesRegex(base.ArtifactError, "unexpectedly passed"):
            subject.classify_result(
                self.completed(0, success_bytes(self.row, len(module)), b""), self.row, module, self.policy
            )

    def test_fragment_interface_stderr_near_match_is_fatal(self) -> None:
        with self.assertRaises(base.ArtifactError):
            subject.classify_result(
                self.completed(2, b"", subject.FRAGMENT_INTERFACE_STDERR[:-1]),
                self.row, fragment_interface_fixture(), self.policy,
            )

    def test_fragment_interface_requires_empty_stdout(self) -> None:
        with self.assertRaises(base.ArtifactError):
            subject.classify_result(
                self.completed(2, b"{}\n", subject.FRAGMENT_INTERFACE_STDERR),
                self.row, fragment_interface_fixture(), self.policy,
            )

    def test_fragment_interface_requires_exact_exit_code(self) -> None:
        with self.assertRaises(base.ArtifactError):
            subject.classify_result(
                self.completed(1, b"", subject.FRAGMENT_INTERFACE_STDERR),
                self.row, fragment_interface_fixture(), self.policy,
            )

    def test_fragment_interface_pass_shaped_result_without_witness_is_fatal(self) -> None:
        with self.assertRaisesRegex(base.ArtifactError, "unexpectedly passed|unclassified"):
            subject.classify_result(
                self.completed(0, success_bytes(self.row), b""),
                self.row, fragment_interface_fixture(), self.policy,
            )

    def test_fragment_interface_witness_none_for_wrong_stage(self) -> None:
        self.assertIsNone(subject.fragment_interface_witness(fragment_interface_fixture(), "vertex", "PSMain", self.policy))

    def test_fragment_interface_witness_none_for_wrong_entry_name(self) -> None:
        self.assertIsNone(subject.fragment_interface_witness(fragment_interface_fixture(entry="VSMain"), "fragment", "VSMain", self.policy))

    def test_fragment_interface_witness_none_when_variable_name_absent(self) -> None:
        self.assertIsNone(subject.fragment_interface_witness(fragment_interface_fixture(variable_name="out.var.OTHER"), "fragment", "PSMain", self.policy))

    def test_fragment_interface_witness_rejects_when_not_direct_interface_member(self) -> None:
        with self.assertRaisesRegex(base.ArtifactError, "interface member"):
            subject.fragment_interface_witness(fragment_interface_fixture(direct_interface=False), "fragment", "PSMain", self.policy)

    def test_fragment_interface_witness_rejects_wrong_storage_class(self) -> None:
        with self.assertRaisesRegex(base.ArtifactError, "storage class"):
            subject.fragment_interface_witness(fragment_interface_fixture(storage_class=1), "fragment", "PSMain", self.policy)

    def test_fragment_interface_witness_rejects_pointer_storage_mismatch(self) -> None:
        module = fragment_interface_fixture()
        words = list(struct.unpack(f"<{len(module) // 4}I", module))
        for index in range(len(words) - 3):
            if words[index] == ((4 << 16) | 32) and words[index + 2] == 3:
                words[index + 2] = 1
                break
        else:
            self.fail("OpTypePointer instruction not found in fixture")
        mutated = struct.pack(f"<{len(words)}I", *words)
        with self.assertRaisesRegex(base.ArtifactError, "pointer storage class"):
            subject.fragment_interface_witness(mutated, "fragment", "PSMain", self.policy)

    def test_fragment_interface_witness_rejects_non_float4(self) -> None:
        with self.assertRaisesRegex(base.ArtifactError, "member type"):
            subject.fragment_interface_witness(fragment_interface_fixture(vector_length=3), "fragment", "PSMain", self.policy)
        with self.assertRaisesRegex(base.ArtifactError, "member type"):
            subject.fragment_interface_witness(fragment_interface_fixture(component_type="uint"), "fragment", "PSMain", self.policy)

    def test_fragment_interface_witness_rejects_missing_location(self) -> None:
        with self.assertRaisesRegex(base.ArtifactError, "location"):
            subject.fragment_interface_witness(fragment_interface_fixture(missing_location=True), "fragment", "PSMain", self.policy)

    def test_fragment_interface_witness_rejects_missing_index(self) -> None:
        with self.assertRaisesRegex(base.ArtifactError, "index"):
            subject.fragment_interface_witness(fragment_interface_fixture(missing_index=True), "fragment", "PSMain", self.policy)

    def test_fragment_interface_witness_rejects_wrong_location(self) -> None:
        with self.assertRaisesRegex(base.ArtifactError, "location"):
            subject.fragment_interface_witness(fragment_interface_fixture(location=1), "fragment", "PSMain", self.policy)

    def test_fragment_interface_witness_rejects_wrong_index(self) -> None:
        with self.assertRaisesRegex(base.ArtifactError, "index"):
            subject.fragment_interface_witness(fragment_interface_fixture(index=1), "fragment", "PSMain", self.policy)

    def test_fragment_interface_witness_accepts_exact_decoration_index_value(self) -> None:
        # Literal 32, not subject.SPIRV_DECORATION_INDEX: if the subject constant were
        # itself wrong, referencing it here would make accept and reject correlated-wrong
        # again, exactly the bug this test exists to catch.
        self.assertEqual(subject.SPIRV_DECORATION_INDEX, 32)
        module = fragment_interface_fixture(index_decoration_kind=32)
        witness = subject.fragment_interface_witness(module, "fragment", "PSMain", self.policy)
        self.assertEqual(witness["index"], 0)

    def test_fragment_interface_witness_rejects_input_attachment_index_decoration(self) -> None:
        self.assertEqual(subject.SPIRV_DECORATION_INPUT_ATTACHMENT_INDEX, 43)
        module = fragment_interface_fixture(index_decoration_kind=43)
        with self.assertRaisesRegex(base.ArtifactError, "index"):
            subject.fragment_interface_witness(module, "fragment", "PSMain", self.policy)

    def test_fragment_interface_witness_rejects_duplicate_index_decoration(self) -> None:
        module = fragment_interface_fixture()
        words = list(struct.unpack(f"<{len(module) // 4}I", module))
        extra = [(4 << 16) | 71, 9, subject.SPIRV_DECORATION_INDEX, 0]  # duplicate Decorate Index
        mutated = struct.pack(f"<{len(words) + len(extra)}I", *(words + extra))
        with self.assertRaisesRegex(base.ArtifactError, "duplicate OpDecorate"):
            subject.fragment_interface_witness(mutated, "fragment", "PSMain", self.policy)

    def test_fragment_interface_witness_rejects_group_decoration(self) -> None:
        module = fragment_interface_fixture()
        words = list(struct.unpack(f"<{len(module) // 4}I", module)) + [(2 << 16) | 73, 19]
        words[3] = 20
        mutated = struct.pack(f"<{len(words)}I", *words)
        with self.assertRaisesRegex(base.ArtifactError, "not implemented"):
            subject.fragment_interface_witness(mutated, "fragment", "PSMain", self.policy)

    def test_fragment_interface_witness_rejects_ambiguous_entry_point(self) -> None:
        module = fragment_interface_fixture()
        words = list(struct.unpack(f"<{len(module) // 4}I", module))

        def literal(value: str) -> list[int]:
            data = value.encode() + b"\0"
            data += b"\0" * (-len(data) % 4)
            return list(struct.unpack(f"<{len(data) // 4}I", data))

        name_words = literal("PSMain")
        extra = [((3 + len(name_words)) << 16) | 15, 4, 8] + name_words
        words[3] = 12
        combined = words + extra
        mutated = struct.pack(f"<{len(combined)}I", *combined)
        with self.assertRaisesRegex(base.ArtifactError, "ambiguous"):
            subject.fragment_interface_witness(mutated, "fragment", "PSMain", self.policy)

    def test_validate_fragment_interface_witness_record_rejects_field_mutation(self) -> None:
        witness = subject.fragment_interface_witness(fragment_interface_fixture(), "fragment", "PSMain", self.policy)
        for key, value in (("location", 1), ("index", 1), ("type", "float3"), ("storage_class", "Input"), ("variable_name", "other")):
            with self.subTest(key=key):
                mutated = copy.deepcopy(witness)
                mutated[key] = value
                with self.assertRaisesRegex(base.ArtifactError, "fields changed"):
                    subject.validate_fragment_interface_witness_record(mutated, self.policy, "fixture")

    def test_validate_fragment_interface_witness_record_rejects_hash_mutation(self) -> None:
        witness = subject.fragment_interface_witness(fragment_interface_fixture(), "fragment", "PSMain", self.policy)
        witness["witness_sha256"] = "0" * 64
        with self.assertRaisesRegex(base.ArtifactError, "identity changed"):
            subject.validate_fragment_interface_witness_record(witness, self.policy, "fixture")


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
        self.policy["profile_derivation"]["expected_corpus_profile_counts"] = {
            name: 56 if name == "baseline" else 0 for name in subject.PROFILE_NAMES
        }

    def make_entry(self, index: int, blocked: bool) -> dict:
        row = reference_row(blocked, index)
        profile_derivation = subject.derive_validator_profile(EMPTY_SPIRV, self.policy)
        selected_profile = profile_derivation["profile"]
        if blocked:
            validation = {
                "arguments": subject.validator_arguments("baseline", "compute", "CSMain"),
                "exit_code": 2, "stdout_sha256": subject.EMPTY_SHA256, "stdout_bytes": 0,
                "stderr_sha256": base.digest_bytes(subject.KNOWN_STDERR), "stderr_bytes": len(subject.KNOWN_STDERR),
            }
            result = None
            reason = self.policy["outcomes"]["blocked_known_shader_nonuniform"]["reason_code"]
        else:
            output = success_bytes(row)
            validation = {
                "arguments": subject.validator_arguments("baseline", "compute", "CSMain"),
                "exit_code": 0, "stdout_sha256": base.digest_bytes(output), "stdout_bytes": len(output),
                "stderr_sha256": subject.EMPTY_SHA256, "stderr_bytes": 0,
            }
            result = subject.validator_success_record("baseline", "compute", "CSMain", 20)
            reason = None
        return {
            **row, "immediate_witness": profile_derivation["immediate_witness"], "selected_profile": selected_profile,
            "scalar_layout_witness": None, "sampled_buffer_witness": None, "fragment_interface_witness": None,
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
                "arguments": validator["arguments"], "profiles": validator["identity"]["profiles"],
                "profile_derivation": self.policy["profile_derivation"],
                "controlled_environment": validator["controlled_environment"],
                "outcome_order": self.policy["outcomes"]["order"],
            },
            "entries": entries, "outcome_counts": {"ingestible": 55, "blocked-known": 1},
            "profile_counts": self.policy["profile_derivation"]["expected_corpus_profile_counts"],
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

    def test_valid_sampled_buffer_blocked_receipt(self) -> None:
        receipt = self.make_receipt()
        row = receipt["entries"][0]
        sampled_buffer = self.policy["outcomes"]["blocked_known_sampled_buffer"]
        row["reason_code"] = sampled_buffer["reason_code"]
        row["sampled_buffer_witness"] = subject.sampled_buffer_witness(sampled_buffer_fixture(), self.policy)
        row["validation"].update({
            "exit_code": 2, "stdout_sha256": subject.EMPTY_SHA256, "stdout_bytes": 0,
            "stderr_sha256": base.digest_bytes(subject.SAMPLED_BUFFER_STDERR),
            "stderr_bytes": len(subject.SAMPLED_BUFFER_STDERR),
        })
        receipt["assessment_set_sha256"] = base.digest_bytes(base.canonical_json(receipt["entries"]))
        subject.validate_assessment_receipt(self.rehash(receipt), self.policy)

    def test_valid_fragment_interface_blocked_receipt(self) -> None:
        receipt = self.make_receipt()
        row = receipt["entries"][0]
        row["stage"], row["entry"] = "fragment", "PSMain"
        row["validation"]["arguments"] = subject.validator_arguments("baseline", "fragment", "PSMain")
        fragment_interface = self.policy["outcomes"]["blocked_known_fragment_direct_blend_src_index_output"]
        row["reason_code"] = fragment_interface["reason_code"]
        row["fragment_interface_witness"] = subject.fragment_interface_witness(fragment_interface_fixture(), "fragment", "PSMain", self.policy)
        row["validation"].update({
            "exit_code": 2, "stdout_sha256": subject.EMPTY_SHA256, "stdout_bytes": 0,
            "stderr_sha256": base.digest_bytes(subject.FRAGMENT_INTERFACE_STDERR),
            "stderr_bytes": len(subject.FRAGMENT_INTERFACE_STDERR),
        })
        receipt["assessment_set_sha256"] = base.digest_bytes(base.canonical_json(receipt["entries"]))
        subject.validate_assessment_receipt(self.rehash(receipt), self.policy)

    def test_receipt_rejects_sampled_buffer_row_missing_witness(self) -> None:
        receipt = self.make_receipt()
        row = receipt["entries"][0]
        sampled_buffer = self.policy["outcomes"]["blocked_known_sampled_buffer"]
        row["reason_code"] = sampled_buffer["reason_code"]
        row["validation"].update({
            "exit_code": 2, "stdout_sha256": subject.EMPTY_SHA256, "stdout_bytes": 0,
            "stderr_sha256": base.digest_bytes(subject.SAMPLED_BUFFER_STDERR),
            "stderr_bytes": len(subject.SAMPLED_BUFFER_STDERR),
        })
        receipt["assessment_set_sha256"] = base.digest_bytes(base.canonical_json(receipt["entries"]))
        with self.assertRaisesRegex(base.ArtifactError, "lacks its exact witness"):
            subject.validate_assessment_receipt(self.rehash(receipt), self.policy)

    def test_receipt_rejects_fragment_interface_row_missing_witness(self) -> None:
        receipt = self.make_receipt()
        row = receipt["entries"][0]
        row["stage"], row["entry"] = "fragment", "PSMain"
        row["validation"]["arguments"] = subject.validator_arguments("baseline", "fragment", "PSMain")
        fragment_interface = self.policy["outcomes"]["blocked_known_fragment_direct_blend_src_index_output"]
        row["reason_code"] = fragment_interface["reason_code"]
        row["validation"].update({
            "exit_code": 2, "stdout_sha256": subject.EMPTY_SHA256, "stdout_bytes": 0,
            "stderr_sha256": base.digest_bytes(subject.FRAGMENT_INTERFACE_STDERR),
            "stderr_bytes": len(subject.FRAGMENT_INTERFACE_STDERR),
        })
        receipt["assessment_set_sha256"] = base.digest_bytes(base.canonical_json(receipt["entries"]))
        with self.assertRaisesRegex(base.ArtifactError, "lacks its exact witness"):
            subject.validate_assessment_receipt(self.rehash(receipt), self.policy)

    def test_receipt_rejects_wrong_witness_attached_to_reason(self) -> None:
        receipt = self.make_receipt()
        row = receipt["entries"][0]
        sampled_buffer = self.policy["outcomes"]["blocked_known_sampled_buffer"]
        row["reason_code"] = sampled_buffer["reason_code"]
        row["scalar_layout_witness"] = subject.scalar_layout_witness(spirv_fixture(), self.policy)
        row["validation"].update({
            "exit_code": 2, "stdout_sha256": subject.EMPTY_SHA256, "stdout_bytes": 0,
            "stderr_sha256": base.digest_bytes(subject.SAMPLED_BUFFER_STDERR),
            "stderr_bytes": len(subject.SAMPLED_BUFFER_STDERR),
        })
        receipt["assessment_set_sha256"] = base.digest_bytes(base.canonical_json(receipt["entries"]))
        with self.assertRaisesRegex(base.ArtifactError, "lacks its exact witness"):
            subject.validate_assessment_receipt(self.rehash(receipt), self.policy)

    def test_receipt_rejects_shader_nonuniform_row_retaining_stale_scalar_witness(self) -> None:
        # Direct regression for the v5 receipt bug: a ShaderNonUniform row must not
        # retain a scalar-layout witness even though it derives cleanly from the bytes.
        receipt = self.make_receipt()
        row = receipt["entries"][0]
        row["scalar_layout_witness"] = subject.scalar_layout_witness(spirv_fixture(), self.policy)
        receipt["assessment_set_sha256"] = base.digest_bytes(base.canonical_json(receipt["entries"]))
        with self.assertRaisesRegex(base.ArtifactError, "retains a nonselected witness"):
            subject.validate_assessment_receipt(self.rehash(receipt), self.policy)

    def test_receipt_rejects_shader_nonuniform_row_retaining_stale_fragment_witness(self) -> None:
        receipt = self.make_receipt()
        row = receipt["entries"][0]
        row["stage"], row["entry"] = "fragment", "PSMain"
        row["validation"]["arguments"] = subject.validator_arguments("baseline", "fragment", "PSMain")
        row["fragment_interface_witness"] = subject.fragment_interface_witness(fragment_interface_fixture(), "fragment", "PSMain", self.policy)
        receipt["assessment_set_sha256"] = base.digest_bytes(base.canonical_json(receipt["entries"]))
        with self.assertRaisesRegex(base.ArtifactError, "retains a nonselected witness"):
            subject.validate_assessment_receipt(self.rehash(receipt), self.policy)

    def test_receipt_rejects_shader_nonuniform_row_retaining_both_stale_witnesses(self) -> None:
        # Exact receipt-level shape of the six real v5 ShaderNonUniform rows: both a
        # stale scalar_layout_witness and a stale fragment_interface_witness present.
        receipt = self.make_receipt()
        row = receipt["entries"][0]
        row["stage"], row["entry"] = "fragment", "PSMain"
        row["validation"]["arguments"] = subject.validator_arguments("baseline", "fragment", "PSMain")
        module = combined_scalar_and_fragment_fixture()
        row["scalar_layout_witness"] = subject.scalar_layout_witness(module, self.policy)
        row["fragment_interface_witness"] = subject.fragment_interface_witness(module, "fragment", "PSMain", self.policy)
        receipt["assessment_set_sha256"] = base.digest_bytes(base.canonical_json(receipt["entries"]))
        with self.assertRaisesRegex(base.ArtifactError, "more than one matching witness"):
            subject.validate_assessment_receipt(self.rehash(receipt), self.policy)

    def test_receipt_rejects_scalar_layout_row_with_two_matching_witnesses(self) -> None:
        receipt = self.make_receipt()
        row = receipt["entries"][0]
        row["stage"], row["entry"] = "fragment", "PSMain"
        row["validation"]["arguments"] = subject.validator_arguments("baseline", "fragment", "PSMain")
        scalar = self.policy["outcomes"]["blocked_known_scalar_layout"]
        row["reason_code"] = scalar["reason_code"]
        module = combined_scalar_and_fragment_fixture()
        row["scalar_layout_witness"] = subject.scalar_layout_witness(module, self.policy)
        row["fragment_interface_witness"] = subject.fragment_interface_witness(module, "fragment", "PSMain", self.policy)
        row["validation"].update({
            "exit_code": 2, "stdout_sha256": subject.EMPTY_SHA256, "stdout_bytes": 0,
            "stderr_sha256": base.digest_bytes(subject.SCALAR_LAYOUT_STDERR),
            "stderr_bytes": len(subject.SCALAR_LAYOUT_STDERR),
        })
        receipt["assessment_set_sha256"] = base.digest_bytes(base.canonical_json(receipt["entries"]))
        with self.assertRaisesRegex(base.ArtifactError, "more than one matching witness"):
            subject.validate_assessment_receipt(self.rehash(receipt), self.policy)

    def test_receipt_rejects_blocked_known_row_with_three_matching_witnesses(self) -> None:
        receipt = self.make_receipt()
        row = receipt["entries"][0]
        row["stage"], row["entry"] = "fragment", "PSMain"
        row["validation"]["arguments"] = subject.validator_arguments("baseline", "fragment", "PSMain")
        sampled_buffer = self.policy["outcomes"]["blocked_known_sampled_buffer"]
        row["reason_code"] = sampled_buffer["reason_code"]
        module = combined_scalar_and_fragment_fixture()
        row["scalar_layout_witness"] = subject.scalar_layout_witness(module, self.policy)
        row["fragment_interface_witness"] = subject.fragment_interface_witness(module, "fragment", "PSMain", self.policy)
        row["sampled_buffer_witness"] = subject.sampled_buffer_witness(sampled_buffer_fixture(), self.policy)
        row["validation"].update({
            "exit_code": 2, "stdout_sha256": subject.EMPTY_SHA256, "stdout_bytes": 0,
            "stderr_sha256": base.digest_bytes(subject.SAMPLED_BUFFER_STDERR),
            "stderr_bytes": len(subject.SAMPLED_BUFFER_STDERR),
        })
        receipt["assessment_set_sha256"] = base.digest_bytes(base.canonical_json(receipt["entries"]))
        with self.assertRaisesRegex(base.ArtifactError, "more than one matching witness"):
            subject.validate_assessment_receipt(self.rehash(receipt), self.policy)

    def test_receipt_rejects_scalar_layout_row_reason_mismatched_against_fragment_witness(self) -> None:
        # reason_code says scalar-layout but the only witness attached is fragment-interface.
        receipt = self.make_receipt()
        row = receipt["entries"][0]
        row["stage"], row["entry"] = "fragment", "PSMain"
        row["validation"]["arguments"] = subject.validator_arguments("baseline", "fragment", "PSMain")
        scalar = self.policy["outcomes"]["blocked_known_scalar_layout"]
        row["reason_code"] = scalar["reason_code"]
        row["fragment_interface_witness"] = subject.fragment_interface_witness(fragment_interface_fixture(), "fragment", "PSMain", self.policy)
        row["validation"].update({
            "exit_code": 2, "stdout_sha256": subject.EMPTY_SHA256, "stdout_bytes": 0,
            "stderr_sha256": base.digest_bytes(subject.SCALAR_LAYOUT_STDERR),
            "stderr_bytes": len(subject.SCALAR_LAYOUT_STDERR),
        })
        receipt["assessment_set_sha256"] = base.digest_bytes(base.canonical_json(receipt["entries"]))
        with self.assertRaisesRegex(base.ArtifactError, "lacks its exact witness"):
            subject.validate_assessment_receipt(self.rehash(receipt), self.policy)

    def test_receipt_rejects_pass_shaped_blocked_known_row(self) -> None:
        receipt = self.make_receipt()
        row = receipt["entries"][0]
        sampled_buffer = self.policy["outcomes"]["blocked_known_sampled_buffer"]
        row["reason_code"] = sampled_buffer["reason_code"]
        row["sampled_buffer_witness"] = subject.sampled_buffer_witness(sampled_buffer_fixture(), self.policy)
        row["validation"].update({
            "exit_code": 0, "stdout_sha256": base.digest_bytes(success_bytes(row)), "stdout_bytes": len(success_bytes(row)),
            "stderr_sha256": subject.EMPTY_SHA256, "stderr_bytes": 0,
        })
        receipt["assessment_set_sha256"] = base.digest_bytes(base.canonical_json(receipt["entries"]))
        with self.assertRaises(base.ArtifactError):
            subject.validate_assessment_receipt(self.rehash(receipt), self.policy)

    def test_valid_immediate_profile_receipt(self) -> None:
        receipt = self.make_receipt()
        row = receipt["entries"][1]
        derived = subject.derive_validator_profile(push_constant_fixture(("f32",) * 5, (0, 4, 8, 12, 16)), self.policy)
        row["immediate_witness"] = derived["immediate_witness"]
        row["selected_profile"] = derived["profile"]
        row["validation"]["arguments"] = subject.validator_arguments("immediates-20", "compute", "CSMain")
        output = success_bytes(row, profile="immediates-20")
        row["validation"].update({"stdout_sha256": base.digest_bytes(output), "stdout_bytes": len(output)})
        row["validation_record"] = subject.validator_success_record("immediates-20", "compute", "CSMain", 20)
        self.policy["profile_derivation"]["expected_corpus_profile_counts"].update({"baseline": 55, "immediates-20": 1})
        receipt["profile_counts"] = self.policy["profile_derivation"]["expected_corpus_profile_counts"]
        receipt["assessment_contract"]["profile_derivation"] = self.policy["profile_derivation"]
        receipt["assessment_set_sha256"] = base.digest_bytes(base.canonical_json(receipt["entries"]))
        subject.validate_assessment_receipt(self.rehash(receipt), self.policy)

    def test_receipt_rejects_selected_profile_witness_mismatch(self) -> None:
        receipt = self.make_receipt()
        receipt["entries"][1]["selected_profile"] = subject.validator_profile("immediates-4")
        receipt["assessment_set_sha256"] = base.digest_bytes(base.canonical_json(receipt["entries"]))
        with self.assertRaisesRegex(base.ArtifactError, "profile changed"):
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


# The exact six accepted M2.5a rows that were ShaderNonUniform-blocked in the
# real (buggy) v5 receipt. Each retained a stale scalar_layout_witness and
# fragment_interface_witness in that receipt, which this repair removes.
SIX_ACTUAL_SHADER_NONUNIFORM_ROW_IDS = (
    "src-shaders-rasterpsdynamic",
    "src-shaders-rasterpsdynamicms",
    "src-shaders-rasterpsspecconstant",
    "src-shaders-rasterpsspecconstantms",
    "src-shaders-rasterpsspecconstantflat",
    "src-shaders-rasterpsspecconstantflatms",
)


class SixActualShaderNonUniformRowsTests(unittest.TestCase):
    """Regression coverage for the v5 receipt bug using the exact six accepted
    M2.5a row identities that exhibited it, not a synthetic stand-in. The
    denominator stays 56 rows (the six real rows plus 50 ingestible filler
    rows) so the policy's fixed row_count/profile-count invariants still hold."""

    FILLER_COUNT = 50

    def setUp(self) -> None:
        self.policy = copy.deepcopy(subject.load_policy())
        all_ids = list(SIX_ACTUAL_SHADER_NONUNIFORM_ROW_IDS) + [f"filler-{index}" for index in range(self.FILLER_COUNT)]
        self.policy["reference_corpus"]["entry_order_sha256"] = base.digest_bytes(base.canonical_json(all_ids))
        self.policy["profile_derivation"]["expected_corpus_profile_counts"] = {
            name: 56 if name == "baseline" else 0 for name in subject.PROFILE_NAMES
        }

    def blocked_entry(self, row_id: str) -> dict:
        row = {
            "id": row_id, "source": "src/shaders/RasterPS.hlsl", "stage": "fragment",
            "entry": "PSMain", "spirv_artifact": f"spirv/{row_id}.spv",
            "spirv_sha256": "a" * 64, "spirv_bytes": 20,
            "semantic_inventory": inventory(True),
        }
        profile_derivation = subject.derive_validator_profile(EMPTY_SPIRV, self.policy)
        return {
            **row, "immediate_witness": profile_derivation["immediate_witness"], "selected_profile": profile_derivation["profile"],
            "scalar_layout_witness": None, "sampled_buffer_witness": None, "fragment_interface_witness": None,
            "outcome": "blocked-known", "reason_code": self.policy["outcomes"]["blocked_known_shader_nonuniform"]["reason_code"],
            "validation": {
                "arguments": subject.validator_arguments("baseline", "fragment", "PSMain"),
                "exit_code": 2, "stdout_sha256": subject.EMPTY_SHA256, "stdout_bytes": 0,
                "stderr_sha256": base.digest_bytes(subject.KNOWN_STDERR), "stderr_bytes": len(subject.KNOWN_STDERR),
            },
            "validation_record": None,
        }

    def filler_entry(self, row_id: str) -> dict:
        row = reference_row(False, 0)
        row["id"] = row_id
        row["spirv_artifact"] = f"spirv/{row_id}.spv"
        profile_derivation = subject.derive_validator_profile(EMPTY_SPIRV, self.policy)
        output = success_bytes(row)
        return {
            **row, "immediate_witness": profile_derivation["immediate_witness"], "selected_profile": profile_derivation["profile"],
            "scalar_layout_witness": None, "sampled_buffer_witness": None, "fragment_interface_witness": None,
            "outcome": "ingestible", "reason_code": None,
            "validation": {
                "arguments": subject.validator_arguments("baseline", "compute", "CSMain"),
                "exit_code": 0, "stdout_sha256": base.digest_bytes(output), "stdout_bytes": len(output),
                "stderr_sha256": subject.EMPTY_SHA256, "stderr_bytes": 0,
            },
            "validation_record": subject.validator_success_record("baseline", "compute", "CSMain", 20),
        }

    def all_entries(self) -> list[dict]:
        blocked = [self.blocked_entry(row_id) for row_id in SIX_ACTUAL_SHADER_NONUNIFORM_ROW_IDS]
        filler = [self.filler_entry(f"filler-{index}") for index in range(self.FILLER_COUNT)]
        return blocked + filler

    def make_receipt(self, entries: list[dict]) -> dict:
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
                "arguments": validator["arguments"], "profiles": validator["identity"]["profiles"],
                "profile_derivation": self.policy["profile_derivation"],
                "controlled_environment": validator["controlled_environment"],
                "outcome_order": self.policy["outcomes"]["order"],
            },
            "entries": entries, "outcome_counts": {"ingestible": self.FILLER_COUNT, "blocked-known": 6},
            "profile_counts": self.policy["profile_derivation"]["expected_corpus_profile_counts"],
            "assessment_set_sha256": base.digest_bytes(base.canonical_json(entries)),
            "runtime_readiness": subject.runtime_readiness(entries, self.policy),
            "claim_boundary": self.policy["claim_boundary"],
        })

    def test_all_six_rows_pass_with_no_witnesses(self) -> None:
        subject.validate_assessment_receipt(self.make_receipt(self.all_entries()), self.policy)

    def test_all_six_rows_fail_with_the_v5_stale_witness_shape(self) -> None:
        # Reproduces the exact v5 defect: every one of the six rows carries both a
        # stale scalar_layout_witness and a stale fragment_interface_witness.
        module = combined_scalar_and_fragment_fixture()
        stale_scalar = subject.scalar_layout_witness(module, self.policy)
        stale_fragment = subject.fragment_interface_witness(module, "fragment", "PSMain", self.policy)
        entries = self.all_entries()
        for entry in entries:
            if entry["id"] in SIX_ACTUAL_SHADER_NONUNIFORM_ROW_IDS:
                entry["scalar_layout_witness"] = stale_scalar
                entry["fragment_interface_witness"] = stale_fragment
        receipt = self.make_receipt(entries)
        with self.assertRaisesRegex(base.ArtifactError, "more than one matching witness"):
            subject.validate_assessment_receipt(receipt, self.policy)

    def test_each_individual_row_rejects_its_own_stale_witness_pair(self) -> None:
        module = combined_scalar_and_fragment_fixture()
        stale_scalar = subject.scalar_layout_witness(module, self.policy)
        stale_fragment = subject.fragment_interface_witness(module, "fragment", "PSMain", self.policy)
        for target in SIX_ACTUAL_SHADER_NONUNIFORM_ROW_IDS:
            with self.subTest(row_id=target):
                entries = self.all_entries()
                row = next(entry for entry in entries if entry["id"] == target)
                row["scalar_layout_witness"] = stale_scalar
                row["fragment_interface_witness"] = stale_fragment
                receipt = self.make_receipt(entries)
                with self.assertRaisesRegex(base.ArtifactError, "more than one matching witness"):
                    subject.validate_assessment_receipt(receipt, self.policy)


class HistoricalCorpusAuthenticationTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.policy = copy.deepcopy(subject.load_policy())
        self.policy["reference_corpus"]["row_count"] = 1
        self.policy["reference_corpus"]["file_count"] = 5
        self.files = {
            "dependency_output_artifact": ("dependency/a.d", b"d"),
            "dependency_manifest_artifact": ("dependency/a.json", b"m"),
            "preprocessed_artifact": ("preprocessed/a.hlsl", b"p"),
            "spirv_artifact": ("spirv/a.spv", b"s"),
        }
        row = {
            "id": "shader-0", "source": "src/shader0.hlsl", "stage": "compute", "entry": "CSMain",
            "semantic_inventory": inventory(False),
        }
        keys = {
            "dependency_output_artifact": ("dependency_output_sha256", "dependency_output_bytes"),
            "dependency_manifest_artifact": ("dependency_manifest_sha256", "dependency_manifest_bytes"),
            "preprocessed_artifact": ("preprocessed_sha256", "preprocessed_bytes"),
            "spirv_artifact": ("spirv_sha256", "spirv_bytes"),
        }
        for path_key, (relative, data) in self.files.items():
            path = self.root / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
            digest_key, length_key = keys[path_key]
            row.update({path_key: relative, digest_key: base.digest_bytes(data), length_key: len(data)})
        artifact_set = [{"path": row["spirv_artifact"], "sha256": row["spirv_sha256"]}]
        ref = self.policy["reference_corpus"]
        receipt = base.add_receipt_hash({
            "schema": ref["receipt_schema"], "status": "complete",
            "orchestration_producer_sha256": ref["orchestration_producer_sha256"],
            "artifact_producer_sha256": ref["artifact_producer_sha256"],
            "reference_policy_sha256": ref["reference_policy_sha256"],
            "artifact_policy_sha256": ref["artifact_policy_sha256"],
            "denominator_sha256": ref["denominator_sha256"],
            "source_snapshot": {"source_set_sha256": ref["source_snapshot_set_sha256"]},
            "dxc_build_receipt_sha256": ref["dxc_build_receipt_sha256"],
            "dxc_compiler_sha256": ref["dxc_compiler_sha256"],
            "spirv_val_build_receipt_sha256": ref["spirv_val_build_receipt_sha256"],
            "spirv_grammar": {"sha256": ref["spirv_grammar_sha256"]},
            "entries": [row],
            "artifact_set_sha256": base.digest_bytes(base.canonical_json(artifact_set)),
        })
        self.receipt = receipt
        self.receipt_path = self.root / subject.reference.RECEIPT_PATH
        self.receipt_path.write_bytes(base.pretty_json(receipt))
        self.repin()
        self.args = argparse.Namespace(reference_artifact_dir=str(self.root))

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def repin(self, *, pretty: bool = True) -> None:
        self.receipt = base.add_receipt_hash(self.receipt)
        encoded = base.pretty_json(self.receipt) if pretty else base.canonical_json(self.receipt)
        self.receipt_path.write_bytes(encoded)
        ref = self.policy["reference_corpus"]
        ref["receipt_sha256"] = self.receipt["receipt_sha256"]
        ref["receipt_file_sha256"] = base.digest_bytes(encoded)
        ref["artifact_set_sha256"] = self.receipt["artifact_set_sha256"]
        ref["entry_order_sha256"] = base.digest_bytes(base.canonical_json([row["id"] for row in self.receipt["entries"]]))

    def verify(self) -> dict:
        return subject.verify_reference_inputs(self.args, self.policy)

    def test_accepts_exact_historical_corpus_without_current_producers(self) -> None:
        self.assertEqual(self.verify(), self.receipt)

    def test_rejects_non_pretty_receipt_even_when_file_digest_is_rebound(self) -> None:
        self.repin(pretty=False)
        with self.assertRaisesRegex(base.ArtifactError, "not exact pretty JSON"):
            self.verify()

    def test_rejects_extra_file(self) -> None:
        (self.root / "extra").write_bytes(b"x")
        with self.assertRaisesRegex(base.ArtifactError, "file denominator"):
            self.verify()

    def test_rejects_artifact_hardlink(self) -> None:
        target = self.root / self.files["spirv_artifact"][0]
        target.unlink()
        os.link(self.root / self.files["preprocessed_artifact"][0], target)
        with self.assertRaisesRegex(base.ArtifactError, "linked or reused"):
            self.verify()

    def test_rejects_artifact_symlink(self) -> None:
        target = self.root / self.files["spirv_artifact"][0]
        target.unlink()
        target.symlink_to(self.root / self.files["preprocessed_artifact"][0])
        with self.assertRaises(base.ArtifactError):
            self.verify()

    def test_rejects_traversal_even_when_receipt_is_rebound(self) -> None:
        self.receipt["entries"][0]["spirv_artifact"] = "../escape.spv"
        self.receipt["artifact_set_sha256"] = base.digest_bytes(base.canonical_json([{
            "path": "../escape.spv", "sha256": self.receipt["entries"][0]["spirv_sha256"],
        }]))
        self.repin()
        with self.assertRaisesRegex(base.ArtifactError, "unsafe"):
            self.verify()

    def test_rejects_rebound_length_or_digest_lie(self) -> None:
        for key in ("spirv_bytes", "spirv_sha256"):
            with self.subTest(key=key):
                original = self.receipt["entries"][0][key]
                self.receipt["entries"][0][key] = original + 1 if key.endswith("bytes") else "0" * 64
                if key == "spirv_sha256":
                    self.receipt["artifact_set_sha256"] = base.digest_bytes(base.canonical_json([{
                        "path": self.receipt["entries"][0]["spirv_artifact"], "sha256": "0" * 64,
                    }]))
                self.repin()
                with self.assertRaisesRegex(base.ArtifactError, "length changed|digest changed"):
                    self.verify()
                self.receipt["entries"][0][key] = original
                if key == "spirv_sha256":
                    self.receipt["artifact_set_sha256"] = base.digest_bytes(base.canonical_json([{
                        "path": self.receipt["entries"][0]["spirv_artifact"], "sha256": original,
                    }]))


class DiagnosticCensusTests(unittest.TestCase):
    def setUp(self) -> None:
        self.policy = subject.load_policy()

    def test_collects_all_rows_past_multiple_unknown_outcomes_in_order(self) -> None:
        rows = [{"id": f"shader-{index}"} for index in range(56)]
        called = []
        policy = copy.deepcopy(self.policy)
        policy["profile_derivation"]["expected_corpus_profile_counts"] = {
            name: 56 if name == "baseline" else 0 for name in subject.PROFILE_NAMES
        }

        def runner(row: dict) -> dict:
            called.append(row["id"])
            index = int(row["id"].split("-")[1])
            exit_code = {7: 17, 11: 23}.get(index, 0)
            return {
                "id": row["id"],
                "selected_profile": subject.validator_profile("baseline"),
                "immediate_witness": None,
                "validation": {
                    "exit_code": exit_code,
                    "stdout_bytes": 1,
                    "stderr_bytes": 2 if exit_code else 0,
                },
            }

        entries, totals = subject.collect_diagnostic_rows(rows, runner, policy)
        self.assertEqual(called, [row["id"] for row in rows])
        self.assertEqual([row["id"] for row in entries], called)
        self.assertEqual(totals["rows"], 56)
        self.assertEqual(totals["exit_code_counts"], {"0": 54, "17": 1, "23": 1})

    def test_collect_rejects_row_and_total_cap_drift(self) -> None:
        rows = [{"id": f"shader-{index}"} for index in range(56)]
        policy = copy.deepcopy(self.policy)
        policy["profile_derivation"]["expected_corpus_profile_counts"] = {
            name: 56 if name == "baseline" else 0 for name in subject.PROFILE_NAMES
        }

        def oversized(row: dict) -> dict:
            return {
                "id": row["id"], "selected_profile": subject.validator_profile("baseline"),
                "immediate_witness": None,
                "validation": {"exit_code": 1, "stdout_bytes": 4096, "stderr_bytes": 4097},
            }

        with self.assertRaisesRegex(base.ArtifactError, "stream cap"):
            subject.collect_diagnostic_rows(rows, oversized, policy)
        constrained = copy.deepcopy(policy)
        constrained["diagnostic_census"]["maximum_total_output_bytes"] = 55

        def one_byte(row: dict) -> dict:
            return {
                "id": row["id"], "selected_profile": subject.validator_profile("baseline"),
                "immediate_witness": None,
                "validation": {"exit_code": 1, "stdout_bytes": 1, "stderr_bytes": 0},
            }

        with self.assertRaisesRegex(base.ArtifactError, "total output cap"):
            subject.collect_diagnostic_rows(rows, one_byte, constrained)

    def test_bounded_text_rejects_success_binary_paths_and_oversize(self) -> None:
        self.assertIsNone(subject.diagnostic_text(b"ok\n", 0, self.policy))
        self.assertIsNone(subject.diagnostic_text(b"\xff", 2, self.policy))
        self.assertIsNone(subject.diagnostic_text(b"failed at /private/tmp/input.spv\n", 2, self.policy))
        self.assertIsNone(subject.diagnostic_text(b"x" * 1025, 2, self.policy))
        self.assertEqual(subject.diagnostic_text(b"bounded diagnostic\n", 2, self.policy), "bounded diagnostic\n")

    def test_census_schema_cannot_validate_as_assessment_receipt(self) -> None:
        census = {
            "schema": subject.DIAGNOSTIC_CENSUS_SCHEMA,
            "authority": "non-authoritative-diagnostic-only",
            "runtime_ready": False,
        }
        with self.assertRaises(base.ArtifactError):
            subject.validate_assessment_receipt(census, self.policy)


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
