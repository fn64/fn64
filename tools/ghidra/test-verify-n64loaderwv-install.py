#!/usr/bin/env python3

from __future__ import annotations

from io import BytesIO
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
import warnings
import zipfile


ROOT = Path(__file__).resolve().parents[2]
VERIFIER = ROOT / "tools/ghidra/verify-n64loaderwv-install.py"
LOADER_CLASS = "n64loaderwv/N64LoaderWVLoader.class"


def jar_with(*entries: tuple[str, bytes]) -> bytes:
    stream = BytesIO()
    with zipfile.ZipFile(stream, "w") as archive:
        for name, data in entries:
            archive.writestr(name, data)
    return stream.getvalue()


class N64LoaderWVInstallVerifierTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory(prefix="fn64-vw-install-")
        self.root = Path(self.temporary.name)
        self.ghidra = self.root / "ghidra"
        self.profile = self.root / "profile"
        self.extension = self.profile / "Extensions/N64LoaderWV"
        self.ghidra.mkdir()
        self.extension.mkdir(parents=True)
        self.loader_class = b"approved loader bytecode"
        self.loader_jar = jar_with(
            (LOADER_CLASS, self.loader_class), ("n64loaderwv/Helper.class", b"helper")
        )
        self.archive = self.root / "approved.zip"
        self.write_archive(
            [
                ("N64LoaderWV/extension.properties", b"name=N64LoaderWV\n"),
                ("N64LoaderWV/lib/N64LoaderWV.jar", self.loader_jar),
                ("N64LoaderWV/data/value.txt", b"value\n"),
            ]
        )
        self.extract_archive()

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def write_archive(self, entries: list[tuple[str, bytes]]) -> None:
        with zipfile.ZipFile(self.archive, "w") as archive:
            for name, data in entries:
                archive.writestr(name, data)

    def extract_archive(self) -> None:
        with zipfile.ZipFile(self.archive) as archive:
            archive.extractall(self.profile / "Extensions")

    def run_verifier(self) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [
                sys.executable,
                str(VERIFIER),
                str(self.archive),
                str(self.extension),
                str(self.ghidra),
                str(self.profile),
            ],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )

    def assert_rejected(self, message: str) -> None:
        result = self.run_verifier()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(message, result.stderr)

    def test_accepts_exact_isolated_install_and_emits_path_free_identity(self) -> None:
        result = self.run_verifier()
        self.assertEqual(result.returncode, 0, result.stderr)
        value = json.loads(result.stdout)
        self.assertEqual(
            value,
            {
                "extension_root": "N64LoaderWV",
                "loader_class": {
                    "byte_length": len(self.loader_class),
                    "sha256": hashlib.sha256(self.loader_class).hexdigest(),
                },
                "loader_jar": {
                    "byte_length": len(self.loader_jar),
                    "sha256": hashlib.sha256(self.loader_jar).hexdigest(),
                },
                "schema": "fn64.n64loaderwv-install-verification",
                "schema_version": 1,
            },
        )
        self.assertNotIn(os.fspath(self.root), result.stdout)

    def test_rejects_changed_or_extra_extracted_content(self) -> None:
        cases = (
            lambda: (self.extension / "data/value.txt").write_bytes(b"changed"),
            lambda: (self.extension / "extra.txt").write_bytes(b"extra"),
            lambda: (self.extension / "extra").mkdir(),
        )
        for mutate in cases:
            with self.subTest(mutate=mutate):
                mutate()
                self.assert_rejected("extracted extension")
                self.tearDown()
                self.setUp()

    @unittest.skipIf(not hasattr(os, "symlink"), "symlinks unavailable")
    def test_rejects_symlink_in_extracted_tree(self) -> None:
        (self.extension / "link").symlink_to(self.extension / "data/value.txt")
        self.assert_rejected("contains a symlink")

    def test_rejects_unsafe_duplicate_and_multiple_root_archives(self) -> None:
        cases = (
            [("../escape", b"x")],
            [("N64LoaderWV\\escape", b"x")],
            [("N64LoaderWV/a", b"x"), ("Other/b", b"y")],
        )
        for entries in cases:
            with self.subTest(entries=entries):
                self.write_archive(entries)
                self.assert_rejected("extension ZIP")

        with warnings.catch_warnings():
            warnings.simplefilter("ignore", UserWarning)
            self.write_archive(
                [("N64LoaderWV/a", b"first"), ("N64LoaderWV/a", b"second")]
            )
        self.assert_rejected("duplicate entries")

    def test_rejects_missing_or_duplicate_approved_loader_class(self) -> None:
        no_loader = jar_with(("n64loaderwv/Helper.class", b"helper"))
        self.write_archive(
            [("N64LoaderWV/lib/N64LoaderWV.jar", no_loader)]
        )
        self.extension.joinpath("lib/N64LoaderWV.jar").write_bytes(no_loader)
        self.extension.joinpath("extension.properties").unlink()
        self.extension.joinpath("data/value.txt").unlink()
        self.extension.joinpath("data").rmdir()
        self.assert_rejected("exactly one loader class")

        with warnings.catch_warnings():
            warnings.simplefilter("ignore", UserWarning)
            duplicate = jar_with((LOADER_CLASS, b"one"), (LOADER_CLASS, b"two"))
        self.write_archive(
            [("N64LoaderWV/lib/N64LoaderWV.jar", duplicate)]
        )
        self.extension.joinpath("lib/N64LoaderWV.jar").write_bytes(duplicate)
        self.assert_rejected("duplicate loader classes")

    def test_rejects_competing_jar_anywhere_in_distribution_or_profile(self) -> None:
        competitor = jar_with((LOADER_CLASS, b"other"))
        locations = (
            self.ghidra / "Ghidra/Features/Other/lib/other.jar",
            self.profile / "OtherExtension/lib/other.jar",
        )
        for location in locations:
            with self.subTest(location=location):
                location.parent.mkdir(parents=True)
                location.write_bytes(competitor)
                self.assert_rejected("competing loader class")
                location.unlink()

    def test_rejects_competing_loose_class_anywhere_in_distribution_or_profile(self) -> None:
        locations = (
            self.ghidra / "Ghidra/Features/Other/bin/n64loaderwv/N64LoaderWVLoader.class",
            self.profile / "Other/bin/n64loaderwv/N64LoaderWVLoader.class",
        )
        for location in locations:
            with self.subTest(location=location):
                location.parent.mkdir(parents=True)
                location.write_bytes(b"other")
                self.assert_rejected("competing loose loader class")
                location.unlink()

    @unittest.skipIf(not hasattr(os, "symlink"), "symlinks unavailable")
    def test_rejects_symlink_hiding_in_scanned_classpath(self) -> None:
        target = self.root / "elsewhere"
        target.mkdir()
        (self.ghidra / "linked").symlink_to(target, target_is_directory=True)
        self.assert_rejected("classpath contains a symlink")


if __name__ == "__main__":
    unittest.main()
