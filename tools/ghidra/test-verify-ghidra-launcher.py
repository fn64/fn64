#!/usr/bin/env python3

from __future__ import annotations

from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]
VERIFIER = ROOT / "tools/ghidra/verify-ghidra-launcher.py"


class GhidraLauncherTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory(prefix="fn64-ghidra-launcher-")
        self.root = Path(self.temporary.name)
        self.installs = [self.make_install("a"), self.make_install("b")]

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def make_install(self, name: str) -> Path:
        install = self.root / name
        (install / "support").mkdir(parents=True)
        (install / "Ghidra").mkdir()
        (install / "support/analyzeHeadless").write_text("#!/bin/sh\n", encoding="utf-8")
        (install / "support/analyzeHeadless").chmod(0o755)
        (install / "ghidraRun").write_text("#!/bin/sh\n", encoding="utf-8")
        (install / "ghidraRun").chmod(0o755)
        (install / "Ghidra/application.properties").write_text(
            "application.version=fixture\n", encoding="utf-8"
        )
        return install

    def verify(self, install: Path, headless: Path) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [sys.executable, str(VERIFIER), str(install), str(headless)],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )

    def test_accepts_launcher_from_selected_install(self) -> None:
        result = self.verify(self.installs[0], self.installs[0] / "support/analyzeHeadless")
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_rejects_launcher_from_another_install(self) -> None:
        result = self.verify(self.installs[0], self.installs[1] / "support/analyzeHeadless")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("does not belong", result.stderr)

    def test_rejects_symlink_launcher(self) -> None:
        alias = self.root / "headless-alias"
        alias.symlink_to(self.installs[0] / "support/analyzeHeadless")
        result = self.verify(self.installs[0], alias)
        self.assertNotEqual(result.returncode, 0)

    def test_accepts_gui_launcher_from_selected_install(self) -> None:
        result = subprocess.run(
            [
                sys.executable,
                str(VERIFIER),
                str(self.installs[0]),
                str(self.installs[0] / "ghidraRun"),
                "ghidraRun",
            ],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_every_vw_launcher_uses_the_distribution_gate(self) -> None:
        invokers = {
            path.name
            for path in (ROOT / "tools/ghidra").glob("*.sh")
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
        for name in invokers:
            source = (ROOT / "tools/ghidra" / name).read_text(encoding="utf-8")
            self.assertIn("verify-ghidra-launcher.py", source, name)


if __name__ == "__main__":
    unittest.main()
