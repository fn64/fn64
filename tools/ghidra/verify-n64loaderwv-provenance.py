#!/usr/bin/env python3
"""Verify the approved N64LoaderWV checkout and conformance artifact chain."""

from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
from typing import NoReturn


POLICY_SCHEMA = "fn64.n64loaderwv-source-policy"
ARTIFACT_POLICY_SCHEMA = "fn64.n64loaderwv-artifact-policy"
RECEIPT_SCHEMA = "fn64.n64loaderwv-conformance.v2"
EXPECTED_REPOSITORY = "https://github.com/fn64/N64LoaderWV"
MAX_POLICY_BYTES = 4096
MAX_RECEIPT_BYTES = 16384
MAX_EXTENSION_BYTES = 128 * 1024 * 1024
SHA1 = re.compile(r"[0-9a-f]{40}\Z")
SHA256 = re.compile(r"[0-9a-f]{64}\Z")
TOKEN = re.compile(r"[A-Za-z0-9._+\-]{1,128}\Z")
POLICY_FIELDS = {
    "schema",
    "schema_version",
    "repository",
    "approved_commit",
    "approved_tree",
}
ARTIFACT_POLICY_FIELDS = {
    "schema",
    "schema_version",
    "source_policy_sha256",
    "approved_conformance_receipt_sha256",
    "approved_extension_sha256",
}
RECEIPT_FIELDS = {
    "schema",
    "conformance_mode",
    "n64loaderwv_repository",
    "n64loaderwv_policy_sha256",
    "n64loaderwv_commit",
    "n64loaderwv_tree",
    "n64loaderwv_source_archive_sha256",
    "n64loaderwv_extension_sha256",
    "ghidra_version",
    "ghidra_build_sha256",
    "export_script_sha256",
    "rom_sha256",
    "rdram_sha256",
    "bank_sha256",
    "mapping_sha256",
    "configuration_sha256",
    "evidence_sha256",
    "provider_jsonl_sha256",
    "build_memory_guard_sha256",
    "analysis_memory_guard_sha256",
    "gate_memory_guard_sha256",
}
RECEIPT_DIGEST_FIELDS = RECEIPT_FIELDS - {
    "schema",
    "conformance_mode",
    "n64loaderwv_repository",
    "n64loaderwv_commit",
    "n64loaderwv_tree",
    "ghidra_version",
}
ALLOWED_ORIGIN_URLS = {
    "git@github.com:fn64/N64LoaderWV.git",
    "ssh://git@github.com/fn64/N64LoaderWV.git",
    "https://github.com/fn64/N64LoaderWV",
    "https://github.com/fn64/N64LoaderWV.git",
}


def fail(message: str) -> NoReturn:
    raise SystemExit(f"n64loaderwv provenance: {message}")


def read_regular(path_value: str, limit: int, label: str) -> tuple[Path, bytes]:
    path = Path(path_value)
    if not path.is_absolute():
        fail(f"{label} path must be absolute")
    if path.is_symlink() or not path.is_file():
        fail(f"{label} must be a regular non-symlink file")
    size = path.stat().st_size
    if size <= 0 or size > limit:
        fail(f"{label} size is outside 1..={limit} bytes")
    data = path.read_bytes()
    if len(data) != size:
        fail(f"{label} changed while reading")
    return path.resolve(strict=True), data


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def strict_json_object(data: bytes, label: str) -> dict[str, object]:
    try:
        text = data.decode("utf-8")
    except UnicodeDecodeError as error:
        fail(f"{label} is not UTF-8: {error}")

    def pairs(values: list[tuple[str, object]]) -> dict[str, object]:
        result: dict[str, object] = {}
        for key, value in values:
            if key in result:
                fail(f"{label} contains duplicate field {key}")
            result[key] = value
        return result

    try:
        value = json.loads(text, object_pairs_hook=pairs)
    except json.JSONDecodeError as error:
        fail(f"{label} is invalid JSON: {error}")
    if not isinstance(value, dict):
        fail(f"{label} must be a JSON object")
    return value


def exact_fields(value: dict[str, object], expected: set[str], label: str) -> None:
    actual = set(value)
    if actual != expected:
        missing = sorted(expected - actual)
        unknown = sorted(actual - expected)
        fail(f"{label} fields differ: missing={missing} unknown={unknown}")


def load_policy(path_value: str) -> tuple[dict[str, object], str]:
    _, data = read_regular(path_value, MAX_POLICY_BYTES, "source policy")
    value = strict_json_object(data, "source policy")
    exact_fields(value, POLICY_FIELDS, "source policy")
    if value["schema"] != POLICY_SCHEMA or value["schema_version"] != 1:
        fail("source policy schema is unsupported")
    if value["repository"] != EXPECTED_REPOSITORY:
        fail("source policy does not name fn64/N64LoaderWV")
    for field in ("approved_commit", "approved_tree"):
        item = value[field]
        if not isinstance(item, str) or SHA1.fullmatch(item) is None:
            fail(f"source policy {field} must be 40 lowercase hexadecimal digits")
    return value, sha256(data)


def load_artifact_policy(path_value: str) -> dict[str, object]:
    _, data = read_regular(path_value, MAX_POLICY_BYTES, "artifact policy")
    value = strict_json_object(data, "artifact policy")
    exact_fields(value, ARTIFACT_POLICY_FIELDS, "artifact policy")
    if value["schema"] != ARTIFACT_POLICY_SCHEMA or value["schema_version"] != 1:
        fail("artifact policy schema is unsupported")
    for field in (
        "source_policy_sha256",
        "approved_conformance_receipt_sha256",
        "approved_extension_sha256",
    ):
        item = value[field]
        if not isinstance(item, str) or SHA256.fullmatch(item) is None:
            fail(f"artifact policy {field} must be a SHA-256 digest")
    return value


def git(checkout: Path, *arguments: str) -> str:
    try:
        result = subprocess.run(
            ["git", "-C", os.fspath(checkout), *arguments],
            check=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
    except (OSError, subprocess.CalledProcessError) as error:
        fail(f"Git verification failed for {' '.join(arguments)}: {error}")
    return result.stdout.strip()


def verify_checkout(policy_path: str, checkout_value: str, commit: str) -> dict[str, str]:
    policy, policy_sha = load_policy(policy_path)
    checkout = Path(checkout_value)
    if not checkout.is_absolute():
        fail("checkout path must be absolute")
    if checkout.is_symlink() or not checkout.is_dir():
        fail("checkout must be a directory and not a symlink")
    if not (checkout / ".git").exists():
        fail("checkout is not a Git worktree")
    checkout = checkout.resolve(strict=True)

    if commit != policy["approved_commit"]:
        fail("requested commit is not the policy-approved commit")
    resolved = git(checkout, "rev-parse", "--verify", f"{commit}^{{commit}}")
    if resolved != commit:
        fail("requested commit did not resolve to itself")
    tree = git(checkout, "rev-parse", "--verify", f"{commit}^{{tree}}")
    if tree != policy["approved_tree"]:
        fail("requested commit tree does not match source policy")
    origin = git(checkout, "remote", "get-url", "origin")
    if origin not in ALLOWED_ORIGIN_URLS:
        fail("origin does not name github.com/fn64/N64LoaderWV")
    containing = git(
        checkout,
        "for-each-ref",
        "--format=%(refname)",
        "--contains",
        commit,
        "refs/remotes/origin/",
    ).splitlines()
    if not any(reference.startswith("refs/remotes/origin/") for reference in containing):
        fail("approved commit is not contained by an origin-tracking ref")
    return {
        "commit": commit,
        "policy_sha256": policy_sha,
        "repository": EXPECTED_REPOSITORY,
        "tree": tree,
    }


def parse_receipt(data: bytes) -> dict[str, str]:
    try:
        text = data.decode("utf-8")
    except UnicodeDecodeError as error:
        fail(f"conformance receipt is not UTF-8: {error}")
    result: dict[str, str] = {}
    for line_number, line in enumerate(text.splitlines(), start=1):
        if not line or "=" not in line:
            fail(f"conformance receipt line {line_number} is not key=value")
        key, value = line.split("=", 1)
        if not TOKEN.fullmatch(key):
            fail(f"conformance receipt line {line_number} has an invalid key")
        if key in result:
            fail(f"conformance receipt contains duplicate field {key}")
        if not value or any(ord(character) < 0x20 or ord(character) == 0x7F for character in value):
            fail(f"conformance receipt field {key} has an invalid value")
        result[key] = value
    exact_fields(result, RECEIPT_FIELDS, "conformance receipt")
    return result


def verify_receipt_integrity(receipt_value: str, extension_value: str) -> tuple[dict[str, str], str, str]:
    _, receipt_data = read_regular(receipt_value, MAX_RECEIPT_BYTES, "conformance receipt")
    _, extension_data = read_regular(extension_value, MAX_EXTENSION_BYTES, "extension ZIP")
    receipt = parse_receipt(receipt_data)
    if receipt["schema"] != RECEIPT_SCHEMA:
        fail("conformance receipt schema is unsupported")
    if receipt["conformance_mode"] != "approved":
        fail("conformance receipt is not approved")
    if receipt["n64loaderwv_repository"] != EXPECTED_REPOSITORY:
        fail("conformance receipt does not name fn64/N64LoaderWV")
    if SHA1.fullmatch(receipt["n64loaderwv_commit"]) is None:
        fail("conformance receipt n64loaderwv_commit is invalid")
    if SHA1.fullmatch(receipt["n64loaderwv_tree"]) is None:
        fail("conformance receipt n64loaderwv_tree is invalid")
    if TOKEN.fullmatch(receipt["ghidra_version"]) is None:
        fail("conformance receipt ghidra_version is invalid")
    for field in RECEIPT_DIGEST_FIELDS:
        if SHA256.fullmatch(receipt[field]) is None:
            fail(f"conformance receipt {field} is not a SHA-256 digest")
    extension_sha = sha256(extension_data)
    if extension_sha != receipt["n64loaderwv_extension_sha256"]:
        fail("extension ZIP digest does not match conformance receipt")
    return receipt, sha256(receipt_data), extension_sha


def verify_candidate_integrity(receipt_value: str, extension_value: str) -> dict[str, str]:
    receipt, receipt_sha, extension_sha = verify_receipt_integrity(receipt_value, extension_value)
    return {
        "commit": receipt["n64loaderwv_commit"],
        "conformance_receipt_sha256": receipt_sha,
        "extension_sha256": extension_sha,
        "repository": EXPECTED_REPOSITORY,
        "source_archive_sha256": receipt["n64loaderwv_source_archive_sha256"],
        "tree": receipt["n64loaderwv_tree"],
        "verification": "candidate_integrity_only",
    }


def verify_artifact(
    artifact_policy_path: str,
    source_policy_path: str,
    receipt_value: str,
    extension_value: str,
) -> dict[str, str]:
    artifact_policy = load_artifact_policy(artifact_policy_path)
    policy, policy_sha = load_policy(source_policy_path)
    if artifact_policy["source_policy_sha256"] != policy_sha:
        fail("artifact policy does not pin this source policy")
    receipt, receipt_sha, extension_sha = verify_receipt_integrity(receipt_value, extension_value)
    if receipt_sha != artifact_policy["approved_conformance_receipt_sha256"]:
        fail("conformance receipt digest is not artifact-policy approved")
    if extension_sha != artifact_policy["approved_extension_sha256"]:
        fail("extension ZIP digest is not artifact-policy approved")
    expected = {
        "n64loaderwv_repository": EXPECTED_REPOSITORY,
        "n64loaderwv_policy_sha256": policy_sha,
        "n64loaderwv_commit": str(policy["approved_commit"]),
        "n64loaderwv_tree": str(policy["approved_tree"]),
    }
    for field, value in expected.items():
        if receipt[field] != value:
            fail(f"conformance receipt {field} does not match source policy")
    return {
        "commit": receipt["n64loaderwv_commit"],
        "conformance_receipt_sha256": receipt_sha,
        "extension_sha256": extension_sha,
        "policy_sha256": policy_sha,
        "repository": EXPECTED_REPOSITORY,
        "source_archive_sha256": receipt["n64loaderwv_source_archive_sha256"],
        "tree": receipt["n64loaderwv_tree"],
    }


def usage() -> NoReturn:
    fail(
        "usage: verify-n64loaderwv-provenance.py "
        "(checkout SOURCE_POLICY CHECKOUT COMMIT | "
        "artifact ARTIFACT_POLICY SOURCE_POLICY RECEIPT EXTENSION_ZIP | "
        "candidate-integrity RECEIPT EXTENSION_ZIP)"
    )


def main(arguments: list[str]) -> None:
    if len(arguments) == 4 and arguments[1] == "candidate-integrity":
        result = verify_candidate_integrity(arguments[2], arguments[3])
    elif len(arguments) == 5 and arguments[1] == "checkout":
        result = verify_checkout(arguments[2], arguments[3], arguments[4])
    elif len(arguments) == 6 and arguments[1] == "artifact":
        result = verify_artifact(
            arguments[2], arguments[3], arguments[4], arguments[5]
        )
    else:
        usage()
    print(json.dumps(result, sort_keys=True, separators=(",", ":")))


if __name__ == "__main__":
    main(sys.argv)
