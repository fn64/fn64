#!/usr/bin/env python3
"""Focused source contract for the first-contact loader identity gates."""

from pathlib import Path
import unittest


SOURCE = Path(__file__).with_name("run-n64loaderwv-first-contact.sh")


class N64LoaderWVFirstContactSourceTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.source = SOURCE.read_text(encoding="utf-8")

    def test_uses_shared_install_verifier_and_parses_live_identities(self) -> None:
        for contract in (
            'install_verifier="$repo/tools/ghidra/verify-n64loaderwv-install.py"',
            '"$install_verifier" "$installed_zip" "$extension_dir" "$ghidra_install"',
            '"$settings_user" > "$install_verification"',
            "loader_jar_sha=$(install_field loader_jar.sha256)",
            "loader_class_sha=$(install_field loader_class.sha256)",
        ):
            with self.subTest(contract=contract):
                self.assertIn(contract, self.source)

    def test_runtime_verifier_binds_loaded_jar_class_and_program_format(self) -> None:
        for contract in (
            "-postScript Fn64VerifyN64LoaderRuntime.java",
            '"$runtime_verification" "$expected_loader_jar" "$loader_jar_sha"',
            '"$loader_class_sha" "N64 Loader by Warranty Voider"',
            '"N64 Loader by Warranty Voider"',
            "runtime_verification_sha256=%s\\n",
        ):
            with self.subTest(contract=contract):
                self.assertIn(contract, self.source)

    def test_rejects_ghidra_shadow_diagnostic(self) -> None:
        self.assertIn(
            'grep -q "Ignoring class \'n64loaderwv.N64LoaderWVLoader\'"',
            self.source,
        )
        self.assertIn(
            "another N64LoaderWV installation shadowed the isolated extension",
            self.source,
        )


if __name__ == "__main__":
    unittest.main()
