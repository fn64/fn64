#!/usr/bin/env python3

from __future__ import annotations

import copy
import argparse
from contextlib import contextmanager
import json
import os
import stat
import struct
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))
import rt64_reference_shader_artifacts as reference
import rt64_shader_artifacts as base


def string_words(value: str) -> list[int]:
    raw = value.encode() + b"\0"
    raw += b"\0" * (-len(raw) % 4)
    return [int.from_bytes(raw[index : index + 4], "little") for index in range(0, len(raw), 4)]


def instruction(opcode: int, *operands: int) -> list[int]:
    return [((len(operands) + 1) << 16) | opcode, *operands]


def module(*instructions: list[int]) -> bytes:
    words = [0x07230203, 0x00010000, 0, 100, 0]
    for row in instructions:
        words.extend(row)
    return struct.pack(f"<{len(words)}I", *words)


def grammar_bytes() -> bytes:
    return json.dumps(
        {
            "instructions": [
                {"opname": "OpCapability", "opcode": 17},
                {"opname": "OpExtension", "opcode": 10},
                {"opname": "OpDecorate", "opcode": 71},
                {"opname": "OpMemberDecorate", "opcode": 72},
                {"opname": "OpDecorationGroup", "opcode": 73},
                {"opname": "OpGroupDecorate", "opcode": 74},
                {"opname": "OpGroupMemberDecorate", "opcode": 75},
            ],
            "operand_kinds": [
                {
                    "kind": "Capability",
                    "category": "ValueEnum",
                    "enumerants": [
                        {"enumerant": "Shader", "value": 1},
                        {"enumerant": "ShaderNonUniform", "value": 5301},
                    ],
                },
                {
                    "kind": "Decoration",
                    "category": "ValueEnum",
                    "enumerants": [
                        {"enumerant": "RelaxedPrecision", "value": 0},
                        {"enumerant": "NonUniform", "value": 5300},
                    ],
                },
            ],
        },
        separators=(",", ":"),
    ).encode()


class PolicyTests(unittest.TestCase):
    def test_policy_binds_accepted_artifact_producer_without_editing_it(self) -> None:
        policy = reference.load_policy()
        self.assertEqual(policy["artifact_producer"]["sha256"], base.digest_file(base.TOOL_PATH))
        self.assertEqual(
            policy["artifact_producer"]["sha256"],
            "b8db5cbcaa0caef60ec3c84a966917d246e25df882ae44951541e65720630d33",
        )

    def test_policy_names_only_additive_direct_consumers(self) -> None:
        self.assertEqual(
            reference.load_policy()["direct_consumers"],
            [
                "tools/rt64_reference_shader_artifacts.py",
                "tools/test_rt64_reference_shader_artifacts.py",
            ],
        )

    def test_policy_binds_all_build_grammars_and_registry(self) -> None:
        dxc = reference.load_policy()["dxc"]
        self.assertEqual(len(dxc["grammar_files"]), 16)
        self.assertEqual(len({row["path"] for row in dxc["grammar_files"]}), 16)
        self.assertIn(dxc["inventory_grammar_path"], {row["path"] for row in dxc["grammar_files"]})
        self.assertEqual(dxc["registry_file"]["sha256"], "204a1c88c736a8d41a7e8249e46dec8384c06ec34be23f223641eef71963501d")

    def test_validation_argv_is_exact_scalar_layout_and_has_one_authority(self) -> None:
        policy = reference.load_policy()
        self.assertNotIn("validation_arguments", policy["spirv_val"])
        value = policy["device_contract"]["validator_argv"]
        self.assertEqual(value, ["--target-env", "vulkan1.0", "--scalar-block-layout", "-"])
        self.assertFalse(any("relax" in item or "skip" in item for item in value))

    def test_typed_scalar_layout_device_contract_is_exact(self) -> None:
        policy = reference.load_policy()
        contract = reference.validated_device_contract(policy)
        self.assertEqual(contract["schema"], "fn64.vulkan-scalar-block-layout-device-contract.v1")
        self.assertEqual(contract["required_extensions"], ["VK_EXT_scalar_block_layout"])
        self.assertEqual(contract["required_features"], [{"name": "scalarBlockLayout", "value": True}])
        self.assertEqual(contract["validator_mode"], "vulkan1.0-scalar-block-layout")
        self.assertIs(contract["required_features"][0]["value"], True)

    def test_device_contract_receipt_identity_and_schema_fail_closed(self) -> None:
        policy = reference.load_policy()
        record = reference.validated_device_contract(policy)
        reference.validate_receipt_device_contract(record, policy, "fixture")
        mutations = []
        for key, value in (
            ("schema", "fn64.vulkan-scalar-block-layout-device-contract.v2"),
            ("required_extensions", []),
            ("required_features", [{"name": "scalarBlockLayout", "value": False}]),
            ("validator_mode", "relaxed-block-layout"),
        ):
            mutation = copy.deepcopy(record)
            mutation[key] = value
            mutations.append((key, mutation))
        for label, mutation in mutations:
            with self.subTest(label=label), self.assertRaisesRegex(base.ArtifactError, "device contract changed"):
                reference.validate_receipt_device_contract(mutation, policy, "fixture")

    def test_build_configuration_excludes_shared_tests_fuzzers_and_mimalloc(self) -> None:
        flags = set(reference.load_policy()["spirv_val"]["flags"])
        for flag in (
            "-DBUILD_SHARED_LIBS=OFF",
            "-DSPIRV_SKIP_EXECUTABLES=OFF",
            "-DSPIRV_SKIP_TESTS=ON",
            "-DSPIRV_TOOLS_USE_MIMALLOC=OFF",
            "-DSPIRV_TOOLS_USE_MIMALLOC_IN_STATIC_BUILD=OFF",
            "-DSPIRV_BUILD_FUZZER=OFF",
            "-DSPIRV_BUILD_LIBFUZZER_TARGETS=OFF",
            "-DSPIRV_ALLOW_TIMERS=OFF",
        ):
            self.assertIn(flag, flags)

    def test_generated_authority_policy_paths_are_exact_and_hostile_to_drift(self) -> None:
        expected = [f"build/{name}" for name in reference.GENERATED_AUTHORITY_NAMES]
        self.assertEqual(
            [path.as_posix() for path in reference.generated_authority_paths(reference.load_policy())],
            expected,
        )
        original_load_json = base.load_json
        original = original_load_json(reference.POLICY_PATH)
        mutations = []
        added = copy.deepcopy(original)
        added["spirv_val"]["generated_authority_files"].append("build/extra.inc")
        mutations.append(("add", added))
        dropped = copy.deepcopy(original)
        dropped["spirv_val"]["generated_authority_files"].pop()
        mutations.append(("drop", dropped))
        reordered = copy.deepcopy(original)
        reordered["spirv_val"]["generated_authority_files"][0:2] = reversed(
            reordered["spirv_val"]["generated_authority_files"][0:2]
        )
        mutations.append(("reorder", reordered))
        renamed = copy.deepcopy(original)
        renamed["spirv_val"]["generated_authority_files"][0] = "build/renamed.inc"
        mutations.append(("rename", renamed))
        old_layout = copy.deepcopy(original)
        old_layout["spirv_val"]["generated_authority_files"] = [
            value.replace("build/", "build/source/", 1) for value in expected
        ]
        mutations.append(("old-layout", old_layout))
        for label, mutation in mutations:
            with self.subTest(label=label), mock.patch.object(
                base,
                "load_json",
                side_effect=lambda path, value=mutation: value
                if path == reference.POLICY_PATH
                else original_load_json(path),
            ), self.assertRaisesRegex(base.ArtifactError, "authority policy denominator changed"):
                reference.load_policy()

    def test_claim_boundary_never_mentions_runtime_or_parity_success(self) -> None:
        policy = reference.load_policy()
        self.assertEqual(policy["schema"], "fn64.rt64-reference-shader-policy.v2")
        self.assertEqual(policy["receipt_schema"], "fn64.rt64-reference-shader-receipt.v2")
        self.assertEqual(policy["spirv_val_smoke_receipt_schema"], "fn64.spirv-val-single-artifact-smoke.v2")
        self.assertEqual(
            policy["claim_boundary"],
            "conditionally-reference-valid-with-scalar-layout-contract-not-adapter-wgpu-pipeline-runtime-parity-or-performance",
        )
        self.assertEqual(
            policy["spirv_val_smoke_claim_boundary"],
            "conditionally-reference-valid-with-scalar-layout-contract-and-inventoried-not-artifact-provenance-corpus-adapter-wgpu-pipeline-runtime-parity-or-performance",
        )

    def test_parser_exposes_additive_commands_only(self) -> None:
        parser = reference.parser()
        self.assertEqual(parser.parse_args(["selftest"]).command, "selftest")
        smoke = parser.parse_args(
            [
                "smoke-spirv-val",
                "--dxc-dir",
                "/d",
                "--build-dir",
                "/b",
                "--artifact",
                "/a.spv",
                "--require-shader-nonuniform",
            ]
        )
        self.assertTrue(smoke.require_shader_nonuniform)
        args = parser.parse_args(
            [
                "verify",
                "--port-dir",
                "/p",
                "--dxc-dir",
                "/d",
                "--dxc-build-dir",
                "/b",
                "--spirv-val-build-dir",
                "/v",
                "--artifact-dir",
                "/a",
            ]
        )
        self.assertEqual(args.command, "verify")

    def test_nested_policy_key_sets_reject_add_drop_and_unknown(self) -> None:
        original_load_json = base.load_json
        original = original_load_json(reference.POLICY_PATH)
        cases = []
        for section in ("dxc", "spirv_val", "device_contract"):
            dropped = copy.deepcopy(original)
            dropped[section].pop(next(iter(dropped[section])))
            cases.append(dropped)
            added = copy.deepcopy(original)
            added[section]["unexpected"] = True
            cases.append(added)
        for mutation in cases:
            with self.subTest(keys=sorted(mutation)), mock.patch.object(
                base,
                "load_json",
                side_effect=lambda path, value=mutation: value
                if path == reference.POLICY_PATH
                else original_load_json(path),
            ), self.assertRaisesRegex(base.ArtifactError, "fields changed"):
                reference.load_policy()

    def test_nested_runtime_policy_key_set_is_exact(self) -> None:
        original_load_json = base.load_json
        mutation = original_load_json(reference.POLICY_PATH)
        mutation["spirv_val"]["darwin_runtime_closure"]["unexpected"] = True
        with mock.patch.object(
            base,
            "load_json",
            side_effect=lambda path: mutation
            if path == reference.POLICY_PATH
            else original_load_json(path),
        ), self.assertRaisesRegex(base.ArtifactError, "fields changed"):
            reference.load_policy()


class InventoryTests(unittest.TestCase):
    def valid_module(self) -> bytes:
        extension = string_words("SPV_EXT_descriptor_indexing")
        return module(
            instruction(17, 1),
            instruction(17, 5301),
            instruction(10, *extension),
            instruction(71, 44, 5300),
        )

    def test_ordered_semantic_inventory_has_word_offsets(self) -> None:
        result = reference.inventory_spirv(self.valid_module(), grammar_bytes())
        self.assertEqual(
            result["capabilities"],
            [
                {"name": "Shader", "value": 1, "word_offset": 5},
                {"name": "ShaderNonUniform", "value": 5301, "word_offset": 7},
            ],
        )
        self.assertEqual(result["extensions"][0]["name"], "SPV_EXT_descriptor_indexing")
        self.assertEqual(result["non_uniform_decorations"], [{"target_id": 44, "word_offset": 17}])
        without_hash = copy.deepcopy(result)
        actual = without_hash.pop("inventory_sha256")
        self.assertEqual(actual, base.digest_bytes(base.canonical_json(without_hash)))

    def test_duplicate_inventory_rows_remain_ordered_evidence(self) -> None:
        result = reference.inventory_spirv(
            module(instruction(17, 1), instruction(17, 1)), grammar_bytes()
        )
        self.assertEqual([row["word_offset"] for row in result["capabilities"]], [5, 7])

    def test_rejects_bad_magic(self) -> None:
        with self.assertRaisesRegex(base.ArtifactError, "magic"):
            reference.inventory_spirv(b"\0" * 20, grammar_bytes())

    def test_rejects_truncated_instruction(self) -> None:
        value = bytearray(module(instruction(17, 1)))
        value[20:24] = ((3 << 16) | 17).to_bytes(4, "little")
        with self.assertRaisesRegex(base.ArtifactError, "malformed"):
            reference.inventory_spirv(bytes(value), grammar_bytes())

    def test_rejects_zero_word_instruction(self) -> None:
        with self.assertRaisesRegex(base.ArtifactError, "malformed"):
            reference.inventory_spirv(module([17]), grammar_bytes())

    def test_rejects_zero_id_bound(self) -> None:
        value = bytearray(module(instruction(17, 1)))
        value[12:16] = (0).to_bytes(4, "little")
        with self.assertRaisesRegex(base.ArtifactError, "bound"):
            reference.inventory_spirv(bytes(value), grammar_bytes())

    def test_rejects_nonzero_schema_word(self) -> None:
        value = bytearray(module(instruction(17, 1)))
        value[16:20] = (1).to_bytes(4, "little")
        with self.assertRaisesRegex(base.ArtifactError, "schema"):
            reference.inventory_spirv(bytes(value), grammar_bytes())

    def test_rejects_unknown_capability(self) -> None:
        with self.assertRaisesRegex(base.ArtifactError, "unknown SPIR-V capability"):
            reference.inventory_spirv(module(instruction(17, 9999)), grammar_bytes())

    def test_rejects_unknown_decoration(self) -> None:
        with self.assertRaisesRegex(base.ArtifactError, "unknown SPIR-V decoration"):
            reference.inventory_spirv(module(instruction(71, 2, 9999)), grammar_bytes())

    def test_rejects_nonuniform_operands(self) -> None:
        with self.assertRaisesRegex(base.ArtifactError, "NonUniform decoration has operands"):
            reference.inventory_spirv(module(instruction(71, 2, 5300, 7)), grammar_bytes())

    def test_rejects_nonuniform_member_decoration(self) -> None:
        with self.assertRaisesRegex(base.ArtifactError, "NonUniform member decoration"):
            reference.inventory_spirv(module(instruction(72, 2, 0, 5300)), grammar_bytes())

    def test_rejects_zero_and_out_of_bound_direct_decoration_targets(self) -> None:
        for target in (0, 100):
            with self.subTest(target=target), self.assertRaisesRegex(base.ArtifactError, "target id"):
                reference.inventory_spirv(module(instruction(71, target, 0)), grammar_bytes())

    def test_rejects_zero_and_out_of_bound_member_decoration_targets(self) -> None:
        for target in (0, 100):
            with self.subTest(target=target), self.assertRaisesRegex(base.ArtifactError, "target id"):
                reference.inventory_spirv(module(instruction(72, target, 0, 0)), grammar_bytes())

    def test_rejects_every_group_decoration_form(self) -> None:
        for opcode in (73, 74, 75):
            with self.subTest(opcode=opcode), self.assertRaisesRegex(base.ArtifactError, "group decoration"):
                reference.inventory_spirv(module(instruction(opcode, 1)), grammar_bytes())

    def test_rejects_unterminated_extension(self) -> None:
        raw = int.from_bytes(b"ABCD", "little")
        with self.assertRaisesRegex(base.ArtifactError, "NUL terminated"):
            reference.inventory_spirv(module(instruction(10, raw)), grammar_bytes())

    def test_rejects_nonzero_extension_padding(self) -> None:
        raw = int.from_bytes(b"A\0BC", "little")
        with self.assertRaisesRegex(base.ArtifactError, "padding"):
            reference.inventory_spirv(module(instruction(10, raw)), grammar_bytes())

    def test_rejects_extension_words_after_terminator(self) -> None:
        terminated = int.from_bytes(b"A\0\0\0", "little")
        trailing = int.from_bytes(b"B\0\0\0", "little")
        with self.assertRaisesRegex(base.ArtifactError, "trailing words"):
            reference.inventory_spirv(
                module(instruction(10, terminated, trailing)), grammar_bytes()
            )

    def test_rejects_missing_grammar_opcode(self) -> None:
        grammar = json.loads(grammar_bytes())
        grammar["instructions"] = grammar["instructions"][:-1]
        with self.assertRaisesRegex(base.ArtifactError, "OpGroupMemberDecorate"):
            reference.inventory_spirv(self.valid_module(), json.dumps(grammar).encode())

    def test_rejects_changed_nonuniform_grammar_value(self) -> None:
        grammar = json.loads(grammar_bytes())
        grammar["operand_kinds"][1]["enumerants"][1]["value"] = 77
        with self.assertRaisesRegex(base.ArtifactError, "NonUniform grammar value"):
            reference.inventory_spirv(self.valid_module(), json.dumps(grammar).encode())


class SpirvValInvocationTests(unittest.TestCase):
    def fixture(self, temporary: str, body: str) -> tuple[Path, Path, Path, dict, dict]:
        root = Path(temporary)
        validator = root / "spirv-val"
        validator.write_text("#!/usr/bin/env python3\n" + body, encoding="utf-8")
        validator.chmod(0o700)
        output = root / "output"
        output.mkdir()
        artifact = output / "x.spv"
        artifact.write_bytes(module(instruction(17, 1)))
        expected = {"id": "x"}
        return root, validator, artifact, expected, reference.load_policy()

    def test_validation_uses_exact_stdin_invocation_and_receipts_input(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            _, validator, artifact, expected, policy = self.fixture(
                temporary,
                "import sys\ndata=sys.stdin.buffer.read()\nsys.exit(0 if sys.argv[1:]==['--target-env','vulkan1.0','--scalar-block-layout','-'] and data[:4]==b'\\x03\\x02#\\x07' else 9)\n",
            )
            result = reference.run_spirv_val(validator, artifact, expected, artifact.parent, policy)
            self.assertEqual(
                result["arguments"],
                ["--target-env", "vulkan1.0", "--scalar-block-layout", "-"],
            )
            self.assertEqual(result["input_sha256"], base.digest_file(artifact))
            self.assertEqual(result["input_bytes"], artifact.stat().st_size)

    def test_rejects_nonzero_validator_exit(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            _, validator, artifact, expected, policy = self.fixture(temporary, "raise SystemExit(3)\n")
            with self.assertRaisesRegex(base.ArtifactError, "spirv-val failed"):
                reference.run_spirv_val(validator, artifact, expected, artifact.parent, policy)

    def test_rejects_unexpected_validator_stdout(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            _, validator, artifact, expected, policy = self.fixture(temporary, "print('noise')\n")
            with self.assertRaisesRegex(base.ArtifactError, "unexpected output"):
                reference.run_spirv_val(validator, artifact, expected, artifact.parent, policy)

    def test_rejects_unexpected_validator_stderr(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            _, validator, artifact, expected, policy = self.fixture(temporary, "import sys\nprint('noise', file=sys.stderr)\n")
            with self.assertRaisesRegex(base.ArtifactError, "unexpected output"):
                reference.run_spirv_val(validator, artifact, expected, artifact.parent, policy)

    def test_rejects_all_noncanonical_validator_argv_before_execution(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            _, validator, artifact, expected, policy = self.fixture(temporary, "raise SystemExit(0)\n")
            mutations = (
                ("omit-scalar", ["--target-env", "vulkan1.0", "-"]),
                ("relax-substitution", ["--target-env", "vulkan1.0", "--relax-block-layout", "-"]),
                (
                    "relax-plus-scalar",
                    ["--target-env", "vulkan1.0", "--scalar-block-layout", "--relax-block-layout", "-"],
                ),
                ("skip", ["--target-env", "vulkan1.0", "--skip-block-layout", "-"]),
                ("reorder", ["--scalar-block-layout", "--target-env", "vulkan1.0", "-"]),
                ("extra", ["--target-env", "vulkan1.0", "--scalar-block-layout", "--before-hlsl-legalization", "-"]),
            )
            for label, argv in mutations:
                mutation = copy.deepcopy(policy)
                mutation["device_contract"]["validator_argv"] = argv
                with self.subTest(label=label), mock.patch.object(reference.subprocess, "run") as executed, self.assertRaisesRegex(
                    base.ArtifactError,
                    "validator argv changed",
                ):
                    reference.run_spirv_val(validator, artifact, expected, artifact.parent, mutation)
                executed.assert_not_called()

    def test_rejects_validator_side_effect_in_output_tree(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root, validator, artifact, expected, policy = self.fixture(
                temporary,
                "import pathlib,sys\npathlib.Path(sys.argv[0]).parent.joinpath('output/side-effect').write_bytes(b'x')\n",
            )
            self.assertEqual(validator.parent, root)
            with self.assertRaisesRegex(base.ArtifactError, "output set"):
                reference.run_spirv_val(validator, artifact, expected, artifact.parent, policy)


class SpirvValSmokeTests(unittest.TestCase):
    def invoke(self, artifact: Path, require: bool = True, observed: dict | None = None) -> dict:
        policy = reference.load_policy()
        grammar = grammar_bytes()
        closure = mock.Mock(grammar_sha256=base.digest_bytes(grammar))
        build_receipt = {
            "receipt_sha256": "1" * 64,
            "validator_sha256": "2" * 64,
        }
        state = observed if observed is not None else {}

        @contextmanager
        def fake_staged(_closure: object, parent: Path):
            state["staged"] = True
            state["parent"] = parent
            state["mode"] = stat.S_IMODE(parent.stat().st_mode)
            validator = parent / "spirv-val"
            validator.write_text("#!/usr/bin/python3\nimport sys\nsys.stdin.buffer.read()\n")
            validator.chmod(0o700)
            grammar_path = parent / "grammar.json"
            grammar_path.write_bytes(grammar)
            yield validator, grammar_path

        args = argparse.Namespace(
            dxc_dir=str(artifact.parent / "dxc"),
            build_dir=str(artifact.parent / "build"),
            artifact=str(artifact),
            require_shader_nonuniform=require,
        )
        with (
            mock.patch.object(reference, "validate_spirv_val_build", return_value=(build_receipt, closure)) as verified,
            mock.patch.object(reference, "staged_spirv_val", fake_staged),
        ):
            result = reference.smoke_spirv_val(args)
        verified.assert_called_once()
        return result

    def test_smoke_verifies_build_validates_stdin_inventories_and_emits_no_path(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            artifact = Path(temporary).resolve() / "witness.spv"
            artifact.write_bytes(module(instruction(17, 5301), instruction(71, 7, 5300)))
            observed = {}
            result = self.invoke(artifact, observed=observed)
            base.validate_receipt_hash(result)
            self.assertEqual(
                set(result),
                {
                    "schema",
                    "status",
                    "orchestration_producer_sha256",
                    "reference_policy_sha256",
                    "spirv_val_build_receipt_sha256",
                    "validator_sha256",
                    "grammar_sha256",
                    "device_contract",
                    "artifact_sha256",
                    "artifact_bytes",
                    "validation",
                    "semantic_inventory",
                    "shader_nonuniform_witness",
                    "claim_boundary",
                    "receipt_sha256",
                },
            )
            self.assertEqual(result["schema"], "fn64.spirv-val-single-artifact-smoke.v2")
            self.assertEqual(result["device_contract"], reference.validated_device_contract(reference.load_policy()))
            self.assertEqual(
                set(result["shader_nonuniform_witness"]),
                {"required", "capability_count", "decoration_count", "satisfied"},
            )
            self.assertEqual(result["artifact_sha256"], base.digest_file(artifact))
            self.assertEqual(result["validation"]["input_sha256"], result["artifact_sha256"])
            self.assertEqual(result["shader_nonuniform_witness"]["capability_count"], 1)
            self.assertEqual(result["shader_nonuniform_witness"]["decoration_count"], 1)
            self.assertEqual(observed["mode"], 0o700)
            self.assertNotIn(str(artifact), json.dumps(result))
            self.assertIsNone(base.LOCAL_PATH_RE.search(json.dumps(result)))

    def test_smoke_rejects_symlink_and_hardlink_before_staging(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            target = root / "target.spv"
            target.write_bytes(module(instruction(17, 5301), instruction(71, 7, 5300)))
            symlink = root / "symlink.spv"
            symlink.symlink_to(target)
            observed = {}
            with self.assertRaisesRegex(base.ArtifactError, "not a regular file"):
                self.invoke(symlink, observed=observed)
            self.assertNotIn("staged", observed)
            real_parent = root / "real-parent"
            real_parent.mkdir()
            parent_target = real_parent / "parent-target.spv"
            parent_target.write_bytes(target.read_bytes())
            parent_link = root / "parent-link"
            parent_link.symlink_to(real_parent, target_is_directory=True)
            with self.assertRaisesRegex(base.ArtifactError, "symlinked parent"):
                self.invoke(parent_link / parent_target.name, observed=observed)
            self.assertNotIn("staged", observed)
            hardlink = root / "hardlink.spv"
            os.link(target, hardlink)
            with self.assertRaisesRegex(base.ArtifactError, "another hardlink"):
                self.invoke(target, observed=observed)
            self.assertNotIn("staged", observed)

    def test_smoke_rejects_artifact_inside_fn64_before_staging(self) -> None:
        policy = reference.load_policy()
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            artifact = root / "inside.spv"
            artifact.write_bytes(module(instruction(17, 5301), instruction(71, 7, 5300)))
            args = argparse.Namespace(
                dxc_dir=str(root / "dxc"),
                build_dir=str(root / "build"),
                artifact=str(artifact),
                require_shader_nonuniform=True,
            )
            with (
                mock.patch.object(reference, "ROOT", root),
                mock.patch.object(reference, "load_policy", return_value=policy),
                mock.patch.object(reference, "validate_spirv_val_build", return_value=({}, object())),
                mock.patch.object(reference, "staged_spirv_val") as staged,
                self.assertRaisesRegex(base.ArtifactError, "must stay outside fn64"),
            ):
                reference.smoke_spirv_val(args)
            staged.assert_not_called()

    def test_required_shader_nonuniform_witness_rejects_missing_semantics(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            artifact = Path(temporary).resolve() / "witness.spv"
            cases = (
                (module(instruction(17, 1), instruction(71, 7, 5300)), "lacks ShaderNonUniform"),
                (module(instruction(17, 5301)), "lacks a NonUniform decoration"),
            )
            for payload, diagnostic in cases:
                artifact.write_bytes(payload)
                with self.subTest(diagnostic=diagnostic), self.assertRaisesRegex(base.ArtifactError, diagnostic):
                    self.invoke(artifact)

    def test_optional_shader_nonuniform_witness_reports_unsatisfied(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            artifact = Path(temporary).resolve() / "ordinary.spv"
            artifact.write_bytes(module(instruction(17, 1)))
            result = self.invoke(artifact, require=False)
            self.assertEqual(
                result["claim_boundary"],
                "conditionally-reference-valid-with-scalar-layout-contract-and-inventoried-not-artifact-provenance-corpus-adapter-wgpu-pipeline-runtime-parity-or-performance",
            )
            self.assertFalse(result["shader_nonuniform_witness"]["required"])
            self.assertFalse(result["shader_nonuniform_witness"]["satisfied"])

    def test_build_receipt_failure_prevents_artifact_access_and_staging(self) -> None:
        args = argparse.Namespace(
            dxc_dir="/does-not-matter",
            build_dir="/does-not-matter",
            artifact="relative.spv",
            require_shader_nonuniform=True,
        )
        with (
            mock.patch.object(reference, "validate_spirv_val_build", side_effect=base.ArtifactError("receipt rejected")),
            mock.patch.object(reference, "read_external_spirv") as read_artifact,
            mock.patch.object(reference, "staged_spirv_val") as staged,
            self.assertRaisesRegex(base.ArtifactError, "receipt rejected"),
        ):
            reference.smoke_spirv_val(args)
        read_artifact.assert_not_called()
        staged.assert_not_called()

    def test_failed_validation_is_receipt_less(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            artifact = root / "witness.spv"
            artifact.write_bytes(module(instruction(17, 5301), instruction(71, 7, 5300)))
            failed = mock.Mock(returncode=1, stdout=b"", stderr=b"validation failed")
            with mock.patch.object(reference.subprocess, "run", return_value=failed), self.assertRaisesRegex(
                base.ArtifactError,
                "spirv-val failed",
            ):
                self.invoke(artifact)
            self.assertEqual(list(root.glob("*.json")), [])

    def test_smoke_rejects_symlinked_temp_parent_inside_fn64_before_staging(self) -> None:
        policy = reference.load_policy()
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            artifact = root / "witness.spv"
            artifact.write_bytes(module(instruction(17, 5301), instruction(71, 7, 5300)))
            fake_repo = root / "repo"
            fake_repo.mkdir()
            temp_link = root / "temp-link"
            temp_link.symlink_to(fake_repo, target_is_directory=True)
            closure = mock.Mock(grammar_sha256=base.digest_bytes(grammar_bytes()))
            build_receipt = {"receipt_sha256": "1" * 64, "validator_sha256": "2" * 64}
            args = argparse.Namespace(
                dxc_dir=str(root / "dxc"),
                build_dir=str(root / "build"),
                artifact=str(artifact),
                require_shader_nonuniform=True,
            )
            temporary_directory = tempfile.TemporaryDirectory

            def inside_repo(**kwargs: object):
                return temporary_directory(dir=temp_link, **kwargs)

            with (
                mock.patch.object(reference, "ROOT", fake_repo),
                mock.patch.object(reference, "load_policy", return_value=policy),
                mock.patch.object(reference, "validate_spirv_val_build", return_value=(build_receipt, closure)),
                mock.patch.object(reference.tempfile, "TemporaryDirectory", side_effect=inside_repo),
                mock.patch.object(reference, "staged_spirv_val") as staged,
                self.assertRaisesRegex(base.ArtifactError, "staging overlaps fn64"),
            ):
                reference.smoke_spirv_val(args)
            staged.assert_not_called()
            self.assertEqual(list(fake_repo.iterdir()), [])

    def test_smoke_rejects_direct_and_symlinked_temp_parent_inside_artifact_tree(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            artifact_directory = root / "artifacts"
            artifact_directory.mkdir()
            artifact = artifact_directory / "witness.spv"
            artifact.write_bytes(module(instruction(17, 5301), instruction(71, 7, 5300)))
            temp_link = root / "temp-link"
            temp_link.symlink_to(artifact_directory, target_is_directory=True)
            temporary_directory = tempfile.TemporaryDirectory
            for label, temp_parent in (("direct", artifact_directory), ("symlink", temp_link)):
                observed = {}

                def inside_artifacts(parent: Path = temp_parent, **kwargs: object):
                    return temporary_directory(dir=parent, **kwargs)

                with self.subTest(label=label), mock.patch.object(
                    reference.tempfile,
                    "TemporaryDirectory",
                    side_effect=inside_artifacts,
                ), self.assertRaisesRegex(base.ArtifactError, "staging overlaps the artifact directory tree"):
                    self.invoke(artifact, observed=observed)
                self.assertNotIn("staged", observed)
            self.assertEqual(list(artifact_directory.iterdir()), [artifact])

    def test_smoke_rejects_renamed_artifact_directory_inode_via_symlinked_temp_parent(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            artifact_directory = root / "artifacts"
            artifact_directory.mkdir()
            artifact = artifact_directory / "witness.spv"
            artifact.write_bytes(module(instruction(17, 5301), instruction(71, 7, 5300)))
            moved_directory = root / "moved-artifacts"
            temp_link = root / "temp-link"
            temporary_directory = tempfile.TemporaryDirectory
            observed = {}

            def rename_then_redirect(**kwargs: object):
                artifact_directory.rename(moved_directory)
                temp_link.symlink_to(moved_directory, target_is_directory=True)
                return temporary_directory(dir=temp_link, **kwargs)

            with mock.patch.object(
                reference.tempfile,
                "TemporaryDirectory",
                side_effect=rename_then_redirect,
            ), self.assertRaisesRegex(base.ArtifactError, "qualified artifact directory identity"):
                self.invoke(artifact, observed=observed)
            self.assertNotIn("staged", observed)
            self.assertTrue((moved_directory / artifact.name).is_file())
            self.assertEqual(
                [path.name for path in moved_directory.iterdir()],
                [artifact.name],
            )

    def test_smoke_rejects_semantic_inventory_amplification_before_receipt(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            artifact = Path(temporary).resolve() / "amplified.spv"
            words = [0x07230203, 0x00010000, 0, 100, 0]
            words.extend(instruction(17, 5301) * 100_000)
            words.extend(instruction(71, 7, 5300) * 100_000)
            artifact.write_bytes(struct.pack(f"<{len(words)}I", *words))
            self.assertEqual(artifact.stat().st_size, 2_000_020)
            with self.assertRaisesRegex(base.ArtifactError, "inventory exceeds the smoke row budget"):
                self.invoke(artifact)

    def test_smoke_rejects_canonical_receipt_over_policy_limit(self) -> None:
        policy = reference.load_policy()
        policy["maximum_receipt_bytes"] = 512
        with tempfile.TemporaryDirectory() as temporary:
            artifact = Path(temporary).resolve() / "witness.spv"
            artifact.write_bytes(module(instruction(17, 5301), instruction(71, 7, 5300)))
            with mock.patch.object(reference, "load_policy", return_value=policy), self.assertRaisesRegex(
                base.ArtifactError,
                "receipt exceeds the maximum canonical receipt size",
            ):
                self.invoke(artifact)

    def test_external_spirv_requires_absolute_path_without_traversal(self) -> None:
        for path in ("relative.spv", "/private/tmp/../tmp/witness.spv"):
            with self.subTest(path=path), self.assertRaisesRegex(base.ArtifactError, "absolute without traversal"):
                reference.read_external_spirv(path)


class ReceiptPrimitiveTests(unittest.TestCase):
    def test_receipt_hash_mutation_is_rejected(self) -> None:
        receipt = base.add_receipt_hash({"schema": "x", "claim_boundary": reference.load_policy()["claim_boundary"]})
        receipt["claim_boundary"] = "wgpu-valid"
        with self.assertRaisesRegex(base.ArtifactError, "identity mismatch"):
            base.validate_receipt_hash(receipt)

    def test_generated_authority_receipt_rejects_path_and_digest_mutation(self) -> None:
        policy = reference.load_policy()
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for index, relative in enumerate(reference.generated_authority_paths(policy)):
                path = root.joinpath(*relative.parts)
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(f"authority-{index}".encode())
            record = reference.record_generated_authority(root, policy)
            reference.validate_generated_authority(record, root, policy)
            mutations = []
            added = copy.deepcopy(record)
            added.append(copy.deepcopy(record[0]))
            mutations.append(("add", added))
            mutations.append(("drop", copy.deepcopy(record[:-1])))
            reordered = copy.deepcopy(record)
            reordered[0], reordered[1] = reordered[1], reordered[0]
            mutations.append(("reorder", reordered))
            renamed = copy.deepcopy(record)
            renamed[0]["path"] = "build/renamed.inc"
            mutations.append(("rename", renamed))
            old_layout = copy.deepcopy(record)
            for row in old_layout:
                row["path"] = row["path"].replace("build/", "build/source/", 1)
            mutations.append(("old-layout", old_layout))
            for label, mutation in mutations:
                with self.subTest(label=label), self.assertRaisesRegex(base.ArtifactError, "denominator changed"):
                    reference.validate_generated_authority(mutation, root, policy)
            changed_digest = copy.deepcopy(record)
            changed_digest[0]["sha256"] = "0" * 64
            with self.assertRaisesRegex(base.ArtifactError, "authority changed"):
                reference.validate_generated_authority(changed_digest, root, policy)

    def test_safe_relative_rejects_escape_absolute_and_normalization(self) -> None:
        for value in ("../x", "/x", "a/../x", "a//x"):
            with self.subTest(value=value), self.assertRaises(base.ArtifactError):
                reference.require_safe_relative(value, "fixture")

    def test_staged_validator_is_private_regular_and_no_link(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            binary = root / "built-validator"
            binary.write_bytes(b"validator")
            binary.chmod(0o700)
            grammar = root / "grammar.json"
            grammar.write_bytes(b"{}")
            contained = base.ContainedExecutable(
                root,
                binary,
                binary,
                {
                    "invocation_path": "built-validator",
                    "kind": "regular",
                    "target_path": "built-validator",
                    "target_sha256": base.digest_file(binary),
                    "invocation_mode": stat.S_IMODE(binary.stat().st_mode),
                    "target_mode": stat.S_IMODE(binary.stat().st_mode),
                },
            )
            closure = reference.SpirvValClosure(
                root,
                contained,
                Path("/usr/bin/otool"),
                {},
                grammar,
                base.digest_file(grammar),
            )
            with reference.staged_spirv_val(closure, root) as (staged, staged_grammar):
                self.assertTrue(staged.is_file() and staged_grammar.is_file())
                self.assertEqual(staged.stat().st_nlink, 1)
                self.assertEqual(stat.S_IMODE(staged.stat().st_mode), 0o500)
                self.assertEqual(stat.S_IMODE(staged_grammar.stat().st_mode), 0o400)

    def expected_artifact(self) -> tuple[dict, dict]:
        expected = {
            "id": "x",
            "source": "src/X.hlsl",
            "flags": ["-spirv", "-E", "CSMain", "-T", "cs_6_3"],
            "dependency_manifest_artifact": "dependencies/X.json",
            "preprocessed_artifact": "preprocessed/X.hlsl",
            "spirv_artifact": "spirv/X.spv",
        }
        row = {
            "dependency_output_artifact": base.dependency_output_artifact(expected)
        }
        return expected, row

    def test_dependency_output_path_matches_exact_phase_contract(self) -> None:
        expected, row = self.expected_artifact()
        reference.validate_reference_artifact_paths(expected, row)
        row["dependency_output_artifact"] = "dependencies/renamed.d"
        with self.assertRaisesRegex(base.ArtifactError, "dependency output path changed"):
            reference.validate_reference_artifact_paths(expected, row)

    def test_dependency_output_rejects_every_retained_path_collision(self) -> None:
        for field, diagnostic in (
            ("dependency_manifest_artifact", "dependency manifest path changed"),
            ("preprocessed_artifact", "collide"),
            ("spirv_artifact", "collide"),
        ):
            expected, row = self.expected_artifact()
            expected[field] = row["dependency_output_artifact"]
            with self.subTest(field=field), self.assertRaisesRegex(base.ArtifactError, diagnostic):
                reference.validate_reference_artifact_paths(expected, row)

    def test_controlled_environment_is_exact_and_ordered(self) -> None:
        policy = reference.load_policy()
        tools = [Path(f"/qualified/{name}/bin/{name}") for name in ("cmake", "ninja", "python", "git", "cc", "cxx")]
        environment = reference.spirv_val_build_environment(*tools, policy)
        self.assertEqual(list(environment), policy["spirv_val"]["controlled_environment_names"])
        self.assertEqual(environment["CC"], str(tools[4]))
        self.assertEqual(environment["CXX"], str(tools[5]))
        self.assertEqual(environment["GIT_CONFIG_GLOBAL"], os.devnull)
        self.assertEqual(environment["GIT_CONFIG_NOSYSTEM"], "1")
        self.assertEqual(environment["GIT_NO_REPLACE_OBJECTS"], "1")
        self.assertEqual(environment["GIT_OPTIONAL_LOCKS"], "0")
        self.assertEqual(environment["GIT_TERMINAL_PROMPT"], "0")

    def test_controlled_environment_rejects_add_drop_reorder_and_value_drift(self) -> None:
        policy = reference.load_policy()
        tools = [Path(f"/qualified/{name}/bin/{name}") for name in ("cmake", "ninja", "python", "git", "cc", "cxx")]
        environment = reference.spirv_val_build_environment(*tools, policy)
        record = reference.environment_record(environment, policy)
        mutations = []
        added = copy.deepcopy(record)
        added.append({"name": "EXTRA", "value_sha256": "0" * 64})
        mutations.append(added)
        mutations.append(copy.deepcopy(record[:-1]))
        reordered = copy.deepcopy(record)
        reordered[0], reordered[1] = reordered[1], reordered[0]
        mutations.append(reordered)
        drifted = copy.deepcopy(record)
        drifted[0]["value_sha256"] = "0" * 64
        mutations.append(drifted)
        for index, mutation in enumerate(mutations):
            with self.subTest(index=index), self.assertRaises(base.ArtifactError):
                reference.validate_environment_record(mutation, environment, policy)

    def test_verify_stages_outside_exact_corpus_file_tree(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            artifact_dir = root / "corpus"
            artifact_dir.mkdir()
            (artifact_dir / reference.RECEIPT_PATH).write_bytes(b"{}\n")
            port = root / "port"
            source = root / "dxc"
            dxc_build = root / "dxc-build"
            validator_build = root / "validator-build"
            for path in (port, source, dxc_build, validator_build):
                path.mkdir()
            observed = {}

            @contextmanager
            def fake_staged(_closure: object, parent: Path):
                observed["parent"] = parent
                observed["mode"] = stat.S_IMODE(parent.stat().st_mode)
                staged = parent / "stage"
                staged.mkdir()
                validator = staged / "spirv-val"
                grammar = staged / "grammar.json"
                validator.write_bytes(b"v")
                grammar.write_bytes(b"{}")
                yield validator, grammar

            def exact_denominator(*_arguments: object) -> None:
                observed["files"] = {
                    path.relative_to(artifact_dir).as_posix()
                    for path in artifact_dir.rglob("*")
                    if path.is_file() or path.is_symlink()
                }

            args = argparse.Namespace(
                port_dir=str(port),
                oracle_dir=None,
                dxc_dir=str(source),
                dxc_build_dir=str(dxc_build),
                spirv_val_build_dir=str(validator_build),
                artifact_dir=str(artifact_dir),
            )
            with (
                mock.patch.object(base, "check_denominator", return_value={}),
                mock.patch.object(base, "validate_build_receipt", return_value=({}, object())),
                mock.patch.object(reference, "validate_spirv_val_build", return_value=({}, object())),
                mock.patch.object(base, "load_canonical_json", return_value={}),
                mock.patch.object(reference, "staged_spirv_val", fake_staged),
                mock.patch.object(reference, "validate_reference_receipt", side_effect=exact_denominator),
            ):
                reference.verify(args)
            self.assertEqual(observed["mode"], 0o700)
            self.assertEqual(observed["files"], {reference.RECEIPT_PATH})
            self.assertNotIn(artifact_dir, observed["parent"].parents)
            self.assertNotIn(observed["parent"], artifact_dir.parents)

    def test_verify_rejects_symlinked_temp_parent_inside_corpus_before_staging(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            artifact_dir = root / "corpus"
            artifact_dir.mkdir()
            receipt_path = artifact_dir / reference.RECEIPT_PATH
            receipt_path.write_bytes(b"{}\n")
            temp_link = root / "temp-link"
            temp_link.symlink_to(artifact_dir, target_is_directory=True)
            port = root / "port"
            source = root / "dxc"
            dxc_build = root / "dxc-build"
            validator_build = root / "validator-build"
            for path in (port, source, dxc_build, validator_build):
                path.mkdir()
            args = argparse.Namespace(
                port_dir=str(port),
                oracle_dir=None,
                dxc_dir=str(source),
                dxc_build_dir=str(dxc_build),
                spirv_val_build_dir=str(validator_build),
                artifact_dir=str(artifact_dir),
            )
            temporary_directory = tempfile.TemporaryDirectory

            def inside_corpus(**kwargs: object):
                self.assertNotIn("dir", kwargs)
                return temporary_directory(dir=temp_link, **kwargs)

            with (
                mock.patch.object(base, "check_denominator", return_value={}),
                mock.patch.object(base, "validate_build_receipt", return_value=({}, object())),
                mock.patch.object(reference, "validate_spirv_val_build", return_value=({}, object())),
                mock.patch.object(base, "load_canonical_json", return_value={}),
                mock.patch.object(reference.tempfile, "TemporaryDirectory", side_effect=inside_corpus),
                mock.patch.object(reference, "staged_spirv_val", side_effect=AssertionError("staging must not run")) as staged,
                mock.patch.object(reference, "validate_reference_receipt", side_effect=AssertionError("validation must not run")),
                self.assertRaisesRegex(base.ArtifactError, "staging overlaps the corpus tree"),
            ):
                reference.verify(args)
            staged.assert_not_called()
            self.assertEqual(list(artifact_dir.iterdir()), [receipt_path])


if __name__ == "__main__":
    unittest.main()
