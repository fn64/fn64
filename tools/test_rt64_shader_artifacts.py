#!/usr/bin/env python3

from __future__ import annotations

import copy
import json
import os
import shutil
import stat
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))
import rt64_shader_artifacts as artifacts


class DenominatorTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.policy = artifacts.load_policy()
        cls.inventory = artifacts.load_inventory(cls.policy)
        cls.denominator = artifacts.load_json(artifacts.DENOMINATOR_PATH)

    def test_complete_dual_pin_denominator(self) -> None:
        denominator = self.denominator
        self.assertEqual(denominator["schema"], artifacts.DENOMINATOR_SCHEMA)
        self.assertEqual(
            denominator["authority"]["port_commit"],
            "5473732a822a4423b5696e7cb18fecc425a59875",
        )
        self.assertTrue(denominator["authority"]["shader_sources_identical"])
        self.assertTrue(denominator["authority"]["dual_pin_source_set_verified"])
        self.assertEqual(
            denominator["authority"]["port_source_set_sha256"],
            "446b5f27ef2df359abf083267cc51449db4da46cb6725aaaca07c8d89153e9bd",
        )
        self.assertEqual(
            denominator["authority"]["oracle_source_set_sha256"],
            denominator["authority"]["port_source_set_sha256"],
        )
        self.assertEqual(denominator["counts"], {
            "spirv_entries": 56,
            "hlsl_sources": 36,
            "hlsli_sources": 20,
            "dependency_files": 86,
            "include_edges": 137,
            "non_spirv_entries": 3,
            "preprocess_only": 1,
        })
        shader_inventory = {
            row["path"]
            for row in self.inventory["files"]
            if row["path"].startswith("src/shaders/")
            and Path(row["path"]).suffix in {".hlsl", ".hlsli"}
        }
        retained_shader_sources = {
            row["path"] for row in denominator["source_files"] if row["path"].startswith("src/shaders/")
        }
        self.assertEqual(retained_shader_sources, shader_inventory)
        compiled = {row["source"] for row in denominator["entries"]}
        libraries = {row["source"] for row in denominator["non_spirv_entries"]}
        self.assertEqual(compiled | libraries, {path for path in shader_inventory if path.endswith(".hlsl")})

    def test_entries_and_graph_are_closed(self) -> None:
        denominator = self.denominator
        source_rows = {row["path"]: row for row in denominator["source_files"]}
        ids = [row["id"] for row in denominator["entries"]]
        self.assertEqual(len(ids), len(set(ids)))
        self.assertEqual(
            {stage: sum(row["stage"] == stage for row in denominator["entries"]) for stage in ("compute", "fragment", "vertex")},
            {"compute": 22, "fragment": 29, "vertex": 5},
        )
        for entry in denominator["entries"]:
            self.assertIn(entry["source"], entry["dependency_files"])
            self.assertLessEqual(set(entry["dependency_files"]), source_rows.keys())
            self.assertIn("-spirv", entry["flags"])
            self.assertIn("-fspv-target-env=vulkan1.0", entry["flags"])
            self.assertNotIn("-Vd", entry["flags"])
        for edge in denominator["include_edges"]:
            self.assertIn(edge["source"], source_rows)
            self.assertIn(edge["resolved"], source_rows)
        orphan = [
            row for row in denominator["unresolved_includes"]
            if row["source"] == "src/shaders/Lights.hlsli" and row["include"] == "Ray.hlsli"
        ]
        self.assertEqual(len(orphan), 1)
        self.assertFalse(any("src/shaders/Lights.hlsli" in row["dependency_files"] for row in denominator["entries"]))

    def test_denominator_hash_and_no_private_path(self) -> None:
        denominator = copy.deepcopy(self.denominator)
        expected = denominator.pop("denominator_sha256")
        self.assertEqual(
            expected,
            "cae8956fff3258bf5c21bb5cea7ffb550ab726118840a16db69764d3507d3ebe",
        )
        self.assertEqual(expected, artifacts.digest_bytes(artifacts.canonical_json(denominator)))
        self.assertIsNone(artifacts.LOCAL_PATH_RE.search(json.dumps(self.denominator)))

    def test_policy_closes_every_dxc_license_and_validator_lock(self) -> None:
        dxc = self.policy["dxc"]
        self.assertEqual(
            {row["path"] for row in dxc["license_files"] + dxc["bundled_license_files"]},
            {
                ".gitmodules",
                "LICENSE.TXT",
                "ThirdPartyNotices.txt",
                "lib/DxilCompression/LICENSE.TXT",
                "lib/Support/COPYRIGHT.regex",
                "test/YAMLParser/LICENSE.txt",
                "tools/clang/lib/Headers/hlsl/LICENSE.txt",
                "utils/unittest/googlemock/LICENSE.txt",
                "utils/unittest/googletest/LICENSE.TXT",
            },
        )
        validator = self.policy["validator"]
        self.assertEqual(
            artifacts.digest_file(artifacts.ROOT / validator["source"] / "Cargo.lock"),
            validator["cargo_lock_sha256"],
        )

    def test_parser_rejects_a_new_unclassified_producer(self) -> None:
        with self.assertRaisesRegex(artifacts.ArtifactError, "unclassified"):
            artifacts.parse_cmake_shader_calls(
                b'build_future_shader(rt64 "src/shaders/Future.hlsl")\n'
            )

    def test_relative_parent_include_is_normalized(self) -> None:
        files = {"src/common/Shared.h": {}}
        self.assertEqual(
            artifacts.resolve_include("src/shaders/X.hlsl", "../common/Shared.h", files),
            "src/common/Shared.h",
        )

    def test_cmake_option_denominator_matches_policy(self) -> None:
        cmake = b"""set (DXC_COMMON_OPTS "-I${PROJECT_SOURCE_DIR}/src")
set (DXC_SPV_OPTS "-spirv" "-fspv-target-env=vulkan1.0" "-fvk-use-dx-layout")
set (DXC_PS_OPTS "${DXC_COMMON_OPTS}" "-E" "PSMain" "-T ps_6_3")
set (DXC_VS_OPTS "${DXC_COMMON_OPTS}" "-E" "VSMain" "-T vs_6_3" "-fvk-invert-y")
set (DXC_CS_OPTS "${DXC_COMMON_OPTS}" "-E" "CSMain" "-T cs_6_3")
"""
        options = artifacts.parse_cmake_shader_options(cmake)
        artifacts.validate_cmake_shader_options(options, self.policy)

    def test_compiler_dependency_output_is_canonicalized_and_closed(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "src/shaders/Fixture.hlsl"
            include = root / "src/shaders/Fixture.hlsli"
            source.parent.mkdir(parents=True)
            source.write_bytes(b"source")
            include.write_bytes(b"include")
            depfile = root / "fixture.d"
            depfile.write_text(
                "outside.spv: src/shaders/Fixture.hlsl \\\n src/shaders/Fixture.hlsli\n",
                encoding="utf-8",
            )
            expected = {
                "id": "fixture",
                "source": "src/shaders/Fixture.hlsl",
                "dependency_files": [
                    "src/shaders/Fixture.hlsl",
                    "src/shaders/Fixture.hlsli",
                ],
            }
            denominator = {
                "source_files": [
                    {"path": expected["source"], "port_sha256": artifacts.digest_file(source)},
                    {"path": "src/shaders/Fixture.hlsli", "port_sha256": artifacts.digest_file(include)},
                ]
            }
            result = artifacts.parse_dxc_dependencies(depfile, root, expected, denominator)
            self.assertEqual(
                [row["path"] for row in result["files"]],
                ["src/shaders/Fixture.hlsl", "src/shaders/Fixture.hlsli"],
            )

    def test_compiler_dependency_escape_fails(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            depfile = root / "fixture.d"
            depfile.write_text("outside.spv: /etc/hosts\n", encoding="utf-8")
            with self.assertRaisesRegex(artifacts.ArtifactError, "escaped"):
                artifacts.parse_dxc_dependencies(
                    depfile,
                    root,
                    {"id": "fixture", "source": "src/X.hlsl", "dependency_files": ["src/X.hlsl"]},
                    {"source_files": []},
                )

    def test_compiler_dependency_hashes_observed_snapshot_bytes(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "src/shaders/Fixture.hlsl"
            source.parent.mkdir(parents=True)
            source.write_bytes(b"changed after denominator")
            depfile = root / "fixture.d"
            depfile.write_text("fixture.spv: src/shaders/Fixture.hlsl\n", encoding="utf-8")
            expected = {
                "id": "fixture",
                "source": "src/shaders/Fixture.hlsl",
                "dependency_files": ["src/shaders/Fixture.hlsl"],
            }
            denominator = {
                "source_files": [{"path": expected["source"], "port_sha256": artifacts.digest_bytes(b"pinned bytes")}]
            }
            with self.assertRaisesRegex(artifacts.ArtifactError, "bytes changed"):
                artifacts.parse_dxc_dependencies(depfile, root, expected, denominator)

    def test_private_snapshot_is_a_pinned_copy_not_a_link(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            port = root / "port"
            source = port / "src/shaders/Fixture.hlsl"
            source.parent.mkdir(parents=True)
            source.write_bytes(b"pinned bytes")
            denominator = {
                "source_files": [{"path": "src/shaders/Fixture.hlsl", "port_sha256": artifacts.digest_bytes(b"pinned bytes")}]
            }
            stage = root / "stage"
            record = artifacts.stage_rt64_source_snapshot(port, stage, denominator, 1024)
            copied = stage / "src/shaders/Fixture.hlsl"
            self.assertEqual(record, artifacts.source_snapshot_record(denominator))
            self.assertEqual(copied.stat().st_nlink, 1)
            self.assertNotEqual(source.stat().st_ino, copied.stat().st_ino)
            source.write_bytes(b"later mutation")
            self.assertEqual(copied.read_bytes(), b"pinned bytes")

    def test_private_snapshot_rejects_symlinked_source(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            port = root / "port"
            target = root / "target.hlsl"
            target.write_bytes(b"pinned bytes")
            source = port / "src/shaders/Fixture.hlsl"
            source.parent.mkdir(parents=True)
            source.symlink_to(target)
            denominator = {
                "source_files": [{"path": "src/shaders/Fixture.hlsl", "port_sha256": artifacts.digest_bytes(b"pinned bytes")}]
            }
            with self.assertRaisesRegex(artifacts.ArtifactError, "cannot open"):
                artifacts.stage_rt64_source_snapshot(port, root / "stage", denominator, 1024)


class DxcBuildManifestTests(unittest.TestCase):
    def fixture(self, temporary: str) -> tuple[Path, Path, Path, Path]:
        root = Path(temporary)
        source = root / "source"
        build = root / "build"
        (source / "lib").mkdir(parents=True)
        (build / "obj").mkdir(parents=True)
        (source / "lib/a.cpp").write_bytes(b"a\n")
        (source / "lib/b.cpp").write_bytes(b"b\n")
        (build / "obj/a.o").write_bytes(b"object a")
        compile_commands = build / "compile_commands.json"
        compile_commands.write_text(
            json.dumps([
                {
                    "directory": str(build),
                    "file": str(source / "lib/a.cpp"),
                    "output": "obj/a.o",
                    "command": "c++ -c lib/a.cpp -o obj/a.o",
                },
                {
                    "directory": str(build),
                    "file": str(source / "lib/b.cpp"),
                    "output": "obj/b.o",
                    "command": "c++ -c lib/b.cpp -o obj/b.o",
                },
            ]),
            encoding="utf-8",
        )
        ninja_log = build / ".ninja_log"
        ninja_log.write_text("# ninja log v5\n1\t2\t0\tobj/a.o\tdeadbeef\n", encoding="utf-8")
        return source, build, compile_commands, ninja_log

    def test_manifest_contains_only_executed_target_translation_units(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            source, build, compile_commands, ninja_log = self.fixture(temporary)
            manifest = artifacts.compiled_source_manifest(source, build, compile_commands, ninja_log)
            self.assertEqual(manifest["selection"], "fresh-ninja-target-executed-output-intersection")
            self.assertEqual(
                [row["path"] for row in manifest["translation_units"]],
                ["source/lib/a.cpp"],
            )

    def test_unidentified_executed_object_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            source, build, compile_commands, ninja_log = self.fixture(temporary)
            ninja_log.write_text(
                "# ninja log v5\n1\t2\t0\tobj/a.o\tdeadbeef\n1\t2\t0\tobj/unknown.o\tbadcafe\n",
                encoding="utf-8",
            )
            with self.assertRaisesRegex(artifacts.ArtifactError, "lack compile-command"):
                artifacts.compiled_source_manifest(source, build, compile_commands, ninja_log)


class DxcCompilerArtifactTests(unittest.TestCase):
    def fixture(self, temporary: str) -> tuple[Path, Path, Path]:
        root = Path(temporary)
        target = root / "build/bin/dxc-3.7"
        target.parent.mkdir(parents=True)
        target.write_bytes(b"#!/bin/sh\nexit 0\n")
        target.chmod(0o700)
        alias = target.with_name("dxc")
        alias.symlink_to("dxc-3.7")
        return root, alias, target

    def qualify(self, root: Path, alias: Path) -> artifacts.ContainedExecutable:
        return artifacts.qualify_contained_executable(root, alias, 1024, "fixture compiler")

    def closure_fixture(self, temporary: str) -> tuple[artifacts.DxcCompilerClosure, Path, Path]:
        root, alias, target = self.fixture(temporary)
        target.write_bytes(b"#!/bin/sh\nprintf 'GOOD\\n'\n")
        target.chmod(0o700)
        library = root / "build/lib/libdxcompiler.dylib"
        library.parent.mkdir(parents=True)
        library.write_bytes(b"GOOD LIBRARY\n")
        library.chmod(0o700)
        compiler = self.qualify(root, alias)
        runtime = artifacts.qualify_contained_executable(root, library, 1024, "fixture runtime")
        closure = artifacts.DxcCompilerClosure(
            root=root,
            compiler=compiler,
            runtime_files=((runtime, "lib/libdxcompiler.dylib"),),
            inspector=Path("/usr/bin/false"),
            receipt_record={"fixture": True},
        )
        return closure, target, library

    def test_official_relative_symlink_binds_alias_text_and_target_bytes(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root, alias, target = self.fixture(temporary)
            qualified = self.qualify(root, alias)
            self.assertEqual(qualified.invocation_path, alias)
            self.assertEqual(qualified.target_path, target)
            self.assertEqual(qualified.receipt_record, {
                "kind": "relative-contained-symlink",
                "invocation_relative_path": "build/bin/dxc",
                "link_text": "dxc-3.7",
                "target_relative_path": "build/bin/dxc-3.7",
                "target_bytes": target.stat().st_size,
                "target_sha256": artifacts.digest_bytes(target.read_bytes()),
            })

    def test_regular_compiler_path_is_bound_without_a_link_edge(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root, alias, target = self.fixture(temporary)
            alias.unlink()
            target.rename(alias)
            qualified = self.qualify(root, alias)
            self.assertEqual(qualified.target_path, alias)
            self.assertEqual(qualified.receipt_record["kind"], "regular")
            self.assertNotIn("link_text", qualified.receipt_record)

    def test_symlinked_admitted_root_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            storage = Path(temporary) / "storage"
            root, alias, _ = self.fixture(str(storage))
            root_alias = Path(temporary) / "root-alias"
            root_alias.symlink_to(root, target_is_directory=True)
            with self.assertRaisesRegex(artifacts.ArtifactError, "root is not a regular directory"):
                self.qualify(root_alias, root_alias / alias.relative_to(root))

    def test_absolute_symlink_target_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root, alias, target = self.fixture(temporary)
            alias.unlink()
            alias.symlink_to(target)
            with self.assertRaisesRegex(artifacts.ArtifactError, "must be relative"):
                self.qualify(root, alias)

    def test_parent_escape_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root, alias, _ = self.fixture(temporary)
            alias.unlink()
            alias.symlink_to("../../../outside-dxc")
            with self.assertRaisesRegex(artifacts.ArtifactError, "may not escape"):
                self.qualify(root, alias)

    def test_multi_hop_symlink_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root, alias, target = self.fixture(temporary)
            intermediate = target.with_name("intermediate")
            intermediate.symlink_to(target.name)
            alias.unlink()
            alias.symlink_to(intermediate.name)
            with self.assertRaisesRegex(artifacts.ArtifactError, "another symlink"):
                self.qualify(root, alias)

    def test_nonregular_target_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root, alias, _ = self.fixture(temporary)
            directory = alias.parent / "directory"
            directory.mkdir()
            alias.unlink()
            alias.symlink_to(directory.name)
            with self.assertRaisesRegex(artifacts.ArtifactError, "not a regular file"):
                self.qualify(root, alias)

    @unittest.skipUnless(hasattr(os, "link"), "hardlinks unavailable")
    def test_hardlinked_target_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root, alias, target = self.fixture(temporary)
            os.link(target, target.with_name("reused-target"))
            with self.assertRaisesRegex(artifacts.ArtifactError, "another hardlink"):
                self.qualify(root, alias)

    def test_link_swap_during_target_read_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root, alias, _ = self.fixture(temporary)
            original = artifacts.stable_file_bytes

            def swap(path: Path, maximum: int, label: str) -> bytes:
                data = original(path, maximum, label)
                alias.unlink()
                alias.symlink_to("different-target")
                return data

            with mock.patch.object(artifacts, "stable_file_bytes", side_effect=swap):
                with self.assertRaisesRegex(artifacts.ArtifactError, "symlink changed"):
                    self.qualify(root, alias)

    def test_target_swap_during_descriptor_read_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root, alias, target = self.fixture(temporary)
            replacement = target.with_name("replacement")
            replacement.write_bytes(b"#!/bin/sh\nexit 1\n")
            replacement.chmod(0o700)
            original = artifacts.stable_file_bytes

            def swap(path: Path, maximum: int, label: str) -> bytes:
                data = original(path, maximum, label)
                target.unlink()
                replacement.rename(target)
                return data

            with mock.patch.object(artifacts, "stable_file_bytes", side_effect=swap):
                with self.assertRaisesRegex(artifacts.ArtifactError, "path changed"):
                    self.qualify(root, alias)

    def test_version_executes_private_closure_during_source_swap_restore(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            closure, target, library = self.closure_fixture(temporary)
            original_run = subprocess.run
            good_target = target.read_bytes()
            good_library = library.read_bytes()

            def swap(command, **kwargs) -> subprocess.CompletedProcess:
                self.assertNotEqual(Path(command[0]), closure.compiler.invocation_path)
                self.assertEqual(Path(command[0]).read_bytes(), good_target)
                self.assertEqual(Path(command[0]).parent.parent.joinpath("lib/libdxcompiler.dylib").read_bytes(), good_library)
                target.write_bytes(b"#!/bin/sh\nprintf 'EVIL\\n'\n")
                library.write_bytes(b"EVIL LIBRARY\n")
                try:
                    return original_run(command, **kwargs)
                finally:
                    target.write_bytes(good_target)
                    library.write_bytes(good_library)

            with mock.patch.object(artifacts.subprocess, "run", side_effect=swap):
                record = artifacts.dxc_closure_tool_record(closure, ["--version"], closure.root)
            self.assertEqual(record["version_stdout"], "GOOD")
            self.assertEqual(target.read_bytes(), good_target)
            self.assertEqual(library.read_bytes(), good_library)

    def test_produce_path_uses_one_private_closure_during_source_swap_restore(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            closure, target, library = self.closure_fixture(temporary)
            good_target = target.read_bytes()
            good_library = library.read_bytes()
            with artifacts.staged_dxc_compiler(closure, closure.root) as compiler:
                self.assertNotEqual(compiler, closure.compiler.invocation_path)
                target.write_bytes(b"#!/bin/sh\nprintf 'EVIL\\n'\n")
                library.write_bytes(b"EVIL LIBRARY\n")
                first = artifacts.run_dxc([str(compiler), "preprocess"], closure.root)
                target.write_bytes(good_target)
                library.write_bytes(good_library)
                target.write_bytes(b"#!/bin/sh\nprintf 'EVIL AGAIN\\n'\n")
                library.write_bytes(b"EVIL LIBRARY AGAIN\n")
                second = artifacts.run_dxc([str(compiler), "compile"], closure.root)
                target.write_bytes(good_target)
                library.write_bytes(good_library)
            self.assertEqual(first["stdout_sha256"], artifacts.digest_bytes(b"GOOD\n"))
            self.assertEqual(second["stdout_sha256"], artifacts.digest_bytes(b"GOOD\n"))
            self.assertEqual(target.read_bytes(), good_target)
            self.assertEqual(library.read_bytes(), good_library)

    def test_unclassified_non_system_runtime_dependency_fails_closed(self) -> None:
        with self.assertRaisesRegex(artifacts.ArtifactError, "unclassified non-system"):
            artifacts.classify_macho_loads(
                [{"load_name": "@rpath/evil.dylib", "descriptor": "@rpath/evil.dylib (compatibility version 1.0.0)"}],
                {},
                {"/usr/lib/libSystem.B.dylib"},
                "fixture",
            )

    def test_runtime_inspector_reads_the_private_closure_snapshot(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root, alias, target = self.fixture(temporary)
            library = root / "build/lib/libdxcompiler.dylib"
            library.parent.mkdir(parents=True)
            library.write_bytes(b"GOOD LIBRARY\n")
            library.chmod(0o700)
            good_target = target.read_bytes()
            good_library = library.read_bytes()
            policy = {
                "maximum_compiler_bytes": 1024,
                "maximum_runtime_dependency_bytes": 1024,
                "darwin_runtime_closure": {
                    "inspector": "/usr/bin/true",
                    "format": "otool-L-v1",
                    "system_load_names": ["/usr/lib/system.dylib"],
                    "retained": [{
                        "load_name": "@rpath/libdxcompiler.dylib",
                        "relative_path": "build/lib/libdxcompiler.dylib",
                        "snapshot_relative_path": "lib/libdxcompiler.dylib",
                        "install_name": "@rpath/libdxcompiler.dylib",
                    }],
                },
            }

            def inspect(_inspector: Path, path: Path, _label: str) -> tuple[list[dict], dict]:
                self.assertIn(".fn64-dxc-runtime-", str(path))
                self.assertNotIn(path, (target, library))
                if path.name == "dxc":
                    self.assertEqual(path.read_bytes(), good_target)
                    rows = [
                        {"load_name": "@rpath/libdxcompiler.dylib", "descriptor": "retained"},
                        {"load_name": "/usr/lib/system.dylib", "descriptor": "system"},
                    ]
                else:
                    self.assertEqual(path.read_bytes(), good_library)
                    rows = [
                        {"load_name": "@rpath/libdxcompiler.dylib", "descriptor": "install"},
                        {"load_name": "/usr/lib/system.dylib", "descriptor": "system"},
                    ]
                target.write_bytes(b"EVIL COMPILER\n")
                library.write_bytes(b"EVIL LIBRARY\n")
                target.write_bytes(good_target)
                library.write_bytes(good_library)
                return rows, {"loads_sha256": artifacts.digest_bytes(artifacts.canonical_json(rows)), "stderr_sha256": "0" * 64}

            with mock.patch.object(artifacts, "inspect_otool_loads", side_effect=inspect):
                closure = artifacts.qualify_dxc_runtime_closure(root, [alias], policy)
            self.assertEqual(closure.compiler.receipt_record["target_sha256"], artifacts.digest_bytes(good_target))
            self.assertEqual(
                closure.runtime_files[0][0].receipt_record["target_sha256"],
                artifacts.digest_bytes(good_library),
            )

    def test_multiple_reviewed_aliases_reusing_one_target_fail_closed(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root, alias, _ = self.fixture(temporary)
            second = alias.with_name("dxc.exe")
            second.symlink_to("dxc-3.7")
            with self.assertRaisesRegex(artifacts.ArtifactError, "multiple dxc invocation paths"):
                artifacts.select_dxc_compiler(root, [alias, second], 1024)


class ValidatorIsolationTests(unittest.TestCase):
    def test_ancestor_cargo_config_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "build/source"
            cargo_home = root / "build/cargo-home"
            source.mkdir(parents=True)
            cargo_home.mkdir()
            config = root / ".cargo/config.toml"
            config.parent.mkdir()
            config.write_text(
                '[target.aarch64-apple-darwin]\nrustflags = ["-C", "link-arg=/hostile/linker"]\n',
                encoding="utf-8",
            )
            with self.assertRaisesRegex(artifacts.ArtifactError, "ambient Cargo config"):
                artifacts.require_isolated_cargo_configuration(source, cargo_home)

    @unittest.skipUnless(shutil.which("cargo") and shutil.which("rustc"), "Rust toolchain unavailable")
    def test_manifest_ancestor_cargo_config_cannot_inject_flags_from_filesystem_root(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            project = root / "project"
            (project / "src").mkdir(parents=True)
            (root / ".cargo").mkdir()
            (root / ".cargo/config.toml").write_text(
                '[build]\nrustflags = ["--cfg", "fn64_hostile_config"]\n',
                encoding="utf-8",
            )
            (project / "Cargo.toml").write_text(
                '[package]\nname = "cargo-isolation-fixture"\nversion = "0.0.0"\nedition = "2024"\n\n[workspace]\n',
                encoding="utf-8",
            )
            (project / "src/main.rs").write_text(
                '#[cfg(fn64_hostile_config)] compile_error!("ancestor Cargo config leaked");\nfn main() {}\n',
                encoding="utf-8",
            )
            supplied_cargo = Path(shutil.which("cargo") or "cargo")
            supplied_rustc = Path(shutil.which("rustc") or "rustc")
            cargo, rustc = artifacts.direct_rust_toolchain(supplied_cargo, supplied_rustc)
            cargo_home = root / "cargo-home"
            cargo_home.mkdir()
            target = root / "target"
            cc = Path(shutil.which("cc") or "/usr/bin/cc")
            cxx = Path(shutil.which("c++") or "/usr/bin/c++")
            environment = artifacts.validator_build_environment(
                cargo, rustc, cc, cxx, cargo_home, target
            )
            result = subprocess.run(
                [str(cargo), "check", "--offline", "--manifest-path", str(project / "Cargo.toml")],
                cwd=Path(root.anchor),
                env=environment,
                capture_output=True,
            )
            self.assertEqual(result.returncode, 0, result.stderr.decode(errors="replace"))

    def test_controlled_cargo_environment_drops_rustup_and_ambient_flags(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            cargo_home = root / "cargo-home"
            target = root / "target"
            tools = {name: Path("/usr/bin") / name for name in ("cargo", "rustc", "cc", "c++")}
            environment = artifacts.validator_build_environment(
                tools["cargo"], tools["rustc"], tools["cc"], tools["c++"], cargo_home, target
            )
            self.assertEqual(environment["CARGO_HOME"], str(cargo_home))
            self.assertEqual(environment["CARGO_TARGET_DIR"], str(target))
            self.assertEqual(environment["RUSTC"], str(tools["rustc"]))
            self.assertEqual(
                environment["CARGO_ENCODED_RUSTFLAGS"],
                f"--remap-path-prefix={root}=/fn64/validator-build",
            )
            self.assertNotIn("RUSTUP_HOME", environment)
            self.assertNotIn("RUSTFLAGS", environment)
            self.assertNotIn("CARGO_BUILD_RUSTFLAGS", environment)


class GitPinTests(unittest.TestCase):
    def repository(self, temporary: str) -> tuple[Path, str, Path]:
        root = Path(temporary) / "repository"
        root.mkdir()
        for command in (
            ["git", "init", "--quiet"],
            ["git", "config", "user.name", "fn64 test"],
            ["git", "config", "user.email", "fn64@example.invalid"],
            ["git", "config", "commit.gpgsign", "false"],
            ["git", "config", "core.filemode", "true"],
        ):
            subprocess.run(command, cwd=root, check=True, capture_output=True)
        tracked = root / "tracked.txt"
        tracked.write_bytes(b"pinned\n")
        subprocess.run(["git", "add", "tracked.txt"], cwd=root, check=True, capture_output=True)
        subprocess.run(["git", "commit", "--quiet", "-m", "pin"], cwd=root, check=True, capture_output=True)
        commit = subprocess.run(
            ["git", "rev-parse", "HEAD"], cwd=root, check=True, capture_output=True, text=True
        ).stdout.strip()
        return root, commit, tracked

    def validate(self, root: Path, commit: str) -> None:
        artifacts.validate_pinned_git_worktree(
            root,
            commit,
            "hostile fixture",
            allow_skip_worktree=False,
            maximum_file_bytes=1024,
        )

    def test_assume_unchanged_cannot_mask_mutated_bytes(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root, commit, tracked = self.repository(temporary)
            subprocess.run(
                ["git", "update-index", "--assume-unchanged", "tracked.txt"],
                cwd=root,
                check=True,
                capture_output=True,
            )
            tracked.write_bytes(b"hostile\n")
            with self.assertRaisesRegex(artifacts.ArtifactError, "index mask"):
                self.validate(root, commit)

    def test_post_build_worktree_blob_must_equal_pinned_git_blob(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root, commit, tracked = self.repository(temporary)
            tracked.write_bytes(b"hostile\n")
            with self.assertRaisesRegex(artifacts.ArtifactError, "pinned Git blob"):
                self.validate(root, commit)

    def test_post_build_worktree_mode_must_equal_pinned_git_mode(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root, commit, tracked = self.repository(temporary)
            tracked.chmod(0o755)
            with self.assertRaisesRegex(artifacts.ArtifactError, "executable mode"):
                self.validate(root, commit)

    def test_post_build_index_mode_drift_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root, commit, _ = self.repository(temporary)
            subprocess.run(
                ["git", "update-index", "--chmod=+x", "tracked.txt"],
                cwd=root,
                check=True,
                capture_output=True,
            )
            with self.assertRaisesRegex(artifacts.ArtifactError, "index blob/mode/tree"):
                self.validate(root, commit)

    def test_post_build_skip_worktree_drift_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root, commit, _ = self.repository(temporary)
            subprocess.run(
                ["git", "update-index", "--skip-worktree", "tracked.txt"],
                cwd=root,
                check=True,
                capture_output=True,
            )
            with self.assertRaisesRegex(artifacts.ArtifactError, "index mask"):
                self.validate(root, commit)

    def test_post_build_sparse_configuration_drift_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root, commit, _ = self.repository(temporary)
            subprocess.run(
                ["git", "config", "core.sparseCheckout", "true"],
                cwd=root,
                check=True,
                capture_output=True,
            )
            with self.assertRaisesRegex(artifacts.ArtifactError, "complete, non-sparse"):
                artifacts.validate_complete_pinned_git_worktree(
                    root,
                    commit,
                    "post-build fixture",
                    maximum_file_bytes=1024,
                )

    def test_replacement_ref_is_rejected_even_when_git_replacement_is_disabled(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root, commit, tracked = self.repository(temporary)
            tracked.write_bytes(b"replacement\n")
            subprocess.run(["git", "add", "tracked.txt"], cwd=root, check=True, capture_output=True)
            subprocess.run(
                ["git", "commit", "--quiet", "-m", "replacement"],
                cwd=root,
                check=True,
                capture_output=True,
            )
            replacement = subprocess.run(
                ["git", "rev-parse", "HEAD"], cwd=root, check=True, capture_output=True, text=True
            ).stdout.strip()
            subprocess.run(
                ["git", "replace", commit, replacement],
                cwd=root,
                check=True,
                capture_output=True,
            )
            with self.assertRaisesRegex(artifacts.ArtifactError, "replacement refs"):
                artifacts.validate_pinned_git_worktree(
                    root,
                    replacement,
                    "replacement fixture",
                    allow_skip_worktree=False,
                    maximum_file_bytes=1024,
                )


class ReceiptMutationTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.validator_temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.validator = Path(self.validator_temporary.name) / "validator"
        self.validator.write_text(
            """#!/usr/bin/env python3
import json, pathlib, sys
arguments = sys.argv[1:]
shader = pathlib.Path(arguments[arguments.index('--shader') + 1])
stage = arguments[arguments.index('--stage') + 1]
entry = arguments[arguments.index('--entry') + 1]
print(json.dumps({'schema':'fn64.wgpu-shader-validation.v1','status':'passed','wgpu_major':30,'stage':stage,'entry':entry,'module_bytes':shader.stat().st_size}, separators=(',', ':')))
""",
            encoding="utf-8",
        )
        self.validator.chmod(self.validator.stat().st_mode | stat.S_IXUSR)
        self.validator_record = {"identity": "synthetic-validator"}
        self.validator_build = artifacts.add_receipt_hash({
            "schema": artifacts.VALIDATOR_BUILD_SCHEMA,
            "binary_sha256": artifacts.digest_file(self.validator),
        })
        self.build = artifacts.add_receipt_hash({
            "schema": artifacts.BUILD_SCHEMA,
            "compiler_sha256": "c" * 64,
        })
        self.expected = {
            "id": "fixture",
            "port_cmake_line": 1,
            "oracle_cmake_line": 1,
            "cmake_function": "build_compute_shader",
            "source": "src/shaders/Fixture.hlsl",
            "output_name": "src/shaders/Fixture.hlsl",
            "stage": "compute",
            "entry": "CSMain",
            "profile": "cs_6_3",
            "flags": ["-spirv", "-E", "CSMain", "-T", "cs_6_3"],
            "preprocessed_artifact": "preprocessed/Fixture.pp.hlsl",
            "dependency_manifest_artifact": "dependencies/Fixture.json",
            "spirv_artifact": "spirv/Fixture.spv",
            "dependency_files": ["src/shaders/Fixture.hlsl"],
            "dependency_set_sha256": "d" * 64,
        }
        source_digest = artifacts.digest_bytes(b"fixture source")
        self.denominator = {
            "denominator_sha256": "e" * 64,
            "entries": [self.expected],
            "source_files": [{"path": self.expected["source"], "port_sha256": source_digest}],
        }
        self.preprocessed = self.root / self.expected["preprocessed_artifact"]
        self.dependency_manifest = self.root / self.expected["dependency_manifest_artifact"]
        self.spirv = self.root / self.expected["spirv_artifact"]
        for path in (self.preprocessed, self.dependency_manifest, self.spirv):
            path.parent.mkdir(parents=True, exist_ok=True)
        self.preprocessed.write_bytes(b"preprocessed fixture\n")
        dependency_files = [{"path": self.expected["source"], "sha256": source_digest}]
        dependency_set_sha256 = artifacts.digest_bytes(artifacts.canonical_json(dependency_files))
        self.dependency_manifest.write_bytes(artifacts.pretty_json({
            "schema": "fn64.dxc-active-include-closure.v1",
            "entry": "fixture",
            "files": dependency_files,
            "dependency_set_sha256": dependency_set_sha256,
        }))
        self.spirv.write_bytes(artifacts.SPIRV_MAGIC + b"\0" * 16)
        validation = artifacts.run_wgpu_validation(self.validator, self.spirv, self.expected)
        row = {
            **self.expected,
            "source_sha256": source_digest,
            "preprocessed_sha256": artifacts.digest_file(self.preprocessed),
            "preprocessed_bytes": self.preprocessed.stat().st_size,
            "dependency_manifest_sha256": artifacts.digest_file(self.dependency_manifest),
            "dependency_manifest_bytes": self.dependency_manifest.stat().st_size,
            "compiler_dependency_files": dependency_files,
            "compiler_dependency_set_sha256": dependency_set_sha256,
            "spirv_sha256": artifacts.digest_file(self.spirv),
            "spirv_bytes": self.spirv.stat().st_size,
            "compiler": {
                "flags": self.expected["flags"],
                "artifact_input": self.expected["preprocessed_artifact"],
                "preprocess_stdout_sha256": "1" * 64,
                "preprocess_stderr_sha256": "2" * 64,
                "compile_stdout_sha256": "3" * 64,
                "compile_stderr_sha256": "4" * 64,
                "built_in_spirv_validation": "passed",
            },
            "wgpu_validation": validation,
        }
        artifact_set = [{"path": self.expected["spirv_artifact"], "sha256": row["spirv_sha256"]}]
        policy = artifacts.load_policy()
        self.receipt = artifacts.add_receipt_hash({
            "schema": artifacts.RECEIPT_SCHEMA,
            "status": "complete",
            "producer_sha256": artifacts.digest_file(artifacts.TOOL_PATH),
            "policy_sha256": artifacts.digest_file(artifacts.POLICY_PATH),
            "denominator_sha256": self.denominator["denominator_sha256"],
            "source_snapshot": artifacts.source_snapshot_record(self.denominator),
            "dxc_build_receipt_sha256": self.build["receipt_sha256"],
            "dxc_compiler_sha256": self.build["compiler_sha256"],
            "validator_build_receipt_sha256": self.validator_build["receipt_sha256"],
            "wgpu_validator": self.validator_record,
            "required_validation": policy["spirv"]["required_validation"],
            "entries": [row],
            "artifact_set_sha256": artifacts.digest_bytes(artifacts.canonical_json(artifact_set)),
            "claim_boundary": "complete-local-artifact-integrity-not-transferable-process-attestation",
        })

    def tearDown(self) -> None:
        self.temporary.cleanup()
        self.validator_temporary.cleanup()

    def verify(self, receipt: dict | None = None) -> None:
        selected = receipt or self.receipt
        (self.root / "receipt.json").write_bytes(artifacts.pretty_json(selected))
        artifacts.validate_artifact_receipt(
            selected,
            self.denominator,
            self.root,
            self.build,
            self.validator_build,
            self.validator,
            self.validator_record,
        )

    def mutated(self, mutate) -> dict:
        receipt = copy.deepcopy(self.receipt)
        mutate(receipt)
        return artifacts.add_receipt_hash(receipt)

    def test_valid_fixture(self) -> None:
        self.verify()

    def test_receipt_self_hash_mutation_fails(self) -> None:
        receipt = copy.deepcopy(self.receipt)
        receipt["status"] = "tampered"
        with self.assertRaises(artifacts.ArtifactError):
            self.verify(receipt)

    def test_spirv_byte_mutation_fails(self) -> None:
        self.spirv.write_bytes(artifacts.SPIRV_MAGIC + b"\1" + b"\0" * 15)
        with self.assertRaises(artifacts.ArtifactError):
            self.verify()

    def test_preprocessed_input_mutation_fails(self) -> None:
        self.preprocessed.write_bytes(b"changed\n")
        with self.assertRaises(artifacts.ArtifactError):
            self.verify()

    def test_flag_mutation_fails_even_with_new_receipt_hash(self) -> None:
        receipt = self.mutated(lambda value: value["entries"][0]["flags"].append("-Vd"))
        with self.assertRaises(artifacts.ArtifactError):
            self.verify(receipt)

    def test_validator_transcript_mutation_fails(self) -> None:
        receipt = self.mutated(lambda value: value["entries"][0]["wgpu_validation"].update({"status": "failed"}))
        with self.assertRaises(artifacts.ArtifactError):
            self.verify(receipt)

    def test_unknown_receipt_field_fails(self) -> None:
        receipt = self.mutated(lambda value: value.update({"future_unreviewed_field": True}))
        with self.assertRaises(artifacts.ArtifactError):
            self.verify(receipt)

    def test_shrunk_entry_denominator_fails(self) -> None:
        receipt = self.mutated(lambda value: value.update({"entries": []}))
        with self.assertRaises(artifacts.ArtifactError):
            self.verify(receipt)

    def test_shrunk_dependency_closure_fails(self) -> None:
        def mutate(value: dict) -> None:
            value["entries"][0]["compiler_dependency_files"] = []
            value["entries"][0]["compiler_dependency_set_sha256"] = artifacts.digest_bytes(artifacts.canonical_json([]))

        with self.assertRaises(artifacts.ArtifactError):
            self.verify(self.mutated(mutate))

    def test_source_identity_mutation_fails(self) -> None:
        receipt = self.mutated(lambda value: value["entries"][0].update({"source_sha256": "0" * 64}))
        with self.assertRaises(artifacts.ArtifactError):
            self.verify(receipt)

    def test_unexpected_artifact_file_fails(self) -> None:
        (self.root / "unreviewed.bin").write_bytes(b"unreviewed")
        with self.assertRaises(artifacts.ArtifactError):
            self.verify()

    @unittest.skipUnless(hasattr(os, "link"), "hardlinks unavailable")
    def test_hardlinked_artifact_fails(self) -> None:
        self.dependency_manifest.unlink()
        os.link(self.preprocessed, self.dependency_manifest)
        with self.assertRaises(artifacts.ArtifactError):
            self.verify()


if __name__ == "__main__":
    unittest.main()
