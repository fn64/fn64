#!/usr/bin/env python3
"""Focused source-contract tests for Fn64VerifyN64LoaderRuntime.java."""

from __future__ import annotations

from pathlib import Path
import unittest


SOURCE = Path(__file__).with_name("Fn64VerifyN64LoaderRuntime.java")


class VerifyN64LoaderRuntimeSourceTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.source = SOURCE.read_text(encoding="utf-8")

    def test_uses_the_same_exact_loader_resolver_as_headless_options(self) -> None:
        self.assertIn(
            'LoaderService.getLoaderClassByName(LOADER_SIMPLE_NAME)', self.source
        )
        self.assertIn('LOADER_SIMPLE_NAME = "N64LoaderWVLoader"', self.source)
        self.assertIn(
            'LOADER_CLASS_NAME = "n64loaderwv.N64LoaderWVLoader"', self.source
        )
        self.assertIn("wrong N64LoaderWV loader class", self.source)

    def test_code_source_must_be_the_expected_regular_non_symlink_jar(self) -> None:
        for contract in (
            "loaderClass.getProtectionDomain().getCodeSource()",
            "N64LoaderWV class has no code source",
            'url.getProtocol().equals("file")',
            "Files.isSymbolicLink(runtimeJar)",
            "Files.isRegularFile(runtimeJar, LinkOption.NOFOLLOW_LINKS)",
            "runtimeJar.equals(expectedJar)",
            "N64LoaderWV code source is not the expected JAR",
        ):
            with self.subTest(contract=contract):
                self.assertIn(contract, self.source)

    def test_hashes_live_jar_and_live_class_bytes(self) -> None:
        self.assertIn("Files.newInputStream(runtimeJar)", self.source)
        self.assertIn("requireDigest(jarContent.sha256(), expectedJarSha", self.source)
        self.assertIn(
            'getResource("/n64loaderwv/N64LoaderWVLoader.class")', self.source
        )
        self.assertIn("connection instanceof JarURLConnection", self.source)
        self.assertIn("jarConnection.getJarFileURL()", self.source)
        self.assertIn("resourceJar.equals(runtimeJar)", self.source)
        self.assertIn("jarConnection.getInputStream()", self.source)
        self.assertIn(
            "requireDigest(classContent.sha256(), expectedClassSha", self.source
        )

    def test_binds_loader_display_name_to_program_executable_format(self) -> None:
        self.assertIn("loaderClass.getDeclaredConstructor().newInstance()", self.source)
        self.assertIn("String displayName = loader.getName()", self.source)
        self.assertIn("expectedDisplayName.equals(expectedExecutableFormat)", self.source)
        self.assertIn("currentProgram.getExecutableFormat()", self.source)
        self.assertIn("program was not imported with the expected loader", self.source)

    def test_receipt_is_path_free_and_records_runtime_identity(self) -> None:
        for field in (
            'fn64.n64loaderwv-runtime-verification.v1',
            '\\"jar_sha256\\"',
            '\\"class_sha256\\"',
            '\\"class_loader_type\\"',
            '\\"module\\"',
            '\\"package\\"',
            '\\"executable_format\\"',
        ):
            with self.subTest(field=field):
                self.assertIn(field, self.source)
        self.assertNotIn('\\"code_source_path\\"', self.source)
        self.assertNotIn('\\"expected_jar_path\\"', self.source)
        self.assertIn("StandardOpenOption.CREATE_NEW", self.source)

    def test_inputs_are_strict(self) -> None:
        self.assertIn("args.length != 6", self.source)
        self.assertIn("path.isAbsolute()", self.source)
        self.assertIn('value.matches("[0-9a-f]{64}")', self.source)
        self.assertIn("expected JAR must not be a symlink", self.source)
        self.assertIn("expected JAR must be a regular file", self.source)


if __name__ == "__main__":
    unittest.main()
