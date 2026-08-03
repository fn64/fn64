#!/usr/bin/env python3

import hashlib
import importlib.util
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

SCRIPT = Path(__file__).with_name("manifest-ghidra-distribution.py")


def load_script_module():
    spec = importlib.util.spec_from_file_location("fn64_ghidra_manifest", SCRIPT)
    if spec is None or spec.loader is None:
        raise RuntimeError("could not load manifest helper")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class DistributionManifestTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="fn64-ghidra-manifest-")
        self.root = Path(self.temporary.name).resolve()
        self.distribution = self.root / "distribution"
        self.cache = self.root / "cache"
        self.out = self.root / "out"
        for directory in (self.distribution, self.cache, self.out):
            directory.mkdir(mode=0o700)
        (self.distribution / "z").mkdir()
        (self.distribution / "z" / "last.jar").write_bytes(b"last")
        (self.distribution / "first.txt").write_bytes(b"first")

    def tearDown(self):
        self.temporary.cleanup()

    def run_script(self, *arguments, success=True):
        result = subprocess.run(
            [sys.executable, SCRIPT, *map(str, arguments)],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            check=False,
        )
        if success and result.returncode != 0:
            self.fail(result.stderr)
        if not success and result.returncode == 0:
            self.fail("command unexpectedly succeeded")
        return result

    def test_sorted_path_free_manifest_and_content_addressed_cache(self):
        first = self.out / "first.json"
        second = self.out / "second.json"
        self.run_script("scan", self.distribution, self.cache, first)
        self.run_script("scan", self.distribution, self.cache, second)
        self.assertEqual(first.read_bytes(), second.read_bytes())
        value = json.loads(first.read_bytes())
        self.assertEqual(value["schema"], "fn64.ghidra-distribution-manifest")
        self.assertEqual(
            [entry["path"] for entry in value["files"]],
            ["first.txt", "z/last.jar"],
        )
        self.assertNotIn(str(self.root), first.read_text())
        digest = hashlib.sha256(first.read_bytes()).hexdigest()
        cached = self.cache / f"{digest}.json"
        self.assertTrue(cached.is_file())
        self.assertEqual(first.stat().st_ino, cached.stat().st_ino)
        self.assertEqual(second.stat().st_ino, cached.stat().st_ino)
        self.run_script("verify", self.distribution, first)

    def test_same_length_mutation_with_restored_mtime_is_detected(self):
        manifest = self.out / "manifest.json"
        self.run_script("scan", self.distribution, self.cache, manifest)
        target = self.distribution / "first.txt"
        before = target.stat()
        target.write_bytes(b"other")
        os.utime(target, ns=(before.st_atime_ns, before.st_mtime_ns))
        result = self.run_script("verify", self.distribution, manifest, success=False)
        self.assertIn("does not match", result.stderr)

    def test_walk_error_fails_closed_instead_of_skipping_subtree(self):
        module = load_script_module()

        def failing_walk(*_args, **kwargs):
            kwargs["onerror"](PermissionError("denied subtree"))
            return iter(())

        with mock.patch.object(module.os, "walk", side_effect=failing_walk):
            with self.assertRaisesRegex(
                SystemExit, "cannot traverse distribution: denied subtree"
            ):
                module.inventory(self.distribution)

    def test_symlinks_and_existing_outputs_are_rejected(self):
        link = self.distribution / "link"
        link.symlink_to(self.distribution / "first.txt")
        result = self.run_script(
            "scan", self.distribution, self.cache, self.out / "manifest.json", success=False
        )
        self.assertIn("non-symlink", result.stderr)
        link.unlink()
        output = self.out / "manifest.json"
        output.write_text("winner")
        result = self.run_script(
            "scan", self.distribution, self.cache, output, success=False
        )
        self.assertIn("refusing to overwrite", result.stderr)
        self.assertEqual(output.read_text(), "winner")

    def test_symlinked_cache_entry_is_rejected(self):
        manifest = self.out / "reference.json"
        self.run_script("scan", self.distribution, self.cache, manifest)
        digest = hashlib.sha256(manifest.read_bytes()).hexdigest()
        cached = self.cache / f"{digest}.json"
        cached.unlink()
        cached.symlink_to(manifest)
        result = self.run_script(
            "scan", self.distribution, self.cache, self.out / "second.json", success=False
        )
        self.assertIn("bounded regular non-symlink", result.stderr)


if __name__ == "__main__":
    unittest.main()
