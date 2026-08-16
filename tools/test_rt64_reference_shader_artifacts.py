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

    def test_validation_arguments_are_exact_and_unrelaxed(self) -> None:
        value = reference.load_policy()["spirv_val"]["validation_arguments"]
        self.assertEqual(value, ["--target-env", "vulkan1.0"])
        self.assertFalse(any("relax" in item or "skip" in item for item in value))

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

    def test_claim_boundary_never_mentions_runtime_or_parity_success(self) -> None:
        self.assertEqual(reference.load_policy()["claim_boundary"], "reference-valid-only-not-wgpu-runtime-or-parity")

    def test_parser_exposes_additive_commands_only(self) -> None:
        parser = reference.parser()
        self.assertEqual(parser.parse_args(["selftest"]).command, "selftest")
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
        for section in ("dxc", "spirv_val"):
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
                "import sys\ndata=sys.stdin.buffer.read()\nsys.exit(0 if sys.argv[1:]==['--target-env','vulkan1.0','-'] and data[:4]==b'\\x03\\x02#\\x07' else 9)\n",
            )
            result = reference.run_spirv_val(validator, artifact, expected, artifact.parent, policy)
            self.assertEqual(result["arguments"], ["--target-env", "vulkan1.0", "-"])
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

    def test_rejects_argument_policy_drift_before_execution(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            _, validator, artifact, expected, policy = self.fixture(temporary, "raise SystemExit(0)\n")
            policy["spirv_val"]["validation_arguments"] = ["--target-env", "vulkan1.1"]
            with self.assertRaisesRegex(base.ArtifactError, "noncanonical"):
                reference.run_spirv_val(validator, artifact, expected, artifact.parent, policy)

    def test_rejects_validator_side_effect_in_output_tree(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root, validator, artifact, expected, policy = self.fixture(
                temporary,
                "import pathlib,sys\npathlib.Path(sys.argv[0]).parent.joinpath('output/side-effect').write_bytes(b'x')\n",
            )
            self.assertEqual(validator.parent, root)
            with self.assertRaisesRegex(base.ArtifactError, "output set"):
                reference.run_spirv_val(validator, artifact, expected, artifact.parent, policy)


class ReceiptPrimitiveTests(unittest.TestCase):
    def test_receipt_hash_mutation_is_rejected(self) -> None:
        receipt = base.add_receipt_hash({"schema": "x", "claim_boundary": reference.load_policy()["claim_boundary"]})
        receipt["claim_boundary"] = "wgpu-valid"
        with self.assertRaisesRegex(base.ArtifactError, "identity mismatch"):
            base.validate_receipt_hash(receipt)

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
