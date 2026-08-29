#!/usr/bin/env python3
"""Game-free regression tests for pgo-release.py."""

from __future__ import annotations

import importlib.util
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).resolve().with_name("pgo-release.py")
SPEC = importlib.util.spec_from_file_location("pgo_release", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
PGO = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = PGO
SPEC.loader.exec_module(PGO)


class PgoFixture:
    def __init__(self, root: Path):
        self.root = root
        self.source = root / "source-input.txt"
        self.source.write_text("source-v1\n")
        self.cargo = root / "cargo"
        self.rustc = root / "rustc"
        self.profdata = root / "llvm-profdata"
        self.cargo.write_text(self.cargo_script())
        self.rustc.write_text(
            f"#!{sys.executable}\nimport sys\nassert sys.argv[1:]==['-vV']\nprint('rustc 1.99.0')\nprint('host: fake-target')\nprint('LLVM version: 99.0.0')\n"
        )
        self.profdata.write_text(self.profdata_script())
        for path in (self.cargo, self.rustc, self.profdata):
            path.chmod(0o700)
        self.manifest = root / "pgo.json"
        self.write_manifest()

    def cargo_script(self) -> str:
        artifact_source = f"""#!{sys.executable}
import os,sys
from pathlib import Path
template=os.environ.get('LLVM_PROFILE_FILE')
if template:
 path=Path(template.replace('%m','module').replace('%p',str(os.getpid())))
 path.write_bytes(('profile:'+sys.argv[1]).encode())
print(__FLAGS__)
"""
        return f'''#!{sys.executable}
import os,sys
from pathlib import Path
if sys.argv[1:]==['--version','--verbose']:
 print('cargo 1.99.0'); print('release: fake'); raise SystemExit(0)
assert sys.argv[1]=='build' and '--release' in sys.argv and '--target' in sys.argv
target=sys.argv[sys.argv.index('--target')+1]
root=Path(os.environ['CARGO_TARGET_DIR'])/target/'release'; root.mkdir(parents=True)
flags=os.environ.get('CARGO_ENCODED_RUSTFLAGS','').split(chr(31))
artifact=root/'game'
artifact.write_text({artifact_source!r}.replace('__FLAGS__',repr(flags)))
artifact.chmod(0o700)
'''

    def profdata_script(self) -> str:
        return f"""#!{sys.executable}
import sys
from pathlib import Path
if sys.argv[1:]==['--version']:
 print('llvm-profdata fake 99.0.0'); raise SystemExit(0)
assert sys.argv[1]=='merge' and sys.argv[2]=='-o'
output=Path(sys.argv[3]); inputs=[Path(item) for item in sys.argv[4:]]
output.write_bytes(b'merged:' + b'|'.join(path.read_bytes() for path in inputs))
"""

    def manifest_value(self) -> dict[str, object]:
        return {
            "schema": PGO.MANIFEST_SCHEMA,
            "schema_version": 1,
            "profile_id": "fixture-routes-v1",
            "target": "fake-target",
            "toolchain": {
                "cargo": [str(self.cargo)],
                "rustc": [str(self.rustc)],
            },
            "build": {
                "arguments": [
                    "build",
                    "--release",
                    "--locked",
                    "--target",
                    "{target}",
                ],
                "cwd": str(self.root),
                "artifact": "{target_dir}/{target}/release/game",
                "rustflags": ["-Ccodegen-units=1"],
                "environment": {},
                "inherit_environment": [],
            },
            "training": [
                {
                    "id": "entrance",
                    "command": ["{artifact}", "entrance"],
                    "cwd": str(self.root),
                    "environment": {"FIXTURE_ROUTE": "entrance"},
                    "inherit_environment": [],
                },
                {
                    "id": "shell",
                    "command": ["{artifact}", "shell"],
                    "cwd": str(self.root),
                    "environment": {"FIXTURE_ROUTE": "shell"},
                    "inherit_environment": [],
                },
            ],
            "identity_files": [{"id": "source", "path": str(self.source)}],
        }

    def write_manifest(self, mutate=None) -> None:
        value = self.manifest_value()
        if mutate is not None:
            mutate(value)
        self.manifest.write_bytes(PGO.canonical_json(value))

    def command(self, operation: str, output: Path, *, include_profdata: bool = True) -> list[str]:
        command = [
            sys.executable,
            str(SCRIPT),
            operation,
            "--manifest",
            str(self.manifest),
            "--output-dir",
            str(output),
            "--timeout-seconds",
            "10",
        ]
        if include_profdata:
            command.extend(["--llvm-profdata", str(self.profdata)])
        return command

    def run(
        self,
        operation: str,
        output: Path,
        *,
        ok: bool = True,
        env=None,
        include_profdata: bool = True,
    ):
        clean_env = os.environ.copy()
        for key in PGO.CONTROLLED_ENVIRONMENT:
            clean_env.pop(key, None)
        if env:
            clean_env.update(env)
        result = subprocess.run(
            self.command(operation, output, include_profdata=include_profdata),
            capture_output=True,
            text=True,
            timeout=30,
            env=clean_env,
        )
        if ok and result.returncode != 0:
            raise AssertionError(result.stderr)
        if not ok and result.returncode == 0:
            raise AssertionError(result.stdout)
        return result


class PgoReleaseTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="fn64-pgo-test-")
        self.root = Path(self.temp.name).resolve()
        self.fixture = PgoFixture(self.root)

    def tearDown(self):
        self.temp.cleanup()

    def read(self, path: Path) -> dict[str, object]:
        return json.loads(path.read_text())

    def test_all_builds_two_explicit_routes_and_profile_use_artifact(self):
        output = self.root / "all-output"
        result = self.fixture.run("all", output)
        self.assertIn("trained profile=fixture-routes-v1", result.stdout)
        self.assertIn("built profile-use artifact", result.stdout)
        raws = sorted(path.name for path in (output / "raw").glob("*.profraw"))
        self.assertEqual(len(raws), 2)
        self.assertTrue(raws[0].startswith("entrance-"))
        self.assertTrue(raws[1].startswith("shell-"))
        profile = self.read(output / "profile-receipt.json")
        self.assertEqual(profile["training_ids"], ["entrance", "shell"])
        self.assertEqual(profile["identity_files"][0]["id"], "source")
        build = self.read(output / "profile-use-build-receipt.json")
        self.assertEqual(build["mode"], "profile_use")
        self.assertEqual(build["profile_receipt_sha256"], profile["receipt_sha256"])
        artifact = output / "profile-use-target/fake-target/release/game"
        text = artifact.read_text()
        self.assertIn("-Cprofile-use=", text)
        self.assertIn("-pgo-warn-missing-function", text)

    def test_ordinary_build_has_no_profile_flag_and_needs_no_profile(self):
        output = self.root / "ordinary-output"
        self.fixture.run("ordinary", output, include_profdata=False)
        artifact = output / "ordinary-target/fake-target/release/game"
        text = artifact.read_text()
        self.assertNotIn("profile-generate", text)
        self.assertNotIn("profile-use", text)
        receipt = self.read(output / "ordinary-build-receipt.json")
        self.assertEqual(receipt["mode"], "ordinary")
        self.assertIsNone(receipt["profile_receipt_sha256"])

    def test_optimize_rejects_changed_source_denominator(self):
        output = self.root / "changed-source"
        self.fixture.run("train", output)
        self.fixture.source.write_text("source-v2\n")
        result = self.fixture.run("optimize", output, ok=False)
        self.assertIn("identity files changed", result.stderr)
        self.assertFalse((output / "profile-use-target").exists())

    def test_optimize_rejects_tampered_merged_profile(self):
        output = self.root / "tampered-profile"
        self.fixture.run("train", output)
        (output / "merged.profdata").write_bytes(b"not the retained profile")
        result = self.fixture.run("optimize", output, ok=False)
        self.assertIn("merged profile bytes do not match", result.stderr)

    def test_optimize_rejects_changed_compiler(self):
        output = self.root / "changed-compiler"
        self.fixture.run("train", output)
        self.fixture.rustc.write_text(
            f"#!{sys.executable}\nimport sys\nprint('rustc 2.0.0')\n"
        )
        self.fixture.rustc.chmod(0o700)
        result = self.fixture.run(
            "optimize", output, ok=False, include_profdata=False
        )
        self.assertIn("compiler identity differs", result.stderr)

    def test_optimize_rejects_changed_inherited_build_environment(self):
        self.fixture.write_manifest(
            lambda value: value["build"].update(  # type: ignore[union-attr]
                {"inherit_environment": ["PGO_FIXTURE_BUILD_ID"]}
            )
        )
        output = self.root / "changed-build-env"
        self.fixture.run("train", output, env={"PGO_FIXTURE_BUILD_ID": "one"})
        result = self.fixture.run(
            "optimize",
            output,
            ok=False,
            env={"PGO_FIXTURE_BUILD_ID": "two"},
            include_profdata=False,
        )
        self.assertIn("build environment differs", result.stderr)

    def test_optimize_rejects_manifest_corpus_change(self):
        output = self.root / "changed-corpus"
        self.fixture.run("train", output)
        self.fixture.write_manifest(
            lambda value: value["training"].reverse()  # type: ignore[union-attr]
        )
        result = self.fixture.run("optimize", output, ok=False)
        self.assertIn("different manifest", result.stderr)

    def test_training_requires_each_declared_route_to_emit_profile(self):
        self.fixture.write_manifest(
            lambda value: value["training"][1].update(  # type: ignore[index,union-attr]
                {"command": [sys.executable, "-c", "pass"]}
            )
        )
        output = self.root / "missing-route"
        result = self.fixture.run("train", output, ok=False)
        self.assertIn("shell emitted no new .profraw", result.stderr)

    def test_ambient_profile_flags_fail_loudly(self):
        output = self.root / "ambient-flags"
        result = self.fixture.run(
            "ordinary", output, ok=False, env={"RUSTFLAGS": "-Ctarget-cpu=native"}
        )
        self.assertIn("ambient environment contains workflow-owned", result.stderr)

    def test_build_target_must_match_declared_target(self):
        self.fixture.write_manifest(
            lambda value: value["build"]["arguments"].__setitem__(  # type: ignore[index,union-attr]
                4, "wrong-target"
            )
        )
        result = self.fixture.run("ordinary", self.root / "wrong-target", ok=False)
        self.assertIn("must equal the manifest target", result.stderr)

    def test_artifact_must_be_in_isolated_target(self):
        self.fixture.write_manifest(
            lambda value: value["build"].update(  # type: ignore[union-attr]
                {"artifact": str(self.root / "game")}
            )
        )
        result = self.fixture.run("ordinary", self.root / "bad-artifact", ok=False)
        self.assertIn("must resolve inside", result.stderr)

    def test_output_inside_fn64_repository_is_rejected(self):
        result = self.fixture.run("ordinary", PGO.REPO_ROOT / "pgo-test-output", ok=False)
        self.assertIn("must be outside the fn64 repository", result.stderr)


if __name__ == "__main__":
    unittest.main()
