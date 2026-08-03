#!/usr/bin/env python3

from __future__ import annotations

import hashlib
from io import BytesIO
import json
from pathlib import Path
import stat
import subprocess
import tempfile
import unittest
import zipfile


ROOT = Path(__file__).resolve().parents[2]
RUNNER = ROOT / "tools/ghidra/run-n64loaderwv-gui.sh"


class N64LoaderWVGuiRunnerTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory(prefix="fn64-n64loaderwv-gui-")
        self.root = Path(self.temporary.name)
        self.repo = self.root / "repo"
        tools = self.repo / "tools/ghidra"
        tools.mkdir(parents=True)
        (self.repo / "scripts").mkdir()
        self.runner = tools / RUNNER.name
        self.runner.write_bytes(RUNNER.read_bytes())
        self.runner.chmod(0o755)
        install_verifier = tools / "verify-n64loaderwv-install.py"
        install_verifier.write_bytes(
            (ROOT / "tools/ghidra/verify-n64loaderwv-install.py").read_bytes()
        )
        install_verifier.chmod(0o755)
        (tools / "n64loaderwv-source-policy.json").write_text("{}\n", encoding="utf-8")
        (tools / "n64loaderwv-artifact-policy.json").write_text("{}\n", encoding="utf-8")

        self.extension = self.root / "extension.zip"
        jar_stream = BytesIO()
        with zipfile.ZipFile(jar_stream, "w") as jar:
            jar.writestr(
                "n64loaderwv/N64LoaderWVLoader.class", b"fixture loader class"
            )
        with zipfile.ZipFile(self.extension, "w") as archive:
            archive.writestr("N64LoaderWV/extension.properties", "name=N64LoaderWV\n")
            archive.writestr("N64LoaderWV/lib/N64LoaderWV.jar", jar_stream.getvalue())
        self.extension_sha = hashlib.sha256(self.extension.read_bytes()).hexdigest()
        self.receipt = self.root / "receipt.json"
        self.receipt.write_text("{}\n", encoding="utf-8")

        verifier = tools / "verify-n64loaderwv-provenance.py"
        verifier.write_text(
            "#!/usr/bin/env python3\n"
            "import hashlib,json,pathlib,sys\n"
            f"expected={self.extension_sha!r}\n"
            "actual=hashlib.sha256(pathlib.Path(sys.argv[5]).read_bytes()).hexdigest()\n"
            "if actual != expected: raise SystemExit(2)\n"
            "print(json.dumps({'extension_sha256': actual}))\n",
            encoding="utf-8",
        )
        verifier.chmod(0o755)
        launcher_verifier = tools / "verify-ghidra-launcher.py"
        launcher_verifier.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
        launcher_verifier.chmod(0o755)

        self.ghidra = self.root / "ghidra"
        (self.ghidra / "Ghidra").mkdir(parents=True)
        (self.ghidra / "Ghidra/application.properties").write_text(
            "application.version=12.1.2\napplication.release.name=PUBLIC\n",
            encoding="utf-8",
        )
        gui = self.ghidra / "ghidraRun"
        gui.write_text(
            "#!/bin/sh\n"
            "{ printf 'HOME=%s\\n' \"$HOME\"; "
            "printf 'MAX=%s\\n' \"$GHIDRA_GUI_MAXMEM\"; "
            "printf 'OPTS=%s\\n' \"$GHIDRA_GUI_JAVA_OPTIONS\"; } > \"$1\"\n",
            encoding="utf-8",
        )
        gui.chmod(0o755)

        self.jdk = self.root / "jdk"
        (self.jdk / "bin").mkdir(parents=True)
        for name, body in (
            ("java", "#!/bin/sh\nexit 0\n"),
            ("jar", "#!/bin/sh\nprintf '%s\\n' n64loaderwv/N64LoaderWVLoader.class\n"),
        ):
            path = self.jdk / "bin" / name
            path.write_text(body, encoding="utf-8")
            path.chmod(0o755)

        self.profile = self.root / "profile"
        self.profile.mkdir(mode=0o700)
        self.record = self.root / "launch.txt"

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def run_gui(self, *extra: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [
                str(self.runner),
                str(self.profile),
                str(self.extension),
                str(self.receipt),
                *extra,
                str(self.record),
            ],
            env={
                "PATH": "/usr/bin:/bin",
                "GHIDRA_INSTALL_DIR": str(self.ghidra),
                "GHIDRA_JAVA_HOME": str(self.jdk),
            },
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )

    def test_materializes_isolated_approved_profile_and_launches_it(self) -> None:
        result = self.run_gui()
        self.assertEqual(result.returncode, 0, result.stderr)
        profile = self.profile.resolve() / f"n64loaderwv-{self.extension_sha}"
        installed = profile / "home/ghidra/ghidra_12.1.2_PUBLIC/Extensions/N64LoaderWV"
        self.assertTrue((installed / "extension.properties").is_file())
        self.assertTrue((installed / "lib/N64LoaderWV.jar").is_file())
        self.assertEqual((profile / "n64loaderwv-extension.zip").read_bytes(), self.extension.read_bytes())
        launch = self.record.read_text(encoding="utf-8")
        self.assertIn(f"HOME={profile / 'home'}", launch)
        self.assertIn("MAX=1G", launch)
        self.assertIn(f"-Dapplication.settingsdir={profile / 'home'}", launch)

        second = self.run_gui()
        self.assertEqual(second.returncode, 0, second.stderr)

    def test_rejects_a_changed_installed_artifact(self) -> None:
        self.assertEqual(self.run_gui().returncode, 0)
        installed_zip = (
            self.profile / f"n64loaderwv-{self.extension_sha}/n64loaderwv-extension.zip"
        )
        installed_zip.write_bytes(b"changed")
        result = self.run_gui()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("does not match the approved fork", result.stderr)

    def test_rejects_a_changed_extracted_loader_jar(self) -> None:
        self.assertEqual(self.run_gui().returncode, 0)
        installed_jar = (
            self.profile
            / f"n64loaderwv-{self.extension_sha}"
            / "home/ghidra/ghidra_12.1.2_PUBLIC/Extensions/N64LoaderWV/lib/N64LoaderWV.jar"
        )
        installed_jar.write_bytes(b"changed")
        result = self.run_gui()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("does not exactly match the approved fn64 fork", result.stderr)

    def test_prepare_only_materializes_without_launching(self) -> None:
        result = self.run_gui("--prepare-only")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("prepared approved fn64 fork", result.stdout)
        self.assertFalse(self.record.exists())


if __name__ == "__main__":
    unittest.main()
