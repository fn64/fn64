#!/usr/bin/env python3
"""Focused orchestration test for run-snapshot-loader-ab.sh."""

from __future__ import annotations

import hashlib
import json
from pathlib import Path
import shutil
import stat
import subprocess
import tempfile
import textwrap
import unittest
import zipfile


ROOT = Path(__file__).resolve().parents[2]
RUNNER = ROOT / "tools/ghidra/run-snapshot-loader-ab.sh"
COMPARATOR = ROOT / "tools/ghidra/compare-snapshot-loader-ab.py"


def executable(path: Path, body: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(textwrap.dedent(body).lstrip(), encoding="utf-8")
    path.chmod(path.stat().st_mode | stat.S_IXUSR)


class SnapshotLoaderAbRunnerTest(unittest.TestCase):
    def test_shared_synthesized_context_sequential_lanes_and_no_ingest(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            repo = root / "fixture-repo"
            workspace = root / "workspace"
            workspace.mkdir(mode=0o700)
            tools = repo / "tools/ghidra"
            scripts = repo / "scripts"
            tools.mkdir(parents=True)
            scripts.mkdir()

            runner = tools / RUNNER.name
            shutil.copy2(RUNNER, runner)
            runner.chmod(0o700)
            shutil.copy2(COMPARATOR, tools / COMPARATOR.name)
            (tools / COMPARATOR.name).chmod(0o700)
            (tools / "Fn64ExportLoaderComparison.java").write_text(
                "// fixture exporter identity\n", encoding="utf-8"
            )
            (tools / "n64loaderwv-source-policy.json").write_text("{}\n", encoding="utf-8")
            (tools / "n64loaderwv-artifact-policy.json").write_text("{}\n", encoding="utf-8")

            executable(
                scripts / "memory-guard.zsh",
                r"""
                #!/bin/sh
                set -eu
                printf '%s\n' '{"event":"fixture"}' > "$FN64_GUARD_JSONL"
                exec "$@"
                """,
            )
            executable(
                tools / "manifest-ghidra-distribution.py",
                r"""
                #!/usr/bin/env python3
                import pathlib, sys
                if sys.argv[1] == "scan":
                    pathlib.Path(sys.argv[4]).write_text('{"fixture":true}\n')
                elif sys.argv[1] != "verify":
                    raise SystemExit(2)
                """,
            )
            executable(
                tools / "verify-n64loaderwv-provenance.py",
                r"""
                #!/usr/bin/env python3
                import hashlib, json, pathlib, sys
                if len(sys.argv) != 6 or sys.argv[1] != "artifact":
                    raise SystemExit(2)
                _artifact_policy, policy, receipt, extension = map(pathlib.Path, sys.argv[2:])
                value = {
                    "repository": "fn64/N64LoaderWV",
                    "policy_sha256": hashlib.sha256(policy.read_bytes()).hexdigest(),
                    "commit": "11" * 20,
                    "tree": "22" * 20,
                    "source_archive_sha256": "33" * 32,
                    "extension_sha256": hashlib.sha256(extension.read_bytes()).hexdigest(),
                    "conformance_receipt_sha256": hashlib.sha256(receipt.read_bytes()).hexdigest(),
                }
                print(json.dumps(value, sort_keys=True))
                """,
            )
            executable(
                tools / "verify-ghidra-launcher.py",
                r"""
                #!/usr/bin/env python3
                import pathlib, sys
                if len(sys.argv) != 3:
                    raise SystemExit(2)
                install, headless = map(pathlib.Path, sys.argv[1:])
                if headless.resolve() != (install / "support/analyzeHeadless").resolve():
                    raise SystemExit(2)
                """,
            )
            executable(
                tools / "verify-n64loaderwv-install.py",
                r"""
                #!/usr/bin/env python3
                import hashlib, json, pathlib, sys
                jar = pathlib.Path(sys.argv[2]) / "lib/N64LoaderWV.jar"
                data = jar.read_bytes()
                print(json.dumps({
                    "schema": "fn64.n64loaderwv-install-verification",
                    "schema_version": 1,
                    "extension_root": "N64LoaderWV",
                    "loader_jar": {"byte_length": len(data), "sha256": hashlib.sha256(data).hexdigest()},
                    "loader_class": {"byte_length": 1, "sha256": hashlib.sha256(b"c").hexdigest()},
                }, sort_keys=True))
                """,
            )
            (tools / "Fn64VerifyN64LoaderRuntime.java").write_text(
                "// fixture runtime verifier identity\n", encoding="utf-8"
            )
            stage = root / "stage-snapshot-bank.py"
            executable(
                stage,
                r"""
                #!/usr/bin/env python3
                import hashlib, json, pathlib, shutil, sys
                if len(sys.argv) != 8 or sys.argv[1] != "--discovery-only":
                    raise SystemExit(2)
                _, snapshot, bank, materialized, _workspace, output, evidence = sys.argv[1:]
                source = pathlib.Path(materialized)
                shutil.copyfile(source, output)
                data = source.read_bytes()
                value = {
                    "schema": "fn64.snapshot-bank-evidence",
                    "schema_version": 3,
                    "program_snapshot_sha256": hashlib.sha256(pathlib.Path(snapshot).read_bytes()).hexdigest(),
                    "input": {
                        "normalized_rom_sha256": "44" * 32,
                        "bank": bank,
                        "bank_bytes_sha256": hashlib.sha256(data).hexdigest(),
                        "mapping_sha256": "55" * 32,
                        "va_start": 0x80001000,
                        "va_end": 0x80001000 + len(data),
                    },
                    "backing": {},
                    "artifact": {"byte_length": len(data), "sha256": hashlib.sha256(data).hexdigest()},
                    "seeds": {"mode": "discovery_only", "role": "candidate_only"},
                }
                pathlib.Path(evidence).write_text(json.dumps(value, sort_keys=True) + "\n")
                """,
            )

            ghidra = root / "ghidra"
            (ghidra / "Ghidra").mkdir(parents=True)
            (ghidra / "support").mkdir()
            (ghidra / "Ghidra/application.properties").write_text(
                "application.version=11.3\napplication.release.name=PUBLIC\n", encoding="utf-8"
            )
            witness = root / "headless-witness.jsonl"
            executable(
                ghidra / "support/analyzeHeadless",
                f'''\
                #!/usr/bin/env python3
                import hashlib, json, pathlib, struct, sys
                args = sys.argv[1:]
                loader = args[args.index("-loader") + 1]
                lane = "binary-loader" if loader == "BinaryLoader" else "n64loaderwv"
                imported = args[args.index("-import") + 1]
                rdram = imported if lane == "binary-loader" else args[args.index("-loader-rdram") + 1]
                record = {{"lane": lane, "import": imported, "rdram": rdram}}
                with open({str(witness)!r}, "a", encoding="utf-8") as stream:
                    stream.write(json.dumps(record, sort_keys=True) + "\\n")
                empty_digest = hashlib.sha256(
                    b"fn64.ghidra-bank-function-inventory.v1\\0" + struct.pack("<Q", 0)
                ).hexdigest()
                empty_rejected_digest = hashlib.sha256(
                    b"fn64.ghidra-bank-rejected-functions.v1\\0" + struct.pack("<Q", 0)
                ).hexdigest()
                for marker, expected_phase in (("-preScript", "pre"), ("-postScript", "post")):
                    index = args.index(marker)
                    script_args = args[index + 2:index + 16]
                    if len(script_args) != 14:
                        raise SystemExit("wrong exporter argument count")
                    (output, actual_lane, phase, bank, va_start, va_end, context_start,
                     context_end, rom_sha, bank_sha, context_sha, mapping_sha,
                     provenance_sha, program_name) = script_args
                    if actual_lane != lane or phase != expected_phase:
                        raise SystemExit("wrong exporter lane/phase order")
                    if program_name != pathlib.Path(imported).name:
                        raise SystemExit("wrong program name")
                    value = {{
                        "schema": "fn64.ghidra-bank-function-inventory",
                        "schema_version": 4,
                        "candidate_only": True,
                        "provenance": {{"lane": lane, "phase": phase, "source_sha256": provenance_sha}},
                        "input": {{
                            "normalized_rom_sha256": rom_sha, "bank": bank,
                            "bank_bytes_sha256": bank_sha, "context_bytes_sha256": context_sha,
                            "mapping_sha256": mapping_sha, "va_start": int(va_start),
                            "va_end": int(va_end), "context_start": int(context_start),
                            "context_end": int(context_end),
                        }},
                        "memory_blocks": [{{
                            "va_start": int(context_start), "va_end": int(context_end),
                            "overlap_start": int(context_start), "overlap_end": int(context_end),
                            "read": True, "write": True, "execute": True, "initialized": True,
                        }}],
                        "entry_point_count": 0,
                        "entry_points_sha256": hashlib.sha256(
                            b"fn64.ghidra-bank-entry-points.v1\\0" + struct.pack("<Q", 0)
                        ).hexdigest(),
                        "entry_points": [],
                        "rejected_function_count": 0,
                        "rejected_functions_sha256": empty_rejected_digest,
                        "rejected_functions": [],
                        "function_count": 0, "function_inventory_sha256": empty_digest,
                        "functions": [],
                    }}
                    pathlib.Path(output).write_text(
                        json.dumps(value, sort_keys=True, separators=(",", ":")) + "\\n"
                    )
                if lane == "n64loaderwv":
                    runtime_index = args.index("Fn64VerifyN64LoaderRuntime.java")
                    pathlib.Path(args[runtime_index + 1]).write_text(
                        '{{"schema":"fn64.n64loaderwv-runtime-verification.v1"}}\\n'
                    )
                if lane == "binary-loader":
                    print("Using Loader: Raw Binary")
                    print("Using Language/Compiler: MIPS:BE:64:64-32addr:o32")
                else:
                    print("Using Loader: N64 Loader by Warranty Voider")
                ''',
            )

            jdk = root / "jdk/bin"
            executable(jdk / "java", "#!/bin/sh\nexit 0\n")
            executable(jdk / "jar", "#!/bin/sh\nexit 0\n")

            extension = root / "N64LoaderWV.zip"
            with zipfile.ZipFile(extension, "w") as archive:
                archive.writestr("N64LoaderWV/extension.properties", "name=N64LoaderWV\n")
                archive.writestr("N64LoaderWV/lib/N64LoaderWV.jar", b"fixture")
            receipt = root / "conformance.json"
            receipt.write_text("{}\n", encoding="utf-8")
            snapshot = root / "snapshot.json"
            snapshot.write_text("{}\n", encoding="utf-8")
            bank_bytes = bytes(range(64))
            materialized = root / "bank.bin"
            materialized.write_bytes(bank_bytes)

            completed = subprocess.run(
                [
                    str(runner), str(snapshot), "bank-a", str(materialized),
                    str(workspace), str(extension), str(receipt),
                ],
                env={
                    "PATH": "/usr/bin:/bin:/usr/sbin:/sbin",
                    "FN64_STAGE_SNAPSHOT_BANK": str(stage),
                    "GHIDRA_JAVA_HOME": str(jdk.parent),
                    "GHIDRA_INSTALL_DIR": str(ghidra),
                },
                text=True,
                capture_output=True,
                check=False,
            )
            diagnostic_text = "\n".join(
                f"{path.name}: {path.read_text(errors='replace')}"
                for path in workspace.rglob("*.log")
            )
            self.assertEqual(
                completed.returncode, 0, completed.stderr + "\n" + diagnostic_text
            )
            outputs = dict(
                line.split("=", 1) for line in completed.stdout.splitlines() if "=" in line
            )
            attempt = Path(outputs["attempt"])
            records = [json.loads(line) for line in witness.read_text().splitlines()]
            self.assertEqual([record["lane"] for record in records], ["binary-loader", "n64loaderwv"])
            self.assertEqual(records[0]["rdram"], records[1]["rdram"])
            rdram = Path(records[0]["rdram"])
            self.assertEqual(rdram.stat().st_size, 4 * 1024 * 1024)
            with rdram.open("rb") as stream:
                stream.seek(0x1000)
                self.assertEqual(stream.read(len(bank_bytes)), bank_bytes)
            self.assertEqual(Path(records[0]["import"]), rdram)
            self.assertEqual(Path(records[1]["import"]).suffix, ".z64")
            report = json.loads(Path(outputs["comparison"]).read_text())
            self.assertEqual(report["authority"], "candidate_only")
            receipt_value = json.loads(Path(outputs["receipt"]).read_text())
            self.assertFalse(receipt_value["production_ingest_performed"])
            self.assertEqual(receipt_value["completed_lanes"], ["binary-loader", "n64loaderwv"])
            self.assertTrue(
                all(
                    not any((attempt / "lanes" / lane / "project").iterdir())
                    for lane in ("binary", "n64")
                )
            )

            source = RUNNER.read_text(encoding="utf-8")
            self.assertNotIn("Fn64SeedFunctions.java", source)
            self.assertNotIn("ingest_tool_claims", source)
            self.assertIn("--discovery-only", source)


if __name__ == "__main__":
    unittest.main()
